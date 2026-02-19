use mqk_broker_paper::{buy, PaperBroker};
use mqk_reconcile::{is_clean_reconcile, reconcile, LocalSnapshot};

/// Scenario: LIVE arming must be gated by a clean reconcile.
/// This uses the paper broker snapshot + reconcile engine.
///
/// Notes:
/// - This test assumes `mqk-testkit` has dev-deps on:
///     - mqk-broker-paper
///     - mqk-reconcile
///       If it doesn't yet, add them to `crates/mqk-testkit/Cargo.toml` as dev-dependencies.
#[test]
fn scenario_clean_reconcile_required_before_live_arm() {
    // 1) Start with a clean broker snapshot.
    let mut broker = PaperBroker::new();
    broker.set_position("AAPL", 0);

    // Submit a deterministic order so we exercise orders as well.
    let _ = broker.submit(buy("AAPL", 10, "cid-001"));

    let (_msg_id, broker_snap) = broker.snapshot();

    // Local snapshot matches the broker exactly => must be clean.
    let local: LocalSnapshot = broker.as_local_snapshot();
    let report = reconcile(&local, &broker_snap);
    assert!(
        report.is_clean(),
        "expected clean reconcile, got: {:?}",
        report
    );
    assert!(
        is_clean_reconcile(&local, &broker_snap),
        "expected clean reconcile gate to pass"
    );

    // 2) Now create a drift: local thinks it has a different position.
    let mut local_drift = local.clone();
    local_drift.positions.insert("AAPL".to_string(), 5);

    let report2 = reconcile(&local_drift, &broker_snap);
    assert!(
        !report2.is_clean(),
        "expected non-clean reconcile due to position mismatch"
    );
    assert!(
        !is_clean_reconcile(&local_drift, &broker_snap),
        "expected clean reconcile gate to fail"
    );
}
