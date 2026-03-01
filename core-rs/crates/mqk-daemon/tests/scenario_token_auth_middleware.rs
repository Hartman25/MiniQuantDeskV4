//! S7-1: Token Auth Middleware
//!
//! Proves that all operator (state-mutating) routes require a valid Bearer
//! token when `AppState::operator_token` is set, while read-only telemetry
//! routes remain accessible without authentication.
//!
//! Five auth properties tested:
//!
//! 1. **Missing auth header on operator route returns 401** — a POST to
//!    `/v1/run/stop` with no `Authorization` header returns `401 Unauthorized`
//!    when a token is configured.  The handler is never reached.
//!
//! 2. **Wrong token on operator route returns 401** — a POST to
//!    `/v1/run/stop` with `Authorization: Bearer wrong-token` returns
//!    `401 Unauthorized`.  Only an exact match of the configured token
//!    is accepted.
//!
//! 3. **Correct token on operator route is allowed** — a POST to
//!    `/v1/run/stop` with the correct Bearer token returns a non-401
//!    response.  The auth middleware passes the request to the handler.
//!
//! 4. **Public route is accessible without token** — `GET /v1/health`
//!    returns `200 OK` without any `Authorization` header, even when a
//!    token is configured.  Telemetry routes are never gated.
//!
//! 5. **No-token-configured mode is fail-open** — when `operator_token`
//!    is `None` (env var `MQK_OPERATOR_TOKEN` not set), operator routes
//!    are accessible without any header.  This is the loopback-only
//!    development posture.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt; // oneshot

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a router with a specific operator token (or `None` for dev mode).
fn make_router(token: Option<&str>) -> axum::Router {
    let st = Arc::new(state::AppState::new_with_token(
        token.map(str::to_owned),
    ));
    routes::build_router(st)
}

/// Drive the router with a single request and return `(status, body_bytes)`.
async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, bytes::Bytes) {
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
// Auth property 1: missing header on operator route returns 401
// ---------------------------------------------------------------------------

/// AUTH 1 of 5.
///
/// `POST /v1/run/stop` without an `Authorization` header must return
/// `401 Unauthorized` with `gate = "operator_token"` when a token is
/// configured.  The handler must never execute.
#[tokio::test]
async fn operator_route_without_auth_header_returns_401_when_token_configured() {
    let router = make_router(Some("secret-token"));

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "operator route without auth header must return 401 when token is configured"
    );

    let json = parse_json(body);
    assert_eq!(
        json["gate"], "operator_token",
        "401 body must name gate = \"operator_token\""
    );
}

// ---------------------------------------------------------------------------
// Auth property 2: wrong token on operator route returns 401
// ---------------------------------------------------------------------------

/// AUTH 2 of 5.
///
/// `POST /v1/run/stop` with `Authorization: Bearer wrong-token` must return
/// `401 Unauthorized`.  Only an exact string match of the configured token
/// is accepted; no prefix, suffix, or case-variation is tolerated.
#[tokio::test]
async fn operator_route_with_wrong_token_returns_401_when_token_configured() {
    let router = make_router(Some("correct-token"));

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        .header("Authorization", "Bearer wrong-token")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "operator route with wrong Bearer token must return 401"
    );

    let json = parse_json(body);
    assert_eq!(
        json["gate"], "operator_token",
        "401 body must name gate = \"operator_token\""
    );
}

// ---------------------------------------------------------------------------
// Auth property 3: correct token on operator route is allowed
// ---------------------------------------------------------------------------

/// AUTH 3 of 5.
///
/// `POST /v1/run/stop` with the exact configured Bearer token must NOT
/// return `401 Unauthorized`.  The auth middleware passes the request to
/// the handler, which returns its own status code (200 in this case).
///
/// This proves the middleware has a valid "accept" path — it is not a
/// deny-all gate.
#[tokio::test]
async fn operator_route_with_correct_token_is_not_401() {
    let router = make_router(Some("my-secret"));

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        .header("Authorization", "Bearer my-secret")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, _body) = call(router, req).await;

    assert_ne!(
        status,
        StatusCode::UNAUTHORIZED,
        "operator route with correct Bearer token must not return 401; got {}",
        status
    );
}

// ---------------------------------------------------------------------------
// Auth property 4: public route is accessible without token
// ---------------------------------------------------------------------------

/// AUTH 4 of 5.
///
/// `GET /v1/health` must return `200 OK` without any `Authorization` header,
/// even when a token is configured.  Read-only telemetry routes are outside
/// the auth middleware layer and must never be gated.
#[tokio::test]
async fn health_route_is_accessible_without_token_even_when_token_configured() {
    let router = make_router(Some("secret-token"));

    let req = Request::builder()
        .method("GET")
        .uri("/v1/health")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "GET /v1/health must return 200 regardless of configured token"
    );

    let json = parse_json(body);
    assert_eq!(json["ok"], true, "health response body must have ok=true");
}

// ---------------------------------------------------------------------------
// Auth property 5: no-token-configured mode is fail-open
// ---------------------------------------------------------------------------

/// AUTH 5 of 5.
///
/// When `operator_token` is `None` (the `MQK_OPERATOR_TOKEN` env var is not
/// set), operator routes must be accessible without any `Authorization`
/// header.  The middleware is a complete no-op in this mode.
///
/// This is the expected behaviour for a daemon bound to the loopback
/// interface only (S7-2), where network isolation is the security layer.
/// A developer running locally should not need a token.
#[tokio::test]
async fn no_token_configured_allows_operator_routes_without_auth() {
    let router = make_router(None); // no token — dev / loopback mode

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, _body) = call(router, req).await;

    assert_ne!(
        status,
        StatusCode::UNAUTHORIZED,
        "operator route must not return 401 when no token is configured; got {}",
        status
    );
}
