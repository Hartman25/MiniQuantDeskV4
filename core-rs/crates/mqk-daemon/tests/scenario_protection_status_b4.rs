//! B4: Protective stop / bracket integration — honest truth contract proof.
//!
//! Closes the gap between the `KillSwitchType::MissingProtectiveStop` risk
//! concept and actual broker-backed stop / bracket order wiring.
//!
//! ## What B4 closes
//!
//! The current paper+alpaca canonical path:
//!   - rejects `order_type = "stop"` at submit validation (explicit gate)
//!   - does not wire OCO / OTOCO bracket types to the Alpaca broker adapter
//!   - carries a `KillSwitchType::MissingProtectiveStop` risk kill-switch that
//!     can never be operator-satisfied under the current order type set
//!
//! B4 closes this gap with an explicit `"not_wired"` truth contract rather than
//! a fabricated `"protected"` status.  These tests prove every surface that
//! could imply false protection coverage is honest.
//!
//! ## Tests
//!
//! - B4-P01: `GET /api/v1/execution/protection-status` returns 200 + `truth_state = "not_wired"`
//! - B4-P02: `stop_order_wiring` and `bracket_order_wiring` are both `"not_supported"`
//! - B4-P03: OMS overview `stop_order_wiring` field is `"not_supported"` (protection lane present)
//! - B4-P04: Route-level proof — `POST /api/v1/execution/orders` with `order_type = "stop"` → 400
//!   `disposition = "rejected"` before any DB call; stop order cannot reach broker adapter.
//! - B4-P05: Route-level proof — `POST /api/v1/execution/orders` with `order_type = "trailing_stop"`
//!   → 400 `disposition = "rejected"`; no trailing stop variant can bypass the validator.
//! - B4-P06: `canonical_route` is stable and machine-readable
//!
//! All tests are pure in-process and always runnable in CI without env vars.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

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

fn protection_status_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/execution/protection-status")
        .body(axum::body::Body::empty())
        .unwrap()
}

fn oms_overview_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/oms/overview")
        .body(axum::body::Body::empty())
        .unwrap()
}

fn submit_order_req_with_type(order_type: &str) -> Request<axum::body::Body> {
    let body = serde_json::json!({
        "client_request_id": format!("b4-test-{order_type}"),
        "symbol": "SPY",
        "side": "buy",
        "qty": 10,
        "order_type": order_type,
        "time_in_force": "day",
    });
    Request::builder()
        .method("POST")
        .uri("/api/v1/execution/orders")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&body).expect("json serialize"),
        ))
        .unwrap()
}

// ---------------------------------------------------------------------------
// B4-P01: protection-status route exists, returns 200, truth_state = "not_wired"
//
// Proves: the route is mounted on the public router (no auth required) and
// returns the explicit honest contract.  A 404 here would mean the route is
// absent; a "wired" status would be a false claim.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b4_p01_protection_status_returns_200_not_wired() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, protection_status_req()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "B4-P01: protection-status must return 200; got {status}\nbody: {}",
        String::from_utf8_lossy(&body)
    );

    let json = parse_json(body);
    assert_eq!(
        json["truth_state"], "not_wired",
        "B4-P01: truth_state must be \"not_wired\" — no broker-backed stops; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// B4-P02: both wiring fields are "not_supported"
//
// Proves: stop_order_wiring and bracket_order_wiring explicitly state the
// gap rather than omitting the fields (which could be mistaken for capability).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b4_p02_both_wiring_fields_are_not_supported() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, protection_status_req()).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(
        json["stop_order_wiring"], "not_supported",
        "B4-P02: stop_order_wiring must be \"not_supported\"; got: {json}"
    );
    assert_eq!(
        json["bracket_order_wiring"], "not_supported",
        "B4-P02: bracket_order_wiring must be \"not_supported\"; got: {json}"
    );
    // note must be a non-empty string explaining the gap.
    assert!(
        json["note"].as_str().map(|s| s.len()).unwrap_or(0) > 20,
        "B4-P02: note must be a non-empty explanatory string; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// B4-P03: OMS overview carries stop_order_wiring = "not_supported"
//
// Proves: the existing OMS overview surface (relied on by operator dashboards)
// now includes the protection lane field.  An absent or null field here would
// mean the OMS could be mistaken for a protected execution environment.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b4_p03_oms_overview_includes_stop_order_wiring_not_supported() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, oms_overview_req()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "B4-P03: oms/overview must return 200; got {status}"
    );

    let json = parse_json(body);
    assert_eq!(
        json["stop_order_wiring"], "not_supported",
        "B4-P03: OMS overview stop_order_wiring must be \"not_supported\"; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// B4-P04: route-level proof — POST /api/v1/execution/orders with order_type = "stop"
//         returns 400 "rejected" before any DB call.
//
// Proves: validate_manual_order_submit (execution.rs:808) fires before the DB
// path; a stop order cannot reach the broker adapter even without a running
// DB.  No env vars required — validation is pure and short-circuits at the
// route handler before the first db.as_ref() check.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b4_p04_submit_validation_rejects_stop_order_type() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, submit_order_req_with_type("stop")).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "B4-P04: stop order must be rejected 400 before DB; got {status}\nbody: {}",
        String::from_utf8_lossy(&body)
    );

    let json = parse_json(body);
    assert_eq!(
        json["disposition"], "rejected",
        "B4-P04: disposition must be \"rejected\"; got: {json}"
    );
    assert_eq!(
        json["accepted"], false,
        "B4-P04: accepted must be false; got: {json}"
    );
    let blockers = json["blockers"].as_array().expect("B4-P04: blockers must be array");
    assert!(
        blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("order_type")),
        "B4-P04: blockers must name order_type as the rejection reason; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// B4-P05: route-level proof — POST /api/v1/execution/orders with
//         order_type = "trailing_stop" returns 400 "rejected".
//
// Belt-and-suspenders: trailing stop variant also blocked before DB.
// No trailing stop path exists on the current canonical execution path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b4_p05_submit_validation_rejects_trailing_stop_order_type() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, submit_order_req_with_type("trailing_stop")).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "B4-P05: trailing_stop order must be rejected 400 before DB; got {status}\nbody: {}",
        String::from_utf8_lossy(&body)
    );

    let json = parse_json(body);
    assert_eq!(
        json["disposition"], "rejected",
        "B4-P05: disposition must be \"rejected\"; got: {json}"
    );
    assert_eq!(
        json["accepted"], false,
        "B4-P05: accepted must be false; got: {json}"
    );
    let blockers = json["blockers"].as_array().expect("B4-P05: blockers must be array");
    assert!(
        blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("order_type")),
        "B4-P05: blockers must name order_type as the rejection reason; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// B4-P06: canonical_route is stable and machine-readable
//
// Proves: the route self-identifies with a stable canonical_route field so
// tooling can distinguish this response from a generic 404 JSON body.
// Canonical route must not change between daemon restarts.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b4_p06_canonical_route_is_stable() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, protection_status_req()).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);
    assert_eq!(
        json["canonical_route"], "/api/v1/execution/protection-status",
        "B4-P06: canonical_route must be the stable route path; got: {json}"
    );
}
