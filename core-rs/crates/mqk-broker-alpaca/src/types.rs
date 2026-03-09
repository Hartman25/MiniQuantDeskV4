//! Alpaca v2 trade-update event types.
//!
//! These are the raw shapes deserialized from Alpaca's trade-update stream.
//! Fields are strings because that is how Alpaca returns quantities and prices
//! in JSON (decimal strings rather than numbers).
//!
//! # Design rules
//!
//! - No `Uuid::new_v4()`, no wall-clock reads — timestamps come from the event.
//! - Quantities and prices remain as `String` until the normalization layer
//!   parses them explicitly, so parsing errors are always captured rather than
//!   silently coerced.

use serde::{Deserialize, Serialize};

/// A single order lifecycle event from Alpaca's trade-update stream.
///
/// Known event types:
/// - `"new"` / `"pending_new"` / `"accepted"` → Ack
/// - `"partial_fill"` → PartialFill
/// - `"fill"` → Fill
/// - `"canceled"` / `"expired"` → CancelAck
/// - `"cancel_rejected"` → CancelReject
/// - `"replaced"` → ReplaceAck
/// - `"replace_rejected"` → ReplaceReject
/// - `"rejected"` → Reject
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlpacaTradeUpdate {
    /// Event type string as returned by Alpaca.
    pub event: String,

    /// ISO 8601 timestamp of the event from Alpaca.
    ///
    /// Used as part of the deterministic `broker_message_id` so that
    /// two fill events on the same order have distinct deduplication keys.
    pub timestamp: String,

    /// The order state at the time of the event.
    pub order: AlpacaOrder,

    /// Fill execution price as a decimal string (e.g. `"150.50"`).
    ///
    /// Present for `"partial_fill"` and `"fill"` events; absent for others.
    pub price: Option<String>,

    /// Fill quantity as a decimal string (e.g. `"40"`).
    ///
    /// For `"partial_fill"` / `"fill"`: the quantity executed in this event,
    /// not the cumulative total.
    pub qty: Option<String>,
}

/// Alpaca order fields present in every trade-update message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlpacaOrder {
    /// Alpaca-assigned broker order UUID (the authoritative broker_order_id).
    ///
    /// This is the identifier Alpaca uses internally. It is distinct from
    /// `client_order_id` and is the value the OMS must use for cancel/replace
    /// targeting after an Ack is received.
    pub id: String,

    /// Caller-assigned client order ID.
    ///
    /// Set by us at submit time; maps back to `internal_order_id` in the OMS.
    /// Alpaca echoes it unchanged on every lifecycle event for this order.
    pub client_order_id: String,

    /// Ticker symbol, e.g. `"AAPL"`.
    pub symbol: String,

    /// Order direction: `"buy"` or `"sell"`.
    pub side: String,

    /// Current total order quantity as a decimal string.
    ///
    /// After a replace this reflects the new total (filled + new leaves).
    /// Used by the normalization layer to populate `new_total_qty` in
    /// `BrokerEvent::ReplaceAck`.
    pub qty: String,

    /// Cumulative filled quantity as a decimal string.
    pub filled_qty: String,
}

/// Raw Alpaca order submission request body for `POST /v2/orders`.
#[derive(Debug, Clone, Serialize)]
pub struct AlpacaSubmitBody {
    pub symbol: String,
    /// Total order quantity (whole shares).
    pub qty: String,
    pub side: String,
    /// `"market"` or `"limit"`.
    #[serde(rename = "type")]
    pub order_type: String,
    pub time_in_force: String,
    /// Limit price as a decimal string. Present for limit orders only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_price: Option<String>,
    /// Our internal order ID — Alpaca echoes it back on every event.
    pub client_order_id: String,
}

/// Raw Alpaca order submission response body from `POST /v2/orders`.
#[derive(Debug, Clone, Deserialize)]
pub struct AlpacaSubmitResponse {
    /// Alpaca-assigned broker order UUID.
    pub id: String,
    /// Echoed client_order_id.
    pub client_order_id: String,
}

/// Raw Alpaca replace request body for `PATCH /v2/orders/{order_id}`.
#[derive(Debug, Clone, Serialize)]
pub struct AlpacaReplaceBody {
    /// New total quantity (filled_qty + new open leaves).
    ///
    /// Alpaca interprets this as the new total — not open leaves.
    /// The adapter must add filled_qty to the new-leaves value from
    /// `BrokerReplaceRequest.quantity` before sending this field.
    pub qty: String,
    /// New limit price as a decimal string. Present for limit order replaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_price: Option<String>,
    pub time_in_force: String,
}

/// Raw Alpaca replace response body from `PATCH /v2/orders/{order_id}`.
#[derive(Debug, Clone, Deserialize)]
pub struct AlpacaReplaceResponse {
    /// Alpaca-assigned broker order UUID.
    pub id: String,
    /// New total quantity as echoed by Alpaca.
    pub qty: String,
}
