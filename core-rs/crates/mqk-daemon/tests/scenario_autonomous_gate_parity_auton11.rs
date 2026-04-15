//! # AUTON-11 — Autonomous gate parity proof
//!
//! ## Purpose
//!
//! Proves that the autonomous session controller path
//! (`run_session_controller_tick` → `attempt_auto_start`) does not bypass any
//! canonical paper-execution gate enforced by `start_execution_runtime`.
//!
//! Prior tests (AU-03..AU-05) prove `start_execution_runtime` gate behaviour
//! by calling the HTTP route directly.  Those tests leave a proof gap:
//! they do not exercise the autonomous controller tick path itself.  This
//! file closes that gap.
//!
//! ## What this file proves
//!
//! | Test  | Gate blocked         | Entry point                                 | Claim                                      |
//! |-------|----------------------|---------------------------------------------|--------------------------------------------|
//! | AP-01 | BRK-00R-04 WS gate   | `run_session_controller_tick` in-session    | WS=ColdStartUnproven → locally_started=false + StartRefused |
//! | AP-02 | BRK-00R-04 WS gate   | `run_session_controller_tick` in-session    | WS=GapDetected → locally_started=false + StartRefused |
//! | AP-03 | try_autonomous_arm   | `run_session_controller_tick` in-session    | integrity=halted → arm refused before start_execution_runtime |
//! | AP-04 | BRK-09R reconcile    | `run_session_controller_tick` in-session    | reconcile=dirty → locally_started=false + StartRefused |
//! | AP-05 | try_autonomous_arm   | `run_session_controller_tick` in-session    | disarmed + no DB → arm refused (Gate 3)    |
//!
//! ## What is NOT claimed
//!
//! - DB-backed gates (TV-02C artifact, TV-04A capital policy, B2A registry,
//!   halt-lifecycle) — these fire after `db_pool()` which requires a real DB.
//!   They are separately proven by `scenario_artifact_deployability_tv02.rs`,
//!   `scenario_capital_policy_tv04.rs`, and `scenario_autonomous_paper_day_auton01.rs`.
//! - Wall-clock or broker connectivity.
//!
//! ## Architecture note
//!
//! The autonomous controller (`session_controller.rs`) calls
//! `start_execution_runtime` exactly — the same function the supervised HTTP
//! route calls.  Gate parity is structural: there is no separate "autonomous
//! start" code path.  These tests provide the missing patch-local proof that
//! the controller respects each canonical blocker at the tick level.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use mqk_daemon::state;
use state::{
    AlpacaWsContinuityState, AutonomousSessionSchedule, AutonomousSessionTruth, BrokerKind,
    ReconcileStatusSnapshot, SessionWindow,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_paper_alpaca() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ))
}

/// A FixedUtcWindow covering 14:30–21:00 UTC and a `now` inside that window
/// (2026-04-14 15:00:00 UTC).  Deterministic regardless of test wall-clock.
fn in_session_schedule() -> (AutonomousSessionSchedule, chrono::DateTime<Utc>) {
    let window =
        SessionWindow::parse("14:30", "21:00").expect("helper: fixed test window must parse");
    let now = Utc.with_ymd_and_hms(2026, 4, 14, 15, 0, 0).unwrap();
    (AutonomousSessionSchedule::FixedUtcWindow(window), now)
}

// ---------------------------------------------------------------------------
// AP-01 — WS=ColdStartUnproven → BRK-00R-04 blocks inside start_execution_runtime
//
// Preconditions:
//   - integrity armed (so we reach the WS gate inside start_execution_runtime)
//   - WS stays at the default ColdStartUnproven for paper+alpaca
//
// Proof: autonomous controller is in-session, not locally-started →
//   attempt_auto_start fires → try_autonomous_arm returns Ok (already armed) →
//   start_execution_runtime fires → BRK-00R-04 gate refuses →
//   attempt_auto_start logs refusal, sets StartRefused →
//   locally_started remains false.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ap01_ws_cold_start_unproven_blocks_autonomous_start() {
    let st = make_paper_alpaca();

    // Arm integrity so we reach the WS gate, not the integrity gate.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    // Default paper+alpaca WS state is ColdStartUnproven — do not advance.
    assert_eq!(
        st.alpaca_ws_continuity().await,
        AlpacaWsContinuityState::ColdStartUnproven,
        "AP-01 precondition: WS must be ColdStartUnproven"
    );

    let (schedule, now) = in_session_schedule();
    let mut locally_started = false;
    state::run_session_controller_tick(&st, schedule, &mut locally_started, now).await;

    assert!(
        !locally_started,
        "AP-01: locally_started must be false when WS is ColdStartUnproven \
         (BRK-00R-04 gate must block inside start_execution_runtime)"
    );
    let truth = st.autonomous_session_truth().await;
    assert!(
        matches!(truth, AutonomousSessionTruth::StartRefused { .. }),
        "AP-01: autonomous session truth must be StartRefused; got: {truth:?}"
    );
    // Verify no run was started.
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "AP-01: no locally-owned run must exist after a gate-blocked start"
    );
}

// ---------------------------------------------------------------------------
// AP-02 — WS=GapDetected → BRK-00R-04 blocks inside start_execution_runtime
//
// Same structure as AP-01 but exercises the GapDetected continuity state.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ap02_ws_gap_detected_blocks_autonomous_start() {
    let st = make_paper_alpaca();

    // Arm integrity so we reach the WS gate.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    // Advance WS to GapDetected.
    st.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
        last_message_id: Some("alpaca:order-x:filled:2026-04-14T10:00:00Z".to_string()),
        last_event_at: Some("2026-04-14T10:00:00Z".to_string()),
        detail: "simulated WS disconnect for AP-02".to_string(),
    })
    .await;

    let (schedule, now) = in_session_schedule();
    let mut locally_started = false;
    state::run_session_controller_tick(&st, schedule, &mut locally_started, now).await;

    assert!(
        !locally_started,
        "AP-02: locally_started must be false when WS is GapDetected \
         (BRK-00R-04 gate must block inside start_execution_runtime)"
    );
    let truth = st.autonomous_session_truth().await;
    assert!(
        matches!(truth, AutonomousSessionTruth::StartRefused { .. }),
        "AP-02: autonomous session truth must be StartRefused; got: {truth:?}"
    );
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "AP-02: no locally-owned run must exist after a gate-blocked start"
    );
}

// ---------------------------------------------------------------------------
// AP-03 — integrity=halted → try_autonomous_arm Gate 1 refuses
//
// When the operator has halted the system, try_autonomous_arm returns an Err
// before start_execution_runtime is ever called.  The autonomous controller
// logs the refusal, sets StartRefused, and leaves locally_started false.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ap03_halted_integrity_blocks_autonomous_arm_and_start() {
    let st = make_paper_alpaca();

    // Assert a halt — operator halt wins unconditionally.
    {
        let mut ig = st.integrity.write().await;
        ig.halted = true;
        ig.disarmed = true;
    }

    // Set WS to Live so WS is NOT the blocking condition (halt is).
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "msg-001".to_string(),
        last_event_at: "2026-04-14T14:30:00Z".to_string(),
    })
    .await;

    let (schedule, now) = in_session_schedule();
    let mut locally_started = false;
    state::run_session_controller_tick(&st, schedule, &mut locally_started, now).await;

    assert!(
        !locally_started,
        "AP-03: locally_started must be false when integrity is halted \
         (try_autonomous_arm Gate 1 must refuse before start_execution_runtime is called)"
    );
    let truth = st.autonomous_session_truth().await;
    assert!(
        matches!(truth, AutonomousSessionTruth::StartRefused { .. }),
        "AP-03: autonomous session truth must be StartRefused; got: {truth:?}"
    );
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "AP-03: no locally-owned run must exist after a halted-arm refusal"
    );
}

// ---------------------------------------------------------------------------
// AP-04 — reconcile=dirty → BRK-09R blocks inside start_execution_runtime
//
// When the reconcile state is dirty (prior session ended with broker/local
// drift), BRK-09R refuses start.  The autonomous controller must propagate
// this gate failure; it cannot start a run through a dirty reconcile state.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ap04_dirty_reconcile_blocks_autonomous_start() {
    let st = make_paper_alpaca();

    // Arm integrity and set WS=Live so those gates pass.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "msg-001".to_string(),
        last_event_at: "2026-04-14T14:30:00Z".to_string(),
    })
    .await;

    // Publish a dirty reconcile snapshot.
    st.publish_reconcile_snapshot(ReconcileStatusSnapshot {
        status: "dirty".to_string(),
        last_run_at: Some("2026-04-14T14:00:00Z".to_string()),
        snapshot_watermark_ms: None,
        mismatched_positions: 1,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some("broker/local drift: AP-04 test".to_string()),
    })
    .await;

    let (schedule, now) = in_session_schedule();
    let mut locally_started = false;
    state::run_session_controller_tick(&st, schedule, &mut locally_started, now).await;

    assert!(
        !locally_started,
        "AP-04: locally_started must be false when reconcile is dirty \
         (BRK-09R gate must block inside start_execution_runtime)"
    );
    let truth = st.autonomous_session_truth().await;
    assert!(
        matches!(truth, AutonomousSessionTruth::StartRefused { .. }),
        "AP-04: autonomous session truth must be StartRefused; got: {truth:?}"
    );
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "AP-04: no locally-owned run must exist after a dirty-reconcile refusal"
    );
}

// ---------------------------------------------------------------------------
// AP-05 — disarmed + no DB → try_autonomous_arm Gate 3 refuses
//
// When the daemon is freshly started (in-memory disarmed=true, not halted)
// and has no DB configured, try_autonomous_arm cannot verify prior arm state
// and must refuse.  This prevents autonomous start when the system has never
// been manually armed by an operator.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ap05_disarmed_no_db_blocks_autonomous_arm() {
    let st = make_paper_alpaca();

    // Preconditions: fresh test AppState has no DB and starts disarmed.
    assert!(
        st.db.is_none(),
        "AP-05 precondition: test AppState must have no DB"
    );
    {
        let ig = st.integrity.read().await;
        assert!(
            ig.disarmed,
            "AP-05 precondition: fresh AppState starts disarmed"
        );
        assert!(
            !ig.halted,
            "AP-05 precondition: fresh AppState is not halted"
        );
    }

    // Set WS=Live so it is not the blocking condition.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "msg-001".to_string(),
        last_event_at: "2026-04-14T14:30:00Z".to_string(),
    })
    .await;

    let (schedule, now) = in_session_schedule();
    let mut locally_started = false;
    state::run_session_controller_tick(&st, schedule, &mut locally_started, now).await;

    assert!(
        !locally_started,
        "AP-05: locally_started must be false when disarmed with no DB \
         (try_autonomous_arm Gate 3 must refuse — cannot verify prior arm state)"
    );
    let truth = st.autonomous_session_truth().await;
    assert!(
        matches!(truth, AutonomousSessionTruth::StartRefused { .. }),
        "AP-05: autonomous session truth must be StartRefused; got: {truth:?}"
    );
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "AP-05: no locally-owned run must exist after a no-DB arm refusal"
    );
}
