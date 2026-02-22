//! Scenario: Periodic Reconcile Drift Halts — Patch L6
//!
//! # Invariants under test
//!
//! 1. A clean tick returns `DriftAction::Continue`.
//! 2. Position drift on a tick returns `DriftAction::HaltAndDisarm`.
//! 3. Unknown broker order on a tick returns `DriftAction::HaltAndDisarm`.
//! 4. Order field drift on a tick returns `DriftAction::HaltAndDisarm`.
//! 5. `HaltAndDisarm` carries the full reconcile report (audit evidence).
//! 6. A single dirty tick prescribes halt regardless of prior clean ticks.
//! 7. After drift is resolved, the tick returns `Continue` again (stateless).
//!
//! All tests are pure in-process; no DB or network required.

use mqk_reconcile::{
    reconcile_tick, BrokerSnapshot, DriftAction, LocalSnapshot, OrderSnapshot, OrderStatus,
    ReconcileAction, Side,
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

fn local_with_pos(symbol: &str, qty: i64) -> LocalSnapshot {
    let mut s = LocalSnapshot::empty();
    s.positions.insert(symbol.to_string(), qty);
    s
}

fn broker_with_pos(symbol: &str, qty: i64) -> BrokerSnapshot {
    let mut s = BrokerSnapshot::empty();
    s.positions.insert(symbol.to_string(), qty);
    s
}

fn make_order(id: &str, symbol: &str, qty: i64, filled: i64, status: OrderStatus) -> OrderSnapshot {
    OrderSnapshot::new(id, symbol, Side::Buy, qty, filled, status)
}

// ---------------------------------------------------------------------------
// 1. Clean tick returns Continue
// ---------------------------------------------------------------------------

#[test]
fn clean_empty_tick_returns_continue() {
    let action = reconcile_tick(&local_empty(), &broker_empty());
    assert_eq!(action, DriftAction::Continue);
    assert!(action.is_safe_to_continue());
    assert!(!action.requires_halt_and_disarm());
}

#[test]
fn matching_positions_tick_returns_continue() {
    let action = reconcile_tick(&local_with_pos("SPY", 100), &broker_with_pos("SPY", 100));
    assert_eq!(action, DriftAction::Continue);
}

#[test]
fn matching_orders_tick_returns_continue() {
    let mut local = local_empty();
    let mut broker = broker_empty();

    let ord = make_order("ORD-1", "AAPL", 10, 5, OrderStatus::PartiallyFilled);
    local.orders.insert("ORD-1".to_string(), ord.clone());
    broker.orders.insert("ORD-1".to_string(), ord);

    assert_eq!(reconcile_tick(&local, &broker), DriftAction::Continue);
}

// ---------------------------------------------------------------------------
// 2. Position drift returns HaltAndDisarm
// ---------------------------------------------------------------------------

#[test]
fn position_drift_returns_halt_and_disarm() {
    let action = reconcile_tick(
        &local_with_pos("SPY", 100),
        &broker_with_pos("SPY", 50), // mismatch
    );
    assert!(
        action.requires_halt_and_disarm(),
        "position drift must prescribe HaltAndDisarm"
    );
    assert!(!action.is_safe_to_continue());
}

#[test]
fn broker_flat_local_has_position_prescribes_halt() {
    let action = reconcile_tick(&local_with_pos("QQQ", 30), &broker_empty());
    assert!(action.requires_halt_and_disarm());
}

#[test]
fn broker_position_local_flat_prescribes_halt() {
    let action = reconcile_tick(&local_empty(), &broker_with_pos("MSFT", 20));
    assert!(action.requires_halt_and_disarm());
}

// ---------------------------------------------------------------------------
// 3. Unknown broker order prescribes HaltAndDisarm
// ---------------------------------------------------------------------------

#[test]
fn unknown_broker_order_prescribes_halt() {
    let local = local_empty();
    let mut broker = broker_empty();
    broker.orders.insert(
        "ROGUE-ORD".to_string(),
        make_order("ROGUE-ORD", "TSLA", 5, 0, OrderStatus::New),
    );

    let action = reconcile_tick(&local, &broker);
    assert!(
        action.requires_halt_and_disarm(),
        "unknown broker order must prescribe HaltAndDisarm"
    );
}

// ---------------------------------------------------------------------------
// 4. Order field drift prescribes HaltAndDisarm
// ---------------------------------------------------------------------------

#[test]
fn order_qty_drift_prescribes_halt() {
    let mut local = local_empty();
    let mut broker = broker_empty();

    local.orders.insert(
        "ORD-A".to_string(),
        make_order("ORD-A", "GLD", 100, 0, OrderStatus::New),
    );
    broker.orders.insert(
        "ORD-A".to_string(),
        make_order("ORD-A", "GLD", 50, 0, OrderStatus::New),
    );

    assert!(reconcile_tick(&local, &broker).requires_halt_and_disarm());
}

// ---------------------------------------------------------------------------
// 5. HaltAndDisarm carries full reconcile report
// ---------------------------------------------------------------------------

#[test]
fn halt_and_disarm_carries_halt_action_and_evidence() {
    let action = reconcile_tick(&local_with_pos("AAPL", 10), &broker_with_pos("AAPL", 20));

    match action {
        DriftAction::HaltAndDisarm { report } => {
            assert_eq!(
                report.action,
                ReconcileAction::Halt,
                "embedded report must have Halt action"
            );
            assert!(
                !report.reasons.is_empty(),
                "embedded report must carry reasons"
            );
            assert!(!report.diffs.is_empty(), "embedded report must carry diffs");
        }
        DriftAction::Continue => panic!("expected HaltAndDisarm but got Continue"),
    }
}

// ---------------------------------------------------------------------------
// 6. Single dirty tick prescribes halt regardless of prior clean ticks
// ---------------------------------------------------------------------------

#[test]
fn one_dirty_tick_prescribes_halt_after_many_clean_ticks() {
    let local = local_with_pos("SPY", 100);
    let clean_broker = broker_with_pos("SPY", 100);
    let dirty_broker = broker_with_pos("SPY", 50);

    // Ten clean ticks in a row.
    for i in 0..10 {
        let action = reconcile_tick(&local, &clean_broker);
        assert_eq!(
            action,
            DriftAction::Continue,
            "clean tick #{i} must return Continue"
        );
    }

    // One drift tick — must immediately prescribe halt.
    let drift_action = reconcile_tick(&local, &dirty_broker);
    assert!(
        drift_action.requires_halt_and_disarm(),
        "single drift tick must prescribe HaltAndDisarm regardless of prior clean ticks"
    );
}

// ---------------------------------------------------------------------------
// 7. Recovery: drift resolved → tick returns Continue
// ---------------------------------------------------------------------------

#[test]
fn resolving_drift_returns_continue_on_next_tick() {
    let local = local_with_pos("SPY", 100);

    // Drift detected.
    let dirty = broker_with_pos("SPY", 50);
    assert!(reconcile_tick(&local, &dirty).requires_halt_and_disarm());

    // After broker is updated to match:
    let clean = broker_with_pos("SPY", 100);
    assert_eq!(
        reconcile_tick(&local, &clean),
        DriftAction::Continue,
        "after drift is resolved, tick must return Continue"
    );
}

// ---------------------------------------------------------------------------
// 8. Multi-symbol partial drift: any mismatch prescribes halt
// ---------------------------------------------------------------------------

#[test]
fn one_symbol_mismatch_in_multi_symbol_portfolio_prescribes_halt() {
    let mut local = local_empty();
    local.positions.insert("SPY".to_string(), 100);
    local.positions.insert("AAPL".to_string(), 50);
    local.positions.insert("MSFT".to_string(), 20);

    let mut broker = broker_empty();
    broker.positions.insert("SPY".to_string(), 100); // match
    broker.positions.insert("AAPL".to_string(), 50); // match
    broker.positions.insert("MSFT".to_string(), 99); // MISMATCH

    let action = reconcile_tick(&local, &broker);
    assert!(
        action.requires_halt_and_disarm(),
        "even a single symbol mismatch must prescribe HaltAndDisarm"
    );
}
