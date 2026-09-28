//! D1 (V4-M5-M8-INDEPENDENT-REVIEW-CORRECTION-01): normalization seam for
//! Alpaca's options-lifecycle non-trade account activities (`OPEXC`
//! exercise, `OPASN` assignment, `OPEXP` expiration) and their paired
//! `OPTRD` trade activity.
//!
//! Per Alpaca's official "Non-Trade Activities for Option Events" docs
//! (re-verified live via Context7 during this correction):
//! - `OPEXC`/`OPASN`/`OPEXP` always carry `net_amount = "0"` and a `symbol`
//!   that IS the OCC option-contract symbol (e.g. `"AAPL230721C00150000"`).
//! - `OPEXC`/`OPASN` (never `OPEXP`, a worthless lapse has no trade) are
//!   paired with a same-`id` `OPTRD` activity whose `symbol` is the
//!   UNDERLYING ticker (e.g. `"AAPL"`) -- never the option contract -- and
//!   whose `net_amount`/`qty` carry the real, already-signed strike cash
//!   consideration and underlying share delta.
//! - The pairing evidence is the shared `id`, not `symbol`/`date`.
//!
//! This module classifies a raw activity by `activity_type` into one of
//! two shapes and preserves every field exactly as reported -- it never
//! infers, estimates, or derives a sign; that already-signed evidence is
//! `net_amount`/`qty` themselves, passed through unchanged.

use crate::types::AlpacaFeeActivity;

/// The four Alpaca options-lifecycle activity types this module
/// recognizes.
pub const ALPACA_OPTION_LIFECYCLE_ACTIVITY_TYPES: [&str; 4] = ["OPEXC", "OPASN", "OPEXP", "OPTRD"];

/// A normalized options-lifecycle activity, shaped for durable ingestion.
/// The two variants are structurally distinct because `OPEXC`/`OPASN`/
/// `OPEXP` and `OPTRD` carry genuinely different symbol semantics --
/// `option_symbol` (a lifecycle event's own OCC contract) and
/// `underlying_symbol_raw` (a paired trade's underlying ticker) are never
/// the same field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizedOptionLifecycleActivity {
    /// `OPEXC`/`OPASN`/`OPEXP`.
    Lifecycle {
        activity_id: String,
        activity_type: String,
        option_symbol: String,
        activity_date: String,
        qty_raw: String,
        net_amount_raw: String,
    },
    /// `OPTRD`, the paired trade for `OPEXC`/`OPASN` (shares `activity_id`
    /// with its lifecycle sibling).
    PairedTrade {
        activity_id: String,
        underlying_symbol_raw: String,
        activity_date: String,
        qty_raw: String,
        price_raw: String,
        net_amount_raw: String,
    },
}

impl NormalizedOptionLifecycleActivity {
    pub fn activity_id(&self) -> &str {
        match self {
            Self::Lifecycle { activity_id, .. } => activity_id,
            Self::PairedTrade { activity_id, .. } => activity_id,
        }
    }
}

/// Error normalizing a raw options-lifecycle activity. Fails closed only
/// when a field the shape genuinely requires is absent -- never estimates
/// or defaults a missing value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionLifecycleNormalizeError {
    UnknownActivityType { activity_id: String, raw: String },
    MissingSymbol { activity_id: String },
    MissingQty { activity_id: String },
    MissingDate { activity_id: String },
    MissingPriceForPairedTrade { activity_id: String },
}

impl std::fmt::Display for OptionLifecycleNormalizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownActivityType { activity_id, raw } => write!(
                f,
                "option_lifecycle_normalize: activity id={activity_id:?} has unrecognized \
                 activity_type: {raw:?}"
            ),
            Self::MissingSymbol { activity_id } => write!(
                f,
                "option_lifecycle_normalize: activity id={activity_id:?} is missing symbol"
            ),
            Self::MissingQty { activity_id } => write!(
                f,
                "option_lifecycle_normalize: activity id={activity_id:?} is missing qty"
            ),
            Self::MissingDate { activity_id } => write!(
                f,
                "option_lifecycle_normalize: activity id={activity_id:?} is missing date"
            ),
            Self::MissingPriceForPairedTrade { activity_id } => write!(
                f,
                "option_lifecycle_normalize: OPTRD activity id={activity_id:?} is missing price"
            ),
        }
    }
}

impl std::error::Error for OptionLifecycleNormalizeError {}

/// Normalize one raw Alpaca options-lifecycle activity.
///
/// `net_amount` is passed through exactly as reported for every activity
/// type -- always `"0"` for `OPEXC`/`OPASN`/`OPEXP` per Alpaca's own
/// contract, the real signed strike consideration for `OPTRD`. This
/// module never computes or assumes a sign; the raw string IS the
/// authoritative evidence.
pub fn normalize_option_lifecycle_activity(
    raw: &AlpacaFeeActivity,
) -> Result<NormalizedOptionLifecycleActivity, OptionLifecycleNormalizeError> {
    let activity_id = raw.id.clone();
    let symbol =
        raw.symbol
            .clone()
            .ok_or_else(|| OptionLifecycleNormalizeError::MissingSymbol {
                activity_id: activity_id.clone(),
            })?;
    let qty_raw = raw
        .qty
        .clone()
        .ok_or_else(|| OptionLifecycleNormalizeError::MissingQty {
            activity_id: activity_id.clone(),
        })?;
    let activity_date =
        raw.date
            .clone()
            .ok_or_else(|| OptionLifecycleNormalizeError::MissingDate {
                activity_id: activity_id.clone(),
            })?;

    match raw.activity_type.as_str() {
        "OPEXC" | "OPASN" | "OPEXP" => Ok(NormalizedOptionLifecycleActivity::Lifecycle {
            activity_id,
            activity_type: raw.activity_type.clone(),
            option_symbol: symbol,
            activity_date,
            qty_raw,
            net_amount_raw: raw.net_amount.clone(),
        }),
        "OPTRD" => {
            let price_raw = raw.price.clone().ok_or_else(|| {
                OptionLifecycleNormalizeError::MissingPriceForPairedTrade {
                    activity_id: activity_id.clone(),
                }
            })?;
            Ok(NormalizedOptionLifecycleActivity::PairedTrade {
                activity_id,
                underlying_symbol_raw: symbol,
                activity_date,
                qty_raw,
                price_raw,
                net_amount_raw: raw.net_amount.clone(),
            })
        }
        other => Err(OptionLifecycleNormalizeError::UnknownActivityType {
            activity_id,
            raw: other.to_string(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn activity(
        id: &str,
        activity_type: &str,
        symbol: Option<&str>,
        qty: Option<&str>,
        price: Option<&str>,
        net_amount: &str,
        date: Option<&str>,
    ) -> AlpacaFeeActivity {
        AlpacaFeeActivity {
            id: id.to_string(),
            activity_type: activity_type.to_string(),
            date: date.map(str::to_string),
            net_amount: net_amount.to_string(),
            description: Some("Option Exercise".to_string()),
            symbol: symbol.map(str::to_string),
            qty: qty.map(str::to_string),
            price: price.map(str::to_string),
            status: Some("executed".to_string()),
        }
    }

    const SHARED_ID: &str = "20190801011955195::5f596936-6f23-4cef-bdf1-3806aae57dbf";

    #[test]
    fn opexc_normalizes_to_lifecycle_with_option_symbol() {
        let raw = activity(
            SHARED_ID,
            "OPEXC",
            Some("AAPL230721C00150000"),
            Some("-2"),
            None,
            "0",
            Some("2023-07-21"),
        );
        let record = normalize_option_lifecycle_activity(&raw).unwrap();
        assert_eq!(
            record,
            NormalizedOptionLifecycleActivity::Lifecycle {
                activity_id: SHARED_ID.to_string(),
                activity_type: "OPEXC".to_string(),
                option_symbol: "AAPL230721C00150000".to_string(),
                activity_date: "2023-07-21".to_string(),
                qty_raw: "-2".to_string(),
                net_amount_raw: "0".to_string(),
            }
        );
    }

    #[test]
    fn opasn_normalizes_to_lifecycle() {
        let raw = activity(
            SHARED_ID,
            "OPASN",
            Some("AAPL230721C00150000"),
            Some("2"),
            None,
            "0",
            Some("2023-07-01"),
        );
        let record = normalize_option_lifecycle_activity(&raw).unwrap();
        assert!(matches!(
            record,
            NormalizedOptionLifecycleActivity::Lifecycle { activity_type, .. }
                if activity_type == "OPASN"
        ));
    }

    #[test]
    fn opexp_normalizes_to_lifecycle_with_zero_net_amount() {
        let raw = activity(
            "opexp-id",
            "OPEXP",
            Some("AAPL230721C00150000"),
            Some("-1"),
            None,
            "0",
            Some("2023-07-21"),
        );
        let record = normalize_option_lifecycle_activity(&raw).unwrap();
        assert!(matches!(
            record,
            NormalizedOptionLifecycleActivity::Lifecycle { net_amount_raw, .. }
                if net_amount_raw == "0"
        ));
    }

    #[test]
    fn optrd_paired_with_opexc_normalizes_to_paired_trade_with_underlying_symbol_and_signed_evidence(
    ) {
        // Real doc example: OPEXC pair, holder exercises a long call, pays
        // strike -- net_amount is NEGATIVE.
        let raw = activity(
            SHARED_ID,
            "OPTRD",
            Some("AAPL"), // the underlying, NOT the option contract
            Some("200"),
            Some("90"),
            "-30000",
            Some("2023-07-21"),
        );
        let record = normalize_option_lifecycle_activity(&raw).unwrap();
        assert_eq!(
            record,
            NormalizedOptionLifecycleActivity::PairedTrade {
                activity_id: SHARED_ID.to_string(),
                underlying_symbol_raw: "AAPL".to_string(),
                activity_date: "2023-07-21".to_string(),
                qty_raw: "200".to_string(),
                price_raw: "90".to_string(),
                net_amount_raw: "-30000".to_string(),
            }
        );
    }

    #[test]
    fn optrd_paired_with_opasn_carries_the_opposite_sign() {
        // Real doc example: OPASN pair, writer assigned/delivers shares,
        // receives strike -- net_amount is POSITIVE. Same shared id pattern
        // as the OPEXC pair, opposite economic direction -- this module
        // preserves whatever sign Alpaca reports, never assumes one.
        let raw = activity(
            SHARED_ID,
            "OPTRD",
            Some("AAPL"),
            Some("-200"),
            Some("150"),
            "30000",
            Some("2023-07-01"),
        );
        let record = normalize_option_lifecycle_activity(&raw).unwrap();
        assert_eq!(
            record,
            NormalizedOptionLifecycleActivity::PairedTrade {
                activity_id: SHARED_ID.to_string(),
                underlying_symbol_raw: "AAPL".to_string(),
                activity_date: "2023-07-01".to_string(),
                qty_raw: "-200".to_string(),
                price_raw: "150".to_string(),
                net_amount_raw: "30000".to_string(),
            }
        );
    }

    #[test]
    fn optrd_and_its_paired_lifecycle_activity_share_the_identical_activity_id() {
        let opexc = activity(
            SHARED_ID,
            "OPEXC",
            Some("AAPL230721C00150000"),
            Some("-2"),
            None,
            "0",
            Some("2023-07-21"),
        );
        let optrd = activity(
            SHARED_ID,
            "OPTRD",
            Some("AAPL"),
            Some("200"),
            Some("90"),
            "-30000",
            Some("2023-07-21"),
        );
        let opexc_record = normalize_option_lifecycle_activity(&opexc).unwrap();
        let optrd_record = normalize_option_lifecycle_activity(&optrd).unwrap();
        assert_eq!(
            opexc_record.activity_id(),
            optrd_record.activity_id(),
            "D1: the pairing evidence is the shared activity id"
        );
    }

    #[test]
    fn optrd_missing_price_fails_closed() {
        let raw = activity(
            SHARED_ID,
            "OPTRD",
            Some("AAPL"),
            Some("200"),
            None,
            "-30000",
            Some("2023-07-21"),
        );
        let err = normalize_option_lifecycle_activity(&raw).unwrap_err();
        assert!(matches!(
            err,
            OptionLifecycleNormalizeError::MissingPriceForPairedTrade { .. }
        ));
    }

    #[test]
    fn missing_symbol_fails_closed_for_every_activity_type() {
        for activity_type in ALPACA_OPTION_LIFECYCLE_ACTIVITY_TYPES {
            let raw = activity(
                "id",
                activity_type,
                None,
                Some("1"),
                Some("1"),
                "0",
                Some("2023-07-21"),
            );
            let err = normalize_option_lifecycle_activity(&raw).unwrap_err();
            assert!(
                matches!(err, OptionLifecycleNormalizeError::MissingSymbol { .. }),
                "activity_type={activity_type} must fail closed on missing symbol"
            );
        }
    }

    #[test]
    fn unknown_activity_type_fails_closed() {
        let raw = activity(
            "id",
            "FEE",
            Some("AAPL"),
            Some("1"),
            None,
            "0",
            Some("2023-07-21"),
        );
        let err = normalize_option_lifecycle_activity(&raw).unwrap_err();
        assert!(matches!(
            err,
            OptionLifecycleNormalizeError::UnknownActivityType { .. }
        ));
    }
}
