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
use std::sync::atomic::Ordering;
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
use crate::dynamic_selection_dispatch_authority::{
    DynamicSelectionDispatchProvenance, RuntimeStrategyDispatchAuthority,
};
use crate::notify::CriticalAlertPayload;
use crate::runtime_opportunity_allocation::PendingDecisionWithBarFacts;

use super::{
    dry_run_strategy_ids_from_env, evaluate_dry_run_strategies, AppState, PerSymbolTargetState,
    DEADMAN_TTL_SECONDS, EXECUTION_LOOP_INTERVAL, STRATEGY_CONTEXT_LOAD_LIMIT,
};

/// TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 Blocker 1: validates one
/// decision envelope's `dynamic_selection_provenance` directly against the
/// active, frozen [`RuntimeStrategyDispatchAuthority`] — never against a
/// detached, separately-reconstructed `(symbol, strategy_id)` map. `Legacy`
/// (Off/Shadow) requires `None`. `DynamicPaperEnforced` requires `Some` whose
/// `run_id`, `plan_id`, canonical symbol, `strategy_id`, and
/// `timeframe_secs` all match the active authority, AND whose
/// `(symbol, strategy_id, timeframe_secs, plan_id)` names a real selected
/// binding in `bindings` — an altered timeframe, wrong run/plan binding, or
/// a reconstructed identity cannot pass merely because a `(symbol,
/// strategy_id)` key happens to exist somewhere.
fn dynamic_selection_envelope_ok(
    dispatch_authority: &RuntimeStrategyDispatchAuthority,
    envelope: &PendingDecisionWithBarFacts,
) -> bool {
    match dispatch_authority {
        RuntimeStrategyDispatchAuthority::Legacy { .. } => {
            envelope.dynamic_selection_provenance.is_none()
        }
        RuntimeStrategyDispatchAuthority::DynamicPaperEnforced {
            run_id: authority_run_id,
            plan_id: authority_plan_id,
            bindings,
            ..
        } => {
            let Some(p) = &envelope.dynamic_selection_provenance else {
                return false;
            };
            if p.run_id != *authority_run_id {
                return false;
            }
            if p.plan_id != *authority_plan_id {
                return false;
            }
            let canonical_provenance_symbol = mqk_portfolio::canonical_symbol(&p.symbol);
            let canonical_decision_symbol =
                mqk_portfolio::canonical_symbol(&envelope.decision.symbol);
            if canonical_provenance_symbol != canonical_decision_symbol {
                return false;
            }
            if p.strategy_id != envelope.decision.strategy_id {
                return false;
            }
            if p.timeframe_secs != envelope.decision.timeframe_secs {
                return false;
            }
            let binding_exists = bindings.iter().any(|b| {
                mqk_portfolio::canonical_symbol(&b.symbol) == canonical_provenance_symbol
                    && b.strategy_id == p.strategy_id
                    && b.timeframe_secs == p.timeframe_secs
                    && b.plan_id == p.plan_id
            });
            if !binding_exists {
                return false;
            }
            // Bar facts, when present, must still name the same decision --
            // never a substitute bar for a different symbol/strategy.
            if let Some(facts) = &envelope.bar_facts {
                if facts.symbol != envelope.decision.symbol
                    || facts.strategy_id != envelope.decision.strategy_id
                {
                    return false;
                }
            }
            true
        }
    }
}

/// Batch form of [`dynamic_selection_envelope_ok`] — every envelope in the
/// slice must pass, or the whole tick refuses closed with zero submissions.
fn dynamic_selection_envelopes_ok(
    dispatch_authority: &RuntimeStrategyDispatchAuthority,
    envelopes: &[PendingDecisionWithBarFacts],
) -> bool {
    envelopes
        .iter()
        .all(|e| dynamic_selection_envelope_ok(dispatch_authority, e))
}

// ---------------------------------------------------------------------------
// spawn_execution_loop
// ---------------------------------------------------------------------------

pub(super) fn spawn_execution_loop(
    state: Arc<AppState>,
    mut orchestrator: DaemonOrchestrator,
    run_id: Uuid,
    // PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE Part 1/3: the one
    // frozen, run-scoped strategy-dispatch authority, built exactly once by
    // `build_dynamic_selection_start_snapshot` before the startup barrier
    // below ever releases — `Legacy` (carrying the exact same frozen
    // per-symbol assignment list ATOMICITY-SINGLE-SNAPSHOT-REPAIR
    // established) for `Off`/`Shadow`, `DynamicPaperEnforced` (owning the
    // one built host pool) only for `PaperEnforcedAllowed`. Moved wholesale
    // into this task — never cloned, never rebuilt, never shared with any
    // other owner. Dropped automatically when this task exits (stop/halt/
    // shutdown/reap/panic), which drops the whole host pool with it.
    dispatch_authority: RuntimeStrategyDispatchAuthority,
    // BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement 3:
    // the startup barrier. The task waits here, before doing ANY economic
    // work (no ticker, no deadman, no tick, no outbox/broker touch), until
    // the caller (`ProductionRuntimeStartEffects::spawn_loop`) has
    // atomically installed this exact handle as `Active` for `run_id` and
    // released the barrier. Raced against `stop_rx` so a cancellation
    // (install failure, or a stop/shutdown that races in) makes the task
    // exit immediately without ever reaching the barrier's economic body —
    // no detached task, no economic work performed while still merely
    // `Starting`.
    start_barrier: tokio::sync::oneshot::Receiver<()>,
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

    // MULTI-STRATEGY-RUNTIME-DRY-RUN-01: one-time env read, mirroring
    // `multi_symbol_assignments` above. Empty unless MQK_DRY_RUN_STRATEGY_IDS
    // is explicitly set — default-off, no behavior change when absent.
    let dry_run_strategy_ids: Vec<String> = dry_run_strategy_ids_from_env();

    let join_handle = tokio::spawn(async move {
        // Requirement 3 steps 4/5/8: wait at the top of the async body,
        // strictly before the ticker (or any other economic construct) is
        // even created. `tokio::time::interval`'s first tick fires
        // immediately once created — creating it before this wait would
        // reopen the exact race this barrier exists to close.
        tokio::select! {
            barrier_result = start_barrier => {
                if barrier_result.is_err() {
                    // Sender dropped without ever releasing (install
                    // failure took the "drop, don't send" branch) — treat
                    // identically to an explicit stop: exit now, no
                    // economic work.
                    //
                    // BARRIER LEADERSHIP/TRUTH (PHASE-7A-FINAL-PRIVATE-
                    // PRODUCTION-EFFECTS-PROOF, structured in PHASE-7A-R6-
                    // EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 1): if this
                    // attempt's `start_runtime_effects` acquired the
                    // orchestrator's runtime leadership lease, dropping
                    // `orchestrator` does NOT release an async DB lease —
                    // the drop is synchronous (deliberately moved off the
                    // async executor by `drop_outside_async_context`, since
                    // it may embed a blocking-client runtime) and cannot
                    // await the release. The lease must be released
                    // explicitly, exactly once, before the drop — and the
                    // outcome must reach the caller as structured truth on
                    // `ExecutionLoopExit`, never reduced to only a log line.
                    let release_outcome = orchestrator
                        .release_runtime_leadership()
                        .await
                        .map_err(|release_err| {
                            tracing::warn!(
                                "runtime_lease_release_failed_before_barrier_release error={release_err}"
                            );
                            release_err.to_string()
                        });
                    state_arc
                        .pre_barrier_leadership_release_count
                        .fetch_add(1, Ordering::SeqCst);
                    drop_outside_async_context(orchestrator);
                    return ExecutionLoopExit {
                        note: Some(
                            "execution loop cancelled before startup barrier release".to_string(),
                        ),
                        leadership_release_outcome: Some(release_outcome),
                    };
                }
            }
            changed = stop_rx.changed() => {
                // Cancelled before the barrier ever released (e.g. a
                // concurrent stop/shutdown signaled this handle directly).
                // `changed.is_err()` (sender dropped) is also treated as a
                // stop — either way, no economic work has happened. Same
                // explicit-release requirement as the barrier-cancellation
                // branch above.
                let _ = changed;
                let release_outcome = orchestrator
                    .release_runtime_leadership()
                    .await
                    .map_err(|release_err| {
                        tracing::warn!(
                            "runtime_lease_release_failed_before_barrier_release error={release_err}"
                        );
                        release_err.to_string()
                    });
                state_arc
                    .pre_barrier_leadership_release_count
                    .fetch_add(1, Ordering::SeqCst);
                drop_outside_async_context(orchestrator);
                return ExecutionLoopExit {
                    note: Some(
                        "execution loop stopped before startup barrier release".to_string(),
                    ),
                    leadership_release_outcome: Some(release_outcome),
                };
            }
        }

        // PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 3 row 21
        // ("spawned-task panic/join failure"): a narrow, always-`false`-in-
        // production panic-injection point, checked once, strictly after
        // the startup barrier has released (so the panic proves a
        // genuinely `Active` task's join/leadership-release truth is
        // surfaced correctly, never a pre-barrier artifact). The setter is
        // `#[cfg(test)]`-gated; this check itself is unconditional (a
        // single atomic load, harmless in production) — matching the
        // existing `force_leadership_release_failure`/`force_install_
        // active_runtime_conflict` convention.
        if state_arc.execution_loop_panic_forced() {
            panic!("PHASE-7A-R6-MATRIX-CLOSURE test-injected execution loop panic");
        }

        let mut ticker = tokio::time::interval(EXECUTION_LOOP_INTERVAL);
        // AUTON-PAPER-RISK-03: countdown to next External broker snapshot refresh.
        let mut external_refresh_ticks: u32 = 0;
        // PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE Part 3: this
        // task is the one exclusive mutable owner of `dispatch_authority`
        // (and, for `DynamicPaperEnforced`, the host pool it carries) for
        // the rest of this task's life — never read or mutated from
        // anywhere else. No selector/plan-builder/promotion/evidence call
        // occurs anywhere in this loop.
        let mut dispatch_authority = dispatch_authority;
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
                                let release_outcome = orchestrator
                                    .release_runtime_leadership()
                                    .await
                                    .map_err(|release_err| {
                                        tracing::warn!("runtime_lease_release_failed error={release_err}");
                                        release_err.to_string()
                                    });
                                let exit = ExecutionLoopExit {
                                    note: Some("execution loop halted: deadman expired".to_string()),
                                    leadership_release_outcome: Some(release_outcome),
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
                                let release_outcome = orchestrator
                                    .release_runtime_leadership()
                                    .await
                                    .map_err(|release_err| {
                                        tracing::warn!("runtime_lease_release_failed error={release_err}");
                                        release_err.to_string()
                                    });
                                let exit = ExecutionLoopExit {
                                    note: Some(format!("execution loop halted: deadman check failed: {err}")),
                                    leadership_release_outcome: Some(release_outcome),
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
                        let release_outcome = orchestrator
                            .release_runtime_leadership()
                            .await
                            .map_err(|release_err| {
                                tracing::warn!("runtime_lease_release_failed error={release_err}");
                                release_err.to_string()
                            });
                        let exit = ExecutionLoopExit {
                            note: Some(
                                "execution loop halted: Alpaca WS continuity gap detected"
                                    .to_string(),
                            ),
                            leadership_release_outcome: Some(release_outcome),
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
                        let release_outcome = orchestrator
                            .release_runtime_leadership()
                            .await
                            .map_err(|release_err| {
                                tracing::warn!("runtime_lease_release_failed error={release_err}");
                                release_err.to_string()
                            });
                        let exit = ExecutionLoopExit {
                            note: Some(format!("execution loop halted: {err}")),
                            leadership_release_outcome: Some(release_outcome),
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
                            let release_outcome = orchestrator
                                .release_runtime_leadership()
                                .await
                                .map_err(|release_err| {
                                    tracing::warn!("runtime_lease_release_failed error={release_err}");
                                    release_err.to_string()
                                });
                            let exit = ExecutionLoopExit {
                                note: Some(
                                    "execution loop halted: deadman expired post-tick".to_string(),
                                ),
                                leadership_release_outcome: Some(release_outcome),
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
                            let release_outcome = orchestrator
                                .release_runtime_leadership()
                                .await
                                .map_err(|release_err| {
                                    tracing::warn!("runtime_lease_release_failed error={release_err}");
                                    release_err.to_string()
                                });
                            let exit = ExecutionLoopExit {
                                note: Some(format!("execution loop heartbeat failed: {err}")),
                                leadership_release_outcome: Some(release_outcome),
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
                    // (artifact order, design doc §5 Q4). For the legacy
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
                    //
                    // PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE Part 8:
                    // the one canonical per-tick pipeline branches here on the
                    // frozen dispatch authority — `Legacy` calls the exact
                    // pre-Phase-7B seam unchanged; `DynamicPaperEnforced` calls
                    // the selected-host backend (Part 4), which is the ONLY
                    // strategy-evaluation authority for its bindings this run
                    // (the legacy native bootstrap is never touched while this
                    // variant is active). A selected-host coherence fault
                    // (Part 5) halts/disarms fail-closed with zero decisions
                    // for the whole tick — never a fallback to legacy dispatch.
                    let dispatch_results = match &mut dispatch_authority {
                        RuntimeStrategyDispatchAuthority::Legacy { assignments } => {
                            state_arc
                                .tick_strategy_dispatch_multi_symbol_with_bar_facts(assignments)
                                .await
                        }
                        RuntimeStrategyDispatchAuthority::DynamicPaperEnforced {
                            run_id: authority_run_id,
                            bindings,
                            host_pool,
                            ..
                        } => {
                            match state_arc
                                .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                                    *authority_run_id,
                                    bindings,
                                    host_pool,
                                )
                                .await
                            {
                                Ok(results) => results,
                                Err(fault) => {
                                    // Part 5: a selected-host result coherence
                                    // mismatch is a structural fault, never an
                                    // ordinary no-signal condition — zero
                                    // decisions this tick, halt/disarm
                                    // fail-closed using the existing loop halt
                                    // authority (the same disarm/halt/
                                    // integrity/leadership-release/exit
                                    // sequence the deadman-supervisor-failure
                                    // path above already uses), never a
                                    // fallback to legacy dispatch.
                                    tracing::error!(
                                        run_id = %run_id,
                                        fault_code = fault.code(),
                                        fault = ?fault,
                                        "phase7b_selected_host_dispatch_fault: halting run"
                                    );
                                    if let Some(ref pool) = db {
                                        let now = Utc::now();
                                        if let Err(halt_err) =
                                            mqk_db::halt_run(pool, run_id, now).await
                                        {
                                            tracing::error!(run_id = %run_id, "execution_loop_halt_run_persist_failed error={halt_err}");
                                        }
                                        if let Err(disarm_err) =
                                            mqk_db::persist_arm_state_canonical(
                                                pool,
                                                mqk_db::ArmState::Disarmed,
                                                Some(
                                                    mqk_db::DisarmReason::DeadmanSupervisorFailure,
                                                ),
                                            )
                                            .await
                                        {
                                            tracing::error!(run_id = %run_id, "execution_loop_disarm_persist_failed error={disarm_err}");
                                        }
                                    }
                                    {
                                        let mut ig = integrity.write().await;
                                        ig.disarmed = true;
                                        ig.halted = true;
                                    }
                                    let release_outcome = orchestrator
                                        .release_runtime_leadership()
                                        .await
                                        .map_err(|release_err| {
                                            tracing::warn!("runtime_lease_release_failed error={release_err}");
                                            release_err.to_string()
                                        });
                                    let exit = ExecutionLoopExit {
                                        note: Some(format!(
                                            "execution loop halted: selected-host dispatch fault: {}",
                                            fault.code()
                                        )),
                                        leadership_release_outcome: Some(release_outcome),
                                    };
                                    drop_outside_async_context(orchestrator);
                                    return exit;
                                }
                            }
                        }
                    };
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

                            // STRATEGY-DECISION-IDEMPOTENCY-01: decision_id is anchored to
                            // the exact completed bar this evaluation ran against, never
                            // wall-clock `now_micros` -- see
                            // decision::decisions_from_bar_facts's doc comment for the
                            // full rationale and its fail-closed handling of a missing
                            // `bar_facts`.
                            let decisions = crate::decision::decisions_from_bar_facts(
                                &bar_result,
                                run_id,
                                bar_facts.as_ref(),
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
                            //
                            // TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01
                            // Blocker 1: every decision derived from this one
                            // `bar_result` also shares the exact same
                            // dynamic-selection provenance — populated
                            // directly from the active frozen
                            // `dispatch_authority`'s own selected binding for
                            // this exact assignment/strategy right here, at
                            // derivation time. `None` for `Legacy`. Never
                            // reattached or reconstructed after this point —
                            // it travels inside the envelope through Bundle 6
                            // and Bundle 5 unchanged.
                            let dynamic_selection_provenance_for_assignment: Option<
                                DynamicSelectionDispatchProvenance,
                            > = match &dispatch_authority {
                                RuntimeStrategyDispatchAuthority::Legacy { .. } => None,
                                RuntimeStrategyDispatchAuthority::DynamicPaperEnforced {
                                    run_id: authority_run_id,
                                    plan_id,
                                    bindings,
                                    ..
                                } => bindings
                                    .iter()
                                    .find(|b| {
                                        b.symbol == assignment.symbol
                                            && b.strategy_id == strategy_id
                                    })
                                    .map(|b| DynamicSelectionDispatchProvenance {
                                        run_id: *authority_run_id,
                                        plan_id: *plan_id,
                                        symbol: b.symbol.clone(),
                                        strategy_id: b.strategy_id.clone(),
                                        timeframe_secs: b.timeframe_secs,
                                    }),
                            };
                            // TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01
                            // Blocker 3 requirement 10: record the exact
                            // provenance this decision is derived with, so a
                            // test can later prove the same fields reach
                            // submission unchanged.
                            #[cfg(test)]
                            if let Some(p) = &dynamic_selection_provenance_for_assignment {
                                state_arc.loop_call_trace_push_for_test(format!(
                                    "derive_provenance:{}:{}:{}:{}:{}",
                                    p.symbol, p.strategy_id, p.run_id, p.plan_id, p.timeframe_secs
                                ));
                            }
                            all_decisions.extend(decisions.into_iter().map(|decision| {
                                PendingDecisionWithBarFacts {
                                    decision,
                                    bar_facts: bar_facts.clone(),
                                    dynamic_selection_provenance:
                                        dynamic_selection_provenance_for_assignment.clone(),
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

                        // PHASE-7B Part 6 / Blocker 1: validate every
                        // decision's dynamic-selection provenance against the
                        // active frozen authority before it is handed to
                        // Bundle 6 — a no-op (always true) for `Legacy`
                        // (every envelope carries `None`). Missing/mismatched
                        // provenance in `paper_enforced` fails the whole
                        // tick closed before the first submission.
                        let provenance_ok_pre_bundle6 =
                            dynamic_selection_envelopes_ok(&dispatch_authority, &all_decisions);
                        if !provenance_ok_pre_bundle6 {
                            tracing::error!(
                                run_id = %run_id,
                                "phase7b_dynamic_selection_provenance_missing_pre_bundle6: \
                                 refusing whole tick closed, zero submissions"
                            );
                            continue;
                        }

                        // FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01 Defect
                        // 1: Bundle 6 no longer receives a global timeframe
                        // at all -- its cycle identity is derived solely
                        // from canonical cycle facts and each candidate's
                        // own timeframe/bar facts (already true; unchanged
                        // by Phase 7B).
                        //
                        // TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01
                        // Blocker 3: this is the one real Bundle 6 call site
                        // for this tick.
                        #[cfg(test)]
                        state_arc.loop_call_trace_push_for_test("bundle6");
                        let conflict_outcome = crate::runtime_strategy_conflict::gather_and_resolve(
                            &state_arc,
                            run_id,
                            now_micros,
                            market_date_today.clone(),
                            all_decisions,
                            &current_positions,
                        )
                        .await;

                        // PHASE-7B Part 6 / Blocker 1: re-validate after
                        // Bundle 6 — provenance must survive conflict
                        // resolution unchanged (Bundle 6 only ever moves the
                        // whole envelope by ordinal selection; it never
                        // touches `dynamic_selection_provenance`, so it
                        // structurally cannot drop, rewrite, or swap it).
                        let provenance_ok_post_bundle6 = dynamic_selection_envelopes_ok(
                            &dispatch_authority,
                            &conflict_outcome.decisions,
                        );
                        if !provenance_ok_post_bundle6 {
                            tracing::error!(
                                run_id = %run_id,
                                "phase7b_dynamic_selection_provenance_missing_post_bundle6: \
                                 refusing whole tick closed, zero submissions"
                            );
                            continue;
                        }

                        // PHASE-7B Part 7: mixed-timeframe cycle-level
                        // authority for Bundle 5. `Legacy` is byte-identical
                        // to the pre-Phase-7B behavior (first-assignment
                        // timeframe, no per-candidate map — the existing
                        // single-`ctx.timeframe` bar-facts admission check
                        // is unchanged). `DynamicPaperEnforced` derives a
                        // truthful canonical multi-timeframe label from the
                        // sorted, deduplicated set of this tick's surviving
                        // candidates' own selected timeframes, and supplies
                        // a per-symbol expected-timeframe-label map so each
                        // candidate's bar facts are checked against *its
                        // own* selected timeframe rather than the single
                        // batch label — never allocator ranking/sizing
                        // policy, only which value the existing coherence
                        // check compares against.
                        let (dispatch_timeframe, per_candidate_timeframe_label) =
                            match &dispatch_authority {
                                RuntimeStrategyDispatchAuthority::Legacy { assignments } => (
                                    assignments
                                        .first()
                                        .map(|a| a.timeframe.clone())
                                        .unwrap_or_default(),
                                    None,
                                ),
                                RuntimeStrategyDispatchAuthority::DynamicPaperEnforced {
                                    bindings,
                                    ..
                                } => {
                                    let mut per_symbol: std::collections::BTreeMap<String, String> =
                                        std::collections::BTreeMap::new();
                                    let mut labels: std::collections::BTreeSet<String> =
                                        std::collections::BTreeSet::new();
                                    for p in &conflict_outcome.decisions {
                                        if let Some(b) = bindings
                                            .iter()
                                            .find(|b| b.symbol == p.decision.symbol)
                                        {
                                            per_symbol.insert(
                                                b.symbol.clone(),
                                                b.db_timeframe_label.clone(),
                                            );
                                            labels.insert(b.db_timeframe_label.clone());
                                        }
                                    }
                                    let canonical_label = labels.into_iter().collect::<Vec<_>>().join("+");
                                    (canonical_label, Some(per_symbol))
                                }
                            };
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
                        //
                        // TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01
                        // Blocker 3: this is the one real Bundle 5 call site
                        // for this tick — always strictly after the Bundle 6
                        // call site above (same straight-line tick body).
                        #[cfg(test)]
                        state_arc.loop_call_trace_push_for_test("bundle5");
                        let allocation_outcome =
                            crate::runtime_opportunity_allocation::gather_and_apply(
                                &state_arc,
                                run_id,
                                now_micros,
                                market_date_today,
                                dispatch_timeframe,
                                per_candidate_timeframe_label,
                                conflict_outcome.decisions,
                                &current_positions,
                            )
                            .await;
                        // RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase G: the plan
                        // (when Some) is already persisted as durable
                        // evidence inside gather_and_apply, best-effort —
                        // nothing further to do with it here.

                        // PHASE-7B Part 6 / Blocker 1: re-validate after
                        // Bundle 5, before cap #6/submission — provenance
                        // must survive allocation unchanged except the
                        // permitted qty/decision_id rebuild
                        // (`rebuild_decision_with_qty` never touches symbol,
                        // strategy_id, or `dynamic_selection_provenance`).
                        let provenance_ok_post_bundle5 = dynamic_selection_envelopes_ok(
                            &dispatch_authority,
                            &allocation_outcome.decisions,
                        );
                        if !provenance_ok_post_bundle5 {
                            tracing::error!(
                                run_id = %run_id,
                                "phase7b_dynamic_selection_provenance_missing_post_bundle5: \
                                 refusing whole tick closed, zero submissions"
                            );
                            continue;
                        }

                        // PHASE-7B Part 6 / Blocker 1: the fourth and final
                        // checkpoint, immediately before the submission loop
                        // below ever calls `submit_internal_strategy_decision`
                        // — proves the exact envelope this tick is about to
                        // submit still carries intact, matching provenance at
                        // the last possible point before an order is placed.
                        let provenance_ok_pre_submission = dynamic_selection_envelopes_ok(
                            &dispatch_authority,
                            &allocation_outcome.decisions,
                        );
                        if !provenance_ok_pre_submission {
                            tracing::error!(
                                run_id = %run_id,
                                "phase7b_dynamic_selection_provenance_missing_pre_submission: \
                                 refusing whole tick closed, zero submissions"
                            );
                            continue;
                        }

                        for envelope in allocation_outcome.decisions {
                            let decision = envelope.decision;
                            // MULTI-SYMBOL-CAPITAL-CAPS-01 cap #6: relocated
                            // here (submission time) because it counts
                            // *accepted* decisions — see this file's
                            // "Phase F" comment above the `all_decisions`
                            // declaration for why. Same running count, same
                            // dispatch order, same reason string as before.
                            //
                            // TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01
                            // Blocker 3: this is the one real cap #6 check
                            // site — strictly after Bundle 5, strictly
                            // before the canonical submission call below.
                            #[cfg(test)]
                            state_arc.loop_call_trace_push_for_test(format!(
                                "cap6_check:{}",
                                decision.symbol
                            ));
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
                            // TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01
                            // Blocker 3 requirement 10: record the exact
                            // provenance reaching this, the one real
                            // canonical submission call site — a test can
                            // diff this against the matching `derive_
                            // provenance` event to prove the envelope
                            // reached submission unchanged (except the
                            // permitted qty/decision_id rebuild, which this
                            // event does not carry).
                            #[cfg(test)]
                            if let Some(p) = &envelope.dynamic_selection_provenance {
                                state_arc.loop_call_trace_push_for_test(format!(
                                    "submit_provenance:{}:{}:{}:{}:{}",
                                    p.symbol, p.strategy_id, p.run_id, p.plan_id, p.timeframe_secs
                                ));
                            }
                            #[cfg(test)]
                            state_arc
                                .loop_call_trace_push_for_test(format!("submit:{did}:{decision_symbol}"));
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

        let release_outcome = orchestrator
            .release_runtime_leadership()
            .await
            .map_err(|err| {
                tracing::warn!("runtime_lease_release_failed error={err}");
                err.to_string()
            });

        let exit = ExecutionLoopExit {
            note: Some("execution loop stopped".to_string()),
            leadership_release_outcome: Some(release_outcome),
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

#[cfg(test)]
mod phase7b_provenance_tests {
    use super::*;
    use crate::decision::InternalStrategyDecision;
    use crate::dynamic_selection_dispatch_authority::SelectedDispatchBinding;
    use crate::dynamic_selection_host_pool::DynamicSelectionHostPool;

    fn run_id() -> Uuid {
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.test.run_id")
    }

    fn plan_id() -> Uuid {
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.test.plan_id")
    }

    fn binding(symbol: &str, strategy_id: &str, timeframe_secs: i64) -> SelectedDispatchBinding {
        SelectedDispatchBinding {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe_secs,
            db_timeframe_label: "5m".to_string(),
            selection_reason_code: "selected".to_string(),
            plan_id: plan_id(),
        }
    }

    /// A `DynamicPaperEnforced` authority with one selected AAPL/
    /// intraday_scalper/300s binding. `host_pool` is deliberately empty --
    /// `dynamic_selection_envelope_ok` never reads it.
    fn dynamic_authority() -> RuntimeStrategyDispatchAuthority {
        RuntimeStrategyDispatchAuthority::DynamicPaperEnforced {
            run_id: run_id(),
            plan_id: plan_id(),
            bindings: vec![binding("AAPL", "intraday_scalper", 300)],
            host_pool: DynamicSelectionHostPool::build(&[]).expect("empty pool builds"),
        }
    }

    fn legacy_authority() -> RuntimeStrategyDispatchAuthority {
        RuntimeStrategyDispatchAuthority::Legacy {
            assignments: Vec::new(),
        }
    }

    fn decision(symbol: &str, strategy_id: &str, timeframe_secs: i64) -> InternalStrategyDecision {
        InternalStrategyDecision {
            decision_id: format!("{symbol}-{strategy_id}"),
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs,
            side: "buy".to_string(),
            qty: 10,
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
        }
    }

    fn valid_provenance() -> DynamicSelectionDispatchProvenance {
        DynamicSelectionDispatchProvenance {
            run_id: run_id(),
            plan_id: plan_id(),
            symbol: "AAPL".to_string(),
            strategy_id: "intraday_scalper".to_string(),
            timeframe_secs: 300,
        }
    }

    fn envelope(
        provenance: Option<DynamicSelectionDispatchProvenance>,
    ) -> PendingDecisionWithBarFacts {
        PendingDecisionWithBarFacts {
            decision: decision("AAPL", "intraday_scalper", 300),
            bar_facts: None,
            dynamic_selection_provenance: provenance,
        }
    }

    // ── Legacy: provenance must always be None ─────────────────────────

    #[test]
    fn legacy_requires_none_provenance() {
        assert!(dynamic_selection_envelope_ok(
            &legacy_authority(),
            &envelope(None)
        ));
        assert!(
            !dynamic_selection_envelope_ok(
                &legacy_authority(),
                &envelope(Some(valid_provenance()))
            ),
            "a Legacy decision carrying provenance must be rejected"
        );
    }

    // ── DynamicPaperEnforced: the happy path ────────────────────────────

    #[test]
    fn dynamic_paper_enforced_accepts_matching_provenance() {
        assert!(dynamic_selection_envelope_ok(
            &dynamic_authority(),
            &envelope(Some(valid_provenance()))
        ));
    }

    // ── Mutation proofs: each field independently, plus missing/swapped ──

    #[test]
    fn missing_provenance_fails_closed() {
        assert!(!dynamic_selection_envelope_ok(
            &dynamic_authority(),
            &envelope(None)
        ));
    }

    #[test]
    fn mutated_run_id_fails_closed() {
        let mut p = valid_provenance();
        p.run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"wrong.run_id");
        assert!(!dynamic_selection_envelope_ok(
            &dynamic_authority(),
            &envelope(Some(p))
        ));
    }

    #[test]
    fn mutated_plan_id_fails_closed() {
        let mut p = valid_provenance();
        p.plan_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"wrong.plan_id");
        assert!(!dynamic_selection_envelope_ok(
            &dynamic_authority(),
            &envelope(Some(p))
        ));
    }

    #[test]
    fn mutated_symbol_fails_closed() {
        // provenance claims MSFT but the decision itself is still AAPL --
        // canonical-symbol comparison must catch the mismatch.
        let mut p = valid_provenance();
        p.symbol = "MSFT".to_string();
        assert!(!dynamic_selection_envelope_ok(
            &dynamic_authority(),
            &envelope(Some(p))
        ));
    }

    #[test]
    fn mutated_strategy_id_fails_closed() {
        let mut p = valid_provenance();
        p.strategy_id = "swing_momentum".to_string();
        assert!(!dynamic_selection_envelope_ok(
            &dynamic_authority(),
            &envelope(Some(p))
        ));
    }

    #[test]
    fn mutated_timeframe_secs_fails_closed() {
        let mut p = valid_provenance();
        p.timeframe_secs = 3600;
        assert!(!dynamic_selection_envelope_ok(
            &dynamic_authority(),
            &envelope(Some(p))
        ));
    }

    #[test]
    fn provenance_naming_a_binding_that_does_not_exist_fails_closed() {
        // Every field is internally self-consistent, but no binding in the
        // active authority actually has this (symbol, strategy_id,
        // timeframe_secs, plan_id) tuple -- a reconstructed-but-plausible
        // identity must still be rejected.
        let mut p = valid_provenance();
        p.symbol = "GOOG".to_string();
        let mut decision_envelope = envelope(Some(p));
        decision_envelope.decision.symbol = "GOOG".to_string();
        assert!(!dynamic_selection_envelope_ok(
            &dynamic_authority(),
            &decision_envelope
        ));
    }

    #[test]
    fn swapped_provenance_between_two_decisions_fails_closed() {
        // Two selected bindings; swap MSFT's decision to carry AAPL's
        // provenance and vice versa -- each must be rejected even though
        // both provenance values are individually valid for *some*
        // decision this tick.
        let authority = RuntimeStrategyDispatchAuthority::DynamicPaperEnforced {
            run_id: run_id(),
            plan_id: plan_id(),
            bindings: vec![
                binding("AAPL", "intraday_scalper", 300),
                binding("MSFT", "volatility_breakout", 3600),
            ],
            host_pool: DynamicSelectionHostPool::build(&[]).expect("empty pool builds"),
        };
        let aapl_provenance = DynamicSelectionDispatchProvenance {
            run_id: run_id(),
            plan_id: plan_id(),
            symbol: "AAPL".to_string(),
            strategy_id: "intraday_scalper".to_string(),
            timeframe_secs: 300,
        };
        let msft_provenance = DynamicSelectionDispatchProvenance {
            run_id: run_id(),
            plan_id: plan_id(),
            symbol: "MSFT".to_string(),
            strategy_id: "volatility_breakout".to_string(),
            timeframe_secs: 3600,
        };
        // Correct pairing: both pass.
        let aapl_envelope_correct = PendingDecisionWithBarFacts {
            decision: decision("AAPL", "intraday_scalper", 300),
            bar_facts: None,
            dynamic_selection_provenance: Some(aapl_provenance.clone()),
        };
        let msft_envelope_correct = PendingDecisionWithBarFacts {
            decision: decision("MSFT", "volatility_breakout", 3600),
            bar_facts: None,
            dynamic_selection_provenance: Some(msft_provenance.clone()),
        };
        assert!(dynamic_selection_envelope_ok(
            &authority,
            &aapl_envelope_correct
        ));
        assert!(dynamic_selection_envelope_ok(
            &authority,
            &msft_envelope_correct
        ));

        // Swapped pairing: both must fail.
        let aapl_envelope_swapped = PendingDecisionWithBarFacts {
            decision: decision("AAPL", "intraday_scalper", 300),
            bar_facts: None,
            dynamic_selection_provenance: Some(msft_provenance),
        };
        let msft_envelope_swapped = PendingDecisionWithBarFacts {
            decision: decision("MSFT", "volatility_breakout", 3600),
            bar_facts: None,
            dynamic_selection_provenance: Some(aapl_provenance),
        };
        assert!(!dynamic_selection_envelope_ok(
            &authority,
            &aapl_envelope_swapped
        ));
        assert!(!dynamic_selection_envelope_ok(
            &authority,
            &msft_envelope_swapped
        ));
    }

    // ── Batch form ───────────────────────────────────────────────────────

    #[test]
    fn batch_form_fails_closed_if_any_single_envelope_fails() {
        let good = envelope(Some(valid_provenance()));
        let bad = envelope(None);
        assert!(!dynamic_selection_envelopes_ok(
            &dynamic_authority(),
            &[good, bad]
        ));
    }

    #[test]
    fn batch_form_passes_when_every_envelope_passes() {
        let good_a = envelope(Some(valid_provenance()));
        let good_b = envelope(Some(valid_provenance()));
        assert!(dynamic_selection_envelopes_ok(
            &dynamic_authority(),
            &[good_a, good_b]
        ));
    }

    #[test]
    fn empty_batch_is_vacuously_ok_for_either_authority() {
        assert!(dynamic_selection_envelopes_ok(&legacy_authority(), &[]));
        assert!(dynamic_selection_envelopes_ok(&dynamic_authority(), &[]));
    }
}
