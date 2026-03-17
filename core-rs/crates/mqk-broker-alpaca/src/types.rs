//! Alpaca v2 wire types.
//!
//! # Modules covered
//!
//! - `AlpacaTradeUpdate` / `AlpacaOrder` - raw shapes from the websocket
//!   trade-update stream, used by the normalization layer.
//! - `AlpacaSubmitBody` / `AlpacaSubmitResponse` - POST /v2/orders wire types.
//! - `AlpacaReplaceBody` / `AlpacaReplaceResponse` - PATCH /v2/orders/{id} wire types.
//! - `AlpacaOrderFull` - GET /v2/orders/{id} response (used by replace to get filled_qty).
//! - `AlpacaOrderActivity` - GET /v2/account/activities polling response.
//!
//! # Design rules
//!
//! - No `Uuid::new_v4()`, no wall-clock reads - timestamps come from the event.
//! - Quantities and prices remain as `String` until the normalization layer
//!   parses them explicitly, so parsing errors are always captured rather than
//!   silently coerced.
use serde::{Deserialize, Serialize};
// ---------------------------------------------------------------------------
// Websocket trade-update types (normalization layer input)
// ---------------------------------------------------------------------------
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
    /// Stable broker-native fill identity when Alpaca supplies one.
    ///
    /// This is distinct from `timestamp`/`event` message identity and should
    /// be used for economic fill identity only when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker_fill_id: Option<String>,
}
/// Adapter-owned opaque inbound resume state persisted in `broker_event_cursor`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AlpacaFetchCursor {
    pub schema_version: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rest_activity_after: Option<String>,
    pub trade_updates: AlpacaTradeUpdatesResume,
}
impl AlpacaFetchCursor {
    pub const SCHEMA_VERSION: u8 = 1;
    pub fn cold_start_unproven(rest_activity_after: Option<String>) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            rest_activity_after,
            trade_updates: AlpacaTradeUpdatesResume::ColdStartUnproven,
        }
    }
    pub fn live(
        rest_activity_after: Option<String>,
        last_message_id: impl Into<String>,
        last_event_at: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            rest_activity_after,
            trade_updates: AlpacaTradeUpdatesResume::Live {
                last_message_id: last_message_id.into(),
                last_event_at: last_event_at.into(),
            },
        }
    }
    pub fn gap_detected(
        rest_activity_after: Option<String>,
        last_message_id: Option<String>,
        last_event_at: Option<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            rest_activity_after,
            trade_updates: AlpacaTradeUpdatesResume::GapDetected {
                last_message_id,
                last_event_at,
                detail: detail.into(),
            },
        }
    }
}
/// Persisted websocket continuity state for Alpaca trade updates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AlpacaTradeUpdatesResume {
    ColdStartUnproven,
    Live {
        last_message_id: String,
        last_event_at: String,
    },
    GapDetected {
        last_message_id: Option<String>,
        last_event_at: Option<String>,
        detail: String,
    },
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
// ---------------------------------------------------------------------------
// Submit types - POST /v2/orders
// ---------------------------------------------------------------------------
/// Raw Alpaca order submission request body for `POST /v2/orders`.
#[derive(Debug, Clone, Serialize)]
pub struct AlpacaSubmitBody {
    pub symbol: String,
    /// Total order quantity (whole shares as string).
    pub qty: String,
    pub side: String,
    /// `"market"` or `"limit"`.
    #[serde(rename = "type")]
    pub order_type: String,
    pub time_in_force: String,
    /// Limit price as a decimal string. Present for limit orders only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_price: Option<String>,
    /// Our internal order ID - Alpaca echoes it back on every event.
    pub client_order_id: String,
}
/// Raw Alpaca order submission response body from `POST /v2/orders`.
#[derive(Debug, Clone, Deserialize)]
pub struct AlpacaSubmitResponse {
    /// Alpaca-assigned broker order UUID.
    pub id: String,
    /// Echoed client_order_id.
    pub client_order_id: String,
    /// Order creation timestamp (ISO 8601). Optional because some Alpaca
    /// environments omit it in sandbox responses.
    pub created_at: Option<String>,
}
// ---------------------------------------------------------------------------
// Replace types - PATCH /v2/orders/{order_id}
// ---------------------------------------------------------------------------
/// Raw Alpaca replace request body for `PATCH /v2/orders/{order_id}`.
#[derive(Debug, Clone, Serialize)]
pub struct AlpacaReplaceBody {
    /// New total quantity (filled_qty + new open leaves).
    ///
    /// Alpaca interprets this as the new total - not open leaves.
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
// ---------------------------------------------------------------------------
// Full order - GET /v2/orders/{order_id}
// ---------------------------------------------------------------------------
/// Full Alpaca order object returned by `GET /v2/orders/{id}`.
///
/// Used by `replace_order` to look up the current `filled_qty` before
/// computing the new total quantity for the replace request.
#[derive(Debug, Clone, Deserialize)]
pub struct AlpacaOrderFull {
    /// Alpaca-assigned broker order UUID.
    pub id: String,
    /// Caller-assigned client order ID (maps to internal_order_id in OMS).
    pub client_order_id: String,
    pub symbol: String,
    /// Order direction: `"buy"` or `"sell"`.
    pub side: String,
    /// Current total order quantity as a decimal string.
    pub qty: String,
    /// Cumulative filled quantity as a decimal string.
    pub filled_qty: String,
}
// ---------------------------------------------------------------------------
// Account activities - GET /v2/account/activities (REST polling)
// ---------------------------------------------------------------------------
/// A single account activity record from `GET /v2/account/activities`.
///
/// `fetch_events` uses this polling endpoint as broker ingress input and maps
/// known order-lifecycle activity classes into canonical lifecycle events.
///
/// Supported activity types:
/// - `NEW` / `PENDING_NEW` / `ACCEPTED`
/// - `PARTIAL_FILL` / `FILL`
/// - `CANCELED` / `EXPIRED`
/// - `CANCEL_REJECTED`
/// - `REPLACED` / `REPLACE_REJECTED`
/// - `REJECTED`
///
/// Unknown activity types are rejected by mapping logic (fail-closed).
#[derive(Debug, Clone, Deserialize)]
pub struct AlpacaOrderActivity {
    /// Unique activity ID.
    ///
    /// Used as the cursor for incremental polling.  Alpaca returns activities
    /// ordered by `transaction_time`; the last `id` in a page is passed as
    /// `after` on the next call.
    ///
    /// Format: `"YYYYMMDDHHMMSS{fraction}::{uuid}"`.
    pub id: String,
    /// Activity type from Alpaca, e.g. `"NEW"`, `"FILL"`, `"CANCELED"`, `"DIV"`.
    pub activity_type: String,
    /// Alpaca broker-assigned order UUID.
    pub order_id: String,
    /// ISO 8601 timestamp of the transaction.
    pub transaction_time: String,
    /// Fill price as a decimal string.  Present for `FILL`/`PARTIAL_FILL`.
    pub price: Option<String>,
    /// Fill quantity for this event (delta, not cumulative).
    pub qty: Option<String>,
    /// Order direction: `"buy"` or `"sell"`.
    pub side: String,
    /// Ticker symbol.
    pub symbol: String,
}
// ---------------------------------------------------------------------------
// Snapshot fetch wire types — AP-03
// GET /v2/account, GET /v2/positions, GET /v2/orders?status=open
// ---------------------------------------------------------------------------

/// Raw Alpaca account response from `GET /v2/account`.
///
/// Used by `fetch_broker_snapshot` to populate the `BrokerAccount` field.
#[derive(Debug, Clone, Deserialize)]
pub struct AlpacaAccountRaw {
    /// Account equity as a decimal string (e.g. `"10000.50"`).
    pub equity: String,
    /// Settled cash as a decimal string.
    pub cash: String,
    /// ISO 4217 currency code (e.g. `"USD"`).
    pub currency: String,
}

/// Raw Alpaca position from `GET /v2/positions`.
///
/// Alpaca returns one row per held position. `qty` is positive for long,
/// negative for short.
#[derive(Debug, Clone, Deserialize)]
pub struct AlpacaPositionRaw {
    /// Ticker symbol.
    pub symbol: String,
    /// Signed position quantity as a decimal string.
    pub qty: String,
    /// Average entry price as a decimal string.
    pub avg_entry_price: String,
}

/// Raw Alpaca order from `GET /v2/orders?status=open`.
///
/// The list endpoint returns the full order object. This type is distinct from
/// `AlpacaOrderFull` in that it also includes `status` and `order_type`, which
/// are required for the canonical `BrokerOrder` mapping.
#[derive(Debug, Clone, Deserialize)]
pub struct AlpacaOpenOrderRaw {
    /// Alpaca-assigned broker order UUID.
    pub id: String,
    /// Caller-assigned client order ID.
    pub client_order_id: String,
    /// Ticker symbol.
    pub symbol: String,
    /// Order direction: `"buy"` or `"sell"`.
    pub side: String,
    /// Order type: `"market"`, `"limit"`, `"stop"`, `"stop_limit"`.
    #[serde(rename = "type")]
    pub order_type: String,
    /// Current Alpaca order status (e.g. `"accepted"`, `"partially_filled"`).
    pub status: String,
    /// Total order quantity as a decimal string.
    pub qty: String,
    /// Limit price as a decimal string. `None` for non-limit orders.
    pub limit_price: Option<String>,
    /// Stop price as a decimal string. `None` for non-stop orders.
    pub stop_price: Option<String>,
    /// Order creation timestamp (ISO 8601 / RFC 3339).
    pub created_at: String,
}
