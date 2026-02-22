//! Scenario: Reconcile Blocks Arm and Start — Patch L6
//!
//! # Invariants under test
//!
//! 1. A CLEAN reconcile (no drift) permits both arm and start gates.
//! 2. Position mismatch blocks arm and start gates.
//! 3. Unknown broker order (not in local book) blocks arm and start gates.
//! 4. Order field drift (qty, filled_qty, status mismatch) blocks the gates.
//! 5. Resolving drift makes the gates permit again (gates are stateless).
//! 6. Blocked gate embeds full drift evidence in the returned report.
//!
//! All tests are pure in-process; no DB or network required.

use mqk_reconcile::{
    check_arm_gate, check_start_gate, ArmStartGate, BrokerSnapshot, LocalSnapshot, OrderSnapshot,
    OrderStatus, ReconcileAction, Side,
};

// ---------------------------------------------------------------------------
// Snapshot helpers
// ---------------------------------------------------------------------------

fn local_empty() -> LocalSnapshot {
    LocalSnapshot::empty()
}

fn broker_empty() -> BrokerSnapshot {
    BrokerSnapshot::empty()
}

fn local_with_position(symbol: &str, qty: i64) -> LocalSnapshot {
    let mut s = LocalSnapshot::empty();
    s.positions.insert(symbol.to_string(), qty);
    s
}

fn broker_with_position(symbol: &str, qty: i64) -> BrokerSnapshot {
    let mut s = BrokerSnapshot::empty();
    s.positions.insert(symbol.to_string(), qty);
    s
}

fn make_order(id: &str, symbol: &str, qty: i64, filled: i64, status: OrderStatus) -> OrderSnapshot {
    OrderSnapshot::new(id, symbol, Side::Buy, qty, filled, status)
}

// ---------------------------------------------------------------------------
// 1. Clean reconcile permits arm and start
// ---------------------------------------------------------------------------

#[test]
fn clean_empty_snapshots_permit_arm_and_start() {
    let local = local_empty();
    let broker = broker_empty();

    assert_eq!(
        check_arm_gate(&local, &broker),
        ArmStartGate::Permitted,
        "empty/matching snapshots must permit arm gate"
    );
    assert_eq!(
        check_start_gate(&local, &broker),
        ArmStartGate::Permitted,
        "empty/matching snapshots must permit start gate"
    );
}

#[test]
fn matching_positions_permit_arm_and_start() {
    let local = local_with_position("SPY", 100);
    let broker = broker_with_position("SPY", 100);

    assert!(check_arm_gate(&local, &broker).is_permitted());
    assert!(check_start_gate(&local, &broker).is_permitted());
}

#[test]
fn matching_orders_permit_gate() {
    let mut local = local_empty();
    let mut broker = broker_empty();

    let ord = make_order("ORD-1", "AAPL", 10, 5, OrderStatus::PartiallyFilled);
    local.orders.insert("ORD-1".to_string(), ord.clone());
    broker.orders.insert("ORD-1".to_string(), ord);

    assert!(check_arm_gate(&local, &broker).is_permitted());
    assert!(check_start_gate(&local, &broker).is_permitted());
}

// ---------------------------------------------------------------------------
// 2. Position mismatch blocks arm and start
// ---------------------------------------------------------------------------

#[test]
fn position_qty_mismatch_blocks_arm() {
    let local = local_with_position("SPY", 100);
    let broker = broker_with_position("SPY", 50);

    let gate = check_arm_gate(&local, &broker);
    assert!(
        gate.is_blocked(),
        "position qty mismatch must block arm gate"
    );
}

#[test]
fn position_qty_mismatch_blocks_start() {
    let local = local_with_position("AAPL", 20);
    let broker = broker_with_position("AAPL", 10);

    let gate = check_start_gate(&local, &broker);
    assert!(
        gate.is_blocked(),
        "position qty mismatch must block start gate"
    );
}

#[test]
fn broker_holds_position_local_is_flat_blocks_gates() {
    let local = local_empty();
    let broker = broker_with_position("QQQ", 75);

    assert!(check_arm_gate(&local, &broker).is_blocked());
    assert!(check_start_gate(&local, &broker).is_blocked());
}

#[test]
fn local_holds_position_broker_is_flat_blocks_gates() {
    let local = local_with_position("TSLA", 30);
    let broker = broker_empty();

    assert!(check_arm_gate(&local, &broker).is_blocked());
    assert!(check_start_gate(&local, &broker).is_blocked());
}

// ---------------------------------------------------------------------------
// 3. Unknown broker order blocks arm and start
// ---------------------------------------------------------------------------

#[test]
fn unknown_broker_order_blocks_arm_gate() {
    let local = local_empty();
    let mut broker = broker_empty();
    broker.orders.insert(
        "UNKNOWN-99".to_string(),
        make_order("UNKNOWN-99", "SPY", 10, 0, OrderStatus::New),
    );

    assert!(
        check_arm_gate(&local, &broker).is_blocked(),
        "unknown broker order must block arm gate"
    );
}

#[test]
fn unknown_broker_order_blocks_start_gate() {
    let local = local_empty();
    let mut broker = broker_empty();
    broker.orders.insert(
        "UNKNOWN-BROKER-ORD".to_string(),
        make_order("UNKNOWN-BROKER-ORD", "MSFT", 5, 0, OrderStatus::Accepted),
    );

    assert!(
        check_start_gate(&local, &broker).is_blocked(),
        "unknown broker order must block start gate"
    );
}

// ---------------------------------------------------------------------------
// 4. Order field drift blocks gates
// ---------------------------------------------------------------------------

#[test]
fn order_qty_mismatch_blocks_arm_gate() {
    let mut local = local_empty();
    let mut broker = broker_empty();

    local.orders.insert(
        "ORD-A".to_string(),
        make_order("ORD-A", "SPY", 100, 0, OrderStatus::New),
    );
    broker.orders.insert(
        "ORD-A".to_string(),
        make_order("ORD-A", "SPY", 50, 0, OrderStatus::New),
    );

    assert!(
        check_arm_gate(&local, &broker).is_blocked(),
        "order qty drift must block arm gate"
    );
}

#[test]
fn order_filled_qty_mismatch_blocks_arm_gate() {
    let mut local = local_empty();
    let mut broker = broker_empty();

    local.orders.insert(
        "ORD-B".to_string(),
        make_order("ORD-B", "AAPL", 20, 5, OrderStatus::PartiallyFilled),
    );
    broker.orders.insert(
        "ORD-B".to_string(),
        make_order("ORD-B", "AAPL", 20, 10, OrderStatus::PartiallyFilled),
    );

    assert!(check_arm_gate(&local, &broker).is_blocked());
}

// ---------------------------------------------------------------------------
// 5. Resolving drift restores permit (gates are stateless)
// ---------------------------------------------------------------------------

#[test]
fn resolving_position_drift_restores_gate() {
    let local = local_with_position("SPY", 100);

    let dirty_broker = broker_with_position("SPY", 50);
    assert!(
        check_arm_gate(&local, &dirty_broker).is_blocked(),
        "must block with drift"
    );

    // After drift resolved:
    let clean_broker = broker_with_position("SPY", 100);
    assert_eq!(
        check_arm_gate(&local, &clean_broker),
        ArmStartGate::Permitted,
        "after drift is resolved arm gate must permit"
    );
    assert_eq!(
        check_start_gate(&local, &clean_broker),
        ArmStartGate::Permitted,
        "after drift is resolved start gate must permit"
    );
}

// ---------------------------------------------------------------------------
// 6. Blocked gate embeds full drift evidence
// ---------------------------------------------------------------------------

#[test]
fn blocked_arm_gate_carries_halt_action_and_diffs() {
    let local = local_with_position("QQQ", 50);
    let broker = broker_with_position("QQQ", 30);

    match check_arm_gate(&local, &broker) {
        ArmStartGate::Blocked { report } => {
            assert_eq!(
                report.action,
                ReconcileAction::Halt,
                "blocked gate report must prescribe Halt"
            );
            assert!(
                !report.reasons.is_empty(),
                "blocked gate report must include reasons"
            );
            assert!(
                !report.diffs.is_empty(),
                "blocked gate report must include diffs"
            );
        }
        ArmStartGate::Permitted => panic!("expected Blocked but got Permitted"),
    }
}

#[test]
fn blocked_start_gate_carries_halt_action_and_diffs() {
    let local = local_empty();
    let mut broker = broker_empty();
    broker.orders.insert(
        "ROGUE-ORDER".to_string(),
        make_order("ROGUE-ORDER", "GLD", 3, 0, OrderStatus::New),
    );

    match check_start_gate(&local, &broker) {
        ArmStartGate::Blocked { report } => {
            assert_eq!(report.action, ReconcileAction::Halt);
            assert!(!report.diffs.is_empty());
        }
        ArmStartGate::Permitted => panic!("expected Blocked"),
    }
}
