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
    api_types::{GateRefusedResponse, HealthResponse, IntegrityResponse},
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

    // Sync the integrity_armed field from the authoritative integrity state.
    {
        let ig = st.integrity.read().await;
        snap.integrity_armed = !ig.disarmed;
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
        s.active_run_id = Some(Uuid::new_v4());
    }
    s.state = "running".to_string();
    s.notes = Some("run started (in-memory); wire orchestrator next".to_string());
    s.daemon_uptime_secs = uptime_secs();

    // Keep integrity_armed in sync.
    {
        let ig = st.integrity.read().await;
        s.integrity_armed = !ig.disarmed;
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

    {
        let ig = st.integrity.read().await;
        s.integrity_armed = !ig.disarmed;
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
    let mut s = st.status.write().await;

    // Keep run_id so the GUI can show what was halted.
    s.state = "halted".to_string();
    s.notes = Some("HALT asserted (in-memory); execution should gate on this later".to_string());
    s.daemon_uptime_secs = uptime_secs();

    {
        let ig = st.integrity.read().await;
        s.integrity_armed = !ig.disarmed;
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
    // arm = clear the disarmed flag
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
    }

    // Sync the status snapshot.
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
