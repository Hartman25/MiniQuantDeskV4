//! B1 — Reconcile auto-repair classification scenario tests.
//!
//! Verifies that every drift class is correctly classified into its severity
//! bucket and that the prescribed repair action is accurate.
//!
//! S1  UnknownBrokerFill            → HaltRequired / HaltImmediate
//! S2  PositionQtyMismatch          → HaltRequired / HaltImmediate
//! S3  UnknownOrder (open/unfilled) → OperatorOnly / OperatorReview
//! S4  LocalOrderMissingAtBroker    → OperatorOnly / OperatorReview
//! S5  OrderMismatch status New→Accepted → AutoRepairable / SyncLocalStatus(Accepted)
//! S6  OrderMismatch status Accepted→Canceled → AutoRepairable / SyncLocalStatus(Canceled)
//! S7  OrderMismatch qty mismatch   → OperatorOnly / OperatorReview
//! S8  Mixed plan (auto + halt) → overall HaltRequired
//! S9  Clean report → empty plan, overall AutoRepairable
//! S10 Audit trail: build_repair_plan captures all diffs in order

use mqk_reconcile::{
    build_repair_plan, classify_diff, reconcile, BrokerSnapshot, DriftSeverity, LocalSnapshot,
    OrderSnapshot, OrderStatus, ReconcileDiff, RepairAction, Side,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn order(id: &str, status: OrderStatus, qty: i64, filled: i64) -> OrderSnapshot {
    OrderSnapshot::new(id, "SPY", Side::Buy, qty, filled, status)
}

// ---------------------------------------------------------------------------
// S1 — UnknownBrokerFill → HaltRequired
// ---------------------------------------------------------------------------

#[test]
fn s1_unknown_broker_fill_is_halt_required() {
    let diff = ReconcileDiff::UnknownBrokerFill {
        order_id: "fill-1".to_string(),
        filled_qty: 50,
    };
    let c = classify_diff(&diff);
    assert_eq!(c.severity, DriftSeverity::HaltRequired);
    assert_eq!(c.action, RepairAction::HaltImmediate);
    assert_eq!(c.diff, diff);
}

// ---------------------------------------------------------------------------
// S2 — PositionQtyMismatch → HaltRequired
// ---------------------------------------------------------------------------

#[test]
fn s2_position_qty_mismatch_is_halt_required() {
    let diff = ReconcileDiff::PositionQtyMismatch {
        symbol: "AAPL".to_string(),
        local_qty: 100,
        broker_qty: 80,
    };
    let c = classify_diff(&diff);
    assert_eq!(c.severity, DriftSeverity::HaltRequired);
    assert_eq!(c.action, RepairAction::HaltImmediate);
}

// ---------------------------------------------------------------------------
// S3 — UnknownOrder (open/unfilled) → OperatorOnly
// ---------------------------------------------------------------------------

#[test]
fn s3_unknown_order_is_operator_only() {
    let diff = ReconcileDiff::UnknownOrder {
        order_id: "ghost-1".to_string(),
    };
    let c = classify_diff(&diff);
    assert_eq!(c.severity, DriftSeverity::OperatorOnly);
    assert_eq!(c.action, RepairAction::OperatorReview);
}

// ---------------------------------------------------------------------------
// S4 — LocalOrderMissingAtBroker → OperatorOnly
// ---------------------------------------------------------------------------

#[test]
fn s4_local_order_missing_at_broker_is_operator_only() {
    let diff = ReconcileDiff::LocalOrderMissingAtBroker {
        order_id: "local-1".to_string(),
    };
    let c = classify_diff(&diff);
    assert_eq!(c.severity, DriftSeverity::OperatorOnly);
    assert_eq!(c.action, RepairAction::OperatorReview);
}

// ---------------------------------------------------------------------------
// S5 — OrderMismatch status New→Accepted → AutoRepairable
// ---------------------------------------------------------------------------

#[test]
fn s5_order_mismatch_status_new_to_accepted_is_auto_repairable() {
    let diff = ReconcileDiff::OrderMismatch {
        order_id: "ord-5".to_string(),
        field: "status".to_string(),
        local: "New".to_string(),
        broker: "Accepted".to_string(),
    };
    let c = classify_diff(&diff);
    assert_eq!(
        c.severity,
        DriftSeverity::AutoRepairable,
        "New→Accepted is a safe forward progression"
    );
    assert_eq!(
        c.action,
        RepairAction::SyncLocalStatus {
            order_id: "ord-5".to_string(),
            to_status: OrderStatus::Accepted,
        }
    );
}

// ---------------------------------------------------------------------------
// S6 — OrderMismatch status Accepted→Canceled → AutoRepairable
// ---------------------------------------------------------------------------

#[test]
fn s6_order_mismatch_status_accepted_to_canceled_is_auto_repairable() {
    let diff = ReconcileDiff::OrderMismatch {
        order_id: "ord-6".to_string(),
        field: "status".to_string(),
        local: "Accepted".to_string(),
        broker: "Canceled".to_string(),
    };
    let c = classify_diff(&diff);
    assert_eq!(c.severity, DriftSeverity::AutoRepairable);
    assert_eq!(
        c.action,
        RepairAction::SyncLocalStatus {
            order_id: "ord-6".to_string(),
            to_status: OrderStatus::Canceled,
        }
    );
}

// ---------------------------------------------------------------------------
// S6b — additional safe advancements
// ---------------------------------------------------------------------------

#[test]
fn s6b_safe_status_advancements_are_all_auto_repairable() {
    let cases: Vec<(&str, &str, OrderStatus)> = vec![
        ("New", "PartiallyFilled", OrderStatus::PartiallyFilled),
        ("Accepted", "PartiallyFilled", OrderStatus::PartiallyFilled),
        ("New", "Filled", OrderStatus::Filled),
        ("Accepted", "Filled", OrderStatus::Filled),
        ("PartiallyFilled", "Filled", OrderStatus::Filled),
        ("PartiallyFilled", "Canceled", OrderStatus::Canceled),
        ("New", "Canceled", OrderStatus::Canceled),
    ];
    for (local_s, broker_s, expected_status) in cases {
        let diff = ReconcileDiff::OrderMismatch {
            order_id: "ord-x".to_string(),
            field: "status".to_string(),
            local: local_s.to_string(),
            broker: broker_s.to_string(),
        };
        let c = classify_diff(&diff);
        assert_eq!(
            c.severity,
            DriftSeverity::AutoRepairable,
            "{local_s}→{broker_s} should be AutoRepairable"
        );
        assert_eq!(
            c.action,
            RepairAction::SyncLocalStatus {
                order_id: "ord-x".to_string(),
                to_status: expected_status.clone(),
            },
            "{local_s}→{broker_s} wrong target status"
        );
    }
}

// ---------------------------------------------------------------------------
// S7 — OrderMismatch on qty → OperatorOnly
// ---------------------------------------------------------------------------

#[test]
fn s7_order_mismatch_qty_is_operator_only() {
    let diff = ReconcileDiff::OrderMismatch {
        order_id: "ord-7".to_string(),
        field: "qty".to_string(),
        local: "100".to_string(),
        broker: "90".to_string(),
    };
    let c = classify_diff(&diff);
    assert_eq!(c.severity, DriftSeverity::OperatorOnly);
    assert_eq!(c.action, RepairAction::OperatorReview);
}

#[test]
fn s7b_ambiguous_status_direction_is_operator_only() {
    // Backward status (Filled→Accepted) is operator-only.
    let diff = ReconcileDiff::OrderMismatch {
        order_id: "ord-7b".to_string(),
        field: "status".to_string(),
        local: "Filled".to_string(),
        broker: "Accepted".to_string(),
    };
    let c = classify_diff(&diff);
    assert_eq!(c.severity, DriftSeverity::OperatorOnly);
    assert_eq!(c.action, RepairAction::OperatorReview);
}

// ---------------------------------------------------------------------------
// S8 — Mixed plan: auto-repairable + halt-required → overall HaltRequired
// ---------------------------------------------------------------------------

#[test]
fn s8_mixed_plan_worst_case_is_halt_required() {
    // Build a report that has both a safe status mismatch and an unknown fill.
    let mut local = LocalSnapshot::empty();
    local.orders.insert(
        "ord-8a".to_string(),
        order("ord-8a", OrderStatus::New, 100, 0),
    );

    let mut broker = BrokerSnapshot::empty();
    // ord-8a exists at broker but with Accepted status (auto-repairable mismatch).
    broker.orders.insert(
        "ord-8a".to_string(),
        order("ord-8a", OrderStatus::Accepted, 100, 0),
    );
    // An unknown filled order at broker (halt-required).
    broker.orders.insert(
        "ghost-filled".to_string(),
        order("ghost-filled", OrderStatus::Filled, 50, 50),
    );

    let report = reconcile(&local, &broker);
    let plan = build_repair_plan(&report);

    assert!(
        plan.requires_halt(),
        "overall_severity must be HaltRequired when any diff is HaltRequired; got {:?}",
        plan.overall_severity
    );
    assert!(!plan.is_fully_auto_repairable());
    assert!(!plan.requires_operator());

    // At least one auto-repairable entry should still be present.
    let auto_count = plan.auto_repairable().count();
    assert!(
        auto_count >= 1,
        "expected at least one AutoRepairable entry in mixed plan"
    );
}

// ---------------------------------------------------------------------------
// S9 — Clean report → empty plan, overall AutoRepairable
// ---------------------------------------------------------------------------

#[test]
fn s9_clean_report_produces_empty_auto_repairable_plan() {
    let local = LocalSnapshot::empty();
    let broker = BrokerSnapshot::empty();

    let report = reconcile(&local, &broker);
    assert!(report.is_clean());

    let plan = build_repair_plan(&report);
    assert!(plan.classifications.is_empty());
    assert_eq!(plan.overall_severity, DriftSeverity::AutoRepairable);
    assert!(plan.is_fully_auto_repairable());
    assert!(!plan.requires_halt());
    assert!(!plan.requires_operator());
}

// ---------------------------------------------------------------------------
// S10 — Audit trail: build_repair_plan captures all diffs in order
// ---------------------------------------------------------------------------

#[test]
fn s10_repair_plan_captures_every_diff_in_order() {
    // Construct a report with 3 distinct diffs.
    let mut local = LocalSnapshot::empty();
    local.orders.insert(
        "active-1".to_string(),
        order("active-1", OrderStatus::Accepted, 10, 0),
    );

    let mut broker = BrokerSnapshot::empty();
    // active-1 is known at broker but with status mismatch (auto-repairable).
    broker.orders.insert(
        "active-1".to_string(),
        order("active-1", OrderStatus::PartiallyFilled, 10, 5),
    );
    // Unknown open order at broker (operator-only).
    broker.orders.insert(
        "unknown-open".to_string(),
        order("unknown-open", OrderStatus::New, 20, 0),
    );
    // Position mismatch (halt-required).
    local.positions.insert("TSLA".to_string(), 50);
    broker.positions.insert("TSLA".to_string(), 30);

    let report = reconcile(&local, &broker);
    let plan = build_repair_plan(&report);

    // Plan must have exactly as many classifications as report diffs.
    assert_eq!(
        plan.classifications.len(),
        report.diffs.len(),
        "plan must classify every diff"
    );

    // Each classification's diff must match the corresponding report diff.
    for (i, (cls, diff)) in plan
        .classifications
        .iter()
        .zip(report.diffs.iter())
        .enumerate()
    {
        assert_eq!(
            &cls.diff, diff,
            "classification[{i}].diff must equal report.diffs[{i}]"
        );
    }

    // Overall severity must be HaltRequired (position mismatch present).
    assert_eq!(
        plan.overall_severity,
        DriftSeverity::HaltRequired,
        "position mismatch must drive overall_severity to HaltRequired"
    );
}
