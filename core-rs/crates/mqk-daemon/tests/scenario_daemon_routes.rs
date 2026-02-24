//! In-process scenario tests for mqk-daemon HTTP endpoints.
//!
//! These tests spin up the Axum router **without** binding a TCP socket.
//! Each test calls `routes::build_router` and drives it via
//! `tower::ServiceExt::oneshot` — no network I/O required.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt; // oneshot

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a fresh in-process router backed by a clean AppState.
fn make_router() -> axum::Router {
    let st = Arc::new(state::AppState::new());
    routes::build_router(st)
}

/// Drive the router with a single request and return (status, body_bytes).
async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    (status, body)
}

/// Parse body bytes as a `serde_json::Value`.
fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

// ---------------------------------------------------------------------------
// GET /v1/health
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_returns_200_ok_true() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/health")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["ok"], true);
    assert_eq!(json["service"], "mqk-daemon");
}

// ---------------------------------------------------------------------------
// GET /v1/status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_returns_200_with_integrity_armed_field() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    // Fresh state: idle, no active run, disarmed (Patch C1 — fail-closed at boot).
    assert_eq!(json["state"], "idle");
    assert!(json["active_run_id"].is_null());
    assert_eq!(
        json["integrity_armed"], false,
        "default state should be disarmed (Patch C1)"
    );
}

// ---------------------------------------------------------------------------
// POST /v1/run/start
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_start_sets_state_running_and_returns_run_id() {
    let st = Arc::new(state::AppState::new());

    // Patch C1: arm before starting (boot is fail-closed/disarmed).
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), arm_req).await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["state"], "running");
    assert!(
        !json["active_run_id"].is_null(),
        "run_id should be set after start"
    );
}

// ---------------------------------------------------------------------------
// POST /v1/run/start is idempotent (same run_id on double-call)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_start_is_idempotent_keeps_run_id() {
    let st = Arc::new(state::AppState::new());

    // Patch C1: arm before starting (boot is fail-closed/disarmed).
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), arm_req).await;

    let req1 = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body1) = call(routes::build_router(Arc::clone(&st)), req1).await;
    let run_id_first = parse_json(body1)["active_run_id"].clone();

    let req2 = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body2) = call(routes::build_router(Arc::clone(&st)), req2).await;
    let run_id_second = parse_json(body2)["active_run_id"].clone();

    assert_eq!(
        run_id_first, run_id_second,
        "second start should preserve existing run_id"
    );
}

// ---------------------------------------------------------------------------
// POST /v1/run/stop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_stop_sets_state_idle_and_clears_run_id() {
    let st = Arc::new(state::AppState::new());

    // Patch C1: arm before starting (boot is fail-closed/disarmed).
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), arm_req).await;

    // Start first.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), start_req).await;

    // Then stop.
    let stop_req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), stop_req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["state"], "idle");
    assert!(json["active_run_id"].is_null(), "run_id cleared after stop");
}

// ---------------------------------------------------------------------------
// POST /v1/run/halt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_halt_sets_state_halted_and_preserves_run_id() {
    let st = Arc::new(state::AppState::new());

    // Patch C1: arm before starting (boot is fail-closed/disarmed).
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), arm_req).await;

    // Start first so there is a run_id.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, start_body) = call(routes::build_router(Arc::clone(&st)), start_req).await;
    let run_id = parse_json(start_body)["active_run_id"].clone();

    // Now halt.
    let halt_req = Request::builder()
        .method("POST")
        .uri("/v1/run/halt")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), halt_req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["state"], "halted");
    assert_eq!(
        json["active_run_id"], run_id,
        "halt should preserve run_id for GUI display"
    );
}

// ---------------------------------------------------------------------------
// POST /v1/integrity/arm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integrity_arm_sets_armed_true() {
    let st = Arc::new(state::AppState::new());

    // Disarm first so we can verify arm actually changes state.
    let disarm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/disarm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), disarm_req).await;

    // Now arm.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["armed"], true, "arm should set armed=true");
}

// ---------------------------------------------------------------------------
// POST /v1/integrity/disarm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integrity_disarm_sets_armed_false() {
    let st = Arc::new(state::AppState::new());

    let req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/disarm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(json["armed"], false, "disarm should set armed=false");
}

// ---------------------------------------------------------------------------
// Status reflects integrity arm/disarm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_reflects_integrity_armed_flag() {
    let st = Arc::new(state::AppState::new());

    // Default: disarmed (Patch C1 — fail-closed at boot).
    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(parse_json(body)["integrity_armed"], false);

    // Disarm (idempotent — already disarmed at boot).
    let req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/disarm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), req).await;

    // Status still shows false.
    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(parse_json(body)["integrity_armed"], false);

    // Arm again.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), req).await;

    // Status back to true.
    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(parse_json(body)["integrity_armed"], true);
}

// ---------------------------------------------------------------------------
// Patch L1: run_start refused (403) when integrity is disarmed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_start_refused_403_when_integrity_disarmed() {
    let st = Arc::new(state::AppState::new());

    // Disarm first.
    let disarm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/disarm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), disarm_req).await;

    // Now try to start — must be refused.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "run/start must be 403 when integrity is disarmed"
    );

    let json = parse_json(body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("GATE_REFUSED"),
        "body should contain GATE_REFUSED: {json}"
    );
    assert_eq!(json["gate"], "integrity_armed");
}

#[tokio::test]
async fn run_start_succeeds_after_rearm() {
    let st = Arc::new(state::AppState::new());

    // Disarm.
    let disarm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/disarm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), disarm_req).await;

    // Confirm 403 while disarmed.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(routes::build_router(Arc::clone(&st)), start_req).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Re-arm.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), arm_req).await;

    // Now start must succeed.
    let start_req2 = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status2, body2) = call(routes::build_router(Arc::clone(&st)), start_req2).await;
    assert_eq!(status2, StatusCode::OK);
    let json = parse_json(body2);
    assert_eq!(json["state"], "running");
}

// ---------------------------------------------------------------------------
// Unknown routes return 404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_route_returns_404() {
    let router = make_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/does_not_exist")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, _) = call(router, req).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
