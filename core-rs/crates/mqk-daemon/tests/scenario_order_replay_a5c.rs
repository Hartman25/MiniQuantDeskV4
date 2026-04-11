//! A5C: per-order execution replay route tests.
//!
//! Tests for `GET /api/v1/execution/orders/:order_id/replay`.
//!
//! # Truth-state contract under test
//!
//! | Condition                                      | truth_state   |
//! |------------------------------------------------|---------------|
//! | No DB pool                                     | no_db         |
//! | DB pool + no active run                        | no_order      |
//! | DB pool + active run + order in OMS snapshot   | no_fills_yet  |
//! | DB pool + active run + fill quality rows       | active        |
//!
//! The DB-backed states (no_order, no_fills_yet, active) require MQK_DATABASE_URL
//! and are skipped gracefully in CI when it is absent.
//!
//! # What the replay claims
//!
//! - Durable fill events from `fill_quality_telemetry`, oldest-first.
//! - Cumulative fill qty per frame (not per-event partial qty).
//! - Current-request-time OMS status (ephemeral snapshot; not per-frame history).
//! - Queue status from the in-memory outbox window (ephemeral; `"unknown"` when absent).
//!
//! # What the replay does NOT claim
//!
//! - Pre-fill broker ACK lifecycle events (not joinable to internal_order_id).
//! - Per-frame historical OMS / risk / reconcile states (not reconstructable).
//! - Cancel / replace lifecycle events (not sourced here).

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
// RP-01: route is mounted and returns 200 with wrapper shape.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rp01_route_mounted_returns_200_with_wrapper() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/order-abc-123/replay")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK, "should always return 200");
    let v = parse_json(body);
    // Wrapper shape contract.
    for field in &[
        "canonical_route",
        "truth_state",
        "backend",
        "order_id",
        "replay_id",
        "replay_scope",
        "source",
        "title",
        "current_frame_index",
        "frames",
    ] {
        assert!(
            v.get(field).is_some(),
            "wrapper must contain field '{field}'"
        );
    }
    assert!(v["frames"].is_array(), "frames must be an array");
}

// ---------------------------------------------------------------------------
// RP-02: canonical_route includes the order_id in the path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rp02_canonical_route_includes_order_id() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/my-order-xyz/replay")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    let route = v["canonical_route"]
        .as_str()
        .expect("canonical_route is a string");
    assert!(
        route.contains("my-order-xyz"),
        "canonical_route must include the resolved order_id; got: {route}"
    );
    assert!(
        route.ends_with("/replay"),
        "canonical_route must end with /replay; got: {route}"
    );
}

// ---------------------------------------------------------------------------
// RP-03: no_db truth_state when DB pool is absent (pure in-process).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rp03_no_db_truth_state_without_db_pool() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/some-order/replay")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);
    assert_eq!(
        v["truth_state"].as_str(),
        Some("no_db"),
        "without DB pool truth_state must be no_db"
    );
    assert_eq!(
        v["backend"].as_str(),
        Some("unavailable"),
        "backend must be unavailable when no_db"
    );
    let frames = v["frames"].as_array().expect("frames is array");
    assert!(
        frames.is_empty(),
        "frames must be empty when truth_state is no_db"
    );
}

// ---------------------------------------------------------------------------
// RP-04: order_id and replay_id are reflected in the response body.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rp04_order_id_and_replay_id_reflected() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/reflected-order-id/replay")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    assert_eq!(
        v["order_id"].as_str(),
        Some("reflected-order-id"),
        "order_id must be echoed back in the response"
    );
    assert_eq!(
        v["replay_id"].as_str(),
        Some("reflected-order-id"),
        "replay_id must equal order_id for single-order scope"
    );
}

// ---------------------------------------------------------------------------
// RP-05: replay_scope and source are fixed for single-order fill replay.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rp05_replay_scope_and_source_are_fixed() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/any-order/replay")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    assert_eq!(
        v["replay_scope"].as_str(),
        Some("single_order"),
        "replay_scope must be single_order"
    );
    assert_eq!(
        v["source"].as_str(),
        Some("fill_quality_telemetry"),
        "source must be fill_quality_telemetry"
    );
}

// ---------------------------------------------------------------------------
// RP-06: replay is a sibling of trace and timeline (same :order_id path segment).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rp06_replay_is_sibling_of_trace_and_timeline() {
    let router = make_router_no_db();
    let order_id = "sibling-order-001";

    let routes_to_check = [
        (
            format!("/api/v1/execution/orders/{order_id}/timeline"),
            "timeline",
        ),
        (
            format!("/api/v1/execution/orders/{order_id}/trace"),
            "trace",
        ),
        (
            format!("/api/v1/execution/orders/{order_id}/replay"),
            "replay",
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
// RP-07: blank order_id returns 400 or 404.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rp07_blank_order_id_returns_400_or_404() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/%20/replay")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(router, req).await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
        "blank order_id must return 400 or 404, got {status}"
    );
}

// ---------------------------------------------------------------------------
// RP-08: frames empty not authoritative (must check truth_state).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rp08_empty_frames_require_truth_state_check() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/no-db-order/replay")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    // Empty frames under no_db must NOT be read as "no fills exist."
    // Consumers must gate on truth_state first.
    assert_eq!(v["truth_state"].as_str(), Some("no_db"));
    assert_eq!(v["frames"].as_array().unwrap().len(), 0);
    // current_frame_index is 0 when frames is empty — not an index into real data.
    assert_eq!(v["current_frame_index"].as_u64(), Some(0));
}
