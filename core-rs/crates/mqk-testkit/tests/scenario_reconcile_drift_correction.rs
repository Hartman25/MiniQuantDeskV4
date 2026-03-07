//! Scenario: Reconcile Drift Correction
//!
//! # Invariants under test
//!
//! 1. A position mismatch produces `DriftAction::HaltAndDisarm`.
//! 2. Correcting that mismatch returns the system to `DriftAction::Continue`.
//! 3. An order drift produces `DriftAction::HaltAndDisarm`.
//! 4. Correcting that order drift returns the system to `DriftAction::Continue`.
//!
//! Pure in-process test. No DB required.

use std::collections::BTreeMap;

use mqk_reconcile::{
    reconcile_tick, BrokerSnapshot, DriftAction, LocalSnapshot, OrderSnapshot, OrderStatus, Side,
};

fn local_empty() -> LocalSnapshot {
    LocalSnapshot {
        orders: BTreeMap::new(),
        positions: BTreeMap::new(),
    }
}

fn broker_empty() -> BrokerSnapshot {
    BrokerSnapshot {
        orders: BTreeMap::new(),
        positions: BTreeMap::new(),
        fetched_at_ms: 1,
    }
}

fn make_order(
    order_id: &str,
    symbol: &str,
    side: Side,
    qty: i64,
    filled_qty: i64,
    status: OrderStatus,
) -> OrderSnapshot {
    OrderSnapshot::new(order_id, symbol, side, qty, filled_qty, status)
}

#[test]
fn position_drift_then_correction_returns_to_continue() {
    let mut local = local_empty();
    local.positions.insert("SPY".to_string(), 100);

    let mut broker_dirty = broker_empty();
    broker_dirty.positions.insert("SPY".to_string(), 50);

    let dirty = reconcile_tick(&local, &broker_dirty);
    assert!(
        matches!(dirty, DriftAction::HaltAndDisarm { .. }),
        "position drift must halt and disarm"
    );

    let mut broker_clean = broker_empty();
    broker_clean.positions.insert("SPY".to_string(), 100);

    let clean = reconcile_tick(&local, &broker_clean);
    assert_eq!(
        clean,
        DriftAction::Continue,
        "after correcting position drift, reconcile must continue"
    );
}

#[test]
fn order_drift_then_correction_returns_to_continue() {
    let mut local = local_empty();
    local.orders.insert(
        "ORD-1".to_string(),
        make_order("ORD-1", "AAPL", Side::Buy, 10, 0, OrderStatus::New),
    );

    let mut broker_dirty = broker_empty();
    broker_dirty.orders.insert(
        "ORD-1".to_string(),
        make_order(
            "ORD-1",
            "AAPL",
            Side::Buy,
            5, // qty drift
            0,
            OrderStatus::New,
        ),
    );

    let dirty = reconcile_tick(&local, &broker_dirty);
    assert!(
        matches!(dirty, DriftAction::HaltAndDisarm { .. }),
        "order drift must halt and disarm"
    );

    let mut broker_clean = broker_empty();
    broker_clean.orders.insert(
        "ORD-1".to_string(),
        make_order("ORD-1", "AAPL", Side::Buy, 10, 0, OrderStatus::New),
    );

    let clean = reconcile_tick(&local, &broker_clean);
    assert_eq!(
        clean,
        DriftAction::Continue,
        "after correcting order drift, reconcile must continue"
    );
}
