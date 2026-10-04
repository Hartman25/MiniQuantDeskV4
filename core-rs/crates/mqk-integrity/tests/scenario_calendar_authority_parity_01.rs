//! US-equity calendar authority parity (M1 system closure).
//!
//! `calendar::CalendarSpec::NyseWeekdays` (intraday classifier, the Paper-path
//! authority via `NyseWeekdaysProvider`) and `sessions::us_equity_regular_sessions_v1`
//! (daily research/Backtest authority) must agree on every shared fact, and the
//! intraday classifier must refuse (never weekday-extrapolate) outside its table.
//!
//! Published authority for 2027-2028 and the 2026 early closes: NYSE holiday and
//! trading-hours page (nyse.com/markets/hours-calendars). 2023/2025 early closes
//! and the 2025-01-09 national-mourning closure are NYSE-published history.

use chrono::{Datelike, NaiveDate, TimeZone};
use chrono_tz::America::New_York;
use mqk_integrity::{nyse_early_close_et, sessions, CalendarSpec};

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).unwrap()
}

/// 12:00 ET on the civil date (never ambiguous across DST).
fn midday_et_ts(day: NaiveDate) -> i64 {
    New_York
        .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
        .single()
        .unwrap()
        .timestamp()
}

fn exchange_state(day: NaiveDate) -> &'static str {
    CalendarSpec::NyseWeekdays.classify_exchange_calendar(midday_et_ts(day))
}

fn dates(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    let mut out = Vec::new();
    let mut x = from;
    while x <= to {
        out.push(x);
        x = x.succ_opt().unwrap();
    }
    out
}

#[test]
fn intraday_classifier_agrees_with_daily_sessions_on_every_shared_date() {
    // 2023-01-01..=2026-12-31 is covered by both authorities.
    for day in dates(d(2023, 1, 1), d(2026, 12, 31)) {
        let daily_is_session = sessions::is_session(day).unwrap();
        assert_eq!(
            exchange_state(day) == "open",
            daily_is_session,
            "{day}: intraday={} daily_is_session={daily_is_session}",
            exchange_state(day)
        );
    }
}

#[test]
fn published_closures_2027_2028_and_weekends_are_classified_exactly() {
    let expected: Vec<NaiveDate> = vec![
        d(2027, 1, 1),
        d(2027, 1, 18),
        d(2027, 2, 15),
        d(2027, 3, 26),
        d(2027, 5, 31),
        d(2027, 6, 18),
        d(2027, 7, 5),
        d(2027, 9, 6),
        d(2027, 11, 25),
        d(2027, 12, 24),
        d(2028, 1, 17),
        d(2028, 2, 21),
        d(2028, 4, 14),
        d(2028, 5, 29),
        d(2028, 6, 19),
        d(2028, 7, 4),
        d(2028, 9, 4),
        d(2028, 11, 23),
        d(2028, 12, 25),
    ];
    let observed: Vec<NaiveDate> = dates(d(2027, 1, 1), d(2028, 12, 31))
        .into_iter()
        .filter(|x| x.weekday().number_from_monday() <= 5 && exchange_state(*x) == "holiday")
        .collect();
    assert_eq!(observed, expected);
    // Saturday New Year's Day 2028 is not observed on Friday 2027-12-31.
    assert_eq!(exchange_state(d(2027, 12, 31)), "open");
    // Weekends are "closed", never "holiday"/"open".
    for day in dates(d(2023, 1, 1), d(2028, 12, 31)) {
        if day.weekday().number_from_monday() > 5 {
            assert_eq!(exchange_state(day), "closed", "{day}");
        }
    }
}

#[test]
fn extraordinary_closure_and_year_boundaries() {
    assert_eq!(exchange_state(d(2025, 1, 9)), "holiday", "Carter mourning");
    assert_eq!(exchange_state(d(2025, 1, 8)), "open");
    assert_eq!(exchange_state(d(2025, 1, 10)), "open");
    // Year boundaries inside coverage.
    assert_eq!(exchange_state(d(2023, 1, 2)), "holiday");
    assert_eq!(exchange_state(d(2023, 12, 29)), "open");
    assert_eq!(exchange_state(d(2024, 1, 1)), "holiday");
    assert_eq!(exchange_state(d(2028, 12, 29)), "open");
}

#[test]
fn early_close_table_equals_published_schedule() {
    let expected: Vec<(NaiveDate, (u32, u32))> = [
        (2023, 7, 3),
        (2023, 11, 24),
        (2024, 7, 3),
        (2024, 11, 29),
        (2024, 12, 24),
        (2025, 7, 3),
        (2025, 11, 28),
        (2025, 12, 24),
        (2026, 11, 27),
        (2026, 12, 24),
        (2027, 11, 26),
        (2028, 7, 3),
        (2028, 11, 24),
    ]
    .iter()
    .map(|&(y, m, day)| (d(y, m, day), (13, 0)))
    .collect();
    let observed: Vec<(NaiveDate, (u32, u32))> = dates(d(2023, 1, 1), d(2028, 12, 31))
        .into_iter()
        .filter_map(|x| {
            nyse_early_close_et(x.year() as i64, x.month() as i64, x.day() as i64).map(|c| (x, c))
        })
        .collect();
    assert_eq!(observed, expected);
    // An early-close day is a session, never a closure.
    for (day, _) in &expected {
        assert_eq!(exchange_state(*day), "open", "{day}");
    }
}

#[test]
fn outside_declared_coverage_the_classifier_is_closed_never_weekday_open() {
    // Ordinary Fridays/Mondays that a weekday fallback would call open.
    for day in [d(2022, 12, 30), d(2029, 1, 2), d(2030, 3, 4), d(2016, 6, 1)] {
        let ts = midday_et_ts(day);
        assert_eq!(
            CalendarSpec::NyseWeekdays.classify_market_session(ts),
            "closed",
            "{day}"
        );
        assert_eq!(
            CalendarSpec::NyseWeekdays.classify_exchange_calendar(ts),
            "closed",
            "{day}"
        );
    }
    // Last and first covered weekdays remain classified.
    assert_eq!(
        CalendarSpec::NyseWeekdays.classify_market_session(midday_et_ts(d(2028, 12, 29))),
        "regular"
    );
    assert_eq!(
        CalendarSpec::NyseWeekdays.classify_market_session(midday_et_ts(d(2023, 1, 3))),
        "regular"
    );
}

#[test]
fn gap_detection_outside_coverage_still_expects_weekday_bars() {
    // Fail-closed for gaps: where holiday truth is unknown, a weekday slot is
    // still EXPECTED, so a missing bar is detected rather than excused.
    let day = d(2029, 1, 2); // a Tuesday outside the table
    let open = New_York
        .from_local_datetime(&day.and_hms_opt(9, 30, 0).unwrap())
        .single()
        .unwrap()
        .timestamp();
    let missing =
        CalendarSpec::NyseWeekdays.missing_bars_between(open + 300, open + 300 + 3 * 300, 300);
    assert_eq!(missing, 2);
}
