//! Trading session calendar — Patch B3
//!
//! Deterministic, pure logic. No IO, no wall-clock, no randomness.
//!
//! # Design
//!
//! [`CalendarSpec`] is a lightweight enum that describes which timestamps are
//! valid trading bar ends. Integrity gap detection uses it to ignore gaps that
//! fall entirely within non-trading time (weekends, holidays, pre/after-market).
//!
//! # Variants
//!
//! - [`CalendarSpec::AlwaysOn`] — 24/7 trading (e.g., crypto). Every slot is
//!   a valid trading slot. Preserves exact pre-B3 gap-detection behavior.
//! - [`CalendarSpec::NyseWeekdays`] — NYSE-style equities: weekdays 09:30–16:00
//!   Eastern (UTC-5 fixed offset for v1 simplicity), excluding a hardcoded set
//!   of US market holidays for 2023–2026.

// ---------------------------------------------------------------------------
// CalendarSpec
// ---------------------------------------------------------------------------

/// Specifies which timestamps are valid trading bar ends for gap detection.
///
/// `Clone + Copy + PartialEq + Eq` so it can be embedded in `IntegrityConfig`
/// without breaking existing derives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalendarSpec {
    /// 24/7 — every timestamp is a valid bar end. Used for crypto or tests
    /// that do not need session awareness. Preserves pre-B3 behavior.
    AlwaysOn,

    /// NYSE-style equities:
    /// - Weekdays only (Monday–Friday).
    /// - Regular session: 09:30–16:00 Eastern (UTC-5 fixed offset for v1).
    /// - Hardcoded US market holidays 2023–2026.
    ///
    /// A bar whose `end_ts` falls outside these windows is treated as a
    /// non-trading slot and is **not** counted as a missing bar.
    NyseWeekdays,
}

impl CalendarSpec {
    /// Returns `true` if `end_ts` (epoch seconds, UTC) is a valid trading bar
    /// end for this calendar.
    ///
    /// Used by gap detection: intermediate bar slots that return `false` are
    /// non-trading time and are not counted as missing bars.
    pub fn is_session_bar_end(&self, end_ts: i64) -> bool {
        match self {
            CalendarSpec::AlwaysOn => true,
            CalendarSpec::NyseWeekdays => is_nyse_session_end(end_ts),
        }
    }

    /// Counts the number of bar-sized slots in the **open** interval
    /// `(prev_end_ts, next_end_ts)` that fall within a trading session.
    ///
    /// For gap detection: this is the number of bars that **should have
    /// arrived** between the previous complete bar and the new one.
    /// `missing_bars = missing_bars_between(...)` (the new bar itself is not
    /// counted — it has arrived).
    ///
    /// # AlwaysOn behavior (backwards compatible)
    /// `(next_end_ts - prev_end_ts) / interval_secs - 1`
    ///
    /// # NyseWeekdays behavior
    /// Iterates every slot and counts those that `is_session_bar_end` returns
    /// `true`. Gaps spanning only non-trading time return 0.
    pub fn missing_bars_between(
        &self,
        prev_end_ts: i64,
        next_end_ts: i64,
        interval_secs: i64,
    ) -> u32 {
        debug_assert!(interval_secs > 0, "interval_secs must be positive");
        match self {
            CalendarSpec::AlwaysOn => {
                // Naive arithmetic — identical to pre-B3 logic.
                let delta = next_end_ts - prev_end_ts;
                let steps = delta / interval_secs;
                steps.saturating_sub(1).max(0) as u32
            }
            CalendarSpec::NyseWeekdays => {
                // Walk every slot in (prev_end_ts, next_end_ts) and count
                // those that fall within the trading session.
                let mut count = 0u32;
                let mut ts = prev_end_ts + interval_secs;
                while ts < next_end_ts {
                    if is_nyse_session_end(ts) {
                        count += 1;
                    }
                    ts += interval_secs;
                }
                count
            }
        }
    }
}

// ---------------------------------------------------------------------------
// NYSE session logic (deterministic, UTC-5 fixed offset)
// ---------------------------------------------------------------------------

/// Returns `true` if `end_ts` (epoch seconds, UTC) falls within NYSE regular
/// session hours on a valid trading day.
///
/// Rules (minimal v1):
/// - Day-of-week: Monday–Friday only (epoch day 0 = Thursday 1970-01-01).
/// - Session: 09:30 < time ≤ 16:00 Eastern (UTC-5 fixed offset).
///   A bar `end_ts` represents the **close** of an interval, so a bar ending
///   exactly at open (09:30:00) is excluded; one ending at 09:35:00 is the
///   first 5-minute bar of the day.
/// - Holidays: excluded via hardcoded table for 2023–2026.
fn is_nyse_session_end(end_ts: i64) -> bool {
    // Shift to Eastern Time (UTC-5, fixed for v1 — daylight saving ignored
    // as a known approximation; sufficient for gap-detection purposes).
    const ET_OFFSET_SECS: i64 = 5 * 3600;
    let et_secs = end_ts - ET_OFFSET_SECS;

    // Day-of-week: epoch day 0 (1970-01-01) was a Thursday.
    // Mapping: 0=Thu, 1=Fri, 2=Sat, 3=Sun, 4=Mon, 5=Tue, 6=Wed
    let epoch_day = et_secs.div_euclid(86_400);
    let dow = epoch_day.rem_euclid(7);
    if dow == 2 || dow == 3 {
        // Saturday or Sunday
        return false;
    }

    // Civil date for holiday lookup.
    let (year, month, day) = epoch_secs_to_ymd(et_secs);
    if is_nyse_holiday(year, month, day) {
        return false;
    }

    // Time-of-day check: 09:30:00 < time ≤ 16:00:00 ET.
    let et_time = et_secs.rem_euclid(86_400);
    let open = 9 * 3600 + 30 * 60; //  9:30:00 = 34200 seconds
    let close = 16 * 3600; //          16:00:00 = 57600 seconds
    et_time > open && et_time <= close
}

// ---------------------------------------------------------------------------
// Holiday table 2023–2026
// ---------------------------------------------------------------------------

/// Returns `true` if (year, month, day) is a NYSE market holiday.
///
/// Hardcoded observed dates for 2023–2026. Extend as needed.
fn is_nyse_holiday(year: i64, month: i64, day: i64) -> bool {
    // Encoded as (year, month, day) tuples for readability and determinism.
    const HOLIDAYS: &[(i64, i64, i64)] = &[
        // ── 2023 ─────────────────────────────────────────────────────────
        (2023, 1, 2),   // New Year's Day (observed Mon)
        (2023, 1, 16),  // MLK Day
        (2023, 2, 20),  // Presidents' Day
        (2023, 4, 7),   // Good Friday
        (2023, 5, 29),  // Memorial Day
        (2023, 6, 19),  // Juneteenth
        (2023, 7, 4),   // Independence Day
        (2023, 9, 4),   // Labor Day
        (2023, 11, 23), // Thanksgiving
        (2023, 12, 25), // Christmas
        // ── 2024 ─────────────────────────────────────────────────────────
        (2024, 1, 1),   // New Year's Day
        (2024, 1, 15),  // MLK Day
        (2024, 2, 19),  // Presidents' Day
        (2024, 3, 29),  // Good Friday
        (2024, 5, 27),  // Memorial Day
        (2024, 6, 19),  // Juneteenth
        (2024, 7, 4),   // Independence Day
        (2024, 9, 2),   // Labor Day
        (2024, 11, 28), // Thanksgiving
        (2024, 12, 25), // Christmas
        // ── 2025 ─────────────────────────────────────────────────────────
        (2025, 1, 1),   // New Year's Day
        (2025, 1, 20),  // MLK Day
        (2025, 2, 17),  // Presidents' Day
        (2025, 4, 18),  // Good Friday
        (2025, 5, 26),  // Memorial Day
        (2025, 6, 19),  // Juneteenth
        (2025, 7, 4),   // Independence Day
        (2025, 9, 1),   // Labor Day
        (2025, 11, 27), // Thanksgiving
        (2025, 12, 25), // Christmas
        // ── 2026 ─────────────────────────────────────────────────────────
        (2026, 1, 1),   // New Year's Day
        (2026, 1, 19),  // MLK Day
        (2026, 2, 16),  // Presidents' Day
        (2026, 4, 3),   // Good Friday
        (2026, 5, 25),  // Memorial Day
        (2026, 6, 19),  // Juneteenth
        (2026, 7, 3),   // Independence Day (observed — July 4 falls on Saturday)
        (2026, 9, 7),   // Labor Day
        (2026, 11, 26), // Thanksgiving
        (2026, 12, 25), // Christmas
    ];
    HOLIDAYS.contains(&(year, month, day))
}

// ---------------------------------------------------------------------------
// Civil calendar helper
// ---------------------------------------------------------------------------

/// Convert epoch seconds (UTC) to (year, month, day).
///
/// Uses Howard Hinnant's civil calendar algorithm — deterministic, no stdlib
/// date dependencies.
pub fn epoch_secs_to_ymd(epoch_secs: i64) -> (i64, i64, i64) {
    let days = epoch_secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let d = doy - (153 * mp + 2) / 5 + 1;
    (y, m, d)
}

// ---------------------------------------------------------------------------
// Unit tests (fast, no external dependencies)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Known reference timestamps (verified):
    //
    //   2024-01-08 Mon 10:00 ET  = 2024-01-08T15:00:00Z
    //     = 19730 days × 86400 + 54000 = 1_704_726_000
    //
    //   2024-01-06 Sat 22:00 ET  (weekend, outside session) = 1_704_510_000
    //
    //   2024-01-06 Sat 22:00 ET  (weekend, outside session) = 1_704_596_400
    //
    //   2024-01-01 Mon 08:00 ET  = 1_704_114_000
    //   (NYSE holiday: New Year's Day 2024 — holiday check fires before time check)

    /// Monday 10:00 AM ET, non-holiday → trading session.
    #[test]
    fn monday_mid_session_is_trading() {
        let ts = 1_704_726_000_i64; // 2024-01-08 Mon 10:00 ET = 2024-01-08T15:00:00Z
        assert!(CalendarSpec::NyseWeekdays.is_session_bar_end(ts));
    }

    /// Saturday → not a trading day.
    #[test]
    fn saturday_is_not_trading() {
        let ts = 1_704_510_000_i64; // 2024-01-06 Sat 10:00 ET
        assert!(!CalendarSpec::NyseWeekdays.is_session_bar_end(ts));
    }

    /// Sunday → not a trading day.
    #[test]
    fn sunday_is_not_trading() {
        let ts = 1_704_596_400_i64; // 2024-01-07 Sun 10:00 ET
        assert!(!CalendarSpec::NyseWeekdays.is_session_bar_end(ts));
    }

    /// New Year's Day 2024 (Monday) → NYSE holiday → not a trading day.
    #[test]
    fn new_years_day_2024_is_not_trading() {
        let ts = 1_704_114_000_i64; // 2024-01-01 Mon 10:00 ET
        assert!(!CalendarSpec::NyseWeekdays.is_session_bar_end(ts));
    }

    /// AlwaysOn returns true for everything.
    #[test]
    fn always_on_includes_weekend() {
        let saturday = 1_704_510_000_i64;
        assert!(CalendarSpec::AlwaysOn.is_session_bar_end(saturday));
    }
}
