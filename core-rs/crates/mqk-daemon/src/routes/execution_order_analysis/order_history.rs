//! Per-order fill-history routes: timeline (A5A), trace (A5B), replay (A5C).
//!
//! All three handlers share the same access pattern:
//!   in-memory snapshot probe → DB gate → active-run gate → fetch fills → map rows.
//!
//! `lifecycle_stage_from_outbox_status` is a private function of the parent module
//! (`execution_order_analysis`); it is accessible here because this module is a
//! descendant of that module (Rust descendant-visibility rule).
//!
//! `oms_stage_label` is `pub(crate)` in `routes::helpers`.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use crate::api_types::{
    OrderReplayFrame, OrderReplayResponse, OrderTimelineResponse, OrderTimelineRow,
    OrderTraceResponse, OrderTraceRow,
};
use crate::state::AppState;

use super::super::helpers::oms_stage_label;
use super::lifecycle_stage_from_outbox_status;

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
