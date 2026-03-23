//! OMS overview and metrics dashboard route handlers.
//!
//! Contains: oms_overview, metrics_dashboards.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};

use crate::api_types::{MetricsDashboardResponse, OmsOverviewResponse};
use crate::state::AppState;

use super::helpers::{
    exposure_breakdown, parse_decimal, position_market_value, runtime_error_response,
    runtime_status_from_state,
};

// ---------------------------------------------------------------------------
// GET /api/v1/oms/overview (CC-04)
// ---------------------------------------------------------------------------

pub(crate) async fn oms_overview(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return runtime_error_response(err),
    };
    let integrity_armed = status.integrity_armed;
    let kill_switch_active = status.state == "halted";

    let reconcile = st.current_reconcile_snapshot().await;
    let reconcile_total_mismatches = reconcile.mismatched_positions
        + reconcile.mismatched_orders
        + reconcile.mismatched_fills
        + reconcile.unmatched_broker_events;

    let fault_signal_count = {
        let mut count = 0usize;
        if status.state == "unknown" {
            count += 1;
        }
        if matches!(reconcile.status.as_str(), "dirty" | "stale" | "unavailable") {
            count += 1;
        }
        if reconcile.status == "unknown" && status.state == "running" {
            count += 1;
        }
        if kill_switch_active {
            count += 1;
        }
        count
    };

    let broker_snap = st.broker_snapshot.read().await.clone();
    let (
        account_snapshot_state,
        account_equity,
        account_cash,
        portfolio_snapshot_state,
        portfolio_snapshot_at_utc,
        position_count,
        open_order_count,
        fill_count,
    ) = match broker_snap {
        None => (
            "no_snapshot".to_string(),
            None,
            None,
            "no_snapshot".to_string(),
            None,
            0usize,
            0usize,
            0usize,
        ),
        Some(snap) => {
            let equity = snap.account.equity.parse::<f64>().ok();
            let cash = snap.account.cash.parse::<f64>().ok();
            let at = Some(snap.captured_at_utc.to_rfc3339());
            (
                "active".to_string(),
                equity,
                cash,
                "active".to_string(),
                at,
                snap.positions.len(),
                snap.orders.len(),
                snap.fills.len(),
            )
        }
    };

    let exec_snap = st.execution_snapshot.read().await.clone();
    let (execution_has_snapshot, execution_active_orders, execution_pending_orders) =
        match exec_snap {
            None => (false, 0usize, 0usize),
            Some(snap) => {
                let active = snap.active_orders.len();
                let pending = snap
                    .pending_outbox
                    .iter()
                    .filter(|o| o.status == "PENDING" || o.status == "CLAIMED")
                    .count();
                (true, active, pending)
            }
        };

    (
        StatusCode::OK,
        Json(OmsOverviewResponse {
            canonical_route: "/api/v1/oms/overview".to_string(),
            runtime_status: runtime_status_from_state(&status.state).to_string(),
            integrity_armed,
            kill_switch_active,
            daemon_mode: st.deployment_mode().as_api_label().to_string(),
            fault_signal_count,
            account_snapshot_state,
            account_equity,
            account_cash,
            portfolio_snapshot_state,
            portfolio_snapshot_at_utc,
            position_count,
            open_order_count,
            fill_count,
            execution_has_snapshot,
            execution_active_orders,
            execution_pending_orders,
            reconcile_status: reconcile.status,
            reconcile_last_run_at: reconcile.last_run_at,
            reconcile_total_mismatches,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/metrics/dashboards (CC-05)
// ---------------------------------------------------------------------------

pub(crate) async fn metrics_dashboards(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return runtime_error_response(err),
    };
    let kill_switch_active = status.state == "halted";

    let reconcile = st.current_reconcile_snapshot().await;
    let reconcile_total_mismatches = reconcile.mismatched_positions
        + reconcile.mismatched_orders
        + reconcile.mismatched_fills
        + reconcile.unmatched_broker_events;

    let broker_snap = st.broker_snapshot.read().await.clone();
    let (
        portfolio_snapshot_state,
        account_equity,
        long_market_value,
        short_market_value,
        cash,
        buying_power,
        risk_snapshot_state,
        gross_exposure,
        net_exposure,
        concentration_pct,
    ) = match broker_snap {
        None => (
            "no_snapshot".to_string(),
            None,
            None,
            None,
            None,
            None,
            "no_snapshot".to_string(),
            None,
            None,
            None,
        ),
        Some(snap) => {
            let equity = parse_decimal(&snap.account.equity);
            let cash_val = parse_decimal(&snap.account.cash);
            let (long_mv, short_mv, gross_exp, max_abs) = exposure_breakdown(&snap.positions);
            let net_exp = snap
                .positions
                .iter()
                .map(position_market_value)
                .sum::<f64>();
            let conc = if gross_exp > 0.0 {
                (max_abs / gross_exp) * 100.0
            } else {
                0.0
            };
            (
                "active".to_string(),
                Some(equity),
                Some(long_mv),
                Some(short_mv),
                Some(cash_val),
                Some(cash_val),
                "active".to_string(),
                Some(gross_exp),
                Some(net_exp),
                Some(conc),
            )
        }
    };

    let exec_snap = st.execution_snapshot.read().await.clone();
    let (
        execution_snapshot_state,
        active_order_count,
        pending_order_count,
        dispatching_order_count,
        reject_count_today,
    ) = match exec_snap {
        None => ("no_snapshot".to_string(), 0usize, 0usize, 0usize, 0usize),
        Some(snap) => {
            let active = snap.active_orders.len();
            let pending = snap
                .pending_outbox
                .iter()
                .filter(|o| o.status == "PENDING" || o.status == "CLAIMED")
                .count();
            let dispatching = snap
                .pending_outbox
                .iter()
                .filter(|o| o.status == "DISPATCHING" || o.status == "SENT")
                .count();
            ("active".to_string(), active, pending, dispatching, 0)
        }
    };

    (
        StatusCode::OK,
        Json(MetricsDashboardResponse {
            canonical_route: "/api/v1/metrics/dashboards".to_string(),
            portfolio_snapshot_state,
            account_equity,
            long_market_value,
            short_market_value,
            cash,
            daily_pnl: None,
            buying_power,
            risk_snapshot_state,
            gross_exposure,
            net_exposure,
            concentration_pct,
            drawdown_pct: None,
            loss_limit_utilization_pct: None,
            kill_switch_active,
            active_breaches: usize::from(kill_switch_active),
            execution_snapshot_state,
            active_order_count,
            pending_order_count,
            dispatching_order_count,
            reject_count_today,
            reconcile_status: reconcile.status,
            reconcile_last_run_at: reconcile.last_run_at,
            reconcile_total_mismatches,
        }),
    )
        .into_response()
}
