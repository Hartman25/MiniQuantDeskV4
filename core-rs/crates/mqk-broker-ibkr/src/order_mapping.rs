//! C3: deterministic, pure mapping from MQD's broker-agnostic
//! [`BrokerSubmitRequest`] to an IBKR-shaped order request. Pure function —
//! no I/O, no wall-clock read, no randomness. Every quantity/price is
//! formatted as an exact decimal string via integer arithmetic (never
//! `f64`), so this mapping can never introduce floating-point rounding
//! error before a value ever reaches the wire.

use mqk_execution::{BrokerError, BrokerSubmitRequest, Side};

/// One order request in IBKR's own vocabulary (`action`/`orderType`/`tif`
/// strings match the TWS API's documented values exactly).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IbkrOrderRequest {
    /// `"BUY"` or `"SELL"`.
    pub action: &'static str,
    /// Exact decimal quantity string (e.g. `"10"`, `"0.5"`) — never `f64`.
    pub total_quantity: String,
    /// `"MKT"` or `"LMT"`.
    pub order_type: &'static str,
    /// Exact decimal limit-price string; `None` for a market order.
    pub limit_price: Option<String>,
    /// IBKR time-in-force string (`"DAY"`, `"GTC"`, `"IOC"`, ...) — passed
    /// through from the caller's own already-validated value, never
    /// defaulted or rewritten here (mirrors this repo's IR-2 contract:
    /// TIF admission is this adapter's caller's responsibility, not a
    /// silent rewrite inside the wire-mapping layer).
    pub tif: String,
}

/// Format `raw` 1e-6-scale integer micros as an exact decimal string with
/// no floating-point arithmetic. `raw` may be negative (never expected for
/// a quantity, but price deltas are represented the same way elsewhere in
/// this repo, so this helper stays sign-correct).
fn micros_to_exact_decimal_string(raw: i64) -> String {
    const SCALE: i64 = 1_000_000;
    let sign = if raw < 0 { "-" } else { "" };
    let magnitude = raw.unsigned_abs();
    let whole = magnitude / (SCALE as u64);
    let frac = magnitude % (SCALE as u64);
    if frac == 0 {
        format!("{sign}{whole}")
    } else {
        // 6-digit fractional part, then trim trailing zeros (never trim
        // past the decimal point itself) so "1.500000" renders as "1.5"
        // and "1.000001" renders exactly, unrounded.
        let frac_str = format!("{frac:06}");
        let trimmed = frac_str.trim_end_matches('0');
        format!("{sign}{whole}.{trimmed}")
    }
}

/// Map a validated [`BrokerSubmitRequest`] into an [`IbkrOrderRequest`].
/// Fails closed (`BrokerError::Reject`) on a non-positive quantity or a
/// limit order missing its price — never guesses or defaults either.
pub fn map_submit_request_to_ibkr_order(
    req: &BrokerSubmitRequest,
) -> Result<IbkrOrderRequest, BrokerError> {
    if !req.quantity.is_positive() {
        return Err(BrokerError::Reject {
            code: "ibkr_qty_not_positive".to_string(),
            detail: format!(
                "map_submit_request_to_ibkr_order: quantity must be strictly positive \
                 (direction is carried by side, not sign), got {}",
                req.quantity
            ),
        });
    }

    let action = match req.side {
        Side::Buy => "BUY",
        Side::Sell => "SELL",
    };

    let order_type_upper = req.order_type.trim().to_ascii_uppercase();
    let (order_type, limit_price) = match order_type_upper.as_str() {
        "MARKET" | "MKT" => {
            if req.limit_price.is_some() {
                return Err(BrokerError::Reject {
                    code: "ibkr_market_order_has_limit_price".to_string(),
                    detail: "map_submit_request_to_ibkr_order: a market order must not carry a \
                             limit_price"
                        .to_string(),
                });
            }
            ("MKT", None)
        }
        "LIMIT" | "LMT" => {
            let Some(price_micros) = req.limit_price else {
                return Err(BrokerError::Reject {
                    code: "ibkr_limit_order_missing_price".to_string(),
                    detail: "map_submit_request_to_ibkr_order: a limit order requires \
                             limit_price; refusing rather than guessing one"
                        .to_string(),
                });
            };
            ("LMT", Some(micros_to_exact_decimal_string(price_micros)))
        }
        other => {
            return Err(BrokerError::Reject {
                code: "ibkr_order_type_unsupported".to_string(),
                detail: format!(
                    "map_submit_request_to_ibkr_order: unsupported order_type {other:?}; this \
                     adapter foundation recognizes only MARKET/MKT and LIMIT/LMT"
                ),
            });
        }
    };

    Ok(IbkrOrderRequest {
        action,
        total_quantity: micros_to_exact_decimal_string(req.quantity.raw()),
        order_type,
        limit_price,
        tif: req.time_in_force.trim().to_ascii_uppercase(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mqk_execution::AssetClass;
    use mqk_schemas::QtyMicros;

    fn base_request() -> BrokerSubmitRequest {
        BrokerSubmitRequest {
            order_id: "order-1".to_string(),
            symbol: "MES".to_string(),
            side: Side::Buy,
            quantity: QtyMicros::new(2_000_000),
            order_type: "MARKET".to_string(),
            limit_price: None,
            time_in_force: "day".to_string(),
            asset_class: AssetClass::Future,
        }
    }

    #[test]
    fn market_buy_maps_action_and_type_with_no_price() {
        let req = base_request();
        let mapped = map_submit_request_to_ibkr_order(&req).expect("must map");
        assert_eq!(mapped.action, "BUY");
        assert_eq!(mapped.order_type, "MKT");
        assert_eq!(mapped.limit_price, None);
        assert_eq!(mapped.total_quantity, "2");
        assert_eq!(mapped.tif, "DAY");
    }

    #[test]
    fn sell_side_maps_to_sell_action() {
        let mut req = base_request();
        req.side = Side::Sell;
        let mapped = map_submit_request_to_ibkr_order(&req).expect("must map");
        assert_eq!(mapped.action, "SELL");
    }

    #[test]
    fn limit_order_carries_exact_decimal_price_no_float_rounding() {
        let mut req = base_request();
        req.order_type = "LIMIT".to_string();
        // 150.505000 micros-scale -> must render exactly, not round to a
        // float-truncated value.
        req.limit_price = Some(150_505_000);
        let mapped = map_submit_request_to_ibkr_order(&req).expect("must map");
        assert_eq!(mapped.order_type, "LMT");
        assert_eq!(mapped.limit_price.as_deref(), Some("150.505"));
    }

    #[test]
    fn fractional_quantity_renders_exact_decimal_string() {
        let mut req = base_request();
        req.quantity = QtyMicros::new(1_500_000); // 1.5
        let mapped = map_submit_request_to_ibkr_order(&req).expect("must map");
        assert_eq!(mapped.total_quantity, "1.5");
    }

    #[test]
    fn zero_quantity_is_refused() {
        let mut req = base_request();
        req.quantity = QtyMicros::ZERO;
        let err = map_submit_request_to_ibkr_order(&req).expect_err("must refuse");
        match err {
            BrokerError::Reject { code, .. } => assert_eq!(code, "ibkr_qty_not_positive"),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn negative_quantity_is_refused() {
        let mut req = base_request();
        req.quantity = QtyMicros::new(-1_000_000);
        let err = map_submit_request_to_ibkr_order(&req).expect_err("must refuse");
        match err {
            BrokerError::Reject { code, .. } => assert_eq!(code, "ibkr_qty_not_positive"),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn market_order_with_limit_price_is_refused() {
        let mut req = base_request();
        req.limit_price = Some(100_000_000);
        let err = map_submit_request_to_ibkr_order(&req).expect_err("must refuse");
        match err {
            BrokerError::Reject { code, .. } => {
                assert_eq!(code, "ibkr_market_order_has_limit_price")
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn limit_order_missing_price_is_refused_never_defaulted() {
        let mut req = base_request();
        req.order_type = "LIMIT".to_string();
        req.limit_price = None;
        let err = map_submit_request_to_ibkr_order(&req).expect_err("must refuse");
        match err {
            BrokerError::Reject { code, .. } => assert_eq!(code, "ibkr_limit_order_missing_price"),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_order_type_is_refused() {
        let mut req = base_request();
        req.order_type = "STOP".to_string();
        let err = map_submit_request_to_ibkr_order(&req).expect_err("must refuse");
        match err {
            BrokerError::Reject { code, .. } => assert_eq!(code, "ibkr_order_type_unsupported"),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn time_in_force_is_passed_through_uppercased_never_rewritten() {
        let mut req = base_request();
        req.time_in_force = "gtc".to_string();
        let mapped = map_submit_request_to_ibkr_order(&req).expect("must map");
        assert_eq!(mapped.tif, "GTC");
    }
}
