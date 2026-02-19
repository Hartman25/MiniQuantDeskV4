use mqk_reconcile::*;

#[test]
fn scenario_unknown_broker_order_triggers_halt() {
    let local = LocalSnapshot::empty();

    let mut broker = BrokerSnapshot::empty();
    broker.orders.insert(
        "broker_only_1".to_string(),
        OrderSnapshot::new(
            "broker_only_1",
            "SPY",
            Side::Buy,
            1,
            0,
            OrderStatus::Accepted,
        ),
    );

    let r = reconcile(&local, &broker);
    assert_eq!(r.action, ReconcileAction::Halt);
    assert!(r.reasons.contains(&ReconcileReason::UnknownBrokerOrder));
}
