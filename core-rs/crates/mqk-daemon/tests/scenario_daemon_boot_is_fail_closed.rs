//! Scenario: Daemon boot is fail-closed.
//!
//! These tests keep the original integrity fail-closed coverage while proving
//! missing operator auth now fails closed on privileged routes by default.

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
    assert_eq!(json["integrity_armed"], false);
}

#[tokio::test]
async fn production_mode_without_token_refuses_startup_or_operator_access() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::MissingTokenFailClosed,
    ));

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let json = parse_json(body);
    assert_eq!(json["gate"], "operator_auth_config");
}

#[tokio::test]
async fn run_start_returns_403_before_arm_in_explicit_dev_no_token_mode() {
    // PT-TRUTH-01: default config is paper+paper (fail-closed). Use paper+alpaca so
    // the deployment readiness gate passes and the integrity gate is what blocks start.
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), req).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "run/start must still be blocked at boot when integrity was never armed"
    );
    let json = parse_json(body);
    assert!(json["error"]
        .as_str()
        .unwrap_or("")
        .contains("GATE_REFUSED"));
    assert_eq!(json["gate"], "integrity_armed");
}

#[tokio::test]
async fn run_start_requires_db_after_explicit_arm_in_explicit_dev_no_token_mode() {
    // PT-TRUTH-01: default config is paper+paper (fail-closed). Use paper+alpaca so
    // the deployment readiness gate passes and the DB gate is what blocks start.
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK);

    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let json = parse_json(body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "body should explain DB-backed runtime requirement: {json}"
    );
}
