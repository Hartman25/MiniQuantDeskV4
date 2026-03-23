//! Scenario: Deadman blocks dispatch — Patch C2
//!
//! # Invariant under test
//!
//! The daemon now treats `/v1/run/halt` as a DB-authoritative lifecycle action.
//! In no-DB mode that route may fail closed, so this file does **not** prove the
//! halted state by assuming the route succeeds.
//!
//! Instead, these tests cover two honest surfaces separately:
//!
//! 1. `POST /v1/run/halt` fails closed without runtime DB truth.
//! 2. A halted/disarmed integrity state blocks `POST /v1/run/start` with 403.
//! 3. `GET /v1/status` reports `integrity_armed = false` after halted state is asserted.
//! 4. `POST /v1/integrity/arm` is the sole escape from halted/disarmed state,
//!    but `POST /v1/run/start` still returns 503 until DB-backed runtime truth exists.
//!
//! All tests are pure in-process; no DB or network required.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt; // oneshot

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

async fn arm(st: &Arc<state::AppState>) {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(st)), req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "arm must succeed: {}",
        parse_json(body)
    );
}

async fn force_halted(st: &Arc<state::AppState>) {
    let mut integrity = st.integrity.write().await;
    integrity.disarmed = true;
    integrity.halted = true;
}

#[tokio::test]
async fn halt_route_fails_closed_without_db_truth() {
    let st = Arc::new(state::AppState::new());
    arm(&st).await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/halt")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "run/halt must fail closed when runtime DB is not configured"
    );

    let json = parse_json(body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "body should explain DB-backed runtime requirement: {json}"
    );
}

#[tokio::test]
async fn run_start_returns_403_after_halt() {
    // PT-TRUTH-01: default paper+paper is fail-closed; use paper+alpaca so the
    // integrity gate (not deployment readiness) is what blocks the start.
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    force_halted(&st).await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "run/start must be 403 after halted integrity state"
    );
    let json = parse_json(body);
    assert_eq!(json["gate"], "integrity_armed");
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("GATE_REFUSED"),
        "body should contain GATE_REFUSED: {json}"
    );
}

#[tokio::test]
async fn status_shows_not_armed_after_halt() {
    let st = Arc::new(state::AppState::new());

    force_halted(&st).await;

    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;

    assert_eq!(status, StatusCode::OK, "status route must stay readable");

    let json = parse_json(body);
    assert_eq!(json["state"], "halted");
    assert_eq!(
        json["integrity_armed"], false,
        "status must report integrity_armed=false after halted state"
    );
}

#[tokio::test]
async fn run_start_requires_db_after_halt_then_arm() {
    // PT-TRUTH-01: default paper+paper is fail-closed; use paper+alpaca so the
    // DB gate (not deployment readiness) is what blocks after arm.
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    force_halted(&st).await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "must be blocked after halt");

    arm(&st).await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "run/start must still fail closed without DB-backed runtime ownership"
    );
    let json = parse_json(body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "body should explain DB-backed runtime requirement: {json}"
    );
}
