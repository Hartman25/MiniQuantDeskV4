use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "trend_sma50";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// Trailing-average window in completed daily closes, including the signal
/// bar. Equals the backtest engine's default `bar_history_len`, so the same
/// decision is reproducible in Backtest and Paper.
const LOOKBACK: usize = 50;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long-only trend filter: long one share while the last completed close is above its trailing 50-day average, otherwise flat.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: LOOKBACK,
    })
}

#[derive(Clone, Debug)]
pub struct TrendSma50Strategy {
    symbol: String,
}

impl TrendSma50Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    /// `1` iff the last completed close is strictly above the integer mean of
    /// the trailing `LOOKBACK` closes (including itself); `0` otherwise and
    /// on any malformed or insufficient input.
    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        if recent.len() < LOOKBACK {
            return 0;
        }
        let last = match recent.last() {
            Some(x) if x.is_complete => x,
            _ => return 0,
        };
        let window = &recent[recent.len() - LOOKBACK..];
        if window.iter().any(|b| !b.is_complete || b.close_micros <= 0) {
            return 0;
        }
        let sum: i128 = window.iter().map(|b| b.close_micros as i128).sum();
        let avg: i128 = sum / LOOKBACK as i128;
        if (last.close_micros as i128) > avg {
            1
        } else {
            0
        }
    }
}

impl Strategy for TrendSma50Strategy {
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
            // Fixed one-share Equity signal (0/+1): never sized for, or
            // registered against, a non-Equity asset class.
            targets: vec![TargetPosition::new(
                self.symbol.clone(),
                QtyMicros::from_whole_units(qty).unwrap_or(QtyMicros::ZERO),
            )],
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

    fn flat_window(close: i64) -> Vec<BarStub> {
        (0..LOOKBACK).map(|_| bar(close, true)).collect()
    }

    fn ctx_with_bars(bars: Vec<BarStub>) -> StrategyContext {
        let len = bars.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, bars))
    }

    #[test]
    fn insufficient_lookback_is_flat() {
        let bars: Vec<BarStub> = (0..LOOKBACK - 1).map(|_| bar(100_000_000, true)).collect();
        assert_eq!(TrendSma50Strategy::signal_from_recent(&bars), 0);
    }

    #[test]
    fn close_equal_to_average_is_flat() {
        assert_eq!(
            TrendSma50Strategy::signal_from_recent(&flat_window(100_000_000)),
            0,
            "strictly-above rule: equality is flat"
        );
    }

    #[test]
    fn close_above_average_is_long() {
        let mut bars = flat_window(100_000_000);
        *bars.last_mut().unwrap() = bar(101_000_000, true);
        assert_eq!(TrendSma50Strategy::signal_from_recent(&bars), 1);
    }

    #[test]
    fn close_below_average_is_flat_never_short() {
        let mut bars = flat_window(100_000_000);
        *bars.last_mut().unwrap() = bar(90_000_000, true);
        assert_eq!(TrendSma50Strategy::signal_from_recent(&bars), 0);
    }

    #[test]
    fn incomplete_last_bar_is_flat() {
        let mut bars = flat_window(100_000_000);
        *bars.last_mut().unwrap() = bar(150_000_000, false);
        assert_eq!(TrendSma50Strategy::signal_from_recent(&bars), 0);
    }

    #[test]
    fn incomplete_interior_bar_fails_closed() {
        let mut bars = flat_window(100_000_000);
        bars[10] = bar(100_000_000, false);
        *bars.last_mut().unwrap() = bar(110_000_000, true);
        assert_eq!(TrendSma50Strategy::signal_from_recent(&bars), 0);
    }

    #[test]
    fn non_positive_close_fails_closed() {
        let mut bars = flat_window(100_000_000);
        bars[0] = bar(0, true);
        *bars.last_mut().unwrap() = bar(110_000_000, true);
        assert_eq!(TrendSma50Strategy::signal_from_recent(&bars), 0);
    }

    /// Only the trailing window participates: older bars cannot leak into the
    /// average. The older bars are extreme lows; if they entered the mean the
    /// last close would look "above average".
    #[test]
    fn bars_older_than_the_window_are_excluded() {
        let mut bars: Vec<BarStub> = (0..5).map(|_| bar(1, true)).collect();
        bars.extend(flat_window(100_000_000));
        assert_eq!(TrendSma50Strategy::signal_from_recent(&bars), 0);
    }

    /// The signal at bar T depends only on bars up to and including T: a
    /// prefix evaluation equals the evaluation inside a longer series.
    #[test]
    fn signal_depends_only_on_history_up_to_the_bar() {
        let series: Vec<BarStub> = (0..120)
            .map(|i| bar(100_000_000 + (i % 7) * 1_000_000 - (i / 11) * 400_000, true))
            .collect();
        for t in LOOKBACK..series.len() {
            let prefix = &series[..=t];
            let full_window = &series[t + 1 - LOOKBACK..=t];
            assert_eq!(
                TrendSma50Strategy::signal_from_recent(prefix),
                TrendSma50Strategy::signal_from_recent(full_window),
                "bar {t}"
            );
        }
    }

    #[test]
    fn on_bar_emits_one_target_for_the_symbol() {
        let mut bars = flat_window(100_000_000);
        *bars.last_mut().unwrap() = bar(110_000_000, true);
        let mut s = TrendSma50Strategy::new("SPY");
        let out = s.on_bar(&ctx_with_bars(bars));
        assert_eq!(out.targets.len(), 1);
        assert_eq!(out.targets[0].symbol, "SPY");
        assert_eq!(out.targets[0].qty.to_whole_units_checked().unwrap(), 1);
    }

    #[test]
    fn fingerprint_is_deterministic_and_symbol_bound() {
        let a = TrendSma50Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(a, TrendSma50Strategy::new("SPY").semantic_fingerprint());
        assert_ne!(a, TrendSma50Strategy::new("EFA").semantic_fingerprint());
        assert_eq!(a.len(), 64);
    }
}
