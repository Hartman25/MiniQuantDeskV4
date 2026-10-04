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

pub(crate) const NAME: &str = "monthly_multihorizon_abs_momentum_consensus_v1";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// Vote horizons in sessions: the month-end close must be strictly above the close this many
/// sessions earlier.
const HORIZONS: [usize; 3] = [21, 63, 252];
/// LONG iff at least this many horizons vote up.
const MIN_VOTES: usize = 2;
/// The deepest horizon before the decision month-end, plus the longest wait for the next one.
const REQUIRED_BARS: usize = 252 + 1 + MAX_SESSIONS_SINCE_MONTH_END;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic monthly long/flat multi-horizon absolute momentum: at each month-end, long when the close is above its close 21, 63 and 252 sessions earlier on at least two of the three, held until the next month-end. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: REQUIRED_BARS,
    })
}

#[derive(Clone, Debug)]
pub struct MonthlyMultihorizonAbsMomentumConsensusV1Strategy {
    symbol: String,
}

impl MonthlyMultihorizonAbsMomentumConsensusV1Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    /// The decision taken at the latest month-end `m` in the window: `1` iff at least
    /// `MIN_VOTES` of the strict inequalities `close[m] > close[m - h]` hold. `0` on every
    /// refusal (short window, malformed bar, non-contiguous or uncovered sessions, no
    /// month-end with the deepest horizon available).
    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        let Some(win) = complete_positive_tail(recent, REQUIRED_BARS) else {
            return 0;
        };
        let Some(m) = month_end_indices(win).and_then(|me| me.last().copied()) else {
            return 0;
        };
        let deepest = HORIZONS[HORIZONS.len() - 1];
        if m < deepest {
            return 0;
        }
        let votes = HORIZONS
            .iter()
            .filter(|&&h| close(&win[m]) > close(&win[m - h]))
            .count();
        i64::from(votes >= MIN_VOTES)
    }
}

impl Strategy for MonthlyMultihorizonAbsMomentumConsensusV1Strategy {
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
        for h in HORIZONS {
            b.push_i64(h as i64);
        }
        b.push_i64(MIN_VOTES as i64)
            .push_i64(REQUIRED_BARS as i64)
            .push_str("decision:latest_month_end_session_in_window")
            .push_str("vote:close_strictly_greater_than_close_h_sessions_earlier")
            .push_str("target:long_if_votes_ge_min_votes_held_to_next_month_end")
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
    use super::super::session_calendar::test_support::*;
    use super::*;
    use crate::RecentBarsWindow;
    use chrono::NaiveDate;
    use mqk_integrity::sessions;

    const BASE: i64 = 100_000_000;

    /// `n` consecutive regular sessions ending on `last`, closes from `f(index_from_start)`.
    fn series(last: NaiveDate, n: usize, f: impl Fn(usize) -> i64) -> Vec<BarStub> {
        let mut dates = vec![last];
        while dates.len() < n {
            let mut x = dates.last().unwrap().pred_opt().unwrap();
            while !sessions::is_session(x).unwrap() {
                x = x.pred_opt().unwrap();
            }
            dates.push(x);
        }
        dates.reverse();
        dates
            .into_iter()
            .enumerate()
            .map(|(i, dt)| bar(dt, f(i), true))
            .collect()
    }

    /// Month-end 2024-06-28 is the last bar; vote closes are set by horizon at that bar.
    fn at_month_end(c: i64, c21: i64, c63: i64, c252: i64) -> Vec<BarStub> {
        let m = REQUIRED_BARS - 1;
        series(d(2024, 6, 28), REQUIRED_BARS, |i| match m - i {
            0 => c,
            21 => c21,
            63 => c63,
            252 => c252,
            _ => 1_000 * BASE, // far above/below nothing: never read
        })
    }

    fn sig(b: &[BarStub]) -> i64 {
        MonthlyMultihorizonAbsMomentumConsensusV1Strategy::signal_from_recent(b)
    }

    fn ctx(bars: Vec<BarStub>) -> StrategyContext {
        let len = bars.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, bars))
    }

    #[test]
    fn required_history_is_275_in_both_authorities() {
        let s = MonthlyMultihorizonAbsMomentumConsensusV1Strategy::new("SPY");
        assert_eq!(REQUIRED_BARS, 275);
        assert_eq!(s.required_history_bars(), 275);
        assert_eq!(
            meta().data_requirements.unwrap().minimum_completed_bars,
            275
        );
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
    }

    #[test]
    fn two_of_three_votes_is_long_and_one_is_not() {
        let up = BASE + 1;
        // (c21 below, c63 below, c252 above) -> 2 votes.
        assert_eq!(sig(&at_month_end(up, BASE, BASE, up + 5)), 1);
        assert_eq!(sig(&at_month_end(up, BASE, up + 5, BASE)), 1);
        assert_eq!(sig(&at_month_end(up, up + 5, BASE, BASE)), 1);
        assert_eq!(sig(&at_month_end(up, BASE, BASE, BASE)), 1, "3 votes");
        // exactly one vote
        assert_eq!(sig(&at_month_end(up, BASE, up + 5, up + 5)), 0);
        assert_eq!(sig(&at_month_end(up, up + 5, BASE, up + 5)), 0);
        assert_eq!(sig(&at_month_end(up, up + 5, up + 5, BASE)), 0);
        assert_eq!(sig(&at_month_end(up, up + 5, up + 5, up + 5)), 0, "0 votes");
    }

    #[test]
    fn a_vote_needs_a_strictly_higher_close_equality_does_not_vote() {
        // Two horizons equal (not up), one up: 1 vote -> flat; one more strict -> 2 votes.
        assert_eq!(sig(&at_month_end(BASE, BASE, BASE, BASE - 1)), 0);
        assert_eq!(sig(&at_month_end(BASE, BASE, BASE - 1, BASE - 1)), 1);
    }

    #[test]
    fn the_target_is_held_between_month_ends_from_the_latest_month_end_decision() {
        // Same price shape ending on a month-end vs 3 and 10 sessions later: the later bars
        // reuse the month-end decision and ignore their own closes.
        let up = BASE + 1;
        let mut long_then = at_month_end(up, BASE, BASE, BASE);
        for (k, next) in [d(2024, 7, 1), d(2024, 7, 2), d(2024, 7, 3)]
            .into_iter()
            .enumerate()
        {
            long_then.remove(0);
            long_then.push(bar(next, 1, true)); // a crash after the month-end
            assert_eq!(sig(&long_then), 1, "day {} after the month-end", k + 1);
        }
        let mut flat_then = at_month_end(BASE, BASE, BASE, BASE);
        assert_eq!(sig(&flat_then), 0);
        flat_then.remove(0);
        flat_then.push(bar(d(2024, 7, 1), 1_000 * BASE, true)); // a rally after the month-end
        assert_eq!(sig(&flat_then), 0);
    }

    #[test]
    fn the_longest_wait_for_the_next_month_end_still_resolves_the_decision() {
        // August 2023 has 23 sessions: its 22nd session (Aug 30) is the latest non-month-end
        // bar, 22 sessions after the July month-end, which then sits at the deepest index.
        let up = BASE + 1;
        let full = series(d(2023, 9, 1), 400, |_| BASE);
        let m_date = d(2023, 7, 31);
        let pos = full.iter().position(|b| b.end_ts == label(m_date)).unwrap();
        let mut bars = full.clone();
        bars[pos].close_micros = up;
        for (h, v) in [(21, BASE), (63, BASE), (252, BASE)] {
            bars[pos - h].close_micros = v;
        }
        let end = pos + MAX_SESSIONS_SINCE_MONTH_END;
        assert_eq!(full[end].end_ts, label(d(2023, 8, 30)));
        let window = &bars[end + 1 - REQUIRED_BARS..=end];
        assert_eq!(sig(window), 1);
    }

    #[test]
    fn short_malformed_noncontiguous_and_incomplete_windows_are_flat() {
        let up = BASE + 1;
        let good = at_month_end(up, BASE, BASE, BASE);
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
        *bad.last_mut().unwrap() = bar(d(2024, 6, 28), up, false);
        assert_eq!(sig(&bad), 0, "incomplete latest bar");
        let mut gap = good.clone();
        gap.remove(120);
        gap.insert(0, bar(d(2023, 1, 3), BASE, true));
        assert_eq!(sig(&gap), 0, "missing session inside the window");
        let mut off = good.clone();
        off[0].end_ts += 3600;
        assert_eq!(sig(&off), 0, "non-midnight label");
    }

    #[test]
    fn bars_older_than_the_required_window_do_not_matter() {
        let up = BASE + 1;
        let good = at_month_end(up, BASE, BASE, BASE);
        let mut longer = vec![bar(d(2020, 1, 2), 1, true)];
        longer.extend(good.clone());
        // only contiguity inside the tail matters: prepend a non-contiguous old bar.
        assert_eq!(sig(&longer), sig(&good));
    }

    #[test]
    fn future_bars_cannot_change_an_earlier_decision() {
        let up = BASE + 1;
        let w = at_month_end(up, BASE, BASE, BASE);
        let mut s = MonthlyMultihorizonAbsMomentumConsensusV1Strategy::new("SPY");
        let first = s.on_bar(&ctx(w.clone())).targets[0].qty.raw();
        let mut again = MonthlyMultihorizonAbsMomentumConsensusV1Strategy::new("SPY");
        assert_eq!(again.on_bar(&ctx(w)).targets[0].qty.raw(), first);
        assert_eq!(first, 1_000_000);
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_hex() {
        let a =
            MonthlyMultihorizonAbsMomentumConsensusV1Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            a,
            MonthlyMultihorizonAbsMomentumConsensusV1Strategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            a,
            MonthlyMultihorizonAbsMomentumConsensusV1Strategy::new("QQQ").semantic_fingerprint()
        );
        assert_eq!(a.len(), 64);
    }

    /// Every behavior-bearing token binds the digest (recipe mirrors the engine).
    #[test]
    fn fingerprint_changes_when_any_semantic_field_changes() {
        let fp = |name: &str,
                  version: &str,
                  horizons: [i64; 3],
                  min_votes: i64,
                  required: i64,
                  tokens: [&str; 8]| {
            let mut b = SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, name, version);
            b.push_str("SPY").push_i64(TIMEFRAME_SECS);
            push_calendar_identity(&mut b);
            for h in horizons {
                b.push_i64(h);
            }
            b.push_i64(min_votes).push_i64(required);
            for t in tokens {
                b.push_str(t);
            }
            b.finish()
        };
        let tokens = [
            "decision:latest_month_end_session_in_window",
            "vote:close_strictly_greater_than_close_h_sessions_earlier",
            "target:long_if_votes_ge_min_votes_held_to_next_month_end",
            "state:none_reconstructible_from_bounded_history",
            "direction:long_flat",
            "malformed_window:fail_closed_flat",
            "noncontiguous_or_uncovered_sessions:fail_closed_flat",
            "incomplete_latest:flat",
        ];
        let live =
            MonthlyMultihorizonAbsMomentumConsensusV1Strategy::new("SPY").semantic_fingerprint();
        let base = fp(NAME, VERSION, [21, 63, 252], 2, 275, tokens);
        assert_eq!(live, base, "recipe mirrors the engine");
        assert_ne!(live, fp(NAME, VERSION, [21, 63, 253], 2, 275, tokens));
        assert_ne!(live, fp(NAME, VERSION, [20, 63, 252], 2, 275, tokens));
        assert_ne!(live, fp(NAME, VERSION, [21, 63, 252], 1, 275, tokens));
        assert_ne!(live, fp(NAME, VERSION, [21, 63, 252], 2, 274, tokens));
        assert_ne!(live, fp(NAME, "0.1.1", [21, 63, 252], 2, 275, tokens));
        for i in 0..tokens.len() {
            let mut mutated = tokens;
            mutated[i] = "mutated";
            assert_ne!(
                live,
                fp(NAME, VERSION, [21, 63, 252], 2, 275, mutated),
                "token {i}"
            );
        }
    }
}
