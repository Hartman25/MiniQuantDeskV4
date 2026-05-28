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
use super::snapshot::{
    outbox_json_side, preserve_fail_closed_reconcile_status, reconcile_status_from_report,
    reconcile_status_from_stale, reconcile_unknown_status,
    synthesize_broker_snapshot_from_execution,
};
use super::types::{
    BrokerSnapshotTruthSource, BusMsg, DaemonOrchestrator, ExecutionLoopCommand, ExecutionLoopExit,
    ExecutionLoopHandle, ReconcileStatusSnapshot, StatusSnapshot,
};
use super::{AppState, DEADMAN_TTL_SECONDS, EXECUTION_LOOP_INTERVAL};

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
                            let adapter_opt: Option<std::sync::Arc<mqk_broker_alpaca::AlpacaBrokerAdapter>> = {
                                let guard = state_arc.external_snapshot_refresher.read().await;
                                guard.as_ref().cloned()
                            };
                            if let Some(adapter) = adapter_opt {
                                let now = Utc::now();
                                match tokio::task::block_in_place(|| {
                                    adapter.fetch_broker_snapshot(now)
                                }) {
                                    Ok(fresh) => {
                                        *broker_snapshot_cache.write().await = Some(fresh);
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
                    // Returns None on most ticks (no pending bar) and when no
                    // active bootstrap exists — both are fail-closed, not errors.
                    // Shadow-mode results produce no decisions (fail-closed).
                    if let Some(bar_result) = state_arc.tick_strategy_dispatch().await {
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

                        let now_micros = Utc::now().timestamp_micros(); // allow: loop-context wall-clock for decision_id
                        // Derive current position truth from the execution snapshot
                        // settled above.  Symbols absent from the map are flat (qty=0).
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
                                 check bar lookback, is_complete, and price availability)"
                            );
                        }
                        for decision in decisions {
                            let did = decision.decision_id.clone();
                            let sid = decision.strategy_id.clone();
                            let outcome = crate::decision::submit_internal_strategy_decision(
                                &state_arc,
                                decision,
                            )
                            .await;
                            if outcome.accepted {
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
pub fn spawn_reconcile_tick<L, B>(
    state: Arc<AppState>,
    local_fn: L,
    broker_fn: B,
    interval: Duration,
) where
    L: Fn() -> mqk_reconcile::LocalSnapshot + Send + 'static,
    B: Fn() -> Option<mqk_reconcile::BrokerSnapshot> + Send + 'static,
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
                    state
                        .publish_reconcile_snapshot(reconcile_status_from_report(
                            &report, &broker, &watermark,
                        ))
                        .await;
                }
                Ok(report) => {
                    publish_reconcile_failure(
                        &state,
                        reconcile_status_from_report(&report, &broker, &watermark),
                        "reconcile drift detected - system disarmed (REC-01R)",
                    )
                    .await;
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
