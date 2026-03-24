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
                                return ExecutionLoopExit {
                                    note: Some("execution loop halted: deadman expired".to_string()),
                                };
                            }
                            Ok(false) => {}
                            Err(err) => {
                                tracing::error!("execution_loop_deadman_check_failed error={err}");
                                let _ = mqk_db::halt_run(pool, run_id, now).await;
                                let _ = mqk_db::persist_arm_state_canonical(
                                    pool,
                                    mqk_db::ArmState::Disarmed,
                                    Some(mqk_db::DisarmReason::DeadmanSupervisorFailure),
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
                                return ExecutionLoopExit {
                                    note: Some(format!("execution loop halted: deadman check failed: {err}")),
                                };
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
                            let _ = mqk_db::halt_run(pool, run_id, now).await;
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
                        return ExecutionLoopExit {
                            note: Some(
                                "execution loop halted: Alpaca WS continuity gap detected"
                                    .to_string(),
                            ),
                        };
                    }

                    if let Err(err) = orchestrator.tick().await {
                        tracing::error!("execution_loop_halt error={err}");
                        if let Some(ref pool) = db {
                            let now = Utc::now();
                            let _ = mqk_db::halt_run(pool, run_id, now).await;
                        }
                        {
                            let mut ig = integrity.write().await;
                            ig.halted = true;
                        }
                        if let Err(release_err) = orchestrator.release_runtime_leadership().await {
                            tracing::warn!("runtime_lease_release_failed error={release_err}");
                        }
                        return ExecutionLoopExit {
                            note: Some(format!("execution loop halted: {err}")),
                        };
                    }

                    if let Some(ref pool) = db {
                        let now = Utc::now();
                        if let Ok(true) =
                            mqk_db::deadman_expired(pool, run_id, DEADMAN_TTL_SECONDS, now).await
                        {
                            tracing::error!(
                                run_id = %run_id,
                                "execution_loop_deadman_expired: heartbeat stale, self-terminating without refresh"
                            );
                            if let Err(release_err) =
                                orchestrator.release_runtime_leadership().await
                            {
                                tracing::warn!("runtime_lease_release_failed error={release_err}");
                            }
                            return ExecutionLoopExit {
                                note: Some("execution loop exited: deadman expired".to_string()),
                            };
                        }
                        if let Err(err) = mqk_db::heartbeat_run(pool, run_id, now).await {
                            tracing::error!("execution_loop_heartbeat_failed error={err}");
                            let _ = mqk_db::halt_run(pool, run_id, now).await;
                            let _ = mqk_db::persist_arm_state_canonical(
                                pool,
                                mqk_db::ArmState::Disarmed,
                                Some(mqk_db::DisarmReason::DeadmanHeartbeatPersistFailed),
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
                            return ExecutionLoopExit {
                                note: Some(format!("execution loop heartbeat failed: {err}")),
                            };
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
                }
            }
        }

        if let Err(err) = orchestrator.release_runtime_leadership().await {
            tracing::warn!("runtime_lease_release_failed error={err}");
        }

        ExecutionLoopExit {
            note: Some("execution loop stopped".to_string()),
        }
    });

    ExecutionLoopHandle {
        run_id,
        stop_tx,
        join_handle,
    }
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
        let _ = mqk_db::persist_arm_state_canonical(
            db,
            mqk_db::ArmState::Disarmed,
            Some(mqk_db::DisarmReason::ReconcileDrift),
        )
        .await;
        let _ =
            mqk_db::persist_risk_block_state(db, true, Some("RECONCILE_BLOCKED"), Utc::now()).await;
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
