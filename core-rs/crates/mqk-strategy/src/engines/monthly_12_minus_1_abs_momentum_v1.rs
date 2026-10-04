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

pub(crate) const NAME: &str = "monthly_12_minus_1_abs_momentum_v1";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// The recent leg ends this many sessions before the decision month-end (the skipped month).
const SKIP_SESSIONS: usize = 21;
/// The lookback leg starts this many sessions before the decision month-end.
const LOOKBACK_SESSIONS: usize = 252;
/// The deepest lookback before the decision month-end, plus the longest wait for the next one.
const REQUIRED_BARS: usize = LOOKBACK_SESSIONS + 1 + MAX_SESSIONS_SINCE_MONTH_END;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic monthly long/flat 12-minus-1 absolute momentum: at each month-end, long when the close 21 sessions earlier is strictly above the close 252 sessions earlier (the latest month skipped), held until the next month-end. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: REQUIRED_BARS,
    })
}

#[derive(Clone, Debug)]
pub struct Monthly12Minus1AbsMomentumV1Strategy {
    symbol: String,
}

impl Monthly12Minus1AbsMomentumV1Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    /// The decision taken at the latest month-end `m` in the window: `1` iff
    /// `close[m - 21] > close[m - 252]`; the decision-bar close is never read. `0` on every
    /// refusal.
    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        let Some(win) = complete_positive_tail(recent, REQUIRED_BARS) else {
            return 0;
        };
        let Some(m) = month_end_indices(win).and_then(|me| me.last().copied()) else {
            return 0;
        };
        if m < LOOKBACK_SESSIONS {
            return 0;
        }
        i64::from(close(&win[m - SKIP_SESSIONS]) > close(&win[m - LOOKBACK_SESSIONS]))
    }
}

impl Strategy for Monthly12Minus1AbsMomentumV1Strategy {
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
        b.push_i64(SKIP_SESSIONS as i64)
            .push_i64(LOOKBACK_SESSIONS as i64)
            .push_i64(REQUIRED_BARS as i64)
            .push_str("decision:latest_month_end_session_in_window")
            .push_str(
                "signal:close_skip_sessions_before_strictly_above_close_lookback_sessions_before",
            )
            .push_str("decision_bar_close:not_read")
            .push_str("target:long_if_signal_held_to_next_month_end")
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

    const BASE: i64 = 100_000_000;
    const FAR: i64 = 1_000 * BASE;

    /// Month-end 2024-06-28 is the last bar; only the three read offsets carry meaning.
    fn at_month_end(c: i64, c21: i64, c252: i64) -> Vec<BarStub> {
        let m = REQUIRED_BARS - 1;
        series(d(2024, 6, 28), REQUIRED_BARS, |i| match m - i {
            0 => c,
            21 => c21,
            252 => c252,
            _ => FAR,
        })
    }

    fn sig(b: &[BarStub]) -> i64 {
        Monthly12Minus1AbsMomentumV1Strategy::signal_from_recent(b)
    }

    #[test]
    fn required_history_is_275_in_both_authorities() {
        let s = Monthly12Minus1AbsMomentumV1Strategy::new("SPY");
        assert_eq!(REQUIRED_BARS, 275);
        assert_eq!(s.required_history_bars(), 275);
        assert_eq!(
            meta().data_requirements.unwrap().minimum_completed_bars,
            275
        );
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
    }

    #[test]
    fn long_only_when_the_skipped_month_close_is_strictly_above_the_year_ago_close() {
        assert_eq!(sig(&at_month_end(BASE, BASE + 1, BASE)), 1);
        assert_eq!(sig(&at_month_end(BASE, BASE, BASE)), 0, "equality is flat");
        assert_eq!(sig(&at_month_end(BASE, BASE - 1, BASE)), 0);
    }

    #[test]
    fn the_decision_bar_close_is_never_read() {
        // A current close far above the year-ago close must not vote; far below must not veto.
        assert_eq!(sig(&at_month_end(FAR, BASE, BASE)), 0);
        assert_eq!(sig(&at_month_end(FAR, BASE + 1, BASE)), 1);
        assert_eq!(sig(&at_month_end(1, BASE + 1, BASE)), 1);
    }

    #[test]
    fn only_the_two_declared_offsets_are_read() {
        // Neighbours of both offsets are adversarial in both directions.
        let m = REQUIRED_BARS - 1;
        let mut w = at_month_end(BASE, BASE + 1, BASE);
        for off in [20, 22] {
            w[m - off].close_micros = 1;
        }
        for off in [251, 253] {
            w[m - off].close_micros = FAR;
        }
        assert_eq!(sig(&w), 1);
    }

    #[test]
    fn the_target_is_held_between_month_ends() {
        let mut long_then = at_month_end(BASE, BASE + 1, BASE);
        for next in [d(2024, 7, 1), d(2024, 7, 2), d(2024, 7, 3)] {
            long_then.remove(0);
            long_then.push(bar(next, 1, true));
            assert_eq!(sig(&long_then), 1);
        }
        let mut flat_then = at_month_end(BASE, BASE, BASE);
        flat_then.remove(0);
        flat_then.push(bar(d(2024, 7, 1), FAR, true));
        assert_eq!(sig(&flat_then), 0);
    }

    #[test]
    fn short_malformed_noncontiguous_and_incomplete_windows_are_flat() {
        let good = at_month_end(BASE, BASE + 1, BASE);
        assert_eq!(sig(&good), 1);
        assert_eq!(sig(&good[1..]), 0, "274 bars");
        assert_eq!(sig(&[]), 0);
        let mut bad = good.clone();
        bad[100].close_micros = 0;
        assert_eq!(sig(&bad), 0, "non-positive close");
        let mut bad = good.clone();
        bad[100].is_complete = false;
        assert_eq!(sig(&bad), 0, "incomplete interior bar");
        let mut bad = good.clone();
        *bad.last_mut().unwrap() = bar(d(2024, 6, 28), BASE, false);
        assert_eq!(sig(&bad), 0, "incomplete latest bar");
        let mut gap = good.clone();
        gap.remove(120);
        gap.insert(0, bar(d(2023, 1, 3), BASE, true));
        assert_eq!(sig(&gap), 0, "missing session inside the window");
        let mut off = good;
        off[0].end_ts += 3600;
        assert_eq!(sig(&off), 0, "non-midnight label");
    }

    #[test]
    fn bars_older_than_the_required_window_do_not_matter_and_on_bar_emits_one_share() {
        let good = at_month_end(BASE, BASE + 1, BASE);
        let mut longer = vec![bar(d(2020, 1, 2), 1, true)];
        longer.extend(good.clone());
        assert_eq!(sig(&longer), sig(&good));
        let mut s = Monthly12Minus1AbsMomentumV1Strategy::new("SPY");
        let ctx = StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(good.len(), good));
        assert_eq!(s.on_bar(&ctx).targets[0].qty.raw(), 1_000_000);
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_changes_with_every_field() {
        let live = Monthly12Minus1AbsMomentumV1Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(live.len(), 64);
        assert_eq!(
            live,
            Monthly12Minus1AbsMomentumV1Strategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            live,
            Monthly12Minus1AbsMomentumV1Strategy::new("QQQ").semantic_fingerprint()
        );
        let fp = |version: &str, nums: [i64; 3], tokens: [&str; 9]| {
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
        let nums = [21, 252, 275];
        let tokens = [
            "decision:latest_month_end_session_in_window",
            "signal:close_skip_sessions_before_strictly_above_close_lookback_sessions_before",
            "decision_bar_close:not_read",
            "target:long_if_signal_held_to_next_month_end",
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
