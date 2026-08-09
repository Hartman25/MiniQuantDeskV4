//! BROKER-EVENT-APPLY-REPLAY-BURST-01 proof tests.
//!
//! # Problem addressed
//!
//! On a fresh runtime start, REST polling (`fetch_events`) fetches all fill
//! activities since the broker cursor's `rest_activity_after` position.  When
//! that position is None or stale (first start, cursor reset, baseline adoption),
//! historical fills from prior runs are returned.  These fills carry
//! `internal_order_id` values from orders never submitted by the current run.
//! In Phase 3 apply they produce `UNKNOWN_ORDER_FILL` → `IntegrityViolation`
//! halt.  The active run has zero outbox entries and zero oms_orders entries.
//!
//! # Fix
//!
//! Phase 2 ingest now gates fill events on `oms_orders` membership before
//! inserting into `oms_inbox`.  Historical fills (absent from `oms_orders`)
//! are logged as `exec_broker_fill_skipped_no_run_ownership` and skipped; the
//! broker cursor still advances past them.  Non-fill events are unaffected.
//!
//! # Coverage
//!
//! | Test   | Claim                                                                        |
//! |--------|------------------------------------------------------------------------------|
//! | REB01  | Fill for order absent from oms_orders: gate returns `true` (skip)            |
//! | REB01B | PartialFill for absent order: gate returns `true` (skip)                     |
//! | REB02  | Fill for order absent from oms_orders: gate returns `true` even if the      |
//! |        | broker_message_id was seen before (idempotent skip, not a dedupe concern)    |
//! | REB03  | Fill for order present in oms_orders: gate returns `false` (ingest)          |
//! | REB03B | PartialFill for present order: gate returns `false` (ingest)                 |
//! | REB04  | Non-fill event for absent order: gate returns `false` (ingest proceeds;      |
//! |        | Phase 3 apply silently skips non-fills for unknown orders without halting)   |
//! | REB05  | Gate is false for every non-fill BrokerEvent variant regardless of           |
//! |        | oms_orders state — confirms the guard never over-fires on non-fills          |
//! | REB06  | Empty oms_orders + mix of fill and non-fill events: fills skipped,           |
//! |        | non-fills not skipped (bulk discriminant proof)                             |
//!
//! # Proof boundary
//!
//! All tests are pure in-process.  No DB or network required.
//! DB-backed Phase 2/3 end-to-end proof is deferred to a future DB-backed
//! variant requiring `MQK_DATABASE_URL`.

use std::collections::BTreeMap;

use mqk_execution::oms::state_machine::OmsOrder;
use mqk_execution::{BrokerEvent, Side};
use mqk_runtime::orchestrator::is_historical_fill_no_run_ownership;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_fill(order_id: &str, msg_id: &str, qty: i64) -> BrokerEvent {
    BrokerEvent::Fill {
        broker_message_id: msg_id.to_string(),
        broker_fill_id: None,
        internal_order_id: order_id.to_string(),
        broker_order_id: None,
        symbol: "SPY".to_string(),
        side: Side::Buy,
        delta_qty: qty,
        price_micros: 500_000_000,
        fee_micros: 0,
    }
}

fn make_partial_fill(order_id: &str, msg_id: &str, qty: i64) -> BrokerEvent {
    BrokerEvent::PartialFill {
        broker_message_id: msg_id.to_string(),
        broker_fill_id: None,
        internal_order_id: order_id.to_string(),
        broker_order_id: None,
        symbol: "SPY".to_string(),
        side: Side::Buy,
        delta_qty: qty,
        price_micros: 500_000_000,
        fee_micros: 0,
        cum_qty_after: None,
    }
}

fn make_ack(order_id: &str, msg_id: &str) -> BrokerEvent {
    BrokerEvent::Ack {
        broker_message_id: msg_id.to_string(),
        internal_order_id: order_id.to_string(),
        broker_order_id: None,
    }
}

fn make_cancel_ack(order_id: &str, msg_id: &str) -> BrokerEvent {
    BrokerEvent::CancelAck {
        broker_message_id: msg_id.to_string(),
        internal_order_id: order_id.to_string(),
        broker_order_id: None,
    }
}

fn make_reject(order_id: &str, msg_id: &str) -> BrokerEvent {
    BrokerEvent::Reject {
        broker_message_id: msg_id.to_string(),
        internal_order_id: order_id.to_string(),
        broker_order_id: None,
    }
}

fn oms_with_order(order_id: &str) -> BTreeMap<String, OmsOrder> {
    let mut map = BTreeMap::new();
    map.insert(order_id.to_string(), OmsOrder::new(order_id, "SPY", 100));
    map
}

// ---------------------------------------------------------------------------
// REB01 — Fill for absent order: gate fires (skip).
// ---------------------------------------------------------------------------

#[test]
fn reb01_historical_fill_absent_order_gate_fires() {
    let oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    let ev = make_fill("old-order-from-jan-2026", "alpaca:old-order:fill:ts", 50);

    assert!(
        is_historical_fill_no_run_ownership(&ev, &oms),
        "REB01: fill for absent order must be identified as historical (gate=true)"
    );
}

// ---------------------------------------------------------------------------
// REB01B — PartialFill for absent order: gate fires (skip).
// ---------------------------------------------------------------------------

#[test]
fn reb01b_historical_partial_fill_absent_order_gate_fires() {
    let oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    let ev = make_partial_fill(
        "old-order-from-feb-2026",
        "alpaca:old-order:partial_fill:ts",
        25,
    );

    assert!(
        is_historical_fill_no_run_ownership(&ev, &oms),
        "REB01B: partial fill for absent order must be identified as historical (gate=true)"
    );
}

// ---------------------------------------------------------------------------
// REB02 — Same broker_message_id with absent order: gate still fires.
//
// Proves the gate is not a dedupe check — it fires based on oms_orders
// membership regardless of whether the broker_message_id was seen before.
// Idempotent skipping is correct: replaying the same historical fill across
// restarts must not create new inbox rows or reach apply.
// ---------------------------------------------------------------------------

#[test]
fn reb02_repeated_historical_fill_same_message_id_gate_fires() {
    let oms: BTreeMap<String, OmsOrder> = BTreeMap::new();
    let msg_id = "alpaca:historical-order:fill:2026-01-15T10:00:00Z";

    let ev1 = make_fill("historical-order", msg_id, 100);
    let ev2 = make_fill("historical-order", msg_id, 100);

    assert!(
        is_historical_fill_no_run_ownership(&ev1, &oms),
        "REB02: first occurrence of historical fill must trigger gate"
    );
    assert!(
        is_historical_fill_no_run_ownership(&ev2, &oms),
        "REB02: repeated historical fill with same broker_message_id must still trigger gate"
    );
}

// ---------------------------------------------------------------------------
// REB03 — Fill for present order: gate does not fire (ingest proceeds).
// ---------------------------------------------------------------------------

#[test]
fn reb03_current_run_fill_present_order_gate_does_not_fire() {
    let oms = oms_with_order("current-run-order-1");
    let ev = make_fill(
        "current-run-order-1",
        "alpaca:current-run-order-1:fill:ts",
        100,
    );

    assert!(
        !is_historical_fill_no_run_ownership(&ev, &oms),
        "REB03: fill for order present in oms_orders must NOT trigger gate (ingest proceeds)"
    );
}

// ---------------------------------------------------------------------------
// REB03B — PartialFill for present order: gate does not fire.
// ---------------------------------------------------------------------------

#[test]
fn reb03b_current_run_partial_fill_present_order_gate_does_not_fire() {
    let oms = oms_with_order("current-run-order-2");
    let ev = make_partial_fill(
        "current-run-order-2",
        "alpaca:current-run-order-2:partial_fill:ts",
        50,
    );

    assert!(
        !is_historical_fill_no_run_ownership(&ev, &oms),
        "REB03B: partial fill for present order must NOT trigger gate"
    );
}

// ---------------------------------------------------------------------------
// REB04 — Non-fill event for absent order: gate does not fire.
//
// Ack/CancelAck/Reject for unknown orders are handled by Phase 3 apply
// (silently skipped without halting).  The Phase 2 gate must not fire on
// non-fills — doing so would suppress events we want to ingest for observability
// and would change existing apply semantics.
// ---------------------------------------------------------------------------

#[test]
fn reb04_non_fill_absent_order_gate_does_not_fire() {
    let oms: BTreeMap<String, OmsOrder> = BTreeMap::new();

    let ack = make_ack("old-order", "alpaca:old-order:new:ts");
    let cancel_ack = make_cancel_ack("old-order", "alpaca:old-order:canceled:ts");
    let reject = make_reject("old-order", "alpaca:old-order:rejected:ts");

    assert!(
        !is_historical_fill_no_run_ownership(&ack, &oms),
        "REB04: Ack for absent order must NOT trigger gate (non-fill is safe to ingest)"
    );
    assert!(
        !is_historical_fill_no_run_ownership(&cancel_ack, &oms),
        "REB04: CancelAck for absent order must NOT trigger gate"
    );
    assert!(
        !is_historical_fill_no_run_ownership(&reject, &oms),
        "REB04: Reject for absent order must NOT trigger gate"
    );
}

// ---------------------------------------------------------------------------
// REB05 — Gate is false for every non-fill BrokerEvent variant.
//
// Exhaustive proof that the gate is strictly limited to Fill and PartialFill.
// ---------------------------------------------------------------------------

#[test]
fn reb05_gate_never_fires_on_any_non_fill_variant() {
    let oms: BTreeMap<String, OmsOrder> = BTreeMap::new();

    let non_fills: Vec<BrokerEvent> = vec![
        BrokerEvent::Ack {
            broker_message_id: "m1".to_string(),
            internal_order_id: "ord".to_string(),
            broker_order_id: None,
        },
        BrokerEvent::CancelAck {
            broker_message_id: "m2".to_string(),
            internal_order_id: "ord".to_string(),
            broker_order_id: None,
        },
        BrokerEvent::CancelReject {
            broker_message_id: "m3".to_string(),
            internal_order_id: "ord".to_string(),
            broker_order_id: None,
        },
        BrokerEvent::ReplaceAck {
            broker_message_id: "m4".to_string(),
            internal_order_id: "ord".to_string(),
            broker_order_id: None,
            new_total_qty: 100,
        },
        BrokerEvent::ReplaceReject {
            broker_message_id: "m5".to_string(),
            internal_order_id: "ord".to_string(),
            broker_order_id: None,
        },
        BrokerEvent::Reject {
            broker_message_id: "m6".to_string(),
            internal_order_id: "ord".to_string(),
            broker_order_id: None,
        },
    ];

    for ev in &non_fills {
        assert!(
            !is_historical_fill_no_run_ownership(ev, &oms),
            "REB05: non-fill variant {:?} must never trigger the historical fill gate",
            std::mem::discriminant(ev)
        );
    }
}

// ---------------------------------------------------------------------------
// REB06 — Bulk discriminant: empty oms, mixed event stream.
//
// Given an empty oms_orders (fresh run with no submitted orders) and a stream
// of mixed broker events — fill events must all trigger the gate, non-fill
// events must never trigger it.
// ---------------------------------------------------------------------------

#[test]
fn reb06_empty_oms_mixed_stream_fills_skipped_non_fills_not() {
    let oms: BTreeMap<String, OmsOrder> = BTreeMap::new();

    let events: Vec<(BrokerEvent, bool)> = vec![
        (
            make_fill("spy-jan-2026", "alpaca:spy-jan:fill:ts1", 100),
            true,
        ),
        (
            make_fill("spy-feb-2026", "alpaca:spy-feb:fill:ts2", 50),
            true,
        ),
        (
            make_partial_fill("aapl-may-2026", "alpaca:aapl-may:partial_fill:ts3", 10),
            true,
        ),
        (make_ack("spy-jan-2026", "alpaca:spy-jan:new:ts0"), false),
        (
            make_cancel_ack("spy-feb-2026", "alpaca:spy-feb:canceled:ts4"),
            false,
        ),
        (
            make_reject("other-old-order", "alpaca:other:rejected:ts5"),
            false,
        ),
    ];

    for (ev, expect_skip) in &events {
        let got = is_historical_fill_no_run_ownership(ev, &oms);
        assert_eq!(
            got,
            *expect_skip,
            "REB06: event broker_message_id='{}' internal_order_id='{}' \
             expected gate={expect_skip} got gate={got}",
            ev.broker_message_id(),
            ev.internal_order_id(),
        );
    }
}
