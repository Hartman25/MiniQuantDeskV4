//! A5B: per-order execution trace route tests.
//!
//! Tests for `GET /api/v1/execution/orders/:order_id/trace`.
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
//! The DB-backed states (no_order, no_fills_yet, active) all require
//! MQK_DATABASE_URL and skip gracefully in CI when it is absent.
//!
//! # What the trace claims
//!
//! - Current OMS identity: symbol, qty, status (ephemeral, from execution snapshot)
//! - Outbox transport status: outbox_status, outbox_lifecycle_stage (ephemeral,
//!   from pending_outbox window in execution snapshot)
//! - Fill events with extended telemetry: fill_qty, fill_price_micros, slippage_bps,
//!   submit_ts_utc, submit_to_fill_ms, side (durable, from fill_quality_telemetry)
//!
//! # What the trace does NOT claim
//!
//! - Pre-fill broker ACK timestamps (not joinable to internal_order_id)
//! - Outbox transition timestamps (PENDING→CLAIMED→SENT history, not in DB by order_id)
//! - Cancel/replace lifecycle events (not sourced here)

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
// TR-01: route is mounted and returns 200 with wrapper shape.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tr01_route_mounted_returns_200_with_wrapper() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/order-abc-123/trace")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK, "should always return 200");
    let v = parse_json(body);
    // Wrapper shape contract
    assert!(v.get("canonical_route").is_some(), "canonical_route field required");
    assert!(v.get("truth_state").is_some(), "truth_state field required");
    assert!(v.get("backend").is_some(), "backend field required");
    assert!(v.get("order_id").is_some(), "order_id field required");
    assert!(v.get("rows").is_some(), "rows field required");
    assert!(v["rows"].is_array(), "rows must be an array");
}

// ---------------------------------------------------------------------------
// TR-02: canonical_route includes the order_id in the path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tr02_canonical_route_includes_order_id() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/my-order-xyz/trace")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    let route = v["canonical_route"].as_str().expect("canonical_route is a string");
    assert!(
        route.contains("my-order-xyz"),
        "canonical_route must include the resolved order_id; got: {route}"
    );
}

// ---------------------------------------------------------------------------
// TR-03: no_db truth_state when DB pool is absent (pure in-process).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tr03_no_db_truth_state_without_db_pool() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/some-order/trace")
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
    let rows = v["rows"].as_array().expect("rows is array");
    assert!(rows.is_empty(), "rows must be empty when truth_state is no_db");
}

// ---------------------------------------------------------------------------
// TR-04: rows array is empty for no_db state — not authoritative-zero.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tr04_rows_empty_not_authoritative_for_no_db() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/order-99/trace")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    // Empty rows under no_db must NOT be treated as "no fills exist".
    // Consumers must check truth_state first.
    assert_eq!(v["truth_state"].as_str(), Some("no_db"));
    assert_eq!(v["rows"].as_array().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// TR-05: blank order_id returns 400 or 404 (routing/handler guard).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tr05_blank_order_id_returns_400_or_404() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/%20/trace") // URL-encoded space → " " → trim → ""
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _body) = call(router, req).await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
        "blank order_id must return 400 or 404, got {status}"
    );
}

// ---------------------------------------------------------------------------
// TR-06: order_id is reflected in the response body.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tr06_order_id_reflected_in_response() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/reflected-order-id/trace")
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
// TR-07: nullable identity fields are present in schema (may be null without snapshot).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tr07_nullable_identity_fields_present_in_schema() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/test-order/trace")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    // These fields must be present in the response (even if null).
    for field in &[
        "broker_order_id",
        "symbol",
        "requested_qty",
        "filled_qty",
        "current_status",
        "current_stage",
        "last_event_at",
        "outbox_status",
        "outbox_lifecycle_stage",
    ] {
        assert!(
            v.get(field).is_some(),
            "response must contain field '{field}'"
        );
    }
}

// ---------------------------------------------------------------------------
// TR-08: outbox fields are null when no execution snapshot (no_db context).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tr08_outbox_fields_null_without_snapshot() {
    let router = make_router_no_db();
    let req = Request::builder()
        .uri("/api/v1/execution/orders/no-snapshot-order/trace")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let v = parse_json(body);
    // Without an execution snapshot, outbox fields must be explicitly null —
    // not absent from the response.
    assert!(
        v["outbox_status"].is_null(),
        "outbox_status must be null without snapshot"
    );
    assert!(
        v["outbox_lifecycle_stage"].is_null(),
        "outbox_lifecycle_stage must be null without snapshot"
    );
}

// ---------------------------------------------------------------------------
// TR-09: trace and timeline are sibling routes (different paths, same order_id).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tr09_trace_and_timeline_are_sibling_routes() {
    let router = make_router_no_db();
    let order_id = "sibling-order-001";

    // Both routes must mount and return 200.
    let (trace_status, trace_body) = call(
        router.clone(),
        Request::builder()
            .uri(format!("/api/v1/execution/orders/{order_id}/trace"))
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    let (timeline_status, timeline_body) = call(
        router,
        Request::builder()
            .uri(format!("/api/v1/execution/orders/{order_id}/timeline"))
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(trace_status, StatusCode::OK);
    assert_eq!(timeline_status, StatusCode::OK);

    let trace_v = parse_json(trace_body);
    let timeline_v = parse_json(timeline_body);

    // Trace canonical_route must end in /trace; timeline in /timeline.
    let trace_route = trace_v["canonical_route"].as_str().unwrap();
    let timeline_route = timeline_v["canonical_route"].as_str().unwrap();
    assert!(trace_route.ends_with("/trace"), "trace canonical_route must end with /trace");
    assert!(timeline_route.ends_with("/timeline"), "timeline canonical_route must end with /timeline");

    // Trace has extra fields not present on timeline.
    assert!(
        trace_v.get("outbox_status").is_some(),
        "trace must have outbox_status field absent from timeline"
    );
    assert!(
        timeline_v.get("outbox_status").is_none(),
        "timeline must NOT have outbox_status field"
    );
}
