//! B1 — Reconcile auto-repair classification.
//!
//! Pure deterministic classification of [`ReconcileDiff`] items into severity
//! classes and prescribed repair actions.  No IO.  No state mutation.
//!
//! ## Severity hierarchy
//!
//! Worst-case severity governs `overall_severity` in the plan:
//!
//! ```text
//! HaltRequired > OperatorOnly > AutoRepairable
//! ```
//!
//! ## Classification rules
//!
//! | Diff variant                  | Severity        | Rationale                                   |
//! |-------------------------------|-----------------|---------------------------------------------|
//! | `UnknownBrokerFill`           | HaltRequired    | Unknown economic exposure; no safe bypass.  |
//! | `PositionQtyMismatch`         | HaltRequired    | Position exposure mismatch; no safe bypass. |
//! | `UnknownOrder`                | OperatorOnly    | Open unfilled broker order; no fills yet.   |
//! | `LocalOrderMissingAtBroker`   | OperatorOnly    | Active local order absent at broker.        |
//! | `OrderMismatch` (status, fwd) | AutoRepairable  | Safe unambiguous status advancement.        |
//! | `OrderMismatch` (other)       | OperatorOnly    | Qty/symbol/side drift or ambiguous status.  |

use crate::types::{OrderStatus, ReconcileDiff, ReconcileReport};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Severity class for a single drift item.
///
/// Implements `Ord` such that `AutoRepairable < OperatorOnly < HaltRequired`;
/// `overall_severity` in [`ReconcileRepairPlan`] is the `.max()` across all
/// classified diffs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DriftSeverity {
    /// Runtime can apply the prescribed repair autonomously.
    AutoRepairable,
    /// Requires operator acknowledgement before execution resumes.
    OperatorOnly,
    /// Must immediately halt and disarm; no auto-repair or operator bypass.
    HaltRequired,
}

/// Prescribed repair action for a single diff.
///
/// These are *descriptions* of what the runtime should do; the reconcile
/// engine itself never performs IO or mutates state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepairAction {
    /// Halt everything immediately.  Manual root-cause analysis required.
    HaltImmediate,
    /// Sync the local order status to the confirmed broker status.
    ///
    /// Safe only for unambiguous forward-only status progressions (e.g.
    /// New → Accepted, Accepted → Canceled).  The runtime is responsible for
    /// updating its OMS state and writing an audit record.
    SyncLocalStatus {
        order_id: String,
        to_status: OrderStatus,
    },
    /// Log and wait for operator to review and resolve the drift.
    OperatorReview,
}

/// Classification and prescribed action for a single reconcile diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriftClassification {
    /// The original diff evidence (cloned from the report).
    pub diff: ReconcileDiff,
    /// Severity assigned by the classification rules.
    pub severity: DriftSeverity,
    /// Prescribed action for the runtime to apply or log.
    pub action: RepairAction,
}

/// Full repair plan built from a [`ReconcileReport`].
///
/// `overall_severity` is the worst-case severity across all diffs.  If any
/// diff is `HaltRequired`, the entire plan is `HaltRequired` regardless of
/// other entries.
///
/// For a clean report (no diffs) the plan is empty with
/// `overall_severity = AutoRepairable` (nothing to repair).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconcileRepairPlan {
    /// Per-diff classifications, in the same order as `report.diffs`.
    pub classifications: Vec<DriftClassification>,
    /// Worst-case severity across all classifications.
    pub overall_severity: DriftSeverity,
}

impl ReconcileRepairPlan {
    /// `true` when every diff is `AutoRepairable` (no operator or halt needed).
    ///
    /// Also `true` when the plan is empty (clean reconcile).
    pub fn is_fully_auto_repairable(&self) -> bool {
        self.overall_severity == DriftSeverity::AutoRepairable
    }

    /// `true` when any diff requires an immediate halt.
    pub fn requires_halt(&self) -> bool {
        self.overall_severity == DriftSeverity::HaltRequired
    }

    /// `true` when operator review is needed but no halt is required.
    pub fn requires_operator(&self) -> bool {
        self.overall_severity == DriftSeverity::OperatorOnly
    }

    /// Iterator over all `AutoRepairable` classifications.
    pub fn auto_repairable(&self) -> impl Iterator<Item = &DriftClassification> {
        self.classifications
            .iter()
            .filter(|c| c.severity == DriftSeverity::AutoRepairable)
    }
}

// ---------------------------------------------------------------------------
// Classification helpers
// ---------------------------------------------------------------------------

/// Returns `Some(target_status)` when the broker status is a valid
/// unambiguous *forward-only* progression that is safe to apply to local
/// state without operator input.  Returns `None` for all other cases.
fn safe_status_advancement(local: &str, broker: &str) -> Option<OrderStatus> {
    match (local, broker) {
        // Broker confirmed the order reached Accepted.
        ("New", "Accepted") => Some(OrderStatus::Accepted),

        // Broker began filling an order we only knew as New or Accepted.
        ("New", "PartiallyFilled") | ("Accepted", "PartiallyFilled") => {
            Some(OrderStatus::PartiallyFilled)
        }

        // Broker confirmed cancellation.
        ("New", "Canceled") | ("Accepted", "Canceled") | ("PartiallyFilled", "Canceled") => {
            Some(OrderStatus::Canceled)
        }

        // Broker confirmed full fill.
        ("New", "Filled") | ("Accepted", "Filled") | ("PartiallyFilled", "Filled") => {
            Some(OrderStatus::Filled)
        }

        // Ambiguous, backward, or unknown transition — operator must decide.
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Classify a single [`ReconcileDiff`] and return the prescribed action.
pub fn classify_diff(diff: &ReconcileDiff) -> DriftClassification {
    match diff {
        // ----------------------------------------------------------------
        // HaltRequired — unknown economic exposure
        // ----------------------------------------------------------------
        ReconcileDiff::UnknownBrokerFill { .. } => DriftClassification {
            diff: diff.clone(),
            severity: DriftSeverity::HaltRequired,
            action: RepairAction::HaltImmediate,
        },

        ReconcileDiff::PositionQtyMismatch { .. } => DriftClassification {
            diff: diff.clone(),
            severity: DriftSeverity::HaltRequired,
            action: RepairAction::HaltImmediate,
        },

        // ----------------------------------------------------------------
        // OperatorOnly — no unknown exposure but not safe to auto-repair
        // ----------------------------------------------------------------
        ReconcileDiff::UnknownOrder { .. } => DriftClassification {
            diff: diff.clone(),
            severity: DriftSeverity::OperatorOnly,
            action: RepairAction::OperatorReview,
        },

        ReconcileDiff::LocalOrderMissingAtBroker { .. } => DriftClassification {
            diff: diff.clone(),
            severity: DriftSeverity::OperatorOnly,
            action: RepairAction::OperatorReview,
        },

        // ----------------------------------------------------------------
        // OrderMismatch — classify by field and direction
        // ----------------------------------------------------------------
        ReconcileDiff::OrderMismatch {
            order_id,
            field,
            local,
            broker,
        } => {
            if field == "status" {
                if let Some(to_status) = safe_status_advancement(local, broker) {
                    return DriftClassification {
                        diff: diff.clone(),
                        severity: DriftSeverity::AutoRepairable,
                        action: RepairAction::SyncLocalStatus {
                            order_id: order_id.clone(),
                            to_status,
                        },
                    };
                }
            }
            // Qty, symbol, side mismatch — or ambiguous status direction.
            DriftClassification {
                diff: diff.clone(),
                severity: DriftSeverity::OperatorOnly,
                action: RepairAction::OperatorReview,
            }
        }
    }
}

/// Build a full repair plan from a [`ReconcileReport`].
///
/// Each diff in `report.diffs` is classified in order.  `overall_severity` is
/// the worst-case (maximum) severity across all classifications.  An empty
/// plan (clean report) returns `overall_severity = AutoRepairable`.
pub fn build_repair_plan(report: &ReconcileReport) -> ReconcileRepairPlan {
    let classifications: Vec<DriftClassification> =
        report.diffs.iter().map(classify_diff).collect();

    let overall_severity = classifications
        .iter()
        .map(|c| c.severity.clone())
        .max()
        .unwrap_or(DriftSeverity::AutoRepairable);

    ReconcileRepairPlan {
        classifications,
        overall_severity,
    }
}
