//! Exact strategy target sizing.
//!
//! A target quantity is the actual asset quantity, carried as
//! [`QtyMicros`] (`1` share == `QTY_MICROS_SCALE`). Equity keeps its
//! historical whole-share semantics (absent/invalid config => 1 share).
//! Crypto has no default: an explicit, exact decimal size (<= 6 fractional
//! digits) is mandatory, and any missing/blank/malformed/non-positive input
//! is refused rather than defaulted.
//!
//! Asset class is an input supplied by the caller from registry truth; this
//! module never infers it from a symbol.

use mqk_execution::{AssetClass, QtyMicros, QTY_MICROS_SCALE};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SizingError {
    /// Non-Equity asset class requires an explicit size and none was given.
    MissingExplicitSize { asset_class: AssetClass },
    /// Not an exact decimal with at most 6 fractional digits.
    Malformed { field: &'static str, raw: String },
    /// Zero or negative where a positive quantity is required.
    NonPositive { field: &'static str },
    /// Value is not a multiple of the venue quantity increment.
    NotIncrementMultiple {
        field: &'static str,
        increment_micros: i64,
    },
    /// Value is below the venue minimum trade quantity.
    BelowMinTradeQty { min_trade_qty_micros: i64 },
    /// Increment / minimum supplied for validation is itself invalid.
    InvalidEconomics { field: &'static str },
    /// Asset class has no supported target-sizing policy.
    UnsupportedAssetClass { asset_class: AssetClass },
}

impl std::fmt::Display for SizingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for SizingError {}

/// Parse an exact positive decimal quantity (no floats, <= 6 decimals).
pub fn parse_positive_qty(field: &'static str, raw: &str) -> Result<QtyMicros, SizingError> {
    let q: QtyMicros = raw.trim().parse().map_err(|_| SizingError::Malformed {
        field,
        raw: raw.to_string(),
    })?;
    if q.is_positive() {
        Ok(q)
    } else {
        Err(SizingError::NonPositive { field })
    }
}

/// Resolved, validated strategy sizing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetSizing {
    asset_class: AssetClass,
    target_qty: QtyMicros,
    max_target_qty: Option<QtyMicros>,
    max_notional_usd: Option<i64>,
}

impl TargetSizing {
    /// Equity/ETF: `target` whole shares (values below 1 are raised to 1, as
    /// before), optional whole-share cap and whole-USD notional cap.
    pub fn equity_whole_units(
        target: i64,
        max_target: Option<i64>,
        max_notional_usd: Option<i64>,
    ) -> Result<Self, SizingError> {
        let target_qty =
            QtyMicros::from_whole_units(target.max(1)).ok_or_else(|| SizingError::Malformed {
                field: "target_qty",
                raw: target.to_string(),
            })?;
        let max_target_qty = match max_target {
            Some(m) => {
                Some(
                    QtyMicros::from_whole_units(m).ok_or_else(|| SizingError::Malformed {
                        field: "max_target_qty",
                        raw: m.to_string(),
                    })?,
                )
            }
            None => None,
        };
        Ok(Self {
            asset_class: AssetClass::Equity,
            target_qty,
            max_target_qty,
            max_notional_usd,
        })
    }

    /// One whole share, no caps: the historical Equity default.
    pub fn equity_default() -> Self {
        Self {
            asset_class: AssetClass::Equity,
            target_qty: QtyMicros::new(QTY_MICROS_SCALE),
            max_target_qty: None,
            max_notional_usd: None,
        }
    }

    /// Resolve sizing from raw operator inputs for `asset_class`.
    ///
    /// Equity: exactly the historical env semantics (absent, blank, zero,
    /// negative or non-integer target => 1 share; invalid caps => no cap).
    /// Crypto: target is mandatory and every supplied value must parse
    /// exactly; nothing is defaulted.
    pub fn resolve(
        asset_class: AssetClass,
        raw_target: Option<&str>,
        raw_max_target: Option<&str>,
        raw_max_notional_usd: Option<&str>,
    ) -> Result<Self, SizingError> {
        match asset_class {
            AssetClass::Equity => {
                let whole = |raw: Option<&str>| {
                    raw.and_then(|v| v.trim().parse::<i64>().ok())
                        .filter(|&q| q > 0)
                };
                Self::equity_whole_units(
                    whole(raw_target).unwrap_or(1),
                    whole(raw_max_target),
                    whole(raw_max_notional_usd),
                )
            }
            AssetClass::Crypto => {
                let target = match raw_target.map(str::trim) {
                    None | Some("") => {
                        return Err(SizingError::MissingExplicitSize { asset_class })
                    }
                    Some(raw) => parse_positive_qty("target_qty", raw)?,
                };
                let max_target_qty = match raw_max_target.map(str::trim) {
                    None | Some("") => None,
                    Some(raw) => Some(parse_positive_qty("max_target_qty", raw)?),
                };
                let max_notional_usd = match raw_max_notional_usd.map(str::trim) {
                    None | Some("") => None,
                    Some(raw) => {
                        Some(raw.parse::<i64>().ok().filter(|&n| n > 0).ok_or_else(|| {
                            SizingError::Malformed {
                                field: "max_notional_usd",
                                raw: raw.to_string(),
                            }
                        })?)
                    }
                };
                Ok(Self {
                    asset_class,
                    target_qty: target,
                    max_target_qty,
                    max_notional_usd,
                })
            }
            other => Err(SizingError::UnsupportedAssetClass { asset_class: other }),
        }
    }

    /// Validate explicit sizing against registry-v2 economics. Never chooses
    /// or rounds the size: a non-conforming value is refused.
    pub fn validate_against(
        &self,
        quantity_increment_micros: i64,
        min_trade_qty_micros: i64,
    ) -> Result<(), SizingError> {
        if quantity_increment_micros <= 0 {
            return Err(SizingError::InvalidEconomics {
                field: "quantity_increment_micros",
            });
        }
        if min_trade_qty_micros < 0 {
            return Err(SizingError::InvalidEconomics {
                field: "min_trade_qty_micros",
            });
        }
        let check = |field: &'static str, q: QtyMicros| {
            if q.raw() % quantity_increment_micros != 0 {
                Err(SizingError::NotIncrementMultiple {
                    field,
                    increment_micros: quantity_increment_micros,
                })
            } else {
                Ok(())
            }
        };
        check("target_qty", self.target_qty)?;
        if let Some(m) = self.max_target_qty {
            check("max_target_qty", m)?;
        }
        if self.target_qty.raw() < min_trade_qty_micros {
            return Err(SizingError::BelowMinTradeQty {
                min_trade_qty_micros,
            });
        }
        Ok(())
    }

    pub fn asset_class(&self) -> AssetClass {
        self.asset_class
    }

    pub fn target_qty(&self) -> QtyMicros {
        self.target_qty
    }

    pub fn max_target_qty(&self) -> Option<QtyMicros> {
        self.max_target_qty
    }

    pub fn max_notional_usd(&self) -> Option<i64> {
        self.max_notional_usd
    }

    /// Granularity a notional-derived cap is floored to: whole units for
    /// Equity (historical), one micro for Crypto. Always floors, never
    /// rounds up.
    pub fn notional_cap_floor_micros(&self) -> i64 {
        match self.asset_class {
            AssetClass::Equity => QTY_MICROS_SCALE,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equity_legacy_semantics_are_preserved() {
        let d = TargetSizing::resolve(AssetClass::Equity, None, None, None).unwrap();
        assert_eq!(d.target_qty(), QtyMicros::new(QTY_MICROS_SCALE));
        assert_eq!(d, TargetSizing::equity_default());
        for bad in ["", " ", "0", "-3", "abc", "2.5", "1.0"] {
            let s =
                TargetSizing::resolve(AssetClass::Equity, Some(bad), Some(bad), Some(bad)).unwrap();
            assert_eq!(s.target_qty(), QtyMicros::new(QTY_MICROS_SCALE), "{bad:?}");
            assert_eq!(s.max_target_qty(), None, "{bad:?}");
            assert_eq!(s.max_notional_usd(), None, "{bad:?}");
        }
        let s =
            TargetSizing::resolve(AssetClass::Equity, Some(" 7 "), Some("3"), Some("900")).unwrap();
        assert_eq!(s.target_qty(), QtyMicros::new(7 * QTY_MICROS_SCALE));
        assert_eq!(
            s.max_target_qty(),
            Some(QtyMicros::new(3 * QTY_MICROS_SCALE))
        );
        assert_eq!(s.max_notional_usd(), Some(900));
        assert_eq!(s.notional_cap_floor_micros(), QTY_MICROS_SCALE);
    }

    #[test]
    fn crypto_decimal_sizing_is_exact() {
        let s = TargetSizing::resolve(AssetClass::Crypto, Some("0.0001"), None, None).unwrap();
        assert_eq!(s.target_qty().raw(), 100);
        let s = TargetSizing::resolve(AssetClass::Crypto, Some("1"), None, None).unwrap();
        assert_eq!(s.target_qty().raw(), QTY_MICROS_SCALE);
        let s =
            TargetSizing::resolve(AssetClass::Crypto, Some("0.000001"), Some("2.5"), None).unwrap();
        assert_eq!(s.target_qty().raw(), 1);
        assert_eq!(s.max_target_qty().unwrap().raw(), 2_500_000);
        assert_eq!(s.notional_cap_floor_micros(), 1);
    }

    #[test]
    fn crypto_never_defaults_to_one_unit() {
        for missing in [None, Some(""), Some("   ")] {
            assert_eq!(
                TargetSizing::resolve(AssetClass::Crypto, missing, None, None),
                Err(SizingError::MissingExplicitSize {
                    asset_class: AssetClass::Crypto
                }),
                "{missing:?}"
            );
        }
    }

    #[test]
    fn crypto_refuses_malformed_zero_negative_overflow_and_excess_precision() {
        for bad in [
            "abc",
            "0.0000001",
            "1.2345678",
            "1e-4",
            "0x10",
            "1,5",
            "0",
            "0.0",
            "-1",
            "9223372036855",
            "9223372036854775807",
            ".",
        ] {
            assert!(
                TargetSizing::resolve(AssetClass::Crypto, Some(bad), None, None).is_err(),
                "target {bad:?} must be refused"
            );
        }
        // Supplied-but-bad caps are refused, never silently dropped.
        for bad in ["x", "0", "-2", "0.0000001"] {
            assert!(
                TargetSizing::resolve(AssetClass::Crypto, Some("1"), Some(bad), None).is_err(),
                "cap {bad:?}"
            );
        }
        for bad in ["x", "0", "-5", "1.5"] {
            assert!(
                TargetSizing::resolve(AssetClass::Crypto, Some("1"), None, Some(bad)).is_err(),
                "notional {bad:?}"
            );
        }
    }

    #[test]
    fn unsupported_asset_classes_fail_closed() {
        for ac in [AssetClass::Option, AssetClass::Future, AssetClass::Forex] {
            assert_eq!(
                TargetSizing::resolve(ac, Some("1"), None, None),
                Err(SizingError::UnsupportedAssetClass { asset_class: ac })
            );
        }
    }

    #[test]
    fn economics_validate_but_never_choose_size() {
        let s = TargetSizing::resolve(AssetClass::Crypto, Some("0.0001"), Some("1"), None).unwrap();
        assert!(s.validate_against(100, 100).is_ok());
        assert_eq!(
            s.validate_against(1_000, 0),
            Err(SizingError::NotIncrementMultiple {
                field: "target_qty",
                increment_micros: 1_000
            })
        );
        assert_eq!(
            s.validate_against(1, 101),
            Err(SizingError::BelowMinTradeQty {
                min_trade_qty_micros: 101
            })
        );
        assert!(s.validate_against(0, 0).is_err());
        assert!(s.validate_against(1, -1).is_err());
        assert!(
            TargetSizing::resolve(AssetClass::Crypto, Some("0.5"), Some("0.3333333"), None)
                .is_err(),
            "7 decimals refused"
        );
        let odd_cap =
            TargetSizing::resolve(AssetClass::Crypto, Some("0.5"), Some("0.333333"), None).unwrap();
        assert!(
            odd_cap.validate_against(1_000, 0).is_err(),
            "cap off-increment"
        );
    }
}
