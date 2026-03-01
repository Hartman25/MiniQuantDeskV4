//! Axum router and all HTTP handlers for mqk-daemon.
//!
//! `build_router` is the single entry point; `main.rs` calls it and attaches
//! middleware layers.  All handlers are `pub(crate)` so the scenario tests in
//! `tests/` can compose the router directly.

use std::{convert::Infallible, sync::Arc};

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures_util::{Stream, StreamExt};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::info;
use uuid::Uuid;

use crate::{
    api_types::{
        GateRefusedResponse, HealthResponse, IntegrityResponse, TradingAccountResponse,
        TradingFillsResponse, TradingOrdersResponse, TradingPositionsResponse,
        TradingSnapshotResponse,
    },
    state::{uptime_secs, AppState, BusMsg},
};

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the complete application router wired to the given shared state.
///
/// Middleware layers (CORS, tracing) are **not** applied here; `main.rs`
/// attaches them after this call so tests can use the bare router.
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/status", get(status_handler))
        .route("/v1/stream", get(stream))
        .route("/v1/run/start", post(run_start))
        .route("/v1/run/stop", post(run_stop))
        .route("/v1/run/halt", post(run_halt))
        .route("/v1/integrity/arm", post(integrity_arm))
        .route("/v1/integrity/disarm", post(integrity_disarm))
        // DAEMON-1: trading read APIs (placeholder until broker wiring exists)
        .route("/v1/trading/account", get(trading_account))
        .route("/v1/trading/positions", get(trading_positions))
        .route("/v1/trading/orders", get(trading_orders))
        .route("/v1/trading/fills", get(trading_fills))
        // DAEMON-2: dev-only snapshot inject/clear + readback
        .route("/v1/trading/snapshot", get(trading_snapshot))
        .route("/v1/trading/snapshot", post(trading_snapshot_set))
        .route(
            "/v1/trading/snapshot",
            axum::routing::delete(trading_snapshot_clear),
        )
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

pub(crate) async fn status_handler(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let mut snap = st.status.read().await.clone();
    snap.daemon_uptime_secs = uptime_secs();

    // Patch C2: sync integrity_armed from the authoritative gate — reflects
    // both `disarmed` and `halted` flags via `is_execution_blocked()`.
    {
        let ig = st.integrity.read().await;
        snap.integrity_armed = !ig.is_execution_blocked();
    }

    let _ = st.bus.send(BusMsg::Status(snap.clone()));
    (StatusCode::OK, Json(snap))
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
    // Gate: integrity must be armed before any run can start.
    {
        let ig = st.integrity.read().await;
        if ig.is_execution_blocked() {
            return (
                StatusCode::FORBIDDEN,
                Json(GateRefusedResponse {
                    error: "GATE_REFUSED: integrity disarmed or halted; arm integrity first"
                        .to_string(),
                    gate: "integrity_armed".to_string(),
                }),
            )
                .into_response();
        }
    }

    let mut s = st.status.write().await;

    if s.state != "running" {
        s.active_run_id = Some(derive_daemon_run_id(st.build.service, st.build.version));
    }
    s.state = "running".to_string();
    s.notes = Some("run started (in-memory); wire orchestrator next".to_string());
    s.daemon_uptime_secs = uptime_secs();

    // Patch C2: sync integrity_armed from the authoritative gate.
    {
        let ig = st.integrity.read().await;
        s.integrity_armed = !ig.is_execution_blocked();
    }

    let snap = s.clone();
    drop(s);

    info!(run_id = ?snap.active_run_id, "run/start");
    let _ = st.bus.send(BusMsg::Status(snap.clone()));
    (StatusCode::OK, Json(snap)).into_response()
}

// ---------------------------------------------------------------------------
// POST /v1/run/stop
// ---------------------------------------------------------------------------

pub(crate) async fn run_stop(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let mut s = st.status.write().await;

    s.active_run_id = None;
    s.state = "idle".to_string();
    s.notes = Some("run stopped (in-memory)".to_string());
    s.daemon_uptime_secs = uptime_secs();

    // Patch C2: sync integrity_armed from the authoritative gate.
    {
        let ig = st.integrity.read().await;
        s.integrity_armed = !ig.is_execution_blocked();
    }

    let snap = s.clone();
    drop(s);

    info!("run/stop");
    let _ = st.bus.send(BusMsg::Status(snap.clone()));
    (StatusCode::OK, Json(snap))
}

// ---------------------------------------------------------------------------
// POST /v1/run/halt
// ---------------------------------------------------------------------------

pub(crate) async fn run_halt(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    // Patch C2: set integrity halted — makes halt sticky across the session.
    // Execution gate (`is_execution_blocked`) will return true until the
    // operator explicitly calls `POST /v1/integrity/arm`.
    {
        let mut ig = st.integrity.write().await;
        ig.halted = true;
    }

    let mut s = st.status.write().await;

    // Keep run_id so the GUI can show what was halted.
    s.state = "halted".to_string();
    s.notes = Some("HALT asserted (in-memory); execution should gate on this later".to_string());
    s.daemon_uptime_secs = uptime_secs();

    // Patch C2: sync integrity_armed from the authoritative gate.
    {
        let ig = st.integrity.read().await;
        s.integrity_armed = !ig.is_execution_blocked();
    }

    let snap = s.clone();
    drop(s);

    info!("run/halt");
    let _ = st.bus.send(BusMsg::Status(snap.clone()));
    (StatusCode::OK, Json(snap))
}

// ---------------------------------------------------------------------------
// POST /v1/integrity/arm
// ---------------------------------------------------------------------------

pub(crate) async fn integrity_arm(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    // Patch C2: arm clears BOTH disarmed and halted — it is the sole escape
    // from any blocked integrity state (mirrors ArmState::arm() semantics).
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    // Sync the status snapshot. Both flags are now false so is_armed = true.
    let (armed, active_run_id, state) = {
        let mut s = st.status.write().await;
        s.integrity_armed = true;
        (true, s.active_run_id, s.state.clone())
    };

    info!("integrity/arm");
    let _ = st.bus.send(BusMsg::LogLine {
        level: "INFO".to_string(),
        msg: "integrity armed".to_string(),
    });

    (
        StatusCode::OK,
        Json(IntegrityResponse {
            armed,
            active_run_id,
            state,
        }),
    )
}

// ---------------------------------------------------------------------------
// POST /v1/integrity/disarm
// ---------------------------------------------------------------------------

pub(crate) async fn integrity_disarm(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    // disarm = set the disarmed flag
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = true;
    }

    // Sync the status snapshot.
    let (armed, active_run_id, state) = {
        let mut s = st.status.write().await;
        s.integrity_armed = false;
        (false, s.active_run_id, s.state.clone())
    };

    info!("integrity/disarm");
    let _ = st.bus.send(BusMsg::LogLine {
        level: "WARN".to_string(),
        msg: "integrity DISARMED".to_string(),
    });

    (
        StatusCode::OK,
        Json(IntegrityResponse {
            armed,
            active_run_id,
            state,
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
    let snap = st.broker_snapshot.read().await.clone();
    (
        StatusCode::OK,
        Json(TradingSnapshotResponse { snapshot: snap }),
    )
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
    let allow = std::env::var("MQK_DEV_ALLOW_SNAPSHOT_INJECT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !allow {
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
    let allow = std::env::var("MQK_DEV_ALLOW_SNAPSHOT_INJECT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !allow {
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
// Run-ID derivation (D1-1)
// ---------------------------------------------------------------------------

/// Derive a deterministic in-memory run ID from daemon build metadata.
///
/// **No RNG.** Uses `Uuid::new_v5` (SHA-1 over the DNS namespace).
///
/// Inputs: `service` (crate name, static str) and `version` (semver, static
/// str). Both are compile-time constants — no wall-clock, no random state.
///
/// The resulting UUID is stable for a given binary version, making it
/// suitable as an in-memory session label. The authoritative run ID for DB
/// persistence and cross-system audit correlation is created by `mqk-cli`
/// (see `derive_cli_run_id` in `mqk-cli/src/commands/run.rs`). A later
/// patch will wire the CLI run ID into the daemon so both IDs unify.
fn derive_daemon_run_id(service: &'static str, version: &'static str) -> Uuid {
    let data = format!("mqk-daemon.run.v1|{}|{}", service, version);
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, data.as_bytes())
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
