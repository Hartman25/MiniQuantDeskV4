//! PORTFOLIO-LIVE-WEIGHTS-01: live position valuation & weight computation.
//!
//! This is a truth seam, not a pricing engine: it turns explicit signed
//! quantities and explicit, attributed mark prices into per-symbol market
//! value, NAV, and portfolio weight — or an honest refusal when an input is
//! missing. It never fabricates a mark, a NAV, or a weight.
//!
//! Pure, deterministic, no IO, no time source, no broker wiring — consistent
//! with the rest of this crate. Timestamps are caller-supplied epoch-second
//! integers (matching the `md_bars.end_ts` convention), not `DateTime`, so
//! this crate does not need a date/time dependency.
//!
//! Where marks come from (md_bars, a broker, a manual override, ...) is the
//! caller's concern and the caller's provenance to attach via
//! [`PositionMark::source`]. This module only knows how to combine marks
//! that are already there.

use std::collections::BTreeMap;

/// A single position's signed quantity.
///
/// Decoupled from any specific portfolio-state representation (no
/// dependency on runtime/broker types) so this module stays a pure seam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionWeightInput {
    pub symbol: String,
    /// Positive = long, negative = short, 0 = flat.
    pub signed_qty: i64,
}

/// An explicit mark price for one symbol, with provenance.
///
/// This module never constructs a `PositionMark` itself — every instance
/// must come from a caller-supplied, attributable source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionMark {
    pub symbol: String,
    pub mark_price_micros: i64,
    /// Epoch seconds (matches `md_bars.end_ts`). `None` if the source has no
    /// associated timestamp.
    pub mark_ts_utc: Option<i64>,
    /// Human-readable provenance, e.g. `"md_bars:1D:close"`. Never empty in
    /// practice — callers should describe exactly where this price came from.
    pub source: String,
}

/// Per-symbol computed valuation row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionWeightRow {
    pub symbol: String,
    pub signed_qty: i64,
    pub mark_price_micros: Option<i64>,
    pub mark_ts_utc: Option<i64>,
    pub mark_source: Option<String>,
    pub market_value_micros: Option<i128>,
    pub absolute_notional_micros: Option<i128>,
    /// Signed weight in basis points of NAV (`market_value / nav * 10_000`).
    /// `None` whenever the snapshot's `truth_state` is not `"active"`, even
    /// if this row's own mark is present — a weight is a portfolio-relative
    /// fact and is never computed against an unconfirmed or non-positive NAV.
    pub weight_bps: Option<i64>,
    /// `true` iff `signed_qty != 0` and no mark was supplied for this symbol.
    pub missing_mark: bool,
}

/// Truthful, point-in-time portfolio valuation snapshot.
///
/// `truth_state`:
/// - `"active"` — every non-flat position has a mark and NAV > 0; NAV,
///   gross exposure, and every row's weight are computed. This also covers
///   a flat / no-position portfolio, where NAV == cash.
/// - `"missing_marks"` — at least one non-flat position has no entry in the
///   supplied marks. NAV, gross exposure, and every row's weight are `None`
///   — a partial NAV would imply marks that were never confirmed. Rows
///   whose own mark *is* present still report `market_value_micros` /
///   `absolute_notional_micros` so callers can see what is actually known.
///   See `missing_mark_symbols` for the full list of culprits.
/// - `"nav_unavailable"` — every non-flat position has a mark, but
///   `cash_micros + sum(market_value_micros) <= 0`. A weight (fraction of
///   NAV) is meaningless against non-positive NAV, so every row's weight is
///   `None`. `nav_micros` and `gross_exposure_micros` are still reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortfolioWeightsSnapshot {
    pub truth_state: String,
    pub nav_micros: Option<i128>,
    pub gross_exposure_micros: Option<i128>,
    pub cash_micros: i64,
    pub positions: Vec<PositionWeightRow>,
    pub missing_mark_symbols: Vec<String>,
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

/// Signed weight in basis points of `nav_micros`. Caller must ensure
/// `nav_micros > 0`.
///
/// Exact for all realistic finance magnitudes. `market_value_micros *
/// 10_000` only fails to fit `i128` when `market_value_micros` itself
/// approaches `i64::MAX` squared in magnitude — economically impossible in
/// practice (it requires a single position's quantity *and* price, each in
/// micros, to simultaneously approach `i64::MAX`). In that case this falls
/// back to dividing first: exact whenever `market_value_micros` is a whole
/// multiple of `nav_micros` (e.g. a single fully-invested position), and a
/// deterministic, non-panicking, sub-bps-precision-losing result otherwise —
/// never a silent wrap or panic.
fn weight_bps_of_nav(market_value_micros: i128, nav_micros: i128) -> i64 {
    debug_assert!(nav_micros > 0, "weight_bps_of_nav requires nav_micros > 0");
    let bps = match market_value_micros.checked_mul(10_000) {
        Some(scaled) => scaled / nav_micros,
        None => (market_value_micros / nav_micros).saturating_mul(10_000),
    };
    clamp_i128_to_i64(bps)
}

/// Compute live per-symbol market values and portfolio weights from explicit
/// signed quantities and explicit marks. See [`PortfolioWeightsSnapshot`]
/// for the full truth-state contract.
///
/// A position with `signed_qty == 0` never requires a mark: its market value
/// is always `0`, deterministically, regardless of mark presence — there is
/// no exposure to misrepresent.
///
/// All products and sums are computed in `i128`. A single position's market
/// value (`i64 * i64`) can never overflow `i128` (max magnitude `2^126 <
/// 2^127`); sums of market values use saturating arithmetic as a defensive
/// backstop that can only matter for economically impossible inputs.
///
/// Callers must supply each symbol in `positions` at most once; behavior for
/// duplicate symbols is unspecified (the source `PortfolioState`/snapshot
/// types this is meant to consume are map-keyed by symbol, so this should
/// not occur in practice).
pub fn compute_portfolio_weights(
    cash_micros: i64,
    positions: &[PositionWeightInput],
    marks: &BTreeMap<String, PositionMark>,
) -> PortfolioWeightsSnapshot {
    let mut missing_mark_symbols: Vec<String> = Vec::new();
    let mut rows: Vec<PositionWeightRow> = Vec::with_capacity(positions.len());

    for p in positions {
        if p.signed_qty == 0 {
            let mark = marks.get(&p.symbol);
            rows.push(PositionWeightRow {
                symbol: p.symbol.clone(),
                signed_qty: 0,
                mark_price_micros: mark.map(|m| m.mark_price_micros),
                mark_ts_utc: mark.and_then(|m| m.mark_ts_utc),
                mark_source: mark.map(|m| m.source.clone()),
                market_value_micros: Some(0),
                absolute_notional_micros: Some(0),
                weight_bps: None,
                missing_mark: false,
            });
            continue;
        }

        match marks.get(&p.symbol) {
            Some(mark) => {
                let mv = (p.signed_qty as i128) * (mark.mark_price_micros as i128);
                rows.push(PositionWeightRow {
                    symbol: p.symbol.clone(),
                    signed_qty: p.signed_qty,
                    mark_price_micros: Some(mark.mark_price_micros),
                    mark_ts_utc: mark.mark_ts_utc,
                    mark_source: Some(mark.source.clone()),
                    market_value_micros: Some(mv),
                    absolute_notional_micros: Some(mv.abs()),
                    weight_bps: None,
                    missing_mark: false,
                });
            }
            None => {
                missing_mark_symbols.push(p.symbol.clone());
                rows.push(PositionWeightRow {
                    symbol: p.symbol.clone(),
                    signed_qty: p.signed_qty,
                    mark_price_micros: None,
                    mark_ts_utc: None,
                    mark_source: None,
                    market_value_micros: None,
                    absolute_notional_micros: None,
                    weight_bps: None,
                    missing_mark: true,
                });
            }
        }
    }

    if !missing_mark_symbols.is_empty() {
        return PortfolioWeightsSnapshot {
            truth_state: "missing_marks".to_string(),
            nav_micros: None,
            gross_exposure_micros: None,
            cash_micros,
            positions: rows,
            missing_mark_symbols,
        };
    }

    let mut nav: i128 = cash_micros as i128;
    let mut gross: i128 = 0;
    for r in &rows {
        nav = nav.saturating_add(r.market_value_micros.unwrap_or(0));
        gross = gross.saturating_add(r.absolute_notional_micros.unwrap_or(0));
    }

    if nav <= 0 {
        return PortfolioWeightsSnapshot {
            truth_state: "nav_unavailable".to_string(),
            nav_micros: Some(nav),
            gross_exposure_micros: Some(gross),
            cash_micros,
            positions: rows,
            missing_mark_symbols,
        };
    }

    for r in &mut rows {
        if let Some(mv) = r.market_value_micros {
            r.weight_bps = Some(weight_bps_of_nav(mv, nav));
        }
    }

    PortfolioWeightsSnapshot {
        truth_state: "active".to_string(),
        nav_micros: Some(nav),
        gross_exposure_micros: Some(gross),
        cash_micros,
        positions: rows,
        missing_mark_symbols,
    }
}

/// PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01B: unrealized P&L in micros for
/// one position, given its signed quantity, a single blended average cost
/// basis, and a mark price — all already in micros.
///
/// Formula: `(mark_price_micros - avg_price_micros) * signed_qty`. This one
/// formula is correct for both long (`signed_qty > 0`) and short
/// (`signed_qty < 0`) positions without branching — a short position's
/// negative qty naturally flips the sign to match the equivalent
/// `(avg_price_micros - mark_price_micros) * abs(signed_qty)` form. A flat
/// position (`signed_qty == 0`) always returns `0`, regardless of the mark
/// or avg price supplied.
///
/// Exact whenever `avg_price_micros` is the quantity-weighted average entry
/// price across lots — algebraically identical to summing
/// `(mark - entry_i) * qty_i` per lot (see
/// [`crate::compute_unrealized_pnl_micros`] in `metrics.rs`, which computes
/// the same quantity from full per-lot `PositionState`). This function
/// exists separately because the broker-snapshot route layer it serves
/// (`/api/v1/portfolio/positions`) only has a single blended `avg_price` per
/// position, not per-lot entry prices — `metrics::compute_unrealized_pnl_micros`
/// is not reusable there without exposing full lot state through that
/// layer, which this patch does not do.
///
/// Computed in `i128`: two `i64` factors multiplied can never overflow
/// `i128` (max magnitude `2^126 < 2^127`).
pub fn unrealized_pnl_micros(
    signed_qty: i64,
    avg_price_micros: i64,
    mark_price_micros: i64,
) -> i128 {
    (mark_price_micros as i128 - avg_price_micros as i128) * (signed_qty as i128)
}

#[cfg(test)]
mod pnl_tests {
    use super::unrealized_pnl_micros;

    #[test]
    fn long_position_mark_above_avg_is_positive_pnl() {
        // 10 long shares, avg 100.00, mark 110.00 -> +100.00 (in micros)
        let pnl = unrealized_pnl_micros(10, 100_000_000, 110_000_000);
        assert_eq!(pnl, 100_000_000);
    }

    #[test]
    fn long_position_mark_below_avg_is_negative_pnl() {
        // 10 long shares, avg 100.00, mark 90.00 -> -100.00
        let pnl = unrealized_pnl_micros(10, 100_000_000, 90_000_000);
        assert_eq!(pnl, -100_000_000);
    }

    #[test]
    fn short_position_mark_above_avg_is_negative_pnl() {
        // 10 short shares (signed_qty = -10), avg 100.00, mark 110.00 ->
        // loss for the short: -100.00
        let pnl = unrealized_pnl_micros(-10, 100_000_000, 110_000_000);
        assert_eq!(pnl, -100_000_000);
    }

    #[test]
    fn short_position_mark_below_avg_is_positive_pnl() {
        // 10 short shares, avg 100.00, mark 90.00 -> gain for the short: +100.00
        let pnl = unrealized_pnl_micros(-10, 100_000_000, 90_000_000);
        assert_eq!(pnl, 100_000_000);
    }

    #[test]
    fn flat_position_is_always_zero_regardless_of_prices() {
        assert_eq!(unrealized_pnl_micros(0, 100_000_000, 999_000_000), 0);
        assert_eq!(unrealized_pnl_micros(0, 0, 0), 0);
    }

    #[test]
    fn mark_equal_avg_is_zero_pnl() {
        assert_eq!(unrealized_pnl_micros(5, 314_810_000, 314_810_000), 0);
    }

    #[test]
    fn extreme_magnitudes_do_not_overflow_i128() {
        // Largest i64 qty and price factors -- must not panic, and the
        // widened i128 product must exactly equal the mathematical value.
        let pnl = unrealized_pnl_micros(i64::MAX, 0, i64::MAX);
        let expected = (i64::MAX as i128) * (i64::MAX as i128);
        assert_eq!(pnl, expected);
    }

    #[test]
    fn proof_02_real_fill_scenario_positive_when_mark_above_avg() {
        // AAPL qty=3 avg_price=314.81 (PAPER-TRADE-LIFECYCLE-PROOF-02 §12).
        // A mark of 320.00 must show a positive unrealized gain.
        let avg_price_micros = 314_810_000i64;
        let mark_price_micros = 320_000_000i64;
        let pnl = unrealized_pnl_micros(3, avg_price_micros, mark_price_micros);
        assert_eq!(pnl, (320_000_000 - 314_810_000) * 3);
        assert!(pnl > 0);
    }
}
