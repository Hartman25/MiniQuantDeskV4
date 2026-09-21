//! CUTOVER-1G / CUTOVER-1C mission proof item G — fractional reconciliation
//! mismatch detection.
//!
//! Proves the reconcile engine correctly detects a genuine drift between a
//! local and broker fractional Crypto position (e.g. 0.0001 BTC), and does
//! NOT collapse a fractional mismatch into a false-clean result (which a
//! whole-unit-only comparison would do by truncating both sides to zero).
//!
//! Pure in-process: no DB, no network.

use mqk_reconcile::{
    reconcile, BrokerSnapshot, LocalSnapshot, QtyMicros, ReconcileAction, ReconcileDiff,
    ReconcileReason,
};

#[test]
fn proof_g_fractional_position_mismatch_is_detected_not_truncated() {
    let mut local = LocalSnapshot::empty();
    local
        .positions
        .insert("BTC/USD".to_string(), "0.0001".parse().unwrap());

    let mut broker = BrokerSnapshot::empty_at(1_000);
    broker
        .positions
        .insert("BTC/USD".to_string(), "0.00011".parse().unwrap());

    let report = reconcile(&local, &broker);

    assert_eq!(
        report.action,
        ReconcileAction::Halt,
        "G: a genuine fractional position mismatch (0.0001 vs 0.00011 BTC) must halt, \
         not pass as clean"
    );
    assert!(
        report.reasons.contains(&ReconcileReason::PositionMismatch),
        "G: must report PositionMismatch; got {:?}",
        report.reasons
    );

    let expected_local: QtyMicros = "0.0001".parse().unwrap();
    let expected_broker: QtyMicros = "0.00011".parse().unwrap();
    let has_exact_diff = report.diffs.iter().any(|d| {
        if let ReconcileDiff::PositionQtyMismatch {
            symbol,
            local_qty,
            broker_qty,
        } = d
        {
            symbol == "BTC/USD" && *local_qty == expected_local && *broker_qty == expected_broker
        } else {
            false
        }
    });
    assert!(
        has_exact_diff,
        "G: diff evidence must carry the exact fractional local/broker quantities \
         (0.0001 vs 0.00011), not a rounded/truncated whole-unit approximation; diffs={:?}",
        report.diffs
    );
}

#[test]
fn proof_g_matching_fractional_positions_reconcile_clean() {
    let mut local = LocalSnapshot::empty();
    local
        .positions
        .insert("BTC/USD".to_string(), "0.0001".parse().unwrap());

    let mut broker = BrokerSnapshot::empty_at(1_000);
    broker
        .positions
        .insert("BTC/USD".to_string(), "0.0001".parse().unwrap());

    let report = reconcile(&local, &broker);

    assert_eq!(
        report.action,
        ReconcileAction::Clean,
        "G: identical fractional positions on both sides must reconcile clean; diffs={:?}",
        report.diffs
    );
}

/// Negative control: proves the mismatch detection is real (not a fixture
/// coincidence) by checking a difference smaller than any whole-unit
/// comparison could ever observe -- a whole-unit-only reconcile would see
/// both sides as "0" and falsely report Clean.
#[test]
fn proof_g_sub_whole_unit_drift_is_not_masked_by_whole_unit_rounding() {
    let mut local = LocalSnapshot::empty();
    local
        .positions
        .insert("BTC/USD".to_string(), QtyMicros::new(1)); // 0.000001 BTC

    let mut broker = BrokerSnapshot::empty_at(1_000);
    broker
        .positions
        .insert("BTC/USD".to_string(), QtyMicros::new(2)); // 0.000002 BTC

    let report = reconcile(&local, &broker);

    assert_eq!(
        report.action,
        ReconcileAction::Halt,
        "G: a 1-raw-micro drift (both sides round to 0 whole units) must still halt -- \
         proves comparison happens on the exact QtyMicros value, not a whole-unit projection"
    );
}
