//! Operator-facing execution lifecycle: start / stop / halt / arm / shutdown.
//!
//! This module contains the `AppState` impl block for the five primary
//! operator-visible lifecycle transitions.  All private helpers (db_pool,
//! reap_finished_execution_loop, take_execution_loop_for_control, etc.) remain
//! in `state.rs`; they are accessible here because Rust allows child modules to
//! read items that are private to a parent module.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use chrono::Utc;

use crate::artifact_intake::{
    evaluate_artifact_deployability, evaluate_artifact_intake_guarded, ArtifactIntakeOutcome,
    ENV_ARTIFACT_PATH,
};
use crate::capital_policy::{
    evaluate_capital_policy_from_env, evaluate_deployment_economics_from_env, CapitalPolicyOutcome,
    DeploymentEconomicsOutcome,
};
use crate::parity_evidence::{evaluate_parity_evidence_from_env, ParityEvidenceOutcome};

use super::loop_runner::spawn_execution_loop;
use super::types::ExecutionLoopCommand;
use super::{
    reconcile_broker_snapshot_from_schema, reconcile_local_snapshot_from_runtime_with_sides,
    spawn_reconcile_tick, uptime_secs,
};
use super::{
    AcceptedArtifactProvenance, BrokerKind, DeploymentMode, OperatorAuthMode,
    RuntimeLifecycleError, StatusSnapshot,
};
use super::{AppState, DAEMON_ENGINE_ID, RECONCILE_TICK_INTERVAL};

use mqk_runtime::native_strategy::{build_daemon_plugin_registry, NativeStrategyBootstrap};

impl AppState {
    pub async fn start_execution_runtime(
        self: &Arc<Self>,
    ) -> Result<StatusSnapshot, RuntimeLifecycleError> {
        let _op = self.lifecycle_op.lock().await;
        self.reap_finished_execution_loop().await?;

        if !self.deployment_readiness().start_allowed {
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.start_refused.deployment_mode_unproven",
                "deployment_mode",
                self.deployment_readiness()
                    .blocker
                    .clone()
                    .unwrap_or_else(|| "deployment mode is not start-ready".to_string()),
            ));
        }

        if self.integrity.read().await.is_execution_blocked() {
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.control_refusal.integrity_disarmed",
                "integrity_armed",
                "GATE_REFUSED: integrity disarmed or halted; arm integrity first",
            ));
        }

        if self.deployment_mode() == DeploymentMode::LiveCapital
            && !matches!(self.operator_auth, OperatorAuthMode::TokenRequired(_))
        {
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.start_refused.capital_requires_operator_token",
                "operator_auth",
                "live-capital mode requires a real operator token; \
                 dev-no-token and missing-token modes are not permitted for capital execution",
            ));
        }

        if let Some(run_id) = self.active_owned_run_id().await {
            return Err(RuntimeLifecycleError::conflict(
                "runtime.control_refusal.already_owned",
                format!("runtime already active under local ownership: {run_id}"),
            ));
        }

        // BRK-00R-04: paper+alpaca WS continuity start gate.
        //
        // The Alpaca paper path requires proven WS continuity before runtime start.
        // ColdStartUnproven and GapDetected are not start-safe: no live WS cursor
        // has been established, so event delivery ordering cannot be trusted.
        //
        // Placed before db_pool() so the check is:
        //   - at the earliest honest enforcement point (continuity state is in-memory)
        //   - in-process testable without a database
        //   - before any DB resources or runtime lease are acquired
        //
        // Full WS transport implementation (subscribe/reconnect/cursor establishment)
        // remains open; this patch only moves the failure forward from first tick.
        if self.deployment_mode() == DeploymentMode::Paper
            && self.runtime_selection.broker_kind == Some(BrokerKind::Alpaca)
        {
            let continuity = self.alpaca_ws_continuity().await;
            if !continuity.is_continuity_proven() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.paper_alpaca_ws_continuity_unproven",
                    "alpaca_ws_continuity",
                    format!(
                        "paper+alpaca requires proven Alpaca WS continuity before starting; \
                         current state: '{}' (WS_CONTINUITY_UNPROVEN) — the WS transport \
                         must establish a live cursor before paper+alpaca can proceed; \
                         full WS transport work remains open",
                        continuity.as_status_str()
                    ),
                ));
            }
        }

        // BRK-09R: Reconcile truth start gate for broker-backed paper path.
        //
        // If the persisted reconcile status is "dirty" or "stale" — meaning the
        // prior session ended with a known broker/local drift condition — refuse
        // start until the operator has investigated and the reconcile state is
        // cleared to "ok" (or the DB row is absent, meaning no prior evidence).
        //
        // "unknown" is the initial in-memory state at fresh boot (no prior run),
        // and is allowed through: it carries no evidence of prior drift.
        //
        // Gate ordering: fires after the WS continuity gate so WS issues are
        // surfaced first.  A dirty reconcile AND a non-live WS yields the WS gate
        // as the named blocker; reconcile is only surfaced when WS is clean.
        //
        // current_reconcile_snapshot() reads from DB when available, falling back
        // to in-memory; it does not require db_pool() to be non-None.
        if self.deployment_mode() == DeploymentMode::Paper
            && self.runtime_selection.broker_kind == Some(BrokerKind::Alpaca)
        {
            let reconcile = self.current_reconcile_snapshot().await;
            if matches!(reconcile.status.as_str(), "dirty" | "stale") {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.reconcile_dirty",
                    "reconcile_truth",
                    format!(
                        "paper+alpaca cannot start with dirty or stale reconcile truth; \
                         current reconcile status: '{}'; the prior session's broker/local \
                         drift must be investigated and the reconcile state must be cleared \
                         before restarting; reconcile note: {}",
                        reconcile.status,
                        reconcile.note.as_deref().unwrap_or("none"),
                    ),
                ));
            }
        }

        // Live-capital WS continuity gate.
        //
        // Placed here — before db_pool() — so it is:
        //   - in-process testable without a database or real broker credentials
        //   - before any DB resources or run rows are acquired (prevents dangling
        //     "Created" run rows on a continuity-refused start)
        //   - symmetric with the Paper+Alpaca continuity gate above
        //
        // Previous position (after build_execution_orchestrator) required
        // orchestrator.release_runtime_leadership() on failure and could leave
        // a "Created" run row in the DB if the check failed after insert_run.
        if self.deployment_mode() == DeploymentMode::LiveCapital {
            let continuity = self.alpaca_ws_continuity().await;
            if !continuity.is_continuity_proven() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.capital_ws_continuity_unproven",
                    "alpaca_ws_continuity",
                    format!(
                        "live-capital requires proven Alpaca WS continuity before starting; \
                         current continuity state: '{}' — \
                         run in live-shadow mode to establish a proven cursor, \
                         then transition to capital",
                        continuity.as_status_str()
                    ),
                ));
            }
        }

        // TV-01 / TV-02C: Evaluate artifact intake exactly once.
        //
        // Hoisted here so the same evaluation result is used for:
        //   - TV-02C deployability gate (below)
        //   - TV-01C provenance capture at successful run start (further below)
        //
        // Evaluating twice would create a TOCTOU window: a file swap or env-var
        // mutation between the gate check and the provenance capture could let
        // a different artifact identity pass the gate while a different one is
        // recorded as the run's provenance.  Single evaluation closes that gap.
        let artifact_intake = evaluate_artifact_intake_guarded();

        // TV-02C: Artifact deployability gate.
        //
        // If MQK_ARTIFACT_PATH is configured and intake is Accepted, the artifact
        // must also pass the deployability gate (deployability_gate.json written by
        // the Python TV-02 pipeline) before runtime start is allowed.
        //
        // Contract:
        //   NotConfigured            → no artifact configured; gate not applicable; pass through.
        //   Accepted + Deployable    → minimum criteria met; pass through.
        //   Accepted + not Deployable→ fail-closed: block start with explicit reason.
        //   Invalid / Unavailable   → artifact configured but intake failed; fail-closed.
        //
        // Placed before db_pool() so it is:
        //   - in-process testable without a database
        //   - before any DB resources or run rows are acquired (no dangling rows on refusal)
        {
            match &artifact_intake {
                ArtifactIntakeOutcome::NotConfigured => {
                    // No artifact configured — deployability gate not applicable.
                }
                ArtifactIntakeOutcome::Accepted { artifact_id, .. } => {
                    let raw = std::env::var(ENV_ARTIFACT_PATH).unwrap_or_default();
                    let manifest_path = std::path::PathBuf::from(raw.trim());
                    let deployability =
                        evaluate_artifact_deployability(Some(&manifest_path), artifact_id);
                    if !deployability.is_deployable() {
                        return Err(RuntimeLifecycleError::forbidden(
                            "runtime.start_refused.artifact_not_deployable",
                            "artifact_deployability",
                            format!(
                                "configured artifact failed the deployability gate \
                                 (truth_state='{}'): artifact_id='{}' was accepted for intake \
                                 but did not pass minimum deployability/tradability criteria; \
                                 run the TV-02 Python gate on this artifact to produce a \
                                 deployability_gate.json that passes all checks",
                                deployability.truth_state(),
                                artifact_id,
                            ),
                        ));
                    }
                }
                ArtifactIntakeOutcome::Invalid { reason } => {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.artifact_intake_invalid",
                        "artifact_intake",
                        format!(
                            "artifact intake failed; runtime cannot proceed with a configured \
                             but invalid artifact: {reason}"
                        ),
                    ));
                }
                ArtifactIntakeOutcome::Unavailable { reason } => {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.artifact_intake_unavailable",
                        "artifact_intake",
                        format!(
                            "artifact intake evaluator failed; runtime cannot proceed when \
                             artifact state is unknown: {reason}"
                        ),
                    ));
                }
            }
        }

        // TV-03C: Parity evidence gate.
        //
        // If MQK_ARTIFACT_PATH is configured, parity evidence for the artifact
        // must exist in the same directory and be structurally valid before
        // runtime start is allowed.
        //
        // Contract:
        //   NotConfigured   → no artifact path configured; gate not applicable; pass through.
        //   Present { .. }  → parity_evidence.json readable and valid; pass through.
        //   Absent          → configured artifact has no parity evidence; fail-closed.
        //   Invalid { .. }  → parity_evidence.json exists but is invalid; fail-closed.
        //   Unavailable { .. } → evaluator failed; fail-closed.
        //
        // Placed after TV-02C (artifact deployability) and before TV-04A (capital policy)
        // so the evidence chain is verified before capital authorization runs.
        // Both TV-02C and TV-03C read MQK_ARTIFACT_PATH; absent path → NotConfigured on both.
        //
        // Cross-validation: when both intake and parity evidence are resolved, the
        // artifact_id embedded in parity_evidence.json must match the accepted intake
        // artifact_id.  This mirrors the TV-02C deployability gate cross-validation and
        // closes the artifact-associated evidence chain: parity evidence produced for a
        // different artifact must not satisfy this gate.  `artifact_intake` is the same
        // evaluation result used for TV-02C above (TOCTOU-safe, evaluated once).
        {
            let parity = evaluate_parity_evidence_from_env();
            // Artifact identity cross-validation: Present evidence for a different
            // artifact is not evidence for this artifact.
            if let (
                ArtifactIntakeOutcome::Accepted {
                    artifact_id: ref accepted_id,
                    ..
                },
                ParityEvidenceOutcome::Present {
                    artifact_id: ref parity_id,
                    ..
                },
            ) = (&artifact_intake, &parity)
            {
                if parity_id != accepted_id {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.parity_evidence_artifact_mismatch",
                        "parity_evidence",
                        format!(
                            "parity evidence artifact_id '{}' does not match the accepted \
                             intake artifact_id '{}'; the parity_evidence.json in the artifact \
                             directory was not produced for the configured artifact — re-run the \
                             TV-03 pipeline against the correct artifact",
                            parity_id, accepted_id
                        ),
                    ));
                }
            }
            if !parity.is_start_safe() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.parity_evidence_not_present",
                    "parity_evidence",
                    format!(
                        "parity evidence gate failed \
                         (truth_state='{}'): {}",
                        parity.truth_state(),
                        match &parity {
                            ParityEvidenceOutcome::Absent => {
                                "parity_evidence.json is absent in the artifact directory; \
                                 run the Python TV-03 pipeline to produce parity evidence \
                                 before starting the runtime"
                                    .to_string()
                            }
                            ParityEvidenceOutcome::Invalid { reason } => {
                                format!("parity_evidence.json is structurally invalid: {reason}")
                            }
                            ParityEvidenceOutcome::Unavailable { reason } => {
                                format!("parity evidence evaluator failed: {reason}")
                            }
                            _ => "parity evidence evaluation failed".to_string(),
                        }
                    ),
                ));
            }
        }

        // TV-04F: Live-capital requires an explicit capital allocation policy.
        //
        // Paper and LiveShadow modes are permissive: absent policy →
        // NotConfigured → gate not applicable; callers pass through.  This is
        // correct for simulation modes where capital policy enforcement is
        // optional at the operator's discretion.
        //
        // LiveCapital is semantically distinct: real capital requires an
        // explicit, operator-configured capital allocation policy before any
        // live-capital execution is authorized.  NotConfigured in live-capital
        // mode is fail-closed — the operator must explicitly configure and
        // enable a policy.  This prevents silent conflation of paper-safe
        // "no policy = no enforcement" with live-capital authorization.
        //
        // Gate ordering: placed after TV-03C (parity evidence) and before
        // TV-04A (policy validity check).  TV-04A then validates the policy
        // is enabled and structurally correct once TV-04F confirms it exists.
        if self.deployment_mode() == DeploymentMode::LiveCapital {
            let policy = evaluate_capital_policy_from_env();
            if matches!(policy, CapitalPolicyOutcome::NotConfigured) {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.live_capital_requires_capital_policy",
                    "live_capital_policy_required",
                    "live-capital mode requires an explicit capital allocation policy; \
                     set MQK_CAPITAL_POLICY_PATH to a valid capital_allocation_policy.json \
                     before starting live-capital execution; paper and live-shadow modes \
                     do not require a policy — this gate is live-capital-only and enforces \
                     the semantic distinction between paper safety and live-capital authorization",
                ));
            }
        }

        // TV-04A: Capital allocation policy gate.
        //
        // If MQK_CAPITAL_POLICY_PATH is configured, the policy file must be
        // valid and `enabled = true` before runtime start is allowed.
        //
        // Contract:
        //   NotConfigured → no policy configured; gate not applicable; pass through.
        //   Authorized    → policy valid and enabled; pass through.
        //   Denied        → policy present but enabled=false; fail-closed.
        //   PolicyInvalid → policy configured but structurally invalid; fail-closed.
        //   Unavailable   → reserved; fail-closed.
        //
        // Placed before db_pool() so the check is:
        //   - in-process testable without a database
        //   - before any DB resources or run rows are acquired (no dangling rows)
        //   - ordered after TV-02C (artifact deployability) so artifact refusals
        //     are surfaced before capital policy refusals
        {
            let policy = evaluate_capital_policy_from_env();
            if !policy.is_start_safe() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.capital_policy_not_authorized",
                    "capital_allocation_policy",
                    format!(
                        "capital allocation policy gate failed \
                         (truth_state='{}'): {}",
                        policy.truth_state(),
                        match &policy {
                            CapitalPolicyOutcome::Denied { reason } => reason.clone(),
                            CapitalPolicyOutcome::PolicyInvalid { reason } => {
                                format!("policy file is invalid: {reason}")
                            }
                            CapitalPolicyOutcome::Unavailable { reason } => {
                                format!("policy evaluator unavailable: {reason}")
                            }
                            _ => "capital policy evaluation failed".to_string(),
                        }
                    ),
                ));
            }
        }

        // TV-04D: Deployment economics gate.
        //
        // An enabled capital policy must carry a valid `max_portfolio_notional_usd`
        // before runtime start is allowed.  This gate is independent of TV-04A:
        // TV-04A checks whether the policy is enabled; TV-04D checks whether the
        // enabled policy specifies deployment economics bounds.
        //
        // Contract:
        //   NotConfigured      → no policy configured; gate not applicable; pass through.
        //   PolicyDisabled     → enabled=false; TV-04A already blocked; pass through.
        //   EconomicsSpecified → policy enabled + valid portfolio cap; pass through.
        //   EconomicsNotSpecified → policy enabled but no economics bound; fail-closed.
        //   PolicyInvalid      → policy configured but structurally invalid; fail-closed.
        //   Unavailable        → reserved; fail-closed.
        //
        // Placed immediately after TV-04A so that capital policy authorization
        // is confirmed before the economics bound is checked.  Placed before
        // db_pool() so the check is in-process testable without a database.
        {
            let economics = evaluate_deployment_economics_from_env();
            if !economics.is_start_safe() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.deployment_economics_not_specified",
                    "deployment_economics",
                    format!(
                        "deployment economics gate failed \
                         (truth_state='{}'): {}",
                        economics.truth_state(),
                        match &economics {
                            DeploymentEconomicsOutcome::EconomicsNotSpecified { reason } => {
                                reason.clone()
                            }
                            DeploymentEconomicsOutcome::PolicyInvalid { reason } => {
                                format!("economics policy file is invalid: {reason}")
                            }
                            DeploymentEconomicsOutcome::Unavailable { reason } => {
                                format!("economics evaluator unavailable: {reason}")
                            }
                            _ => "deployment economics evaluation failed".to_string(),
                        }
                    ),
                ));
            }
        }

        // B1A: Native strategy bootstrap gate.
        //
        // Evaluate the native strategy bootstrap from fleet truth (MQK_STRATEGY_IDS)
        // and the daemon plugin registry before acquiring any DB resources.
        //
        // Contract:
        //   Dormant (fleet absent/empty) → pass through.
        //   Active (fleet entry + registry match) → pass through; bootstrap stored.
        //   Failed (fleet entry present, not in registry) → fail-closed.
        //
        // Placed before db_pool() so it is:
        //   - in-process testable without a database
        //   - before any DB resources or run rows are acquired (no dangling rows)
        //   - ordered after all deployment/capital/policy gates (last pre-DB gate)
        //
        // The bootstrap is kept as a local binding and stored in AppState only
        // after a fully successful run start so the field is never left populated
        // by a failed start attempt.
        let native_strategy_bootstrap = {
            let fleet_ids = self.strategy_fleet_snapshot().await.map(|entries| {
                entries
                    .into_iter()
                    .map(|e| e.strategy_id)
                    .collect::<Vec<_>>()
            });
            let registry = build_daemon_plugin_registry();
            let bootstrap = NativeStrategyBootstrap::bootstrap(fleet_ids.as_deref(), &registry);
            if bootstrap.is_failed() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.native_strategy_bootstrap_failed",
                    "native_strategy_bootstrap",
                    format!(
                        "native strategy bootstrap failed (truth_state='{}'): {}; \
                         ensure the strategy named in MQK_STRATEGY_IDS is registered \
                         in the daemon plugin registry before starting; \
                         operators must not set MQK_STRATEGY_IDS until the target \
                         strategy engine is wired into the registry",
                        bootstrap.truth_state(),
                        bootstrap.failure_reason().unwrap_or("unknown"),
                    ),
                ));
            }
            bootstrap
        };

        let db = self.db_pool()?;

        // B2A: DB strategy registry gate.
        //
        // When a native strategy is Active (plugin bootstrap passed), the strategy
        // must also be present AND enabled in the durable sys_strategy_registry.
        // This is the final activation authority: plugin presence is necessary but
        // not sufficient — registry truth is authoritative.
        //
        // Contract:
        //   Dormant bootstrap    → skip (no fleet configured; allowed).
        //   Active + enabled     → pass through.
        //   Active + disabled    → fail-closed (403, gate=strategy_registry).
        //   Active + missing     → fail-closed (403, gate=strategy_registry).
        //   Active + DB error    → fail-closed (503, gate=strategy_registry).
        //
        // Placed immediately after db_pool() so the gate runs once, before any
        // run rows are created or leadership is acquired.
        if let Some(strategy_id) = native_strategy_bootstrap.active_strategy_id() {
            match mqk_db::fetch_strategy_registry_entry(&db, strategy_id).await {
                Ok(Some(record)) if record.enabled => {
                    // Registered and enabled — pass through.
                }
                Ok(Some(_record)) => {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.strategy_registry_disabled",
                        "strategy_registry",
                        format!(
                            "native strategy '{strategy_id}' is registered but disabled \
                             in the strategy registry; enable the strategy in \
                             sys_strategy_registry before starting",
                        ),
                    ));
                }
                Ok(None) => {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.strategy_registry_missing",
                        "strategy_registry",
                        format!(
                            "native strategy '{strategy_id}' is not registered in \
                             the strategy registry; insert an enabled row in \
                             sys_strategy_registry before starting",
                        ),
                    ));
                }
                Err(err) => {
                    return Err(RuntimeLifecycleError::internal(
                        "start strategy_registry lookup failed",
                        err,
                    ));
                }
            }
        }

        if let Some(active) = mqk_db::fetch_active_run_for_engine(
            &db,
            DAEMON_ENGINE_ID,
            self.deployment_mode().as_db_mode(),
        )
        .await
        .map_err(|err| RuntimeLifecycleError::internal("start active-run lookup failed", err))?
        {
            return Err(RuntimeLifecycleError::conflict(
                "runtime.truth_mismatch.durable_active_without_local_owner",
                format!(
                    "durable active run exists without local ownership: {}",
                    active.run_id
                ),
            ));
        }

        let latest = mqk_db::fetch_latest_run_for_engine(
            &db,
            DAEMON_ENGINE_ID,
            self.deployment_mode().as_db_mode(),
        )
        .await
        .map_err(|err| RuntimeLifecycleError::internal("start latest-run lookup failed", err))?;

        let run_id = match latest.as_ref() {
            Some(run) => match run.status {
                mqk_db::RunStatus::Created | mqk_db::RunStatus::Stopped => run.run_id,
                mqk_db::RunStatus::Halted => {
                    return Err(RuntimeLifecycleError::conflict(
                        "runtime.start_refused.halted_lifecycle",
                        format!(
                            "durable run {} is halted; operator must clear the halted lifecycle before starting again",
                            run.run_id
                        ),
                    ))
                }
                mqk_db::RunStatus::Armed | mqk_db::RunStatus::Running => {
                    return Err(RuntimeLifecycleError::conflict(
                        "runtime.start_refused.durable_run_active",
                        format!("durable run {} is already active", run.run_id),
                    ))
                }
            },
            None => {
                let run_id = self.next_daemon_run_id(&db).await?;
                mqk_db::insert_run(
                    &db,
                    &mqk_db::NewRun {
                        run_id,
                        engine_id: DAEMON_ENGINE_ID.to_string(),
                        mode: self.deployment_mode().as_db_mode().to_string(),
                        started_at_utc: Utc::now(),
                        git_hash: "UNKNOWN".to_string(),
                        config_hash: self.run_config_hash().to_string(),
                        config_json: serde_json::json!({
                            "runtime": "mqk-daemon",
                            "adapter": self.adapter_id(),
                            "mode": self.deployment_mode().as_db_mode(),
                        }),
                        host_fingerprint: self.node_id.clone(),
                    },
                )
                .await
                .map_err(|err| RuntimeLifecycleError::internal("start insert_run failed", err))?;
                run_id
            }
        };

        let mut orchestrator = self
            .build_execution_orchestrator(db.clone(), run_id)
            .await?;

        if let Err(err) = mqk_db::arm_run(&db, run_id).await {
            if let Err(rel_err) = orchestrator.release_runtime_leadership().await {
                tracing::warn!("runtime_lease_release_failed_on_arm_rollback error={rel_err}");
            }
            return Err(RuntimeLifecycleError::internal("start arm_run failed", err));
        }
        if let Err(err) = mqk_db::begin_run(&db, run_id).await {
            if let Err(rel_err) = orchestrator.release_runtime_leadership().await {
                tracing::warn!("runtime_lease_release_failed_on_begin_rollback error={rel_err}");
            }
            return Err(RuntimeLifecycleError::internal(
                "start begin_run failed",
                err,
            ));
        }
        if let Err(err) = mqk_db::heartbeat_run(&db, run_id, Utc::now()).await {
            if let Err(rel_err) = orchestrator.release_runtime_leadership().await {
                tracing::warn!("runtime_lease_release_failed_on_heartbeat_rollback error={rel_err}");
            }
            return Err(RuntimeLifecycleError::internal(
                "start initial heartbeat failed",
                err,
            ));
        }
        if let Err(err) = orchestrator.tick().await {
            let message = err.to_string();
            if let Err(rel_err) = orchestrator.release_runtime_leadership().await {
                tracing::warn!("runtime_lease_release_failed_on_tick_rollback error={rel_err}");
            }
            if message.contains("RUNTIME_LEASE") {
                return Err(RuntimeLifecycleError::conflict(
                    "runtime.start_refused.service_unavailable",
                    format!("runtime leader lease unavailable: {message}"),
                ));
            }
            return Err(RuntimeLifecycleError::internal(
                "start initial tick failed",
                err,
            ));
        }

        if let Ok(initial_snapshot) = orchestrator.snapshot().await {
            *self.execution_snapshot.write().await = Some(initial_snapshot);
        }

        // PT-AUTO-02: reset per-run signal intake counter at each new start so
        // the bound applies per execution run, not per daemon process lifetime.
        self.day_signal_count.store(0, Ordering::SeqCst);

        // TV-01C: capture artifact provenance at run start.
        //
        // Uses the artifact intake result evaluated once above (TV-01 hoist) —
        // the same identity that passed all pre-DB gates is the identity recorded
        // as this run's provenance.  No second evaluation; TOCTOU gap closed.
        //
        // Only `Accepted` carries positive provenance; all other outcomes leave
        // `accepted_artifact` as `None` (fail-closed: absent/invalid/unavailable
        // artifacts are not recorded as consumed).
        {
            let provenance = match artifact_intake {
                ArtifactIntakeOutcome::Accepted {
                    artifact_id,
                    artifact_type,
                    stage,
                    produced_by,
                } => Some(AcceptedArtifactProvenance {
                    artifact_id,
                    artifact_type,
                    stage,
                    produced_by,
                }),
                _ => None,
            };
            *self.accepted_artifact.write().await = provenance;
        }

        // B1A: store native strategy bootstrap for the active run.
        // Placed after all DB operations and the initial tick succeed so the
        // field is only populated when the run is fully live.
        *self.native_strategy_bootstrap.lock().await = Some(native_strategy_bootstrap);

        let handle = spawn_execution_loop(Arc::clone(self), orchestrator, run_id);
        {
            let mut lock = self.execution_loop.lock().await;
            if lock.is_some() {
                return Err(RuntimeLifecycleError::conflict(
                    "runtime.start_refused.local_ownership_conflict",
                    "runtime ownership changed while starting; refusing duplicate loop",
                ));
            }
            *lock = Some(handle);
        }

        {
            let snap_arc = Arc::clone(&self.execution_snapshot);
            let sides_arc = Arc::clone(&self.local_order_sides);
            let broker_arc = Arc::clone(&self.broker_snapshot);
            let local_fn = move || {
                let snapshot = snap_arc.try_read().ok().and_then(|g| g.clone());
                if let Some(snapshot) = snapshot {
                    let sides = sides_arc.try_read().map(|g| g.clone()).unwrap_or_default();
                    reconcile_local_snapshot_from_runtime_with_sides(&snapshot, &sides)
                } else {
                    mqk_reconcile::LocalSnapshot::empty()
                }
            };
            let broker_fn = move || {
                let schema = broker_arc.try_read().ok().and_then(|g| g.clone())?;
                reconcile_broker_snapshot_from_schema(&schema).ok()
            };
            spawn_reconcile_tick(
                Arc::clone(self),
                local_fn,
                broker_fn,
                RECONCILE_TICK_INTERVAL,
            );
        }

        let snapshot = StatusSnapshot {
            daemon_uptime_secs: uptime_secs(),
            active_run_id: Some(run_id),
            state: "running".to_string(),
            notes: Some("daemon owns active execution loop".to_string()),
            integrity_armed: self.integrity_armed().await,
            deadman_status: "healthy".to_string(),
            deadman_last_heartbeat_utc: Some(Utc::now().to_rfc3339()),
        };
        self.publish_status(snapshot.clone()).await;
        Ok(snapshot)
    }

    pub async fn stop_execution_runtime(
        self: &Arc<Self>,
    ) -> Result<StatusSnapshot, RuntimeLifecycleError> {
        let _op = self.lifecycle_op.lock().await;
        self.reap_finished_execution_loop().await?;
        let handle = match self.take_execution_loop_for_control().await? {
            Some(handle) => handle,
            None => {
                if let Some(db) = self.db.as_ref() {
                    if let Some(active) = mqk_db::fetch_active_run_for_engine(
                        db,
                        DAEMON_ENGINE_ID,
                        self.deployment_mode().as_db_mode(),
                    )
                    .await
                    .map_err(|err| {
                        RuntimeLifecycleError::internal("stop active-run lookup failed", err)
                    })? {
                        return Err(RuntimeLifecycleError::conflict(
                            "runtime.truth_mismatch.durable_active_without_local_owner",
                            format!(
                                "durable active run exists without local ownership: {}",
                                active.run_id
                            ),
                        ));
                    }
                }
                return self.current_status_snapshot().await;
            }
        };

        let run_id = handle.run_id;
        let _ = handle.stop_tx.send(ExecutionLoopCommand::Stop);
        let _ = handle
            .join_handle
            .await
            .map_err(|err| RuntimeLifecycleError::internal("stop join failed", err))?;

        let db = self.db_pool()?;
        let run = mqk_db::fetch_run(&db, run_id)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("stop fetch_run failed", err))?;
        if matches!(
            run.status,
            mqk_db::RunStatus::Armed | mqk_db::RunStatus::Running
        ) {
            mqk_db::stop_run(&db, run_id)
                .await
                .map_err(|err| RuntimeLifecycleError::internal("stop_run failed", err))?;
        }

        // TV-01C: clear artifact provenance on stop — no active run means no active artifact.
        *self.accepted_artifact.write().await = None;
        // B1A: clear native strategy bootstrap on stop — host is not active without a run.
        *self.native_strategy_bootstrap.lock().await = None;

        let snapshot = self.current_status_snapshot().await?;
        Ok(snapshot)
    }

    pub async fn halt_execution_runtime(
        self: &Arc<Self>,
    ) -> Result<StatusSnapshot, RuntimeLifecycleError> {
        let _op = self.lifecycle_op.lock().await;
        self.reap_finished_execution_loop().await?;

        let handle = self.take_execution_loop_for_control().await?;
        if handle.is_none() {
            if let Some(db) = self.db.as_ref() {
                if let Some(active) = mqk_db::fetch_active_run_for_engine(
                    db,
                    DAEMON_ENGINE_ID,
                    self.deployment_mode().as_db_mode(),
                )
                .await
                .map_err(|err| {
                    RuntimeLifecycleError::internal("halt active-run lookup failed", err)
                })? {
                    return Err(RuntimeLifecycleError::conflict(
                        "runtime.truth_mismatch.durable_active_without_local_owner",
                        format!(
                            "durable active run exists without local ownership: {}",
                            active.run_id
                        ),
                    ));
                }
            }
        }

        {
            let mut integrity = self.integrity.write().await;
            integrity.disarmed = true;
            integrity.halted = true;
        }

        let db = self.db_pool()?;
        if let Some(handle) = handle {
            let run_id = handle.run_id;
            let _ = handle.stop_tx.send(ExecutionLoopCommand::Stop);
            let _ = handle
                .join_handle
                .await
                .map_err(|err| RuntimeLifecycleError::internal("halt join failed", err))?;

            mqk_db::halt_run(&db, run_id, Utc::now())
                .await
                .map_err(|err| RuntimeLifecycleError::internal("halt_run failed", err))?;
        }
        mqk_db::persist_arm_state_canonical(
            &db,
            mqk_db::ArmState::Disarmed,
            Some(mqk_db::DisarmReason::OperatorHalt),
        )
        .await
        .map_err(|err| RuntimeLifecycleError::internal("persist_arm_state failed", err))?;

        // TV-01C: clear artifact provenance on halt — no active run means no active artifact.
        *self.accepted_artifact.write().await = None;
        // B1A: clear native strategy bootstrap on halt — host is not active without a run.
        *self.native_strategy_bootstrap.lock().await = None;

        let snapshot = StatusSnapshot {
            daemon_uptime_secs: uptime_secs(),
            active_run_id: self.current_status_snapshot().await?.active_run_id,
            state: "halted".to_string(),
            notes: Some("operator halt asserted; execution loop disarmed".to_string()),
            integrity_armed: false,
            deadman_status: "expired".to_string(),
            deadman_last_heartbeat_utc: None,
        };
        self.publish_status(snapshot.clone()).await;
        Ok(snapshot)
    }

    /// AUTON-PAPER-01B: Pre-session autonomous arm seam.
    ///
    /// Attempts to advance in-memory integrity state from disarmed to armed by
    /// reading the persisted arm state from the DB.  Called by the autonomous
    /// session controller immediately before `start_execution_runtime` so the
    /// daily session can start without a manual operator arm.
    ///
    /// # Gate rules (fail-closed ordering)
    ///
    /// 1. `integrity.halted == true` → refuse unconditionally (operator halt
    ///    wins; not reversible by the controller).
    /// 2. `integrity.disarmed == false` → already armed; return `Ok(())`.
    /// 3. No DB configured → refuse (cannot verify prior session state).
    /// 4. No DB row → refuse (first-time install; operator must arm once).
    /// 5. DB state = `"ARMED"` → auto-arm: set `disarmed=false, halted=false`,
    ///    re-persist `Armed`, return `Ok(())`.
    /// 6. DB state = anything else (`"DISARMED"`) → refuse with stored reason.
    ///
    /// # Daily-cycle property
    ///
    /// `stop_execution_runtime` does NOT write `Disarmed` to the DB, so after a
    /// clean daily stop the DB remains `ARMED`.  On the next daemon boot the
    /// in-memory integrity state starts as `disarmed=true` (fail-closed), but
    /// the DB row carries the prior `ARMED` state → auto-arm succeeds → the
    /// session controller can start the next day without operator intervention.
    ///
    /// Only `halt_execution_runtime` writes `DISARMED` to the DB.  A halted
    /// daemon therefore requires manual operator arm before the controller can
    /// restart, which is the correct safety posture.
    pub async fn try_autonomous_arm(&self) -> Result<(), String> {
        // Gate 1: operator halt wins unconditionally.
        // Gate 2: already armed is idempotent success.
        {
            let ig = self.integrity.read().await;
            if ig.halted {
                return Err(
                    "operator halt asserted; autonomous arm refused (integrity.halted=true)"
                        .to_string(),
                );
            }
            if !ig.disarmed {
                return Ok(());
            }
        }

        // Gate 3: DB required to verify prior session state.
        let db = match self.db.as_ref() {
            Some(db) => db,
            None => {
                return Err(
                    "no DB configured; autonomous arm requires persisted arm state".to_string(),
                )
            }
        };

        // Gate 4/5/6: load prior arm state from the singleton row.
        let row = mqk_db::load_arm_state(db)
            .await
            .map_err(|err| format!("autonomous arm: load_arm_state failed: {err}"))?;

        match row {
            None => Err(
                "no prior arm state in DB; operator must arm manually at least once \
                 (first-time install or DB was wiped)"
                    .to_string(),
            ),
            Some((ref state_str, _)) if state_str == "ARMED" => {
                // Prior session ended cleanly (stop does not write DISARMED).
                // Advance in-memory integrity to armed.
                {
                    let mut ig = self.integrity.write().await;
                    ig.disarmed = false;
                    ig.halted = false;
                }
                // Re-persist Armed so another daemon restart also sees ARMED.
                mqk_db::persist_arm_state_canonical(db, mqk_db::ArmState::Armed, None)
                    .await
                    .map_err(|err| {
                        format!("autonomous arm: persist_arm_state_canonical failed: {err}")
                    })?;
                Ok(())
            }
            Some((_, reason)) => {
                let reason_str = reason.as_deref().unwrap_or("unknown");
                Err(format!(
                    "DB arm state is DISARMED (reason={reason_str}); autonomous arm refused"
                ))
            }
        }
    }

    pub async fn stop_for_shutdown(self: &Arc<Self>) {
        if let Some(handle) = self.take_execution_loop_for_shutdown().await {
            let run_id = handle.run_id;
            let _ = handle.stop_tx.send(ExecutionLoopCommand::Stop);
            match handle.join_handle.await {
                Ok(_) => {
                    if let Some(db) = self.db.as_ref() {
                        match mqk_db::fetch_run(db, run_id).await {
                            Ok(run) => {
                                if matches!(
                                    run.status,
                                    mqk_db::RunStatus::Armed | mqk_db::RunStatus::Running
                                ) {
                                    if let Err(err) = mqk_db::stop_run(db, run_id).await {
                                        tracing::warn!("shutdown stop_run failed for {run_id}: {err}");
                                    }
                                }
                            }
                            Err(err) => {
                                tracing::warn!("shutdown fetch_run_failed for {run_id}: {err}");
                            }
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!("shutdown join failed for {run_id}: {err}");
                }
            }
        }
    }
}
