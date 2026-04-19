//! # SIGNAL-REFUSAL-OBS-01 — Signal gate refusal observability
//!
//! ## Purpose
//!
//! Proves that every important signal admission refusal now leaves operator-grade
//! diagnostic evidence: the response body carries `signal_id`, `strategy_id`,
//! `symbol`, `disposition`, and `blockers[]` — and the daemon log emits a
//! structured `warn!` for each refusal.
//!
//! These tests cover the pure in-process refusal paths (no DB required).
//! DB-backed paths (Gate 3 arm state, Gate 5 not running, Gate 6 suppressed)
//! are covered by the durable `write_signal_refusal_event` helper which writes
//! `signal.refused` to `audit_events`; those are proven by compilation and the
//! existing JOUR-01 tests that verify the audit_events write path.
//!
//! ## What this file proves
//!
//! | Test   | Gate              | Key claim                                          |
//! |--------|-------------------|----------------------------------------------------|
//! | OBS-01 | validation        | Blank signal_id → 400 + "rejected" + blockers      |
//! | OBS-02 | gate_1_ingestion  | Not configured → 503 + "unavailable" + blocker     |
//! | OBS-03 | gate_1b_ws_cold   | ColdStart → 503 + "unavailable" + cold-start text  |
//! | OBS-04 | gate_1b_ws_gap    | GapDetected → 503 + "continuity_gap" disposition   |
//! | OBS-05 | gate_1c_session   | Outside session → 409 + "outside_session"          |
//!
//! ## What is NOT claimed
//!
//! - DB-backed gate refusals (Gates 2-7 require MQK_DATABASE_URL + run setup)
//! - The `warn!` log text itself (logging infra not wired in unit/scenario tests)
//! - Durable `signal.refused` audit events (require DB + active run_id)
//!
//! All tests are pure in-process.  No `MQK_DATABASE_URL` required.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    routes,
    state::{AlpacaWsContinuityState, AppState, BrokerKind, DeploymentMode},
};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

fn signal_req(body: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

fn valid_signal_body(signal_id: &str) -> serde_json::Value {
    serde_json::json!({
        "signal_id": signal_id,
        "strategy_id": "strat-obs01",
        "symbol": "SPY",
        "side": "buy",
        "qty": 1
    })
}

/// Paper+Alpaca state: ExternalSignalIngestion configured, ColdStartUnproven by default.
fn paper_alpaca_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ))
}

/// Paper+Paper state: no ExternalSignalIngestion → Gate 1 fires.
fn paper_paper_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Paper))
}

/// Premarket Unix timestamp: 2024-01-08 13:00:00 UTC = 08:00 ET.
const NYSE_PREMARKET_TS: i64 = 1_704_697_200;

/// GapDetected state for tests that need it.
fn gap_detected_ws() -> AlpacaWsContinuityState {
    AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:ord-obs01:fill:2026-01-01T00:00:00Z".to_string()),
        last_event_at: Some("2026-01-01T00:00:00Z".to_string()),
        detail: "obs01-test: simulated gap".to_string(),
    }
}

fn live_ws() -> AlpacaWsContinuityState {
    AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:ord-obs01:new:2026-01-01T00:00:00Z".to_string(),
        last_event_at: "2026-01-01T00:00:00Z".to_string(),
    }
}

// ---------------------------------------------------------------------------
// OBS-01: Validation failure → 400 + "rejected" + actionable blockers
// ---------------------------------------------------------------------------
//
// Proves that a malformed signal produces a 400 with disposition="rejected"
// and non-empty blockers that identify what was wrong.  The daemon log now
// also emits a structured warn! at gate="validation" for this path.

#[tokio::test]
async fn obs01_validation_failure_produces_actionable_blockers() {
    let st = paper_alpaca_state();
    let router = routes::build_router(st);

    // Blank signal_id — validation must catch this.
    let body = serde_json::json!({
        "signal_id": "",
        "strategy_id": "strat-obs01",
        "symbol": "SPY",
        "side": "buy",
        "qty": 1
    });

    let (status, json) = call(router, signal_req(body)).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "OBS-01: expect 400");
    assert_eq!(
        json["disposition"], "rejected",
        "OBS-01: disposition must be 'rejected'"
    );
    assert_eq!(json["accepted"], false, "OBS-01: accepted must be false");
    assert_eq!(json["intent_placed"], false, "OBS-01: intent_placed must be false");

    let blockers = json["blockers"].as_array().expect("OBS-01: blockers must be array");
    assert!(!blockers.is_empty(), "OBS-01: blockers must be non-empty for diagnosis");

    let blocker_text = blockers[0].as_str().unwrap_or("");
    assert!(
        blocker_text.contains("signal_id"),
        "OBS-01: blocker must name the field that failed, got: {blocker_text}"
    );
}

// ---------------------------------------------------------------------------
// OBS-02: Gate 1 (ingestion not configured) → 503 + "unavailable"
// ---------------------------------------------------------------------------
//
// Paper+Paper has no ExternalSignalIngestion. Proves Gate 1 fires immediately
// and the blocker text names "not configured" so an operator can act.

#[tokio::test]
async fn obs02_gate1_ingestion_not_configured_is_diagnosable() {
    let st = paper_paper_state();
    let router = routes::build_router(st);

    let (status, json) = call(router, signal_req(valid_signal_body("obs02-sig-001"))).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "OBS-02: expect 503");
    assert_eq!(
        json["disposition"], "unavailable",
        "OBS-02: disposition must be 'unavailable'"
    );
    assert_eq!(json["accepted"], false, "OBS-02: accepted must be false");
    assert_eq!(json["signal_id"], "obs02-sig-001", "OBS-02: signal_id echoed");
    assert_eq!(json["strategy_id"], "strat-obs01", "OBS-02: strategy_id echoed");

    let blockers = json["blockers"].as_array().expect("OBS-02: blockers must be array");
    assert!(!blockers.is_empty(), "OBS-02: blockers must be non-empty");
    let text = blockers[0].as_str().unwrap_or("");
    assert!(
        text.contains("not configured"),
        "OBS-02: blocker must say 'not configured', got: {text}"
    );
}

// ---------------------------------------------------------------------------
// OBS-03: Gate 1b ColdStart → 503 + "unavailable" + "cold start" blocker text
// ---------------------------------------------------------------------------
//
// Paper+Alpaca default state is ColdStartUnproven. Proves the WS cold-start
// refusal path is diagnosable: disposition, signal/strategy identity, and
// the "cold start" explanation all appear in the response.

#[tokio::test]
async fn obs03_gate1b_cold_start_refusal_is_diagnosable() {
    let st = paper_alpaca_state();
    // Default is ColdStartUnproven — no override needed.
    let router = routes::build_router(st);

    let (status, json) = call(router, signal_req(valid_signal_body("obs03-sig-001"))).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "OBS-03: expect 503");
    assert_eq!(
        json["disposition"], "unavailable",
        "OBS-03: disposition must be 'unavailable'"
    );
    assert_eq!(json["accepted"], false, "OBS-03: accepted must be false");
    assert_eq!(json["signal_id"], "obs03-sig-001", "OBS-03: signal_id echoed");
    assert_eq!(json["strategy_id"], "strat-obs01", "OBS-03: strategy_id echoed");

    let blockers = json["blockers"].as_array().expect("OBS-03: blockers must be array");
    assert!(!blockers.is_empty(), "OBS-03: blockers must be non-empty");
    let text = blockers[0].as_str().unwrap_or("");
    assert!(
        text.contains("cold start") || text.contains("ColdStart"),
        "OBS-03: blocker must describe cold start condition, got: {text}"
    );
}

// ---------------------------------------------------------------------------
// OBS-04: Gate 1b GapDetected → 503 + "continuity_gap" disposition
// ---------------------------------------------------------------------------
//
// Proves that a GapDetected WS state produces a uniquely-named disposition
// ("continuity_gap") so an operator can distinguish WS gap refusals from
// other unavailability causes.

#[tokio::test]
async fn obs04_gate1b_gap_detected_disposition_is_unique() {
    let st = paper_alpaca_state();
    st.update_ws_continuity(gap_detected_ws()).await;
    let router = routes::build_router(st);

    let (status, json) = call(router, signal_req(valid_signal_body("obs04-sig-001"))).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "OBS-04: expect 503");
    assert_eq!(
        json["disposition"], "continuity_gap",
        "OBS-04: disposition must be 'continuity_gap' — distinct from generic 'unavailable'"
    );
    assert_eq!(json["accepted"], false, "OBS-04: accepted must be false");
    assert_eq!(json["intent_placed"], false, "OBS-04: intent_placed must be false");

    let blockers = json["blockers"].as_array().expect("OBS-04: blockers must be array");
    assert!(!blockers.is_empty(), "OBS-04: blockers must be non-empty");
    let text = blockers[0].as_str().unwrap_or("");
    assert!(
        text.contains("gap") || text.contains("Gap"),
        "OBS-04: blocker must name the gap condition, got: {text}"
    );
}

// ---------------------------------------------------------------------------
// OBS-05: Gate 1c outside session → 409 + "outside_session" + NYSE text
// ---------------------------------------------------------------------------
//
// Proves that an outside-session refusal names the NYSE market session so an
// operator can understand why signals were dropped on a weekend or holiday.

#[tokio::test]
async fn obs05_gate1c_outside_session_is_diagnosable() {
    let st = paper_alpaca_state();
    // Override WS to Live so Gate 1b passes.
    st.update_ws_continuity(live_ws()).await;
    // Set session clock to premarket (08:00 ET → outside regular session).
    st.set_session_clock_ts_for_test(NYSE_PREMARKET_TS).await;
    let router = routes::build_router(st);

    let (status, json) = call(router, signal_req(valid_signal_body("obs05-sig-001"))).await;

    assert_eq!(status, StatusCode::CONFLICT, "OBS-05: expect 409");
    assert_eq!(
        json["disposition"], "outside_session",
        "OBS-05: disposition must be 'outside_session'"
    );
    assert_eq!(json["accepted"], false, "OBS-05: accepted must be false");

    let blockers = json["blockers"].as_array().expect("OBS-05: blockers must be array");
    assert!(!blockers.is_empty(), "OBS-05: blockers must be non-empty");
    let text = blockers[0].as_str().unwrap_or("");
    assert!(
        text.contains("NYSE") || text.contains("session"),
        "OBS-05: blocker must mention NYSE or session, got: {text}"
    );
    assert!(
        text.contains("regular"),
        "OBS-05: blocker must say 'regular' so operator knows what's expected, got: {text}"
    );
}
