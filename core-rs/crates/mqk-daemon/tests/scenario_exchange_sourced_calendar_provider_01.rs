//! NYSE-CALENDAR-EXTENSION-AND-EXCHANGE-PROVIDER-01 — Exchange-sourced provider seam proof.
//!
//! Proves that:
//! - A fixture-backed [`ExchangeSourcedCalendarProvider`] correctly classifies
//!   regular open, holiday, and early-close days using injected per-day data.
//! - Unavailable, stale, and invalid source states fail closed (`Unknown`).
//! - The explicit fallback-to-static policy is correctly labeled.
//! - The static `NyseWeekdaysProvider` remains the safe default.
//! - No network calls are made — all data is fixture-injected.
//!
//! All tests are pure in-process.  No DB, no network, no daemon startup.
//!
//! | Test  | Claim                                                                          |
//! |-------|--------------------------------------------------------------------------------|
//! | XCP01 | Fixture provider returns RegularOpen for a normal trading day                  |
//! | XCP02 | Fixture provider returns Holiday for a full-close holiday entry                |
//! | XCP03 | Fixture provider returns EarlyClose after 13:00 ET on an early-close day       |
//! | XCP04 | Unavailable source state returns Unknown (fail-closed)                         |
//! | XCP05 | Stale source state returns Unknown (fail-closed)                               |
//! | XCP06 | Invalid source state returns Unknown (fail-closed)                             |
//! | XCP07 | FallbackToStatic policy delegates to heuristic and labels source honestly      |
//! | XCP08 | No network calls in any test (structural guarantee — fixture-only)             |

use std::collections::HashMap;

use chrono::{TimeZone, Utc};
use mqk_daemon::state::{
    ExchangeCalendarDay, ExchangeCalendarMeta, ExchangeDayStatus, ExchangeSourceState,
    ExchangeSourcedCalendarProvider, ExchangeUnavailablePolicy, MarketCalendarProvider,
    MarketSessionState,
};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Build a UTC timestamp from (year, month, day, hour, min, sec).
fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
}

/// Fixture metadata covering 2027-01-01 to 2027-12-31.
fn fixture_meta() -> ExchangeCalendarMeta {
    ExchangeCalendarMeta {
        source_name: "fixture_nyse_2027",
        exchange: "NYSE",
        generated_at: Some("2026-05-27T00:00:00Z"),
        coverage_start: (2027, 1, 1),
        coverage_end: (2027, 12, 31),
    }
}

/// Fixture days map containing:
/// - 2027-01-01: Holiday (New Year's Day)
/// - 2027-11-26: EarlyClose at 13:00 ET (day after Thanksgiving)
/// - 2027-12-24: Holiday (Christmas observed, full close)
///
/// All other weekdays within 2027 are treated as normal open days (not in map).
fn fixture_days() -> HashMap<(i64, i64, i64), ExchangeCalendarDay> {
    let mut days = HashMap::new();
    days.insert(
        (2027, 1, 1),
        ExchangeCalendarDay {
            date: (2027, 1, 1),
            status: ExchangeDayStatus::Holiday,
            early_close_et: None,
        },
    );
    days.insert(
        (2027, 11, 26),
        ExchangeCalendarDay {
            date: (2027, 11, 26),
            status: ExchangeDayStatus::EarlyClose,
            early_close_et: Some((13, 0)),
        },
    );
    days.insert(
        (2027, 12, 24),
        ExchangeCalendarDay {
            date: (2027, 12, 24),
            status: ExchangeDayStatus::Holiday,
            early_close_et: None,
        },
    );
    days
}

/// Build an active fixture provider covering 2027.
fn active_provider() -> ExchangeSourcedCalendarProvider {
    ExchangeSourcedCalendarProvider {
        meta: fixture_meta(),
        source_state: ExchangeSourceState::Active,
        days: fixture_days(),
        on_unavailable: ExchangeUnavailablePolicy::FailClosed,
    }
}

/// Build a provider with the given source state (fail-closed on unavailable).
fn provider_with_state(state: ExchangeSourceState) -> ExchangeSourcedCalendarProvider {
    ExchangeSourcedCalendarProvider {
        meta: fixture_meta(),
        source_state: state,
        days: fixture_days(),
        on_unavailable: ExchangeUnavailablePolicy::FailClosed,
    }
}

// ---------------------------------------------------------------------------
// XCP01 — Fixture provider returns RegularOpen for a normal trading day
// ---------------------------------------------------------------------------

/// 2027-01-04 is a Monday (not a holiday in the fixture).
/// 15:00 UTC = 10:00 EST → within the regular session window → RegularOpen.
/// Source label must be the fixture source name, not the static heuristic label.
#[test]
fn xcp01_fixture_returns_regular_open_for_normal_day() {
    let p = active_provider();

    // 15:00 UTC = 10:00 EST (January, UTC-5)
    let t = p.session_for(ts(2027, 1, 4, 15, 0, 0));
    assert_eq!(
        t.state,
        MarketSessionState::RegularOpen,
        "XCP01: 2027-01-04 Mon 10:00 EST must be RegularOpen via fixture provider"
    );
    assert!(t.is_in_session(), "XCP01: must be in-session");
    assert!(t.is_trading_day, "XCP01: must be a trading day");
    assert_eq!(
        t.source, "fixture_nyse_2027",
        "XCP01: source must be the fixture source name, not the static heuristic"
    );
    assert_eq!(t.exchange, "NYSE", "XCP01: exchange must be NYSE");
    assert!(
        !t.is_early_close,
        "XCP01: normal day must not be early-close"
    );
}

// ---------------------------------------------------------------------------
// XCP02 — Fixture provider returns Holiday for a full-close holiday entry
// ---------------------------------------------------------------------------

/// 2027-01-01 is in the fixture as Holiday (New Year's Day).
/// Any time on that day must return Holiday regardless of time-of-day.
#[test]
fn xcp02_fixture_returns_holiday_for_full_close_entry() {
    let p = active_provider();

    // 14:00 UTC = 09:00 EST — would be premarket if not a holiday
    let t = p.session_for(ts(2027, 1, 1, 14, 0, 0));
    assert_eq!(
        t.state,
        MarketSessionState::Holiday,
        "XCP02: 2027-01-01 must return Holiday (New Year's Day in fixture)"
    );
    assert!(!t.is_in_session(), "XCP02: Holiday must not be in-session");
    assert!(!t.is_trading_day, "XCP02: Holiday is not a trading day");
    assert_eq!(
        t.source, "fixture_nyse_2027",
        "XCP02: source must be fixture"
    );

    // Also check mid-session time on the same holiday
    let t2 = p.session_for(ts(2027, 1, 1, 16, 0, 0));
    assert_eq!(
        t2.state,
        MarketSessionState::Holiday,
        "XCP02: holiday must be full-day — 11:00 EST also returns Holiday"
    );
}

// ---------------------------------------------------------------------------
// XCP03 — Fixture provider returns EarlyClose after 13:00 ET on early-close day
// ---------------------------------------------------------------------------

/// 2027-11-26 (day after Thanksgiving) is in the fixture as EarlyClose at 13:00 ET.
/// November = EST (UTC-5): 13:00 ET = 18:00 UTC.
///
/// Before early close: 17:59 UTC = 12:59 EST → RegularOpen with is_early_close=true.
/// After early close:  18:01 UTC = 13:01 EST → EarlyClose.
#[test]
fn xcp03_fixture_returns_early_close_with_correct_close_time() {
    let p = active_provider();

    let before_ec = ts(2027, 11, 26, 17, 59, 0); // 12:59 EST
    let t_before = p.session_for(before_ec);
    assert_eq!(
        t_before.state,
        MarketSessionState::RegularOpen,
        "XCP03: 17:59 UTC on 2027-11-26 must be RegularOpen (12:59 EST, before early close)"
    );
    assert!(
        t_before.is_in_session(),
        "XCP03: must be in-session before early close"
    );
    assert!(
        t_before.is_early_close,
        "XCP03: is_early_close must be true on early-close day"
    );
    assert_eq!(t_before.source, "fixture_nyse_2027", "XCP03: source label");

    let after_ec = ts(2027, 11, 26, 18, 1, 0); // 13:01 EST
    let t_after = p.session_for(after_ec);
    assert_eq!(
        t_after.state,
        MarketSessionState::EarlyClose,
        "XCP03: 18:01 UTC on 2027-11-26 must be EarlyClose (13:01 EST, after early close)"
    );
    assert!(
        !t_after.is_in_session(),
        "XCP03: must not be in-session after early close"
    );
    assert!(
        t_after.is_early_close,
        "XCP03: is_early_close must be true after early close"
    );
    assert!(
        t_after.session_close_note.is_some(),
        "XCP03: session_close_note must be present on EarlyClose"
    );
}

// ---------------------------------------------------------------------------
// XCP04 — Unavailable source state returns Unknown (fail-closed)
// ---------------------------------------------------------------------------

/// When `source_state == Unavailable`, the provider must return `Unknown`
/// regardless of the queried date (SCSP07 fail-closed contract).
#[test]
fn xcp04_unavailable_source_returns_unknown() {
    let p = provider_with_state(ExchangeSourceState::Unavailable);

    // Query a date that would otherwise be a normal trading day
    let t = p.session_for(ts(2027, 1, 4, 15, 0, 0));
    assert_eq!(
        t.state,
        MarketSessionState::Unknown,
        "XCP04: Unavailable source must return Unknown (fail-closed)"
    );
    assert!(!t.is_in_session(), "XCP04: Unknown must not be in-session");
    assert!(
        !t.is_trading_day,
        "XCP04: Unknown must not claim trading_day=true"
    );
}

// ---------------------------------------------------------------------------
// XCP05 — Stale source state returns Unknown (fail-closed)
// ---------------------------------------------------------------------------

/// When `source_state == Stale`, the provider must return `Unknown`.
/// Stale data is not authoritative — callers must treat it as closed.
#[test]
fn xcp05_stale_source_fails_closed() {
    let p = provider_with_state(ExchangeSourceState::Stale);

    let t = p.session_for(ts(2027, 6, 7, 15, 0, 0)); // normal Monday in June
    assert_eq!(
        t.state,
        MarketSessionState::Unknown,
        "XCP05: Stale source must return Unknown (fail-closed)"
    );
    assert!(!t.is_in_session(), "XCP05: Unknown must not be in-session");
}

// ---------------------------------------------------------------------------
// XCP06 — Invalid source state returns Unknown (fail-closed)
// ---------------------------------------------------------------------------

/// When `source_state == Invalid`, the provider must return `Unknown`.
/// Structurally invalid data cannot be trusted for session truth.
#[test]
fn xcp06_invalid_source_fails_closed() {
    let p = provider_with_state(ExchangeSourceState::Invalid);

    let t = p.session_for(ts(2027, 3, 15, 15, 0, 0)); // normal Monday in March
    assert_eq!(
        t.state,
        MarketSessionState::Unknown,
        "XCP06: Invalid source must return Unknown (fail-closed)"
    );
    assert!(!t.is_in_session(), "XCP06: Unknown must not be in-session");
}

// ---------------------------------------------------------------------------
// XCP07 — FallbackToStatic policy delegates to heuristic and labels honestly
// ---------------------------------------------------------------------------

/// When `on_unavailable == FallbackToStatic` and the source is not Active,
/// the provider must delegate to the static heuristic and label the source
/// as `"exchange_sourced_fallback_to_static"` — not as the fixture or the
/// raw heuristic source.  This lets operators trace the fallback path.
///
/// Contrast: FailClosed produces Unknown; FallbackToStatic produces heuristic
/// result with honest fallback label.
#[test]
fn xcp07_fallback_to_static_delegates_with_honest_label() {
    // Build a provider with Unavailable source but FallbackToStatic policy.
    let p_fallback = ExchangeSourcedCalendarProvider {
        meta: fixture_meta(),
        source_state: ExchangeSourceState::Unavailable,
        days: fixture_days(),
        on_unavailable: ExchangeUnavailablePolicy::FallbackToStatic,
    };

    // Build the fail-closed counterpart for contrast.
    let p_fail = provider_with_state(ExchangeSourceState::Unavailable);

    // Query a normal Monday session: 2027-01-04 15:00 UTC = 10:00 EST → in session.
    let normal_day = ts(2027, 1, 4, 15, 0, 0);

    let t_fallback = p_fallback.session_for(normal_day);
    let t_fail = p_fail.session_for(normal_day);

    // FailClosed must return Unknown.
    assert_eq!(
        t_fail.state,
        MarketSessionState::Unknown,
        "XCP07 (contrast): FailClosed with Unavailable source must return Unknown"
    );

    // FallbackToStatic must NOT return Unknown for a normal trading day.
    assert_ne!(
        t_fallback.state,
        MarketSessionState::Unknown,
        "XCP07: FallbackToStatic with Unavailable source must not return Unknown for trading day"
    );
    assert_eq!(
        t_fallback.state,
        MarketSessionState::RegularOpen,
        "XCP07: FallbackToStatic must classify 10:00 EST as RegularOpen via heuristic"
    );

    // Source label must be the honest fallback label, not the fixture or heuristic name.
    assert_eq!(
        t_fallback.source, "exchange_sourced_fallback_to_static",
        "XCP07: FallbackToStatic source must be 'exchange_sourced_fallback_to_static'"
    );
    assert_ne!(
        t_fallback.source, "fixture_nyse_2027",
        "XCP07: fallback source must not claim to be the fixture provider"
    );
    assert_ne!(
        t_fallback.source, "nyse_weekdays_heuristic",
        "XCP07: fallback source must not silently pose as the raw heuristic"
    );

    // is_in_session must reflect the heuristic result.
    assert!(
        t_fallback.is_in_session(),
        "XCP07: FallbackToStatic must be in-session on a normal trading day"
    );
}

// ---------------------------------------------------------------------------
// XCP08 — No network calls (structural guarantee)
// ---------------------------------------------------------------------------

/// All ExchangeSourcedCalendarProvider tests use only fixture-injected data.
/// This test proves the structural property: constructing and querying the
/// provider requires no external dependencies — all data is caller-supplied.
///
/// Network-freedom is guaranteed by the absence of any I/O in the provider
/// implementation (it only queries the `days` HashMap and `utc_to_et_components`).
/// This test exercises a full lifecycle (build → query → assert) without any
/// external call, proving the seam is usable in pure unit-test contexts.
#[test]
fn xcp08_no_network_calls_all_fixture_data() {
    // Build provider entirely from fixture data (no env vars, no DB, no network).
    let mut days = HashMap::new();
    days.insert(
        (2027, 7, 19),
        ExchangeCalendarDay {
            date: (2027, 7, 19),
            status: ExchangeDayStatus::Open,
            early_close_et: None,
        },
    );
    let p = ExchangeSourcedCalendarProvider {
        meta: ExchangeCalendarMeta {
            source_name: "fixture_no_network",
            exchange: "NYSE",
            generated_at: None,
            coverage_start: (2027, 7, 1),
            coverage_end: (2027, 7, 31),
        },
        source_state: ExchangeSourceState::Active,
        days,
        on_unavailable: ExchangeUnavailablePolicy::FailClosed,
    };

    // 2027-07-19 is a Monday (non-holiday) at 14:00 UTC = 10:00 EDT (UTC-4, DST)
    let t = p.session_for(ts(2027, 7, 19, 14, 0, 0));
    assert_eq!(
        t.state,
        MarketSessionState::RegularOpen,
        "XCP08: 2027-07-19 Mon 10:00 EDT must be RegularOpen via pure fixture provider"
    );
    assert!(t.is_in_session(), "XCP08: must be in-session");
    assert_eq!(
        t.source, "fixture_no_network",
        "XCP08: source must be fixture label"
    );

    // Date outside coverage range must fail closed.
    let outside = ts(2027, 8, 1, 14, 0, 0); // Aug 1 outside Jul coverage
    let t_outside = p.session_for(outside);
    assert_eq!(
        t_outside.state,
        MarketSessionState::Unknown,
        "XCP08: date outside coverage range must return Unknown (fail-closed)"
    );
    assert!(
        !t_outside.is_in_session(),
        "XCP08: Unknown must not be in-session"
    );
}
