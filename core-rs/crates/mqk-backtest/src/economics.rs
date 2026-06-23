//! BACKTEST-MULTIPLIER-MARGIN-01: multiplier-aware backtest economics seam.
//!
//! Pure, deterministic helpers that let backtest notional/P&L math represent
//! contract-multiplier instruments (futures, options) without touching
//! live/paper portfolio accounting. `mqk-portfolio` (shared by both the live
//! runtime via `mqk-runtime::orchestrator` and this backtest engine) is not
//! modified by this module at all, and nothing in this module is wired into
//! `BacktestEngine` yet — it is a standalone, additively-tested foundation.
//!
//! Equity callers use [`BacktestInstrumentEconomics::equity`] (multiplier=1),
//! which reproduces the un-multiplied `qty * price` arithmetic used
//! throughout `mqk-portfolio::accounting` and `mqk-portfolio::metrics` today.
//!
//! Margin fields are metadata only. No function in this module reads them to
//! gate, block, or alter any computation — there is no margin enforcement.
//!
//! No IO, no broker/provider/DB access, no wall-clock reads, no RNG.

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
                write!(f, "contract_multiplier must be positive, got {}", multiplier)
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
pub fn notional_micros(qty: i64, price_micros: i64, economics: &BacktestInstrumentEconomics) -> i64 {
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
        assert_eq!(mark_to_market_value_micros(2, 4_510 * M, &econ), 451_000 * M);
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
}
