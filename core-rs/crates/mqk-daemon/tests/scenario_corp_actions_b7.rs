//! B7: Corporate-actions / earnings screening — explicit truth contract proof.
//!
//! ## What B7 closes
//!
//! The backtest engine has an explicit `CorporateActionPolicy` (fail-closed halt
//! on forbidden periods, declared per symbol).  The live/paper daemon execution
//! path has **no** equivalent:
//!
//!   - No earnings calendar feed is connected.
//!   - No pre-event position flattening gate exists.
//!   - No ex-dividend price-adjustment ingestion is wired.
//!   - No earnings blackout is enforced at signal admission or order submit.
//!
//! Before B7 neither operator-facing surface (`/api/v1/oms/overview` or
//! `/api/v1/metrics/dashboards`) contained any field describing this gap.  An
//! operator reading those surfaces could not distinguish "corp-actions not
//! wired" from "corp-actions handled".
//!
//! B7 adds `corp_actions_screening: "not_wired"` to both surfaces, following
//! the exact pattern established by B4 (`stop_order_wiring: "not_supported"`).
//!
//! ## Tests
//!
//! - B7-E01: OMS overview `corp_actions_screening` is `"not_wired"` (field present + honest)
//! - B7-E02: Metrics dashboard `corp_actions_screening` is `"not_wired"` (risk panel)
//! - B7-E03: Neither surface omits the field (field must be a string, never null)
//! - B7-E04: B4 `stop_order_wiring` is unaffected — B7 is strictly additive
//! - B7-E05: Corp-actions screening value is stable across two independent calls
//!   (stateless constant — not derived from ephemeral runtime state)
//!
//! All tests are pure in-process and always runnable in CI without environment
//! variables.

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

fn oms_overview_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/oms/overview")
        .body(axum::body::Body::empty())
        .unwrap()
}

fn metrics_dashboards_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/metrics/dashboards")
        .body(axum::body::Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// B7-E01: OMS overview corp_actions_screening is "not_wired"
//
// Proves: the OMS overview surface explicitly labels corp-actions screening
// as absent.  An absent field or a value of "active" / "wired" would be a
// false claim about execution capabilities.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b7_e01_oms_overview_corp_actions_screening_is_not_wired() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, oms_overview_req()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "B7-E01: oms/overview must return 200; got {status}\nbody: {}",
        String::from_utf8_lossy(&body)
    );

    let json = parse_json(body);
    assert_eq!(
        json["corp_actions_screening"], "not_wired",
        "B7-E01: OMS overview corp_actions_screening must be \"not_wired\" — \
         no corp-actions or earnings screening exists on the canonical path; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// B7-E02: Metrics dashboard corp_actions_screening is "not_wired"
//
// Proves: the risk panel of the metrics dashboard also carries the explicit
// corp-actions truth field.  The risk panel is the most natural place an
// operator would look for event-risk coverage — it must not be silent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b7_e02_metrics_dashboard_corp_actions_screening_is_not_wired() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, metrics_dashboards_req()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "B7-E02: metrics/dashboards must return 200; got {status}\nbody: {}",
        String::from_utf8_lossy(&body)
    );

    let json = parse_json(body);
    assert_eq!(
        json["corp_actions_screening"], "not_wired",
        "B7-E02: metrics dashboard corp_actions_screening must be \"not_wired\" — \
         the risk panel must not imply event-risk coverage that does not exist; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// B7-E03: Neither surface omits the field — field must be a string, never null
//
// Proves: the field is always present and always a non-empty string.  A null
// or missing field could be interpreted by tooling as "unknown / possibly
// covered", which is a false operator impression.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b7_e03_corp_actions_screening_field_never_null_on_either_surface() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(Arc::clone(&st));

    // OMS overview
    let (_, overview_body) = call(router, oms_overview_req()).await;
    let overview_json = parse_json(overview_body);
    assert!(
        overview_json["corp_actions_screening"].is_string(),
        "B7-E03: OMS overview corp_actions_screening must be a string, never null; got: {overview_json}"
    );
    assert!(
        !overview_json["corp_actions_screening"]
            .as_str()
            .unwrap_or("")
            .is_empty(),
        "B7-E03: OMS overview corp_actions_screening must be non-empty; got: {overview_json}"
    );

    // Metrics dashboard (fresh router for second call)
    let st2 = Arc::new(state::AppState::new());
    let router2 = routes::build_router(st2);
    let (_, dashboard_body) = call(router2, metrics_dashboards_req()).await;
    let dashboard_json = parse_json(dashboard_body);
    assert!(
        dashboard_json["corp_actions_screening"].is_string(),
        "B7-E03: metrics dashboard corp_actions_screening must be a string, never null; got: {dashboard_json}"
    );
    assert!(
        !dashboard_json["corp_actions_screening"]
            .as_str()
            .unwrap_or("")
            .is_empty(),
        "B7-E03: metrics dashboard corp_actions_screening must be non-empty; got: {dashboard_json}"
    );
}

// ---------------------------------------------------------------------------
// B7-E04: B4 stop_order_wiring is unaffected — B7 is strictly additive
//
// Proves: adding the B7 field did not alter the B4 protection-lane field.
// Both must coexist correctly.  A regression here would mean B7 inadvertently
// changed the protection truth contract.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b7_e04_b4_stop_order_wiring_unaffected_by_b7() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, oms_overview_req()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "B7-E04: oms/overview must return 200"
    );

    let json = parse_json(body);

    // B4 field must still be present and correct.
    assert_eq!(
        json["stop_order_wiring"], "not_supported",
        "B7-E04: B4 stop_order_wiring must remain \"not_supported\" after B7 patch; got: {json}"
    );

    // B7 field must also be present and correct alongside B4.
    assert_eq!(
        json["corp_actions_screening"], "not_wired",
        "B7-E04: corp_actions_screening must be \"not_wired\" alongside B4 field; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// B7-E05: Corp-actions screening value is stable across two calls
//
// Proves: the value is a stateless constant derived from execution-path
// capability, not from ephemeral runtime state.  Two independent calls to
// the same route on the same AppState must return the same value.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b7_e05_corp_actions_screening_is_stateless_constant() {
    let st = Arc::new(state::AppState::new());

    // First call.
    let router1 = routes::build_router(Arc::clone(&st));
    let (_, body1) = call(router1, oms_overview_req()).await;
    let json1 = parse_json(body1);

    // Second call (same state — no mutations between calls).
    let router2 = routes::build_router(Arc::clone(&st));
    let (_, body2) = call(router2, oms_overview_req()).await;
    let json2 = parse_json(body2);

    assert_eq!(
        json1["corp_actions_screening"], json2["corp_actions_screening"],
        "B7-E05: corp_actions_screening must be identical across two calls to the same state; \
         first={}, second={}",
        json1["corp_actions_screening"], json2["corp_actions_screening"]
    );
    assert_eq!(
        json1["corp_actions_screening"], "not_wired",
        "B7-E05: value must be \"not_wired\" on both calls; got: {}",
        json1["corp_actions_screening"]
    );
}
