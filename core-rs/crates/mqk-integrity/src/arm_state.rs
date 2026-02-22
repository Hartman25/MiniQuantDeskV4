//! Arm state with fail-closed boot semantics — Patch L7
//!
//! # Invariants
//!
//! - **Boot is always fail-closed**: the system starts DISARMED on every
//!   restart, regardless of what the last persisted state was.  A previously
//!   armed state is NOT trusted; explicit re-arm is required every session.
//!
//! - **Disarm reason is preserved across restart**: when a deadman or
//!   integrity violation triggered the disarm, that reason survives in the
//!   persisted record so operators can identify the cause before re-arming.
//!
//! - **Explicit arm is the only escape**: `ArmState::arm()` is the sole path
//!   to `Armed`. Callers MUST have passed the reconcile gate (L6) before
//!   calling it — that enforcement lives at the single choke-point (L1).
//!
//! All logic is pure deterministic — no IO, no clock, no randomness.

use crate::{IntegrityAction, IntegrityDecision, IntegrityState};

// ---------------------------------------------------------------------------
// Disarm reason
// ---------------------------------------------------------------------------

/// The reason the system is disarmed.
///
/// Preserved in persistence so operators know why a re-arm is required.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisarmReason {
    /// System booted without a previously armed state — fail-closed default.
    BootDefault,
    /// Operator explicitly disarmed the system.
    ManualDisarm,
    /// Integrity engine detected a deadman / halt condition (gap, etc.).
    DeadmanHalt,
    /// Integrity engine detected a data integrity violation (stale, disagreement).
    IntegrityViolation,
    /// Reconcile drift detected — see Patch L6.
    ReconcileDrift,
}

// ---------------------------------------------------------------------------
// Arm state
// ---------------------------------------------------------------------------

/// The system's top-level arm state.
///
/// Tracked in memory and persisted to `sys_arm_state` in the database.
/// On every boot, `ArmState::boot` determines the starting state (always
/// DISARMED — see invariants above).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArmState {
    /// System is armed — execution is permitted (subject to other gates).
    Armed,
    /// System is disarmed — execution is blocked regardless of other conditions.
    Disarmed { reason: DisarmReason },
}

impl ArmState {
    // -----------------------------------------------------------------------
    // Boot / persistence
    // -----------------------------------------------------------------------

    /// Fail-closed boot semantics.
    ///
    /// | Persisted state              | Boot result                        |
    /// |------------------------------|------------------------------------|
    /// | `None` (no record)           | `Disarmed { BootDefault }`         |
    /// | `Some(Armed)`                | `Disarmed { BootDefault }`         |
    /// | `Some(Disarmed { reason })`  | `Disarmed { reason }` (preserved)  |
    ///
    /// The system NEVER auto-arms from a persisted `Armed` state.  Re-arm
    /// always requires explicit operator action after each restart.
    pub fn boot(persisted: Option<ArmState>) -> Self {
        match persisted {
            None => ArmState::Disarmed {
                reason: DisarmReason::BootDefault,
            },
            Some(ArmState::Armed) => ArmState::Disarmed {
                reason: DisarmReason::BootDefault,
            },
            Some(d @ ArmState::Disarmed { .. }) => d,
        }
    }

    // -----------------------------------------------------------------------
    // Transitions
    // -----------------------------------------------------------------------

    /// Explicit operator arm.
    ///
    /// Callers MUST have passed the reconcile gate (L6) before calling this.
    /// This function does not re-verify the gate; that is the caller's
    /// responsibility, enforced at the single choke-point established in L1.
    pub fn arm() -> Self {
        ArmState::Armed
    }

    /// Manual operator disarm.
    pub fn manual_disarm() -> Self {
        ArmState::Disarmed {
            reason: DisarmReason::ManualDisarm,
        }
    }

    /// Disarm triggered by a reconcile drift detection (Patch L6).
    pub fn reconcile_disarm() -> Self {
        ArmState::Disarmed {
            reason: DisarmReason::ReconcileDrift,
        }
    }

    // -----------------------------------------------------------------------
    // Integration with integrity engine
    // -----------------------------------------------------------------------

    /// Derive an `ArmState` change from an [`IntegrityDecision`].
    ///
    /// Returns `Some(ArmState::Disarmed { … })` when the decision requires
    /// halting or disarming.  Returns `None` for `Allow` and `Reject`
    /// decisions that do not affect the arm state.
    pub fn from_integrity_decision(decision: &IntegrityDecision) -> Option<Self> {
        match decision.action {
            IntegrityAction::Halt => Some(ArmState::Disarmed {
                reason: DisarmReason::DeadmanHalt,
            }),
            IntegrityAction::Disarm => Some(ArmState::Disarmed {
                reason: DisarmReason::IntegrityViolation,
            }),
            IntegrityAction::Allow | IntegrityAction::Reject => None,
        }
    }

    /// Derive the current arm state from a live [`IntegrityState`].
    ///
    /// Maps the in-memory integrity flags to the corresponding arm state.
    /// Used to produce the value that must be persisted when integrity
    /// transitions to halted or disarmed.
    pub fn from_integrity_state(st: &IntegrityState) -> Self {
        if st.halted {
            ArmState::Disarmed {
                reason: DisarmReason::DeadmanHalt,
            }
        } else if st.disarmed {
            ArmState::Disarmed {
                reason: DisarmReason::IntegrityViolation,
            }
        } else {
            ArmState::Armed
        }
    }

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    /// `true` if execution is permitted.
    pub fn is_armed(&self) -> bool {
        matches!(self, ArmState::Armed)
    }

    /// `true` if execution is blocked.
    pub fn is_disarmed(&self) -> bool {
        !self.is_armed()
    }
}
