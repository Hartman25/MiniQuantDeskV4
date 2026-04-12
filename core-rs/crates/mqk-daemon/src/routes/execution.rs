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
    ExecutionOrderRow, ExecutionSummaryResponse, FillQualityTelemetryResponse,
    FillQualityTelemetryRow, ManualOrderCancelRequest, ManualOrderCancelResponse,
    ManualOrderSubmitRequest, ManualOrderSubmitResponse,
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
