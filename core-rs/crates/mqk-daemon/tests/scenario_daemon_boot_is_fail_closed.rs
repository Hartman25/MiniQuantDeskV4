//! Scenario: Daemon boot is fail-closed — Patch C1
//!
//! # Invariant under test
//!
//! `AppState::new()` initialises `IntegrityState` with `disarmed = true`
//! (fail-closed). The daemon must require an explicit operator arm before any
//! broker operation is permitted.
//!
//! Three tests:
//!
//! 1. Fresh status snapshot exposes `integrity_armed: false`.
//! 2. `POST /v1/run/start` returns 403 on a fresh (never-armed) daemon.
//! 3. `POST /v1/run/start` succeeds after an explicit arm.
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

// ---------------------------------------------------------------------------
// 1. Fresh status snapshot reports integrity_armed = false
// ---------------------------------------------------------------------------

#[tokio::test]
async fn boot_status_reports_integrity_disarmed() {
    let st = Arc::new(state::AppState::new());

    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;

    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(
        json["integrity_armed"], false,
        "daemon must boot disarmed (fail-closed, Patch C1)"
    );
}

// ---------------------------------------------------------------------------
// 2. run/start returns 403 before any arm call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_start_returns_403_before_arm() {
    let st = Arc::new(state::AppState::new());

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "run/start must be blocked at boot (integrity never armed)"
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

// ---------------------------------------------------------------------------
// 3. run/start succeeds after explicit arm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_start_succeeds_after_explicit_arm() {
    let st = Arc::new(state::AppState::new());

    // Arm explicitly.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "arm must succeed");

    // Now start is allowed.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "run/start must succeed after explicit arm"
    );
    let json = parse_json(body);
    assert_eq!(json["state"], "running");
    assert!(
        !json["active_run_id"].is_null(),
        "run_id should be set after start"
    );
}
