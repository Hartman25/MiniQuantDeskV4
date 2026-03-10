//! B4: Observability & Operator Diagnostics — pure in-memory tests.
//!
//! All tests use only in-memory types (no DB, no async).  They prove that the
//! pure builder functions in `mqk_runtime::observability` produce correct,
//! deterministic output from known inputs.

use std::collections::BTreeMap;

use chrono::Utc;
use mqk_db::{InboxRow, OutboxRow};
use mqk_execution::{
    oms::state_machine::{OmsOrder, OrderState},
    BrokerOrderMap,
};
use mqk_portfolio::{Lot, PortfolioState, PositionState};
use mqk_runtime::observability::{
    build_inbox_snapshots, build_order_snapshots, build_outbox_snapshots, build_portfolio_snapshot,
    build_system_block_state, order_state_name,
};
use serde_json::json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// B4-1: empty OMS yields no active orders
// ---------------------------------------------------------------------------

#[test]
fn b4_1_empty_oms_yields_no_active_orders() {
    let oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    let map = BrokerOrderMap::new();
    let snaps = build_order_snapshots(&oms, &map);
    assert!(snaps.is_empty());
}

// ---------------------------------------------------------------------------
// B4-2: order snapshot includes broker_order_id when registered
// ---------------------------------------------------------------------------

#[test]
fn b4_2_order_snapshot_includes_broker_id() {
    let mut oms = BTreeMap::new();
    oms.insert("ord-1".to_string(), OmsOrder::new("ord-1", "AAPL", 100));
    let mut map = BrokerOrderMap::new();
    map.register("ord-1", "broker-abc");

    let snaps = build_order_snapshots(&oms, &map);
    assert_eq!(snaps.len(), 1);
    let s = &snaps[0];
    assert_eq!(s.order_id, "ord-1");
    assert_eq!(s.broker_order_id.as_deref(), Some("broker-abc"));
    assert_eq!(s.symbol, "AAPL");
    assert_eq!(s.total_qty, 100);
    assert_eq!(s.filled_qty, 0);
    assert_eq!(s.status, "Open");
}

// ---------------------------------------------------------------------------
// B4-3: broker_order_id is None when not registered
// ---------------------------------------------------------------------------

#[test]
fn b4_3_order_snapshot_no_broker_id_when_unregistered() {
    let mut oms = BTreeMap::new();
    oms.insert("ord-2".to_string(), OmsOrder::new("ord-2", "MSFT", 50));
    let map = BrokerOrderMap::new(); // empty — submit not yet confirmed

    let snaps = build_order_snapshots(&oms, &map);
    assert_eq!(snaps.len(), 1);
    assert!(snaps[0].broker_order_id.is_none());
}

// ---------------------------------------------------------------------------
// B4-4: order_state_name covers all OrderState variants
// ---------------------------------------------------------------------------

#[test]
fn b4_4_order_state_name_all_variants() {
    assert_eq!(order_state_name(&OrderState::Open), "Open");
    assert_eq!(
        order_state_name(&OrderState::PartiallyFilled),
        "PartiallyFilled"
    );
    assert_eq!(order_state_name(&OrderState::Filled), "Filled");
    assert_eq!(
        order_state_name(&OrderState::CancelPending),
        "CancelPending"
    );
    assert_eq!(order_state_name(&OrderState::Cancelled), "Cancelled");
    assert_eq!(
        order_state_name(&OrderState::ReplacePending),
        "ReplacePending"
    );
    assert_eq!(order_state_name(&OrderState::Rejected), "Rejected");
}

// ---------------------------------------------------------------------------
// B4-5: portfolio snapshot reflects cash and realized PnL
// ---------------------------------------------------------------------------

#[test]
fn b4_5_portfolio_snapshot_cash_and_pnl() {
    let mut p = PortfolioState::new(1_000_000_000); // 1000 USD
                                                    // Manually set cash and realized pnl to simulate post-trade state.
    p.cash_micros = 900_000_000;
    p.realized_pnl_micros = 50_000_000;

    let snap = build_portfolio_snapshot(&p);
    assert_eq!(snap.cash_micros, 900_000_000);
    assert_eq!(snap.realized_pnl_micros, 50_000_000);
    assert!(snap.positions.is_empty());
}

// ---------------------------------------------------------------------------
// B4-6: portfolio snapshot includes position net_qty from lots
// ---------------------------------------------------------------------------

#[test]
fn b4_6_portfolio_snapshot_positions() {
    let mut p = PortfolioState::new(1_000_000_000);

    // Long 10 AAPL @ 150 USD each.
    let mut pos = PositionState::new("AAPL");
    pos.lots.push(Lot::long(10, 150_000_000));
    p.positions.insert("AAPL".to_string(), pos);

    // Short 5 MSFT @ 300 USD each.
    let mut pos2 = PositionState::new("MSFT");
    pos2.lots.push(Lot::short(5, 300_000_000));
    p.positions.insert("MSFT".to_string(), pos2);

    let snap = build_portfolio_snapshot(&p);
    assert_eq!(snap.positions.len(), 2);

    let aapl = snap
        .positions
        .iter()
        .find(|pos| pos.symbol == "AAPL")
        .unwrap();
    assert_eq!(aapl.net_qty, 10);

    let msft = snap
        .positions
        .iter()
        .find(|pos| pos.symbol == "MSFT")
        .unwrap();
    assert_eq!(msft.net_qty, -5);
}

// ---------------------------------------------------------------------------
// B4-7: outbox snapshots correctly map OutboxRow fields
// ---------------------------------------------------------------------------

#[test]
fn b4_7_outbox_snapshots_map_fields() {
    let now = Utc::now();
    let run_id = Uuid::new_v4();
    let rows = vec![OutboxRow {
        outbox_id: 42,
        run_id,
        idempotency_key: "key-abc".to_string(),
        order_json: json!({"symbol": "AAPL", "quantity": 10}),
        status: "PENDING".to_string(),
        created_at_utc: now,
        sent_at_utc: None,
        claimed_at_utc: None,
        claimed_by: None,
        dispatching_at_utc: None,
        dispatch_attempt_id: None,
    }];

    let snaps = build_outbox_snapshots(&rows);
    assert_eq!(snaps.len(), 1);
    let s = &snaps[0];
    assert_eq!(s.outbox_id, 42);
    assert_eq!(s.idempotency_key, "key-abc");
    assert_eq!(s.status, "PENDING");
    assert!(s.sent_at_utc.is_none());
    assert!(s.claimed_at_utc.is_none());
    assert!(s.dispatching_at_utc.is_none());
}

// ---------------------------------------------------------------------------
// B4-8: inbox snapshots extract event_type; unknown fallback
// ---------------------------------------------------------------------------

#[test]
fn b4_8_inbox_snapshots_extract_event_type() {
    let now = Utc::now();
    let run_id = Uuid::new_v4();

    let rows = vec![
        // Row with "type" field present.
        InboxRow {
            inbox_id: 1,
            run_id,
            broker_message_id: "msg-fill-1".to_string(),
            message_json: json!({"type": "fill", "broker_order_id": "brk-1"}),
            received_at_utc: now,
            applied_at_utc: Some(now),
        },
        // Row without "type" field → should fall back to "unknown".
        InboxRow {
            inbox_id: 2,
            run_id,
            broker_message_id: "msg-no-type".to_string(),
            message_json: json!({"broker_order_id": "brk-2"}),
            received_at_utc: now,
            applied_at_utc: None,
        },
    ];

    let snaps = build_inbox_snapshots(&rows);
    assert_eq!(snaps.len(), 2);

    let fill = snaps
        .iter()
        .find(|s| s.broker_message_id == "msg-fill-1")
        .unwrap();
    assert_eq!(fill.event_type, "fill");
    assert!(fill.applied);
    assert!(fill.applied_at_utc.is_some());

    let no_type = snaps
        .iter()
        .find(|s| s.broker_message_id == "msg-no-type")
        .unwrap();
    assert_eq!(no_type.event_type, "unknown");
    assert!(!no_type.applied);
    assert!(no_type.applied_at_utc.is_none());
}

// ---------------------------------------------------------------------------
// B4-9: HALTED_IN_DB takes priority over INTEGRITY_DISARMED
// ---------------------------------------------------------------------------

#[test]
fn b4_9_system_block_halted_takes_priority_over_disarmed() {
    // Both halted AND disarmed: HALTED_IN_DB must win.
    let block = build_system_block_state(true, true, Some("manual disarm"), vec![])
        .expect("should be blocked");

    assert_eq!(block.reason_code, "HALTED_IN_DB");

    // Evidence must contain run_status = HALTED.
    let has_run_status = block
        .evidence
        .iter()
        .any(|(k, v)| k == "run_status" && v == "HALTED");
    assert!(has_run_status);
}

// ---------------------------------------------------------------------------
// B4-10: None when system is not blocked
// ---------------------------------------------------------------------------

#[test]
fn b4_10_system_block_none_when_not_blocked() {
    let block = build_system_block_state(false, false, None, vec![]);
    assert!(block.is_none(), "expected no block state when healthy");
}
