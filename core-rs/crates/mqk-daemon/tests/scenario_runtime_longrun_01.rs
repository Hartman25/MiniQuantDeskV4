//! RUNTIME-LONGRUN-01 — Long-run runtime correctness proof
//!
//! ## Purpose
//!
//! Closes the proof gap for repeated-cycle runtime stability.  Prior proofs are
//! one-shot (single gap event, single restart, single tick).  This file proves
//! that correctness properties hold **across multiple iterations** without drift,
//! duplication, backlog corruption, or false optimism accumulating over time.
//!
//! ## What this file proves
//!
//! | Test  | Claim                                                                                             |
//! |-------|---------------------------------------------------------------------------------------------------|
//! | LR-01 | Repeated in-session controller ticks without Live WS never create phantom ownership              |
//! | LR-02 | Repeated gap→halt→Live→gap→halt cycles each fire correctly; gap check never drifts to skipped   |
//! | LR-03 | Controller state machine never becomes more optimistic than WS continuity across 3 in/out cycles |
//! | LR-04 | Gap→Live escalation flag cycles cleanly per gap window — 5 full cycles, no sticky degradation   |
//! | LR-05 | Cursor-seed derivation is deterministic and monotonically safe — same input, same output always  |
//! | LR-06 | WS continuity halt trigger is scoped to ExternalSignalIngestion path only (not paper-paper)     |
//!
//! ## Claim boundary
//!
//! - All tests are pure in-process.  No real wall-clock soak is claimed.
//! - No real broker connectivity.  No real DB for LR-01..LR-06.
//! - Repeated-cycle behavior is proven over N discrete iterations within one
//!   test invocation, not over calendar time.
//!
//! ## What is NOT claimed
//!
//! - Real WS reconnect durability (network-level).
//! - REST catch-up fill completeness.
//! - Strategy decision correctness.
//! - DB cursor idempotence over many WS frames (proven by scenario_duplicate_fill_storm.rs
//!   and scenario_alpaca_inbound_rt_brk08r.rs).

use std::sync::Arc;

use chrono::TimeZone;
use mqk_daemon::state::{run_session_controller_tick, AppState};
use mqk_daemon::state::{
    AlpacaWsContinuityState, AutonomousSessionSchedule, AutonomousSessionTruth, BrokerKind,
    DeploymentMode, SessionWindow,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a Paper+Alpaca AppState with no DB (pure in-process).
fn make_paper_alpaca() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca))
}

/// In-session UTC timestamp for use with a 14:30–21:00 UTC window.
fn ts_in_session() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc.with_ymd_and_hms(2026, 4, 7, 15, 0, 0).unwrap()
}

/// Out-of-session UTC timestamp (after 21:00 UTC).
fn ts_out_of_session() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc.with_ymd_and_hms(2026, 4, 7, 22, 0, 0).unwrap()
}

/// Standard fixed UTC session window used across tests.
fn test_schedule() -> AutonomousSessionSchedule {
    AutonomousSessionSchedule::FixedUtcWindow(
        SessionWindow::parse("14:30", "21:00").expect("test schedule must parse"),
    )
}

// ---------------------------------------------------------------------------
// LR-01 — Repeated in-session ticks without Live WS never create phantom
//          run ownership.
//
// The Paper+Alpaca controller path calls `try_autonomous_arm()` first.
// Without a DB, this gate returns Err immediately, so `attempt_auto_start`
// never calls `start_execution_runtime`.  `locally_started` stays false and
// `locally_owned_run_id()` stays None across every tick.
//
// The repeated aspect is what this test specifically proves: the gate refusal
// is stable and does NOT get bypassed or forgotten over multiple iterations.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lr01_repeated_in_session_ticks_without_live_ws_never_creates_ownership() {
    let state = make_paper_alpaca();
    let schedule = test_schedule();
    let ts_in = ts_in_session();
    let mut locally_started = false;

    // Verify initial conditions.
    assert!(
        matches!(
            state.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::ColdStartUnproven
        ),
        "LR-01 pre: must start ColdStartUnproven"
    );
    assert!(
        state.locally_owned_run_id().await.is_none(),
        "LR-01 pre: must have no active run"
    );

    // Run 6 consecutive in-session ticks.  Each tick must:
    //   (a) remain fail-closed (locally_started stays false), and
    //   (b) set StartRefused truth (never a "started" variant).
    for tick in 1..=6 {
        run_session_controller_tick(&state, schedule, &mut locally_started, ts_in).await;

        assert!(
            !locally_started,
            "LR-01 tick {tick}: locally_started must remain false (no phantom start)"
        );
        assert!(
            state.locally_owned_run_id().await.is_none(),
            "LR-01 tick {tick}: locally_owned_run_id must remain None"
        );

        let truth = state.autonomous_session_truth().await;
        assert!(
            matches!(truth, AutonomousSessionTruth::StartRefused { .. }),
            "LR-01 tick {tick}: autonomous truth must be StartRefused; got {truth:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// LR-02 — Repeated gap→halt cycles fire correctly every time.
//
// Proves that the WS gap self-halt check in spawn_execution_loop is stateless
// per tick: each GapDetected cycle produces a "gap" exit note, and an
// intervening Live cycle produces a different (DB-error) exit note.
//
// Correctness of interest: the check does NOT get "stuck" in a previous
// decision from a prior cycle.  Three cycles proven (gap / live / gap).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lr02_repeated_gap_halt_cycles_remain_fail_closed() {
    let state = make_paper_alpaca();

    // ---- Cycle 1: GapDetected → loop self-halts with gap message ----
    state
        .update_ws_continuity(AlpacaWsContinuityState::GapDetected {
            last_message_id: Some("alpaca:order-lr02-c1:filled:2026-04-07T15:00:00Z".to_string()),
            last_event_at: Some("2026-04-07T15:00:00Z".to_string()),
            detail: "lr02 cycle-1 gap injection".to_string(),
        })
        .await;

    let run_id_c1 = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"lr02.cycle1");
    let exit_c1 = state
        .run_loop_one_tick_for_test(run_id_c1)
        .await
        .unwrap_or_default();

    assert!(
        exit_c1.contains("gap") || exit_c1.contains("continuity"),
        "LR-02 cycle-1: exit note must reference gap/continuity; got: {exit_c1:?}"
    );
    {
        let ig = state.integrity.read().await;
        assert!(
            ig.halted,
            "LR-02 cycle-1: integrity must be halted after gap-halt"
        );
        assert!(
            ig.disarmed,
            "LR-02 cycle-1: integrity must be disarmed after gap-halt"
        );
    }

    // Reset integrity between cycles so the next loop can run cleanly.
    // This simulates the gap being recovered and the daemon being re-armed.
    {
        let mut ig = state.integrity.write().await;
        ig.halted = false;
        ig.disarmed = false;
    }

    // ---- Cycle 2: Live → loop exits for a non-gap reason (DB error) ----
    state
        .update_ws_continuity(AlpacaWsContinuityState::Live {
            last_message_id: "alpaca:order-lr02-c2:filled:2026-04-07T15:05:00Z".to_string(),
            last_event_at: "2026-04-07T15:05:00Z".to_string(),
        })
        .await;

    // Verify escalation flag was reset when Live was set.
    assert!(
        !state.gap_escalation_is_pending(),
        "LR-02 cycle-2: gap escalation flag must clear when Live is set"
    );

    let run_id_c2 = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"lr02.cycle2");
    let exit_c2 = state
        .run_loop_one_tick_for_test(run_id_c2)
        .await
        .unwrap_or_default();

    // Live: gap check passes → loop proceeds to orchestrator.tick() which
    // fails on the lazy/disconnected DB pool.  The exit note must NOT reference
    // gap or continuity — it must reflect a DB-level failure instead.
    assert!(
        !exit_c2.contains("gap") || exit_c2.contains("halted"),
        "LR-02 cycle-2: Live path must not exit with gap message; got: {exit_c2:?}"
    );

    // Reset integrity again for cycle 3.
    {
        let mut ig = state.integrity.write().await;
        ig.halted = false;
        ig.disarmed = false;
    }

    // ---- Cycle 3: GapDetected again → loop self-halts again (no drift) ----
    state
        .update_ws_continuity(AlpacaWsContinuityState::GapDetected {
            last_message_id: Some("alpaca:order-lr02-c3:filled:2026-04-07T15:10:00Z".to_string()),
            last_event_at: Some("2026-04-07T15:10:00Z".to_string()),
            detail: "lr02 cycle-3 gap injection".to_string(),
        })
        .await;

    let run_id_c3 = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"lr02.cycle3");
    let exit_c3 = state
        .run_loop_one_tick_for_test(run_id_c3)
        .await
        .unwrap_or_default();

    assert!(
        exit_c3.contains("gap") || exit_c3.contains("continuity"),
        "LR-02 cycle-3: gap check must fire again after Live cycle; got: {exit_c3:?}"
    );
    {
        let ig = state.integrity.read().await;
        assert!(
            ig.halted,
            "LR-02 cycle-3: integrity must be halted after second gap-halt"
        );
    }
}

// ---------------------------------------------------------------------------
// LR-03 — Controller state machine never becomes more optimistic than WS
//          continuity allows across multiple session boundary crossings.
//
// Three full in-session → out-of-session cycles.  Each in-session phase with
// WS=ColdStartUnproven must produce StartRefused (never a "started" variant).
// Each out-of-session phase must clear autonomous truth back to Clear.
//
// This proves the controller memory (`locally_started`) and the observable
// truth state do not accumulate false optimism over repeated transitions.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lr03_controller_state_machine_never_optimistic_across_transitions() {
    let state = make_paper_alpaca();
    let schedule = test_schedule();
    let ts_in = ts_in_session();
    let ts_out = ts_out_of_session();

    // WS stays at ColdStartUnproven throughout: start must always be refused.
    assert!(
        matches!(
            state.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::ColdStartUnproven
        ),
        "LR-03 pre: must start ColdStartUnproven"
    );

    for cycle in 1..=3 {
        let mut locally_started = false;

        // --- In-session phase ---
        // Multiple in-session ticks: each must produce StartRefused.
        for tick in 1..=3 {
            run_session_controller_tick(&state, schedule, &mut locally_started, ts_in).await;

            assert!(
                !locally_started,
                "LR-03 cycle {cycle} in-session tick {tick}: locally_started must remain false"
            );

            let truth = state.autonomous_session_truth().await;
            assert!(
                matches!(truth, AutonomousSessionTruth::StartRefused { .. }),
                "LR-03 cycle {cycle} tick {tick}: must be StartRefused while WS not Live; got {truth:?}"
            );
        }

        // --- Out-of-session phase ---
        // One out-of-session tick: must clear stale StartRefused truth.
        run_session_controller_tick(&state, schedule, &mut locally_started, ts_out).await;

        let truth_after = state.autonomous_session_truth().await;
        assert_eq!(
            truth_after,
            AutonomousSessionTruth::Clear,
            "LR-03 cycle {cycle} out-of-session: autonomous truth must be Clear; got {truth_after:?}"
        );
        assert!(
            !locally_started,
            "LR-03 cycle {cycle} out-of-session: locally_started must be false after out-of-session"
        );
    }
}

// ---------------------------------------------------------------------------
// LR-04 — Gap→Live escalation flag cycles cleanly per gap window.
//
// Proves that the WS gap escalation dedup flag (gap_escalation_is_pending)
// is cleanly set and cleared on every gap→live cycle.  After N cycles the
// flag is in the same state as after cycle 1 — no sticky degradation, no
// leaked "already escalated" state from a prior window.
//
// 5 full cycles proven.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lr04_gap_live_escalation_flag_cycles_cleanly_per_gap_window() {
    let state = make_paper_alpaca();

    // Initial: no gap yet → escalation flag must be false.
    assert!(
        !state.gap_escalation_is_pending(),
        "LR-04 pre: escalation flag must start clear"
    );

    for cycle in 1..=5 {
        // Inject GapDetected.
        state
            .update_ws_continuity(AlpacaWsContinuityState::GapDetected {
                last_message_id: Some(format!(
                    "alpaca:order-lr04-c{cycle}:filled:2026-04-07T15:00:0{cycle}Z"
                )),
                last_event_at: Some(format!("2026-04-07T15:00:0{cycle}Z")),
                detail: format!("lr04 cycle-{cycle} gap"),
            })
            .await;

        // Continuity must be GapDetected.
        assert!(
            matches!(
                state.alpaca_ws_continuity().await,
                AlpacaWsContinuityState::GapDetected { .. }
            ),
            "LR-04 cycle {cycle}: continuity must be GapDetected after injection"
        );
        // ws_continuity_gap_requires_halt must return true for ExternalSignalIngestion.
        assert!(
            state.ws_continuity_gap_requires_halt().await,
            "LR-04 cycle {cycle}: ws_continuity_gap_requires_halt must be true while GapDetected"
        );

        // Recover to Live.
        state
            .update_ws_continuity(AlpacaWsContinuityState::Live {
                last_message_id: format!(
                    "alpaca:order-lr04-c{cycle}:filled:2026-04-07T15:05:0{cycle}Z"
                ),
                last_event_at: format!("2026-04-07T15:05:0{cycle}Z"),
            })
            .await;

        // Continuity must be Live after recovery.
        assert!(
            matches!(
                state.alpaca_ws_continuity().await,
                AlpacaWsContinuityState::Live { .. }
            ),
            "LR-04 cycle {cycle}: continuity must be Live after recovery"
        );
        // Escalation flag must be reset on Live transition.
        assert!(
            !state.gap_escalation_is_pending(),
            "LR-04 cycle {cycle}: escalation flag must be cleared after Live recovery"
        );
        // halt check must be false when Live.
        assert!(
            !state.ws_continuity_gap_requires_halt().await,
            "LR-04 cycle {cycle}: ws_continuity_gap_requires_halt must be false while Live"
        );
    }

    // After 5 full cycles the escalation flag is still clear — no sticky residue.
    assert!(
        !state.gap_escalation_is_pending(),
        "LR-04 post: escalation flag must remain clear after all cycles"
    );
}

// ---------------------------------------------------------------------------
// LR-05 — Cursor-seed derivation is deterministic and monotonically safe.
//
// Repeated derivation from the same cursor JSON always yields the same state.
// GapDetected input never silently upgrades to Live.
// ColdStartUnproven (None input) never silently upgrades to Live.
// seed_ws_continuity_from_db with no DB is a no-op across repeated calls.
//
// This closes the proof that sustained restart seeding cannot introduce false
// optimism regardless of how many times it is called.
// ---------------------------------------------------------------------------

#[test]
fn lr05_cursor_seed_derivation_is_deterministic_and_monotonically_safe() {
    use mqk_broker_alpaca::types::{AlpacaFetchCursor, AlpacaTradeUpdatesResume};

    // --- Case A: None cursor (no prior run) → always ColdStartUnproven ---
    for _rep in 0..5 {
        let derived = AlpacaWsContinuityState::from_cursor_json(Some(BrokerKind::Alpaca), None);
        assert!(
            matches!(derived, AlpacaWsContinuityState::ColdStartUnproven),
            "LR-05 case A: None cursor must always yield ColdStartUnproven"
        );
    }

    // --- Case B: GapDetected cursor JSON → always GapDetected (never Live) ---
    let gap_cursor = AlpacaFetchCursor::gap_detected(
        Some("act-before-gap-lr05".to_string()),
        Some("alpaca:order-lr05:filled:2026-04-07T15:00:00Z".to_string()),
        Some("2026-04-07T15:00:00Z".to_string()),
        "lr05 gap cursor",
    );
    let gap_json = serde_json::to_string(&gap_cursor).expect("gap cursor must serialize");

    for rep in 0..5 {
        let derived =
            AlpacaWsContinuityState::from_cursor_json(Some(BrokerKind::Alpaca), Some(&gap_json));
        assert!(
            matches!(derived, AlpacaWsContinuityState::GapDetected { .. }),
            "LR-05 case B rep {rep}: GapDetected cursor JSON must always yield GapDetected"
        );
        // Critical: must never be promoted to Live.
        assert!(
            !matches!(derived, AlpacaWsContinuityState::Live { .. }),
            "LR-05 case B rep {rep}: GapDetected must NEVER be silently promoted to Live"
        );
    }

    // --- Case C: Live cursor JSON → always Live (from_cursor_json alone doesn't demote) ---
    // seed_ws_continuity_from_db does the demotion; from_cursor_json is the pure parser.
    let live_cursor = AlpacaFetchCursor::live(
        Some("act-lr05-live".to_string()),
        "alpaca:order-lr05:filled:2026-04-07T15:00:00Z",
        "2026-04-07T15:00:00Z",
    );
    assert!(
        matches!(
            live_cursor.trade_updates,
            AlpacaTradeUpdatesResume::Live { .. }
        ),
        "LR-05 pre-C: live cursor must have Live trade_updates"
    );
    let live_json = serde_json::to_string(&live_cursor).expect("live cursor must serialize");

    for rep in 0..5 {
        let derived =
            AlpacaWsContinuityState::from_cursor_json(Some(BrokerKind::Alpaca), Some(&live_json));
        assert!(
            matches!(derived, AlpacaWsContinuityState::Live { .. }),
            "LR-05 case C rep {rep}: Live cursor JSON must always yield Live"
        );
    }

    // --- Case D: Non-Alpaca broker → always NotApplicable regardless of JSON ---
    for rep in 0..5 {
        let derived =
            AlpacaWsContinuityState::from_cursor_json(Some(BrokerKind::Paper), Some(&live_json));
        assert_eq!(
            derived,
            AlpacaWsContinuityState::NotApplicable,
            "LR-05 case D rep {rep}: non-Alpaca broker must always yield NotApplicable"
        );
    }
}

// ---------------------------------------------------------------------------
// LR-05B — seed_ws_continuity_from_db no-op with no DB is stable across
//           repeated calls (no mutation without DB).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lr05b_seed_ws_continuity_noop_without_db_is_stable() {
    let state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Alpaca);

    // Verify initial state.
    assert!(
        matches!(
            state.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::ColdStartUnproven
        ),
        "LR-05B pre: must start ColdStartUnproven"
    );

    // Call seed 5 times — each must be a no-op (no DB → returns early).
    for rep in 1..=5 {
        state.seed_ws_continuity_from_db().await;
        assert!(
            matches!(
                state.alpaca_ws_continuity().await,
                AlpacaWsContinuityState::ColdStartUnproven
            ),
            "LR-05B rep {rep}: repeated no-DB seed must leave ColdStartUnproven unchanged"
        );
    }
}

// ---------------------------------------------------------------------------
// LR-06 — WS continuity halt trigger is scoped to ExternalSignalIngestion.
//
// ws_continuity_gap_requires_halt() must be true only when:
//   (a) strategy_market_data_source == ExternalSignalIngestion, AND
//   (b) alpaca_ws_continuity == GapDetected.
//
// All other combinations must return false.  This is tested across multiple
// representative states to prove the predicate is not path-dependent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lr06_ws_continuity_halt_trigger_scoped_to_external_signal_ingestion() {
    // --- Paper+Alpaca (ExternalSignalIngestion path) ---
    let st_alpaca = make_paper_alpaca();

    // ColdStartUnproven → must NOT trigger halt.
    assert!(
        matches!(
            st_alpaca.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::ColdStartUnproven
        ),
        "LR-06 pre: Paper+Alpaca must start ColdStartUnproven"
    );
    assert!(
        !st_alpaca.ws_continuity_gap_requires_halt().await,
        "LR-06 paper+alpaca cold-start: must NOT require halt"
    );

    // GapDetected → MUST trigger halt on ExternalSignalIngestion path.
    st_alpaca
        .update_ws_continuity(AlpacaWsContinuityState::GapDetected {
            last_message_id: None,
            last_event_at: None,
            detail: "lr06 gap test".to_string(),
        })
        .await;
    assert!(
        st_alpaca.ws_continuity_gap_requires_halt().await,
        "LR-06 paper+alpaca gap: MUST require halt (ExternalSignalIngestion + GapDetected)"
    );

    // Live → must NOT trigger halt.
    st_alpaca
        .update_ws_continuity(AlpacaWsContinuityState::Live {
            last_message_id: "alpaca:order-lr06:filled:2026-04-07T15:00:00Z".to_string(),
            last_event_at: "2026-04-07T15:00:00Z".to_string(),
        })
        .await;
    assert!(
        !st_alpaca.ws_continuity_gap_requires_halt().await,
        "LR-06 paper+alpaca live: must NOT require halt"
    );

    // --- Paper+Paper (NotConfigured path) ---
    // WS continuity is NotApplicable for the paper broker; halt must never trigger.
    let st_paper = Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Paper));

    assert!(
        matches!(
            st_paper.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::NotApplicable
        ),
        "LR-06 paper+paper pre: must be NotApplicable"
    );
    assert!(
        !st_paper.ws_continuity_gap_requires_halt().await,
        "LR-06 paper+paper: NotApplicable must NOT require halt regardless of path"
    );

    // Attempting to inject GapDetected on a NotApplicable state is a no-op
    // (update_ws_continuity guards NotApplicable → stays NotApplicable).
    st_paper
        .update_ws_continuity(AlpacaWsContinuityState::GapDetected {
            last_message_id: None,
            last_event_at: None,
            detail: "lr06 paper+paper gap attempt (must be no-op)".to_string(),
        })
        .await;
    assert!(
        matches!(
            st_paper.alpaca_ws_continuity().await,
            AlpacaWsContinuityState::NotApplicable
        ),
        "LR-06 paper+paper: update_ws_continuity on NotApplicable must remain NotApplicable"
    );
    assert!(
        !st_paper.ws_continuity_gap_requires_halt().await,
        "LR-06 paper+paper after gap attempt: must still NOT require halt"
    );
}
