//! Snapshot Adapter — deserialize broker wire format and normalize to internal types.
//!
//! # Purpose
//! Broker REST APIs return order/position data in their own JSON schema.  This
//! module defines the *raw* (wire-level) structs that mirror those responses and
//! provides a single [`normalize`] function that converts them into the internal
//! [`BrokerSnapshot`] / [`OrderSnapshot`] / [`PositionSnapshot`] types consumed
//! by the reconciliation engine.
//!
//! # Design constraints
//! - Pure, deterministic conversion. No IO, no broker calls, no async.
//! - All normalization errors are surfaced as [`SnapshotAdapterError`]; callers
//!   decide whether to HALT or retry.
//! - Field names in the raw structs use `#[serde(rename_all = "snake_case")]` to
//!   match the common Alpaca / generic broker REST conventions.  Adapting to a
//!   different broker requires only adding a new `Raw*` struct + normalization arm.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{BrokerSnapshot, OrderSnapshot, OrderStatus, Side};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// All errors that can occur during snapshot normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotAdapterError {
    /// An `order_id` field was empty or missing.
    MissingOrderId,
    /// A `symbol` field was empty or missing.
    MissingSymbol { order_id: String },
    /// A `side` string could not be mapped to [`Side`].
    UnknownSide { order_id: String, raw: String },
    /// A `status` string could not be mapped to [`OrderStatus`].
    UnknownStatus { order_id: String, raw: String },
    /// `qty` is negative (broker returned a malformed value).
    NegativeQty { order_id: String, qty: i64 },
    /// `filled_qty` is negative.
    NegativeFilledQty { order_id: String, filled_qty: i64 },
    /// `filled_qty` exceeds `qty`.
    FilledExceedsQty {
        order_id: String,
        qty: i64,
        filled_qty: i64,
    },
}

impl std::fmt::Display for SnapshotAdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingOrderId => write!(f, "broker order has empty order_id"),
            Self::MissingSymbol { order_id } => {
                write!(f, "broker order '{order_id}' has empty symbol")
            }
            Self::UnknownSide { order_id, raw } => {
                write!(f, "broker order '{order_id}' has unrecognised side '{raw}'")
            }
            Self::UnknownStatus { order_id, raw } => {
                write!(
                    f,
                    "broker order '{order_id}' has unrecognised status '{raw}'"
                )
            }
            Self::NegativeQty { order_id, qty } => {
                write!(f, "broker order '{order_id}' has negative qty {qty}")
            }
            Self::NegativeFilledQty {
                order_id,
                filled_qty,
            } => {
                write!(
                    f,
                    "broker order '{order_id}' has negative filled_qty {filled_qty}"
                )
            }
            Self::FilledExceedsQty {
                order_id,
                qty,
                filled_qty,
            } => {
                write!(
                    f,
                    "broker order '{order_id}' filled_qty {filled_qty} exceeds qty {qty}"
                )
            }
        }
    }
}

impl std::error::Error for SnapshotAdapterError {}

// ---------------------------------------------------------------------------
// Raw wire-level structs  (broker JSON → these → internal types)
// ---------------------------------------------------------------------------

/// Wire-level order entry from the broker REST API.
///
/// Field names follow common Alpaca / generic REST conventions.
/// Unknown fields are silently ignored (`deny_unknown_fields` is NOT set so
/// that future broker API additions don't break deserialization).
#[derive(Debug, Clone, Deserialize)]
pub struct RawBrokerOrder {
    /// Broker-assigned order identifier (must be non-empty).
    pub order_id: String,
    /// Instrument symbol (e.g. `"AAPL"`).
    pub symbol: String,
    /// Side string: `"buy"` | `"sell"` (case-insensitive).
    pub side: String,
    /// Requested quantity (must be non-negative).
    pub qty: i64,
    /// Filled quantity so far (must be 0 ≤ filled_qty ≤ qty).
    pub filled_qty: i64,
    /// Status string: see [`normalize_status`] for accepted values.
    pub status: String,
}

/// Wire-level position entry from the broker REST API.
#[derive(Debug, Clone, Deserialize)]
pub struct RawBrokerPosition {
    /// Instrument symbol.
    pub symbol: String,
    /// Signed quantity: positive = long, negative = short.
    pub qty_signed: i64,
}

/// Top-level broker snapshot as returned by the broker REST API.
///
/// Callers construct this from `serde_json::from_str` / `serde_json::from_value`
/// after fetching the broker endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct RawBrokerSnapshot {
    /// All open/working (and optionally recent) orders visible at the broker.
    pub orders: Vec<RawBrokerOrder>,
    /// Current positions visible at the broker.
    pub positions: Vec<RawBrokerPosition>,
}

// ---------------------------------------------------------------------------
// Normalization helpers
// ---------------------------------------------------------------------------

fn normalize_side(order_id: &str, raw: &str) -> Result<Side, SnapshotAdapterError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "buy" | "b" => Ok(Side::Buy),
        "sell" | "s" | "short" => Ok(Side::Sell),
        other => Err(SnapshotAdapterError::UnknownSide {
            order_id: order_id.to_string(),
            raw: other.to_string(),
        }),
    }
}

fn normalize_status(order_id: &str, raw: &str) -> Result<OrderStatus, SnapshotAdapterError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "new" | "open" | "pending_new" => Ok(OrderStatus::New),
        "accepted" | "pending" | "accepted_for_bidding" => Ok(OrderStatus::Accepted),
        "partially_filled" | "partial" => Ok(OrderStatus::PartiallyFilled),
        "filled" | "complete" | "completed" => Ok(OrderStatus::Filled),
        "canceled" | "cancelled" | "done_for_day" | "expired" | "replaced" => {
            Ok(OrderStatus::Canceled)
        }
        "rejected" | "stopped" | "suspended" => Ok(OrderStatus::Rejected),
        _ => Err(SnapshotAdapterError::UnknownStatus {
            order_id: order_id.to_string(),
            raw: raw.to_string(),
        }),
    }
}

fn normalize_order(raw: RawBrokerOrder) -> Result<OrderSnapshot, SnapshotAdapterError> {
    if raw.order_id.trim().is_empty() {
        return Err(SnapshotAdapterError::MissingOrderId);
    }
    let order_id = raw.order_id.trim().to_string();

    if raw.symbol.trim().is_empty() {
        return Err(SnapshotAdapterError::MissingSymbol { order_id });
    }
    let symbol = raw.symbol.trim().to_string();

    let side = normalize_side(&order_id, &raw.side)?;
    let status = normalize_status(&order_id, &raw.status)?;

    if raw.qty < 0 {
        return Err(SnapshotAdapterError::NegativeQty {
            order_id,
            qty: raw.qty,
        });
    }
    if raw.filled_qty < 0 {
        return Err(SnapshotAdapterError::NegativeFilledQty {
            order_id,
            filled_qty: raw.filled_qty,
        });
    }
    if raw.filled_qty > raw.qty {
        return Err(SnapshotAdapterError::FilledExceedsQty {
            order_id,
            qty: raw.qty,
            filled_qty: raw.filled_qty,
        });
    }

    Ok(OrderSnapshot {
        order_id,
        symbol,
        side,
        qty: raw.qty,
        filled_qty: raw.filled_qty,
        status,
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Normalize a raw broker snapshot into the internal [`BrokerSnapshot`] type.
///
/// # Errors
/// Returns the first [`SnapshotAdapterError`] encountered.  Orders are
/// processed in input order; the caller must decide whether to HALT on error
/// or discard the malformed entry.
///
/// For a lenient variant that skips invalid orders instead of failing, see
/// [`normalize_lenient`].
pub fn normalize(raw: RawBrokerSnapshot) -> Result<BrokerSnapshot, SnapshotAdapterError> {
    let mut orders: BTreeMap<String, OrderSnapshot> = BTreeMap::new();
    for raw_order in raw.orders {
        let snap = normalize_order(raw_order)?;
        orders.insert(snap.order_id.clone(), snap);
    }

    let mut positions: BTreeMap<String, i64> = BTreeMap::new();
    for pos in raw.positions {
        let sym = pos.symbol.trim().to_string();
        if !sym.is_empty() {
            positions.insert(sym, pos.qty_signed);
        }
    }

    Ok(BrokerSnapshot { orders, positions })
}

/// Lenient variant: skip malformed orders rather than failing.
///
/// Use this when you want best-effort normalization and prefer to surface
/// warnings rather than hard errors.  The caller receives both the partial
/// snapshot and a list of errors for any skipped orders.
pub fn normalize_lenient(raw: RawBrokerSnapshot) -> (BrokerSnapshot, Vec<SnapshotAdapterError>) {
    let mut orders: BTreeMap<String, OrderSnapshot> = BTreeMap::new();
    let mut errors: Vec<SnapshotAdapterError> = Vec::new();

    for raw_order in raw.orders {
        match normalize_order(raw_order) {
            Ok(snap) => {
                orders.insert(snap.order_id.clone(), snap);
            }
            Err(e) => errors.push(e),
        }
    }

    let mut positions: BTreeMap<String, i64> = BTreeMap::new();
    for pos in raw.positions {
        let sym = pos.symbol.trim().to_string();
        if !sym.is_empty() {
            positions.insert(sym, pos.qty_signed);
        }
    }

    (BrokerSnapshot { orders, positions }, errors)
}

/// Deserialize a JSON string directly into a [`BrokerSnapshot`].
///
/// Convenience wrapper: `json_str → RawBrokerSnapshot → BrokerSnapshot`.
/// Returns a boxed error so callers don't need to import serde_json directly.
pub fn normalize_json(json: &str) -> Result<BrokerSnapshot, Box<dyn std::error::Error>> {
    let raw: RawBrokerSnapshot = serde_json::from_str(json)?;
    let snap = normalize(raw)?;
    Ok(snap)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_raw_order(
        order_id: &str,
        symbol: &str,
        side: &str,
        qty: i64,
        filled_qty: i64,
        status: &str,
    ) -> RawBrokerOrder {
        RawBrokerOrder {
            order_id: order_id.to_string(),
            symbol: symbol.to_string(),
            side: side.to_string(),
            qty,
            filled_qty,
            status: status.to_string(),
        }
    }

    // --- Side normalization ---

    #[test]
    fn side_buy_variants() {
        assert_eq!(normalize_side("o1", "buy"), Ok(Side::Buy));
        assert_eq!(normalize_side("o1", "BUY"), Ok(Side::Buy));
        assert_eq!(normalize_side("o1", "b"), Ok(Side::Buy));
    }

    #[test]
    fn side_sell_variants() {
        assert_eq!(normalize_side("o1", "sell"), Ok(Side::Sell));
        assert_eq!(normalize_side("o1", "SELL"), Ok(Side::Sell));
        assert_eq!(normalize_side("o1", "short"), Ok(Side::Sell));
        assert_eq!(normalize_side("o1", "s"), Ok(Side::Sell));
    }

    #[test]
    fn side_unknown_errors() {
        let err = normalize_side("o1", "long");
        assert!(matches!(err, Err(SnapshotAdapterError::UnknownSide { .. })));
    }

    // --- Status normalization ---

    #[test]
    fn status_new_variants() {
        assert_eq!(normalize_status("o1", "new"), Ok(OrderStatus::New));
        assert_eq!(normalize_status("o1", "open"), Ok(OrderStatus::New));
        assert_eq!(normalize_status("o1", "pending_new"), Ok(OrderStatus::New));
    }

    #[test]
    fn status_filled_variants() {
        assert_eq!(normalize_status("o1", "filled"), Ok(OrderStatus::Filled));
        assert_eq!(normalize_status("o1", "complete"), Ok(OrderStatus::Filled));
        assert_eq!(normalize_status("o1", "COMPLETED"), Ok(OrderStatus::Filled));
    }

    #[test]
    fn status_canceled_variants() {
        assert_eq!(
            normalize_status("o1", "canceled"),
            Ok(OrderStatus::Canceled)
        );
        assert_eq!(
            normalize_status("o1", "cancelled"),
            Ok(OrderStatus::Canceled)
        );
        assert_eq!(normalize_status("o1", "expired"), Ok(OrderStatus::Canceled));
    }

    #[test]
    fn status_unknown_errors() {
        let err = normalize_status("o1", "warp_speed");
        assert!(matches!(
            err,
            Err(SnapshotAdapterError::UnknownStatus { .. })
        ));
    }

    // --- Order validation ---

    #[test]
    fn empty_order_id_errors() {
        let raw = make_raw_order("", "AAPL", "buy", 100, 0, "new");
        assert_eq!(
            normalize_order(raw),
            Err(SnapshotAdapterError::MissingOrderId)
        );
    }

    #[test]
    fn empty_symbol_errors() {
        let raw = make_raw_order("o1", "", "buy", 100, 0, "new");
        assert!(matches!(
            normalize_order(raw),
            Err(SnapshotAdapterError::MissingSymbol { .. })
        ));
    }

    #[test]
    fn negative_qty_errors() {
        let raw = make_raw_order("o1", "AAPL", "buy", -1, 0, "new");
        assert!(matches!(
            normalize_order(raw),
            Err(SnapshotAdapterError::NegativeQty { .. })
        ));
    }

    #[test]
    fn negative_filled_qty_errors() {
        let raw = make_raw_order("o1", "AAPL", "buy", 100, -1, "new");
        assert!(matches!(
            normalize_order(raw),
            Err(SnapshotAdapterError::NegativeFilledQty { .. })
        ));
    }

    #[test]
    fn filled_exceeds_qty_errors() {
        let raw = make_raw_order("o1", "AAPL", "buy", 100, 101, "new");
        assert!(matches!(
            normalize_order(raw),
            Err(SnapshotAdapterError::FilledExceedsQty { .. })
        ));
    }

    #[test]
    fn valid_order_normalizes_correctly() {
        let raw = make_raw_order("ord-1", "TSLA", "sell", 50, 25, "partially_filled");
        let snap = normalize_order(raw).unwrap();
        assert_eq!(snap.order_id, "ord-1");
        assert_eq!(snap.symbol, "TSLA");
        assert_eq!(snap.side, Side::Sell);
        assert_eq!(snap.qty, 50);
        assert_eq!(snap.filled_qty, 25);
        assert_eq!(snap.status, OrderStatus::PartiallyFilled);
    }

    // --- Full snapshot normalization ---

    #[test]
    fn normalize_empty_snapshot_is_ok() {
        let raw = RawBrokerSnapshot {
            orders: vec![],
            positions: vec![],
        };
        let snap = normalize(raw).unwrap();
        assert!(snap.orders.is_empty());
        assert!(snap.positions.is_empty());
    }

    #[test]
    fn normalize_positions_keyed_by_symbol() {
        let raw = RawBrokerSnapshot {
            orders: vec![],
            positions: vec![
                RawBrokerPosition {
                    symbol: "AAPL".to_string(),
                    qty_signed: 100,
                },
                RawBrokerPosition {
                    symbol: "TSLA".to_string(),
                    qty_signed: -50,
                },
            ],
        };
        let snap = normalize(raw).unwrap();
        assert_eq!(snap.positions["AAPL"], 100);
        assert_eq!(snap.positions["TSLA"], -50);
    }

    #[test]
    fn normalize_stops_on_first_bad_order() {
        let raw = RawBrokerSnapshot {
            orders: vec![
                make_raw_order("o1", "AAPL", "buy", 100, 0, "new"),
                make_raw_order("", "TSLA", "sell", 50, 0, "new"), // bad: empty order_id
            ],
            positions: vec![],
        };
        assert_eq!(normalize(raw), Err(SnapshotAdapterError::MissingOrderId));
    }

    #[test]
    fn normalize_lenient_skips_bad_orders() {
        let raw = RawBrokerSnapshot {
            orders: vec![
                make_raw_order("o1", "AAPL", "buy", 100, 0, "new"),
                make_raw_order("", "TSLA", "sell", 50, 0, "new"), // bad
            ],
            positions: vec![],
        };
        let (snap, errors) = normalize_lenient(raw);
        assert_eq!(snap.orders.len(), 1);
        assert!(snap.orders.contains_key("o1"));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0], SnapshotAdapterError::MissingOrderId);
    }

    #[test]
    fn normalize_json_round_trip() {
        let json = r#"{
            "orders": [
                {
                    "order_id": "abc-123",
                    "symbol": "MSFT",
                    "side": "buy",
                    "qty": 200,
                    "filled_qty": 200,
                    "status": "filled"
                }
            ],
            "positions": [
                { "symbol": "MSFT", "qty_signed": 200 }
            ]
        }"#;

        let snap = normalize_json(json).unwrap();
        assert_eq!(snap.orders.len(), 1);
        let ord = &snap.orders["abc-123"];
        assert_eq!(ord.symbol, "MSFT");
        assert_eq!(ord.side, Side::Buy);
        assert_eq!(ord.qty, 200);
        assert_eq!(ord.filled_qty, 200);
        assert_eq!(ord.status, OrderStatus::Filled);
        assert_eq!(snap.positions["MSFT"], 200);
    }

    #[test]
    fn normalize_orders_keyed_by_order_id() {
        let raw = RawBrokerSnapshot {
            orders: vec![
                make_raw_order("o1", "AAPL", "buy", 100, 0, "new"),
                make_raw_order("o2", "TSLA", "sell", 50, 50, "filled"),
            ],
            positions: vec![],
        };
        let snap = normalize(raw).unwrap();
        assert!(snap.orders.contains_key("o1"));
        assert!(snap.orders.contains_key("o2"));
        assert_eq!(snap.orders["o2"].status, OrderStatus::Filled);
    }

    #[test]
    fn whitespace_trimmed_in_order_id_and_symbol() {
        let raw = RawBrokerSnapshot {
            orders: vec![make_raw_order("  o1  ", "  AAPL  ", "buy", 10, 0, "new")],
            positions: vec![RawBrokerPosition {
                symbol: "  AAPL  ".to_string(),
                qty_signed: 10,
            }],
        };
        let snap = normalize(raw).unwrap();
        assert!(snap.orders.contains_key("o1"));
        assert!(snap.positions.contains_key("AAPL"));
    }
}
