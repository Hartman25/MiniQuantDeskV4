//! Axum router and all HTTP handlers for mqk-daemon.
//!
//! `build_router` is the single entry point; `main.rs` calls it and attaches
//! middleware layers.  All handlers are `pub(crate)` so the scenario tests in
//! `tests/` can compose the router directly.

pub mod control;

use std::{convert::Infallible, sync::Arc};

use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    middleware::Next,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures_util::{Stream, StreamExt};
use mqk_schemas::BrokerPosition;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::info;

use crate::{
    api_types::{
        DiagnosticsSnapshotResponse, ExecutionSummaryResponse, GateRefusedResponse, HealthResponse,
        IntegrityResponse, PortfolioSummaryResponse, PreflightStatusResponse,
        ReconcileSummaryResponse, RiskSummaryResponse, SystemStatusResponse,
        TradingAccountResponse, TradingFillsResponse, TradingOrdersResponse,
        TradingPositionsResponse, TradingSnapshotResponse,
    },
    state::{AppState, BusMsg, OperatorAuthMode, RuntimeLifecycleError},
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
    // --- Public (unauthenticated) routes — read-only telemetry & data. ---
    let public = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/status", get(status_handler))
        .route("/v1/stream", get(stream))
        .route("/api/v1/system/status", get(system_status))
        .route("/api/v1/system/preflight", get(system_preflight))
        .route("/api/v1/execution/summary", get(execution_summary))
        .route("/api/v1/portfolio/summary", get(portfolio_summary))
        .route("/api/v1/risk/summary", get(risk_summary))
        .route("/api/v1/reconcile/status", get(reconcile_status))
        // DAEMON-1: trading read APIs (placeholder until broker wiring exists)
        .route("/v1/trading/account", get(trading_account))
        .route("/v1/trading/positions", get(trading_positions))
        .route("/v1/trading/orders", get(trading_orders))
        .route("/v1/trading/fills", get(trading_fills))
        // DAEMON-2: read-back of current snapshot (no auth — read-only)
        .route("/v1/trading/snapshot", get(trading_snapshot))
        // B4: execution diagnostics snapshot (no auth — read-only)
        .route("/v1/diagnostics/snapshot", get(diagnostics_snapshot));

    // --- Operator (authenticated) routes — mutating state changes. ---
    let operator = Router::new()
        .route("/v1/run/start", post(run_start))
        .route("/v1/run/stop", post(run_stop))
        .route("/v1/run/halt", post(run_halt))
        .route("/v1/integrity/arm", post(integrity_arm))
        .route("/v1/integrity/disarm", post(integrity_disarm))
        // DAEMON-2: dev-only snapshot inject/clear
        .route("/v1/trading/snapshot", post(trading_snapshot_set))
        .route(
            "/v1/trading/snapshot",
            axum::routing::delete(trading_snapshot_clear),
        )
        .merge(control::router())
        // RT-03R: apply auth consistently to every privileged route, including
        // /control/* surfaces.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            token_auth_middleware,
        ));

    Router::new()
        .merge(public)
        .merge(operator)
        .with_state(state)
}

// ---------------------------------------------------------------------------
// GET /v1/health
// ---------------------------------------------------------------------------

pub(crate) async fn health(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(HealthResponse {
            ok: true,
            service: st.build.service,
            version: st.build.version,
        }),
    )
}

// ---------------------------------------------------------------------------
// GET /v1/status
// ---------------------------------------------------------------------------

pub(crate) async fn status_handler(State(st): State<Arc<AppState>>) -> Response {
    match st.current_status_snapshot().await {
        Ok(snapshot) => (StatusCode::OK, Json(snapshot)).into_response(),
        Err(err) => runtime_error_response(err),
    }
}

// ---------------------------------------------------------------------------
// POST /v1/run/start
// ---------------------------------------------------------------------------

/// Start a live run.
///
/// # Gate (Patch L1)
/// Returns `403 Forbidden` if the integrity engine is disarmed or halted.
/// Execution cannot be started when system integrity is not armed.
/// This mirrors the `BrokerGateway` gate check at the control-plane level.
pub(crate) async fn run_start(State(st): State<Arc<AppState>>) -> Response {
    match st.start_execution_runtime().await {
        Ok(snapshot) => {
            info!(run_id = ?snapshot.active_run_id, "run/start");
            (StatusCode::OK, Json(snapshot)).into_response()
        }
        Err(err) => runtime_error_response(err),
    }
}

// ---------------------------------------------------------------------------
// POST /v1/run/stop
// ---------------------------------------------------------------------------

pub(crate) async fn run_stop(State(st): State<Arc<AppState>>) -> Response {
    match st.stop_execution_runtime().await {
        Ok(snapshot) => {
            info!("run/stop");
            (StatusCode::OK, Json(snapshot)).into_response()
        }
        Err(err) => runtime_error_response(err),
    }
}

// ---------------------------------------------------------------------------
// POST /v1/run/halt
// ---------------------------------------------------------------------------

pub(crate) async fn run_halt(State(st): State<Arc<AppState>>) -> Response {
    match st.halt_execution_runtime().await {
        Ok(snapshot) => {
            info!("run/halt");
            (StatusCode::OK, Json(snapshot)).into_response()
        }
        Err(err) => runtime_error_response(err),
    }
}

// ---------------------------------------------------------------------------
// POST /v1/integrity/arm
// ---------------------------------------------------------------------------

pub(crate) async fn integrity_arm(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    if let Some(db) = st.db.as_ref() {
        if let Err(err) = mqk_db::persist_arm_state(db, "ARMED", None).await {
            return runtime_error_response(RuntimeLifecycleError::Internal(format!(
                "integrity/arm persist_arm_state failed: {err}"
            )));
        }
    }

    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return runtime_error_response(err),
    };

    info!("integrity/arm");
    let _ = st.bus.send(BusMsg::LogLine {
        level: "INFO".to_string(),
        msg: "integrity armed".to_string(),
    });

    (
        StatusCode::OK,
        Json(IntegrityResponse {
            armed: true,
            active_run_id: status.active_run_id,
            state: status.state,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /v1/integrity/disarm
// ---------------------------------------------------------------------------

pub(crate) async fn integrity_disarm(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = true;
    }

    if let Some(db) = st.db.as_ref() {
        if let Err(err) = mqk_db::persist_arm_state(db, "DISARMED", Some("OperatorDisarm")).await {
            return runtime_error_response(RuntimeLifecycleError::Internal(format!(
                "integrity/disarm persist_arm_state failed: {err}"
            )));
        }
    }

    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return runtime_error_response(err),
    };

    info!("integrity/disarm");
    let _ = st.bus.send(BusMsg::LogLine {
        level: "WARN".to_string(),
        msg: "integrity DISARMED".to_string(),
    });

    (
        StatusCode::OK,
        Json(IntegrityResponse {
            armed: false,
            active_run_id: status.active_run_id,
            state: status.state,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// /api/v1 summary spine — GUI alignment patch
// ---------------------------------------------------------------------------

fn runtime_error_response(err: RuntimeLifecycleError) -> Response {
    match err {
        RuntimeLifecycleError::Forbidden { gate, message } => (
            StatusCode::FORBIDDEN,
            Json(GateRefusedResponse {
                error: message,
                gate,
            }),
        )
            .into_response(),
        RuntimeLifecycleError::ServiceUnavailable(message) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": message })),
        )
            .into_response(),
        RuntimeLifecycleError::Conflict(message) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": message })),
        )
            .into_response(),
        RuntimeLifecycleError::Internal(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": message })),
        )
            .into_response(),
    }
}

fn parse_decimal(value: &str) -> f64 {
    value.parse::<f64>().unwrap_or(0.0)
}

fn runtime_status_from_state(state: &str) -> &'static str {
    match state {
        "idle" => "idle",
        "running" => "running",
        "halted" => "halted",
        "unknown" => "unknown",
        _ => "degraded",
    }
}

fn is_terminal_order_status(status: &str) -> bool {
    matches!(
        status,
        "filled" | "cancelled" | "canceled" | "rejected" | "expired" | "done_for_day"
    )
}

fn is_pending_order_status(status: &str) -> bool {
    matches!(status, "new" | "pending" | "accepted")
}

fn is_dispatching_order_status(status: &str) -> bool {
    status.contains("submit")
}

fn position_market_value(position: &BrokerPosition) -> f64 {
    parse_decimal(&position.qty) * parse_decimal(&position.avg_price)
}

fn exposure_breakdown(positions: &[BrokerPosition]) -> (f64, f64, f64, f64) {
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

pub(crate) async fn system_status(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return runtime_error_response(err),
    };
    let reconcile = st.current_reconcile_snapshot().await;
    let snapshot_present = st.broker_snapshot.read().await.is_some();
    let integrity_armed = status.integrity_armed;
    let risk_blocked = if let Some(db) = st.db.as_ref() {
        mqk_db::load_risk_block_state(db)
            .await
            .ok()
            .flatten()
            .is_some_and(|risk| risk.blocked)
    } else {
        false
    };

    let runtime_status = runtime_status_from_state(&status.state).to_string();
    let broker_status = if snapshot_present { "ok" } else { "warning" }.to_string();
    let integrity_status = if integrity_armed { "ok" } else { "warning" }.to_string();
    let reconcile_status = reconcile.status.clone();
    let has_critical = matches!(reconcile_status.as_str(), "dirty" | "stale");
    let has_warning = broker_status != "ok"
        || integrity_status != "ok"
        || reconcile_status != "ok"
        || status.notes.is_some()
        || reconcile.note.is_some();

    (
        StatusCode::OK,
        Json(SystemStatusResponse {
            environment: "paper".to_string(),
            runtime_status,
            broker_status,
            db_status: "unknown".to_string(),
            market_data_health: "unknown".to_string(),
            reconcile_status,
            integrity_status,
            audit_writer_status: "unknown".to_string(),
            last_heartbeat: status.deadman_last_heartbeat_utc.clone(),
            deadman_status: status.deadman_status.clone(),
            loop_latency_ms: None,
            active_account_id: None,
            config_profile: None,
            has_warning,
            has_critical,
            strategy_armed: integrity_armed,
            execution_armed: integrity_armed,
            live_routing_enabled: false,
            kill_switch_active: status.state == "halted",
            risk_halt_active: risk_blocked,
            integrity_halt_active: !integrity_armed,
            daemon_reachable: true,
        }),
    )
        .into_response()
}

pub(crate) async fn system_preflight(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return runtime_error_response(err),
    };
    let integrity_armed = {
        let ig = st.integrity.read().await;
        !ig.is_execution_blocked()
    };

    let strategy_disarmed = !integrity_armed;
    let execution_disarmed = !integrity_armed;

    let mut warnings = vec![
        "Preflight is partially derived from in-memory daemon state; DB, broker config, market data config, and audit writer readiness are not wired yet.".to_string(),
    ];
    if status.notes.is_some() {
        warnings.push(
            "Daemon status contains placeholder notes; runtime wiring is incomplete.".to_string(),
        );
    }

    let mut blockers = vec![
        "DB reachability is unproven by daemon state.".to_string(),
        "Broker config presence is unproven by daemon state.".to_string(),
        "Market data config presence is unproven by daemon state.".to_string(),
        "Audit writer readiness is unproven by daemon state.".to_string(),
    ];
    if execution_disarmed {
        blockers.push("Execution is disarmed at the integrity gate.".to_string());
    }

    (
        StatusCode::OK,
        Json(PreflightStatusResponse {
            daemon_reachable: true,
            db_reachable: false,
            broker_config_present: false,
            market_data_config_present: false,
            audit_writer_ready: false,
            runtime_idle: status.state != "running",
            strategy_disarmed,
            execution_disarmed,
            live_routing_disabled: true,
            warnings,
            blockers,
        }),
    )
        .into_response()
}

pub(crate) async fn execution_summary(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    let now = chrono::Utc::now();

    let summary = if let Some(snapshot) = snap {
        let active_orders = snapshot
            .orders
            .iter()
            .filter(|order| !is_terminal_order_status(order.status.as_str()))
            .count();
        let pending_orders = snapshot
            .orders
            .iter()
            .filter(|order| is_pending_order_status(order.status.as_str()))
            .count();
        let dispatching_orders = snapshot
            .orders
            .iter()
            .filter(|order| is_dispatching_order_status(order.status.as_str()))
            .count();
        let reject_count_today = snapshot
            .orders
            .iter()
            .filter(|order| order.status.eq_ignore_ascii_case("rejected"))
            .count();
        let stuck_orders = snapshot
            .orders
            .iter()
            .filter(|order| {
                !is_terminal_order_status(order.status.as_str())
                    && (now - order.created_at_utc).num_minutes() >= 5
            })
            .count();

        ExecutionSummaryResponse {
            has_snapshot: true,
            active_orders,
            pending_orders,
            dispatching_orders,
            reject_count_today,
            cancel_replace_count_today: None,
            avg_ack_latency_ms: None,
            stuck_orders,
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

pub(crate) async fn portfolio_summary(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();

    let summary = if let Some(snapshot) = snap {
        let account_equity = parse_decimal(&snapshot.account.equity);
        let cash = parse_decimal(&snapshot.account.cash);
        let (long_market_value, short_market_value, _, _) = exposure_breakdown(&snapshot.positions);

        PortfolioSummaryResponse {
            has_snapshot: true,
            account_equity: Some(account_equity),
            cash: Some(cash),
            long_market_value: Some(long_market_value),
            short_market_value: Some(short_market_value),
            daily_pnl: None,
            buying_power: Some(cash),
        }
    } else {
        PortfolioSummaryResponse {
            has_snapshot: false,
            account_equity: None,
            cash: None,
            long_market_value: None,
            short_market_value: None,
            daily_pnl: None,
            buying_power: None,
        }
    };

    (StatusCode::OK, Json(summary)).into_response()
}

pub(crate) async fn risk_summary(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    let durable_risk = if let Some(db) = st.db.as_ref() {
        mqk_db::load_risk_block_state(db).await.ok().flatten()
    } else {
        None
    };
    let risk_blocked = durable_risk.as_ref().is_some_and(|state| state.blocked);

    let summary = if let Some(snapshot) = snap {
        let (_, _, gross_exposure, max_abs_position) = exposure_breakdown(&snapshot.positions);
        let net_exposure = snapshot
            .positions
            .iter()
            .map(position_market_value)
            .sum::<f64>();
        let concentration_pct = if gross_exposure > 0.0 {
            (max_abs_position / gross_exposure) * 100.0
        } else {
            0.0
        };

        RiskSummaryResponse {
            has_snapshot: true,
            gross_exposure: Some(gross_exposure),
            net_exposure: Some(net_exposure),
            concentration_pct: Some(concentration_pct),
            daily_pnl: None,
            drawdown_pct: None,
            loss_limit_utilization_pct: None,
            kill_switch_active: risk_blocked,
            active_breaches: usize::from(risk_blocked),
        }
    } else {
        RiskSummaryResponse {
            has_snapshot: false,
            gross_exposure: None,
            net_exposure: None,
            concentration_pct: None,
            daily_pnl: None,
            drawdown_pct: None,
            loss_limit_utilization_pct: None,
            kill_switch_active: risk_blocked,
            active_breaches: usize::from(risk_blocked),
        }
    };

    (StatusCode::OK, Json(summary)).into_response()
}

pub(crate) async fn reconcile_status(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let reconcile = st.current_reconcile_snapshot().await;

    (
        StatusCode::OK,
        Json(ReconcileSummaryResponse {
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
// GET /v1/trading/*  — DAEMON-1 (read-only placeholders)
// ---------------------------------------------------------------------------

pub(crate) async fn trading_account(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();

    let (has_snapshot, account) = match snap {
        Some(s) => (true, s.account),
        None => (
            false,
            mqk_schemas::BrokerAccount {
                equity: "0".to_string(),
                cash: "0".to_string(),
                currency: "USD".to_string(),
            },
        ),
    };

    (
        StatusCode::OK,
        Json(TradingAccountResponse {
            has_snapshot,
            account,
        }),
    )
}

pub(crate) async fn trading_positions(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    let (has_snapshot, positions) = match snap {
        Some(s) => (true, s.positions),
        None => (false, Vec::new()),
    };

    (
        StatusCode::OK,
        Json(TradingPositionsResponse {
            has_snapshot,
            positions,
        }),
    )
}

pub(crate) async fn trading_orders(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    let (has_snapshot, orders) = match snap {
        Some(s) => (true, s.orders),
        None => (false, Vec::new()),
    };

    (
        StatusCode::OK,
        Json(TradingOrdersResponse {
            has_snapshot,
            orders,
        }),
    )
}

pub(crate) async fn trading_fills(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    let (has_snapshot, fills) = match snap {
        Some(s) => (true, s.fills),
        None => (false, Vec::new()),
    };

    (
        StatusCode::OK,
        Json(TradingFillsResponse {
            has_snapshot,
            fills,
        }),
    )
}

pub(crate) async fn trading_snapshot(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snapshot = st.broker_snapshot.read().await.clone();

    (StatusCode::OK, Json(TradingSnapshotResponse { snapshot }))
}

// ---------------------------------------------------------------------------
// DAEMON-2: Dev-only snapshot inject/clear
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct OkResponse {
    ok: bool,
}

pub(crate) async fn trading_snapshot_set(
    State(st): State<Arc<AppState>>,
    Json(body): Json<mqk_schemas::BrokerSnapshot>,
) -> Response {
    // S7-3: compile-time disabled in release builds; runtime-gated in debug.
    if !crate::dev_gate::snapshot_inject_allowed() {
        return (
            StatusCode::FORBIDDEN,
            Json(GateRefusedResponse {
                error:
                    "GATE_REFUSED: snapshot injection disabled; set MQK_DEV_ALLOW_SNAPSHOT_INJECT=1"
                        .to_string(),
                gate: "dev_snapshot_inject".to_string(),
            }),
        )
            .into_response();
    }

    {
        let mut lock = st.broker_snapshot.write().await;
        *lock = Some(body);
    }

    let _ = st.bus.send(BusMsg::LogLine {
        level: "INFO".to_string(),
        msg: "broker snapshot injected (dev)".to_string(),
    });

    (StatusCode::OK, Json(OkResponse { ok: true })).into_response()
}

pub(crate) async fn trading_snapshot_clear(State(st): State<Arc<AppState>>) -> Response {
    // S7-3: compile-time disabled in release builds; runtime-gated in debug.
    if !crate::dev_gate::snapshot_inject_allowed() {
        return (
            StatusCode::FORBIDDEN,
            Json(GateRefusedResponse {
                error: "GATE_REFUSED: snapshot clear disabled; set MQK_DEV_ALLOW_SNAPSHOT_INJECT=1"
                    .to_string(),
                gate: "dev_snapshot_inject".to_string(),
            }),
        )
            .into_response();
    }

    {
        let mut lock = st.broker_snapshot.write().await;
        *lock = None;
    }

    let _ = st.bus.send(BusMsg::LogLine {
        level: "INFO".to_string(),
        msg: "broker snapshot cleared (dev)".to_string(),
    });

    (StatusCode::OK, Json(OkResponse { ok: true })).into_response()
}

// ---------------------------------------------------------------------------
// GET /v1/diagnostics/snapshot  (B4)
// ---------------------------------------------------------------------------

/// Return the latest execution pipeline snapshot.
///
/// This is a read-only public endpoint — no auth required.  Returns the
/// most-recent snapshot written by the execution loop, or `{ "snapshot": null }`
/// if no loop is running or no tick has completed yet.
pub(crate) async fn diagnostics_snapshot(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snapshot = st.execution_snapshot.read().await.clone();
    (
        StatusCode::OK,
        Json(DiagnosticsSnapshotResponse { snapshot }),
    )
}

// ---------------------------------------------------------------------------
// GET /v1/stream  (SSE)
// ---------------------------------------------------------------------------

pub(crate) async fn stream(State(st): State<Arc<AppState>>) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert("Cache-Control", HeaderValue::from_static("no-cache"));
    headers.insert("Connection", HeaderValue::from_static("keep-alive"));

    let rx = st.bus.subscribe();
    let events = broadcast_to_sse(rx);

    (headers, Sse::new(events).keep_alive(KeepAlive::new())).into_response()
}

fn broadcast_to_sse(
    rx: broadcast::Receiver<BusMsg>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    BroadcastStream::new(rx).filter_map(|msg| async move {
        match msg {
            Ok(m) => {
                let event_name = match &m {
                    BusMsg::Heartbeat { .. } => "heartbeat",
                    BusMsg::Status(_) => "status",
                    BusMsg::LogLine { .. } => "log",
                };
                let data = serde_json::to_string(&m).ok()?;
                Some(Ok(Event::default().event(event_name).data(data)))
            }
            Err(_) => None, // lagged / closed
        }
    })
}
