use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};

pub(crate) const NAME: &str = "volatility_breakout";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 3_600; // 1H
const LOOKBACK: usize = 20;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic breakout engine using prior-window min/max closes.",
    )
    // signal_from_recent requires recent.len() >= LOOKBACK + 1 (a separate
    // current-bar comparison beyond the LOOKBACK-bar prior window; see
    // signal_from_recent below) — the declared requirement must reflect the
    // real minimum, not just the prior-window constant.
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: LOOKBACK + 1,
    })
}

#[derive(Clone, Debug)]
pub struct VolatilityBreakoutStrategy {
    symbol: String,
}

impl VolatilityBreakoutStrategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        if recent.len() < LOOKBACK + 1 {
            return 0;
        }

        let last = match recent.last() {
            Some(x) if x.is_complete => x,
            _ => return 0,
        };

        let prior = &recent[recent.len() - (LOOKBACK + 1)..recent.len() - 1];

        let mut min_close = i64::MAX;
        let mut max_close = i64::MIN;

        for bar in prior {
            min_close = min_close.min(bar.close_micros);
            max_close = max_close.max(bar.close_micros);
        }

        if last.close_micros > max_close {
            1
        } else if last.close_micros < min_close {
            -1
        } else {
            0
        }
    }
}

impl Strategy for VolatilityBreakoutStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(NAME, TIMEFRAME_SECS)
    }

    fn semantic_fingerprint(&self) -> String {
        SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, VERSION)
            .push_str(&self.symbol)
            .push_i64(TIMEFRAME_SECS)
            .push_i64(LOOKBACK as i64)
            .finish()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let qty = Self::signal_from_recent(&ctx.recent.bars);
        StrategyOutput {
            targets: vec![TargetPosition {
                symbol: self.symbol.clone(),
                qty,
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BarStub, RecentBarsWindow, StrategyContext};

    fn bar(close_micros: i64, is_complete: bool) -> BarStub {
        BarStub::new(0, is_complete, close_micros, 1)
    }

    fn ctx_with_bars(bars: Vec<BarStub>) -> StrategyContext {
        let len = bars.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, bars))
    }

    /// A 20-bar prior window with 10 bars at `low` and 10 bars at `high`,
    /// giving a known min/max range of exactly [low, high].
    fn prior_range_bars(low: i64, high: i64) -> Vec<BarStub> {
        let mut v: Vec<BarStub> = (0..10).map(|_| bar(low, true)).collect();
        v.extend((0..10).map(|_| bar(high, true)));
        v
    }

    /// VB-01: fewer than LOOKBACK+1 bars (only the prior window, no current bar
    /// beyond it) → insufficient prior history → qty=0.
    #[test]
    fn vb01_insufficient_prior_history_returns_zero() {
        let bars: Vec<BarStub> = (0..LOOKBACK).map(|_| bar(100_000_000, true)).collect();
        assert_eq!(bars.len(), LOOKBACK, "precondition: exactly LOOKBACK bars");
        assert_eq!(
            VolatilityBreakoutStrategy::signal_from_recent(&bars),
            0,
            "VB-01: len == LOOKBACK (< LOOKBACK+1) → insufficient → 0"
        );
    }

    /// VB-02: upper breakout — current close strictly above the prior 20-bar max → qty=+1.
    #[test]
    fn vb02_upper_breakout_is_positive() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(150_000_000, true));
        assert_eq!(
            VolatilityBreakoutStrategy::signal_from_recent(&bars),
            1,
            "VB-02: close above prior max → upper breakout"
        );
    }

    /// VB-03: lower breakout — current close strictly below the prior 20-bar min → qty=-1.
    #[test]
    fn vb03_lower_breakout_is_negative() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(50_000_000, true));
        assert_eq!(
            VolatilityBreakoutStrategy::signal_from_recent(&bars),
            -1,
            "VB-03: close below prior min → lower breakout"
        );
    }

    /// VB-04: current close inside the prior [min, max] range → no breakout → qty=0.
    #[test]
    fn vb04_inside_range_is_no_breakout() {
        let mut bars = prior_range_bars(90_000_000, 110_000_000);
        bars.push(bar(100_000_000, true)); // strictly inside [90M, 110M]
        assert_eq!(
            VolatilityBreakoutStrategy::signal_from_recent(&bars),
            0,
            "VB-04: close inside prior range → no breakout"
        );
    }

    /// VB-05: current close exactly equal to the prior max → not a breakout
    /// (comparison is strict `>`), qty=0.
    #[test]
    fn vb05_exact_equal_to_max_is_not_breakout() {
        let mut bars = prior_range_bars(90_000_000, 110_000_000);
        bars.push(bar(110_000_000, true)); // == prior max exactly
        assert_eq!(
            VolatilityBreakoutStrategy::signal_from_recent(&bars),
            0,
            "VB-05: close == prior max (not strictly above) → no breakout"
        );
    }

    /// VB-06: current close exactly equal to the prior min → not a breakout
    /// (comparison is strict `<`), qty=0.
    #[test]
    fn vb06_exact_equal_to_min_is_not_breakout() {
        let mut bars = prior_range_bars(90_000_000, 110_000_000);
        bars.push(bar(90_000_000, true)); // == prior min exactly
        assert_eq!(
            VolatilityBreakoutStrategy::signal_from_recent(&bars),
            0,
            "VB-06: close == prior min (not strictly below) → no breakout"
        );
    }

    /// VB-07: the prior window is exactly the LOOKBACK bars immediately before
    /// the current bar. Bars older than that window must not influence min/max.
    #[test]
    fn vb07_prior_window_excludes_older_bars() {
        // 5 extreme outlier bars (would blow out the max if wrongly included),
        // followed by the real 20-bar prior window, followed by the current bar.
        let mut bars: Vec<BarStub> = (0..5).map(|_| bar(200_000_000, true)).collect();
        bars.extend((0..LOOKBACK).map(|_| bar(100_000_000, true)));
        bars.push(bar(150_000_000, true)); // breakout vs the real window's max (100M)
        assert_eq!(bars.len(), 5 + LOOKBACK + 1);
        assert_eq!(
            VolatilityBreakoutStrategy::signal_from_recent(&bars),
            1,
            "VB-07: outlier bars before the 20-bar prior window are excluded"
        );
    }

    /// VB-08: the current bar itself must not be included in the prior min/max
    /// computation (no same-bar/lookahead semantic change). If the current bar's
    /// own close were folded into the prior window, this exact breakout would be
    /// suppressed (the bar can never be strictly greater than a max that includes
    /// itself).
    #[test]
    fn vb08_current_bar_excluded_from_prior_window_no_lookahead() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(150_000_000, true));
        assert_eq!(
            VolatilityBreakoutStrategy::signal_from_recent(&bars),
            1,
            "VB-08: current bar's own close must not participate in the prior max"
        );
    }

    /// VB-09: incomplete current bar → qty=0 regardless of displacement.
    #[test]
    fn vb09_incomplete_last_bar_returns_zero() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(150_000_000, false));
        assert_eq!(
            VolatilityBreakoutStrategy::signal_from_recent(&bars),
            0,
            "VB-09: incomplete current bar → 0"
        );
    }

    /// VB-10: on_bar wraps the raw signal into a TargetPosition for the strategy's symbol.
    #[test]
    fn vb10_on_bar_emits_target_position_for_symbol() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(50_000_000, true)); // lower breakout
        let ctx = ctx_with_bars(bars);
        let mut s = VolatilityBreakoutStrategy::new("AAPL");
        let out = s.on_bar(&ctx);
        assert_eq!(out.targets.len(), 1, "VB-10: exactly one target");
        assert_eq!(out.targets[0].symbol, "AAPL", "VB-10: target symbol matches");
        assert_eq!(out.targets[0].qty, -1, "VB-10: qty matches raw signal");
    }
}
