//! RT-03R: Token auth middleware
//!
//! Proves that privileged daemon routes fail closed unless a valid operator
//! auth mode is selected explicitly.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt; // oneshot

fn make_router(operator_auth: state::OperatorAuthMode) -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(operator_auth));
    routes::build_router(st)
}

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
async fn production_mode_without_token_refuses_startup_or_operator_access() {
    let router = make_router(state::OperatorAuthMode::MissingTokenFailClosed);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "missing operator token must fail closed on privileged routes"
    );

    let json = parse_json(body);
    assert_eq!(json["gate"], "operator_auth_config");
}

#[tokio::test]
async fn explicit_dev_no_token_mode_is_allowed_and_clearly_marked() {
    let state = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    assert_eq!(state.operator_auth_mode().label(), "explicit_dev_no_token");

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, _body) = call(routes::build_router(state), req).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "explicit dev no-token mode should allow privileged routes locally"
    );
}

#[tokio::test]
async fn invalid_token_is_rejected_on_privileged_routes() {
    let router = make_router(state::OperatorAuthMode::TokenRequired(
        "correct-token".to_string(),
    ));

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        .header("Authorization", "Bearer wrong-token")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let json = parse_json(body);
    assert_eq!(json["gate"], "operator_token");
}

#[tokio::test]
async fn missing_token_is_rejected_on_privileged_routes() {
    let router = make_router(state::OperatorAuthMode::TokenRequired(
        "secret-token".to_string(),
    ));

    let req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/disarm")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let json = parse_json(body);
    assert_eq!(json["gate"], "operator_token");
}

#[tokio::test]
async fn read_only_routes_follow_existing_auth_policy() {
    let router = make_router(state::OperatorAuthMode::MissingTokenFailClosed);

    let health_req = Request::builder()
        .method("GET")
        .uri("/v1/health")
        .body(axum::body::Body::empty())
        .unwrap();
    let (health_status, health_body) = call(router.clone(), health_req).await;
    assert_eq!(health_status, StatusCode::OK);
    assert_eq!(parse_json(health_body)["ok"], true);

    let trading_req = Request::builder()
        .method("GET")
        .uri("/v1/trading/account")
        .body(axum::body::Body::empty())
        .unwrap();
    let (trading_status, trading_body) = call(router, trading_req).await;
    assert_eq!(trading_status, StatusCode::OK);
    assert_eq!(parse_json(trading_body)["snapshot_state"], "no_snapshot");
}

#[tokio::test]
async fn control_routes_do_not_bypass_auth_policy() {
    let router = make_router(state::OperatorAuthMode::TokenRequired(
        "control-secret".to_string(),
    ));

    let req = Request::builder()
        .method("POST")
        .uri("/control/arm")
        .header("Authorization", "Bearer wrong-token")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let json = parse_json(body);
    assert_eq!(json["gate"], "operator_token");
}

#[tokio::test]
async fn control_surface_has_no_detached_shadow_mount() {
    let router = make_router(state::OperatorAuthMode::TokenRequired(
        "control-secret".to_string(),
    ));

    let canonical = Request::builder()
        .method("GET")
        .uri("/control/status")
        .header("Authorization", "Bearer control-secret")
        .body(axum::body::Body::empty())
        .unwrap();
    let (canonical_status, _canonical_body) = call(router.clone(), canonical).await;
    assert_ne!(canonical_status, StatusCode::NOT_FOUND);

    let shadow = Request::builder()
        .method("GET")
        .uri("/v1/control/status")
        .header("Authorization", "Bearer control-secret")
        .body(axum::body::Body::empty())
        .unwrap();
    let (shadow_status, _shadow_body) = call(router, shadow).await;
    assert_eq!(shadow_status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// AUTH-OPS-01: focused fail-closed auth proofs
// ---------------------------------------------------------------------------

/// A valid Bearer token allows passage through the auth middleware on
/// privileged routes (the route handler itself then responds, not the auth
/// layer).
#[tokio::test]
async fn correct_token_passes_privileged_route() {
    let router = make_router(state::OperatorAuthMode::TokenRequired(
        "correct-secret".to_string(),
    ));

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        .header("Authorization", "Bearer correct-secret")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, _body) = call(router, req).await;

    // Auth passes; the route handler responds — must not be 401 or 503.
    assert_ne!(
        status,
        StatusCode::UNAUTHORIZED,
        "correct token must not be rejected by auth middleware"
    );
    assert_ne!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "correct token must not be blocked by missing-token gate"
    );
}

/// A raw token value sent without the "Bearer " prefix is malformed and must
/// be rejected — the daemon does not strip or guess the scheme.
#[tokio::test]
async fn malformed_header_no_bearer_prefix_is_rejected() {
    let router = make_router(state::OperatorAuthMode::TokenRequired(
        "my-secret".to_string(),
    ));

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        // Missing "Bearer " scheme prefix — raw token only
        .header("Authorization", "my-secret")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "token without Bearer prefix must be rejected"
    );
    assert_eq!(parse_json(body)["gate"], "operator_token");
}

/// An empty value after "Bearer " (whitespace-only) must be rejected; the
/// middleware strips the prefix and compares the remainder, so an empty
/// remainder never matches a configured token.
#[tokio::test]
async fn empty_bearer_value_is_rejected() {
    let router = make_router(state::OperatorAuthMode::TokenRequired(
        "real-secret".to_string(),
    ));

    let req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .header("Authorization", "Bearer ")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "empty Bearer value must be rejected"
    );
    assert_eq!(parse_json(body)["gate"], "operator_token");
}

/// The auth middleware covers BOTH the legacy /v1/ namespace and the
/// /api/v1/ operator routes (ops/action, strategy/signal).
/// MissingTokenFailClosed must produce 503 on all of them.
#[tokio::test]
async fn api_v1_operator_routes_fail_closed_without_token() {
    let router = make_router(state::OperatorAuthMode::MissingTokenFailClosed);

    for uri in &["/api/v1/ops/action", "/api/v1/strategy/signal"] {
        let req = Request::builder()
            .method("POST")
            .uri(*uri)
            .body(axum::body::Body::empty())
            .unwrap();

        let (status, body) = call(router.clone(), req).await;

        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "api/v1 operator route {} must fail closed under MissingTokenFailClosed",
            uri
        );
        assert_eq!(
            parse_json(body)["gate"],
            "operator_auth_config",
            "route {} must report operator_auth_config gate",
            uri
        );
    }
}
