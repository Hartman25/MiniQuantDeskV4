//! Scenario: Restart Defaults to Disarmed — Patch L7
//!
//! # Invariants under test
//!
//! 1. Fresh boot (no persisted record) → DISARMED with BootDefault reason.
//! 2. Boot from a previously ARMED state → DISARMED with BootDefault (fail-closed).
//! 3. Boot from a previously DISARMED state → reason is preserved (not overwritten).
//! 4. Explicit `arm()` transitions the state to ARMED.
//! 5. Arm followed by restart → DISARMED again (every restart is fail-closed).
//!
//! All tests are pure in-process; no DB or network required.

use mqk_integrity::{ArmState, DisarmReason};

// ---------------------------------------------------------------------------
// 1. Fresh boot (no persisted record) → DISARMED BootDefault
// ---------------------------------------------------------------------------

#[test]
fn fresh_boot_with_no_record_defaults_to_disarmed_boot_default() {
    let state = ArmState::boot(None);
    assert!(
        state.is_disarmed(),
        "boot with no record must produce a disarmed state"
    );
    assert_eq!(
        state,
        ArmState::Disarmed {
            reason: DisarmReason::BootDefault
        },
        "boot with no record must use BootDefault reason"
    );
}

// ---------------------------------------------------------------------------
// 2. Boot from Armed → DISARMED BootDefault (fail-closed)
// ---------------------------------------------------------------------------

#[test]
fn boot_from_armed_state_is_fail_closed() {
    let persisted = Some(ArmState::arm());
    let state = ArmState::boot(persisted);

    assert!(
        state.is_disarmed(),
        "system must start DISARMED even when last persisted state was ARMED"
    );
    assert_eq!(
        state,
        ArmState::Disarmed {
            reason: DisarmReason::BootDefault
        },
        "fail-closed boot from ARMED must produce BootDefault reason"
    );
}

// ---------------------------------------------------------------------------
// 3. Boot from Disarmed → reason is preserved across restart
// ---------------------------------------------------------------------------

#[test]
fn boot_from_disarmed_boot_default_preserves_reason() {
    let persisted = Some(ArmState::Disarmed {
        reason: DisarmReason::BootDefault,
    });
    let state = ArmState::boot(persisted);
    assert_eq!(
        state,
        ArmState::Disarmed {
            reason: DisarmReason::BootDefault
        }
    );
}

#[test]
fn boot_from_disarmed_manual_disarm_preserves_reason() {
    let persisted = Some(ArmState::manual_disarm());
    let state = ArmState::boot(persisted);
    assert_eq!(
        state,
        ArmState::Disarmed {
            reason: DisarmReason::ManualDisarm
        },
        "ManualDisarm reason must survive restart"
    );
}

#[test]
fn boot_from_disarmed_deadman_halt_preserves_reason() {
    let persisted = Some(ArmState::Disarmed {
        reason: DisarmReason::DeadmanHalt,
    });
    let state = ArmState::boot(persisted);
    assert_eq!(
        state,
        ArmState::Disarmed {
            reason: DisarmReason::DeadmanHalt
        },
        "DeadmanHalt reason must survive restart so operators know why re-arm is required"
    );
}

#[test]
fn boot_from_disarmed_integrity_violation_preserves_reason() {
    let persisted = Some(ArmState::Disarmed {
        reason: DisarmReason::IntegrityViolation,
    });
    let state = ArmState::boot(persisted);
    assert_eq!(
        state,
        ArmState::Disarmed {
            reason: DisarmReason::IntegrityViolation
        },
        "IntegrityViolation reason must survive restart"
    );
}

#[test]
fn boot_from_disarmed_reconcile_drift_preserves_reason() {
    let persisted = Some(ArmState::reconcile_disarm());
    let state = ArmState::boot(persisted);
    assert_eq!(
        state,
        ArmState::Disarmed {
            reason: DisarmReason::ReconcileDrift
        },
        "ReconcileDrift reason must survive restart"
    );
}

// ---------------------------------------------------------------------------
// 4. Explicit arm() transitions to ARMED
// ---------------------------------------------------------------------------

#[test]
fn explicit_arm_produces_armed_state() {
    let state = ArmState::arm();
    assert!(state.is_armed(), "arm() must produce ARMED state");
    assert!(!state.is_disarmed());
    assert_eq!(state, ArmState::Armed);
}

// ---------------------------------------------------------------------------
// 5. Arm followed by restart → DISARMED again
// ---------------------------------------------------------------------------

#[test]
fn arm_then_restart_produces_disarmed_again() {
    // Simulate: operator arms → system persists Armed → process restarts.
    let armed = ArmState::arm();
    assert!(armed.is_armed());

    // On restart, the persisted Armed state is loaded and passed to boot().
    let after_restart = ArmState::boot(Some(armed));
    assert!(
        after_restart.is_disarmed(),
        "every restart must require explicit re-arm — Armed must not survive boot"
    );
    assert_eq!(
        after_restart,
        ArmState::Disarmed {
            reason: DisarmReason::BootDefault
        }
    );
}
