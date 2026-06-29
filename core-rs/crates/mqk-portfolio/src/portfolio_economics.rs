//! ASSET-CORE-04C: pure, default-unused multi-asset portfolio NAV/exposure
//! aggregation model.
//!
//! This is the portfolio-level composition layer `ASSET-CORE-04A`'s module
//! docs anticipated (and its reserved
//! [`crate::InstrumentEconomicsTruthState::NavUnavailable`] variant
//! foreshadowed): it turns cash plus a set of already-valued
//! [`crate::PositionEconomicsValue`] rows (produced by
//! [`crate::value_position_economics`], optionally via `mqk-daemon`'s
//! `ASSET-CORE-04B` registry-v2 bridge) into a portfolio-level NAV, gross/net
//! exposure, per-position weight, and asset-class/currency exposure
//! breakdown.
//!
//! **Nothing in this module is called by `accounting.rs`, `metrics.rs`, or
//! `valuation.rs`.** [`crate::valuation::compute_portfolio_weights`] remains
//! the unmodified, authoritative live equity-accounting and live-weights
//! path. This module is additive and has zero production callers anywhere
//! in the workspace.
//!
//! # Relationship to existing seams
//!
//! - [`crate::valuation::compute_portfolio_weights`] (`PORTFOLIO-LIVE-WEIGHTS-01`)
//!   is this crate's *existing* portfolio-level seam: signed whole-share
//!   quantities + per-symbol marks -> NAV/weights, implicitly multiplier=1,
//!   implicitly single-currency, `i64` cash, saturating arithmetic. This
//!   module is the *multiplier/currency-aware* portfolio-level seam:
//!   already-valued [`crate::PositionEconomicsValue`] rows (which carry
//!   their own multiplier/currency/asset-class truth) -> the same
//!   NAV/gross/net/weight shape, but `i128` cash and explicit
//!   (non-saturating) overflow reporting. The two are intentionally
//!   parallel, not unified -- unifying them would mean changing
//!   `compute_portfolio_weights`'s live behavior, which this patch must not
//!   do.
//! - [`crate::value_position_economics`] (`ASSET-CORE-04A`) values exactly
//!   one position. This module composes many already-valued positions into
//!   one portfolio snapshot, the way `compute_portfolio_weights` already
//!   does for equities -- but never re-derives or re-checks a position's
//!   *own* multiplier/mark math; it only trusts (or refuses to trust) the
//!   [`crate::PositionEconomicsValue`] it is handed.
//! - `mqk-daemon`'s `ASSET-CORE-04B` bridge
//!   (`instrument_economics_bridge::bridge_instrument_registry_v2_to_economics`)
//!   produces the [`crate::InstrumentEconomics`] rows that, once valued by
//!   [`crate::value_position_economics`], become this module's input. No
//!   dependency edge exists in either direction: this module never calls
//!   the bridge (it lives in `mqk-daemon`, a downstream crate), and this
//!   module's own zero-Cargo-dependency posture (see `mqk-portfolio/Cargo.toml`)
//!   is unaffected.
//!
//! # Design
//!
//! [`aggregate_portfolio_economics`] is the one pure function. It never
//! fabricates a NAV, a weight, or a cross-currency conversion: every
//! refusal returns an explicit [`PortfolioEconomicsTruthState`] and
//! `reason_code`, mirroring the check-order discipline
//! [`crate::value_position_economics`] established. All arithmetic is
//! integer, deterministic, and overflow-checked via `i128` (never `f64`,
//! never silent saturation) -- a stricter posture than
//! `compute_portfolio_weights`'s saturating sums, justified by this
//! module's explicit `Overflow` truth state. No IO, no time source, no
//! broker/provider/DB access, no RNG.

use std::collections::BTreeMap;

use crate::instrument_economics::{InstrumentEconomicsTruthState, PositionEconomicsValue};

/// Truthful outcome of [`aggregate_portfolio_economics`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortfolioEconomicsTruthState {
    /// Every position (if any) is currency-consistent and value-available,
    /// and `nav_micros > 0`. NAV, gross/net exposure, every position's
    /// weight, and both exposure breakdowns are computed.
    Active,
    /// `positions` is empty and `nav_micros` (== `cash_micros`) is positive.
    /// NAV/gross/net are computed (gross/net are trivially `0`); there is
    /// nothing to weight or break down by asset class/currency.
    NoPositions,
    /// At least one position has `quote_currency == account_currency` (so
    /// it is not a currency refusal) but is not itself
    /// [`InstrumentEconomicsTruthState::Active`], or is missing a notional
    /// value. A partial NAV would imply a value that was never confirmed,
    /// so NAV, gross/net exposure, every weight, and both exposure
    /// breakdowns are all `None`/empty -- mirroring
    /// [`crate::valuation::PortfolioWeightsSnapshot`]'s `"missing_marks"`
    /// precedent. Each position's own row still reports whatever the
    /// underlying [`PositionEconomicsValue`] knows, per-row, for
    /// transparency.
    PositionValueUnavailable,
    /// Either `account_currency` itself is empty, or at least one position's
    /// `quote_currency` differs from the portfolio's `account_currency`.
    /// This model never fakes FX conversion -- the caller must resolve
    /// currency before calling, or accept this refusal. NAV, gross/net
    /// exposure, every weight, and both exposure breakdowns are all
    /// `None`/empty.
    CurrencyConversionUnsupported,
    /// Every position is currency-consistent and value-available, but
    /// `nav_micros <= 0`. Gross/net exposure and both exposure breakdowns
    /// (without weights) are still computed -- only a *weight* (a fraction
    /// of NAV) is meaningless against non-positive NAV. Mirrors
    /// `compute_portfolio_weights`'s `"nav_unavailable"` precedent exactly.
    NavUnavailable,
    /// Summing NAV, gross exposure, net exposure, or a per-asset-class/
    /// per-currency subtotal could not be represented in `i128`. NAV,
    /// gross/net exposure, every weight, and both exposure breakdowns are
    /// all `None`/empty.
    Overflow,
}

/// One portfolio's cash plus a set of already-valued positions to aggregate.
///
/// `cash_micros` and every position's notional share this crate's `*_micros`
/// (1e-6) fixed-point convention. Unlike
/// [`crate::valuation::compute_portfolio_weights`] (which takes `i64` cash),
/// this model uses `i128` cash throughout -- consistent with
/// [`PositionEconomicsValue::notional_micros`] already being `i128`-scaled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioEconomicsInput {
    pub cash_micros: i128,
    pub account_currency: String,
    pub positions: Vec<PositionEconomicsValue>,
}

/// One position's row in a [`PortfolioEconomicsSnapshot`].
///
/// `notional_micros`/`absolute_notional_micros` are passed through from the
/// underlying [`PositionEconomicsValue`] verbatim whenever it computed one,
/// regardless of this row's own `truth_state` -- a position's own truth is
/// never hidden, even when the *snapshot* refuses to aggregate it (mirrors
/// `compute_portfolio_weights`'s `PositionWeightRow` precedent: "known
/// marks are still surfaced even though the snapshot-wide NAV is blocked").
/// `weight_bps` is the one field gated on the *snapshot's* truth state: it
/// is `Some` only when the overall snapshot is
/// [`PortfolioEconomicsTruthState::Active`] or
/// [`PortfolioEconomicsTruthState::NoPositions`] (vacuously, for the latter).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioEconomicsPositionRow {
    pub instrument_id: String,
    pub symbol: String,
    pub asset_class: String,
    pub quote_currency: String,
    pub signed_qty_micros: i64,
    pub notional_micros: Option<i128>,
    pub absolute_notional_micros: Option<i128>,
    /// Absolute (always non-negative) weight in basis points of NAV --
    /// `absolute_notional_micros / nav_micros * 10_000`. Deliberately
    /// magnitude-only (unlike `compute_portfolio_weights`'s signed
    /// `weight_bps`): sign is already conveyed by `notional_micros` and
    /// `signed_qty_micros`.
    pub weight_bps: Option<i64>,
    /// `"active"` | `"currency_conversion_unsupported"` |
    /// `"position_value_unavailable"`. This row's own classification,
    /// independent of whether the snapshot as a whole could aggregate it.
    pub truth_state: String,
    pub reason_code: String,
}

/// One row of a [`PortfolioEconomicsSnapshot::asset_class_exposures`] or
/// [`PortfolioEconomicsSnapshot::currency_exposures`] breakdown.
///
/// `key` is the `asset_class` or `quote_currency` string this row
/// aggregates. `weight_bps` is `Some` under exactly the same condition as
/// [`PortfolioEconomicsPositionRow::weight_bps`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioEconomicsExposureRow {
    pub key: String,
    pub signed_notional_micros: i128,
    pub absolute_notional_micros: i128,
    pub weight_bps: Option<i64>,
}

/// Truthful, point-in-time portfolio economics snapshot. See
/// [`PortfolioEconomicsTruthState`] for the full truth-state contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioEconomicsSnapshot {
    pub account_currency: String,
    pub cash_micros: i128,
    pub nav_micros: Option<i128>,
    pub gross_exposure_micros: Option<i128>,
    pub net_exposure_micros: Option<i128>,
    pub positions: Vec<PortfolioEconomicsPositionRow>,
    pub asset_class_exposures: Vec<PortfolioEconomicsExposureRow>,
    pub currency_exposures: Vec<PortfolioEconomicsExposureRow>,
    pub truth_state: PortfolioEconomicsTruthState,
    pub reason_code: String,
}

fn clamp_i128_to_i64(x: i128) -> i64 {
    if x > i64::MAX as i128 {
        i64::MAX
    } else if x < i64::MIN as i128 {
        i64::MIN
    } else {
        x as i64
    }
}

/// Absolute (non-negative) weight in basis points of `nav_micros`. Caller
/// must ensure `nav_micros > 0`. Mirrors `valuation.rs`'s
/// `weight_bps_of_nav` overflow fallback exactly (divide-then-multiply when
/// the `* 10_000` step would overflow `i128`), adapted to an always-positive
/// input rather than a signed one.
fn weight_bps_of_nav(absolute_notional_micros: i128, nav_micros: i128) -> i64 {
    debug_assert!(nav_micros > 0, "weight_bps_of_nav requires nav_micros > 0");
    let bps = match absolute_notional_micros.checked_mul(10_000) {
        Some(scaled) => scaled / nav_micros,
        None => (absolute_notional_micros / nav_micros).saturating_mul(10_000),
    };
    clamp_i128_to_i64(bps)
}

/// Checked accumulate of `(signed, absolute)` into `map[key]`. Returns
/// `false` (leaving `map` partially updated but never panicking) the moment
/// either component would overflow `i128`, so the caller can fail closed
/// with an explicit [`PortfolioEconomicsTruthState::Overflow`].
fn checked_accumulate(
    map: &mut BTreeMap<String, (i128, i128)>,
    key: String,
    signed: i128,
    absolute: i128,
) -> bool {
    let entry = map.entry(key).or_insert((0, 0));
    match (entry.0.checked_add(signed), entry.1.checked_add(absolute)) {
        (Some(s), Some(a)) => {
            entry.0 = s;
            entry.1 = a;
            true
        }
        _ => false,
    }
}

/// Converts an accumulated `BTreeMap` into deterministic (sorted-by-key)
/// exposure rows. `nav_for_weight` is `Some(nav)` (`nav > 0`) to fill in
/// `weight_bps`, or `None` to leave every row's `weight_bps` unset.
fn exposure_rows_from_map(
    map: BTreeMap<String, (i128, i128)>,
    nav_for_weight: Option<i128>,
) -> Vec<PortfolioEconomicsExposureRow> {
    map.into_iter()
        .map(
            |(key, (signed_notional_micros, absolute_notional_micros))| {
                let weight_bps =
                    nav_for_weight.map(|nav| weight_bps_of_nav(absolute_notional_micros, nav));
                PortfolioEconomicsExposureRow {
                    key,
                    signed_notional_micros,
                    absolute_notional_micros,
                    weight_bps,
                }
            },
        )
        .collect()
}

fn empty_snapshot(
    account_currency: String,
    cash_micros: i128,
    positions: Vec<PortfolioEconomicsPositionRow>,
    truth_state: PortfolioEconomicsTruthState,
    reason_code: &str,
) -> PortfolioEconomicsSnapshot {
    PortfolioEconomicsSnapshot {
        account_currency,
        cash_micros,
        nav_micros: None,
        gross_exposure_micros: None,
        net_exposure_micros: None,
        positions,
        asset_class_exposures: Vec::new(),
        currency_exposures: Vec::new(),
        truth_state,
        reason_code: reason_code.to_string(),
    }
}

/// Aggregate cash plus a set of already-valued positions into a
/// portfolio-level NAV/exposure snapshot. Never fabricates a NAV, a weight,
/// or a currency conversion.
///
/// Check order (first match wins -- documented so multi-failure inputs are
/// deterministic):
///
/// 1. Empty `account_currency` -> [`CurrencyConversionUnsupported`](PortfolioEconomicsTruthState::CurrencyConversionUnsupported).
/// 2. Any position's `quote_currency != account_currency` -> [`CurrencyConversionUnsupported`](PortfolioEconomicsTruthState::CurrencyConversionUnsupported).
/// 3. Any (currency-consistent) position not [`InstrumentEconomicsTruthState::Active`]
///    or missing a notional -> [`PositionValueUnavailable`](PortfolioEconomicsTruthState::PositionValueUnavailable).
/// 4. Summing NAV/gross/net or any per-asset-class/per-currency subtotal
///    overflows `i128` -> [`Overflow`](PortfolioEconomicsTruthState::Overflow).
/// 5. `nav_micros <= 0` -> [`NavUnavailable`](PortfolioEconomicsTruthState::NavUnavailable);
///    weights are not computed (gross/net/breakdowns still are).
/// 6. `positions` is empty -> [`NoPositions`](PortfolioEconomicsTruthState::NoPositions).
/// 7. Otherwise -> [`Active`](PortfolioEconomicsTruthState::Active).
///
/// Every position row always echoes whatever the underlying
/// [`PositionEconomicsValue`] knows (`notional_micros`,
/// `absolute_notional_micros`) regardless of which check fired -- only the
/// snapshot-level aggregates (NAV, gross/net exposure, weights, exposure
/// breakdowns) are withheld on refusal.
pub fn aggregate_portfolio_economics(input: PortfolioEconomicsInput) -> PortfolioEconomicsSnapshot {
    let PortfolioEconomicsInput {
        cash_micros,
        account_currency,
        positions,
    } = input;

    if account_currency.trim().is_empty() {
        return empty_snapshot(
            account_currency,
            cash_micros,
            Vec::new(),
            PortfolioEconomicsTruthState::CurrencyConversionUnsupported,
            "empty_account_currency",
        );
    }

    // Pass 1: classify every position; detect currency mismatch / value
    // unavailability before trusting any notional enough to sum it.
    let mut rows: Vec<PortfolioEconomicsPositionRow> = Vec::with_capacity(positions.len());
    let mut any_currency_mismatch = false;
    let mut any_value_unavailable = false;

    for p in &positions {
        let currency_mismatch = p.quote_currency != account_currency;
        let value_available = !currency_mismatch
            && p.truth_state == InstrumentEconomicsTruthState::Active
            && p.notional_micros.is_some()
            && p.absolute_notional_micros.is_some();

        let (row_truth_state, row_reason_code) = if currency_mismatch {
            any_currency_mismatch = true;
            (
                "currency_conversion_unsupported".to_string(),
                "quote_currency_does_not_match_portfolio_account_currency".to_string(),
            )
        } else if value_available {
            ("active".to_string(), "active".to_string())
        } else {
            any_value_unavailable = true;
            (
                "position_value_unavailable".to_string(),
                p.reason_code.clone(),
            )
        };

        rows.push(PortfolioEconomicsPositionRow {
            instrument_id: p.instrument_id.clone(),
            symbol: p.symbol.clone(),
            asset_class: p.asset_class.clone(),
            quote_currency: p.quote_currency.clone(),
            signed_qty_micros: p.signed_qty_micros,
            notional_micros: p.notional_micros,
            absolute_notional_micros: p.absolute_notional_micros,
            weight_bps: None,
            truth_state: row_truth_state,
            reason_code: row_reason_code,
        });
    }

    if any_currency_mismatch {
        return empty_snapshot(
            account_currency,
            cash_micros,
            rows,
            PortfolioEconomicsTruthState::CurrencyConversionUnsupported,
            "one_or_more_positions_have_unsupported_quote_currency",
        );
    }

    if any_value_unavailable {
        return empty_snapshot(
            account_currency,
            cash_micros,
            rows,
            PortfolioEconomicsTruthState::PositionValueUnavailable,
            "one_or_more_positions_have_unavailable_value",
        );
    }

    // Pass 2: every position (if any) is currency-consistent and
    // value-available. Sum NAV/gross/net and group by asset class/currency,
    // all overflow-checked together.
    let mut nav: i128 = cash_micros;
    let mut gross: i128 = 0;
    let mut by_asset_class: BTreeMap<String, (i128, i128)> = BTreeMap::new();
    let mut by_currency: BTreeMap<String, (i128, i128)> = BTreeMap::new();
    let mut overflowed = false;

    for p in &positions {
        let signed = p.notional_micros.unwrap_or(0);
        let absolute = p.absolute_notional_micros.unwrap_or(0);

        nav = match nav.checked_add(signed) {
            Some(v) => v,
            None => {
                overflowed = true;
                break;
            }
        };
        gross = match gross.checked_add(absolute) {
            Some(v) => v,
            None => {
                overflowed = true;
                break;
            }
        };
        if !checked_accumulate(&mut by_asset_class, p.asset_class.clone(), signed, absolute) {
            overflowed = true;
            break;
        }
        if !checked_accumulate(&mut by_currency, p.quote_currency.clone(), signed, absolute) {
            overflowed = true;
            break;
        }
    }

    if overflowed {
        return empty_snapshot(
            account_currency,
            cash_micros,
            rows,
            PortfolioEconomicsTruthState::Overflow,
            "nav_or_exposure_sum_overflowed_i128",
        );
    }

    let net = match nav.checked_sub(cash_micros) {
        Some(v) => v,
        None => {
            return empty_snapshot(
                account_currency,
                cash_micros,
                rows,
                PortfolioEconomicsTruthState::Overflow,
                "nav_or_exposure_sum_overflowed_i128",
            );
        }
    };

    if nav <= 0 {
        let asset_class_exposures = exposure_rows_from_map(by_asset_class, None);
        let currency_exposures = exposure_rows_from_map(by_currency, None);
        return PortfolioEconomicsSnapshot {
            account_currency,
            cash_micros,
            nav_micros: Some(nav),
            gross_exposure_micros: Some(gross),
            net_exposure_micros: Some(net),
            positions: rows,
            asset_class_exposures,
            currency_exposures,
            truth_state: PortfolioEconomicsTruthState::NavUnavailable,
            reason_code: "nav_micros_not_positive".to_string(),
        };
    }

    for row in &mut rows {
        if let Some(abs) = row.absolute_notional_micros {
            row.weight_bps = Some(weight_bps_of_nav(abs, nav));
        }
    }

    let asset_class_exposures = exposure_rows_from_map(by_asset_class, Some(nav));
    let currency_exposures = exposure_rows_from_map(by_currency, Some(nav));

    let (truth_state, reason_code) = if positions.is_empty() {
        (
            PortfolioEconomicsTruthState::NoPositions,
            "no_positions".to_string(),
        )
    } else {
        (PortfolioEconomicsTruthState::Active, "active".to_string())
    };

    PortfolioEconomicsSnapshot {
        account_currency,
        cash_micros,
        nav_micros: Some(nav),
        gross_exposure_micros: Some(gross),
        net_exposure_micros: Some(net),
        positions: rows,
        asset_class_exposures,
        currency_exposures,
        truth_state,
        reason_code,
    }
}
