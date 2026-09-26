use mqk_reconcile::*;

#[test]
fn scenario_reconcile_required_before_live() {
    // Dirty reconcile => cannot arm
    let mut local = LocalSnapshot::empty();
    local
        .positions
        .insert("SPY".to_string(), QtyMicros::from_whole_units(10).unwrap());

    let mut broker = BrokerSnapshot::empty();
    broker
        .positions
        .insert("SPY".to_string(), QtyMicros::from_whole_units(9).unwrap());

    assert!(!is_clean_reconcile(&local, &broker));

    // Clean reconcile => can arm
    broker
        .positions
        .insert("SPY".to_string(), QtyMicros::from_whole_units(10).unwrap());
    assert!(is_clean_reconcile(&local, &broker));
}
