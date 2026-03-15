//! BRK-03R / BRK-04R / BRK-05R / BRK-06R — Canonical broker event mapping proofs.
//!
//! # Purpose
//!
//! Proves that every Alpaca lifecycle event type flows correctly through the
//! authoritative WS ingest path:
//!
//! ```text
//! raw WS JSON bytes
//!   → parse_ws_message
//!   → AlpacaWsMessage::TradeUpdate
//!   → build_inbound_batch_from_ws_update
//!   → InboundBatch { events: [BrokerEvent], cursor: Live }
//! ```
//!
//! The normalization layer (`normalize_trade_update`) is already proven complete
//! by the A5 contract tests (C1-C10) and lifecycle tests (IL-1-IL-11).  These
//! tests extend that coverage to the WS parse + ingest boundary — the path
//! actually traversed by live inbound events.
//!
//! All tests are pure in-memory — no network, no DB, no wall-clock reads.
//!
//! # Coverage
//!
//! ## BRK-03R — Ack mapping
//! A1  "new"         → BrokerEvent::Ack with correct identity fields.
//! A2  "pending_new" → BrokerEvent::Ack; same identity contract.
//! A3  "accepted"    → BrokerEvent::Ack; same identity contract.
//! A4  Ack broker_message_id format: "alpaca:{broker_id}:{event}:{ts}".
//! A5  Ack broker_order_id is Some(broker_id), not internal_order_id.
//! A6  Ack internal_order_id is client_order_id, not broker_id.
//! A7  All three Ack strings produce distinct broker_message_ids.
//!
//! ## BRK-04R — CancelAck / CancelReject mapping
//! K1  "canceled"       → BrokerEvent::CancelAck with correct identity fields.
//! K2  "expired"        → BrokerEvent::CancelAck (treated as cancel).
//! K3  "cancel_rejected"→ BrokerEvent::CancelReject with correct identity fields.
//! K4  CancelAck and CancelReject are distinct variants (not silently merged).
//! K5  broker_message_id for "canceled" includes the event string.
//!
//! ## BRK-05R — ReplaceAck / ReplaceReject mapping
//! P1  "replaced"       → BrokerEvent::ReplaceAck with new_total_qty from order.qty.
//! P2  "replace_rejected"→ BrokerEvent::ReplaceReject with correct identity fields.
//! P3  ReplaceAck.new_total_qty is taken from order.qty (Alpaca total post-replace).
//! P4  ReplaceAck and ReplaceReject are distinct variants.
//! P5  Cursor from ReplaceAck batch is Live with correct last_message_id.
//!
//! ## BRK-06R — Reject mapping
//! R1  "rejected" → BrokerEvent::Reject with correct identity fields.
//! R2  Reject broker_message_id is deterministic.
//! R3  Reject cursor advances to Live (event is a terminal lifecycle event).
//! R4  Reject is distinct from CancelReject and ReplaceReject.
use mqk_broker_alpaca::{
    build_inbound_batch_from_ws_update, parse_ws_message,
    types::{AlpacaFetchCursor, AlpacaTradeUpdatesResume},
    AlpacaWsMessage,
};
use mqk_execution::BrokerEvent;
use serde_json::json;
// ---------------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------------
const BROKER_ID: &str = "alpaca-broker-uuid-brk0346r";
const CLIENT_ID: &str = "internal-client-brk0346r";
const SYMBOL: &str = "AAPL";
const TS: &str = "2024-06-15T09:30:00.000000Z";
/// Build raw WS bytes for a single trade-update frame.
fn ws_bytes(event: &str, extra_fields: Option<serde_json::Value>) -> Vec<u8> {
    let mut data = json!({
        "event": event,
        "timestamp": TS,
        "order": {
            "id": BROKER_ID,
            "client_order_id": CLIENT_ID,
            "symbol": SYMBOL,
            "side": "buy",
            "qty": "200",
            "filled_qty": "0"
        }
    });
    if let Some(fields) = extra_fields {
        if let (Some(obj), Some(extra_obj)) = (data.as_object_mut(), fields.as_object()) {
            for (k, v) in extra_obj {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    serde_json::to_vec(&json!([{"T": "trade_updates", "data": data}])).unwrap()
}
/// Parse a single TradeUpdate from a WS frame and return the BrokerEvent.
fn ingest(event: &str, extra: Option<serde_json::Value>) -> BrokerEvent {
    ingest_batch(event, extra).0
}
/// Parse a WS frame and return the InboundBatch (caller owns it for cursor checks).
fn ingest_batch(event: &str, extra: Option<serde_json::Value>) -> (BrokerEvent, AlpacaFetchCursor) {
    let raw = ws_bytes(event, extra);
    let msgs = parse_ws_message(&raw).expect("parse failed");
    let tu = match msgs.into_iter().next().unwrap() {
        AlpacaWsMessage::TradeUpdate(tu) => tu,
        other => panic!("expected TradeUpdate, got {other:?}"),
    };
    let prev = AlpacaFetchCursor::cold_start_unproven(None);
    let batch = build_inbound_batch_from_ws_update(&prev, tu).expect("build failed");
    assert_eq!(batch.events.len(), 1);
    let ev = batch.events[0].clone();
    let cursor = batch.into_cursor_for_persist();
    (ev, cursor)
}
// ---------------------------------------------------------------------------
// BRK-03R — Ack mapping
// ---------------------------------------------------------------------------
/// A1: "new" → BrokerEvent::Ack with correct identity fields.
#[test]
fn brk03r_a1_new_produces_ack_with_correct_identity() {
    let ev = ingest("new", None);
    match ev {
        BrokerEvent::Ack {
            broker_message_id,
            internal_order_id,
            broker_order_id,
        } => {
            assert_eq!(
                broker_message_id,
                format!("alpaca:{BROKER_ID}:new:{TS}"),
                "A1: broker_message_id must follow deterministic format"
            );
            assert_eq!(
                internal_order_id, CLIENT_ID,
                "A1: internal_order_id is client_order_id"
            );
            assert_eq!(
                broker_order_id.as_deref(),
                Some(BROKER_ID),
                "A1: broker_order_id is Alpaca's order.id"
            );
        }
        other => panic!("A1: expected Ack, got {other:?}"),
    }
}
/// A2: "pending_new" → BrokerEvent::Ack with correct identity fields.
#[test]
fn brk03r_a2_pending_new_produces_ack() {
    let ev = ingest("pending_new", None);
    assert!(
        matches!(ev, BrokerEvent::Ack { .. }),
        "A2: pending_new must produce Ack"
    );
    assert_eq!(
        ev.broker_message_id(),
        format!("alpaca:{BROKER_ID}:pending_new:{TS}")
    );
    assert_eq!(ev.internal_order_id(), CLIENT_ID);
    assert_eq!(ev.broker_order_id(), Some(BROKER_ID));
}
/// A3: "accepted" → BrokerEvent::Ack with correct identity fields.
#[test]
fn brk03r_a3_accepted_produces_ack() {
    let ev = ingest("accepted", None);
    assert!(
        matches!(ev, BrokerEvent::Ack { .. }),
        "A3: accepted must produce Ack"
    );
    assert_eq!(
        ev.broker_message_id(),
        format!("alpaca:{BROKER_ID}:accepted:{TS}")
    );
}
/// A4: broker_message_id format is "alpaca:{broker_id}:{event}:{timestamp}".
#[test]
fn brk03r_a4_ack_broker_message_id_format_is_canonical() {
    for event in ["new", "pending_new", "accepted"] {
        let ev = ingest(event, None);
        let expected_mid = format!("alpaca:{BROKER_ID}:{event}:{TS}");
        assert_eq!(
            ev.broker_message_id(),
            expected_mid,
            "A4: broker_message_id must be canonical for event={event}"
        );
    }
}
/// A5: broker_order_id is Some(broker_id), never None, never the client id.
#[test]
fn brk03r_a5_ack_broker_order_id_is_alpaca_order_id_not_client_id() {
    let ev = ingest("new", None);
    assert_eq!(
        ev.broker_order_id(),
        Some(BROKER_ID),
        "A5: broker_order_id must be Alpaca's order.id"
    );
    assert_ne!(
        ev.broker_order_id(),
        Some(CLIENT_ID),
        "A5: broker_order_id must not be client_order_id"
    );
}
/// A6: internal_order_id is client_order_id, not broker_id.
#[test]
fn brk03r_a6_ack_internal_order_id_is_client_order_id() {
    let ev = ingest("new", None);
    assert_eq!(
        ev.internal_order_id(),
        CLIENT_ID,
        "A6: internal_order_id is client_order_id"
    );
    assert_ne!(
        ev.internal_order_id(),
        BROKER_ID,
        "A6: internal_order_id must not be broker order id"
    );
}
/// A7: All three Ack strings produce distinct broker_message_ids
/// (event-type component ensures uniqueness even on the same order/timestamp).
#[test]
fn brk03r_a7_all_ack_strings_produce_distinct_message_ids() {
    let new_id = ingest("new", None).broker_message_id().to_string();
    let pnew_id = ingest("pending_new", None).broker_message_id().to_string();
    let acc_id = ingest("accepted", None).broker_message_id().to_string();
    assert_ne!(
        new_id, pnew_id,
        "A7: new and pending_new must have distinct IDs"
    );
    assert_ne!(
        pnew_id, acc_id,
        "A7: pending_new and accepted must have distinct IDs"
    );
    assert_ne!(
        new_id, acc_id,
        "A7: new and accepted must have distinct IDs"
    );
}
// ---------------------------------------------------------------------------
// BRK-04R — CancelAck / CancelReject mapping
// ---------------------------------------------------------------------------
/// K1: "canceled" → BrokerEvent::CancelAck with correct identity fields.
#[test]
fn brk04r_k1_canceled_produces_cancel_ack_with_correct_identity() {
    let ev = ingest("canceled", None);
    match &ev {
        BrokerEvent::CancelAck {
            broker_message_id,
            internal_order_id,
            broker_order_id,
        } => {
            assert_eq!(
                broker_message_id,
                &format!("alpaca:{BROKER_ID}:canceled:{TS}")
            );
            assert_eq!(internal_order_id, CLIENT_ID);
            assert_eq!(broker_order_id.as_deref(), Some(BROKER_ID));
        }
        other => panic!("K1: expected CancelAck, got {other:?}"),
    }
}
/// K2: "expired" → BrokerEvent::CancelAck (treated as cancel/expired).
#[test]
fn brk04r_k2_expired_produces_cancel_ack() {
    let ev = ingest("expired", None);
    assert!(
        matches!(ev, BrokerEvent::CancelAck { .. }),
        "K2: expired must produce CancelAck"
    );
    assert_eq!(
        ev.broker_message_id(),
        format!("alpaca:{BROKER_ID}:expired:{TS}")
    );
}
/// K3: "cancel_rejected" → BrokerEvent::CancelReject with correct identity fields.
#[test]
fn brk04r_k3_cancel_rejected_produces_cancel_reject_with_correct_identity() {
    let ev = ingest("cancel_rejected", None);
    match &ev {
        BrokerEvent::CancelReject {
            broker_message_id,
            internal_order_id,
            broker_order_id,
        } => {
            assert_eq!(
                broker_message_id,
                &format!("alpaca:{BROKER_ID}:cancel_rejected:{TS}")
            );
            assert_eq!(internal_order_id, CLIENT_ID);
            assert_eq!(broker_order_id.as_deref(), Some(BROKER_ID));
        }
        other => panic!("K3: expected CancelReject, got {other:?}"),
    }
}
/// K4: CancelAck and CancelReject are distinct variants.
#[test]
fn brk04r_k4_cancel_ack_and_cancel_reject_are_distinct_variants() {
    let ack = ingest("canceled", None);
    let reject = ingest("cancel_rejected", None);
    assert!(
        matches!(ack, BrokerEvent::CancelAck { .. }),
        "K4: canceled → CancelAck"
    );
    assert!(
        matches!(reject, BrokerEvent::CancelReject { .. }),
        "K4: cancel_rejected → CancelReject"
    );
}
/// K5: broker_message_id for "canceled" and "expired" include their event string.
#[test]
fn brk04r_k5_cancel_message_ids_include_event_string() {
    let canceled_id = ingest("canceled", None).broker_message_id().to_string();
    let expired_id = ingest("expired", None).broker_message_id().to_string();
    assert!(
        canceled_id.contains(":canceled:"),
        "K5: canceled ID must contain ':canceled:'"
    );
    assert!(
        expired_id.contains(":expired:"),
        "K5: expired ID must contain ':expired:'"
    );
    // They are distinct even on the same order/timestamp.
    assert_ne!(
        canceled_id, expired_id,
        "K5: canceled and expired must have distinct IDs"
    );
}
// ---------------------------------------------------------------------------
// BRK-05R — ReplaceAck / ReplaceReject mapping
// ---------------------------------------------------------------------------
/// P1: "replaced" → BrokerEvent::ReplaceAck with new_total_qty from order.qty.
#[test]
fn brk05r_p1_replaced_produces_replace_ack_with_new_total_qty() {
    // order.qty = 200 (set in ws_bytes via the fixture); replaced event uses this as new total.
    let ev = ingest("replaced", None);
    match &ev {
        BrokerEvent::ReplaceAck {
            broker_message_id,
            internal_order_id,
            broker_order_id,
            new_total_qty,
        } => {
            assert_eq!(
                broker_message_id,
                &format!("alpaca:{BROKER_ID}:replaced:{TS}")
            );
            assert_eq!(internal_order_id, CLIENT_ID);
            assert_eq!(broker_order_id.as_deref(), Some(BROKER_ID));
            assert_eq!(
                *new_total_qty, 200,
                "P1: new_total_qty comes from order.qty"
            );
        }
        other => panic!("P1: expected ReplaceAck, got {other:?}"),
    }
}
/// P2: "replace_rejected" → BrokerEvent::ReplaceReject with correct identity fields.
#[test]
fn brk05r_p2_replace_rejected_produces_replace_reject() {
    let ev = ingest("replace_rejected", None);
    match &ev {
        BrokerEvent::ReplaceReject {
            broker_message_id,
            internal_order_id,
            broker_order_id,
        } => {
            assert_eq!(
                broker_message_id,
                &format!("alpaca:{BROKER_ID}:replace_rejected:{TS}")
            );
            assert_eq!(internal_order_id, CLIENT_ID);
            assert_eq!(broker_order_id.as_deref(), Some(BROKER_ID));
        }
        other => panic!("P2: expected ReplaceReject, got {other:?}"),
    }
}
/// P3: new_total_qty in ReplaceAck is the authoritative post-replace total
/// from order.qty, not the open-leaves qty or any synthetic value.
#[test]
fn brk05r_p3_replace_ack_new_total_qty_from_order_qty_not_filled_qty() {
    // Build a "replaced" event where the order has a non-trivial new total.
    // order.qty = 200, filled_qty = 0 → new_total_qty must be 200.
    let ev = ingest("replaced", None);
    if let BrokerEvent::ReplaceAck { new_total_qty, .. } = ev {
        assert_eq!(new_total_qty, 200);
    } else {
        panic!("P3: expected ReplaceAck");
    }
}
/// P4: ReplaceAck and ReplaceReject are distinct variants, not merged.
#[test]
fn brk05r_p4_replace_ack_and_replace_reject_are_distinct_variants() {
    let ack = ingest("replaced", None);
    let reject = ingest("replace_rejected", None);
    assert!(
        matches!(ack, BrokerEvent::ReplaceAck { .. }),
        "P4: replaced → ReplaceAck"
    );
    assert!(
        matches!(reject, BrokerEvent::ReplaceReject { .. }),
        "P4: replace_rejected → ReplaceReject"
    );
}
/// P5: Cursor from a ReplaceAck batch is Live with the correct last_message_id.
#[test]
fn brk05r_p5_replace_ack_cursor_is_live_with_correct_message_id() {
    let (ev, cursor) = ingest_batch("replaced", None);
    assert!(matches!(ev, BrokerEvent::ReplaceAck { .. }));
    match &cursor.trade_updates {
        AlpacaTradeUpdatesResume::Live {
            last_message_id, ..
        } => {
            assert_eq!(
                last_message_id,
                &format!("alpaca:{BROKER_ID}:replaced:{TS}"),
                "P5: cursor last_message_id must match the replace event's message_id"
            );
        }
        other => panic!("P5: expected Live cursor, got {other:?}"),
    }
}
// ---------------------------------------------------------------------------
// BRK-06R — Reject mapping
// ---------------------------------------------------------------------------
/// R1: "rejected" → BrokerEvent::Reject with correct identity fields.
#[test]
fn brk06r_r1_rejected_produces_reject_with_correct_identity() {
    let ev = ingest("rejected", None);
    match &ev {
        BrokerEvent::Reject {
            broker_message_id,
            internal_order_id,
            broker_order_id,
        } => {
            assert_eq!(
                broker_message_id,
                &format!("alpaca:{BROKER_ID}:rejected:{TS}")
            );
            assert_eq!(internal_order_id, CLIENT_ID);
            assert_eq!(broker_order_id.as_deref(), Some(BROKER_ID));
        }
        other => panic!("R1: expected Reject, got {other:?}"),
    }
}
/// R2: Reject broker_message_id is deterministic: same input → same ID.
#[test]
fn brk06r_r2_reject_broker_message_id_is_deterministic() {
    let ev1 = ingest("rejected", None);
    let ev2 = ingest("rejected", None);
    assert_eq!(
        ev1.broker_message_id(),
        ev2.broker_message_id(),
        "R2: same input must produce identical broker_message_id"
    );
    assert_eq!(
        ev1.broker_message_id(),
        format!("alpaca:{BROKER_ID}:rejected:{TS}")
    );
}
/// R3: Reject cursor advances to Live (terminal event, WS cursor still advances).
#[test]
fn brk06r_r3_reject_cursor_advances_to_live() {
    let (ev, cursor) = ingest_batch("rejected", None);
    assert!(
        matches!(ev, BrokerEvent::Reject { .. }),
        "R3: rejected → Reject"
    );
    assert!(
        matches!(cursor.trade_updates, AlpacaTradeUpdatesResume::Live { .. }),
        "R3: cursor must advance to Live after Reject event"
    );
}
/// R4: Reject is distinct from CancelReject and ReplaceReject.
#[test]
fn brk06r_r4_reject_is_distinct_from_cancel_reject_and_replace_reject() {
    let reject = ingest("rejected", None);
    let cancel_reject = ingest("cancel_rejected", None);
    let replace_reject = ingest("replace_rejected", None);
    assert!(
        matches!(reject, BrokerEvent::Reject { .. }),
        "R4: rejected → Reject"
    );
    assert!(
        matches!(cancel_reject, BrokerEvent::CancelReject { .. }),
        "R4: cancel_rejected → CancelReject"
    );
    assert!(
        matches!(replace_reject, BrokerEvent::ReplaceReject { .. }),
        "R4: replace_rejected → ReplaceReject"
    );
    // All three have distinct broker_message_ids.
    assert_ne!(
        reject.broker_message_id(),
        cancel_reject.broker_message_id()
    );
    assert_ne!(
        reject.broker_message_id(),
        replace_reject.broker_message_id()
    );
    assert_ne!(
        cancel_reject.broker_message_id(),
        replace_reject.broker_message_id()
    );
}
// ---------------------------------------------------------------------------
// Cross-patch: all 11 Alpaca event strings through the WS path
// ---------------------------------------------------------------------------
/// XP1: All 11 known Alpaca event strings flow through parse_ws_message →
/// build_inbound_batch_from_ws_update without error.
#[test]
fn cross_patch_xp1_all_11_event_strings_normalize_through_ws_path() {
    let fill_extra = Some(json!({"price": "150.50", "qty": "100"}));
    let cases: &[(&str, Option<serde_json::Value>)] = &[
        ("new", None),
        ("pending_new", None),
        ("accepted", None),
        ("partial_fill", fill_extra.clone()),
        ("fill", fill_extra),
        ("canceled", None),
        ("expired", None),
        ("cancel_rejected", None),
        ("replaced", None),
        ("replace_rejected", None),
        ("rejected", None),
    ];
    for (event, extra) in cases {
        let raw = ws_bytes(event, extra.clone());
        let msgs = parse_ws_message(&raw).expect("parse failed");
        let tu = match msgs.into_iter().next().unwrap() {
            AlpacaWsMessage::TradeUpdate(tu) => tu,
            other => panic!("XP1: {event}: expected TradeUpdate, got {other:?}"),
        };
        let prev = AlpacaFetchCursor::cold_start_unproven(None);
        let result = build_inbound_batch_from_ws_update(&prev, tu);
        assert!(
            result.is_ok(),
            "XP1: {event} must normalize through WS path without error"
        );
        let batch = result.unwrap();
        assert_eq!(
            batch.events.len(),
            1,
            "XP1: {event} must produce exactly one event"
        );
        // Cursor must advance to Live.
        let cursor = batch.into_cursor_for_persist();
        assert!(
            matches!(cursor.trade_updates, AlpacaTradeUpdatesResume::Live { .. }),
            "XP1: {event} must produce a Live cursor"
        );
    }
}
/// XP2: All 8 canonical BrokerEvent variants are produced by the 11 event strings.
#[test]
fn cross_patch_xp2_all_8_canonical_variants_produced_by_ws_path() {
    let fill_extra = Some(json!({"price": "150.50", "qty": "100"}));
    let expected_variants: &[(&str, Option<serde_json::Value>, &str)] = &[
        ("new", None, "Ack"),
        ("partial_fill", fill_extra.clone(), "PartialFill"),
        ("fill", fill_extra, "Fill"),
        ("canceled", None, "CancelAck"),
        ("cancel_rejected", None, "CancelReject"),
        ("replaced", None, "ReplaceAck"),
        ("replace_rejected", None, "ReplaceReject"),
        ("rejected", None, "Reject"),
    ];
    let mut seen_variants = std::collections::HashSet::new();
    for (event, extra, variant_name) in expected_variants {
        let ev = ingest(event, extra.clone());
        let got = match &ev {
            BrokerEvent::Ack { .. } => "Ack",
            BrokerEvent::PartialFill { .. } => "PartialFill",
            BrokerEvent::Fill { .. } => "Fill",
            BrokerEvent::CancelAck { .. } => "CancelAck",
            BrokerEvent::CancelReject { .. } => "CancelReject",
            BrokerEvent::ReplaceAck { .. } => "ReplaceAck",
            BrokerEvent::ReplaceReject { .. } => "ReplaceReject",
            BrokerEvent::Reject { .. } => "Reject",
        };
        assert_eq!(
            got, *variant_name,
            "XP2: event={event} expected {variant_name}, got {got}"
        );
        seen_variants.insert(*variant_name);
    }
    assert_eq!(
        seen_variants.len(),
        8,
        "XP2: all 8 canonical BrokerEvent variants must be reachable through the WS path"
    );
}
