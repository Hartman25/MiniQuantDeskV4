use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "dual_sma_50_200_trend";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// Completed closes in the fast average (the latest `FAST_LOOKBACK`).
const FAST_LOOKBACK: usize = 50;
/// Completed closes in the slow average and the minimum history.
const SLOW_LOOKBACK: usize = 200;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat trend regime: long one share while the 50-day simple average of completed closes is above the 200-day average, otherwise flat. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: SLOW_LOOKBACK,
    })
}

#[derive(Clone, Debug)]
pub struct DualSma50200TrendStrategy {
    symbol: String,
}

impl DualSma50200TrendStrategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    /// `1` iff the 50-close arithmetic mean is strictly greater than the
    /// 200-close arithmetic mean of the trailing completed closes; `0` on
    /// equality, on fewer than 200 bars, and on any incomplete bar or
    /// non-positive close in the 200-bar window. The means are compared
    /// exactly (`fast_sum * 200 > slow_sum * 50`), with no integer-division
    /// rounding.
    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        if recent.len() < SLOW_LOOKBACK {
            return 0;
        }
        match recent.last() {
            Some(last) if last.is_complete => {}
            _ => return 0,
        }
        let slow_window = &recent[recent.len() - SLOW_LOOKBACK..];
        if slow_window
            .iter()
            .any(|b| !b.is_complete || b.close_micros <= 0)
        {
            return 0;
        }
        let slow_sum: i128 = slow_window.iter().map(|b| b.close_micros as i128).sum();
        let fast_sum: i128 = slow_window[SLOW_LOOKBACK - FAST_LOOKBACK..]
            .iter()
            .map(|b| b.close_micros as i128)
            .sum();
        if fast_sum * SLOW_LOOKBACK as i128 > slow_sum * FAST_LOOKBACK as i128 {
            1
        } else {
            0
        }
    }
}

impl Strategy for DualSma50200TrendStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(NAME, TIMEFRAME_SECS)
    }

    fn required_history_bars(&self) -> usize {
        SLOW_LOOKBACK
    }

    fn semantic_fingerprint(&self) -> String {
        SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, VERSION)
            .push_str(&self.symbol)
            .push_i64(TIMEFRAME_SECS)
            .push_i64(FAST_LOOKBACK as i64)
            .push_i64(SLOW_LOOKBACK as i64)
            .finish()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let qty = Self::signal_from_recent(&ctx.recent.bars);
        StrategyOutput {
            // Fixed one-share Equity signal (0/+1): never short, never sized
            // for, or registered against, a non-Equity asset class.
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

    /// `slow_n` older closes at `slow`, then the latest `FAST_LOOKBACK` at `fast`.
    fn series(slow: i64, fast: i64) -> Vec<BarStub> {
        let mut v: Vec<BarStub> = (0..SLOW_LOOKBACK - FAST_LOOKBACK)
            .map(|_| bar(slow, true))
            .collect();
        v.extend((0..FAST_LOOKBACK).map(|_| bar(fast, true)));
        v
    }

    fn ctx(bars: Vec<BarStub>) -> StrategyContext {
        let len = bars.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, bars))
    }

    fn sig(bars: &[BarStub]) -> i64 {
        DualSma50200TrendStrategy::signal_from_recent(bars)
    }

    #[test]
    fn fewer_than_200_completed_bars_is_flat() {
        let bars: Vec<BarStub> = (0..SLOW_LOOKBACK - 1)
            .map(|_| bar(100_000_000, true))
            .collect();
        assert_eq!(sig(&bars), 0);
        // Even a strongly rising short history stays flat.
        let rising: Vec<BarStub> = (0..SLOW_LOOKBACK - 1)
            .map(|i| bar(100_000_000 + i as i64 * 1_000_000, true))
            .collect();
        assert_eq!(sig(&rising), 0);
    }

    #[test]
    fn exactly_200_bars_with_fast_above_slow_is_long() {
        let bars = series(100_000_000, 110_000_000);
        assert_eq!(bars.len(), SLOW_LOOKBACK);
        assert_eq!(sig(&bars), 1);
    }

    #[test]
    fn fast_below_slow_is_flat() {
        assert_eq!(sig(&series(110_000_000, 100_000_000)), 0);
    }

    #[test]
    fn equality_is_flat_exactly() {
        assert_eq!(sig(&series(100_000_000, 100_000_000)), 0);
        // fast mean exactly equals slow mean with unequal bars: slow window =
        // 150 closes at 90 + 50 at 130 -> slow mean 100; fast = 50 at 130 > 100,
        // so shift: make the fast closes average exactly the slow mean.
        let mut v: Vec<BarStub> = (0..SLOW_LOOKBACK - FAST_LOOKBACK)
            .map(|_| bar(100_000_000, true))
            .collect();
        v.extend(
            (0..FAST_LOOKBACK)
                .map(|i| bar(if i % 2 == 0 { 90_000_000 } else { 110_000_000 }, true)),
        );
        assert_eq!(sig(&v), 0, "fast mean == slow mean exactly -> flat");
    }

    #[test]
    fn a_single_micro_above_equality_is_long() {
        let mut v: Vec<BarStub> = (0..SLOW_LOOKBACK - FAST_LOOKBACK)
            .map(|_| bar(100_000_000, true))
            .collect();
        v.extend(
            (0..FAST_LOOKBACK)
                .map(|i| bar(if i % 2 == 0 { 90_000_000 } else { 110_000_000 }, true)),
        );
        // Raise one fast close by 1 micro: fast sum exceeds the exact-equal sum.
        v[SLOW_LOOKBACK - 1] = bar(110_000_001, true);
        assert_eq!(sig(&v), 1, "exact comparison, no integer-division rounding");
    }

    #[test]
    fn incomplete_latest_bar_cannot_create_action() {
        let mut bars = series(100_000_000, 110_000_000);
        *bars.last_mut().unwrap() = bar(200_000_000, false);
        assert_eq!(sig(&bars), 0);
    }

    #[test]
    fn incomplete_interior_bar_or_non_positive_close_fails_closed() {
        let mut bars = series(100_000_000, 110_000_000);
        bars[5] = bar(100_000_000, false);
        assert_eq!(sig(&bars), 0);
        let mut bars = series(100_000_000, 110_000_000);
        bars[5] = bar(0, true);
        assert_eq!(sig(&bars), 0);
    }

    #[test]
    fn bars_older_than_the_200_window_do_not_influence_the_result() {
        let base = series(100_000_000, 110_000_000);
        let mut with_old = vec![bar(1, true); 25];
        with_old.extend(base.clone());
        let mut with_huge_old = vec![bar(900_000_000_000, true); 25];
        with_huge_old.extend(base.clone());
        assert_eq!(sig(&base), 1);
        assert_eq!(sig(&with_old), 1);
        assert_eq!(sig(&with_huge_old), 1);
    }

    /// The decision at bar T depends only on bars up to and including T.
    #[test]
    fn future_bars_are_not_used() {
        let all: Vec<BarStub> = (0..320)
            .map(|i| bar(100_000_000 + (i % 13) * 700_000 + (i / 7) * 150_000, true))
            .collect();
        for t in SLOW_LOOKBACK - 1..all.len() {
            let prefix = &all[..=t];
            let window = &all[t + 1 - SLOW_LOOKBACK..=t];
            assert_eq!(sig(prefix), sig(window), "bar {t}");
        }
        // Appending a wildly different future bar leaves every earlier decision untouched.
        let mut extended = all.clone();
        extended.push(bar(1, true));
        for t in SLOW_LOOKBACK - 1..all.len() {
            assert_eq!(sig(&extended[..=t]), sig(&all[..=t]), "bar {t}");
        }
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_hex() {
        let a = DualSma50200TrendStrategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            a,
            DualSma50200TrendStrategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            a,
            DualSma50200TrendStrategy::new("EFA").semantic_fingerprint()
        );
        assert_eq!(a.len(), 64);
        assert!(a
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }

    /// Mutation proof: the fingerprint binds both lookbacks, the version and the
    /// timeframe -- changing any one changes the digest.
    #[test]
    fn fingerprint_changes_when_any_semantic_field_changes() {
        let fp = |name: &str, version: &str, tf: i64, fast: i64, slow: i64| {
            SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, name, version)
                .push_str("SPY")
                .push_i64(tf)
                .push_i64(fast)
                .push_i64(slow)
                .finish()
        };
        let live = DualSma50200TrendStrategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            live,
            fp(NAME, VERSION, TIMEFRAME_SECS, 50, 200),
            "recipe mirrors the engine"
        );
        assert_ne!(live, fp(NAME, VERSION, TIMEFRAME_SECS, 40, 200));
        assert_ne!(live, fp(NAME, VERSION, TIMEFRAME_SECS, 50, 250));
        assert_ne!(live, fp(NAME, "0.1.1", TIMEFRAME_SECS, 50, 200));
        assert_ne!(live, fp(NAME, VERSION, 3_600, 50, 200));
        assert_ne!(live, fp("trend_sma50", VERSION, TIMEFRAME_SECS, 50, 200));
    }

    #[test]
    fn output_uses_the_expected_symbol_timeframe_and_never_shorts() {
        let mut s = DualSma50200TrendStrategy::new("SPY");
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
        assert_eq!(s.required_history_bars(), SLOW_LOOKBACK);
        let out = s.on_bar(&ctx(series(100_000_000, 110_000_000)));
        assert_eq!(out.targets.len(), 1);
        assert_eq!(out.targets[0].symbol, "SPY");
        assert_eq!(out.targets[0].qty.to_whole_units_checked().unwrap(), 1);

        // Across many shapes (including a collapsing fast average) the target is never negative.
        for (slow, fast) in [(100, 1), (1, 100), (100, 100), (500, 20)] {
            let out = s.on_bar(&ctx(series(slow * 1_000_000, fast * 1_000_000)));
            assert!(out.targets[0].qty.raw() >= 0);
        }
    }
}
