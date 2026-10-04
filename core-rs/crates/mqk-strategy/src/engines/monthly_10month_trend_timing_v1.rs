use super::daily_math::close;
use super::monthly::{month_end_indices, MAX_SESSIONS_PER_MONTH, MAX_SESSIONS_SINCE_MONTH_END};
use super::session_calendar::push_calendar_identity;
use super::window::complete_positive_tail;
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "monthly_10month_trend_timing_v1";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// The mean is taken over this many month-end closes immediately BEFORE the decision month-end.
const PRIOR_MONTH_ENDS: usize = 10;
/// The oldest of the prior month-ends is at most `PRIOR_MONTH_ENDS` months before the decision
/// month-end, plus the longest wait for the next month-end.
const REQUIRED_BARS: usize =
    PRIOR_MONTH_ENDS * MAX_SESSIONS_PER_MONTH + 1 + MAX_SESSIONS_SINCE_MONTH_END;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic monthly long/flat 10-month trend timing: at each month-end, long when the close is strictly above the mean of the prior 10 month-end closes (current excluded), held until the next month-end. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: REQUIRED_BARS,
    })
}

#[derive(Clone, Debug)]
pub struct Monthly10MonthTrendTimingV1Strategy {
    symbol: String,
}

impl Monthly10MonthTrendTimingV1Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    /// The decision taken at the latest month-end `m` in the window: `1` iff
    /// `close[m] * 10 > sum(close of the 10 month-ends before m)`. `0` on every refusal.
    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        let Some(win) = complete_positive_tail(recent, REQUIRED_BARS) else {
            return 0;
        };
        let Some(me) = month_end_indices(win) else {
            return 0;
        };
        let Some((&m, prior)) = me.split_last() else {
            return 0;
        };
        if prior.len() < PRIOR_MONTH_ENDS {
            return 0;
        }
        let sum: i128 = prior[prior.len() - PRIOR_MONTH_ENDS..]
            .iter()
            .map(|&i| close(&win[i]))
            .sum();
        i64::from(close(&win[m]) * PRIOR_MONTH_ENDS as i128 > sum)
    }
}

impl Strategy for Monthly10MonthTrendTimingV1Strategy {
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
        b.push_i64(PRIOR_MONTH_ENDS as i64)
            .push_i64(REQUIRED_BARS as i64)
            .push_str("decision:latest_month_end_session_in_window")
            .push_str("mean:prior_month_end_closes_current_month_end_excluded")
            .push_str("target:long_if_close_strictly_above_mean_held_to_next_month_end")
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
    use super::super::monthly::test_support::tape_with_decision;
    use super::super::session_calendar::test_support::*;
    use super::*;
    use crate::RecentBarsWindow;

    const BASE: i64 = 100_000_000;
    /// Every non-month-end close: far from every month-end close, so a mean over all sessions
    /// (or any non-month-end read) would flip the decision.
    const NOISE: i64 = 1_000 * BASE;

    fn sig(b: &[BarStub]) -> i64 {
        Monthly10MonthTrendTimingV1Strategy::signal_from_recent(b)
    }

    /// Tape, the decision month-end index `m`, the next month-end index, and the 10 prior
    /// month-end indices of `m` (ascending), with prior closes `prior[k]` and `m` closing `c`.
    fn setup(c: i64, prior: [i64; 10]) -> (Vec<BarStub>, usize, usize, [usize; 10]) {
        let (mut tape, m, next) = tape_with_decision(REQUIRED_BARS, NOISE);
        let me = month_end_indices(&tape).unwrap();
        let pos = me.iter().position(|&i| i == m).unwrap();
        let idx: [usize; 10] = me[pos - 10..pos].try_into().unwrap();
        for (k, &i) in idx.iter().enumerate() {
            tape[i].close_micros = prior[k];
        }
        tape[m].close_micros = c;
        (tape, m, next, idx)
    }

    fn window(tape: &[BarStub], e: usize) -> &[BarStub] {
        &tape[e + 1 - REQUIRED_BARS..=e]
    }

    #[test]
    fn required_history_is_253_in_both_authorities() {
        let s = Monthly10MonthTrendTimingV1Strategy::new("SPY");
        assert_eq!(REQUIRED_BARS, 253);
        assert_eq!(s.required_history_bars(), 253);
        assert_eq!(
            meta().data_requirements.unwrap().minimum_completed_bars,
            253
        );
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
    }

    #[test]
    fn long_only_when_strictly_above_the_prior_ten_month_end_mean() {
        let (t, m, _, _) = setup(BASE + 1, [BASE; 10]);
        assert_eq!(sig(window(&t, m)), 1);
        let (t, m, _, _) = setup(BASE, [BASE; 10]);
        assert_eq!(sig(window(&t, m)), 0, "equality is flat");
        let (t, m, _, _) = setup(BASE - 1, [BASE; 10]);
        assert_eq!(sig(window(&t, m)), 0);
        // Integer-exact mean: ten closes summing to 10*BASE + 9 -> mean BASE + 0.9.
        let mut p = [BASE; 10];
        p[0] += 9;
        let (t, m, _, _) = setup(BASE, p);
        assert_eq!(sig(window(&t, m)), 0, "BASE <= BASE + 0.9");
        let (t, m, _, _) = setup(BASE + 1, p);
        assert_eq!(sig(window(&t, m)), 1, "BASE + 1 > BASE + 0.9");
    }

    #[test]
    fn the_current_month_end_is_excluded_and_the_oldest_prior_month_end_is_included() {
        // Oldest prior month-end huge: the 10-prior mean is high -> flat. A mean over the latest
        // ten month-ends that includes the current close and drops the oldest would go long.
        let mut p = [BASE; 10];
        p[0] = 50 * BASE;
        let (t, m, _, _) = setup(BASE + 1, p);
        assert_eq!(sig(window(&t, m)), 0);
        // The newest prior month-end carries equal weight.
        let mut p = [BASE; 10];
        p[9] = 50 * BASE;
        let (t, m, _, _) = setup(BASE + 1, p);
        assert_eq!(sig(window(&t, m)), 0);
    }

    #[test]
    fn only_month_end_closes_are_read_never_other_sessions() {
        let (t, m, _, idx) = setup(BASE + 1, [BASE; 10]);
        assert!(t[idx[0] + 1].close_micros == NOISE && t[m - 1].close_micros == NOISE);
        assert_eq!(sig(window(&t, m)), 1);
    }

    #[test]
    fn the_decision_is_held_until_the_next_month_end_regardless_of_later_closes() {
        let (t, m, next, _) = setup(BASE + 1, [BASE; 10]);
        for e in m..next {
            assert_eq!(
                sig(window(&t, e)),
                1,
                "session {} after the month-end",
                e - m
            );
        }
        let (t, m, next, _) = setup(BASE, [BASE; 10]);
        for e in m..next {
            assert_eq!(
                sig(window(&t, e)),
                0,
                "session {} after the month-end",
                e - m
            );
        }
    }

    #[test]
    fn short_malformed_noncontiguous_and_incomplete_windows_are_flat() {
        let (t, m, _, _) = setup(BASE + 1, [BASE; 10]);
        let good = window(&t, m).to_vec();
        assert_eq!(sig(&good), 1);
        assert_eq!(sig(&good[1..]), 0, "252 bars");
        assert_eq!(sig(&[]), 0);
        let mut bad = good.clone();
        bad[100].close_micros = 0;
        assert_eq!(sig(&bad), 0, "non-positive close");
        let mut bad = good.clone();
        bad[100].is_complete = false;
        assert_eq!(sig(&bad), 0, "incomplete interior bar");
        let mut bad = good.clone();
        bad.last_mut().unwrap().is_complete = false;
        assert_eq!(sig(&bad), 0, "incomplete latest bar");
        let mut gap = good.clone();
        gap.remove(120);
        gap.insert(0, bar(d(2020, 1, 2), BASE, true));
        assert_eq!(sig(&gap), 0, "missing session inside the window");
        let mut off = good;
        off[0].end_ts += 3600;
        assert_eq!(sig(&off), 0, "non-midnight label");
    }

    #[test]
    fn the_mean_is_unchanged_by_older_bars_and_future_bars_cannot_change_a_decision() {
        let (t, m, _, _) = setup(BASE + 1, [BASE; 10]);
        let w = window(&t, m).to_vec();
        let mut older = vec![bar(d(2019, 1, 2), 1, true)];
        older.extend(w.clone());
        assert_eq!(sig(&older), sig(&w));
        let mut s = Monthly10MonthTrendTimingV1Strategy::new("SPY");
        let ctx = StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(w.len(), w));
        assert_eq!(s.on_bar(&ctx).targets[0].qty.raw(), 1_000_000);
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_hex() {
        let a = Monthly10MonthTrendTimingV1Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            a,
            Monthly10MonthTrendTimingV1Strategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            a,
            Monthly10MonthTrendTimingV1Strategy::new("QQQ").semantic_fingerprint()
        );
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn fingerprint_changes_when_any_semantic_field_changes() {
        let fp = |version: &str, months: i64, required: i64, tokens: [&str; 8]| {
            let mut b = SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, version);
            b.push_str("SPY").push_i64(TIMEFRAME_SECS);
            push_calendar_identity(&mut b);
            b.push_i64(months).push_i64(required);
            for t in tokens {
                b.push_str(t);
            }
            b.finish()
        };
        let tokens = [
            "decision:latest_month_end_session_in_window",
            "mean:prior_month_end_closes_current_month_end_excluded",
            "target:long_if_close_strictly_above_mean_held_to_next_month_end",
            "state:none_reconstructible_from_bounded_history",
            "direction:long_flat",
            "malformed_window:fail_closed_flat",
            "noncontiguous_or_uncovered_sessions:fail_closed_flat",
            "incomplete_latest:flat",
        ];
        let live = Monthly10MonthTrendTimingV1Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            live,
            fp(VERSION, 10, 253, tokens),
            "recipe mirrors the engine"
        );
        assert_ne!(live, fp(VERSION, 9, 253, tokens));
        assert_ne!(live, fp(VERSION, 10, 252, tokens));
        assert_ne!(live, fp("0.1.1", 10, 253, tokens));
        for i in 0..tokens.len() {
            let mut mutated = tokens;
            mutated[i] = "mutated";
            assert_ne!(live, fp(VERSION, 10, 253, mutated), "token {i}");
        }
    }
}
