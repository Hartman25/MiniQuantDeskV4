//! Contract tests for the Alpaca normalization layer — Patch A5.
//!
//! # Coverage
//!
//! C1  Every canonical Alpaca event type produces the correct BrokerEvent variant.
//! C2  broker_order_id is always Some (never None) and equals order.id, not
//!     client_order_id — proves the non-identity requirement.
//! C3  broker_message_id is deterministic: "alpaca:{order.id}:{event}:{timestamp}".
//! C4  internal_order_id comes from client_order_id, not from order.id.
//! C5  PartialFill and Fill carry correct delta_qty, price_micros, fee_micros=0.
//! C6  ReplaceAck.new_total_qty comes from order.qty (Alpaca total post-replace).
//! C7  Unknown event type returns NormalizeError::UnknownEventType.
//! C8  Missing qty or price on fill events returns NormalizeError::MissingField.
//! C9  Invalid price string returns NormalizeError::InvalidPrice.
//! C10 Unknown side string returns NormalizeError::UnknownSide.
//!
//! All tests are pure in-memory — no network, no DB, no wall-clock reads.

use mqk_broker_alpaca::{
    normalize::{normalize_trade_update, NormalizeError},
    types::{AlpacaOrder, AlpacaTradeUpdate},
};
use mqk_execution::{BrokerEvent, Side};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Broker-assigned order UUID — must differ from CLIENT_ID to prove non-identity.
const BROKER_ID: &str = "alpaca-broker-uuid-abc123";
/// Internal / client order ID set by us at submit time.
const CLIENT_ID: &str = "internal-client-ord-xyz789";
const SYMBOL: &str = "AAPL";
const TS: &str = "2024-06-15T09:30:00.000000Z";

fn order(id: &str, client_id: &str, symbol: &str, side: &str, qty: &str) -> AlpacaOrder {
    AlpacaOrder {
        id: id.to_string(),
        client_order_id: client_id.to_string(),
        symbol: symbol.to_string(),
        side: side.to_string(),
        qty: qty.to_string(),
        filled_qty: "0".to_string(),
    }
}

fn update(
    event: &str,
    ord: AlpacaOrder,
    price: Option<&str>,
    qty: Option<&str>,
) -> AlpacaTradeUpdate {
    AlpacaTradeUpdate {
        event: event.to_string(),
        timestamp: TS.to_string(),
        order: ord,
        price: price.map(str::to_string),
        qty: qty.map(str::to_string),
        broker_fill_id: None,
    }
}

/// Minimal valid order using the shared test constants.
fn default_order(side: &str, qty: &str) -> AlpacaOrder {
    order(BROKER_ID, CLIENT_ID, SYMBOL, side, qty)
}

// ---------------------------------------------------------------------------
// C1 — Every canonical event type produces the correct BrokerEvent variant
// ---------------------------------------------------------------------------

#[test]
fn c1_new_produces_ack() {
    let u = update("new", default_order("buy", "100"), None, None);
    assert!(matches!(
        normalize_trade_update(&u).unwrap(),
        BrokerEvent::Ack { .. }
    ));
}

#[test]
fn c1_pending_new_produces_ack() {
    let u = update("pending_new", default_order("buy", "100"), None, None);
    assert!(matches!(
        normalize_trade_update(&u).unwrap(),
        BrokerEvent::Ack { .. }
    ));
}

#[test]
fn c1_accepted_produces_ack() {
    let u = update("accepted", default_order("buy", "100"), None, None);
    assert!(matches!(
        normalize_trade_update(&u).unwrap(),
        BrokerEvent::Ack { .. }
    ));
}

#[test]
fn c1_partial_fill_produces_partial_fill() {
    let u = update(
        "partial_fill",
        default_order("buy", "100"),
        Some("150.00"),
        Some("40"),
    );
    assert!(matches!(
        normalize_trade_update(&u).unwrap(),
        BrokerEvent::PartialFill { .. }
    ));
}

#[test]
fn c1_fill_produces_fill() {
    let u = update(
        "fill",
        default_order("buy", "100"),
        Some("150.00"),
        Some("60"),
    );
    assert!(matches!(
        normalize_trade_update(&u).unwrap(),
        BrokerEvent::Fill { .. }
    ));
}

#[test]
fn c1_canceled_produces_cancel_ack() {
    let u = update("canceled", default_order("sell", "50"), None, None);
    assert!(matches!(
        normalize_trade_update(&u).unwrap(),
        BrokerEvent::CancelAck { .. }
    ));
}

#[test]
fn c1_expired_produces_cancel_ack() {
    let u = update("expired", default_order("sell", "50"), None, None);
    assert!(matches!(
        normalize_trade_update(&u).unwrap(),
        BrokerEvent::CancelAck { .. }
    ));
}

#[test]
fn c1_cancel_rejected_produces_cancel_reject() {
    let u = update("cancel_rejected", default_order("buy", "100"), None, None);
    assert!(matches!(
        normalize_trade_update(&u).unwrap(),
        BrokerEvent::CancelReject { .. }
    ));
}

#[test]
fn c1_replaced_produces_replace_ack() {
    let u = update("replaced", default_order("buy", "80"), None, None);
    assert!(matches!(
        normalize_trade_update(&u).unwrap(),
        BrokerEvent::ReplaceAck { .. }
    ));
}

#[test]
fn c1_replace_rejected_produces_replace_reject() {
    let u = update("replace_rejected", default_order("buy", "100"), None, None);
    assert!(matches!(
        normalize_trade_update(&u).unwrap(),
        BrokerEvent::ReplaceReject { .. }
    ));
}

#[test]
fn c1_rejected_produces_reject() {
    let u = update("rejected", default_order("buy", "100"), None, None);
    assert!(matches!(
        normalize_trade_update(&u).unwrap(),
        BrokerEvent::Reject { .. }
    ));
}

// ---------------------------------------------------------------------------
// C2 — broker_order_id is always Some, equals order.id, not client_order_id
// ---------------------------------------------------------------------------

#[test]
fn c2_broker_order_id_is_always_some() {
    // Pair: (event_str, optional_price, optional_qty)
    let cases: &[(&str, Option<&str>, Option<&str>)] = &[
        ("new", None, None),
        ("partial_fill", Some("100.00"), Some("10")),
        ("fill", Some("100.00"), Some("10")),
        ("canceled", None, None),
        ("cancel_rejected", None, None),
        ("replaced", None, None),
        ("replace_rejected", None, None),
        ("rejected", None, None),
    ];

    for (event_str, price, qty) in cases {
        let u = update(event_str, default_order("buy", "10"), *price, *qty);
        let ev = normalize_trade_update(&u).unwrap();
        let bid = ev.broker_order_id();
        assert!(
            bid.is_some(),
            "event={event_str}: broker_order_id must be Some, got None"
        );
        // Must equal the Alpaca-assigned order.id
        assert_eq!(
            bid.unwrap(),
            BROKER_ID,
            "event={event_str}: broker_order_id must equal order.id"
        );
        // Must NOT fall back to client_order_id (non-identity guarantee)
        assert_ne!(
            bid.unwrap(),
            CLIENT_ID,
            "event={event_str}: broker_order_id must differ from client_order_id"
        );
    }
}

// ---------------------------------------------------------------------------
// C3 — broker_message_id is deterministic
// ---------------------------------------------------------------------------

#[test]
fn c3_broker_message_id_format_ack() {
    let u = update("new", default_order("buy", "100"), None, None);
    let ev = normalize_trade_update(&u).unwrap();
    let expected = format!("alpaca:{BROKER_ID}:new:{TS}");
    assert_eq!(ev.broker_message_id(), expected);
}

#[test]
fn c3_broker_message_id_differs_by_event_type() {
    let ord = || default_order("buy", "100");
    let ack = normalize_trade_update(&update("new", ord(), None, None)).unwrap();
    let cancel = normalize_trade_update(&update("canceled", ord(), None, None)).unwrap();
    assert_ne!(
        ack.broker_message_id(),
        cancel.broker_message_id(),
        "different event types must produce different broker_message_ids"
    );
}

#[test]
fn c3_broker_message_id_is_reproducible() {
    // Calling normalize twice on identical input must yield the same id.
    let u = update(
        "fill",
        default_order("buy", "100"),
        Some("150.00"),
        Some("100"),
    );
    let ev1 = normalize_trade_update(&u).unwrap();
    let ev2 = normalize_trade_update(&u).unwrap();
    assert_eq!(ev1.broker_message_id(), ev2.broker_message_id());
}

// ---------------------------------------------------------------------------
// C4 — internal_order_id comes from client_order_id, not from order.id
// ---------------------------------------------------------------------------

#[test]
fn c4_internal_order_id_is_client_order_id() {
    let u = update("new", default_order("buy", "100"), None, None);
    let ev = normalize_trade_update(&u).unwrap();
    assert_eq!(
        ev.internal_order_id(),
        CLIENT_ID,
        "internal_order_id must equal client_order_id"
    );
    assert_ne!(
        ev.internal_order_id(),
        BROKER_ID,
        "internal_order_id must not equal the Alpaca broker order id"
    );
}

// ---------------------------------------------------------------------------
// C5 — Fill / PartialFill: delta_qty, price_micros, fee_micros=0, side
// ---------------------------------------------------------------------------

#[test]
fn c5_partial_fill_fields_buy() {
    // 40 shares BUY at $150.50 → price_micros = 150_500_000
    let u = update(
        "partial_fill",
        default_order("buy", "100"),
        Some("150.50"),
        Some("40"),
    );
    let ev = normalize_trade_update(&u).unwrap();
    match ev {
        BrokerEvent::PartialFill {
            delta_qty,
            price_micros,
            fee_micros,
            side,
            ..
        } => {
            assert_eq!(delta_qty, 40);
            assert_eq!(price_micros, 150_500_000);
            assert_eq!(fee_micros, 0, "Alpaca does not carry per-trade fee data");
            assert!(matches!(side, Side::Buy));
        }
        other => panic!("expected PartialFill, got {other:?}"),
    }
}

#[test]
fn c5_fill_fields_sell() {
    // 60 shares SELL at $200.25 → price_micros = 200_250_000
    let u = update(
        "fill",
        default_order("sell", "60"),
        Some("200.25"),
        Some("60"),
    );
    let ev = normalize_trade_update(&u).unwrap();
    match ev {
        BrokerEvent::Fill {
            delta_qty,
            price_micros,
            fee_micros,
            side,
            ..
        } => {
            assert_eq!(delta_qty, 60);
            assert_eq!(price_micros, 200_250_000);
            assert_eq!(fee_micros, 0);
            assert!(matches!(side, Side::Sell));
        }
        other => panic!("expected Fill, got {other:?}"),
    }
}

#[test]
fn c5_decimal_qty_string_rounds_correctly() {
    // Alpaca sometimes returns qty as "40.000000" rather than "40".
    let u = update(
        "partial_fill",
        default_order("buy", "100"),
        Some("100.00"),
        Some("40.000000"),
    );
    let ev = normalize_trade_update(&u).unwrap();
    match ev {
        BrokerEvent::PartialFill { delta_qty, .. } => {
            assert_eq!(delta_qty, 40, "decimal qty string must parse to 40");
        }
        other => panic!("expected PartialFill, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// C6 — ReplaceAck.new_total_qty comes from order.qty
// ---------------------------------------------------------------------------

#[test]
fn c6_replace_ack_new_total_qty_from_order_qty() {
    // After 20 shares filled and a replace to 80-open-leaves, Alpaca echoes
    // order.qty = 100 (filled + new open leaves = 20 + 80).  The normalization
    // layer must pass it through unchanged.
    let u = update("replaced", default_order("buy", "100"), None, None);
    let ev = normalize_trade_update(&u).unwrap();
    match ev {
        BrokerEvent::ReplaceAck { new_total_qty, .. } => {
            assert_eq!(new_total_qty, 100, "new_total_qty must come from order.qty");
        }
        other => panic!("expected ReplaceAck, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// C7 — Unknown event type returns NormalizeError::UnknownEventType
// ---------------------------------------------------------------------------

#[test]
fn c7_unknown_event_types_return_error() {
    for unknown in &[
        "held",
        "done_for_day",
        "calculated",
        "stopped",
        "suspended",
        "pending_cancel",
    ] {
        let u = update(unknown, default_order("buy", "100"), None, None);
        let err = normalize_trade_update(&u).unwrap_err();
        assert!(
            matches!(err, NormalizeError::UnknownEventType(_)),
            "event={unknown}: expected UnknownEventType, got {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// C8 — Missing qty or price on fill events returns MissingField
// ---------------------------------------------------------------------------

#[test]
fn c8_fill_missing_qty() {
    let u = update("fill", default_order("buy", "100"), Some("150.00"), None);
    let err = normalize_trade_update(&u).unwrap_err();
    assert!(
        matches!(err, NormalizeError::MissingField("qty")),
        "expected MissingField(qty), got {err:?}"
    );
}

#[test]
fn c8_fill_missing_price() {
    let u = update("fill", default_order("buy", "100"), None, Some("100"));
    let err = normalize_trade_update(&u).unwrap_err();
    assert!(
        matches!(err, NormalizeError::MissingField("price")),
        "expected MissingField(price), got {err:?}"
    );
}

#[test]
fn c8_partial_fill_missing_qty() {
    let u = update(
        "partial_fill",
        default_order("buy", "100"),
        Some("150.00"),
        None,
    );
    let err = normalize_trade_update(&u).unwrap_err();
    assert!(matches!(err, NormalizeError::MissingField("qty")));
}

#[test]
fn c8_partial_fill_missing_price() {
    let u = update(
        "partial_fill",
        default_order("buy", "100"),
        None,
        Some("40"),
    );
    let err = normalize_trade_update(&u).unwrap_err();
    assert!(matches!(err, NormalizeError::MissingField("price")));
}

// ---------------------------------------------------------------------------
// C9 — Invalid price returns NormalizeError::InvalidPrice
// ---------------------------------------------------------------------------

#[test]
fn c9_invalid_price_string() {
    let u = update(
        "fill",
        default_order("buy", "100"),
        Some("not-a-price"),
        Some("100"),
    );
    let err = normalize_trade_update(&u).unwrap_err();
    assert!(
        matches!(err, NormalizeError::InvalidPrice { .. }),
        "expected InvalidPrice, got {err:?}"
    );
}

#[test]
fn c9_nan_price_is_rejected() {
    let u = update(
        "fill",
        default_order("buy", "100"),
        Some("NaN"),
        Some("100"),
    );
    let err = normalize_trade_update(&u).unwrap_err();
    assert!(matches!(err, NormalizeError::InvalidPrice { .. }));
}

// ---------------------------------------------------------------------------
// C10 — Unknown side returns NormalizeError::UnknownSide
// ---------------------------------------------------------------------------

#[test]
fn c10_unknown_side_on_fill() {
    // "short" is not a valid Alpaca side for normalisation purposes.
    let u = update(
        "fill",
        default_order("short", "100"),
        Some("150.00"),
        Some("100"),
    );
    let err = normalize_trade_update(&u).unwrap_err();
    assert!(
        matches!(err, NormalizeError::UnknownSide(_)),
        "expected UnknownSide, got {err:?}"
    );
}

#[test]
fn c10_empty_side_is_unknown() {
    let u = update(
        "partial_fill",
        default_order("", "100"),
        Some("150.00"),
        Some("10"),
    );
    let err = normalize_trade_update(&u).unwrap_err();
    assert!(matches!(err, NormalizeError::UnknownSide(_)));
}
