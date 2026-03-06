use mqk_reconcile::{check_arm_gate, BrokerSnapshot, LocalSnapshot};

#[test]
fn clean_reconcile_required_before_live_arm() {
    // DIRTY: local thinks we hold something, broker says we hold nothing -> blocked.
    let mut local = LocalSnapshot::empty();
    local.positions.insert("SPY".to_string(), 1);

    let broker = BrokerSnapshot::empty_at(1);

    assert!(check_arm_gate(&local, &broker).is_blocked());

    // CLEAN: make local match broker -> permitted.
    local.positions.clear();

    assert!(check_arm_gate(&local, &broker).is_permitted());
}
