//! Corporate action policy — Patch B4
//!
//! Enforces an explicit policy for corporate actions (splits, dividends, mergers).
//!
//! # Policy variants
//!
//! - [`CorporateActionPolicy::Allow`] — no enforcement; all bars are processed.
//!   The caller is responsible for providing clean / pre-adjusted data.
//! - [`CorporateActionPolicy::ForbidPeriods`] — the backtest engine halts
//!   immediately when a bar arrives for a declared (symbol, period) exclusion.
//!   This prevents silent contamination from unadjusted price data.
//!
//! # Design rationale
//!
//! Corporate actions (splits, dividends, mergers) make raw price data ambiguous.
//! Without adjustment, a 2-for-1 split causes an apparent 50% overnight drop that
//! is not a real loss. Rather than implement adjustment tables (complex, error-prone,
//! data-source-specific), this module enforces an explicit choice:
//!
//! 1. Caller provides adjusted data → use `Allow`.
//! 2. Caller cannot guarantee adjusted data → use `ForbidPeriods` to declare which
//!    (symbol, period) pairs are affected, and the engine halts before running any
//!    strategy logic on contaminated bars.

// ---------------------------------------------------------------------------
// ForbidEntry
// ---------------------------------------------------------------------------

/// A single corporate-action exclusion window.
///
/// Any bar for `symbol` whose `end_ts` falls in `[start_ts, end_ts]` (inclusive)
/// will cause the backtest to halt with a `CorporateActionExclusion` reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForbidEntry {
    /// The symbol affected by the corporate action.
    pub symbol: String,
    /// Inclusive start of the forbidden period (epoch seconds).
    pub start_ts: i64,
    /// Inclusive end of the forbidden period (epoch seconds).
    pub end_ts: i64,
}

impl ForbidEntry {
    /// Construct a new exclusion entry.
    pub fn new(symbol: impl Into<String>, start_ts: i64, end_ts: i64) -> Self {
        debug_assert!(end_ts >= start_ts, "end_ts must be >= start_ts");
        Self {
            symbol: symbol.into(),
            start_ts,
            end_ts,
        }
    }
}

// ---------------------------------------------------------------------------
// CorporateActionPolicy
// ---------------------------------------------------------------------------

/// Explicit policy for handling corporate actions in backtests.
///
/// See the [module-level docs](self) for design rationale.
///
/// `Clone + Debug + PartialEq + Eq` so it embeds cleanly in [`BacktestConfig`].
///
/// [`BacktestConfig`]: crate::types::BacktestConfig
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CorporateActionPolicy {
    /// No enforcement: all bars processed regardless of corporate actions.
    ///
    /// Use this when:
    /// - the data source provides split/dividend-adjusted OHLCV data, or
    /// - the backtest period is known to contain no corporate actions.
    Allow,

    /// Halt on any bar that falls within a declared exclusion period.
    ///
    /// Use this when unadjusted data is unavoidable and you want the backtest
    /// to fail loudly rather than produce silently biased results.
    ForbidPeriods(Vec<ForbidEntry>),
}

impl CorporateActionPolicy {
    /// Returns `true` if the bar should be excluded (backtest must halt).
    ///
    /// `symbol` is the bar's symbol; `bar_end_ts` is its `end_ts` in epoch seconds.
    pub fn is_excluded(&self, symbol: &str, bar_end_ts: i64) -> bool {
        match self {
            CorporateActionPolicy::Allow => false,
            CorporateActionPolicy::ForbidPeriods(entries) => entries
                .iter()
                .any(|e| e.symbol == symbol && bar_end_ts >= e.start_ts && bar_end_ts <= e.end_ts),
        }
    }
}
