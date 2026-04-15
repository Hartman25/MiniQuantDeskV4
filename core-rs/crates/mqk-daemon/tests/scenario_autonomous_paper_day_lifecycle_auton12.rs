//! # AUTON-12 — Canonical unattended paper-day lifecycle proof
//!
//! ## Purpose
//!
//! Closes the proof gap left open after AUTON-11 (gate parity: blocked cases
//! only) and AUTON-PAPER-01 (AU-01..AU-16: sub-lifecycles, recovery, alerts).
//! Neither of those files drives `run_session_controller_tick` through the
//! session-boundary stop arm (`(false, true, true)` → `StoppedAtBoundary`),
//! and neither proves that all pre-DB start gates pass together under valid
//! conditions through the tick.
//!
//! ## What this file proves
//!
//! | Test  | Kind        | Claim                                                                                          |
//! |-------|-------------|------------------------------------------------------------------------------------------------|
//! | AL-01 | DB-backed   | Tick-driven session-boundary stop: locally_started=true + out-of-session tick → `StoppedAtBoundary` + DB run = Stopped |
//! | AL-02 | Pure, no-DB | Tick-driven start: armed + WS=Live + all pre-DB gates pass → `StartRefused` with `service_unavailable` (DB is the only missing piece) |
//!
//! ## What is NOT claimed
//!
//! - Wall-clock soak or broker network connectivity.
//! - Full unattended start through `start_execution_runtime` — that path
//!   requires a DB, which is proven by the gate chain reaching the DB fault in AL-02.
//! - Multi-day soak or live-capital semantics.
//! - Recovery or restart behaviour (proven by AU-10F in AUTON-PAPER-01).
//!
//! ## Architecture note
//!
//! AL-01 uses `establish_db_backed_active_run_for_test` (the AUTON-PAPER-03B
//! seam) to establish coherent DB + in-memory run state without invoking the
//! full start path.  This is the minimal honest approach: the test-only seam
//! establishes exactly the state the real start path would leave, proving that
//! `run_session_controller_tick` dispatches correctly to `attempt_auto_stop`
//! and that `stop_execution_runtime` produces the canonical durable outcome.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use mqk_daemon::state;
use state::{
    AlpacaWsContinuityState, AutonomousSessionSchedule, AutonomousSessionTruth, BrokerKind,
    DeploymentMode, SessionWindow,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_paper_alpaca() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ))
}

/// Fixed UTC window 14:30–21:00 and a deterministic in-session / out-of-session pair.
fn session_fixtures() -> (
    AutonomousSessionSchedule,
    chrono::DateTime<Utc>,
    chrono::DateTime<Utc>,
) {
    let window = SessionWindow::parse("14:30", "21:00")
        .expect("AUTON-12 helper: fixed test window must parse");
    let in_session = Utc.with_ymd_and_hms(2026, 4, 14, 15, 0, 0).unwrap();
    let out_of_session = Utc.with_ymd_and_hms(2026, 4, 14, 22, 0, 0).unwrap();
    (
        AutonomousSessionSchedule::FixedUtcWindow(window),
        in_session,
        out_of_session,
    )
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
            .expect("AUTON-12 DB test: failed to connect to MQK_DATABASE_URL"),
    )
}

// ---------------------------------------------------------------------------
// AL-01 (DB-backed) — Tick-driven session-boundary stop → StoppedAtBoundary
//
// Preconditions (simulate post-start controller state):
//   - DB-backed active run established via AUTON-PAPER-03B seam
//   - locally_started = true  (the controller's local variable after auto-start)
//   - schedule's now is outside the session window
//
// Proof: tick arm (false, true, true) fires →
//   attempt_auto_stop → stop_execution_runtime:
//     - sends Stop to the injected loop; loop exits ("test loop stopped")
//     - db_pool() succeeds (DB configured)
//     - fetch_run finds run in Running status
//     - stop_run writes Stopped to DB
//   → locally_started becomes false
//   → truth becomes StoppedAtBoundary
//   → locally_owned_run_id() returns None (loop joined + handle taken)
//   → DB run status = Stopped (durable evidence)
//
// Skips gracefully when MQK_DATABASE_URL is not set.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn al01_tick_driven_session_boundary_stop_produces_stopped_at_boundary() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("AL-01: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    mqk_db::migrate(&pool)
        .await
        .expect("AL-01: migration failed");

    let run_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        b"auton12.al01.tick_driven_boundary_stop",
    );

    // Idempotent pre-test cleanup.
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("AL-01: pre-test run cleanup failed");

    let mut st_inner = state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st_inner.set_adapter_id_for_test("auton12-al01");
    let st = Arc::new(st_inner);

    // Arm integrity so run state is coherent.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    // Establish DB-backed active run + inject fake execution loop (AUTON-PAPER-03B seam).
    st.establish_db_backed_active_run_for_test(run_id)
        .await
        .expect("AL-01: DB-backed active run must be established");

    // Verify preconditions before the tick.
    assert!(
        st.locally_owned_run_id().await.is_some(),
        "AL-01 precondition: locally_owned_run_id must be Some before the stop tick"
    );
    {
        let run = mqk_db::fetch_run(&pool, run_id)
            .await
            .expect("AL-01: fetch pre-stop run failed");
        assert!(
            matches!(run.status, mqk_db::RunStatus::Running),
            "AL-01 precondition: DB run must be Running before the stop tick; got: {:?}",
            run.status
        );
    }

    // Simulate the controller's local variable state after a successful auto-start.
    let mut locally_started = true;

    let (schedule, _in_session, out_of_session) = session_fixtures();
    // Drive the out-of-session tick: arm (false, true, true) fires → attempt_auto_stop.
    state::run_session_controller_tick(&st, schedule, &mut locally_started, out_of_session).await;

    // ── Core lifecycle claim ─────────────────────────────────────────────────

    assert!(
        !locally_started,
        "AL-01: locally_started must be false after session-boundary stop"
    );

    let truth = st.autonomous_session_truth().await;
    assert!(
        matches!(truth, AutonomousSessionTruth::StoppedAtBoundary { .. }),
        "AL-01: autonomous session truth must be StoppedAtBoundary after tick-driven stop; \
         got: {truth:?}"
    );

    // ── Durable evidence ────────────────────────────────────────────────────

    let run = mqk_db::fetch_run(&pool, run_id)
        .await
        .expect("AL-01: fetch post-stop run failed");
    assert!(
        matches!(run.status, mqk_db::RunStatus::Stopped),
        "AL-01: DB run must be Stopped after session-boundary stop; got: {:?}",
        run.status
    );

    // No dangling local ownership after stop.
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "AL-01: locally_owned_run_id must be None after session-boundary stop (loop joined)"
    );
}

// ---------------------------------------------------------------------------
// AL-02 (pure, no-DB) — Tick-driven start: all pre-DB gates pass →
//                        StartRefused at DB service_unavailable gate
//
// Preconditions (all pre-DB admission gates satisfied for Paper+Alpaca):
//   - integrity armed  (disarmed=false, halted=false)           → gate 2 passes
//   - WS = Live                                                 → BRK-00R-04 passes
//   - reconcile = "unknown" (boot default)                      → BRK-09R passes
//   - no artifact path set (NotConfigured)                      → TV-02C passes
//   - no parity evidence path (NotConfigured)                   → TV-03C passes
//   - Paper mode (not LiveCapital)                              → TV-04F passes
//   - no capital policy (NotConfigured)                         → TV-04A passes
//   - no deployment economics (NotConfigured)                   → TV-04D passes
//   - no strategy fleet (MQK_STRATEGY_IDS absent → Dormant)    → B1A passes
//   - no DB                                                     → db_pool() fires
//
// Proof: tick arm (true, false, _) fires →
//   attempt_auto_start →
//   try_autonomous_arm returns Ok (already armed: early-exit, no DB needed) →
//   start_execution_runtime traverses all pre-DB gates without blocking →
//   db_pool() fails (service_unavailable: "runtime DB is not configured") →
//   attempt_auto_start sets StartRefused { detail: "…service_unavailable…" }
//
// This proves all pre-DB start gates pass under canonical preconditions and
// that only the DB prevents a full autonomous paper-day start.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn al02_tick_driven_start_passes_all_pre_db_gates_reaches_db_gate() {
    let st = make_paper_alpaca();

    // Arm integrity — passes gate 2.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    // WS = Live — passes BRK-00R-04.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:order-al02:accepted:2026-04-14T14:30:00Z".to_string(),
        last_event_at: "2026-04-14T14:30:00Z".to_string(),
    })
    .await;

    // reconcile defaults to "unknown" — passes BRK-09R ("dirty"/"stale" block, "unknown" passes).
    // No artifact, no capital policy, no deployment economics, no strategy fleet — all pass-through.

    assert!(
        st.db.is_none(),
        "AL-02 precondition: test AppState must have no DB (db_pool() is the target gate)"
    );

    let mut locally_started = false;
    let (schedule, in_session, _out_of_session) = session_fixtures();

    state::run_session_controller_tick(&st, schedule, &mut locally_started, in_session).await;

    // Start was refused at the DB gate, not at any pre-DB gate.
    assert!(
        !locally_started,
        "AL-02: locally_started must be false (start failed at DB gate, not at a pre-DB gate)"
    );

    let truth = st.autonomous_session_truth().await;
    let AutonomousSessionTruth::StartRefused { ref detail } = truth else {
        panic!("AL-02: truth must be StartRefused; got: {truth:?}");
    };

    // The detail must name the DB / service_unavailable fault — proving all pre-DB gates passed.
    assert!(
        detail.contains("service_unavailable"),
        "AL-02: StartRefused detail must mention 'service_unavailable' (DB gate), \
         proving all pre-DB gates were satisfied; got detail: {detail:?}"
    );

    // Negative guard: detail must not name any pre-DB gate as the blocker.
    // These substrings appear in the fault_class or error message only if those gates fired.
    for pre_db_marker in &[
        "integrity_disarmed",
        "ws_continuity",
        "reconcile_dirty",
        "artifact_intake",
        "artifact_deployability",
        "parity_evidence",
        "live_capital",
        "capital_policy",
        "deployment_economics",
        "native_strategy_bootstrap",
        "deployment_mode_unproven",
    ] {
        assert!(
            !detail.contains(pre_db_marker),
            "AL-02: StartRefused detail must not mention pre-DB gate marker '{pre_db_marker}'; \
             this would mean a pre-DB gate fired when all should have passed; \
             got detail: {detail:?}"
        );
    }

    // No dangling local ownership.
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "AL-02: locally_owned_run_id must be None after a DB-gate-blocked start"
    );
}
