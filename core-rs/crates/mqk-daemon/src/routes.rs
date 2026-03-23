//! Axum router and all HTTP handlers for mqk-daemon.
//!
//! `build_router` is the single entry point; `main.rs` calls it and attaches
//! middleware layers.  All handlers are `pub(crate)` so the scenario tests in
//! `tests/` can compose the router directly.
//!
//! # Module layout
//!
//! | Module           | Contents                                              |
//! |------------------|-------------------------------------------------------|
//! | `control`        | Existing control sub-router (unchanged)               |
//! | `helpers`        | Shared pure functions used by multiple route modules  |
//! | `system`         | health, status, preflight, metadata, leadership,      |
//! |                  | session, config-fingerprint, config-diffs             |
//! | `control_plane`  | run_start/stop/halt, integrity arm/disarm, ops_action,|
//! |                  | ops_catalog, ops_mode_change_guidance                 |
//! | `execution`      | execution_summary, execution_orders, order submit/cancel |
//! | `portfolio`      | portfolio_summary/positions/orders/fills, risk        |
//! | `reconcile`      | reconcile_status, reconcile_mismatches                |
//! | `strategy`       | strategy_summary, strategy_suppressions               |
//! | `audit_ops`      | audit_operator_actions, audit_artifacts,              |
//! |                  | ops_operator_timeline                                 |
//! | `oms_metrics`    | oms_overview, metrics_dashboards                      |
//! | `trading`        | /v1/trading/*, diagnostics_snapshot, stream           |

pub(crate) mod alerts_events;
pub(crate) mod audit_ops;
pub mod control;
pub(crate) mod control_plane;
pub(crate) mod execution;
pub(crate) mod helpers;
pub(crate) mod oms_metrics;
pub(crate) mod portfolio;
pub(crate) mod reconcile;
pub(crate) mod strategy;
pub(crate) mod system;
pub(crate) mod trading;

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};

use crate::{
    api_types::GateRefusedResponse,
    state::{AppState, OperatorAuthMode},
};

// ---------------------------------------------------------------------------
// S7-1: Token auth middleware
// ---------------------------------------------------------------------------

/// Axum middleware that enforces fail-closed operator authentication.
///
/// # RT-03R — Fail-Closed Auth in Production
///
/// The daemon now distinguishes three explicit privileged-route postures:
///
/// - [`OperatorAuthMode::TokenRequired`] — a valid `Authorization: Bearer ...`
///   header is mandatory.
/// - [`OperatorAuthMode::ExplicitDevNoToken`] — a caller intentionally opted
///   into local no-token development; privileged routes are reachable.
/// - [`OperatorAuthMode::MissingTokenFailClosed`] — no trustworthy operator
///   token is configured, so privileged routes are denied explicitly.
///
/// Loopback bind is not treated as sufficient authorization for privileged
/// actions. Missing operator auth now fails closed instead of passing through.
async fn token_auth_middleware(
    State(st): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    match st.operator_auth_mode() {
        OperatorAuthMode::TokenRequired(expected_token) => {
            let provided = req
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer "));

            if provided != Some(expected_token.as_str()) {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(GateRefusedResponse {
                        error: "GATE_REFUSED: valid Bearer token required on operator routes"
                            .to_string(),
                        gate: "operator_token".to_string(),
                    }),
                )
                    .into_response();
            }

            next.run(req).await
        }
        OperatorAuthMode::ExplicitDevNoToken => next.run(req).await,
        OperatorAuthMode::MissingTokenFailClosed => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(GateRefusedResponse {
                error: "GATE_REFUSED: operator token missing; privileged routes stay disabled until MQK_OPERATOR_TOKEN is configured or explicit debug-only dev mode is selected"
                    .to_string(),
                gate: "operator_auth_config".to_string(),
            }),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the complete application router wired to the given shared state.
///
/// # Route classification (S7-1)
///
/// | Category      | Methods                              | Auth required |
/// |---------------|--------------------------------------|---------------|
/// | Telemetry     | GET /v1/health, /v1/status, /v1/stream | No          |
/// | Trading read  | GET /v1/trading/*                    | No            |
/// | Operator      | POST/DELETE /v1/run/*, /v1/integrity/*, /v1/trading/snapshot, /control/* | Yes |
///
/// Operator routes are wrapped in [`token_auth_middleware`]. Telemetry and
/// read routes are on the public sub-router — no middleware is applied.
///
/// Middleware layers (CORS, tracing) are **not** applied here; `main.rs`
/// attaches them after this call so tests can use the bare router.
pub fn build_router(state: Arc<AppState>) -> Router {
    use alerts_events::{alerts_active, events_feed};
    use audit_ops::{audit_artifacts, audit_operator_actions, ops_operator_timeline};
    use control_plane::{
        integrity_arm, integrity_disarm, ops_action, ops_catalog, ops_mode_change_guidance,
        run_halt, run_start, run_stop,
    };
    use execution::{
        execution_fill_quality, execution_order_cancel, execution_order_submit, execution_orders,
        execution_summary,
    };
    use oms_metrics::{metrics_dashboards, oms_overview};
    use portfolio::{
        portfolio_fills, portfolio_open_orders, portfolio_positions, portfolio_summary,
        risk_denials, risk_summary,
    };
    use reconcile::{reconcile_mismatches, reconcile_status};
    use strategy::{strategy_summary, strategy_suppressions};
    use system::{
        health, status_handler, system_config_diffs, system_config_fingerprint, system_metadata,
        system_preflight, system_runtime_leadership, system_session, system_status,
    };
    use trading::{
        diagnostics_snapshot, stream, trading_account, trading_fills, trading_orders,
        trading_positions, trading_snapshot, trading_snapshot_clear, trading_snapshot_set,
    };

    // --- Public (unauthenticated) routes — read-only telemetry & data. ---
    let public = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/status", get(status_handler))
        .route("/v1/stream", get(stream))
        .route("/api/v1/system/status", get(system_status))
        .route("/api/v1/system/preflight", get(system_preflight))
        .route("/api/v1/system/metadata", get(system_metadata))
        .route(
            "/api/v1/system/runtime-leadership",
            get(system_runtime_leadership),
        )
        .route("/api/v1/execution/summary", get(execution_summary))
        .route("/api/v1/execution/orders", get(execution_orders))
        .route(
            "/api/v1/execution/fill-quality",
            get(execution_fill_quality),
        )
        .route("/api/v1/portfolio/summary", get(portfolio_summary))
        .route("/api/v1/portfolio/positions", get(portfolio_positions))
        .route("/api/v1/portfolio/orders/open", get(portfolio_open_orders))
        .route("/api/v1/portfolio/fills", get(portfolio_fills))
        .route("/api/v1/risk/summary", get(risk_summary))
        .route("/api/v1/risk/denials", get(risk_denials))
        .route("/api/v1/reconcile/status", get(reconcile_status))
        .route("/api/v1/reconcile/mismatches", get(reconcile_mismatches))
        .route("/api/v1/system/session", get(system_session))
        .route(
            "/api/v1/system/config-fingerprint",
            get(system_config_fingerprint),
        )
        .route("/api/v1/system/config-diffs", get(system_config_diffs))
        .route("/api/v1/strategy/summary", get(strategy_summary))
        .route("/api/v1/strategy/suppressions", get(strategy_suppressions))
        .route(
            "/api/v1/audit/operator-actions",
            get(audit_operator_actions),
        )
        .route("/api/v1/audit/artifacts", get(audit_artifacts))
        .route("/api/v1/ops/operator-timeline", get(ops_operator_timeline))
        .route("/api/v1/ops/catalog", get(ops_catalog))
        .route(
            "/api/v1/ops/mode-change-guidance",
            get(ops_mode_change_guidance),
        )
        .route("/api/v1/alerts/active", get(alerts_active))
        .route("/api/v1/events/feed", get(events_feed))
        .route("/api/v1/oms/overview", get(oms_overview))
        .route("/api/v1/metrics/dashboards", get(metrics_dashboards))
        .route("/v1/trading/account", get(trading_account))
        .route("/v1/trading/positions", get(trading_positions))
        .route("/v1/trading/orders", get(trading_orders))
        .route("/v1/trading/fills", get(trading_fills))
        .route("/v1/trading/snapshot", get(trading_snapshot))
        .route("/v1/diagnostics/snapshot", get(diagnostics_snapshot));

    // --- Operator (authenticated) routes — mutating state changes. ---
    let operator = Router::new()
        .route("/v1/run/start", post(run_start))
        .route("/v1/run/stop", post(run_stop))
        .route("/v1/run/halt", post(run_halt))
        .route("/v1/integrity/arm", post(integrity_arm))
        .route("/v1/integrity/disarm", post(integrity_disarm))
        .route("/api/v1/execution/orders", post(execution_order_submit))
        .route(
            "/api/v1/execution/orders/:order_id/cancel",
            post(execution_order_cancel),
        )
        .route("/api/v1/ops/action", post(ops_action))
        .route("/v1/trading/snapshot", post(trading_snapshot_set))
        .route(
            "/v1/trading/snapshot",
            axum::routing::delete(trading_snapshot_clear),
        )
        .merge(control::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            token_auth_middleware,
        ));

    Router::new()
        .merge(public)
        .merge(operator)
        .with_state(state)
}
