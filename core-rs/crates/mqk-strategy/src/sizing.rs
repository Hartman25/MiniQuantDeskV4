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

    /// The one cap engine: reduce a positive `requested` target by the hard
    /// quantity cap and the hard notional cap evaluated at
    /// `reference_close_micros`. Caps only ever reduce; a non-positive
    /// reference price with a notional cap fails closed to zero.
    ///
    /// Returns `(effective_target, capped_by)` where `capped_by` is one of
    /// `"none"`, `"max_qty"`, `"max_notional"`, `"max_notional_no_price"`.
    pub fn apply_caps(
        &self,
        requested: QtyMicros,
        reference_close_micros: i64,
    ) -> (QtyMicros, &'static str) {
        let (after_qty_cap, qty_cap_fired) = match self.max_target_qty {
            Some(max_qty) if requested > max_qty => (max_qty, true),
            _ => (requested, false),
        };

        let (effective, capped_by) = match self.max_notional_usd {
            None => (
                after_qty_cap,
                if qty_cap_fired { "max_qty" } else { "none" },
            ),
            Some(max_notional_usd) => {
                if reference_close_micros <= 0 {
                    return (QtyMicros::ZERO, "max_notional_no_price");
                }
                // max qty (raw micros) = usd * 1e6 (usd micros) * QTY_MICROS_SCALE
                //                        / close_micros, floored to the asset
                // class's cap granularity (whole shares for Equity), never up.
                // Example: max=$1000, close=$200 → 5 shares (5_000_000 micros).
                let raw = (max_notional_usd as i128 * 1_000_000i128 * QTY_MICROS_SCALE as i128)
                    / reference_close_micros as i128;
                let floor = self.notional_cap_floor_micros() as i128;
                let floored = (raw / floor) * floor;
                let max_from_notional =
                    QtyMicros::new(i64::try_from(floored).unwrap_or(i64::MAX)).max(QtyMicros::ZERO);
                if after_qty_cap > max_from_notional {
                    (max_from_notional, "max_notional")
                } else if qty_cap_fired {
                    (after_qty_cap, "max_qty")
                } else {
                    (after_qty_cap, "none")
                }
            }
        };

        (effective.max(QtyMicros::ZERO), capped_by)
    }
}

// ---------------------------------------------------------------------------
// Sizing policy: FixedInitialCapitalFractionV1
// ---------------------------------------------------------------------------

pub const SIZING_POLICY_FIXED_QUANTITY_V1: &str = "fixed_quantity_v1";
pub const SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1: &str =
    "fixed_initial_capital_fraction_v1";
pub const ALLOCATION_FRACTION_BPS_DENOMINATOR: i64 = 10_000;

/// Versioned strategy sizing policy.
///
/// `FixedQuantityV1` is the historical behavior (the existing fixed
/// `target_qty` / caps sizing) and is the default. It contributes nothing to
/// any canonical identity string.
///
/// `FixedInitialCapitalFractionV1` sizes a flat->long entry as
/// `floor(initial_allocated_capital * allocation_fraction_bps / 10_000)` of
/// budget at the causal close of the completed entry bar (see
/// [`resolve_capital_fraction_target`]). The fraction has no default and no
/// float representation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SizingPolicy {
    #[default]
    FixedQuantityV1,
    FixedInitialCapitalFractionV1 {
        allocation_fraction_bps: i64,
    },
}

impl SizingPolicy {
    /// Construct the capital-fraction policy; refuses a fraction outside
    /// `1..=10_000`.
    pub fn capital_fraction_v1(
        allocation_fraction_bps: i64,
    ) -> Result<Self, CapitalFractionRefusal> {
        let p = Self::FixedInitialCapitalFractionV1 {
            allocation_fraction_bps,
        };
        p.validate()?;
        Ok(p)
    }

    pub fn policy_id(&self) -> &'static str {
        match self {
            Self::FixedQuantityV1 => SIZING_POLICY_FIXED_QUANTITY_V1,
            Self::FixedInitialCapitalFractionV1 { .. } => {
                SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1
            }
        }
    }

    pub fn allocation_fraction_bps(&self) -> Option<i64> {
        match self {
            Self::FixedQuantityV1 => None,
            Self::FixedInitialCapitalFractionV1 {
                allocation_fraction_bps,
            } => Some(*allocation_fraction_bps),
        }
    }

    pub fn is_capital_fraction(&self) -> bool {
        matches!(self, Self::FixedInitialCapitalFractionV1 { .. })
    }

    pub fn validate(&self) -> Result<(), CapitalFractionRefusal> {
        match self {
            Self::FixedQuantityV1 => Ok(()),
            Self::FixedInitialCapitalFractionV1 {
                allocation_fraction_bps,
            } => {
                if (1..=ALLOCATION_FRACTION_BPS_DENOMINATOR).contains(allocation_fraction_bps) {
                    Ok(())
                } else {
                    Err(CapitalFractionRefusal::InvalidAllocationFractionBps {
                        bps: *allocation_fraction_bps,
                    })
                }
            }
        }
    }

    /// Canonical identity suffix. EMPTY for `FixedQuantityV1`, so every
    /// historical canonical string stays byte-identical; non-empty only when
    /// the new policy is selected.
    pub fn canonical_suffix(&self) -> String {
        match self {
            Self::FixedQuantityV1 => String::new(),
            Self::FixedInitialCapitalFractionV1 {
                allocation_fraction_bps,
            } => format!(
                "|sz_policy={SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1}|sz_frac_bps={allocation_fraction_bps}"
            ),
        }
    }
}

/// Deterministic refusal reasons for capital-fraction sizing. Never falls
/// back to a default quantity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapitalFractionRefusal {
    InvalidAllocationFractionBps {
        bps: i64,
    },
    NonPositiveInitialCapital {
        initial_allocated_capital_micros: i64,
    },
    NonPositiveReferencePrice {
        causal_reference_price_micros: i64,
    },
    UnsupportedAssetClass {
        asset_class: AssetClass,
    },
    BudgetOverflow,
    NonPositiveBudget {
        position_budget_micros: i64,
    },
    InsufficientBudgetForMinimumQuantity {
        position_budget_micros: i64,
        causal_reference_price_micros: i64,
    },
    CapsReducedBelowMinimumQuantity {
        capped_by: &'static str,
    },
    NoCompletedReferenceBar,
}

impl CapitalFractionRefusal {
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::InvalidAllocationFractionBps { .. } => "invalid_allocation_fraction_bps",
            Self::NonPositiveInitialCapital { .. } => "non_positive_initial_capital",
            Self::NonPositiveReferencePrice { .. } => "non_positive_reference_price",
            Self::UnsupportedAssetClass { .. } => "unsupported_asset_class",
            Self::BudgetOverflow => "budget_overflow",
            Self::NonPositiveBudget { .. } => "non_positive_budget",
            Self::InsufficientBudgetForMinimumQuantity { .. } => {
                "insufficient_budget_for_minimum_quantity"
            }
            Self::CapsReducedBelowMinimumQuantity { .. } => "caps_reduced_below_minimum_quantity",
            Self::NoCompletedReferenceBar => "no_completed_reference_bar",
        }
    }
}

impl std::fmt::Display for CapitalFractionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {self:?}", self.reason_code())
    }
}

impl std::error::Error for CapitalFractionRefusal {}

/// Provenance of one capital-fraction resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapitalFractionResolution {
    pub sizing_policy_id: &'static str,
    pub allocation_fraction_bps: i64,
    pub initial_allocated_capital_micros: i64,
    pub position_budget_micros: i64,
    pub causal_reference_price_micros: i64,
    /// `floor(budget / price)` before caps.
    pub uncapped_target_qty: QtyMicros,
    pub resolved_target_qty: QtyMicros,
    pub max_target_qty: Option<QtyMicros>,
    pub max_notional_usd: Option<i64>,
    pub capped_by: &'static str,
}

/// The single pure capital-fraction resolver shared by Research, Backtest,
/// scanner and Paper.
///
/// * `budget = floor(initial_allocated_capital * bps / 10_000)` (checked i128).
/// * `Q = floor(budget / causal_reference_price)` in whole Equity shares; never
///   rounded up, never raised to 1.
/// * `caps` (a [`TargetSizing`], whose `target_qty` is ignored) can only reduce
///   `Q`, through the one cap engine [`TargetSizing::apply_caps`].
/// * Asset class comes from `caps.asset_class()` (registry truth), never from a
///   symbol. Only Equity is supported; every other class is refused.
pub fn resolve_capital_fraction_target(
    allocation_fraction_bps: i64,
    initial_allocated_capital_micros: i64,
    causal_reference_price_micros: i64,
    caps: &TargetSizing,
) -> Result<CapitalFractionResolution, CapitalFractionRefusal> {
    SizingPolicy::FixedInitialCapitalFractionV1 {
        allocation_fraction_bps,
    }
    .validate()?;
    if caps.asset_class() != AssetClass::Equity {
        return Err(CapitalFractionRefusal::UnsupportedAssetClass {
            asset_class: caps.asset_class(),
        });
    }
    if initial_allocated_capital_micros <= 0 {
        return Err(CapitalFractionRefusal::NonPositiveInitialCapital {
            initial_allocated_capital_micros,
        });
    }
    if causal_reference_price_micros <= 0 {
        return Err(CapitalFractionRefusal::NonPositiveReferencePrice {
            causal_reference_price_micros,
        });
    }

    let budget_wide = (initial_allocated_capital_micros as i128)
        .checked_mul(allocation_fraction_bps as i128)
        .ok_or(CapitalFractionRefusal::BudgetOverflow)?
        / ALLOCATION_FRACTION_BPS_DENOMINATOR as i128;
    let position_budget_micros =
        i64::try_from(budget_wide).map_err(|_| CapitalFractionRefusal::BudgetOverflow)?;
    if position_budget_micros <= 0 {
        return Err(CapitalFractionRefusal::NonPositiveBudget {
            position_budget_micros,
        });
    }

    let whole_units = position_budget_micros as i128 / causal_reference_price_micros as i128;
    if whole_units < 1 {
        return Err(
            CapitalFractionRefusal::InsufficientBudgetForMinimumQuantity {
                position_budget_micros,
                causal_reference_price_micros,
            },
        );
    }
    let uncapped_micros = whole_units
        .checked_mul(QTY_MICROS_SCALE as i128)
        .and_then(|v| i64::try_from(v).ok())
        .ok_or(CapitalFractionRefusal::BudgetOverflow)?;
    let uncapped_target_qty = QtyMicros::new(uncapped_micros);

    let (resolved_target_qty, capped_by) =
        caps.apply_caps(uncapped_target_qty, causal_reference_price_micros);
    if resolved_target_qty.raw() < QTY_MICROS_SCALE {
        return Err(CapitalFractionRefusal::CapsReducedBelowMinimumQuantity { capped_by });
    }

    Ok(CapitalFractionResolution {
        sizing_policy_id: SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1,
        allocation_fraction_bps,
        initial_allocated_capital_micros,
        position_budget_micros,
        causal_reference_price_micros,
        uncapped_target_qty,
        resolved_target_qty,
        max_target_qty: caps.max_target_qty(),
        max_notional_usd: caps.max_notional_usd(),
        capped_by,
    })
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
