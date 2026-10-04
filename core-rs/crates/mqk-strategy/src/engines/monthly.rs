//! Month-end structure of a daily window, resolved only from the `us_equity_regular_sessions_v1`
//! authority in `mqk-integrity` (never from a weekday or price heuristic).
//!
//! A monthly engine's target at bar `t` is the decision taken at the latest month-end session
//! `m <= t`, held until the next month-end. It is therefore a pure function of the latest
//! `REQUIRED` contiguous completed sessions: a month has at most 23 sessions, so `t - m <= 22`.

use chrono::{Datelike, NaiveDate};
use mqk_integrity::sessions;

use crate::BarStub;

/// Most sessions after a month-end decision bar before the next month-end.
pub(crate) const MAX_SESSIONS_SINCE_MONTH_END: usize = 22;

fn month_key(d: NaiveDate) -> (i32, u32) {
    (d.year(), d.month())
}

/// Indices (ascending) of the month-end sessions in `win`, or `None` when the window is not a
/// run of consecutive regular sessions (unknown label, holiday label, a missing or repeated
/// session) or the next session after the last bar is outside the calendar coverage.
/// Month-end = the next regular session falls in a different calendar month.
pub(crate) fn month_end_indices(win: &[BarStub]) -> Option<Vec<usize>> {
    let mut dates = Vec::with_capacity(win.len());
    for b in win {
        let d = sessions::session_of_daily_bar(b.end_ts).ok()?;
        if let Some(&prev) = dates.last() {
            if sessions::next_session_after(prev).ok()? != d {
                return None;
            }
        }
        dates.push(d);
    }
    let last = *dates.last()?;
    let after_last = sessions::next_session_after(last).ok()?;
    let mut out = Vec::new();
    for (i, d) in dates.iter().enumerate() {
        let next = dates.get(i + 1).copied().unwrap_or(after_last);
        if month_key(*d) != month_key(next) {
            out.push(i);
        }
    }
    Some(out)
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
}
