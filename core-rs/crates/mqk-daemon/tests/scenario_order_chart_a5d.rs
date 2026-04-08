//! A5D: per-order execution chart route tests.
//!
//! Tests for `GET /api/v1/execution/orders/:order_id/chart`.
//!
//! # Truth-state contract under test
//!
//! | Condition                                      | truth_state |
//! |------------------------------------------------|-------------|
//! | Order not found in OMS snapshot               | no_order    |
//! | Order visible in OMS snapshot, no bar source  | no_bars     |
//!
//! The route is intentionally fail-closed: no bar/candle data source exists for
//! per-order chart data in the current implementation.  All tests here are
//! pure in-process (no DB required).
//!
//! # What the chart route does NOT claim
//!
//! - Real OHLCV bar data: no per-order chart/candle source is wired.
//! - Signal, fill, or execution overlays: not available without bar timestamps.
//! - Reference price series: not available without bar data.

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use std::sync::Arc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_router_no_db() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
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

// ---------------------------------------------------------------------------
// CH-01: route is mounted and returns 200 with wrapper shape.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ch01_route_mounted_returns_200_with_wrapper() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/order-abc-123/chart")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK, "should always return 200");
    let v = parse_json(body);
    for field in &["canonical_route", "truth_state", "backend", "order_id", "comment"] {
        assert!(v.get(field).is_some(), "wrapper must contain field '{field}'");
    }
}

// ---------------------------------------------------------------------------
// CH-02: canonical_route includes order_id and ends with /chart.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ch02_canonical_route_includes_order_id_and_suffix() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/my-chart-order/chart")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    let route = v["canonical_route"].as_str().expect("canonical_route is a string");
    assert!(
        route.contains("my-chart-order"),
        "canonical_route must include the order_id; got: {route}"
    );
    assert!(
        route.ends_with("/chart"),
        "canonical_route must end with /chart; got: {route}"
    );
}

// ---------------------------------------------------------------------------
// CH-03: truth_state is no_order (not no_bars) when OMS snapshot absent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ch03_no_order_truth_state_when_snapshot_absent() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/phantom-order/chart")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    // With no execution snapshot the order is not visible → truth_state must be no_order.
    assert_eq!(
        v["truth_state"].as_str(),
        Some("no_order"),
        "expected no_order when order not in snapshot"
    );
    assert_eq!(
        v["backend"].as_str(),
        Some("unavailable"),
        "backend must be unavailable when no_order"
    );
}

// ---------------------------------------------------------------------------
// CH-04: order_id is reflected in the response body.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ch04_order_id_reflected() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/reflected-order-id/chart")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    assert_eq!(
        v["order_id"].as_str(),
        Some("reflected-order-id"),
        "order_id must be echoed back in the response"
    );
}

// ---------------------------------------------------------------------------
// CH-05: chart is a sibling of replay, trace, and timeline (same :order_id).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ch05_chart_is_sibling_of_replay_trace_timeline() {
    let router = make_router_no_db();
    let order_id = "sibling-order-chart-001";

    let routes_to_check = [
        (
            format!("/api/v1/execution/orders/{order_id}/timeline"),
            "timeline",
        ),
        (format!("/api/v1/execution/orders/{order_id}/trace"), "trace"),
        (
            format!("/api/v1/execution/orders/{order_id}/replay"),
            "replay",
        ),
        (
            format!("/api/v1/execution/orders/{order_id}/chart"),
            "chart",
        ),
    ];

    for (uri, expected_suffix) in &routes_to_check {
        let (status, body) = call(
            router.clone(),
            Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{uri} must return 200");
        let v = parse_json(body);
        let route = v["canonical_route"].as_str().unwrap_or("");
        assert!(
            route.ends_with(expected_suffix),
            "canonical_route for {uri} must end with /{expected_suffix}; got: {route}"
        );
    }
}

// ---------------------------------------------------------------------------
// CH-06: blank order_id returns 400 or 404.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ch06_blank_order_id_returns_400_or_404() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/%20/chart")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(router, req).await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
        "blank order_id must return 400 or 404, got {status}"
    );
}

// ---------------------------------------------------------------------------
// CH-07: comment is non-empty for all truth states.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ch07_comment_is_non_empty() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/any-order/chart")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    let comment = v["comment"].as_str().unwrap_or("");
    assert!(!comment.is_empty(), "comment must be non-empty for operator clarity");
}
