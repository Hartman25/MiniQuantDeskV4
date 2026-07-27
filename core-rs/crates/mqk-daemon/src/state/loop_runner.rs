//! Background loop management for mqk-daemon.
//!
//! `spawn_execution_loop` — ticks the ExecutionOrchestrator on a 1-second
//! interval, enforces deadman, and owns the runtime lease.
//!
//! `spawn_reconcile_tick` — runs a periodic reconcile tick and disarms the
//! system on any drift or stale snapshot.
//!
//! `publish_reconcile_failure` — shared helper: persists disarm state and
//! broadcasts a halted status snapshot.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use mqk_reconcile::SnapshotWatermark;
use tokio::sync::watch;
use uuid::Uuid;

use super::env::uptime_secs;
use super::orchestrator_build::select_external_snapshot_fetcher;
use super::snapshot::{
    outbox_json_side, preserve_fail_closed_reconcile_status, reconcile_status_from_report,
    reconcile_status_from_stale, reconcile_unknown_status,
    synthesize_broker_snapshot_from_execution,
};
use super::types::{
    BrokerSnapshotTruthSource, BusMsg, DaemonOrchestrator, ExecutionLoopCommand, ExecutionLoopExit,
    ExecutionLoopHandle, ReconcileStatusSnapshot, StatusSnapshot,
};
use crate::notify::CriticalAlertPayload;

use super::{
    dry_run_strategy_ids_from_env, evaluate_dry_run_strategies, AppState, PerSymbolTargetState,
    DEADMAN_TTL_SECONDS, EXECUTION_LOOP_INTERVAL, STRATEGY_CONTEXT_LOAD_LIMIT,
};

// ---------------------------------------------------------------------------
// spawn_execution_loop
// ---------------------------------------------------------------------------

pub(super) fn spawn_execution_loop(
    state: Arc<AppState>,
    mut orchestrator: DaemonOrchestrator,
    run_id: Uuid,
) -> ExecutionLoopHandle {
    let (stop_tx, mut stop_rx) = watch::channel(ExecutionLoopCommand::Run);
    let snapshot_cache = Arc::clone(&state.execution_snapshot);
    let broker_snapshot_cache = Arc::clone(&state.broker_snapshot);
    let side_cache = Arc::clone(&state.local_order_sides);
    let db = state.db.clone();
    let integrity = Arc::clone(&state.integrity);
    let broker_snapshot_source = state.broker_snapshot_source();
    // PT-AUTO-01: retained for ws_continuity_gap_requires_halt() check per tick.
    let state_arc = Arc::clone(&state);

    // MULTI-SYMBOL-DISPATCH-LOOP-01: build the per-symbol dispatch assignment
    // list once, synchronously, before entering the async loop.
    // `build_multi_symbol_runtime_config_from_env` is pure (env vars plus one
    // watchlist-artifact file read via `evaluate_watchlist_intake_from_env`) —
    // a one-time read here, not per tick. `Err` means even the legacy
    // single-symbol env fallback is unconfigured (`MQK_STRATEGY_SYMBOL` /
    // `MQK_STRATEGY_IDS` / `MQK_STRATEGY_MD_TIMEFRAME` absent or empty) — an
    // empty assignment list, matching the existing no-op behavior of
    // `tick_strategy_dispatch` when strategy dispatch is not configured.
    let multi_symbol_assignments: Vec<super::SymbolStrategyAssignment> =
        super::build_multi_symbol_runtime_config_from_env()
            .map(|cfg| cfg.symbols)
            .unwrap_or_default();

    // MULTI-STRATEGY-RUNTIME-DRY-RUN-01: one-time env read, mirroring
    // `multi_symbol_assignments` above. Empty unless MQK_DRY_RUN_STRATEGY_IDS
    // is explicitly set — default-off, no behavior change when absent.
    let dry_run_strategy_ids: Vec<String> = dry_run_strategy_ids_from_env();

    let join_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(EXECUTION_LOOP_INTERVAL);
        // AUTON-PAPER-RISK-03: countdown to next External broker snapshot refresh.
        let mut external_refresh_ticks: u32 = 0;
        loop {
            tokio::select! {
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() == ExecutionLoopCommand::Stop {
                        break;
                    }
                }
                _ = ticker.tick() => {
                    if let Some(ref pool) = db {
                        let now = Utc::now();
                        match mqk_db::enforce_deadman_or_halt(pool, run_id, DEADMAN_TTL_SECONDS, now).await {
                            Ok(true) => {
                                let _ = mqk_db::persist_arm_state_canonical(
                                    pool,
                                    mqk_db::ArmState::Disarmed,
                                    Some(mqk_db::DisarmReason::DeadmanExpired),
                                )
                                .await;
                                {
                                    let mut ig = integrity.write().await;
                                    ig.disarmed = true;
                                    ig.halted = true;
                                }
                                // DISCORD-DEADMAN-ALERTS-01: fire-and-forget alert after
                                // durable halt/disarm; must never block the exit path.
                                {
                                    let notifier = state_arc.discord_notifier.clone();
                                    let env = Some(
                                        state_arc.deployment_mode().as_api_label().to_string(),
                                    );
                                    let run_id_short = format!("{:.8}", run_id);
                                    let ts = chrono::Utc::now().to_rfc3339(); // allow: alert timestamp
                                    tokio::spawn(async move {
                                        notifier
                                            .notify_critical_alert(&CriticalAlertPayload {
                                                alert_class: "halt.deadman_expired".to_string(),
                                                severity: "critical".to_string(),
                                                summary: format!(
                                                    "Deadman TTL expired — run halted and \
                                                     disarmed | run={run_id_short}"
                                                ),
                                                detail: Some(
                                                    "disarm_reason=DeadmanExpired \
                                                     phase=pre_tick"
                                                        .to_string(),
                                                ),
                                                environment: env,
                                                run_id: Some(run_id_short),
                                                ts_utc: ts,
                                            })
                                            .await;
                                    });
                                }
                                if let Err(release_err) = orchestrator.release_runtime_leadership().await {
                                    tracing::warn!("runtime_lease_release_failed error={release_err}");
                                }
                                let exit = ExecutionLoopExit {
                                    note: Some("execution loop halted: deadman expired".to_string()),
                                };
                                drop_outside_async_context(orchestrator);
                                return exit;
                            }
                            Ok(false) => {}
                            Err(err) => {
                                tracing::error!("execution_loop_deadman_check_failed error={err}");
                                if let Err(halt_err) = mqk_db::halt_run(pool, run_id, now).await {
                                    tracing::error!(run_id = %run_id, "execution_loop_halt_run_persist_failed error={halt_err}");
                                }
                                if let Err(disarm_err) = mqk_db::persist_arm_state_canonical(
                                    pool,
                                    mqk_db::ArmState::Disarmed,
                                    Some(mqk_db::DisarmReason::DeadmanSupervisorFailure),
                                )
                                .await
                                {
                                    tracing::error!(run_id = %run_id, "execution_loop_disarm_persist_failed error={disarm_err}");
                                }
                                {
                                    let mut ig = integrity.write().await;
                                    ig.disarmed = true;
                                    ig.halted = true;
                                }
                                // DISCORD-DEADMAN-ALERTS-01: supervisor failure alert.
                                {
                                    let notifier = state_arc.discord_notifier.clone();
                                    let env = Some(
                                        state_arc.deployment_mode().as_api_label().to_string(),
                                    );
                                    let run_id_short = format!("{:.8}", run_id);
                                    let err_str = err.to_string();
                                    let ts = chrono::Utc::now().to_rfc3339(); // allow: alert timestamp
                                    tokio::spawn(async move {
                                        notifier
                                            .notify_critical_alert(&CriticalAlertPayload {
                                                alert_class: "halt.deadman_supervisor_failure"
                                                    .to_string(),
                                                severity: "critical".to_string(),
                                                summary: format!(
                                                    "Deadman supervisor check failed — run \
                                                     halted and disarmed | run={run_id_short}"
                                                ),
                                                detail: Some(format!(
                                                    "disarm_reason=DeadmanSupervisorFailure \
                                                     error={err_str}"
                                                )),
                                                environment: env,
                                                run_id: Some(run_id_short),
                                                ts_utc: ts,
                                            })
                                            .await;
                                    });
                                }
                                if let Err(release_err) = orchestrator.release_runtime_leadership().await {
                                    tracing::warn!("runtime_lease_release_failed error={release_err}");
                                }
                                let exit = ExecutionLoopExit {
                                    note: Some(format!("execution loop halted: deadman check failed: {err}")),
                                };
                                drop_outside_async_context(orchestrator);
                                return exit;
                            }
                        }
                    }

                    // PT-AUTO-01: WS continuity gap self-halt.
                    //
                    // On the ExternalSignalIngestion (paper+alpaca) path a
                    // GapDetected cursor means the broker event stream is
                    // broken.  Continuing to dispatch orders without fill
                    // tracking is unsound — the loop self-halts before the
                    // next tick so no further orders are placed.
                    if state_arc.ws_continuity_gap_requires_halt().await {
                        tracing::error!(
                            run_id = %run_id,
                            "execution_loop_ws_gap_halt: \
                             Alpaca WS continuity gap detected; halting execution loop"
                        );
                        if let Some(ref pool) = db {
                            let now = Utc::now();
                            if let Err(halt_err) = mqk_db::halt_run(pool, run_id, now).await {
                                tracing::error!(run_id = %run_id, "execution_loop_halt_run_persist_failed error={halt_err}");
                            }
                        }
                        {
                            let mut ig = integrity.write().await;
                            ig.disarmed = true;
                            ig.halted = true;
                        }
                        if let Err(release_err) =
                            orchestrator.release_runtime_leadership().await
                        {
                            tracing::warn!("runtime_lease_release_failed error={release_err}");
                        }
                        let exit = ExecutionLoopExit {
                            note: Some(
                                "execution loop halted: Alpaca WS continuity gap detected"
                                    .to_string(),
                            ),
                        };
                        drop_outside_async_context(orchestrator);
                        return exit;
                    }

                    if let Err(err) = orchestrator.tick().await {
                        tracing::error!("execution_loop_halt error={err}");
                        if let Some(ref pool) = db {
                            let now = Utc::now();
                            if let Err(halt_err) = mqk_db::halt_run(pool, run_id, now).await {
                                tracing::error!(run_id = %run_id, "execution_loop_halt_run_persist_failed error={halt_err}");
                            }
                        }
                        {
                            let mut ig = integrity.write().await;
                            ig.halted = true;
                        }
                        if let Err(release_err) = orchestrator.release_runtime_leadership().await {
                            tracing::warn!("runtime_lease_release_failed error={release_err}");
                        }
                        let exit = ExecutionLoopExit {
                            note: Some(format!("execution loop halted: {err}")),
                        };
                        drop_outside_async_context(orchestrator);
                        return exit;
                    }

                    if let Some(ref pool) = db {
                        let now = Utc::now();
                        if let Ok(true) =
                            mqk_db::deadman_expired(pool, run_id, DEADMAN_TTL_SECONDS, now).await
                        {
                            // DEADMAN-EXPIRED-AFTER-START-01: tick() blocked beyond
                            // DEADMAN_TTL (e.g., fetch_events with no HTTP timeout).
                            // Halt and disarm explicitly so the run is not left RUNNING
                            // with a dead loop — mirrors pre-tick enforce_deadman_or_halt.
                            tracing::error!(
                                run_id = %run_id,
                                "execution_loop_deadman_post_tick: tick duration exceeded \
                                 DEADMAN_TTL; halting and disarming"
                            );
                            let _ = mqk_db::halt_run(pool, run_id, now).await;
                            let _ = mqk_db::persist_arm_state_canonical(
                                pool,
                                mqk_db::ArmState::Disarmed,
                                Some(mqk_db::DisarmReason::DeadmanExpired),
                            )
                            .await;
                            {
                                let mut ig = integrity.write().await;
                                ig.disarmed = true;
                                ig.halted = true;
                            }
                            // DISCORD-DEADMAN-ALERTS-01: post-tick deadman expiry alert.
                            {
                                let notifier = state_arc.discord_notifier.clone();
                                let env = Some(
                                    state_arc.deployment_mode().as_api_label().to_string(),
                                );
                                let run_id_short = format!("{:.8}", run_id);
                                let ts = chrono::Utc::now().to_rfc3339(); // allow: alert timestamp
                                tokio::spawn(async move {
                                    notifier
                                        .notify_critical_alert(&CriticalAlertPayload {
                                            alert_class: "halt.deadman_expired".to_string(),
                                            severity: "critical".to_string(),
                                            summary: format!(
                                                "Deadman TTL exceeded during tick — run halted \
                                                 and disarmed | run={run_id_short}"
                                            ),
                                            detail: Some(
                                                "disarm_reason=DeadmanExpired \
                                                 phase=post_tick (tick blocked beyond TTL)"
                                                    .to_string(),
                                            ),
                                            environment: env,
                                            run_id: Some(run_id_short),
                                            ts_utc: ts,
                                        })
                                        .await;
                                });
                            }
                            if let Err(release_err) =
                                orchestrator.release_runtime_leadership().await
                            {
                                tracing::warn!("runtime_lease_release_failed error={release_err}");
                            }
                            let exit = ExecutionLoopExit {
                                note: Some(
                                    "execution loop halted: deadman expired post-tick".to_string(),
                                ),
                            };
                            drop_outside_async_context(orchestrator);
                            return exit;
                        }
                        if let Err(err) = mqk_db::heartbeat_run(pool, run_id, now).await {
                            tracing::error!("execution_loop_heartbeat_failed error={err}");
                            if let Err(halt_err) = mqk_db::halt_run(pool, run_id, now).await {
                                tracing::error!(run_id = %run_id, "execution_loop_halt_run_persist_failed error={halt_err}");
                            }
                            if let Err(disarm_err) = mqk_db::persist_arm_state_canonical(
                                pool,
                                mqk_db::ArmState::Disarmed,
                                Some(mqk_db::DisarmReason::DeadmanHeartbeatPersistFailed),
                            )
                            .await
                            {
                                tracing::error!(run_id = %run_id, "execution_loop_disarm_persist_failed error={disarm_err}");
                            }
                            {
                                let mut ig = integrity.write().await;
                                ig.disarmed = true;
                                ig.halted = true;
                            }
                            // DISCORD-DEADMAN-ALERTS-01: heartbeat persist failure alert.
                            {
                                let notifier = state_arc.discord_notifier.clone();
                                let env = Some(
                                    state_arc.deployment_mode().as_api_label().to_string(),
                                );
                                let run_id_short = format!("{:.8}", run_id);
                                let err_str = err.to_string();
                                let ts = chrono::Utc::now().to_rfc3339(); // allow: alert timestamp
                                tokio::spawn(async move {
                                    notifier
                                        .notify_critical_alert(&CriticalAlertPayload {
                                            alert_class: "halt.deadman_heartbeat_failed"
                                                .to_string(),
                                            severity: "critical".to_string(),
                                            summary: format!(
                                                "Deadman heartbeat persist failed — run halted \
                                                 and disarmed | run={run_id_short}"
                                            ),
                                            detail: Some(format!(
                                                "disarm_reason=DeadmanHeartbeatPersistFailed \
                                                 error={err_str}"
                                            )),
                                            environment: env,
                                            run_id: Some(run_id_short),
                                            ts_utc: ts,
                                        })
                                        .await;
                                });
                            }
                            if let Err(release_err) = orchestrator.release_runtime_leadership().await {
                                tracing::warn!("runtime_lease_release_failed error={release_err}");
                            }
                            let exit = ExecutionLoopExit {
                                note: Some(format!("execution loop heartbeat failed: {err}")),
                            };
                            drop_outside_async_context(orchestrator);
                            return exit;
                        }
                    }

                    match orchestrator.snapshot().await.context("snapshot failed") {
                        Ok(snapshot) => {
                            if let Some(ref pool) = db {
                                if let Ok(outbox_rows) =
                                    mqk_db::outbox_list_unacked_for_run(pool, run_id).await
                                {
                                    let mut sides = side_cache.write().await;
                                    for row in &outbox_rows {
                                        sides.insert(
                                            row.idempotency_key.clone(),
                                            outbox_json_side(&row.order_json),
                                        );
                                    }
                                }
                                if broker_snapshot_source == BrokerSnapshotTruthSource::Synthetic {
                                    let sides_snapshot = side_cache.read().await.clone();
                                    let now = Utc::now();
                                    let synth = synthesize_broker_snapshot_from_execution(
                                        &snapshot,
                                        &sides_snapshot,
                                        now,
                                    );
                                    *broker_snapshot_cache.write().await = Some(synth);
                                }
                            }
                            *snapshot_cache.write().await = Some(snapshot);
                        }
                        Err(err) => {
                            tracing::warn!("execution_snapshot_refresh_failed error={err}");
                        }
                    }

                    // HEARTBEAT-TICK-01: record successful tick progress.
                    //
                    // Placed here — after orchestrator.tick() succeeded and the
                    // execution snapshot was committed — so a mid-tick hang (blocked
                    // orchestrator or snapshot) does NOT advance this timestamp.
                    // Early-exit paths (deadman, WS gap, orchestrator error, heartbeat
                    // failure) all return before reaching this point, so they never
                    // mark progress.  Operator surfaces compare this timestamp against
                    // wall-clock to detect a stalled loop before the DB deadman fires.
                    state_arc.record_execution_tick(Utc::now().timestamp());

                    // AUTON-PAPER-RISK-03: Periodic External broker snapshot refresh.
                    //
                    // For Synthetic source the snapshot is rebuilt every tick above.
                    // For External source (paper+alpaca) we must re-fetch from the
                    // broker REST API so reconcile compares against a reasonably
                    // fresh snapshot rather than the permanently stale startup one.
                    //
                    // We refresh every EXTERNAL_SNAPSHOT_REFRESH_TICKS ticks (60 s).
                    // On fetch failure we log and keep the last good snapshot — reconcile
                    // still has something to compare against and will drift/halt if the
                    // position truth is genuinely wrong.  This is fail-closed: a missing
                    // refresh is never silently treated as a clean match.
                    //
                    // PAPER-EAGER-SNAPSHOT-REFRESH-WIRE-01: the fetcher used here is
                    // `state_arc.snapshot_fetcher`, selected via
                    // `select_external_snapshot_fetcher` — the same seed-state-independent
                    // seam that wires the terminal-fill expiry refresher in
                    // `orchestrator_build.rs`. The previously-used
                    // `external_snapshot_refresher` field is only populated on the
                    // cold-fetch path and stays `None` whenever `broker_snapshot` was
                    // pre-seeded (e.g. via `adopt-broker-position-baseline`), which left
                    // this eager/periodic refresh permanently dead on that path.
                    //
                    // AUTON-SENT-ORDER-BROKER-TRUTH-01: active-order eager refresh.
                    //
                    // The broker seed snapshot is fetched at run-start before any orders
                    // are placed.  After the first order dispatch, the local snapshot
                    // shows the in-flight order but the stale seed does not — causing
                    // Phase 0c to fire LocalOrderMissingAtBroker and halt the run.
                    //
                    // When the current execution snapshot has any non-terminal orders
                    // (Open / PartiallyFilled / CancelPending / ReplacePending), force the
                    // external broker snapshot refresh this tick so Phase 0c on the next
                    // tick compares against a current Alpaca order list.
                    if broker_snapshot_source == BrokerSnapshotTruthSource::External {
                        let (has_active_orders, has_recent_terminal_fill) = snapshot_cache
                            .read()
                            .await
                            .as_ref()
                            .map(|s| {
                                let active = s.active_orders.iter().any(|o| {
                                    matches!(
                                        o.status.as_str(),
                                        "Open"
                                            | "PartiallyFilled"
                                            | "CancelPending"
                                            | "ReplacePending"
                                    )
                                });
                                (active, s.has_recent_terminal_fill)
                            })
                            .unwrap_or((false, false));
                        if has_active_orders || has_recent_terminal_fill {
                            // Force refresh this tick — do NOT wait for the 60-tick cadence.
                            // has_active_orders: order is in-flight, need current broker order list.
                            // has_recent_terminal_fill (RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01):
                            // fill just applied locally; broker REST snapshot needs to reflect the
                            // updated position before the grace window expires.
                            external_refresh_ticks = super::EXTERNAL_SNAPSHOT_REFRESH_TICKS;
                        }
                        external_refresh_ticks += 1;
                        if external_refresh_ticks >= super::EXTERNAL_SNAPSHOT_REFRESH_TICKS {
                            external_refresh_ticks = 0;
                            let fetcher_opt = select_external_snapshot_fetcher(
                                broker_snapshot_source,
                                state_arc.snapshot_fetcher.clone(),
                            );
                            if let Some(fetcher) = fetcher_opt {
                                match tokio::task::block_in_place(|| fetcher.fetch_snapshot()) {
                                    Ok(fresh) => {
                                        // DURABLE-PAPER-PORTFOLIO-AND-PNL-01C: canonical
                                        // acceptance seam -- writes the in-memory cache
                                        // (unchanged) and additively persists this as
                                        // authoritative Paper+Alpaca portfolio truth.
                                        super::snapshot::accept_external_broker_snapshot(
                                            &state_arc,
                                            fresh,
                                            Some(run_id),
                                            None,
                                        )
                                        .await;
                                        tracing::debug!(
                                            run_id = %run_id,
                                            "external_broker_snapshot_refreshed"
                                        );
                                    }
                                    Err(err) => {
                                        tracing::warn!(
                                            run_id = %run_id,
                                            "external_broker_snapshot_refresh_failed error={err}"
                                        );
                                    }
                                }
                            }
                        }
                    }

                    // EVENT-RISK-FLATTEN-WIRE-01: Pre-event flatten check.
                    //
                    // For each non-flat position, evaluate both configured event-risk
                    // sources (blackout-v1 / earnings-calendar-v1).  When either source
                    // returns FlattenRequired or Unavailable (fail-closed: cannot verify
                    // it is safe to hold), enqueue a market close order into the outbox.
                    // The order is dispatched by the orchestrator's Phase 1 on the next
                    // tick via the normal outbox claim + submit path.
                    //
                    // Idempotency: UUIDv5 key scoped to (run_id, symbol, minute) so
                    // re-evaluating the same trigger within the same 60-second window is
                    // a no-op.  Enqueue failure is non-fatal (logged, retried next tick).
                    if let Some(ref pool) = db {
                        let positions_to_check: Vec<(String, i64)> = {
                            let snap = snapshot_cache.read().await;
                            snap.as_ref()
                                .map(|s| {
                                    s.portfolio
                                        .positions
                                        .iter()
                                        .filter(|p| p.net_qty != 0)
                                        .map(|p| (p.symbol.clone(), p.net_qty))
                                        .collect()
                                })
                                .unwrap_or_default()
                        };
                        for (symbol, net_qty) in &positions_to_check {
                            let ts_secs = Utc::now().timestamp();
                            let outcome =
                                crate::pre_event_flatten::evaluate_flatten_trigger_from_env(
                                    symbol,
                                    ts_secs,
                                    crate::pre_event_flatten::DEFAULT_FLATTEN_LEAD_SECS,
                                );
                            if outcome.is_flatten_required() || outcome.is_unavailable() {
                                let (key, order_json) =
                                    crate::pre_event_flatten::build_flatten_close_order_json(
                                        symbol, *net_qty, ts_secs, run_id,
                                    );
                                match mqk_db::outbox_enqueue(pool, run_id, &key, order_json).await
                                {
                                    Ok(true) => {
                                        tracing::warn!(
                                            run_id = %run_id,
                                            symbol = %symbol,
                                            net_qty = %net_qty,
                                            idempotency_key = %key,
                                            "pre_event_flatten_close_enqueued"
                                        );
                                    }
                                    Ok(false) => {
                                        tracing::debug!(
                                            run_id = %run_id,
                                            symbol = %symbol,
                                            idempotency_key = %key,
                                            "pre_event_flatten_close_already_pending"
                                        );
                                    }
                                    Err(err) => {
                                        tracing::warn!(
                                            run_id = %run_id,
                                            symbol = %symbol,
                                            error = %err,
                                            "pre_event_flatten_close_enqueue_failed"
                                        );
                                    }
                                }
                            }
                        }
                    }

                    // B1C: Dispatch pending strategy bar input and submit Live-intent
                    // decisions through the canonical internal admission seam.
                    //
                    // The execution loop is the canonical runtime-owned `on_bar`
                    // dispatch owner.  The signal route only deposits bar input;
                    // on_bar fires here, in the loop's tick context, after the
                    // orchestrator tick and snapshot are settled.
                    //
                    // TargetPosition.qty is a target portfolio state; order qty
                    // is the delta against current holdings (from the execution
                    // snapshot built above).  If no snapshot is available yet
                    // (rare: first-tick snapshot failure), decisions are skipped
                    // this tick rather than assuming a flat position — fail-closed.
                    //
                    // MULTI-SYMBOL-DISPATCH-LOOP-01: dispatch is a per-symbol loop
                    // over `multi_symbol_assignments` (artifact order, design doc
                    // §5 Q4), built once at loop startup. For the legacy
                    // EnvSingleSymbolFallback config (exactly one assignment, the
                    // MQK_STRATEGY_SYMBOL / MQK_STRATEGY_MD_TIMEFRAME pair) this is
                    // behaviorally identical to the prior single-dispatch call.
                    //
                    // Returns no results on most ticks (no pending bar) and when no
                    // active bootstrap exists — both are fail-closed, not errors.
                    // Shadow-mode results produce no decisions (fail-closed).
                    // RUNTIME-OPPORTUNITY-ALLOCATION-01 authority repair
                    // (Phase A): use the bar-facts-carrying dispatch seam so
                    // each symbol's exact evaluated-bar identity/close is
                    // available below, without a second DB fetch.
                    let dispatch_results = state_arc
                        .tick_strategy_dispatch_multi_symbol_with_bar_facts(
                            &multi_symbol_assignments,
                        )
                        .await;
                    if !dispatch_results.is_empty() {
                        let now_micros = Utc::now().timestamp_micros(); // allow: loop-context wall-clock for decision_id
                        // Derive current position truth from the execution snapshot
                        // settled above.  Symbols absent from the map are flat (qty=0).
                        // Q2: one shared snapshot read covers every symbol dispatched
                        // this tick — no torn-snapshot race across symbols.
                        let current_positions: Option<BTreeMap<String, i64>> = {
                            let snap = snapshot_cache.read().await;
                            snap.as_ref().map(|s| {
                                s.portfolio
                                    .positions
                                    .iter()
                                    .map(|p| (p.symbol.clone(), p.net_qty))
                                    .collect()
                            })
                        };
                        let Some(current_positions) = current_positions else {
                            tracing::warn!(
                                run_id = %run_id,
                                "b1c_skip_no_snapshot: execution snapshot absent; \
                                 native strategy decisions skipped this tick"
                            );
                            continue;
                        };

                        // MULTI-SYMBOL-CAPITAL-CAPS-01 cap #6
                        // (max_new_orders_per_tick, design doc §6): per-tick
                        // counter of accepted decisions, incremented below as
                        // each symbol's decisions are submitted. `None`
                        // (MQK_MAX_NEW_ORDERS_PER_TICK unset) is unbounded —
                        // the default, matching today's behavior where every
                        // configured symbol is dispatched every tick.
                        let max_new_orders_per_tick_cap =
                            state_arc.max_new_orders_per_tick().await;
                        let mut new_orders_this_tick: u32 = 0;

                        // RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase F: decisions
                        // are now collected across every symbol in this tick
                        // before any of them are submitted, so Bundle 5's
                        // opportunity allocator can run once over the whole
                        // same-cycle candidate set. Cap #6
                        // (max_new_orders_per_tick) necessarily moves with the
                        // submission step below (it counts *accepted*
                        // decisions, which can only happen at submission
                        // time) — see runtime_opportunity_allocation.rs's
                        // module docs for why this is unavoidable and why the
                        // cap's effect (same running count, same order, same
                        // "max_new_orders_per_tick_reached" reason) is
                        // unchanged.
                        let mut all_decisions: Vec<
                            crate::runtime_opportunity_allocation::PendingDecisionWithBarFacts,
                        > = Vec::new();

                        for (assignment, mut bar_result, bar_facts) in dispatch_results {
                            let strategy_id = bar_result.spec.name.clone();

                            // MULTI-SYMBOL-DISPATCH-LOOP-01 fail-closed symbol guard:
                            // the native strategy bootstrap's StrategyHost emits
                            // TargetPosition.symbol fixed at construction time from
                            // MQK_STRATEGY_SYMBOL, independent of which symbol's
                            // bar window was just dispatched. A target whose
                            // symbol does not match the assignment being dispatched
                            // would otherwise carry a qty computed from a *different*
                            // symbol's bars under the dispatched symbol's name — drop
                            // it rather than submit a misattributed decision. See
                            // docs/design/native_multi_symbol_dispatch.md (per-symbol
                            // strategy bootstrap gap).
                            let dropped = AppState::retain_targets_matching_symbol(
                                &mut bar_result.intents.output.targets,
                                &assignment.symbol,
                            );
                            if dropped > 0 {
                                tracing::warn!(
                                    run_id = %run_id,
                                    dispatched_symbol = %assignment.symbol,
                                    dropped_targets = dropped,
                                    "b1c_symbol_mismatch_skipped: strategy-emitted target \
                                     symbol does not match the dispatched assignment; \
                                     per-symbol strategy bootstrap not yet implemented"
                                );
                                if bar_result.intents.output.targets.is_empty() {
                                    let current = current_positions
                                        .get(&assignment.symbol)
                                        .copied()
                                        .unwrap_or(0);
                                    state_arc
                                        .record_per_symbol_target_state(
                                            build_per_symbol_target_state(
                                                assignment.symbol.clone(),
                                                strategy_id.clone(),
                                                current,
                                                current,
                                                "symbol_mismatch_skipped",
                                            ),
                                        )
                                        .await;
                                }
                            }

                            // MULTI-SYMBOL-CAPITAL-CAPS-01 cap #2
                            // (per_symbol_max_position_qty, design doc §6):
                            // clamp any remaining target whose |qty| exceeds the
                            // configured per-symbol position cap, preserving
                            // sign, before delta/decision derivation. Disabled
                            // (no-op) when MQK_PER_SYMBOL_MAX_POSITION_QTY is
                            // unset — the default.
                            if let Some(cap) = state_arc.per_symbol_max_position_qty().await {
                                let clamped = AppState::clamp_targets_to_per_symbol_position_cap(
                                    &mut bar_result.intents.output.targets,
                                    cap,
                                );
                                for (symbol, original_qty, clamped_qty) in clamped {
                                    tracing::warn!(
                                        run_id = %run_id,
                                        symbol = %symbol,
                                        original_qty,
                                        clamped_qty,
                                        cap,
                                        "b1c_target_qty_clamped_per_symbol_cap: strategy target \
                                         qty exceeds per_symbol_max_position_qty; clamped to cap"
                                    );
                                    if state_arc
                                        .try_claim_per_symbol_position_cap_alert(&symbol)
                                        .await
                                    {
                                        let notifier = state_arc.discord_notifier.clone();
                                        let env = Some(
                                            state_arc.deployment_mode().as_api_label().to_string(),
                                        );
                                        let run_id_short = format!("{:.8}", run_id.to_string());
                                        let symbol_owned = symbol.clone();
                                        let ts = chrono::Utc::now().to_rfc3339(); // allow: ops-metadata notification timestamp
                                        tokio::spawn(async move {
                                            notifier
                                                .notify_trade_event(&crate::notify::TradeEventPayload {
                                                    stage: "signal.blocked".to_string(),
                                                    run_id: Some(run_id_short.clone()),
                                                    symbol: Some(symbol_owned.clone()),
                                                    side: None,
                                                    qty: Some(clamped_qty),
                                                    price_micros: None,
                                                    order_id: None,
                                                    detail: Some(format!(
                                                        "gate=per_symbol_max_position_qty_cap \
                                                         original_qty={original_qty} \
                                                         clamped_qty={clamped_qty} cap={cap}"
                                                    )),
                                                    environment: env,
                                                    summary: format!(
                                                        "signal.blocked [per_symbol_max_position_qty_cap] \
                                                         symbol={symbol_owned} run={run_id_short} \
                                                         original_qty={original_qty} \
                                                         clamped_to={clamped_qty} (cap={cap})"
                                                    ),
                                                    ts_utc: ts,
                                                })
                                                .await;
                                        });
                                    }
                                }
                            }

                            // AUTON-NO-TRADE-01: record signal qty before decisions are
                            // derived. This is the raw strategy output — zero means the
                            // strategy returned hold/flat for all targets this tick.
                            let raw_signal_qty: i64 = bar_result
                                .intents
                                .output
                                .targets
                                .iter()
                                .map(|t| t.qty)
                                .sum();
                            state_arc.record_bar_tick_outcome(raw_signal_qty);

                            if bar_result.intents.output.targets.is_empty() && dropped == 0 {
                                let current =
                                    current_positions.get(&assignment.symbol).copied().unwrap_or(0);
                                state_arc
                                    .record_per_symbol_target_state(build_per_symbol_target_state(
                                        assignment.symbol.clone(),
                                        strategy_id.clone(),
                                        current,
                                        current,
                                        "no_decisions",
                                    ))
                                    .await;
                            }

                            // STRATEGY-SIZING-AND-EXIT-AUDIT-01: log target vs current
                            // position diagnostics before computing decisions.  This surfaces
                            // the no_order_reason=already_at_target case in operator logs.
                            // DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: fire one Discord alert
                            // per (run, symbol) when B5 short-sale guard blocks a sell.
                            for t in &bar_result.intents.output.targets {
                                let symbol = t.symbol.clone();
                                let target_qty = t.qty;
                                let current = current_positions.get(&symbol).copied().unwrap_or(0);
                                let delta = target_qty - current;
                                let no_order_reason = if delta == 0 {
                                    "already_at_target"
                                } else if delta < 0 && (current <= 0 || (-delta) > current) {
                                    "b5_short_sale_guard"
                                } else {
                                    "order_will_be_submitted"
                                };
                                tracing::info!(
                                    run_id = %run_id,
                                    symbol = %t.symbol,
                                    strategy_target_qty = t.qty,
                                    current_position_qty = current,
                                    computed_delta_qty = delta,
                                    no_order_reason,
                                    "b1c_position_delta_diagnostic"
                                );
                                state_arc
                                    .record_per_symbol_target_state(build_per_symbol_target_state(
                                        symbol.clone(),
                                        strategy_id.clone(),
                                        current,
                                        target_qty,
                                        no_order_reason,
                                    ))
                                    .await;
                                if no_order_reason == "b5_short_sale_guard"
                                    && state_arc.try_claim_b5_alert(&symbol).await
                                {
                                    let notifier = state_arc.discord_notifier.clone();
                                    let env = Some(
                                        state_arc.deployment_mode().as_api_label().to_string(),
                                    );
                                    let run_id_short = format!("{:.8}", run_id.to_string());
                                    let qty_to_sell = -delta;
                                    let ts = chrono::Utc::now().to_rfc3339(); // allow: ops-metadata notification timestamp
                                    tokio::spawn(async move {
                                        notifier
                                            .notify_trade_event(&crate::notify::TradeEventPayload {
                                                stage: "signal.blocked".to_string(),
                                                run_id: Some(run_id_short.clone()),
                                                symbol: Some(symbol.clone()),
                                                side: Some("sell".to_string()),
                                                qty: Some(qty_to_sell),
                                                price_micros: None,
                                                order_id: None,
                                                detail: Some(format!(
                                                    "gate=b5_short_sale_guard \
                                                     current={current} target_delta={delta}"
                                                )),
                                                environment: env,
                                                summary: format!(
                                                    "signal.blocked [b5_short_sale_guard] \
                                                     symbol={symbol} run={run_id_short} \
                                                     qty_to_sell={qty_to_sell} current={current} \
                                                     (no long position to sell against)"
                                                ),
                                                ts_utc: ts,
                                            })
                                            .await;
                                    });
                                }
                            }

                            // MULTI-STRATEGY-RUNTIME-DRY-RUN-01: optional dry-run secondary
                            // strategy diagnostics. Default-off (dry_run_strategy_ids empty
                            // unless MQK_DRY_RUN_STRATEGY_IDS is set) — no-op in that case.
                            //
                            // Rides along with the primary dispatch for this assignment only:
                            // re-fetches the same md_bars window the primary strategy was just
                            // evaluated against (read-only; same call shape as
                            // `dispatch_native_strategy_for_symbol_with_loaded_bars`). The
                            // evaluator (`evaluate_dry_run_strategies`) takes no DB/AppState/
                            // broker handle, so it cannot submit a decision or write an outbox
                            // row — see `state/dry_run_strategy.rs` for the structural proof.
                            if !dry_run_strategy_ids.is_empty() {
                                if let Some(ref pool) = db {
                                    match mqk_db::fetch_recent_completed_bars_for_strategy(
                                        pool,
                                        &assignment.symbol,
                                        &assignment.timeframe,
                                        STRATEGY_CONTEXT_LOAD_LIMIT,
                                    )
                                    .await
                                    {
                                        Ok(db_bars) if !db_bars.is_empty() => {
                                            let stubs: Vec<mqk_strategy::BarStub> = db_bars
                                                .iter()
                                                .map(|b| {
                                                    mqk_strategy::BarStub::new(
                                                        b.end_ts,
                                                        b.is_complete,
                                                        b.close_micros,
                                                        b.volume,
                                                    )
                                                })
                                                .collect();
                                            let window = mqk_strategy::RecentBarsWindow::new(
                                                stubs.len().max(1),
                                                stubs,
                                            );
                                            let dry_run_current = current_positions
                                                .get(&assignment.symbol)
                                                .copied()
                                                .unwrap_or(0);
                                            let dry_run_diags = evaluate_dry_run_strategies(
                                                &dry_run_strategy_ids,
                                                &assignment.symbol,
                                                0,
                                                &window,
                                                dry_run_current,
                                            );
                                            for diag in &dry_run_diags {
                                                tracing::info!(
                                                    run_id = %run_id,
                                                    symbol = %diag.symbol,
                                                    strategy_id = %diag.strategy_id,
                                                    timeframe_secs = diag.timeframe_secs,
                                                    current_qty = diag.current_qty,
                                                    target_qty = diag.target_qty,
                                                    delta_qty = diag.delta_qty,
                                                    decision = diag.decision,
                                                    would_classify_as = diag.would_classify_as,
                                                    would_b5_block = diag.would_b5_block,
                                                    would_policy_block = diag.would_policy_block,
                                                    policy_reason_code = ?diag.policy_reason_code,
                                                    submitted = diag.submitted,
                                                    reason = %diag.reason,
                                                    "multi_strategy_dry_run_diagnostic"
                                                );
                                            }
                                            // MULTI-STRATEGY-DRY-RUN-STATUS-01: store the latest
                                            // snapshot for operator-visible status surfacing
                                            // (GET /api/v1/strategy/dry-run/status). Replaces the
                                            // prior snapshot wholesale — see
                                            // `AppState::set_dry_run_diagnostics`. This is the
                                            // only place this snapshot is written; it is never
                                            // read by any decision/submission path.
                                            state_arc
                                                .set_dry_run_diagnostics(
                                                    dry_run_diags,
                                                    Utc::now().timestamp(),
                                                )
                                                .await;
                                        }
                                        Ok(_) => {
                                            // No completed bars yet for this symbol/timeframe;
                                            // nothing to evaluate this tick.
                                        }
                                        Err(err) => {
                                            tracing::warn!(
                                                run_id = %run_id,
                                                symbol = %assignment.symbol,
                                                error = %err,
                                                "multi_strategy_dry_run_bar_load_failed"
                                            );
                                        }
                                    }
                                }
                            }

                            let decisions = crate::decision::bar_result_to_decisions(
                                &bar_result,
                                run_id,
                                now_micros,
                                &current_positions,
                            );
                            // AUTON-NO-TRADE-01: log when strategy produced no admissible
                            // decisions.  This is honest: signal=0 means hold/flat or the
                            // strategy's conditions (lookback, completeness, price guards)
                            // were not satisfied.  No fabrication; no forced trades.
                            if decisions.is_empty() {
                                tracing::info!(
                                    run_id = %run_id,
                                    raw_signal_qty,
                                    bar_tick_count = state_arc.bar_tick_dispatch_count(),
                                    "b1c_bar_tick_no_decisions: strategy returned no admissible \
                                     decisions this tick (signal_qty={raw_signal_qty}; \
                                     check b1c_position_delta_diagnostic for no_order_reason)"
                                );
                            }
                            // RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase F:
                            // collect this symbol's decisions rather than
                            // submitting them immediately — the whole tick's
                            // decisions are batched below so the opportunity
                            // allocator sees every same-cycle candidate at
                            // once (one call per tick, never one call per
                            // symbol).
                            //
                            // Authority repair Phase A: every decision
                            // derived from this one `bar_result` shares the
                            // exact same evaluated-bar facts (`bar_facts`) —
                            // captured once, at dispatch time, above; never
                            // re-fetched here or later.
                            all_decisions.extend(decisions.into_iter().map(|decision| {
                                crate::runtime_opportunity_allocation::PendingDecisionWithBarFacts {
                                    decision,
                                    bar_facts: bar_facts.clone(),
                                }
                            }));
                        }

                        // MULTI-STRATEGY-CONFLICT-POLICY-01 Phase B: resolve
                        // same-symbol conflicts across this tick's whole
                        // batch, once, before Bundle 5's opportunity
                        // allocation ever sees it. When
                        // MQK_STRATEGY_CONFLICT_POLICY_MODE=off (the
                        // default), this is a zero-cost passthrough — no
                        // candidate construction, no conflict-plan DB write,
                        // `all_decisions` returned unchanged and in original
                        // order. The current one-strategy-per-symbol runtime
                        // means this is normally a no-op even in shadow/
                        // paper_enforced mode (see
                        // docs/specs/multi_strategy_conflict_policy_01a):
                        // today at most one candidate ever competes per
                        // symbol per tick.
                        let market_date_today = Utc::now().format("%Y-%m-%d").to_string();
                        let dispatch_timeframe = multi_symbol_assignments
                            .first()
                            .map(|a| a.timeframe.clone())
                            .unwrap_or_default();
                        let conflict_outcome = crate::runtime_strategy_conflict::gather_and_resolve(
                            &state_arc,
                            run_id,
                            now_micros,
                            market_date_today.clone(),
                            dispatch_timeframe.clone(),
                            all_decisions,
                            &current_positions,
                        )
                        .await;
                        // MULTI-STRATEGY-CONFLICT-POLICY-01 Phase C: the
                        // conflict plan (when Some) is persisted as durable
                        // evidence inside gather_and_resolve, best-effort —
                        // nothing further to do with it here.

                        // RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase F: one
                        // allocation call for this tick's whole batch. When
                        // MQK_RUNTIME_OPPORTUNITY_ALLOCATION_MODE=off (the
                        // default) or there are no buy-side decisions this
                        // tick, this is a zero-cost passthrough — no I/O, no
                        // allocator call, `all_decisions` returned unchanged.
                        let allocation_outcome =
                            crate::runtime_opportunity_allocation::gather_and_apply(
                                &state_arc,
                                run_id,
                                now_micros,
                                market_date_today,
                                dispatch_timeframe,
                                conflict_outcome.decisions,
                                &current_positions,
                            )
                            .await;
                        // RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase G: the plan
                        // (when Some) is already persisted as durable
                        // evidence inside gather_and_apply, best-effort —
                        // nothing further to do with it here.

                        for decision in allocation_outcome.decisions {
                            // MULTI-SYMBOL-CAPITAL-CAPS-01 cap #6: relocated
                            // here (submission time) because it counts
                            // *accepted* decisions — see this file's
                            // "Phase F" comment above the `all_decisions`
                            // declaration for why. Same running count, same
                            // dispatch order, same reason string as before.
                            if AppState::max_new_orders_per_tick_reason(
                                new_orders_this_tick,
                                max_new_orders_per_tick_cap,
                            )
                            .is_some()
                            {
                                let no_order_reason = "max_new_orders_per_tick_reached";
                                tracing::warn!(
                                    run_id = %run_id,
                                    symbol = %decision.symbol,
                                    new_orders_this_tick,
                                    cap = ?max_new_orders_per_tick_cap,
                                    no_order_reason,
                                    "b1c_symbol_skipped_max_new_orders_per_tick: per-tick \
                                     new-order cap reached; decision skipped this tick, \
                                     re-evaluated next tick"
                                );
                                let current = current_positions
                                    .get(&decision.symbol)
                                    .copied()
                                    .unwrap_or(0);
                                state_arc
                                    .record_per_symbol_target_state(build_per_symbol_target_state(
                                        decision.symbol.clone(),
                                        decision.strategy_id.clone(),
                                        current,
                                        current,
                                        no_order_reason,
                                    ))
                                    .await;
                                continue;
                            }

                            let did = decision.decision_id.clone();
                            let sid = decision.strategy_id.clone();
                            let decision_symbol = decision.symbol.clone();
                            let decision_side = decision.side.clone();
                            let decision_qty = decision.qty;
                            let outcome = crate::decision::submit_internal_strategy_decision(
                                &state_arc,
                                decision,
                            )
                            .await;
                            let mut target_state = state_arc
                                .per_symbol_target_state_for_symbol(&decision_symbol)
                                .await
                                .unwrap_or_else(|| {
                                    let current = current_positions
                                        .get(&decision_symbol)
                                        .copied()
                                        .unwrap_or(0);
                                    let target = match decision_side
                                        .trim()
                                        .to_ascii_lowercase()
                                        .as_str()
                                    {
                                        "sell" => current - decision_qty,
                                        _ => current + decision_qty,
                                    };
                                    build_per_symbol_target_state(
                                        decision_symbol.clone(),
                                        sid.clone(),
                                        current,
                                        target,
                                        "order_will_be_submitted",
                                    )
                                });
                            target_state.last_decision_id = Some(did.clone());
                            target_state.last_decision_disposition =
                                Some(outcome.disposition.clone());
                            target_state.updated_at_utc = Utc::now().to_rfc3339();
                            state_arc
                                .record_per_symbol_target_state(target_state)
                                .await;
                            if outcome.accepted {
                                // MULTI-SYMBOL-CAPITAL-CAPS-01 cap #6: count
                                // this accepted decision toward the per-tick
                                // new-order cap so later decisions in this
                                // tick's dispatch order can be skipped once
                                // the cap is reached.
                                new_orders_this_tick += 1;
                                tracing::info!(
                                    run_id = %run_id,
                                    decision_id = %did,
                                    strategy_id = %sid,
                                    "b1c_native_decision_accepted"
                                );
                            } else {
                                tracing::warn!(
                                    run_id = %run_id,
                                    decision_id = %did,
                                    strategy_id = %sid,
                                    disposition = %outcome.disposition,
                                    "b1c_native_decision_not_accepted"
                                );
                            }
                        }
                    }
                }
            }
        }

        if let Err(err) = orchestrator.release_runtime_leadership().await {
            tracing::warn!("runtime_lease_release_failed error={err}");
        }

        let exit = ExecutionLoopExit {
            note: Some("execution loop stopped".to_string()),
        };
        drop_outside_async_context(orchestrator);
        exit
    });

    ExecutionLoopHandle {
        run_id,
        stop_tx,
        join_handle,
    }
}

fn build_per_symbol_target_state(
    symbol: String,
    strategy_id: String,
    current_qty: i64,
    target_qty: i64,
    no_order_reason: &str,
) -> PerSymbolTargetState {
    PerSymbolTargetState {
        symbol,
        strategy_id,
        current_qty,
        target_qty,
        delta: target_qty - current_qty,
        no_order_reason: no_order_reason.to_string(),
        last_decision_id: None,
        last_decision_disposition: None,
        updated_at_utc: Utc::now().to_rfc3339(),
    }
}

// ---------------------------------------------------------------------------
// drop_outside_async_context
// ---------------------------------------------------------------------------

/// Move `val` onto a fresh OS thread for dropping.
///
/// `reqwest::blocking::Client` (embedded in `AlpacaBrokerAdapter`) holds an
/// internal `tokio::runtime::Runtime`.  Dropping that runtime inside a Tokio
/// task panics on schedulers where blocking is not allowed (including the
/// `current_thread` scheduler used by `#[tokio::test]`).  This helper ensures
/// the drop happens off the async executor so no Tokio context is active.
///
/// The thread is detached; callers must not rely on the drop completing before
/// the current task continues.  For ordered shutdown, join the spawned handle
/// if synchronisation is needed.
fn drop_outside_async_context<T: Send + 'static>(val: T) {
    std::thread::spawn(move || drop(val));
}

// ---------------------------------------------------------------------------
// spawn_reconcile_tick
// ---------------------------------------------------------------------------

/// Spawn a background task that periodically runs a reconcile tick (R3-1).
///
/// `settle_fn` — returns `true` when a terminal-fill broker-snapshot settle
/// window is active (RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01).  When true and
/// reconcile is dirty, the background tick defers the disarm rather than
/// immediately halting, mirroring the orchestrator's Phase 0c deferral.
pub fn spawn_reconcile_tick<L, B, S>(
    state: Arc<AppState>,
    local_fn: L,
    broker_fn: B,
    settle_fn: S,
    interval: Duration,
) where
    L: Fn() -> mqk_reconcile::LocalSnapshot + Send + 'static,
    B: Fn() -> Option<mqk_reconcile::BrokerSnapshot> + Send + 'static,
    S: Fn() -> bool + Send + 'static,
{
    tokio::spawn(async move {
        let start = tokio::time::Instant::now() + interval;
        let mut ticker = tokio::time::interval_at(start, interval);
        let mut watermark = SnapshotWatermark::new();
        loop {
            ticker.tick().await;
            let local = local_fn();
            let Some(broker) = broker_fn() else {
                let previous = state.current_reconcile_snapshot().await;
                let reconcile = if previous.status == "dirty" {
                    preserve_fail_closed_reconcile_status(
                        &previous,
                        "broker snapshot absent; retaining prior dirty reconcile state under fail-closed semantics",
                    )
                } else {
                    reconcile_unknown_status(
                        "broker snapshot absent; reconcile ordering is not proven and remains fail-closed",
                    )
                };
                publish_reconcile_failure(
                    &state,
                    reconcile,
                    "reconcile broker snapshot absent - system disarmed (REC-01R)",
                )
                .await;
                continue;
            };

            match mqk_reconcile::reconcile_monotonic(&mut watermark, &local, &broker) {
                Ok(report) if report.is_clean() => {
                    // DISCORD-TRADE-LIFECYCLE-ALERTS-01: alert on dirty→clean transition only.
                    // Read previous status before publishing so we can detect the transition.
                    let prev_status = state.current_reconcile_snapshot().await.status;
                    state
                        .publish_reconcile_snapshot(reconcile_status_from_report(
                            &report, &broker, &watermark,
                        ))
                        .await;
                    if prev_status != "ok" {
                        let notifier = state.discord_notifier.clone();
                        let env = Some(state.deployment_mode().as_api_label().to_string());
                        let prior = prev_status.clone();
                        let ts = chrono::Utc::now().to_rfc3339();
                        tokio::spawn(async move {
                            notifier
                                .notify_trade_event(&crate::notify::TradeEventPayload {
                                    stage: "reconcile.clean".to_string(),
                                    run_id: None,
                                    symbol: None,
                                    side: None,
                                    qty: None,
                                    price_micros: None,
                                    order_id: None,
                                    detail: Some(format!("transitioned_from={prior}")),
                                    environment: env,
                                    summary: format!("reconcile clean after {prior}"),
                                    ts_utc: ts,
                                })
                                .await;
                        });
                    }
                }
                Ok(report) => {
                    // RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01: if a terminal fill
                    // was applied within the settle grace window, defer this tick's
                    // disarm rather than halting immediately.  The orchestrator's
                    // Phase 0c carries the same deferral; both must agree to avoid
                    // the background tick racing ahead and halting first.
                    if settle_fn() {
                        tracing::debug!(
                            "reconcile_tick_deferred_terminal_fill_settle: \
                             dirty reconcile within fill settle window; \
                             skipping disarm this tick \
                             (RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01)"
                        );
                    } else {
                        publish_reconcile_failure(
                            &state,
                            reconcile_status_from_report(&report, &broker, &watermark),
                            "reconcile drift detected - system disarmed (REC-01R)",
                        )
                        .await;
                    }
                }
                Err(stale) => {
                    let previous = state.current_reconcile_snapshot().await;
                    let reconcile = if previous.status == "dirty" {
                        preserve_fail_closed_reconcile_status(
                            &previous,
                            format!(
                                "stale broker snapshot rejected; retaining prior dirty reconcile state: {}",
                                reconcile_status_from_stale(&stale, &watermark)
                                    .note
                                    .unwrap_or_else(|| "stale broker snapshot rejected".to_string())
                            ),
                        )
                    } else {
                        reconcile_status_from_stale(&stale, &watermark)
                    };
                    publish_reconcile_failure(
                        &state,
                        reconcile,
                        "stale broker snapshot rejected by monotonic reconcile - system disarmed (REC-01R)",
                    )
                    .await;
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// publish_reconcile_failure
// ---------------------------------------------------------------------------

pub(super) async fn publish_reconcile_failure(
    state: &Arc<AppState>,
    reconcile: ReconcileStatusSnapshot,
    note: &str,
) {
    state.publish_reconcile_snapshot(reconcile).await;
    {
        let mut ig = state.integrity.write().await;
        ig.disarmed = true;
        ig.halted = true;
    }

    if let Some(db) = state.db.as_ref() {
        if let Err(e) = mqk_db::persist_arm_state_canonical(
            db,
            mqk_db::ArmState::Disarmed,
            Some(mqk_db::DisarmReason::ReconcileDrift),
        )
        .await
        {
            tracing::error!(
                "reconcile_disarm_persist_failed: durable disarm not written; error={e}"
            );
        }
        if let Err(e) =
            mqk_db::persist_risk_block_state(db, true, Some("RECONCILE_BLOCKED"), Utc::now()).await
        {
            tracing::error!(
                "reconcile_risk_block_persist_failed: risk block not written; error={e}"
            );
        }
    }

    let active_run_id = state.status.read().await.active_run_id;
    let snapshot = StatusSnapshot {
        daemon_uptime_secs: uptime_secs(),
        active_run_id,
        state: "halted".to_string(),
        notes: Some(note.to_string()),
        integrity_armed: false,
        deadman_status: "unknown".to_string(),
        deadman_last_heartbeat_utc: None,
    };
    state.publish_status(snapshot).await;
    let _ = state.bus.send(BusMsg::LogLine {
        level: "ERROR".to_string(),
        msg: note.to_string(),
    });
}
