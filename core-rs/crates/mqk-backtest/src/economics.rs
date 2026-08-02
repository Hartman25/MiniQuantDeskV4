//! BACKTEST-MULTIPLIER-MARGIN-01: multiplier-aware backtest economics seam.
//!
//! Pure, deterministic helpers that let backtest notional/P&L math represent
//! contract-multiplier instruments (futures, options) without touching
//! live/paper portfolio accounting. `mqk-portfolio` (shared by both the live
//! runtime via `mqk-runtime::orchestrator` and this backtest engine) is not
//! modified by this module at all.
//!
//! Equity callers use [`BacktestInstrumentEconomics::equity`] (multiplier=1),
//! which reproduces the un-multiplied `qty * price` arithmetic used
//! throughout `mqk-portfolio::accounting` and `mqk-portfolio::metrics` today.
//!
//! Margin fields are metadata only. No function in this module reads them to
//! gate, block, or alter any computation — there is no margin enforcement.
//!
//! No IO, no broker/provider/DB access, no wall-clock reads, no RNG.
//!
//! # BACKTEST-MULTIPLIER-RUN-WIRE-01 — engine wiring
//!
//! [`BacktestEconomicsLedger`] is the run-path consumer of these helpers: a
//! backtest-only shadow ledger that `mqk_backtest::engine::BacktestEngine`
//! updates in parallel with `mqk_portfolio::PortfolioState` (which remains
//! the unmodified, authoritative source for risk gating and the existing
//! `equity_curve`). See `BacktestEngine::with_economics`.

/// Per-instrument economics used to scale backtest notional/P&L math.
///
/// `contract_multiplier` is the number of underlying units represented by
/// one contract. Equities are `1` (one share = one unit). Futures/options
/// use registry-style values (e.g. ES futures = 50, standard equity options
/// = 100) — mirroring the `multiplier` field already validated (but not yet
/// consumed by any P&L path) on `ContractDefinitionV2::Future` /
/// `ContractDefinitionV2::Option` in `mqk-md::instrument_registry_v2`.
///
/// `initial_margin_micros` / `maintenance_margin_micros` are an optional
/// scaffold for a future margin model. They are carried for forward
/// compatibility and are never read by [`notional_micros`],
/// [`mark_to_market_value_micros`], or [`realized_pnl_micros`] — no margin
/// enforcement exists yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacktestInstrumentEconomics {
    pub contract_multiplier: i64,
    pub initial_margin_micros: Option<i64>,
    pub maintenance_margin_micros: Option<i64>,
}

/// Economics validation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EconomicsError {
    /// `contract_multiplier` must be strictly positive. Fails closed rather
    /// than silently defaulting to 1, since a silent default could mask a
    /// misconfigured futures/options fixture.
    InvalidMultiplier { multiplier: i64 },
}

impl core::fmt::Display for EconomicsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EconomicsError::InvalidMultiplier { multiplier } => {
                write!(
                    f,
                    "contract_multiplier must be positive, got {}",
                    multiplier
                )
            }
        }
    }
}

impl std::error::Error for EconomicsError {}

impl BacktestInstrumentEconomics {
    /// Default equity economics: multiplier=1, no margin scaffold.
    ///
    /// This is the implicit behavior of every equity backtest/accounting
    /// path today (`mqk-portfolio::accounting`, `mqk-portfolio::metrics`).
    /// Infallible by construction.
    pub const fn equity() -> Self {
        Self {
            contract_multiplier: 1,
            initial_margin_micros: None,
            maintenance_margin_micros: None,
        }
    }

    /// Construct economics for a multiplier-bearing instrument
    /// (futures/options-style). Rejects non-positive multipliers.
    pub fn new(
        contract_multiplier: i64,
        initial_margin_micros: Option<i64>,
        maintenance_margin_micros: Option<i64>,
    ) -> Result<Self, EconomicsError> {
        if contract_multiplier <= 0 {
            return Err(EconomicsError::InvalidMultiplier {
                multiplier: contract_multiplier,
            });
        }
        Ok(Self {
            contract_multiplier,
            initial_margin_micros,
            maintenance_margin_micros,
        })
    }

    /// True iff this economics value is identical to [`Self::equity`]
    /// (multiplier=1, no margin scaffold).
    ///
    /// BACKTEST-REPORT-ECONOMICS-ARTIFACT-01: used to decide whether a run's
    /// `run_id` stays byte-identical to the pre-existing (economics-unaware)
    /// formula, or becomes economics-sensitive. See `derive_run_id_with_economics`.
    pub(crate) fn is_default_equity(&self) -> bool {
        *self == Self::equity()
    }
}

// ---------------------------------------------------------------------------
// BacktestEconomicsReport — BACKTEST-REPORT-ECONOMICS-ARTIFACT-01
// ---------------------------------------------------------------------------

/// Truthful economics summary attached to every [`crate::types::BacktestReport`].
///
/// Default (equity) reports carry [`BacktestEconomicsReport::equity`] --
/// multiplier=1, no margin, `margin_enforced=false` -- identical for every
/// backtest that never calls `BacktestEngine::with_economics`. Multiplier- or
/// margin-bearing runs carry the actual configured values plus the final
/// multiplier-aware realized P&L from [`BacktestEconomicsLedger`].
///
/// `margin_enforced` is always `false`: margin fields remain metadata-only
/// scaffolding (see module docs above) -- no code path in this crate gates,
/// blocks, or alters behavior based on them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacktestEconomicsReport {
    pub contract_multiplier: i64,
    pub initial_margin_micros: Option<i64>,
    pub maintenance_margin_micros: Option<i64>,
    pub realized_pnl_micros: i64,
    pub margin_enforced: bool,
}

impl BacktestEconomicsReport {
    /// Default equity report economics: multiplier=1, no margin, zero realized P&L.
    pub const fn equity() -> Self {
        Self {
            contract_multiplier: 1,
            initial_margin_micros: None,
            maintenance_margin_micros: None,
            realized_pnl_micros: 0,
            margin_enforced: false,
        }
    }

    /// Build the report section from the run's configured economics and the
    /// final realized P&L accumulated in the [`BacktestEconomicsLedger`].
    pub fn from_run(economics: &BacktestInstrumentEconomics, realized_pnl_micros: i64) -> Self {
        Self {
            contract_multiplier: economics.contract_multiplier,
            initial_margin_micros: economics.initial_margin_micros,
            maintenance_margin_micros: economics.maintenance_margin_micros,
            realized_pnl_micros,
            margin_enforced: false,
        }
    }
}

/// Saturating i128 multiplication: returns `i128::MAX`/`i128::MIN` on
/// overflow instead of panicking (debug builds) or silently wrapping
/// (release builds). Needed because this module multiplies three
/// caller-supplied i64 magnitudes together (qty * price * multiplier),
/// one more factor than the qty*price-only formulas in `mqk-portfolio`,
/// which raises the realistic overflow ceiling.
fn saturating_mul_i128(a: i128, b: i128) -> i128 {
    match a.checked_mul(b) {
        Some(v) => v,
        None if (a >= 0) == (b >= 0) => i128::MAX,
        None => i128::MIN,
    }
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

/// Multiplier-aware notional: `qty * price_micros * contract_multiplier`.
///
/// With `economics.contract_multiplier == 1` this reproduces the
/// un-multiplied `qty * price_micros` notional computed inline today (e.g.
/// the allocation-cap check in `BacktestEngine::run`).
pub fn notional_micros(
    qty: i64,
    price_micros: i64,
    economics: &BacktestInstrumentEconomics,
) -> i64 {
    let step1 = saturating_mul_i128(qty as i128, price_micros as i128);
    let step2 = saturating_mul_i128(step1, economics.contract_multiplier as i128);
    clamp_i128_to_i64(step2)
}

/// Multiplier-aware signed mark-to-market value:
/// `signed_qty * mark_price_micros * contract_multiplier`.
///
/// With multiplier=1 this matches the per-position term inside
/// `mqk_portfolio::compute_equity_micros` / `compute_exposure_micros`
/// (`qty * mark`, summed across positions).
pub fn mark_to_market_value_micros(
    signed_qty: i64,
    mark_price_micros: i64,
    economics: &BacktestInstrumentEconomics,
) -> i64 {
    let step1 = saturating_mul_i128(signed_qty as i128, mark_price_micros as i128);
    let step2 = saturating_mul_i128(step1, economics.contract_multiplier as i128);
    clamp_i128_to_i64(step2)
}

/// Multiplier-aware realized P&L for closing (or partially closing) a
/// position:
/// `signed_position_effect * (exit_price_micros - entry_price_micros) * contract_multiplier`.
///
/// `signed_position_effect` is positive when closing/reducing a long
/// (selling) and negative when closing/reducing a short (buying to cover) —
/// mirroring the FIFO sign convention in `mqk_portfolio::accounting::{buy_fifo, sell_fifo}`.
/// With multiplier=1 this reproduces `(price_a - price_b) * qty` exactly as
/// computed there.
pub fn realized_pnl_micros(
    signed_position_effect: i64,
    entry_price_micros: i64,
    exit_price_micros: i64,
    economics: &BacktestInstrumentEconomics,
) -> i64 {
    let diff = (exit_price_micros as i128) - (entry_price_micros as i128);
    let step1 = saturating_mul_i128(signed_position_effect as i128, diff);
    let step2 = saturating_mul_i128(step1, economics.contract_multiplier as i128);
    clamp_i128_to_i64(step2)
}

// ---------------------------------------------------------------------------
// BacktestEconomicsLedger — BACKTEST-MULTIPLIER-RUN-WIRE-01
// ---------------------------------------------------------------------------

/// One FIFO lot in a [`BacktestEconomicsLedger`].
///
/// Mirrors `mqk_portfolio::types::Lot` exactly (`qty_signed > 0` = long,
/// `< 0` = short) but lives entirely inside `mqk-backtest`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct EconomicsLot {
    qty_signed: i64,
    entry_price_micros: i64,
}

/// Backtest-only, multiplier-aware shadow ledger.
///
/// Mirrors the control flow of `mqk_portfolio::accounting::{apply_fill,
/// buy_fifo, sell_fifo}` exactly — same FIFO consumption order, same
/// cash/lot bookkeeping shape — but every notional and realized-P&L term is
/// computed through [`notional_micros`] / [`realized_pnl_micros`] /
/// [`mark_to_market_value_micros`] instead of raw `qty * price`.
///
/// With `economics.contract_multiplier == 1` every value produced here is
/// byte-identical to what `mqk_portfolio::PortfolioState` computes for the
/// same fill sequence (proven by the `bmw_ledger_*` tests below). This
/// struct never reads from or mutates `mqk_portfolio::PortfolioState` — it
/// is a parallel, additive computation only. `mqk_portfolio` remains the
/// sole source of truth for risk gating, halts, and the engine's existing
/// `equity_curve`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BacktestEconomicsLedger {
    economics: BacktestInstrumentEconomics,
    cash_micros: i64,
    realized_pnl_micros: i64,
    positions: std::collections::BTreeMap<String, Vec<EconomicsLot>>,
}

impl BacktestEconomicsLedger {
    pub(crate) fn new(initial_cash_micros: i64, economics: BacktestInstrumentEconomics) -> Self {
        Self {
            economics,
            cash_micros: initial_cash_micros,
            realized_pnl_micros: 0,
            positions: std::collections::BTreeMap::new(),
        }
    }

    pub(crate) fn realized_pnl_micros(&self) -> i64 {
        self.realized_pnl_micros
    }

    /// Multiplier-aware equity: `cash + Σ mark_to_market_value_micros(lot, mark)`.
    /// Marks missing for a held symbol are treated as `0`, matching
    /// `mqk_portfolio::compute_equity_micros`'s `unwrap_or(&0)` convention.
    pub(crate) fn equity_micros(&self, marks: &std::collections::BTreeMap<String, i64>) -> i64 {
        let mut mv: i128 = self.cash_micros as i128;
        for (sym, lots) in &self.positions {
            let mark = *marks.get(sym).unwrap_or(&0);
            for lot in lots {
                mv += mark_to_market_value_micros(lot.qty_signed, mark, &self.economics) as i128;
            }
        }
        clamp_i128_to_i64(mv)
    }

    /// Apply one fill. `qty` must be positive; `is_buy` selects direction.
    /// Mirrors `mqk_portfolio::accounting::apply_fill` 1:1 in control flow.
    pub(crate) fn apply_fill(
        &mut self,
        symbol: &str,
        is_buy: bool,
        qty: i64,
        price_micros: i64,
        fee_micros: i64,
    ) {
        debug_assert!(qty > 0);
        debug_assert!(price_micros >= 0);
        debug_assert!(fee_micros >= 0);

        let notional = notional_micros(qty, price_micros, &self.economics);
        if is_buy {
            self.cash_micros = self.cash_micros.saturating_sub(notional);
            self.cash_micros = self.cash_micros.saturating_sub(fee_micros);
        } else {
            self.cash_micros = self.cash_micros.saturating_add(notional);
            self.cash_micros = self.cash_micros.saturating_sub(fee_micros);
        }

        let lots = self.positions.entry(symbol.to_string()).or_default();
        if is_buy {
            Self::buy_fifo(
                lots,
                &mut self.realized_pnl_micros,
                qty,
                price_micros,
                &self.economics,
            );
        } else {
            Self::sell_fifo(
                lots,
                &mut self.realized_pnl_micros,
                qty,
                price_micros,
                &self.economics,
            );
        }

        if lots.iter().map(|l| l.qty_signed).sum::<i64>() == 0 {
            self.positions.remove(symbol);
        }
    }

    /// Buy FIFO: covers shorts first (realized P&L), then opens a long lot.
    /// Structurally identical to `mqk_portfolio::accounting::buy_fifo`.
    fn buy_fifo(
        lots: &mut Vec<EconomicsLot>,
        realized: &mut i64,
        mut qty: i64,
        buy_px: i64,
        economics: &BacktestInstrumentEconomics,
    ) {
        let mut i = 0usize;
        while qty > 0 && i < lots.len() {
            if lots[i].qty_signed >= 0 {
                i += 1;
                continue;
            }
            let abs_qty = -lots[i].qty_signed;
            let coverable = abs_qty.min(qty);
            let entry_px = lots[i].entry_price_micros;

            // Covering a short is a negative position effect (mirrors the
            // docstring convention on `realized_pnl_micros`).
            let pnl = realized_pnl_micros(-coverable, entry_px, buy_px, economics);
            *realized = realized.saturating_add(pnl);

            let remaining_abs = abs_qty - coverable;
            if remaining_abs == 0 {
                lots.remove(i);
            } else {
                lots[i].qty_signed = -remaining_abs;
                i += 1;
            }
            qty -= coverable;
        }

        if qty > 0 {
            lots.push(EconomicsLot {
                qty_signed: qty,
                entry_price_micros: buy_px,
            });
        }
    }

    /// Sell FIFO: reduces longs first (realized P&L), then opens a short lot.
    /// Structurally identical to `mqk_portfolio::accounting::sell_fifo`.
    fn sell_fifo(
        lots: &mut Vec<EconomicsLot>,
        realized: &mut i64,
        mut qty: i64,
        sell_px: i64,
        economics: &BacktestInstrumentEconomics,
    ) {
        let mut i = 0usize;
        while qty > 0 && i < lots.len() {
            if lots[i].qty_signed <= 0 {
                i += 1;
                continue;
            }
            let abs_qty = lots[i].qty_signed;
            let sellable = abs_qty.min(qty);
            let entry_px = lots[i].entry_price_micros;

            // Reducing a long is a positive position effect.
            let pnl = realized_pnl_micros(sellable, entry_px, sell_px, economics);
            *realized = realized.saturating_add(pnl);

            let remaining_abs = abs_qty - sellable;
            if remaining_abs == 0 {
                lots.remove(i);
            } else {
                lots[i].qty_signed = remaining_abs;
                i += 1;
            }
            qty -= sellable;
        }

        if qty > 0 {
            lots.push(EconomicsLot {
                qty_signed: -qty,
                entry_price_micros: sell_px,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: i64 = 1_000_000;

    // --- bmm01: multiplier=1 (equity) preserves existing un-multiplied math ---

    #[test]
    fn bmm01_equity_constructor_is_multiplier_one_no_margin() {
        let econ = BacktestInstrumentEconomics::equity();
        assert_eq!(econ.contract_multiplier, 1);
        assert_eq!(econ.initial_margin_micros, None);
        assert_eq!(econ.maintenance_margin_micros, None);
    }

    #[test]
    fn bmm01_equity_multiplier_one_preserves_notional() {
        let econ = BacktestInstrumentEconomics::equity();
        // 10 shares @ $100 = $1,000 notional (plain qty*price, no scaling).
        assert_eq!(notional_micros(10, 100 * M, &econ), 1_000 * M);
    }

    #[test]
    fn bmm01_equity_multiplier_one_preserves_realized_pnl() {
        let econ = BacktestInstrumentEconomics::equity();
        // Mirrors scenario_pnl_partial_fills_fifo.rs: sell 5 @ 120 closing a
        // long entered @ 100 -> realized = (120-100)*5 = $100.
        let pnl = realized_pnl_micros(5, 100 * M, 120 * M, &econ);
        assert_eq!(pnl, 100 * M);
    }

    #[test]
    fn bmm01_equity_multiplier_one_preserves_mark_to_market_and_equity() {
        let econ = BacktestInstrumentEconomics::equity();
        // Mirrors scenario_pnl_partial_fills_fifo.rs end state exactly:
        // 15 shares @ mark 115 -> market value 1,725; cash 98,500 -> equity 100,225.
        let mv = mark_to_market_value_micros(15, 115 * M, &econ);
        assert_eq!(mv, 1_725 * M);
        let cash = 98_500 * M;
        assert_eq!(cash + mv, 100_225 * M);
    }

    // --- bmm02: synthetic futures-style multiplier (50, e.g. ES) scales P&L/notional ---

    #[test]
    fn bmm02_futures_multiplier_scales_notional() {
        let econ = BacktestInstrumentEconomics::new(50, None, None).unwrap();
        // 2 contracts @ 4,500.00 * 50 = $450,000 notional.
        assert_eq!(notional_micros(2, 4_500 * M, &econ), 450_000 * M);
    }

    #[test]
    fn bmm02_futures_multiplier_scales_realized_pnl_long() {
        let econ = BacktestInstrumentEconomics::new(50, None, None).unwrap();
        // Long 2 contracts, entry 4,500 -> exit 4,510: (4510-4500)*2*50 = $1,000.
        let pnl = realized_pnl_micros(2, 4_500 * M, 4_510 * M, &econ);
        assert_eq!(pnl, 1_000 * M);
    }

    #[test]
    fn bmm02_futures_multiplier_scales_realized_pnl_short() {
        let econ = BacktestInstrumentEconomics::new(50, None, None).unwrap();
        // Short 2 contracts (signed_position_effect = -2), entry 4,500,
        // covered lower at 4,490: (4490-4500)*(-2)*50 = $1,000 profit.
        let pnl = realized_pnl_micros(-2, 4_500 * M, 4_490 * M, &econ);
        assert_eq!(pnl, 1_000 * M);
    }

    #[test]
    fn bmm02_futures_multiplier_scales_mark_to_market() {
        let econ = BacktestInstrumentEconomics::new(50, None, None).unwrap();
        // 2 long contracts @ mark 4,510 -> 2*4510*50 = $451,000 market value.
        assert_eq!(
            mark_to_market_value_micros(2, 4_510 * M, &econ),
            451_000 * M
        );
    }

    // --- bmm03: synthetic options-style multiplier (100) scales P&L/notional ---

    #[test]
    fn bmm03_options_multiplier_scales_notional() {
        let econ = BacktestInstrumentEconomics::new(100, None, None).unwrap();
        // 3 contracts @ $5.00 premium * 100 = $1,500 notional.
        assert_eq!(notional_micros(3, 5 * M, &econ), 1_500 * M);
    }

    #[test]
    fn bmm03_options_multiplier_scales_realized_pnl() {
        let econ = BacktestInstrumentEconomics::new(100, None, None).unwrap();
        // Long 3 contracts, entry premium $5.00 -> exit $8.00: (8-5)*3*100 = $900.
        let pnl = realized_pnl_micros(3, 5 * M, 8 * M, &econ);
        assert_eq!(pnl, 900 * M);
    }

    // --- bmm04: invalid multiplier fails closed ---

    #[test]
    fn bmm04_zero_multiplier_rejected() {
        let err = BacktestInstrumentEconomics::new(0, None, None).unwrap_err();
        assert_eq!(err, EconomicsError::InvalidMultiplier { multiplier: 0 });
    }

    #[test]
    fn bmm04_negative_multiplier_rejected() {
        let err = BacktestInstrumentEconomics::new(-5, None, None).unwrap_err();
        assert_eq!(err, EconomicsError::InvalidMultiplier { multiplier: -5 });
    }

    // --- bmm05: margin metadata is scaffold-only and never alters P&L/notional ---

    #[test]
    fn bmm05_margin_metadata_does_not_alter_notional_or_pnl() {
        let bare = BacktestInstrumentEconomics::new(50, None, None).unwrap();
        let with_margin =
            BacktestInstrumentEconomics::new(50, Some(10_000 * M), Some(5_000 * M)).unwrap();

        assert_eq!(
            notional_micros(2, 4_500 * M, &bare),
            notional_micros(2, 4_500 * M, &with_margin)
        );
        assert_eq!(
            realized_pnl_micros(2, 4_500 * M, 4_510 * M, &bare),
            realized_pnl_micros(2, 4_500 * M, 4_510 * M, &with_margin)
        );
        assert_eq!(
            mark_to_market_value_micros(2, 4_510 * M, &bare),
            mark_to_market_value_micros(2, 4_510 * M, &with_margin)
        );
    }

    // --- bmm06: extreme magnitudes saturate rather than panic or wrap ---

    #[test]
    fn bmm06_extreme_values_saturate_without_panic() {
        let econ = BacktestInstrumentEconomics::new(i64::MAX, None, None).unwrap();
        assert_eq!(notional_micros(i64::MAX, i64::MAX, &econ), i64::MAX);
        assert_eq!(
            mark_to_market_value_micros(i64::MAX, i64::MAX, &econ),
            i64::MAX
        );
        assert_eq!(
            mark_to_market_value_micros(i64::MIN, i64::MAX, &econ),
            i64::MIN
        );
    }

    // --- bmm07: is_default_equity (BACKTEST-REPORT-ECONOMICS-ARTIFACT-01) ---

    #[test]
    fn bmm07_is_default_equity_true_only_for_bare_multiplier_one() {
        assert!(BacktestInstrumentEconomics::equity().is_default_equity());
        assert!(BacktestInstrumentEconomics::new(1, None, None)
            .unwrap()
            .is_default_equity());
    }

    #[test]
    fn bmm07b_is_default_equity_false_for_multiplier_or_margin() {
        assert!(!BacktestInstrumentEconomics::new(50, None, None)
            .unwrap()
            .is_default_equity());
        assert!(!BacktestInstrumentEconomics::new(1, Some(10 * M), None)
            .unwrap()
            .is_default_equity());
        assert!(!BacktestInstrumentEconomics::new(1, None, Some(10 * M))
            .unwrap()
            .is_default_equity());
    }

    // --- ber01: BacktestEconomicsReport (BACKTEST-REPORT-ECONOMICS-ARTIFACT-01) ---

    #[test]
    fn ber01_equity_default_is_truthful_zero_state() {
        let r = BacktestEconomicsReport::equity();
        assert_eq!(r.contract_multiplier, 1);
        assert_eq!(r.initial_margin_micros, None);
        assert_eq!(r.maintenance_margin_micros, None);
        assert_eq!(r.realized_pnl_micros, 0);
        assert!(!r.margin_enforced);
    }

    #[test]
    fn ber02_from_run_round_trips_economics_and_pnl_truthfully() {
        let econ = BacktestInstrumentEconomics::new(50, Some(10_000 * M), Some(5_000 * M)).unwrap();
        let r = BacktestEconomicsReport::from_run(&econ, 12_345 * M);
        assert_eq!(r.contract_multiplier, 50);
        assert_eq!(r.initial_margin_micros, Some(10_000 * M));
        assert_eq!(r.maintenance_margin_micros, Some(5_000 * M));
        assert_eq!(r.realized_pnl_micros, 12_345 * M);
        // Margin metadata is scaffold-only -- never enforced.
        assert!(!r.margin_enforced);
    }

    #[test]
    fn ber03_from_run_at_equity_matches_equity_constructor() {
        let econ = BacktestInstrumentEconomics::equity();
        let r = BacktestEconomicsReport::from_run(&econ, 0);
        assert_eq!(r, BacktestEconomicsReport::equity());
    }

    // --- bmw_ledger: BacktestEconomicsLedger (BACKTEST-MULTIPLIER-RUN-WIRE-01) ---

    #[test]
    fn bmw_ledger_matches_portfolio_state_at_multiplier_one() {
        use mqk_portfolio::{apply_fill as pf_apply_fill, Fill, PortfolioState, Side};
        use std::collections::BTreeMap;

        let initial_cash = 100_000 * M;
        let fills: [(bool, i64, i64, i64); 3] = [
            (true, 10, 100 * M, 0),
            (false, 4, 120 * M, 0),
            (true, 3, 90 * M, 1_000),
        ];

        let mut pf = PortfolioState::new(initial_cash);
        let mut ledger =
            BacktestEconomicsLedger::new(initial_cash, BacktestInstrumentEconomics::equity());

        for (is_buy, qty, price, fee) in fills {
            let side = if is_buy { Side::Buy } else { Side::Sell };
            let fill = Fill::new("AAPL", side, qty, price, fee);
            pf_apply_fill(&mut pf, &fill);
            ledger.apply_fill("AAPL", is_buy, qty, price, fee);
        }

        assert_eq!(
            ledger.cash_micros, pf.cash_micros,
            "cash must match mqk_portfolio at multiplier=1"
        );
        assert_eq!(
            ledger.realized_pnl_micros(),
            pf.realized_pnl_micros,
            "realized pnl must match mqk_portfolio at multiplier=1"
        );

        let mut marks: BTreeMap<String, i64> = BTreeMap::new();
        marks.insert("AAPL".to_string(), 98 * M);
        let pf_equity = mqk_portfolio::compute_equity_micros(pf.cash_micros, &pf.positions, &marks);
        assert_eq!(
            ledger.equity_micros(&marks),
            pf_equity,
            "equity must match mqk_portfolio at multiplier=1"
        );
    }

    #[test]
    fn bmw_ledger_scales_long_realized_pnl_by_multiplier() {
        let initial_cash = 1_000_000 * M;
        let mut ledger1 =
            BacktestEconomicsLedger::new(initial_cash, BacktestInstrumentEconomics::equity());
        let mut ledger50 = BacktestEconomicsLedger::new(
            initial_cash,
            BacktestInstrumentEconomics::new(50, None, None).unwrap(),
        );

        // Long 2 contracts @ 4,500 -> sell @ 4,510.
        ledger1.apply_fill("ES", true, 2, 4_500 * M, 0);
        ledger1.apply_fill("ES", false, 2, 4_510 * M, 0);
        ledger50.apply_fill("ES", true, 2, 4_500 * M, 0);
        ledger50.apply_fill("ES", false, 2, 4_510 * M, 0);

        assert_eq!(ledger1.realized_pnl_micros(), 20 * M); // (4510-4500)*2
        assert_eq!(
            ledger50.realized_pnl_micros(),
            50 * ledger1.realized_pnl_micros()
        );
    }

    #[test]
    fn bmw_ledger_scales_short_cover_pnl_by_multiplier() {
        let initial_cash = 1_000_000 * M;
        let mut ledger1 =
            BacktestEconomicsLedger::new(initial_cash, BacktestInstrumentEconomics::equity());
        let mut ledger100 = BacktestEconomicsLedger::new(
            initial_cash,
            BacktestInstrumentEconomics::new(100, None, None).unwrap(),
        );

        // Short 3 contracts @ 5.00 premium -> cover @ 3.00 (profit).
        ledger1.apply_fill("OPT", false, 3, 5 * M, 0);
        ledger1.apply_fill("OPT", true, 3, 3 * M, 0);
        ledger100.apply_fill("OPT", false, 3, 5 * M, 0);
        ledger100.apply_fill("OPT", true, 3, 3 * M, 0);

        assert_eq!(ledger1.realized_pnl_micros(), 6 * M); // (5-3)*3
        assert_eq!(
            ledger100.realized_pnl_micros(),
            100 * ledger1.realized_pnl_micros()
        );
    }

    #[test]
    fn bmw_ledger_scales_equity_gain_by_multiplier() {
        let initial_cash = 1_000_000 * M;
        let mut ledger1 =
            BacktestEconomicsLedger::new(initial_cash, BacktestInstrumentEconomics::equity());
        let mut ledger50 = BacktestEconomicsLedger::new(
            initial_cash,
            BacktestInstrumentEconomics::new(50, None, None).unwrap(),
        );

        ledger1.apply_fill("ES", true, 2, 4_500 * M, 0);
        ledger50.apply_fill("ES", true, 2, 4_500 * M, 0);

        let mut marks: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
        marks.insert("ES".to_string(), 4_510 * M);

        let gain1 = ledger1.equity_micros(&marks) - initial_cash;
        let gain50 = ledger50.equity_micros(&marks) - initial_cash;

        assert_eq!(gain1, 20 * M); // 2 * (4510-4500)
        assert_eq!(gain50, 50 * gain1);
    }

    #[test]
    fn bmw_ledger_flat_position_drops_from_equity_contribution() {
        let initial_cash = 100_000 * M;
        let econ = BacktestInstrumentEconomics::new(50, None, None).unwrap();
        let mut ledger = BacktestEconomicsLedger::new(initial_cash, econ);
        ledger.apply_fill("ES", true, 2, 4_500 * M, 0);
        ledger.apply_fill("ES", false, 2, 4_510 * M, 0); // fully closed -> flat

        let mut marks: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
        marks.insert("ES".to_string(), 9_999 * M); // must not matter once flat
        assert_eq!(
            ledger.equity_micros(&marks),
            ledger.cash_micros,
            "a flat position must contribute zero market value regardless of mark"
        );
        assert!(
            ledger.positions.is_empty(),
            "flat position must be removed from the map"
        );
    }
}
