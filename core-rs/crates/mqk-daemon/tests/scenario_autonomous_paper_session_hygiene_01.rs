//! AUTONOMOUS-PAPER-SESSION-HYGIENE-BUNDLE-01 — Consolidated session hygiene
//! contract proof for autonomous paper trading across repeated start/stop/restart
//! cycles.
//!
//! ## Purpose
//!
//! Assembles the proof matrix for the clean-startup / clean-shutdown / restart
//! hygiene contract in one place.  Each test is either a NEW pure in-process
//! proof or a cross-reference to the canonical file that proves the item.
//!
//! ## Startup contract — where each item is proven
//!
//! | Item | Canonical proof                                                         |
//! |------|-------------------------------------------------------------------------|
//! | No stale daemon owns port      | runtime/ops (process management)                   |
//! | DB reachable                   | start_execution_runtime DB gate (503)               |
//! | Mode is paper                  | deployment gate (403) — PT-TRUTH-01                 |
//! | live_routing_enabled=false     | SH07 this file; scenario_daemon_routes.rs           |
//! | Reconcile clean                | R01/R02 scenario_reconcile_start_gate_brk09r.rs     |
//! | kill_switch_active=false       | scenario_kill_switch_persistence_lo02e.rs           |
//! | Arm state armed or armable     | AU-05 scenario_autonomous_paper_day_auton01.rs      |
//! | No orphaned local owner        | QR-03 scenario_restart_quarantine_lo02d.rs          |
//! | Deadman heartbeat active       | scenario_daemon_routes.rs (heartbeat proof)         |
//! | Broker/DB baseline loaded      | SP01-SP08 scenario_runtime_seeds_position.rs        |
//! | current_position_qty==baseline | SP02/SP03 same file                                 |
//! | Stale outbox claims recovered  | scenario_stale_claim_recovery.rs (mqk-db)           |
//! | Stale inbox rows idempotent    | H07 scenario_clear_halted_run_auton04.rs            |
//! | Broker order map recovered     | recover_oms_and_portfolio unit tests in snapshot.rs |
//! | Flatten sees position snapshot | PSF-10 scenario_paper_flatten_psf01.rs              |
//! | Evidence capture classifies    | scenario_evidence_durability_ed01.rs                |
//!
//! ## Shutdown contract — where each item is proven
//!
//! | Item | Canonical proof                                                         |
//! |------|-------------------------------------------------------------------------|
//! | Runtime stops cleanly          | AL-01 scenario_autonomous_paper_day_lifecycle.rs    |
//! | Run status terminal/restartable| AL-01 + lifecycle.rs stop_run                       |
//! | Deadman does not halt orphan   | SH01-SH04 this file (RunEndedUnexpectedly)          |
//! | No duplicate fills on restart  | H07 scenario_clear_halted_run_auton04.rs            |
//! | No duplicate order submit      | W4/W5 scenario_crash_matrix_i9_3.rs                 |
//! | Reconcile remains clean        | R01-R04 scenario_reconcile_start_gate_brk09r.rs     |
//!
//! ## Newly proven in this file
//!
//! | Test | Kind   | Claim                                                              |
//! |------|--------|--------------------------------------------------------------------|
//! | SH01 | pure   | `(in_session=T, locally_started=T, active_run=F)` → `RunEndedUnexpectedly`, `locally_started=false` |
//! | SH02 | pure   | After `RunEndedUnexpectedly`, next in-session tick re-enters start path |
//! | SH03 | pure   | `RunEndedUnexpectedly` detail string is non-empty for operator triage |
//! | SH04 | pure   | Two consecutive RunEndedUnexpectedly ticks are idempotent (locally_started stays false) |
//! | SH05 | pure   | Deployment gate fires before WS continuity gate (paper+paper → deployment blocks first) |
//! | SH06 | pure   | Integrity gate fires before WS continuity gate (disarmed → integrity blocks before WS) |
//! | SH07 | pure   | `live_routing_enabled` is always explicit `false` in system/status for paper mode |
//! | SH08 | pure   | `try_autonomous_arm` gate 1: halted integrity unconditionally refuses |
//! | SH09 | pure   | `try_autonomous_arm` gate 3: no DB configured → refuses              |
//! | SH10 | pure   | Flatten route: paper mode + no DB → 503 (db_unavailable)            |
//! | SH11 | pure   | Flatten route: non-paper mode → 403 (not_paper_mode)                |
//!
//! SH01-SH04 prove `AutonomousSessionTruth::RunEndedUnexpectedly` which was
//! not tested anywhere in the prior test suite.
//!
//! All tests are pure in-process.  No `MQK_DATABASE_URL` required.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use chrono::{TimeZone, Utc};
use mqk_daemon::{
    routes::build_router,
    state::{
        AlpacaWsContinuityState, AppState, AutonomousSessionSchedule, AutonomousSessionTruth,
        BrokerKind, DeploymentMode, SessionWindow,
    },
};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn paper_alpaca_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca))
}

fn paper_paper_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_mode(DeploymentMode::Paper))
}

fn live_capital_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_mode(DeploymentMode::LiveCapital))
}

/// Fixed UTC session window 14:30–21:00.
fn test_window() -> SessionWindow {
    SessionWindow::parse("14:30", "21:00").expect("test window must parse")
}

/// An in-session timestamp (15:00 UTC, inside 14:30–21:00).
fn ts_in_session() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 14, 15, 0, 0).unwrap()
}

async fn post_action(
    router: axum::Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, j)
}

async fn get_json(router: axum::Router, path: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, j)
}

// ---------------------------------------------------------------------------
// SH01: (in_session=true, locally_started=true, active_run=false) →
//        sets RunEndedUnexpectedly, resets locally_started=false
//
// This is the "run vanished mid-session" path.  Proves:
//   1. Session controller detects the missing run.
//   2. AutonomousSessionTruth is set to RunEndedUnexpectedly.
//   3. locally_started is reset to false so the next tick attempts restart.
//
// Prior to this patch, RunEndedUnexpectedly had ZERO test coverage as a
// tick outcome anywhere in the test suite.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sh01_run_ended_unexpectedly_sets_truth_and_resets_locally_started() {
    let st = paper_alpaca_state();
    let schedule = AutonomousSessionSchedule::FixedUtcWindow(test_window());
    let now = ts_in_session();

    // Precondition: session is active, controller thinks it owns a run
    // (locally_started=true), but there is NO active run in the in-memory
    // state (no execution loop handle — default fresh state).
    let mut locally_started = true;

    mqk_daemon::state::run_session_controller_tick(&st, schedule, &mut locally_started, now)
        .await;

    // locally_started must be reset to false — the controller no longer owns the run.
    assert!(
        !locally_started,
        "SH01: locally_started must be false after run ends unexpectedly mid-session"
    );

    // AutonomousSessionTruth must be RunEndedUnexpectedly.
    let truth = st.autonomous_session_truth().await;
    assert!(
        matches!(truth, AutonomousSessionTruth::RunEndedUnexpectedly { .. }),
        "SH01: autonomous session truth must be RunEndedUnexpectedly \
         when run vanishes mid-session; got: {truth:?}"
    );
}

// ---------------------------------------------------------------------------
// SH02: After RunEndedUnexpectedly, the next in-session tick re-enters the
//        start path (locally_started=false + in_session → attempt_auto_start).
//
// Proves the controller retries start on the next tick instead of staying
// stuck.  The start attempt will fail (no DB, etc.) but the important
// invariant is that the controller transitions AWAY from RunEndedUnexpectedly.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sh02_after_run_ended_unexpectedly_next_tick_attempts_restart() {
    let st = paper_alpaca_state();
    let schedule = AutonomousSessionSchedule::FixedUtcWindow(test_window());
    let now = ts_in_session();

    // Tick 1: run vanishes → RunEndedUnexpectedly, locally_started=false.
    let mut locally_started = true;
    mqk_daemon::state::run_session_controller_tick(&st, schedule, &mut locally_started, now)
        .await;
    assert!(!locally_started, "SH02 precondition: tick 1 must reset locally_started");
    assert!(
        matches!(
            st.autonomous_session_truth().await,
            AutonomousSessionTruth::RunEndedUnexpectedly { .. }
        ),
        "SH02 precondition: truth must be RunEndedUnexpectedly after tick 1"
    );

    // Tick 2: still in-session, locally_started=false → attempt_auto_start fires.
    // The attempt fails (no DB, arm gate, etc.) and surfaces as StartRefused.
    mqk_daemon::state::run_session_controller_tick(&st, schedule, &mut locally_started, now)
        .await;

    // After tick 2, the start path was entered.  locally_started must remain
    // false because no run was successfully started (no DB).
    assert!(
        !locally_started,
        "SH02: locally_started must remain false when start attempt was blocked \
         (no execution loop started without DB)"
    );

    // RunEndedUnexpectedly must be gone — the controller entered attempt_auto_start
    // which either sets StartRefused or leaves the truth in a start-retry state.
    let truth_after_tick2 = st.autonomous_session_truth().await;
    assert!(
        !matches!(
            truth_after_tick2,
            AutonomousSessionTruth::RunEndedUnexpectedly { .. }
        ),
        "SH02: after second tick with start attempt, RunEndedUnexpectedly must be \
         cleared (controller re-entered the start path); got: {truth_after_tick2:?}"
    );
}

// ---------------------------------------------------------------------------
// SH03: RunEndedUnexpectedly detail is a non-empty operator-readable string.
//
// Proves the operator surface carries a useful diagnostic message.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sh03_run_ended_unexpectedly_detail_is_non_empty() {
    let st = paper_alpaca_state();
    let schedule = AutonomousSessionSchedule::FixedUtcWindow(test_window());
    let now = ts_in_session();
    let mut locally_started = true;

    mqk_daemon::state::run_session_controller_tick(&st, schedule, &mut locally_started, now)
        .await;

    let truth = st.autonomous_session_truth().await;
    if let AutonomousSessionTruth::RunEndedUnexpectedly { ref detail } = truth {
        assert!(
            !detail.is_empty(),
            "SH03: RunEndedUnexpectedly detail must be non-empty for operator triage"
        );
    } else {
        panic!("SH03: expected RunEndedUnexpectedly; got: {truth:?}");
    }
}

// ---------------------------------------------------------------------------
// SH04: Two consecutive RunEndedUnexpectedly ticks → locally_started stays
//        false (idempotent).
//
// Proves the controller does not accumulate stale ownership state across
// multiple "run vanished" ticks.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sh04_consecutive_run_ended_ticks_are_idempotent() {
    let st = paper_alpaca_state();
    let schedule = AutonomousSessionSchedule::FixedUtcWindow(test_window());
    let now = ts_in_session();

    // Tick 1: locally_started=true → RunEndedUnexpectedly, locally_started=false.
    let mut locally_started = true;
    mqk_daemon::state::run_session_controller_tick(&st, schedule, &mut locally_started, now)
        .await;
    assert!(!locally_started, "SH04: tick 1 must reset locally_started");

    // Force locally_started back to true to simulate a race or double-fire.
    locally_started = true;

    // Tick 2: same conditions again.
    mqk_daemon::state::run_session_controller_tick(&st, schedule, &mut locally_started, now)
        .await;

    assert!(
        !locally_started,
        "SH04: tick 2 must also reset locally_started — \
         consecutive RunEndedUnexpectedly ticks are idempotent"
    );
}

// ---------------------------------------------------------------------------
// SH05: Startup gate ordering: paper+paper deployment gate fires before WS
//        continuity gate.
//
// paper+paper (PT-TRUTH-01: start_allowed=false) → deployment gate (403)
// fires before any WS check.  WS continuity is set to Live to prove the
// deployment gate is the definitive blocker.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sh05_deployment_gate_fires_before_ws_continuity_gate() {
    let st = paper_paper_state();
    // Arm integrity so integrity is not the confounding blocker.
    st.integrity.write().await.disarmed = false;
    // Set WS continuity to Live — if WS gate ran first, it would pass.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "sh05-test-msg".to_string(),
        last_event_at: "2026-04-14T15:00:00Z".to_string(),
    })
    .await;

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SH05: paper+paper must fail at deployment gate");

    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.deployment_mode_unproven",
        "SH05: deployment gate must fire before WS gate; fault_class: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// SH06: Startup gate ordering: integrity gate fires before WS continuity gate.
//
// paper+alpaca with integrity disarmed → integrity gate (403) fires before WS.
// WS continuity set to Live to prove integrity is the definitive blocker.
// Cross-reference: BRK-00R-04 scenario_ws_continuity_gate_brk00r04.rs P01.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sh06_integrity_gate_fires_before_ws_continuity_gate() {
    let st = paper_alpaca_state();
    // Leave integrity disarmed (default for new_for_test_with_broker_kind).
    // Set WS continuity to Live — if WS gate ran first, it would pass.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "sh06-test-msg".to_string(),
        last_event_at: "2026-04-14T15:00:00Z".to_string(),
    })
    .await;

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SH06: disarmed integrity must block before WS gate");

    assert_eq!(
        err.fault_class(),
        "runtime.control_refusal.integrity_disarmed",
        "SH06: integrity gate must fire before WS gate; fault_class: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// SH07: live_routing_enabled is always explicit false in system/status.
//
// The operator surface must never return null or true for this field in paper
// mode, regardless of runtime state (idle, active, halted).
// Cross-reference: scenario_daemon_routes.rs ap04a/ap04b.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sh07_live_routing_enabled_is_always_explicit_false_in_paper_mode() {
    let st = paper_alpaca_state();
    let router = build_router(Arc::clone(&st));

    let (status, j) = get_json(router, "/api/v1/system/status").await;

    assert_eq!(
        status,
        StatusCode::OK,
        "SH07: system/status must return 200; body: {j}"
    );
    assert!(
        j["live_routing_enabled"].is_boolean(),
        "SH07: live_routing_enabled must be a boolean (not null, not absent); body: {j}"
    );
    assert_eq!(
        j["live_routing_enabled"], false,
        "SH07: live_routing_enabled must be explicit false in paper mode (not null, not true); \
         body: {j}"
    );
}

// ---------------------------------------------------------------------------
// SH08: try_autonomous_arm gate 1: halted integrity unconditionally refuses.
//
// When integrity.halted=true, autonomous arm must refuse even if the DB
// were to contain ARMED state.  Operator halt is not reversible by the
// controller.
// Cross-reference: AU-05 scenario_autonomous_paper_day_auton01.rs.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sh08_try_autonomous_arm_refuses_when_halted() {
    let st = paper_alpaca_state();
    {
        let mut ig = st.integrity.write().await;
        ig.halted = true;
        ig.disarmed = true;
    }

    let result = st.try_autonomous_arm().await;

    assert!(
        result.is_err(),
        "SH08: try_autonomous_arm must refuse when integrity.halted=true"
    );
    let reason = result.unwrap_err();
    assert!(
        reason.contains("halted"),
        "SH08: refusal reason must mention 'halted'; got: {reason}"
    );
}

// ---------------------------------------------------------------------------
// SH09: try_autonomous_arm gate 3: no DB → refuses.
//
// Without a DB, autonomous arm cannot verify prior session state and must
// refuse.  Fail-closed: missing DB is not authorization to auto-arm.
// Cross-reference: AU-06 scenario_autonomous_paper_day_auton01.rs.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sh09_try_autonomous_arm_refuses_without_db() {
    let st = paper_alpaca_state();
    // Integrity not halted (gate 1 skipped), not yet armed (gate 2 skipped).
    // Gate 3 (no DB) should fire.

    let result = st.try_autonomous_arm().await;

    assert!(
        result.is_err(),
        "SH09: try_autonomous_arm must refuse without a DB (gate 3)"
    );
    let reason = result.unwrap_err();
    assert!(
        reason.to_lowercase().contains("db")
            || reason.to_lowercase().contains("database"),
        "SH09: refusal reason must mention DB; got: {reason}"
    );
}

// ---------------------------------------------------------------------------
// SH10: Flatten route: paper mode + no DB → 503 db_unavailable.
//
// The flatten route requires a DB for durable outbox writes. Without DB it
// must fail closed — not silently accept.
// Cross-reference: PSF-02 scenario_paper_flatten_psf01.rs.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sh10_flatten_route_paper_no_db_returns_503() {
    let st = paper_paper_state();
    let router = build_router(st);

    let (status, j) = post_action(
        router,
        serde_json::json!({
            "action_key": "flatten-paper-positions",
            "reason": "sh10-hygiene-test"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "SH10: paper flatten without DB must return 503; body: {j}"
    );
    assert_eq!(j["accepted"], false, "SH10: must not be accepted; body: {j}");
    assert_eq!(
        j["disposition"].as_str(),
        Some("db_unavailable"),
        "SH10: disposition must be db_unavailable; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// SH11: Flatten route: non-paper mode → 403 not_paper_mode.
//
// flatten-paper-positions is paper-only.  Live-capital mode must be blocked
// before any DB or position logic runs.
// Cross-reference: PSF-01 scenario_paper_flatten_psf01.rs.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sh11_flatten_route_live_capital_returns_403_not_paper_mode() {
    let st = live_capital_state();
    let router = build_router(st);

    let (status, j) = post_action(
        router,
        serde_json::json!({
            "action_key": "flatten-paper-positions",
            "reason": "sh11-hygiene-test"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "SH11: non-paper flatten must return 403; body: {j}"
    );
    assert_eq!(j["accepted"], false, "SH11: must not be accepted; body: {j}");
    assert_eq!(
        j["disposition"].as_str(),
        Some("not_paper_mode"),
        "SH11: disposition must be not_paper_mode; body: {j}"
    );
}
