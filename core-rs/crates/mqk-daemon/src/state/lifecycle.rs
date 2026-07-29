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
use crate::market_data_freshness::{
    evaluate_md_freshness_status_for_symbols, required_symbols_for_freshness_gate_from_env,
};
use crate::parity_evidence::{evaluate_parity_evidence_from_env, ParityEvidenceOutcome};

use sqlx::PgPool;

use super::loop_runner::spawn_execution_loop;
use super::types::{DaemonOrchestrator, ExecutionLoopCommand};
use super::{
    reconcile_broker_snapshot_from_schema, reconcile_local_snapshot_from_runtime_with_sides,
    spawn_reconcile_tick, uptime_secs,
};
use super::{
    AcceptedArtifactProvenance, BrokerKind, DeploymentMode, DynamicSelectionLifecycleFaultSeam,
    DynamicSelectionRuntimeState, OperatorAuthMode, RuntimeLifecycleError, StatusSnapshot,
    StrategyMarketDataSource,
};
use super::{AppState, DAEMON_ENGINE_ID, RECONCILE_TICK_INTERVAL};

use mqk_runtime::native_strategy::NativeStrategyBootstrap;

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2: typed autonomous arm outcome
// ---------------------------------------------------------------------------

/// Typed success outcome of [`AppState::try_autonomous_arm_typed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutonomousArmOutcome {
    /// Integrity was already armed (`integrity.disarmed == false`) —
    /// idempotent success, no DB access performed.
    AlreadyArmed,
    /// Integrity was disarmed but the persisted arm-state row was `ARMED`
    /// (the ordinary clean-stop-then-restart daily cycle); in-memory
    /// integrity was advanced to armed and re-persisted.
    ArmedFromPersistedState,
}

/// Typed failure outcome of [`AppState::try_autonomous_arm_typed`]. Never a
/// broad free-text variant -- the durable daily coordinator classifies this
/// value exclusively by variant, never by parsing rendered text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutonomousArmRejection {
    /// `integrity.halted == true`. Operator halt wins unconditionally and is
    /// never reversible by the autonomous arm seam.
    IntegrityHalted,
    /// No DB is configured on this daemon; autonomous arm cannot verify
    /// prior session state.
    DatabaseNotConfigured,
    /// A DB is configured, but no arm-state row exists yet (first-time
    /// install, or the DB was wiped). Requires one manual operator arm.
    NoPersistedArmState,
    /// The persisted arm-state row is `DISARMED`, with an optional stored
    /// reason.
    DurableDisarmed { reason: Option<String> },
    /// A DB operation (`load_arm_state` or `persist_arm_state_canonical`)
    /// failed against an otherwise-configured, otherwise-reachable
    /// database. `operation` names the specific call that failed.
    TemporaryDatabaseOperationFailure { operation: &'static str },
}

impl AutonomousArmRejection {
    /// Render the exact historical `try_autonomous_arm()` message text for
    /// each rejection, so the `Result<(), String>` compatibility wrapper
    /// remains byte-for-byte unchanged for every existing caller/test.
    fn legacy_message(&self) -> String {
        match self {
            Self::IntegrityHalted => {
                "operator halt asserted; autonomous arm refused (integrity.halted=true)".to_string()
            }
            Self::DatabaseNotConfigured => {
                "no DB configured; autonomous arm requires persisted arm state".to_string()
            }
            Self::NoPersistedArmState => "no prior arm state in DB; operator must arm manually \
                 at least once (first-time install or DB was wiped)"
                .to_string(),
            Self::DurableDisarmed { reason } => {
                let reason_str = reason.as_deref().unwrap_or("unknown");
                format!("DB arm state is DISARMED (reason={reason_str}); autonomous arm refused")
            }
            Self::TemporaryDatabaseOperationFailure { operation } => {
                format!("autonomous arm: {operation} failed")
            }
        }
    }
}

impl AppState {
    /// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: build the
    /// authoritative, frozen dynamic-selection start snapshot for `run_id`.
    ///
    /// Resolves the dynamic-selection mode, the `MultiSymbolRuntimeConfig`,
    /// and the calendar/session authority timestamp exactly once each, then
    /// evaluates `evaluate_dynamic_selection_start_gate` exactly once. Never
    /// touches `AppState` — the caller commits the returned value later, at
    /// the same point `ProductionRuntimeStartEffects` publishes every other
    /// run-start effect.
    ///
    /// `Ok(state)` covers every disposition except `PaperEnforcedRefused`:
    /// `Off` (zero I/O beyond the one env-var mode read), `ShadowAllowed`,
    /// `ShadowInvalid` (including a Shadow-mode `MultiSymbolRuntimeConfig`
    /// resolution failure — Shadow never blocks the run), and
    /// `PaperEnforcedAllowed`. `Err(_)` covers `PaperEnforcedRefused` and a
    /// PaperEnforced-mode config-resolution failure — both refuse the whole
    /// start before any run advancement, with no `AppState` mutation.
    async fn build_dynamic_selection_start_snapshot(
        self: &Arc<Self>,
        run_id: uuid::Uuid,
    ) -> Result<DynamicSelectionRuntimeState, RuntimeLifecycleError> {
        use crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition;
        use mqk_portfolio::DynamicSelectionMode;

        // Mode resolved exactly once: one env-var read, one pure live-lock
        // combinator call. Never reread later in this function.
        let mode_resolution =
            crate::dynamic_selection_mode::resolve_dynamic_selection_mode_from_env();
        let effective = crate::dynamic_selection_mode::effective_mode(
            &mode_resolution,
            self.deployment_mode(),
            self.runtime_selection.broker_kind,
        );

        if effective.effective_mode == DynamicSelectionMode::Off {
            // Off: zero further I/O — no config resolution, no calendar
            // provider, no plan builder, no promotion query, no host pool.
            return Ok(DynamicSelectionRuntimeState {
                run_id,
                disposition: DynamicSelectionStartGateDisposition::Off,
                configured_mode: effective.configured_mode,
                effective_mode: effective.effective_mode,
                live_lock_applied: effective.live_lock_applied,
                plan: None,
                selected_pairs: Vec::new(),
                host_pool: None,
                reasons: Vec::new(),
                approved_for_live: false,
            });
        }

        // Non-Off (Shadow or PaperEnforced) is only reachable when
        // deployment_mode==Paper && broker_kind==Alpaca (the mode live-lock
        // proves this) — exactly the predicate the pre-existing
        // daily-data-readiness/premarket-freshness gates above already use —
        // so the resolution calls below are safe to make unconditionally.
        let multi_symbol_config = match crate::state::build_multi_symbol_runtime_config_from_env() {
            Ok(cfg) => cfg,
            Err(err) => {
                if effective.effective_mode == DynamicSelectionMode::Shadow {
                    // Shadow never blocks the run — record the truthful
                    // failure and let the legacy start continue.
                    return Ok(DynamicSelectionRuntimeState {
                        run_id,
                        disposition: DynamicSelectionStartGateDisposition::ShadowInvalid,
                        configured_mode: effective.configured_mode,
                        effective_mode: effective.effective_mode,
                        live_lock_applied: effective.live_lock_applied,
                        plan: None,
                        selected_pairs: Vec::new(),
                        host_pool: None,
                        reasons: vec![
                            crate::dynamic_selection_start_gate::DynamicSelectionStartGateReason::PlanInvalid {
                                truth_state: format!(
                                    "multi_symbol_config_unavailable:{}",
                                    err.as_str()
                                ),
                            },
                        ],
                        approved_for_live: false,
                    });
                }
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.dynamic_selection_config_unavailable",
                    "dynamic_selection",
                    format!(
                        "dynamic selection paper_enforced start refused: MultiSymbolRuntimeConfig \
                         resolution failed: {} (DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A)",
                        err.as_str()
                    ),
                ));
            }
        };

        // Calendar/session authority + provider/instrument registries,
        // resolved exactly once from the same env-driven composition the
        // daily-data-readiness gate above already uses
        // (`daily_data_readiness::load_readiness_context_from_env`).
        let readiness_context = crate::daily_data_readiness::load_readiness_context_from_env();
        let configured_strategy_ids =
            crate::daily_data_readiness::fleet_ids_from_env().unwrap_or_default();
        let now_utc = Utc::now();
        let run_id_str = run_id.to_string();

        let context = crate::dynamic_selection_start_gate::build_dynamic_selection_context(
            &run_id_str,
            &effective,
            &multi_symbol_config,
            readiness_context.calendar_provider.as_ref(),
            now_utc,
        );

        let plan_ctx = crate::dynamic_selection_plan_builder::DynamicSelectionPlanBuildContext {
            db: self.db.as_ref(),
            st: self,
            calendar_provider: readiness_context.calendar_provider.as_ref(),
            provider_configs: &readiness_context.provider_configs,
            instruments: &readiness_context.instruments,
        };

        let outcome = crate::dynamic_selection_start_gate::evaluate_dynamic_selection_start_gate(
            &plan_ctx,
            &multi_symbol_config,
            &configured_strategy_ids,
            &effective,
            context,
            &run_id_str,
            now_utc,
        )
        .await;

        if outcome.disposition == DynamicSelectionStartGateDisposition::PaperEnforcedRefused {
            let reason_codes: Vec<&'static str> =
                outcome.reasons.iter().map(|r| r.code()).collect();
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.start_refused.dynamic_selection_paper_enforced_refused",
                "dynamic_selection_start_gate",
                format!(
                    "dynamic selection paper_enforced start gate refused start: run_id={run_id}; \
                     reasons={reason_codes:?} (DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A)"
                ),
            ));
        }

        let selected_pairs = outcome
            .plan
            .as_ref()
            .map(crate::dynamic_selection_start_gate::selected_host_pool_keys)
            .unwrap_or_default();

        Ok(DynamicSelectionRuntimeState {
            run_id,
            disposition: outcome.disposition,
            configured_mode: effective.configured_mode,
            effective_mode: effective.effective_mode,
            live_lock_applied: effective.live_lock_applied,
            plan: outcome.plan.map(Arc::new),
            selected_pairs,
            host_pool: outcome.host_pool.map(Arc::new),
            reasons: outcome.reasons,
            approved_for_live: false,
        })
    }

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
        //
        // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D1-TYPED-COORDINATOR-POLICY:
        // this resolution is extracted into the shared, side-effect-free
        // `autonomous_runtime_context::resolve_autonomous_runtime_context`
        // seam so a future daily coordinator can resolve the identical
        // bootstrap/binding without a second, independently-derived
        // resolution. Gate order, fault classes, and messages are unchanged
        // — this call site behaves identically to the former inline block.
        let super::autonomous_runtime_context::ResolvedAutonomousRuntimeContext {
            native_strategy_bootstrap,
            effective_runtime_binding,
        } = super::autonomous_runtime_context::resolve_autonomous_runtime_context(self).await?;

        // DAILY-DATA-READINESS-01C-ENFORCEMENT-01: strict daily data
        // readiness start gate.
        //
        // Applicable only to Paper+ExternalSignalIngestion — the same
        // predicate the PREMARKET-DATA-READINESS-GATE-01 legacy gate below
        // uses, never hardcoded to BrokerKind::Alpaca (contract §C.5).
        //
        // Uses the exact `native_strategy_bootstrap`/`effective_runtime_binding`
        // pair constructed above (B1A) — never a second, independently
        // constructed bootstrap (Phase B's `evaluate_daily_data_readiness_from_env`
        // lifecycle-safety contract).
        //
        // Placed after B1A (bootstrap/binding resolution) and before
        // db_pool()? so this evaluator — not a generic db_pool() error —
        // produces the canonical db_unavailable verdict when the DB is
        // absent (contract §16), and so a blocked verdict denies before any
        // DB resource, run row, or broker/provider call.
        //
        // DAILY-DATA-READINESS-01C-MISSING-ASSIGNMENT-EVIDENCE-REPAIR-01:
        // required ordering is bootstrap/binding resolution (B1A, above) ->
        // capture evaluated_at_utc -> allocate attempt_seq -> attempt
        // assignment resolution -> construct resolved-or-blocked assignment
        // identity -> compute evaluation_id -> construct readiness report ->
        // attempt pre-start evidence persistence -> return blocked verdict or
        // continue. An applicable attempt whose assignment resolution itself
        // fails (`build_multi_symbol_runtime_config_from_env()` returns
        // `Err`) still receives a distinct `attempt_seq`, a real
        // `evaluation_id`, a truthful bounded blocked report, and a genuine
        // pre-start evidence persist attempt — never an early return before
        // any of that exists.
        // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-CANONICAL-CALENDAR-REPAIR-01:
        // narrow shared-calendar blocker input. A configured-but-invalid
        // fixed session-window override (MQK_SESSION_START_HH_MM/
        // _STOP_HH_MM) must never be silently treated as absent by an
        // applicable autonomous start attempt — refuse before any readiness
        // evaluation, run creation, or provider/broker call. Absent or Valid
        // configuration is the ordinary case and falls through unchanged,
        // so this never disturbs the existing readiness evidence ordering
        // below.
        if self.deployment_mode() == DeploymentMode::Paper
            && self.strategy_market_data_source()
                == StrategyMarketDataSource::ExternalSignalIngestion
        {
            if let super::autonomous_daily_operation::FixedWindowOverrideConfig::Invalid {
                detail,
            } =
                super::autonomous_daily_operation::resolve_fixed_window_override_config_from_env()
            {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.fixed_window_override_invalid",
                    "fixed_window_override",
                    format!(
                        "fixed session-window override configuration is invalid: {detail} ({})",
                        super::autonomous_daily_operation::AutonomousDailyPlanReason::FixedWindowOverrideInvalid
                            .as_str()
                    ),
                ));
            }
        }

        let mut daily_data_readiness_evaluation_id: Option<uuid::Uuid> = None;
        if self.deployment_mode() == DeploymentMode::Paper
            && self.strategy_market_data_source()
                == StrategyMarketDataSource::ExternalSignalIngestion
        {
            let evaluated_at_utc = self.daily_data_readiness_now().await;
            // REPAIR 1 (...MISSING-ASSIGNMENT-EVIDENCE-REPAIR-01): allocate a
            // fresh attempt sequence number for this actual start-gate
            // evaluation (never for a GET/preview evaluation) BEFORE
            // assignment resolution is even attempted, so two
            // otherwise-identical attempts — including two that both fail
            // assignment resolution — never collide on `evaluation_id`.
            let attempt_seq = self.next_daily_data_readiness_attempt_seq();

            let config_result = crate::state::build_multi_symbol_runtime_config_from_env();

            // REPAIR 2/3: construct the resolved-or-blocked assignment
            // identity, `evaluation_id`, and readiness report from whichever
            // branch actually happened — never a fabricated
            // `MultiSymbolRuntimeConfig` for the failure branch.
            let (report, evaluation_id, assignment_resolution_error) = match &config_result {
                Ok(config) => {
                    let readiness_context =
                        crate::daily_data_readiness::load_readiness_context_from_env();
                    let report = crate::daily_data_readiness::evaluate_readiness_with_binding(
                        self.db.as_ref(),
                        config,
                        &effective_runtime_binding,
                        &readiness_context,
                        evaluated_at_utc,
                    )
                    .await;
                    let evaluation_id = crate::daily_data_readiness::compute_evaluation_id(
                        evaluated_at_utc,
                        attempt_seq,
                        &effective_runtime_binding,
                        config,
                    );
                    (report, evaluation_id, None::<&'static str>)
                }
                Err(err) => {
                    let top_level_blocker =
                        crate::daily_data_readiness::top_level_blocker_for_config_error(err);
                    let report = crate::daily_data_readiness::blocked_report(top_level_blocker);
                    // Bounded, stable failure identity (REPAIR 2) — never
                    // secrets, full environment dumps, or unbounded
                    // filesystem errors; `MultiSymbolConfigError::as_str()`
                    // is itself already a fixed, small set of literal
                    // strings.
                    let assignment_identity =
                        vec![format!("assignment_resolution_error:{}", err.as_str())];
                    let evaluation_id =
                        crate::daily_data_readiness::compute_evaluation_id_from_assignment_identity(
                            evaluated_at_utc,
                            attempt_seq,
                            &effective_runtime_binding,
                            &assignment_identity,
                        );
                    (report, evaluation_id, Some(err.as_str()))
                }
            };

            // The real write is always attempted (regardless of any test
            // override) — the pre-start event must be attempted before run
            // creation for every applicable evaluation (§C.8), including an
            // assignment-resolution failure. The override (test-only) only
            // substitutes what gets *reported*/gated on afterward, so tests
            // can prove the §C.9 failure policy without needing the real
            // write to actually fail.
            let real_evidence_persisted = match self.db.as_ref() {
                Some(db) => {
                    crate::daily_data_readiness::persist_pre_start_readiness_evidence(
                        db,
                        evaluation_id,
                        evaluated_at_utc,
                        "applicable",
                        &report,
                    )
                    .await
                }
                None => false,
            };
            let evidence_persisted = self
                .daily_data_readiness_evidence_override()
                .await
                .unwrap_or(real_evidence_persisted);

            if report.start_allowed {
                if !evidence_persisted {
                    return Err(RuntimeLifecycleError::service_unavailable(
                        "runtime.start_refused.readiness_evidence_persist_failed",
                        format!(
                            "strict daily data readiness evaluation_id={evaluation_id} was ready \
                             but durable pre-start evidence could not be persisted; refusing \
                             start ({})",
                            crate::daily_data_readiness::REASON_READINESS_EVIDENCE_PERSIST_FAILED
                        ),
                    ));
                }
                daily_data_readiness_evaluation_id = Some(evaluation_id);
            } else {
                let assignment_blockers: Vec<String> = report
                    .assignments
                    .iter()
                    .map(|a| {
                        format!(
                            "{}/{}: {:?}",
                            a.assignment_symbol, a.assignment_timeframe, a.blockers
                        )
                    })
                    .collect();
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.daily_data_readiness_blocked",
                    "daily_data_readiness",
                    format!(
                        "strict daily data readiness gate refused start: evaluation_id={evaluation_id}; \
                         start_allowed=false; top_level_blocker={:?}; \
                         assignment_resolution_error={assignment_resolution_error:?}; \
                         assignment_blockers={assignment_blockers:?}; \
                         evidence_persisted={evidence_persisted} (DAILY-DATA-READINESS-01C-ENFORCEMENT-01)",
                        report.top_level_blocker,
                    ),
                ));
            }
        }

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

        // PREMARKET-DATA-READINESS-GATE-01: Multi-symbol premarket market-data
        // readiness gate (extends DATA-FRESHNESS-READINESS-GATE-01 from a single
        // symbol/timeframe check to every symbol the current deployment requires —
        // the approved watchlist-v2 artifact's symbols when one is configured and
        // approved, otherwise the legacy single MQK_STRATEGY_SYMBOL).
        //
        // For the Paper+Alpaca path only: verify that md_bars contains sufficient
        // fresh completed bars for every required symbol/timeframe before any
        // execution run is created. Any single required symbol failing blocks
        // start for the whole run (fail-closed).
        //
        // Contract (per required symbol, same thresholds as the prior single-symbol gate):
        //   not_applicable — no symbol configured; gate not applicable; pass.
        //   unavailable    — DB query failed; cannot assert missing; pass (honest).
        //   ok             — sufficient fresh bars exist; pass.
        //   missing        — 0 completed bars; fail-closed.
        //   insufficient   — fewer than MD_FRESHNESS_MIN_BARS completed bars; fail-closed.
        //   stale          — latest bar older than MD_FRESHNESS_STALE_SECS; fail-closed.
        //
        // Placed after db_pool() (requires DB) and before insert_run / lease acquisition
        // so a readiness refusal leaves no dangling run rows.
        if self.deployment_mode() == DeploymentMode::Paper
            && self.strategy_market_data_source()
                == StrategyMarketDataSource::ExternalSignalIngestion
        {
            let required = required_symbols_for_freshness_gate_from_env();
            let readiness = evaluate_md_freshness_status_for_symbols(
                Some(&db),
                &required,
                Utc::now().timestamp(),
            )
            .await;
            if !readiness.start_allowed {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.market_data_not_fresh",
                    "market_data_freshness",
                    format!(
                        "premarket market-data readiness gate failed \
                         (aggregate_status='{}', required_symbols={:?}): {} \
                         Run Prep-PremarketMarketData.ps1 before starting \
                         (PREMARKET-DATA-READINESS-GATE-01)",
                        readiness.aggregate_status,
                        readiness.required_symbols,
                        readiness.blockers.join("; "),
                    ),
                ));
            }
        }

        let run_id = self.create_or_reuse_run_for_start(&db).await?;

        // BUNDLE-7-PHASE-7A fault seam: after run row creation, before
        // dynamic-selection evaluation. No AppState selection state exists
        // yet — nothing to clean up beyond returning the error.
        if self
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::AfterRunRowCreation,
            )
            .await
        {
            return Err(RuntimeLifecycleError::internal(
                "dynamic_selection.fault_seam.after_run_row_creation",
                "test-injected fault after run row creation",
            ));
        }

        // DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: build the frozen
        // dynamic-selection outcome for this exact run_id now — `run_id`
        // must exist before the evaluator can be called (it validates the
        // caller-supplied context's run_id against this one). A
        // `PaperEnforcedRefused` disposition refuses the whole start here,
        // before any run advancement — the run row is left `Created`
        // (unarmed/unbegun/unspawned), matching this codebase's existing
        // fail-closed convention for other pre-advancement refusals (see
        // `RunLinkPersistFailed` below). Every other disposition (`Off`,
        // `ShadowAllowed`, `ShadowInvalid`, `PaperEnforcedAllowed`) is
        // committed to `AppState` later, inside the same atomic
        // start-commit sequence that publishes every other run-start
        // effect — never here.
        let dynamic_selection_outcome = self.build_dynamic_selection_start_snapshot(run_id).await?;

        // BUNDLE-7-PHASE-7A fault seam: after selection evaluation, before
        // effects construction. No AppState selection state has been
        // committed yet — nothing to clean up beyond returning the error.
        if self
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::AfterSelectionEvaluation,
            )
            .await
        {
            return Err(RuntimeLifecycleError::internal(
                "dynamic_selection.fault_seam.after_selection_evaluation",
                "test-injected fault after selection evaluation",
            ));
        }

        // DAILY-DATA-READINESS-01C-ENFORCEMENT-01 / REPAIR 3-4
        // (DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01): advance the
        // created/reused run to an active execution loop. `run_id` now
        // exists — link it to the pre-start evaluation_id captured by the
        // strict readiness gate (fail-closed on a link-persist failure: no
        // arm/begin/tick/spawn — REPAIR 4 replaces the prior non-fatal
        // sticky-degraded policy for this specific evidence boundary), then
        // perform runtime construction/safety-setup and spawn the execution
        // loop, in this exact order. Non-applicable deployments have no
        // evaluation_id to link and proceed straight to runtime effects,
        // exactly as before this patch.
        //
        // The production start path and the synthetic lifecycle proof test
        // (`scenario_daily_data_readiness_start_gate_01.rs`) both drive this
        // exact sequencing function against `RuntimeStartEffects` — never
        // two independently-ordered implementations.
        let readiness_link = daily_data_readiness_evaluation_id.map(|id| (id, Utc::now()));
        let effects = ProductionRuntimeStartEffects {
            state: self,
            db: db.clone(),
            artifact_intake: std::sync::Mutex::new(Some(artifact_intake)),
            native_strategy_bootstrap: std::sync::Mutex::new(Some(native_strategy_bootstrap)),
            orchestrator: std::sync::Mutex::new(None),
            dynamic_selection_outcome: std::sync::Mutex::new(Some(dynamic_selection_outcome)),
        };
        let mut lifecycle_trace: Vec<&'static str> = Vec::new();
        crate::daily_data_readiness::advance_run_to_active(
            &db,
            &effects,
            run_id,
            readiness_link,
            &mut lifecycle_trace,
        )
        .await
        .map_err(|err| match err {
            crate::daily_data_readiness::RuntimeStartSequenceError::RunLinkPersistFailed {
                evaluation_id,
                run_id,
            } => RuntimeLifecycleError::service_unavailable(
                "runtime.start_refused.readiness_run_link_persist_failed",
                format!(
                    "strict daily data readiness run-linked evidence persist failed for \
                     evaluation_id={evaluation_id} run_id={run_id}; refusing to arm, begin, \
                     tick, or spawn the execution loop — the run row exists but must not be \
                     presented as an actively started runtime ({})",
                    crate::daily_data_readiness::REASON_READINESS_RUN_LINK_PERSIST_FAILED,
                ),
            ),
            crate::daily_data_readiness::RuntimeStartSequenceError::Effects(effects_err) => {
                match effects_err.kind {
                    crate::daily_data_readiness::RuntimeStartEffectsErrorKind::Internal => {
                        RuntimeLifecycleError::internal(
                            effects_err.fault_class,
                            effects_err.message,
                        )
                    }
                    crate::daily_data_readiness::RuntimeStartEffectsErrorKind::Conflict => {
                        RuntimeLifecycleError::conflict(
                            effects_err.fault_class,
                            effects_err.message,
                        )
                    }
                }
            }
        })?;

        {
            let snap_arc = Arc::clone(&self.execution_snapshot);
            // Separate clone for the settle closure — snap_arc is moved into local_fn.
            let snap_arc_settle = Arc::clone(&self.execution_snapshot);
            let sides_arc = Arc::clone(&self.local_order_sides);
            let broker_arc = Arc::clone(&self.broker_snapshot);
            // BROKER-POSITION-BASELINE-ADOPTION-01: when no execution run is active,
            // use the operator-adopted baseline (if any) as local truth so the
            // reconcile tick sees local == broker and publishes clean state.
            let baseline_arc = Arc::clone(&self.broker_baseline);
            let local_fn = move || {
                let snapshot = snap_arc.try_read().ok().and_then(|g| g.clone());
                if let Some(snapshot) = snapshot {
                    let sides = sides_arc.try_read().map(|g| g.clone()).unwrap_or_default();
                    // RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01: execution_snapshot
                    // .portfolio.positions already carries the adopted broker
                    // baseline (seeded once at run start by
                    // seed_portfolio_from_baseline — see the comment above the
                    // seeding call in build_execution_orchestrator) plus any
                    // same-run fill delta. Re-merging baseline_arc here would
                    // double-count it: local = fills + 2x baseline, while broker
                    // truth = fills + baseline, producing a false ReconcileDrift
                    // halt/disarm. Derive local truth directly from the seeded
                    // snapshot — no merge.
                    reconcile_local_snapshot_from_runtime_with_sides(&snapshot, &sides)
                } else {
                    // No active run: use adopted baseline if present, else empty.
                    baseline_arc
                        .try_read()
                        .ok()
                        .and_then(|g| g.clone())
                        .unwrap_or_else(mqk_reconcile::LocalSnapshot::empty)
                }
            };
            let broker_fn = move || {
                let schema = broker_arc.try_read().ok().and_then(|g| g.clone())?;
                reconcile_broker_snapshot_from_schema(&schema).ok()
            };
            // RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01: defer background reconcile
            // disarm when the execution snapshot signals a recent terminal fill.
            let settle_fn = move || {
                snap_arc_settle
                    .try_read()
                    .ok()
                    .and_then(|g| g.as_ref().map(|s| s.has_recent_terminal_fill))
                    .unwrap_or(false)
            };
            spawn_reconcile_tick(
                Arc::clone(self),
                local_fn,
                broker_fn,
                settle_fn,
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

    /// Look up any existing durable run for this engine/mode and either
    /// reuse a `Created` run or create a fresh one.
    ///
    /// REPAIR 3 (DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01): extracted from
    /// `start_execution_runtime`'s previously-inline lookup/creation logic
    /// (unchanged) so the production start path and the synthetic lifecycle
    /// proof test both create a run through the exact same code — never a
    /// separate test-only reimplementation of this identity resolution.
    pub async fn create_or_reuse_run_for_start(
        self: &Arc<Self>,
        db: &PgPool,
    ) -> Result<uuid::Uuid, RuntimeLifecycleError> {
        if let Some(active) = mqk_db::fetch_active_run_for_engine(
            db,
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
            db,
            DAEMON_ENGINE_ID,
            self.deployment_mode().as_db_mode(),
        )
        .await
        .map_err(|err| RuntimeLifecycleError::internal("start latest-run lookup failed", err))?;

        match latest.as_ref() {
            Some(run) => match run.status {
                mqk_db::RunStatus::Created => Ok(run.run_id),
                mqk_db::RunStatus::Stopped => {
                    let run_id = self.next_daemon_run_id(db).await?;
                    mqk_db::insert_run(
                        db,
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
                    Ok(run_id)
                }
                mqk_db::RunStatus::Halted => Err(RuntimeLifecycleError::conflict(
                    "runtime.start_refused.halted_lifecycle",
                    format!(
                        "durable run {} is halted; operator must clear the halted lifecycle before starting again",
                        run.run_id
                    ),
                )),
                mqk_db::RunStatus::Armed | mqk_db::RunStatus::Running => {
                    Err(RuntimeLifecycleError::conflict(
                        "runtime.start_refused.durable_run_active",
                        format!("durable run {} is already active", run.run_id),
                    ))
                }
            },
            None => {
                let run_id = self.next_daemon_run_id(db).await?;
                mqk_db::insert_run(
                    db,
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
                Ok(run_id)
            }
        }
    }

    pub async fn stop_execution_runtime(
        self: &Arc<Self>,
    ) -> Result<StatusSnapshot, RuntimeLifecycleError> {
        let _op = self.lifecycle_op.lock().await;
        self.reap_finished_execution_loop().await?;
        // BUNDLE-7-PHASE-7A: an operator-initiated stop immediately
        // disowns dynamic-selection authority — before any DB call below
        // that could fail, and before either the truth-mismatch conflict
        // return or the no-local-owner idle return. Idempotent no-op when
        // already `None`.
        self.clear_dynamic_selection_runtime_state().await;
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
        // BUNDLE-7-PHASE-7A: an operator-initiated halt immediately disowns
        // dynamic-selection authority — before any DB call below that could
        // fail, and before either the truth-mismatch conflict return or the
        // no-local-owner path. Idempotent no-op when already `None`.
        self.clear_dynamic_selection_runtime_state().await;

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
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2: typed autonomous arm seam.
    ///
    /// Same gate order/rules as [`Self::try_autonomous_arm`]'s doc comment
    /// above, but returns a closed [`AutonomousArmOutcome`]/
    /// [`AutonomousArmRejection`] pair instead of a free-form
    /// `Result<(), String>`, so the durable daily coordinator (D2) can
    /// classify the result exclusively by typed variant -- never by parsing
    /// rendered text. [`Self::try_autonomous_arm`] becomes a thin
    /// compatibility wrapper over this method.
    pub async fn try_autonomous_arm_typed(
        &self,
    ) -> Result<AutonomousArmOutcome, AutonomousArmRejection> {
        // Gate 1: operator halt wins unconditionally.
        // Gate 2: already armed is idempotent success.
        {
            let ig = self.integrity.read().await;
            if ig.halted {
                return Err(AutonomousArmRejection::IntegrityHalted);
            }
            if !ig.disarmed {
                return Ok(AutonomousArmOutcome::AlreadyArmed);
            }
        }

        // Gate 3: DB required to verify prior session state.
        let db = match self.db.as_ref() {
            Some(db) => db,
            None => return Err(AutonomousArmRejection::DatabaseNotConfigured),
        };

        // Gate 4/5/6: load prior arm state from the singleton row.
        let row = mqk_db::load_arm_state(db).await.map_err(|_err| {
            AutonomousArmRejection::TemporaryDatabaseOperationFailure {
                operation: "load_arm_state",
            }
        })?;

        match row {
            None => Err(AutonomousArmRejection::NoPersistedArmState),
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
                    .map_err(
                        |_err| AutonomousArmRejection::TemporaryDatabaseOperationFailure {
                            operation: "persist_arm_state_canonical",
                        },
                    )?;
                Ok(AutonomousArmOutcome::ArmedFromPersistedState)
            }
            Some((_, reason)) => Err(AutonomousArmRejection::DurableDisarmed { reason }),
        }
    }

    pub async fn try_autonomous_arm(&self) -> Result<(), String> {
        self.try_autonomous_arm_typed()
            .await
            .map(|_| ())
            .map_err(|rejection| rejection.legacy_message())
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
                                        tracing::warn!(
                                            "shutdown stop_run failed for {run_id}: {err}"
                                        );
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
        // BUNDLE-7-PHASE-7A: unlike the pre-existing `accepted_artifact`/
        // `native_strategy_bootstrap` fields (which this function does not
        // clear — a pre-existing asymmetry, not introduced here), shutdown
        // MUST clear dynamic-selection state explicitly: it runs regardless
        // of whether a local loop handle was found, and regardless of the
        // DB `stop_run` outcome above, so the process never carries stale
        // selection authority into the next boot's status truth. Idempotent
        // no-op when already `None`.
        self.clear_dynamic_selection_runtime_state().await;
    }
}

// ---------------------------------------------------------------------------
// REPAIR 3 (DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01): production
// implementation of `daily_data_readiness::RuntimeStartEffects`.
//
// Wraps the exact runtime-construction/arm/begin/tick/heartbeat/snapshot/
// counter-reset/provenance-capture/bootstrap-storage/spawn sequence
// `start_execution_runtime` previously performed inline, unchanged in
// substance — only relocated so the identical trait the synthetic lifecycle
// proof test implements can be driven by `daily_data_readiness::
// advance_run_to_active` in production too.
// ---------------------------------------------------------------------------

struct ProductionRuntimeStartEffects<'a> {
    state: &'a Arc<AppState>,
    db: PgPool,
    /// Consumed exactly once by `start_runtime_effects` (TV-01C provenance
    /// capture) — `std::sync::Mutex` for interior mutability behind `&self`;
    /// never held across an `.await`.
    artifact_intake: std::sync::Mutex<Option<ArtifactIntakeOutcome>>,
    /// Consumed exactly once by `start_runtime_effects` (B1A bootstrap
    /// storage).
    native_strategy_bootstrap: std::sync::Mutex<Option<NativeStrategyBootstrap>>,
    /// Populated by `start_runtime_effects`, consumed by `spawn_loop`.
    orchestrator: std::sync::Mutex<Option<DaemonOrchestrator>>,
    /// BUNDLE-7-PHASE-7A: the already-evaluated, frozen dynamic-selection
    /// outcome for this run, consumed exactly once by `start_runtime_effects`
    /// (committed to `AppState` near the end, alongside
    /// `native_strategy_bootstrap`).
    dynamic_selection_outcome: std::sync::Mutex<Option<DynamicSelectionRuntimeState>>,
}

#[async_trait::async_trait]
impl crate::daily_data_readiness::RuntimeStartEffects for ProductionRuntimeStartEffects<'_> {
    async fn start_runtime_effects(
        &self,
        run_id: uuid::Uuid,
    ) -> Result<(), crate::daily_data_readiness::RuntimeStartEffectsError> {
        use crate::daily_data_readiness::RuntimeStartEffectsError;

        let mut orchestrator = self
            .state
            .build_execution_orchestrator(self.db.clone(), run_id)
            .await
            .map_err(|err| {
                let fault_class = err.fault_class();
                let message = err.to_string();
                match err {
                    RuntimeLifecycleError::Conflict { .. } => {
                        RuntimeStartEffectsError::conflict(fault_class, message)
                    }
                    _ => RuntimeStartEffectsError::internal(fault_class, message),
                }
            })?;

        // BUNDLE-7-PHASE-7A fault seam: after orchestrator construction,
        // before arm/begin/tick. No dynamic-selection state has been
        // committed yet — release the freshly-acquired lease and return;
        // nothing else to clean up.
        if self
            .state
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::AfterOrchestratorConstruction,
            )
            .await
        {
            if let Err(rel_err) = orchestrator.release_runtime_leadership().await {
                tracing::warn!(
                    "runtime_lease_release_failed_on_dynamic_selection_fault_seam error={rel_err}"
                );
            }
            return Err(RuntimeStartEffectsError::internal(
                "dynamic_selection.fault_seam.after_orchestrator_construction",
                "test-injected fault after orchestrator construction",
            ));
        }

        if let Err(err) = mqk_db::arm_run(&self.db, run_id).await {
            if let Err(rel_err) = orchestrator.release_runtime_leadership().await {
                tracing::warn!("runtime_lease_release_failed_on_arm_rollback error={rel_err}");
            }
            return Err(RuntimeStartEffectsError::internal(
                "start arm_run failed",
                err.to_string(),
            ));
        }
        if let Err(err) = mqk_db::begin_run(&self.db, run_id).await {
            if let Err(rel_err) = orchestrator.release_runtime_leadership().await {
                tracing::warn!("runtime_lease_release_failed_on_begin_rollback error={rel_err}");
            }
            return Err(RuntimeStartEffectsError::internal(
                "start begin_run failed",
                err.to_string(),
            ));
        }
        if let Err(err) = mqk_db::heartbeat_run(&self.db, run_id, Utc::now()).await {
            if let Err(rel_err) = orchestrator.release_runtime_leadership().await {
                tracing::warn!(
                    "runtime_lease_release_failed_on_heartbeat_rollback error={rel_err}"
                );
            }
            return Err(RuntimeStartEffectsError::internal(
                "start initial heartbeat failed",
                err.to_string(),
            ));
        }
        if let Err(err) = orchestrator.tick().await {
            let message = err.to_string();
            if let Err(rel_err) = orchestrator.release_runtime_leadership().await {
                tracing::warn!("runtime_lease_release_failed_on_tick_rollback error={rel_err}");
            }
            if message.contains("RUNTIME_LEASE") {
                return Err(RuntimeStartEffectsError::conflict(
                    "runtime.start_refused.service_unavailable",
                    format!("runtime leader lease unavailable: {message}"),
                ));
            }
            return Err(RuntimeStartEffectsError::internal(
                "start initial tick failed",
                message,
            ));
        }

        // DEADMAN-EXPIRED-AFTER-START-01: refresh heartbeat after the initial
        // tick.  orchestrator.tick() may block for tens of seconds (Alpaca
        // fetch_events has no HTTP timeout).  The heartbeat written above can
        // be stale by the time tick() returns; the execution loop's first
        // pre-tick deadman check would then fire immediately.  A fresh
        // heartbeat here ensures the loop starts with a current timestamp.
        if let Err(err) = mqk_db::heartbeat_run(&self.db, run_id, Utc::now()).await {
            if let Err(rel_err) = orchestrator.release_runtime_leadership().await {
                tracing::warn!(
                    "runtime_lease_release_failed_on_post_tick_heartbeat error={rel_err}"
                );
            }
            return Err(RuntimeStartEffectsError::internal(
                "post-initial-tick heartbeat refresh failed",
                err.to_string(),
            ));
        }

        // BUNDLE-7-PHASE-7A fault seam: after run arm/begin/initial tick
        // (and the post-tick heartbeat refresh), before any counter
        // reset/snapshot/provenance/bootstrap/selection commit. No
        // dynamic-selection state has been committed yet.
        if self
            .state
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::AfterRunArmBeginInitialTick,
            )
            .await
        {
            if let Err(rel_err) = orchestrator.release_runtime_leadership().await {
                tracing::warn!(
                    "runtime_lease_release_failed_on_dynamic_selection_fault_seam error={rel_err}"
                );
            }
            return Err(RuntimeStartEffectsError::internal(
                "dynamic_selection.fault_seam.after_run_arm_begin_initial_tick",
                "test-injected fault after run arm/begin/initial tick",
            ));
        }

        if let Ok(initial_snapshot) = orchestrator.snapshot().await {
            *self.state.execution_snapshot.write().await = Some(initial_snapshot);
        }

        // PT-AUTO-02: reset per-run signal intake counter at each new start so
        // the bound applies per execution run, not per daemon process lifetime.
        self.state.day_signal_count.store(0, Ordering::SeqCst);
        // MULTI-SYMBOL-DAY-ORDER-CAP-01: reset per-symbol order intake counters
        // (cap #4) at the same run-start boundary as day_signal_count.
        self.state.reset_symbol_day_order_counts().await;
        // DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: reset dedup state so each new
        // run gets fresh B5 and day-limit alert windows.
        self.state.reset_signal_blocked_alert_state();
        // AUTON-NO-TRADE-01: reset bar-tick counters alongside signal counter.
        self.state.reset_bar_tick_counters();
        // PER-SYMBOL-TARGET-STATE-01: clear observability-only target state at
        // the same run-start boundary as other in-memory per-run tracking.
        self.state.clear_per_symbol_target_states().await;

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
            let artifact_intake = self
                .artifact_intake
                .lock()
                .expect("artifact_intake mutex poisoned")
                .take()
                .expect("start_runtime_effects must be called at most once");
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
            *self.state.accepted_artifact.write().await = provenance;
        }

        // B1A: store native strategy bootstrap for the active run.
        // Placed after all DB operations and the initial tick succeed so the
        // field is only populated when the run is fully live.
        //
        // The `.take()` is hoisted into its own statement (rather than
        // inline in the `if let` scrutinee) so the `std::sync::MutexGuard`
        // temporary is dropped before the `.await` below — a guard alive
        // across an `if let` scrutinee is kept alive for the whole block by
        // Rust's temporary-lifetime rules, which would make this function's
        // returned future `!Send`.
        let bootstrap_to_store = self
            .native_strategy_bootstrap
            .lock()
            .expect("native_strategy_bootstrap mutex poisoned")
            .take();
        if let Some(bootstrap) = bootstrap_to_store {
            *self.state.native_strategy_bootstrap.lock().await = Some(bootstrap);
        }

        // BUNDLE-7-PHASE-7A: commit the already-evaluated, frozen
        // dynamic-selection outcome now — after every other run-start
        // effect above has succeeded, immediately before this run is handed
        // to `spawn_loop`. From this point on a spawn conflict/failure MUST
        // clear what was just committed (see the fault seam and the
        // ownership-conflict branch in `spawn_loop` below) — this function
        // deliberately never leaves committed selection state paired with
        // no local loop owner.
        //
        // The `.take()` is hoisted for the same reason as
        // `bootstrap_to_store` above: keep the `std::sync::MutexGuard`
        // temporary from living across the `.await` in
        // `commit_dynamic_selection_runtime_state`.
        let dynamic_selection_outcome_to_commit = self
            .dynamic_selection_outcome
            .lock()
            .expect("dynamic_selection_outcome mutex poisoned")
            .take();
        if let Some(outcome) = dynamic_selection_outcome_to_commit {
            self.state
                .commit_dynamic_selection_runtime_state(outcome)
                .await;
        }

        // BUNDLE-7-PHASE-7A fault seam: immediately after the process-local
        // selection commit above, before this function returns `Ok(())`.
        // Selection state IS committed at this point — clear it before
        // propagating the failure, so no observer can see committed
        // selection state with no corresponding local loop owner.
        if self
            .state
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::AfterProcessLocalSelectionCommit,
            )
            .await
        {
            self.state.clear_dynamic_selection_runtime_state().await;
            if let Err(rel_err) = orchestrator.release_runtime_leadership().await {
                tracing::warn!(
                    "runtime_lease_release_failed_on_dynamic_selection_fault_seam error={rel_err}"
                );
            }
            return Err(RuntimeStartEffectsError::internal(
                "dynamic_selection.fault_seam.after_process_local_selection_commit",
                "test-injected fault after process-local selection commit",
            ));
        }

        *self
            .orchestrator
            .lock()
            .expect("orchestrator mutex poisoned") = Some(orchestrator);
        Ok(())
    }

    async fn spawn_loop(
        &self,
        run_id: uuid::Uuid,
    ) -> Result<(), crate::daily_data_readiness::RuntimeStartEffectsError> {
        use crate::daily_data_readiness::RuntimeStartEffectsError;

        // BUNDLE-7-PHASE-7A fault seam: immediately before loop spawn.
        // Selection state was already committed by `start_runtime_effects`
        // above — clear it before propagating the failure so no observer
        // can see committed selection state with no local loop owner.
        if self
            .state
            .dynamic_selection_fault_seam_is(
                DynamicSelectionLifecycleFaultSeam::ImmediatelyBeforeLoopSpawn,
            )
            .await
        {
            self.state.clear_dynamic_selection_runtime_state().await;
            return Err(RuntimeStartEffectsError::internal(
                "dynamic_selection.fault_seam.immediately_before_loop_spawn",
                "test-injected fault immediately before loop spawn",
            ));
        }

        let orchestrator = self
            .orchestrator
            .lock()
            .expect("orchestrator mutex poisoned")
            .take()
            .expect("spawn_loop must be called only after start_runtime_effects succeeds");
        let handle = spawn_execution_loop(Arc::clone(self.state), orchestrator, run_id);
        let mut lock = self.state.execution_loop.lock().await;
        if lock.is_some() {
            // BUNDLE-7-PHASE-7A: real loop-ownership conflict — selection
            // state was already committed above; a duplicate loop is being
            // refused, so no local loop will ever own this commit. Clear it
            // (never leave committed selection state paired with no owner).
            self.state.clear_dynamic_selection_runtime_state().await;
            return Err(RuntimeStartEffectsError::conflict(
                "runtime.start_refused.local_ownership_conflict",
                "runtime ownership changed while starting; refusing duplicate loop",
            ));
        }
        *lock = Some(handle);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: `build_dynamic_selection_
// start_snapshot` disposition-determination unit proof.
//
// These exercise the private method directly (in-crate, same module tree),
// deliberately bypassing every earlier unrelated start gate (WS continuity,
// daily-data-readiness, artifact intake, capital policy) that a full
// `start_execution_runtime()` call would also have to satisfy for
// Paper+Alpaca — none of which this patch touches or needs to re-prove.
// None of these tests require a database connection or Alpaca credentials:
// `build_dynamic_selection_start_snapshot` never constructs a broker.
//
// Full end-to-end lifecycle wiring (atomic commit, fault seams, cleanup on
// every exit path, real loop-ownership-conflict cleanup) is proven
// separately in `tests/scenario_bundle7_phase7a_lifecycle_wiring_01.rs`
// against Paper+Paper (Off disposition, zero Alpaca dependency) using only
// the public API.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod dynamic_selection_start_snapshot_tests {
    use super::*;
    use std::sync::OnceLock;
    use tokio::sync::Mutex as TokioMutex;

    /// Serializes every test in this module that touches the process-global
    /// `MQK_STRATEGY_SYMBOL` / `MQK_STRATEGY_IDS` / `MQK_STRATEGY_MD_TIMEFRAME`
    /// / `MQK_DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE` env vars — mirrors the
    /// `env_lock()` convention already used by the daily-data-readiness
    /// start-gate scenario tests. Scoped to this compiled test binary
    /// (`cargo test -p mqk-daemon --lib`) only.
    fn env_lock() -> &'static TokioMutex<()> {
        static LOCK: OnceLock<TokioMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| TokioMutex::new(()))
    }

    fn clear_dynamic_selection_env() {
        std::env::remove_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
        );
        std::env::remove_var("MQK_STRATEGY_SYMBOL");
        std::env::remove_var("MQK_STRATEGY_IDS");
        std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
    }

    /// Off: broker=Paper (never Alpaca) live-locks to Off regardless of the
    /// configured mode env var — proves the live lock AND that Off requires
    /// no DB and no `MQK_STRATEGY_SYMBOL`/`MQK_STRATEGY_IDS` configuration
    /// (zero selection I/O).
    #[tokio::test]
    async fn off_via_live_lock_needs_no_db_and_no_symbol_config() {
        let _guard = env_lock().lock().await;
        clear_dynamic_selection_env();
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "shadow",
        );

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Paper,
        ));
        assert!(state.db.is_none(), "precondition: no DB configured");

        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.off_live_lock",
        );
        let result = state.build_dynamic_selection_start_snapshot(run_id).await;
        clear_dynamic_selection_env();

        let outcome = result.expect("Off must never refuse the start");
        assert_eq!(
            outcome.disposition,
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::Off
        );
        assert_eq!(
            outcome.configured_mode,
            mqk_portfolio::DynamicSelectionMode::Shadow,
            "configured_mode must reflect the raw env value, even though it was demoted"
        );
        assert_eq!(
            outcome.effective_mode,
            mqk_portfolio::DynamicSelectionMode::Off
        );
        assert!(
            outcome.live_lock_applied,
            "broker=Paper must trigger the live lock"
        );
        assert!(outcome.plan.is_none());
        assert!(outcome.selected_pairs.is_empty());
        assert!(outcome.host_pool.is_none());
        assert!(outcome.reasons.is_empty());
        assert!(!outcome.approved_for_live);
        assert_eq!(outcome.run_id, run_id);
    }

    /// Off: mode env var unset (the honest default) on Paper+Alpaca — Off
    /// without any live-lock demotion (`live_lock_applied` must be `false`
    /// since `configured_mode` was already `Off`, not forced there).
    #[tokio::test]
    async fn off_by_default_configuration_is_not_live_locked() {
        let _guard = env_lock().lock().await;
        clear_dynamic_selection_env();

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.off_default",
        );
        let outcome = state
            .build_dynamic_selection_start_snapshot(run_id)
            .await
            .expect("Off must never refuse the start");

        assert_eq!(
            outcome.disposition,
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::Off
        );
        assert_eq!(
            outcome.configured_mode,
            mqk_portfolio::DynamicSelectionMode::Off
        );
        assert!(!outcome.live_lock_applied);
    }

    /// Shadow, `MultiSymbolRuntimeConfig` resolution itself fails
    /// (`MQK_STRATEGY_SYMBOL` unset) — Shadow must never block the run: the
    /// truthful failure is recorded as `ShadowInvalid` with no plan/pool.
    #[tokio::test]
    async fn shadow_with_unresolvable_config_is_invalid_not_blocking() {
        let _guard = env_lock().lock().await;
        clear_dynamic_selection_env();
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "shadow",
        );

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.shadow_no_config",
        );
        let result = state.build_dynamic_selection_start_snapshot(run_id).await;
        clear_dynamic_selection_env();

        let outcome =
            result.expect("Shadow must never refuse the start, even on its own config failure");
        assert_eq!(
            outcome.disposition,
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::ShadowInvalid
        );
        assert!(outcome.plan.is_none());
        assert!(outcome.host_pool.is_none());
        assert!(outcome.selected_pairs.is_empty());
        assert_eq!(outcome.reasons.len(), 1);
        assert!(matches!(
            &outcome.reasons[0],
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateReason::PlanInvalid { truth_state }
                if truth_state.starts_with("multi_symbol_config_unavailable:")
        ));
    }

    /// PaperEnforced, `MultiSymbolRuntimeConfig` resolution fails — must
    /// refuse the whole start (`Err`), before any run advancement.
    #[tokio::test]
    async fn paper_enforced_with_unresolvable_config_is_refused() {
        let _guard = env_lock().lock().await;
        clear_dynamic_selection_env();
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "paper_enforced",
        );

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.paper_enforced_no_config",
        );
        let result = state.build_dynamic_selection_start_snapshot(run_id).await;
        clear_dynamic_selection_env();

        let err = result.expect_err("PaperEnforced must refuse when config cannot be resolved");
        assert_eq!(
            err.fault_class(),
            "runtime.start_refused.dynamic_selection_config_unavailable"
        );
    }

    /// Shadow, `MultiSymbolRuntimeConfig` resolves successfully but no DB is
    /// configured — the evaluator's own `DbUnavailable` path fires
    /// (distinct from this function's own config-resolution failure above).
    #[tokio::test]
    async fn shadow_with_resolved_config_and_no_db_is_invalid_via_db_unavailable() {
        let _guard = env_lock().lock().await;
        clear_dynamic_selection_env();
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "shadow",
        );
        std::env::set_var("MQK_STRATEGY_SYMBOL", "AAPL");
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        assert!(state.db.is_none(), "precondition: no DB configured");
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.shadow_db_unavail",
        );
        let result = state.build_dynamic_selection_start_snapshot(run_id).await;
        clear_dynamic_selection_env();

        let outcome = result.expect("Shadow must never refuse the start");
        assert_eq!(
            outcome.disposition,
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::ShadowInvalid
        );
        assert!(outcome.plan.is_none());
        assert!(outcome.host_pool.is_none());
        assert_eq!(
            outcome.reasons,
            vec![
                crate::dynamic_selection_start_gate::DynamicSelectionStartGateReason::DbUnavailable
            ]
        );
    }

    /// PaperEnforced, config resolves but no DB — the evaluator's own
    /// `DbUnavailable` refusal fires (distinct from this function's own
    /// config-resolution refusal above).
    #[tokio::test]
    async fn paper_enforced_with_resolved_config_and_no_db_is_refused_via_db_unavailable() {
        let _guard = env_lock().lock().await;
        clear_dynamic_selection_env();
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "paper_enforced",
        );
        std::env::set_var("MQK_STRATEGY_SYMBOL", "AAPL");
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.paper_enforced_db_unavail",
        );
        let result = state.build_dynamic_selection_start_snapshot(run_id).await;
        clear_dynamic_selection_env();

        let err = result.expect_err("PaperEnforced must refuse when DB is unavailable");
        assert_eq!(
            err.fault_class(),
            "runtime.start_refused.dynamic_selection_paper_enforced_refused"
        );
    }

    /// Live deployments resolve Off regardless of configured mode, and
    /// cannot store a pool (Off never builds one) — proves the live lock for
    /// the `LiveCapital` boundary specifically, not just non-Alpaca brokers.
    #[tokio::test]
    async fn live_capital_resolves_off_and_stores_no_pool() {
        let _guard = env_lock().lock().await;
        clear_dynamic_selection_env();
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "paper_enforced",
        );

        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::LiveCapital,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.live_capital_off",
        );
        let result = state.build_dynamic_selection_start_snapshot(run_id).await;
        clear_dynamic_selection_env();

        let outcome = result.expect("Off must never refuse the start");
        assert_eq!(
            outcome.disposition,
            crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::Off
        );
        assert!(outcome.live_lock_applied);
        assert!(outcome.host_pool.is_none());
        assert!(outcome.plan.is_none());
    }
}

// ---------------------------------------------------------------------------
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: cleanup-contract proof.
//
// `commit_dynamic_selection_runtime_state` is `pub(crate)`, so these must be
// in-crate tests. They deliberately bypass the credential-gated start path
// entirely (see `tests/scenario_bundle7_phase7a_lifecycle_wiring_01.rs` for
// why a real `start_execution_runtime()` success cannot be driven here
// without Alpaca credentials this patch must not load): a fixture
// `DynamicSelectionRuntimeState` is committed directly, then the real public
// `stop_execution_runtime`/`halt_execution_runtime`/`stop_for_shutdown`/
// `reap_finished_execution_loop` functions are exercised to prove they clear
// it — genuine proof of the cleanup wiring this patch adds, independent of
// whichever disposition produced the committed state.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod dynamic_selection_cleanup_contract_tests {
    use super::*;

    fn fixture_off_state(run_id: uuid::Uuid) -> DynamicSelectionRuntimeState {
        DynamicSelectionRuntimeState {
            run_id,
            disposition:
                crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::Off,
            configured_mode: mqk_portfolio::DynamicSelectionMode::Off,
            effective_mode: mqk_portfolio::DynamicSelectionMode::Off,
            live_lock_applied: false,
            plan: None,
            selected_pairs: Vec::new(),
            host_pool: None,
            reasons: Vec::new(),
            approved_for_live: false,
        }
    }

    /// `stop_execution_runtime` clears committed dynamic-selection state
    /// even with no active local loop and no DB configured (the "idle,
    /// nothing to stop" path).
    #[tokio::test]
    async fn stop_clears_committed_state_with_no_active_loop_and_no_db() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.cleanup.stop",
        );
        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_id))
            .await;
        assert!(state.dynamic_selection_runtime_snapshot().await.is_some());

        state
            .stop_execution_runtime()
            .await
            .expect("stop with no active loop and no DB must succeed (idle status)");
        assert!(
            state.dynamic_selection_runtime_snapshot().await.is_none(),
            "stop_execution_runtime must clear dynamic-selection state"
        );
    }

    /// `halt_execution_runtime` clears committed dynamic-selection state
    /// *even when the overall call itself errors* on a missing DB — the
    /// clear is placed before `db_pool()?` deliberately, so local
    /// dynamic-selection authority is disowned the instant an operator halts,
    /// independent of whether the DB bookkeeping step later succeeds.
    #[tokio::test]
    async fn halt_clears_committed_state_even_when_the_call_itself_errors() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.cleanup.halt",
        );
        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_id))
            .await;
        assert!(state.dynamic_selection_runtime_snapshot().await.is_some());

        let err = state
            .halt_execution_runtime()
            .await
            .expect_err("halt without a configured DB must error at db_pool()");
        assert_eq!(
            err.fault_class(),
            "runtime.start_refused.service_unavailable"
        );
        assert!(
            state.dynamic_selection_runtime_snapshot().await.is_none(),
            "halt_execution_runtime must clear dynamic-selection state before the DB-dependent \
             steps that can fail, not only on a fully successful halt"
        );
    }

    /// `stop_for_shutdown` clears committed dynamic-selection state even
    /// with no active local loop — unlike the pre-existing
    /// `accepted_artifact`/`native_strategy_bootstrap` fields (which this
    /// function does not clear at all), this is new, required behavior.
    #[tokio::test]
    async fn stop_for_shutdown_clears_committed_state_with_no_active_loop() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.cleanup.shutdown",
        );
        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_id))
            .await;

        state.stop_for_shutdown().await;
        assert!(
            state.dynamic_selection_runtime_snapshot().await.is_none(),
            "stop_for_shutdown must clear dynamic-selection state"
        );
    }

    /// Cleanup is idempotent: clearing twice (or clearing when already
    /// `None`) never panics and always leaves `None`.
    #[tokio::test]
    async fn clear_is_idempotent() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        state.clear_dynamic_selection_runtime_state().await;
        assert!(state.dynamic_selection_runtime_snapshot().await.is_none());

        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.cleanup.idempotent",
        );
        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_id))
            .await;
        state.clear_dynamic_selection_runtime_state().await;
        state.clear_dynamic_selection_runtime_state().await;
        assert!(state.dynamic_selection_runtime_snapshot().await.is_none());
    }

    /// A fresh commit is always a full overwrite, never a merge with a
    /// stale prior value — proves restart-cannot-reuse-stale-state at the
    /// container level, independent of run-creation machinery.
    #[tokio::test]
    async fn commit_always_overwrites_never_merges() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_a = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.cleanup.run_a",
        );
        let run_b = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.cleanup.run_b",
        );

        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_a))
            .await;
        assert_eq!(
            state
                .dynamic_selection_runtime_snapshot()
                .await
                .unwrap()
                .run_id,
            run_a
        );

        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_b))
            .await;
        let after = state.dynamic_selection_runtime_snapshot().await.unwrap();
        assert_eq!(
            after.run_id, run_b,
            "the second commit must fully replace the first"
        );
    }

    /// `reap_finished_execution_loop` clears committed dynamic-selection
    /// state when it reaps a loop that finished on its own (crash/supervisor
    /// exit), independent of `stop_execution_runtime`/`halt_execution_runtime`
    /// ever being called.
    #[tokio::test]
    async fn reap_of_a_finished_loop_clears_committed_state() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.phase7a.cleanup.reap",
        );
        state
            .commit_dynamic_selection_runtime_state(fixture_off_state(run_id))
            .await;

        let (stop_tx, _stop_rx) = tokio::sync::watch::channel(ExecutionLoopCommand::Run);
        let join_handle =
            tokio::spawn(async { crate::state::types::ExecutionLoopExit { note: None } });
        // Let the trivial task actually finish before installing it, so
        // `reap_finished_execution_loop`'s `is_finished()` check takes the
        // "already finished" branch deterministically.
        while !join_handle.is_finished() {
            tokio::task::yield_now().await;
        }
        let handle = crate::state::types::ExecutionLoopHandle {
            run_id,
            stop_tx,
            join_handle,
        };
        *state.execution_loop.lock().await = Some(handle);

        let exit = state
            .reap_finished_execution_loop()
            .await
            .expect("reap must not error");
        assert!(exit.is_some(), "reap must observe the finished loop");
        assert!(
            state.dynamic_selection_runtime_snapshot().await.is_none(),
            "reap_finished_execution_loop must clear dynamic-selection state \
             for a loop that finished on its own"
        );
    }

    /// The fault-seam get/set primitive round-trips correctly and defaults
    /// to `None` (no seam installed) — the invariant every fault-seam check
    /// inside `start_execution_runtime`/`ProductionRuntimeStartEffects`
    /// depends on.
    #[tokio::test]
    async fn fault_seam_primitive_round_trips_and_defaults_to_none() {
        let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ));
        assert!(
            !state
                .dynamic_selection_fault_seam_is(
                    DynamicSelectionLifecycleFaultSeam::AfterRunRowCreation
                )
                .await,
            "a fresh state must have no fault seam installed"
        );

        state
            .set_dynamic_selection_fault_seam_for_test(Some(
                DynamicSelectionLifecycleFaultSeam::AfterRunRowCreation,
            ))
            .await;
        assert!(
            state
                .dynamic_selection_fault_seam_is(
                    DynamicSelectionLifecycleFaultSeam::AfterRunRowCreation
                )
                .await
        );
        assert!(
            !state
                .dynamic_selection_fault_seam_is(
                    DynamicSelectionLifecycleFaultSeam::ImmediatelyBeforeLoopSpawn
                )
                .await,
            "a different seam variant must not match"
        );

        state.set_dynamic_selection_fault_seam_for_test(None).await;
        assert!(
            !state
                .dynamic_selection_fault_seam_is(
                    DynamicSelectionLifecycleFaultSeam::AfterRunRowCreation
                )
                .await,
            "clearing the seam must be respected"
        );
    }
}
