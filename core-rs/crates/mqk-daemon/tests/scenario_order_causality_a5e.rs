//! A5E: per-order execution causality route tests.
//!
//! Tests for `GET /api/v1/execution/orders/:order_id/causality`.
//!
//! # Truth-state contract under test
//!
//! | Condition                                         | truth_state   |
//! |---------------------------------------------------|---------------|
//! | No DB pool                                        | no_db         |
//! | DB pool + no active run                           | no_order      |
//! | DB pool + active run + order visible, no fills    | no_fills_yet  |
//! | DB pool + active run + fill quality rows exist    | partial       |
//!
//! The DB-backed states require `MQK_DATABASE_URL` and are skipped gracefully
//! in CI when it is absent.
//!
//! # What the causality route claims
//!
//! - `"partial"` (never `"active"`): only the execution-fill lane is proven.
//! - Nodes are derived from `fill_quality_telemetry`, oldest-first.
//! - `proven_lanes`: `["execution_fill"]` when fills exist; `[]` otherwise.
//! - `unproven_lanes`: always lists signal/intent/broker_ack/risk/reconcile/portfolio.
//!
//! # What the causality route does NOT claim
//!
//! - Signal, intent, or risk provenance: not joinable to internal_order_id.
//! - Broker ACK events: not joinable in current schema.
//! - Portfolio effects or reconcile outcomes: not linked per-order.

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
// CA-01: route is mounted and returns 200 with wrapper shape.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca01_route_mounted_returns_200_with_wrapper() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/order-abc-123/causality")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK, "should always return 200");
    let v = parse_json(body);
    for field in &[
        "canonical_route",
        "truth_state",
        "backend",
        "order_id",
        "proven_lanes",
        "unproven_lanes",
        "nodes",
        "comment",
    ] {
        assert!(
            v.get(field).is_some(),
            "wrapper must contain field '{field}'"
        );
    }
    assert!(v["nodes"].is_array(), "nodes must be an array");
    assert!(
        v["proven_lanes"].is_array(),
        "proven_lanes must be an array"
    );
    assert!(
        v["unproven_lanes"].is_array(),
        "unproven_lanes must be an array"
    );
}

// ---------------------------------------------------------------------------
// CA-02: canonical_route includes order_id and ends with /causality.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca02_canonical_route_includes_order_id_and_suffix() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/my-causal-order/causality")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    let route = v["canonical_route"]
        .as_str()
        .expect("canonical_route is a string");
    assert!(
        route.contains("my-causal-order"),
        "canonical_route must include the order_id; got: {route}"
    );
    assert!(
        route.ends_with("/causality"),
        "canonical_route must end with /causality; got: {route}"
    );
}

// ---------------------------------------------------------------------------
// CA-03: no_db truth_state when DB pool is absent (pure in-process).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca03_no_db_truth_state_without_db_pool() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/some-order/causality")
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
    let nodes = v["nodes"].as_array().expect("nodes is array");
    assert!(
        nodes.is_empty(),
        "nodes must be empty when truth_state is no_db"
    );
}

// ---------------------------------------------------------------------------
// CA-04: unproven_lanes are always present and list the expected subsystems.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca04_unproven_lanes_always_present() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/any-order/causality")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    let unproven = v["unproven_lanes"]
        .as_array()
        .expect("unproven_lanes is array");
    let unproven_strs: Vec<&str> = unproven.iter().filter_map(|v| v.as_str()).collect();
    for expected in &[
        "signal",
        "intent",
        "broker_ack",
        "risk",
        "reconcile",
        "portfolio",
    ] {
        assert!(
            unproven_strs.contains(expected),
            "unproven_lanes must include '{expected}'; got: {unproven_strs:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// CA-05: no_db means proven_lanes is empty (nothing proven without data).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca05_no_db_means_proven_lanes_empty() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/no-db-order/causality")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    assert_eq!(v["truth_state"].as_str(), Some("no_db"));
    let proven = v["proven_lanes"].as_array().expect("proven_lanes is array");
    assert!(proven.is_empty(), "proven_lanes must be empty when no_db");
}

// ---------------------------------------------------------------------------
// CA-06: causality is a sibling of chart/replay/trace (same :order_id segment).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca06_causality_is_sibling_of_chart_replay_trace() {
    let router = make_router_no_db();
    let order_id = "sibling-order-causality-001";

    let routes_to_check = [
        (
            format!("/api/v1/execution/orders/{order_id}/replay"),
            "replay",
        ),
        (
            format!("/api/v1/execution/orders/{order_id}/chart"),
            "chart",
        ),
        (
            format!("/api/v1/execution/orders/{order_id}/causality"),
            "causality",
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
// CA-07: blank order_id returns 400 or 404.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca07_blank_order_id_returns_400_or_404() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/%20/causality")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(router, req).await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
        "blank order_id must return 400 or 404, got {status}"
    );
}

// ---------------------------------------------------------------------------
// CA-08: "partial" truth state is not the same as "active" (partial is honest).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca08_no_db_truth_state_is_not_active_or_partial() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/some-order/causality")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    let ts = v["truth_state"].as_str().unwrap_or("");
    assert_ne!(
        ts, "active",
        "causality never reports truth_state=active; it is always partial or narrower"
    );
    assert_ne!(
        ts, "partial",
        "without DB partial causality is impossible; must be no_db"
    );
}
