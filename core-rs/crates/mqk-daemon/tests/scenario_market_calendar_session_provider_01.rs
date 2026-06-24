//! # SMART-CALENDAR-SESSION-PROVIDER-01 — Market calendar session provider proof
//! # NYSE-CALENDAR-EXTENSION-AND-EXCHANGE-PROVIDER-01 — 2027/2028 static table proof
//! # ASSET-CORE-05A — Multi-asset session-classification seam proof (model-only)
//! # ASSET-CORE-05B — Equity calendar authority audit + session-profile resolution seam
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
//! - The bounded 2023–2028 holiday/early-close table coverage is contract-proved
//!   for every covered year, not just a sample of years (ASSET-CORE-05B Part A)
//! - The additive `MarketVenueSessionKind`/`MarketSessionProfile` seam can
//!   represent equity, crypto, futures, and forex session shapes without
//!   changing equity runtime behavior, and remains fail-closed on `Unknown`
//! - `resolve_session_profile_for_instrument_metadata` resolves equity/ETF to
//!   the real wired profile, resolves crypto/futures/forex to their known
//!   model-only profiles without enabling them, resolves options to an
//!   explicit no-profile placeholder, and fails closed on unknown/blank
//!   asset classes (ASSET-CORE-05B Part B)
//!
//! All tests are pure in-process.  No DB, no network, no daemon startup.
//!
//! | Test     | Claim                                                                        |
//! |----------|------------------------------------------------------------------------------|
//! | MCSP01   | July (EDT) trading day: 13:28 UTC premarket, 13:32 UTC regular, 20:01 closed |
//! | MCSP02   | January (EST) trading day: 14:28 UTC premarket, 14:32 UTC regular, 21:01 AH |
//! | MCSP03   | Saturday returns Closed (SCSP03)                                             |
//! | MCSP04   | Known full-day holiday (2026-07-03) returns Holiday (SCSP04)                 |
//! | MCSP05   | Known early-close day (2024-11-29) closes at 13:00 ET (SCSP05)              |
//! | MCSP06   | FixedWindowOverrideProvider is labeled "fixed_window_override" (SCSP06)      |
//! | MCSP07   | MarketSessionTruth::unknown() fails closed (SCSP07)                          |
//! | MCSP08   | NyseWeekdaysProvider.is_in_session() agrees with CalendarSpec (SCSP08)       |
//! | NCAL01   | 2027 full-day holiday (Jan 1) returns Holiday                                |
//! | NCAL02   | 2027 normal trading day (Jan 4) is regular session during session hours       |
//! | NCAL03   | 2027 day-after-Thanksgiving (Nov 26) closes at 13:00 ET                      |
//! | NCAL04   | 2028 full-day holiday (Dec 25 Christmas) returns Holiday                     |
//! | NCAL05   | 2028 day-after-Thanksgiving (Nov 24) closes at 13:00 ET                      |
//! | ACS05A01 | Equity regular-session state maps onto generic `Regular`/open kind          |
//! | ACS05A02 | Equity outside-window states map onto generic non-open kinds                |
//! | ACS05A03 | `Unknown` remains fail-closed on the generic `MarketVenueSessionKind` model  |
//! | ACS05A04 | Crypto continuous profile is open at any time, any weekday                  |
//! | ACS05A05 | Futures profile distinguishes regular / extended / overnight / closed       |
//! | ACS05A06 | Forex profile distinguishes weekday-open vs weekend-closed                  |
//! | ACS05A07 | Only the equity profile is labeled `implemented_current`; rest `model_only` |
//! | EQCAL01  | Every covered year 2023–2028 has a known full-day holiday classified correctly |
//! | EQCAL02  | Every covered year 2023–2028 has a known early-close date closing at 13:00 ET |
//! | ACS05B01 | Equity (bare) resolves to EquityUsRegular, Active                           |
//! | ACS05B02 | Equity + ETF resolves to EquityUsRegular, Active                            |
//! | ACS05B03 | Crypto resolves to model-only CryptoContinuous, UnsupportedAssetClass        |
//! | ACS05B04 | "future"/"futures" resolve to model-only FuturesGlobex, UnsupportedAssetClass|
//! | ACS05B05 | Forex resolves to model-only ForexWeekdayContinuous, UnsupportedAssetClass   |
//! | ACS05B06 | "option"/"options" resolve to UnsupportedAssetClass with no invented profile |
//! | ACS05B07 | Unrecognized asset_class fails closed to Unknown                            |
//! | ACS05B08 | Blank/whitespace asset_class fails closed to Unknown                        |

use chrono::{TimeZone, Utc};
use mqk_daemon::state::{
    classify_crypto_continuous_session, classify_equity_us_regular_session,
    classify_forex_weekday_continuous_session, classify_futures_globex_session,
    resolve_session_profile_for_instrument_metadata, supported_session_profiles,
    FixedWindowOverrideProvider, FuturesSessionWindows, MarketCalendarProvider,
    MarketSessionProfile, MarketSessionState, MarketSessionTruth, MarketVenueSessionKind,
    NyseWeekdaysProvider, SessionAuthority, SessionProfileResolutionTruth, SessionProfileStatus,
    SessionWindow,
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

// ---------------------------------------------------------------------------
// ACS05A01 — Equity regular-session state maps onto generic Regular/open kind
// ---------------------------------------------------------------------------

/// Today's equity regular-session truth (`MarketSessionState::RegularOpen`,
/// produced by the real `NyseWeekdaysProvider`) must map onto the generic
/// seam as `MarketVenueSessionKind::Regular`, and that kind must be `is_open`.
/// This proves the additive seam can represent current equity behavior
/// without re-deriving it — it only re-labels the existing provider's output.
#[test]
fn acs05a01_equity_regular_session_maps_to_generic_regular_open() {
    let p = provider();
    // July weekday regular session (reuses MCSP01's known-good timestamp).
    let truth = p.session_for(ts(2024, 7, 8, 13, 32, 0));
    assert_eq!(truth.state, MarketSessionState::RegularOpen);

    let kind = truth.state.to_venue_kind();
    assert_eq!(
        kind,
        MarketVenueSessionKind::Regular,
        "ACS05A01: RegularOpen must map to generic Regular"
    );
    assert!(
        kind.is_open(),
        "ACS05A01: generic Regular kind must be is_open"
    );

    // classify_equity_us_regular_session is a thin wrapper; must agree.
    assert_eq!(
        classify_equity_us_regular_session(truth.state),
        MarketVenueSessionKind::Regular,
        "ACS05A01: classify_equity_us_regular_session must agree with to_venue_kind"
    );
}

// ---------------------------------------------------------------------------
// ACS05A02 — Equity outside-window states map onto generic non-open kinds
// ---------------------------------------------------------------------------

/// Every non-`RegularOpen` equity state must map onto a generic kind that is
/// NOT `is_open`, preserving the existing fail-closed posture under the new
/// model. `Holiday` and `Closed` both collapse to generic `Closed`;
/// `EarlyClose` and `AfterHours` both collapse to generic `PostMarket`
/// (the trading day's regular session has already ended in both cases).
#[test]
fn acs05a02_equity_outside_window_states_map_to_non_open_kinds() {
    let cases = [
        (MarketSessionState::PreMarket, MarketVenueSessionKind::PreMarket),
        (MarketSessionState::AfterHours, MarketVenueSessionKind::PostMarket),
        (MarketSessionState::Closed, MarketVenueSessionKind::Closed),
        (MarketSessionState::Holiday, MarketVenueSessionKind::Closed),
        (MarketSessionState::EarlyClose, MarketVenueSessionKind::PostMarket),
    ];
    for (equity_state, expected_kind) in cases {
        let kind = equity_state.to_venue_kind();
        assert_eq!(
            kind, expected_kind,
            "ACS05A02: {equity_state:?} must map to {expected_kind:?}"
        );
        assert!(
            !kind.is_open(),
            "ACS05A02: {equity_state:?} -> {kind:?} must not be is_open"
        );
    }
}

// ---------------------------------------------------------------------------
// ACS05A03 — Unknown remains fail-closed on the generic model
// ---------------------------------------------------------------------------

/// `MarketSessionState::Unknown` must map to generic `Unknown`, and generic
/// `Unknown` must not be `is_open`. Also proves that `is_open()` is `true`
/// for exactly `Regular` and `Continuous` and `false` for every other
/// variant — the fail-closed contract for the new model.
#[test]
fn acs05a03_unknown_remains_fail_closed_on_generic_model() {
    assert_eq!(
        MarketSessionState::Unknown.to_venue_kind(),
        MarketVenueSessionKind::Unknown,
        "ACS05A03: equity Unknown must map to generic Unknown"
    );
    assert!(
        !MarketVenueSessionKind::Unknown.is_open(),
        "ACS05A03: generic Unknown must fail closed (is_open=false)"
    );

    let open_kinds = [MarketVenueSessionKind::Regular, MarketVenueSessionKind::Continuous];
    let closed_kinds = [
        MarketVenueSessionKind::Closed,
        MarketVenueSessionKind::PreMarket,
        MarketVenueSessionKind::PostMarket,
        MarketVenueSessionKind::Extended,
        MarketVenueSessionKind::Overnight,
        MarketVenueSessionKind::Unknown,
    ];
    for kind in open_kinds {
        assert!(kind.is_open(), "ACS05A03: {kind:?} must be is_open");
    }
    for kind in closed_kinds {
        assert!(!kind.is_open(), "ACS05A03: {kind:?} must NOT be is_open");
    }
}

// ---------------------------------------------------------------------------
// ACS05A04 — Crypto continuous profile is open at any time, any weekday
// ---------------------------------------------------------------------------

/// Crypto's 24/7 continuous concept must classify as `Continuous` (and thus
/// `is_open`) at a weekday, a Saturday, a Sunday, midnight UTC, and a
/// far-future timestamp — there is no time input that produces a closed
/// result. Model-only: not backed by any real crypto exchange calendar.
#[test]
fn acs05a04_crypto_continuous_profile_open_at_any_time() {
    let probes = [
        ts(2026, 3, 30, 14, 0, 0),       // Monday, regular-equity-hours-equivalent
        ts(2026, 3, 28, 3, 0, 0),        // Saturday, 03:00 UTC
        ts(2026, 3, 29, 23, 59, 0),      // Sunday, 23:59 UTC
        ts(2026, 7, 3, 0, 0, 0),         // known equity holiday date, midnight UTC
        Utc.with_ymd_and_hms(9999, 12, 31, 23, 59, 59).unwrap(), // far future
    ];
    for probe in probes {
        let kind = classify_crypto_continuous_session(probe);
        assert_eq!(
            kind,
            MarketVenueSessionKind::Continuous,
            "ACS05A04: crypto must classify as Continuous at {probe}"
        );
        assert!(
            kind.is_open(),
            "ACS05A04: crypto Continuous at {probe} must be is_open"
        );
    }
    assert_eq!(
        MarketSessionProfile::CryptoContinuous.implementation_status(),
        "model_only"
    );
}

// ---------------------------------------------------------------------------
// ACS05A05 — Futures distinguishes regular / extended / overnight / closed
// ---------------------------------------------------------------------------

/// Using illustrative (non-authoritative) UTC HH:MM bounds, the futures
/// classifier must produce all four reachable kinds: `Regular` inside the
/// regular window, `Extended` inside the extended window but outside
/// regular (both before and after), `Overnight` outside both windows, and
/// `Closed` on Saturday regardless of time-of-day. No CME calendar is
/// consulted — these bounds are test fixtures, not real product hours.
#[test]
fn acs05a05_futures_distinguishes_regular_extended_overnight_closed() {
    let windows = FuturesSessionWindows {
        regular: SessionWindow::parse("14:30", "21:00").expect("regular window must parse"),
        extended: SessionWindow::parse("12:00", "23:00").expect("extended window must parse"),
    };

    // Weekday inside regular window (Monday 2026-03-30).
    let regular = classify_futures_globex_session(ts(2026, 3, 30, 15, 0, 0), &windows);
    assert_eq!(regular, MarketVenueSessionKind::Regular, "ACS05A05: regular window");
    assert!(regular.is_open(), "ACS05A05: Regular must be is_open");

    // Weekday inside extended window, before regular opens.
    let extended_before =
        classify_futures_globex_session(ts(2026, 3, 30, 12, 30, 0), &windows);
    assert_eq!(
        extended_before,
        MarketVenueSessionKind::Extended,
        "ACS05A05: extended-before-regular window"
    );
    assert!(
        !extended_before.is_open(),
        "ACS05A05: Extended must NOT be is_open"
    );

    // Weekday inside extended window, after regular closes.
    let extended_after = classify_futures_globex_session(ts(2026, 3, 30, 22, 0, 0), &windows);
    assert_eq!(
        extended_after,
        MarketVenueSessionKind::Extended,
        "ACS05A05: extended-after-regular window"
    );

    // Weekday outside both windows entirely (deep overnight).
    let overnight = classify_futures_globex_session(ts(2026, 3, 30, 2, 0, 0), &windows);
    assert_eq!(
        overnight,
        MarketVenueSessionKind::Overnight,
        "ACS05A05: outside both windows must be Overnight"
    );
    assert!(!overnight.is_open(), "ACS05A05: Overnight must NOT be is_open");

    // Saturday: Closed regardless of time-of-day, even during what would
    // otherwise be the regular window.
    let saturday = classify_futures_globex_session(ts(2026, 4, 4, 15, 0, 0), &windows);
    assert_eq!(
        saturday,
        MarketVenueSessionKind::Closed,
        "ACS05A05: Saturday must be Closed regardless of time-of-day"
    );
    assert!(!saturday.is_open(), "ACS05A05: Closed must NOT be is_open");

    assert_eq!(
        MarketSessionProfile::FuturesGlobex.implementation_status(),
        "model_only"
    );
}

// ---------------------------------------------------------------------------
// ACS05A06 — Forex distinguishes weekday-open vs weekend-closed
// ---------------------------------------------------------------------------

/// Forex's 24/5 concept: any weekday timestamp classifies as `Continuous`
/// (and `is_open`); Saturday and Sunday classify as `Closed`. Model-only —
/// does not encode the precise Friday/Sunday-evening ET rollover.
#[test]
fn acs05a06_forex_distinguishes_weekday_open_vs_weekend_closed() {
    let weekdays = [
        ts(2026, 3, 30, 0, 0, 0),  // Monday
        ts(2026, 3, 31, 12, 0, 0), // Tuesday
        ts(2026, 4, 2, 23, 59, 0), // Thursday
        ts(2026, 4, 3, 0, 0, 1),   // Friday, just after midnight UTC
    ];
    for wd in weekdays {
        let kind = classify_forex_weekday_continuous_session(wd);
        assert_eq!(
            kind,
            MarketVenueSessionKind::Continuous,
            "ACS05A06: weekday {wd} must classify as Continuous"
        );
        assert!(kind.is_open(), "ACS05A06: weekday {wd} must be is_open");
    }

    let weekend = [
        ts(2026, 4, 4, 12, 0, 0), // Saturday
        ts(2026, 4, 5, 12, 0, 0), // Sunday
    ];
    for we in weekend {
        let kind = classify_forex_weekday_continuous_session(we);
        assert_eq!(
            kind,
            MarketVenueSessionKind::Closed,
            "ACS05A06: weekend {we} must classify as Closed"
        );
        assert!(!kind.is_open(), "ACS05A06: weekend {we} must NOT be is_open");
    }

    assert_eq!(
        MarketSessionProfile::ForexWeekdayContinuous.implementation_status(),
        "model_only"
    );
}

// ---------------------------------------------------------------------------
// ACS05A07 — Only equity is implemented_current; rest are model_only
// ---------------------------------------------------------------------------

/// Honesty check on the profile metadata itself: exactly one profile
/// (`EquityUsRegular`) claims `implemented_current`; the other three
/// (crypto, futures, forex) must claim `model_only`. This is the seam's own
/// "do not claim full authoritative calendars are complete" promise,
/// expressed as an assertion rather than just a doc comment.
#[test]
fn acs05a07_only_equity_profile_is_implemented_current() {
    assert_eq!(
        MarketSessionProfile::EquityUsRegular.implementation_status(),
        "implemented_current"
    );
    for model_only_profile in [
        MarketSessionProfile::CryptoContinuous,
        MarketSessionProfile::FuturesGlobex,
        MarketSessionProfile::ForexWeekdayContinuous,
    ] {
        assert_eq!(
            model_only_profile.implementation_status(),
            "model_only",
            "ACS05A07: {model_only_profile:?} must be model_only, not implemented_current"
        );
    }
}

// ---------------------------------------------------------------------------
// ACS05A08 — Session authority/status diagnostics are typed and explicit
// ---------------------------------------------------------------------------

/// The read-only session-profile diagnostics use typed authority/status values
/// instead of raw strings at the model boundary. This proves every authority
/// label is explicit and the supported-profile list names exactly the four
/// scaffolded profiles; it does not wire any non-equity profile into trading.
#[test]
fn acs05a08_session_authority_and_supported_profiles_are_explicit() {
    let status = SessionProfileStatus {
        profile: MarketSessionProfile::EquityUsRegular,
        authority: SessionAuthority::Fallback,
        is_open: Some(true),
        reason_code: "test_reason",
        message: "test message",
    };
    assert_eq!(status.profile.as_str(), "equity_us_regular");
    assert_eq!(status.authority.as_str(), "fallback");
    assert_eq!(status.is_open, Some(true));

    assert_eq!(SessionAuthority::Authoritative.as_str(), "authoritative");
    assert_eq!(
        SessionAuthority::ConfiguredOverride.as_str(),
        "configured_override"
    );
    assert_eq!(SessionAuthority::Unavailable.as_str(), "unavailable");

    let profiles: Vec<&'static str> = supported_session_profiles()
        .into_iter()
        .map(MarketSessionProfile::as_str)
        .collect();
    assert_eq!(
        profiles,
        vec![
            "equity_us_regular",
            "crypto_continuous",
            "futures_globex",
            "forex_24x5"
        ],
        "ACS05A08: supported profile diagnostics must remain additive and deterministic"
    );
}

// ---------------------------------------------------------------------------
// EQCAL01 — Bounded coverage contract: every covered year (2023-2028) has at
// least one known full-day holiday that classifies correctly
// ---------------------------------------------------------------------------

/// ASSET-CORE-05B (Part A): contract-proves the bounded coverage claim for
/// the existing NYSE/Nasdaq holiday table consulted by `NyseWeekdaysProvider`
/// (`mqk_integrity::CalendarSpec::NyseWeekdays`). One known full-day-closure
/// holiday per covered year, 2023-2028 inclusive — every date below is
/// already present in the repo's holiday table
/// (`core-rs/crates/mqk-integrity/src/calendar.rs`); none invented here.
/// Holiday full-day closure does not depend on time-of-day, so each date is
/// probed at one representative mid-session UTC hour.
#[test]
fn eqcal01_every_covered_year_has_a_known_holiday() {
    let p = provider();
    // (year, month, day, label) — one full-day-closure holiday per covered year.
    let cases: &[(i32, u32, u32, &str)] = &[
        (2023, 12, 25, "2023 Christmas"),
        (2024, 11, 28, "2024 Thanksgiving"),
        (2025, 7, 4, "2025 Independence Day"),
        (2026, 7, 3, "2026 Independence Day (observed)"),
        (2027, 1, 1, "2027 New Year's Day"),
        (2028, 12, 25, "2028 Christmas"),
    ];
    for (y, m, d, label) in cases {
        let t = p.session_for(ts(*y, *m, *d, 16, 0, 0));
        assert_eq!(
            t.state,
            MarketSessionState::Holiday,
            "EQCAL01: {label} ({y}-{m:02}-{d:02}) must classify as Holiday"
        );
        assert!(!t.is_in_session(), "EQCAL01: {label} must not be in-session");
        assert!(!t.is_trading_day, "EQCAL01: {label} is not a trading day");
    }
}

// ---------------------------------------------------------------------------
// EQCAL02 — Bounded coverage contract: every covered year (2023-2028) has at
// least one known early-close date that closes at the correct ET time
// ---------------------------------------------------------------------------

/// Companion to EQCAL01: one known early-close date per covered year,
/// 2023-2028 inclusive — every date and ET close time below is already
/// present in the repo's early-close table
/// (`core-rs/crates/mqk-integrity/src/calendar.rs`); none invented here.
/// Each case checks one minute before vs. one minute after the 13:00 ET
/// close, in UTC (accounting for EST/EDT per date).
/// (year, month, day, before_close_utc_hm, after_close_utc_hm, label).
type EarlyCloseCoverageCase = (i32, u32, u32, (u32, u32), (u32, u32), &'static str);

#[test]
fn eqcal02_every_covered_year_has_a_known_early_close() {
    let p = provider();
    let cases: &[EarlyCloseCoverageCase] = &[
        (2023, 11, 24, (17, 59), (18, 1), "2023 day-after-Thanksgiving (EST)"),
        (2024, 7, 3, (16, 59), (17, 1), "2024 Independence Day Eve (EDT)"),
        (2025, 12, 24, (17, 59), (18, 1), "2025 Christmas Eve (EST)"),
        (2026, 11, 27, (17, 59), (18, 1), "2026 day-after-Thanksgiving (EST)"),
        (2027, 11, 26, (17, 59), (18, 1), "2027 day-after-Thanksgiving (EST)"),
        (2028, 11, 24, (17, 59), (18, 1), "2028 day-after-Thanksgiving (EST)"),
    ];
    for (y, m, d, (bh, bm), (ah, am), label) in cases {
        let before = p.session_for(ts(*y, *m, *d, *bh, *bm, 0));
        assert_eq!(
            before.state,
            MarketSessionState::RegularOpen,
            "EQCAL02: {label} ({y}-{m:02}-{d:02}) one minute before early close must be RegularOpen"
        );
        assert!(
            before.is_early_close,
            "EQCAL02: {label} must report is_early_close=true before close"
        );

        let after = p.session_for(ts(*y, *m, *d, *ah, *am, 0));
        assert_eq!(
            after.state,
            MarketSessionState::EarlyClose,
            "EQCAL02: {label} one minute after early close must be EarlyClose"
        );
        assert!(
            !after.is_in_session(),
            "EQCAL02: {label} must not be in-session after early close"
        );
        assert!(
            after.is_early_close,
            "EQCAL02: {label} must report is_early_close=true after close"
        );
    }
}

// ---------------------------------------------------------------------------
// ACS05B01 — Equity (bare) resolves to EquityUsRegular, Active
// ---------------------------------------------------------------------------

/// Plain equity metadata (no `instrument_kind`) must resolve to the real,
/// currently-wired equity session profile with `truth_state = Active`.
#[test]
fn acs05b01_equity_bare_resolves_to_equity_us_regular_active() {
    let r = resolve_session_profile_for_instrument_metadata("equity", None);
    assert_eq!(r.truth_state, SessionProfileResolutionTruth::Active);
    assert_eq!(r.profile, Some(MarketSessionProfile::EquityUsRegular));
    assert_eq!(
        r.profile.unwrap().implementation_status(),
        "implemented_current",
        "ACS05B01: equity must resolve to the implemented_current profile"
    );
}

// ---------------------------------------------------------------------------
// ACS05B02 — Equity + ETF resolves to EquityUsRegular, Active
// ---------------------------------------------------------------------------

/// ETF metadata (`asset_class = "equity"`, `instrument_kind = Some("etf")`,
/// matching `mqk-md`'s convention that ETFs are equities with a kind tag,
/// not a distinct asset class) must resolve identically to bare equity.
#[test]
fn acs05b02_equity_etf_resolves_to_equity_us_regular_active() {
    let r = resolve_session_profile_for_instrument_metadata("equity", Some("etf"));
    assert_eq!(r.truth_state, SessionProfileResolutionTruth::Active);
    assert_eq!(r.profile, Some(MarketSessionProfile::EquityUsRegular));
}

// ---------------------------------------------------------------------------
// ACS05B03 — Crypto resolves to a known model-only profile, unsupported
// ---------------------------------------------------------------------------

/// Crypto metadata must resolve to its model-only profile shape
/// (`CryptoContinuous`) but with `truth_state = UnsupportedAssetClass` —
/// known, but not wired/tradable.
#[test]
fn acs05b03_crypto_resolves_to_model_only_known_profile_unsupported() {
    let r = resolve_session_profile_for_instrument_metadata("crypto", None);
    assert_eq!(
        r.truth_state,
        SessionProfileResolutionTruth::UnsupportedAssetClass
    );
    assert_eq!(r.profile, Some(MarketSessionProfile::CryptoContinuous));
    assert_eq!(r.profile.unwrap().implementation_status(), "model_only");
    assert_ne!(r.truth_state, SessionProfileResolutionTruth::Active);
}

// ---------------------------------------------------------------------------
// ACS05B04 — Futures (singular and plural) resolve to a known model-only profile
// ---------------------------------------------------------------------------

/// Both `"future"` and `"futures"` (both spellings appear across the repo's
/// provider/registry layers) must resolve identically to the model-only
/// `FuturesGlobex` profile, unsupported (not wired/tradable).
#[test]
fn acs05b04_futures_singular_and_plural_resolve_to_model_only_known_profile() {
    for asset_class in ["future", "futures"] {
        let r = resolve_session_profile_for_instrument_metadata(asset_class, None);
        assert_eq!(
            r.truth_state,
            SessionProfileResolutionTruth::UnsupportedAssetClass,
            "ACS05B04: {asset_class} must be UnsupportedAssetClass"
        );
        assert_eq!(r.profile, Some(MarketSessionProfile::FuturesGlobex));
        assert_eq!(r.profile.unwrap().implementation_status(), "model_only");
    }
}

// ---------------------------------------------------------------------------
// ACS05B05 — Forex resolves to a known model-only profile, unsupported
// ---------------------------------------------------------------------------

/// Forex metadata must resolve to its model-only profile shape
/// (`ForexWeekdayContinuous`) but with `truth_state = UnsupportedAssetClass`.
#[test]
fn acs05b05_forex_resolves_to_model_only_known_profile_unsupported() {
    let r = resolve_session_profile_for_instrument_metadata("forex", None);
    assert_eq!(
        r.truth_state,
        SessionProfileResolutionTruth::UnsupportedAssetClass
    );
    assert_eq!(r.profile, Some(MarketSessionProfile::ForexWeekdayContinuous));
    assert_eq!(r.profile.unwrap().implementation_status(), "model_only");
}

// ---------------------------------------------------------------------------
// ACS05B06 — Options (singular and plural) resolve to unsupported, no profile
// ---------------------------------------------------------------------------

/// Options have no independent session-profile shape in this seam (see
/// `market_calendar.rs` module docs — they will likely inherit the
/// underlying equity session in a future patch). Both `"option"` and
/// `"options"` must resolve to `UnsupportedAssetClass` with `profile: None`
/// — explicitly not tradable, and no invented options calendar.
#[test]
fn acs05b06_options_singular_and_plural_resolve_to_unsupported_with_no_profile() {
    for asset_class in ["option", "options"] {
        let r = resolve_session_profile_for_instrument_metadata(asset_class, None);
        assert_eq!(
            r.truth_state,
            SessionProfileResolutionTruth::UnsupportedAssetClass,
            "ACS05B06: {asset_class} must be UnsupportedAssetClass"
        );
        assert_eq!(
            r.profile, None,
            "ACS05B06: {asset_class} must not carry an invented session profile"
        );
    }
}

// ---------------------------------------------------------------------------
// ACS05B07 — Unrecognized asset_class fails closed
// ---------------------------------------------------------------------------

/// Asset classes outside the known set — including case mismatches, since
/// matching is exact-case per `mqk-md`'s existing convention — must fail
/// closed to `Unknown` with no profile.
#[test]
fn acs05b07_unknown_asset_class_fails_closed() {
    for asset_class in ["bond", "commodity", "EQUITY", "Crypto", "totally-unrecognized"] {
        let r = resolve_session_profile_for_instrument_metadata(asset_class, None);
        assert_eq!(
            r.truth_state,
            SessionProfileResolutionTruth::Unknown,
            "ACS05B07: {asset_class} must fail closed to Unknown"
        );
        assert_eq!(
            r.profile, None,
            "ACS05B07: {asset_class} must not carry a profile"
        );
    }
}

// ---------------------------------------------------------------------------
// ACS05B08 — Blank / whitespace asset_class fails closed
// ---------------------------------------------------------------------------

/// Empty and whitespace-only `asset_class` values must fail closed to
/// `Unknown` rather than falling through to any default profile.
#[test]
fn acs05b08_blank_or_whitespace_asset_class_fails_closed() {
    for asset_class in ["", "   ", "\t"] {
        let r = resolve_session_profile_for_instrument_metadata(asset_class, None);
        assert_eq!(
            r.truth_state,
            SessionProfileResolutionTruth::Unknown,
            "ACS05B08: blank asset_class must fail closed to Unknown"
        );
        assert_eq!(r.profile, None);
    }
}
