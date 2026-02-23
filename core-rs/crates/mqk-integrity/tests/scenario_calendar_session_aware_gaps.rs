//! Patch B3 — Session-aware gap detection scenario tests.
//!
//! Validates that gap detection does not false-positive on:
//! - Weekend bars (Saturday / Sunday).
//! - NYSE market holidays.
//! - Pre/after-market bar slots.
//!
//! And continues to correctly detect real intra-session gaps.
//!
//! All timestamps are derived from well-known UTC epoch values; comments
//! document the human-readable date and ET time for traceability.
//!
//! Reference epoch offsets (UTC):
//!   NYSE open:  09:30 ET = 14:30 UTC (UTC-5)
//!   NYSE close: 16:00 ET = 21:00 UTC (UTC-5)
//!
//!   2024-01-08 Mon  — regular trading day
//!   2024-01-06 Sat  — weekend
//!   2024-01-07 Sun  — weekend
//!   2024-01-01 Mon  — New Year's Day 2024 (NYSE holiday)
//!   2024-12-25 Wed  — Christmas 2024 (NYSE holiday)
//!
//! 5-minute bar interval = 300 seconds throughout.

use mqk_integrity::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const INTERVAL: i64 = 300; // 5-minute bars

fn nyse_cfg() -> IntegrityConfig {
    IntegrityConfig {
        gap_tolerance_bars: 0,
        stale_threshold_ticks: 0,
        enforce_feed_disagreement: false,
        calendar: CalendarSpec::NyseWeekdays,
    }
}

fn always_on_cfg() -> IntegrityConfig {
    IntegrityConfig {
        gap_tolerance_bars: 0,
        stale_threshold_ticks: 0,
        enforce_feed_disagreement: false,
        calendar: CalendarSpec::AlwaysOn,
    }
}

fn feed() -> FeedId {
    FeedId::new("main")
}

fn bar(end_ts: i64) -> Bar {
    Bar::new(
        BarKey::new("SPY", Timeframe::secs(INTERVAL), end_ts),
        true,
        500_000_000,
        1000,
    )
}

// ---------------------------------------------------------------------------
// Weekend gap → NOT a missing bar (NyseWeekdays)
// ---------------------------------------------------------------------------

/// Friday close (16:00 ET) to Monday open+5min (09:35 ET):
/// the entire weekend gap contains zero trading sessions → no halt.
///
/// Friday  2024-01-05 16:00 ET = 2024-01-05T21:00:00Z = 1_704_495_600
/// Monday  2024-01-08 09:35 ET = 2024-01-08T14:35:00Z = 1_704_723_300
#[test]
fn weekend_gap_nyse_does_not_halt() {
    let friday_close: i64 = 1_704_495_600; // 2024-01-05 16:00 ET
    let monday_open5: i64 = 1_704_723_300; // 2024-01-08 09:35 ET

    let mut st = IntegrityState::new();
    let cfg = nyse_cfg();

    // Establish baseline at Friday close.
    let d1 = evaluate_bar(&cfg, &mut st, &feed(), 1, &bar(friday_close));
    assert_eq!(
        d1.action,
        IntegrityAction::Allow,
        "Friday close should be allowed"
    );

    // Monday's first bar arrives after a full weekend.
    let d2 = evaluate_bar(&cfg, &mut st, &feed(), 2, &bar(monday_open5));
    assert_eq!(
        d2.action,
        IntegrityAction::Allow,
        "Weekend gap must NOT trigger halt; got reason {:?}",
        d2.reason
    );
    assert!(!st.halted, "halted flag must not be set for weekend gap");
}

/// Same timestamps with AlwaysOn calendar: the weekend gap should HALT
/// (no session awareness → naive arithmetic counts every slot as missing).
#[test]
fn weekend_gap_always_on_halts() {
    let friday_close: i64 = 1_704_495_600;
    let monday_open5: i64 = 1_704_723_300;

    let mut st = IntegrityState::new();
    let cfg = always_on_cfg();

    evaluate_bar(&cfg, &mut st, &feed(), 1, &bar(friday_close));
    let d2 = evaluate_bar(&cfg, &mut st, &feed(), 2, &bar(monday_open5));

    assert_eq!(
        d2.action,
        IntegrityAction::Halt,
        "AlwaysOn should still halt on weekend gap (many missing bars)"
    );
    assert!(st.halted);
}

// ---------------------------------------------------------------------------
// Holiday gap → NOT a missing bar (NyseWeekdays)
// ---------------------------------------------------------------------------

/// Christmas 2024 falls on Wednesday 2024-12-25 (NYSE closed).
/// Tuesday 2024-12-24 close → Thursday 2024-12-26 open+5min:
/// the holiday slot is not a trading session → no halt.
///
/// Tue 2024-12-24 16:00 ET = 2024-12-24T21:00:00Z = 1_735_077_600
/// Thu 2024-12-26 09:35 ET = 2024-12-26T14:35:00Z = 1_735_220_100
#[test]
fn holiday_gap_nyse_does_not_halt() {
    let xmas_eve_close: i64 = 1_735_077_600; // Tue 2024-12-24 16:00 ET
    let day_after_xmas: i64 = 1_735_220_100; // Thu 2024-12-26 09:35 ET

    let mut st = IntegrityState::new();
    let cfg = nyse_cfg();

    let d1 = evaluate_bar(&cfg, &mut st, &feed(), 1, &bar(xmas_eve_close));
    assert_eq!(d1.action, IntegrityAction::Allow);

    let d2 = evaluate_bar(&cfg, &mut st, &feed(), 2, &bar(day_after_xmas));
    assert_eq!(
        d2.action,
        IntegrityAction::Allow,
        "Holiday gap (Christmas) must NOT trigger halt; got reason {:?}",
        d2.reason
    );
    assert!(!st.halted, "halted flag must not be set for holiday gap");
}

// ---------------------------------------------------------------------------
// Intra-session gap → HALT (NyseWeekdays)
// ---------------------------------------------------------------------------

/// A gap within regular trading hours (four consecutive 5-min slots missing)
/// must still halt, even with the NYSE calendar.
///
/// Mon 2024-01-08 10:00 ET = 2024-01-08T15:00:00Z = 1_704_726_000
/// Mon 2024-01-08 10:25 ET = 2024-01-08T15:25:00Z = 1_704_727_500  (skips 10:05, 10:10, 10:15, 10:20)
#[test]
fn intra_session_gap_nyse_still_halts() {
    let bar_10_00: i64 = 1_704_726_000; // 2024-01-08 10:00 ET = 2024-01-08T15:00:00Z
    let bar_10_25: i64 = 1_704_727_500; // 2024-01-08 10:25 ET = 2024-01-08T15:25:00Z (gap of 4 bars)

    let mut st = IntegrityState::new();
    let cfg = nyse_cfg();

    let d1 = evaluate_bar(&cfg, &mut st, &feed(), 1, &bar(bar_10_00));
    assert_eq!(d1.action, IntegrityAction::Allow);

    let d2 = evaluate_bar(&cfg, &mut st, &feed(), 2, &bar(bar_10_25));
    assert_eq!(
        d2.action,
        IntegrityAction::Halt,
        "Intra-session gap must still halt with NYSE calendar"
    );
    assert_eq!(d2.reason, IntegrityReason::GapDetected);
    assert!(st.halted);
}

// ---------------------------------------------------------------------------
// Consecutive session bars → no gap
// ---------------------------------------------------------------------------

/// Two back-to-back 5-min bars during session hours must both be allowed.
///
/// Mon 2024-01-08 10:00 ET = 2024-01-08T15:00:00Z = 1_704_726_000
/// Mon 2024-01-08 10:05 ET = 2024-01-08T15:05:00Z = 1_704_726_300
#[test]
fn consecutive_session_bars_allowed() {
    let bar_a: i64 = 1_704_726_000; // 2024-01-08 10:00 ET
    let bar_b: i64 = 1_704_726_300; // 2024-01-08 10:05 ET

    let mut st = IntegrityState::new();
    let cfg = nyse_cfg();

    assert_eq!(
        evaluate_bar(&cfg, &mut st, &feed(), 1, &bar(bar_a)).action,
        IntegrityAction::Allow
    );
    assert_eq!(
        evaluate_bar(&cfg, &mut st, &feed(), 2, &bar(bar_b)).action,
        IntegrityAction::Allow
    );
    assert!(!st.halted);
}

// ---------------------------------------------------------------------------
// Thanksgiving 2024 holiday gap
// ---------------------------------------------------------------------------

/// Thanksgiving 2024: Thursday 2024-11-28 (NYSE closed).
/// Wed 2024-11-27 close → Fri 2024-11-29 open+5min:
/// one holiday slot, no real trading session → no halt.
///
/// Wed 2024-11-27 16:00 ET = 2024-11-27T21:00:00Z = 1_732_741_200
/// Fri 2024-11-29 09:35 ET = 2024-11-29T14:35:00Z = 1_732_883_700
#[test]
fn thanksgiving_holiday_gap_nyse_does_not_halt() {
    let wed_close: i64 = 1_732_741_200; // Wed 2024-11-27 16:00 ET
    let fri_open5: i64 = 1_732_883_700; // Fri 2024-11-29 09:35 ET

    let mut st = IntegrityState::new();
    let cfg = nyse_cfg();

    let d1 = evaluate_bar(&cfg, &mut st, &feed(), 1, &bar(wed_close));
    assert_eq!(d1.action, IntegrityAction::Allow);

    let d2 = evaluate_bar(&cfg, &mut st, &feed(), 2, &bar(fri_open5));
    assert_eq!(
        d2.action,
        IntegrityAction::Allow,
        "Thanksgiving holiday gap must NOT halt; got reason {:?}",
        d2.reason
    );
    assert!(!st.halted);
}

// ---------------------------------------------------------------------------
// CalendarSpec::missing_bars_between unit assertions
// ---------------------------------------------------------------------------

/// `AlwaysOn.missing_bars_between` matches naive arithmetic.
#[test]
fn always_on_missing_bars_matches_naive() {
    // 3 steps from t=1000, next bar at t=1900 with interval=300:
    // steps = (1900 - 1000) / 300 = 3 => missing = 2
    let missing = CalendarSpec::AlwaysOn.missing_bars_between(1000, 1900, 300);
    assert_eq!(missing, 2, "AlwaysOn: expected 2 missing bars");
}

/// `NyseWeekdays.missing_bars_between` over a weekend gap returns 0.
#[test]
fn nyse_missing_bars_weekend_is_zero() {
    // Friday close to Monday open+5min: all slots between are non-session.
    let friday_close: i64 = 1_704_495_600;
    let monday_open5: i64 = 1_704_723_300;
    let missing =
        CalendarSpec::NyseWeekdays.missing_bars_between(friday_close, monday_open5, INTERVAL);
    assert_eq!(
        missing, 0,
        "NyseWeekdays: weekend gap should have 0 missing trading bars, got {missing}"
    );
}
