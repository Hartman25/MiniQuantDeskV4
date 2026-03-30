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

pub(crate) fn build_fault_signals(
    status: &StatusSnapshot,
    reconcile: &crate::state::ReconcileStatusSnapshot,
    risk_blocked: bool,
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

    if risk_blocked {
        signals.push(FaultSignal {
            class: "risk.dispatch_denied.engine_blocked".to_string(),
            severity: "critical".to_string(),
            summary: "Risk engine indicates dispatch is blocked.".to_string(),
            detail: None,
        });
    }

    if status.state == "halted" {
        signals.push(FaultSignal {
            class: "runtime.halt.operator_or_safety".to_string(),
            severity: "critical".to_string(),
            summary: "Runtime is halted; dispatch remains fail-closed.".to_string(),
            detail: status.notes.clone(),
        });
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
    let Some(db) = st.db.as_ref() else {
        return None;
    };
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
