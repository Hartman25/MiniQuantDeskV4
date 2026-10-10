use super::session_calendar::{push_calendar_identity_for, CalendarContract};
use super::window::complete_positive_tail;
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};
use chrono::{Datelike, NaiveDate};
use mqk_execution::QtyMicros;
use mqk_integrity::sessions_v2;

pub(crate) const NAME: &str = "pre_holiday_two_session_long_v1";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// The latest two completed bars must be consecutive actual sessions: a missing, repeated or
/// stale bar is never bridged.
const REQUIRED_BARS: usize = 2;
/// Sessions immediately preceding a scheduled exchange holiday that belong to its window.
const WINDOW_SESSIONS: i64 = 2;
const CONTRACT: CalendarContract = CalendarContract::V2;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat pre-holiday rule: long when the next scheduled session is one of the two scheduled sessions immediately preceding a scheduled exchange holiday, otherwise flat. Never short; unscheduled closures are never events.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: REQUIRED_BARS,
    })
}

/// True iff a weekday lies strictly between `a` and `b`. For two CONSECUTIVE scheduled sessions
/// every such weekday is a scheduled closure, i.e. a holiday event.
fn weekday_between(a: NaiveDate, b: NaiveDate) -> bool {
    let mut day = a;
    loop {
        day = match day.succ_opt() {
            Some(d) => d,
            None => return false,
        };
        if day >= b {
            return false;
        }
        if day.weekday().number_from_monday() <= 5 {
            return true;
        }
    }
}

/// Whether scheduled session `x` is one of the [`WINDOW_SESSIONS`] sessions immediately
/// preceding a holiday event, given only `next_scheduled`, the first scheduled session strictly
/// after a date (`None` when the calendar cannot answer). `x` is the last such session before an
/// event iff a weekday separates it from the next scheduled session; it is the one before that
/// iff its successor is. Overlapping or consecutive events yield the union of their windows.
/// `None` (the caller stays flat) when the lookahead leaves the calendar coverage.
fn in_pre_holiday_window(
    x: NaiveDate,
    next_scheduled: &impl Fn(NaiveDate) -> Option<NaiveDate>,
) -> Option<bool> {
    let mut cursor = x;
    for _ in 0..WINDOW_SESSIONS {
        let next = next_scheduled(cursor)?;
        if weekday_between(cursor, next) {
            return Some(true);
        }
        cursor = next;
    }
    Some(false)
}

#[derive(Clone, Debug)]
pub struct PreHolidayTwoSessionLongV1Strategy {
    symbol: String,
}

impl PreHolidayTwoSessionLongV1Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    /// `1` iff the latest two completed bars are consecutive actual sessions and the next
    /// scheduled session `N(t)` is in a pre-holiday window; `0` on every refusal.
    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        let Some(tail) = complete_positive_tail(recent, REQUIRED_BARS) else {
            return 0;
        };
        let (Some(prev), Some(latest)) = (
            CONTRACT.session_of_daily_bar(tail[0].end_ts),
            CONTRACT.session_of_daily_bar(tail[1].end_ts),
        ) else {
            return 0;
        };
        if CONTRACT.next_session_after(prev) != Some(latest) {
            return 0;
        }
        let next_scheduled = |d: NaiveDate| sessions_v2::next_scheduled_session_after(d).ok();
        let Some(n) = next_scheduled(latest) else {
            return 0;
        };
        i64::from(in_pre_holiday_window(n, &next_scheduled).unwrap_or(false))
    }
}

impl Strategy for PreHolidayTwoSessionLongV1Strategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(NAME, TIMEFRAME_SECS)
    }

    fn required_history_bars(&self) -> usize {
        REQUIRED_BARS
    }

    fn semantic_fingerprint(&self) -> String {
        let mut b = SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, VERSION);
        b.push_str(&self.symbol).push_i64(TIMEFRAME_SECS);
        push_calendar_identity_for(CONTRACT, &mut b);
        b.push_str("event:scheduled_weekday_full_closure")
            .push_str("unscheduled_closures:never_events")
            .push_i64(WINDOW_SESSIONS)
            .push_str("window:scheduled_sessions_immediately_preceding_event")
            .push_str("overlap:union_of_windows")
            .push_str("decision:next_scheduled_session_after_latest_completed_bar_in_window")
            .push_str("target:long_if_in_window_else_flat")
            .push_i64(REQUIRED_BARS as i64)
            .push_str("continuity:latest_two_bars_consecutive_actual_sessions_else_flat")
            .push_str("state:none_reconstructible_from_latest_two_bars")
            .push_str("half_day:counts_as_session")
            .push_str("calendar_unresolved_or_uncovered:fail_closed_flat")
            .push_str("incomplete_or_nonpositive:flat")
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
    use std::collections::BTreeSet;

    const PX: i64 = 100_000_000;

    /// Consecutive actual sessions `prev`, `latest` under the v2 calendar.
    fn pair(latest: NaiveDate) -> Vec<BarStub> {
        let mut prev = latest.pred_opt().unwrap();
        while !sessions_v2::is_session(prev).unwrap() {
            prev = prev.pred_opt().unwrap();
        }
        vec![bar(prev, PX, true), bar(latest, PX, true)]
    }

    fn sig_on(latest: NaiveDate) -> i64 {
        PreHolidayTwoSessionLongV1Strategy::signal_from_recent(&pair(latest))
    }

    fn every_day(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
        let mut out = Vec::new();
        let mut x = from;
        while x <= to {
            out.push(x);
            x = x.succ_opt().unwrap();
        }
        out
    }

    /// Scheduled holidays whose whole window (and decision session) lies inside the coverage:
    /// the first covered date, 2016-01-01, is itself a holiday with its window in 2015.
    fn scheduled_holidays() -> Vec<NaiveDate> {
        every_day(sessions_v2::coverage_start(), sessions_v2::coverage_end())
            .into_iter()
            .filter(|x| *x > d(2016, 1, 10) && sessions_v2::is_scheduled_holiday(*x).unwrap())
            .collect()
    }

    /// Independent reference: walk backwards over scheduled sessions from each holiday.
    fn reference_window_sessions(
        holidays: &[NaiveDate],
        is_sched_session: &dyn Fn(NaiveDate) -> bool,
    ) -> BTreeSet<NaiveDate> {
        let mut out = BTreeSet::new();
        for &h in holidays {
            let mut x = h;
            for _ in 0..WINDOW_SESSIONS {
                x = x.pred_opt().unwrap();
                while !is_sched_session(x) {
                    x = x.pred_opt().unwrap();
                }
                out.insert(x);
            }
        }
        out
    }

    #[test]
    fn required_history_is_two_and_declared_in_meta() {
        let s = PreHolidayTwoSessionLongV1Strategy::new("SPY");
        assert_eq!(s.required_history_bars(), 2);
        assert_eq!(meta().data_requirements.unwrap().minimum_completed_bars, 2);
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
        assert_eq!(
            meta().restart_recovery,
            crate::RestartRecovery::BoundedHistoryReconstructible
        );
    }

    #[test]
    fn hand_verified_causal_timelines_around_real_holidays() {
        // (decision session t, expected target after t completes)
        let cases = [
            // Thanksgiving 2024-11-28: P2 = Tue 26, P1 = Wed 27; reopening Fri 29 (early close).
            (d(2024, 11, 22), 0),
            (d(2024, 11, 25), 1), // N = P2: entry signal, fills on the P2 bar
            (d(2024, 11, 26), 1), // N = P1: hold
            (d(2024, 11, 27), 0), // N = Fri 29: exit signal, fills on the reopening bar
            (d(2024, 11, 29), 0),
            // Good Friday 2024-03-29: P2 = Wed 27, P1 = Thu 28.
            (d(2024, 3, 25), 0),
            (d(2024, 3, 26), 1),
            (d(2024, 3, 27), 1),
            (d(2024, 3, 28), 0),
            // Christmas Monday 2023-12-25: P2 = Thu 21, P1 = Fri 22 (weekend gap).
            (d(2023, 12, 20), 1),
            (d(2023, 12, 21), 1),
            (d(2023, 12, 22), 0),
            // New Year 2024-01-01: P2 = Thu 2023-12-28, P1 = Fri 12-29 (year boundary).
            (d(2023, 12, 27), 1),
            (d(2023, 12, 28), 1),
            (d(2023, 12, 29), 0),
            // Independence Day 2024-07-04 (Thu): P2 = Tue 7-2, P1 = Wed 7-3 (early close).
            (d(2024, 6, 28), 0),
            (d(2024, 7, 1), 1),
            (d(2024, 7, 2), 1),
            (d(2024, 7, 3), 0), // N = Fri 7-5, the reopening
            // Saturday New Year's Day 2022-01-01 is not observed: no event.
            (d(2021, 12, 29), 0),
            (d(2021, 12, 30), 0),
        ];
        for (t, expect) in cases {
            assert_eq!(sig_on(t), expect, "decision on {t}");
        }
    }

    #[test]
    fn matches_an_independent_backward_walk_over_every_covered_session() {
        let holidays = scheduled_holidays();
        let sched = |x: NaiveDate| sessions_v2::is_scheduled_session(x).unwrap();
        let window = reference_window_sessions(&holidays, &sched);
        // Real data contains no overlap in 2016-2028: every window is exactly two sessions.
        assert_eq!(
            window.len(),
            holidays.len() * 2,
            "windows are disjoint in the real calendar"
        );
        let days = every_day(sessions_v2::coverage_start(), d(2028, 12, 20));
        let (mut longs, mut flats) = (0usize, 0usize);
        for t in days
            .iter()
            .filter(|x| sessions_v2::is_session(**x).unwrap())
            .skip(1)
        {
            let n = sessions_v2::next_scheduled_session_after(*t).unwrap();
            let expect = i64::from(window.contains(&n));
            assert_eq!(sig_on(*t), expect, "t={t} N={n}");
            if expect == 1 {
                longs += 1;
            } else {
                flats += 1;
            }
        }
        // 124 scheduled-or-not closures minus two unscheduled = 122 events up to 2028-12-31;
        // two long decisions per event whose window starts after the first covered bar.
        assert!(longs > 200 && flats > 2500, "{longs}/{flats}");
    }

    #[test]
    fn synthetic_calendars_prove_consecutive_overlapping_and_isolated_events() {
        // A weekday-only calendar over one fixed month with a chosen holiday set.
        let monday = d(2024, 9, 2); // not used as a holiday; just a Monday anchor
        let mk = |holidays: &'static [(u32, u32)]| {
            let hs: BTreeSet<NaiveDate> = holidays.iter().map(|&(m, dd)| d(2024, m, dd)).collect();
            move |x: NaiveDate| -> Option<NaiveDate> {
                let mut c = x;
                loop {
                    c = c.succ_opt()?;
                    if c.weekday().number_from_monday() <= 5 && !hs.contains(&c) {
                        return Some(c);
                    }
                }
            }
        };
        let hs_of = |holidays: &[(u32, u32)]| -> BTreeSet<NaiveDate> {
            holidays.iter().map(|&(m, dd)| d(2024, m, dd)).collect()
        };
        let cases: [&'static [(u32, u32)]; 5] = [
            &[(9, 11)],                                     // isolated Wednesday
            &[(9, 11), (9, 13)], // two holidays separated by ONE session (overlap)
            &[(9, 11), (9, 12)], // consecutive weekday holidays (coincident windows)
            &[(9, 10), (9, 11), (9, 12), (9, 13), (9, 16)], // week-long closure
            &[(9, 4), (9, 18)],  // far apart
        ];
        for holidays in cases {
            let next = mk(holidays);
            let hset = hs_of(holidays);
            let is_sched =
                |x: NaiveDate| x.weekday().number_from_monday() <= 5 && !hset.contains(&x);
            let reference =
                reference_window_sessions(&hset.iter().copied().collect::<Vec<_>>(), &is_sched);
            let mut x = monday - chrono::Duration::days(7);
            while x < d(2024, 9, 27) {
                if is_sched(x) {
                    let got = in_pre_holiday_window(x, &next).unwrap();
                    assert_eq!(got, reference.contains(&x), "{holidays:?} x={x}");
                }
                x = x.succ_opt().unwrap();
            }
        }
        // Overlap case in words: Wed 9-11 and Fri 9-13 are holidays; Mon 9-9, Tue 9-10 precede the
        // first, Tue 9-10 and Thu 9-12 the second -> union {Mon, Tue, Thu}, ONE continuous hold:
        // the decision at Tue 9-10 (N = Thu 9-12) is long, so no flat-then-rebuy across Wed.
        let next = mk(&[(9, 11), (9, 13)]);
        assert_eq!(in_pre_holiday_window(d(2024, 9, 9), &next), Some(true));
        assert_eq!(in_pre_holiday_window(d(2024, 9, 10), &next), Some(true));
        assert_eq!(in_pre_holiday_window(d(2024, 9, 12), &next), Some(true));
        assert_eq!(in_pre_holiday_window(d(2024, 9, 16), &next), Some(false));
        assert_eq!(in_pre_holiday_window(d(2024, 9, 6), &next), Some(false));
        // An oracle that cannot answer fails the whole query.
        let none = |_: NaiveDate| -> Option<NaiveDate> { None };
        assert_eq!(in_pre_holiday_window(d(2024, 9, 9), &none), None);
    }

    #[test]
    fn a_surprise_closure_can_neither_create_nor_move_a_signal() {
        // Real surprise closures: 2018-12-05 and 2025-01-09.
        for (u, around) in [
            (
                d(2018, 12, 5),
                [
                    d(2018, 12, 3),
                    d(2018, 12, 4),
                    d(2018, 12, 6),
                    d(2018, 12, 7),
                ],
            ),
            (
                d(2025, 1, 9),
                [d(2025, 1, 7), d(2025, 1, 8), d(2025, 1, 10), d(2025, 1, 13)],
            ),
        ] {
            assert!(!sessions_v2::is_session(u).unwrap());
            assert!(sessions_v2::is_scheduled_session(u).unwrap());
            for t in around {
                assert_eq!(
                    sig_on(t),
                    0,
                    "no signal near the surprise closure {u}, t={t}"
                );
            }
            // A bar labelled on the surprise-closure date is not a session: flat, never a signal.
            assert_eq!(
                PreHolidayTwoSessionLongV1Strategy::signal_from_recent(&[
                    bar(u.pred_opt().unwrap(), PX, true),
                    bar(u, PX, true)
                ]),
                0
            );
        }
        // Negative control: had the closure been (wrongly) classified scheduled, the decision
        // BEFORE it was knowable would turn long. The real classification never does this.
        let u = d(2025, 1, 9);
        let wrong = |x: NaiveDate| -> Option<NaiveDate> {
            let mut c = x;
            loop {
                c = c.succ_opt()?;
                if c.weekday().number_from_monday() <= 5
                    && c != u
                    && !sessions_v2::is_scheduled_holiday(c).ok()?
                {
                    return Some(c);
                }
            }
        };
        assert_eq!(in_pre_holiday_window(d(2025, 1, 8), &wrong), Some(true));
        assert_eq!(
            in_pre_holiday_window(d(2025, 1, 8), &|x| {
                sessions_v2::next_scheduled_session_after(x).ok()
            }),
            Some(false)
        );
    }

    #[test]
    fn the_signal_is_irrelevant_to_prices_and_bars_older_than_the_latest_two() {
        for t in [d(2024, 11, 25), d(2024, 11, 27), d(2024, 3, 28)] {
            let base = sig_on(t);
            for close in [1, 987_654_321_000, i64::MAX / 4] {
                let mut w = pair(t);
                w[1].close_micros = close;
                w[0].close_micros = close / 2 + 1;
                assert_eq!(
                    PreHolidayTwoSessionLongV1Strategy::signal_from_recent(&w),
                    base
                );
            }
            let mut with_history = vec![bar(d(2019, 1, 2), -5, false), bar(d(2021, 6, 1), 0, true)];
            with_history.extend(pair(t));
            assert_eq!(
                PreHolidayTwoSessionLongV1Strategy::signal_from_recent(&with_history),
                base
            );
        }
    }

    #[test]
    fn malformed_stale_gapped_uncovered_and_incomplete_inputs_are_flat() {
        let good = pair(d(2024, 11, 25));
        assert_eq!(
            PreHolidayTwoSessionLongV1Strategy::signal_from_recent(&good),
            1
        );
        let sig = PreHolidayTwoSessionLongV1Strategy::signal_from_recent;
        assert_eq!(sig(&[]), 0);
        assert_eq!(sig(&good[1..]), 0, "one bar only");
        let mut x = good.clone();
        x[1].is_complete = false;
        assert_eq!(sig(&x), 0, "incomplete latest bar");
        let mut x = good.clone();
        x[0].is_complete = false;
        assert_eq!(sig(&x), 0, "incomplete previous bar");
        let mut x = good.clone();
        x[1].close_micros = 0;
        assert_eq!(sig(&x), 0, "non-positive close");
        let mut x = good.clone();
        x[1].end_ts += 3600;
        assert_eq!(sig(&x), 0, "non-midnight label");
        // Missing session between the two bars (stale input).
        let gap = vec![
            bar(d(2024, 11, 21), PX, true),
            bar(d(2024, 11, 25), PX, true),
        ];
        assert_eq!(sig(&gap), 0, "a missing session is never bridged");
        // Repeated and reversed bars.
        let dup = vec![good[1].clone(), good[1].clone()];
        assert_eq!(sig(&dup), 0);
        let rev = vec![good[1].clone(), good[0].clone()];
        assert_eq!(sig(&rev), 0);
        // Weekend, holiday and surprise-closure labels.
        for bad in [d(2024, 11, 23), d(2024, 11, 28), d(2025, 1, 9)] {
            assert_eq!(sig(&[good[0].clone(), bar(bad, PX, true)]), 0, "{bad}");
        }
        // Out of coverage on either side, and a lookahead that leaves the coverage.
        assert_eq!(
            sig(&[
                bar(d(2015, 12, 30), PX, true),
                bar(d(2015, 12, 31), PX, true)
            ]),
            0
        );
        assert_eq!(
            sig(&[bar(d(2029, 1, 2), PX, true), bar(d(2029, 1, 3), PX, true)]),
            0
        );
        // 2028-12-27: N = 12-28 is not itself the end of the window question; the lookahead of
        // two further sessions leaves the coverage, so the decision fails closed.
        assert_eq!(sig(&pair(d(2028, 12, 27))), 0);
        assert_eq!(sig(&pair(d(2028, 12, 29))), 0);
        // A bar timestamped on Dec 25 2023 (holiday) is refused.
        assert_eq!(
            sig(&[
                bar(d(2023, 12, 22), PX, true),
                bar(d(2023, 12, 25), PX, true)
            ]),
            0
        );
    }

    #[test]
    fn no_event_window_straddles_a_march_first_fold_or_holdout_boundary() {
        // Evaluation folds start every 1 March from 2016-03-01 and the holdout starts 2026-03-01.
        // A window straddles a boundary iff its entry decision (P3) precedes it and the exit
        // reopening session (R) is on or after it; then force-flat-at-fold-end would truncate it.
        for h in scheduled_holidays() {
            let mut sessions_back = Vec::new();
            let mut x = h;
            while sessions_back.len() < 3 {
                x = x.pred_opt().unwrap();
                if sessions_v2::is_scheduled_session(x).unwrap() {
                    sessions_back.push(x);
                }
            }
            let p3 = sessions_back[2];
            let r = sessions_v2::next_session_after(h).unwrap_or(h);
            for year in 2016..=2028 {
                let boundary = d(year, 3, 1);
                assert!(
                    !(p3 < boundary && boundary <= r),
                    "event {h} (decision {p3}, reopening {r}) straddles {boundary}"
                );
            }
        }
    }

    #[test]
    fn the_engine_target_stream_equals_restart_at_every_boundary() {
        // Stateless: the target after bar k computed from the full history equals the target
        // computed from only the two-bar bounded window a restarted host would reload.
        let sessions: Vec<NaiveDate> = every_day(d(2023, 11, 1), d(2024, 4, 30))
            .into_iter()
            .filter(|x| sessions_v2::is_session(*x).unwrap())
            .collect();
        let bars: Vec<BarStub> = sessions.iter().map(|s| bar(*s, PX, true)).collect();
        let mut continuous = PreHolidayTwoSessionLongV1Strategy::new("SPY");
        let mut stream = Vec::new();
        for k in 1..bars.len() {
            let ctx = StrategyContext::new(
                TIMEFRAME_SECS,
                k as u64,
                RecentBarsWindow::new(bars.len(), bars[..=k].to_vec()),
            );
            stream.push(continuous.on_bar(&ctx).targets[0].qty.raw());
            // A brand-new instance fed only the bounded window.
            let mut restarted = PreHolidayTwoSessionLongV1Strategy::new("SPY");
            let ctx2 = StrategyContext::new(
                TIMEFRAME_SECS,
                0,
                RecentBarsWindow::new(2, bars[k - 1..=k].to_vec()),
            );
            assert_eq!(
                restarted.on_bar(&ctx2).targets[0].qty.raw(),
                *stream.last().unwrap(),
                "k={k}"
            );
        }
        assert!(stream.contains(&1_000_000) && stream.contains(&0));
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_hex_and_v2_calendar_bound() {
        let a = PreHolidayTwoSessionLongV1Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            a,
            PreHolidayTwoSessionLongV1Strategy::new("SPY").semantic_fingerprint()
        );
        for other in ["QQQ", "IWM", "DIA"] {
            assert_ne!(
                a,
                PreHolidayTwoSessionLongV1Strategy::new(other).semantic_fingerprint()
            );
        }
        assert_eq!(a.len(), 64);
        // The same recipe under the v1 calendar identity is a different fingerprint.
        let mut b = SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, VERSION);
        b.push_str("SPY").push_i64(TIMEFRAME_SECS);
        push_calendar_identity_for(CalendarContract::V1, &mut b);
        assert_ne!(a, b.finish());
    }

    #[test]
    fn fingerprint_changes_when_any_semantic_field_changes() {
        let tokens = [
            "event:scheduled_weekday_full_closure",
            "unscheduled_closures:never_events",
            "window:scheduled_sessions_immediately_preceding_event",
            "overlap:union_of_windows",
            "decision:next_scheduled_session_after_latest_completed_bar_in_window",
            "target:long_if_in_window_else_flat",
            "continuity:latest_two_bars_consecutive_actual_sessions_else_flat",
            "state:none_reconstructible_from_latest_two_bars",
            "half_day:counts_as_session",
            "calendar_unresolved_or_uncovered:fail_closed_flat",
            "incomplete_or_nonpositive:flat",
            "direction:long_flat",
        ];
        let fp = |version: &str, window: i64, required: i64, tokens: &[&str]| {
            let mut b = SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, version);
            b.push_str("SPY").push_i64(TIMEFRAME_SECS);
            push_calendar_identity_for(CalendarContract::V2, &mut b);
            b.push_str(tokens[0]).push_str(tokens[1]).push_i64(window);
            b.push_str(tokens[2])
                .push_str(tokens[3])
                .push_str(tokens[4])
                .push_str(tokens[5]);
            b.push_i64(required);
            for t in &tokens[6..] {
                b.push_str(t);
            }
            b.finish()
        };
        let live = PreHolidayTwoSessionLongV1Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            live,
            fp(VERSION, 2, 2, &tokens),
            "recipe mirrors the engine"
        );
        assert_ne!(live, fp(VERSION, 1, 2, &tokens));
        assert_ne!(live, fp(VERSION, 3, 2, &tokens));
        assert_ne!(live, fp(VERSION, 2, 3, &tokens));
        assert_ne!(live, fp("0.1.1", 2, 2, &tokens));
        for i in 0..tokens.len() {
            let mut mutated = tokens;
            mutated[i] = "mutated";
            assert_ne!(live, fp(VERSION, 2, 2, &mutated), "token {i}");
        }
    }

    #[test]
    fn on_bar_emits_a_fixed_one_share_signal_never_short() {
        let mut s = PreHolidayTwoSessionLongV1Strategy::new("SPY");
        for (t, expect) in [(d(2024, 11, 25), 1_000_000), (d(2024, 11, 27), 0)] {
            let w = pair(t);
            let ctx = StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(2, w));
            let out = s.on_bar(&ctx);
            assert_eq!(out.targets.len(), 1);
            assert_eq!(out.targets[0].qty.raw(), expect);
            assert!(out.targets[0].qty.raw() >= 0);
        }
    }
}
