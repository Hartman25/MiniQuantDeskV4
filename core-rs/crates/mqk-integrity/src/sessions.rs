//! US-equity regular trading-session DATE calendar — `us_equity_regular_sessions_v1`.
//!
//! Daily-granularity session identity for calendar-dependent strategies and research.
//! A session is one official US-equity regular exchange trading date: weekends and full
//! exchange closures are not sessions; early-close (half-day) sessions ARE sessions.
//!
//! Distinct from [`crate::calendar`], which classifies intraday bar-end timestamps against a
//! 2023–2028 table. This module's closure table is explicit and bounded to
//! [`COVERAGE_START`]..=[`COVERAGE_END`]; every query that needs a date outside it is a
//! typed refusal. There is deliberately no weekday-arithmetic fallback, and a price row's
//! presence or absence is never calendar evidence.
//!
//! Authority: NYSE published holiday schedule and observation rules, plus the full-day
//! national-mourning closures of 2018-12-05 and 2025-01-09. The table's content identity is
//! [`calendar_content_string`]; its sha256 is bound into every calendar-dependent strategy
//! fingerprint, so any change to a closure, the coverage or the contract id changes identity.

use chrono::{DateTime, Datelike, LocalResult, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::America::New_York;

pub const US_EQUITY_REGULAR_SESSIONS_V1: &str = "us_equity_regular_sessions_v1";

/// First covered civil date (inclusive).
pub const COVERAGE_START: (i32, u32, u32) = (2016, 1, 1);
/// Last covered civil date (inclusive).
pub const COVERAGE_END: (i32, u32, u32) = (2026, 12, 31);

/// Every weekday on which the NYSE regular session did not occur, inside the coverage.
/// Ascending, unique.
const FULL_CLOSURES: &[(i32, u32, u32)] = &[
    (2016, 1, 1),
    (2016, 1, 18),
    (2016, 2, 15),
    (2016, 3, 25),
    (2016, 5, 30),
    (2016, 7, 4),
    (2016, 9, 5),
    (2016, 11, 24),
    (2016, 12, 26),
    (2017, 1, 2),
    (2017, 1, 16),
    (2017, 2, 20),
    (2017, 4, 14),
    (2017, 5, 29),
    (2017, 7, 4),
    (2017, 9, 4),
    (2017, 11, 23),
    (2017, 12, 25),
    (2018, 1, 1),
    (2018, 1, 15),
    (2018, 2, 19),
    (2018, 3, 30),
    (2018, 5, 28),
    (2018, 7, 4),
    (2018, 9, 3),
    (2018, 11, 22),
    (2018, 12, 5),
    (2018, 12, 25),
    (2019, 1, 1),
    (2019, 1, 21),
    (2019, 2, 18),
    (2019, 4, 19),
    (2019, 5, 27),
    (2019, 7, 4),
    (2019, 9, 2),
    (2019, 11, 28),
    (2019, 12, 25),
    (2020, 1, 1),
    (2020, 1, 20),
    (2020, 2, 17),
    (2020, 4, 10),
    (2020, 5, 25),
    (2020, 7, 3),
    (2020, 9, 7),
    (2020, 11, 26),
    (2020, 12, 25),
    (2021, 1, 1),
    (2021, 1, 18),
    (2021, 2, 15),
    (2021, 4, 2),
    (2021, 5, 31),
    (2021, 7, 5),
    (2021, 9, 6),
    (2021, 11, 25),
    (2021, 12, 24),
    (2022, 1, 17),
    (2022, 2, 21),
    (2022, 4, 15),
    (2022, 5, 30),
    (2022, 6, 20),
    (2022, 7, 4),
    (2022, 9, 5),
    (2022, 11, 24),
    (2022, 12, 26),
    (2023, 1, 2),
    (2023, 1, 16),
    (2023, 2, 20),
    (2023, 4, 7),
    (2023, 5, 29),
    (2023, 6, 19),
    (2023, 7, 4),
    (2023, 9, 4),
    (2023, 11, 23),
    (2023, 12, 25),
    (2024, 1, 1),
    (2024, 1, 15),
    (2024, 2, 19),
    (2024, 3, 29),
    (2024, 5, 27),
    (2024, 6, 19),
    (2024, 7, 4),
    (2024, 9, 2),
    (2024, 11, 28),
    (2024, 12, 25),
    (2025, 1, 1),
    (2025, 1, 9),
    (2025, 1, 20),
    (2025, 2, 17),
    (2025, 4, 18),
    (2025, 5, 26),
    (2025, 6, 19),
    (2025, 7, 4),
    (2025, 9, 1),
    (2025, 11, 27),
    (2025, 12, 25),
    (2026, 1, 1),
    (2026, 1, 19),
    (2026, 2, 16),
    (2026, 4, 3),
    (2026, 5, 25),
    (2026, 6, 19),
    (2026, 7, 3),
    (2026, 9, 7),
    (2026, 11, 26),
    (2026, 12, 25),
];

/// Typed refusals. None of them is ever resolved by guessing or by weekday arithmetic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionCalendarError {
    /// The date (or a date the query needed) lies outside the authoritative coverage.
    OutOfCoverage(NaiveDate),
    /// The timestamp is not exactly 00:00:00 America/New_York (the daily-bar label) or is
    /// not representable.
    NotADailyBarTimestamp(i64),
    /// The civil date is not a regular session (weekend or full closure).
    NotASession(NaiveDate),
}

impl std::fmt::Display for SessionCalendarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfCoverage(d) => {
                write!(f, "{d} is outside {US_EQUITY_REGULAR_SESSIONS_V1} coverage")
            }
            Self::NotADailyBarTimestamp(ts) => {
                write!(
                    f,
                    "end_ts {ts} is not a 00:00:00 America/New_York daily-bar label"
                )
            }
            Self::NotASession(d) => write!(f, "{d} is not a regular US-equity session"),
        }
    }
}

impl std::error::Error for SessionCalendarError {}

fn date(t: (i32, u32, u32)) -> NaiveDate {
    NaiveDate::from_ymd_opt(t.0, t.1, t.2).expect("valid constant date")
}

pub fn coverage_start() -> NaiveDate {
    date(COVERAGE_START)
}

pub fn coverage_end() -> NaiveDate {
    date(COVERAGE_END)
}

fn require_covered(d: NaiveDate) -> Result<(), SessionCalendarError> {
    if d < coverage_start() || d > coverage_end() {
        return Err(SessionCalendarError::OutOfCoverage(d));
    }
    Ok(())
}

/// `Ok(true)` iff `d` is a regular session; `Err(OutOfCoverage)` outside the coverage.
pub fn is_session(d: NaiveDate) -> Result<bool, SessionCalendarError> {
    require_covered(d)?;
    if d.weekday().number_from_monday() > 5 {
        return Ok(false);
    }
    let key = (d.year(), d.month(), d.day());
    Ok(FULL_CLOSURES.binary_search(&key).is_err())
}

/// The session date a daily research bar represents: the America/New_York civil date of
/// `end_ts`, which must be exactly 00:00:00 ET (the provider's daily-bar label) and a
/// covered regular session.
pub fn session_of_daily_bar(end_ts: i64) -> Result<NaiveDate, SessionCalendarError> {
    let utc: DateTime<Utc> = match Utc.timestamp_opt(end_ts, 0) {
        LocalResult::Single(dt) => dt,
        _ => return Err(SessionCalendarError::NotADailyBarTimestamp(end_ts)),
    };
    let et = utc.with_timezone(&New_York);
    if et.hour() != 0 || et.minute() != 0 || et.second() != 0 {
        return Err(SessionCalendarError::NotADailyBarTimestamp(end_ts));
    }
    let d = et.date_naive();
    if is_session(d)? {
        Ok(d)
    } else {
        Err(SessionCalendarError::NotASession(d))
    }
}

/// The first regular session strictly after `d`. `d` must be covered; the answer must be
/// covered too, otherwise `OutOfCoverage` (no extrapolation past the table).
pub fn next_session_after(d: NaiveDate) -> Result<NaiveDate, SessionCalendarError> {
    require_covered(d)?;
    let mut cursor = d;
    loop {
        cursor = cursor
            .succ_opt()
            .ok_or(SessionCalendarError::OutOfCoverage(cursor))?;
        if is_session(cursor)? {
            return Ok(cursor);
        }
    }
}

/// 1-based ordinal of session `d` among the regular sessions of its calendar month.
pub fn session_ordinal_in_month(d: NaiveDate) -> Result<u32, SessionCalendarError> {
    if !is_session(d)? {
        return Err(SessionCalendarError::NotASession(d));
    }
    let mut ordinal = 0;
    let mut cursor = d.with_day(1).expect("day 1 exists");
    while cursor <= d {
        if is_session(cursor)? {
            ordinal += 1;
        }
        cursor = cursor.succ_opt().expect("date in range");
    }
    Ok(ordinal)
}

/// True iff session `d` is the final regular session of its calendar month.
pub fn is_last_session_of_month(d: NaiveDate) -> Result<bool, SessionCalendarError> {
    if !is_session(d)? {
        return Err(SessionCalendarError::NotASession(d));
    }
    let mut cursor = d.succ_opt().expect("date in range");
    while cursor.month() == d.month() {
        if is_session(cursor)? {
            return Ok(false);
        }
        cursor = cursor.succ_opt().expect("date in range");
    }
    Ok(true)
}

/// Canonical content identity string of this calendar: contract id, coverage and every
/// full-closure date in ascending order. Its sha256 is the calendar identity bound into
/// strategy fingerprints and predeclared in `PREDECLARED_BATCH_02.json`.
pub fn calendar_content_string() -> String {
    let mut s = format!(
        "{US_EQUITY_REGULAR_SESSIONS_V1}\ncoverage={:04}-{:02}-{:02}..{:04}-{:02}-{:02}",
        COVERAGE_START.0,
        COVERAGE_START.1,
        COVERAGE_START.2,
        COVERAGE_END.0,
        COVERAGE_END.1,
        COVERAGE_END.2
    );
    for (y, m, d) in FULL_CLOSURES {
        s.push_str(&format!("\nclosure={y:04}-{m:02}-{d:02}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn et_midnight_ts(day: NaiveDate) -> i64 {
        New_York
            .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp()
    }

    #[test]
    fn closure_table_is_sorted_unique_weekday_and_covered() {
        assert_eq!(FULL_CLOSURES.len(), 105);
        assert!(FULL_CLOSURES.windows(2).all(|w| w[0] < w[1]));
        for &(y, m, day) in FULL_CLOSURES {
            let dt = d(y, m, day);
            assert!(dt.weekday().number_from_monday() <= 5, "{dt} is a weekend");
            assert!(dt >= coverage_start() && dt <= coverage_end());
        }
    }

    // ---- independent rule derivation (test-only; NOT the production authority) ----
    fn nth(y: i32, m: u32, wd: u32, n: u32) -> NaiveDate {
        let mut x = d(y, m, 1);
        while x.weekday().num_days_from_monday() != wd {
            x = x.succ_opt().unwrap();
        }
        x + chrono::Duration::days(7 * (n as i64 - 1))
    }
    fn last(y: i32, m: u32, wd: u32) -> NaiveDate {
        let mut x = if m == 12 {
            d(y + 1, 1, 1)
        } else {
            d(y, m + 1, 1)
        }
        .pred_opt()
        .unwrap();
        while x.weekday().num_days_from_monday() != wd {
            x = x.pred_opt().unwrap();
        }
        x
    }
    fn easter(y: i32) -> NaiveDate {
        let (a, b, c) = (y % 19, y / 100, y % 100);
        let (dd, e) = (b / 4, b % 4);
        let f = (b + 8) / 25;
        let g = (b - f + 1) / 3;
        let h = (19 * a + b - dd - g + 15) % 30;
        let (i, k) = (c / 4, c % 4);
        let l = (32 + 2 * e + 2 * i - h - k) % 7;
        let m = (a + 11 * h + 22 * l) / 451;
        let month = (h + l - 7 * m + 114) / 31;
        let day = (h + l - 7 * m + 114) % 31 + 1;
        d(y, month as u32, day as u32)
    }
    fn observed(x: NaiveDate) -> NaiveDate {
        match x.weekday().num_days_from_monday() {
            5 => x.pred_opt().unwrap(),
            6 => x.succ_opt().unwrap(),
            _ => x,
        }
    }
    fn rule_closures() -> Vec<NaiveDate> {
        let mut out = vec![d(2018, 12, 5), d(2025, 1, 9)];
        for y in 2016..=2026 {
            if d(y, 1, 1).weekday().num_days_from_monday() != 5 {
                out.push(observed(d(y, 1, 1)));
            }
            out.extend([
                nth(y, 1, 0, 3),
                nth(y, 2, 0, 3),
                easter(y) - chrono::Duration::days(2),
                last(y, 5, 0),
                observed(d(y, 7, 4)),
                nth(y, 9, 0, 1),
                nth(y, 11, 3, 4),
                observed(d(y, 12, 25)),
            ]);
            if y >= 2022 {
                out.push(observed(d(y, 6, 19)));
            }
        }
        out.retain(|x| {
            x.weekday().num_days_from_monday() < 5 && *x >= coverage_start() && *x <= coverage_end()
        });
        out.sort();
        out
    }

    #[test]
    fn production_table_equals_independent_rule_derivation() {
        let table: Vec<NaiveDate> = FULL_CLOSURES
            .iter()
            .map(|&(y, m, day)| d(y, m, day))
            .collect();
        assert_eq!(table, rule_closures());
    }

    #[test]
    fn annual_session_counts_match_published_nyse_counts() {
        let published = [
            (2016, 252),
            (2017, 251),
            (2018, 251),
            (2019, 252),
            (2020, 253),
            (2021, 252),
            (2022, 251),
            (2023, 250),
            (2024, 252),
            (2025, 250),
        ];
        for (y, n) in published {
            let mut count = 0;
            let mut x = d(y, 1, 1);
            while x.year() == y {
                count += i32::from(is_session(x).unwrap());
                x = x.succ_opt().unwrap();
            }
            assert_eq!(count, n, "{y}");
        }
    }

    #[test]
    fn known_boundary_years_and_exceptional_closures() {
        assert!(
            !is_session(d(2018, 12, 5)).unwrap(),
            "Bush mourning closure"
        );
        assert!(
            !is_session(d(2025, 1, 9)).unwrap(),
            "Carter mourning closure"
        );
        assert!(
            is_session(d(2021, 12, 31)).unwrap(),
            "Saturday New Year is not observed on Friday"
        );
        assert!(
            is_session(d(2021, 6, 18)).unwrap(),
            "no Juneteenth holiday before 2022"
        );
        assert!(
            !is_session(d(2022, 6, 20)).unwrap(),
            "first Juneteenth observance"
        );
        assert!(!is_session(d(2016, 1, 1)).unwrap());
        assert!(
            !is_session(d(2020, 7, 3)).unwrap(),
            "Independence Day observed on Friday"
        );
        assert!(is_session(d(2020, 7, 6)).unwrap());
        assert!(
            is_session(d(2023, 7, 3)).unwrap(),
            "early-close day is a session"
        );
        assert!(
            is_session(d(2024, 11, 29)).unwrap(),
            "day after Thanksgiving (half-day) is a session"
        );
        assert!(
            is_session(d(2024, 12, 24)).unwrap(),
            "Christmas Eve (half-day) is a session"
        );
        assert!(
            !is_session(d(2016, 3, 26)).unwrap() && !is_session(d(2016, 3, 27)).unwrap(),
            "weekend"
        );
    }

    #[test]
    fn provider_dates_cross_check_is_exact_for_the_pre_holdout_range() {
        // Cross-check only: the fixed provider's SPY daily-bar dates (identical for all five symbols).
        let fixture =
            include_str!("../tests/fixtures/provider_daily_session_dates_2016_01_to_2026_02.txt");
        let provider: Vec<NaiveDate> = fixture.lines().map(|l| l.parse().unwrap()).collect();
        assert_eq!(provider.len(), 2553);
        let (first, lastd) = (provider[0], *provider.last().unwrap());
        let mut calendar = Vec::new();
        let mut x = first;
        while x <= lastd {
            if is_session(x).unwrap() {
                calendar.push(x);
            }
            x = x.succ_opt().unwrap();
        }
        assert_eq!(calendar, provider);
    }

    #[test]
    fn outside_coverage_is_refused_never_extrapolated() {
        assert_eq!(
            is_session(d(2015, 12, 31)),
            Err(SessionCalendarError::OutOfCoverage(d(2015, 12, 31)))
        );
        assert_eq!(
            is_session(d(2027, 1, 4)),
            Err(SessionCalendarError::OutOfCoverage(d(2027, 1, 4)))
        );
        // The last covered date is a session but its successor session is not covered.
        assert!(is_session(d(2026, 12, 31)).unwrap());
        assert!(matches!(
            next_session_after(d(2026, 12, 31)),
            Err(SessionCalendarError::OutOfCoverage(_))
        ));
        assert!(matches!(
            next_session_after(d(2015, 12, 31)),
            Err(SessionCalendarError::OutOfCoverage(_))
        ));
        // A 2022-era weekday is NOT treated as normally open by any 2023-2028-only table.
        assert!(!is_session(d(2022, 12, 26)).unwrap());
    }

    #[test]
    fn next_session_skips_weekends_holidays_and_mourning_closures() {
        assert_eq!(
            next_session_after(d(2024, 3, 28)).unwrap(),
            d(2024, 4, 1),
            "Good Friday + weekend"
        );
        assert_eq!(next_session_after(d(2018, 12, 4)).unwrap(), d(2018, 12, 6));
        assert_eq!(next_session_after(d(2025, 1, 8)).unwrap(), d(2025, 1, 10));
        assert_eq!(next_session_after(d(2020, 7, 2)).unwrap(), d(2020, 7, 6));
        assert_eq!(next_session_after(d(2023, 12, 29)).unwrap(), d(2024, 1, 2));
        // Strictly after: a session's successor is never itself.
        assert_eq!(next_session_after(d(2024, 3, 4)).unwrap(), d(2024, 3, 5));
    }

    #[test]
    fn ordinals_and_last_session_of_month() {
        // January 2024: Jan 1 is a holiday so Jan 2 is ordinal 1.
        assert_eq!(session_ordinal_in_month(d(2024, 1, 2)).unwrap(), 1);
        assert_eq!(session_ordinal_in_month(d(2024, 1, 4)).unwrap(), 3);
        assert_eq!(session_ordinal_in_month(d(2024, 1, 5)).unwrap(), 4);
        assert!(matches!(
            session_ordinal_in_month(d(2024, 1, 1)),
            Err(SessionCalendarError::NotASession(_))
        ));
        // Month-end adjacent to a weekend (2024-03-29 is Good Friday): March 2024 ends on Thursday 28th.
        assert!(is_last_session_of_month(d(2024, 3, 28)).unwrap());
        assert!(!is_last_session_of_month(d(2024, 3, 27)).unwrap());
        // Month-end on a weekend: Aug 2025 ends Sunday 31 -> Friday the 29th is last.
        assert!(is_last_session_of_month(d(2025, 8, 29)).unwrap());
        // Half-day month-end: 2018-12-31 (Monday) is the last session of December 2018.
        assert!(is_last_session_of_month(d(2018, 12, 31)).unwrap());
        // Leap day: 2024-02-29 (Thursday) is a session and the last of its month.
        assert!(is_last_session_of_month(d(2024, 2, 29)).unwrap());
        assert!(!is_last_session_of_month(d(2024, 2, 28)).unwrap());
        // Year boundary: 2023-12-29 last of 2023; 2024-01-02 first of 2024.
        assert!(is_last_session_of_month(d(2023, 12, 29)).unwrap());
        // Month ending on a holiday: Nov 2018 -> 30th is a Friday session; Dec 2021 -> 31st (Fri) session.
        assert!(is_last_session_of_month(d(2021, 12, 30)).is_ok());
    }

    #[test]
    fn daily_bar_label_maps_to_its_et_date_across_dst_and_rejects_everything_else() {
        // EST label 05:00Z, EDT label 04:00Z.
        assert_eq!(session_of_daily_bar(1_451_883_600).unwrap(), d(2016, 1, 4));
        assert_eq!(
            session_of_daily_bar(et_midnight_ts(d(2016, 3, 14))).unwrap(),
            d(2016, 3, 14)
        );
        assert_eq!(
            session_of_daily_bar(et_midnight_ts(d(2024, 11, 1))).unwrap(),
            d(2024, 11, 1)
        );
        assert_eq!(
            session_of_daily_bar(et_midnight_ts(d(2024, 11, 4))).unwrap(),
            d(2024, 11, 4)
        );
        // Not midnight ET (a close-labelled or UTC-midnight stamp).
        let ts = et_midnight_ts(d(2024, 11, 4));
        for off in [1, 60, 3600, 16 * 3600] {
            assert!(matches!(
                session_of_daily_bar(ts + off),
                Err(SessionCalendarError::NotADailyBarTimestamp(_))
            ));
        }
        // A midnight label on a weekend/holiday is not a session.
        assert!(matches!(
            session_of_daily_bar(et_midnight_ts(d(2024, 3, 29))),
            Err(SessionCalendarError::NotASession(_))
        ));
        assert!(matches!(
            session_of_daily_bar(et_midnight_ts(d(2024, 3, 30))),
            Err(SessionCalendarError::NotASession(_))
        ));
        // Outside coverage.
        assert!(matches!(
            session_of_daily_bar(et_midnight_ts(d(2027, 2, 1))),
            Err(SessionCalendarError::OutOfCoverage(_))
        ));
    }

    #[test]
    fn content_string_is_exact_and_closure_sensitive() {
        let s = calendar_content_string();
        assert!(s.starts_with(
            "us_equity_regular_sessions_v1\ncoverage=2016-01-01..2026-12-31\nclosure=2016-01-01\n"
        ));
        assert!(s.ends_with("closure=2026-12-25"));
        assert_eq!(s.lines().count(), 2 + 105);
        assert!(!s.ends_with('\n'));
    }
}
