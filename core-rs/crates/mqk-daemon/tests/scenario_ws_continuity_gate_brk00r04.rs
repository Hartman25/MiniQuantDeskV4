//! BRK-00R-04: Proactive WS continuity start gate for paper+alpaca.
//!
//! Proves that `start_execution_runtime` (via POST /v1/run/start) now refuses
//! paper+alpaca startup when Alpaca WS continuity is not start-safe — before any
//! DB operations are attempted.
//!
//! # Gate ordering contract proven here
//!
//! ```text
//! 1. deployment_readiness gate  (paper+alpaca → allowed)
//! 2. integrity gate             (must be armed)
//! 3. WS continuity gate (NEW)   (paper+alpaca, ColdStartUnproven|GapDetected → 403)
//! 4. DB pool gate               (no DB → 503)  — reachable only with Live continuity
//! ```
//!
//! # What remains open after this patch
//!
//! Full WS transport implementation (subscribe / reconnect / cursor establishment).
//! Until the WS transport establishes a Live cursor, paper+alpaca startup is
//! explicitly blocked at gate 3.  This is the correct honest behaviour.
//!
//! All tests are pure in-process (no DB required).

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

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
// BRK00R04-P01 — paper+alpaca + ColdStartUnproven → 403 before DB
//
// The default Alpaca continuity state on boot is ColdStartUnproven.
// Start must be refused with gate=alpaca_ws_continuity before any DB call.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk00r04_p01_cold_start_unproven_blocks_paper_alpaca_start() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    // Arm integrity gate so it is not the blocker.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "P01: arm must succeed");

    // Default continuity is ColdStartUnproven — do not set anything.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "P01: paper+alpaca with ColdStartUnproven must return 403; got: {status}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["gate"], "alpaca_ws_continuity",
        "P01: gate must be alpaca_ws_continuity; got: {json}"
    );
    assert_eq!(
        json["fault_class"], "runtime.start_refused.paper_alpaca_ws_continuity_unproven",
        "P01: fault_class must identify paper+alpaca continuity refusal; got: {json}"
    );
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("WS_CONTINUITY_UNPROVEN"),
        "P01: error must name WS_CONTINUITY_UNPROVEN; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// BRK00R04-P02 — paper+alpaca + GapDetected → 403 before DB
//
// GapDetected is a known-unsafe continuity state (reconnect without replay).
// Start must be refused even when the daemon has previously been running.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk00r04_p02_gap_detected_blocks_paper_alpaca_start() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    // Arm integrity gate.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "P02: arm must succeed");

    // Inject GapDetected continuity state.
    st.update_ws_continuity(state::AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:test-order-id:fill:2026-01-01T00:00:00Z".to_string()),
        last_event_at: Some("2026-01-01T00:00:00Z".to_string()),
        detail: "brk00r04-test: simulated WS reconnect gap".to_string(),
    })
    .await;

    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "P02: paper+alpaca with GapDetected must return 403; got: {status}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["gate"], "alpaca_ws_continuity",
        "P02: gate must be alpaca_ws_continuity; got: {json}"
    );
    assert_eq!(
        json["fault_class"], "runtime.start_refused.paper_alpaca_ws_continuity_unproven",
        "P02: fault_class must identify paper+alpaca continuity refusal; got: {json}"
    );
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("WS_CONTINUITY_UNPROVEN"),
        "P02: error must name WS_CONTINUITY_UNPROVEN; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// BRK00R04-P03 — Integrity gate still fires BEFORE the WS continuity gate
//
// Gate ordering: integrity check is at a higher precedence than continuity.
// Without arming, the integrity gate fires first (403 integrity_armed),
// not the WS continuity gate.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk00r04_p03_integrity_gate_fires_before_continuity_gate() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    // Do NOT arm — integrity gate should fire first.

    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "P03: unaligned integrity must return 403; got: {status}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["gate"], "integrity_armed",
        "P03: gate must be integrity_armed (not alpaca_ws_continuity); got: {json}"
    );
}

// ---------------------------------------------------------------------------
// BRK00R04-P04 — WS continuity gate fires BEFORE the DB gate
//
// Proves the correct enforcement ordering: after arm, the WS continuity gate
// returns 403 before the DB pool gate can return 503.  No DB is configured.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk00r04_p04_continuity_gate_fires_before_db_gate() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));

    // Arm integrity gate so it is not the blocker.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "P04: arm must succeed");

    // No DB configured.  Without BRK-00R-04 this would return 503 (DB gate).
    // With BRK-00R-04 it must return 403 (continuity gate fires first).
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "P04: continuity gate must return 403 before DB gate (503); got: {status}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["gate"], "alpaca_ws_continuity",
        "P04: gate must be alpaca_ws_continuity, not a DB error; got: {json}"
    );
    // Confirm it is NOT the DB error.
    assert!(
        !json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "P04: must not reach DB gate; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// BRK00R04-P05 — live-shadow+alpaca is unaffected by the new gate
//
// The WS continuity gate only applies to Paper+Alpaca.  LiveShadow+Alpaca
// must still fall through to the DB gate (503 without DB).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk00r04_p05_live_shadow_alpaca_unaffected_reaches_db_gate() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));

    // Arm integrity gate.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "P05: arm must succeed");

    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "P05: live-shadow+alpaca without DB must return 503 (DB gate, not continuity gate); got: {status}"
    );
    let json = parse_json(body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "P05: live-shadow must reach DB gate, not continuity gate; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// BRK00R04-P06 — paper+paper still blocked at deployment readiness (unchanged)
//
// PT-TRUTH-01 blocking must not be weakened by BRK-00R-04.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk00r04_p06_paper_paper_still_blocked_at_deployment_readiness() {
    // Default config (paper+paper).
    let st = Arc::new(state::AppState::new());

    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "P06: paper+paper must still return 403 at deployment readiness gate; got: {status}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["gate"], "deployment_mode",
        "P06: gate must be deployment_mode (not alpaca_ws_continuity); got: {json}"
    );
}
