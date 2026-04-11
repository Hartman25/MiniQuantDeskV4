//! Execution route handlers.
//!
//! Contains: execution_summary, execution_orders, execution_order_submit,
//! execution_order_cancel, execution_fill_quality, ValidatedManualOrderSubmit,
//! validate_manual_order_submit, validate_manual_order_cancel, parse_integer_field,
//! manual_order_submit_response, manual_order_cancel_response.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use crate::api_types::{
    ExecutionOrderRow, ExecutionOutboxResponse, ExecutionOutboxRow, ExecutionSummaryResponse,
    FillQualityTelemetryResponse, FillQualityTelemetryRow, ManualOrderCancelRequest,
    ManualOrderCancelResponse, ManualOrderSubmitRequest, ManualOrderSubmitResponse,
    OrderCausalityCausalNode, OrderCausalityResponse, OrderChartResponse, OrderReplayFrame,
    OrderReplayResponse, OrderTimelineResponse, OrderTimelineRow, OrderTraceResponse,
    OrderTraceRow,
};
use crate::state::AppState;

use super::helpers::oms_stage_label;

// ---------------------------------------------------------------------------
// GET /api/v1/execution/summary
// ---------------------------------------------------------------------------

pub(crate) async fn execution_summary(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.execution_snapshot.read().await.clone();

    let summary = if let Some(snapshot) = snap {
        let active_orders = snapshot.active_orders.len();
        let pending_orders = snapshot
            .pending_outbox
            .iter()
            .filter(|o| o.status == "PENDING" || o.status == "CLAIMED")
            .count();
        let dispatching_orders = snapshot
            .pending_outbox
            .iter()
            .filter(|o| o.status == "DISPATCHING" || o.status == "SENT")
            .count();
        // Derived from the current OMS snapshot: count of orders in "Rejected"
        // state.  Not a durable all-day count — reflects the current snapshot.
        let reject_count_today = snapshot
            .active_orders
            .iter()
            .filter(|o| o.status == "Rejected")
            .count();

        ExecutionSummaryResponse {
            has_snapshot: true,
            active_orders,
            pending_orders,
            dispatching_orders,
            reject_count_today,
            cancel_replace_count_today: None,
            avg_ack_latency_ms: None,
            stuck_orders: 0,
        }
    } else {
        ExecutionSummaryResponse {
            has_snapshot: false,
            active_orders: 0,
            pending_orders: 0,
            dispatching_orders: 0,
            reject_count_today: 0,
            cancel_replace_count_today: None,
            avg_ack_latency_ms: None,
            stuck_orders: 0,
        }
    };

    (StatusCode::OK, Json(summary)).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/orders
// ---------------------------------------------------------------------------

pub(crate) async fn execution_orders(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.execution_snapshot.read().await.clone();

    let Some(snapshot) = snap else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "no_execution_snapshot",
                "detail": "Execution loop has not started or has no active run; OMS order truth is unavailable."
            })),
        )
            .into_response();
    };

    // Snapshot the side cache once and release the lock before building rows.
    let sides = st.local_order_sides.read().await.clone();

    let updated_at = snapshot.snapshot_at_utc.to_rfc3339();
    let rows: Vec<ExecutionOrderRow> = snapshot
        .active_orders
        .iter()
        .map(|o| {
            let has_critical = o.status == "Rejected";
            let current_stage = oms_stage_label(&o.status).to_string();
            // Derive side from the local side cache populated at signal intake /
            // manual submit.  None when the order pre-dates the current run or
            // the side was never recorded — honest null, not fabricated.
            let side = sides.get(&o.order_id).map(|s| match s {
                mqk_reconcile::Side::Buy => "buy".to_string(),
                mqk_reconcile::Side::Sell => "sell".to_string(),
            });
            ExecutionOrderRow {
                internal_order_id: o.order_id.clone(),
                broker_order_id: o.broker_order_id.clone(),
                symbol: o.symbol.clone(),
                strategy_id: None,
                side,
                order_type: None,
                requested_qty: o.total_qty,
                filled_qty: o.filled_qty,
                current_status: o.status.clone(),
                current_stage,
                age_ms: None,
                has_warning: false,
                has_critical,
                updated_at: updated_at.clone(),
            }
        })
        .collect();

    (StatusCode::OK, Json(rows)).into_response()
}

// ---------------------------------------------------------------------------
// POST /api/v1/execution/orders
// ---------------------------------------------------------------------------

pub(crate) async fn execution_order_submit(
    State(st): State<Arc<AppState>>,
    Json(body): Json<ManualOrderSubmitRequest>,
) -> Response {
    let validated = match validate_manual_order_submit(body) {
        Ok(validated) => validated,
        Err((client_request_id, blockers)) => {
            return manual_order_submit_response(
                StatusCode::BAD_REQUEST,
                false,
                "rejected",
                client_request_id,
                None,
                blockers,
            );
        }
    };

    let _lifecycle = st.lifecycle_guard().await;

    let Some(db) = st.db.as_ref() else {
        return manual_order_submit_response(
            StatusCode::SERVICE_UNAVAILABLE,
            false,
            "unavailable",
            validated.client_request_id,
            None,
            vec!["durable execution DB truth is unavailable on this daemon".to_string()],
        );
    };

    let (durable_arm_state, durable_arm_reason) = match mqk_db::load_arm_state(db).await {
        Ok(Some((state, reason))) => (state, reason),
        Ok(None) => {
            return manual_order_submit_response(
                StatusCode::FORBIDDEN,
                false,
                "rejected",
                validated.client_request_id,
                None,
                vec![
                    "execution order submit refused: durable arm state is not armed; fresh systems default to disarmed until explicitly armed"
                        .to_string(),
                ],
            );
        }
        Err(err) => {
            return manual_order_submit_response(
                StatusCode::SERVICE_UNAVAILABLE,
                false,
                "unavailable",
                validated.client_request_id,
                None,
                vec![format!(
                    "execution order submit unavailable: durable arm-state truth could not be loaded: {err}"
                )],
            );
        }
    };

    if durable_arm_state != "ARMED" {
        let blocker = match durable_arm_reason.as_deref() {
            Some("OperatorHalt") => {
                "execution order submit refused: durable arm state is halted".to_string()
            }
            Some(reason) => {
                format!("execution order submit refused: durable arm state is disarmed ({reason})")
            }
            None => "execution order submit refused: durable arm state is not armed".to_string(),
        };
        return manual_order_submit_response(
            StatusCode::FORBIDDEN,
            false,
            "rejected",
            validated.client_request_id,
            None,
            vec![blocker],
        );
    }

    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => {
            return manual_order_submit_response(
                StatusCode::SERVICE_UNAVAILABLE,
                false,
                "unavailable",
                validated.client_request_id,
                None,
                vec![err.to_string()],
            );
        }
    };

    let Some(active_run_id) = status.active_run_id else {
        return manual_order_submit_response(
            StatusCode::CONFLICT,
            false,
            "unavailable",
            validated.client_request_id,
            None,
            vec!["execution order submit refused: no active durable run is available".to_string()],
        );
    };

    if status.state != "running" {
        let mut blockers = vec![format!(
            "execution order submit refused: runtime state '{}' is not accepting operator orders",
            status.state
        )];
        if let Some(note) = status.notes {
            blockers.push(note);
        }
        return manual_order_submit_response(
            StatusCode::CONFLICT,
            false,
            "unavailable",
            validated.client_request_id,
            Some(active_run_id),
            blockers,
        );
    }

    let order_json = validated.order_json();
    match mqk_db::outbox_enqueue(db, active_run_id, &validated.client_request_id, order_json).await
    {
        Ok(true) => manual_order_submit_response(
            StatusCode::OK,
            true,
            "enqueued",
            validated.client_request_id,
            Some(active_run_id),
            vec![],
        ),
        Ok(false) => {
            let mut blockers = vec![format!(
                "client_request_id '{}' already exists; no new outbox row was created",
                validated.client_request_id
            )];
            if let Ok(Some(existing)) =
                mqk_db::outbox_fetch_by_idempotency_key(db, &validated.client_request_id).await
            {
                if existing.run_id != active_run_id {
                    blockers.push(format!(
                        "existing order intent is already bound to durable run {}",
                        existing.run_id
                    ));
                }
            }
            manual_order_submit_response(
                StatusCode::OK,
                false,
                "duplicate",
                validated.client_request_id,
                Some(active_run_id),
                blockers,
            )
        }
        Err(err) => manual_order_submit_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            false,
            "unavailable",
            validated.client_request_id,
            Some(active_run_id),
            vec![format!("outbox enqueue failed: {err}")],
        ),
    }
}

// ---------------------------------------------------------------------------
// POST /api/v1/execution/orders/:order_id/cancel
// ---------------------------------------------------------------------------

pub(crate) async fn execution_order_cancel(
    State(st): State<Arc<AppState>>,
    Path(order_id): Path<String>,
    Json(body): Json<ManualOrderCancelRequest>,
) -> Response {
    let order_id = order_id.trim().to_string();
    if order_id.is_empty() {
        return manual_order_cancel_response(
            StatusCode::BAD_REQUEST,
            false,
            "rejected",
            String::new(),
            None,
            vec!["order_id must not be blank".to_string()],
        );
    }

    let cancel_request_id = match validate_manual_order_cancel(body) {
        Ok(cancel_request_id) => cancel_request_id,
        Err(blockers) => {
            return manual_order_cancel_response(
                StatusCode::BAD_REQUEST,
                false,
                "rejected",
                order_id,
                None,
                blockers,
            );
        }
    };

    let _lifecycle = st.lifecycle_guard().await;

    let Some(db) = st.db.as_ref() else {
        return manual_order_cancel_response(
            StatusCode::SERVICE_UNAVAILABLE,
            false,
            "unavailable",
            order_id,
            None,
            vec!["durable execution DB truth is unavailable on this daemon".to_string()],
        );
    };

    let (durable_arm_state, durable_arm_reason) = match mqk_db::load_arm_state(db).await {
        Ok(Some((state, reason))) => (state, reason),
        Ok(None) => {
            return manual_order_cancel_response(
                StatusCode::FORBIDDEN,
                false,
                "rejected",
                order_id,
                None,
                vec![
                    "execution order cancel refused: durable arm state is not armed; fresh systems default to disarmed until explicitly armed"
                        .to_string(),
                ],
            );
        }
        Err(err) => {
            return manual_order_cancel_response(
                StatusCode::SERVICE_UNAVAILABLE,
                false,
                "unavailable",
                order_id,
                None,
                vec![format!(
                    "execution order cancel unavailable: durable arm-state truth could not be loaded: {err}"
                )],
            );
        }
    };

    if durable_arm_state != "ARMED" {
        let blocker = match durable_arm_reason.as_deref() {
            Some("OperatorHalt") => {
                "execution order cancel refused: durable arm state is halted".to_string()
            }
            Some(reason) => {
                format!("execution order cancel refused: durable arm state is disarmed ({reason})")
            }
            None => "execution order cancel refused: durable arm state is not armed".to_string(),
        };
        return manual_order_cancel_response(
            StatusCode::FORBIDDEN,
            false,
            "rejected",
            order_id,
            None,
            vec![blocker],
        );
    }

    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => {
            return manual_order_cancel_response(
                StatusCode::SERVICE_UNAVAILABLE,
                false,
                "unavailable",
                order_id,
                None,
                vec![err.to_string()],
            );
        }
    };

    let Some(active_run_id) = status.active_run_id else {
        return manual_order_cancel_response(
            StatusCode::CONFLICT,
            false,
            "unavailable",
            order_id,
            None,
            vec!["execution order cancel refused: no active durable run is available".to_string()],
        );
    };

    if status.state != "running" {
        let mut blockers = vec![format!(
            "execution order cancel refused: runtime state '{}' is not accepting operator cancel actions",
            status.state
        )];
        if let Some(note) = status.notes {
            blockers.push(note);
        }
        return manual_order_cancel_response(
            StatusCode::CONFLICT,
            false,
            "unavailable",
            order_id,
            Some(active_run_id),
            blockers,
        );
    }

    let execution_snapshot = match st.execution_snapshot.read().await.clone() {
        Some(snapshot) => snapshot,
        None => {
            return manual_order_cancel_response(
                StatusCode::SERVICE_UNAVAILABLE,
                false,
                "unavailable",
                order_id,
                Some(active_run_id),
                vec![
                    "execution order cancel unavailable: no execution snapshot is available"
                        .to_string(),
                ],
            );
        }
    };

    let broker_map = match mqk_db::broker_map_load(db).await {
        Ok(rows) => rows,
        Err(err) => {
            return manual_order_cancel_response(
                StatusCode::SERVICE_UNAVAILABLE,
                false,
                "unavailable",
                order_id,
                Some(active_run_id),
                vec![format!(
                    "execution order cancel unavailable: durable broker-order-map truth could not be loaded: {err}"
                )],
            );
        }
    };

    if !broker_map
        .iter()
        .any(|(internal_id, _broker_id)| internal_id == &order_id)
    {
        return manual_order_cancel_response(
            StatusCode::CONFLICT,
            false,
            "rejected",
            order_id.clone(),
            Some(active_run_id),
            vec![format!(
                "execution order cancel refused: order_id '{}' is unknown or not durably targetable",
                order_id
            )],
        );
    }

    let Some(order) = execution_snapshot
        .active_orders
        .iter()
        .find(|row| row.order_id == order_id)
    else {
        return manual_order_cancel_response(
            StatusCode::CONFLICT,
            false,
            "rejected",
            order_id.clone(),
            Some(active_run_id),
            vec![format!(
                "execution order cancel refused: order_id '{}' is not present in the active execution snapshot",
                order_id
            )],
        );
    };

    match order.status.as_str() {
        "Open" | "PartiallyFilled" => {}
        "CancelPending" => {
            return manual_order_cancel_response(
                StatusCode::OK,
                false,
                "duplicate",
                order_id.clone(),
                Some(active_run_id),
                vec![format!(
                    "execution order cancel for '{}' is already in flight",
                    order_id
                )],
            );
        }
        other => {
            return manual_order_cancel_response(
                StatusCode::CONFLICT,
                false,
                "rejected",
                order_id.clone(),
                Some(active_run_id),
                vec![format!(
                    "execution order cancel refused: order_id '{}' is not cancelable from status '{}'",
                    order_id, other
                )],
            );
        }
    }

    let cancel_json = serde_json::json!({
        "request_type": "cancel",
        "cancel_request_id": cancel_request_id.clone(),
        "target_order_id": order_id.clone(),
    });

    match mqk_db::outbox_enqueue(db, active_run_id, &cancel_request_id, cancel_json).await {
        Ok(true) => manual_order_cancel_response(
            StatusCode::OK,
            true,
            "enqueued",
            order_id,
            Some(active_run_id),
            vec![],
        ),
        Ok(false) => match mqk_db::outbox_fetch_by_idempotency_key(db, &cancel_request_id).await {
            Ok(Some(existing)) => {
                let Some(existing_target_order_id) = existing
                    .order_json
                    .get("target_order_id")
                    .and_then(|value| value.as_str())
                else {
                    return manual_order_cancel_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        false,
                        "unavailable",
                        order_id,
                        Some(active_run_id),
                        vec![format!(
                            "execution order cancel unavailable: cancel_request_id '{}' collided with an existing outbox row that is missing durable target_order_id truth",
                            cancel_request_id
                        )],
                    );
                };

                if existing_target_order_id == order_id.as_str() {
                    manual_order_cancel_response(
                        StatusCode::OK,
                        false,
                        "duplicate",
                        order_id,
                        Some(active_run_id),
                        vec![format!(
                            "cancel request '{}' already exists for order_id '{}'; no new outbox row was created",
                            cancel_request_id, existing_target_order_id
                        )],
                    )
                } else {
                    manual_order_cancel_response(
                        StatusCode::CONFLICT,
                        false,
                        "rejected",
                        order_id.clone(),
                        Some(active_run_id),
                        vec![format!(
                            "execution order cancel refused: cancel_request_id '{}' is already bound to different order_id '{}' and cannot be reused for order_id '{}'",
                            cancel_request_id, existing_target_order_id, order_id
                        )],
                    )
                }
            }
            Ok(None) => manual_order_cancel_response(
                StatusCode::SERVICE_UNAVAILABLE,
                false,
                "unavailable",
                order_id,
                Some(active_run_id),
                vec![format!(
                    "execution order cancel unavailable: cancel_request_id '{}' collided with an existing outbox key but the durable outbox row could not be loaded",
                    cancel_request_id
                )],
            ),
            Err(err) => manual_order_cancel_response(
                StatusCode::SERVICE_UNAVAILABLE,
                false,
                "unavailable",
                order_id,
                Some(active_run_id),
                vec![format!(
                    "execution order cancel unavailable: duplicate-target truth could not be loaded for cancel_request_id '{}': {}",
                    cancel_request_id, err
                )],
            ),
        },
        Err(err) => manual_order_cancel_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            false,
            "unavailable",
            order_id,
            Some(active_run_id),
            vec![format!("outbox enqueue failed: {err}")],
        ),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/fill-quality
// ---------------------------------------------------------------------------

pub(crate) async fn execution_fill_quality(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    const CANONICAL: &str = "/api/v1/execution/fill-quality";

    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(FillQualityTelemetryResponse {
                canonical_route: CANONICAL.to_string(),
                truth_state: "no_db".to_string(),
                backend: "unavailable".to_string(),
                rows: vec![],
            }),
        )
            .into_response();
    };

    // Derive active run_id from the durable status snapshot.
    let active_run_id = match st.current_status_snapshot().await {
        Ok(snap) => snap.active_run_id,
        Err(_) => None,
    };

    let Some(run_id) = active_run_id else {
        return (
            StatusCode::OK,
            Json(FillQualityTelemetryResponse {
                canonical_route: CANONICAL.to_string(),
                truth_state: "no_active_run".to_string(),
                backend: "unavailable".to_string(),
                rows: vec![],
            }),
        )
            .into_response();
    };

    let rows = match mqk_db::fetch_fill_quality_telemetry_recent(db, run_id, 100).await {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "fill_quality_fetch_failed",
                    "detail": e.to_string(),
                })),
            )
                .into_response();
        }
    };

    let api_rows: Vec<FillQualityTelemetryRow> = rows
        .into_iter()
        .map(|r| FillQualityTelemetryRow {
            telemetry_id: r.telemetry_id,
            run_id: r.run_id,
            internal_order_id: r.internal_order_id,
            broker_order_id: r.broker_order_id,
            broker_fill_id: r.broker_fill_id,
            broker_message_id: r.broker_message_id,
            symbol: r.symbol,
            side: r.side,
            ordered_qty: r.ordered_qty,
            fill_qty: r.fill_qty,
            fill_price_micros: r.fill_price_micros,
            reference_price_micros: r.reference_price_micros,
            slippage_bps: r.slippage_bps,
            submit_ts_utc: r.submit_ts_utc.map(|t| t.to_rfc3339()),
            fill_received_at_utc: r.fill_received_at_utc.to_rfc3339(),
            submit_to_fill_ms: r.submit_to_fill_ms,
            fill_kind: r.fill_kind,
            provenance_ref: r.provenance_ref,
            created_at_utc: r.created_at_utc.to_rfc3339(),
        })
        .collect();

    (
        StatusCode::OK,
        Json(FillQualityTelemetryResponse {
            canonical_route: CANONICAL.to_string(),
            truth_state: "active".to_string(),
            backend: "postgres.fill_quality_telemetry".to_string(),
            rows: api_rows,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ValidatedManualOrderSubmit {
    client_request_id: String,
    symbol: String,
    side: String,
    qty: i64,
    order_type: String,
    time_in_force: String,
    limit_price: Option<i64>,
}

impl ValidatedManualOrderSubmit {
    fn order_json(&self) -> serde_json::Value {
        serde_json::json!({
            "symbol": self.symbol,
            "side": self.side,
            "qty": self.qty,
            "order_type": self.order_type,
            "time_in_force": self.time_in_force,
            "limit_price": self.limit_price,
        })
    }
}

fn validate_manual_order_submit(
    body: ManualOrderSubmitRequest,
) -> Result<ValidatedManualOrderSubmit, (String, Vec<String>)> {
    let client_request_id = body.client_request_id.trim().to_string();
    let mut blockers = Vec::new();

    if client_request_id.is_empty() {
        blockers.push("client_request_id is required".to_string());
    }

    let symbol = body.symbol.trim().to_string();
    if symbol.is_empty() {
        blockers.push("symbol must not be blank".to_string());
    }

    let side = body.side.trim().to_ascii_lowercase();
    if !matches!(side.as_str(), "buy" | "sell") {
        blockers.push("side must be one of: buy, sell".to_string());
    }

    let qty = match parse_integer_field("qty", &body.qty) {
        Ok(value) => {
            if value <= 0 {
                blockers.push("qty must be positive".to_string());
                None
            } else if value > i32::MAX as i64 {
                blockers.push("qty is out of range for broker request".to_string());
                None
            } else {
                Some(value)
            }
        }
        Err(err) => {
            blockers.push(err);
            None
        }
    };

    let order_type = body
        .order_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("market")
        .to_ascii_lowercase();
    if !matches!(order_type.as_str(), "market" | "limit") {
        blockers.push("order_type must be one of: market, limit".to_string());
    }

    let time_in_force = body
        .time_in_force
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("day")
        .to_ascii_lowercase();
    if !matches!(
        time_in_force.as_str(),
        "day" | "gtc" | "ioc" | "fok" | "opg" | "cls"
    ) {
        blockers.push("time_in_force must be one of: day, gtc, ioc, fok, opg, cls".to_string());
    }

    let limit_price = match body.limit_price.as_ref() {
        Some(value) => match parse_integer_field("limit_price", value) {
            Ok(parsed) => {
                if parsed <= 0 {
                    blockers.push("limit_price must be positive".to_string());
                    None
                } else {
                    Some(parsed)
                }
            }
            Err(err) => {
                blockers.push(err);
                None
            }
        },
        None => None,
    };

    match order_type.as_str() {
        "market" if body.limit_price.is_some() => {
            blockers.push("market order must not carry limit_price".to_string());
        }
        "limit" if limit_price.is_none() => {
            blockers.push("limit order must carry limit_price".to_string());
        }
        _ => {}
    }

    if !blockers.is_empty() {
        return Err((client_request_id, blockers));
    }

    Ok(ValidatedManualOrderSubmit {
        client_request_id,
        symbol,
        side,
        qty: qty.expect("validated qty"),
        order_type,
        time_in_force,
        limit_price,
    })
}

fn parse_integer_field(name: &str, value: &serde_json::Value) -> Result<i64, String> {
    match value {
        serde_json::Value::Number(number) => number
            .as_i64()
            .ok_or_else(|| format!("{name} must be an integer without lossy conversion")),
        serde_json::Value::String(raw) => raw
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{name} must be an integer without lossy conversion")),
        _ => Err(format!("{name} must be an integer-compatible value")),
    }
}

fn manual_order_submit_response(
    status: StatusCode,
    accepted: bool,
    disposition: &str,
    client_request_id: String,
    active_run_id: Option<uuid::Uuid>,
    blockers: Vec<String>,
) -> Response {
    (
        status,
        Json(ManualOrderSubmitResponse {
            accepted,
            disposition: disposition.to_string(),
            client_request_id,
            active_run_id,
            blockers,
        }),
    )
        .into_response()
}

fn validate_manual_order_cancel(body: ManualOrderCancelRequest) -> Result<String, Vec<String>> {
    let cancel_request_id = body.cancel_request_id.trim().to_string();
    let mut blockers = Vec::new();

    if cancel_request_id.is_empty() {
        blockers.push("cancel_request_id is required".to_string());
    }

    if blockers.is_empty() {
        Ok(cancel_request_id)
    } else {
        Err(blockers)
    }
}

fn manual_order_cancel_response(
    status: StatusCode,
    accepted: bool,
    disposition: &str,
    order_id: String,
    active_run_id: Option<uuid::Uuid>,
    blockers: Vec<String>,
) -> Response {
    (
        status,
        Json(ManualOrderCancelResponse {
            accepted,
            disposition: disposition.to_string(),
            order_id,
            active_run_id,
            blockers,
        }),
    )
        .into_response()
}

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
// GET /api/v1/execution/replace-cancel-chains (A4)
// ---------------------------------------------------------------------------

/// Replace/cancel chain surface — mounted but not wired.
///
/// No chain-lineage provenance exists in the current OMS implementation.
/// Returns an explicit `"not_wired"` wrapper rather than 404 so the GUI can
/// surface honest unavailable truth instead of treating the missing route as
/// a backend error.
pub(crate) async fn execution_replace_cancel_chains(_: State<Arc<AppState>>) -> impl IntoResponse {
    use crate::api_types::ReplaceCancelChainsResponse;

    (
        StatusCode::OK,
        Json(ReplaceCancelChainsResponse {
            canonical_route: "/api/v1/execution/replace-cancel-chains".to_string(),
            truth_state: "not_wired".to_string(),
            backend: "none".to_string(),
            note: "No replace/cancel chain lineage is tracked in the current OMS. \
                   Empty chains must not be interpreted as absence of historical \
                   replace or cancel operations."
                .to_string(),
            chains: vec![],
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
