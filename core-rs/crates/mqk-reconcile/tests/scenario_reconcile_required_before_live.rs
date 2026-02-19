use mqk_reconcile::*;

#[test]
fn scenario_reconcile_required_before_live() {
    // Dirty reconcile => cannot arm
    let mut local = LocalSnapshot::empty();
    local.positions.insert("SPY".to_string(), 10);

    let mut broker = BrokerSnapshot::empty();
    broker.positions.insert("SPY".to_string(), 9);

    assert!(!is_clean_reconcile(&local, &broker));

    // Clean reconcile => can arm
    broker.positions.insert("SPY".to_string(), 10);
    assert!(is_clean_reconcile(&local, &broker));
}
