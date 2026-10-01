use super::window::complete_positive_tail;
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "near_high_momentum_252_3pct";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// Completed closes in the trailing high window and the minimum history.
const HIGH_LOOKBACK: usize = 252;
/// Completed closes in the trailing mean window.
const MEAN_LOOKBACK: usize = 50;
/// Long requires `100 * close >= PROXIMITY_NUM * high` (within 3 percent of the high).
const PROXIMITY_NUM: i128 = 97;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat 52-week-high proximity momentum: long one share while the latest completed close is within 3 percent of its trailing 252-close high and strictly above its 50-close mean, otherwise flat. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: HIGH_LOOKBACK,
    })
}

#[derive(Clone, Debug)]
pub struct NearHighMomentum2523PctStrategy {
    symbol: String,
}

impl NearHighMomentum2523PctStrategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    /// `1` iff `100*close >= 97*high252` (equality included) AND
    /// `50*close > sum50` (strict), both exact i128 comparisons over integer
    /// micros; `0` otherwise, on fewer than 252 bars, and on any incomplete bar
    /// or non-positive close in the 252-bar window.
    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        let Some(window) = complete_positive_tail(recent, HIGH_LOOKBACK) else {
            return 0;
        };
        let close = window[HIGH_LOOKBACK - 1].close_micros as i128;
        let high = window
            .iter()
            .map(|b| b.close_micros as i128)
            .max()
            .unwrap_or(0);
        let sum50: i128 = window[HIGH_LOOKBACK - MEAN_LOOKBACK..]
            .iter()
            .map(|b| b.close_micros as i128)
            .sum();
        let near_high = 100 * close >= PROXIMITY_NUM * high;
        let above_mean = MEAN_LOOKBACK as i128 * close > sum50;
        i64::from(near_high && above_mean)
    }
}

impl Strategy for NearHighMomentum2523PctStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(NAME, TIMEFRAME_SECS)
    }

    fn required_history_bars(&self) -> usize {
        HIGH_LOOKBACK
    }

    fn semantic_fingerprint(&self) -> String {
        SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, VERSION)
            .push_str(&self.symbol)
            .push_i64(TIMEFRAME_SECS)
            .push_i64(HIGH_LOOKBACK as i64)
            .push_i64(MEAN_LOOKBACK as i64)
            .push_i64(PROXIMITY_NUM as i64)
            .push_str("high_window:max_close_including_latest")
            .push_str("entry_long:100*close>=97*high252_and_50*close>sum50")
            .push_str("direction:long_flat")
            .push_str("malformed_window:fail_closed_flat")
            .push_str("incomplete_latest:flat")
            .finish()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let qty = Self::signal_from_recent(&ctx.recent.bars);
        StrategyOutput {
            // Fixed one-share Equity signal (0/+1): never short.
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
    use crate::{RecentBarsWindow, StrategyContext};

    const P: i64 = 100_000_000;

    fn bar(close_micros: i64, is_complete: bool) -> BarStub {
        BarStub::new(0, is_complete, close_micros, 1)
    }

    /// 252 bars: `older` for the first 202 closes (the high comes from here), `recent`
    /// for the 49 closes before the latest, then `latest`.
    fn series(older: i64, recent: i64, latest: i64) -> Vec<BarStub> {
        let mut v: Vec<BarStub> = (0..HIGH_LOOKBACK - MEAN_LOOKBACK)
            .map(|_| bar(older, true))
            .collect();
        v.extend((0..MEAN_LOOKBACK - 1).map(|_| bar(recent, true)));
        v.push(bar(latest, true));
        v
    }

    fn sig(bars: &[BarStub]) -> i64 {
        NearHighMomentum2523PctStrategy::signal_from_recent(bars)
    }

    fn ctx(bars: Vec<BarStub>) -> StrategyContext {
        let len = bars.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, bars))
    }

    #[test]
    fn required_history_is_252_and_declared_in_meta() {
        let s = NearHighMomentum2523PctStrategy::new("SPY");
        assert_eq!(s.required_history_bars(), 252);
        assert_eq!(
            meta().data_requirements.unwrap().minimum_completed_bars,
            252
        );
    }

    #[test]
    fn fewer_than_252_completed_bars_is_flat() {
        let rising: Vec<BarStub> = (0..HIGH_LOOKBACK - 1)
            .map(|i| bar(P + i as i64 * 1_000_000, true))
            .collect();
        assert_eq!(
            sig(&rising),
            0,
            "a strongly rising 251-bar history is still flat"
        );
    }

    /// High = 100 (older closes); recent = 95; boundary latest close 97 => 100*97 == 97*100.
    #[test]
    fn exact_proximity_boundary_just_inside_and_just_outside() {
        let (hi, lo) = (100 * 1_000_000, 95 * 1_000_000);
        // latest = 97.000000 exactly: equality on the 3% band, and above the 50-mean (~95.04).
        assert_eq!(
            sig(&series(hi, lo, 97 * 1_000_000)),
            1,
            "exactly 3% below the high is long"
        );
        assert_eq!(
            sig(&series(hi, lo, 97 * 1_000_000 + 1)),
            1,
            "one micro inside the band"
        );
        assert_eq!(
            sig(&series(hi, lo, 97 * 1_000_000 - 1)),
            0,
            "one micro outside the band is flat"
        );
    }

    /// The latest close itself can be the high; then the band holds trivially and only the mean matters.
    #[test]
    fn mean_boundary_is_strict() {
        // High is an old close of 100. Latest sits inside the band at 98 and all 50 closes of the
        // mean window equal 98, so sum50 == 50*98 exactly -> NOT strictly above the mean -> flat.
        let mut v: Vec<BarStub> = (0..HIGH_LOOKBACK - MEAN_LOOKBACK)
            .map(|_| bar(100 * 1_000_000, true))
            .collect();
        let latest = 98 * 1_000_000_i64;
        v.extend((0..MEAN_LOOKBACK - 1).map(|_| bar(latest, true)));
        v.push(bar(latest, true));
        assert_eq!(sig(&v), 0, "close equal to the 50-mean is flat");
        // Raise the latest by one micro: 50*c > sum50 and still inside the band.
        let n = v.len();
        v[n - 1] = bar(latest + 1, true);
        assert_eq!(sig(&v), 1, "one micro above the mean is long");
        // Lower one interior close by 1 micro with latest at 98: sum50 drops -> strictly above -> long.
        let mut w: Vec<BarStub> = (0..HIGH_LOOKBACK - MEAN_LOOKBACK)
            .map(|_| bar(100 * 1_000_000, true))
            .collect();
        w.extend((0..MEAN_LOOKBACK - 1).map(|_| bar(latest, true)));
        w.push(bar(latest, true));
        w[HIGH_LOOKBACK - 2] = bar(latest - 1, true);
        assert_eq!(sig(&w), 1);
    }

    #[test]
    fn near_high_but_below_mean_is_flat() {
        // Old closes at 100, recent 49 closes at 120 (so the high is 120), latest 119: within 3% of 120
        // but the mean50 (~119.98) is above the latest -> flat.
        let mut v: Vec<BarStub> = (0..HIGH_LOOKBACK - MEAN_LOOKBACK)
            .map(|_| bar(100 * 1_000_000, true))
            .collect();
        v.extend((0..MEAN_LOOKBACK - 1).map(|_| bar(120 * 1_000_000, true)));
        v.push(bar(119 * 1_000_000, true));
        assert_eq!(sig(&v), 0);
    }

    #[test]
    fn above_mean_but_far_from_the_high_is_flat() {
        // High 200 long ago, everything since ~100, latest 101 > mean but far below 97% of 200.
        let mut v: Vec<BarStub> = vec![bar(200 * 1_000_000, true)];
        v.extend((0..HIGH_LOOKBACK - 1 - MEAN_LOOKBACK).map(|_| bar(100 * 1_000_000, true)));
        v.extend((0..MEAN_LOOKBACK - 1).map(|_| bar(100 * 1_000_000, true)));
        v.push(bar(101 * 1_000_000, true));
        assert_eq!(v.len(), HIGH_LOOKBACK);
        assert_eq!(sig(&v), 0);
    }

    #[test]
    fn the_high_includes_the_latest_close_and_is_the_window_maximum() {
        // A new high on the latest bar: close == high, above the mean -> long.
        assert_eq!(
            sig(&series(90 * 1_000_000, 95 * 1_000_000, 120 * 1_000_000)),
            1
        );
        // A single old spike at the start of the window sets the high and excludes the latest.
        let mut v = series(90 * 1_000_000, 95 * 1_000_000, 96 * 1_000_000);
        v[0] = bar(1_000 * 1_000_000, true);
        assert_eq!(
            sig(&v),
            0,
            "the in-window high (1000) is far above the latest"
        );
        // The same spike one bar older than the window has no effect.
        let mut longer = vec![bar(1_000 * 1_000_000, true)];
        longer.extend(series(90 * 1_000_000, 95 * 1_000_000, 96 * 1_000_000));
        assert_eq!(longer.len(), HIGH_LOOKBACK + 1);
        assert_eq!(sig(&longer), 1, "older-than-required history has no effect");
    }

    #[test]
    fn incomplete_latest_or_interior_bar_and_non_positive_close_fail_closed() {
        let base = series(90 * 1_000_000, 95 * 1_000_000, 120 * 1_000_000);
        assert_eq!(sig(&base), 1);
        let mut b = base.clone();
        *b.last_mut().unwrap() = bar(120 * 1_000_000, false);
        assert_eq!(sig(&b), 0, "incomplete latest bar cannot create an action");
        let mut b = base.clone();
        b[10] = bar(90 * 1_000_000, false);
        assert_eq!(sig(&b), 0);
        let mut b = base.clone();
        b[10] = bar(0, true);
        assert_eq!(sig(&b), 0);
        let mut b = base.clone();
        b[0] = bar(-5, true);
        assert_eq!(sig(&b), 0);
    }

    #[test]
    fn future_bars_are_not_used() {
        let all: Vec<BarStub> = (0..420)
            .map(|i| {
                bar(
                    P + (i % 19) * 800_000 + (i / 4) * 90_000 - (i % 11) * 500_000,
                    true,
                )
            })
            .collect();
        for t in HIGH_LOOKBACK - 1..all.len() {
            assert_eq!(
                sig(&all[..=t]),
                sig(&all[t + 1 - HIGH_LOOKBACK..=t]),
                "bar {t}"
            );
        }
        let mut extended = all.clone();
        extended.push(bar(1, true));
        for t in HIGH_LOOKBACK - 1..all.len() {
            assert_eq!(sig(&extended[..=t]), sig(&all[..=t]), "bar {t}");
        }
    }

    #[test]
    fn never_shorts_and_emits_one_symbol_target() {
        let mut s = NearHighMomentum2523PctStrategy::new("SPY");
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
        let out = s.on_bar(&ctx(series(
            90 * 1_000_000,
            95 * 1_000_000,
            120 * 1_000_000,
        )));
        assert_eq!(out.targets.len(), 1);
        assert_eq!(out.targets[0].symbol, "SPY");
        assert_eq!(out.targets[0].qty.to_whole_units_checked().unwrap(), 1);
        for (o, r, l) in [(100, 1, 1), (1, 100, 100), (100, 100, 100), (500, 20, 3)] {
            let out = s.on_bar(&ctx(series(o * 1_000_000, r * 1_000_000, l * 1_000_000)));
            assert!(out.targets[0].qty.raw() >= 0);
        }
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_hex() {
        let a = NearHighMomentum2523PctStrategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            a,
            NearHighMomentum2523PctStrategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            a,
            NearHighMomentum2523PctStrategy::new("EFA").semantic_fingerprint()
        );
        assert_eq!(a.len(), 64);
        assert!(a
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }

    /// Mutation proof: every behavior-changing semantic is bound.
    #[test]
    fn fingerprint_changes_when_any_semantic_field_changes() {
        let fp =
            |name: &str, version: &str, tf: i64, high: i64, mean: i64, prox: i64, rule: &str| {
                SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, name, version)
                    .push_str("SPY")
                    .push_i64(tf)
                    .push_i64(high)
                    .push_i64(mean)
                    .push_i64(prox)
                    .push_str("high_window:max_close_including_latest")
                    .push_str(rule)
                    .push_str("direction:long_flat")
                    .push_str("malformed_window:fail_closed_flat")
                    .push_str("incomplete_latest:flat")
                    .finish()
            };
        let live = NearHighMomentum2523PctStrategy::new("SPY").semantic_fingerprint();
        let rule = "entry_long:100*close>=97*high252_and_50*close>sum50";
        assert_eq!(
            live,
            fp(NAME, VERSION, TIMEFRAME_SECS, 252, 50, 97, rule),
            "recipe mirrors the engine"
        );
        assert_ne!(live, fp(NAME, VERSION, TIMEFRAME_SECS, 250, 50, 97, rule));
        assert_ne!(live, fp(NAME, VERSION, TIMEFRAME_SECS, 252, 40, 97, rule));
        assert_ne!(live, fp(NAME, VERSION, TIMEFRAME_SECS, 252, 50, 95, rule));
        assert_ne!(
            live,
            fp(
                NAME,
                VERSION,
                TIMEFRAME_SECS,
                252,
                50,
                97,
                "entry_long:100*close>97*high252_and_50*close>=sum50"
            )
        );
        assert_ne!(live, fp(NAME, "0.1.1", TIMEFRAME_SECS, 252, 50, 97, rule));
        assert_ne!(live, fp(NAME, VERSION, 3_600, 252, 50, 97, rule));
    }
}
