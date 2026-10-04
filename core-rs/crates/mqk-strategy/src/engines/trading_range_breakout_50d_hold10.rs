use super::window::complete_positive_tail;
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "trading_range_breakout_50d_hold10";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// An event is a close strictly above the maximum of this many PRIOR completed closes
/// (the event bar itself is excluded).
const PRIOR_BARS: usize = 50;
/// The target is long while an event occurred on any of the latest `HOLD_BARS` completed bars.
const HOLD_BARS: usize = 10;
/// The oldest inspected event bar is `HOLD_BARS - 1` bars before the latest and needs
/// `PRIOR_BARS` closes before it: `PRIOR_BARS + HOLD_BARS` bars end at the latest bar.
const REQUIRED_BARS: usize = PRIOR_BARS + HOLD_BARS;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat range breakout: long one share for ten completed bars after a close strictly above the highest of the preceding 50 completed closes, otherwise flat. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: REQUIRED_BARS,
    })
}

#[derive(Clone, Debug)]
pub struct TradingRangeBreakout50dHold10Strategy {
    symbol: String,
}

impl TradingRangeBreakout50dHold10Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    /// Event on `window[j]`: `close[j] > max(close[j-50..=j-1])`, exact i64 comparison
    /// (equality is not an event; the current bar is excluded from the prior maximum).
    /// Requires `j >= PRIOR_BARS`.
    fn event(window: &[BarStub], j: usize) -> bool {
        window[j - PRIOR_BARS..j]
            .iter()
            .map(|b| b.close_micros)
            .max()
            .is_some_and(|prior_max| window[j].close_micros > prior_max)
    }

    /// `1` iff an event occurred on any of the latest ten completed bars; `0` otherwise, on
    /// fewer than 60 bars, and on any incomplete bar or non-positive close in the 60-bar window.
    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        let Some(window) = complete_positive_tail(recent, REQUIRED_BARS) else {
            return 0;
        };
        let latest = REQUIRED_BARS - 1;
        i64::from((0..HOLD_BARS).any(|k| Self::event(window, latest - k)))
    }
}

impl Strategy for TradingRangeBreakout50dHold10Strategy {
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
            .push_i64(PRIOR_BARS as i64)
            .push_i64(HOLD_BARS as i64)
            .push_i64(REQUIRED_BARS as i64)
            .push_str("event:close_strictly_greater_than_max_of_prior_50_closes")
            .push_str("current_bar:excluded_from_prior_maximum")
            .push_str("target:long_if_event_in_latest_10_bars_including_latest")
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

    const BASE: i64 = 100_000_000;

    fn bar(close_micros: i64, is_complete: bool) -> BarStub {
        BarStub::new(0, is_complete, close_micros, 1)
    }

    fn flat(n: usize, close: i64) -> Vec<BarStub> {
        (0..n).map(|_| bar(close, true)).collect()
    }

    fn sig(bars: &[BarStub]) -> i64 {
        TradingRangeBreakout50dHold10Strategy::signal_from_recent(bars)
    }

    fn ctx(bars: Vec<BarStub>) -> StrategyContext {
        let len = bars.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, bars))
    }

    /// 60 bars at `BASE` with one isolated event whose event bar is `k` bars before the latest:
    /// the event bar closes one micro above the prior maximum. Bars after the event bar sit
    /// BELOW the (raised) prior maximum, so no other event exists.
    fn series_with_event_at(k: usize) -> Vec<BarStub> {
        let mut v = flat(REQUIRED_BARS, BASE);
        let j = REQUIRED_BARS - 1 - k;
        v[j] = bar(BASE + 1, true);
        v
    }

    #[test]
    fn required_history_is_60_and_declared_in_meta() {
        let s = TradingRangeBreakout50dHold10Strategy::new("SPY");
        assert_eq!(s.required_history_bars(), 60);
        assert_eq!(REQUIRED_BARS, PRIOR_BARS + HOLD_BARS);
        assert_eq!(meta().data_requirements.unwrap().minimum_completed_bars, 60);
    }

    #[test]
    fn fewer_than_60_bars_is_flat_even_with_an_event_today_and_exactly_60_suffices() {
        let full = series_with_event_at(0);
        assert_eq!(sig(&full), 1, "exactly 60 valid bars suffice");
        assert_eq!(sig(&full[1..]), 0, "59 bars is insufficient");
        assert_eq!(sig(&[]), 0);
    }

    #[test]
    fn an_event_on_each_of_the_latest_ten_bars_keeps_the_target_long_and_older_does_not() {
        for k in 0..HOLD_BARS {
            assert_eq!(
                sig(&series_with_event_at(k)),
                1,
                "event {k} bars before the latest"
            );
        }
        // The event bar at t-10 needs closes t-60..t-11, which are not in the 60-bar window:
        // a t-10 event can never be inspected, so it cannot hold the target.
        assert_eq!(sig(&series_with_event_at(10)), 0);
        assert_eq!(sig(&series_with_event_at(11)), 0);
    }

    #[test]
    fn equality_with_the_prior_maximum_is_not_an_event_and_one_micro_above_is() {
        let build = |close: i64| {
            let mut v = flat(REQUIRED_BARS, BASE);
            v[REQUIRED_BARS - 1] = bar(close, true);
            v
        };
        assert_eq!(sig(&build(BASE)), 0, "close == prior maximum");
        assert_eq!(sig(&build(BASE + 1)), 1, "one micro above");
        assert_eq!(sig(&build(BASE - 1)), 0);
    }

    #[test]
    fn current_bar_is_excluded_and_the_oldest_prior_bar_binds() {
        // Latest bar closes at BASE+10; if the current bar were part of its own maximum there
        // could never be an event. The prior maximum is set by exactly one bar at varying age.
        let build = |peak_age: usize, peak: i64, latest: i64| {
            let mut v = flat(REQUIRED_BARS, BASE);
            let j = REQUIRED_BARS - 1; // event candidate = latest bar
            v[j - peak_age] = bar(peak, true);
            v[j] = bar(latest, true);
            v
        };
        // Peak 1 bar old above the latest close: no event at the latest bar; the peak bar
        // itself is an event only at its own age (k = 1).
        assert_eq!(
            sig(&build(1, BASE + 100, BASE + 10)),
            1,
            "peak bar is itself an event 1 bar ago"
        );
        // The oldest bar of the latest bar's 50-bar prior window is age 50: it still binds.
        // Keep every OTHER bar from being an event by making the peak exactly the latest close.
        let mut v = flat(REQUIRED_BARS, BASE);
        let j = REQUIRED_BARS - 1;
        v[j - 50] = bar(BASE + 10, true); // oldest prior bar of the latest event window
        v[j] = bar(BASE + 10, true);
        assert_eq!(
            sig(&v),
            0,
            "equal to the oldest prior bar: no event at the latest bar"
        );
        // v[j-50] is itself an event at k = 50? No: k only spans 0..10, and j-50 is outside.
        v[j - 50] = bar(BASE + 9, true);
        assert_eq!(
            sig(&v),
            1,
            "the oldest prior bar below the latest close lets it break out"
        );
    }

    #[test]
    fn a_bar_older_than_the_latest_60_cannot_matter() {
        for k in [0, 5, 9, 10] {
            let base = series_with_event_at(k);
            let mut with_huge = vec![bar(900_000_000_000, false); 25];
            with_huge.extend(base.clone());
            let mut with_neg = vec![bar(-5, true); 25];
            with_neg.extend(base.clone());
            assert_eq!(sig(&with_huge), sig(&base), "k={k}");
            assert_eq!(sig(&with_neg), sig(&base), "k={k}");
        }
    }

    #[test]
    fn incomplete_or_non_positive_required_bars_fail_closed_flat() {
        let base = series_with_event_at(0);
        assert_eq!(sig(&base), 1);
        let mut b = base.clone();
        *b.last_mut().unwrap() = bar(BASE + 1, false);
        assert_eq!(sig(&b), 0, "incomplete latest bar");
        let mut b = base.clone();
        b[10] = bar(BASE, false);
        assert_eq!(sig(&b), 0, "interior incomplete bar");
        let mut b = base.clone();
        b[0] = bar(BASE, false);
        assert_eq!(sig(&b), 0, "oldest required bar incomplete");
        let mut b = base.clone();
        b[10] = bar(0, true);
        assert_eq!(sig(&b), 0, "required zero close");
        let mut b = base.clone();
        b[REQUIRED_BARS - 2] = bar(-1, true);
        assert_eq!(sig(&b), 0, "required negative close");
    }

    /// Independent reference over a long deterministic walk with repeated breakouts: the target
    /// at every bar is "any event in the latest ten bars", overlapping events extend only
    /// through the rolling rule, and fresh instances equal the long-lived one.
    #[test]
    fn matches_an_independent_reference_with_overlapping_events_and_fresh_instances() {
        let n = 900;
        let wave: Vec<BarStub> = (0..n)
            .map(|i| {
                let drift = i as i64 * 90_000;
                let swing = (((i % 61) as i64) - 30).pow(2) * 12_000;
                let noise = (((i * 53) % 29) as i64 - 14) * 70_000;
                bar(100_000_000 + drift - swing + noise, true)
            })
            .collect();
        let reference_event = |j: usize| -> bool {
            let prior = (j - 50..j).map(|x| wave[x].close_micros).max().unwrap();
            wave[j].close_micros > prior
        };
        let mut long_lived = TradingRangeBreakout50dHold10Strategy::new("SPY");
        let (mut longs, mut flats, mut overlapped) = (0, 0, 0);
        for t in REQUIRED_BARS - 1..n {
            let events = (0..10).filter(|k| reference_event(t - k)).count();
            let expected = i64::from(events > 0);
            assert_eq!(sig(&wave[..=t]), expected, "bar {t}");
            let window = wave[..=t].to_vec();
            let a = long_lived.on_bar(&ctx(window.clone())).targets[0].qty.raw();
            let b = TradingRangeBreakout50dHold10Strategy::new("SPY")
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
            longs > 40 && flats > 40,
            "fixture must exercise both outcomes ({longs}/{flats})"
        );
        assert!(
            overlapped > 10,
            "fixture must contain overlapping events ({overlapped})"
        );
    }

    #[test]
    fn future_bars_are_not_used() {
        let all: Vec<BarStub> = (0..300)
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
        extended.push(bar(900_000_000_000, true));
        for t in REQUIRED_BARS - 1..all.len() {
            assert_eq!(sig(&extended[..=t]), sig(&all[..=t]), "bar {t}");
        }
    }

    #[test]
    fn never_shorts_and_emits_one_symbol_target() {
        let mut s = TradingRangeBreakout50dHold10Strategy::new("SPY");
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
        let out = s.on_bar(&ctx(series_with_event_at(0)));
        assert_eq!(out.targets.len(), 1);
        assert_eq!(out.targets[0].symbol, "SPY");
        assert_eq!(out.targets[0].qty.to_whole_units_checked().unwrap(), 1);
        for k in 0..12 {
            let out = s.on_bar(&ctx(series_with_event_at(k)));
            assert!(out.targets[0].qty.raw() >= 0);
        }
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_hex() {
        let a = TradingRangeBreakout50dHold10Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            a,
            TradingRangeBreakout50dHold10Strategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            a,
            TradingRangeBreakout50dHold10Strategy::new("EFA").semantic_fingerprint()
        );
        assert_eq!(a.len(), 64);
        assert!(a
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }

    /// Mutation proof: every behavior-changing semantic is bound.
    #[test]
    fn fingerprint_changes_when_any_semantic_field_changes() {
        let fp = |version: &str,
                  tf: i64,
                  prior: i64,
                  hold: i64,
                  required: i64,
                  event: &str,
                  cur: &str,
                  target: &str| {
            SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, version)
                .push_str("SPY")
                .push_i64(tf)
                .push_i64(prior)
                .push_i64(hold)
                .push_i64(required)
                .push_str(event)
                .push_str(cur)
                .push_str(target)
                .push_str("state:none_reconstructible_from_bounded_history")
                .push_str("direction:long_flat")
                .push_str("malformed_window:fail_closed_flat")
                .push_str("incomplete_latest:flat")
                .finish()
        };
        let event = "event:close_strictly_greater_than_max_of_prior_50_closes";
        let cur = "current_bar:excluded_from_prior_maximum";
        let target = "target:long_if_event_in_latest_10_bars_including_latest";
        let live = TradingRangeBreakout50dHold10Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            live,
            fp(VERSION, TIMEFRAME_SECS, 50, 10, 60, event, cur, target),
            "recipe mirrors the engine"
        );
        assert_ne!(
            live,
            fp(VERSION, TIMEFRAME_SECS, 49, 10, 59, event, cur, target)
        );
        assert_ne!(
            live,
            fp(VERSION, TIMEFRAME_SECS, 51, 10, 61, event, cur, target)
        );
        assert_ne!(
            live,
            fp(VERSION, TIMEFRAME_SECS, 50, 9, 59, event, cur, target)
        );
        assert_ne!(
            live,
            fp(VERSION, TIMEFRAME_SECS, 50, 11, 61, event, cur, target)
        );
        assert_ne!(
            live,
            fp(
                VERSION,
                TIMEFRAME_SECS,
                50,
                10,
                60,
                "event:close_greater_or_equal_max_of_prior_50_closes",
                cur,
                target
            )
        );
        assert_ne!(
            live,
            fp(
                VERSION,
                TIMEFRAME_SECS,
                50,
                10,
                60,
                event,
                "current_bar:included_in_prior_maximum",
                target
            )
        );
        assert_ne!(
            live,
            fp(
                VERSION,
                TIMEFRAME_SECS,
                50,
                10,
                60,
                event,
                cur,
                "target:long_if_event_in_latest_9_bars_including_latest"
            )
        );
        assert_ne!(
            live,
            fp("0.1.1", TIMEFRAME_SECS, 50, 10, 60, event, cur, target)
        );
        assert_ne!(live, fp(VERSION, 3_600, 50, 10, 60, event, cur, target));
    }
}
