//! US-equity regular trading-session DATE calendar — `us_equity_regular_sessions_v2`.
//!
//! Versioned successor of [`crate::sessions`] (`us_equity_regular_sessions_v1`, which is
//! untouched: its table, coverage, content string and hash stay the identity of every frozen
//! campaign). Differences from v1:
//!
//! * coverage `2016-01-01..=2028-12-31` (v1 ends 2026-12-31; the Paper-runtime table in
//!   [`crate::calendar`] covers 2023-2028);
//! * every full closure is classified [`ClosureClass::Scheduled`] (derivable from the published
//!   exchange observation rules and the civil date alone) or [`ClosureClass::Unscheduled`]
//!   (not derivable: national-mourning closures);
//! * early-close sessions are recorded (they are sessions);
//! * as-of knowledge is explicit: [`closure_knowable_at`].
//!
//! Out-of-coverage dates are typed refusals; there is no weekday-arithmetic fallback and a
//! price row's presence or absence is never calendar evidence. The table's content identity is
//! [`calendar_content_string`]; its sha256 is bound into every v2-dependent strategy
//! fingerprint, so any change to a closure, a class, an early close, the coverage or the
//! contract id changes identity.
//!
//! Authority: the v1 closure table (2016-2026, cross-checked against its provider-date fixture)
//! and the Paper-runtime table (2023-2028); tests fail on any contradiction between v1, v2 and
//! the Paper-runtime table. The 2016-2022 early closes are derived by the exchange rule
//! (July 3 before a weekday July 4, the day after Thanksgiving, Christmas Eve before a weekday
//! Christmas); that rule reproduces the published 2023-2028 early-close table exactly.

use chrono::{DateTime, Datelike, LocalResult, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::America::New_York;

pub const US_EQUITY_REGULAR_SESSIONS_V2: &str = "us_equity_regular_sessions_v2";

/// First covered civil date (inclusive).
pub const COVERAGE_START: (i32, u32, u32) = (2016, 1, 1);
/// Last covered civil date (inclusive).
pub const COVERAGE_END: (i32, u32, u32) = (2028, 12, 31);

/// Close time of every early-close session, America/New_York.
pub const EARLY_CLOSE_ET: &str = "13:00";

/// Why a weekday had no regular session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClosureClass {
    /// Known prospectively: derivable from the exchange observation rules and the date alone.
    Scheduled,
    /// Not derivable from the rules (national mourning); knowable only once announced.
    Unscheduled,
}

impl ClosureClass {
    fn tag(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Unscheduled => "unscheduled",
        }
    }
}

/// Every weekday on which the NYSE regular session did not occur, inside the coverage.
/// Ascending, unique.
const CLOSURES: &[(i32, u32, u32, ClosureClass)] = &[
    (2016, 1, 1, ClosureClass::Scheduled),
    (2016, 1, 18, ClosureClass::Scheduled),
    (2016, 2, 15, ClosureClass::Scheduled),
    (2016, 3, 25, ClosureClass::Scheduled),
    (2016, 5, 30, ClosureClass::Scheduled),
    (2016, 7, 4, ClosureClass::Scheduled),
    (2016, 9, 5, ClosureClass::Scheduled),
    (2016, 11, 24, ClosureClass::Scheduled),
    (2016, 12, 26, ClosureClass::Scheduled),
    (2017, 1, 2, ClosureClass::Scheduled),
    (2017, 1, 16, ClosureClass::Scheduled),
    (2017, 2, 20, ClosureClass::Scheduled),
    (2017, 4, 14, ClosureClass::Scheduled),
    (2017, 5, 29, ClosureClass::Scheduled),
    (2017, 7, 4, ClosureClass::Scheduled),
    (2017, 9, 4, ClosureClass::Scheduled),
    (2017, 11, 23, ClosureClass::Scheduled),
    (2017, 12, 25, ClosureClass::Scheduled),
    (2018, 1, 1, ClosureClass::Scheduled),
    (2018, 1, 15, ClosureClass::Scheduled),
    (2018, 2, 19, ClosureClass::Scheduled),
    (2018, 3, 30, ClosureClass::Scheduled),
    (2018, 5, 28, ClosureClass::Scheduled),
    (2018, 7, 4, ClosureClass::Scheduled),
    (2018, 9, 3, ClosureClass::Scheduled),
    (2018, 11, 22, ClosureClass::Scheduled),
    (2018, 12, 5, ClosureClass::Unscheduled),
    (2018, 12, 25, ClosureClass::Scheduled),
    (2019, 1, 1, ClosureClass::Scheduled),
    (2019, 1, 21, ClosureClass::Scheduled),
    (2019, 2, 18, ClosureClass::Scheduled),
    (2019, 4, 19, ClosureClass::Scheduled),
    (2019, 5, 27, ClosureClass::Scheduled),
    (2019, 7, 4, ClosureClass::Scheduled),
    (2019, 9, 2, ClosureClass::Scheduled),
    (2019, 11, 28, ClosureClass::Scheduled),
    (2019, 12, 25, ClosureClass::Scheduled),
    (2020, 1, 1, ClosureClass::Scheduled),
    (2020, 1, 20, ClosureClass::Scheduled),
    (2020, 2, 17, ClosureClass::Scheduled),
    (2020, 4, 10, ClosureClass::Scheduled),
    (2020, 5, 25, ClosureClass::Scheduled),
    (2020, 7, 3, ClosureClass::Scheduled),
    (2020, 9, 7, ClosureClass::Scheduled),
    (2020, 11, 26, ClosureClass::Scheduled),
    (2020, 12, 25, ClosureClass::Scheduled),
    (2021, 1, 1, ClosureClass::Scheduled),
    (2021, 1, 18, ClosureClass::Scheduled),
    (2021, 2, 15, ClosureClass::Scheduled),
    (2021, 4, 2, ClosureClass::Scheduled),
    (2021, 5, 31, ClosureClass::Scheduled),
    (2021, 7, 5, ClosureClass::Scheduled),
    (2021, 9, 6, ClosureClass::Scheduled),
    (2021, 11, 25, ClosureClass::Scheduled),
    (2021, 12, 24, ClosureClass::Scheduled),
    (2022, 1, 17, ClosureClass::Scheduled),
    (2022, 2, 21, ClosureClass::Scheduled),
    (2022, 4, 15, ClosureClass::Scheduled),
    (2022, 5, 30, ClosureClass::Scheduled),
    (2022, 6, 20, ClosureClass::Scheduled),
    (2022, 7, 4, ClosureClass::Scheduled),
    (2022, 9, 5, ClosureClass::Scheduled),
    (2022, 11, 24, ClosureClass::Scheduled),
    (2022, 12, 26, ClosureClass::Scheduled),
    (2023, 1, 2, ClosureClass::Scheduled),
    (2023, 1, 16, ClosureClass::Scheduled),
    (2023, 2, 20, ClosureClass::Scheduled),
    (2023, 4, 7, ClosureClass::Scheduled),
    (2023, 5, 29, ClosureClass::Scheduled),
    (2023, 6, 19, ClosureClass::Scheduled),
    (2023, 7, 4, ClosureClass::Scheduled),
    (2023, 9, 4, ClosureClass::Scheduled),
    (2023, 11, 23, ClosureClass::Scheduled),
    (2023, 12, 25, ClosureClass::Scheduled),
    (2024, 1, 1, ClosureClass::Scheduled),
    (2024, 1, 15, ClosureClass::Scheduled),
    (2024, 2, 19, ClosureClass::Scheduled),
    (2024, 3, 29, ClosureClass::Scheduled),
    (2024, 5, 27, ClosureClass::Scheduled),
    (2024, 6, 19, ClosureClass::Scheduled),
    (2024, 7, 4, ClosureClass::Scheduled),
    (2024, 9, 2, ClosureClass::Scheduled),
    (2024, 11, 28, ClosureClass::Scheduled),
    (2024, 12, 25, ClosureClass::Scheduled),
    (2025, 1, 1, ClosureClass::Scheduled),
    (2025, 1, 9, ClosureClass::Unscheduled),
    (2025, 1, 20, ClosureClass::Scheduled),
    (2025, 2, 17, ClosureClass::Scheduled),
    (2025, 4, 18, ClosureClass::Scheduled),
    (2025, 5, 26, ClosureClass::Scheduled),
    (2025, 6, 19, ClosureClass::Scheduled),
    (2025, 7, 4, ClosureClass::Scheduled),
    (2025, 9, 1, ClosureClass::Scheduled),
    (2025, 11, 27, ClosureClass::Scheduled),
    (2025, 12, 25, ClosureClass::Scheduled),
    (2026, 1, 1, ClosureClass::Scheduled),
    (2026, 1, 19, ClosureClass::Scheduled),
    (2026, 2, 16, ClosureClass::Scheduled),
    (2026, 4, 3, ClosureClass::Scheduled),
    (2026, 5, 25, ClosureClass::Scheduled),
    (2026, 6, 19, ClosureClass::Scheduled),
    (2026, 7, 3, ClosureClass::Scheduled),
    (2026, 9, 7, ClosureClass::Scheduled),
    (2026, 11, 26, ClosureClass::Scheduled),
    (2026, 12, 25, ClosureClass::Scheduled),
    (2027, 1, 1, ClosureClass::Scheduled),
    (2027, 1, 18, ClosureClass::Scheduled),
    (2027, 2, 15, ClosureClass::Scheduled),
    (2027, 3, 26, ClosureClass::Scheduled),
    (2027, 5, 31, ClosureClass::Scheduled),
    (2027, 6, 18, ClosureClass::Scheduled),
    (2027, 7, 5, ClosureClass::Scheduled),
    (2027, 9, 6, ClosureClass::Scheduled),
    (2027, 11, 25, ClosureClass::Scheduled),
    (2027, 12, 24, ClosureClass::Scheduled),
    (2028, 1, 17, ClosureClass::Scheduled),
    (2028, 2, 21, ClosureClass::Scheduled),
    (2028, 4, 14, ClosureClass::Scheduled),
    (2028, 5, 29, ClosureClass::Scheduled),
    (2028, 6, 19, ClosureClass::Scheduled),
    (2028, 7, 4, ClosureClass::Scheduled),
    (2028, 9, 4, ClosureClass::Scheduled),
    (2028, 11, 23, ClosureClass::Scheduled),
    (2028, 12, 25, ClosureClass::Scheduled),
];

/// NYSE early-close sessions (13:00 ET), inside the coverage. Ascending, unique, never a closure.
const EARLY_CLOSES: &[(i32, u32, u32)] = &[
    (2016, 11, 25),
    (2017, 7, 3),
    (2017, 11, 24),
    (2018, 7, 3),
    (2018, 11, 23),
    (2018, 12, 24),
    (2019, 7, 3),
    (2019, 11, 29),
    (2019, 12, 24),
    (2020, 11, 27),
    (2020, 12, 24),
    (2021, 11, 26),
    (2022, 11, 25),
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
];

/// Typed refusals. None of them is ever resolved by guessing or by weekday arithmetic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionCalendarV2Error {
    /// The date (or a date the query needed) lies outside the authoritative coverage.
    OutOfCoverage(NaiveDate),
    /// The timestamp is not exactly 00:00:00 America/New_York (the daily-bar label) or is
    /// not representable.
    NotADailyBarTimestamp(i64),
    /// The civil date is not a regular session (weekend or full closure).
    NotASession(NaiveDate),
    /// The date is not a weekday full closure.
    NotAClosure(NaiveDate),
}

impl std::fmt::Display for SessionCalendarV2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfCoverage(d) => {
                write!(f, "{d} is outside {US_EQUITY_REGULAR_SESSIONS_V2} coverage")
            }
            Self::NotADailyBarTimestamp(ts) => {
                write!(
                    f,
                    "end_ts {ts} is not a 00:00:00 America/New_York daily-bar label"
                )
            }
            Self::NotASession(d) => write!(f, "{d} is not a regular US-equity session"),
            Self::NotAClosure(d) => write!(f, "{d} is not a weekday full closure"),
        }
    }
}

impl std::error::Error for SessionCalendarV2Error {}

type Result<T> = std::result::Result<T, SessionCalendarV2Error>;

fn date(t: (i32, u32, u32)) -> NaiveDate {
    NaiveDate::from_ymd_opt(t.0, t.1, t.2).expect("valid constant date")
}

pub fn coverage_start() -> NaiveDate {
    date(COVERAGE_START)
}

pub fn coverage_end() -> NaiveDate {
    date(COVERAGE_END)
}

fn require_covered(d: NaiveDate) -> Result<()> {
    if d < coverage_start() || d > coverage_end() {
        return Err(SessionCalendarV2Error::OutOfCoverage(d));
    }
    Ok(())
}

fn is_weekday(d: NaiveDate) -> bool {
    d.weekday().number_from_monday() <= 5
}

/// The class of the weekday full closure on `d`: `Ok(None)` for a weekday with a regular
/// session and for weekends; `Err(OutOfCoverage)` outside the coverage.
pub fn closure_class(d: NaiveDate) -> Result<Option<ClosureClass>> {
    require_covered(d)?;
    let key = (d.year(), d.month(), d.day());
    Ok(CLOSURES
        .binary_search_by(|&(y, m, day, _)| (y, m, day).cmp(&key))
        .ok()
        .map(|i| CLOSURES[i].3))
}

/// `Ok(true)` iff `d` is a regular session (a weekday that is not a closure of any class).
pub fn is_session(d: NaiveDate) -> Result<bool> {
    let class = closure_class(d)?;
    Ok(is_weekday(d) && class.is_none())
}

/// `Ok(true)` iff `d` is a session as far as the *scheduled* calendar is concerned: a weekday
/// that is not a scheduled closure. An unscheduled closure date is still a scheduled session:
/// nothing prospective says it will not trade.
pub fn is_scheduled_session(d: NaiveDate) -> Result<bool> {
    let class = closure_class(d)?;
    Ok(is_weekday(d) && class != Some(ClosureClass::Scheduled))
}

/// `Ok(true)` iff `d` is a scheduled exchange holiday (a weekday scheduled full closure).
pub fn is_scheduled_holiday(d: NaiveDate) -> Result<bool> {
    Ok(closure_class(d)? == Some(ClosureClass::Scheduled))
}

/// `Ok(true)` iff `d` is a regular session that closes early (13:00 ET).
pub fn is_early_close(d: NaiveDate) -> Result<bool> {
    require_covered(d)?;
    let key = (d.year(), d.month(), d.day());
    Ok(EARLY_CLOSES.binary_search(&key).is_ok())
}

/// Whether the closure on `closure` can be known to a decision taken on `decision`.
/// Scheduled closures are rule-derived and knowable at any decision date. An unscheduled
/// closure is treated as knowable only from its own date: announcement dates are not recorded,
/// so this is the conservative reading, and no native decision rule may consume it earlier.
pub fn closure_knowable_at(closure: NaiveDate, decision: NaiveDate) -> Result<bool> {
    require_covered(decision)?;
    match closure_class(closure)? {
        Some(ClosureClass::Scheduled) => Ok(true),
        Some(ClosureClass::Unscheduled) => Ok(decision >= closure),
        None => Err(SessionCalendarV2Error::NotAClosure(closure)),
    }
}

/// The session date a daily research bar represents: the America/New_York civil date of
/// `end_ts`, which must be exactly 00:00:00 ET (the provider's daily-bar label) and a
/// covered regular session.
pub fn session_of_daily_bar(end_ts: i64) -> Result<NaiveDate> {
    let utc: DateTime<Utc> = match Utc.timestamp_opt(end_ts, 0) {
        LocalResult::Single(dt) => dt,
        _ => return Err(SessionCalendarV2Error::NotADailyBarTimestamp(end_ts)),
    };
    let et = utc.with_timezone(&New_York);
    if et.hour() != 0 || et.minute() != 0 || et.second() != 0 {
        return Err(SessionCalendarV2Error::NotADailyBarTimestamp(end_ts));
    }
    let d = et.date_naive();
    if is_session(d)? {
        Ok(d)
    } else {
        Err(SessionCalendarV2Error::NotASession(d))
    }
}

fn next_matching(d: NaiveDate, pred: fn(NaiveDate) -> Result<bool>) -> Result<NaiveDate> {
    require_covered(d)?;
    let mut cursor = d;
    loop {
        cursor = cursor
            .succ_opt()
            .ok_or(SessionCalendarV2Error::OutOfCoverage(cursor))?;
        if pred(cursor)? {
            return Ok(cursor);
        }
    }
}

/// The first regular session strictly after `d`. `d` must be covered; the answer must be
/// covered too, otherwise `OutOfCoverage` (no extrapolation past the table).
pub fn next_session_after(d: NaiveDate) -> Result<NaiveDate> {
    next_matching(d, is_session)
}

/// The first scheduled session strictly after `d` (an unscheduled closure date counts as a
/// session): the prospective calendar a decision taken before any surprise could rely on.
pub fn next_scheduled_session_after(d: NaiveDate) -> Result<NaiveDate> {
    next_matching(d, is_scheduled_session)
}

/// 1-based ordinal of session `d` among the regular sessions of its calendar month.
pub fn session_ordinal_in_month(d: NaiveDate) -> Result<u32> {
    if !is_session(d)? {
        return Err(SessionCalendarV2Error::NotASession(d));
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

/// True iff session `d` is the final regular session of its calendar month. A month whose
/// remaining days leave the coverage is `OutOfCoverage`, never assumed.
pub fn is_last_session_of_month(d: NaiveDate) -> Result<bool> {
    if !is_session(d)? {
        return Err(SessionCalendarV2Error::NotASession(d));
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

/// Canonical content identity string of this calendar: contract id, coverage, every full
/// closure with its class, and every early close, in ascending order. Its sha256 is the
/// calendar identity bound into v2 strategy fingerprints and predeclarations.
pub fn calendar_content_string() -> String {
    let mut s = format!(
        "{US_EQUITY_REGULAR_SESSIONS_V2}\ncoverage={:04}-{:02}-{:02}..{:04}-{:02}-{:02}",
        COVERAGE_START.0,
        COVERAGE_START.1,
        COVERAGE_START.2,
        COVERAGE_END.0,
        COVERAGE_END.1,
        COVERAGE_END.2
    );
    for (y, m, d, class) in CLOSURES {
        s.push_str(&format!("\nclosure={y:04}-{m:02}-{d:02}|{}", class.tag()));
    }
    for (y, m, d) in EARLY_CLOSES {
        s.push_str(&format!(
            "\nearly_close={y:04}-{m:02}-{d:02}|{EARLY_CLOSE_ET}"
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{calendar, sessions};

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

    fn every_day(from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
        let mut out = Vec::new();
        let mut x = from;
        while x <= to {
            out.push(x);
            x = x.succ_opt().unwrap();
        }
        out
    }

    fn closure_dates() -> Vec<NaiveDate> {
        CLOSURES
            .iter()
            .map(|&(y, m, day, _)| d(y, m, day))
            .collect()
    }

    #[test]
    fn tables_are_sorted_unique_weekday_covered_and_disjoint() {
        assert_eq!(CLOSURES.len(), 124);
        assert!(CLOSURES
            .windows(2)
            .all(|w| (w[0].0, w[0].1, w[0].2) < (w[1].0, w[1].1, w[1].2)));
        for day in closure_dates() {
            assert!(is_weekday(day), "{day} is a weekend");
            assert!(day >= coverage_start() && day <= coverage_end());
        }
        assert_eq!(EARLY_CLOSES.len(), 26);
        assert!(EARLY_CLOSES.windows(2).all(|w| w[0] < w[1]));
        for &(y, m, day) in EARLY_CLOSES {
            let x = d(y, m, day);
            assert!(is_session(x).unwrap(), "{x} early close must be a session");
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
        let mut x = d(y, m + 1, 1).pred_opt().unwrap();
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
    /// Scheduled closures by the published observation rules (New Year's Day on a Saturday is
    /// not observed on the preceding Friday; Juneteenth from 2022).
    fn rule_scheduled_closures() -> Vec<NaiveDate> {
        let mut out = Vec::new();
        for y in 2016..=2028 {
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
        out.retain(|x| is_weekday(*x) && *x >= coverage_start() && *x <= coverage_end());
        out.sort();
        out
    }
    fn rule_early_closes() -> Vec<NaiveDate> {
        let scheduled_session =
            |x: NaiveDate| is_weekday(x) && !rule_scheduled_closures().contains(&x);
        let mut out = Vec::new();
        for y in 2016..=2028 {
            let jul3 = d(y, 7, 3);
            if scheduled_session(jul3) && is_weekday(d(y, 7, 4)) {
                out.push(jul3);
            }
            out.push(nth(y, 11, 3, 4) + chrono::Duration::days(1));
            let eve = d(y, 12, 24);
            if scheduled_session(eve) && is_weekday(d(y, 12, 25)) {
                out.push(eve);
            }
        }
        out.sort();
        out
    }

    #[test]
    fn scheduled_closures_equal_independent_rule_derivation_and_unscheduled_are_exactly_two() {
        let scheduled: Vec<NaiveDate> = CLOSURES
            .iter()
            .filter(|c| c.3 == ClosureClass::Scheduled)
            .map(|&(y, m, day, _)| d(y, m, day))
            .collect();
        assert_eq!(scheduled, rule_scheduled_closures());
        let unscheduled: Vec<NaiveDate> = CLOSURES
            .iter()
            .filter(|c| c.3 == ClosureClass::Unscheduled)
            .map(|&(y, m, day, _)| d(y, m, day))
            .collect();
        assert_eq!(unscheduled, vec![d(2018, 12, 5), d(2025, 1, 9)]);
        // No unscheduled closure is derivable from the rules.
        for u in unscheduled {
            assert!(!rule_scheduled_closures().contains(&u));
        }
    }

    #[test]
    fn early_closes_equal_the_exchange_rule_and_the_published_runtime_table() {
        let table: Vec<NaiveDate> = EARLY_CLOSES
            .iter()
            .map(|&(y, m, day)| d(y, m, day))
            .collect();
        assert_eq!(table, rule_early_closes());
        // Reconciliation with the Paper-runtime early-close table on its whole coverage.
        for day in every_day(d(2023, 1, 1), d(2028, 12, 31)) {
            let runtime = calendar::nyse_early_close_et(
                day.year() as i64,
                day.month() as i64,
                day.day() as i64,
            );
            assert_eq!(
                runtime.is_some(),
                is_early_close(day).unwrap(),
                "{day} early-close disagreement"
            );
            if let Some(t) = runtime {
                assert_eq!(t, (13, 0));
            }
        }
    }

    #[test]
    fn v2_agrees_with_v1_on_every_date_both_cover() {
        for day in every_day(sessions::coverage_start(), sessions::coverage_end()) {
            assert_eq!(
                is_session(day).unwrap(),
                sessions::is_session(day).unwrap(),
                "{day}"
            );
        }
        // v1 next-session agrees wherever v1 can answer.
        for day in every_day(sessions::coverage_start(), d(2026, 12, 30)) {
            assert_eq!(
                next_session_after(day).unwrap(),
                sessions::next_session_after(day).unwrap(),
                "{day}"
            );
        }
    }

    #[test]
    fn v2_agrees_with_the_paper_runtime_table_on_every_date_both_cover() {
        for day in every_day(d(2023, 1, 1), d(2028, 12, 31)) {
            assert_eq!(
                is_session(day).unwrap(),
                calendar::nyse_is_regular_session_date(day).unwrap(),
                "{day}"
            );
        }
        for day in every_day(d(2023, 1, 1), d(2028, 12, 28)) {
            assert_eq!(
                next_session_after(day).unwrap(),
                calendar::nyse_next_regular_session_after(day).unwrap(),
                "{day}"
            );
        }
    }

    #[test]
    fn v1_identity_is_preserved_and_v2_has_its_own() {
        let v1 = sessions::calendar_content_string();
        assert!(v1.starts_with("us_equity_regular_sessions_v1\ncoverage=2016-01-01..2026-12-31\n"));
        assert_eq!(v1.lines().count(), 2 + 105);
        let v2 = calendar_content_string();
        assert!(v2.starts_with("us_equity_regular_sessions_v2\ncoverage=2016-01-01..2028-12-31\nclosure=2016-01-01|scheduled\n"));
        assert!(v2.ends_with("early_close=2028-11-24|13:00"));
        assert_eq!(v2.lines().count(), 2 + 124 + 26);
        assert!(!v2.ends_with('\n'));
        assert_ne!(v1, v2);
    }

    #[test]
    fn content_string_is_sensitive_to_every_semantic_element() {
        let base = calendar_content_string();
        // Class, closure date, early close, coverage and contract id are all in the string.
        assert!(base.contains("closure=2018-12-05|unscheduled"));
        assert!(base.contains("closure=2025-01-09|unscheduled"));
        assert!(base.contains("closure=2018-12-25|scheduled"));
        assert!(base.contains("early_close=2018-12-24|13:00"));
        assert_eq!(base.matches("|scheduled").count(), 122);
        assert_eq!(base.matches("|unscheduled").count(), 2);
        let flipped = base.replace(
            "closure=2018-12-05|unscheduled",
            "closure=2018-12-05|scheduled",
        );
        assert_ne!(flipped, base, "a class change must change identity");
        let dropped = base.replace("\nearly_close=2018-12-24|13:00", "");
        assert_ne!(dropped, base, "an early-close change must change identity");
    }

    #[test]
    fn known_holiday_versus_surprise_closure() {
        assert_eq!(
            closure_class(d(2024, 12, 25)).unwrap(),
            Some(ClosureClass::Scheduled)
        );
        assert_eq!(
            closure_class(d(2018, 12, 5)).unwrap(),
            Some(ClosureClass::Unscheduled)
        );
        assert_eq!(
            closure_class(d(2025, 1, 9)).unwrap(),
            Some(ClosureClass::Unscheduled)
        );
        assert_eq!(closure_class(d(2024, 12, 24)).unwrap(), None);
        assert_eq!(
            closure_class(d(2024, 12, 28)).unwrap(),
            None,
            "weekend has no closure class"
        );
        assert!(is_scheduled_holiday(d(2024, 11, 28)).unwrap());
        assert!(
            !is_scheduled_holiday(d(2025, 1, 9)).unwrap(),
            "mourning closure is no holiday"
        );
        assert!(
            !is_scheduled_holiday(d(2024, 12, 28)).unwrap(),
            "weekend is no holiday"
        );
        // Neither closure class is a session; only a surprise closure is a scheduled session.
        assert!(!is_session(d(2025, 1, 9)).unwrap());
        assert!(is_scheduled_session(d(2025, 1, 9)).unwrap());
        assert!(!is_scheduled_session(d(2024, 12, 25)).unwrap());
        assert_eq!(next_session_after(d(2025, 1, 8)).unwrap(), d(2025, 1, 10));
        assert_eq!(
            next_scheduled_session_after(d(2025, 1, 8)).unwrap(),
            d(2025, 1, 9)
        );
        assert_eq!(
            next_scheduled_session_after(d(2018, 12, 4)).unwrap(),
            d(2018, 12, 5)
        );
        assert_eq!(
            next_scheduled_session_after(d(2024, 12, 24)).unwrap(),
            d(2024, 12, 26)
        );
    }

    #[test]
    fn a_surprise_closure_is_never_knowable_before_its_own_date() {
        let carter = d(2025, 1, 9);
        assert!(!closure_knowable_at(carter, d(2025, 1, 8)).unwrap());
        assert!(!closure_knowable_at(carter, d(2024, 1, 2)).unwrap());
        assert!(closure_knowable_at(carter, carter).unwrap());
        assert!(closure_knowable_at(carter, d(2025, 1, 10)).unwrap());
        // A scheduled closure is knowable arbitrarily early.
        assert!(closure_knowable_at(d(2024, 12, 25), d(2016, 1, 4)).unwrap());
        assert_eq!(
            closure_knowable_at(d(2024, 12, 24), d(2024, 12, 1)),
            Err(SessionCalendarV2Error::NotAClosure(d(2024, 12, 24)))
        );
        assert!(matches!(
            closure_knowable_at(d(2024, 12, 25), d(2029, 1, 1)),
            Err(SessionCalendarV2Error::OutOfCoverage(_))
        ));
    }

    #[test]
    fn outside_coverage_is_refused_never_extrapolated() {
        for out in [d(2015, 12, 31), d(2029, 1, 1)] {
            assert_eq!(
                is_session(out),
                Err(SessionCalendarV2Error::OutOfCoverage(out))
            );
            assert!(closure_class(out).is_err());
            assert!(is_early_close(out).is_err());
            assert!(next_session_after(out).is_err());
            assert!(next_scheduled_session_after(out).is_err());
            assert!(is_scheduled_session(out).is_err());
        }
        // The last covered date is a session but its successor session is not covered.
        assert!(is_session(d(2028, 12, 29)).unwrap());
        assert!(!is_session(d(2028, 12, 30)).unwrap());
        assert!(matches!(
            next_session_after(d(2028, 12, 29)),
            Err(SessionCalendarV2Error::OutOfCoverage(_))
        ));
        // 2027-2028 are covered here and refused by v1.
        assert!(is_session(d(2027, 1, 4)).unwrap());
        assert!(sessions::is_session(d(2027, 1, 4)).is_err());
    }

    #[test]
    fn boundary_dates_and_year_transitions() {
        assert!(!is_session(d(2016, 1, 1)).unwrap());
        assert_eq!(next_session_after(d(2016, 1, 1)).unwrap(), d(2016, 1, 4));
        // 2026 -> 2027: v1's horizon is crossed.
        assert_eq!(next_session_after(d(2026, 12, 31)).unwrap(), d(2027, 1, 4));
        // 2027-12-31 trades (Saturday New Year 2028-01-01 is not observed on Friday).
        assert!(is_session(d(2027, 12, 31)).unwrap());
        assert_eq!(
            next_session_after(d(2027, 12, 30)).unwrap(),
            d(2027, 12, 31)
        );
        assert_eq!(next_session_after(d(2027, 12, 31)).unwrap(), d(2028, 1, 3));
        assert_eq!(
            next_session_after(d(2027, 12, 23)).unwrap(),
            d(2027, 12, 27),
            "Christmas observed Fri 12-24"
        );
        // Early-close sessions are sessions.
        for early in [
            d(2023, 7, 3),
            d(2024, 11, 29),
            d(2024, 12, 24),
            d(2018, 12, 24),
            d(2027, 11, 26),
        ] {
            assert!(
                is_session(early).unwrap() && is_early_close(early).unwrap(),
                "{early}"
            );
        }
        assert!(!is_early_close(d(2024, 12, 23)).unwrap());
        assert!(!is_early_close(d(2027, 12, 23)).unwrap());
        assert!(is_last_session_of_month(d(2027, 12, 31)).unwrap());
        assert!(is_last_session_of_month(d(2028, 12, 29)).unwrap());
        assert_eq!(session_ordinal_in_month(d(2028, 1, 3)).unwrap(), 1);
    }

    #[test]
    fn annual_session_counts_are_consistent_with_v1_and_plausible_for_2027_2028() {
        let count = |y: i32| {
            every_day(d(y, 1, 1), d(y, 12, 31))
                .into_iter()
                .filter(|x| is_session(*x).unwrap())
                .count()
        };
        for (y, n) in [
            (2016, 252),
            (2020, 253),
            (2023, 250),
            (2024, 252),
            (2025, 250),
        ] {
            assert_eq!(count(y), n, "{y}");
        }
        assert_eq!(count(2027), 251);
        assert_eq!(count(2028), 251);
    }

    #[test]
    fn daily_bar_label_maps_to_its_et_date_and_rejects_everything_else() {
        assert_eq!(
            session_of_daily_bar(et_midnight_ts(d(2027, 3, 1))).unwrap(),
            d(2027, 3, 1)
        );
        assert_eq!(
            session_of_daily_bar(et_midnight_ts(d(2024, 11, 4))).unwrap(),
            d(2024, 11, 4)
        );
        let ts = et_midnight_ts(d(2024, 11, 4));
        for off in [1, 60, 3600, 16 * 3600] {
            assert!(matches!(
                session_of_daily_bar(ts + off),
                Err(SessionCalendarV2Error::NotADailyBarTimestamp(_))
            ));
        }
        for closed in [
            d(2024, 3, 29),
            d(2024, 3, 30),
            d(2025, 1, 9),
            d(2018, 12, 5),
        ] {
            assert!(
                matches!(
                    session_of_daily_bar(et_midnight_ts(closed)),
                    Err(SessionCalendarV2Error::NotASession(_))
                ),
                "{closed}"
            );
        }
        assert!(matches!(
            session_of_daily_bar(et_midnight_ts(d(2029, 2, 1))),
            Err(SessionCalendarV2Error::OutOfCoverage(_))
        ));
        assert!(matches!(
            session_of_daily_bar(et_midnight_ts(d(2015, 12, 31))),
            Err(SessionCalendarV2Error::OutOfCoverage(_))
        ));
    }
}
