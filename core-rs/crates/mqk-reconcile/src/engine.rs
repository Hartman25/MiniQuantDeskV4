use crate::watermark::{SnapshotFreshness, SnapshotWatermark};
use crate::{
    BrokerSnapshot, LocalSnapshot, OrderSnapshot, ReconcileAction, ReconcileDiff, ReconcileReason,
    ReconcileReport,
};

fn push_reason_once(reasons: &mut Vec<ReconcileReason>, r: ReconcileReason) {
    if !reasons.contains(&r) {
        reasons.push(r);
    }
}

fn compare_orders(
    order_id: &str,
    local: &OrderSnapshot,
    broker: &OrderSnapshot,
    diffs: &mut Vec<ReconcileDiff>,
    reasons: &mut Vec<ReconcileReason>,
) {
    // Symbol
    if local.symbol != broker.symbol {
        diffs.push(ReconcileDiff::OrderMismatch {
            order_id: order_id.to_string(),
            field: "symbol".to_string(),
            local: local.symbol.clone(),
            broker: broker.symbol.clone(),
        });
        push_reason_once(reasons, ReconcileReason::OrderDrift);
    }

    // Side
    if local.side != broker.side {
        diffs.push(ReconcileDiff::OrderMismatch {
            order_id: order_id.to_string(),
            field: "side".to_string(),
            local: format!("{:?}", local.side),
            broker: format!("{:?}", broker.side),
        });
        push_reason_once(reasons, ReconcileReason::OrderDrift);
    }

    // Qty
    if local.qty != broker.qty {
        diffs.push(ReconcileDiff::OrderMismatch {
            order_id: order_id.to_string(),
            field: "qty".to_string(),
            local: local.qty.to_string(),
            broker: broker.qty.to_string(),
        });
        push_reason_once(reasons, ReconcileReason::OrderDrift);
    }

    // Filled qty
    if local.filled_qty != broker.filled_qty {
        diffs.push(ReconcileDiff::OrderMismatch {
            order_id: order_id.to_string(),
            field: "filled_qty".to_string(),
            local: local.filled_qty.to_string(),
            broker: broker.filled_qty.to_string(),
        });
        push_reason_once(reasons, ReconcileReason::OrderDrift);
    }

    // Status
    if local.status != broker.status {
        diffs.push(ReconcileDiff::OrderMismatch {
            order_id: order_id.to_string(),
            field: "status".to_string(),
            local: format!("{:?}", local.status),
            broker: format!("{:?}", broker.status),
        });
        push_reason_once(reasons, ReconcileReason::OrderDrift);
    }
}

/// Deterministic reconciliation:
/// - Unknown broker order => HALT
/// - Any mismatch in positions => HALT
/// - Any drift in orders that exist on both sides => HALT
pub fn reconcile(local: &LocalSnapshot, broker: &BrokerSnapshot) -> ReconcileReport {
    let mut reasons: Vec<ReconcileReason> = Vec::new();
    let mut diffs: Vec<ReconcileDiff> = Vec::new();

    // 1) Unknown broker orders
    for order_id in broker.orders.keys() {
        if !local.orders.contains_key(order_id) {
            diffs.push(ReconcileDiff::UnknownOrder {
                order_id: order_id.clone(),
            });
            push_reason_once(&mut reasons, ReconcileReason::UnknownBrokerOrder);
        }
    }

    // 2) Order drift for common ids
    for (order_id, local_ord) in &local.orders {
        if let Some(broker_ord) = broker.orders.get(order_id) {
            compare_orders(order_id, local_ord, broker_ord, &mut diffs, &mut reasons);
        }
        // NOTE: broker missing local order is not specified as HALT in your patch text.
        // We intentionally do NOT enforce it here to avoid false halts on broker retention windows.
        // If you want it later, add a policy flag in a separate patch.
    }

    // 3) Position mismatches
    // Compare union of symbols deterministically.
    let mut symbols: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for s in local.positions.keys() {
        symbols.insert(s.clone());
    }
    for s in broker.positions.keys() {
        symbols.insert(s.clone());
    }

    for sym in symbols {
        let lq = *local.positions.get(&sym).unwrap_or(&0);
        let bq = *broker.positions.get(&sym).unwrap_or(&0);
        if lq != bq {
            diffs.push(ReconcileDiff::PositionQtyMismatch {
                symbol: sym,
                local_qty: lq,
                broker_qty: bq,
            });
            push_reason_once(&mut reasons, ReconcileReason::PositionMismatch);
        }
    }

    // Stable ordering for reasons + diffs (deterministic output).
    reasons.sort();
    diffs.sort();

    if reasons.is_empty() {
        ReconcileReport::clean()
    } else {
        ReconcileReport {
            action: ReconcileAction::Halt,
            reasons,
            diffs,
        }
    }
}

/// Gate for LIVE arming: must be clean reconcile.
pub fn is_clean_reconcile(local: &LocalSnapshot, broker: &BrokerSnapshot) -> bool {
    reconcile(local, broker).is_clean()
}

// ---------------------------------------------------------------------------
// Monotonicity-enforced entry point — Patch B2
// ---------------------------------------------------------------------------

/// Error returned by [`reconcile_monotonic`] when the broker snapshot fails
/// the monotonicity watermark check (Patch B2).
///
/// The `freshness` field carries the full rejection evidence:
/// - [`SnapshotFreshness::Stale`] — snapshot timestamp is strictly older than
///   the last accepted watermark.
/// - [`SnapshotFreshness::NoTimestamp`] — snapshot has `fetched_at_ms == 0`
///   (fail-closed: an untimed snapshot cannot be proven fresh).
///
/// [`SnapshotFreshness::Fresh`] is never stored here; it is produced by
/// acceptance, not rejection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaleBrokerSnapshot {
    /// Rejection reason and evidence from the watermark check.
    pub freshness: SnapshotFreshness,
}

impl std::fmt::Display for StaleBrokerSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.freshness {
            SnapshotFreshness::Stale {
                watermark_ms,
                got_ms,
            } => write!(
                f,
                "stale broker snapshot rejected: watermark={watermark_ms}ms \
                 got={got_ms}ms (Patch B2 monotonicity enforcement)"
            ),
            SnapshotFreshness::NoTimestamp => write!(
                f,
                "broker snapshot has no timestamp (fetched_at_ms=0): rejected \
                 under fail-closed semantics (Patch B2)"
            ),
            SnapshotFreshness::Fresh => {
                write!(
                    f,
                    "StaleBrokerSnapshot: constructed with Fresh (logic error)"
                )
            }
        }
    }
}

impl std::error::Error for StaleBrokerSnapshot {}

/// Monotonicity-enforced reconcile entry point — Patch B2.
///
/// This is the **required production path**.  Before comparing positions and
/// orders the broker snapshot is checked against the [`SnapshotWatermark`]:
///
/// - **Fresh** (timestamp ≥ watermark): watermark is advanced and [`reconcile`]
///   is called normally.
/// - **Stale or no-timestamp**: returns `Err(StaleBrokerSnapshot)` immediately;
///   no content comparison is performed.
///
/// A stale snapshot can mask real position drift by presenting outdated broker
/// state — accepting it would give the engine a false sense of cleanliness.
///
/// Use [`reconcile`] directly only in unit tests not concerned with freshness
/// (pure content comparison).
pub fn reconcile_monotonic(
    wm: &mut SnapshotWatermark,
    local: &LocalSnapshot,
    broker: &BrokerSnapshot,
) -> Result<ReconcileReport, StaleBrokerSnapshot> {
    let freshness = wm.accept(broker);
    if freshness.is_rejected() {
        return Err(StaleBrokerSnapshot { freshness });
    }
    Ok(reconcile(local, broker))
}
