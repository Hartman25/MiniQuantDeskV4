//! PATCH 15g — Reconcile + lifecycle gate integration test
//!
//! Validates: PATCH 09 + PATCH 14 integration
//!
//! GREEN when:
//! - Attempting arm on a LIVE run while dirty reconcile is simulated fails.
//! - Clean reconcile + valid lifecycle state succeeds.
//! - Various mismatch scenarios (position, order drift, unknown broker orders)
//!   all produce non-clean reconcile that would block arming.
//!
//! Note: Since arm_run() in mqk-db requires an async Postgres connection,
//! this test validates the reconcile gate function (is_clean_reconcile) that
//! MUST be called before arm_run in any correct arming sequence. This proves
//! the gate logic is correct; wiring it into the actual arm path is PATCH 20.

use mqk_reconcile::*;

/// Helper: build a realistic local snapshot with orders and positions.
fn local_with_orders_and_positions() -> LocalSnapshot {
    let mut local = LocalSnapshot::empty();

    // One open order
    local.orders.insert(
        "ORD-001".to_string(),
        OrderSnapshot::new("ORD-001", "SPY", Side::Buy, 100, 0, OrderStatus::Accepted),
    );

    // One filled order
    local.orders.insert(
        "ORD-002".to_string(),
        OrderSnapshot::new("ORD-002", "AAPL", Side::Sell, 50, 50, OrderStatus::Filled),
    );

    // Positions
    local.positions.insert("SPY".to_string(), 200);
    local.positions.insert("AAPL".to_string(), -50);

    local
}

/// Helper: build a broker snapshot that exactly matches the local snapshot.
fn matching_broker_snapshot(local: &LocalSnapshot) -> BrokerSnapshot {
    BrokerSnapshot {
        orders: local.orders.clone(),
        positions: local.positions.clone(),
    }
}

// ============================================================================
// Core gate tests
// ============================================================================

#[test]
fn clean_reconcile_allows_arming() {
    let local = local_with_orders_and_positions();
    let broker = matching_broker_snapshot(&local);

    assert!(
        is_clean_reconcile(&local, &broker),
        "matching local/broker should produce clean reconcile (arm allowed)"
    );

    let report = reconcile(&local, &broker);
    assert!(report.is_clean());
    assert_eq!(report.action, ReconcileAction::Clean);
    assert!(report.reasons.is_empty());
    assert!(report.diffs.is_empty());
}

#[test]
fn position_mismatch_blocks_arming() {
    let local = local_with_orders_and_positions();
    let mut broker = matching_broker_snapshot(&local);

    // Broker thinks we have 199 SPY instead of 200
    broker.positions.insert("SPY".to_string(), 199);

    assert!(
        !is_clean_reconcile(&local, &broker),
        "position mismatch should block arming"
    );

    let report = reconcile(&local, &broker);
    assert_eq!(report.action, ReconcileAction::Halt);
    assert!(
        report.reasons.contains(&ReconcileReason::PositionMismatch),
        "should report PositionMismatch reason"
    );
}

#[test]
fn order_qty_drift_blocks_arming() {
    let local = local_with_orders_and_positions();
    let mut broker = matching_broker_snapshot(&local);

    // Broker shows different qty for ORD-001
    let mut drifted = broker.orders.get("ORD-001").unwrap().clone();
    drifted.qty = 150; // local says 100, broker says 150
    broker.orders.insert("ORD-001".to_string(), drifted);

    assert!(
        !is_clean_reconcile(&local, &broker),
        "order qty drift should block arming"
    );

    let report = reconcile(&local, &broker);
    assert_eq!(report.action, ReconcileAction::Halt);
    assert!(
        report.reasons.contains(&ReconcileReason::OrderDrift),
        "should report OrderDrift reason"
    );
}

#[test]
fn unknown_broker_order_blocks_arming() {
    let local = local_with_orders_and_positions();
    let mut broker = matching_broker_snapshot(&local);

    // Broker has an order we don't know about
    broker.orders.insert(
        "ORD-UNKNOWN".to_string(),
        OrderSnapshot::new(
            "ORD-UNKNOWN",
            "TSLA",
            Side::Buy,
            500,
            0,
            OrderStatus::Accepted,
        ),
    );

    assert!(
        !is_clean_reconcile(&local, &broker),
        "unknown broker order should block arming"
    );

    let report = reconcile(&local, &broker);
    assert_eq!(report.action, ReconcileAction::Halt);
    assert!(
        report.reasons.contains(&ReconcileReason::UnknownBrokerOrder),
        "should report UnknownBrokerOrder reason"
    );
}

#[test]
fn order_status_drift_blocks_arming() {
    let local = local_with_orders_and_positions();
    let mut broker = matching_broker_snapshot(&local);

    // Broker shows ORD-001 as Filled, but local shows Accepted
    let mut drifted = broker.orders.get("ORD-001").unwrap().clone();
    drifted.status = OrderStatus::Filled;
    drifted.filled_qty = 100;
    broker.orders.insert("ORD-001".to_string(), drifted);

    assert!(
        !is_clean_reconcile(&local, &broker),
        "order status drift should block arming"
    );
}

#[test]
fn broker_has_extra_position_blocks_arming() {
    let local = local_with_orders_and_positions();
    let mut broker = matching_broker_snapshot(&local);

    // Broker has a position we don't know about
    broker.positions.insert("TSLA".to_string(), 100);

    assert!(
        !is_clean_reconcile(&local, &broker),
        "broker holding position not in local snapshot should block arming"
    );

    let report = reconcile(&local, &broker);
    assert!(
        report.reasons.contains(&ReconcileReason::PositionMismatch),
        "should report PositionMismatch for unknown broker position"
    );
}

#[test]
fn flat_both_sides_is_clean() {
    // No orders, no positions on either side
    let local = LocalSnapshot::empty();
    let broker = BrokerSnapshot::empty();

    assert!(
        is_clean_reconcile(&local, &broker),
        "both sides flat should be clean reconcile"
    );
}

#[test]
fn multiple_mismatches_all_reported() {
    let local = local_with_orders_and_positions();
    let mut broker = matching_broker_snapshot(&local);

    // Position mismatch
    broker.positions.insert("SPY".to_string(), 199);

    // Unknown order
    broker.orders.insert(
        "ORD-ROGUE".to_string(),
        OrderSnapshot::new("ORD-ROGUE", "GOOG", Side::Sell, 10, 0, OrderStatus::New),
    );

    // Order drift on existing
    let mut drifted = broker.orders.get("ORD-001").unwrap().clone();
    drifted.side = Side::Sell; // was Buy
    broker.orders.insert("ORD-001".to_string(), drifted);

    let report = reconcile(&local, &broker);
    assert_eq!(report.action, ReconcileAction::Halt);

    // All three reason types should be present
    assert!(
        report.reasons.contains(&ReconcileReason::PositionMismatch),
        "should contain PositionMismatch"
    );
    assert!(
        report.reasons.contains(&ReconcileReason::UnknownBrokerOrder),
        "should contain UnknownBrokerOrder"
    );
    assert!(
        report.reasons.contains(&ReconcileReason::OrderDrift),
        "should contain OrderDrift"
    );

    // Diffs should enumerate all specific problems
    assert!(
        report.diffs.len() >= 3,
        "should have at least 3 diffs, got {}",
        report.diffs.len()
    );
}

#[test]
fn reconcile_report_is_deterministic() {
    let local = local_with_orders_and_positions();
    let mut broker = matching_broker_snapshot(&local);
    broker.positions.insert("SPY".to_string(), 199);
    broker.orders.insert(
        "ORD-ROGUE".to_string(),
        OrderSnapshot::new("ORD-ROGUE", "GOOG", Side::Sell, 10, 0, OrderStatus::New),
    );

    // Run twice, should produce identical reports
    let report_a = reconcile(&local, &broker);
    let report_b = reconcile(&local, &broker);

    assert_eq!(
        report_a, report_b,
        "reconcile reports should be deterministic"
    );
}
