//! Reconcile gate — Patch L6
//!
//! Provides two mandatory check surfaces:
//!
//! 1. **Arm/start gate** (`check_arm_gate`, `check_start_gate`) — every live
//!    arm and every live start MUST pass through one of these functions.
//!    Both block on any non-CLEAN reconcile.
//!
//! 2. **Periodic drift tick** (`reconcile_tick`) — called on each monitoring
//!    interval. Returns [`DriftAction::HaltAndDisarm`] on any detected drift;
//!    the runtime is responsible for immediately stopping execution and
//!    persisting a DISARM record (L7).
//!
//! All functions are pure deterministic — no IO, no clock, no randomness.

use crate::{reconcile, BrokerSnapshot, LocalSnapshot, ReconcileReport};

// ---------------------------------------------------------------------------
// Arm / Start gate
// ---------------------------------------------------------------------------

/// Result of an arm or start gate check.
///
/// Arm and start may not proceed unless [`ArmStartGate::Permitted`] is returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArmStartGate {
    /// Reconcile is CLEAN — arm or start is permitted.
    Permitted,
    /// Reconcile is NOT CLEAN — arm or start is blocked.
    ///
    /// The embedded `report` carries the full drift evidence for logging and
    /// audit.  Callers must treat this as a hard stop.
    Blocked { report: ReconcileReport },
}

impl ArmStartGate {
    /// `true` when arm or start may proceed.
    pub fn is_permitted(&self) -> bool {
        matches!(self, ArmStartGate::Permitted)
    }

    /// `true` when arm or start is blocked by drift.
    pub fn is_blocked(&self) -> bool {
        !self.is_permitted()
    }
}

/// Gate check for LIVE arm — reconcile MUST be CLEAN.
///
/// Call this immediately before transitioning the system to the armed state.
/// If [`ArmStartGate::Blocked`] is returned, the arm transition must not occur.
pub fn check_arm_gate(local: &LocalSnapshot, broker: &BrokerSnapshot) -> ArmStartGate {
    let report = reconcile(local, broker);
    if report.is_clean() {
        ArmStartGate::Permitted
    } else {
        ArmStartGate::Blocked { report }
    }
}

/// Gate check for LIVE start — reconcile MUST be CLEAN.
///
/// Semantically identical to [`check_arm_gate`]; provided as a separate entry
/// point so arm and start appear as distinct mandatory checks in call-graphs
/// and audit logs.  If [`ArmStartGate::Blocked`] is returned, the start
/// transition must not occur.
pub fn check_start_gate(local: &LocalSnapshot, broker: &BrokerSnapshot) -> ArmStartGate {
    let report = reconcile(local, broker);
    if report.is_clean() {
        ArmStartGate::Permitted
    } else {
        ArmStartGate::Blocked { report }
    }
}

// ---------------------------------------------------------------------------
// Periodic drift tick
// ---------------------------------------------------------------------------

/// Action prescribed by a periodic reconcile tick.
///
/// The runtime MUST act on [`DriftAction::HaltAndDisarm`] immediately:
/// 1. Stop all order submission.
/// 2. Persist a DISARM record so restart defaults to disarmed (L7).
///
/// The `report` field provides drift evidence for audit logging.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriftAction {
    /// Reconcile is CLEAN — execution may continue.
    Continue,
    /// Drift detected — runtime MUST halt and persist a DISARM.
    HaltAndDisarm { report: ReconcileReport },
}

impl DriftAction {
    /// `true` if execution may safely continue.
    pub fn is_safe_to_continue(&self) -> bool {
        matches!(self, DriftAction::Continue)
    }

    /// `true` if the runtime must halt and persist a disarm record.
    pub fn requires_halt_and_disarm(&self) -> bool {
        !self.is_safe_to_continue()
    }
}

/// Periodic reconcile tick — call on every monitoring interval.
///
/// Returns [`DriftAction::HaltAndDisarm`] if **any** drift is detected between
/// the local and broker snapshots.  Returns [`DriftAction::Continue`] only
/// when the reconcile is fully CLEAN.
///
/// This function is stateless; the same inputs always produce the same output.
pub fn reconcile_tick(local: &LocalSnapshot, broker: &BrokerSnapshot) -> DriftAction {
    let report = reconcile(local, broker);
    if report.is_clean() {
        DriftAction::Continue
    } else {
        DriftAction::HaltAndDisarm { report }
    }
}
