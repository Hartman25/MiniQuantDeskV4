//! Shared pure helpers used by multiple route modules.
//!
//! Contains: runtime_error_response, build_fault_signals, parse_decimal,
//! oms_stage_label, runtime_status_from_state, environment_and_live_routing_truth,
//! position_market_value, exposure_breakdown, write_operator_audit_event,
//! runtime_transition_for_action.

use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{http::StatusCode, response::Response, Json};
use mqk_schemas::BrokerPosition;

use crate::api_types::{FaultSignal, RuntimeErrorResponse};
use crate::state::{AppState, RuntimeLifecycleError, StatusSnapshot};

// ---------------------------------------------------------------------------
// runtime_error_response
// ---------------------------------------------------------------------------

pub(crate) fn runtime_error_response(err: RuntimeLifecycleError) -> Response {
    match err {
        RuntimeLifecycleError::Forbidden {
            fault_class,
            gate,
            message,
        } => (
            StatusCode::FORBIDDEN,
            Json(RuntimeErrorResponse {
                error: message,
                fault_class: fault_class.to_string(),
                gate: Some(gate),
            }),
        )
            .into_response(),
        RuntimeLifecycleError::ServiceUnavailable {
            fault_class,
            message,
        } => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(RuntimeErrorResponse {
                error: message,
                fault_class: fault_class.to_string(),
                gate: None,
            }),
        )
            .into_response(),
        RuntimeLifecycleError::Conflict {
            fault_class,
            message,
        } => (
            StatusCode::CONFLICT,
            Json(RuntimeErrorResponse {
                error: message,
                fault_class: fault_class.to_string(),
                gate: None,
            }),
        )
            .into_response(),
        RuntimeLifecycleError::Internal {
            fault_class,
            message,
        } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(RuntimeErrorResponse {
                error: message,
                fault_class: fault_class.to_string(),
                gate: None,
            }),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// build_fault_signals
// ---------------------------------------------------------------------------

/// HEARTBEAT-TICK-01: stall threshold for the execution loop.
///
/// A running daemon that has not recorded a tick in this many seconds surfaces
/// a critical fault signal.  Set to 2× the deadman TTL (5 s) — tighter than
/// what the DB deadman alone can detect (mid-tick hangs are invisible to the
/// deadman until the stuck tick unblocks), but loose enough to avoid
/// false-alarms on normal 1 s scheduler jitter.
const EXECUTION_LOOP_STALL_THRESHOLD_SECS: i64 = 10;

/// Build fault signals from runtime snapshot state.
///
/// `risk_truth` distinguishes three states:
/// - `None` — DB is absent or the risk query failed; risk block state is unknown.
///   Emits a warning-level signal so operators cannot mistake absence of data for safety.
/// - `Some(true)` — DB query succeeded and risk engine reports blocked.
///   Emits a critical-level signal.
/// - `Some(false)` — DB query succeeded and risk engine reports not blocked.
///   No signal emitted.
///
/// `execution_loop_stall_secs` is `Some(elapsed)` only when `state == "running"`
/// and the loop has completed at least one tick (`last_tick_at > 0`).  `None`
/// suppresses the stall check (idle, never ticked, or not applicable).
pub(crate) fn build_fault_signals(
    status: &StatusSnapshot,
    reconcile: &crate::state::ReconcileStatusSnapshot,
    risk_truth: Option<bool>,
    execution_loop_stall_secs: Option<i64>,
) -> Vec<FaultSignal> {
    let mut signals = Vec::new();

    if status.state == "unknown" {
        signals.push(FaultSignal {
            class: "runtime.truth_mismatch.durable_active_without_local_owner".to_string(),
            severity: "critical".to_string(),
            summary: "Durable run appears active without daemon-owned runtime loop.".to_string(),
            detail: status.notes.clone(),
        });
    }

    if matches!(reconcile.status.as_str(), "dirty" | "stale" | "unavailable") {
        signals.push(FaultSignal {
            class: format!("reconcile.dispatch_block.{}", reconcile.status),
            severity: if reconcile.status == "dirty" {
                "critical"
            } else {
                "warning"
            }
            .to_string(),
            summary: "Reconcile state blocks or degrades safe dispatch.".to_string(),
            detail: reconcile.note.clone(),
        });
    }

    if reconcile.status == "unknown" && status.state == "running" {
        signals.push(FaultSignal {
            class: "reconcile.unproven.running_without_reconcile_result".to_string(),
            severity: "critical".to_string(),
            summary: "Runtime is running but reconcile result is unproven; order consistency cannot be verified.".to_string(),
            detail: None,
        });
    }

    match risk_truth {
        None => {
            // DB absent or query failed — cannot confirm risk state.  Emit a warning
            // so the operator sees an explicit signal rather than a false-clear surface.
            signals.push(FaultSignal {
                class: "risk.truth_unavailable".to_string(),
                severity: "warning".to_string(),
                summary: "Risk block state is unknown: DB is absent or query failed; \
                          risk-block enforcement cannot be confirmed."
                    .to_string(),
                detail: None,
            });
        }
        Some(true) => {
            signals.push(FaultSignal {
                class: "risk.dispatch_denied.engine_blocked".to_string(),
                severity: "critical".to_string(),
                summary: "Risk engine indicates dispatch is blocked.".to_string(),
                detail: None,
            });
        }
        Some(false) => {} // confirmed not blocked — no signal
    }

    if status.state == "halted" {
        signals.push(FaultSignal {
            class: "runtime.halt.operator_or_safety".to_string(),
            severity: "critical".to_string(),
            summary: "Runtime is halted; dispatch remains fail-closed.".to_string(),
            detail: status.notes.clone(),
        });
    }

    // HEARTBEAT-TICK-01: detect stalled execution loop.
    //
    // `execution_loop_stall_secs` is `Some` only when the daemon is running and
    // has completed at least one tick.  A zero `last_tick_at` (never ticked)
    // causes callers to pass `None`, suppressing this check.
    if let Some(stall_secs) = execution_loop_stall_secs {
        if stall_secs > EXECUTION_LOOP_STALL_THRESHOLD_SECS {
            signals.push(FaultSignal {
                class: "runtime.execution_loop.stalled".to_string(),
                severity: "critical".to_string(),
                summary: format!(
                    "Execution loop has not completed a tick in {stall_secs}s; \
                     liveness unproven — dispatch cannot be trusted."
                ),
                detail: None,
            });
        }
    }

    signals
}

// ---------------------------------------------------------------------------
// parse_decimal / oms_stage_label / runtime_status_from_state
// ---------------------------------------------------------------------------

pub(crate) fn parse_decimal(value: &str) -> f64 {
    value.parse::<f64>().unwrap_or(0.0)
}

/// Map an OMS canonical state name to a display-friendly lifecycle stage label.
pub(crate) fn oms_stage_label(status: &str) -> &'static str {
    match status {
        "Open" => "Submitted",
        "PartiallyFilled" => "Partial Fill",
        "Filled" => "Filled",
        "CancelPending" => "Cancel Pending",
        "Cancelled" => "Cancelled",
        "ReplacePending" => "Replace Pending",
        "Rejected" => "Rejected",
        _ => "Unknown",
    }
}

pub(crate) fn runtime_status_from_state(state: &str) -> &'static str {
    match state {
        "idle" => "idle",
        "running" => "running",
        "halted" => "halted",
        "unknown" => "unknown",
        _ => "degraded",
    }
}

// ---------------------------------------------------------------------------
// environment_and_live_routing_truth
// ---------------------------------------------------------------------------

pub(crate) async fn environment_and_live_routing_truth(
    st: &AppState,
    status: &StatusSnapshot,
) -> (Option<String>, Option<bool>) {
    let live_routing_enabled = match status.state.as_str() {
        "idle" | "halted" | "unknown" => Some(false),
        _ => None,
    };

    let Some(run_id) = status.active_run_id else {
        return (None, live_routing_enabled);
    };

    let Some(db) = st.db.as_ref() else {
        return (None, live_routing_enabled);
    };

    let Ok(run) = mqk_db::fetch_run(db, run_id).await else {
        return (None, live_routing_enabled);
    };

    let environment = Some(run.mode.to_ascii_lowercase());
    let live_routing_enabled = if status.state == "running" {
        Some(run.mode.eq_ignore_ascii_case("LIVE") || run.mode.eq_ignore_ascii_case("LIVE-CAPITAL"))
    } else {
        live_routing_enabled
    };

    (environment, live_routing_enabled)
}

// ---------------------------------------------------------------------------
// position_market_value / exposure_breakdown
// ---------------------------------------------------------------------------

pub(crate) fn position_market_value(position: &BrokerPosition) -> f64 {
    parse_decimal(&position.qty) * parse_decimal(&position.avg_price)
}

pub(crate) fn exposure_breakdown(positions: &[BrokerPosition]) -> (f64, f64, f64, f64) {
    let mut long_market_value: f64 = 0.0;
    let mut short_market_value: f64 = 0.0;
    let mut max_abs_position: f64 = 0.0;

    for position in positions {
        let market_value = position_market_value(position);
        let abs_market_value = market_value.abs();
        max_abs_position = max_abs_position.max(abs_market_value);

        if market_value >= 0.0 {
            long_market_value += market_value;
        } else {
            short_market_value += abs_market_value;
        }
    }

    let gross_exposure = long_market_value + short_market_value;
    (
        long_market_value,
        short_market_value,
        gross_exposure,
        max_abs_position,
    )
}

// ---------------------------------------------------------------------------
// write_operator_audit_event / runtime_transition_for_action
// ---------------------------------------------------------------------------

pub(crate) async fn write_operator_audit_event(
    st: &Arc<AppState>,
    run_id: Option<uuid::Uuid>,
    event_type: &str,
    runtime_transition: &str,
) -> anyhow::Result<Option<uuid::Uuid>> {
    let Some(db) = st.db.as_ref() else {
        return Ok(None);
    };
    let Some(run_id) = run_id else {
        return Ok(None);
    };

    let ts_utc = chrono::Utc::now();
    let event_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!(
            "mqk-daemon.ops-audit.v1|{}|{}|{}",
            run_id,
            event_type,
            ts_utc.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        )
        .as_bytes(),
    );
    mqk_db::insert_audit_event(
        db,
        &mqk_db::NewAuditEvent {
            event_id,
            run_id,
            ts_utc,
            topic: "operator".to_string(),
            event_type: event_type.to_string(),
            payload: serde_json::json!({
                "runtime_transition": runtime_transition,
                "source": "mqk-daemon.routes",
            }),
            hash_prev: None,
            hash_self: None,
        },
    )
    .await?;
    Ok(Some(event_id))
}

// ---------------------------------------------------------------------------
// write_signal_admission_event — JOUR-01
// ---------------------------------------------------------------------------

/// Write a durable signal-admission audit event at Gate 7 success.
///
/// Called by `strategy_signal` when Gate 7 enqueues a new outbox row
/// (`Ok(true)`).  Creates a permanent record in `audit_events` with
/// `topic='signal_ingestion'` so the journal and events feed can surface
/// what signals were admitted for dispatch during a run.
///
/// # Non-fatal contract
///
/// If the DB write fails, the failure is logged at `warn` level and `None`
/// is returned.  The caller's signal-admission response is **not** affected —
/// the outbox write that Gate 7 performed is the authoritative admission; the
/// audit event is a supervision artifact, not a gate.
///
/// # Idempotency
///
/// `event_id` is a UUIDv5 derived from `(run_id, signal_id, ts_utc_micros)`.
/// Re-writes for the same signal within the same microsecond are idempotent
/// (DB `ON CONFLICT DO NOTHING` in `insert_audit_event`).
pub(crate) async fn write_signal_admission_event(
    st: &Arc<AppState>,
    run_id: uuid::Uuid,
    signal_id: &str,
    strategy_id: &str,
    symbol: &str,
    side: &str,
    qty: i64,
) -> Option<uuid::Uuid> {
    let db = st.db.as_ref()?;
    let ts_utc = chrono::Utc::now();
    let event_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!(
            "mqk-daemon.signal-admission.v1|{}|{}|{}",
            run_id,
            signal_id,
            ts_utc.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        )
        .as_bytes(),
    );
    if let Err(err) = mqk_db::insert_audit_event(
        db,
        &mqk_db::NewAuditEvent {
            event_id,
            run_id,
            ts_utc,
            topic: "signal_ingestion".to_string(),
            event_type: "signal.admitted".to_string(),
            payload: serde_json::json!({
                "signal_id": signal_id,
                "strategy_id": strategy_id,
                "symbol": symbol,
                "side": side,
                "qty": qty,
                "source": "mqk-daemon.routes.strategy_signal",
            }),
            hash_prev: None,
            hash_self: None,
        },
    )
    .await
    {
        tracing::warn!("write_signal_admission_event failed (non-fatal): {err}");
        return None;
    }
    Some(event_id)
}

// ---------------------------------------------------------------------------
// write_signal_refusal_event — SIGNAL-REFUSAL-OBS-01
// ---------------------------------------------------------------------------

/// Write a durable signal-refusal audit event when an active run is present.
///
/// Called by `strategy_signal` at Gates 5 and 6, where `active_run_id` is
/// known.  Creates a `signal.refused` record in `audit_events` with
/// `topic='signal_ingestion'` so the events feed surfaces why a signal was
/// denied during a run.
///
/// # Non-fatal contract
///
/// If the DB write fails, the failure is logged at `warn` level and `None`
/// is returned.  The caller's refusal response is not affected.
///
/// # Idempotency
///
/// `event_id` is a UUIDv5 derived from `(run_id, signal_id, gate, ts_utc_micros)`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SignalRefusalAudit<'a> {
    pub gate: &'a str,
    pub disposition: &'a str,
    pub signal_id: &'a str,
    pub strategy_id: &'a str,
    pub symbol: &'a str,
    pub blockers: &'a [String],
}

pub(crate) async fn write_signal_refusal_event(
    st: &Arc<AppState>,
    run_id: uuid::Uuid,
    refusal: &SignalRefusalAudit<'_>,
) -> Option<uuid::Uuid> {
    let db = st.db.as_ref()?;
    let ts_utc = chrono::Utc::now();
    let event_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!(
            "mqk-daemon.signal-refusal.v1|{}|{}|{}|{}",
            run_id,
            refusal.signal_id,
            refusal.gate,
            ts_utc.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        )
        .as_bytes(),
    );
    if let Err(err) = mqk_db::insert_audit_event(
        db,
        &mqk_db::NewAuditEvent {
            event_id,
            run_id,
            ts_utc,
            topic: "signal_ingestion".to_string(),
            event_type: "signal.refused".to_string(),
            payload: serde_json::json!({
                "signal_id": refusal.signal_id,
                "strategy_id": refusal.strategy_id,
                "symbol": refusal.symbol,
                "gate": refusal.gate,
                "disposition": refusal.disposition,
                "blockers": refusal.blockers,
                "source": "mqk-daemon.routes.strategy_signal",
            }),
            hash_prev: None,
            hash_self: None,
        },
    )
    .await
    {
        tracing::warn!("write_signal_refusal_event failed (non-fatal): {err}");
        return None;
    }
    Some(event_id)
}

// ---------------------------------------------------------------------------
// CTRL-ARM-01: arm safety preflight gate
// ---------------------------------------------------------------------------

/// Check safety preconditions before persisting ARMED state.
///
/// Called by all three HTTP arm paths before any state mutation.  Returns
/// `Ok(())` when arming is safe; `Err((gate, blockers))` when a blocking
/// condition is found so the caller can return a 403 with structured body.
///
/// Gate order:
/// 1. Reconcile — blocks if `dirty` or `stale` (proven-bad state).
///    Checked from in-memory snapshot; no DB required.
/// 2. Risk — blocks if DB confirms risk engine is blocking dispatch.
///    Skipped when DB is absent (only in-memory arm path; no truth to read).
///    Fails closed if the DB query itself fails.
pub(crate) async fn check_arm_safety(
    st: &Arc<AppState>,
    db: Option<&sqlx::PgPool>,
) -> Result<(), (String, Vec<String>)> {
    // Gate 1: reconcile must not be proven-bad.
    let reconcile = st.current_reconcile_snapshot().await;
    if matches!(reconcile.status.as_str(), "dirty" | "stale") {
        return Err((
            "arm_preflight.reconcile_not_clean".to_string(),
            vec![format!(
                "reconcile status is '{}'; arm refused until reconcile is clean \
                 — resolve the reconcile mismatch before arming (CTRL-ARM-01)",
                reconcile.status
            )],
        ));
    }

    // Gate 2: risk block state — only when DB is present.
    if let Some(pool) = db {
        match mqk_db::load_risk_block_state(pool).await {
            Ok(Some(risk)) if risk.blocked => {
                return Err((
                    "arm_preflight.risk_blocked".to_string(),
                    vec![risk
                        .reason
                        .unwrap_or_else(|| "risk engine is blocking dispatch".to_string())],
                ));
            }
            Err(_) => {
                return Err((
                    "arm_preflight.risk_state_unavailable".to_string(),
                    vec!["risk block state query failed; arm refused fail-closed \
                         (CTRL-ARM-01)"
                        .to_string()],
                ));
            }
            _ => {} // Ok(None) or Ok(Some(risk)) where !risk.blocked → pass
        }
    }

    Ok(())
}

pub(crate) fn runtime_transition_for_action(action: &str) -> Option<String> {
    match action {
        "control.arm" => Some("ARMED".to_string()),
        "control.disarm" => Some("DISARMED".to_string()),
        "run.start" => Some("RUNNING".to_string()),
        "run.stop" => Some("STOPPED".to_string()),
        "run.halt" => Some("HALTED".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ReconcileStatusSnapshot, StatusSnapshot};

    fn ok_status() -> StatusSnapshot {
        StatusSnapshot {
            daemon_uptime_secs: 10,
            active_run_id: None,
            state: "idle".to_string(),
            notes: None,
            integrity_armed: true,
            deadman_status: "ok".to_string(),
            deadman_last_heartbeat_utc: None,
        }
    }

    fn ok_reconcile() -> ReconcileStatusSnapshot {
        ReconcileStatusSnapshot {
            status: "ok".to_string(),
            last_run_at: None,
            snapshot_watermark_ms: None,
            mismatched_positions: 0,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: None,
        }
    }

    #[test]
    fn st01_risk_truth_none_emits_warning_signal() {
        let signals = build_fault_signals(&ok_status(), &ok_reconcile(), None, None);
        let risk_signals: Vec<_> = signals
            .iter()
            .filter(|s| s.class.starts_with("risk."))
            .collect();
        assert_eq!(risk_signals.len(), 1, "expected exactly one risk signal");
        assert_eq!(risk_signals[0].class, "risk.truth_unavailable");
        assert_eq!(risk_signals[0].severity, "warning");
    }

    #[test]
    fn st01_risk_truth_some_true_emits_critical_signal() {
        let signals = build_fault_signals(&ok_status(), &ok_reconcile(), Some(true), None);
        let risk_signals: Vec<_> = signals
            .iter()
            .filter(|s| s.class.starts_with("risk."))
            .collect();
        assert_eq!(risk_signals.len(), 1);
        assert_eq!(risk_signals[0].class, "risk.dispatch_denied.engine_blocked");
        assert_eq!(risk_signals[0].severity, "critical");
    }

    #[test]
    fn st01_risk_truth_some_false_emits_no_risk_signal() {
        let signals = build_fault_signals(&ok_status(), &ok_reconcile(), Some(false), None);
        let risk_signals: Vec<_> = signals
            .iter()
            .filter(|s| s.class.starts_with("risk."))
            .collect();
        assert!(
            risk_signals.is_empty(),
            "confirmed-clear risk must not fire a signal"
        );
    }

    #[test]
    fn st01_risk_truth_none_distinct_from_confirmed_blocked() {
        let signals_unknown = build_fault_signals(&ok_status(), &ok_reconcile(), None, None);
        let signals_blocked = build_fault_signals(&ok_status(), &ok_reconcile(), Some(true), None);
        // Both fire a risk signal but with different classes — operators can distinguish
        let class_unknown = &signals_unknown
            .iter()
            .find(|s| s.class.starts_with("risk."))
            .unwrap()
            .class;
        let class_blocked = &signals_blocked
            .iter()
            .find(|s| s.class.starts_with("risk."))
            .unwrap()
            .class;
        assert_ne!(class_unknown, class_blocked);
    }

    // HEARTBEAT-TICK-01 stall detection tests.
    //
    // build_fault_signals is pure so these are hermetic (no time.Now() calls
    // inside; callers pre-compute stall_secs and pass it in).

    #[test]
    fn ht01_no_stall_signal_when_stall_secs_none() {
        // None means "not running" or "never ticked" — no stall signal regardless.
        let signals = build_fault_signals(&ok_status(), &ok_reconcile(), Some(false), None);
        let stall: Vec<_> = signals
            .iter()
            .filter(|s| s.class == "runtime.execution_loop.stalled")
            .collect();
        assert!(
            stall.is_empty(),
            "None stall_secs must produce no stall signal"
        );
    }

    #[test]
    fn ht01_no_stall_signal_under_threshold() {
        // 5 s elapsed — under the 10 s threshold; no signal.
        let signals = build_fault_signals(&ok_status(), &ok_reconcile(), Some(false), Some(5));
        let stall: Vec<_> = signals
            .iter()
            .filter(|s| s.class == "runtime.execution_loop.stalled")
            .collect();
        assert!(stall.is_empty(), "stall below threshold must not fire");
    }

    #[test]
    fn ht01_no_stall_signal_at_exact_threshold() {
        // Threshold is strict >; exactly 10 s is not stalled.
        let signals = build_fault_signals(&ok_status(), &ok_reconcile(), Some(false), Some(10));
        let stall: Vec<_> = signals
            .iter()
            .filter(|s| s.class == "runtime.execution_loop.stalled")
            .collect();
        assert!(
            stall.is_empty(),
            "stall at exact threshold must not fire (strict >)"
        );
    }

    #[test]
    fn ht01_critical_stall_signal_over_threshold() {
        // 15 s elapsed — over the 10 s threshold; critical signal.
        let signals = build_fault_signals(&ok_status(), &ok_reconcile(), Some(false), Some(15));
        let stall: Vec<_> = signals
            .iter()
            .filter(|s| s.class == "runtime.execution_loop.stalled")
            .collect();
        assert_eq!(
            stall.len(),
            1,
            "stall over threshold must fire exactly one signal"
        );
        assert_eq!(stall[0].severity, "critical");
        assert_eq!(stall[0].class, "runtime.execution_loop.stalled");
    }

    #[test]
    fn ht01_stall_signal_detail_includes_elapsed_secs() {
        let signals = build_fault_signals(&ok_status(), &ok_reconcile(), Some(false), Some(30));
        let stall = signals
            .iter()
            .find(|s| s.class == "runtime.execution_loop.stalled")
            .expect("stall signal must be present at 30 s");
        assert!(
            stall.summary.contains("30s"),
            "stall signal summary must include elapsed seconds; got: {}",
            stall.summary
        );
    }
}
