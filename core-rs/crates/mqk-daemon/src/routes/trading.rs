//! Trading and diagnostics route handlers.
//!
//! Contains: trading_account, trading_positions, trading_orders, trading_fills,
//! trading_snapshot, trading_snapshot_set, trading_snapshot_clear,
//! diagnostics_snapshot, stream, broadcast_to_sse,
//! trading_snapshot_state_label.

use std::{convert::Infallible, sync::Arc};

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Json,
};
use futures_util::{Stream, StreamExt};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::api_types::{
    DiagnosticsSnapshotResponse, GateRefusedResponse, TradingAccountResponse, TradingFillsResponse,
    TradingOrdersResponse, TradingPositionsResponse, TradingSnapshotResponse,
};
use crate::state::{AppState, BusMsg};

// ---------------------------------------------------------------------------
// Snapshot state helper
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

// ---------------------------------------------------------------------------
// GET /v1/trading/account
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// GET /v1/trading/positions
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// GET /v1/trading/orders
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// GET /v1/trading/fills
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// GET /v1/trading/snapshot
// ---------------------------------------------------------------------------

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
// GET /v1/diagnostics/snapshot (B4)
// ---------------------------------------------------------------------------

pub(crate) async fn diagnostics_snapshot(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let snapshot = st.execution_snapshot.read().await.clone();
    (
        StatusCode::OK,
        Json(DiagnosticsSnapshotResponse { snapshot }),
    )
}

// ---------------------------------------------------------------------------
// GET /v1/stream (SSE)
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
            Err(_) => None,
        }
    })
}
