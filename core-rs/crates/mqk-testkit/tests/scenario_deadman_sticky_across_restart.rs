//! Scenario: Deadman Sticky Across Restart — Patch L7
//!
//! # Invariants under test
//!
//! 1. `IntegrityAction::Halt` → `from_integrity_decision` → `Some(Disarmed { DeadmanHalt })`.
//! 2. `IntegrityAction::Disarm` → `from_integrity_decision` → `Some(Disarmed { IntegrityViolation })`.
//! 3. `IntegrityAction::Allow` and `Reject` → `from_integrity_decision` → `None`.
//! 4. Gap detection via `evaluate_bar` sets `st.halted` → `from_integrity_state` → `DeadmanHalt`.
//! 5. `boot(Some(Disarmed { DeadmanHalt }))` → still `Disarmed { DeadmanHalt }` after restart.
//! 6. `boot(Some(reconcile_disarm()))` → still `Disarmed { ReconcileDrift }` after restart.
//! 7. Repeated restart cycles do not clear DeadmanHalt — only explicit `arm()` can escape.
//! 8. `arm()` is the only escape hatch: it unconditionally produces `Armed`.
//!
//! All tests are pure in-process; no DB or network required.

use mqk_integrity::{
    evaluate_bar, ArmState, Bar, BarKey, DisarmReason, FeedId, IntegrityAction, IntegrityConfig,
    IntegrityDecision, IntegrityReason, IntegrityState, Timeframe,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn tf_daily() -> Timeframe {
    Timeframe::secs(86_400)
}

fn feed_a() -> FeedId {
    FeedId::new("feed-a")
}

fn complete_bar(end_ts: i64) -> Bar {
    Bar::new(
        BarKey::new("SPY", tf_daily(), end_ts),
        true, // is_complete
        10_000_000,
        1_000,
    )
}

fn halt_decision() -> IntegrityDecision {
    IntegrityDecision {
        action: IntegrityAction::Halt,
        reason: IntegrityReason::GapDetected,
    }
}

fn disarm_decision() -> IntegrityDecision {
    IntegrityDecision {
        action: IntegrityAction::Disarm,
        reason: IntegrityReason::StaleFeed,
    }
}

fn allow_decision() -> IntegrityDecision {
    IntegrityDecision {
        action: IntegrityAction::Allow,
        reason: IntegrityReason::Allowed,
    }
}

fn reject_decision() -> IntegrityDecision {
    IntegrityDecision {
        action: IntegrityAction::Reject,
        reason: IntegrityReason::IncompleteBar,
    }
}

// ---------------------------------------------------------------------------
// 1. Halt decision → DeadmanHalt arm state
// ---------------------------------------------------------------------------

#[test]
fn halt_decision_maps_to_deadman_halt_arm_state() {
    let decision = halt_decision();
    let arm_state = ArmState::from_integrity_decision(&decision);

    assert_eq!(
        arm_state,
        Some(ArmState::Disarmed {
            reason: DisarmReason::DeadmanHalt
        }),
        "Halt integrity decision must map to Disarmed {{ DeadmanHalt }}"
    );
}

// ---------------------------------------------------------------------------
// 2. Disarm decision → IntegrityViolation arm state
// ---------------------------------------------------------------------------

#[test]
fn disarm_decision_maps_to_integrity_violation_arm_state() {
    let decision = disarm_decision();
    let arm_state = ArmState::from_integrity_decision(&decision);

    assert_eq!(
        arm_state,
        Some(ArmState::Disarmed {
            reason: DisarmReason::IntegrityViolation
        }),
        "Disarm integrity decision must map to Disarmed {{ IntegrityViolation }}"
    );
}

// ---------------------------------------------------------------------------
// 3. Allow and Reject decisions → None (no arm state change)
// ---------------------------------------------------------------------------

#[test]
fn allow_decision_yields_no_arm_state_change() {
    assert_eq!(
        ArmState::from_integrity_decision(&allow_decision()),
        None,
        "Allow decision must not produce an arm state change"
    );
}

#[test]
fn reject_decision_yields_no_arm_state_change() {
    assert_eq!(
        ArmState::from_integrity_decision(&reject_decision()),
        None,
        "Reject decision must not produce an arm state change"
    );
}

// ---------------------------------------------------------------------------
// 4. evaluate_bar gap detection → from_integrity_state → DeadmanHalt
// ---------------------------------------------------------------------------

#[test]
fn gap_detection_via_evaluate_bar_produces_deadman_halt_arm_state() {
    let cfg = IntegrityConfig {
        gap_tolerance_bars: 0,
        stale_threshold_ticks: 0,
        enforce_feed_disagreement: false,
        calendar: mqk_integrity::CalendarSpec::AlwaysOn,
    };
    let mut st = IntegrityState::new();
    let feed = feed_a();

    // First bar at day 1 end_ts=86400.
    let bar1 = complete_bar(86_400);
    let d1 = evaluate_bar(&cfg, &mut st, &feed, 1, &bar1);
    assert_eq!(
        d1.action,
        IntegrityAction::Allow,
        "first bar must be allowed"
    );

    // Second bar at day 3 end_ts=259200 — gap of 1 missing bar (day 2), exceeds tolerance=0.
    let bar3 = complete_bar(259_200); // 86400 * 3
    let d3 = evaluate_bar(&cfg, &mut st, &feed, 2, &bar3);
    assert_eq!(
        d3.action,
        IntegrityAction::Halt,
        "gap must trigger Halt decision"
    );
    assert!(st.halted, "st.halted must be set sticky after gap");

    // from_integrity_state on the halted state must produce DeadmanHalt.
    let arm_state = ArmState::from_integrity_state(&st);
    assert_eq!(
        arm_state,
        ArmState::Disarmed {
            reason: DisarmReason::DeadmanHalt
        },
        "halted integrity state must map to Disarmed {{ DeadmanHalt }}"
    );
}

// ---------------------------------------------------------------------------
// 5. boot(Disarmed{DeadmanHalt}) → still DeadmanHalt after restart
// ---------------------------------------------------------------------------

#[test]
fn deadman_halt_reason_survives_restart() {
    let deadman = ArmState::Disarmed {
        reason: DisarmReason::DeadmanHalt,
    };
    let after_restart = ArmState::boot(Some(deadman));

    assert_eq!(
        after_restart,
        ArmState::Disarmed {
            reason: DisarmReason::DeadmanHalt
        },
        "DeadmanHalt must survive restart so operators cannot silently lose the alert"
    );
}

// ---------------------------------------------------------------------------
// 6. boot(reconcile_disarm()) → still ReconcileDrift after restart
// ---------------------------------------------------------------------------

#[test]
fn reconcile_drift_disarm_survives_restart() {
    let reconcile_state = ArmState::reconcile_disarm();
    let after_restart = ArmState::boot(Some(reconcile_state));

    assert_eq!(
        after_restart,
        ArmState::Disarmed {
            reason: DisarmReason::ReconcileDrift
        },
        "ReconcileDrift must survive restart"
    );
}

// ---------------------------------------------------------------------------
// 7. Multiple restart cycles do not clear DeadmanHalt
// ---------------------------------------------------------------------------

#[test]
fn ten_restart_cycles_do_not_clear_deadman_halt() {
    let mut state = ArmState::Disarmed {
        reason: DisarmReason::DeadmanHalt,
    };

    for cycle in 0..10 {
        state = ArmState::boot(Some(state));
        assert_eq!(
            state,
            ArmState::Disarmed {
                reason: DisarmReason::DeadmanHalt
            },
            "DeadmanHalt must persist through restart cycle #{cycle}"
        );
    }
}

// ---------------------------------------------------------------------------
// 8. arm() is the only escape hatch
// ---------------------------------------------------------------------------

#[test]
fn arm_is_the_only_escape_from_deadman_halt() {
    // After a deadman halt, the state persists across restarts...
    let mut state = ArmState::Disarmed {
        reason: DisarmReason::DeadmanHalt,
    };

    // ...no matter how many times we restart.
    for _ in 0..5 {
        state = ArmState::boot(Some(state));
        assert!(
            state.is_disarmed(),
            "deadman cannot be cleared by restart alone"
        );
    }

    // Only an explicit arm() call (after operator has verified conditions) escapes.
    let armed = ArmState::arm();
    assert!(
        armed.is_armed(),
        "arm() must be the sole path back to ARMED state"
    );
    assert_eq!(armed, ArmState::Armed);
}
