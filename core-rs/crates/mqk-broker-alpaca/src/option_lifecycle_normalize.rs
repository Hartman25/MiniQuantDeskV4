//! D1: normalization seam for Alpaca's options-lifecycle non-trade account
//! activities (`OPEXC` exercise, `OPASN` assignment, `OPEXP` expiration) and
//! their `OPTRD` trade activity, over the dedicated raw type
//! [`AlpacaOptionLifecycleActivity`].
//!
//! Provider facts (current official Alpaca docs, "Non-Trade Activities for
//! Option Events", re-read 2026-09-28):
//! - `OPEXC`/`OPASN`/`OPEXP`: `symbol` IS the OCC option-contract symbol,
//!   `net_amount` is `"0"`, `qty` is the signed change to the option position
//!   (`OPEXC` example `-2` = long contracts removed, `OPASN` example `2` =
//!   short contracts removed).
//! - `OPTRD` accompanies `OPEXC`/`OPASN` (never a worthless `OPEXP`): `symbol`
//!   is the UNDERLYING ticker, `qty` the signed share delta, `net_amount` the
//!   signed strike cash.
//! - The documented examples show a lifecycle row and its `OPTRD` row carrying
//!   the same `id`, but the docs' own OPEXC sample is internally inconsistent
//!   (price 90 vs strike 150 vs net_amount -30000), and the newer
//!   Activity-event surface exposes explicit `ref_id`/`group_id`. Same-`id` is
//!   therefore only ONE correlation input, never universal authority -- see
//!   the daemon's `option_lifecycle_correlation`.
//!
//! This module classifies a raw activity by `activity_type`, preserves every
//! field exactly as reported (never infers or signs a value) and enforces the
//! symbol-shape contract: a lifecycle row's symbol must parse as an OCC option
//! contract, an `OPTRD` row's symbol must NOT (it is the underlying), so an
//! option contract is never confused with its underlying.

use crate::types::AlpacaOptionLifecycleActivity;
use mqk_execution::OptionContractIdentity;

/// The four Alpaca options-lifecycle activity types this module recognizes.
pub const ALPACA_OPTION_LIFECYCLE_ACTIVITY_TYPES: [&str; 4] = ["OPEXC", "OPASN", "OPEXP", "OPTRD"];

/// Provider-supplied descriptive/correlation fields preserved verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OptionLifecycleProvenance {
    pub group_id: Option<String>,
    pub ref_id: Option<String>,
    pub status: Option<String>,
    pub description: Option<String>,
}

/// A normalized options-lifecycle activity, shaped for durable ingestion.
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
        provenance: OptionLifecycleProvenance,
    },
    /// `OPTRD`.
    PairedTrade {
        activity_id: String,
        underlying_symbol_raw: String,
        activity_date: String,
        qty_raw: String,
        price_raw: String,
        net_amount_raw: String,
        provenance: OptionLifecycleProvenance,
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

/// Error normalizing a raw options-lifecycle activity. Fails closed only when
/// a field the shape genuinely requires is absent or the symbol shape
/// contradicts the activity type -- never estimates or defaults a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionLifecycleNormalizeError {
    UnknownActivityType {
        activity_id: String,
        raw: String,
    },
    MissingSymbol {
        activity_id: String,
    },
    MissingQty {
        activity_id: String,
    },
    MissingDate {
        activity_id: String,
    },
    MissingPriceForPairedTrade {
        activity_id: String,
    },
    /// OPEXC/OPASN/OPEXP `symbol` is not a standard OCC option contract.
    LifecycleSymbolNotAnOptionContract {
        activity_id: String,
        symbol: String,
    },
    /// OPTRD `symbol` parses as an option contract; it must be the underlying.
    PairedTradeSymbolIsAnOptionContract {
        activity_id: String,
        symbol: String,
    },
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
            Self::LifecycleSymbolNotAnOptionContract {
                activity_id,
                symbol,
            } => write!(
                f,
                "option_lifecycle_normalize: lifecycle activity id={activity_id:?} symbol \
                 {symbol:?} is not a standard OCC option contract"
            ),
            Self::PairedTradeSymbolIsAnOptionContract {
                activity_id,
                symbol,
            } => write!(
                f,
                "option_lifecycle_normalize: OPTRD activity id={activity_id:?} symbol {symbol:?} \
                 is an option contract; OPTRD carries the underlying ticker"
            ),
        }
    }
}

impl std::error::Error for OptionLifecycleNormalizeError {}

/// Normalize one raw Alpaca options-lifecycle activity.
pub fn normalize_option_lifecycle_activity(
    raw: &AlpacaOptionLifecycleActivity,
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
    let provenance = OptionLifecycleProvenance {
        group_id: raw.group_id.clone(),
        ref_id: raw.ref_id.clone(),
        status: raw.status.clone(),
        description: raw.description.clone(),
    };

    match raw.activity_type.as_str() {
        "OPEXC" | "OPASN" | "OPEXP" => {
            if OptionContractIdentity::parse(&symbol).is_err() {
                return Err(
                    OptionLifecycleNormalizeError::LifecycleSymbolNotAnOptionContract {
                        activity_id,
                        symbol,
                    },
                );
            }
            Ok(NormalizedOptionLifecycleActivity::Lifecycle {
                activity_id,
                activity_type: raw.activity_type.clone(),
                option_symbol: symbol,
                activity_date,
                qty_raw,
                net_amount_raw: raw.net_amount.clone(),
                provenance,
            })
        }
        "OPTRD" => {
            let price_raw = raw.price.clone().ok_or_else(|| {
                OptionLifecycleNormalizeError::MissingPriceForPairedTrade {
                    activity_id: activity_id.clone(),
                }
            })?;
            if OptionContractIdentity::parse(&symbol).is_ok() {
                return Err(
                    OptionLifecycleNormalizeError::PairedTradeSymbolIsAnOptionContract {
                        activity_id,
                        symbol,
                    },
                );
            }
            Ok(NormalizedOptionLifecycleActivity::PairedTrade {
                activity_id,
                underlying_symbol_raw: symbol,
                activity_date,
                qty_raw,
                price_raw,
                net_amount_raw: raw.net_amount.clone(),
                provenance,
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
    ) -> AlpacaOptionLifecycleActivity {
        AlpacaOptionLifecycleActivity {
            id: id.to_string(),
            activity_type: activity_type.to_string(),
            date: date.map(str::to_string),
            net_amount: net_amount.to_string(),
            description: Some("Option Exercise".to_string()),
            symbol: symbol.map(str::to_string),
            qty: qty.map(str::to_string),
            price: price.map(str::to_string),
            status: Some("executed".to_string()),
            group_id: None,
            ref_id: None,
        }
    }

    const SHARED_ID: &str = "20190801011955195::5f596936-6f23-4cef-bdf1-3806aae57dbf";

    fn executed() -> OptionLifecycleProvenance {
        OptionLifecycleProvenance {
            group_id: None,
            ref_id: None,
            status: Some("executed".to_string()),
            description: Some("Option Exercise".to_string()),
        }
    }

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
        assert_eq!(
            normalize_option_lifecycle_activity(&raw).unwrap(),
            NormalizedOptionLifecycleActivity::Lifecycle {
                activity_id: SHARED_ID.to_string(),
                activity_type: "OPEXC".to_string(),
                option_symbol: "AAPL230721C00150000".to_string(),
                activity_date: "2023-07-21".to_string(),
                qty_raw: "-2".to_string(),
                net_amount_raw: "0".to_string(),
                provenance: executed(),
            }
        );
    }

    #[test]
    fn opasn_and_opexp_normalize_to_lifecycle() {
        for (ty, qty) in [("OPASN", "2"), ("OPEXP", "-1")] {
            let raw = activity(
                "id",
                ty,
                Some("AAPL230721C00150000"),
                Some(qty),
                None,
                "0",
                Some("2023-07-21"),
            );
            assert!(matches!(
                normalize_option_lifecycle_activity(&raw).unwrap(),
                NormalizedOptionLifecycleActivity::Lifecycle { activity_type, net_amount_raw, .. }
                    if activity_type == ty && net_amount_raw == "0"
            ));
        }
    }

    #[test]
    fn optrd_carries_the_underlying_and_the_signed_provider_evidence() {
        for (qty, price, net) in [("200", "150", "-30000"), ("-200", "150", "30000")] {
            let raw = activity(
                SHARED_ID,
                "OPTRD",
                Some("AAPL"),
                Some(qty),
                Some(price),
                net,
                Some("2023-07-21"),
            );
            assert_eq!(
                normalize_option_lifecycle_activity(&raw).unwrap(),
                NormalizedOptionLifecycleActivity::PairedTrade {
                    activity_id: SHARED_ID.to_string(),
                    underlying_symbol_raw: "AAPL".to_string(),
                    activity_date: "2023-07-21".to_string(),
                    qty_raw: qty.to_string(),
                    price_raw: price.to_string(),
                    net_amount_raw: net.to_string(),
                    provenance: executed(),
                }
            );
        }
    }

    #[test]
    fn optional_provider_correlation_fields_are_preserved_never_invented() {
        let mut raw = activity(
            "id",
            "OPEXC",
            Some("AAPL230721C00150000"),
            Some("-2"),
            None,
            "0",
            Some("2023-07-21"),
        );
        let NormalizedOptionLifecycleActivity::Lifecycle { provenance, .. } =
            normalize_option_lifecycle_activity(&raw).unwrap()
        else {
            panic!()
        };
        assert_eq!((provenance.group_id, provenance.ref_id), (None, None));
        raw.group_id = Some("GRP-1".to_string());
        raw.ref_id = Some("REF-1".to_string());
        let NormalizedOptionLifecycleActivity::Lifecycle { provenance, .. } =
            normalize_option_lifecycle_activity(&raw).unwrap()
        else {
            panic!()
        };
        assert_eq!(provenance.group_id.as_deref(), Some("GRP-1"));
        assert_eq!(provenance.ref_id.as_deref(), Some("REF-1"));
    }

    #[test]
    fn symbol_shape_must_match_the_activity_type() {
        // Underlying ticker on a lifecycle row: never an option contract.
        let raw = activity(
            "id",
            "OPEXC",
            Some("AAPL"),
            Some("-2"),
            None,
            "0",
            Some("2023-07-21"),
        );
        assert!(matches!(
            normalize_option_lifecycle_activity(&raw).unwrap_err(),
            OptionLifecycleNormalizeError::LifecycleSymbolNotAnOptionContract { .. }
        ));
        // Option contract on an OPTRD row: OPTRD carries the underlying.
        let raw = activity(
            "id",
            "OPTRD",
            Some("AAPL230721C00150000"),
            Some("200"),
            Some("150"),
            "-30000",
            Some("2023-07-21"),
        );
        assert!(matches!(
            normalize_option_lifecycle_activity(&raw).unwrap_err(),
            OptionLifecycleNormalizeError::PairedTradeSymbolIsAnOptionContract { .. }
        ));
    }

    #[test]
    fn missing_required_fields_fail_closed() {
        let base = |ty: &str, sym, qty, price, date| activity("id", ty, sym, qty, price, "0", date);
        assert!(matches!(
            normalize_option_lifecycle_activity(&base(
                "OPEXC",
                None,
                Some("-1"),
                None,
                Some("2023-07-21")
            ))
            .unwrap_err(),
            OptionLifecycleNormalizeError::MissingSymbol { .. }
        ));
        assert!(matches!(
            normalize_option_lifecycle_activity(&base(
                "OPEXC",
                Some("AAPL230721C00150000"),
                None,
                None,
                Some("2023-07-21")
            ))
            .unwrap_err(),
            OptionLifecycleNormalizeError::MissingQty { .. }
        ));
        assert!(matches!(
            normalize_option_lifecycle_activity(&base(
                "OPEXC",
                Some("AAPL230721C00150000"),
                Some("-1"),
                None,
                None
            ))
            .unwrap_err(),
            OptionLifecycleNormalizeError::MissingDate { .. }
        ));
        assert!(matches!(
            normalize_option_lifecycle_activity(&base(
                "OPTRD",
                Some("AAPL"),
                Some("200"),
                None,
                Some("2023-07-21")
            ))
            .unwrap_err(),
            OptionLifecycleNormalizeError::MissingPriceForPairedTrade { .. }
        ));
    }

    #[test]
    fn unknown_activity_type_fails_closed() {
        let raw = activity(
            "id",
            "DIV",
            Some("AAPL"),
            Some("1"),
            None,
            "1",
            Some("2023-07-21"),
        );
        assert!(matches!(
            normalize_option_lifecycle_activity(&raw).unwrap_err(),
            OptionLifecycleNormalizeError::UnknownActivityType { .. }
        ));
    }

    #[test]
    fn wire_shape_deserializes_the_documented_rest_examples() {
        let json = r#"[
          {"id":"20190801011955195::5f596936","activity_type":"OPASN","date":"2023-07-01",
           "net_amount":"0","description":"Option Assignment","symbol":"AAPL230721C00150000",
           "qty":"2","status":"executed"},
          {"id":"20190801011955195::5f596936","activity_type":"OPTRD","date":"2023-07-01",
           "net_amount":"30000","description":"Option Trade","symbol":"AAPL","qty":"-200",
           "price":"150","status":"executed"}]"#;
        let rows: Vec<AlpacaOptionLifecycleActivity> = serde_json::from_str(json).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|r| r.group_id.is_none() && r.ref_id.is_none()));
        for r in &rows {
            normalize_option_lifecycle_activity(r).unwrap();
        }
    }
}
