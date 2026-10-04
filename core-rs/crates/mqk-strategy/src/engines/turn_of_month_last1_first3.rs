use super::session_calendar::{next_session_of_latest_bar, push_calendar_identity};
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;
use mqk_integrity::sessions;

pub(crate) const NAME: &str = "turn_of_month_last1_first3";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// Only the latest completed bar's session identity is needed.
const REQUIRED_BARS: usize = 1;
/// Long for the next session when it is among the first three regular sessions of its month.
const FIRST_SESSIONS: u32 = 3;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat turn-of-month calendar rule: long one share when the next regular session is the last of its month or one of the first three of its month, otherwise flat. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: REQUIRED_BARS,
    })
}

#[derive(Clone, Debug)]
pub struct TurnOfMonthLast1First3Strategy {
    symbol: String,
}

impl TurnOfMonthLast1First3Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    /// `1` iff the next regular session `N(t)` after the latest completed bar is the last
    /// regular session of its month or has month ordinal 1..=3; `0` otherwise and on every
    /// calendar refusal (no/incomplete latest bar, unresolvable label, uncovered date).
    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        let Some(next) = next_session_of_latest_bar(recent) else {
            return 0;
        };
        let (Ok(last_of_month), Ok(ordinal)) = (
            sessions::is_last_session_of_month(next),
            sessions::session_ordinal_in_month(next),
        ) else {
            return 0;
        };
        i64::from(last_of_month || ordinal <= FIRST_SESSIONS)
    }
}

impl Strategy for TurnOfMonthLast1First3Strategy {
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
        b.push_str("decision:next_regular_session_after_latest_completed_bar")
            .push_str("rule:long_if_last_session_of_month_or_ordinal_in_set")
            .push_i64(1)
            .push_i64(2)
            .push_i64(FIRST_SESSIONS as i64)
            .push_i64(REQUIRED_BARS as i64)
            .push_str("half_day:counts_as_session")
            .push_str("calendar_unresolved_or_uncovered:fail_closed_flat")
            .push_str("incomplete_latest:flat")
            .push_str("state:none_reconstructible_from_latest_bar")
            .push_str("direction:long_flat");
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
    use chrono::{Datelike, NaiveDate};
    use std::collections::BTreeMap;

    fn sig_on(date: NaiveDate) -> i64 {
        TurnOfMonthLast1First3Strategy::signal_from_recent(&[bar(date, 100_000_000, true)])
    }

    fn all_sessions() -> Vec<NaiveDate> {
        let mut out = Vec::new();
        let mut x = sessions::coverage_start();
        while x <= sessions::coverage_end() {
            if sessions::is_session(x).unwrap() {
                out.push(x);
            }
            x = x.succ_opt().unwrap();
        }
        out
    }

    fn ctx(bars: Vec<BarStub>) -> StrategyContext {
        let len = bars.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, bars))
    }

    #[test]
    fn required_history_is_one_and_declared_in_meta() {
        let s = TurnOfMonthLast1First3Strategy::new("SPY");
        assert_eq!(s.required_history_bars(), 1);
        assert_eq!(meta().data_requirements.unwrap().minimum_completed_bars, 1);
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
    }

    /// Hand-verified sequences from the published NYSE calendar.
    #[test]
    fn hand_verified_month_boundaries_across_weekends_holidays_half_days_and_leap_years() {
        // December 2023 -> January 2024 (year boundary; Jan 1 holiday so Jan 2 is ordinal 1).
        let cases = [
            (d(2023, 12, 27), 0), // N = Dec 28 (not last, mid-month)
            (d(2023, 12, 28), 1), // N = Dec 29 = last session of December
            (d(2023, 12, 29), 1), // N = Jan 2 ordinal 1
            (d(2024, 1, 2), 1),   // N = Jan 3 ordinal 2
            (d(2024, 1, 3), 1),   // N = Jan 4 ordinal 3
            (d(2024, 1, 4), 0),   // N = Jan 5 ordinal 4 -> flat
            (d(2024, 1, 10), 0),  // ordinary mid-month
            // Month-end adjacent to an exchange holiday: Good Friday 2024-03-29 -> March ends Mar 28.
            (d(2024, 3, 26), 0),
            (d(2024, 3, 27), 1), // N = Mar 28 last
            (d(2024, 3, 28), 1), // N = Apr 1 ordinal 1
            // Month-end adjacent to a weekend and Labor Day: Aug 2025 ends Fri Aug 29.
            (d(2025, 8, 27), 0),
            (d(2025, 8, 28), 1),
            (d(2025, 8, 29), 1), // N = Sep 2 ordinal 1 (Sep 1 is Labor Day)
            (d(2025, 9, 2), 1),  // N = Sep 3 ordinal 2
            (d(2025, 9, 3), 1),  // N = Sep 4 ordinal 3
            (d(2025, 9, 4), 0),  // N = Sep 5 ordinal 4
            // Half-day as the month-end session: Nov 29 2024 (day after Thanksgiving) is the last.
            (d(2024, 11, 26), 0), // N = Nov 27
            (d(2024, 11, 27), 1), // N = Nov 29 (half-day, last session)
            (d(2024, 11, 29), 1), // N = Dec 2 ordinal 1
            // Leap-year February: Feb 29 2024 is the last session.
            (d(2024, 2, 27), 0),
            (d(2024, 2, 28), 1),
            (d(2024, 2, 29), 1),
            // Non-leap February: Feb 28 2023 is the last session.
            (d(2023, 2, 27), 1),
            // Mourning closure 2018-12-05 shifts ordinals: Dec 3 = 1, Dec 4 = 2, Dec 6 = 3, Dec 7 = 4.
            (d(2018, 12, 4), 1),
            (d(2018, 12, 6), 0),
        ];
        for (date, expect) in cases {
            assert_eq!(sig_on(date), expect, "latest completed bar on {date}");
        }
    }

    /// Independent reference over every covered session: group the session list by calendar
    /// month, derive ordinals / last-of-month by position, and compare with the engine.
    #[test]
    fn matches_an_independent_reference_over_every_covered_session() {
        let s = all_sessions();
        let mut by_month: BTreeMap<(i32, u32), Vec<NaiveDate>> = BTreeMap::new();
        for x in &s {
            by_month.entry((x.year(), x.month())).or_default().push(*x);
        }
        let reference_long = |n: NaiveDate| {
            let m = &by_month[&(n.year(), n.month())];
            let idx = m.iter().position(|x| *x == n).unwrap();
            idx < 3 || idx == m.len() - 1
        };
        let (mut longs, mut flats) = (0, 0);
        for w in s.windows(2) {
            let (t, n) = (w[0], w[1]);
            let expect = i64::from(reference_long(n));
            assert_eq!(sig_on(t), expect, "t={t} N={n}");
            if expect == 1 {
                longs += 1
            } else {
                flats += 1
            }
        }
        // Roughly 4 of ~21 sessions per month are long.
        assert!(longs > 400 && flats > 1800, "{longs}/{flats}");
        // The last covered session has no covered successor: fail closed flat.
        assert_eq!(sig_on(*s.last().unwrap()), 0);
    }

    #[test]
    fn price_is_irrelevant_and_only_the_latest_bar_matters() {
        for date in [d(2024, 3, 27), d(2024, 3, 26), d(2025, 9, 4)] {
            let base = sig_on(date);
            for close in [1, 1_000_000, 987_654_321_000, i64::MAX / 4] {
                let one = TurnOfMonthLast1First3Strategy::signal_from_recent(&[bar(date, close, true)]);
                assert_eq!(one, base);
            }
            // Older bars of any kind, price or completeness, cannot change the target.
            let with_history = [
                bar(d(2020, 1, 2), -5, false),
                bar(d(2021, 6, 1), 0, true),
                bar(date, 100_000_000, true),
            ];
            assert_eq!(TurnOfMonthLast1First3Strategy::signal_from_recent(&with_history), base);
        }
    }

    #[test]
    fn fails_closed_flat_on_incomplete_unknown_and_uncovered_calendar_input() {
        let flat = |bars: &[BarStub]| TurnOfMonthLast1First3Strategy::signal_from_recent(bars);
        assert_eq!(sig_on(d(2024, 3, 27)), 1);
        assert_eq!(flat(&[bar(d(2024, 3, 27), 100, false)]), 0, "incomplete latest bar");
        assert_eq!(flat(&[]), 0, "no bars");
        assert_eq!(flat(&[bar(d(2024, 3, 29), 100, true)]), 0, "holiday label");
        assert_eq!(flat(&[bar(d(2024, 3, 30), 100, true)]), 0, "weekend label");
        let mut shifted = bar(d(2024, 3, 27), 100, true);
        shifted.end_ts += 16 * 3600; // a close-labelled stamp, not the daily label
        assert_eq!(flat(&[shifted]), 0, "unresolvable session identity");
        assert_eq!(flat(&[bar(d(2015, 12, 30), 100, true)]), 0, "before coverage");
        assert_eq!(flat(&[bar(d(2027, 1, 28), 100, true)]), 0, "after coverage");
        assert_eq!(flat(&[bar(d(2026, 12, 31), 100, true)]), 0, "next session uncovered");
    }

    #[test]
    fn fresh_instance_equals_long_lived_instance_and_never_shorts() {
        let s = all_sessions();
        let mut long_lived = TurnOfMonthLast1First3Strategy::new("SPY");
        for x in s.iter().step_by(7) {
            let c = ctx(vec![bar(*x, 100_000_000, true)]);
            let a = long_lived.on_bar(&c);
            let b = TurnOfMonthLast1First3Strategy::new("SPY").on_bar(&c);
            assert_eq!(a.targets[0].qty.raw(), b.targets[0].qty.raw(), "{x}");
            assert!(a.targets[0].qty.raw() >= 0);
            assert_eq!(a.targets.len(), 1);
            assert_eq!(a.targets[0].symbol, "SPY");
        }
        assert_eq!(long_lived.on_bar(&ctx(vec![bar(d(2024, 3, 27), 1, true)])).targets[0].qty.to_whole_units_checked().unwrap(), 1);
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_hex() {
        let a = TurnOfMonthLast1First3Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(a, TurnOfMonthLast1First3Strategy::new("SPY").semantic_fingerprint());
        assert_ne!(a, TurnOfMonthLast1First3Strategy::new("EFA").semantic_fingerprint());
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }

    /// Mutation proof: every behavior-bearing semantic, including the calendar identity, is bound.
    #[test]
    fn fingerprint_changes_when_any_semantic_field_including_the_calendar_changes() {
        let fp = |version: &str,
                  tf: i64,
                  cal_id: &str,
                  cal_sha: &str,
                  first: i64,
                  required: i64,
                  rule: &str,
                  decision: &str| {
            SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, version)
                .push_str("SPY")
                .push_i64(tf)
                .push_str(&format!("calendar:{cal_id}"))
                .push_str(cal_sha)
                .push_str(decision)
                .push_str(rule)
                .push_i64(1)
                .push_i64(2)
                .push_i64(first)
                .push_i64(required)
                .push_str("half_day:counts_as_session")
                .push_str("calendar_unresolved_or_uncovered:fail_closed_flat")
                .push_str("incomplete_latest:flat")
                .push_str("state:none_reconstructible_from_latest_bar")
                .push_str("direction:long_flat")
                .finish()
        };
        let sha = super::super::session_calendar::calendar_content_sha256();
        let id = sessions::US_EQUITY_REGULAR_SESSIONS_V1;
        let rule = "rule:long_if_last_session_of_month_or_ordinal_in_set";
        let decision = "decision:next_regular_session_after_latest_completed_bar";
        let live = TurnOfMonthLast1First3Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(live, fp(VERSION, TIMEFRAME_SECS, id, &sha, 3, 1, rule, decision), "recipe mirrors the engine");
        assert_ne!(live, fp(VERSION, TIMEFRAME_SECS, id, &sha, 4, 1, rule, decision), "first-three -> first-four");
        assert_ne!(live, fp(VERSION, TIMEFRAME_SECS, id, &sha, 2, 1, rule, decision));
        assert_ne!(live, fp(VERSION, TIMEFRAME_SECS, id, &sha, 3, 2, rule, decision), "required history");
        assert_ne!(live, fp(VERSION, TIMEFRAME_SECS, id, &sha, 3, 1, "rule:long_if_ordinal_in_set", decision));
        assert_ne!(live, fp(VERSION, TIMEFRAME_SECS, id, &sha, 3, 1, rule, "decision:current_session"), "current session substituted for next");
        assert_ne!(live, fp(VERSION, TIMEFRAME_SECS, id, "0".repeat(64).as_str(), 3, 1, rule, decision), "calendar content identity");
        assert_ne!(live, fp(VERSION, TIMEFRAME_SECS, "us_equity_regular_sessions_v2", &sha, 3, 1, rule, decision), "calendar contract id");
        assert_ne!(live, fp("0.1.1", TIMEFRAME_SECS, id, &sha, 3, 1, rule, decision));
        assert_ne!(live, fp(VERSION, 3_600, id, &sha, 3, 1, rule, decision));
    }
}
