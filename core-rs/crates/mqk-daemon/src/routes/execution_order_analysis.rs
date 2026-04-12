//! Order analysis route handlers (A5 batch + outbox view).
//!
//! Contains: lifecycle_stage_from_outbox_status, execution_outbox,
//! execution_order_timeline, execution_order_trace, execution_order_replay,
//! execution_order_chart, execution_order_causality,
//! execution_protection_status, execution_replace_cancel_chains.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use crate::api_types::{
    ExecutionOutboxResponse, ExecutionOutboxRow, OrderCausalityCausalNode,
    OrderCausalityResponse, OrderChartResponse, OrderReplayFrame, OrderReplayResponse,
    OrderTimelineResponse, OrderTimelineRow, OrderTraceResponse, OrderTraceRow,
};
use crate::state::AppState;

use super::helpers::oms_stage_label;

// ---------------------------------------------------------------------------
// GET /api/v1/execution/outbox — OPS-08 / EXEC-06: paper execution timeline
// ---------------------------------------------------------------------------

/// Map a durable outbox status string to a display-friendly lifecycle stage.
///
/// Pure function — no state, no I/O.  All unknown values map to `"unknown"`.
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
// GET /api/v1/execution/orders/:order_id/timeline  (Batch A5A)
// ---------------------------------------------------------------------------

pub(crate) async fn execution_order_timeline(
    State(st): State<Arc<AppState>>,
    Path(order_id): Path<String>,
) -> impl IntoResponse {
    let order_id = order_id.trim().to_string();

    if order_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "order_id must not be blank",
            })),
        )
            .into_response();
    }

    let canonical_route = format!("/api/v1/execution/orders/{order_id}/timeline");

    // Step 1: Derive order identity fields from the in-memory execution snapshot.
    // The snapshot is ephemeral (not durable across restart), so all fields derived
    // from it are nullable in the response.
    let snap = st.execution_snapshot.read().await.clone();
    let order_in_snapshot = snap.as_ref().and_then(|s| {
        s.active_orders
            .iter()
            .find(|o| o.order_id == order_id)
            .cloned()
    });

    let broker_order_id = order_in_snapshot
        .as_ref()
        .and_then(|o| o.broker_order_id.clone());
    let symbol = order_in_snapshot.as_ref().map(|o| o.symbol.clone());
    let requested_qty = order_in_snapshot.as_ref().map(|o| o.total_qty);
    let filled_qty = order_in_snapshot.as_ref().map(|o| o.filled_qty);
    let current_status = order_in_snapshot.as_ref().map(|o| o.status.clone());
    let current_stage = current_status
        .as_deref()
        .map(|s| oms_stage_label(s).to_string());

    // Step 2: Check DB availability.
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(OrderTimelineResponse {
                canonical_route,
                truth_state: "no_db".to_string(),
                backend: "unavailable".to_string(),
                order_id,
                broker_order_id,
                symbol,
                requested_qty,
                filled_qty,
                current_status,
                current_stage,
                last_event_at: None,
                rows: vec![],
            }),
        )
            .into_response();
    };

    // Step 3: Get the active run_id from the durable status snapshot.
    let active_run_id = match st.current_status_snapshot().await {
        Ok(snap) => snap.active_run_id,
        Err(_) => None,
    };

    let Some(run_id) = active_run_id else {
        return (
            StatusCode::OK,
            Json(OrderTimelineResponse {
                canonical_route,
                truth_state: "no_order".to_string(),
                backend: "unavailable".to_string(),
                order_id,
                broker_order_id,
                symbol,
                requested_qty,
                filled_qty,
                current_status,
                current_stage,
                last_event_at: None,
                rows: vec![],
            }),
        )
            .into_response();
    };

    // Step 4: Fetch fill quality rows for this specific order, oldest-first.
    let fill_rows =
        match mqk_db::fetch_fill_quality_telemetry_for_order(db, run_id, &order_id).await {
            Ok(rows) => rows,
            Err(e) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": "timeline_fetch_failed",
                        "detail": e.to_string(),
                    })),
                )
                    .into_response();
            }
        };

    // Step 5: Map fill rows to timeline rows (oldest-first is already provided by the DB query).
    let last_event_at = fill_rows
        .last()
        .map(|r| r.fill_received_at_utc.to_rfc3339());

    let rows: Vec<OrderTimelineRow> = fill_rows
        .into_iter()
        .map(|r| {
            let detail = Some(format!(
                "qty={} fill_price={:.6} ({})",
                r.fill_qty,
                r.fill_price_micros as f64 / 1_000_000.0,
                r.fill_kind,
            ));
            OrderTimelineRow {
                event_id: r.telemetry_id.to_string(),
                ts_utc: r.fill_received_at_utc.to_rfc3339(),
                stage: r.fill_kind,
                source: "fill_quality_telemetry".to_string(),
                detail,
                fill_qty: Some(r.fill_qty),
                fill_price_micros: Some(r.fill_price_micros),
                slippage_bps: r.slippage_bps,
                provenance_ref: Some(r.provenance_ref),
            }
        })
        .collect();

    // Step 6: Determine truth_state from what we found.
    let truth_state = if !rows.is_empty() {
        "active"
    } else if order_in_snapshot.is_some() {
        "no_fills_yet"
    } else {
        "no_order"
    };

    let backend = if truth_state == "no_order" {
        "unavailable"
    } else {
        "postgres.fill_quality_telemetry"
    };

    (
        StatusCode::OK,
        Json(OrderTimelineResponse {
            canonical_route,
            truth_state: truth_state.to_string(),
            backend: backend.to_string(),
            order_id,
            broker_order_id,
            symbol,
            requested_qty,
            filled_qty,
            current_status,
            current_stage,
            last_event_at,
            rows,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/orders/:order_id/trace  (Batch A5B)
// ---------------------------------------------------------------------------

pub(crate) async fn execution_order_trace(
    State(st): State<Arc<AppState>>,
    Path(order_id): Path<String>,
) -> impl IntoResponse {
    let order_id = order_id.trim().to_string();

    if order_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "order_id must not be blank",
            })),
        )
            .into_response();
    }

    let canonical_route = format!("/api/v1/execution/orders/{order_id}/trace");

    // Step 1: Derive order identity and outbox status from the in-memory
    // execution snapshot (ephemeral — not durable across restart).
    let snap = st.execution_snapshot.read().await.clone();

    let order_in_snapshot = snap.as_ref().and_then(|s| {
        s.active_orders
            .iter()
            .find(|o| o.order_id == order_id)
            .cloned()
    });

    let broker_order_id = order_in_snapshot
        .as_ref()
        .and_then(|o| o.broker_order_id.clone());
    let symbol = order_in_snapshot.as_ref().map(|o| o.symbol.clone());
    let requested_qty = order_in_snapshot.as_ref().map(|o| o.total_qty);
    let filled_qty = order_in_snapshot.as_ref().map(|o| o.filled_qty);
    let current_status = order_in_snapshot.as_ref().map(|o| o.status.clone());
    let current_stage = current_status
        .as_deref()
        .map(|s| oms_stage_label(s).to_string());

    // Outbox status from the in-memory pending outbox window.
    // idempotency_key == order_id by convention established in OUTBOX_SIGNAL_SOURCE.
    let outbox_snap = snap.as_ref().and_then(|s| {
        s.pending_outbox
            .iter()
            .find(|o| o.idempotency_key == order_id)
            .cloned()
    });
    let outbox_status = outbox_snap.as_ref().map(|o| o.status.clone());
    let outbox_lifecycle_stage = outbox_status
        .as_deref()
        .map(|s| lifecycle_stage_from_outbox_status(s).to_string());

    // Step 2: Check DB availability.
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(OrderTraceResponse {
                canonical_route,
                truth_state: "no_db".to_string(),
                backend: "unavailable".to_string(),
                order_id,
                broker_order_id,
                symbol,
                requested_qty,
                filled_qty,
                current_status,
                current_stage,
                outbox_status,
                outbox_lifecycle_stage,
                last_event_at: None,
                rows: vec![],
            }),
        )
            .into_response();
    };

    // Step 3: Get the active run_id from the durable status snapshot.
    let active_run_id = match st.current_status_snapshot().await {
        Ok(snap) => snap.active_run_id,
        Err(_) => None,
    };

    let Some(run_id) = active_run_id else {
        return (
            StatusCode::OK,
            Json(OrderTraceResponse {
                canonical_route,
                truth_state: "no_order".to_string(),
                backend: "unavailable".to_string(),
                order_id,
                broker_order_id,
                symbol,
                requested_qty,
                filled_qty,
                current_status,
                current_stage,
                outbox_status,
                outbox_lifecycle_stage,
                last_event_at: None,
                rows: vec![],
            }),
        )
            .into_response();
    };

    // Step 4: Fetch fill quality rows for this specific order, oldest-first.
    let fill_rows =
        match mqk_db::fetch_fill_quality_telemetry_for_order(db, run_id, &order_id).await {
            Ok(rows) => rows,
            Err(e) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": "trace_fetch_failed",
                        "detail": e.to_string(),
                    })),
                )
                    .into_response();
            }
        };

    // Step 5: Map fill rows to trace rows (oldest-first provided by DB query).
    let last_event_at = fill_rows
        .last()
        .map(|r| r.fill_received_at_utc.to_rfc3339());

    let rows: Vec<OrderTraceRow> = fill_rows
        .into_iter()
        .map(|r| {
            let detail = Some(format!(
                "qty={} fill_price={:.6} ({})",
                r.fill_qty,
                r.fill_price_micros as f64 / 1_000_000.0,
                r.fill_kind,
            ));
            OrderTraceRow {
                event_id: r.telemetry_id.to_string(),
                ts_utc: r.fill_received_at_utc.to_rfc3339(),
                stage: r.fill_kind,
                source: "fill_quality_telemetry".to_string(),
                detail,
                fill_qty: Some(r.fill_qty),
                fill_price_micros: Some(r.fill_price_micros),
                slippage_bps: r.slippage_bps,
                submit_ts_utc: r.submit_ts_utc.map(|t| t.to_rfc3339()),
                submit_to_fill_ms: r.submit_to_fill_ms,
                side: Some(r.side),
                provenance_ref: Some(r.provenance_ref),
            }
        })
        .collect();

    // Step 6: Determine truth_state from what was found.
    let truth_state = if !rows.is_empty() {
        "active"
    } else if order_in_snapshot.is_some() {
        "no_fills_yet"
    } else {
        "no_order"
    };

    let backend = if truth_state == "no_order" {
        "unavailable"
    } else {
        "postgres.fill_quality_telemetry"
    };

    (
        StatusCode::OK,
        Json(OrderTraceResponse {
            canonical_route,
            truth_state: truth_state.to_string(),
            backend: backend.to_string(),
            order_id,
            broker_order_id,
            symbol,
            requested_qty,
            filled_qty,
            current_status,
            current_stage,
            outbox_status,
            outbox_lifecycle_stage,
            last_event_at,
            rows,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/orders/:order_id/replay  (Batch A5C)
// ---------------------------------------------------------------------------

pub(crate) async fn execution_order_replay(
    State(st): State<Arc<AppState>>,
    Path(order_id): Path<String>,
) -> impl IntoResponse {
    let order_id = order_id.trim().to_string();

    if order_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "order_id must not be blank",
            })),
        )
            .into_response();
    }

    let canonical_route = format!("/api/v1/execution/orders/{order_id}/replay");

    // Step 1: Derive identity and outbox status from the in-memory execution snapshot
    // (ephemeral — not durable across restart).
    let snap = st.execution_snapshot.read().await.clone();

    let order_in_snapshot = snap.as_ref().and_then(|s| {
        s.active_orders
            .iter()
            .find(|o| o.order_id == order_id)
            .cloned()
    });

    let symbol = order_in_snapshot.as_ref().map(|o| o.symbol.clone());
    let requested_qty = order_in_snapshot.as_ref().map(|o| o.total_qty);
    let current_status = order_in_snapshot.as_ref().map(|o| o.status.clone());

    // Outbox status from the in-memory pending outbox window (idempotency_key == order_id).
    let outbox_snap = snap.as_ref().and_then(|s| {
        s.pending_outbox
            .iter()
            .find(|o| o.idempotency_key == order_id)
            .cloned()
    });
    let queue_status = outbox_snap
        .as_ref()
        .map(|o| o.status.clone())
        .unwrap_or_else(|| "unknown".to_string());

    // Step 2: Check DB availability.
    let Some(db) = st.db.as_ref() else {
        let title = format!("order replay — {order_id}");
        return (
            StatusCode::OK,
            Json(OrderReplayResponse {
                canonical_route,
                truth_state: "no_db".to_string(),
                backend: "unavailable".to_string(),
                order_id: order_id.clone(),
                replay_id: order_id.clone(),
                replay_scope: "single_order".to_string(),
                source: "fill_quality_telemetry".to_string(),
                title,
                current_frame_index: 0,
                frames: vec![],
            }),
        )
            .into_response();
    };

    // Step 3: Get the active run_id from the durable status snapshot.
    let active_run_id = match st.current_status_snapshot().await {
        Ok(snap) => snap.active_run_id,
        Err(_) => None,
    };

    let Some(run_id) = active_run_id else {
        let title = format!("order replay — {order_id}");
        return (
            StatusCode::OK,
            Json(OrderReplayResponse {
                canonical_route,
                truth_state: "no_order".to_string(),
                backend: "unavailable".to_string(),
                order_id: order_id.clone(),
                replay_id: order_id.clone(),
                replay_scope: "single_order".to_string(),
                source: "fill_quality_telemetry".to_string(),
                title,
                current_frame_index: 0,
                frames: vec![],
            }),
        )
            .into_response();
    };

    // Step 4: Fetch fill quality rows for this specific order, oldest-first.
    let fill_rows =
        match mqk_db::fetch_fill_quality_telemetry_for_order(db, run_id, &order_id).await {
            Ok(rows) => rows,
            Err(e) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": "replay_fetch_failed",
                        "detail": e.to_string(),
                    })),
                )
                    .into_response();
            }
        };

    // Step 5: Map fill rows to replay frames (oldest-first; accumulate cumulative fill qty).
    let oms_state = current_status.as_deref().unwrap_or("unknown").to_string();
    let mut cumulative_filled: i64 = 0;

    let frames: Vec<OrderReplayFrame> = fill_rows
        .into_iter()
        .map(|r| {
            cumulative_filled += r.fill_qty;
            let open_qty = requested_qty.map(|rq| (rq - cumulative_filled).max(0));
            let boundary_tags = if r.fill_kind == "final_fill" {
                vec!["final_fill".to_string()]
            } else {
                vec![]
            };
            let state_delta = format!(
                "fill_qty={} fill_price={:.6} ({})",
                r.fill_qty,
                r.fill_price_micros as f64 / 1_000_000.0,
                r.fill_kind,
            );
            OrderReplayFrame {
                frame_id: r.telemetry_id.to_string(),
                timestamp: r.fill_received_at_utc.to_rfc3339(),
                subsystem: "execution".to_string(),
                event_type: r.fill_kind,
                state_delta,
                message_digest: r.provenance_ref,
                order_execution_state: oms_state.clone(),
                oms_state: oms_state.clone(),
                filled_qty: cumulative_filled,
                open_qty,
                risk_state: "unknown".to_string(),
                reconcile_state: "unknown".to_string(),
                queue_status: queue_status.clone(),
                anomaly_tags: vec![],
                boundary_tags,
            }
        })
        .collect();

    // Step 6: Determine truth_state from what was found.
    let truth_state = if !frames.is_empty() {
        "active"
    } else if order_in_snapshot.is_some() {
        "no_fills_yet"
    } else {
        "no_order"
    };

    let backend = if truth_state == "no_order" {
        "unavailable"
    } else {
        "postgres.fill_quality_telemetry"
    };

    let current_frame_index = frames.len().saturating_sub(1);
    let title = if let Some(sym) = &symbol {
        format!("{sym} {order_id} replay")
    } else {
        format!("order replay — {order_id}")
    };

    (
        StatusCode::OK,
        Json(OrderReplayResponse {
            canonical_route,
            truth_state: truth_state.to_string(),
            backend: backend.to_string(),
            order_id: order_id.clone(),
            replay_id: order_id.clone(),
            replay_scope: "single_order".to_string(),
            source: "fill_quality_telemetry".to_string(),
            title,
            current_frame_index,
            frames,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/orders/:order_id/chart  (Batch A5D)
// ---------------------------------------------------------------------------

pub(crate) async fn execution_order_chart(
    State(st): State<Arc<AppState>>,
    Path(order_id): Path<String>,
) -> impl IntoResponse {
    let order_id = order_id.trim().to_string();

    if order_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "order_id must not be blank",
            })),
        )
            .into_response();
    }

    let canonical_route = format!("/api/v1/execution/orders/{order_id}/chart");

    // Probe the in-memory snapshot for basic identity (symbol, existence).
    // This is the only source we have — no durable chart data exists.
    let snap = st.execution_snapshot.read().await.clone();
    let order_in_snapshot = snap.as_ref().and_then(|s| {
        s.active_orders
            .iter()
            .find(|o| o.order_id == order_id)
            .cloned()
    });
    let symbol = order_in_snapshot.as_ref().map(|o| o.symbol.clone());

    // truth_state: no_order when the order is not visible in any current source;
    // no_bars otherwise.  We intentionally do not probe the DB here: there is no
    // joinable bar/candle table for a specific order, so a DB probe would not
    // change the truth_state from no_bars.
    let truth_state = if order_in_snapshot.is_some() {
        "no_bars"
    } else {
        "no_order"
    };

    (
        StatusCode::OK,
        Json(OrderChartResponse {
            canonical_route,
            truth_state: truth_state.to_string(),
            backend: "unavailable".to_string(),
            order_id,
            symbol,
            comment: "No per-order bar/candle source is available. Chart data requires \
                market-data wiring that is not yet implemented (open)."
                .to_string(),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/orders/:order_id/causality  (Batch A5E)
// ---------------------------------------------------------------------------

/// Lanes that are always unproven at this tier.
const UNPROVEN_CAUSALITY_LANES: &[&str] = &[
    "signal",
    "intent",
    "broker_ack",
    "risk",
    "reconcile",
    "portfolio",
];

pub(crate) async fn execution_order_causality(
    State(st): State<Arc<AppState>>,
    Path(order_id): Path<String>,
) -> impl IntoResponse {
    let order_id = order_id.trim().to_string();

    if order_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "order_id must not be blank",
            })),
        )
            .into_response();
    }

    let canonical_route = format!("/api/v1/execution/orders/{order_id}/causality");

    let unproven_lanes: Vec<String> = UNPROVEN_CAUSALITY_LANES
        .iter()
        .map(|s| s.to_string())
        .collect();

    // Step 1: Probe in-memory snapshot for identity.
    let snap = st.execution_snapshot.read().await.clone();
    let order_in_snapshot = snap.as_ref().and_then(|s| {
        s.active_orders
            .iter()
            .find(|o| o.order_id == order_id)
            .cloned()
    });
    let symbol = order_in_snapshot.as_ref().map(|o| o.symbol.clone());

    // Step 2: Check DB availability.
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(OrderCausalityResponse {
                canonical_route,
                truth_state: "no_db".to_string(),
                backend: "unavailable".to_string(),
                order_id,
                symbol,
                proven_lanes: vec![],
                unproven_lanes,
                nodes: vec![],
                comment: "No database connection — causality unavailable.".to_string(),
            }),
        )
            .into_response();
    };

    // Step 3: Get active run_id.
    let active_run_id = match st.current_status_snapshot().await {
        Ok(snap) => snap.active_run_id,
        Err(_) => None,
    };

    let Some(run_id) = active_run_id else {
        return (
            StatusCode::OK,
            Json(OrderCausalityResponse {
                canonical_route,
                truth_state: "no_order".to_string(),
                backend: "unavailable".to_string(),
                order_id,
                symbol,
                proven_lanes: vec![],
                unproven_lanes,
                nodes: vec![],
                comment: "No active run — causality unavailable.".to_string(),
            }),
        )
            .into_response();
    };

    // Step 4: Fetch fill quality rows for this order, oldest-first.
    let fill_rows =
        match mqk_db::fetch_fill_quality_telemetry_for_order(db, run_id, &order_id).await {
            Ok(rows) => rows,
            Err(e) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": "causality_fetch_failed",
                        "detail": e.to_string(),
                    })),
                )
                    .into_response();
            }
        };

    // Step 5: Build execution causality nodes (fill lane only).
    let nodes: Vec<OrderCausalityCausalNode> = {
        let mut prev_ts_ms: Option<i64> = None;
        fill_rows
            .iter()
            .map(|r| {
                let ts_ms = r.fill_received_at_utc.timestamp_millis(); // allow: ops-metadata
                let elapsed = prev_ts_ms.map(|prev| ts_ms - prev);
                prev_ts_ms = Some(ts_ms);
                OrderCausalityCausalNode {
                    node_key: format!("execution_fill_{}", r.telemetry_id),
                    node_type: "execution_fill".to_string(),
                    title: format!("{} {}", r.fill_kind, r.symbol),
                    status: "ok".to_string(),
                    subsystem: "execution".to_string(),
                    linked_id: r.broker_fill_id.clone(),
                    timestamp: Some(r.fill_received_at_utc.to_rfc3339()),
                    elapsed_from_prev_ms: elapsed,
                    anomaly_tags: vec![],
                    summary: format!(
                        "fill_qty={} fill_price={:.6} ({})",
                        r.fill_qty,
                        r.fill_price_micros as f64 / 1_000_000.0,
                        r.fill_kind,
                    ),
                }
            })
            .collect()
    };

    // Step 6: Determine truth_state.
    let (truth_state, backend, proven_lanes) = if !nodes.is_empty() {
        (
            "partial",
            "postgres.fill_quality_telemetry",
            vec!["execution_fill".to_string()],
        )
    } else if order_in_snapshot.is_some() {
        ("no_fills_yet", "postgres.fill_quality_telemetry", vec![])
    } else {
        ("no_order", "unavailable", vec![])
    };

    (
        StatusCode::OK,
        Json(OrderCausalityResponse {
            canonical_route,
            truth_state: truth_state.to_string(),
            backend: backend.to_string(),
            order_id,
            symbol,
            proven_lanes,
            unproven_lanes,
            nodes,
            comment: "Causality is partial: only fill events from fill_quality_telemetry \
                are joinable by internal_order_id. Signal, intent, broker ACK, risk, \
                portfolio, and reconcile lanes are not linked in the current schema."
                .to_string(),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/replace-cancel-chains (A4)
// ---------------------------------------------------------------------------

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
