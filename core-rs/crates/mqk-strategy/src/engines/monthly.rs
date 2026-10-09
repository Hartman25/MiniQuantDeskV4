//! Month-end structure of a daily window, resolved only from the `us_equity_regular_sessions_v1`
//! authority in `mqk-integrity` (never from a weekday or price heuristic).
//!
//! A monthly engine's target at bar `t` is the decision taken at the latest month-end session
//! `m <= t`, held until the next month-end. It is therefore a pure function of the latest
//! `REQUIRED` contiguous completed sessions: a month has at most 23 sessions, so `t - m <= 22`.

use chrono::{Datelike, NaiveDate};
#[cfg(test)]
use mqk_integrity::sessions;

use super::session_calendar::CalendarContract;
use crate::BarStub;

/// Most sessions after a month-end decision bar before the next month-end.
pub(crate) const MAX_SESSIONS_SINCE_MONTH_END: usize = 22;

/// Most regular sessions in one calendar month (consecutive month-ends are at most this far apart).
pub(crate) const MAX_SESSIONS_PER_MONTH: usize = 23;

fn month_key(d: NaiveDate) -> (i32, u32) {
    (d.year(), d.month())
}

/// Indices (ascending) of the month-end sessions in `win`, or `None` when the window is not a
/// run of consecutive regular sessions (unknown label, holiday label, a missing or repeated
/// session) or the next session after the last bar is outside the calendar coverage.
/// Month-end = the next regular session falls in a different calendar month.
/// Resolved under the explicit calendar `contract`.
pub(crate) fn month_end_indices_in(
    contract: CalendarContract,
    win: &[BarStub],
) -> Option<Vec<usize>> {
    let mut dates = Vec::with_capacity(win.len());
    for b in win {
        let d = contract.session_of_daily_bar(b.end_ts)?;
        if let Some(&prev) = dates.last() {
            if contract.next_session_after(prev)? != d {
                return None;
            }
        }
        dates.push(d);
    }
    let last = *dates.last()?;
    let after_last = contract.next_session_after(last)?;
    let mut out = Vec::new();
    for (i, d) in dates.iter().enumerate() {
        let next = dates.get(i + 1).copied().unwrap_or(after_last);
        if month_key(*d) != month_key(next) {
            out.push(i);
        }
    }
    Some(out)
}

/// [`month_end_indices_in`] under the frozen v1 contract (every registered monthly engine).
pub(crate) fn month_end_indices(win: &[BarStub]) -> Option<Vec<usize>> {
    month_end_indices_in(CalendarContract::V1, win)
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::super::session_calendar::test_support::{bar, d};
    use super::*;

    /// `n` consecutive regular sessions ending on `last`, closes from `f(index_from_start)`.
    pub(crate) fn series(last: NaiveDate, n: usize, f: impl Fn(usize) -> i64) -> Vec<BarStub> {
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

    /// A tape of `required + 60` sessions all closing at `default`, and the index `m` of its
    /// second-to-last month-end (so every window `tape[e+1-required..=e]` for `e` from `m` to the
    /// last month-end `- 1` holds `m` with at least `required - 23` bars before it).
    pub(crate) fn tape_with_decision(
        required: usize,
        default: i64,
    ) -> (Vec<BarStub>, usize, usize) {
        let tape = series(d(2025, 3, 31), required + 60, |_| default);
        let me = month_end_indices(&tape).unwrap();
        let (m, next) = (me[me.len() - 2], me[me.len() - 1]);
        assert!(m + 1 >= required);
        (tape, m, next)
    }
}

#[cfg(test)]
mod tests {
    use super::super::session_calendar::test_support::*;
    use super::*;
    use mqk_integrity::sessions;

    fn run(from: NaiveDate, n: usize) -> Vec<BarStub> {
        let mut out = Vec::new();
        let mut day = from;
        while out.len() < n {
            if sessions::is_session(day).unwrap() {
                out.push(bar(day, 100_000_000, true));
            }
            day = day.succ_opt().unwrap();
        }
        out
    }

    #[test]
    fn month_ends_follow_the_calendar_across_holidays_and_half_days() {
        // 2024-03-26 .. : Mar 28 is the last March session (Good Friday Mar 29 is closed).
        let w = run(d(2024, 3, 26), 6); // Mar 26,27,28, Apr 1,2,3
        assert_eq!(month_end_indices(&w), Some(vec![2]));
        // The last bar's month-end status comes from the calendar, not the window.
        let w = run(d(2024, 4, 25), 4); // Apr 25,26,29,30 -> Apr 30 is a month-end
        assert_eq!(month_end_indices(&w), Some(vec![3]));
        // Dec 2024: the last session is Dec 31; Jan 1 is a holiday.
        let w = run(d(2024, 12, 27), 3); // Dec 27, 30, 31
        assert_eq!(month_end_indices(&w), Some(vec![2]));
    }

    #[test]
    fn a_missing_repeated_or_unresolvable_session_is_refused() {
        let w = run(d(2024, 3, 4), 8);
        assert!(month_end_indices(&w).is_some());
        let mut gap = w.clone();
        gap.remove(3);
        assert_eq!(month_end_indices(&gap), None, "missing session");
        let mut dup = w.clone();
        dup.insert(3, w[3].clone());
        assert_eq!(month_end_indices(&dup), None, "repeated session");
        let mut off = w.clone();
        off[2].end_ts += 3600;
        assert_eq!(month_end_indices(&off), None, "non-midnight label");
        assert_eq!(month_end_indices(&[]), None);
        // Next session after the last covered one is out of coverage.
        assert_eq!(
            month_end_indices(&[bar(d(2026, 12, 31), 1, true)]),
            None,
            "uncovered next session"
        );
    }

    #[test]
    fn v2_month_ends_resolve_past_the_v1_horizon_with_identical_results_where_both_cover() {
        use super::super::session_calendar::CalendarContract::{V1, V2};
        // v1 refuses a window whose next session leaves its coverage; v2 resolves it.
        let edge = [bar(d(2026, 12, 31), 1, true)];
        assert_eq!(month_end_indices_in(V1, &edge), None);
        assert_eq!(month_end_indices_in(V2, &edge), Some(vec![0]));
        let w: Vec<BarStub> = [28, 29, 30]
            .iter()
            .map(|&day| bar(d(2027, 12, day), 100_000_000, true))
            .collect(); // Dec 31 2027 trades
        assert_eq!(month_end_indices_in(V1, &w), None);
        assert_eq!(month_end_indices_in(V2, &w), Some(vec![]));
        // Wherever v1 can answer, v2 gives the same answer.
        for (from, n) in [
            (d(2024, 3, 26), 6),
            (d(2024, 4, 25), 4),
            (d(2024, 12, 27), 3),
            (d(2018, 11, 28), 9),
        ] {
            let w = run(from, n);
            assert_eq!(
                month_end_indices_in(V1, &w),
                month_end_indices_in(V2, &w),
                "{from}"
            );
        }
        // The v1 entry point is exactly the v1 contract.
        let w = run(d(2024, 3, 26), 6);
        assert_eq!(month_end_indices(&w), month_end_indices_in(V1, &w));
    }

    #[test]
    fn no_month_has_more_than_the_declared_maximum_sessions_after_a_month_end() {
        // Every covered month-end is followed by at most 22 non-month-end sessions.
        let mut gap = 0usize;
        let mut max_gap = 0usize;
        let mut day = sessions::coverage_start();
        while day <= sessions::coverage_end() {
            if sessions::is_session(day).unwrap() {
                if sessions::is_last_session_of_month(day).unwrap() {
                    gap = 0;
                } else {
                    gap += 1;
                    max_gap = max_gap.max(gap);
                }
            }
            day = day.succ_opt().unwrap();
        }
        assert!(max_gap <= MAX_SESSIONS_SINCE_MONTH_END, "{max_gap}");
    }

    #[test]
    fn no_covered_month_has_more_than_the_declared_maximum_sessions() {
        let mut count = 0usize;
        let mut max_count = 0usize;
        let mut day = sessions::coverage_start();
        while day <= sessions::coverage_end() {
            if sessions::is_session(day).unwrap() {
                count += 1;
                max_count = max_count.max(count);
                if sessions::is_last_session_of_month(day).unwrap() {
                    count = 0;
                }
            }
            day = day.succ_opt().unwrap();
        }
        assert!(max_count <= MAX_SESSIONS_PER_MONTH, "{max_count}");
    }
}
