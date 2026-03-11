//! Normalization: Alpaca raw trade-update events to canonical BrokerEvent.
//!
//! # Design rules
//!
//! - No Uuid::new_v4(). broker_message_id is deterministic from the event
//!   payload: "alpaca:{order.id}:{event}:{timestamp}".
//! - No wall-clock reads. Every timestamp in broker_message_id comes from
//!   the Alpaca event payload itself, not from SystemTime::now().
//! - broker_order_id is always Some(order.id) -- live events always carry
//!   the Alpaca-assigned UUID; None is never produced by this layer.
//! - internal_order_id is order.client_order_id (the ID we set at submit).
//! - Price strings are parsed via mqk_execution::price_to_micros at the
//!   wire boundary only; no f64 crosses the decision surface.
use crate::types::AlpacaTradeUpdate;
use mqk_execution::{price_to_micros, BrokerEvent, Side};
// ---------------------------------------------------------------------------
// NormalizeError
// ---------------------------------------------------------------------------
/// Error returned when an Alpaca trade-update event cannot be normalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizeError {
    /// The event type string is not one the adapter recognizes.
    UnknownEventType(String),
    /// A quantity field could not be parsed as a non-negative integer.
    InvalidQuantity { field: &'static str, raw: String },
    /// A price field could not be parsed or converted to integer micros.
    InvalidPrice { raw: String },
    /// The side field was not "buy" or "sell".
    UnknownSide(String),
    /// A required field was absent.
    MissingField(&'static str),
}
impl std::fmt::Display for NormalizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NormalizeError::UnknownEventType(t) => {
                write!(f, "normalize: unknown Alpaca event type {t:?}")
            }
            NormalizeError::InvalidQuantity { field, raw } => {
                write!(f, "normalize: invalid quantity in {field}: {raw:?}")
            }
            NormalizeError::InvalidPrice { raw } => {
                write!(f, "normalize: invalid price: {raw:?}")
            }
            NormalizeError::UnknownSide(s) => {
                write!(f, "normalize: unknown side: {s:?}")
            }
            NormalizeError::MissingField(name) => {
                write!(f, "normalize: missing required field: {name}")
            }
        }
    }
}
impl std::error::Error for NormalizeError {}
// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------
/// Parse a decimal quantity string to a non-negative i64.
///
/// Accepts both integer ("100") and decimal ("100.0") forms because Alpaca
/// sometimes returns quantities as "100.000000".
fn parse_qty(raw: &str, field: &'static str) -> Result<i64, NormalizeError> {
    let v: f64 = raw.parse().map_err(|_| NormalizeError::InvalidQuantity {
        field,
        raw: raw.to_string(),
    })?;
    if !v.is_finite() || v < 0.0 {
        return Err(NormalizeError::InvalidQuantity {
            field,
            raw: raw.to_string(),
        });
    }
    Ok(v.round() as i64)
}
/// Parse a decimal price string to integer micros.
fn parse_price(raw: &str) -> Result<i64, NormalizeError> {
    let f: f64 = raw.parse().map_err(|_| NormalizeError::InvalidPrice {
        raw: raw.to_string(),
    })?;
    price_to_micros(f).map_err(|_| NormalizeError::InvalidPrice {
        raw: raw.to_string(),
    })
}
/// Parse an Alpaca side string to Side.
fn parse_side(s: &str) -> Result<Side, NormalizeError> {
    match s {
        "buy" => Ok(Side::Buy),
        "sell" => Ok(Side::Sell),
        other => Err(NormalizeError::UnknownSide(other.to_string())),
    }
}
/// Build a deterministic broker_message_id from the event payload.
///
/// Format: "alpaca:{order.id}:{event}:{timestamp}"
fn make_broker_message_id(order_id: &str, event_type: &str, timestamp: &str) -> String {
    format!("alpaca:{order_id}:{event_type}:{timestamp}")
}
/// Build the deterministic deduplication key for an Alpaca trade update.
pub fn trade_update_message_id(ev: &AlpacaTradeUpdate) -> String {
    make_broker_message_id(&ev.order.id, &ev.event, &ev.timestamp)
}
// ---------------------------------------------------------------------------
// normalize_trade_update
// ---------------------------------------------------------------------------
/// Normalize an AlpacaTradeUpdate into a canonical BrokerEvent.
///
/// # Contract (A5)
///
/// - broker_order_id is always Some(order.id) -- never None.
/// - internal_order_id comes from order.client_order_id.
/// - broker_message_id is deterministic; no wall-clock or RNG.
/// - new_total_qty in ReplaceAck is taken from order.qty in the "replaced"
///   event (Alpaca's authoritative total after the amend was accepted).
/// - fee_micros is 0 for fills; Alpaca does not carry per-trade fee data
///   in the trade-update stream.
///
/// # Errors
/// Returns NormalizeError if any required field is missing or unparsable,
/// or if the event type is unrecognized.
pub fn normalize_trade_update(ev: &AlpacaTradeUpdate) -> Result<BrokerEvent, NormalizeError> {
    let broker_order_id = ev.order.id.clone();
    let internal_order_id = ev.order.client_order_id.clone();
    let broker_message_id = trade_update_message_id(ev);
    match ev.event.as_str() {
        // ------------------------------------------------------------------
        // Ack: order acknowledged by broker.
        // Covers "new", "pending_new", "accepted".
        // ------------------------------------------------------------------
        "new" | "pending_new" | "accepted" => Ok(BrokerEvent::Ack {
            broker_message_id,
            internal_order_id,
            broker_order_id: Some(broker_order_id),
        }),
        // ------------------------------------------------------------------
        // PartialFill: broker executed part of the order.
        // ev.qty is the quantity filled in THIS event (not cumulative).
        // ------------------------------------------------------------------
        "partial_fill" => {
            let fill_qty_str = ev
                .qty
                .as_deref()
                .ok_or(NormalizeError::MissingField("qty"))?;
            let price_str = ev
                .price
                .as_deref()
                .ok_or(NormalizeError::MissingField("price"))?;
            Ok(BrokerEvent::PartialFill {
                broker_message_id,
                internal_order_id,
                broker_order_id: Some(broker_order_id),
                symbol: ev.order.symbol.clone(),
                side: parse_side(&ev.order.side)?,
                delta_qty: parse_qty(fill_qty_str, "qty")?,
                price_micros: parse_price(price_str)?,
                fee_micros: 0,
            })
        }
        // ------------------------------------------------------------------
        // Fill: order completely filled. Same fields as PartialFill.
        // ------------------------------------------------------------------
        "fill" => {
            let fill_qty_str = ev
                .qty
                .as_deref()
                .ok_or(NormalizeError::MissingField("qty"))?;
            let price_str = ev
                .price
                .as_deref()
                .ok_or(NormalizeError::MissingField("price"))?;
            Ok(BrokerEvent::Fill {
                broker_message_id,
                internal_order_id,
                broker_order_id: Some(broker_order_id),
                symbol: ev.order.symbol.clone(),
                side: parse_side(&ev.order.side)?,
                delta_qty: parse_qty(fill_qty_str, "qty")?,
                price_micros: parse_price(price_str)?,
                fee_micros: 0,
            })
        }
        // ------------------------------------------------------------------
        // CancelAck: order canceled or expired.
        // ------------------------------------------------------------------
        "canceled" | "expired" => Ok(BrokerEvent::CancelAck {
            broker_message_id,
            internal_order_id,
            broker_order_id: Some(broker_order_id),
        }),
        // ------------------------------------------------------------------
        // CancelReject: cancel request was rejected by the broker.
        // ------------------------------------------------------------------
        "cancel_rejected" => Ok(BrokerEvent::CancelReject {
            broker_message_id,
            internal_order_id,
            broker_order_id: Some(broker_order_id),
        }),
        // ------------------------------------------------------------------
        // ReplaceAck: replace was accepted.
        // order.qty in the "replaced" event is the new authoritative total.
        // ------------------------------------------------------------------
        "replaced" => {
            let new_total_qty = parse_qty(&ev.order.qty, "order.qty")?;
            Ok(BrokerEvent::ReplaceAck {
                broker_message_id,
                internal_order_id,
                broker_order_id: Some(broker_order_id),
                new_total_qty,
            })
        }
        // ------------------------------------------------------------------
        // ReplaceReject: replace request was rejected by the broker.
        // ------------------------------------------------------------------
        "replace_rejected" => Ok(BrokerEvent::ReplaceReject {
            broker_message_id,
            internal_order_id,
            broker_order_id: Some(broker_order_id),
        }),
        // ------------------------------------------------------------------
        // Reject: order outright rejected by broker or exchange.
        // ------------------------------------------------------------------
        "rejected" => Ok(BrokerEvent::Reject {
            broker_message_id,
            internal_order_id,
            broker_order_id: Some(broker_order_id),
        }),
        // ------------------------------------------------------------------
        // Unknown: fail normalization; event is not persisted to OMS.
        // ------------------------------------------------------------------
        other => Err(NormalizeError::UnknownEventType(other.to_string())),
    }
}
// ---------------------------------------------------------------------------
// Unit tests (no network, no DB, no clock)
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AlpacaOrder;
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
            timestamp: "2024-01-01T00:00:00.000000Z".to_string(),
            order: ord,
            price: price.map(str::to_string),
            qty: qty.map(str::to_string),
        }
    }
    #[test]
    fn new_event_produces_ack() {
        let u = update(
            "new",
            order("alpaca-uuid-001", "internal-001", "AAPL", "buy", "100"),
            None,
            None,
        );
        let ev = normalize_trade_update(&u).unwrap();
        assert!(matches!(ev, BrokerEvent::Ack { .. }));
        assert_eq!(ev.broker_order_id(), Some("alpaca-uuid-001"));
        assert_eq!(ev.internal_order_id(), "internal-001");
    }
    #[test]
    fn unknown_event_returns_error() {
        let u = update(
            "held",
            order("alpaca-uuid-x", "internal-x", "AAPL", "buy", "10"),
            None,
            None,
        );
        let err = normalize_trade_update(&u).unwrap_err();
        assert!(matches!(err, NormalizeError::UnknownEventType(_)));
    }
}
