//! Scenario: Deadman blocks dispatch — Patch C2
//!
//! # Invariant under test
//!
//! `POST /v1/run/halt` sets `ig.halted = true` in the integrity state.
//! Because `IntegrityState::is_execution_blocked()` = `disarmed || halted`,
//! a subsequent `POST /v1/run/start` returns 403 until the operator explicitly
//! calls `POST /v1/integrity/arm` — the sole escape from any blocked state.
//!
//! `POST /v1/integrity/arm` clears BOTH `disarmed` AND `halted`, mirroring
//! the `ArmState::arm()` semantics proved in the pure-logic layer tests.
//!
//! Three tests:
//!
//! 1. After halt, run/start returns 403 (deadman blocks dispatch).
//! 2. After halt, GET /v1/status reports `integrity_armed: false`.
//! 3. After halt then explicit arm, run/start succeeds.
//!
//! All tests are pure in-process; no DB or network required.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt; // oneshot

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

/// Arm the integrity gate (required before any run can start; Patch C1).
async fn arm(st: &Arc<state::AppState>) {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(routes::build_router(Arc::clone(st)), req).await;
    assert_eq!(status, StatusCode::OK, "arm must succeed");
}

/// Halt the run (sets ig.halted = true; Patch C2).
async fn halt(st: &Arc<state::AppState>) {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/halt")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(routes::build_router(Arc::clone(st)), req).await;
    assert_eq!(status, StatusCode::OK, "halt must succeed");
}

// ---------------------------------------------------------------------------
// 1. run/start returns 403 after halt (deadman blocks dispatch)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_start_returns_403_after_halt() {
    let st = Arc::new(state::AppState::new());

    // Arm first so the halt is meaningful (arm then halt, not just boot-disarmed).
    arm(&st).await;
    halt(&st).await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "run/start must be 403 after halt (deadman sticky)"
    );
    let json = parse_json(body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("GATE_REFUSED"),
        "body should contain GATE_REFUSED: {json}"
    );
}

// ---------------------------------------------------------------------------
// 2. Status reports integrity_armed = false after halt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_shows_not_armed_after_halt() {
    let st = Arc::new(state::AppState::new());

    arm(&st).await;
    halt(&st).await;

    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    let json = parse_json(body);

    assert_eq!(
        json["integrity_armed"], false,
        "status must report integrity_armed=false after halt (Patch C2)"
    );
}

// ---------------------------------------------------------------------------
// 3. After halt then explicit arm, run/start succeeds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_start_succeeds_after_halt_then_arm() {
    let st = Arc::new(state::AppState::new());

    arm(&st).await;
    halt(&st).await;

    // Confirm blocked.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "must be blocked after halt");

    // Re-arm — the sole escape from any blocked integrity state.
    arm(&st).await;

    // Now start succeeds.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "run/start must succeed after halt + explicit arm"
    );
    let json = parse_json(body);
    assert_eq!(json["state"], "running");
    assert!(
        !json["active_run_id"].is_null(),
        "run_id should be set after start"
    );
}
