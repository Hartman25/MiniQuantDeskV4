use std::collections::BTreeSet;

use crate::{
    BrokerSnapshot, LocalSnapshot, OrderSnapshot, ReconcileAction, ReconcileDiff, ReconcileReason,
    ReconcileReport,
};

fn push_reason_once(reasons: &mut Vec<ReconcileReason>, r: ReconcileReason) {
    if !reasons.contains(&r) {
        reasons.push(r);
    }
}

fn compare_orders(order_id: &str, local: &OrderSnapshot, broker: &OrderSnapshot, diffs: &mut Vec<ReconcileDiff>, reasons: &mut Vec<ReconcileReason>) {
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
    let local_ids: BTreeSet<&String> = local.orders.keys().collect();
    for order_id in broker.orders.keys() {
        if !local_ids.contains(order_id) {
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
        // If you want it later, we add a policy flag in a separate patch.
    }

    // 3) Position mismatches
    // Compare union of symbols deterministically.
    let mut symbols: BTreeSet<String> = BTreeSet::new();
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
