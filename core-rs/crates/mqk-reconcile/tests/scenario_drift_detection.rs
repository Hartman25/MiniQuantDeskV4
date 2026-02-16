use mqk_reconcile::*;

#[test]
fn scenario_drift_detection_position_mismatch_triggers_halt() {
    let mut local = LocalSnapshot::empty();
    local.positions.insert("SPY".to_string(), 10);

    let mut broker = BrokerSnapshot::empty();
    broker.positions.insert("SPY".to_string(), 8);

    let r = reconcile(&local, &broker);
    assert_eq!(r.action, ReconcileAction::Halt);
    assert!(r.reasons.contains(&ReconcileReason::PositionMismatch));
}
