//! CRYPTO-FEE-ATTRIBUTION-01: normalization seam for Alpaca's day-end
//! CFEE/FEE account activities (operator decision — see the mission's
//! CRYPTO-FEE-ATTRIBUTION-01 disposition).
//!
//! Alpaca crypto trading fees are maker/taker- and 30-day-volume-tiered,
//! and are calculated and posted **once per day** as a separate account
//! activity — never as part of the synchronous fill event a crypto order
//! produces. A synchronous crypto `BrokerEvent::Fill`/`PartialFill` therefore
//! always carries `fee_micros = 0`, but that value must never be read as
//! "this trade had zero economic cost" — it means "not yet attributed."
//! This module is the seam that later attributes the real, broker-confirmed
//! fee once Alpaca posts it, without ever hardcoding, estimating, or
//! assuming a fee-tier rate.
//!
//! # Two fee shapes
//!
//! Per Alpaca's documented `CFEE`/`FEE` activity schema, a fee may be
//! charged in fiat (`net_amount`, a dollar amount) or, for a coin-pair
//! transaction fee, in the traded asset itself (`qty`, in the asset's own
//! units). Only the fiat case can be applied as a simple portfolio cash
//! adjustment via the existing `mqk_portfolio::CashEntry` seam. An
//! asset-denominated fee would need to reduce a position's quantity, not
//! cash — this module does not implement that (no production caller
//! constructs one today); it surfaces the activity as
//! [`FeeAttributionRecord::AssetDenominatedFeeUnsupported`] so the evidence
//! is recognized and preserved rather than silently dropped or
//! misapplied as a zero-cost cash entry.
//!
//! # No randomness, no wall-clock reads, no rate assumptions
//!
//! Every value here comes from the Alpaca activity payload itself. This
//! module never selects, estimates, or defaults a maker/taker tier or fee
//! rate — it only parses what Alpaca has already confirmed and posted.

use crate::types::AlpacaFeeActivity;
use mqk_execution::price_to_micros;

/// The two Alpaca day-end fee activity types this module recognizes.
pub const ALPACA_FEE_ACTIVITY_TYPES: [&str; 2] = ["CFEE", "FEE"];

/// A normalized, broker-confirmed fee attribution derived from one Alpaca
/// `CFEE`/`FEE` activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeeAttributionRecord {
    /// A confirmed fiat (USD) fee that can be applied to portfolio cash via
    /// `mqk_portfolio::CashEntry`. `fee_micros` carries Alpaca's own sign
    /// (negative for a debit) — apply it to cash unchanged, do not negate.
    CashFee {
        /// Alpaca's own activity `id` — the deterministic idempotency key;
        /// an activity must never be applied to the ledger twice.
        activity_id: String,
        activity_type: String,
        symbol: Option<String>,
        fee_micros: i64,
    },
    /// A fee charged in the traded asset itself (`net_amount` absent/zero
    /// but `qty` present and nonzero) — recognized but not applied. Not an
    /// error: the activity is genuine, broker-confirmed evidence; it is
    /// surfaced so a caller can record/alert on it rather than silently
    /// treating the trade as free or misapplying an asset-quantity fee as
    /// a cash entry.
    AssetDenominatedFeeUnsupported {
        activity_id: String,
        activity_type: String,
        symbol: Option<String>,
        qty_raw: String,
    },
    /// A genuinely zero fee, confirmed by the broker (both `net_amount` and
    /// `qty` are absent or zero) — e.g. a fee waiver. Distinct in meaning
    /// from a synchronous fill's unattributed `fee_micros = 0`: this IS
    /// broker-confirmed evidence of zero cost, not a placeholder.
    ConfirmedZeroFee {
        activity_id: String,
        activity_type: String,
        symbol: Option<String>,
    },
}

/// Error normalizing an [`AlpacaFeeActivity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeeNormalizeError {
    /// `net_amount` is present but not a parseable, finite decimal number.
    InvalidNetAmount { activity_id: String, raw: String },
}

impl std::fmt::Display for FeeNormalizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidNetAmount { activity_id, raw } => write!(
                f,
                "fee_attribution: activity id={activity_id:?} has non-parseable net_amount: {raw:?}"
            ),
        }
    }
}

impl std::error::Error for FeeNormalizeError {}

/// True when `raw` parses to exactly zero. Used to distinguish "no
/// meaningful quantity/amount reported" from a genuine nonzero value,
/// without ever rounding a small-but-real value down to nothing.
fn parses_to_nonzero(raw: &str) -> bool {
    raw.trim().parse::<f64>().map(|v| v != 0.0).unwrap_or(false)
}

/// Normalize one raw Alpaca `CFEE`/`FEE` activity into a
/// [`FeeAttributionRecord`].
///
/// Fails closed only when `net_amount` cannot be parsed at all — every
/// other field is optional/descriptive. Never estimates, rounds, or
/// defaults a fee value; every number here is exactly what Alpaca reported.
pub fn normalize_fee_activity(
    raw: &AlpacaFeeActivity,
) -> Result<FeeAttributionRecord, FeeNormalizeError> {
    let net_amount: f64 = raw
        .net_amount
        .trim()
        .parse()
        .map_err(|_| FeeNormalizeError::InvalidNetAmount {
            activity_id: raw.id.clone(),
            raw: raw.net_amount.clone(),
        })?;
    if !net_amount.is_finite() {
        return Err(FeeNormalizeError::InvalidNetAmount {
            activity_id: raw.id.clone(),
            raw: raw.net_amount.clone(),
        });
    }

    if net_amount != 0.0 {
        let fee_micros =
            price_to_micros(net_amount).map_err(|_| FeeNormalizeError::InvalidNetAmount {
                activity_id: raw.id.clone(),
                raw: raw.net_amount.clone(),
            })?;
        return Ok(FeeAttributionRecord::CashFee {
            activity_id: raw.id.clone(),
            activity_type: raw.activity_type.clone(),
            symbol: raw.symbol.clone(),
            fee_micros,
        });
    }

    // net_amount == 0: either a genuinely free activity, or the fee was
    // charged in the asset itself (qty nonzero) rather than fiat.
    let asset_denominated = raw
        .qty
        .as_deref()
        .map(parses_to_nonzero)
        .unwrap_or(false);

    if asset_denominated {
        return Ok(FeeAttributionRecord::AssetDenominatedFeeUnsupported {
            activity_id: raw.id.clone(),
            activity_type: raw.activity_type.clone(),
            symbol: raw.symbol.clone(),
            qty_raw: raw.qty.clone().unwrap_or_default(),
        });
    }

    Ok(FeeAttributionRecord::ConfirmedZeroFee {
        activity_id: raw.id.clone(),
        activity_type: raw.activity_type.clone(),
        symbol: raw.symbol.clone(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn activity(net_amount: &str, qty: Option<&str>) -> AlpacaFeeActivity {
        AlpacaFeeActivity {
            id: "20220812000000000::53be51ba-46f9-43de-b81f-576f241dc680".to_string(),
            activity_type: "CFEE".to_string(),
            date: Some("2022-08-12".to_string()),
            net_amount: net_amount.to_string(),
            description: Some("Coin Pair Transaction Fee (Non USD)".to_string()),
            symbol: Some("ETHUSD".to_string()),
            qty: qty.map(str::to_string),
            price: Some("1884.5".to_string()),
            status: Some("executed".to_string()),
        }
    }

    #[test]
    fn a_confirmed_fiat_debit_normalizes_to_cash_fee_with_alpacas_own_sign() {
        let a = activity("-0.01", None);
        let record = normalize_fee_activity(&a).unwrap();
        assert_eq!(
            record,
            FeeAttributionRecord::CashFee {
                activity_id: a.id.clone(),
                activity_type: "CFEE".to_string(),
                symbol: Some("ETHUSD".to_string()),
                fee_micros: -10_000,
            }
        );
    }

    #[test]
    fn zero_net_amount_with_nonzero_qty_is_asset_denominated_unsupported() {
        let a = activity("0", Some("-0.000195"));
        let record = normalize_fee_activity(&a).unwrap();
        assert_eq!(
            record,
            FeeAttributionRecord::AssetDenominatedFeeUnsupported {
                activity_id: a.id.clone(),
                activity_type: "CFEE".to_string(),
                symbol: Some("ETHUSD".to_string()),
                qty_raw: "-0.000195".to_string(),
            }
        );
    }

    #[test]
    fn zero_net_amount_with_no_qty_is_confirmed_zero_fee() {
        let a = activity("0", None);
        let record = normalize_fee_activity(&a).unwrap();
        assert_eq!(
            record,
            FeeAttributionRecord::ConfirmedZeroFee {
                activity_id: a.id.clone(),
                activity_type: "CFEE".to_string(),
                symbol: Some("ETHUSD".to_string()),
            }
        );
    }

    #[test]
    fn zero_net_amount_with_zero_qty_is_confirmed_zero_fee_not_asset_denominated() {
        // A qty field present but itself zero must not be misread as an
        // asset-denominated fee.
        let a = activity("0", Some("0"));
        let record = normalize_fee_activity(&a).unwrap();
        assert!(matches!(record, FeeAttributionRecord::ConfirmedZeroFee { .. }));
    }

    #[test]
    fn non_numeric_net_amount_fails_closed() {
        let a = activity("not-a-number", None);
        let err = normalize_fee_activity(&a).unwrap_err();
        assert!(matches!(err, FeeNormalizeError::InvalidNetAmount { .. }));
    }

    #[test]
    fn nan_or_infinite_net_amount_fails_closed() {
        for bad in ["NaN", "inf", "-inf"] {
            let a = activity(bad, None);
            let err = normalize_fee_activity(&a).unwrap_err();
            assert!(
                matches!(err, FeeNormalizeError::InvalidNetAmount { .. }),
                "net_amount={bad:?} must fail closed"
            );
        }
    }

    #[test]
    fn a_positive_net_amount_is_preserved_exactly_not_assumed_negative() {
        // Not every fee activity is necessarily a debit (e.g. a rebate) —
        // this module must never flip or assume a sign.
        let a = activity("0.05", None);
        let record = normalize_fee_activity(&a).unwrap();
        assert_eq!(
            record,
            FeeAttributionRecord::CashFee {
                activity_id: a.id.clone(),
                activity_type: "CFEE".to_string(),
                symbol: Some("ETHUSD".to_string()),
                fee_micros: 50_000,
            }
        );
    }
}
