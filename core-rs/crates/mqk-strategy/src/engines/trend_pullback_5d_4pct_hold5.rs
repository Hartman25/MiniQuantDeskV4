use super::window::complete_positive_tail;
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "trend_pullback_5d_4pct_hold5";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// Completed closes in the long-term trend mean (including the event bar).
const TREND_LOOKBACK: usize = 200;
/// The pullback is measured against the close this many completed bars earlier.
const PULLBACK_BARS: usize = 5;
/// An event is a decline to at most `RETAIN_PCT` percent of that earlier close (>= 4% decline).
const RETAIN_PCT: i128 = 96;
/// The target is long while an event occurred on any of the latest `HOLD_BARS` completed bars.
const HOLD_BARS: usize = 5;
/// The oldest inspected event bar is `HOLD_BARS - 1` bars before the latest and needs
/// `TREND_LOOKBACK` closes ending there (which also covers its `PULLBACK_BARS` look-back).
const REQUIRED_BARS: usize = TREND_LOOKBACK + HOLD_BARS - 1;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat trend-conditioned pullback: long one share for five completed bars after a close at least 4 percent below the close five bars earlier while above the 200-close mean, otherwise flat. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: REQUIRED_BARS,
    })
}

#[derive(Clone, Debug)]
pub struct TrendPullback5d4pctHold5Strategy {
    symbol: String,
}

impl TrendPullback5d4pctHold5Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    /// Entry event on `window[j]`: `200*close > sum(200 closes ending at j)` (strict)
    /// AND `100*close <= 96*close[j-5]` (equality included); exact i128 comparisons.
    /// Requires `j + 1 >= TREND_LOOKBACK`, which also guarantees `j >= PULLBACK_BARS`.
    fn entry_event(window: &[BarStub], j: usize) -> bool {
        let close = window[j].close_micros as i128;
        let trend_sum: i128 = window[j + 1 - TREND_LOOKBACK..=j]
            .iter()
            .map(|b| b.close_micros as i128)
            .sum();
        let earlier = window[j - PULLBACK_BARS].close_micros as i128;
        TREND_LOOKBACK as i128 * close > trend_sum && 100 * close <= RETAIN_PCT * earlier
    }

    /// `1` iff an entry event occurred on any of the latest five completed bars;
    /// `0` otherwise, on fewer than 204 bars, and on any incomplete bar or
    /// non-positive close in the 204-bar window.
    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        let Some(window) = complete_positive_tail(recent, REQUIRED_BARS) else {
            return 0;
        };
        let latest = REQUIRED_BARS - 1;
        i64::from((0..HOLD_BARS).any(|k| Self::entry_event(window, latest - k)))
    }
}

impl Strategy for TrendPullback5d4pctHold5Strategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(NAME, TIMEFRAME_SECS)
    }

    fn required_history_bars(&self) -> usize {
        REQUIRED_BARS
    }

    fn semantic_fingerprint(&self) -> String {
        SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, VERSION)
            .push_str(&self.symbol)
            .push_i64(TIMEFRAME_SECS)
            .push_i64(TREND_LOOKBACK as i64)
            .push_i64(PULLBACK_BARS as i64)
            .push_i64(RETAIN_PCT as i64)
            .push_i64(HOLD_BARS as i64)
            .push_str("event:200*close>sum200_strict_and_100*close<=96*close_5_bars_earlier")
            .push_str("target:long_if_event_in_latest_5_bars_including_latest")
            .push_str("state:none_reconstructible_from_bounded_history")
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

    const P: i64 = 90_000_000;
    const SPIKE: i64 = 120_000_000;
    const EVENT_CLOSE: i64 = 100_000_000;

    fn bar(close_micros: i64, is_complete: bool) -> BarStub {
        BarStub::new(0, is_complete, close_micros, 1)
    }

    fn flat(n: usize, close: i64) -> Vec<BarStub> {
        (0..n).map(|_| bar(close, true)).collect()
    }

    /// 204 bars of `P` with one isolated entry event whose event bar is `k` bars before the latest:
    /// a spike at event-5 and the event close at the event bar. Everything else sits below its mean,
    /// so no other entry event exists.
    fn series_with_event_at(k: usize) -> Vec<BarStub> {
        let mut v = flat(REQUIRED_BARS, P);
        let j = REQUIRED_BARS - 1 - k;
        v[j - PULLBACK_BARS] = bar(SPIKE, true);
        v[j] = bar(EVENT_CLOSE, true);
        v
    }

    fn sig(bars: &[BarStub]) -> i64 {
        TrendPullback5d4pctHold5Strategy::signal_from_recent(bars)
    }

    fn ctx(bars: Vec<BarStub>) -> StrategyContext {
        let len = bars.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, bars))
    }

    #[test]
    fn required_history_is_204_and_declared_in_meta() {
        let s = TrendPullback5d4pctHold5Strategy::new("SPY");
        assert_eq!(s.required_history_bars(), 204);
        assert_eq!(REQUIRED_BARS, TREND_LOOKBACK + HOLD_BARS - 1);
        assert_eq!(
            meta().data_requirements.unwrap().minimum_completed_bars,
            204
        );
    }

    #[test]
    fn fewer_than_204_completed_bars_is_flat_even_with_a_valid_event_today() {
        let full = series_with_event_at(0);
        assert_eq!(sig(&full), 1);
        assert_eq!(sig(&full[1..]), 0, "203 bars is insufficient history");
        assert_eq!(sig(&[]), 0);
    }

    #[test]
    fn an_event_on_each_of_the_latest_five_bars_keeps_the_target_long() {
        for k in 0..HOLD_BARS {
            assert_eq!(
                sig(&series_with_event_at(k)),
                1,
                "event {k} bars before the latest"
            );
        }
    }

    #[test]
    fn an_event_older_than_five_evaluation_bars_does_not() {
        assert_eq!(sig(&series_with_event_at(5)), 0);
        assert_eq!(sig(&series_with_event_at(6)), 0);
    }

    #[test]
    fn pullback_boundary_is_inclusive_at_exactly_four_percent() {
        // earlier = 125 => 96% of it is exactly 120; event bar closes at 120 (equality), +1, -1.
        let build = |close: i64| {
            let mut v = flat(REQUIRED_BARS, P);
            v[REQUIRED_BARS - 1 - PULLBACK_BARS] = bar(125_000_000, true);
            v[REQUIRED_BARS - 1] = bar(close, true);
            v
        };
        assert_eq!(
            sig(&build(120_000_000)),
            1,
            "exactly a 4% decline is an event"
        );
        assert_eq!(sig(&build(120_000_001)), 0, "one micro shallower is not");
        assert_eq!(sig(&build(119_999_999)), 1, "one micro deeper is");
    }

    #[test]
    fn trend_boundary_is_strict_and_binds_the_oldest_required_bar() {
        // 198 closes at 100, the earlier spike 498, the event close 102: sum200 == 200*102 exactly.
        let build = |close: i64, oldest: i64| {
            let mut v = flat(REQUIRED_BARS, 100_000_000);
            v[0] = bar(oldest, true);
            v[REQUIRED_BARS - 1 - PULLBACK_BARS] = bar(498_000_000, true);
            v[REQUIRED_BARS - 1] = bar(close, true);
            v
        };
        // Event at k=4 puts the 200-window at tail indices 0..=199, so the oldest tail bar counts.
        let build_k4 = |close: i64, oldest: i64| {
            let mut v = flat(REQUIRED_BARS, 100_000_000);
            v[0] = bar(oldest, true);
            let j = REQUIRED_BARS - 1 - 4;
            v[j - PULLBACK_BARS] = bar(498_000_000, true);
            v[j] = bar(close, true);
            v
        };
        // k=0: the 200-window is tail indices 4..=203, so tail[0] is outside it.
        assert_eq!(
            sig(&build(102_000_000, 100_000_000)),
            0,
            "close == 200-mean is not above it"
        );
        assert_eq!(
            sig(&build(102_000_001, 100_000_000)),
            1,
            "one micro above the mean"
        );
        assert_eq!(
            sig(&build(102_000_000, 1)),
            0,
            "a bar outside the 200-window cannot matter"
        );
        assert_eq!(
            sig(&build(102_000_001, 900_000_000_000)),
            1,
            "...in either direction"
        );
        // k=4: tail[0] is the oldest bar of the 200-window; lowering it lowers the mean enough to enter.
        assert_eq!(sig(&build_k4(102_000_000, 100_000_000)), 0);
        assert_eq!(
            sig(&build_k4(102_000_000, 99_999_999)),
            1,
            "the oldest required bar is part of the trend sum"
        );
    }

    #[test]
    fn pullback_without_trend_and_trend_without_pullback_are_flat() {
        // Pullback only: everything falling, so the close is below its mean.
        let falling: Vec<BarStub> = (0..REQUIRED_BARS)
            .map(|i| bar(200_000_000 - i as i64 * 400_000, true))
            .collect();
        assert_eq!(sig(&falling), 0);
        // Trend only: steadily rising, never a 4% five-day decline.
        let rising: Vec<BarStub> = (0..REQUIRED_BARS)
            .map(|i| bar(100_000_000 + i as i64 * 400_000, true))
            .collect();
        assert_eq!(sig(&rising), 0);
    }

    #[test]
    fn incomplete_latest_or_interior_bar_and_non_positive_close_fail_closed() {
        let base = series_with_event_at(0);
        assert_eq!(sig(&base), 1);
        let mut b = base.clone();
        *b.last_mut().unwrap() = bar(EVENT_CLOSE, false);
        assert_eq!(sig(&b), 0, "incomplete latest bar cannot create an action");
        let mut b = base.clone();
        b[10] = bar(P, false);
        assert_eq!(sig(&b), 0);
        let mut b = base.clone();
        b[10] = bar(0, true);
        assert_eq!(sig(&b), 0);
        let mut b = base.clone();
        b[REQUIRED_BARS - 2] = bar(-1, true);
        assert_eq!(sig(&b), 0);
    }

    #[test]
    fn bars_older_than_the_204_window_do_not_influence_the_result() {
        for k in [0, 4, 5] {
            let base = series_with_event_at(k);
            let mut with_old = vec![bar(1, true); 25];
            with_old.extend(base.clone());
            let mut with_huge = vec![bar(900_000_000_000, false); 25];
            with_huge.extend(base.clone());
            assert_eq!(sig(&with_old), sig(&base), "k={k}");
            assert_eq!(sig(&with_huge), sig(&base), "k={k}");
        }
    }

    /// Independent reference over a long rising wave with repeating dips: the target at every
    /// bar equals "any event in the latest five bars", events overlap, and the state is the same
    /// for a long-lived and a fresh instance.
    #[test]
    fn matches_an_independent_reference_with_overlapping_events_and_fresh_instances() {
        let n = 700;
        let wave: Vec<BarStub> = (0..n)
            .map(|i| {
                let phase = (i % 17) as i64;
                let dip = if (3..=6).contains(&phase) {
                    (phase - 2) * 2_500_000
                } else {
                    0
                };
                bar(100_000_000 + i as i64 * 150_000 - dip, true)
            })
            .collect();
        let reference_event = |j: usize| -> bool {
            let c = wave[j].close_micros as i128;
            let s: i128 = wave[j + 1 - 200..=j]
                .iter()
                .map(|b| b.close_micros as i128)
                .sum();
            200 * c > s && 100 * c <= 96 * wave[j - 5].close_micros as i128
        };
        let mut long_lived = TrendPullback5d4pctHold5Strategy::new("SPY");
        let (mut longs, mut flats, mut overlapped) = (0, 0, 0);
        for t in REQUIRED_BARS - 1..n {
            let events = (0..5).filter(|k| reference_event(t - k)).count();
            let expected = i64::from(events > 0);
            assert_eq!(sig(&wave[..=t]), expected, "bar {t}");
            let window = wave[..=t].to_vec();
            let a = long_lived.on_bar(&ctx(window.clone())).targets[0].qty.raw();
            let b = TrendPullback5d4pctHold5Strategy::new("SPY")
                .on_bar(&ctx(window))
                .targets[0]
                .qty
                .raw();
            assert_eq!(a, b, "fresh instance == long-lived instance at bar {t}");
            if expected == 1 {
                longs += 1
            } else {
                flats += 1
            }
            if events >= 2 {
                overlapped += 1
            }
        }
        assert!(
            longs > 20 && flats > 20,
            "fixture must exercise both outcomes ({longs}/{flats})"
        );
        assert!(
            overlapped > 5,
            "fixture must contain overlapping events ({overlapped})"
        );
    }

    #[test]
    fn future_bars_are_not_used() {
        let all: Vec<BarStub> = (0..420)
            .map(|i| {
                bar(
                    100_000_000 + (i % 13) * 700_000 + (i / 6) * 130_000 - (i % 9) * 900_000,
                    true,
                )
            })
            .collect();
        for t in REQUIRED_BARS - 1..all.len() {
            assert_eq!(
                sig(&all[..=t]),
                sig(&all[t + 1 - REQUIRED_BARS..=t]),
                "bar {t}"
            );
        }
        let mut extended = all.clone();
        extended.push(bar(1, true));
        for t in REQUIRED_BARS - 1..all.len() {
            assert_eq!(sig(&extended[..=t]), sig(&all[..=t]), "bar {t}");
        }
    }

    #[test]
    fn never_shorts_and_emits_one_symbol_target() {
        let mut s = TrendPullback5d4pctHold5Strategy::new("SPY");
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
        let out = s.on_bar(&ctx(series_with_event_at(0)));
        assert_eq!(out.targets.len(), 1);
        assert_eq!(out.targets[0].symbol, "SPY");
        assert_eq!(out.targets[0].qty.to_whole_units_checked().unwrap(), 1);
        for k in 0..8 {
            let out = s.on_bar(&ctx(series_with_event_at(k.min(8))));
            assert!(out.targets[0].qty.raw() >= 0);
        }
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_hex() {
        let a = TrendPullback5d4pctHold5Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            a,
            TrendPullback5d4pctHold5Strategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            a,
            TrendPullback5d4pctHold5Strategy::new("EFA").semantic_fingerprint()
        );
        assert_eq!(a.len(), 64);
        assert!(a
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }

    /// Mutation proof: every behavior-changing semantic is bound.
    #[test]
    fn fingerprint_changes_when_any_semantic_field_changes() {
        let fp = |name: &str,
                  version: &str,
                  tf: i64,
                  trend: i64,
                  pull: i64,
                  keep: i64,
                  hold: i64,
                  event: &str,
                  target: &str| {
            SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, name, version)
                .push_str("SPY")
                .push_i64(tf)
                .push_i64(trend)
                .push_i64(pull)
                .push_i64(keep)
                .push_i64(hold)
                .push_str(event)
                .push_str(target)
                .push_str("state:none_reconstructible_from_bounded_history")
                .push_str("direction:long_flat")
                .push_str("malformed_window:fail_closed_flat")
                .push_str("incomplete_latest:flat")
                .finish()
        };
        let live = TrendPullback5d4pctHold5Strategy::new("SPY").semantic_fingerprint();
        let event = "event:200*close>sum200_strict_and_100*close<=96*close_5_bars_earlier";
        let target = "target:long_if_event_in_latest_5_bars_including_latest";
        assert_eq!(
            live,
            fp(NAME, VERSION, TIMEFRAME_SECS, 200, 5, 96, 5, event, target),
            "recipe mirrors the engine"
        );
        assert_ne!(
            live,
            fp(NAME, VERSION, TIMEFRAME_SECS, 150, 5, 96, 5, event, target)
        );
        assert_ne!(
            live,
            fp(NAME, VERSION, TIMEFRAME_SECS, 200, 4, 96, 5, event, target)
        );
        assert_ne!(
            live,
            fp(NAME, VERSION, TIMEFRAME_SECS, 200, 5, 95, 5, event, target)
        );
        assert_ne!(
            live,
            fp(NAME, VERSION, TIMEFRAME_SECS, 200, 5, 96, 4, event, target)
        );
        assert_ne!(
            live,
            fp(
                NAME,
                VERSION,
                TIMEFRAME_SECS,
                200,
                5,
                96,
                5,
                "event:200*close>=sum200_and_100*close<96*close_5_bars_earlier",
                target
            )
        );
        assert_ne!(
            live,
            fp(
                NAME,
                VERSION,
                TIMEFRAME_SECS,
                200,
                5,
                96,
                5,
                event,
                "target:long_if_event_in_latest_4_bars_including_latest"
            )
        );
        assert_ne!(
            live,
            fp(NAME, "0.1.1", TIMEFRAME_SECS, 200, 5, 96, 5, event, target)
        );
        assert_ne!(live, fp(NAME, VERSION, 3_600, 200, 5, 96, 5, event, target));
    }
}
