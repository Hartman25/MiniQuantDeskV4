//! EVENT-RISK-01: Dedicated event-risk screening status surface — honest truth contract proof.
//!
//! ## What EVENT-RISK-01 closes
//!
//! B7 added `corp_actions_screening: "not_wired"` to the OMS overview and metrics
//! dashboard aggregate surfaces.  However, there was no dedicated, machine-queryable
//! route for event-risk screening status — analogous to B4's dedicated
//! `/api/v1/execution/protection-status` route for protection-lane truth.
//!
//! EVENT-RISK-01 adds `GET /api/v1/execution/event-risk-status`, surfacing three
//! specific capability fields with honest "not_wired" / "not_connected" values:
//!
//! - `earnings_calendar_feed` — no earnings calendar data source is connected.
//! - `pre_event_flattening` — no pre-event position flattening gate exists.
//! - `signal_admission_gate` — Gate 1h (event-risk blackout, EVENT-RISK-GATE-01)
//!   exists in code and enforces symbol-level blackout periods when
//!   `MQK_EVENT_RISK_BLACKOUT_PATH` is set; `"not_configured"` otherwise.
//!
//! ## Tests
//!
//! - E7B-01: route returns 200 + `truth_state = "not_wired"` when no gate is configured (CI baseline)
//! - E7B-02: all three capability fields carry explicit "not_wired" / "not_connected" / "not_configured" values
//! - E7B-03: all fields are non-empty strings — never null (field presence invariant)
//! - E7B-04: B7 OMS overview `corp_actions_screening` is unaffected — EVENT-RISK-01 is
//!   strictly additive (no regression on the existing B7 disclosure)
//! - E7B-05: `truth_state = "partial"` when Gate 1h is configured (env var set)
//!
//! All tests are pure in-process and always runnable in CI without environment variables.

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

fn event_risk_status_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/execution/event-risk-status")
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

// ---------------------------------------------------------------------------
// E7B-01: route returns 200 + truth_state = "not_wired"
//
// Proves: the route is mounted on the public router (no auth required) and
// returns an honest "not_wired" contract.  A 404 here would mean the route is
// absent; a value of "active" or "wired" would be a false capability claim.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e7b_01_event_risk_status_returns_200_not_wired() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, event_risk_status_req()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "E7B-01: event-risk-status must return 200; got {status}\nbody: {}",
        String::from_utf8_lossy(&body)
    );

    let json = parse_json(body);
    // truth_state is still "not_wired" when no source is configured — the flatten
    // wiring is present in the loop but does not make truth_state "partial" on its
    // own (no source means the evaluator always returns NotConfigured, a no-op).
    assert_eq!(
        json["truth_state"], "not_wired",
        "E7B-01: truth_state must be \"not_wired\" when no gate is configured (CI baseline); got: {json}"
    );
    assert_eq!(
        json["canonical_route"], "/api/v1/execution/event-risk-status",
        "E7B-01: canonical_route must be the stable route path; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// E7B-02: all three capability fields carry explicit "not_wired"/"not_connected" values
//
// Proves: each specific event-risk capability is explicitly labeled absent.
// An absent or null field for any of these could be interpreted as "possibly
// covered", which is a false operator impression.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e7b_02_capability_fields_are_explicitly_absent() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, event_risk_status_req()).await;
    assert_eq!(status, StatusCode::OK, "E7B-02: must return 200");

    let json = parse_json(body);

    assert_eq!(
        json["earnings_calendar_feed"], "not_connected",
        "E7B-02: earnings_calendar_feed must be \"not_connected\" — no calendar source exists; \
         got: {json}"
    );
    // EVENT-RISK-FLATTEN-WIRE-01: flatten wiring is now present in the loop;
    // "wired_no_source" is the honest state when no source is configured.
    assert_eq!(
        json["pre_event_flattening"], "wired_no_source",
        "E7B-02: pre_event_flattening must be \"wired_no_source\" — flatten wiring is in the \
         execution loop but no blackout or earnings source is configured; got: {json}"
    );
    assert_eq!(
        json["signal_admission_gate"], "not_configured",
        "E7B-02: signal_admission_gate must be \"not_configured\" — Gate 1h exists in code \
         but MQK_EVENT_RISK_BLACKOUT_PATH is not set in this test environment; \
         got: {json}"
    );
    // note must be a non-empty explanatory string.
    assert!(
        json["note"].as_str().map(|s| s.len()).unwrap_or(0) > 20,
        "E7B-02: note must be a non-empty explanatory string; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// E7B-03: all fields are non-empty strings — never null
//
// Proves: every field on the response is a non-null, non-empty string.  A null
// or missing field cannot be relied on by tooling to distinguish capability
// states.  The contract must be fully explicit at all times.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e7b_03_all_fields_are_non_null_non_empty_strings() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (_, body) = call(router, event_risk_status_req()).await;
    let json = parse_json(body);

    for field in &[
        "canonical_route",
        "truth_state",
        "earnings_calendar_feed",
        "pre_event_flattening",
        "signal_admission_gate",
        "note",
    ] {
        assert!(
            json[field].is_string(),
            "E7B-03: field {field} must be a string, never null; got: {json}"
        );
        assert!(
            !json[field].as_str().unwrap_or("").is_empty(),
            "E7B-03: field {field} must be non-empty; got: {json}"
        );
    }
}

// ---------------------------------------------------------------------------
// E7B-04: B7 OMS overview corp_actions_screening is unaffected — additive only
//
// Proves: adding the dedicated event-risk-status route did not alter the B7
// aggregate field on the OMS overview.  EVENT-RISK-01 is strictly additive.
// A regression here would mean EVENT-RISK-01 inadvertently changed the B7
// truth contract.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e7b_04_b7_oms_overview_unaffected_by_event_risk_01() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, oms_overview_req()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "E7B-04: oms/overview must return 200"
    );

    let json = parse_json(body);

    // B7 field must still be present and correct.
    assert_eq!(
        json["corp_actions_screening"], "not_wired",
        "E7B-04: B7 corp_actions_screening must remain \"not_wired\" after EVENT-RISK-01 patch; \
         got: {json}"
    );

    // B4 field must also remain unaffected.
    assert_eq!(
        json["stop_order_wiring"], "not_supported",
        "E7B-04: B4 stop_order_wiring must remain \"not_supported\" after EVENT-RISK-01 patch; \
         got: {json}"
    );
}

// ---------------------------------------------------------------------------
// E7B-05: truth_state = "partial" when Gate 1h is configured
//
// Proves: when MQK_EVENT_RISK_BLACKOUT_PATH is set (Gate 1h active), the top-level
// truth_state transitions to "partial" — acknowledging that the admission gate is
// enforcing even though earnings_calendar_feed and pre_event_flattening remain absent.
// A static "not_wired" at this point would mislead an operator who has configured
// the gate.
//
// Run with --test-threads=1 to avoid env-var races between tests.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e7b_05_truth_state_is_partial_when_gate_configured() {
    // Point the env var at any non-empty path; blackout_surface_label only checks
    // that the var is set — actual file content is evaluated at signal-admission time.
    std::env::set_var(
        "MQK_EVENT_RISK_BLACKOUT_PATH",
        "/tmp/mqk_test_e7b05_blackout.json",
    );

    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);
    let (status, body) = call(router, event_risk_status_req()).await;

    std::env::remove_var("MQK_EVENT_RISK_BLACKOUT_PATH");

    assert_eq!(
        status,
        StatusCode::OK,
        "E7B-05: must return 200; got {status}"
    );

    let json = parse_json(body);
    assert_eq!(
        json["truth_state"], "partial",
        "E7B-05: truth_state must be \"partial\" when Gate 1h is configured; got: {json}"
    );
    assert_eq!(
        json["signal_admission_gate"], "configured",
        "E7B-05: signal_admission_gate must be \"configured\" when env var is set; got: {json}"
    );
    // Calendar feed must remain absent — "partial" must not overstate.
    assert_eq!(
        json["earnings_calendar_feed"], "not_connected",
        "E7B-05: earnings_calendar_feed must remain \"not_connected\"; got: {json}"
    );
    // With blackout source configured, flatten wiring is "active" (source present).
    assert_eq!(
        json["pre_event_flattening"], "active",
        "E7B-05: pre_event_flattening must be \"active\" when Gate 1h source is configured; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// E7B-06: earnings_calendar_feed = "operator_file" and truth_state = "partial"
//         when MQK_EARNINGS_CALENDAR_PATH is set (Gate 1h-calendar configured)
//
// Proves: configuring the earnings calendar source alone (without Gate 1h)
// is sufficient to transition earnings_calendar_feed → "operator_file" and
// truth_state → "partial".  Neither requires the other to be configured.
//
// Run with --test-threads=1 to avoid env-var races between tests.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e7b_06_earnings_calendar_feed_is_operator_file_when_configured() {
    // Point the env var at any non-empty path — earnings_calendar_surface_label
    // only checks that the var is set and non-empty; actual file content is
    // evaluated at signal-admission time.
    std::env::set_var(
        "MQK_EARNINGS_CALENDAR_PATH",
        "/tmp/mqk_test_e7b06_earnings_calendar.json",
    );
    // Ensure Gate 1h blackout var is absent so this test proves the earnings
    // calendar path exclusively.
    std::env::remove_var("MQK_EVENT_RISK_BLACKOUT_PATH");

    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);
    let (status, body) = call(router, event_risk_status_req()).await;

    std::env::remove_var("MQK_EARNINGS_CALENDAR_PATH");

    assert_eq!(
        status,
        StatusCode::OK,
        "E7B-06: must return 200; got {status}"
    );

    let json = parse_json(body);
    assert_eq!(
        json["earnings_calendar_feed"], "operator_file",
        "E7B-06: earnings_calendar_feed must be \"operator_file\" when env var is set; got: {json}"
    );
    assert_eq!(
        json["truth_state"], "partial",
        "E7B-06: truth_state must be \"partial\" when earnings calendar is configured; got: {json}"
    );
    // signal_admission_gate must remain not_configured — these are independent sources.
    assert_eq!(
        json["signal_admission_gate"], "not_configured",
        "E7B-06: signal_admission_gate must be \"not_configured\" when only earnings calendar \
         is set; got: {json}"
    );
    // With earnings source configured, flatten wiring is "active" (source present).
    assert_eq!(
        json["pre_event_flattening"], "active",
        "E7B-06: pre_event_flattening must be \"active\" when earnings source is configured; got: {json}"
    );
}
