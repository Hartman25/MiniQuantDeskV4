//! Inbound lifecycle proof tests - Patch A5 closure.
//!
//! # Purpose
//!
//! Proves that all 8 canonical Alpaca inbound lifecycle variants can be
//! ingested and normalized correctly through the adapter ingestion boundary
//! (`normalize_trade_update`).
//!
//! The contract tests (C1-C10) prove variant correctness and field-level
//! parsing rules.  These tests go further: they verify exact field values
//! (broker_message_id format, broker_order_id, internal_order_id,
//! new_total_qty) for every non-fill lifecycle event, prove
//! broker_message_id uniqueness across all 8 variants on the same order at
//! the same timestamp, and prove that complete order lifecycle sequences
//! normalize correctly without state interference.
//!
//! # Coverage
//!
//! IL-1  Ack (new / pending_new / accepted): all ID fields verified exactly.
//! IL-2  CancelAck (canceled / expired): all ID fields verified exactly.
//! IL-3  CancelReject: all ID fields verified exactly.
//! IL-4  ReplaceAck: new_total_qty and all ID fields verified exactly.
//! IL-5  ReplaceReject: all ID fields verified exactly.
//! IL-6  Reject: all ID fields verified exactly.
//! IL-7  All 8 canonical variants at the same timestamp produce distinct
//!       broker_message_ids.  Uniqueness comes from the event-type component.
//! IL-8  Full order lifecycle: new → partial_fill → fill.
//!       Per-event fields verified for all three events; no state interference.
//! IL-9  Cancel lifecycle: new → canceled.  Terminal state is CancelAck.
//! IL-10 Replace lifecycle: new → replaced.  new_total_qty from order.qty.
//! IL-11 Adapter ingestion boundary accepts all 11 known Alpaca event strings
//!       (11 strings → 8 canonical BrokerEvent variants).
//!
//! # A5 classification
//!
//! **Normalization boundary: complete.**  `normalize_trade_update` is fully
//! proven for all 8 lifecycle variants.
//!
//! **`fetch_events` production path: partial.**  REST polling
//! (`GET /v2/account/activities`) only surfaces `FILL` and `PARTIAL_FILL`.
//! The 6 remaining lifecycle variants - Ack, CancelAck, CancelReject,
//! ReplaceAck, ReplaceReject, Reject - require the Alpaca websocket
//! trade-update stream, which is not yet integrated.  IL-11 documents this
//! boundary explicitly as a machine-readable record.
//!
//! All tests are pure in-memory - no network, no DB, no wall-clock reads.
use mqk_broker_alpaca::{
    normalize::{normalize_trade_update, trade_update_message_id},
    types::{AlpacaOrder, AlpacaTradeUpdate},
};
use mqk_execution::BrokerEvent;
// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------
/// Alpaca-assigned broker order UUID - must differ from CLIENT_ID.
const BROKER_ID: &str = "alpaca-broker-uuid-il-abc123";
/// Internal / client order ID set by us at submit time.
const CLIENT_ID: &str = "internal-client-il-xyz789";
const SYMBOL: &str = "MSFT";
/// Three ordered timestamps; distinct values prove temporal ordering in IL-8/9/10.
const TS_1: &str = "2024-06-15T09:30:00.000000Z";
const TS_2: &str = "2024-06-15T09:31:00.000000Z";
const TS_3: &str = "2024-06-15T09:32:00.000000Z";
fn make_order(id: &str, client_id: &str, symbol: &str, side: &str, qty: &str) -> AlpacaOrder {
    AlpacaOrder {
        id: id.to_string(),
        client_order_id: client_id.to_string(),
        symbol: symbol.to_string(),
        side: side.to_string(),
        qty: qty.to_string(),
        filled_qty: "0".to_string(),
    }
}
fn make_update(
    event: &str,
    ord: AlpacaOrder,
    price: Option<&str>,
    qty: Option<&str>,
    ts: &str,
) -> AlpacaTradeUpdate {
    AlpacaTradeUpdate {
        event: event.to_string(),
        timestamp: ts.to_string(),
        order: ord,
        price: price.map(str::to_string),
        qty: qty.map(str::to_string),
    }
}
/// Default order using the shared test constants (buy side, given qty).
fn default_order(qty: &str) -> AlpacaOrder {
    make_order(BROKER_ID, CLIENT_ID, SYMBOL, "buy", qty)
}
/// Minimal update with no price/qty fields, using the shared test constants.
fn simple_update(event: &str, ts: &str) -> AlpacaTradeUpdate {
    make_update(event, default_order("100"), None, None, ts)
}
// ---------------------------------------------------------------------------
// IL-1 - Ack: all ID fields verified for "new", "pending_new", "accepted"
// ---------------------------------------------------------------------------
#[test]
fn il1_ack_new_all_id_fields_exact() {
    let u = simple_update("new", TS_1);
    let ev = normalize_trade_update(&u).unwrap();
    // Verify all three ID fields before consuming ev in the variant check.
    assert_eq!(
        ev.broker_message_id(),
        format!("alpaca:{BROKER_ID}:new:{TS_1}")
    );
    assert_eq!(ev.internal_order_id(), CLIENT_ID);
    assert_eq!(ev.broker_order_id(), Some(BROKER_ID));
    assert!(matches!(ev, BrokerEvent::Ack { .. }), "variant must be Ack");
}
#[test]
fn il1_ack_pending_new_all_id_fields_exact() {
    let u = simple_update("pending_new", TS_1);
    let ev = normalize_trade_update(&u).unwrap();
    assert_eq!(
        ev.broker_message_id(),
        format!("alpaca:{BROKER_ID}:pending_new:{TS_1}")
    );
    assert_eq!(ev.internal_order_id(), CLIENT_ID);
    assert_eq!(ev.broker_order_id(), Some(BROKER_ID));
    assert!(matches!(ev, BrokerEvent::Ack { .. }), "variant must be Ack");
}
#[test]
fn il1_ack_accepted_all_id_fields_exact() {
    let u = simple_update("accepted", TS_1);
    let ev = normalize_trade_update(&u).unwrap();
    assert_eq!(
        ev.broker_message_id(),
        format!("alpaca:{BROKER_ID}:accepted:{TS_1}")
    );
    assert_eq!(ev.internal_order_id(), CLIENT_ID);
    assert_eq!(ev.broker_order_id(), Some(BROKER_ID));
    assert!(matches!(ev, BrokerEvent::Ack { .. }), "variant must be Ack");
}
// ---------------------------------------------------------------------------
// IL-2 - CancelAck: all ID fields for "canceled" and "expired"
// ---------------------------------------------------------------------------
#[test]
fn il2_cancel_ack_canceled_all_id_fields_exact() {
    let u = simple_update("canceled", TS_2);
    let ev = normalize_trade_update(&u).unwrap();
    assert_eq!(
        ev.broker_message_id(),
        format!("alpaca:{BROKER_ID}:canceled:{TS_2}")
    );
    assert_eq!(ev.internal_order_id(), CLIENT_ID);
    assert_eq!(ev.broker_order_id(), Some(BROKER_ID));
    assert!(
        matches!(ev, BrokerEvent::CancelAck { .. }),
        "variant must be CancelAck"
    );
}
#[test]
fn il2_cancel_ack_expired_all_id_fields_exact() {
    let u = simple_update("expired", TS_2);
    let ev = normalize_trade_update(&u).unwrap();
    assert_eq!(
        ev.broker_message_id(),
        format!("alpaca:{BROKER_ID}:expired:{TS_2}")
    );
    assert_eq!(ev.internal_order_id(), CLIENT_ID);
    assert_eq!(ev.broker_order_id(), Some(BROKER_ID));
    assert!(
        matches!(ev, BrokerEvent::CancelAck { .. }),
        "variant must be CancelAck"
    );
}
// ---------------------------------------------------------------------------
// IL-3 - CancelReject: all ID fields verified
// ---------------------------------------------------------------------------
#[test]
fn il3_cancel_reject_all_id_fields_exact() {
    let u = simple_update("cancel_rejected", TS_2);
    let ev = normalize_trade_update(&u).unwrap();
    assert_eq!(
        ev.broker_message_id(),
        format!("alpaca:{BROKER_ID}:cancel_rejected:{TS_2}")
    );
    assert_eq!(ev.internal_order_id(), CLIENT_ID);
    assert_eq!(ev.broker_order_id(), Some(BROKER_ID));
    assert!(
        matches!(ev, BrokerEvent::CancelReject { .. }),
        "variant must be CancelReject"
    );
}
// ---------------------------------------------------------------------------
// IL-4 - ReplaceAck: new_total_qty and all ID fields verified
// ---------------------------------------------------------------------------
#[test]
fn il4_replace_ack_new_total_qty_and_id_fields_exact() {
    // Alpaca echoes order.qty = 120 after a replace (e.g. 20 filled + 100 new leaves).
    // new_total_qty in the canonical event must equal order.qty from the wire.
    let ord = make_order(BROKER_ID, CLIENT_ID, SYMBOL, "buy", "120");
    let u = make_update("replaced", ord, None, None, TS_2);
    let ev = normalize_trade_update(&u).unwrap();
    match ev {
        BrokerEvent::ReplaceAck {
            broker_message_id,
            internal_order_id,
            broker_order_id,
            new_total_qty,
        } => {
            assert_eq!(
                broker_message_id,
                format!("alpaca:{BROKER_ID}:replaced:{TS_2}")
            );
            assert_eq!(internal_order_id, CLIENT_ID);
            assert_eq!(broker_order_id, Some(BROKER_ID.to_string()));
            assert_eq!(new_total_qty, 120, "new_total_qty must equal order.qty");
        }
        other => panic!("expected ReplaceAck, got {other:?}"),
    }
}
// ---------------------------------------------------------------------------
// IL-5 - ReplaceReject: all ID fields verified
// ---------------------------------------------------------------------------
#[test]
fn il5_replace_reject_all_id_fields_exact() {
    let u = simple_update("replace_rejected", TS_2);
    let ev = normalize_trade_update(&u).unwrap();
    assert_eq!(
        ev.broker_message_id(),
        format!("alpaca:{BROKER_ID}:replace_rejected:{TS_2}")
    );
    assert_eq!(ev.internal_order_id(), CLIENT_ID);
    assert_eq!(ev.broker_order_id(), Some(BROKER_ID));
    assert!(
        matches!(ev, BrokerEvent::ReplaceReject { .. }),
        "variant must be ReplaceReject"
    );
}
// ---------------------------------------------------------------------------
// IL-6 - Reject: all ID fields verified
// ---------------------------------------------------------------------------
#[test]
fn il6_reject_all_id_fields_exact() {
    let u = simple_update("rejected", TS_1);
    let ev = normalize_trade_update(&u).unwrap();
    assert_eq!(
        ev.broker_message_id(),
        format!("alpaca:{BROKER_ID}:rejected:{TS_1}")
    );
    assert_eq!(ev.internal_order_id(), CLIENT_ID);
    assert_eq!(ev.broker_order_id(), Some(BROKER_ID));
    assert!(
        matches!(ev, BrokerEvent::Reject { .. }),
        "variant must be Reject"
    );
}
// ---------------------------------------------------------------------------
// IL-7 - All 8 canonical variants at the same timestamp produce distinct
//         broker_message_ids.  Uniqueness is from the event-type component.
// ---------------------------------------------------------------------------
#[test]
fn il7_all_8_variants_produce_distinct_broker_message_ids_at_same_timestamp() {
    // All 8 event types use TS_1 as the timestamp.
    // The uniqueness of broker_message_id must come from the event-type
    // component, not from distinct timestamps.
    let event_args: &[(&str, Option<&str>, Option<&str>)] = &[
        ("new", None, None),
        ("partial_fill", Some("100.00"), Some("10")),
        ("fill", Some("100.00"), Some("10")),
        ("canceled", None, None),
        ("cancel_rejected", None, None),
        ("replaced", None, None),
        ("replace_rejected", None, None),
        ("rejected", None, None),
    ];
    let mut seen = std::collections::HashSet::new();
    for (event_str, price, qty) in event_args {
        let u = make_update(
            event_str,
            default_order("100"),
            *price,
            *qty,
            TS_1, // identical timestamp for all
        );
        let ev = normalize_trade_update(&u).unwrap();
        let mid = ev.broker_message_id().to_string();
        assert!(
            seen.insert(mid.clone()),
            "event={event_str}: broker_message_id {mid:?} collides with a prior variant"
        );
    }
    assert_eq!(
        seen.len(),
        8,
        "all 8 canonical event types must produce distinct broker_message_ids"
    );
}
// ---------------------------------------------------------------------------
// IL-8 - Full order lifecycle: new → partial_fill → fill
//         Per-event fields verified; no state interference between events.
// ---------------------------------------------------------------------------
#[test]
fn il8_fill_lifecycle_new_then_partial_fill_then_fill() {
    // T1: Ack - order placed and accepted.
    let u1 = simple_update("new", TS_1);
    let ev1 = normalize_trade_update(&u1).unwrap();
    assert_eq!(ev1.broker_order_id(), Some(BROKER_ID));
    assert_eq!(ev1.internal_order_id(), CLIENT_ID);
    assert_eq!(
        ev1.broker_message_id(),
        format!("alpaca:{BROKER_ID}:new:{TS_1}")
    );
    assert!(matches!(ev1, BrokerEvent::Ack { .. }), "T1 must be Ack");
    // T2: PartialFill - 40 shares at $150.00.
    let u2 = make_update(
        "partial_fill",
        default_order("100"),
        Some("150.00"),
        Some("40"),
        TS_2,
    );
    let ev2 = normalize_trade_update(&u2).unwrap();
    match ev2 {
        BrokerEvent::PartialFill {
            broker_message_id,
            delta_qty,
            price_micros,
            fee_micros,
            ..
        } => {
            assert_eq!(
                broker_message_id,
                format!("alpaca:{BROKER_ID}:partial_fill:{TS_2}")
            );
            assert_eq!(delta_qty, 40);
            assert_eq!(price_micros, 150_000_000);
            assert_eq!(fee_micros, 0);
        }
        other => panic!("T2 expected PartialFill, got {other:?}"),
    }
    // T3: Fill - remaining 60 shares at $150.00.
    let u3 = make_update(
        "fill",
        default_order("100"),
        Some("150.00"),
        Some("60"),
        TS_3,
    );
    let ev3 = normalize_trade_update(&u3).unwrap();
    match ev3 {
        BrokerEvent::Fill {
            broker_message_id,
            delta_qty,
            price_micros,
            fee_micros,
            ..
        } => {
            assert_eq!(broker_message_id, format!("alpaca:{BROKER_ID}:fill:{TS_3}"));
            assert_eq!(delta_qty, 60);
            assert_eq!(price_micros, 150_000_000);
            assert_eq!(fee_micros, 0);
        }
        other => panic!("T3 expected Fill, got {other:?}"),
    }
}
// ---------------------------------------------------------------------------
// IL-9 - Cancel lifecycle: new → canceled
//         Terminal state is CancelAck; broker_message_ids are distinct.
// ---------------------------------------------------------------------------
#[test]
fn il9_cancel_lifecycle_new_then_cancel_ack() {
    // T1: order placed, Ack received.
    let u1 = simple_update("new", TS_1);
    let ev1 = normalize_trade_update(&u1).unwrap();
    let mid1 = ev1.broker_message_id().to_string();
    assert!(matches!(ev1, BrokerEvent::Ack { .. }), "T1 must be Ack");
    assert_eq!(mid1, format!("alpaca:{BROKER_ID}:new:{TS_1}"));
    // T2: cancel accepted, CancelAck received.
    let u2 = simple_update("canceled", TS_2);
    let ev2 = normalize_trade_update(&u2).unwrap();
    assert_eq!(ev2.broker_order_id(), Some(BROKER_ID));
    assert_eq!(ev2.internal_order_id(), CLIENT_ID);
    let mid2 = ev2.broker_message_id().to_string();
    assert!(
        matches!(ev2, BrokerEvent::CancelAck { .. }),
        "T2 must be CancelAck"
    );
    assert_eq!(mid2, format!("alpaca:{BROKER_ID}:canceled:{TS_2}"));
    // Different event types must produce different broker_message_ids.
    assert_ne!(
        mid1, mid2,
        "Ack and CancelAck on the same order must have distinct broker_message_ids"
    );
}
// ---------------------------------------------------------------------------
// IL-10 - Replace lifecycle: new → replaced
//          ReplaceAck.new_total_qty is populated from order.qty.
// ---------------------------------------------------------------------------
#[test]
fn il10_replace_lifecycle_new_then_replace_ack() {
    // T1: original order for 100 shares, Ack received.
    let u1 = simple_update("new", TS_1);
    let ev1 = normalize_trade_update(&u1).unwrap();
    assert!(matches!(ev1, BrokerEvent::Ack { .. }), "T1 must be Ack");
    // T2: replace accepted; Alpaca echoes order.qty = 80 (the amended total).
    let ord2 = make_order(BROKER_ID, CLIENT_ID, SYMBOL, "buy", "80");
    let u2 = make_update("replaced", ord2, None, None, TS_2);
    let ev2 = normalize_trade_update(&u2).unwrap();
    match ev2 {
        BrokerEvent::ReplaceAck {
            new_total_qty,
            broker_order_id,
            internal_order_id,
            broker_message_id,
        } => {
            assert_eq!(
                new_total_qty, 80,
                "new_total_qty must reflect the amended order.qty"
            );
            assert_eq!(broker_order_id, Some(BROKER_ID.to_string()));
            assert_eq!(internal_order_id, CLIENT_ID);
            assert_eq!(
                broker_message_id,
                format!("alpaca:{BROKER_ID}:replaced:{TS_2}")
            );
        }
        other => panic!("T2 expected ReplaceAck, got {other:?}"),
    }
}
// ---------------------------------------------------------------------------
// IL-11 - Adapter ingestion boundary accepts all 11 known Alpaca event strings.
//
// This test is the machine-readable record of the normalization boundary
// contract.  All 11 event strings listed here must be accepted by
// `normalize_trade_update` and produce a canonical `BrokerEvent`.
//
// 11 accepted strings → 8 canonical BrokerEvent variants:
//   "new" | "pending_new" | "accepted"  → Ack
//   "partial_fill"                       → PartialFill
//   "fill"                               → Fill
//   "canceled" | "expired"              → CancelAck
//   "cancel_rejected"                    → CancelReject
//   "replaced"                           → ReplaceAck
//   "replace_rejected"                   → ReplaceReject
//   "rejected"                           → Reject
//
// fetch_events limitation: Ack, CancelAck, CancelReject, ReplaceAck,
// ReplaceReject, and Reject are NOT delivered by REST polling.  The
// normalization layer is proven complete here; the event source gap
// (websocket) is the remaining work for full A5 closure.
// ---------------------------------------------------------------------------
#[test]
fn il11_adapter_ingestion_boundary_accepts_all_11_known_event_type_strings() {
    let known_types: &[(&str, Option<&str>, Option<&str>)] = &[
        ("new", None, None),
        ("pending_new", None, None),
        ("accepted", None, None),
        ("partial_fill", Some("100.00"), Some("10")),
        ("fill", Some("100.00"), Some("10")),
        ("canceled", None, None),
        ("expired", None, None),
        ("cancel_rejected", None, None),
        ("replaced", None, None),
        ("replace_rejected", None, None),
        ("rejected", None, None),
    ];
    for (event_str, price, qty) in known_types {
        let u = make_update(event_str, default_order("100"), *price, *qty, TS_1);
        normalize_trade_update(&u).unwrap_or_else(|e| {
            panic!(
                "event={event_str}: adapter ingestion boundary must accept this event type, \
                 got {e:?}"
            )
        });
    }
}
#[test]
fn alpaca_websocket_message_identity_is_stable() {
    let update = simple_update("accepted", TS_1);
    let id1 = trade_update_message_id(&update);
    let id2 = trade_update_message_id(&update);
    assert_eq!(id1, id2);
    assert_eq!(id1, format!("alpaca:{BROKER_ID}:accepted:{TS_1}"));
}
#[test]
fn duplicate_websocket_trade_update_is_dedupe_safe() {
    let update = make_update(
        "partial_fill",
        default_order("100"),
        Some("100.00"),
        Some("10"),
        TS_2,
    );
    let ev1 = normalize_trade_update(&update).expect("first normalize");
    let ev2 = normalize_trade_update(&update).expect("second normalize");
    assert_eq!(ev1.broker_message_id(), ev2.broker_message_id());
}
#[test]
fn out_of_order_websocket_update_is_safe() {
    let rejected = simple_update("rejected", TS_3);
    let accepted = simple_update("accepted", TS_1);
    let reject_event = normalize_trade_update(&rejected).expect("reject normalize");
    let ack_event = normalize_trade_update(&accepted).expect("ack normalize");
    assert!(matches!(reject_event, BrokerEvent::Reject { .. }));
    assert!(matches!(ack_event, BrokerEvent::Ack { .. }));
    assert_ne!(
        reject_event.broker_message_id(),
        ack_event.broker_message_id()
    );
}
