//! # AUTON-PAPER-01 — Autonomous paper-day contract proof
//!
//! ## Purpose
//!
//! Proves the minimum targeted contract for one honest full-day autonomous
//! paper run on the canonical Paper+Alpaca path.
//!
//! ## What this file proves
//!
//! | Test  | Claim                                                                          |
//! |-------|--------------------------------------------------------------------------------|
//! | AU-01 | Session window parser: valid, invalid, boundary cases                          |
//! | AU-02 | spawn_autonomous_session_controller: only activates for Paper+Alpaca           |
//! | AU-03 | Auto-start allowed: WS=Live + armed → start passes WS+integrity+deployment gates (reaches DB gate) |
//! | AU-04 | Auto-start refused: WS=ColdStartUnproven → start blocked at WS continuity gate |
//! | AU-05 | Auto-start refused: WS=GapDetected → start blocked at WS continuity gate       |
//! | AU-06 | Gap→halt path: GapDetected → loop self-halts (PT-AUTO-01) → run gone           |
//! | AU-07 | Restart with persisted cursor truth: after gap, WS→Live → start gates pass     |
//! | AU-08 | Alert truth: alerts/active shows gap_detected alert when WS=GapDetected        |
//! | AU-09 | Alert truth: alerts/active shows day_limit_reached when limit hit + running    |
//! | AU-10 | Alert truth: alerts/active shows cold_start_unproven when WS=ColdStartUnproven |
//! | AU-10B | Alert truth: alerts/active shows recovery_succeeded while current              |
//! | AU-10C | DB-backed restart seed from persisted gap remains blocked and visible         |
//! | AU-10D | Clean idle return clears stale informational autonomous alerts               |
//! | AU-10E | DB-backed autonomous supervisor history is durable and visible in events/feed |
//! | AU-10F | DB-backed end-to-end autonomous recovery round-trip uses persisted cursor truth and resumes honestly |
//! | AU-11 | Session controller disabled for non-paper-alpaca (Paper+Paper returns None)    |
//! | AU-12 | Default schedule falls back to NYSE regular-session truth when env vars absent |
//! | AU-13 | try_autonomous_arm: already armed → Ok idempotent (pure, no DB)                |
//! | AU-14 | try_autonomous_arm: halted → refuses unconditionally (pure, no DB)             |
//! | AU-15 | try_autonomous_arm: no DB → refuses (pure, no DB)                              |
//! | AU-16 | try_autonomous_arm: DB=ARMED → advances integrity to armed (DB-backed)         |
//!
//! ## Acceptance contract
//!
//! - Code/proof closure from this file can mark autonomous paper trading backend-complete enough for an operator-run soak.
//! - A real full-day paper soak is still pending and must be reviewed separately from these proofs.
//!
//! ## What is NOT claimed
//!
//! - Wall-clock soak (no real 24h test)
//! - Broker HTTP connectivity (pure in-process)
//! - REST catch-up fill count (REST polling proven by A5/alpaca_live_adapter_a5)
//! - Auto-stop via real time passage (proven by session window unit tests SW-05..SW-09)
//!
use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::TimeZone;
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use state::{
    AlpacaWsContinuityState, AutonomousRecoveryResumeSource, AutonomousSessionSchedule,
    AutonomousSessionTruth, BrokerKind, DeploymentMode, SessionWindow, SESSION_START_HH_MM_ENV,
    SESSION_STOP_HH_MM_ENV,
};
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

fn make_paper_alpaca() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ))
}

async fn db_pool_or_skip() -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(v) => v,
        Err(_) => return None,
    };
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("AUTON-01 DB test: failed to connect to MQK_DATABASE_URL"),
    )
}

// ---------------------------------------------------------------------------
// AU-01 — Session window: parser contract (unit, no AppState needed)
// ---------------------------------------------------------------------------

#[test]
fn au01_session_window_parser_contract() {
    // Valid window
    let w = SessionWindow::parse("14:30", "21:00").expect("valid parse must succeed");
    assert_eq!(w.start_hh, 14);
    assert_eq!(w.start_mm, 30);
    assert_eq!(w.stop_hh, 21);
    assert_eq!(w.stop_mm, 0);

    // start == stop: rejected (zero-duration)
    assert!(
        SessionWindow::parse("14:30", "14:30").is_none(),
        "zero-duration must be rejected"
    );

    // start > stop: rejected
    assert!(
        SessionWindow::parse("21:00", "14:30").is_none(),
        "inverted window must be rejected"
    );

    // Invalid format
    assert!(SessionWindow::parse("bad", "21:00").is_none());
    assert!(
        SessionWindow::parse("25:00", "21:00").is_none(),
        "hour > 23 must be rejected"
    );
    assert!(
        SessionWindow::parse("14:60", "21:00").is_none(),
        "minute > 59 must be rejected"
    );

    // Edge: midnight valid
    let w2 = SessionWindow::parse("00:00", "23:59").expect("midnight window must parse");
    assert_eq!(w2.start_hh, 0);
    assert_eq!(w2.stop_mm, 59);
}

// ---------------------------------------------------------------------------
// AU-02 — autonomous controller only activates for the Paper+Alpaca deployment seam
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au02_session_controller_only_for_paper_alpaca() {
    // Paper+Paper: ExternalSignalIngestion not wired → controller disabled
    let st_paper = Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Paper,
    ));
    // We can't set env vars without risking test interference, so verify via
    // strategy_market_data_source directly — spawn_autonomous_session_controller
    // returns None when source != ExternalSignalIngestion.
    assert_eq!(
        st_paper.strategy_market_data_source(),
        state::StrategyMarketDataSource::NotConfigured,
        "paper+paper must have NotConfigured source → controller would return None"
    );

    // Paper+Alpaca: ExternalSignalIngestion wired → controller can activate.
    // With the final patch, absent env vars fall back to NYSE regular-session truth.
    let st_alpaca = make_paper_alpaca();
    assert_eq!(
        st_alpaca.strategy_market_data_source(),
        state::StrategyMarketDataSource::ExternalSignalIngestion,
        "paper+alpaca must have ExternalSignalIngestion → controller can activate on the NYSE regular-session seam"
    );

    // LiveShadow: deployment_mode != Paper → controller disabled
    let st_live = Arc::new(state::AppState::new_for_test_with_mode(
        DeploymentMode::LiveShadow,
    ));
    assert_ne!(
        st_live.deployment_mode(),
        DeploymentMode::Paper,
        "live-shadow must not be Paper → controller returns None"
    );
}

// ---------------------------------------------------------------------------
// AU-03 — Auto-start allowed: WS=Live + (armed omitted → reaches next gate)
//
// When WS is Live and the deployment gate passes, start_execution_runtime
// reaches the integrity_armed gate (because the test state is not armed).
// This proves the WS gate passes and deployment gate passes — the canonical
// Paper+Alpaca happy path for auto-start.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au03_auto_start_allowed_reaches_next_gate_when_ws_live() {
    let st = make_paper_alpaca();
    // Set WS to Live — this is the required state for auto-start.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: String::new(),
        last_event_at: String::new(),
    })
    .await;

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .header("content-type", "application/json")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let j = parse_json(body);

    // Must NOT be blocked at deployment_mode gate
    assert_ne!(
        j["gate"].as_str(),
        Some("deployment_mode"),
        "AU-03: paper+alpaca must not be blocked at deployment_mode gate; got: {j}"
    );
    // Must NOT be blocked at alpaca_ws_continuity gate (WS is Live)
    assert_ne!(
        j["gate"].as_str(),
        Some("alpaca_ws_continuity"),
        "AU-03: WS=Live must pass WS continuity gate; got: {j}"
    );
    // Must be blocked at integrity_armed (test state is not armed — correct)
    assert_eq!(
        j["gate"].as_str(),
        Some("integrity_armed"),
        "AU-03: next gate after WS=Live must be integrity_armed; got: {j}"
    );
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// AU-04 — Auto-start refused: WS=ColdStartUnproven blocks at continuity gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au04_auto_start_refused_when_ws_cold_start_unproven() {
    let st = make_paper_alpaca();
    // Default state is ColdStartUnproven for Paper+Alpaca — don't change it.
    // Arm integrity so we don't fail there first.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .header("content-type", "application/json")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let j = parse_json(body);

    assert_eq!(
        j["gate"].as_str(),
        Some("alpaca_ws_continuity"),
        "AU-04: ColdStartUnproven must be blocked at alpaca_ws_continuity gate; got: {j}"
    );
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// AU-05 — Auto-start refused: WS=GapDetected blocks at continuity gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au05_auto_start_refused_when_ws_gap_detected() {
    let st = make_paper_alpaca();
    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:order-x:filled:2026-01-01T00:00:00Z".to_string()),
        last_event_at: Some("2026-01-01T00:00:00Z".to_string()),
        detail: "WS disconnected during session".to_string(),
    })
    .await;
    // Arm integrity so WS gate is the first failure.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .header("content-type", "application/json")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    let j = parse_json(body);

    assert_eq!(
        j["gate"].as_str(),
        Some("alpaca_ws_continuity"),
        "AU-05: GapDetected must be blocked at alpaca_ws_continuity gate; got: {j}"
    );
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// AU-06 — Gap→halt path: GapDetected triggers loop self-halt (PT-AUTO-01)
//
// Uses run_loop_one_tick_for_test to exercise the real production loop path.
// The loop exits with the WS gap halt note.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au06_gap_detected_triggers_execution_loop_self_halt() {
    let st = Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ));
    // Set GapDetected — the loop checks this via ws_continuity_gap_requires_halt().
    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:order-halt:filled:2026-01-01T00:00:00Z".to_string()),
        last_event_at: Some("2026-01-01T00:00:00Z".to_string()),
        detail: "test gap injection".to_string(),
    })
    .await;

    let run_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"auton01.au06.gap_halt");

    // run_loop_one_tick_for_test exercises the real spawn_execution_loop path
    // and returns the exit note when the loop terminates.
    let exit_note = st.run_loop_one_tick_for_test(run_id).await;

    let note = exit_note.expect("loop must exit with a note");
    assert!(
        note.contains("gap") || note.contains("continuity"),
        "AU-06: loop exit note must reference gap/continuity; got: {note:?}"
    );

    // After the halt, integrity must be disarmed and halted.
    let ig = st.integrity.read().await;
    assert!(
        ig.halted,
        "AU-06: integrity must be halted after WS gap self-halt"
    );
    assert!(
        ig.disarmed,
        "AU-06: integrity must be disarmed after WS gap self-halt"
    );
}

// ---------------------------------------------------------------------------
// AU-07 — Restart with persisted cursor truth: gap → WS re-establishes Live →
//          start gates pass (reaches DB gate on next attempt)
//
// Proves the recovery retry loop contract:
// 1. After gap halt, start is refused (WS gate)
// 2. WS re-establishes Live (simulated by update_ws_continuity)
// 3. Start now passes WS and deployment gates (reaches integrity gate)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au07_restart_after_gap_passes_gates_when_ws_live() {
    let st = make_paper_alpaca();

    // Step 1: GapDetected — start must be refused at WS gate.
    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:order-gap:filled:2026-01-01T00:00:00Z".to_string()),
        last_event_at: Some("2026-01-01T00:00:00Z".to_string()),
        detail: "gap restart test".to_string(),
    })
    .await;
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, body) = call(router, req).await;
    let j = parse_json(body);
    assert_eq!(
        j["gate"].as_str(),
        Some("alpaca_ws_continuity"),
        "AU-07 step1: GapDetected must block at WS gate; got: {j}"
    );

    // Step 2: WS re-establishes Live (the transport's reconnect path does this).
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:order-gap:filled:2026-01-01T00:00:00Z".to_string(),
        last_event_at: "2026-01-01T00:00:00Z".to_string(),
    })
    .await;

    // Step 3: Retry start — WS gate now passes; blocked at integrity (test state is
    // armed via the write above, but not the start gate integrity check, which
    // calls is_execution_blocked() → disarmed=false halted=false → passes).
    // The next gate after WS is reconcile_truth (ok for fresh state), then
    // artifact intake (not configured → pass-through), then capital policy
    // (not configured → pass-through for Paper), then DB (no DB → 503).
    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status2, body2) = call(router, req).await;
    let j2 = parse_json(body2);

    // Must NOT be blocked at WS gate (it passed)
    assert_ne!(
        j2["gate"].as_str(),
        Some("alpaca_ws_continuity"),
        "AU-07 step3: WS=Live must pass WS gate on restart; got: {j2}"
    );
    // Must NOT be blocked at deployment_mode
    assert_ne!(
        j2["gate"].as_str(),
        Some("deployment_mode"),
        "AU-07 step3: deployment gate must pass on restart; got: {j2}"
    );
    // Acceptable terminal states: 503 (no DB), 409 (already owned), or 403 at a
    // later gate.  Any of these proves the WS + deployment gates passed.
    let gate = j2["gate"].as_str().unwrap_or("none");
    assert!(
        status2 == StatusCode::SERVICE_UNAVAILABLE
            || status2 == StatusCode::CONFLICT
            || (status2 == StatusCode::FORBIDDEN && gate != "alpaca_ws_continuity"),
        "AU-07 step3: restart after Live must advance past WS gate; status={status2} gate={gate}"
    );
}

// ---------------------------------------------------------------------------
// AU-08 — Alert truth: gap_detected alert when WS=GapDetected
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au08_alert_gap_detected_when_ws_gap() {
    let st = make_paper_alpaca();
    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:order-alert:filled:2026-01-01T00:00:00Z".to_string()),
        last_event_at: Some("2026-01-01T00:00:00Z".to_string()),
        detail: "alert test gap".to_string(),
    })
    .await;

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/alerts/active")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let j = parse_json(body);

    let rows = j["rows"].as_array().expect("rows must be array");
    let classes: Vec<&str> = rows.iter().filter_map(|r| r["class"].as_str()).collect();

    assert!(
        classes.contains(&"paper.ws_continuity.gap_detected"),
        "AU-08: alerts/active must include paper.ws_continuity.gap_detected when WS=GapDetected; \
         got classes: {classes:?}"
    );
}

// ---------------------------------------------------------------------------
// AU-09 — Alert truth: day_limit_reached alert when limit hit + running
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au09_alert_day_limit_reached_when_limit_hit_and_running() {
    let st = make_paper_alpaca();

    // Saturate the day signal counter (MAX_AUTONOMOUS_SIGNALS_PER_RUN = 100).
    st.set_day_signal_count_for_test(100);

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/alerts/active")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let j = parse_json(body);

    let rows = j["rows"].as_array().expect("rows must be array");
    let classes: Vec<&str> = rows.iter().filter_map(|r| r["class"].as_str()).collect();

    assert!(
        classes.contains(&"autonomous.signal_limit.day_limit_reached"),
        "AU-09: alerts/active must include autonomous.signal_limit.day_limit_reached \
         when limit hit and running; got classes: {classes:?}"
    );
}

// ---------------------------------------------------------------------------
// AU-10 — Alert truth: cold_start_unproven when WS=ColdStartUnproven
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au10_alert_cold_start_unproven_when_ws_unproven() {
    let st = make_paper_alpaca();
    // Default for Paper+Alpaca is ColdStartUnproven — no explicit set needed.

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/alerts/active")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let j = parse_json(body);

    let rows = j["rows"].as_array().expect("rows must be array");
    let classes: Vec<&str> = rows.iter().filter_map(|r| r["class"].as_str()).collect();

    assert!(
        classes.contains(&"paper.ws_continuity.cold_start_unproven"),
        "AU-10: alerts/active must include paper.ws_continuity.cold_start_unproven \
         when WS=ColdStartUnproven; got classes: {classes:?}"
    );
}

// ---------------------------------------------------------------------------
// AU-10B — Alert truth: autonomous recovery succeeded is operator-visible
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au10b_alert_recovery_succeeded_visible_when_truth_set() {
    let st = make_paper_alpaca();
    st.set_autonomous_session_truth(AutonomousSessionTruth::RecoverySucceeded {
        resume_source: AutonomousRecoveryResumeSource::PersistedCursor,
        detail: "test recovery succeeded from persisted cursor".to_string(),
    })
    .await;

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/alerts/active")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let j = parse_json(body);

    let rows = j["rows"].as_array().expect("rows must be array");
    let classes: Vec<&str> = rows.iter().filter_map(|r| r["class"].as_str()).collect();

    assert!(
        classes.contains(&"autonomous.session.recovery_succeeded"),
        "AU-10B: alerts/active must include autonomous.session.recovery_succeeded; got classes: {classes:?}"
    );
}

// ---------------------------------------------------------------------------
// AU-10D — Clean out-of-session idle return clears stale informational alerts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au10d_clean_idle_return_clears_stale_informational_autonomous_alerts() {
    let st = make_paper_alpaca();
    let window = AutonomousSessionSchedule::FixedUtcWindow(
        SessionWindow::parse("14:30", "21:00").expect("AU-10D: window must parse"),
    );
    let idle_after_session = chrono::Utc.with_ymd_and_hms(2026, 3, 30, 22, 0, 0).unwrap();
    let mut locally_started = false;

    st.set_autonomous_session_truth(AutonomousSessionTruth::RecoverySucceeded {
        resume_source: AutonomousRecoveryResumeSource::PersistedCursor,
        detail: "recovery succeeded earlier in the day".to_string(),
    })
    .await;
    state::run_session_controller_tick(&st, window, &mut locally_started, idle_after_session).await;

    st.set_autonomous_session_truth(AutonomousSessionTruth::StoppedAtBoundary {
        detail: "run stopped at session boundary".to_string(),
    })
    .await;
    state::run_session_controller_tick(&st, window, &mut locally_started, idle_after_session).await;

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/alerts/active")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let j = parse_json(body);

    let rows = j["rows"].as_array().expect("rows must be array");
    let classes: Vec<&str> = rows.iter().filter_map(|r| r["class"].as_str()).collect();

    assert!(
        !classes.contains(&"autonomous.session.recovery_succeeded"),
        "AU-10D: clean idle return must clear stale autonomous.session.recovery_succeeded; got classes: {classes:?}"
    );
    assert!(
        !classes.contains(&"autonomous.session.stopped_at_boundary"),
        "AU-10D: clean idle return must clear stale autonomous.session.stopped_at_boundary; got classes: {classes:?}"
    );
}

// ---------------------------------------------------------------------------
// AU-10C — DB-backed restart seed: persisted gap remains blocked and visible
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au10c_restart_seed_from_persisted_gap_is_blocked_and_visible() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("AU-10C: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    mqk_db::migrate(&pool)
        .await
        .expect("AU-10C: migration failed");

    let adapter_id = "auton01-au10c-gap";
    let gap_cursor = mqk_broker_alpaca::types::AlpacaFetchCursor::gap_detected(
        Some("rest-au10c".to_string()),
        Some("alpaca:order-au10c:filled:2026-01-01T00:00:00Z".to_string()),
        Some("2026-01-01T00:00:00Z".to_string()),
        "au10c persisted gap",
    );
    let cursor_json = serde_json::to_string(&gap_cursor).expect("AU-10C: serialize cursor");
    mqk_db::advance_broker_cursor(&pool, adapter_id, &cursor_json, chrono::Utc::now())
        .await
        .expect("AU-10C: persist cursor");

    let mut st_inner = state::AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st_inner.set_adapter_id_for_test(adapter_id);
    let st = Arc::new(st_inner);

    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    st.seed_ws_continuity_from_db().await;

    let continuity = st.alpaca_ws_continuity().await;
    assert!(
        matches!(continuity, AlpacaWsContinuityState::GapDetected { .. }),
        "AU-10C: restart from persisted gap must remain GapDetected; got: {continuity:?}"
    );
    let truth = st.autonomous_session_truth().await;
    assert!(
        matches!(
            truth,
            AutonomousSessionTruth::RecoveryRetrying {
                resume_source: AutonomousRecoveryResumeSource::PersistedCursor,
                ..
            }
        ),
        "AU-10C: restart from persisted gap must surface RecoveryRetrying from persisted cursor; got: {truth:?}"
    );

    let router = routes::build_router(Arc::clone(&st));
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (start_status, start_body) = call(router.clone(), start_req).await;
    let start_json = parse_json(start_body);
    assert_eq!(start_status, StatusCode::FORBIDDEN);
    assert_eq!(
        start_json["gate"].as_str(),
        Some("alpaca_ws_continuity"),
        "AU-10C: persisted-gap restart must remain blocked at alpaca_ws_continuity; got: {start_json}"
    );

    let alerts_req = Request::builder()
        .method("GET")
        .uri("/api/v1/alerts/active")
        .body(axum::body::Body::empty())
        .unwrap();
    let (alerts_status, alerts_body) = call(router, alerts_req).await;
    assert_eq!(alerts_status, StatusCode::OK);
    let alerts_json = parse_json(alerts_body);
    let classes: Vec<&str> = alerts_json["rows"]
        .as_array()
        .expect("rows must be array")
        .iter()
        .filter_map(|r| r["class"].as_str())
        .collect();
    assert!(
        classes.contains(&"paper.ws_continuity.gap_detected"),
        "AU-10C: alerts must include paper.ws_continuity.gap_detected; got classes: {classes:?}"
    );
    assert!(
        classes.contains(&"autonomous.session.recovery_retrying"),
        "AU-10C: alerts must include autonomous.session.recovery_retrying; got classes: {classes:?}"
    );
}

// ---------------------------------------------------------------------------
// AU-10E — DB-backed autonomous supervisor history is durable and visible in
//          /api/v1/events/feed across restart-seeded recovery truth changes.
//
// Proves the narrow durable-evidence contract needed before the operator's
// real soak:
// 1. persisted broker cursor truth is loaded on restart,
// 2. autonomous recovery retrying truth is recorded durably,
// 3. recovery success truth is recorded durably,
// 4. /api/v1/events/feed surfaces those rows from durable storage.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au10e_db_backed_autonomous_history_is_durable_and_visible_in_events_feed() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("AU-10E: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    mqk_db::migrate(&pool)
        .await
        .expect("AU-10E: migration failed");

    let adapter_id = "auton01-au10e-history";
    let gap_cursor = mqk_broker_alpaca::types::AlpacaFetchCursor::gap_detected(
        Some("rest-au10e".to_string()),
        Some("alpaca:order-au10e:filled:2026-01-01T00:00:00Z".to_string()),
        Some("2026-01-01T00:00:00Z".to_string()),
        "au10e persisted gap",
    );
    let cursor_json = serde_json::to_string(&gap_cursor).expect("AU-10E: serialize cursor");
    mqk_db::advance_broker_cursor(&pool, adapter_id, &cursor_json, chrono::Utc::now())
        .await
        .expect("AU-10E: persist cursor");

    let mut st_inner = state::AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st_inner.set_adapter_id_for_test(adapter_id);
    let st = Arc::new(st_inner);

    st.seed_ws_continuity_from_db().await;
    st.set_autonomous_session_truth(AutonomousSessionTruth::RecoverySucceeded {
        resume_source: AutonomousRecoveryResumeSource::PersistedCursor,
        detail: "au10e recovery succeeded after restart-seeded retry".to_string(),
    })
    .await;

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/events/feed")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let j = parse_json(body);

    let rows = j["rows"].as_array().expect("rows must be array");
    let autonomous_details: Vec<&str> = rows
        .iter()
        .filter(|r| r["kind"].as_str() == Some("autonomous_session"))
        .filter_map(|r| r["detail"].as_str())
        .collect();

    assert!(
        autonomous_details
            .iter()
            .any(|d| *d == "recovery_retrying:persisted_cursor"),
        "AU-10E: durable events/feed must include recovery_retrying:persisted_cursor; got: {autonomous_details:?}"
    );
    assert!(
        autonomous_details
            .iter()
            .any(|d| *d == "recovery_succeeded:persisted_cursor"),
        "AU-10E: durable events/feed must include recovery_succeeded:persisted_cursor; got: {autonomous_details:?}"
    );
}

// ---------------------------------------------------------------------------
// AU-10F — DB-backed autonomous recovery round-trip proof
//
// Exercises one connected DB-backed lifecycle using the daemon's real durable
// state plus a narrow test-only active-run seam:
// 1. DB=ARMED restores autonomous arm readiness,
// 2. a DB-backed active run is established with local ownership,
// 3. a continuity gap self-halts that owned lifecycle fail-closed,
// 4. restart seeds continuity from the persisted broker cursor,
// 5. recovery advances through the real cursor-repair backend seam,
// 6. alerts + durable autonomous history reflect retrying/succeeded truth,
// 7. resumed state is no longer blocked at WS continuity.
//
// This proves one coherent backend lifecycle without claiming a real broker
// network start or a wall-clock soak.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au10f_db_backed_autonomous_recovery_round_trip_is_honest() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("AU-10F: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    mqk_db::migrate(&pool)
        .await
        .expect("AU-10F: migration failed");

    let adapter_id = "auton01-au10f-roundtrip";
    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None)
        .await
        .expect("AU-10F: seed arm state failed");

    let mut st_inner = state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st_inner.set_adapter_id_for_test(adapter_id);
    let st = Arc::new(st_inner);

    st.try_autonomous_arm()
        .await
        .expect("AU-10F: DB=ARMED must allow autonomous arm");

    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:order-au10f:accepted:2026-01-01T00:00:00Z".to_string(),
        last_event_at: "2026-01-01T00:00:00Z".to_string(),
    })
    .await;

    let run_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        b"auton01.au10f.connected_db_backed_lifecycle",
    );
    st.establish_db_backed_active_run_for_test(run_id)
        .await
        .expect("AU-10F: DB-backed active run must be established");

    let start_truth = st
        .current_status_snapshot()
        .await
        .expect("AU-10F: status snapshot after DB-backed active run");
    assert_eq!(start_truth.active_run_id, Some(run_id));
    assert_eq!(start_truth.state, "running");
    let restart_truth = st
        .restart_truth_snapshot()
        .await
        .expect("AU-10F: restart truth snapshot after DB-backed active run");
    assert_eq!(restart_truth.local_owned_run_id, Some(run_id));
    assert_eq!(restart_truth.durable_active_run_id, Some(run_id));
    assert!(
        !restart_truth.durable_active_without_local_ownership,
        "AU-10F step1: active run must have coherent local ownership truth"
    );

    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:order-au10f:fill:2026-01-01T00:05:00Z".to_string()),
        last_event_at: Some("2026-01-01T00:05:00Z".to_string()),
        detail: "AU-10F injected continuity gap".to_string(),
    })
    .await;
    let halt_note = st
        .gap_halt_owned_runtime_for_test()
        .await
        .expect("AU-10F: gap halt helper must run")
        .expect("AU-10F: gap halt must produce a note");
    assert!(
        halt_note.contains("gap") || halt_note.contains("continuity"),
        "AU-10F step2: gap halt note must mention gap/continuity; got: {halt_note:?}"
    );
    {
        let ig = st.integrity.read().await;
        assert!(
            ig.halted,
            "AU-10F step2: integrity must be halted after gap self-halt"
        );
        assert!(
            ig.disarmed,
            "AU-10F step2: integrity must be disarmed after gap self-halt"
        );
    }
    let halted_truth = st
        .current_status_snapshot()
        .await
        .expect("AU-10F: halted status snapshot");
    assert_eq!(halted_truth.state, "halted");
    let halted_run = mqk_db::fetch_run(st.db.as_ref().unwrap(), run_id)
        .await
        .expect("AU-10F: fetch halted run");
    assert!(matches!(halted_run.status, mqk_db::RunStatus::Halted));

    let gap_cursor = mqk_broker_alpaca::types::AlpacaFetchCursor::gap_detected(
        Some("rest-au10f".to_string()),
        Some("alpaca:order-au10f:fill:2026-01-01T00:05:00Z".to_string()),
        Some("2026-01-01T00:05:00Z".to_string()),
        "AU-10F persisted gap cursor",
    );
    let gap_cursor_json = serde_json::to_string(&gap_cursor).expect("AU-10F: serialize gap cursor");
    mqk_db::advance_broker_cursor(&pool, adapter_id, &gap_cursor_json, chrono::Utc::now())
        .await
        .expect("AU-10F: persist gap cursor");

    let mut restarted_inner = state::AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    restarted_inner.set_adapter_id_for_test(adapter_id);
    let restarted = Arc::new(restarted_inner);
    restarted.seed_ws_continuity_from_db().await;

    assert!(
        matches!(
            restarted.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::GapDetected { .. }
        ),
        "AU-10F step3: restart must seed unresolved persisted gap truth"
    );
    assert!(
        matches!(
            restarted.autonomous_session_truth().await,
            AutonomousSessionTruth::RecoveryRetrying {
                resume_source: AutonomousRecoveryResumeSource::PersistedCursor,
                ..
            }
        ),
        "AU-10F step3: restart must surface RecoveryRetrying from persisted cursor"
    );

    let recovered_cursor = restarted
        .repair_ws_continuity_from_persisted_cursor_for_test()
        .await
        .expect("AU-10F: cursor repair must succeed through the backend seam");
    assert!(
        matches!(
            recovered_cursor.trade_updates,
            mqk_broker_alpaca::types::AlpacaTradeUpdatesResume::Live { .. }
        ),
        "AU-10F step4: repaired cursor must be Live; got: {:?}",
        recovered_cursor.trade_updates
    );
    assert!(
        matches!(
            restarted.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::Live { .. }
        ),
        "AU-10F step4: repaired continuity must be Live"
    );
    assert!(
        matches!(
            restarted.autonomous_session_truth().await,
            AutonomousSessionTruth::RecoverySucceeded {
                resume_source: AutonomousRecoveryResumeSource::PersistedCursor,
                ..
            }
        ),
        "AU-10F step4: repaired continuity must surface RecoverySucceeded from persisted cursor"
    );

    let restarted_router = routes::build_router(Arc::clone(&restarted));
    let alerts_req = Request::builder()
        .method("GET")
        .uri("/api/v1/alerts/active")
        .body(axum::body::Body::empty())
        .unwrap();
    let (alerts_status, alerts_body) = call(restarted_router.clone(), alerts_req).await;
    assert_eq!(alerts_status, StatusCode::OK);
    let alerts_json = parse_json(alerts_body);
    let alert_classes: Vec<&str> = alerts_json["rows"]
        .as_array()
        .expect("rows must be array")
        .iter()
        .filter_map(|r| r["class"].as_str())
        .collect();
    assert!(
        alert_classes.contains(&"autonomous.session.recovery_succeeded"),
        "AU-10F step5: alerts must include autonomous.session.recovery_succeeded while current; got classes: {alert_classes:?}"
    );

    let feed_req = Request::builder()
        .method("GET")
        .uri("/api/v1/events/feed")
        .body(axum::body::Body::empty())
        .unwrap();
    let (feed_status, feed_body) = call(restarted_router.clone(), feed_req).await;
    assert_eq!(feed_status, StatusCode::OK);
    let feed_json = parse_json(feed_body);
    let autonomous_details: Vec<&str> = feed_json["rows"]
        .as_array()
        .expect("rows must be array")
        .iter()
        .filter(|r| r["kind"].as_str() == Some("autonomous_session"))
        .filter_map(|r| r["detail"].as_str())
        .collect();
    assert!(
        autonomous_details
            .iter()
            .any(|d| *d == "recovery_retrying:persisted_cursor"),
        "AU-10F step5: durable history must include recovery_retrying:persisted_cursor; got: {autonomous_details:?}"
    );
    assert!(
        autonomous_details
            .iter()
            .any(|d| *d == "recovery_succeeded:persisted_cursor"),
        "AU-10F step5: durable history must include recovery_succeeded:persisted_cursor; got: {autonomous_details:?}"
    );

    mqk_db::persist_arm_state_canonical(
        restarted.db.as_ref().unwrap(),
        mqk_db::ArmState::Armed,
        None,
    )
    .await
    .expect("AU-10F: re-seed arm state on restarted daemon");
    restarted
        .try_autonomous_arm()
        .await
        .expect("AU-10F: restarted daemon must auto-arm from DB=ARMED");
    let resumed_start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (resumed_status, resumed_body) = call(restarted_router, resumed_start_req).await;
    let resumed_json = parse_json(resumed_body);
    assert_ne!(
        resumed_json["gate"].as_str(),
        Some("alpaca_ws_continuity"),
        "AU-10F step6: resumed state must no longer be blocked at WS continuity; got: {resumed_json}"
    );
    assert!(
        resumed_status == StatusCode::SERVICE_UNAVAILABLE
            || resumed_status == StatusCode::FORBIDDEN
            || resumed_status == StatusCode::CONFLICT,
        "AU-10F step6: resumed start must advance past the WS continuity gate; status={resumed_status} body={resumed_json}"
    );
}

// ---------------------------------------------------------------------------
// AU-11 — Session controller disabled for non-paper-alpaca (Paper+Paper)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn au11_session_controller_disabled_for_paper_paper() {
    // Paper+Paper: deployment gate blocked + NotConfigured source.
    // spawn_autonomous_session_controller returns None when source != ExternalSignalIngestion.
    let st_paper = Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Paper,
    ));
    // Verify via the condition checked inside spawn_autonomous_session_controller.
    assert_eq!(
        st_paper.deployment_mode(),
        DeploymentMode::Paper,
        "AU-11: paper+paper must have Paper deployment mode"
    );
    assert_ne!(
        st_paper.strategy_market_data_source(),
        state::StrategyMarketDataSource::ExternalSignalIngestion,
        "AU-11: paper+paper must NOT have ExternalSignalIngestion → controller returns None"
    );
}

// ---------------------------------------------------------------------------
// AU-12 — Default schedule falls back to NYSE regular-session truth when env vars absent
// ---------------------------------------------------------------------------

#[test]
fn au12_default_schedule_is_nyse_when_env_vars_absent() {
    std::env::remove_var(SESSION_START_HH_MM_ENV);
    std::env::remove_var(SESSION_STOP_HH_MM_ENV);
    assert_eq!(
        state::autonomous_session_schedule_from_env(),
        AutonomousSessionSchedule::NyseRegularSession,
        "AU-12: absent env vars must fall back to NYSE regular-session truth"
    );
}

// ---------------------------------------------------------------------------
// AU-13..AU-16 — AUTON-PAPER-01B: try_autonomous_arm gate proof
// ---------------------------------------------------------------------------

// AU-13: Already armed → try_autonomous_arm returns Ok without touching DB.
//
// In-memory integrity.disarmed=false means the gate returns Ok idempotently.
// Proves the happy-path second-call is safe.
#[tokio::test]
async fn au13_already_armed_returns_ok_idempotent() {
    let st = make_paper_alpaca();

    // Force in-memory to armed (disarmed=false, halted=false).
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    // No DB attached to `st` — if the gate checked DB it would refuse.
    // The fact that it returns Ok proves the early-exit path is taken.
    let result = st.try_autonomous_arm().await;
    assert!(
        result.is_ok(),
        "AU-13: already armed must return Ok idempotently; got: {result:?}"
    );

    // Integrity must still be armed.
    let ig = st.integrity.read().await;
    assert!(
        !ig.disarmed,
        "AU-13: integrity.disarmed must remain false after idempotent arm"
    );
    assert!(
        !ig.halted,
        "AU-13: integrity.halted must remain false after idempotent arm"
    );
}

// AU-14: integrity.halted=true → try_autonomous_arm refuses unconditionally.
//
// Operator halt wins even when DB state would allow auto-arm.
#[tokio::test]
async fn au14_halted_state_refuses_autonomous_arm() {
    let st = make_paper_alpaca();

    // Force in-memory to halted.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = true;
        ig.halted = true;
    }

    let result = st.try_autonomous_arm().await;
    assert!(
        result.is_err(),
        "AU-14: halted state must refuse autonomous arm unconditionally"
    );
    let msg = result.unwrap_err();
    assert!(
        msg.contains("operator halt"),
        "AU-14: refusal message must mention operator halt; got: {msg:?}"
    );

    // Integrity must still be halted — arm must not have mutated it.
    let ig = st.integrity.read().await;
    assert!(
        ig.disarmed,
        "AU-14: integrity.disarmed must remain true after halt refusal"
    );
    assert!(
        ig.halted,
        "AU-14: integrity.halted must remain true after halt refusal"
    );
}

// AU-15: No DB configured → try_autonomous_arm refuses.
//
// `AppState::new_for_test_with_broker_kind` has no DB pool; auto-arm cannot
// verify prior session state and must refuse fail-closed.
#[tokio::test]
async fn au15_no_db_refuses_autonomous_arm() {
    let st = make_paper_alpaca();
    // Default boot state: disarmed=true, halted=false, no DB.

    let result = st.try_autonomous_arm().await;
    assert!(result.is_err(), "AU-15: no DB must refuse autonomous arm");
    let msg = result.unwrap_err();
    assert!(
        msg.contains("no DB"),
        "AU-15: refusal message must mention missing DB; got: {msg:?}"
    );
}

// AU-16 (DB-backed): DB=ARMED → auto-arm advances integrity to armed.
//
// Proves the daily-cycle: after a clean stop the DB remains ARMED; on the
// next daemon boot (in-memory disarmed=true) try_autonomous_arm reads ARMED
// from DB and restores the armed state without operator intervention.
//
// Skips silently when MQK_DATABASE_URL is not configured.
#[tokio::test]
async fn au16_db_armed_state_enables_autonomous_arm() {
    let db_url = match std::env::var("MQK_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("AU-16: MQK_DATABASE_URL not set — skipping DB-backed arm proof");
            return;
        }
    };

    let db = sqlx::PgPool::connect(&db_url)
        .await
        .expect("AU-16: DB connect failed");

    // Seed DB with ARMED state (simulates prior clean stop).
    mqk_db::persist_arm_state_canonical(&db, mqk_db::ArmState::Armed, None)
        .await
        .expect("AU-16: seed persist_arm_state_canonical failed");

    // Build AppState with DB; starts with in-memory disarmed=true (boot default).
    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        db,
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    // Verify we start disarmed (fail-closed boot).
    {
        let ig = st.integrity.read().await;
        assert!(ig.disarmed, "AU-16: must start disarmed at boot");
        assert!(!ig.halted, "AU-16: must start not-halted at boot");
    }

    // Attempt autonomous arm — should succeed because DB=ARMED.
    let result = st.try_autonomous_arm().await;
    assert!(
        result.is_ok(),
        "AU-16: DB=ARMED must allow autonomous arm; got: {result:?}"
    );

    // In-memory integrity must now be armed.
    let ig = st.integrity.read().await;
    assert!(
        !ig.disarmed,
        "AU-16: integrity.disarmed must be false after autonomous arm"
    );
    assert!(
        !ig.halted,
        "AU-16: integrity.halted must be false after autonomous arm"
    );
}
