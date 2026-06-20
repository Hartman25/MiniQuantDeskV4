//! OBS-SESSION-DISCORD-01 — Session-window diagnostic fields proof.
//!
//! Proves that `GET /api/v1/autonomous/readiness` exposes enough information for
//! an operator to debug session-window failures without inspecting logs:
//!
//! | Test     | Claim                                                                              |
//! |----------|------------------------------------------------------------------------------------|
//! | OBS-SW-01 | Env-configured fixed window → source="fixed_window_override", raw values, UTC    |
//! | OBS-SW-02 | Env vars absent → source="nyse_weekdays", null raw values, null start/stop UTC   |
//! | OBS-SW-03 | Diagnostic fields do not alter overall_ready, session_in_window, or blockers     |
//! | OBS-SW-04 | now_utc is present and parses as valid RFC 3339                                  |
//! | OBS-SW-05 | Invalid env var (bad format) → source="default", raw shows bad value             |
//! | OBS-SW-06 | not_applicable path (paper+paper) also includes diagnostic fields                |
//!
//! All tests are pure in-process (no DB, no network, no real Discord call).

use std::sync::{Arc, OnceLock};

use axum::{body::Body, http::Request};
use chrono::DateTime;
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use state::{AlpacaWsContinuityState, BrokerKind, StrategyFleetEntry};
use tokio::sync::Mutex;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Env-var serialization — TEST-FLAKE-SESSION-WINDOW-OBS01-ENV-LOCK-01.
//
// OBS-SW-01/05/06 mutate the process-global MQK_SESSION_START_HH_MM /
// MQK_SESSION_STOP_HH_MM env vars that session_window_from_env() reads.
// Without serialization, the default parallel test runner can interleave a
// mutator with OBS-SW-02/03 (which assert the env vars are absent),
// producing nondeterministic failures. Mirrors the same fix applied to
// scenario_event_risk_status_b7b.rs / scenario_premarket_data_readiness_gate_01.rs.
// ---------------------------------------------------------------------------

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn call_readiness(st: Arc<state::AppState>) -> serde_json::Value {
    let router = routes::build_router(st);
    let req = Request::builder()
        .uri("/api/v1/autonomous/readiness")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect")
        .to_bytes();
    serde_json::from_slice(&body).expect("body is not valid JSON")
}

fn make_paper_alpaca() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ))
}

fn make_paper_paper() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Paper,
    ))
}

// Make a paper+alpaca state that is fully ready (all gates pass) given a
// NYSE-regular-session clock already injected by the caller.
async fn make_ready_paper_alpaca(nyse_ts: i64) -> Arc<state::AppState> {
    let st = make_paper_alpaca();
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "obs-sw-msg".to_string(),
        last_event_at: "2026-03-30T14:00:00Z".to_string(),
    })
    .await;
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    st.set_session_clock_ts_for_test(nyse_ts).await;
    st.set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;
    st
}

// Monday 2026-03-30 14:00 UTC = 10:00 ET (NYSE regular session).
fn nyse_regular_ts() -> i64 {
    chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 3, 30, 14, 0, 0)
        .unwrap()
        .timestamp()
}

// ---------------------------------------------------------------------------
// OBS-SW-01: env-configured fixed window → source="env" + start/stop UTC + raw
// ---------------------------------------------------------------------------

/// Proves that when both `MQK_SESSION_START_HH_MM` and `MQK_SESSION_STOP_HH_MM`
/// are set to valid values, the readiness response exposes:
/// - `session_window_source = "fixed_window_override"` (SCSP06)
/// - `session_start_env_raw` and `session_stop_env_raw` matching the env values
/// - `session_start_utc` and `session_stop_utc` as `"HH:MM UTC"` strings
/// - `session_window_basis = "UTC"`
#[tokio::test]
async fn obs_sw_01_env_window_exposes_source_and_derived_times() {
    let _g = env_lock().lock().await;
    // Set env vars for this test only.  Use a very specific window so we can
    // assert exact values without risk of conflict with other tests.
    std::env::set_var("MQK_SESSION_START_HH_MM", "13:30");
    std::env::set_var("MQK_SESSION_STOP_HH_MM", "20:00");

    let st = make_paper_alpaca();
    let v = call_readiness(st).await;

    // Clean up immediately so other tests are not affected.
    std::env::remove_var("MQK_SESSION_START_HH_MM");
    std::env::remove_var("MQK_SESSION_STOP_HH_MM");

    assert_eq!(
        v["session_window_source"], "fixed_window_override",
        "OBS-SW-01: env vars present + valid → session_window_source must be 'fixed_window_override' (SCSP06)"
    );
    assert_eq!(
        v["session_start_env_raw"], "13:30",
        "OBS-SW-01: session_start_env_raw must echo the raw env value"
    );
    assert_eq!(
        v["session_stop_env_raw"], "20:00",
        "OBS-SW-01: session_stop_env_raw must echo the raw env value"
    );
    assert_eq!(
        v["session_start_utc"], "13:30 UTC",
        "OBS-SW-01: session_start_utc must be derived from the fixed window"
    );
    assert_eq!(
        v["session_stop_utc"], "20:00 UTC",
        "OBS-SW-01: session_stop_utc must be derived from the fixed window"
    );
    assert_eq!(
        v["session_window_basis"], "UTC",
        "OBS-SW-01: session_window_basis must always be UTC"
    );
}

// ---------------------------------------------------------------------------
// OBS-SW-02: env vars absent → source="default", null raw values, null times
// ---------------------------------------------------------------------------

/// Proves that when `MQK_SESSION_START_HH_MM` and `MQK_SESSION_STOP_HH_MM` are
/// absent (the default autonomous-session seam uses NYSE regular-session truth),
/// the readiness response exposes:
/// - `session_window_source = "nyse_weekdays"` (SCSP06)
/// - `session_start_env_raw = null`
/// - `session_stop_env_raw = null`
/// - `session_start_utc = null`
/// - `session_stop_utc = null`
#[tokio::test]
async fn obs_sw_02_absent_env_exposes_default_source_and_null_times() {
    let _g = env_lock().lock().await;
    // Guarantee env vars are absent.
    std::env::remove_var("MQK_SESSION_START_HH_MM");
    std::env::remove_var("MQK_SESSION_STOP_HH_MM");

    let st = make_paper_alpaca();
    let v = call_readiness(st).await;

    assert_eq!(
        v["session_window_source"], "nyse_weekdays",
        "OBS-SW-02: absent env vars → session_window_source must be 'nyse_weekdays' (SCSP06)"
    );
    assert!(
        v["session_start_env_raw"].is_null(),
        "OBS-SW-02: session_start_env_raw must be null when env var is absent"
    );
    assert!(
        v["session_stop_env_raw"].is_null(),
        "OBS-SW-02: session_stop_env_raw must be null when env var is absent"
    );
    assert!(
        v["session_start_utc"].is_null(),
        "OBS-SW-02: session_start_utc must be null for NYSE seam (no fixed time)"
    );
    assert!(
        v["session_stop_utc"].is_null(),
        "OBS-SW-02: session_stop_utc must be null for NYSE seam (no fixed time)"
    );
    assert_eq!(
        v["session_window_basis"], "UTC",
        "OBS-SW-02: session_window_basis must be UTC even for default seam"
    );
}

// ---------------------------------------------------------------------------
// OBS-SW-03: diagnostics do not alter overall_ready / session_in_window / blockers
// ---------------------------------------------------------------------------

/// Proves that adding diagnostic fields does not change the readiness verdict.
/// With a fully-ready state (WS=Live, armed, no active run, fleet set, NYSE session),
/// `overall_ready` must still be `true` and `blockers` must still be empty.
#[tokio::test]
async fn obs_sw_03_diagnostics_do_not_alter_readiness_verdict() {
    let _g = env_lock().lock().await;
    std::env::remove_var("MQK_SESSION_START_HH_MM");
    std::env::remove_var("MQK_SESSION_STOP_HH_MM");

    let st = make_ready_paper_alpaca(nyse_regular_ts()).await;
    let v = call_readiness(st).await;

    assert_eq!(
        v["overall_ready"], true,
        "OBS-SW-03: fully-ready state must still yield overall_ready = true"
    );
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    assert!(
        blockers.is_empty(),
        "OBS-SW-03: no blockers expected when overall_ready; got: {blockers:?}"
    );
    assert_eq!(
        v["session_in_window"], true,
        "OBS-SW-03: session_in_window must be true for NYSE regular-session clock"
    );
    // Diagnostic fields must also be present.
    assert!(
        v["now_utc"].is_string(),
        "OBS-SW-03: now_utc must be present"
    );
    assert!(
        v["session_window_source"].is_string(),
        "OBS-SW-03: session_window_source must be present"
    );
    assert!(
        v["session_window_basis"].is_string(),
        "OBS-SW-03: session_window_basis must be present"
    );
}

// ---------------------------------------------------------------------------
// OBS-SW-04: now_utc is present and parses as valid RFC 3339
// ---------------------------------------------------------------------------

/// Proves that `now_utc` is always a valid RFC 3339 timestamp on both the
/// `active` and `not_applicable` paths.
#[tokio::test]
async fn obs_sw_04_now_utc_is_valid_rfc3339() {
    let _g = env_lock().lock().await;
    let st_active = make_paper_alpaca();
    let v_active = call_readiness(st_active).await;
    let active_now = v_active["now_utc"]
        .as_str()
        .expect("OBS-SW-04: now_utc must be a string on active path");
    assert!(
        DateTime::parse_from_rfc3339(active_now).is_ok(),
        "OBS-SW-04: now_utc on active path must parse as RFC 3339; got: {active_now}"
    );

    let st_not = make_paper_paper();
    let v_not = call_readiness(st_not).await;
    let not_now = v_not["now_utc"]
        .as_str()
        .expect("OBS-SW-04: now_utc must be a string on not_applicable path");
    assert!(
        DateTime::parse_from_rfc3339(not_now).is_ok(),
        "OBS-SW-04: now_utc on not_applicable path must parse as RFC 3339; got: {not_now}"
    );
}

// ---------------------------------------------------------------------------
// OBS-SW-05: invalid env value → source="default", raw preserves bad value
// ---------------------------------------------------------------------------

/// Proves that an unparseable env value causes the seam to fall back to the
/// NYSE-default schedule (source="nyse_weekdays"), while still surfacing the raw
/// bad value so the operator can identify the misconfiguration.
#[tokio::test]
async fn obs_sw_05_invalid_env_falls_back_to_default_with_raw_visible() {
    let _g = env_lock().lock().await;
    std::env::set_var("MQK_SESSION_START_HH_MM", "bad-value");
    std::env::set_var("MQK_SESSION_STOP_HH_MM", "20:00");

    let st = make_paper_alpaca();
    let v = call_readiness(st).await;

    std::env::remove_var("MQK_SESSION_START_HH_MM");
    std::env::remove_var("MQK_SESSION_STOP_HH_MM");

    assert_eq!(
        v["session_window_source"], "nyse_weekdays",
        "OBS-SW-05: invalid env value → fallback to 'nyse_weekdays' (SCSP06)"
    );
    assert_eq!(
        v["session_start_env_raw"], "bad-value",
        "OBS-SW-05: session_start_env_raw must preserve the invalid raw value for diagnosis"
    );
    assert_eq!(
        v["session_stop_env_raw"], "20:00",
        "OBS-SW-05: session_stop_env_raw must still echo the raw value"
    );
    assert!(
        v["session_start_utc"].is_null(),
        "OBS-SW-05: session_start_utc must be null when parse fails"
    );
}

// ---------------------------------------------------------------------------
// OBS-SW-06: not_applicable path also includes all diagnostic fields
// ---------------------------------------------------------------------------

/// Proves that even on the `not_applicable` (paper+paper) path, all diagnostic
/// fields are present in the response so the operator can check the session
/// window configuration from any deployment.
#[tokio::test]
async fn obs_sw_06_not_applicable_path_includes_diagnostic_fields() {
    let _g = env_lock().lock().await;
    std::env::set_var("MQK_SESSION_START_HH_MM", "13:30");
    std::env::set_var("MQK_SESSION_STOP_HH_MM", "20:00");

    let st = make_paper_paper();
    let v = call_readiness(st).await;

    std::env::remove_var("MQK_SESSION_START_HH_MM");
    std::env::remove_var("MQK_SESSION_STOP_HH_MM");

    assert_eq!(
        v["truth_state"], "not_applicable",
        "OBS-SW-06: paper+paper must still return not_applicable"
    );
    assert_eq!(
        v["overall_ready"], false,
        "OBS-SW-06: not_applicable must stay overall_ready = false"
    );
    // Diagnostic fields must be present.
    assert!(
        v["now_utc"].is_string(),
        "OBS-SW-06: now_utc must be present on not_applicable path"
    );
    assert_eq!(
        v["session_window_source"], "fixed_window_override",
        "OBS-SW-06: env vars visible even on not_applicable path (SCSP06)"
    );
    assert_eq!(
        v["session_start_env_raw"], "13:30",
        "OBS-SW-06: session_start_env_raw present on not_applicable path"
    );
    assert_eq!(
        v["session_start_utc"], "13:30 UTC",
        "OBS-SW-06: session_start_utc derived even on not_applicable path"
    );
}
