use super::daily_math::close;
use super::monthly::{month_end_indices, MAX_SESSIONS_SINCE_MONTH_END};
use super::session_calendar::push_calendar_identity;
use super::window::complete_positive_tail;
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "monthly_52week_high_proximity_v1";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// The 52-week high is the highest close of this many sessions ending at, and including, the
/// decision month-end.
const HIGH_SESSIONS: usize = 252;
/// LONG iff `close * 100 >= high * PROXIMITY_PERCENT` (within 5% of the high, inclusive).
const PROXIMITY_PERCENT: i128 = 95;
/// The high window ends at the decision month-end; add the longest wait for the next one.
const REQUIRED_BARS: usize = HIGH_SESSIONS + MAX_SESSIONS_SINCE_MONTH_END;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic monthly long/flat 52-week-high proximity: at each month-end, long when the close is at least 95% of the highest close of the last 252 sessions (inclusive), held until the next month-end. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: REQUIRED_BARS,
    })
}

#[derive(Clone, Debug)]
pub struct Monthly52WeekHighProximityV1Strategy {
    symbol: String,
}

impl Monthly52WeekHighProximityV1Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    /// The decision taken at the latest month-end `m` in the window: `1` iff
    /// `close[m] * 100 >= max(close[m-251..=m]) * 95` (integer-exact). `0` on every refusal.
    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        let Some(win) = complete_positive_tail(recent, REQUIRED_BARS) else {
            return 0;
        };
        let Some(m) = month_end_indices(win).and_then(|me| me.last().copied()) else {
            return 0;
        };
        if m + 1 < HIGH_SESSIONS {
            return 0;
        }
        let Some(high) = win[m + 1 - HIGH_SESSIONS..=m].iter().map(close).max() else {
            return 0;
        };
        i64::from(close(&win[m]) * 100 >= high * PROXIMITY_PERCENT)
    }
}

impl Strategy for Monthly52WeekHighProximityV1Strategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(NAME, TIMEFRAME_SECS)
    }

    fn required_history_bars(&self) -> usize {
        REQUIRED_BARS
    }

    fn semantic_fingerprint(&self) -> String {
        let mut b = SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, VERSION);
        b.push_str(&self.symbol).push_i64(TIMEFRAME_SECS);
        push_calendar_identity(&mut b);
        b.push_i64(HIGH_SESSIONS as i64)
            .push_i64(PROXIMITY_PERCENT as i64)
            .push_i64(REQUIRED_BARS as i64)
            .push_str("decision:latest_month_end_session_in_window")
            .push_str("high:max_close_of_high_sessions_ending_at_and_including_decision_bar")
            .push_str("target:long_if_close_at_least_proximity_percent_of_high_inclusive_held_to_next_month_end")
            .push_str("state:none_reconstructible_from_bounded_history")
            .push_str("direction:long_flat")
            .push_str("malformed_window:fail_closed_flat")
            .push_str("noncontiguous_or_uncovered_sessions:fail_closed_flat")
            .push_str("incomplete_latest:flat");
        b.finish()
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
    use super::super::monthly::test_support::series;
    use super::super::session_calendar::test_support::*;
    use super::*;
    use crate::RecentBarsWindow;

    const HIGH: i64 = 200_000_000;
    /// 95% of HIGH.
    const EDGE: i64 = 190_000_000;

    /// Month-end 2024-06-28 is the last bar; all earlier closes sit at `floor`, except the
    /// session at offset `high_off` before the month-end which carries `HIGH`.
    fn at_month_end(c: i64, high_off: usize, floor: i64) -> Vec<BarStub> {
        let m = REQUIRED_BARS - 1;
        series(d(2024, 6, 28), REQUIRED_BARS, |i| match m - i {
            0 => c,
            o if o == high_off => HIGH,
            _ => floor,
        })
    }

    fn sig(b: &[BarStub]) -> i64 {
        Monthly52WeekHighProximityV1Strategy::signal_from_recent(b)
    }

    #[test]
    fn required_history_is_274_in_both_authorities() {
        let s = Monthly52WeekHighProximityV1Strategy::new("SPY");
        assert_eq!(REQUIRED_BARS, 274);
        assert_eq!(s.required_history_bars(), 274);
        assert_eq!(
            meta().data_requirements.unwrap().minimum_completed_bars,
            274
        );
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
    }

    #[test]
    fn the_95_percent_boundary_is_inclusive_and_integer_exact() {
        assert_eq!(sig(&at_month_end(EDGE, 100, 1)), 1, "exactly 95%");
        assert_eq!(sig(&at_month_end(EDGE - 1, 100, 1)), 0, "just under 95%");
        assert_eq!(sig(&at_month_end(EDGE + 1, 100, 1)), 1);
        assert_eq!(sig(&at_month_end(HIGH, 100, 1)), 1, "at the high itself");
        // A truncating division (190_000_000 / 100 * 95 style) would misplace this odd edge.
        let odd_high = 199_999_999;
        let m = REQUIRED_BARS - 1;
        let w = |c: i64| {
            series(d(2024, 6, 28), REQUIRED_BARS, |i| match m - i {
                0 => c,
                100 => odd_high,
                _ => 1,
            })
        };
        // 95% of 199_999_999 = 189_999_999.05 -> 189_999_999 is below, 190_000_000 is above.
        assert_eq!(sig(&w(189_999_999)), 0);
        assert_eq!(sig(&w(190_000_000)), 1);
    }

    #[test]
    fn the_high_window_is_252_sessions_inclusive_of_the_decision_bar() {
        // The oldest bar of the 252-session window (offset 251) counts; offset 252 does not.
        assert_eq!(sig(&at_month_end(EDGE - 1, 251, 1)), 0);
        assert_eq!(
            sig(&at_month_end(EDGE - 1, 252, 1)),
            1,
            "high just outside the window"
        );
        // Nothing older than the window can lower the bar.
        assert_eq!(
            sig(&at_month_end(1, 252, 1)),
            1,
            "close is its own 252-session high"
        );
    }

    #[test]
    fn the_target_is_held_between_month_ends() {
        let mut long_then = at_month_end(EDGE, 100, 1);
        for next in [d(2024, 7, 1), d(2024, 7, 2), d(2024, 7, 3)] {
            long_then.remove(0);
            long_then.push(bar(next, 1, true));
            assert_eq!(sig(&long_then), 1);
        }
        let mut flat_then = at_month_end(EDGE - 1, 100, 1);
        flat_then.remove(0);
        flat_then.push(bar(d(2024, 7, 1), 10 * HIGH, true));
        assert_eq!(sig(&flat_then), 0);
    }

    #[test]
    fn short_malformed_noncontiguous_and_incomplete_windows_are_flat() {
        let good = at_month_end(EDGE, 100, 1);
        assert_eq!(sig(&good), 1);
        assert_eq!(sig(&good[1..]), 0, "273 bars");
        assert_eq!(sig(&[]), 0);
        let mut bad = good.clone();
        bad[100].close_micros = 0;
        assert_eq!(sig(&bad), 0, "non-positive close");
        let mut bad = good.clone();
        bad[100].is_complete = false;
        assert_eq!(sig(&bad), 0, "incomplete interior bar");
        let mut bad = good.clone();
        *bad.last_mut().unwrap() = bar(d(2024, 6, 28), EDGE, false);
        assert_eq!(sig(&bad), 0, "incomplete latest bar");
        let mut gap = good.clone();
        gap.remove(120);
        gap.insert(0, bar(d(2023, 1, 3), 1, true));
        assert_eq!(sig(&gap), 0, "missing session inside the window");
        let mut off = good;
        off[0].end_ts += 3600;
        assert_eq!(sig(&off), 0, "non-midnight label");
    }

    #[test]
    fn bars_older_than_the_required_window_do_not_matter_and_on_bar_emits_one_share() {
        let good = at_month_end(EDGE, 100, 1);
        let mut longer = vec![bar(d(2020, 1, 2), 10 * HIGH, true)];
        longer.extend(good.clone());
        assert_eq!(sig(&longer), sig(&good));
        let mut s = Monthly52WeekHighProximityV1Strategy::new("SPY");
        let ctx = StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(good.len(), good));
        assert_eq!(s.on_bar(&ctx).targets[0].qty.raw(), 1_000_000);
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_changes_with_every_field() {
        let live = Monthly52WeekHighProximityV1Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(live.len(), 64);
        assert_eq!(
            live,
            Monthly52WeekHighProximityV1Strategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            live,
            Monthly52WeekHighProximityV1Strategy::new("QQQ").semantic_fingerprint()
        );
        let fp = |version: &str, nums: [i64; 3], tokens: [&str; 8]| {
            let mut b = SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, version);
            b.push_str("SPY").push_i64(TIMEFRAME_SECS);
            push_calendar_identity(&mut b);
            for n in nums {
                b.push_i64(n);
            }
            for t in tokens {
                b.push_str(t);
            }
            b.finish()
        };
        let nums = [252, 95, 274];
        let tokens = [
            "decision:latest_month_end_session_in_window",
            "high:max_close_of_high_sessions_ending_at_and_including_decision_bar",
            "target:long_if_close_at_least_proximity_percent_of_high_inclusive_held_to_next_month_end",
            "state:none_reconstructible_from_bounded_history",
            "direction:long_flat",
            "malformed_window:fail_closed_flat",
            "noncontiguous_or_uncovered_sessions:fail_closed_flat",
            "incomplete_latest:flat",
        ];
        assert_eq!(live, fp(VERSION, nums, tokens), "recipe mirrors the engine");
        assert_ne!(live, fp("0.1.1", nums, tokens));
        for i in 0..nums.len() {
            let mut m = nums;
            m[i] += 1;
            assert_ne!(live, fp(VERSION, m, tokens), "number {i}");
        }
        for i in 0..tokens.len() {
            let mut m = tokens;
            m[i] = "mutated";
            assert_ne!(live, fp(VERSION, nums, m), "token {i}");
        }
    }
}
