//! PAPER-SOAK-ALPACA-FILL-ECONOMIC-AUTHORITY-CLOSURE-01: the ONE canonical
//! seam for turning an already-mapped Alpaca `TradeActivity`
//! ([`AlpacaOrderActivity`]) into a validated, fill-class [`BrokerEvent`].
//!
//! Every REST-derived fill-class `BrokerEvent` construction that does not go
//! through the WS-shaped [`crate::normalize::normalize_trade_update`] path
//! (WS-gap REST recovery, halted-run REST recovery) must go through
//! [`activity_to_fill_broker_event`] instead of hand-rolling qty/price/side
//! parsing and the partial-fill `cum_qty_after` requirement a second (or
//! third) time. `fetch_events` (normal REST polling) still needs
//! `activity_to_trade_update` + `normalize_trade_update` because it must also
//! emit non-fill lifecycle events (Ack/Cancel/etc.) from the same activity
//! stream, but its internal `parse_qty` and partial-fill `cum_qty_after`
//! extraction both delegate to the same canonical
//! [`crate::normalize::parse_alpaca_whole_share_qty`] parser this module
//! uses — so quantity/cum_qty parsing is shared even though the surrounding
//! control flow differs, and (PAPER-SOAK-ALPACA-FILL-AUTHORITY-FINAL-CLOSURE-02)
//! both paths now require a parseable cumulative for a production Alpaca
//! partial fill, failing the event/page closed rather than degrading to an
//! unproven `cum_qty_after=None`.
use crate::classify_fill_subtype;
use crate::normalize::parse_alpaca_whole_share_qty;
use crate::types::AlpacaOrderActivity;
use mqk_execution::{price_to_micros, BrokerEvent, Side};

/// Error converting an [`AlpacaOrderActivity`] into a fill-class [`BrokerEvent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityFillConversionError {
    /// `classify_fill_subtype` reported this is not a FILL-class activity at all
    /// (e.g. `NEW`, `CANCELED`) — not an error, just not applicable here.
    NotFillClass,
    /// `classify_fill_subtype` failed: missing/unrecognized `type` field.
    Subtype(String),
    MissingQty,
    InvalidQty(String),
    MissingPrice,
    InvalidPrice(String),
    UnknownSide(String),
    /// `type == "partial_fill"` but the activity's own broker-native `cum_qty`
    /// is missing — cannot establish cross-lane economic identity.
    MissingPartialCumQty,
    /// `type == "partial_fill"` but `cum_qty` did not parse under the
    /// canonical whole-share model.
    InvalidPartialCumQty(String),
}

impl std::fmt::Display for ActivityFillConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFillClass => write!(f, "activity is not FILL-class"),
            Self::Subtype(e) => write!(f, "fill subtype classification failed: {e}"),
            Self::MissingQty => write!(f, "activity is missing qty"),
            Self::InvalidQty(raw) => write!(f, "activity qty is not a valid quantity: {raw:?}"),
            Self::MissingPrice => write!(f, "activity is missing price"),
            Self::InvalidPrice(raw) => write!(f, "activity price is not valid: {raw:?}"),
            Self::UnknownSide(raw) => write!(f, "activity side is not buy/sell: {raw:?}"),
            Self::MissingPartialCumQty => write!(
                f,
                "partial_fill activity is missing broker-native cum_qty; refusing to guess \
                 cross-lane economic identity"
            ),
            Self::InvalidPartialCumQty(raw) => write!(
                f,
                "partial_fill activity cum_qty is not a valid whole-share quantity: {raw:?}"
            ),
        }
    }
}

impl std::error::Error for ActivityFillConversionError {}

/// Build a canonical fill-class [`BrokerEvent`] from an Alpaca `TradeActivity`.
///
/// Enforces, in one place, the full economic-authority contract every
/// REST-derived fill path must obey:
///
/// - fill-subtype classification (`activity_type == "FILL"` + documented
///   `type` field — see [`classify_fill_subtype`]);
/// - canonical whole-share quantity parsing (no silent rounding, no `1e6`
///   unit confusion);
/// - price parsing at the wire boundary;
/// - side parsing;
/// - for `type == "partial_fill"`: a parseable broker-native `cum_qty` is
///   REQUIRED — `cum_qty_after` is never `None` for a partial fill built by
///   this function. Callers must refuse (not insert an economically
///   ambiguous row) on `Err`.
///
/// `broker_message_id` is caller-supplied because each REST-derived caller
/// uses its own stable synthetic identity scheme (gap-recovery uses
/// `"alpaca-rest-gap:{activity.id}"`; halted-run recovery uses
/// `"alpaca-rest-recovery:{activity.id}"`).
pub fn activity_to_fill_broker_event(
    activity: &AlpacaOrderActivity,
    broker_message_id: String,
    internal_order_id: String,
) -> Result<BrokerEvent, ActivityFillConversionError> {
    let subtype = classify_fill_subtype(activity)
        .map_err(ActivityFillConversionError::Subtype)?
        .ok_or(ActivityFillConversionError::NotFillClass)?;

    let qty_str = activity
        .qty
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or(ActivityFillConversionError::MissingQty)?;
    let delta_qty =
        parse_alpaca_whole_share_qty(qty_str).map_err(ActivityFillConversionError::InvalidQty)?;

    let price_str = activity
        .price
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or(ActivityFillConversionError::MissingPrice)?;
    let price_f64: f64 = price_str
        .parse()
        .map_err(|_| ActivityFillConversionError::InvalidPrice(price_str.to_string()))?;
    let price_micros = price_to_micros(price_f64)
        .map_err(|_| ActivityFillConversionError::InvalidPrice(price_str.to_string()))?;

    let side = match activity.side.as_str() {
        "buy" => Side::Buy,
        "sell" => Side::Sell,
        other => return Err(ActivityFillConversionError::UnknownSide(other.to_string())),
    };

    if subtype == "partial_fill" {
        let cum_qty_after = match activity.cum_qty.as_deref() {
            None => return Err(ActivityFillConversionError::MissingPartialCumQty),
            Some(raw) => parse_alpaca_whole_share_qty(raw)
                .map_err(ActivityFillConversionError::InvalidPartialCumQty)?,
        };
        Ok(BrokerEvent::PartialFill {
            broker_message_id,
            broker_fill_id: Some(activity.id.clone()),
            internal_order_id,
            broker_order_id: Some(activity.order_id.clone()),
            symbol: activity.symbol.clone(),
            side,
            delta_qty,
            price_micros,
            fee_micros: 0,
            cum_qty_after: Some(cum_qty_after),
        })
    } else {
        Ok(BrokerEvent::Fill {
            broker_message_id,
            broker_fill_id: Some(activity.id.clone()),
            internal_order_id,
            broker_order_id: Some(activity.order_id.clone()),
            symbol: activity.symbol.clone(),
            side,
            delta_qty,
            price_micros,
            fee_micros: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activity(
        activity_type: &str,
        trade_type: Option<&str>,
        qty: Option<&str>,
        price: Option<&str>,
        cum_qty: Option<&str>,
        side: &str,
    ) -> AlpacaOrderActivity {
        AlpacaOrderActivity {
            id: "activity-1".to_string(),
            activity_type: activity_type.to_string(),
            trade_type: trade_type.map(str::to_string),
            order_id: "broker-order-1".to_string(),
            transaction_time: "2024-01-15T10:30:00Z".to_string(),
            price: price.map(str::to_string),
            qty: qty.map(str::to_string),
            side: side.to_string(),
            symbol: "AAPL".to_string(),
            cum_qty: cum_qty.map(str::to_string),
        }
    }

    #[test]
    fn partial_fill_with_cum_qty_builds_some_cum_qty_after() {
        let a = activity(
            "FILL",
            Some("partial_fill"),
            Some("5"),
            Some("100.00"),
            Some("5"),
            "buy",
        );
        let ev = activity_to_fill_broker_event(&a, "msg-1".to_string(), "int-1".to_string())
            .expect("must build");
        match ev {
            BrokerEvent::PartialFill {
                delta_qty,
                cum_qty_after,
                ..
            } => {
                assert_eq!(delta_qty, 5);
                assert_eq!(cum_qty_after, Some(5));
            }
            other => panic!("expected PartialFill, got {other:?}"),
        }
    }

    #[test]
    fn partial_fill_missing_cum_qty_fails_closed() {
        let a = activity(
            "FILL",
            Some("partial_fill"),
            Some("5"),
            Some("100.00"),
            None,
            "buy",
        );
        let err = activity_to_fill_broker_event(&a, "msg-1".to_string(), "int-1".to_string())
            .unwrap_err();
        assert_eq!(err, ActivityFillConversionError::MissingPartialCumQty);
    }

    #[test]
    fn terminal_fill_has_no_cum_qty_requirement() {
        let a = activity(
            "FILL",
            Some("fill"),
            Some("5"),
            Some("100.00"),
            None,
            "sell",
        );
        let ev = activity_to_fill_broker_event(&a, "msg-1".to_string(), "int-1".to_string())
            .expect("terminal fill must not require cum_qty");
        assert!(matches!(ev, BrokerEvent::Fill { .. }));
    }

    #[test]
    fn fractional_qty_fails_closed_not_rounded() {
        let a = activity(
            "FILL",
            Some("fill"),
            Some("5.5"),
            Some("100.00"),
            None,
            "buy",
        );
        let err = activity_to_fill_broker_event(&a, "msg-1".to_string(), "int-1".to_string())
            .unwrap_err();
        assert!(matches!(err, ActivityFillConversionError::InvalidQty(_)));
    }

    #[test]
    fn non_fill_activity_is_not_fill_class() {
        let a = activity("NEW", None, None, None, None, "buy");
        let err = activity_to_fill_broker_event(&a, "msg-1".to_string(), "int-1".to_string())
            .unwrap_err();
        assert_eq!(err, ActivityFillConversionError::NotFillClass);
    }
}
