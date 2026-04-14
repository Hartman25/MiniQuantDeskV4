//! Per-order diagnostic routes: chart (A5D), causality (A5E).
//!
//! Neither handler shares helpers with other clusters in this module, making
//! both fully self-contained for extraction.
//!
//! `execution_order_chart` — pure in-memory snapshot probe; no DB queries.
//! `execution_order_causality` — multi-lane DB-backed causality graph.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use crate::api_types::{OrderCausalityCausalNode, OrderCausalityResponse, OrderChartResponse};
use crate::state::AppState;

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

    // Step 4b: Fetch the durable outbox row for the intent lane (non-fatal).
    //
    // idempotency_key == order_id by convention established in OUTBOX_SIGNAL_SOURCE.
    // On DB error, treat as absent — fills are still surfaced.
    let outbox_row = mqk_db::outbox_fetch_by_idempotency_key(db, &order_id)
        .await
        .unwrap_or(None);

    // Step 4c: Fetch broker ACK inbox rows for the broker_ack lane (non-fatal).
    //
    // Queries oms_inbox for rows where event_kind = 'ack' and internal_order_id
    // matches.  ACK events are stored by the WS inbound path (alpaca_inbound.rs)
    // with event_kind = "ack" via broker_event_kind(BrokerEvent::Ack).
    // On DB error, treat as absent — other lanes are still surfaced.
    let ack_rows = mqk_db::inbox_fetch_ack_rows_for_order(db, run_id, &order_id)
        .await
        .unwrap_or_default();

    // Step 5: Build intent lane nodes from the outbox row (when present).
    //
    // outbox_enqueued: always present when the outbox row exists.
    // outbox_sent: only when sent_at_utc is Some (order reached the broker adapter).
    let outbox_enqueued_node: Option<OrderCausalityCausalNode> =
        outbox_row.as_ref().map(|ob| OrderCausalityCausalNode {
            node_key: format!("outbox_enqueued:{order_id}"),
            node_type: "outbox_enqueued".to_string(),
            title: "intent enqueued to outbox".to_string(),
            status: "ok".to_string(),
            subsystem: "execution".to_string(),
            linked_id: Some(ob.outbox_id.to_string()),
            timestamp: Some(ob.created_at_utc.to_rfc3339()),
            elapsed_from_prev_ms: None,
            anomaly_tags: vec![],
            summary: format!("status={}", ob.status),
            submit_ts_utc: None,
            submit_to_fill_ms: None,
        });

    let outbox_sent_node: Option<OrderCausalityCausalNode> = outbox_row.as_ref().and_then(|ob| {
        let sent = ob.sent_at_utc?;
        let enqueued_ms = ob.created_at_utc.timestamp_millis(); // allow: ops-metadata
        Some(OrderCausalityCausalNode {
            node_key: format!("outbox_sent:{order_id}"),
            node_type: "outbox_sent".to_string(),
            title: "intent sent to broker".to_string(),
            status: "ok".to_string(),
            subsystem: "execution".to_string(),
            linked_id: Some(ob.outbox_id.to_string()),
            timestamp: Some(sent.to_rfc3339()),
            elapsed_from_prev_ms: Some(sent.timestamp_millis() - enqueued_ms), // allow: ops-metadata
            anomaly_tags: vec![],
            summary: "order dispatched to broker adapter".to_string(),
            submit_ts_utc: None,
            submit_to_fill_ms: None,
        })
    });

    // Step 6: Build execution causality nodes (broker_ack + fill lanes).
    //
    // Ordering: outbox_enqueued → outbox_sent → broker_ack → submit_event → execution_fills
    //
    // broker_ack nodes come from oms_inbox rows where event_kind = 'ack'.
    // Each ACK row yields one node; linked_id carries the broker_message_id.
    //
    // If the first fill has submit_ts_utc, a synthetic "submit_event" node is
    // prepended to the fill chain so the chain is anchored to the submit moment.
    let nodes: Vec<OrderCausalityCausalNode> = {
        // Build broker_ack nodes (one per ACK inbox row, oldest-first).
        let ack_nodes: Vec<OrderCausalityCausalNode> = ack_rows
            .iter()
            .enumerate()
            .map(|(i, r)| OrderCausalityCausalNode {
                node_key: format!("broker_ack_{}_{}", order_id, i),
                node_type: "broker_ack".to_string(),
                title: "broker ACK received".to_string(),
                status: "ok".to_string(),
                subsystem: "execution".to_string(),
                linked_id: Some(r.broker_message_id.clone()),
                timestamp: Some(r.received_at_utc.to_rfc3339()),
                elapsed_from_prev_ms: None,
                anomaly_tags: vec![],
                summary: format!("inbox_id={}", r.inbox_id),
                submit_ts_utc: None,
                submit_to_fill_ms: None,
            })
            .collect();

        // Determine whether a synthetic submit anchor should precede the fills.
        let submit_anchor: Option<OrderCausalityCausalNode> = fill_rows.first().and_then(|r| {
            r.submit_ts_utc.map(|ts| OrderCausalityCausalNode {
                node_key: format!("submit:{order_id}"),
                node_type: "submit_event".to_string(),
                title: "order submitted".to_string(),
                status: "ok".to_string(),
                subsystem: "execution".to_string(),
                linked_id: None,
                timestamp: Some(ts.to_rfc3339()),
                elapsed_from_prev_ms: None,
                anomaly_tags: vec![],
                summary: String::new(),
                submit_ts_utc: None,
                submit_to_fill_ms: None,
            })
        });

        let mut prev_ts_ms: Option<i64> = submit_anchor
            .as_ref()
            .and_then(|n| n.timestamp.as_deref())
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.timestamp_millis()); // allow: ops-metadata

        let fill_nodes: Vec<OrderCausalityCausalNode> = fill_rows
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
                    submit_ts_utc: r.submit_ts_utc.map(|ts| ts.to_rfc3339()),
                    submit_to_fill_ms: r.submit_to_fill_ms,
                }
            })
            .collect();

        let mut out = Vec::with_capacity(fill_nodes.len() + ack_nodes.len() + 3);
        if let Some(n) = outbox_enqueued_node {
            out.push(n);
        }
        if let Some(n) = outbox_sent_node {
            out.push(n);
        }
        out.extend(ack_nodes);
        if let Some(anchor) = submit_anchor {
            out.push(anchor);
        }
        out.extend(fill_nodes);
        out
    };

    // Step 7: Determine truth_state and proven/unproven lanes.
    //
    // "intent" is proven when the durable oms_outbox row is found.
    // "broker_ack" is proven when oms_inbox ACK rows exist for this order.
    // "execution_fill" is proven when fill_quality_telemetry rows exist.
    let has_outbox = outbox_row.is_some();
    let has_ack = !ack_rows.is_empty();
    let has_fills = !fill_rows.is_empty();

    let proven_lanes: Vec<String> = {
        let mut lanes = Vec::new();
        if has_outbox {
            lanes.push("intent".to_string());
        }
        if has_ack {
            lanes.push("broker_ack".to_string());
        }
        if has_fills {
            lanes.push("execution_fill".to_string());
        }
        lanes
    };

    // Remove any lane from unproven that is now proven.
    let unproven_lanes: Vec<String> = UNPROVEN_CAUSALITY_LANES
        .iter()
        .filter(|&&lane| !proven_lanes.iter().any(|p| p == lane))
        .map(|s| s.to_string())
        .collect();

    let has_any = has_fills || has_outbox || has_ack;
    let truth_state = if has_any {
        "partial"
    } else if order_in_snapshot.is_some() {
        "no_fills_yet"
    } else {
        "no_order"
    };

    let backend: String = {
        let mut parts: Vec<&str> = Vec::new();
        if has_outbox {
            parts.push("postgres.oms_outbox");
        }
        if has_ack {
            parts.push("postgres.oms_inbox");
        }
        if has_fills {
            parts.push("postgres.fill_quality_telemetry");
        }
        if parts.is_empty() {
            "unavailable".to_string()
        } else {
            parts.join(",")
        }
    };

    (
        StatusCode::OK,
        Json(OrderCausalityResponse {
            canonical_route,
            truth_state: truth_state.to_string(),
            backend,
            order_id,
            symbol,
            proven_lanes,
            unproven_lanes,
            nodes,
            comment: "Causality is partial: intent nodes from oms_outbox, broker ACK events \
                from oms_inbox, and fill events from fill_quality_telemetry are proven when \
                present. Signal, risk, portfolio, and reconcile lanes are not linked in the \
                current schema."
                .to_string(),
        }),
    )
        .into_response()
}
