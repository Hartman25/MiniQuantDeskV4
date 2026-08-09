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

use chrono::{Datelike, Timelike, Utc};
use mqk_execution::{wiring::build_gateway, BrokerError, BrokerOrderMap};
use sqlx::PgPool;
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

        // AUTON-PAPER-BLOCKER-01: daemon-created runs store a minimal config_json
        // with no /risk subtree.  Supplement from env vars so RuntimeRiskGate
        // receives real inputs.  Fields already in config_json are never overwritten.
        let (env_equity_micros, env_daily_loss_limit) = load_risk_env();
        let effective_config = effective_run_config_for_risk(
            &run.config_json,
            env_equity_micros,
            env_daily_loss_limit,
        );

        let initial_equity_micros = effective_config
            .pointer("/risk/initial_equity_micros")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);

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

        // AUTON-PAPER-RISK-04: derive real day/window identifiers from UTC
        // wall-clock at orchestrator construction time.  day_id is YYYYMMDD
        // (matching RiskInput documentation); reject_window_id is the
        // minute-of-day bucket (0..1439, matching "minute bucket counter").
        // Both are evaluated once at run-start — the risk engine tracks
        // subsequent window transitions via RiskState::record_reject().
        let risk_now = Utc::now();
        let risk_day = risk_now.date_naive();
        let day_id: u32 =
            (risk_day.year() as u32) * 10_000 + risk_day.month() * 100 + risk_day.day();
        let reject_window_id: u32 = risk_now.hour() * 60 + risk_now.minute();

        let gateway = build_gateway(
            daemon_broker,
            StateIntegrityGate {
                integrity: Arc::clone(&self.integrity),
            },
            mqk_runtime::runtime_risk::RuntimeRiskGate::from_run_config(
                &effective_config,
                initial_equity_micros,
                day_id,
                reject_window_id,
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

/// Read the two required risk fields from env.
///
/// Returns `(equity_micros, daily_loss_limit)`.  Either or both may be `None`
/// if the env var is absent, empty, unparseable, or out of range.  The risk
/// gate already fails closed when these are absent, so `None` just preserves
/// the prior fail-closed behavior.
fn load_risk_env() -> (Option<i64>, Option<f64>) {
    let equity_micros = std::env::var(ENV_RISK_INITIAL_EQUITY_USD)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|&usd| usd > 0.0 && usd.is_finite() && usd * 1_000_000.0 <= i64::MAX as f64)
        .map(|usd| (usd * 1_000_000.0).round() as i64);

    let daily_loss_limit = std::env::var(ENV_RISK_DAILY_LOSS_LIMIT)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|&r| r.is_finite() && r > 0.0 && r < 1.0);

    (equity_micros, daily_loss_limit)
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
fn effective_run_config_for_risk(
    base: &serde_json::Value,
    env_equity_micros: Option<i64>,
    env_daily_loss_limit: Option<f64>,
) -> serde_json::Value {
    let need_equity = base
        .pointer("/risk/initial_equity_micros")
        .and_then(|v| v.as_i64())
        .is_none();
    let need_loss_limit = base
        .pointer("/risk/daily_loss_limit")
        .and_then(|v| v.as_f64())
        .is_none();

    let will_add_equity = need_equity && env_equity_micros.is_some();
    let will_add_loss_limit = need_loss_limit && env_daily_loss_limit.is_some();

    if !will_add_equity && !will_add_loss_limit {
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
    }

    merged
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

    // AUTON-PAPER-RISK-04: prove the day_id/reject_window_id derivation
    // formulas produce the exact values the risk engine documentation specifies.
    #[test]
    fn risk_time_context_day_id_and_window_derivation_is_correct() {
        use chrono::{Datelike, TimeZone, Timelike};
        // 2024-01-15 09:32:45 UTC — a known reference moment.
        let ts = chrono::Utc
            .with_ymd_and_hms(2024, 1, 15, 9, 32, 45)
            .unwrap();
        let d = ts.date_naive();
        let day_id: u32 = (d.year() as u32) * 10_000 + d.month() * 100 + d.day();
        assert_eq!(day_id, 20_240_115, "day_id must be YYYYMMDD");

        let reject_window_id: u32 = ts.hour() * 60 + ts.minute();
        assert_eq!(
            reject_window_id,
            9 * 60 + 32,
            "reject_window_id must be minute-of-day bucket"
        );

        // Boundary: midnight (00:00) yields bucket 0.
        let midnight = chrono::Utc.with_ymd_and_hms(2024, 1, 15, 0, 0, 0).unwrap();
        assert_eq!(midnight.hour() * 60 + midnight.minute(), 0);

        // Boundary: 23:59 yields bucket 1439 (max for a 24-hour day).
        let last_minute = chrono::Utc
            .with_ymd_and_hms(2024, 1, 15, 23, 59, 0)
            .unwrap();
        assert_eq!(last_minute.hour() * 60 + last_minute.minute(), 1439);

        // day_id must never overflow u32 for any sane calendar year.
        let far: u32 = 9999 * 10_000 + 12 * 100 + 31;
        assert!(far < u32::MAX, "day_id fits in u32 for any calendar date");
    }

    #[test]
    fn supplements_missing_risk_fields_from_env_values() {
        let base = serde_json::json!({
            "runtime": "mqk-daemon",
            "adapter": "alpaca",
            "mode": "paper",
        });
        let effective = effective_run_config_for_risk(&base, Some(50_000 * 1_000_000), Some(0.02));

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
            }
        });
        // Env values that would overwrite if the guard failed.
        let effective = effective_run_config_for_risk(&base, Some(99_999_000_000), Some(0.99));

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
    }

    #[test]
    fn returns_base_unchanged_when_env_absent() {
        let base = serde_json::json!({ "runtime": "mqk-daemon" });
        let effective = effective_run_config_for_risk(&base, None, None);

        // No /risk subtree added — fail-closed behavior preserved.
        assert!(effective.pointer("/risk/initial_equity_micros").is_none());
        assert!(effective.pointer("/risk/daily_loss_limit").is_none());
        assert_eq!(effective, base);
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
        let effective = effective_run_config_for_risk(&base, Some(99_000_000_000), Some(0.03));

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
}
