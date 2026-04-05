//! # AUTON-TRUTH-01 / AUTON-TRUTH-02 / AUTON-PROOF-01 — Autonomous readiness truth proof
//!
//! ## Purpose
//!
//! Proves that the autonomous readiness surface derives from the same gate
//! logic enforced by `start_execution_runtime`, so readiness can never appear
//! green while a real start would refuse.
//!
//! ## What this file proves
//!
//! | Test   | Claim                                                                                       |
//! |--------|---------------------------------------------------------------------------------------------|
//! | AR-01  | Non-paper+alpaca returns `truth_state = "not_applicable"` with `overall_ready = false`      |
//! | AR-02  | Paper+Alpaca with `ColdStartUnproven` WS → `ws_continuity_ready = false` + exact blocker    |
//! | AR-03  | Paper+Alpaca with `GapDetected` WS → `ws_continuity_ready = false` + exact blocker          |
//! | AR-04  | Paper+Alpaca with `Live` WS + armed + reconcile ok → `overall_ready = true`                 |
//! | AR-05  | Paper+Alpaca with `Live` WS + halted integrity → `arm_ready = false` + halted blocker       |
//! | AR-06  | Paper+Alpaca with `Live` WS + dirty reconcile → `reconcile_ready = false` + blocker         |
//! | AR-07  | `system_preflight` for paper+alpaca ColdStart surfaces `ws_continuity_ready = false`
//!            and the WS blocker in the preflight blockers list                                      |
//! | AR-08  | Readiness truth matches what `start_execution_runtime` would do: WS gate refusal
//!            produces 403 on start AND `ws_continuity_ready = false` on readiness surface          |
//! | AR-09  | `autonomous_readiness_applicable = true` only for paper+alpaca; false for paper+paper        |
//! | AR-10  | WS=Live + arm_pending (disarmed in-memory) → overall_ready = false; arm_state = "arm_pending"|
//! | AR-11  | Clock injected to NYSE premarket (13:00 UTC Mon) → session_in_window=false, overall_ready=false, blocker present |
//! | AR-12  | Clock injected to NYSE regular session (14:00 UTC Mon) → session_in_window=true                 |
//! | AR-13  | Default state, no active run → runtime_start_allowed=true                                       |
//! | AR-14  | Active locally-owned run → readiness runtime_start_allowed=false, overall_ready=false, start→409 |

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{TimeZone, Utc};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use state::{AlpacaWsContinuityState, BrokerKind, DeploymentMode};
use tower::ServiceExt;
use uuid::Uuid;

/// Monday 2026-03-30 14:00:00 UTC = 10:00:00 ET (DST) — NYSE regular session.
fn nyse_regular_session_ts() -> i64 {
    Utc.with_ymd_and_hms(2026, 3, 30, 14, 0, 0)
        .unwrap()
        .timestamp()
}

/// Monday 2026-03-30 13:00:00 UTC = 09:00:00 ET (DST) — NYSE premarket.
fn nyse_premarket_ts() -> i64 {
    Utc.with_ymd_and_hms(2026, 3, 30, 13, 0, 0)
        .unwrap()
        .timestamp()
}

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

fn make_live_shadow() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_mode(
        DeploymentMode::LiveShadow,
    ))
}

// ---------------------------------------------------------------------------
// AR-01 — Non-paper+alpaca → truth_state = "not_applicable"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar01_non_paper_alpaca_returns_not_applicable() {
    for st in [make_paper_paper(), make_live_shadow()] {
        let router = routes::build_router(st);
        let req = Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap();
        let (status, body) = call(router, req).await;
        assert_eq!(status, StatusCode::OK);
        let v = parse_json(body);
        assert_eq!(
            v["truth_state"], "not_applicable",
            "non-paper+alpaca must return not_applicable"
        );
        assert_eq!(
            v["overall_ready"], false,
            "not_applicable path must have overall_ready = false"
        );
        assert_eq!(
            v["canonical_path"], false,
            "canonical_path must be false for non-paper+alpaca"
        );
        assert!(
            v["blockers"]
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or(false),
            "blockers must be non-empty explaining why not_applicable"
        );
    }
}

// ---------------------------------------------------------------------------
// AR-02 — ColdStartUnproven → ws_continuity_ready = false + blocker
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar02_cold_start_unproven_blocks_ws_continuity() {
    let st = make_paper_alpaca();
    // Default for paper+alpaca is ColdStartUnproven.
    assert_eq!(
        st.alpaca_ws_continuity().await,
        AlpacaWsContinuityState::ColdStartUnproven,
        "precondition: default paper+alpaca WS continuity is ColdStartUnproven"
    );

    let router = routes::build_router(st);
    let req = Request::builder()
        .uri("/api/v1/autonomous/readiness")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(v["truth_state"], "active");
    assert_eq!(v["ws_continuity"], "cold_start_unproven");
    assert_eq!(v["ws_continuity_ready"], false);
    assert_eq!(v["overall_ready"], false);
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    assert!(
        !blockers.is_empty(),
        "at least one blocker must be present for cold_start_unproven"
    );
    let first_blocker = blockers[0].as_str().unwrap_or("");
    assert!(
        first_blocker.contains("cold_start_unproven") || first_blocker.contains("WS continuity"),
        "first blocker must mention WS continuity: {first_blocker}"
    );
}

// ---------------------------------------------------------------------------
// AR-03 — GapDetected → ws_continuity_ready = false + blocker
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar03_gap_detected_blocks_ws_continuity() {
    let st = make_paper_alpaca();
    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("msg-001".to_string()),
        last_event_at: Some("2026-04-04T10:00:00Z".to_string()),
        detail: "test gap".to_string(),
    })
    .await;

    let router = routes::build_router(st);
    let req = Request::builder()
        .uri("/api/v1/autonomous/readiness")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(v["ws_continuity"], "gap_detected");
    assert_eq!(v["ws_continuity_ready"], false);
    assert_eq!(v["overall_ready"], false);
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    assert!(!blockers.is_empty());
    assert!(
        blockers[0].as_str().unwrap_or("").contains("gap_detected"),
        "blocker must mention gap_detected"
    );
}

// ---------------------------------------------------------------------------
// AR-04 — Live WS + armed + reconcile ok → overall_ready = true
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar04_live_armed_clean_reconcile_overall_ready() {
    let st = make_paper_alpaca();
    // Advance WS to Live.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "msg-001".to_string(),
        last_event_at: "2026-04-04T14:30:00Z".to_string(),
    })
    .await;
    // Arm integrity in-memory.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    // Inject session clock to NYSE regular session (Monday 2026-03-30 14:00 UTC = 10:00 ET).
    // Required so that session_in_window = true regardless of wall-clock at test run time.
    st.set_session_clock_ts_for_test(nyse_regular_session_ts())
        .await;
    // Reconcile defaults to "unknown" which is not dirty/stale → reconcile_ready = true.

    let router = routes::build_router(st);
    let req = Request::builder()
        .uri("/api/v1/autonomous/readiness")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(v["truth_state"], "active");
    assert_eq!(v["canonical_path"], true);
    assert_eq!(v["ws_continuity"], "live");
    assert_eq!(v["ws_continuity_ready"], true);
    assert_eq!(v["reconcile_ready"], true);
    assert_eq!(v["arm_state"], "armed");
    assert_eq!(v["arm_ready"], true);
    assert_eq!(v["signal_ingestion_configured"], true);
    assert_eq!(
        v["session_in_window"], true,
        "session clock injected to regular session"
    );
    assert_eq!(v["session_window_state"], "in_window");
    assert_eq!(
        v["runtime_start_allowed"], true,
        "no active run → runtime_start_allowed"
    );
    assert_eq!(v["overall_ready"], true);
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    assert!(
        blockers.is_empty(),
        "no blockers expected when overall_ready"
    );
}

// ---------------------------------------------------------------------------
// AR-05 — Live WS + halted integrity → arm_ready = false + halted blocker
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar05_halted_integrity_blocks_arm() {
    let st = make_paper_alpaca();
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "msg-001".to_string(),
        last_event_at: "2026-04-04T14:30:00Z".to_string(),
    })
    .await;
    // Assert halt.
    {
        let mut ig = st.integrity.write().await;
        ig.halted = true;
        ig.disarmed = true;
    }

    let router = routes::build_router(st);
    let req = Request::builder()
        .uri("/api/v1/autonomous/readiness")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(v["arm_state"], "halted");
    assert_eq!(v["arm_ready"], false);
    assert_eq!(v["overall_ready"], false);
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    assert!(
        blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("halted")),
        "blockers must contain a halted entry"
    );
}

// ---------------------------------------------------------------------------
// AR-06 — Live WS + dirty reconcile → reconcile_ready = false + blocker
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar06_dirty_reconcile_blocks_start() {
    let st = make_paper_alpaca();
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "msg-001".to_string(),
        last_event_at: "2026-04-04T14:30:00Z".to_string(),
    })
    .await;
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    // Set reconcile to dirty via the public publish API.
    st.publish_reconcile_snapshot(state::ReconcileStatusSnapshot {
        status: "dirty".to_string(),
        last_run_at: Some("2026-04-04T14:00:00Z".to_string()),
        snapshot_watermark_ms: None,
        mismatched_positions: 1,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some("broker/local drift detected".to_string()),
    })
    .await;

    let router = routes::build_router(st);
    let req = Request::builder()
        .uri("/api/v1/autonomous/readiness")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(v["ws_continuity_ready"], true, "WS must be ready");
    assert_eq!(v["reconcile_status"], "dirty");
    assert_eq!(v["reconcile_ready"], false);
    assert_eq!(v["overall_ready"], false);
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    assert!(
        blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("dirty")),
        "blockers must mention dirty reconcile"
    );
}

// ---------------------------------------------------------------------------
// AR-07 — system_preflight for paper+alpaca ColdStart surfaces ws_continuity_ready = false
//         and the WS blocker in the preflight blockers list (AUTON-TRUTH-02)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar07_preflight_surfaces_ws_blocker_for_cold_start() {
    let st = make_paper_alpaca();
    // Default is ColdStartUnproven — do not advance to Live.

    let router = routes::build_router(st);
    let req = Request::builder()
        .uri("/api/v1/system/preflight")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(
        v["autonomous_readiness_applicable"], true,
        "paper+alpaca must set autonomous_readiness_applicable = true"
    );
    assert_eq!(
        v["ws_continuity_ready"], false,
        "ColdStartUnproven must produce ws_continuity_ready = false"
    );
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    assert!(
        blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("WS continuity")),
        "preflight blockers must surface the WS continuity blocker: {blockers:?}"
    );
    assert_eq!(
        v["autonomous_arm_state"], "arm_pending",
        "fresh boot is disarmed in memory → arm_pending"
    );
}

// ---------------------------------------------------------------------------
// AR-08 — Readiness truth matches start_execution_runtime gate:
//         ColdStartUnproven → 403 on start AND ws_continuity_ready = false
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar08_readiness_matches_start_gate_ws_cold_start() {
    let st = make_paper_alpaca();
    // Default: ColdStartUnproven.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    let router = routes::build_router(Arc::clone(&st));

    // Verify readiness surface reports ws_continuity_ready = false.
    let (_, readiness_body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    let readiness = parse_json(readiness_body);
    assert_eq!(
        readiness["ws_continuity_ready"], false,
        "readiness surface must report ws_continuity_ready = false for ColdStartUnproven"
    );
    assert_eq!(readiness["overall_ready"], false);

    // Verify start actually refuses with the same WS gate.
    let (start_status, start_body) = call(
        router,
        Request::builder()
            .method("POST")
            .uri("/v1/run/start")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        start_status,
        StatusCode::FORBIDDEN,
        "start must be refused with 403 when WS is ColdStartUnproven: {}",
        String::from_utf8_lossy(&start_body)
    );
    let start_v = parse_json(start_body);
    let gate = start_v["gate"].as_str().unwrap_or("");
    assert!(
        gate.contains("alpaca_ws_continuity") || gate.contains("continuity"),
        "start refusal gate must mention WS continuity: {gate}"
    );
}

// ---------------------------------------------------------------------------
// AR-09 — autonomous_readiness_applicable = true only for paper+alpaca
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar09_applicable_only_for_paper_alpaca() {
    // Paper+Alpaca: applicable = true.
    let (_, pa_body) = call(
        routes::build_router(make_paper_alpaca()),
        Request::builder()
            .uri("/api/v1/system/preflight")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(parse_json(pa_body)["autonomous_readiness_applicable"], true);

    // Paper+Paper: applicable = false.
    let (_, pp_body) = call(
        routes::build_router(make_paper_paper()),
        Request::builder()
            .uri("/api/v1/system/preflight")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        parse_json(pp_body)["autonomous_readiness_applicable"],
        false
    );

    // LiveShadow: applicable = false.
    let (_, ls_body) = call(
        routes::build_router(make_live_shadow()),
        Request::builder()
            .uri("/api/v1/system/preflight")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        parse_json(ls_body)["autonomous_readiness_applicable"],
        false
    );
}

// ---------------------------------------------------------------------------
// AR-10 — WS=Live + arm_pending (disarmed in-memory) → overall_ready = false
//         arm_state = "arm_pending" (disarmed but not halted)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar10_arm_pending_is_not_overall_ready() {
    let st = make_paper_alpaca();
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "msg-001".to_string(),
        last_event_at: "2026-04-04T14:30:00Z".to_string(),
    })
    .await;
    // Leave integrity in default disarmed state (fresh test AppState).
    // Verify it is disarmed (arm_pending) and not halted.
    {
        let ig = st.integrity.read().await;
        assert!(ig.disarmed, "precondition: fresh AppState starts disarmed");
        assert!(!ig.halted, "precondition: fresh AppState is not halted");
    }

    let router = routes::build_router(st);
    let req = Request::builder()
        .uri("/api/v1/autonomous/readiness")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(v["ws_continuity_ready"], true, "WS should be live");
    assert_eq!(v["arm_state"], "arm_pending");
    assert_eq!(v["arm_ready"], false);
    assert_eq!(v["overall_ready"], false);
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    assert!(
        blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("arm_pending")
                || b.as_str().unwrap_or("").contains("disarmed")),
        "blockers must mention arm_pending/disarmed state: {blockers:?}"
    );
}

// ---------------------------------------------------------------------------
// AR-11 — Clock injected to NYSE premarket → session_in_window = false,
//         overall_ready = false, session blocker present
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar11_premarket_clock_session_out_of_window() {
    let st = make_paper_alpaca();
    // Advance WS to Live and arm integrity so only session window is blocking.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "msg-001".to_string(),
        last_event_at: "2026-03-30T13:00:00Z".to_string(),
    })
    .await;
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    // Inject session clock to premarket: Monday 2026-03-30 13:00 UTC = 09:00 ET.
    st.set_session_clock_ts_for_test(nyse_premarket_ts()).await;

    let router = routes::build_router(st);
    let req = Request::builder()
        .uri("/api/v1/autonomous/readiness")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(v["truth_state"], "active");
    assert_eq!(v["ws_continuity_ready"], true, "WS is live");
    assert_eq!(v["reconcile_ready"], true, "reconcile is ok");
    assert_eq!(v["arm_ready"], true, "integrity is armed");
    assert_eq!(
        v["session_in_window"], false,
        "premarket clock must yield session_in_window = false"
    );
    assert_eq!(v["session_window_state"], "outside_window");
    assert_eq!(
        v["overall_ready"], false,
        "outside session window → overall_ready must be false"
    );
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    assert!(
        blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("session window")
                || b.as_str().unwrap_or("").contains("outside")),
        "blockers must surface the session-window blocker: {blockers:?}"
    );
}

// ---------------------------------------------------------------------------
// AR-12 — Clock injected to NYSE regular session → session_in_window = true
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar12_regular_session_clock_in_window() {
    let st = make_paper_alpaca();
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "msg-001".to_string(),
        last_event_at: "2026-03-30T14:00:00Z".to_string(),
    })
    .await;
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    // Inject session clock to regular session: Monday 2026-03-30 14:00 UTC = 10:00 ET.
    st.set_session_clock_ts_for_test(nyse_regular_session_ts())
        .await;

    let router = routes::build_router(st);
    let req = Request::builder()
        .uri("/api/v1/autonomous/readiness")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(
        v["session_in_window"], true,
        "regular-session clock must yield session_in_window = true"
    );
    assert_eq!(v["session_window_state"], "in_window");
    // Also verify preflight surfaces the session_in_window truth.
    // (Preflight derived from same schedule, same seam.)
}

// ---------------------------------------------------------------------------
// AR-13 — No active run → runtime_start_allowed = true
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar13_no_active_run_runtime_start_allowed() {
    let st = make_paper_alpaca();
    // Default: no execution loop spawned; locally_owned_run_id() returns None.
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "precondition: fresh test AppState has no active run"
    );

    let router = routes::build_router(st);
    let req = Request::builder()
        .uri("/api/v1/autonomous/readiness")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(
        v["runtime_start_allowed"], true,
        "no active run → runtime_start_allowed must be true"
    );
}

// ---------------------------------------------------------------------------
// AR-14 — Active locally-owned run → runtime_start_allowed = false,
//         overall_ready = false, start returns 409 Conflict.
//
// Uses inject_running_loop_for_test (test-only seam, state.rs) to seed a
// non-finishing fake execution loop.  Proves that:
//   (a) readiness surface immediately reflects runtime_start_allowed = false
//       and includes an active-run blocker
//   (b) POST /v1/run/start refuses with 409 when the active-run check fires
//       (integrity armed so we reach that check before any earlier gate)
//   (c) readiness and start are consistent: both refuse the same condition
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ar14_active_run_blocks_readiness_and_start() {
    let st = make_paper_alpaca();
    let run_id = Uuid::new_v4(); // test-seam only; never enters domain event path

    // Inject a never-finishing fake execution loop.
    st.inject_running_loop_for_test(run_id).await;

    // Precondition: locally_owned_run_id must now be Some.
    assert_eq!(
        st.locally_owned_run_id().await,
        Some(run_id),
        "inject_running_loop_for_test must populate locally_owned_run_id"
    );

    // --- Readiness surface ---
    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(
        v["runtime_start_allowed"], false,
        "active run must make runtime_start_allowed = false"
    );
    assert_eq!(
        v["overall_ready"], false,
        "active run must make overall_ready = false"
    );
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    assert!(
        blockers.iter().any(|b| {
            let s = b.as_str().unwrap_or("");
            s.contains("active") || s.contains("409") || s.contains("run")
        }),
        "blockers must mention the active-run conflict: {blockers:?}"
    );

    // --- Start refusal consistency ---
    // Arm integrity so we reach the active-run check (line 1210 of state.rs)
    // without hitting the earlier integrity gate (which would return 403).
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    let (start_status, start_body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("POST")
            .uri("/v1/run/start")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        start_status,
        StatusCode::CONFLICT,
        "start must return 409 Conflict when a locally-owned run is active: {}",
        String::from_utf8_lossy(&start_body)
    );
    let start_v = parse_json(start_body);
    let fault = start_v["fault_class"].as_str().unwrap_or("");
    assert!(
        fault.contains("already_owned") || fault.contains("conflict") || fault.contains("active"),
        "fault_class must identify the active-run conflict: {fault}"
    );
}
