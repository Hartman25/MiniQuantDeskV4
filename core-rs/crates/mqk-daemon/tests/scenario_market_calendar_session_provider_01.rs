//! # SMART-CALENDAR-SESSION-PROVIDER-01 — Market calendar session provider proof
//! # NYSE-CALENDAR-EXTENSION-AND-EXCHANGE-PROVIDER-01 — 2027/2028 static table proof
//!
//! Proves that:
//! - DST is handled correctly (EDT vs EST → 13:30 vs 14:30 UTC open)
//! - Weekends are closed
//! - Known full-day holidays are closed
//! - Known early-close days close early
//! - Fixed env-window override is available and labeled as override source
//! - Unknown/unavailable provider state fails closed
//! - `NyseWeekdaysProvider` and `CalendarSpec::NyseWeekdays.classify_market_session`
//!   agree on `is_in_session()` (SCSP08 consistency)
//! - Extended 2027–2028 holiday and early-close dates are correctly classified
//!
//! All tests are pure in-process.  No DB, no network, no daemon startup.
//!
//! | Test    | Claim                                                                        |
//! |---------|------------------------------------------------------------------------------|
//! | MCSP01  | July (EDT) trading day: 13:28 UTC premarket, 13:32 UTC regular, 20:01 closed |
//! | MCSP02  | January (EST) trading day: 14:28 UTC premarket, 14:32 UTC regular, 21:01 AH |
//! | MCSP03  | Saturday returns Closed (SCSP03)                                             |
//! | MCSP04  | Known full-day holiday (2026-07-03) returns Holiday (SCSP04)                 |
//! | MCSP05  | Known early-close day (2024-11-29) closes at 13:00 ET (SCSP05)              |
//! | MCSP06  | FixedWindowOverrideProvider is labeled "fixed_window_override" (SCSP06)      |
//! | MCSP07  | MarketSessionTruth::unknown() fails closed (SCSP07)                          |
//! | MCSP08  | NyseWeekdaysProvider.is_in_session() agrees with CalendarSpec (SCSP08)       |
//! | NCAL01  | 2027 full-day holiday (Jan 1) returns Holiday                                |
//! | NCAL02  | 2027 normal trading day (Jan 4) is regular session during session hours       |
//! | NCAL03  | 2027 day-after-Thanksgiving (Nov 26) closes at 13:00 ET                      |
//! | NCAL04  | 2028 full-day holiday (Dec 25 Christmas) returns Holiday                     |
//! | NCAL05  | 2028 day-after-Thanksgiving (Nov 24) closes at 13:00 ET                      |

use chrono::{TimeZone, Utc};
use mqk_daemon::state::{
    FixedWindowOverrideProvider, MarketCalendarProvider, MarketSessionState, MarketSessionTruth,
    NyseWeekdaysProvider, SessionWindow,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn provider() -> NyseWeekdaysProvider {
    NyseWeekdaysProvider
}

/// Build a UTC timestamp from (year, month, day, hour, min, sec).
fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
}

// ---------------------------------------------------------------------------
// MCSP01 — July (EDT) trading day: DST-correct open at 13:30 UTC
// ---------------------------------------------------------------------------

/// NYSE opens at 09:30 ET.  In July (EDT = UTC-4): open = 13:30 UTC.
/// 13:28 UTC = 09:28 ET → premarket.
/// 13:32 UTC = 09:32 ET → regular session.
/// 20:01 UTC = 16:01 ET → after hours.
#[test]
fn mcsp01_july_dst_session_boundaries() {
    // 2024-07-08 (Monday, non-holiday, DST)
    let before_open = ts(2024, 7, 8, 13, 28, 0);
    let during = ts(2024, 7, 8, 13, 32, 0);
    let after_close = ts(2024, 7, 8, 20, 1, 0);

    let p = provider();

    let t_before = p.session_for(before_open);
    assert_eq!(
        t_before.state,
        MarketSessionState::PreMarket,
        "MCSP01: 13:28 UTC on a July weekday must be PreMarket (09:28 EDT = before NYSE open)"
    );
    assert!(
        !t_before.is_in_session(),
        "MCSP01: PreMarket must not be in-session"
    );
    assert_eq!(t_before.source, "nyse_weekdays_heuristic");

    let t_during = p.session_for(during);
    assert_eq!(
        t_during.state,
        MarketSessionState::RegularOpen,
        "MCSP01: 13:32 UTC on a July weekday must be RegularOpen (09:32 EDT = within NYSE session)"
    );
    assert!(
        t_during.is_in_session(),
        "MCSP01: RegularOpen must be in-session"
    );

    let t_after = p.session_for(after_close);
    assert_eq!(
        t_after.state,
        MarketSessionState::AfterHours,
        "MCSP01: 20:01 UTC on a July weekday must be AfterHours (16:01 EDT = after close)"
    );
    assert!(
        !t_after.is_in_session(),
        "MCSP01: AfterHours must not be in-session"
    );
}

// ---------------------------------------------------------------------------
// MCSP02 — January (EST) trading day: DST-correct open at 14:30 UTC
// ---------------------------------------------------------------------------

/// NYSE opens at 09:30 ET.  In January (EST = UTC-5): open = 14:30 UTC.
/// 14:28 UTC = 09:28 ET → premarket.
/// 14:32 UTC = 09:32 ET → regular session.
/// 21:01 UTC = 16:01 ET → after hours.
#[test]
fn mcsp02_january_est_session_boundaries() {
    // 2025-01-06 (Monday, non-holiday, EST)
    let before_open = ts(2025, 1, 6, 14, 28, 0);
    let during = ts(2025, 1, 6, 14, 32, 0);
    let after_close = ts(2025, 1, 6, 21, 1, 0);

    let p = provider();

    let t_before = p.session_for(before_open);
    assert_eq!(
        t_before.state,
        MarketSessionState::PreMarket,
        "MCSP02: 14:28 UTC in January must be PreMarket (09:28 EST = before NYSE open)"
    );
    assert!(
        !t_before.is_in_session(),
        "MCSP02: PreMarket must not be in-session"
    );

    let t_during = p.session_for(during);
    assert_eq!(
        t_during.state,
        MarketSessionState::RegularOpen,
        "MCSP02: 14:32 UTC in January must be RegularOpen (09:32 EST = within NYSE session)"
    );
    assert!(
        t_during.is_in_session(),
        "MCSP02: RegularOpen must be in-session"
    );

    let t_after = p.session_for(after_close);
    assert_eq!(
        t_after.state,
        MarketSessionState::AfterHours,
        "MCSP02: 21:01 UTC in January must be AfterHours (16:01 EST = after close)"
    );
    assert!(
        !t_after.is_in_session(),
        "MCSP02: AfterHours must not be in-session"
    );
}

// ---------------------------------------------------------------------------
// MCSP03 — Weekend: Saturday and Sunday return Closed (SCSP03)
// ---------------------------------------------------------------------------

#[test]
fn mcsp03_weekend_returns_closed() {
    let p = provider();

    // Saturday 2026-03-28 15:00 UTC
    let sat = ts(2026, 3, 28, 15, 0, 0);
    let t_sat = p.session_for(sat);
    assert_eq!(
        t_sat.state,
        MarketSessionState::Closed,
        "MCSP03: Saturday must return Closed"
    );
    assert!(
        !t_sat.is_in_session(),
        "MCSP03: Saturday must not be in-session"
    );
    assert!(
        !t_sat.is_trading_day,
        "MCSP03: Saturday is not a trading day"
    );

    // Sunday 2026-03-29 14:00 UTC
    let sun = ts(2026, 3, 29, 14, 0, 0);
    let t_sun = p.session_for(sun);
    assert_eq!(
        t_sun.state,
        MarketSessionState::Closed,
        "MCSP03: Sunday must return Closed"
    );
    assert!(
        !t_sun.is_in_session(),
        "MCSP03: Sunday must not be in-session"
    );
}

// ---------------------------------------------------------------------------
// MCSP04 — Known full-day holiday: 2026-07-03 (Independence Day observed)
// ---------------------------------------------------------------------------

/// 2026-07-04 falls on Saturday; NYSE observes the holiday on Friday 2026-07-03.
/// Any time on that Friday must return Holiday with is_in_session=false.
#[test]
fn mcsp04_known_holiday_returns_holiday() {
    let p = provider();

    // 2026-07-03 14:00 UTC = 10:00 EDT — would be "regular" if not a holiday
    let t = p.session_for(ts(2026, 7, 3, 14, 0, 0));
    assert_eq!(
        t.state,
        MarketSessionState::Holiday,
        "MCSP04: 2026-07-03 (observed Independence Day) must return Holiday"
    );
    assert!(!t.is_in_session(), "MCSP04: Holiday must not be in-session");
    assert!(!t.is_trading_day, "MCSP04: Holiday is not a trading day");
}

// ---------------------------------------------------------------------------
// MCSP05 — Known early-close day: 2024-11-29 (Black Friday)
// ---------------------------------------------------------------------------

/// Black Friday 2024 (day after Thanksgiving): NYSE closes at 13:00 ET.
/// November is EST (UTC-5): 13:00 ET = 18:00 UTC.
///
/// Before early close (17:59 UTC = 12:59 ET) → RegularOpen.
/// After early close (18:01 UTC = 13:01 ET)  → EarlyClose.
#[test]
fn mcsp05_early_close_day_closes_at_13_et() {
    let p = provider();

    // 17:59 UTC = 12:59 EST = 1 minute before early close → still regular
    let before_ec = ts(2024, 11, 29, 17, 59, 0);
    let t_before = p.session_for(before_ec);
    assert_eq!(
        t_before.state,
        MarketSessionState::RegularOpen,
        "MCSP05: 17:59 UTC on Black Friday 2024 must be RegularOpen (12:59 EST, before early close)"
    );
    assert!(
        t_before.is_in_session(),
        "MCSP05: must be in-session before early close"
    );
    assert!(
        t_before.is_early_close,
        "MCSP05: is_early_close must be true on early-close day"
    );

    // 18:01 UTC = 13:01 EST = 1 minute after early close → EarlyClose
    let after_ec = ts(2024, 11, 29, 18, 1, 0);
    let t_after = p.session_for(after_ec);
    assert_eq!(
        t_after.state,
        MarketSessionState::EarlyClose,
        "MCSP05: 18:01 UTC on Black Friday 2024 must be EarlyClose (13:01 EST, after early close)"
    );
    assert!(
        !t_after.is_in_session(),
        "MCSP05: must not be in-session after early close"
    );
    assert!(
        t_after.is_early_close,
        "MCSP05: is_early_close must be true after early close"
    );
    assert!(
        t_after.session_close_note.is_some(),
        "MCSP05: session_close_note must be present on EarlyClose"
    );
}

// ---------------------------------------------------------------------------
// MCSP06 — FixedWindowOverrideProvider labeled "fixed_window_override" (SCSP06)
// ---------------------------------------------------------------------------

/// When the env-var fixed-UTC-window override is configured, the provider
/// source must be "fixed_window_override" — not "env" or "default".
/// This lets operators distinguish a configured override from the authoritative
/// exchange-calendar path.
#[test]
fn mcsp06_fixed_window_override_labeled_correctly() {
    let window = SessionWindow::parse("13:30", "20:00").expect("MCSP06: test window must parse");
    let p = FixedWindowOverrideProvider { window };

    // Inside window: 2026-03-30 (Monday) 14:00 UTC = inside 13:30–20:00
    let inside = ts(2026, 3, 30, 14, 0, 0);
    let t_in = p.session_for(inside);
    assert_eq!(
        t_in.source, "fixed_window_override",
        "MCSP06: FixedWindowOverrideProvider source must be 'fixed_window_override'"
    );
    assert_eq!(t_in.state, MarketSessionState::RegularOpen);
    assert!(
        t_in.is_in_session(),
        "MCSP06: must be in-session inside the override window"
    );

    // Outside window: 22:00 UTC = after 20:00 stop
    let outside = ts(2026, 3, 30, 22, 0, 0);
    let t_out = p.session_for(outside);
    assert_eq!(
        t_out.source, "fixed_window_override",
        "MCSP06: source must be 'fixed_window_override' outside window too"
    );
    assert_eq!(t_out.state, MarketSessionState::Closed);
    assert!(
        !t_out.is_in_session(),
        "MCSP06: must not be in-session outside override window"
    );
}

// ---------------------------------------------------------------------------
// MCSP07 — Unknown/unavailable provider state fails closed (SCSP07)
// ---------------------------------------------------------------------------

/// `MarketSessionTruth::unknown()` must return `is_in_session() = false`.
/// Any provider that cannot determine truth must surface `Unknown` and it
/// must not allow session-open to propagate.
#[test]
fn mcsp07_unknown_truth_fails_closed() {
    let unknown = MarketSessionTruth::unknown();
    assert_eq!(
        unknown.state,
        MarketSessionState::Unknown,
        "MCSP07: state must be Unknown"
    );
    assert!(
        !unknown.is_in_session(),
        "MCSP07: Unknown must fail closed (is_in_session=false)"
    );
    assert!(
        !unknown.is_trading_day,
        "MCSP07: Unknown must not claim trading_day=true"
    );

    // Also verify all non-RegularOpen variants are fail-closed.
    let non_trading = [
        MarketSessionState::PreMarket,
        MarketSessionState::AfterHours,
        MarketSessionState::Closed,
        MarketSessionState::Holiday,
        MarketSessionState::EarlyClose,
        MarketSessionState::Unknown,
    ];
    for state in non_trading {
        assert!(
            !state.is_in_session(),
            "MCSP07: {:?} must not be in-session (fail-closed)",
            state
        );
    }

    // Only RegularOpen is in-session.
    assert!(
        MarketSessionState::RegularOpen.is_in_session(),
        "MCSP07: RegularOpen must be in-session"
    );

    // Far-future valid timestamp: provider must not panic and must fail closed.
    // Year 9999-12-31 23:59:59 UTC is beyond all NYSE calendar tables (2023–2028)
    // and at 23:59 ET is firmly after-hours → is_in_session must be false.
    let far_future = Utc.with_ymd_and_hms(9999, 12, 31, 23, 59, 59).unwrap();
    let t = NyseWeekdaysProvider.session_for(far_future);
    assert!(
        !t.is_in_session(),
        "MCSP07: far-future timestamp (year 9999) must not be in-session"
    );
}

// ---------------------------------------------------------------------------
// MCSP08 — NyseWeekdaysProvider agrees with CalendarSpec (SCSP08 consistency)
// ---------------------------------------------------------------------------

/// The `NyseWeekdaysProvider` must agree with
/// `CalendarSpec::NyseWeekdays.classify_market_session(ts) == "regular"` on
/// the is_in_session decision.  Both derive from the same underlying logic;
/// this test proves they never diverge for a representative set of timestamps.
#[test]
fn mcsp08_provider_and_calendar_spec_agree_on_session_truth() {
    use mqk_integrity::CalendarSpec;

    let cases: &[(chrono::DateTime<Utc>, &str)] = &[
        // July weekday regular session (EDT, UTC-4)
        (ts(2024, 7, 8, 14, 0, 0), "regular"),
        // July weekday premarket
        (ts(2024, 7, 8, 13, 0, 0), "premarket"),
        // July weekday after-hours
        (ts(2024, 7, 8, 20, 30, 0), "after_hours"),
        // January weekday regular session (EST, UTC-5)
        (ts(2025, 1, 6, 15, 0, 0), "regular"),
        // January weekday premarket
        (ts(2025, 1, 6, 14, 0, 0), "premarket"),
        // Saturday
        (ts(2026, 3, 28, 15, 0, 0), "closed"),
        // Full-day holiday
        (ts(2026, 7, 3, 14, 0, 0), "closed"),
        // Early-close day before close
        (ts(2024, 11, 29, 17, 59, 0), "regular"),
        // Early-close day after close (modified nyse_classify_session → "after_hours")
        (ts(2024, 11, 29, 18, 1, 0), "after_hours"),
    ];

    let p = NyseWeekdaysProvider;

    for (dt, expected_classify) in cases {
        let ts_val = dt.timestamp();
        let classify_result = CalendarSpec::NyseWeekdays.classify_market_session(ts_val);
        let provider_truth = p.session_for(*dt);

        assert_eq!(
            classify_result, *expected_classify,
            "MCSP08: CalendarSpec classify_market_session mismatch at {dt}: \
             expected {expected_classify}, got {classify_result}"
        );

        // The provider's is_in_session must match "regular" from CalendarSpec.
        let spec_in_session = classify_result == "regular";
        let provider_in_session = provider_truth.is_in_session();
        assert_eq!(
            spec_in_session, provider_in_session,
            "MCSP08: provider.is_in_session() disagrees with CalendarSpec at {dt}: \
             spec={spec_in_session}, provider={provider_in_session}"
        );
    }
}

// ---------------------------------------------------------------------------
// NCAL01 — 2027 full-day holiday: New Year's Day Jan 1
// ---------------------------------------------------------------------------

/// 2027-01-01 is a Friday (New Year's Day).  Any time on that day must return
/// Holiday with is_in_session=false, regardless of time-of-day.
/// Proves the extended 2027 holiday table entry (NYSE-CALENDAR-EXTENSION-01).
#[test]
fn ncal01_2027_new_years_day_returns_holiday() {
    let p = provider();

    // 14:00 UTC = 09:00 EST — would be premarket if not a holiday
    let mid_morning = ts(2027, 1, 1, 14, 0, 0);
    let t = p.session_for(mid_morning);
    assert_eq!(
        t.state,
        MarketSessionState::Holiday,
        "NCAL01: 2027-01-01 (New Year's Day) must return Holiday"
    );
    assert!(!t.is_in_session(), "NCAL01: Holiday must not be in-session");
    assert!(!t.is_trading_day, "NCAL01: Holiday is not a trading day");
}

// ---------------------------------------------------------------------------
// NCAL02 — 2027 normal trading day in regular session hours
// ---------------------------------------------------------------------------

/// 2027-01-04 (Monday, not a holiday, EST) at 15:00 UTC = 10:00 EST → in session.
/// Proves that the 2027 calendar table does not accidentally block normal trading.
#[test]
fn ncal02_2027_normal_trading_day_is_regular_session() {
    let p = provider();

    // 15:00 UTC = 10:00 EST (EST = UTC-5, early January, no DST)
    let in_session = ts(2027, 1, 4, 15, 0, 0);
    let t = p.session_for(in_session);
    assert_eq!(
        t.state,
        MarketSessionState::RegularOpen,
        "NCAL02: 2027-01-04 Mon 10:00 EST must be RegularOpen"
    );
    assert!(t.is_in_session(), "NCAL02: must be in-session");
    assert!(t.is_trading_day, "NCAL02: must be a trading day");
}

// ---------------------------------------------------------------------------
// NCAL03 — 2027 day-after-Thanksgiving (Nov 26) early close at 13:00 ET
// ---------------------------------------------------------------------------

/// 2027-11-26 (Friday, day after Thanksgiving Nov 25): NYSE closes at 13:00 EST.
/// November is EST (UTC-5): 13:00 ET = 18:00 UTC.
///
/// 17:59 UTC = 12:59 EST → RegularOpen, is_early_close=true.
/// 18:01 UTC = 13:01 EST → EarlyClose.
#[test]
fn ncal03_2027_black_friday_early_close_at_13_et() {
    let p = provider();

    let before_ec = ts(2027, 11, 26, 17, 59, 0); // 12:59 EST
    let t_before = p.session_for(before_ec);
    assert_eq!(
        t_before.state,
        MarketSessionState::RegularOpen,
        "NCAL03: 17:59 UTC on 2027-11-26 must be RegularOpen (12:59 EST, before early close)"
    );
    assert!(
        t_before.is_early_close,
        "NCAL03: is_early_close must be true on early-close day"
    );

    let after_ec = ts(2027, 11, 26, 18, 1, 0); // 13:01 EST
    let t_after = p.session_for(after_ec);
    assert_eq!(
        t_after.state,
        MarketSessionState::EarlyClose,
        "NCAL03: 18:01 UTC on 2027-11-26 must be EarlyClose (13:01 EST, after early close)"
    );
    assert!(
        !t_after.is_in_session(),
        "NCAL03: must not be in-session after early close"
    );
    assert!(
        t_after.is_early_close,
        "NCAL03: is_early_close must be true after early close"
    );
}

// ---------------------------------------------------------------------------
// NCAL04 — 2028 full-day holiday: Christmas Dec 25 (Monday)
// ---------------------------------------------------------------------------

/// 2028-12-25 is a Monday (Christmas).  Any time on that day must return
/// Holiday with is_in_session=false.
/// Proves the extended 2028 holiday table entry.
#[test]
fn ncal04_2028_christmas_day_returns_holiday() {
    let p = provider();

    // 15:00 UTC = 10:00 EST — would be regular session on a normal day
    let mid_session = ts(2028, 12, 25, 15, 0, 0);
    let t = p.session_for(mid_session);
    assert_eq!(
        t.state,
        MarketSessionState::Holiday,
        "NCAL04: 2028-12-25 (Christmas, Monday) must return Holiday"
    );
    assert!(!t.is_in_session(), "NCAL04: Holiday must not be in-session");
    assert!(!t.is_trading_day, "NCAL04: Holiday is not a trading day");
}

// ---------------------------------------------------------------------------
// NCAL05 — 2028 day-after-Thanksgiving (Nov 24) early close at 13:00 ET
// ---------------------------------------------------------------------------

/// 2028-11-24 (Friday, day after Thanksgiving Nov 23): NYSE closes at 13:00 EST.
/// November is EST (UTC-5): 13:00 ET = 18:00 UTC.
///
/// 17:59 UTC = 12:59 EST → RegularOpen, is_early_close=true.
/// 18:01 UTC = 13:01 EST → EarlyClose.
#[test]
fn ncal05_2028_black_friday_early_close_at_13_et() {
    let p = provider();

    let before_ec = ts(2028, 11, 24, 17, 59, 0); // 12:59 EST
    let t_before = p.session_for(before_ec);
    assert_eq!(
        t_before.state,
        MarketSessionState::RegularOpen,
        "NCAL05: 17:59 UTC on 2028-11-24 must be RegularOpen (12:59 EST, before early close)"
    );
    assert!(
        t_before.is_early_close,
        "NCAL05: is_early_close must be true on early-close day"
    );

    let after_ec = ts(2028, 11, 24, 18, 1, 0); // 13:01 EST
    let t_after = p.session_for(after_ec);
    assert_eq!(
        t_after.state,
        MarketSessionState::EarlyClose,
        "NCAL05: 18:01 UTC on 2028-11-24 must be EarlyClose (13:01 EST, after early close)"
    );
    assert!(
        !t_after.is_in_session(),
        "NCAL05: must not be in-session after early close"
    );
    assert!(
        t_after.is_early_close,
        "NCAL05: is_early_close must be true after early close"
    );
}
