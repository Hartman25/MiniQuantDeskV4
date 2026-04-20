//! Order analysis route handlers (A5 batch + outbox view).
//!
//! This is the parent module.  Per-order fill-history and diagnostic surfaces
//! have been extracted into focused submodules:
//!
//! | Submodule          | Routes                                          |
//! |--------------------|--------------------------------------------------|
//! | `order_history`    | timeline (A5A), trace (A5B), replay (A5C)       |
//! | `order_diagnostics`| chart (A5D), causality (A5E)                    |
//!
//! Retained here: `lifecycle_stage_from_outbox_status`, `execution_outbox`,
//! `execution_protection_status`, `execution_replace_cancel_chains`.

pub(crate) mod order_diagnostics;
pub(crate) mod order_history;

pub(crate) use order_diagnostics::{execution_order_causality, execution_order_chart};
pub(crate) use order_history::{
    execution_order_replay, execution_order_timeline, execution_order_trace,
};

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};

use crate::api_types::{EventRiskStatusResponse, ExecutionOutboxResponse, ExecutionOutboxRow};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// GET /api/v1/execution/outbox — OPS-08 / EXEC-06: paper execution timeline
// ---------------------------------------------------------------------------

/// Map a durable outbox status string to a display-friendly lifecycle stage.
///
/// Pure function — no state, no I/O.  All unknown values map to `"unknown"`.
/// Also used by `order_history::execution_order_trace` via descendant access.
fn lifecycle_stage_from_outbox_status(status: &str) -> &'static str {
    match status {
        "PENDING" => "queued",
        "CLAIMED" => "claimed",
        "DISPATCHING" => "submitting",
        "SENT" => "sent_to_broker",
        "ACKED" => "acknowledged",
        "FAILED" => "failed",
        "AMBIGUOUS" => "ambiguous",
        _ => "unknown",
    }
}

pub(crate) async fn execution_outbox(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    const CANONICAL: &str = "/api/v1/execution/outbox";

    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(ExecutionOutboxResponse {
                canonical_route: CANONICAL.to_string(),
                truth_state: "no_db".to_string(),
                backend: "unavailable".to_string(),
                run_id: None,
                rows: vec![],
            }),
        )
            .into_response();
    };

    let active_run_id = match st.current_status_snapshot().await {
        Ok(snap) => snap.active_run_id,
        Err(_) => None,
    };

    let Some(run_id) = active_run_id else {
        return (
            StatusCode::OK,
            Json(ExecutionOutboxResponse {
                canonical_route: CANONICAL.to_string(),
                truth_state: "no_active_run".to_string(),
                backend: "unavailable".to_string(),
                run_id: None,
                rows: vec![],
            }),
        )
            .into_response();
    };

    let db_rows = match mqk_db::outbox_fetch_for_supervisor(db, run_id).await {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "outbox_fetch_failed",
                    "detail": e.to_string(),
                })),
            )
                .into_response();
        }
    };

    let api_rows: Vec<ExecutionOutboxRow> = db_rows
        .into_iter()
        .map(|r| {
            let symbol = r
                .order_json
                .get("symbol")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let side = r
                .order_json
                .get("side")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let qty = r.order_json.get("qty").and_then(|v| v.as_i64());
            let order_type = r
                .order_json
                .get("order_type")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let strategy_id = r
                .order_json
                .get("strategy_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let signal_source = r
                .order_json
                .get("signal_source")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let lifecycle_stage = lifecycle_stage_from_outbox_status(&r.status).to_string();

            ExecutionOutboxRow {
                idempotency_key: r.idempotency_key,
                run_id: r.run_id.to_string(),
                status: r.status,
                lifecycle_stage,
                symbol,
                side,
                qty,
                order_type,
                strategy_id,
                signal_source,
                created_at_utc: r.created_at_utc.to_rfc3339(),
                claimed_at_utc: r.claimed_at_utc.map(|t| t.to_rfc3339()),
                dispatching_at_utc: r.dispatching_at_utc.map(|t| t.to_rfc3339()),
                sent_at_utc: r.sent_at_utc.map(|t| t.to_rfc3339()),
            }
        })
        .collect();

    (
        StatusCode::OK,
        Json(ExecutionOutboxResponse {
            canonical_route: CANONICAL.to_string(),
            truth_state: "active".to_string(),
            backend: "postgres.oms_outbox".to_string(),
            run_id: Some(run_id.to_string()),
            rows: api_rows,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/protection-status (B4)
// ---------------------------------------------------------------------------

/// Protective stop / bracket order status surface — explicitly not wired.
///
/// B4 closure: stop and bracket orders are NOT supported on the canonical
/// paper+alpaca execution path.  This route returns an honest `"not_wired"`
/// contract rather than a 404 or a fabricated "protected" status, so operator
/// tooling and runbooks can explicitly distinguish "protection absent" from
/// "route unavailable".
///
/// # Why this matters
///
/// The submit validator explicitly rejects `order_type = "stop"` (and
/// `"trailing_stop"`).  No OCO / OTOCO bracket types are passed through the
/// Alpaca broker adapter.  The `KillSwitchType::MissingProtectiveStop`
/// kill-switch policy is defined in the risk config but cannot be
/// operator-satisfied until stop order wiring is implemented (B5+).
///
/// Operators relying on this surface will see `truth_state = "not_wired"` until
/// a future patch promotes it to `"broker_backed"` with proof tests.
pub(crate) async fn execution_protection_status(_: State<Arc<AppState>>) -> impl IntoResponse {
    use crate::api_types::ProtectionStatusResponse;

    (
        StatusCode::OK,
        Json(ProtectionStatusResponse {
            canonical_route: "/api/v1/execution/protection-status".to_string(),
            truth_state: "not_wired".to_string(),
            stop_order_wiring: "not_supported".to_string(),
            bracket_order_wiring: "not_supported".to_string(),
            note: "Protective stop and bracket orders are not supported on the current \
                   paper+alpaca canonical execution path.  Submit validation explicitly \
                   rejects order_type=\"stop\".  No OCO / OTOCO bracket types are wired \
                   to the Alpaca broker adapter.  The KillSwitchType::MissingProtectiveStop \
                   kill-switch policy is defined in the risk config but cannot be satisfied \
                   until stop order wiring is implemented (B5+).  This surface transitions to \
                   truth_state=\"broker_backed\" only when a future patch proves end-to-end \
                   broker stop / bracket order submission."
                .to_string(),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/replace-cancel-chains (EXEC-02)
// ---------------------------------------------------------------------------

/// Replace/cancel chain surface — DB-backed via EXEC-02.
///
/// Returns lifecycle events (cancel_ack, replace_ack, cancel_reject,
/// replace_reject) recorded by the orchestrator's Phase 3b hook for the
/// active run.  Source: `postgres.oms_order_lifecycle_events`.
///
/// truth_state values:
/// - `"no_db"` — DB pool unavailable.
/// - `"no_active_run"` — DB present but no active run ID known.
/// - `"active"` — DB-backed; chains may be empty (no cancel/replace yet).
pub(crate) async fn execution_replace_cancel_chains(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    use crate::api_types::{OrderLifecycleEventApiRow, ReplaceCancelChainsResponse};

    const CANONICAL: &str = "/api/v1/execution/replace-cancel-chains";

    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(ReplaceCancelChainsResponse {
                canonical_route: CANONICAL.to_string(),
                truth_state: "no_db".to_string(),
                backend: "unavailable".to_string(),
                note: "DB pool unavailable — lifecycle events cannot be read.".to_string(),
                chains: vec![],
            }),
        )
            .into_response();
    };

    let active_run_id = match st.current_status_snapshot().await {
        Ok(snap) => snap.active_run_id,
        Err(_) => None,
    };

    let Some(run_id) = active_run_id else {
        return (
            StatusCode::OK,
            Json(ReplaceCancelChainsResponse {
                canonical_route: CANONICAL.to_string(),
                truth_state: "no_active_run".to_string(),
                backend: "unavailable".to_string(),
                note: "No active run — no lifecycle events to show.".to_string(),
                chains: vec![],
            }),
        )
            .into_response();
    };

    let rows = match mqk_db::fetch_order_lifecycle_events_for_run(db, run_id).await {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "lifecycle_events_fetch_failed",
                    "detail": e.to_string(),
                })),
            )
                .into_response();
        }
    };

    let api_rows: Vec<OrderLifecycleEventApiRow> = rows
        .into_iter()
        .map(|r| OrderLifecycleEventApiRow {
            event_id: r.event_id,
            internal_order_id: r.internal_order_id,
            operation: r.operation,
            broker_order_id: r.broker_order_id,
            new_total_qty: r.new_total_qty,
            recorded_at_utc: r.recorded_at_utc.to_rfc3339(),
        })
        .collect();

    (
        StatusCode::OK,
        Json(ReplaceCancelChainsResponse {
            canonical_route: CANONICAL.to_string(),
            truth_state: "active".to_string(),
            backend: "postgres.oms_order_lifecycle_events".to_string(),
            note: "Source: oms_order_lifecycle_events. Events recorded for \
                   cancel_ack, replace_ack, cancel_reject, replace_reject \
                   operations per run by ExecutionOrchestrator Phase 3b (EXEC-02)."
                .to_string(),
            chains: api_rows,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/event-risk-status (EVENT-RISK-01)
// ---------------------------------------------------------------------------

/// Event-risk screening status surface.
///
/// EVENT-RISK-GATE-01 (Gate 1h) exists in the signal admission path and enforces
/// symbol-level blackout periods when `MQK_EVENT_RISK_BLACKOUT_PATH` is set.
/// `truth_state` reflects the current posture dynamically:
///
/// - `"partial"` — Gate 1h is configured and enforcing; the operator-declared
///   blackout check runs at signal admission.  Earnings calendar feed and
///   pre-event flattening remain absent.
/// - `"not_wired"` — `MQK_EVENT_RISK_BLACKOUT_PATH` is absent; the gate exists
///   in code but is not enforcing.
///
/// This route follows the exact pattern established by B4 (`execution_protection_status`)
/// so operator tooling and runbooks can explicitly distinguish the current
/// enforcement posture from "route unavailable".
///
/// # Why this matters
///
/// The backtest engine has an explicit `CorporateActionPolicy::ForbidPeriods`
/// that halts simulation on declared exclusion periods.  Gate 1h brings equivalent
/// enforcement to the live/paper execution path for operator-declared periods only
/// — not from a live calendar feed.  The following capabilities remain absent
/// regardless of Gate 1h configuration:
///
/// - No earnings calendar feed is connected (`earnings_calendar_feed = "not_connected"`).
/// - No pre-event position flattening gate exists (`pre_event_flattening = "not_wired"`).
pub(crate) async fn execution_event_risk_status(_: State<Arc<AppState>>) -> impl IntoResponse {
    use crate::earnings_calendar::earnings_calendar_surface_label;
    use crate::event_risk_blackout::blackout_surface_label;

    // signal_admission_gate reflects whether Gate 1h (static blackout) is active:
    //   "configured"     — MQK_EVENT_RISK_BLACKOUT_PATH is set (gate enforcing).
    //   "not_configured" — env var absent (gate present in code but not enforcing).
    let signal_admission_gate = blackout_surface_label().to_string();

    // earnings_calendar_feed reflects whether Gate 1h-calendar (earnings proximity) is active:
    //   "operator_file"  — MQK_EARNINGS_CALENDAR_PATH is set (earnings-calendar-v1 source active).
    //   "not_connected"  — env var absent (no earnings calendar source configured).
    let earnings_calendar_feed = earnings_calendar_surface_label().to_string();

    // truth_state reflects the overall screening posture:
    //   "partial"   — at least one event-risk gate is configured and enforcing;
    //                 one or more capabilities remain absent.
    //   "not_wired" — no event-risk gate is configured; nothing is enforcing.
    let truth_state =
        if signal_admission_gate == "configured" || earnings_calendar_feed == "operator_file" {
            "partial"
        } else {
            "not_wired"
        }
        .to_string();

    // pre_event_flattening reflects the flatten wiring state
    // (EVENT-RISK-FLATTEN-WIRE-01: wiring is in the execution loop):
    //   "active"          — wiring present AND ≥1 source configured; flatten orders
    //                       will be enqueued when blackout conditions are met.
    //   "wired_no_source" — wiring present but no source configured; evaluator
    //                       returns NotConfigured every tick (no-op).
    let flatten_source_active =
        signal_admission_gate == "configured" || earnings_calendar_feed == "operator_file";
    let pre_event_flattening = if flatten_source_active {
        "active"
    } else {
        "wired_no_source"
    }
    .to_string();

    (
        StatusCode::OK,
        Json(EventRiskStatusResponse {
            canonical_route: "/api/v1/execution/event-risk-status".to_string(),
            truth_state,
            earnings_calendar_feed,
            pre_event_flattening,
            signal_admission_gate,
            note: "Two operator-configured event-risk sources are available.  \
                   (1) Static blackout periods (Gate 1h): configure MQK_EVENT_RISK_BLACKOUT_PATH \
                   with a blackout-v1 JSON file to enforce symbol-level blackout ranges at signal \
                   ingestion (signal_admission_gate → \"configured\").  \
                   (2) Earnings calendar proximity (Gate 1h-calendar): configure \
                   MQK_EARNINGS_CALENDAR_PATH with an earnings-calendar-v1 JSON file; the gate \
                   derives blackout windows from declared earnings events with operator-specified \
                   pre/post windows (earnings_calendar_feed → \"operator_file\").  \
                   Either source active → truth_state = \"partial\".  \
                   No live data-provider feed is connected.  \
                   Pre-event position flattening is wired in the execution loop \
                   (EVENT-RISK-FLATTEN-WIRE-01): when any configured source returns FlattenRequired \
                   or Unavailable for a held position, a market close order is enqueued to the \
                   outbox for dispatch on the next tick.  \
                   pre_event_flattening = \"active\" when ≥1 source is configured; \
                   \"wired_no_source\" when no source is configured (no-op).  \
                   The backtest engine has CorporateActionPolicy::ForbidPeriods; both gates bring \
                   equivalent enforcement to the live/paper execution path for operator-declared \
                   periods only."
                .to_string(),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::lifecycle_stage_from_outbox_status;

    // U01: every known outbox status maps to a non-"unknown" lifecycle stage.
    #[test]
    fn known_statuses_map_to_named_stages() {
        let cases = [
            ("PENDING", "queued"),
            ("CLAIMED", "claimed"),
            ("DISPATCHING", "submitting"),
            ("SENT", "sent_to_broker"),
            ("ACKED", "acknowledged"),
            ("FAILED", "failed"),
            ("AMBIGUOUS", "ambiguous"),
        ];
        for (status, expected_stage) in cases {
            assert_eq!(
                lifecycle_stage_from_outbox_status(status),
                expected_stage,
                "status={status}"
            );
        }
    }

    // U02: unknown / future statuses map to "unknown" and never panic.
    #[test]
    fn unknown_status_maps_to_unknown() {
        assert_eq!(lifecycle_stage_from_outbox_status("MYSTERY"), "unknown");
        assert_eq!(lifecycle_stage_from_outbox_status(""), "unknown");
    }
}
