//! Axum router and all HTTP handlers for mqk-daemon.
//!
//! `build_router` is the single entry point; `main.rs` calls it and attaches
//! middleware layers.  All handlers are `pub(crate)` so the scenario tests in
//! `tests/` can compose the router directly.

pub mod control;

use std::{convert::Infallible, sync::Arc};

use chrono::Utc;

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
use sqlx::Row;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::info;

use crate::{
    api_types::{
        ActionCatalogEntry, ActionCatalogResponse, AuditArtifactRow, AuditArtifactsResponse,
        ConfigDiffsResponse, ConfigFingerprintResponse, DiagnosticsSnapshotResponse,
        ExecutionOrderRow, ExecutionSummaryResponse, FaultSignal, GateRefusedResponse,
        HealthResponse, IntegrityResponse, OperatorActionAuditFields, OperatorActionAuditRow,
        OperatorActionResponse, OperatorActionsAuditResponse, OperatorTimelineResponse,
        OperatorTimelineRow, OpsActionRequest, PortfolioFillRow, PortfolioFillsResponse,
        PortfolioOpenOrderRow, PortfolioOpenOrdersResponse, PortfolioPositionRow,
        PortfolioPositionsResponse, PortfolioSummaryResponse, PreflightStatusResponse,
        ReconcileMismatchRow, ReconcileMismatchesResponse, ReconcileSummaryResponse, RiskDenialRow,
        RiskDenialsResponse, RiskSummaryResponse, RuntimeErrorResponse,
        RuntimeLeadershipCheckpointRow, RuntimeLeadershipResponse, SessionStateResponse,
        StrategySummaryResponse, StrategySuppressionsResponse, SystemMetadataResponse,
        SystemStatusResponse, TradingAccountResponse, TradingFillsResponse, TradingOrdersResponse,
        TradingPositionsResponse, TradingSnapshotResponse,
    },
    state::{AppState, BusMsg, OperatorAuthMode, RuntimeLifecycleError, StatusSnapshot},
};

const DAEMON_ENGINE_ID: &str = "mqk-daemon";

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
        .route("/api/v1/system/metadata", get(system_metadata))
        .route(
            "/api/v1/system/runtime-leadership",
            get(system_runtime_leadership),
        )
        .route("/api/v1/execution/summary", get(execution_summary))
        .route("/api/v1/execution/orders", get(execution_orders))
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
        // Canonical Action Catalog: state-aware availability for all supported operator actions.
        // Read-only — no auth required.  Aligned with POST /api/v1/ops/action dispatcher.
        .route("/api/v1/ops/catalog", get(ops_catalog))
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
        // Canonical operator action dispatcher (GUI primary path).
        // Dispatches arm/disarm/start/stop/halt. Returns 409 for change-system-mode
        // (not authoritative: requires controlled restart). Returns 400 for unknown keys.
        .route("/api/v1/ops/action", post(ops_action))
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
            if let Some(run_id) = snapshot.active_run_id {
                let _ = write_operator_audit_event(&st, Some(run_id), "run.start", "RUNNING").await;
            }
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
            let _ = write_operator_audit_event(&st, snapshot.active_run_id, "run.stop", "STOPPED")
                .await;
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
            let _ =
                write_operator_audit_event(&st, snapshot.active_run_id, "run.halt", "HALTED").await;
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
        if let Err(err) =
            mqk_db::persist_arm_state_canonical(db, mqk_db::ArmState::Armed, None).await
        {
            return runtime_error_response(RuntimeLifecycleError::Internal {
                fault_class: "control.persistence.integrity_arm",
                message: format!("integrity/arm persist_arm_state failed: {err}"),
            });
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
        if let Err(err) = mqk_db::persist_arm_state_canonical(
            db,
            mqk_db::ArmState::Disarmed,
            Some(mqk_db::DisarmReason::OperatorDisarm),
        )
        .await
        {
            return runtime_error_response(RuntimeLifecycleError::Internal {
                fault_class: "control.persistence.integrity_disarm",
                message: format!("integrity/disarm persist_arm_state failed: {err}"),
            });
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
// POST /api/v1/ops/action — canonical GUI action dispatcher
// ---------------------------------------------------------------------------

/// Canonical operator action dispatcher.
///
/// The GUI's `invokeOperatorAction` calls this path first. Accepted action_keys:
/// - `arm-execution` / `arm-strategy`     → arm integrity gate
/// - `disarm-execution` / `disarm-strategy` → disarm integrity gate
/// - `start-system`                        → start execution runtime
/// - `stop-system`                         → stop execution runtime
/// - `kill-switch`                         → halt execution runtime
/// - `change-system-mode`                  → 409: not authoritative (restart required)
/// - anything else                         → 400: unknown action
pub(crate) async fn ops_action(
    State(st): State<Arc<AppState>>,
    Json(body): Json<OpsActionRequest>,
) -> Response {
    let action_key = body.action_key.as_str();

    match action_key {
        "arm-execution" | "arm-strategy" => {
            {
                let mut ig = st.integrity.write().await;
                ig.disarmed = false;
                ig.halted = false;
            }
            if let Some(db) = st.db.as_ref() {
                if let Err(err) =
                    mqk_db::persist_arm_state_canonical(db, mqk_db::ArmState::Armed, None).await
                {
                    return runtime_error_response(RuntimeLifecycleError::Internal {
                        fault_class: "ops.action.arm",
                        message: format!("ops/action arm persist failed: {err}"),
                    });
                }
            }
            info!("ops/action arm");
            let _ = st.bus.send(BusMsg::LogLine {
                level: "INFO".to_string(),
                msg: "ops/action: integrity armed".to_string(),
            });
            (
                StatusCode::OK,
                Json(OperatorActionResponse {
                    requested_action: "control.arm".to_string(),
                    accepted: true,
                    disposition: "applied".to_string(),
                    resulting_integrity_state: Some("ARMED".to_string()),
                    resulting_desired_armed: Some(true),
                    blockers: vec![],
                    warnings: vec![],
                    environment: Some(st.deployment_mode().as_api_label().to_string()),
                    scope: Some("daemon_instance".to_string()),
                    audit: OperatorActionAuditFields {
                        durable_db_write: st.db.is_some(),
                        durable_targets: if st.db.is_some() {
                            vec!["sys_arm_state".to_string()]
                        } else {
                            vec![]
                        },
                        audit_event_id: None,
                    },
                }),
            )
                .into_response()
        }

        "disarm-execution" | "disarm-strategy" => {
            {
                let mut ig = st.integrity.write().await;
                ig.disarmed = true;
            }
            if let Some(db) = st.db.as_ref() {
                if let Err(err) = mqk_db::persist_arm_state_canonical(
                    db,
                    mqk_db::ArmState::Disarmed,
                    Some(mqk_db::DisarmReason::OperatorDisarm),
                )
                .await
                {
                    return runtime_error_response(RuntimeLifecycleError::Internal {
                        fault_class: "ops.action.disarm",
                        message: format!("ops/action disarm persist failed: {err}"),
                    });
                }
            }
            info!("ops/action disarm");
            let _ = st.bus.send(BusMsg::LogLine {
                level: "WARN".to_string(),
                msg: "ops/action: integrity DISARMED".to_string(),
            });
            (
                StatusCode::OK,
                Json(OperatorActionResponse {
                    requested_action: "control.disarm".to_string(),
                    accepted: true,
                    disposition: "applied".to_string(),
                    resulting_integrity_state: Some("DISARMED".to_string()),
                    resulting_desired_armed: Some(false),
                    blockers: vec![],
                    warnings: vec![],
                    environment: Some(st.deployment_mode().as_api_label().to_string()),
                    scope: Some("daemon_instance".to_string()),
                    audit: OperatorActionAuditFields {
                        durable_db_write: st.db.is_some(),
                        durable_targets: if st.db.is_some() {
                            vec!["sys_arm_state".to_string()]
                        } else {
                            vec![]
                        },
                        audit_event_id: None,
                    },
                }),
            )
                .into_response()
        }

        "start-system" => match st.start_execution_runtime().await {
            Ok(snapshot) => {
                info!("ops/action start-system");
                if let Some(run_id) = snapshot.active_run_id {
                    let _ =
                        write_operator_audit_event(&st, Some(run_id), "run.start", "RUNNING").await;
                }
                (
                    StatusCode::OK,
                    Json(OperatorActionResponse {
                        requested_action: "start-system".to_string(),
                        accepted: true,
                        disposition: "applied".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![],
                        warnings: vec![],
                        environment: Some(st.deployment_mode().as_api_label().to_string()),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: st.db.is_some(),
                            durable_targets: if st.db.is_some() {
                                vec!["audit_events".to_string()]
                            } else {
                                vec![]
                            },
                            audit_event_id: None,
                        },
                    }),
                )
                    .into_response()
            }
            Err(err) => runtime_error_response(err),
        },

        "stop-system" => match st.stop_execution_runtime().await {
            Ok(snapshot) => {
                info!("ops/action stop-system");
                let _ =
                    write_operator_audit_event(&st, snapshot.active_run_id, "run.stop", "STOPPED")
                        .await;
                (
                    StatusCode::OK,
                    Json(OperatorActionResponse {
                        requested_action: "stop-system".to_string(),
                        accepted: true,
                        disposition: "applied".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![],
                        warnings: vec![],
                        environment: Some(st.deployment_mode().as_api_label().to_string()),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: st.db.is_some(),
                            durable_targets: if st.db.is_some() {
                                vec!["audit_events".to_string()]
                            } else {
                                vec![]
                            },
                            audit_event_id: None,
                        },
                    }),
                )
                    .into_response()
            }
            Err(err) => runtime_error_response(err),
        },

        "kill-switch" => match st.halt_execution_runtime().await {
            Ok(snapshot) => {
                info!("ops/action kill-switch");
                let _ =
                    write_operator_audit_event(&st, snapshot.active_run_id, "run.halt", "HALTED")
                        .await;
                (
                    StatusCode::OK,
                    Json(OperatorActionResponse {
                        requested_action: "kill-switch".to_string(),
                        accepted: true,
                        disposition: "applied".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![],
                        warnings: vec![],
                        environment: Some(st.deployment_mode().as_api_label().to_string()),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: st.db.is_some(),
                            durable_targets: if st.db.is_some() {
                                vec!["audit_events".to_string()]
                            } else {
                                vec![]
                            },
                            audit_event_id: None,
                        },
                    }),
                )
                    .into_response()
            }
            Err(err) => runtime_error_response(err),
        },

        "change-system-mode" => (
            // Mode transitions require a controlled daemon restart with configuration reload.
            // This cannot be done via API in the current architecture.
            // The GUI disables mode-change buttons to prevent this from being called,
            // but fail-close here as a defense-in-depth gate.
            StatusCode::CONFLICT,
            Json(OperatorActionResponse {
                requested_action: "change-system-mode".to_string(),
                accepted: false,
                disposition: "not_authoritative".to_string(),
                resulting_integrity_state: None,
                resulting_desired_armed: None,
                blockers: vec![
                    "Mode transitions require a controlled daemon restart with configuration reload. \
                     This is not authoritative via API in the current architecture.".to_string(),
                ],
                warnings: vec![],
                environment: Some(st.deployment_mode().as_api_label().to_string()),
                scope: Some("daemon_instance".to_string()),
                audit: OperatorActionAuditFields {
                    durable_db_write: false,
                    durable_targets: vec![],
                    audit_event_id: None,
                },
            }),
        )
            .into_response(),

        _ => (
            StatusCode::BAD_REQUEST,
            Json(OperatorActionResponse {
                requested_action: body.action_key.clone(),
                accepted: false,
                disposition: "unknown_action".to_string(),
                resulting_integrity_state: None,
                resulting_desired_armed: None,
                blockers: vec![format!(
                    "Unknown action_key '{}'; accepted keys: arm-execution, arm-strategy, \
                     disarm-execution, disarm-strategy, start-system, stop-system, kill-switch",
                    body.action_key
                )],
                warnings: vec![],
                environment: Some(st.deployment_mode().as_api_label().to_string()),
                scope: Some("daemon_instance".to_string()),
                audit: OperatorActionAuditFields {
                    durable_db_write: false,
                    durable_targets: vec![],
                    audit_event_id: None,
                },
            }),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/ops/catalog — canonical Action Catalog
// ---------------------------------------------------------------------------

/// Return the canonical operator Action Catalog with state-aware availability.
///
/// The catalog lists exactly the action keys that `POST /api/v1/ops/action` can
/// execute successfully right now.  `enabled` and `disabled_reason` reflect the
/// daemon's live integrity + runtime state at the moment of the request.
///
/// `change-system-mode` is intentionally absent from the catalog because it returns
/// 409 from the dispatcher (mode transitions require a controlled daemon restart).
pub(crate) async fn ops_catalog(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let (is_disarmed, is_halted_integrity) = {
        let ig = st.integrity.read().await;
        (ig.disarmed, ig.halted)
    };

    let state_str = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot.state,
        Err(_) => "idle".to_string(),
    };

    let armed = !is_disarmed && !is_halted_integrity;
    let halted = is_halted_integrity || state_str == "halted";
    let running = state_str == "running";
    let idle = state_str == "idle";

    let actions = vec![
        ActionCatalogEntry {
            action_key: "arm-execution".to_string(),
            label: "Arm Execution".to_string(),
            level: 1,
            description:
                "Arm the execution integrity gate. Required before any live order dispatch."
                    .to_string(),
            requires_reason: false,
            confirm_text: "Confirm: arm execution gate".to_string(),
            enabled: !armed && !halted,
            disabled_reason: if armed {
                Some("Execution is already armed.".to_string())
            } else if halted {
                Some("Cannot arm while system is halted.".to_string())
            } else {
                None
            },
        },
        ActionCatalogEntry {
            action_key: "disarm-execution".to_string(),
            label: "Disarm Execution".to_string(),
            level: 1,
            description:
                "Disarm the execution integrity gate. Stops new order dispatch immediately."
                    .to_string(),
            requires_reason: false,
            confirm_text: "Confirm: disarm execution gate".to_string(),
            enabled: armed,
            disabled_reason: if !armed {
                Some("Execution is already disarmed.".to_string())
            } else {
                None
            },
        },
        ActionCatalogEntry {
            action_key: "start-system".to_string(),
            label: "Start System".to_string(),
            level: 1,
            description: "Start the execution runtime. System must be idle to start.".to_string(),
            requires_reason: false,
            confirm_text: "Confirm: start execution runtime".to_string(),
            enabled: idle && !halted,
            disabled_reason: if halted {
                Some("Cannot start while system is halted.".to_string())
            } else if running {
                Some("System is already running.".to_string())
            } else if !idle {
                Some("System must be idle to start.".to_string())
            } else {
                None
            },
        },
        ActionCatalogEntry {
            action_key: "stop-system".to_string(),
            label: "Stop System".to_string(),
            level: 2,
            description:
                "Stop the execution runtime gracefully. Drains pending outbox before halting."
                    .to_string(),
            requires_reason: false,
            confirm_text: "Confirm: stop execution runtime".to_string(),
            enabled: running,
            disabled_reason: if !running {
                Some("System is not currently running.".to_string())
            } else {
                None
            },
        },
        ActionCatalogEntry {
            action_key: "kill-switch".to_string(),
            label: "Kill Switch".to_string(),
            level: 3,
            description:
                "Immediately halt all execution and disarm. Use only in emergency. Requires reason."
                    .to_string(),
            requires_reason: true,
            confirm_text:
                "Type CONFIRM to activate kill switch -- this halts all execution immediately"
                    .to_string(),
            enabled: !halted,
            disabled_reason: if halted {
                Some("System is already halted.".to_string())
            } else {
                None
            },
        },
    ];

    (
        StatusCode::OK,
        Json(ActionCatalogResponse {
            canonical_route: "/api/v1/ops/catalog".to_string(),
            actions,
        }),
    )
}

// ---------------------------------------------------------------------------
// /api/v1 summary spine — GUI alignment patch
// ---------------------------------------------------------------------------

fn runtime_error_response(err: RuntimeLifecycleError) -> Response {
    match err {
        RuntimeLifecycleError::Forbidden {
            fault_class,
            gate,
            message,
        } => (
            StatusCode::FORBIDDEN,
            Json(RuntimeErrorResponse {
                error: message,
                fault_class: fault_class.to_string(),
                gate: Some(gate),
            }),
        )
            .into_response(),
        RuntimeLifecycleError::ServiceUnavailable {
            fault_class,
            message,
        } => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(RuntimeErrorResponse {
                error: message,
                fault_class: fault_class.to_string(),
                gate: None,
            }),
        )
            .into_response(),
        RuntimeLifecycleError::Conflict {
            fault_class,
            message,
        } => (
            StatusCode::CONFLICT,
            Json(RuntimeErrorResponse {
                error: message,
                fault_class: fault_class.to_string(),
                gate: None,
            }),
        )
            .into_response(),
        RuntimeLifecycleError::Internal {
            fault_class,
            message,
        } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(RuntimeErrorResponse {
                error: message,
                fault_class: fault_class.to_string(),
                gate: None,
            }),
        )
            .into_response(),
    }
}

fn build_fault_signals(
    status: &crate::state::StatusSnapshot,
    reconcile: &crate::state::ReconcileStatusSnapshot,
    risk_blocked: bool,
) -> Vec<FaultSignal> {
    let mut signals = Vec::new();

    if status.state == "unknown" {
        signals.push(FaultSignal {
            class: "runtime.truth_mismatch.durable_active_without_local_owner".to_string(),
            severity: "critical".to_string(),
            summary: "Durable run appears active without daemon-owned runtime loop.".to_string(),
            detail: status.notes.clone(),
        });
    }

    if matches!(reconcile.status.as_str(), "dirty" | "stale" | "unavailable") {
        signals.push(FaultSignal {
            class: format!("reconcile.dispatch_block.{}", reconcile.status),
            severity: if reconcile.status == "dirty" {
                "critical"
            } else {
                "warning"
            }
            .to_string(),
            summary: "Reconcile state blocks or degrades safe dispatch.".to_string(),
            detail: reconcile.note.clone(),
        });
    }

    // PROD-02: reconcile "unknown" while the runtime is running means the first
    // reconcile tick has not yet completed — order consistency is unproven.
    if reconcile.status == "unknown" && status.state == "running" {
        signals.push(FaultSignal {
            class: "reconcile.unproven.running_without_reconcile_result".to_string(),
            severity: "critical".to_string(),
            summary: "Runtime is running but reconcile result is unproven; order consistency cannot be verified.".to_string(),
            detail: None,
        });
    }

    if risk_blocked {
        signals.push(FaultSignal {
            class: "risk.dispatch_denied.engine_blocked".to_string(),
            severity: "critical".to_string(),
            summary: "Risk engine indicates dispatch is blocked.".to_string(),
            detail: None,
        });
    }

    if status.state == "halted" {
        signals.push(FaultSignal {
            class: "runtime.halt.operator_or_safety".to_string(),
            severity: "critical".to_string(),
            summary: "Runtime is halted; dispatch remains fail-closed.".to_string(),
            detail: status.notes.clone(),
        });
    }

    signals
}

fn parse_decimal(value: &str) -> f64 {
    value.parse::<f64>().unwrap_or(0.0)
}

/// Map an OMS canonical state name to a display-friendly lifecycle stage label.
fn oms_stage_label(status: &str) -> &'static str {
    match status {
        "Open" => "Submitted",
        "PartiallyFilled" => "Partial Fill",
        "Filled" => "Filled",
        "CancelPending" => "Cancel Pending",
        "Cancelled" => "Cancelled",
        "ReplacePending" => "Replace Pending",
        "Rejected" => "Rejected",
        _ => "Unknown",
    }
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

async fn environment_and_live_routing_truth(
    st: &AppState,
    status: &StatusSnapshot,
) -> (Option<String>, Option<bool>) {
    // PROD-02: "unknown" state means durable active-run truth is unresolved;
    // fail-closed with explicit Some(false) rather than emitting None which
    // could be misread as "not yet determined, possibly live".
    let live_routing_enabled = match status.state.as_str() {
        "idle" | "halted" | "unknown" => Some(false),
        _ => None,
    };

    let Some(run_id) = status.active_run_id else {
        return (None, live_routing_enabled);
    };

    let Some(db) = st.db.as_ref() else {
        return (None, live_routing_enabled);
    };

    let Ok(run) = mqk_db::fetch_run(db, run_id).await else {
        return (None, live_routing_enabled);
    };

    let environment = Some(run.mode.to_ascii_lowercase());
    let live_routing_enabled = if status.state == "running" {
        Some(run.mode.eq_ignore_ascii_case("LIVE") || run.mode.eq_ignore_ascii_case("LIVE-CAPITAL"))
    } else {
        live_routing_enabled
    };

    (environment, live_routing_enabled)
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

    // Derive db_status and risk_blocked from a single DB query so the status
    // surface reflects real DB reachability at zero extra cost.
    // "unavailable" = no pool configured (intentional, e.g. paper/dev mode).
    // "ok" = pool configured and query succeeded.
    // "warning" = pool configured but query failed (reachability problem).
    let (risk_blocked, db_status) = if let Some(db) = st.db.as_ref() {
        let risk_result = mqk_db::load_risk_block_state(db).await;
        let db_ok = risk_result.is_ok();
        let risk_blocked = risk_result.ok().flatten().is_some_and(|risk| risk.blocked);
        let db_status = if db_ok { "ok" } else { "warning" }.to_string();
        (risk_blocked, db_status)
    } else {
        (false, "unavailable".to_string())
    };

    // Audit writer uses the DB; its status is directly proxied via db_status.
    let audit_writer_status = db_status.clone();

    let runtime_status = runtime_status_from_state(&status.state).to_string();
    let (environment, live_routing_enabled) =
        environment_and_live_routing_truth(&st, &status).await;
    let broker_status = if snapshot_present { "ok" } else { "warning" }.to_string();
    let integrity_status = if integrity_armed { "ok" } else { "warning" }.to_string();
    let reconcile_status = reconcile.status.clone();
    // PROD-02: reconcile "unknown" while the runtime is actively running is a
    // critical signal — we cannot verify order state is consistent.
    let has_critical = matches!(reconcile_status.as_str(), "dirty" | "stale")
        || (reconcile_status == "unknown" && runtime_status == "running");
    // db_status "warning" means a configured DB is not responding — that is a
    // real warning.  "unavailable" (no pool) is not a warning by itself.
    let has_warning = broker_status != "ok"
        || integrity_status != "ok"
        || reconcile_status != "ok"
        || db_status == "warning"
        || status.notes.is_some()
        || reconcile.note.is_some();

    (
        StatusCode::OK,
        Json(SystemStatusResponse {
            environment,
            daemon_mode: st.deployment_mode().as_api_label().to_string(),
            adapter_id: st.adapter_id().to_string(),
            deployment_start_allowed: st.deployment_readiness().start_allowed,
            deployment_blocker: st.deployment_readiness().blocker.clone(),
            runtime_status,
            broker_status,
            // AP-04: surface which source populates broker_snapshot for this adapter.
            broker_snapshot_source: st.broker_snapshot_source().as_str().to_string(),
            // AP-05: surface daemon-owned Alpaca WS continuity truth.
            // "not_applicable" for Paper; "cold_start_unproven"/"live"/"gap_detected" for Alpaca.
            // Only "live" is proven continuity — all others are fail-closed.
            alpaca_ws_continuity: st.alpaca_ws_continuity().await.as_status_str().to_string(),
            db_status,
            // AP-04B: market_data_health is derived from the typed StrategyMarketDataSource,
            // not hardcoded.  The value is "not_configured" for all current modes because
            // strategy feed policy is independent of broker kind — changing the adapter
            // does not change the feed source.
            market_data_health: st.strategy_market_data_source().as_health_str().to_string(),
            reconcile_status,
            integrity_status,
            audit_writer_status,
            last_heartbeat: status.deadman_last_heartbeat_utc.clone(),
            deadman_status: status.deadman_status.clone(),
            loop_latency_ms: None,
            active_account_id: None,
            config_profile: None,
            has_warning,
            has_critical,
            strategy_armed: integrity_armed,
            execution_armed: integrity_armed,
            live_routing_enabled,
            kill_switch_active: status.state == "halted",
            risk_halt_active: risk_blocked,
            integrity_halt_active: !integrity_armed,
            daemon_reachable: true,
            fault_signals: build_fault_signals(&status, &reconcile, risk_blocked),
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

    // Check DB reachability when a pool is configured.  When no pool is
    // configured (e.g. in tests or no-DB dev mode) return None rather than
    // emitting a synthetic blocker.
    let db_reachable: Option<bool> = if let Some(db) = st.db.as_ref() {
        Some(sqlx::query("SELECT 1").execute(db).await.is_ok())
    } else {
        None
    };

    // Broker config presence is knowable from the adapter identity.
    // Synthetic/paper adapters have no real broker config; any named live
    // adapter (e.g. "alpaca") has one configured.
    let broker_config_present: Option<bool> = match st.adapter_id() {
        "" | "null" | "paper" => Some(false),
        _ => Some(true),
    };

    // Market data config readiness is not observable at this level without
    // probing env/config; leave explicitly as None rather than inventing a
    // blocker.
    let market_data_config_present: Option<bool> = None;

    // Audit writer uses the DB; proxy its readiness via DB reachability.
    let audit_writer_ready: Option<bool> = db_reachable;

    let mut warnings = Vec::new();
    if status.notes.is_some() {
        warnings.push("Daemon status contains notes; verify runtime state.".to_string());
    }
    if market_data_config_present.is_none() {
        warnings.push(
            "Market data config readiness is not probed at preflight; verify separately."
                .to_string(),
        );
    }

    let mut blockers = Vec::new();
    if db_reachable == Some(false) {
        blockers.push("Database is not reachable.".to_string());
    }
    if execution_disarmed {
        blockers.push("Execution is disarmed at the integrity gate.".to_string());
    }
    if let Some(blocker) = st.deployment_readiness().blocker.clone() {
        blockers.push(blocker);
    }

    (
        StatusCode::OK,
        Json(PreflightStatusResponse {
            daemon_reachable: true,
            daemon_mode: st.deployment_mode().as_api_label().to_string(),
            adapter_id: st.adapter_id().to_string(),
            deployment_start_allowed: st.deployment_readiness().start_allowed,
            db_reachable,
            broker_config_present,
            market_data_config_present,
            audit_writer_ready,
            runtime_idle: Some(status.state != "running"),
            strategy_disarmed,
            execution_disarmed,
            live_routing_disabled: true,
            warnings,
            blockers,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/metadata
// ---------------------------------------------------------------------------

pub(crate) async fn system_metadata(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let integrity_armed = {
        let ig = st.integrity.read().await;
        !ig.is_execution_blocked()
    };
    // "ok" when execution is armed and the daemon is ready for dispatch.
    // "warning" when disarmed or halted — operator action is required.
    let endpoint_status = if integrity_armed { "ok" } else { "warning" }.to_string();

    (
        StatusCode::OK,
        Json(SystemMetadataResponse {
            build_version: st.build.version.to_string(),
            api_version: "v1".to_string(),
            broker_adapter: st.adapter_id().to_string(),
            endpoint_status,
            daemon_mode: st.deployment_mode().as_api_label().to_string(),
            adapter_id: st.adapter_id().to_string(),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/runtime-leadership
// ---------------------------------------------------------------------------

pub(crate) async fn system_runtime_leadership(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return runtime_error_response(err),
    };
    let reconcile = st.current_reconcile_snapshot().await;

    // For a single-node daemon the local process always holds or loses the
    // lease — there is no cluster election.
    let leader_node = "local".to_string();
    let leader_lease_state = match status.state.as_str() {
        "running" => "held",
        "unknown" => "contested",
        _ => "lost", // idle, halted
    }
    .to_string();

    // Fetch the latest run record once — reused for generation_id, restart
    // time, and the initial checkpoint row.
    let latest_run = if let Some(db) = st.db.as_ref() {
        mqk_db::fetch_latest_run_for_engine(db, DAEMON_ENGINE_ID, st.deployment_mode().as_db_mode())
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    // Generation ID: active run → latest run. When neither authoritative
    // source exists, report null rather than fabricating a placeholder.
    let generation_id = status
        .active_run_id
        .map(|id| id.to_string())
        .or_else(|| latest_run.as_ref().map(|r| r.run_id.to_string()));

    let last_restart_at = latest_run.as_ref().map(|r| r.started_at_utc.to_rfc3339());

    // Post-restart recovery state is derived from reconcile result.
    // "unknown" reconcile means reconcile has not yet run since the last
    // restart (or daemon started without DB) — this is "in_progress", not
    // "degraded", to distinguish it from a confirmed mismatch.
    let post_restart_recovery_state = match reconcile.status.as_str() {
        "ok" => "complete",
        "unknown" => "in_progress",
        _ => "degraded", // dirty, stale, unavailable
    }
    .to_string();

    let recovery_checkpoint = reconcile
        .last_run_at
        .as_deref()
        .unwrap_or("none")
        .to_string();

    // Build checkpoint rows from observable events.  When DB is available
    // the latest run start is the first concrete checkpoint.
    let mut checkpoints: Vec<RuntimeLeadershipCheckpointRow> = Vec::new();
    if let Some(run) = &latest_run {
        checkpoints.push(RuntimeLeadershipCheckpointRow {
            checkpoint_id: run.run_id.to_string(),
            checkpoint_type: "restart".to_string(),
            timestamp: run.started_at_utc.to_rfc3339(),
            generation_id: run.run_id.to_string(),
            leader_node: leader_node.clone(),
            status: "ok".to_string(),
            note: format!(
                "Run started; mode={}; adapter={}",
                st.deployment_mode().as_api_label(),
                st.adapter_id()
            ),
        });
    }

    (
        StatusCode::OK,
        Json(RuntimeLeadershipResponse {
            leader_node,
            leader_lease_state,
            generation_id,
            restart_count_24h: if let Some(db) = st.db.as_ref() {
                mqk_db::count_runs_in_last_24h(
                    db,
                    DAEMON_ENGINE_ID,
                    st.deployment_mode().as_db_mode(),
                )
                .await
                .ok()
                .map(|n| n as u32)
            } else {
                None
            },
            last_restart_at,
            post_restart_recovery_state,
            recovery_checkpoint,
            checkpoints,
        }),
    )
        .into_response()
}

pub(crate) async fn execution_summary(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    // DMON-06: derive counts from execution_snapshot (local OMS truth) instead
    // of broker_snapshot so this surface reflects the daemon's own order state.
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

        ExecutionSummaryResponse {
            has_snapshot: true,
            active_orders,
            pending_orders,
            dispatching_orders,
            reject_count_today: 0,
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

pub(crate) async fn execution_orders(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    // Canonical OMS truth: active orders from the in-memory execution snapshot.
    //
    // SEMANTIC INVARIANT: this route distinguishes two states that must NOT be
    // conflated:
    //   • HTTP 200 + [] → execution snapshot exists; there are zero active orders.
    //   • HTTP 503       → no execution snapshot; OMS truth is unavailable.
    //
    // The GUI reads the 503 as a "no_snapshot" signal and keeps this endpoint in
    // missingEndpoints so isMissingPanelTruth fires and the execution panel blocks.
    // Returning 200 + [] for both states would let the GUI render an empty order
    // list as authoritative healthy truth when the execution loop has never started.
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

    let updated_at = snapshot.snapshot_at_utc.to_rfc3339();
    let rows: Vec<ExecutionOrderRow> = snapshot
        .active_orders
        .iter()
        .map(|o| {
            let has_critical = o.status == "Rejected";
            let current_stage = oms_stage_label(&o.status).to_string();
            ExecutionOrderRow {
                internal_order_id: o.order_id.clone(),
                broker_order_id: o.broker_order_id.clone(),
                symbol: o.symbol.clone(),
                // OMS runtime has no per-order strategy attribution.
                strategy_id: None,
                // Per-order side is not tracked in the OMS snapshot.
                side: None,
                // Order type is not captured at OMS snapshot level.
                order_type: None,
                requested_qty: o.total_qty,
                filled_qty: o.filled_qty,
                current_status: o.status.clone(),
                current_stage,
                // Per-order creation time not tracked in OMS snapshot.
                age_ms: None,
                has_warning: false,
                has_critical,
                updated_at: updated_at.clone(),
            }
        })
        .collect();

    (StatusCode::OK, Json(rows)).into_response()
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

/// GET /api/v1/portfolio/positions
///
/// Returns broker-snapshot positions as canonical `PortfolioPositionRow` rows.
///
/// - `snapshot_state: "active"` + rows when `broker_snapshot` is loaded.
///   An empty `rows` array is authoritative ("zero positions in broker view").
/// - `snapshot_state: "no_snapshot"` + empty rows when no snapshot is loaded.
///   The GUI must NOT treat this as "zero positions"; it is missing broker truth.
pub(crate) async fn portfolio_positions(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    match snap {
        None => (
            StatusCode::OK,
            Json(PortfolioPositionsResponse {
                snapshot_state: "no_snapshot".to_string(),
                captured_at_utc: None,
                rows: vec![],
            }),
        )
            .into_response(),
        Some(snapshot) => {
            let captured_at_utc = snapshot.captured_at_utc.to_rfc3339();
            let rows = snapshot
                .positions
                .iter()
                .map(|p| {
                    let qty = p.qty.parse::<i64>().unwrap_or(0);
                    let avg_price = parse_decimal(&p.avg_price);
                    PortfolioPositionRow {
                        symbol: p.symbol.clone(),
                        // Broker snapshot has no strategy attribution.
                        strategy_id: None,
                        qty,
                        avg_price,
                        // Mark prices are not present in the broker snapshot.
                        mark_price: None,
                        // Broker snapshot has no unrealized PnL.
                        unrealized_pnl: None,
                        // Broker snapshot has no today-only realized PnL.
                        realized_pnl_today: None,
                        broker_qty: qty,
                        // Reconcile-level drift is not assessed at broker snapshot layer.
                        drift: None,
                    }
                })
                .collect();
            (
                StatusCode::OK,
                Json(PortfolioPositionsResponse {
                    snapshot_state: "active".to_string(),
                    captured_at_utc: Some(captured_at_utc),
                    rows,
                }),
            )
                .into_response()
        }
    }
}

/// GET /api/v1/portfolio/orders/open
///
/// Returns broker-snapshot open orders as canonical `PortfolioOpenOrderRow` rows.
///
/// - `snapshot_state: "active"` + rows when `broker_snapshot` is loaded.
/// - `snapshot_state: "no_snapshot"` + empty rows when no snapshot is loaded.
pub(crate) async fn portfolio_open_orders(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    match snap {
        None => (
            StatusCode::OK,
            Json(PortfolioOpenOrdersResponse {
                snapshot_state: "no_snapshot".to_string(),
                captured_at_utc: None,
                rows: vec![],
            }),
        )
            .into_response(),
        Some(snapshot) => {
            let captured_at_utc = snapshot.captured_at_utc.to_rfc3339();
            let rows = snapshot
                .orders
                .iter()
                .map(|o| {
                    let requested_qty = o.qty.parse::<i64>().unwrap_or(0);
                    PortfolioOpenOrderRow {
                        internal_order_id: o.client_order_id.clone(),
                        symbol: o.symbol.clone(),
                        // Broker snapshot has no strategy attribution.
                        strategy_id: None,
                        side: o.side.clone(),
                        status: o.status.clone(),
                        requested_qty,
                        // Partial fill quantity is not tracked in the broker snapshot.
                        filled_qty: None,
                        entered_at: o.created_at_utc.to_rfc3339(),
                    }
                })
                .collect();
            (
                StatusCode::OK,
                Json(PortfolioOpenOrdersResponse {
                    snapshot_state: "active".to_string(),
                    captured_at_utc: Some(captured_at_utc),
                    rows,
                }),
            )
                .into_response()
        }
    }
}

/// GET /api/v1/portfolio/fills
///
/// Returns broker-snapshot fills as canonical `PortfolioFillRow` rows.
///
/// - `snapshot_state: "active"` + rows when `broker_snapshot` is loaded.
/// - `snapshot_state: "no_snapshot"` + empty rows when no snapshot is loaded.
pub(crate) async fn portfolio_fills(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    match snap {
        None => (
            StatusCode::OK,
            Json(PortfolioFillsResponse {
                snapshot_state: "no_snapshot".to_string(),
                captured_at_utc: None,
                rows: vec![],
            }),
        )
            .into_response(),
        Some(snapshot) => {
            let captured_at_utc = snapshot.captured_at_utc.to_rfc3339();
            let rows = snapshot
                .fills
                .iter()
                .map(|f| {
                    let qty = f.qty.parse::<i64>().unwrap_or(0);
                    let price = parse_decimal(&f.price);
                    PortfolioFillRow {
                        fill_id: f.broker_fill_id.clone(),
                        internal_order_id: f.client_order_id.clone(),
                        symbol: f.symbol.clone(),
                        // Broker snapshot has no strategy attribution.
                        strategy_id: None,
                        side: f.side.clone(),
                        qty,
                        price,
                        broker_exec_id: f.broker_fill_id.clone(),
                        applied: true,
                        at: f.ts_utc.to_rfc3339(),
                    }
                })
                .collect();
            (
                StatusCode::OK,
                Json(PortfolioFillsResponse {
                    snapshot_state: "active".to_string(),
                    captured_at_utc: Some(captured_at_utc),
                    rows,
                }),
            )
                .into_response()
        }
    }
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

/// GET /api/v1/risk/denials — canonical risk denial truth surface.
///
/// # Truth-state contract
///
/// - `"active"` — execution loop is running AND a DB pool is available.
///   `denials` contains ONLY rows durably stored in `sys_risk_denial_events`.
///   Restart-safe.  An empty `denials` array means the risk gate has
///   genuinely never denied any order in this deployment.
///
/// - `"active_session_only"` — execution loop is running but NO DB pool is
///   available (test environments only).  `denials` is populated from the
///   in-memory ring buffer.  NOT restart-safe.  Not returned in production.
///
/// - `"durable_history"` — execution loop is not running but the DB has
///   historical rows from a prior session.  Restart-safe.
///
/// - `"no_snapshot"` — loop not running and no durable rows exist.  GUI IIFE
///   emits ok:false → risk panel blocks.
///
/// # Strict durable truth
///
/// When a DB pool is available, the route returns ONLY rows from
/// `sys_risk_denial_events`.  The in-memory ring buffer is NOT merged into
/// durable-history responses.  A denial whose DB persist call failed will be
/// absent from this response (the denial is still live in the ring buffer for
/// diagnostic purposes, but is not surfaced as restart-safe history).
///
/// `strategy_id` is always `null` — the risk gate path does not carry
/// strategy attribution.
pub(crate) async fn risk_denials(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.execution_snapshot.read().await.clone();

    // Helper: map a DB row to an API denial row.
    // `strategy_id` is always None — not available on the risk gate path.
    let db_to_api = |r: &mqk_db::RiskDenialEventRow| RiskDenialRow {
        id: r.id.clone(),
        at: r.denied_at_utc.to_rfc3339(),
        strategy_id: None,
        symbol: r.symbol.clone().unwrap_or_default(),
        rule: r.rule.clone(),
        message: r.message.clone(),
        severity: r.severity.clone(),
    };

    // -----------------------------------------------------------------------
    // Path A — DB pool is available.
    // Only DB rows are surfaced.  Ring buffer is NOT merged.
    // A denial that failed to persist to DB is excluded from this response.
    // -----------------------------------------------------------------------
    if let Some(pool) = st.db.as_ref() {
        let db_rows = match mqk_db::load_recent_risk_denial_events(pool, 100).await {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!("load_recent_risk_denial_events failed: {err}");
                // DB read failed — fall through to no_snapshot; do not
                // present ring-buffer rows as durable history.
                return (
                    StatusCode::OK,
                    Json(RiskDenialsResponse {
                        truth_state: "no_snapshot".to_string(),
                        snapshot_at_utc: None,
                        denials: vec![],
                    }),
                )
                    .into_response();
            }
        };

        return if let Some(snapshot) = snap {
            // Loop is running.  DB rows are the authoritative durable source.
            let denials = db_rows.iter().map(db_to_api).collect();
            (
                StatusCode::OK,
                Json(RiskDenialsResponse {
                    truth_state: "active".to_string(),
                    snapshot_at_utc: Some(snapshot.snapshot_at_utc.to_rfc3339()),
                    denials,
                }),
            )
                .into_response()
        } else if db_rows.is_empty() {
            // Loop not running, no rows in DB — truth entirely absent.
            (
                StatusCode::OK,
                Json(RiskDenialsResponse {
                    truth_state: "no_snapshot".to_string(),
                    snapshot_at_utc: None,
                    denials: vec![],
                }),
            )
                .into_response()
        } else {
            // Loop not running, historical durable rows exist from a prior session.
            let denials = db_rows.iter().map(db_to_api).collect();
            (
                StatusCode::OK,
                Json(RiskDenialsResponse {
                    truth_state: "durable_history".to_string(),
                    snapshot_at_utc: None,
                    denials,
                }),
            )
                .into_response()
        };
    }

    // -----------------------------------------------------------------------
    // Path B — No DB pool (test environments only; never reached in production).
    // Ring buffer only.  Explicitly labeled as session-only (not restart-safe).
    // -----------------------------------------------------------------------
    let Some(snapshot) = snap else {
        // No pool and no loop — denial truth is entirely absent.
        return (
            StatusCode::OK,
            Json(RiskDenialsResponse {
                truth_state: "no_snapshot".to_string(),
                snapshot_at_utc: None,
                denials: vec![],
            }),
        )
            .into_response();
    };

    // Loop running, no pool — ring buffer only.
    let denials = snapshot
        .recent_risk_denials
        .iter()
        .map(|r| RiskDenialRow {
            id: r.id.clone(),
            at: r.denied_at_utc.to_rfc3339(),
            strategy_id: None,
            symbol: r.symbol.clone().unwrap_or_default(),
            rule: r.rule.clone(),
            message: r.message.clone(),
            severity: r.severity.clone(),
        })
        .collect();
    (
        StatusCode::OK,
        Json(RiskDenialsResponse {
            truth_state: "active_session_only".to_string(),
            snapshot_at_utc: Some(snapshot.snapshot_at_utc.to_rfc3339()),
            denials,
        }),
    )
        .into_response()
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

pub(crate) async fn reconcile_mismatches(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let reconcile = st.current_reconcile_snapshot().await;
    match reconcile.status.as_str() {
        "unknown" => {
            return (
                StatusCode::OK,
                Json(ReconcileMismatchesResponse {
                    truth_state: "no_snapshot".to_string(),
                    snapshot_at_utc: None,
                    rows: vec![],
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
            }),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(ReconcileMismatchesResponse {
            truth_state: "active".to_string(),
            snapshot_at_utc: Some(schema_snapshot.captured_at_utc.to_rfc3339()),
            rows: reconcile_diff_rows(&report, &local, &broker),
        }),
    )
        .into_response()
}

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

pub(crate) async fn system_session(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return runtime_error_response(err),
    };
    let strategy_allowed = status.integrity_armed;
    // PROD-02: execution is only allowed when integrity is armed AND there is a
    // durable active run owned by this daemon.  Integrity-armed-but-idle is not
    // execution-capable — surfacing it as "enabled" is a misleading overclaim.
    let execution_allowed =
        strategy_allowed && status.state == "running" && status.active_run_id.is_some();

    let calendar = st.calendar_spec();
    let now_ts = Utc::now().timestamp(); // allow: operator-metadata wall-clock
    (
        StatusCode::OK,
        Json(SessionStateResponse {
            daemon_mode: st.deployment_mode().as_db_mode().to_string(),
            adapter_id: st.adapter_id().to_string(),
            deployment_start_allowed: st.deployment_readiness().start_allowed,
            deployment_blocker: st.deployment_readiness().blocker.clone(),
            operator_auth_mode: st.operator_auth_mode().label().to_string(),
            strategy_allowed,
            execution_allowed,
            system_trading_window: if execution_allowed {
                "enabled".to_string()
            } else {
                "disabled".to_string()
            },
            // Classify the market session using the dedicated session-truth
            // methods rather than the gap-detection `is_session_bar_end`.
            // AlwaysOn (paper/backtest) → "regular" (synthetic policy).
            // NyseWeekdays (live/shadow) → time-of-day classification (heuristic).
            market_session: calendar.classify_market_session(now_ts).to_string(),
            exchange_calendar_state: calendar.classify_exchange_calendar(now_ts).to_string(),
            calendar_spec_id: calendar.spec_id().to_string(),
            notes: vec![calendar.session_truth_note().to_string()],
        }),
    )
        .into_response()
}

pub(crate) async fn system_config_fingerprint(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let latest_run = if let Some(db) = st.db.as_ref() {
        mqk_db::fetch_latest_run_for_engine(db, DAEMON_ENGINE_ID, st.deployment_mode().as_db_mode())
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    (
        StatusCode::OK,
        Json(ConfigFingerprintResponse {
            config_hash: latest_run
                .as_ref()
                .map(|run| run.config_hash.clone())
                .unwrap_or_else(|| st.run_config_hash().to_string()),
            adapter_id: st.adapter_id().to_string(),
            risk_policy_version: None,
            strategy_bundle_version: None,
            build_version: st.build.version.to_string(),
            environment_profile: st.deployment_mode().as_api_label().to_string(),
            runtime_generation_id: latest_run.as_ref().map(|run| run.run_id.to_string()),
            last_restart_at: latest_run
                .as_ref()
                .map(|run| run.started_at_utc.to_rfc3339()),
        }),
    )
        .into_response()
}

pub(crate) async fn system_config_diffs() -> impl IntoResponse {
    // Config-diff persistence is not yet implemented.  Return an explicit
    // "not_wired" truth state so the GUI does not treat the empty rows as
    // authoritative "zero diffs."  The GUI IIFE checks this field and emits
    // ok:false, pushing "configDiffs" to usedMockSections and preventing the
    // config panel from rendering a misleading empty diff table.
    (
        StatusCode::OK,
        Json(ConfigDiffsResponse {
            truth_state: "not_wired".to_string(),
            rows: Vec::new(),
        }),
    )
        .into_response()
}

pub(crate) async fn strategy_summary() -> impl IntoResponse {
    // No real strategy-fleet registry is implemented yet.  The former
    // synthetic `daemon_integrity_gate` surrogate row has been removed: it was
    // daemon integrity state masquerading as a strategy-fleet row and gave the
    // operator false confidence that a real strategy was running.
    //
    // Return an explicit "not_wired" truth state so the GUI IIFE emits ok:false,
    // pushing "strategies" to usedMockSections, collapsing the strategy panel
    // authority to "placeholder", and blocking the StrategyScreen with an
    // "Unimplemented" notice rather than rendering a fake strategy row.
    (
        StatusCode::OK,
        Json(StrategySummaryResponse {
            truth_state: "not_wired".to_string(),
            rows: Vec::new(),
        }),
    )
        .into_response()
}

pub(crate) async fn strategy_suppressions() -> impl IntoResponse {
    // Suppression persistence is not yet implemented.  Return an explicit
    // "not_wired" truth state so the GUI does not treat the empty rows as
    // authoritative "zero suppressions."  The GUI IIFE checks this field and
    // emits ok:false, pushing "strategySuppressions" to usedMockSections and
    // preventing the strategy panel from rendering a misleading empty
    // suppressions table.
    (
        StatusCode::OK,
        Json(StrategySuppressionsResponse {
            truth_state: "not_wired".to_string(),
            rows: Vec::new(),
        }),
    )
        .into_response()
}

pub(crate) async fn audit_operator_actions(State(st): State<Arc<AppState>>) -> Response {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(OperatorActionsAuditResponse {
                canonical_route: "/api/v1/audit/operator-actions".to_string(),
                truth_state: "backend_unavailable".to_string(),
                backend: "unavailable".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    let rows = match sqlx::query(
        r#"
        select event_id, run_id, ts_utc, event_type
        from audit_events
        where topic = 'operator'
        order by ts_utc desc
        limit 200
        "#,
    )
    .fetch_all(db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("audit/operator-actions query failed: {err}"),
                    fault_class: "audit.operator_actions.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response()
        }
    };

    let rows = rows
        .into_iter()
        .map(|row| {
            let event_id: uuid::Uuid = row.get("event_id");
            let run_id: Option<uuid::Uuid> = row.get("run_id");
            let ts_utc: chrono::DateTime<chrono::Utc> = row.get("ts_utc");
            let event_type: String = row.get("event_type");
            OperatorActionAuditRow {
                audit_event_id: event_id.to_string(),
                ts_utc: ts_utc.to_rfc3339(),
                requested_action: event_type.clone(),
                disposition: "applied".to_string(),
                run_id: run_id.map(|v| v.to_string()),
                runtime_transition: runtime_transition_for_action(&event_type),
                provenance_ref: format!("audit_events:{}", event_id),
            }
        })
        .collect();

    (
        StatusCode::OK,
        Json(OperatorActionsAuditResponse {
            canonical_route: "/api/v1/audit/operator-actions".to_string(),
            truth_state: "active".to_string(),
            backend: "postgres.audit_events".to_string(),
            rows,
        }),
    )
        .into_response()
}

pub(crate) async fn audit_artifacts(State(st): State<Arc<AppState>>) -> Response {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(AuditArtifactsResponse {
                canonical_route: "/api/v1/audit/artifacts".to_string(),
                truth_state: "backend_unavailable".to_string(),
                backend: "unavailable".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    let runs = match sqlx::query(
        r#"
        select run_id, started_at_utc
        from runs
        order by started_at_utc desc, run_id desc
        limit 200
        "#,
    )
    .fetch_all(db)
    .await
    {
        Ok(runs) => runs,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("audit/artifacts query failed: {err}"),
                    fault_class: "audit.artifacts.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response()
        }
    };

    let rows = runs
        .into_iter()
        .map(|row| {
            let run_id: uuid::Uuid = row.get("run_id");
            let started_at_utc: chrono::DateTime<chrono::Utc> = row.get("started_at_utc");
            AuditArtifactRow {
                artifact_id: format!("run-config:{}", run_id),
                artifact_type: "run_config".to_string(),
                run_id: run_id.to_string(),
                created_at_utc: started_at_utc.to_rfc3339(),
                provenance_ref: format!("runs:{}", run_id),
            }
        })
        .collect();

    (
        StatusCode::OK,
        Json(AuditArtifactsResponse {
            canonical_route: "/api/v1/audit/artifacts".to_string(),
            truth_state: "active".to_string(),
            backend: "postgres.runs".to_string(),
            rows,
        }),
    )
        .into_response()
}

pub(crate) async fn ops_operator_timeline(State(st): State<Arc<AppState>>) -> Response {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(OperatorTimelineResponse {
                canonical_route: "/api/v1/ops/operator-timeline".to_string(),
                truth_state: "backend_unavailable".to_string(),
                backend: "unavailable".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    let runs = match sqlx::query(
        r#"
        select run_id, started_at_utc, armed_at_utc, running_at_utc, stopped_at_utc, halted_at_utc
        from runs
        order by started_at_utc desc, run_id desc
        limit 200
        "#,
    )
    .fetch_all(db)
    .await
    {
        Ok(runs) => runs,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("ops/operator-timeline runs query failed: {err}"),
                    fault_class: "ops.operator_timeline.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response()
        }
    };

    let mut rows: Vec<OperatorTimelineRow> = Vec::new();
    for row in &runs {
        let run_id: uuid::Uuid = row.get("run_id");
        let started_at_utc: chrono::DateTime<chrono::Utc> = row.get("started_at_utc");
        let armed_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("armed_at_utc");
        let running_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("running_at_utc");
        let stopped_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("stopped_at_utc");
        let halted_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("halted_at_utc");

        rows.push(OperatorTimelineRow {
            ts_utc: started_at_utc.to_rfc3339(),
            kind: "runtime_transition".to_string(),
            run_id: Some(run_id.to_string()),
            detail: "CREATED".to_string(),
            provenance_ref: format!("runs:{}:started_at_utc", run_id),
        });
        if let Some(ts) = armed_at_utc {
            rows.push(OperatorTimelineRow {
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                run_id: Some(run_id.to_string()),
                detail: "ARMED".to_string(),
                provenance_ref: format!("runs:{}:armed_at_utc", run_id),
            });
        }
        if let Some(ts) = running_at_utc {
            rows.push(OperatorTimelineRow {
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                run_id: Some(run_id.to_string()),
                detail: "RUNNING".to_string(),
                provenance_ref: format!("runs:{}:running_at_utc", run_id),
            });
        }
        if let Some(ts) = stopped_at_utc {
            rows.push(OperatorTimelineRow {
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                run_id: Some(run_id.to_string()),
                detail: "STOPPED".to_string(),
                provenance_ref: format!("runs:{}:stopped_at_utc", run_id),
            });
        }
        if let Some(ts) = halted_at_utc {
            rows.push(OperatorTimelineRow {
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                run_id: Some(run_id.to_string()),
                detail: "HALTED".to_string(),
                provenance_ref: format!("runs:{}:halted_at_utc", run_id),
            });
        }
    }

    let operator_events = match sqlx::query(
        r#"
        select event_id, run_id, ts_utc, event_type
        from audit_events
        where topic = 'operator'
        order by ts_utc desc
        limit 200
        "#,
    )
    .fetch_all(db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("ops/operator-timeline operator events query failed: {err}"),
                    fault_class: "ops.operator_timeline.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response()
        }
    };

    for row in operator_events {
        let event_id: uuid::Uuid = row.get("event_id");
        let run_id: Option<uuid::Uuid> = row.get("run_id");
        let ts_utc: chrono::DateTime<chrono::Utc> = row.get("ts_utc");
        let event_type: String = row.get("event_type");

        rows.push(OperatorTimelineRow {
            ts_utc: ts_utc.to_rfc3339(),
            kind: "operator_action".to_string(),
            run_id: run_id.map(|id| id.to_string()),
            detail: event_type,
            provenance_ref: format!("audit_events:{}", event_id),
        });
    }

    rows.sort_by(|a, b| b.ts_utc.cmp(&a.ts_utc));

    (
        StatusCode::OK,
        Json(OperatorTimelineResponse {
            canonical_route: "/api/v1/ops/operator-timeline".to_string(),
            truth_state: "active".to_string(),
            backend: "postgres.runs+postgres.audit_events".to_string(),
            rows,
        }),
    )
        .into_response()
}

async fn write_operator_audit_event(
    st: &Arc<AppState>,
    run_id: Option<uuid::Uuid>,
    event_type: &str,
    runtime_transition: &str,
) -> anyhow::Result<()> {
    let Some(db) = st.db.as_ref() else {
        return Ok(());
    };
    let Some(run_id) = run_id else {
        return Ok(());
    };

    // D1 — event_id is UUIDv5 derived from (run_id, event_type, ts_utc).
    // Both ts_utc and event_id share the same wall-clock read so there is no
    // independent drift between the stored timestamp and the event identifier.
    let ts_utc = chrono::Utc::now();
    let event_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!(
            "mqk-daemon.ops-audit.v1|{}|{}|{}",
            run_id,
            event_type,
            ts_utc.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        )
        .as_bytes(),
    );
    mqk_db::insert_audit_event(
        db,
        &mqk_db::NewAuditEvent {
            event_id,
            run_id,
            ts_utc,
            topic: "operator".to_string(),
            event_type: event_type.to_string(),
            payload: serde_json::json!({
                "runtime_transition": runtime_transition,
                "source": "mqk-daemon.routes",
            }),
            hash_prev: None,
            hash_self: None,
        },
    )
    .await
}

fn runtime_transition_for_action(action: &str) -> Option<String> {
    match action {
        "control.arm" => Some("ARMED".to_string()),
        "control.disarm" => Some("DISARMED".to_string()),
        "run.start" => Some("RUNNING".to_string()),
        "run.stop" => Some("STOPPED".to_string()),
        "run.halt" => Some("HALTED".to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// GET /v1/trading/*  — DAEMON-1 (read-only placeholders)
// ---------------------------------------------------------------------------

fn trading_snapshot_state_label(reconcile_status: &str, has_snapshot: bool) -> &'static str {
    if !has_snapshot {
        "no_snapshot"
    } else if reconcile_status == "stale" {
        "stale_snapshot"
    } else {
        "current_snapshot"
    }
}

pub(crate) async fn trading_account(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    let reconcile = st.current_reconcile_snapshot().await;

    let snapshot_state =
        trading_snapshot_state_label(&reconcile.status, snap.is_some()).to_string();
    let snapshot_captured_at_utc = snap
        .as_ref()
        .map(|snapshot| snapshot.captured_at_utc.to_rfc3339());
    let account = if snapshot_state == "current_snapshot" {
        snap.map(|snapshot| snapshot.account)
    } else {
        None
    };

    (
        StatusCode::OK,
        Json(TradingAccountResponse {
            snapshot_state,
            snapshot_captured_at_utc,
            account,
        }),
    )
}

pub(crate) async fn trading_positions(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    let reconcile = st.current_reconcile_snapshot().await;

    let snapshot_state =
        trading_snapshot_state_label(&reconcile.status, snap.is_some()).to_string();
    let snapshot_captured_at_utc = snap
        .as_ref()
        .map(|snapshot| snapshot.captured_at_utc.to_rfc3339());
    let positions = if snapshot_state == "current_snapshot" {
        snap.map(|snapshot| snapshot.positions)
    } else {
        None
    };

    (
        StatusCode::OK,
        Json(TradingPositionsResponse {
            snapshot_state,
            snapshot_captured_at_utc,
            positions,
        }),
    )
}

pub(crate) async fn trading_orders(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    let reconcile = st.current_reconcile_snapshot().await;

    let snapshot_state =
        trading_snapshot_state_label(&reconcile.status, snap.is_some()).to_string();
    let snapshot_captured_at_utc = snap
        .as_ref()
        .map(|snapshot| snapshot.captured_at_utc.to_rfc3339());
    let orders = if snapshot_state == "current_snapshot" {
        snap.map(|snapshot| snapshot.orders)
    } else {
        None
    };

    (
        StatusCode::OK,
        Json(TradingOrdersResponse {
            snapshot_state,
            snapshot_captured_at_utc,
            orders,
        }),
    )
}

pub(crate) async fn trading_fills(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = st.broker_snapshot.read().await.clone();
    let reconcile = st.current_reconcile_snapshot().await;

    let snapshot_state =
        trading_snapshot_state_label(&reconcile.status, snap.is_some()).to_string();
    let snapshot_captured_at_utc = snap
        .as_ref()
        .map(|snapshot| snapshot.captured_at_utc.to_rfc3339());
    let fills = if snapshot_state == "current_snapshot" {
        snap.map(|snapshot| snapshot.fills)
    } else {
        None
    };

    (
        StatusCode::OK,
        Json(TradingFillsResponse {
            snapshot_state,
            snapshot_captured_at_utc,
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
