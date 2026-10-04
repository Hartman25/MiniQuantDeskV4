use super::session_calendar::{next_session_of_latest_bar, push_calendar_identity};
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};
use chrono::Datelike;
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "halloween_nov_apr";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// Only the latest completed bar's session identity is needed.
const REQUIRED_BARS: usize = 1;
/// Long calendar months, by the month of the next regular session: November through April.
const LONG_MONTHS: [u32; 6] = [11, 12, 1, 2, 3, 4];

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat Halloween seasonal rule: long one share when the next regular session falls in November through April, otherwise flat. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: REQUIRED_BARS,
    })
}

#[derive(Clone, Debug)]
pub struct HalloweenNovAprStrategy {
    symbol: String,
}

impl HalloweenNovAprStrategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    /// `1` iff the month of the next regular session `N(t)` is in November..=April; `0`
    /// otherwise and on every calendar refusal.
    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        let Some(next) = next_session_of_latest_bar(recent) else {
            return 0;
        };
        i64::from(LONG_MONTHS.contains(&next.month()))
    }
}

impl Strategy for HalloweenNovAprStrategy {
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
            .push_str("rule:long_if_next_session_month_in_set");
        for m in LONG_MONTHS {
            b.push_i64(m as i64);
        }
        b.push_str("flat_months:5_6_7_8_9_10")
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
    use chrono::NaiveDate;
    use mqk_integrity::sessions;

    fn sig_on(date: NaiveDate) -> i64 {
        HalloweenNovAprStrategy::signal_from_recent(&[bar(date, 100_000_000, true)])
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
        let s = HalloweenNovAprStrategy::new("SPY");
        assert_eq!(s.required_history_bars(), 1);
        assert_eq!(meta().data_requirements.unwrap().minimum_completed_bars, 1);
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
    }

    #[test]
    fn hand_verified_season_transitions_and_calendar_edges() {
        let cases = [
            // Final October session -> long for the first November session.
            (d(2023, 10, 30), 0), // N = Oct 31 -> October
            (d(2023, 10, 31), 1), // N = Nov 1
            (d(2023, 11, 1), 1),
            (d(2023, 12, 29), 1), // December -> January stays long
            (d(2024, 1, 2), 1),
            (d(2024, 2, 29), 1),
            (d(2024, 3, 28), 1), // N = Apr 1 across Good Friday
            (d(2024, 4, 29), 1), // N = Apr 30
            (d(2024, 4, 30), 0), // final April session -> flat for first May session
            (d(2024, 5, 1), 0),
            (d(2024, 6, 14), 0),
            (d(2024, 8, 30), 0), // N = Sep 3 (Sep 2 Labor Day)
            (d(2024, 9, 30), 0), // N = Oct 1
            (d(2024, 10, 30), 0),
            (d(2024, 10, 31), 1),
            // Holiday / weekend transitions: Oct 2020 ends Fri Oct 30; Nov 2 is Monday.
            (d(2020, 10, 29), 0),
            (d(2020, 10, 30), 1),
            // Half-day: day after Thanksgiving 2024 is November and counts as a session.
            (d(2024, 11, 27), 1),
            (d(2024, 11, 29), 1),
            // April 2022 ends on Friday Apr 29; first May session is Monday May 2.
            (d(2022, 4, 28), 1),
            (d(2022, 4, 29), 0),
        ];
        for (date, expect) in cases {
            assert_eq!(sig_on(date), expect, "latest completed bar on {date}");
        }
    }

    #[test]
    fn matches_an_independent_month_table_over_every_covered_session() {
        let s = all_sessions();
        let (mut longs, mut flats) = (0, 0);
        for w in s.windows(2) {
            let n_month = w[1].month();
            let expect = i64::from(matches!(n_month, 11 | 12 | 1 | 2 | 3 | 4));
            assert_eq!(sig_on(w[0]), expect, "t={} N={}", w[0], w[1]);
            if expect == 1 {
                longs += 1
            } else {
                flats += 1
            }
        }
        assert!(longs > 1000 && flats > 1000, "{longs}/{flats}");
        assert_eq!(sig_on(*s.last().unwrap()), 0, "next session uncovered");
        // Neither May nor October is ever long; no month outside the set is.
        for w in s.windows(2) {
            if matches!(w[1].month(), 5..=10) {
                assert_eq!(sig_on(w[0]), 0, "N month {}", w[1].month());
            }
        }
    }

    #[test]
    fn price_is_irrelevant_and_only_the_latest_bar_matters() {
        for date in [d(2023, 10, 31), d(2024, 4, 30), d(2024, 7, 15)] {
            let base = sig_on(date);
            for close in [1, 1_000_000, 987_654_321_000, i64::MAX / 4] {
                assert_eq!(
                    HalloweenNovAprStrategy::signal_from_recent(&[bar(date, close, true)]),
                    base
                );
            }
            let with_history = [
                bar(d(2020, 1, 2), -5, false),
                bar(d(2021, 6, 1), 0, true),
                bar(date, 100_000_000, true),
            ];
            assert_eq!(
                HalloweenNovAprStrategy::signal_from_recent(&with_history),
                base
            );
        }
    }

    #[test]
    fn fails_closed_flat_on_incomplete_unknown_and_uncovered_calendar_input() {
        let f = |bars: &[BarStub]| HalloweenNovAprStrategy::signal_from_recent(bars);
        assert_eq!(sig_on(d(2023, 12, 20)), 1);
        assert_eq!(
            f(&[bar(d(2023, 12, 20), 100, false)]),
            0,
            "incomplete latest bar"
        );
        assert_eq!(f(&[]), 0);
        assert_eq!(f(&[bar(d(2023, 12, 25), 100, true)]), 0, "holiday label");
        assert_eq!(f(&[bar(d(2023, 12, 23), 100, true)]), 0, "weekend label");
        let mut shifted = bar(d(2023, 12, 20), 100, true);
        shifted.end_ts -= 1;
        assert_eq!(f(&[shifted]), 0);
        assert_eq!(f(&[bar(d(2015, 12, 31), 100, true)]), 0, "before coverage");
        assert_eq!(f(&[bar(d(2027, 12, 1), 100, true)]), 0, "after coverage");
        assert_eq!(
            f(&[bar(d(2026, 12, 31), 100, true)]),
            0,
            "next session uncovered"
        );
    }

    #[test]
    fn fresh_instance_equals_long_lived_instance_and_never_shorts() {
        let s = all_sessions();
        let mut long_lived = HalloweenNovAprStrategy::new("SPY");
        for x in s.iter().step_by(11) {
            let c = ctx(vec![bar(*x, 100_000_000, true)]);
            let a = long_lived.on_bar(&c);
            let b = HalloweenNovAprStrategy::new("SPY").on_bar(&c);
            assert_eq!(a.targets[0].qty.raw(), b.targets[0].qty.raw(), "{x}");
            assert!(a.targets[0].qty.raw() >= 0);
            assert_eq!((a.targets.len(), a.targets[0].symbol.as_str()), (1, "SPY"));
        }
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_hex() {
        let a = HalloweenNovAprStrategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            a,
            HalloweenNovAprStrategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            a,
            HalloweenNovAprStrategy::new("EFA").semantic_fingerprint()
        );
        assert_eq!(a.len(), 64);
        assert!(a
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }

    #[test]
    fn fingerprint_changes_when_any_semantic_field_including_the_calendar_changes() {
        let fp = |version: &str,
                  tf: i64,
                  cal_id: &str,
                  cal_sha: &str,
                  months: &[i64],
                  flat: &str,
                  decision: &str| {
            let mut b = SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, version);
            b.push_str("SPY")
                .push_i64(tf)
                .push_str(&format!("calendar:{cal_id}"))
                .push_str(cal_sha)
                .push_str(decision)
                .push_str("rule:long_if_next_session_month_in_set");
            for m in months {
                b.push_i64(*m);
            }
            b.push_str(flat)
                .push_i64(1)
                .push_str("half_day:counts_as_session")
                .push_str("calendar_unresolved_or_uncovered:fail_closed_flat")
                .push_str("incomplete_latest:flat")
                .push_str("state:none_reconstructible_from_latest_bar")
                .push_str("direction:long_flat");
            b.finish()
        };
        let sha = super::super::session_calendar::calendar_content_sha256();
        let id = sessions::US_EQUITY_REGULAR_SESSIONS_V1;
        let months = [11, 12, 1, 2, 3, 4];
        let flat = "flat_months:5_6_7_8_9_10";
        let decision = "decision:next_regular_session_after_latest_completed_bar";
        let live = HalloweenNovAprStrategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            live,
            fp(VERSION, TIMEFRAME_SECS, id, &sha, &months, flat, decision),
            "recipe mirrors the engine"
        );
        assert_ne!(
            live,
            fp(
                VERSION,
                TIMEFRAME_SECS,
                id,
                &sha,
                &[11, 12, 1, 2, 3, 4, 5],
                flat,
                decision
            ),
            "May included"
        );
        assert_ne!(
            live,
            fp(
                VERSION,
                TIMEFRAME_SECS,
                id,
                &sha,
                &[10, 11, 12, 1, 2, 3, 4],
                flat,
                decision
            ),
            "October included"
        );
        assert_ne!(
            live,
            fp(
                VERSION,
                TIMEFRAME_SECS,
                id,
                &sha,
                &[11, 12, 1, 2, 3],
                flat,
                decision
            )
        );
        assert_ne!(
            live,
            fp(
                VERSION,
                TIMEFRAME_SECS,
                id,
                &sha,
                &months,
                flat,
                "decision:current_session"
            ),
            "current session substituted for next"
        );
        assert_ne!(
            live,
            fp(
                VERSION,
                TIMEFRAME_SECS,
                id,
                "0".repeat(64).as_str(),
                &months,
                flat,
                decision
            ),
            "calendar content identity"
        );
        assert_ne!(
            live,
            fp(
                VERSION,
                TIMEFRAME_SECS,
                "us_equity_regular_sessions_v2",
                &sha,
                &months,
                flat,
                decision
            )
        );
        assert_ne!(
            live,
            fp("0.1.1", TIMEFRAME_SECS, id, &sha, &months, flat, decision)
        );
        assert_ne!(live, fp(VERSION, 3_600, id, &sha, &months, flat, decision));
    }
}
