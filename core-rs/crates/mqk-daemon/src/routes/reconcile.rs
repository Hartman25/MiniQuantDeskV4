//! Reconcile route handlers.
//!
//! Contains: reconcile_status, reconcile_mismatches, reconcile_diff_rows,
//! reconcile_order_symbol.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};

use crate::api_types::{
    ReconcileMismatchRow, ReconcileMismatchesResponse, ReconcileSummaryResponse,
};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// GET /api/v1/reconcile/status
// ---------------------------------------------------------------------------

pub(crate) async fn reconcile_status(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let reconcile = st.current_reconcile_snapshot().await;

    // RECON-06: disambiguate "unknown" (never-run) from an active result.
    // "unknown" is the initial state set by initial_reconcile_status() — it
    // means the reconcile loop has not yet completed a tick, NOT that an error
    // occurred.  We surface this as truth_state="never_run" so operators can
    // distinguish a fresh daemon from a daemon that has reconciled and found issues.
    let truth_state = match reconcile.status.as_str() {
        "unknown" => "never_run",
        "stale" => "stale",
        _ => "active",
    };

    (
        StatusCode::OK,
        Json(ReconcileSummaryResponse {
            truth_state: truth_state.to_string(),
            status: reconcile.status,
            last_run_at: reconcile.last_run_at,
            snapshot_watermark_ms: reconcile.snapshot_watermark_ms,
            mismatched_positions: reconcile.mismatched_positions,
            mismatched_orders: reconcile.mismatched_orders,
            mismatched_fills: reconcile.mismatched_fills,
            unmatched_broker_events: reconcile.unmatched_broker_events,
        }),
    )
}

// ---------------------------------------------------------------------------
// GET /api/v1/reconcile/mismatches
// ---------------------------------------------------------------------------

pub(crate) async fn reconcile_mismatches(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let reconcile = st.current_reconcile_snapshot().await;
    match reconcile.status.as_str() {
        "unknown" => {
            // RECON-06: "unknown" = never_run. Not a mismatch surface — operator
            // must wait for the reconcile loop to complete a tick.
            return (
                StatusCode::OK,
                Json(ReconcileMismatchesResponse {
                    truth_state: "never_run".to_string(),
                    snapshot_at_utc: None,
                    rows: vec![],
                    review_workflow: None,
                }),
            )
                .into_response();
        }
        "stale" => {
            return (
                StatusCode::OK,
                Json(ReconcileMismatchesResponse {
                    truth_state: "stale".to_string(),
                    snapshot_at_utc: reconcile.last_run_at,
                    rows: vec![],
                    review_workflow: None,
                }),
            )
                .into_response();
        }
        _ => {}
    }

    let Some(execution_snapshot) = st.current_execution_snapshot().await else {
        return (
            StatusCode::OK,
            Json(ReconcileMismatchesResponse {
                truth_state: "no_snapshot".to_string(),
                snapshot_at_utc: None,
                rows: vec![],
                review_workflow: None,
            }),
        )
            .into_response();
    };

    let Some(schema_snapshot) = st.current_broker_snapshot().await else {
        return (
            StatusCode::OK,
            Json(ReconcileMismatchesResponse {
                truth_state: "no_snapshot".to_string(),
                snapshot_at_utc: None,
                rows: vec![],
                review_workflow: None,
            }),
        )
            .into_response();
    };

    let sides = st.current_local_order_sides().await;
    let local =
        crate::state::reconcile_local_snapshot_from_runtime_with_sides(&execution_snapshot, &sides);
    let Ok(broker) = crate::state::reconcile_broker_snapshot_from_schema(&schema_snapshot) else {
        return (
            StatusCode::OK,
            Json(ReconcileMismatchesResponse {
                truth_state: "no_snapshot".to_string(),
                snapshot_at_utc: None,
                rows: vec![],
                review_workflow: None,
            }),
        )
            .into_response();
    };

    let report = mqk_reconcile::reconcile(&local, &broker);
    let expected_clean = reconcile.status == "ok";
    if expected_clean != report.is_clean() {
        return (
            StatusCode::OK,
            Json(ReconcileMismatchesResponse {
                truth_state: "stale".to_string(),
                snapshot_at_utc: Some(schema_snapshot.captured_at_utc.to_rfc3339()),
                rows: vec![],
                review_workflow: None,
            }),
        )
            .into_response();
    }

    let rows = reconcile_diff_rows(&report, &local, &broker);
    // RECON-06: review_workflow is Some only when mismatches are present and
    // truth is active. The guidance is explicit about severity and action.
    let review_workflow = if rows.is_empty() {
        None
    } else {
        let has_critical = rows.iter().any(|r| r.status == "critical");
        Some(if has_critical {
            "Critical mismatches detected. Compare internal_value (local OMS) vs \
             broker_value (broker snapshot) for each row. Position qty mismatches \
             are critical: halt execution and reconcile manually before resuming. \
             Use /v1/run/halt to stop the execution loop immediately."
                .to_string()
        } else {
            "Warning mismatches detected. Compare internal_value (local OMS) vs \
             broker_value (broker snapshot) for each row. Order field drift is \
             warning-severity: investigate before the next execution cycle. \
             Halt execution if drift is unexplained."
                .to_string()
        })
    };

    (
        StatusCode::OK,
        Json(ReconcileMismatchesResponse {
            truth_state: "active".to_string(),
            snapshot_at_utc: Some(schema_snapshot.captured_at_utc.to_rfc3339()),
            rows,
            review_workflow,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

fn reconcile_diff_rows(
    report: &mqk_reconcile::ReconcileReport,
    local: &mqk_reconcile::LocalSnapshot,
    broker: &mqk_reconcile::BrokerSnapshot,
) -> Vec<ReconcileMismatchRow> {
    report
        .diffs
        .iter()
        .map(|diff| match diff {
            mqk_reconcile::ReconcileDiff::PositionQtyMismatch {
                symbol,
                local_qty,
                broker_qty,
            } => ReconcileMismatchRow {
                id: format!("position:{symbol}"),
                domain: "position".to_string(),
                symbol: symbol.clone(),
                internal_value: format!("qty={local_qty}"),
                broker_value: format!("qty={broker_qty}"),
                status: "critical".to_string(),
                note: "Position quantity mismatch detected during reconcile.".to_string(),
            },
            mqk_reconcile::ReconcileDiff::OrderMismatch {
                order_id,
                field,
                local: local_value,
                broker: broker_value,
            } => ReconcileMismatchRow {
                id: format!("order:{order_id}:{field}"),
                domain: "order".to_string(),
                symbol: reconcile_order_symbol(local, broker, order_id),
                internal_value: format!("{field}={local_value}"),
                broker_value: format!("{field}={broker_value}"),
                status: "warning".to_string(),
                note: "Order field drift detected during reconcile.".to_string(),
            },
            mqk_reconcile::ReconcileDiff::UnknownBrokerFill {
                order_id,
                filled_qty,
            } => ReconcileMismatchRow {
                id: format!("fill:{order_id}"),
                domain: "fill".to_string(),
                symbol: reconcile_order_symbol(local, broker, order_id),
                internal_value: "missing_local_order".to_string(),
                broker_value: format!("filled_qty={filled_qty}"),
                status: "critical".to_string(),
                note: "Broker reports a fill for an order absent from local OMS.".to_string(),
            },
            mqk_reconcile::ReconcileDiff::UnknownOrder { order_id } => ReconcileMismatchRow {
                id: format!("order:{order_id}:unknown"),
                domain: "order".to_string(),
                symbol: reconcile_order_symbol(local, broker, order_id),
                internal_value: "missing_local_order".to_string(),
                broker_value: "present_at_broker".to_string(),
                status: "warning".to_string(),
                note: "Broker reports an open order absent from local OMS.".to_string(),
            },
            mqk_reconcile::ReconcileDiff::LocalOrderMissingAtBroker { order_id } => {
                ReconcileMismatchRow {
                    id: format!("order:{order_id}:missing_at_broker"),
                    domain: "order".to_string(),
                    symbol: reconcile_order_symbol(local, broker, order_id),
                    internal_value: "present_locally".to_string(),
                    broker_value: "missing_at_broker".to_string(),
                    status: "warning".to_string(),
                    note: "Local active order is absent from the broker snapshot.".to_string(),
                }
            }
        })
        .collect()
}

fn reconcile_order_symbol(
    local: &mqk_reconcile::LocalSnapshot,
    broker: &mqk_reconcile::BrokerSnapshot,
    order_id: &str,
) -> String {
    local
        .orders
        .get(order_id)
        .map(|order| order.symbol.clone())
        .or_else(|| {
            broker
                .orders
                .get(order_id)
                .map(|order| order.symbol.clone())
        })
        .unwrap_or_else(|| "—".to_string())
}
