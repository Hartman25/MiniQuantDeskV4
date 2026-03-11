use crate::watermark::{SnapshotFreshness, SnapshotWatermark};
use crate::{
    BrokerSnapshot, LocalSnapshot, OrderSnapshot, OrderStatus, ReconcileAction, ReconcileDiff,
    ReconcileReason, ReconcileReport,
};

fn push_reason_once(reasons: &mut Vec<ReconcileReason>, r: ReconcileReason) {
    if !reasons.contains(&r) {
        reasons.push(r);
    }
}

/// Section E: returns true for order statuses that represent an active
/// (non-terminal) order.  Terminal orders (Filled/Canceled/Rejected) are
/// excluded from LocalOrderMissingAtBroker detection because the broker may
/// purge them from its retention window after completion.
fn is_active_status(status: &OrderStatus) -> bool {
    matches!(
        status,
        OrderStatus::New
            | OrderStatus::Accepted
            | OrderStatus::PartiallyFilled
            | OrderStatus::Unknown
    )
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

    // 1) Unknown broker orders (Section E: distinguish fill-touched from open-only)
    for (order_id, broker_ord) in &broker.orders {
        if !local.orders.contains_key(order_id) {
            // Economic exposure has changed if the unknown order has been filled
            // (partially or fully).  Classify more specifically so callers can
            // distinguish timing artifacts (open, unfilled) from real exposure drift.
            let fill_touched = broker_ord.filled_qty > 0
                || matches!(
                    broker_ord.status,
                    OrderStatus::Filled | OrderStatus::PartiallyFilled
                );
            if fill_touched {
                diffs.push(ReconcileDiff::UnknownBrokerFill {
                    order_id: order_id.clone(),
                    filled_qty: broker_ord.filled_qty,
                });
                push_reason_once(&mut reasons, ReconcileReason::UnknownBrokerFill);
            } else {
                diffs.push(ReconcileDiff::UnknownOrder {
                    order_id: order_id.clone(),
                });
                push_reason_once(&mut reasons, ReconcileReason::UnknownBrokerOrder);
            }
        }
    }

    // 2) Order drift for common ids + local active orders absent at broker (Section E)
    for (order_id, local_ord) in &local.orders {
        if let Some(broker_ord) = broker.orders.get(order_id) {
            compare_orders(order_id, local_ord, broker_ord, &mut diffs, &mut reasons);
        } else if is_active_status(&local_ord.status) {
            // Section E: local active order has no broker counterpart — drift.
            // Terminal orders (Filled/Canceled/Rejected) are excluded: the broker
            // may have purged them from its retention window after completion.
            diffs.push(ReconcileDiff::LocalOrderMissingAtBroker {
                order_id: order_id.clone(),
            });
            push_reason_once(&mut reasons, ReconcileReason::LocalOrderMissingAtBroker);
        }
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

    for sym in &symbols {
        let local_has = local.positions.contains_key(sym);
        let broker_has = broker.positions.contains_key(sym);
        let lq = *local.positions.get(sym).unwrap_or(&0);
        let bq = *broker.positions.get(sym).unwrap_or(&0);
        if lq != bq {
            diffs.push(ReconcileDiff::PositionQtyMismatch {
                symbol: sym.clone(),
                local_qty: lq,
                broker_qty: bq,
            });
            // Section E: emit UnknownBrokerPosition when the broker holds a
            // position we have no portfolio record for.  Also emit PositionMismatch
            // so callers relying on that reason for the broker-only case continue
            // to work (backward-compat dual-emit).
            if broker_has && !local_has {
                push_reason_once(&mut reasons, ReconcileReason::UnknownBrokerPosition);
            }
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
// Section E unit tests — one per drift class
// ---------------------------------------------------------------------------

#[cfg(test)]
mod section_e_tests {
    use super::*;
    use crate::{
        BrokerSnapshot, LocalSnapshot, OrderSnapshot, OrderStatus, ReconcileAction, ReconcileDiff,
        ReconcileReason, Side,
    };

    fn order(id: &str, status: OrderStatus, qty: i64, filled: i64) -> OrderSnapshot {
        OrderSnapshot::new(id, "SPY", Side::Buy, qty, filled, status)
    }

    // E-T1: UnknownBrokerFill — broker reports a filled order we have no OMS record of.
    #[test]
    fn unknown_broker_fill_triggers_halt() {
        let local = LocalSnapshot::empty();
        let mut broker = BrokerSnapshot::empty();
        broker.orders.insert(
            "fill-ord-1".to_string(),
            order("fill-ord-1", OrderStatus::Filled, 100, 100),
        );

        let r = reconcile(&local, &broker);

        assert_eq!(r.action, ReconcileAction::Halt);
        assert!(
            r.reasons.contains(&ReconcileReason::UnknownBrokerFill),
            "expected UnknownBrokerFill, got {:?}",
            r.reasons
        );
        let has_fill_diff = r.diffs.iter().any(|d| {
            if let ReconcileDiff::UnknownBrokerFill {
                order_id,
                filled_qty,
            } = d
            {
                order_id == "fill-ord-1" && *filled_qty == 100
            } else {
                false
            }
        });
        assert!(
            has_fill_diff,
            "expected UnknownBrokerFill diff for fill-ord-1 with filled_qty=100"
        );
        // Must NOT emit UnknownBrokerOrder — it is the more specific class.
        assert!(
            !r.reasons.contains(&ReconcileReason::UnknownBrokerOrder),
            "fill-touched unknown order must not be classified as UnknownBrokerOrder"
        );
    }

    // E-T2: UnknownBrokerPosition — broker holds a position our portfolio has no record of.
    #[test]
    fn unknown_broker_position_triggers_halt() {
        let local = LocalSnapshot::empty(); // no positions
        let mut broker = BrokerSnapshot::empty();
        broker.positions.insert("AAPL".to_string(), 200);

        let r = reconcile(&local, &broker);

        assert_eq!(r.action, ReconcileAction::Halt);
        assert!(
            r.reasons.contains(&ReconcileReason::UnknownBrokerPosition),
            "expected UnknownBrokerPosition, got {:?}",
            r.reasons
        );
    }

    // E-T3: LocalOrderMissingAtBroker — OMS has an active order that broker does not report.
    #[test]
    fn local_active_order_missing_at_broker_triggers_halt() {
        let mut local = LocalSnapshot::empty();
        local.orders.insert(
            "local-ord-1".to_string(),
            order("local-ord-1", OrderStatus::Accepted, 50, 0),
        );
        let broker = BrokerSnapshot::empty(); // broker knows nothing

        let r = reconcile(&local, &broker);

        assert_eq!(r.action, ReconcileAction::Halt);
        assert!(
            r.reasons
                .contains(&ReconcileReason::LocalOrderMissingAtBroker),
            "expected LocalOrderMissingAtBroker, got {:?}",
            r.reasons
        );
        let has_diff = r.diffs.iter().any(|d| {
            if let ReconcileDiff::LocalOrderMissingAtBroker { order_id } = d {
                order_id == "local-ord-1"
            } else {
                false
            }
        });
        assert!(
            has_diff,
            "expected LocalOrderMissingAtBroker diff for local-ord-1"
        );
    }

    // E-T4: BrokerOrderMissingLocally — broker has an open order OMS has no record of.
    #[test]
    fn broker_order_missing_locally_triggers_halt() {
        let local = LocalSnapshot::empty();
        let mut broker = BrokerSnapshot::empty();
        broker.orders.insert(
            "ghost-ord-1".to_string(),
            order("ghost-ord-1", OrderStatus::Accepted, 30, 0),
        );

        let r = reconcile(&local, &broker);

        assert_eq!(r.action, ReconcileAction::Halt);
        assert!(
            r.reasons.contains(&ReconcileReason::UnknownBrokerOrder),
            "expected UnknownBrokerOrder, got {:?}",
            r.reasons
        );
        // Not fill-touched — must NOT be classified as UnknownBrokerFill.
        assert!(
            !r.reasons.contains(&ReconcileReason::UnknownBrokerFill),
            "open unfilled broker order must not be classified as UnknownBrokerFill"
        );
    }

    // E-T5: PositionQuantityMismatch — both sides know the symbol but quantities differ.
    #[test]
    fn position_qty_mismatch_both_sides_known_triggers_halt() {
        let mut local = LocalSnapshot::empty();
        local.positions.insert("TSLA".to_string(), 100);
        let mut broker = BrokerSnapshot::empty();
        broker.positions.insert("TSLA".to_string(), 80);

        let r = reconcile(&local, &broker);

        assert_eq!(r.action, ReconcileAction::Halt);
        assert!(
            r.reasons.contains(&ReconcileReason::PositionMismatch),
            "expected PositionMismatch, got {:?}",
            r.reasons
        );
        // Both sides have TSLA — must NOT emit UnknownBrokerPosition.
        assert!(
            !r.reasons.contains(&ReconcileReason::UnknownBrokerPosition),
            "should not emit UnknownBrokerPosition when both sides hold the position"
        );
    }
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

#[cfg(test)]
mod monotonic_tests {
    use super::*;

    #[test]
    fn monotonic_reconcile_rejects_stale_snapshot() {
        let local = LocalSnapshot::empty();
        let mut watermark = SnapshotWatermark::new();

        let fresh = BrokerSnapshot::empty_at(2_000);
        reconcile_monotonic(&mut watermark, &local, &fresh)
            .expect("fresh broker snapshot must be accepted");

        let stale = BrokerSnapshot::empty_at(1_000);
        let err = reconcile_monotonic(&mut watermark, &local, &stale)
            .expect_err("stale broker snapshot must be rejected");

        assert_eq!(
            err.freshness,
            SnapshotFreshness::Stale {
                watermark_ms: 2_000,
                got_ms: 1_000,
            }
        );
    }
}
