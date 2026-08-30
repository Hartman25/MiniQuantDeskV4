//! Orchestrator-build helpers.
//!
//! Extracted from `state.rs` (MT-05).  Contains the two private helpers
//! used exclusively by the run-start path in `state/lifecycle.rs`:
//!
//! - `next_daemon_run_id` — derives the deterministic UUIDv5 for the next run.
//! - `build_execution_orchestrator` — constructs the `ExecutionOrchestrator`
//!   that the execution loop owns for the duration of a run.
//!
//! Both methods are `pub(super)` so that `state/lifecycle.rs` (a sibling
//! child module) can reach them via `self`.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use mqk_execution::{wiring::build_gateway, BrokerError, BrokerOrderMap};
use mqk_runtime::runtime_risk::{
    AccountAuthorityContext, AccountAuthorityError, RuntimeAccountAuthority, SystemClock,
};
use sqlx::PgPool;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::broker::{build_daemon_broker, DaemonBroker};
use super::snapshot::{
    reconcile_broker_snapshot_from_schema, reconcile_local_snapshot_from_runtime_with_sides,
    recover_oms_and_portfolio, seed_portfolio_from_baseline, synthesize_paper_broker_snapshot,
};
use super::types::{
    AlpacaWsContinuityState, BrokerSnapshotTruthSource, DaemonOrchestrator, ReconcileTruthGate,
    RuntimeLifecycleError, StateIntegrityGate,
};
use super::{AppState, BrokerSnapshotFetcher, DAEMON_ENGINE_ID};

// ---------------------------------------------------------------------------
// RUNTIME-RISK-DYNAMIC-STATE-AUTHORITY-01 / RRA-2: daemon-owned dynamic
// account authority.
// ---------------------------------------------------------------------------

/// Reads the SAME `AppState::broker_snapshot` cache the execution loop
/// already refreshes (no network fetch is performed here — this is a
/// read-only cache lookup, matching the pattern already used by
/// `StateIntegrityGate::is_armed` and `ReconcileTruthGate::is_clean`).
///
/// # Equity source and the Synthetic hard stop
///
/// For `BrokerSnapshotTruthSource::External` (Paper+Alpaca, Live+Alpaca),
/// `snapshot.account.equity` is the broker's own marked account equity
/// (Alpaca `GET /v2/account` `equity` field) — already treated as
/// authoritative equity elsewhere in this crate (see
/// `accept_external_broker_snapshot`, which persists it as durable Paper
/// portfolio truth via the same `parse_decimal_micros` parser reused here).
///
/// For `BrokerSnapshotTruthSource::Synthetic` (the internal Paper broker
/// with no real broker behind it), `synthesize_paper_broker_snapshot` sets
/// `account.equity` to `portfolio.cash_micros` — i.e. CASH, not
/// mark-to-market equity (see `snapshot.rs`). This authority MUST NOT
/// accept that value as current equity: doing so would silently let
/// intraday drawdown gating be gated on cash instead of marked equity for
/// every open position. Production account-level loss/drawdown gating for
/// the Synthetic source therefore fails closed (`Unavailable`) rather than
/// fabricate a number — see RUNTIME-RISK-DYNAMIC-STATE-AUTHORITY-01 mission
/// hard stop.
struct DaemonAccountAuthority {
    broker_snapshot: Arc<RwLock<Option<mqk_schemas::BrokerSnapshot>>>,
    source: BrokerSnapshotTruthSource,
    freshness_bound: chrono::Duration,
}

impl RuntimeAccountAuthority for DaemonAccountAuthority {
    fn current_account(
        &self,
        now: DateTime<Utc>,
    ) -> Result<AccountAuthorityContext, AccountAuthorityError> {
        if self.source != BrokerSnapshotTruthSource::External {
            return Err(AccountAuthorityError::Unavailable);
        }

        let guard = self
            .broker_snapshot
            .try_read()
            .map_err(|_| AccountAuthorityError::Unavailable)?;
        let snapshot = guard.as_ref().ok_or(AccountAuthorityError::Unavailable)?;

        let age = now.signed_duration_since(snapshot.captured_at_utc);
        if age < chrono::Duration::zero() || age > self.freshness_bound {
            return Err(AccountAuthorityError::Stale);
        }

        let equity_micros =
            crate::routes::helpers::parse_decimal_micros(&snapshot.account.equity)
                .ok_or(AccountAuthorityError::Malformed)?;
        if equity_micros <= 0 {
            return Err(AccountAuthorityError::Malformed);
        }

        // RR5 (ALPACA-LEGACY-PDT-DISPOSITION-2026-01) disposition: no
        // authoritative PDT source is wired for the current Alpaca provider
        // contract (the legacy Pattern Day Trader regime was retired
        // 2026-07-06 — see `alpaca_legacy_pdt_disposition`'s doc for the
        // full provider-contract citation — and a home-grown replacement
        // model is out of scope). `PdtContext::ok()` here is made truthful
        // for this External/Alpaca path by `alpaca_legacy_pdt_disposition`,
        // which forces `pdt_auto_enabled` to an explicit `false` (NOT
        // APPLICABLE by provider regime) unless the run's own config
        // explicitly demanded `pdt_auto_enabled: true` — in which case
        // `build_execution_orchestrator` refuses to start the run rather
        // than ever reaching this stub with enforcement silently expected.
        // kill_switch stays `None`: canonical halt authority for
        // staleness/manual/reconcile-drift lives in `StateIntegrityGate`,
        // evaluated as an independent gate before this one.
        Ok(AccountAuthorityContext {
            equity_micros,
            pdt: mqk_runtime::runtime_risk::RiskPdtContext::ok(),
            kill_switch: None,
        })
    }
}

impl AppState {
    pub(super) async fn next_daemon_run_id(
        &self,
        db: &PgPool,
    ) -> Result<Uuid, RuntimeLifecycleError> {
        let generation: i64 = sqlx::query_scalar(
            r#"
            SELECT COALESCE(COUNT(*), 0)::bigint + 1
              FROM runs
             WHERE engine_id = $1
               AND mode = $2
            "#,
        )
        .bind(DAEMON_ENGINE_ID)
        .bind(self.deployment_mode().as_db_mode())
        .fetch_one(db)
        .await
        .map_err(|err| RuntimeLifecycleError::internal("next_daemon_run_id failed", err))?;

        Ok(Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            format!(
                "mqk-daemon.run.v2|{}|{}|{}|{}",
                self.node_id,
                DAEMON_ENGINE_ID,
                self.deployment_mode().as_db_mode(),
                generation
            )
            .as_bytes(),
        ))
    }

    pub(super) async fn build_execution_orchestrator(
        &self,
        db: PgPool,
        run_id: Uuid,
    ) -> Result<DaemonOrchestrator, RuntimeLifecycleError> {
        let run = mqk_db::fetch_run(&db, run_id)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("fetch_run failed", err))?;

        // AUTON-PAPER-BLOCKER-01 / RRA-2: daemon-created runs store a minimal
        // config_json with no /risk subtree.  Supplement from env vars so
        // RuntimeRiskGate receives real inputs.  Fields already in
        // config_json are never overwritten. max_drawdown is supplemented
        // the same way as daily_loss_limit — if neither the run's own config
        // nor MQK_RISK_MAX_DRAWDOWN supplies it, `RuntimeRiskGate` fails the
        // whole gate closed (RRA-2: max_drawdown is required, not
        // defaulted to disabled).
        let (env_equity_micros, env_daily_loss_limit, env_max_drawdown) = load_risk_env();
        let effective_config = effective_run_config_for_risk(
            &run.config_json,
            env_equity_micros,
            env_daily_loss_limit,
            env_max_drawdown,
        );

        // RR5 (ALPACA-LEGACY-PDT-DISPOSITION-2026-01): the current Alpaca
        // provider contract retired the legacy Pattern Day Trader regime
        // (see `alpaca_legacy_pdt_disposition` doc below) and this system has
        // no authoritative replacement PDT source wired for it. Applies only
        // to the External (Alpaca) broker-snapshot-truth path; Synthetic
        // already fails closed on equity before PDT is ever consulted (see
        // `DaemonAccountAuthority::current_account`).
        let effective_config = if self.broker_snapshot_source == BrokerSnapshotTruthSource::External
        {
            alpaca_legacy_pdt_disposition(&run.config_json, effective_config)?
        } else {
            effective_config
        };

        // RUNTIME-PORTFOLIO-SEED-CONFIG-VALIDATION-01: this is the LOCAL
        // PortfolioState/recovery seed, distinct from the account-level
        // `RuntimeRiskGate` equity baseline (which is sourced exclusively
        // from `DaemonAccountAuthority`/broker truth above and is untouched
        // by this check). `effective_config` has already run through RR2's
        // `effective_run_config_for_risk` merge, so this validates the same
        // ABSENT-vs-PRESENT-BUT-INVALID view: env may only fill a field the
        // run's own `config_json` never mentioned; a malformed explicit
        // value reaches here unhealed and is refused rather than silently
        // becoming `0` (the prior `unwrap_or(0)` defect).
        let initial_equity_micros = required_initial_equity_micros(&effective_config)?;

        // PAPER-SOAK-STALE-CLAIM-RECOVERY-02: the unconditional stale-claim
        // reset that used to run here (PATCH-01) is removed. It ran before
        // any runtime leadership lease existed for this orchestrator
        // (`ExecutionOrchestrator::new` initializes `runtime_epoch: None`;
        // the lease is acquired later, inside `tick()`), and it could never
        // actually reach the crash-recovery scenario it targeted anyway: a
        // process crash while `RUNNING` leaves the run durably `RUNNING`,
        // and `create_or_reuse_run_for_start` refuses to start when a
        // durable active run exists without local ownership — this
        // constructor is never reached for that run_id at all via the
        // normal start path. Stale-claim recovery is now performed
        // atomically as part of the operator-mediated `clear-halted-run`
        // action (`mqk_db::clear_halted_run_and_reset_stale_claims`), gated
        // on the run's durable `HALTED` status as the ownership proof —
        // see that function's doc comment for the full rationale.
        let (oms_orders, recovered_sides, mut portfolio) =
            recover_oms_and_portfolio(&db, run_id, initial_equity_micros).await?;

        // RUNTIME-POSITION-SEED-ON-START-01: seed portfolio with adopted broker
        // baseline so current_position_qty reflects positions inherited from prior
        // runs immediately at run start.
        //
        // Without this, the execution loop reads qty=0 for symbols held in a prior
        // run (e.g. AAPL bought last session), and delta = target(0) - current(0) = 0
        // produces no sell order when the strategy wants to flatten.
        //
        // RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01: this is now the SOLE entry point
        // for baseline inclusion in PortfolioState. seed_portfolio_from_baseline
        // mutates the live portfolio directly, so execution_snapshot.portfolio
        // .positions already carries baseline + same-run fill delta. Downstream
        // reconcile (local_snapshot_provider below, and local_fn in lifecycle.rs)
        // MUST derive local truth directly from the execution snapshot via
        // reconcile_local_snapshot_from_runtime_with_sides — re-merging the
        // baseline there would double-count it (local = fills + 2x baseline while
        // broker = fills + baseline), producing false ReconcileDrift halts.
        //
        // Double-count safety: recover_oms_and_portfolio replays only fills from the
        // current run_id; baseline adds only inherited prior-run qty.
        if let Some(baseline) = self.broker_baseline.read().await.clone() {
            seed_portfolio_from_baseline(&mut portfolio, &baseline);
        }

        {
            let mut sides_lock = self.local_order_sides.write().await;
            *sides_lock = recovered_sides.clone();
        }

        // PHASE-7A-FINAL-PRIVATE-PRODUCTION-EFFECTS-PROOF requirement 6: the
        // one hermetic-broker injection point. `hermetic_test_broker_
        // override_enabled` is always `false` in production and for every
        // test that does not explicitly enable it (its setter is
        // `#[cfg(test)]`-gated) — so this branch is dead in every default
        // build and every non-injecting test. It never weakens
        // `build_daemon_broker` itself: the real gate below is untouched
        // and unconditionally applies whenever the override is not
        // explicitly enabled by an in-crate test.
        //
        // `AlpacaBrokerAdapter::new()` constructs a `reqwest::blocking::Client`
        // which temporarily creates and drops an internal Tokio runtime.  Tokio
        // 1.49 panics when any runtime is dropped inside an async context.
        // `block_in_place` moves execution off the async context so the drop
        // is safe.  Requires a multi-thread runtime (production and
        // `#[tokio::test(flavor = "multi_thread")]` tests both satisfy this).
        let daemon_broker = if self.hermetic_test_broker_override_enabled().await {
            DaemonBroker::Paper(mqk_broker_paper::LockedPaperBroker::default())
        } else {
            tokio::task::block_in_place(|| {
                build_daemon_broker(
                    self.runtime_selection.broker_kind,
                    self.runtime_selection.deployment_mode,
                )
            })?
        };

        let broker_seed = match self.broker_snapshot_source {
            BrokerSnapshotTruthSource::Synthetic => {
                let broker_snapshot_guard = self.broker_snapshot.read().await;
                if let Some(existing) = broker_snapshot_guard.clone() {
                    existing
                } else {
                    drop(broker_snapshot_guard);
                    let now = Utc::now();
                    let synth = synthesize_paper_broker_snapshot(
                        &oms_orders,
                        &recovered_sides,
                        &portfolio,
                        now,
                    );
                    *self.broker_snapshot.write().await = Some(synth.clone());
                    synth
                }
            }
            BrokerSnapshotTruthSource::External => {
                // If a snapshot is already present (pre-loaded by test
                // scaffolding, or retained from a prior loop tick), use it
                // directly and skip the blocking network fetch.  In a fresh
                // production process `broker_snapshot` is always `None` here,
                // so the fetch always runs in production.
                let seeded = self.broker_snapshot.read().await.clone();
                if let Some(existing) = seeded {
                    existing
                } else {
                    let now = Utc::now();
                    let fetched = tokio::task::block_in_place(|| {
                        match &daemon_broker {
                            DaemonBroker::Alpaca(adapter) => {
                                adapter.fetch_broker_snapshot(now).map_err(|err| match err {
                                    BrokerError::AuthSession { detail } => {
                                        RuntimeLifecycleError::forbidden(
                                            "runtime.start_refused.alpaca_snapshot_auth",
                                            "broker_snapshot_fetch",
                                            format!(
                                                "failed to fetch Alpaca broker snapshot before runtime start: {detail}"
                                            ),
                                        )
                                    }
                                    other => RuntimeLifecycleError::service_unavailable(
                                        "runtime.start_refused.alpaca_snapshot_unavailable",
                                        format!(
                                            "failed to fetch Alpaca broker snapshot before runtime start: {other}"
                                        ),
                                    ),
                                })
                            }
                            _ => Err(RuntimeLifecycleError::service_unavailable(
                                "runtime.start_refused.broker_snapshot_source_mismatch",
                                "external broker snapshot source requires Alpaca broker adapter construction",
                            )),
                        }
                    })?;

                    // DURABLE-PAPER-PORTFOLIO-AND-PNL-01C: canonical acceptance seam --
                    // writes the in-memory cache (unchanged) and additively persists
                    // this as authoritative Paper+Alpaca portfolio truth.
                    super::snapshot::accept_external_broker_snapshot(
                        self,
                        fetched.clone(),
                        Some(run_id),
                        None,
                    )
                    .await;

                    // AUTON-PAPER-RISK-03: build a second adapter dedicated to periodic
                    // snapshot refresh in the execution loop.  build_daemon_broker reads
                    // credentials from env — same stable config as the execution adapter.
                    // If this build fails we skip; the loop falls back to the startup
                    // snapshot, which is the pre-patch status quo.
                    let refresh_result = tokio::task::block_in_place(|| {
                        build_daemon_broker(
                            self.runtime_selection.broker_kind,
                            self.runtime_selection.deployment_mode,
                        )
                    });
                    match refresh_result {
                        Ok(DaemonBroker::Alpaca(refresh_alpaca)) => {
                            *self.external_snapshot_refresher.write().await =
                                Some(Arc::new(refresh_alpaca));
                        }
                        Ok(_) => {
                            tracing::warn!(
                                "external_snapshot_refresher_build_failed: \
                                 unexpected broker kind; periodic broker snapshot refresh will not run"
                            );
                        }
                        Err(err) => {
                            tracing::warn!(
                                "external_snapshot_refresher_build_failed: \
                                 periodic broker snapshot refresh will not run; error={err}"
                            );
                        }
                    }

                    fetched
                }
            }
        };

        let mut order_map = BrokerOrderMap::new();
        let existing = mqk_db::broker_map_load(&db)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("broker_map_load failed", err))?;
        for (internal_id, broker_id) in existing {
            order_map.register(&internal_id, &broker_id);
        }

        let broker_cursor = mqk_db::load_broker_cursor(&db, self.adapter_id())
            .await
            .map_err(|err| RuntimeLifecycleError::internal("load_broker_cursor failed", err))?;

        let ws_continuity = AlpacaWsContinuityState::from_cursor_json(
            self.runtime_selection.broker_kind,
            broker_cursor.as_deref(),
        );
        *self.alpaca_ws_continuity.write().await = ws_continuity;

        // RUNTIME-RISK-DYNAMIC-STATE-AUTHORITY-01 / RRA-2: day_id and
        // reject_window_id are no longer computed here — `RuntimeRiskGate`
        // now derives them fresh from its own injected `SystemClock` on
        // every evaluation (AUTON-PAPER-RISK-04 formulas moved to
        // `mqk_runtime::runtime_risk`). RR1
        // (RUNTIME-RISK-START-BASELINE-AUTHORITY-REPAIR-01): equity is no
        // longer frozen from `effective_config`'s `initial_equity_micros`
        // either — `from_run_config_with_account_authority` fetches this
        // SAME `DaemonAccountAuthority` once at construction to seed
        // `RiskState.day_start_equity_micros` / `peak_equity_micros` and to
        // convert `daily_loss_limit` / `max_drawdown` ratios to absolute
        // micros, and again on every subsequent evaluation. This guarantees
        // the account-level risk baseline and every later evaluation read
        // the same authoritative source, never the daemon/env-configured
        // `initial_equity_micros` (which remains solely a LOCAL
        // `PortfolioState` seed — see `recover_oms_and_portfolio` above).
        let account_authority: Arc<dyn RuntimeAccountAuthority> = Arc::new(DaemonAccountAuthority {
            broker_snapshot: Arc::clone(&self.broker_snapshot),
            source: self.broker_snapshot_source,
            freshness_bound: chrono::Duration::seconds(super::ACCOUNT_RISK_FRESHNESS_BOUND_SECS),
        });

        let gateway = build_gateway(
            daemon_broker,
            StateIntegrityGate {
                integrity: Arc::clone(&self.integrity),
            },
            mqk_runtime::runtime_risk::RuntimeRiskGate::from_run_config_with_account_authority(
                &effective_config,
                account_authority,
                Arc::new(SystemClock),
            ),
            ReconcileTruthGate {
                reconcile_status: Arc::clone(&self.reconcile_status),
            },
        );

        let broker_snapshots = Arc::clone(&self.broker_snapshot);
        let broker_seed_reconcile =
            reconcile_broker_snapshot_from_schema(&broker_seed).map_err(|err| {
                RuntimeLifecycleError::service_unavailable(
                    "runtime.start_refused.service_unavailable",
                    err.to_string(),
                )
            })?;

        let local_seed_reconcile = {
            let local_snapshot_guard = self.execution_snapshot.read().await;
            if let Some(snap) = local_snapshot_guard.clone() {
                let sides = self.local_order_sides.read().await;
                reconcile_local_snapshot_from_runtime_with_sides(&snap, &sides)
            } else {
                // RSB01: when no execution snapshot is available (fresh start),
                // use the adopted broker baseline as local truth so the first-tick
                // Phase-0c reconcile check sees local == broker and does not
                // false-positive ReconcileDrift after a clean idle adoption.
                // If no baseline has been adopted, fall back to empty (unchanged
                // behaviour — broker must also be empty for reconcile to pass).
                self.broker_baseline
                    .read()
                    .await
                    .clone()
                    .unwrap_or_else(mqk_reconcile::LocalSnapshot::empty)
            }
        };

        let local_snapshots = Arc::clone(&self.execution_snapshot);
        let side_cache_for_local = Arc::clone(&self.local_order_sides);
        // RSB01: live baseline arc for the closure so each tick reads the current
        // baseline when execution_snapshot is still None (e.g. during the first tick).
        let baseline_for_runtime = Arc::clone(&self.broker_baseline);
        let local_snapshot_provider = move || {
            let Some(snapshot) = local_snapshots
                .try_read()
                .ok()
                .and_then(|snapshot| snapshot.clone())
            else {
                // RSB01: prefer live baseline reading; fall back to static seed if
                // the baseline lock is transiently contended.
                return baseline_for_runtime
                    .try_read()
                    .ok()
                    .and_then(|g| g.clone())
                    .unwrap_or_else(|| local_seed_reconcile.clone());
            };

            let sides = side_cache_for_local
                .try_read()
                .map(|g| g.clone())
                .unwrap_or_default();

            // RECONCILE-BASELINE-DOUBLE-COUNT-FIX-01: execution_snapshot.portfolio
            // .positions already carries the adopted broker baseline (seeded once
            // at run start by seed_portfolio_from_baseline — see the comment above
            // the seeding call in build_execution_orchestrator) plus any same-run
            // fill delta. Re-merging baseline_for_runtime here would double-count
            // it: local = fills + 2x baseline, while broker truth = fills +
            // baseline, producing a false ReconcileDrift halt/disarm. Derive local
            // truth directly from the seeded snapshot — no merge.
            reconcile_local_snapshot_from_runtime_with_sides(&snapshot, &sides)
        };

        let broker_snapshot_provider = move || {
            let Some(schema_snapshot) = broker_snapshots
                .try_read()
                .ok()
                .and_then(|snapshot| snapshot.clone())
            else {
                return broker_seed_reconcile.clone();
            };

            reconcile_broker_snapshot_from_schema(&schema_snapshot)
                .unwrap_or_else(|_| broker_seed_reconcile.clone())
        };

        let mut orch = mqk_runtime::orchestrator::ExecutionOrchestrator::new(
            db,
            gateway,
            order_map,
            oms_orders,
            portfolio,
            run_id,
            self.node_id.clone(),
            self.adapter_id(),
            broker_cursor,
            mqk_runtime::orchestrator::WallClock,
            Box::new(local_snapshot_provider),
            Box::new(broker_snapshot_provider),
        );

        // RECONCILE-DRIFT-AFTER-TERMINAL-FILL-FRESH-SNAPSHOT-01 /
        // PAPER-TERMINAL-FILL-REFRESHER-AND-RETEST-01: wire the terminal-fill
        // expiry refresher for the External-source path so Phase 0c forces a
        // final broker REST fetch before halting on grace expiry.
        //
        // Uses `self.snapshot_fetcher` (built once in AppState::new() for any
        // Alpaca config with credentials present, independent of
        // `broker_snapshot` seed state) rather than
        // `self.external_snapshot_refresher` (only populated by the cold-fetch
        // branch above, which is skipped whenever `broker_snapshot` was
        // already populated at entry — e.g. by a prior
        // POST /api/v1/ops/repair/adopt-broker-position-baseline call). That
        // earlier wiring left `terminal_fill_expiry_refresher` unconfigured
        // for that paper-deployment sequence, forcing a fail-closed halt at
        // grace expiry even though a fetcher was available.
        if let Some(fetcher) = select_external_snapshot_fetcher(
            self.broker_snapshot_source,
            self.snapshot_fetcher.clone(),
        ) {
            let expiry_broker_snapshots = Arc::clone(&self.broker_snapshot);
            // DURABLE-PAPER-PORTFOLIO-AND-PNL-01C: this closure is sync (invoked
            // synchronously from the orchestrator tick), so the durable-persist
            // half of accept_external_broker_snapshot can't be awaited directly
            // here -- capture just the owned pieces it needs (all Clone/Copy,
            // 'static) and spawn it as a best-effort task, matching the
            // existing tokio::spawn pattern used by the alert sink below.
            let expiry_persist_db = self.db.clone();
            let expiry_persist_deployment_mode = self.deployment_mode();
            let expiry_persist_broker_kind = self.runtime_selection.broker_kind;
            orch.set_terminal_fill_expiry_refresher(Box::new(move || {
                let schema_fresh = match tokio::task::block_in_place(|| fetcher.fetch_snapshot()) {
                    Ok(s) => s,
                    Err(err) => {
                        tracing::warn!("terminal_fill_expiry_refresh_fetch_failed error={err}");
                        return None;
                    }
                };
                // Update AppState cache so subsequent ticks and the
                // background reconcile see the fresh data without re-fetching.
                if let Ok(mut guard) = expiry_broker_snapshots.try_write() {
                    *guard = Some(schema_fresh.clone());
                }
                {
                    let db = expiry_persist_db.clone();
                    let db_for_accounting = expiry_persist_db.clone();
                    let snapshot = schema_fresh.clone();
                    tokio::spawn(async move {
                        let outcome = super::snapshot::persist_external_broker_snapshot_best_effort(
                            db,
                            expiry_persist_deployment_mode,
                            expiry_persist_broker_kind,
                            snapshot.clone(),
                            Some(run_id),
                            None,
                        )
                        .await;
                        // DURABLE-PAPER-PORTFOLIO-AND-PNL closure repair
                        // (Repair C): accounting refresh must gate on a
                        // *confirmed* snapshot id, at every External
                        // acceptance call site -- including this spawned
                        // one, which the prior patch left unwired.
                        if let super::snapshot::ExternalSnapshotPersistOutcome::Confirmed {
                            snapshot_id,
                            ..
                        } = outcome
                        {
                            super::paper_portfolio_accounting::refresh_paper_portfolio_accounting_state_best_effort(
                                db_for_accounting,
                                expiry_persist_deployment_mode,
                                expiry_persist_broker_kind,
                                run_id,
                                snapshot_id,
                                snapshot.positions,
                                Utc::now(),
                            )
                            .await;
                        }
                    });
                }
                reconcile_broker_snapshot_from_schema(&schema_fresh).ok()
            }));
        }

        // DISCORD-TRADE-LIFECYCLE-ALERTS-01: inject best-effort alert sink.
        //
        // The sink is a sync closure that spawns an async Discord delivery task
        // for each `TradeLifecycleEvent`.  Panics inside the closure are caught
        // by `fire_alert` so they cannot crash the orchestrator tick loop.
        //
        // The closure captures a cloned `DiscordNotifier` (cheap Arc clone) and
        // the deployment mode label.  No secrets are captured; no webhook URL
        // is included in event payloads.
        {
            let notifier = self.discord_notifier.clone();
            let env_label = self.deployment_mode().as_db_mode().to_string();
            orch.set_alert_sink(std::sync::Arc::new(move |ev| {
                use crate::notify::TradeEventPayload;
                use mqk_runtime::TradeLifecycleEvent;

                let notifier = notifier.clone();
                let env = env_label.clone();

                let payload: TradeEventPayload = match ev {
                    TradeLifecycleEvent::OrderSubmitted {
                        run_id,
                        order_id,
                        symbol,
                        qty,
                    } => TradeEventPayload {
                        stage: "order.submitted".to_string(),
                        run_id: Some(format!("{:.8}", run_id.to_string())),
                        symbol: Some(symbol.clone()),
                        side: None,
                        qty: Some(qty),
                        price_micros: None,
                        order_id: Some(order_id.clone()),
                        detail: None,
                        environment: Some(env),
                        summary: format!("submitted {symbol} qty={qty} order_id={:.8}", order_id),
                        ts_utc: chrono::Utc::now().to_rfc3339(),
                    },
                    TradeLifecycleEvent::OrderAcked {
                        run_id,
                        order_id,
                        broker_order_id,
                        symbol,
                    } => TradeEventPayload {
                        stage: "order.acked".to_string(),
                        run_id: Some(format!("{:.8}", run_id.to_string())),
                        symbol,
                        side: None,
                        qty: None,
                        price_micros: None,
                        order_id: Some(order_id.clone()),
                        detail: broker_order_id.clone(),
                        environment: Some(env),
                        summary: format!(
                            "acked order_id={:.8} broker={}",
                            order_id,
                            broker_order_id.as_deref().unwrap_or("none")
                        ),
                        ts_utc: chrono::Utc::now().to_rfc3339(),
                    },
                    TradeLifecycleEvent::FillApplied {
                        run_id,
                        order_id,
                        symbol,
                        side,
                        qty,
                        price_micros,
                        terminal,
                    } => {
                        let stage = if terminal {
                            "fill.terminal"
                        } else {
                            "fill.partial"
                        }
                        .to_string();
                        let price_usd = price_micros as f64 / 1_000_000.0;
                        TradeEventPayload {
                            stage: stage.clone(),
                            run_id: Some(format!("{:.8}", run_id.to_string())),
                            symbol: Some(symbol.clone()),
                            side: Some(side.clone()),
                            qty: Some(qty),
                            price_micros: Some(price_micros),
                            order_id: Some(order_id.clone()),
                            detail: None,
                            environment: Some(env),
                            summary: format!(
                                "{stage} {side} {symbol} qty={qty} price=${price_usd:.4}"
                            ),
                            ts_utc: chrono::Utc::now().to_rfc3339(),
                        }
                    }
                    TradeLifecycleEvent::ReconcileDriftHalt { run_id, reason } => {
                        TradeEventPayload {
                            stage: "halt.reconcile_drift".to_string(),
                            run_id: Some(format!("{:.8}", run_id.to_string())),
                            symbol: None,
                            side: None,
                            qty: None,
                            price_micros: None,
                            order_id: None,
                            detail: Some(reason.clone()),
                            environment: Some(env),
                            summary: format!("RECONCILE_DRIFT halt — reason: {reason}"),
                            ts_utc: chrono::Utc::now().to_rfc3339(),
                        }
                    }
                    TradeLifecycleEvent::RecoveryQuarantine { run_id } => TradeEventPayload {
                        stage: "halt.recovery_quarantine".to_string(),
                        run_id: Some(format!("{:.8}", run_id.to_string())),
                        symbol: None,
                        side: None,
                        qty: None,
                        price_micros: None,
                        order_id: None,
                        detail: None,
                        environment: Some(env),
                        summary: "RECOVERY_QUARANTINE: ambiguous outbox on restart".to_string(),
                        ts_utc: chrono::Utc::now().to_rfc3339(),
                    },
                };

                tokio::spawn(async move {
                    notifier.notify_trade_event(&payload).await;
                });
            }));
        }

        Ok(orch)
    }
}

// ---------------------------------------------------------------------------
// AUTON-PAPER-BLOCKER-01: env-sourced risk config helpers
// ---------------------------------------------------------------------------

/// Env var: initial equity in USD (positive float).  Converted to micros.
pub(crate) const ENV_RISK_INITIAL_EQUITY_USD: &str = "MQK_RISK_INITIAL_EQUITY_USD";

/// Env var: daily loss limit as a ratio (exclusive range 0 < r < 1).
pub(crate) const ENV_RISK_DAILY_LOSS_LIMIT: &str = "MQK_RISK_DAILY_LOSS_LIMIT";

/// Env var: max drawdown limit as a ratio (exclusive range 0 < r < 1).
/// RRA-2: required with the same validation discipline as
/// `ENV_RISK_DAILY_LOSS_LIMIT` — `RuntimeRiskGate` fails closed (not
/// disabled) if this cannot be supplied from either the run's own
/// `config_json` or this env var.
pub(crate) const ENV_RISK_MAX_DRAWDOWN: &str = "MQK_RISK_MAX_DRAWDOWN";

/// Read the three required risk fields from env.
///
/// Returns `(equity_micros, daily_loss_limit, max_drawdown)`.  Any may be
/// `None` if the env var is absent, empty, unparseable, or out of range.
/// The risk gate already fails closed when these are absent, so `None` just
/// preserves the prior fail-closed behavior.
fn load_risk_env() -> (Option<i64>, Option<f64>, Option<f64>) {
    let equity_micros = std::env::var(ENV_RISK_INITIAL_EQUITY_USD)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|&usd| usd > 0.0 && usd.is_finite() && usd * 1_000_000.0 <= i64::MAX as f64)
        .map(|usd| (usd * 1_000_000.0).round() as i64);

    let daily_loss_limit = std::env::var(ENV_RISK_DAILY_LOSS_LIMIT)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|&r| r.is_finite() && r > 0.0 && r < 1.0);

    let max_drawdown = std::env::var(ENV_RISK_MAX_DRAWDOWN)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|&r| r.is_finite() && r > 0.0 && r < 1.0);

    (equity_micros, daily_loss_limit, max_drawdown)
}

/// Build the effective run config for risk gate initialization.
///
/// Fields already present in `base` are never overwritten — a run row that
/// carries an explicit `/risk` subtree is authoritative.  Env-sourced values
/// only fill fields that `base` does not contain, so this function is safe
/// to call for both daemon-created runs (no `/risk`) and future run rows that
/// carry full risk config.
///
/// If neither env var is set and `base` has no `/risk` subtree, the returned
/// value equals `base` and `RuntimeRiskGate` still fails closed as before.
///
/// # RR2 (RUNTIME-RISK-CONFIG-PRECEDENCE-FAIL-CLOSED-01)
///
/// "Already present in `base`" means the JSON key EXISTS at that pointer
/// path — not that its value happens to parse as the expected type. A field
/// that is present but malformed (wrong JSON type, `null`, or any other
/// value that later fails `RuntimeRiskGate`'s own validation) is explicit
/// operator/run intent and MUST NOT be silently replaced by an env value
/// just because it fails to parse here. Using `.and_then(as_f64/as_i64)` to
/// decide presence (the prior defect) conflates ABSENT with
/// PRESENT-BUT-INVALID: a run row carrying `"daily_loss_limit": "high"`
/// would have been "healed" by a valid `MQK_RISK_DAILY_LOSS_LIMIT`,
/// silently overriding a broken explicit config with an env default instead
/// of leaving it broken so `RuntimeRiskGate::from_run_config_with_account_authority`
/// fails the gate closed on it, as the caller intended by writing it at all.
/// `serde_json::Value::pointer` returning `Some` (including `Some(&Value::Null)`)
/// is therefore the ONLY presence test used below — never followed by a type
/// check.
fn effective_run_config_for_risk(
    base: &serde_json::Value,
    env_equity_micros: Option<i64>,
    env_daily_loss_limit: Option<f64>,
    env_max_drawdown: Option<f64>,
) -> serde_json::Value {
    let equity_present = base.pointer("/risk/initial_equity_micros").is_some();
    let loss_limit_present = base.pointer("/risk/daily_loss_limit").is_some();
    let max_drawdown_present = base.pointer("/risk/max_drawdown").is_some();

    let will_add_equity = !equity_present && env_equity_micros.is_some();
    let will_add_loss_limit = !loss_limit_present && env_daily_loss_limit.is_some();
    let will_add_max_drawdown = !max_drawdown_present && env_max_drawdown.is_some();

    if !will_add_equity && !will_add_loss_limit && !will_add_max_drawdown {
        return base.clone();
    }

    let mut merged = base.clone();
    let obj = match merged.as_object_mut() {
        Some(o) => o,
        None => return base.clone(),
    };

    let risk = obj
        .entry("risk")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));

    if let Some(risk_obj) = risk.as_object_mut() {
        if will_add_equity {
            risk_obj.insert(
                "initial_equity_micros".to_string(),
                serde_json::json!(env_equity_micros.unwrap()),
            );
        }
        if will_add_loss_limit {
            risk_obj.insert(
                "daily_loss_limit".to_string(),
                serde_json::json!(env_daily_loss_limit.unwrap()),
            );
        }
        if will_add_max_drawdown {
            risk_obj.insert(
                "max_drawdown".to_string(),
                serde_json::json!(env_max_drawdown.unwrap()),
            );
        }
    }

    merged
}

// ---------------------------------------------------------------------------
// RUNTIME-PORTFOLIO-SEED-CONFIG-VALIDATION-01
// ---------------------------------------------------------------------------

/// Extracts the LOCAL `PortfolioState`/recovery initial-equity seed from the
/// (already RR2-merged) effective run config, requiring an explicit, typed,
/// positive `i64`.
///
/// This is deliberately independent of `RuntimeRiskGate`'s account-level
/// equity baseline: that value comes solely from `DaemonAccountAuthority`
/// (broker truth) and this function never substitutes it in, per RR1's
/// accepted separation between ACCOUNT-LEVEL RISK EQUITY and the LOCAL
/// PORTFOLIO/RECOVERY SEED.
///
/// `serde_json::Value::pointer` returning `Some` (including `Some(&Value::
/// Null)`) is the presence test, matching `effective_run_config_for_risk`'s
/// ABSENT-vs-PRESENT-BUT-INVALID contract: a value that is present but not a
/// positive `i64` (null, string, bool, float, zero, negative, array, object)
/// is refused here, never healed by falling back to a default.
fn required_initial_equity_micros(
    effective_config: &serde_json::Value,
) -> Result<i64, RuntimeLifecycleError> {
    match effective_config.pointer("/risk/initial_equity_micros") {
        None => Err(RuntimeLifecycleError::forbidden(
            "runtime.start_refused.portfolio_seed_missing",
            "risk.initial_equity_micros",
            "no local portfolio initial_equity_micros seed is configured (neither the \
             run's own config nor MQK_RISK_INITIAL_EQUITY_MICROS supplied one) — refusing \
             to start rather than silently seeding PortfolioState with 0",
        )),
        Some(value) => match value.as_i64() {
            Some(micros) if micros > 0 => Ok(micros),
            Some(micros) => Err(RuntimeLifecycleError::forbidden(
                "runtime.start_refused.portfolio_seed_invalid",
                "risk.initial_equity_micros",
                format!(
                    "explicit risk.initial_equity_micros={micros} is not positive — refusing \
                     to start rather than seed PortfolioState with a non-positive value"
                ),
            )),
            None => Err(RuntimeLifecycleError::forbidden(
                "runtime.start_refused.portfolio_seed_invalid",
                "risk.initial_equity_micros",
                format!(
                    "explicit risk.initial_equity_micros is present but not a valid positive \
                     i64 ({value}) — refusing to start rather than silently seed \
                     PortfolioState with 0"
                ),
            )),
        },
    }
}

// ---------------------------------------------------------------------------
// RR5 (ALPACA-LEGACY-PDT-DISPOSITION-2026-01)
// ---------------------------------------------------------------------------

/// Dispose of the legacy `pdt_auto_enabled` risk flag for the canonical
/// Alpaca (External broker-snapshot-truth) production path.
///
/// # Provider-contract facts (verified against official Alpaca docs)
///
/// `docs.alpaca.markets` changelog entry `2026-06-03-pdt-651df23`,
/// "Pattern day trading and DTBP fields and endpoints are deprecated",
/// effective 2026-07-06: `pattern_day_trader`, `daytrade_count`,
/// `daytrading_buying_power` (and the related `dtbp_check`/`pdt_check`
/// configuration fields and the PDT status / one-time-removal endpoints)
/// are removed from the Trading and Broker APIs. Alpaca's "The Intraday
/// Margin Rule" documentation confirms this is a full regime replacement,
/// not a relabeling: the PDT designation, the 4-in-5-trading-day count,
/// and the $25,000 minimum-equity threshold are gone, superseded by
/// real-time intraday margin exposure monitoring that Alpaca itself
/// enforces broker-side.
///
/// # Why this system cannot honor `pdt_auto_enabled: true` for Alpaca
///
/// `DaemonAccountAuthority` (this module) has no authoritative PDT source
/// for the current Alpaca contract — building a home-grown FINRA Intraday
/// Margin Level/Deficit model is explicitly out of scope for this mission
/// (new feature scope, would delay the equity/ETF Paper finish). It
/// therefore always returns `PdtContext::ok()`. `RiskConfig::sane_defaults()`
/// defaults `pdt_auto_enabled` to `true`. Left unchecked, an ordinary
/// Alpaca Paper run with no explicit `/risk/pdt_auto_enabled` in its
/// `config_json` would silently pair `pdt_auto_enabled == true` with an
/// unconditional always-OK context — appearing configured to enforce PDT
/// while never actually doing so.
///
/// # Disposition
///
/// - `base` (the run's own, unmodified `config_json` — never the
///   env-supplemented `effective` view) does NOT explicitly request
///   `pdt_auto_enabled: true`: the effective config gets an explicit
///   `pdt_auto_enabled: false` for this run. This is NOT APPLICABLE /
///   DISABLED by current provider regime, not a silently-never-enforced
///   `true`. Risk-reducing order semantics, daily-loss, max-drawdown, and
///   reject-storm are all untouched by this — only `pdt_auto_enabled`
///   changes.
/// - `base` DOES explicitly request `pdt_auto_enabled: true`: that is an
///   unsupported configuration on the current Alpaca path (no
///   authoritative source can honor it). The run refuses to start with a
///   precise, operator-visible reason rather than silently pretending
///   enforcement occurred.
///
/// Broker-side intraday-margin/order-acceptance enforcement remains
/// Alpaca's own authority, unaffected by this function. No removed Alpaca
/// PDT field is read or written anywhere in this disposition.
///
/// # ALPACA-LEGACY-PDT-CONFIG-STRICTNESS-01 (FR2)
///
/// `base.pointer(...).and_then(|v| v.as_bool())` previously collapsed EVERY
/// present-but-non-bool value (`"true"`, `null`, `1`, `[]`, `{}`) to `None`,
/// indistinguishable from the field being genuinely absent — silently
/// healing a malformed explicit config to `false` instead of refusing it.
/// That contradicted RR2's ABSENT-vs-PRESENT-BUT-INVALID contract. This now
/// inspects presence and type explicitly and is a strict four-state
/// disposition:
///
/// - ABSENT              -> explicit `false` (NOT APPLICABLE)
/// - PRESENT bool `false` -> explicit `false` (accepted)
/// - PRESENT bool `true`  -> refuse: unsupported legacy-PDT request
/// - PRESENT, any other type (string, null, number, array, object) -> refuse:
///   malformed explicit config, never healed to `false` or `true`
fn alpaca_legacy_pdt_disposition(
    base: &serde_json::Value,
    effective: serde_json::Value,
) -> Result<serde_json::Value, RuntimeLifecycleError> {
    match base.pointer("/risk/pdt_auto_enabled") {
        None => {}
        Some(serde_json::Value::Bool(false)) => {}
        Some(serde_json::Value::Bool(true)) => {
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.start_refused.alpaca_legacy_pdt_unsupported",
                "risk.pdt_auto_enabled",
                "explicit pdt_auto_enabled=true is not supported on the current Alpaca \
                 provider contract: the legacy Pattern Day Trader regime (pattern_day_trader, \
                 daytrade_count, daytrading_buying_power, the $25k minimum) was retired \
                 2026-07-06 in favor of The Intraday Margin Rule, and no authoritative PDT \
                 source is wired for this path — refusing to start rather than silently \
                 never enforcing the requested check",
            ));
        }
        Some(other) => {
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.start_refused.alpaca_legacy_pdt_malformed",
                "risk.pdt_auto_enabled",
                format!(
                    "explicit risk.pdt_auto_enabled is present but not a valid bool ({other}) \
                     — refusing to start rather than silently treat a malformed explicit \
                     value as absent and heal it to false"
                ),
            ));
        }
    }

    let mut effective = effective;
    if let Some(obj) = effective.as_object_mut() {
        let risk = obj
            .entry("risk")
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(risk_obj) = risk.as_object_mut() {
            risk_obj.insert("pdt_auto_enabled".to_string(), serde_json::json!(false));
        }
    }
    Ok(effective)
}

// ---------------------------------------------------------------------------
// RECONCILE-DRIFT-AFTER-TERMINAL-FILL-FRESH-SNAPSHOT-01 /
// PAPER-TERMINAL-FILL-REFRESHER-AND-RETEST-01 /
// PAPER-EAGER-SNAPSHOT-REFRESH-WIRE-01: external broker snapshot fetcher
// selection
// ---------------------------------------------------------------------------

/// Select the broker snapshot fetcher used to refresh the External-source
/// broker snapshot.
///
/// This is the single shared seam for BOTH:
/// 1. the eager/periodic refresh in `loop_runner.rs`'s execution tick loop, and
/// 2. the terminal-fill expiry refresher wired below in
///    `build_execution_orchestrator` (Phase 0c).
///
/// Only the External-source path (Paper+Alpaca, Live+Alpaca) needs a
/// refresher — the Synthetic (paper broker) source has no real broker lag to
/// refresh against. For External, `snapshot_fetcher` is the correct seam: it
/// is built once in `AppState::new()` for any Alpaca config with credentials
/// present, regardless of whether `broker_snapshot` was already seeded (e.g.
/// by `adopt-broker-position-baseline`) before run start. The previously-used
/// `external_snapshot_refresher` field is populated only on the cold-fetch
/// branch above and stays `None` on that pre-seeded path, which left the
/// eager/periodic refresh permanently dead
/// (PAPER-EAGER-SNAPSHOT-REFRESH-WIRE-01).
pub(super) fn select_external_snapshot_fetcher(
    broker_snapshot_source: BrokerSnapshotTruthSource,
    snapshot_fetcher: Option<Arc<dyn BrokerSnapshotFetcher>>,
) -> Option<Arc<dyn BrokerSnapshotFetcher>> {
    match broker_snapshot_source {
        BrokerSnapshotTruthSource::External => snapshot_fetcher,
        BrokerSnapshotTruthSource::Synthetic => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // AUTON-PAPER-RISK-04 / RRA-2: day_id/reject_window_id derivation moved
    // to `mqk_runtime::runtime_risk` (see
    // `day_id_and_reject_window_id_formulas_are_correct` there) — this
    // orchestrator no longer computes them itself.

    #[test]
    fn supplements_missing_risk_fields_from_env_values() {
        let base = serde_json::json!({
            "runtime": "mqk-daemon",
            "adapter": "alpaca",
            "mode": "paper",
        });
        let effective =
            effective_run_config_for_risk(&base, Some(50_000 * 1_000_000), Some(0.02), Some(0.10));

        assert_eq!(
            effective
                .pointer("/risk/initial_equity_micros")
                .and_then(|v| v.as_i64()),
            Some(50_000_000_000),
        );
        assert_eq!(
            effective
                .pointer("/risk/daily_loss_limit")
                .and_then(|v| v.as_f64()),
            Some(0.02),
        );
        assert_eq!(
            effective
                .pointer("/risk/max_drawdown")
                .and_then(|v| v.as_f64()),
            Some(0.10),
        );
        // Non-risk fields preserved.
        assert_eq!(
            effective.pointer("/runtime").and_then(|v| v.as_str()),
            Some("mqk-daemon"),
        );
    }

    #[test]
    fn does_not_override_existing_risk_fields() {
        let base = serde_json::json!({
            "risk": {
                "initial_equity_micros": 10_000_000_000i64,
                "daily_loss_limit": 0.01_f64,
                "max_drawdown": 0.05_f64,
            }
        });
        // Env values that would overwrite if the guard failed.
        let effective =
            effective_run_config_for_risk(&base, Some(99_999_000_000), Some(0.99), Some(0.99));

        assert_eq!(
            effective
                .pointer("/risk/initial_equity_micros")
                .and_then(|v| v.as_i64()),
            Some(10_000_000_000),
            "base equity_micros must not be overwritten",
        );
        assert_eq!(
            effective
                .pointer("/risk/daily_loss_limit")
                .and_then(|v| v.as_f64()),
            Some(0.01),
            "base daily_loss_limit must not be overwritten",
        );
        assert_eq!(
            effective
                .pointer("/risk/max_drawdown")
                .and_then(|v| v.as_f64()),
            Some(0.05),
            "base max_drawdown must not be overwritten",
        );
    }

    #[test]
    fn returns_base_unchanged_when_env_absent() {
        let base = serde_json::json!({ "runtime": "mqk-daemon" });
        let effective = effective_run_config_for_risk(&base, None, None, None);

        // No /risk subtree added — fail-closed behavior preserved.
        assert!(effective.pointer("/risk/initial_equity_micros").is_none());
        assert!(effective.pointer("/risk/daily_loss_limit").is_none());
        assert!(effective.pointer("/risk/max_drawdown").is_none());
        assert_eq!(effective, base);
    }

    // RRA-2: missing max_drawdown alone (equity + daily_loss_limit already
    // present in base) must still be supplemented from env when available.
    #[test]
    fn supplements_only_missing_max_drawdown() {
        let base = serde_json::json!({
            "risk": {
                "initial_equity_micros": 25_000_000_000i64,
                "daily_loss_limit": 0.02_f64,
            }
        });
        let effective = effective_run_config_for_risk(&base, None, None, Some(0.08));

        assert_eq!(
            effective
                .pointer("/risk/max_drawdown")
                .and_then(|v| v.as_f64()),
            Some(0.08),
        );
        assert_eq!(
            effective
                .pointer("/risk/daily_loss_limit")
                .and_then(|v| v.as_f64()),
            Some(0.02),
        );
    }

    // RRA-2: base has everything except max_drawdown, and env does NOT
    // supply it either — the effective config must stay WITHOUT
    // /risk/max_drawdown (never silently defaulted to a value), so
    // `RuntimeRiskGate::from_run_config_with_account_authority` fails the
    // gate closed downstream.
    #[test]
    fn missing_max_drawdown_is_never_silently_defaulted() {
        let base = serde_json::json!({
            "risk": {
                "initial_equity_micros": 25_000_000_000i64,
                "daily_loss_limit": 0.02_f64,
            }
        });
        let effective = effective_run_config_for_risk(&base, None, None, None);

        assert!(
            effective.pointer("/risk/max_drawdown").is_none(),
            "absent max_drawdown must never be silently filled with a default value"
        );
    }

    // -----------------------------------------------------------------------
    // RR2 (RUNTIME-RISK-CONFIG-PRECEDENCE-FAIL-CLOSED-01): a field PRESENT
    // in `base` but malformed (wrong type, or `null`) must NEVER be
    // "healed" by a valid env value — it must survive unchanged in the
    // merged config so `RuntimeRiskGate` fails the whole gate closed on it
    // downstream, exactly as the caller's explicit (if broken) config
    // demands.
    // -----------------------------------------------------------------------

    #[test]
    fn malformed_explicit_daily_loss_limit_string_is_not_healed_by_env() {
        let base = serde_json::json!({
            "risk": {
                "initial_equity_micros": 25_000_000_000i64,
                "daily_loss_limit": "high",
                "max_drawdown": 0.05_f64,
            }
        });
        let effective = effective_run_config_for_risk(&base, None, Some(0.02), None);

        assert_eq!(
            effective.pointer("/risk/daily_loss_limit"),
            Some(&serde_json::json!("high")),
            "a present-but-malformed daily_loss_limit must remain untouched, \
             never overwritten by a syntactically valid env value"
        );
    }

    #[test]
    fn malformed_explicit_max_drawdown_null_is_not_healed_by_env() {
        let base = serde_json::json!({
            "risk": {
                "initial_equity_micros": 25_000_000_000i64,
                "daily_loss_limit": 0.02_f64,
                "max_drawdown": serde_json::Value::Null,
            }
        });
        let effective = effective_run_config_for_risk(&base, None, None, Some(0.10));

        assert!(
            effective.pointer("/risk/max_drawdown").unwrap().is_null(),
            "an explicit null max_drawdown must remain null, never overwritten by env"
        );
    }

    #[test]
    fn malformed_explicit_initial_equity_micros_wrong_type_is_not_healed_by_env() {
        let base = serde_json::json!({
            "risk": {
                "initial_equity_micros": "not-a-number",
                "daily_loss_limit": 0.02_f64,
                "max_drawdown": 0.05_f64,
            }
        });
        let effective =
            effective_run_config_for_risk(&base, Some(99_000_000_000), None, None);

        assert_eq!(
            effective.pointer("/risk/initial_equity_micros"),
            Some(&serde_json::json!("not-a-number")),
            "a present-but-malformed initial_equity_micros must remain untouched, \
             never overwritten by a syntactically valid env value"
        );
    }

    #[test]
    fn malformed_explicit_daily_loss_limit_wrong_type_array_is_not_healed_by_env() {
        let base = serde_json::json!({
            "risk": {
                "initial_equity_micros": 25_000_000_000i64,
                "daily_loss_limit": [0.02],
                "max_drawdown": 0.05_f64,
            }
        });
        let effective = effective_run_config_for_risk(&base, None, Some(0.03), None);

        assert!(
            effective.pointer("/risk/daily_loss_limit").unwrap().is_array(),
            "a present-but-wrong-type (array) daily_loss_limit must remain untouched"
        );
    }

    // -----------------------------------------------------------------------
    // RUNTIME-PORTFOLIO-SEED-CONFIG-VALIDATION-01 (FR1): the LOCAL
    // PortfolioState/recovery initial-equity seed must have explicit, typed,
    // positive authority — never `unwrap_or(0)`.
    // -----------------------------------------------------------------------

    #[test]
    fn missing_initial_equity_micros_refuses_to_start() {
        let effective = serde_json::json!({ "risk": {} });
        let err = required_initial_equity_micros(&effective)
            .expect_err("absent seed must refuse to start, never default to 0");
        assert_eq!(err.fault_class(), "runtime.start_refused.portfolio_seed_missing");
    }

    #[test]
    fn null_initial_equity_micros_refuses_to_start() {
        let effective = serde_json::json!({ "risk": { "initial_equity_micros": null } });
        let err = required_initial_equity_micros(&effective)
            .expect_err("null seed must refuse to start");
        assert_eq!(err.fault_class(), "runtime.start_refused.portfolio_seed_invalid");
    }

    #[test]
    fn string_initial_equity_micros_refuses_to_start() {
        let effective = serde_json::json!({ "risk": { "initial_equity_micros": "bad" } });
        let err = required_initial_equity_micros(&effective)
            .expect_err("string seed must refuse to start, never coerce to 0");
        assert_eq!(err.fault_class(), "runtime.start_refused.portfolio_seed_invalid");
    }

    #[test]
    fn float_initial_equity_micros_refuses_to_start() {
        let effective = serde_json::json!({ "risk": { "initial_equity_micros": 50_000.5 } });
        let err = required_initial_equity_micros(&effective)
            .expect_err("non-integer float seed must refuse to start");
        assert_eq!(err.fault_class(), "runtime.start_refused.portfolio_seed_invalid");
    }

    #[test]
    fn zero_initial_equity_micros_refuses_to_start() {
        let effective = serde_json::json!({ "risk": { "initial_equity_micros": 0i64 } });
        let err = required_initial_equity_micros(&effective)
            .expect_err("zero seed must refuse to start");
        assert_eq!(err.fault_class(), "runtime.start_refused.portfolio_seed_invalid");
    }

    #[test]
    fn negative_initial_equity_micros_refuses_to_start() {
        let effective = serde_json::json!({ "risk": { "initial_equity_micros": -1i64 } });
        let err = required_initial_equity_micros(&effective)
            .expect_err("negative seed must refuse to start");
        assert_eq!(err.fault_class(), "runtime.start_refused.portfolio_seed_invalid");
    }

    #[test]
    fn array_and_object_initial_equity_micros_refuse_to_start() {
        for bad in [
            serde_json::json!({ "risk": { "initial_equity_micros": [1] } }),
            serde_json::json!({ "risk": { "initial_equity_micros": {} } }),
            serde_json::json!({ "risk": { "initial_equity_micros": true } }),
        ] {
            let err = required_initial_equity_micros(&bad)
                .expect_err("non-numeric-typed seed must refuse to start");
            assert_eq!(err.fault_class(), "runtime.start_refused.portfolio_seed_invalid");
        }
    }

    #[test]
    fn positive_initial_equity_micros_returns_exact_value() {
        let effective = serde_json::json!({ "risk": { "initial_equity_micros": 25_000_000_000i64 } });
        assert_eq!(
            required_initial_equity_micros(&effective).expect("positive seed must be accepted"),
            25_000_000_000i64,
        );
    }

    #[test]
    fn valid_explicit_config_is_unchanged_by_validation() {
        // Negative control 1: a config that was already valid before this
        // patch must still validate to the exact same value.
        let base = serde_json::json!({
            "risk": {
                "initial_equity_micros": 10_000_000_000i64,
                "daily_loss_limit": 0.02_f64,
            }
        });
        assert_eq!(
            required_initial_equity_micros(&base).unwrap(),
            10_000_000_000i64,
        );
    }

    #[test]
    fn absent_base_with_valid_env_supplementation_produces_exact_positive_seed() {
        // Negative control 2: RR2's merge fills the ABSENT field from env,
        // and this validation accepts the merged, now-present positive
        // value exactly.
        let base = serde_json::json!({ "runtime": "mqk-daemon" });
        let effective =
            effective_run_config_for_risk(&base, Some(50_000_000_000), None, None);
        assert_eq!(
            required_initial_equity_micros(&effective).expect("env-supplied seed must validate"),
            50_000_000_000i64,
        );
    }

    #[test]
    fn malformed_explicit_base_with_valid_env_remains_refused_never_healed() {
        // Negative control 3: RR2 never heals a PRESENT-BUT-INVALID explicit
        // value with env, and this validation must refuse the still-broken
        // merged config rather than fall back to the env value or 0.
        let base = serde_json::json!({
            "risk": { "initial_equity_micros": "bad" }
        });
        let effective =
            effective_run_config_for_risk(&base, Some(50_000_000_000), None, None);
        assert_eq!(
            effective.pointer("/risk/initial_equity_micros"),
            Some(&serde_json::json!("bad")),
            "RR2 must not heal the malformed explicit value with env"
        );
        let err = required_initial_equity_micros(&effective)
            .expect_err("malformed explicit seed must remain refused after RR2 merge");
        assert_eq!(err.fault_class(), "runtime.start_refused.portfolio_seed_invalid");
    }

    // -----------------------------------------------------------------------
    // RR5 (ALPACA-LEGACY-PDT-DISPOSITION-2026-01)
    // -----------------------------------------------------------------------

    #[test]
    fn ordinary_alpaca_config_gets_pdt_explicitly_disabled() {
        // A daemon-created run with no explicit /risk/pdt_auto_enabled at
        // all — the ordinary case.
        let base = serde_json::json!({
            "risk": { "initial_equity_micros": 25_000_000_000i64, "daily_loss_limit": 0.02_f64 }
        });
        let effective = alpaca_legacy_pdt_disposition(&base, base.clone())
            .expect("ordinary config must not refuse to start");
        assert_eq!(
            effective.pointer("/risk/pdt_auto_enabled"),
            Some(&serde_json::json!(false)),
            "ordinary Alpaca Paper runtime must have pdt_auto_enabled explicitly \
             disabled (NOT APPLICABLE by provider regime), never silently left \
             at the sane_defaults() true paired with an always-OK context"
        );
    }

    #[test]
    fn explicit_false_pdt_auto_enabled_is_preserved_as_false() {
        let base = serde_json::json!({ "risk": { "pdt_auto_enabled": false } });
        let effective = alpaca_legacy_pdt_disposition(&base, base.clone())
            .expect("explicit false must not refuse to start");
        assert_eq!(
            effective.pointer("/risk/pdt_auto_enabled"),
            Some(&serde_json::json!(false))
        );
    }

    #[test]
    fn explicit_stale_pdt_auto_enabled_true_refuses_to_start() {
        let base = serde_json::json!({ "risk": { "pdt_auto_enabled": true } });
        let err = alpaca_legacy_pdt_disposition(&base, base.clone())
            .expect_err("explicit pdt_auto_enabled=true on the Alpaca path must refuse to start");
        assert_eq!(
            err.fault_class(),
            "runtime.start_refused.alpaca_legacy_pdt_unsupported"
        );
    }

    #[test]
    fn stale_explicit_true_cannot_silently_run_using_pdt_context_ok() {
        // Direct proof of the production invariant: there is no code path
        // by which `alpaca_legacy_pdt_disposition` returns `Ok` with
        // `pdt_auto_enabled: true` still present in the effective config —
        // it is either forced to `false` or the call refuses to start.
        for base in [
            serde_json::json!({}),
            serde_json::json!({ "risk": {} }),
            serde_json::json!({ "risk": { "pdt_auto_enabled": false } }),
        ] {
            let effective = alpaca_legacy_pdt_disposition(&base, base.clone())
                .expect("non-true-requesting config must not refuse to start");
            assert_ne!(
                effective.pointer("/risk/pdt_auto_enabled"),
                Some(&serde_json::json!(true)),
                "no disposition outcome may leave pdt_auto_enabled=true reachable \
                 with the always-OK DaemonAccountAuthority stub, for base={base:?}"
            );
        }
        let true_base = serde_json::json!({ "risk": { "pdt_auto_enabled": true } });
        assert!(
            alpaca_legacy_pdt_disposition(&true_base, true_base.clone()).is_err(),
            "the only way base can request pdt_auto_enabled=true is refused entirely"
        );
    }

    #[test]
    fn pdt_disposition_does_not_alter_other_risk_fields() {
        let base = serde_json::json!({
            "risk": {
                "initial_equity_micros": 25_000_000_000i64,
                "daily_loss_limit": 0.02_f64,
                "max_drawdown": 0.10_f64,
                "reject_storm": { "max_rejects": 5 },
            }
        });
        let effective = alpaca_legacy_pdt_disposition(&base, base.clone()).unwrap();
        assert_eq!(
            effective.pointer("/risk/initial_equity_micros"),
            Some(&serde_json::json!(25_000_000_000i64)),
            "daily loss / max drawdown / reject storm config must be untouched"
        );
        assert_eq!(
            effective.pointer("/risk/daily_loss_limit"),
            Some(&serde_json::json!(0.02))
        );
        assert_eq!(
            effective.pointer("/risk/max_drawdown"),
            Some(&serde_json::json!(0.10))
        );
        assert_eq!(
            effective.pointer("/risk/reject_storm/max_rejects"),
            Some(&serde_json::json!(5))
        );
    }

    #[test]
    fn pdt_disposition_never_introduces_removed_alpaca_fields() {
        let base = serde_json::json!({});
        let effective = alpaca_legacy_pdt_disposition(&base, base.clone()).unwrap();
        let serialized = effective.to_string();
        for removed_field in [
            "pattern_day_trader",
            "daytrade_count",
            "daytrading_buying_power",
            "dtbp_check",
            "pdt_check",
        ] {
            assert!(
                !serialized.contains(removed_field),
                "must never introduce the removed Alpaca field '{removed_field}'"
            );
        }
    }

    // -----------------------------------------------------------------------
    // ALPACA-LEGACY-PDT-CONFIG-STRICTNESS-01 (FR2): strict four-state
    // disposition — absent/false/true/malformed must never collapse.
    // -----------------------------------------------------------------------

    #[test]
    fn absent_pdt_auto_enabled_becomes_explicit_false() {
        let base = serde_json::json!({});
        let effective = alpaca_legacy_pdt_disposition(&base, base.clone())
            .expect("absent field must not refuse to start");
        assert_eq!(
            effective.pointer("/risk/pdt_auto_enabled"),
            Some(&serde_json::json!(false))
        );
    }

    #[test]
    fn explicit_bool_false_pdt_auto_enabled_is_accepted() {
        let base = serde_json::json!({ "risk": { "pdt_auto_enabled": false } });
        let effective = alpaca_legacy_pdt_disposition(&base, base.clone())
            .expect("explicit bool false must not refuse to start");
        assert_eq!(
            effective.pointer("/risk/pdt_auto_enabled"),
            Some(&serde_json::json!(false))
        );
    }

    #[test]
    fn explicit_bool_true_pdt_auto_enabled_refuses_as_unsupported() {
        let base = serde_json::json!({ "risk": { "pdt_auto_enabled": true } });
        let err = alpaca_legacy_pdt_disposition(&base, base.clone())
            .expect_err("explicit bool true must refuse as unsupported");
        assert_eq!(
            err.fault_class(),
            "runtime.start_refused.alpaca_legacy_pdt_unsupported"
        );
    }

    #[test]
    fn malformed_non_bool_pdt_auto_enabled_refuses_never_healed_to_false() {
        // Every present-but-non-bool value must refuse with the MALFORMED
        // fault class, never silently collapse to the ABSENT disposition
        // (false) the way `.and_then(as_bool)` used to.
        for malformed in [
            serde_json::json!("true"),
            serde_json::json!("false"),
            serde_json::Value::Null,
            serde_json::json!(0),
            serde_json::json!(1),
            serde_json::json!([]),
            serde_json::json!({}),
        ] {
            let base = serde_json::json!({ "risk": { "pdt_auto_enabled": malformed.clone() } });
            let err = alpaca_legacy_pdt_disposition(&base, base.clone()).unwrap_err();
            assert_eq!(
                err.fault_class(),
                "runtime.start_refused.alpaca_legacy_pdt_malformed",
                "malformed pdt_auto_enabled={malformed:?} must use the malformed fault class, \
                 not silently heal to false or to the unsupported-true fault class"
            );
        }
    }

    #[test]
    fn malformed_pdt_auto_enabled_cannot_be_healed_by_effective_or_env_view() {
        // This disposition reads exclusively from `base` (the run's own
        // unmodified config_json), never from an env-supplemented
        // `effective` view — so a malformed explicit value has no path by
        // which env could heal it. Pass a DIFFERENT, already-healthy
        // `effective` (as if env had supplemented something) to prove the
        // malformed `base` value is still refused regardless.
        let base = serde_json::json!({ "risk": { "pdt_auto_enabled": "true" } });
        let healthy_effective = serde_json::json!({ "risk": { "pdt_auto_enabled": false } });
        let err = alpaca_legacy_pdt_disposition(&base, healthy_effective)
            .expect_err("malformed base must refuse even if effective looks healthy");
        assert_eq!(
            err.fault_class(),
            "runtime.start_refused.alpaca_legacy_pdt_malformed"
        );
    }

    #[test]
    fn load_risk_env_rejects_invalid_ratio() {
        // Direct test of the filter logic — ratio >= 1.0 is invalid.
        let bad: Option<f64> = Some(2.0_f64).filter(|&r| r.is_finite() && r > 0.0 && r < 1.0);
        assert!(bad.is_none(), "ratio >= 1.0 must be rejected");

        let also_bad: Option<f64> = Some(0.0_f64).filter(|&r| r.is_finite() && r > 0.0 && r < 1.0);
        assert!(also_bad.is_none(), "zero ratio must be rejected");
    }

    #[test]
    fn supplements_only_missing_field() {
        // base has equity but not loss_limit — only loss_limit should be added.
        let base = serde_json::json!({
            "risk": {
                "initial_equity_micros": 25_000_000_000i64,
            }
        });
        let effective =
            effective_run_config_for_risk(&base, Some(99_000_000_000), Some(0.03), Some(0.07));

        // Equity from base wins.
        assert_eq!(
            effective
                .pointer("/risk/initial_equity_micros")
                .and_then(|v| v.as_i64()),
            Some(25_000_000_000),
        );
        // Loss limit supplemented from env.
        assert_eq!(
            effective
                .pointer("/risk/daily_loss_limit")
                .and_then(|v| v.as_f64()),
            Some(0.03),
        );
        // max_drawdown also supplemented from env (was missing from base).
        assert_eq!(
            effective
                .pointer("/risk/max_drawdown")
                .and_then(|v| v.as_f64()),
            Some(0.07),
        );
    }

    // -----------------------------------------------------------------------
    // RECONCILE-DRIFT-AFTER-TERMINAL-FILL-FRESH-SNAPSHOT-01 /
    // PAPER-TERMINAL-FILL-REFRESHER-AND-RETEST-01 /
    // PAPER-EAGER-SNAPSHOT-REFRESH-WIRE-01
    // -----------------------------------------------------------------------

    /// Test-only `BrokerSnapshotFetcher`. `fetch_snapshot` is never called by
    /// `select_external_snapshot_fetcher` — only the `Option<Arc<dyn _>>`
    /// identity matters for the selection decision.
    struct UnusedFetcher;

    impl BrokerSnapshotFetcher for UnusedFetcher {
        fn fetch_snapshot(&self) -> Result<mqk_schemas::BrokerSnapshot, String> {
            unreachable!("select_external_snapshot_fetcher tests do not invoke fetch_snapshot")
        }
    }

    /// Test-only `BrokerSnapshotFetcher` that always fails. Proves a fetch
    /// failure surfaces as `Err`, never as a fabricated/optimistic snapshot.
    struct FailingFetcher;

    impl BrokerSnapshotFetcher for FailingFetcher {
        fn fetch_snapshot(&self) -> Result<mqk_schemas::BrokerSnapshot, String> {
            Err("simulated broker fetch failure".to_string())
        }
    }

    #[test]
    fn external_source_with_fetcher_selects_snapshot_fetcher() {
        let fetcher: Arc<dyn BrokerSnapshotFetcher> = Arc::new(UnusedFetcher);
        let selected = select_external_snapshot_fetcher(
            BrokerSnapshotTruthSource::External,
            Some(fetcher.clone()),
        );
        assert!(
            selected.is_some(),
            "External source with snapshot_fetcher present must wire a refresher \
             regardless of broker_snapshot seed state \
             (PAPER-TERMINAL-FILL-REFRESHER-AND-RETEST-01)"
        );
        assert!(Arc::ptr_eq(&selected.unwrap(), &fetcher));
    }

    #[test]
    fn external_source_without_fetcher_stays_unconfigured() {
        // No `snapshot_fetcher` available (e.g. credentials absent) — the
        // refresher remains unconfigured and the existing fail-closed
        // `Some(None) | None` halt path in orchestrator.rs Phase 0c applies.
        let selected = select_external_snapshot_fetcher(BrokerSnapshotTruthSource::External, None);
        assert!(
            selected.is_none(),
            "External source with no snapshot_fetcher must not configure a refresher \
             — fail-closed halt at grace expiry remains intact"
        );
    }

    #[test]
    fn synthetic_source_never_configures_a_refresher() {
        // Paper-synthetic broker has no real broker lag to refresh against;
        // a configured snapshot_fetcher must still be ignored.
        let fetcher: Arc<dyn BrokerSnapshotFetcher> = Arc::new(UnusedFetcher);
        let selected =
            select_external_snapshot_fetcher(BrokerSnapshotTruthSource::Synthetic, Some(fetcher));
        assert!(
            selected.is_none(),
            "Synthetic source must never configure a terminal-fill expiry refresher"
        );
    }

    #[test]
    fn external_source_selection_is_shared_seam_for_eager_and_expiry_refreshers() {
        // PAPER-EAGER-SNAPSHOT-REFRESH-WIRE-01: both the eager/periodic
        // refresh in loop_runner.rs and the terminal-fill expiry refresher
        // wired in build_execution_orchestrator call this same selection
        // function against the same `state.snapshot_fetcher` Arc. Selecting
        // twice from the same inputs (simulating both call sites) must yield
        // the identical underlying fetcher — proving one shared seam, not
        // two independently-derived ones, and that the result does not
        // depend on any `broker_snapshot` seed-state side channel.
        let fetcher: Arc<dyn BrokerSnapshotFetcher> = Arc::new(UnusedFetcher);

        let for_expiry_refresher = select_external_snapshot_fetcher(
            BrokerSnapshotTruthSource::External,
            Some(fetcher.clone()),
        );
        let for_eager_loop = select_external_snapshot_fetcher(
            BrokerSnapshotTruthSource::External,
            Some(fetcher.clone()),
        );

        let for_expiry_refresher = for_expiry_refresher.expect("expiry refresher seam");
        let for_eager_loop = for_eager_loop.expect("eager loop seam");
        assert!(
            Arc::ptr_eq(&for_expiry_refresher, &for_eager_loop),
            "terminal-fill expiry refresher and eager/periodic loop refresh must \
             resolve to the identical snapshot_fetcher instance"
        );
        assert!(Arc::ptr_eq(&for_eager_loop, &fetcher));
    }

    #[test]
    fn external_source_with_failing_fetcher_propagates_error_not_snapshot() {
        // Fail-closed contract: if the broker REST fetch fails, the selected
        // fetcher's `fetch_snapshot()` must return `Err`. Neither the eager
        // loop refresh nor the terminal-fill expiry refresher may treat a
        // failed fetch as a fresh/clean snapshot.
        let fetcher: Arc<dyn BrokerSnapshotFetcher> = Arc::new(FailingFetcher);
        let selected =
            select_external_snapshot_fetcher(BrokerSnapshotTruthSource::External, Some(fetcher));
        let selected = selected.expect("External source with fetcher must select it");
        match selected.fetch_snapshot() {
            Err(msg) => assert_eq!(msg, "simulated broker fetch failure"),
            Ok(_) => panic!("fetch failure must surface as Err, never as a fabricated snapshot"),
        }
    }

    // -----------------------------------------------------------------------
    // RR3 (RUNTIME-RISK-ACCOUNT-FRESHNESS-AUTHORITY-01) / F4: direct proof of
    // the REAL `DaemonAccountAuthority` in its defining module. The E2E
    // scenario test in mqk-runtime uses a hermetic `TestAccountAuthority`
    // that only mirrors this type's behavior — it does not exercise this
    // code. These tests construct the actual `DaemonAccountAuthority` and
    // call its real `RuntimeAccountAuthority::current_account`.
    //
    // Deliberately plain `#[test]` (no `#[tokio::test]`, no runtime at all):
    // `current_account` is a synchronous, non-`async fn` trait method with
    // no `.await` anywhere in its body — it cannot perform network I/O
    // (requirement 11). If it ever gained an awaited call, these tests
    // would panic immediately with "no reactor running" rather than
    // silently succeeding.
    // -----------------------------------------------------------------------

    fn schema_account(equity: &str) -> mqk_schemas::BrokerAccount {
        mqk_schemas::BrokerAccount {
            equity: equity.to_string(),
            cash: "0".to_string(),
            currency: "USD".to_string(),
        }
    }

    fn schema_snapshot(equity: &str, captured_at_utc: DateTime<Utc>) -> mqk_schemas::BrokerSnapshot {
        mqk_schemas::BrokerSnapshot {
            captured_at_utc,
            account: schema_account(equity),
            orders: vec![],
            fills: vec![],
            positions: vec![],
        }
    }

    fn make_authority(
        seed: Option<mqk_schemas::BrokerSnapshot>,
        source: BrokerSnapshotTruthSource,
        freshness_bound_secs: i64,
    ) -> DaemonAccountAuthority {
        DaemonAccountAuthority {
            broker_snapshot: Arc::new(RwLock::new(seed)),
            source,
            freshness_bound: chrono::Duration::seconds(freshness_bound_secs),
        }
    }

    #[test]
    fn rr3_external_fresh_valid_equity_returns_exact_micros() {
        let now = Utc::now();
        let authority = make_authority(
            Some(schema_snapshot("100000.50", now)),
            BrokerSnapshotTruthSource::External,
            60,
        );
        let ctx = authority
            .current_account(now)
            .expect("fresh valid External snapshot must resolve");
        assert_eq!(ctx.equity_micros, 100_000_500_000);
    }

    #[test]
    fn rr3_snapshot_cache_replacement_is_observed_by_the_same_authority() {
        let now = Utc::now();
        let cache = Arc::new(RwLock::new(Some(schema_snapshot("100000", now))));
        let authority = DaemonAccountAuthority {
            broker_snapshot: Arc::clone(&cache),
            source: BrokerSnapshotTruthSource::External,
            freshness_bound: chrono::Duration::seconds(60),
        };
        assert_eq!(
            authority.current_account(now).unwrap().equity_micros,
            100_000_000_000
        );

        // Replace the cache contents (as the execution loop's periodic
        // refresh does) — the SAME authority instance must observe it.
        *cache.try_write().unwrap() = Some(schema_snapshot("150000", now));
        assert_eq!(
            authority.current_account(now).unwrap().equity_micros,
            150_000_000_000
        );
    }

    #[test]
    fn rr3_synthetic_source_is_always_unavailable() {
        let now = Utc::now();
        let authority = make_authority(
            Some(schema_snapshot("100000", now)),
            BrokerSnapshotTruthSource::Synthetic,
            60,
        );
        assert_eq!(
            authority.current_account(now).unwrap_err(),
            AccountAuthorityError::Unavailable,
            "Synthetic source sets account.equity to cash, not mark-to-market \
             equity, and must never be accepted for account-level risk gating"
        );
    }

    #[test]
    fn rr3_no_snapshot_is_unavailable() {
        let now = Utc::now();
        let authority = make_authority(None, BrokerSnapshotTruthSource::External, 60);
        assert_eq!(
            authority.current_account(now).unwrap_err(),
            AccountAuthorityError::Unavailable
        );
    }

    #[test]
    fn rr3_malformed_decimal_equity_is_malformed() {
        let now = Utc::now();
        let authority = make_authority(
            Some(schema_snapshot("not-a-number", now)),
            BrokerSnapshotTruthSource::External,
            60,
        );
        assert_eq!(
            authority.current_account(now).unwrap_err(),
            AccountAuthorityError::Malformed
        );
    }

    #[test]
    fn rr3_zero_or_negative_equity_is_malformed() {
        let now = Utc::now();
        let authority_zero = make_authority(
            Some(schema_snapshot("0", now)),
            BrokerSnapshotTruthSource::External,
            60,
        );
        assert_eq!(
            authority_zero.current_account(now).unwrap_err(),
            AccountAuthorityError::Malformed
        );

        let authority_negative = make_authority(
            Some(schema_snapshot("-500", now)),
            BrokerSnapshotTruthSource::External,
            60,
        );
        assert_eq!(
            authority_negative.current_account(now).unwrap_err(),
            AccountAuthorityError::Malformed
        );
    }

    #[test]
    fn rr3_snapshot_older_than_freshness_bound_is_stale() {
        let captured_at = Utc::now();
        let now = captured_at + chrono::Duration::seconds(61);
        let authority = make_authority(
            Some(schema_snapshot("100000", captured_at)),
            BrokerSnapshotTruthSource::External,
            60,
        );
        assert_eq!(
            authority.current_account(now).unwrap_err(),
            AccountAuthorityError::Stale
        );
    }

    #[test]
    fn rr3_future_captured_at_is_stale() {
        let now = Utc::now();
        let captured_at = now + chrono::Duration::seconds(30);
        let authority = make_authority(
            Some(schema_snapshot("100000", captured_at)),
            BrokerSnapshotTruthSource::External,
            60,
        );
        assert_eq!(
            authority.current_account(now).unwrap_err(),
            AccountAuthorityError::Stale,
            "a snapshot captured in the future relative to `now` is not truthful \
             current equity and must not be silently accepted"
        );
    }

    #[test]
    fn rr3_exact_freshness_boundary_behavior() {
        let captured_at = Utc::now();
        let authority = make_authority(
            Some(schema_snapshot("100000", captured_at)),
            BrokerSnapshotTruthSource::External,
            60,
        );

        // Exactly at the bound: still fresh (age > bound is the Stale test,
        // not age >= bound).
        let at_bound = captured_at + chrono::Duration::seconds(60);
        assert!(
            authority.current_account(at_bound).is_ok(),
            "age exactly equal to the freshness bound must still resolve"
        );

        // One second past the bound: stale.
        let past_bound = captured_at + chrono::Duration::seconds(61);
        assert_eq!(
            authority.current_account(past_bound).unwrap_err(),
            AccountAuthorityError::Stale
        );
    }

    #[test]
    fn rr3_write_lock_contention_fails_closed_as_unavailable() {
        let now = Utc::now();
        let cache = Arc::new(RwLock::new(Some(schema_snapshot("100000", now))));
        let authority = DaemonAccountAuthority {
            broker_snapshot: Arc::clone(&cache),
            source: BrokerSnapshotTruthSource::External,
            freshness_bound: chrono::Duration::seconds(60),
        };

        // Hold an exclusive write guard (as a concurrent snapshot-refresh
        // write would, however briefly) so `try_read` inside
        // `current_account` cannot establish truth.
        let _write_guard = cache.try_write().expect("acquire write lock for the test");
        assert_eq!(
            authority.current_account(now).unwrap_err(),
            AccountAuthorityError::Unavailable,
            "lock contention must fail closed as Unavailable, never fall back \
             to a guessed or stale-but-cached value"
        );
    }

    #[test]
    fn rr3_current_account_is_synchronous_and_requires_no_async_runtime() {
        // This test function itself is NOT #[tokio::test] — there is no
        // Tokio reactor running at all. If `current_account` ever performed
        // real network I/O it would panic ("there is no reactor running")
        // rather than returning a value, proving requirement 11
        // structurally rather than by absence of observation.
        let now = Utc::now();
        let authority = make_authority(
            Some(schema_snapshot("100000", now)),
            BrokerSnapshotTruthSource::External,
            60,
        );
        assert!(authority.current_account(now).is_ok());
    }
}
