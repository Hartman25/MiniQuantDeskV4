//! SMART-CALENDAR-BAR-GAP-EARLY-CLOSE-01 — bar/session-end early-close proof.
//!
//! Proves that `CalendarSpec::NyseWeekdays.is_session_bar_end` and
//! `missing_bars_between` respect the same early-close calendar truth as
//! `classify_market_session` and `NyseWeekdaysProvider`.
//!
//! # Reference timestamps
//!
//! ## Black Friday 2024-11-29 (EST = UTC-5, early close 13:00 ET)
//!   09:35 EST = 14:35 UTC = 1_732_841_700  ← first bar of the day
//!   12:55 EST = 17:55 UTC = 1_732_900_500  ← last normal 5-min bar before close
//!   13:00 EST = 18:00 UTC = 1_732_903_200  ← SCBG04 early-close bar end (last session bar)
//!   13:05 EST = 18:05 UTC = 1_732_903_500  ← SCBG06 after early close — NOT session
//!   16:00 EST = 21:00 UTC = 1_732_914_000  ← NOT session (normal close blocked by early close)
//!   Mon 2024-12-02 09:35 EST = 14:35 UTC = 1_733_150_100 ← next trading session open+5min
//!
//! ## Independence Day Eve 2024-07-03 (EDT = UTC-4, early close 13:00 ET)
//!   13:00 EDT = 17:00 UTC = 1_720_026_000  ← SCBG03 early-close bar end (DST variant)
//!   13:05 EDT = 17:05 UTC = 1_720_026_300  ← after early close — NOT session
//!
//! ## Regular DST day 2024-04-15 Mon (EDT = UTC-4, normal close 16:00 ET)
//!   16:00 EDT = 20:00 UTC = 1_713_211_200  ← SCBG01 normal session bar end
//!
//! ## Regular standard-time day 2024-01-08 Mon (EST = UTC-5, normal close 16:00 ET)
//!   16:00 EST = 21:00 UTC = 1_704_747_600  ← SCBG02 normal session bar end
//!
//! # Test index
//!
//! | ID     | Behaviour proved                                                  |
//! |--------|-------------------------------------------------------------------|
//! | SCBG01 | Normal DST day — 16:00 EDT (20:00 UTC) is a session bar end       |
//! | SCBG02 | Normal EST day — 16:00 EST (21:00 UTC) is a session bar end       |
//! | SCBG03 | Early-close EDT — 13:00 EDT (17:00 UTC) is the last session bar   |
//! | SCBG04 | Early-close EST — 13:00 EST (18:00 UTC) is the last session bar   |
//! | SCBG05 | Early-close EST — 13:05 ET (18:05 UTC) is NOT a session bar       |
//! | SCBG06 | Gap from early close to next Mon open has 0 missing bars          |
//! | SCBG07 | Intra-session gap on early-close day still halts                  |
//! | SCBG08 | classify_market_session agrees with is_session_bar_end (no drift) |

use mqk_integrity::{
    evaluate_bar, Bar, BarKey, CalendarSpec, FeedId, IntegrityAction, IntegrityConfig,
    IntegrityReason, IntegrityState, Timeframe,
};

const INTERVAL: i64 = 300; // 5-minute bars

fn nyse_cfg() -> IntegrityConfig {
    IntegrityConfig {
        gap_tolerance_bars: 0,
        stale_threshold_ticks: 0,
        enforce_feed_disagreement: false,
        calendar: CalendarSpec::NyseWeekdays,
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
// SCBG01 — Normal DST day session bar end is 20:00 UTC
// ---------------------------------------------------------------------------

/// Mon 2024-04-15 16:00 EDT (20:00 UTC) is the last session bar on a normal day.
#[test]
fn scbg01_normal_dst_close_20_utc_is_session_bar_end() {
    let ts = 1_713_211_200_i64; // 2024-04-15T20:00:00Z = 16:00 EDT
    assert!(
        CalendarSpec::NyseWeekdays.is_session_bar_end(ts),
        "SCBG01: 16:00 EDT (20:00 UTC) must be a session bar end"
    );
}

// ---------------------------------------------------------------------------
// SCBG02 — Normal standard-time day session bar end is 21:00 UTC
// ---------------------------------------------------------------------------

/// Mon 2024-01-08 16:00 EST (21:00 UTC) is the last session bar on a normal day.
#[test]
fn scbg02_normal_est_close_21_utc_is_session_bar_end() {
    let ts = 1_704_747_600_i64; // 2024-01-08T21:00:00Z = 16:00 EST
    assert!(
        CalendarSpec::NyseWeekdays.is_session_bar_end(ts),
        "SCBG02: 16:00 EST (21:00 UTC) must be a session bar end"
    );
}

// ---------------------------------------------------------------------------
// SCBG03 — Early-close DST day session bar end is 17:00 UTC
// ---------------------------------------------------------------------------

/// Independence Day Eve 2024-07-03 (EDT=UTC-4): 13:00 EDT (17:00 UTC) is last session bar.
#[test]
fn scbg03_early_close_dst_13h_edt_17_utc_is_last_session_bar() {
    let ts = 1_720_026_000_i64; // 2024-07-03T17:00:00Z = 13:00 EDT (early close)
    assert!(
        CalendarSpec::NyseWeekdays.is_session_bar_end(ts),
        "SCBG03: 13:00 EDT on early-close day must be the last session bar (17:00 UTC)"
    );
}

/// Independence Day Eve: 13:05 EDT (17:05 UTC) is after early close — NOT session.
#[test]
fn scbg03_early_close_dst_13h05_edt_not_session() {
    let ts = 1_720_026_300_i64; // 2024-07-03T17:05:00Z = 13:05 EDT
    assert!(
        !CalendarSpec::NyseWeekdays.is_session_bar_end(ts),
        "SCBG03: 13:05 EDT on early-close day is after early close — must NOT be session"
    );
}

// ---------------------------------------------------------------------------
// SCBG04 — Early-close standard-time day session bar end is 18:00 UTC
// ---------------------------------------------------------------------------

/// Black Friday 2024-11-29 (EST=UTC-5): 13:00 EST (18:00 UTC) is last session bar.
#[test]
fn scbg04_early_close_est_13h_est_18_utc_is_last_session_bar() {
    let ts = 1_732_903_200_i64; // 2024-11-29T18:00:00Z = 13:00 EST (early close)
    assert!(
        CalendarSpec::NyseWeekdays.is_session_bar_end(ts),
        "SCBG04: 13:00 EST on Black Friday (early-close) must be the last session bar (18:00 UTC)"
    );
}

// ---------------------------------------------------------------------------
// SCBG05 — Bar after early close is NOT a session bar end
// ---------------------------------------------------------------------------

/// Black Friday 2024-11-29 (EST): bar at 13:05 ET (5 min after early close) is not session.
#[test]
fn scbg05_after_early_close_est_is_not_session() {
    let ts = 1_732_903_500_i64; // 2024-11-29T18:05:00Z = 13:05 EST
    assert!(
        !CalendarSpec::NyseWeekdays.is_session_bar_end(ts),
        "SCBG05: 13:05 EST on early-close day is after close — must NOT be session bar"
    );
}

/// Black Friday 2024-11-29 (EST): bar at 16:00 ET (normal close, but market was early close) — not session.
#[test]
fn scbg05b_normal_close_time_on_early_close_day_is_not_session() {
    let ts = 1_732_914_000_i64; // 2024-11-29T21:00:00Z = 16:00 EST
    assert!(
        !CalendarSpec::NyseWeekdays.is_session_bar_end(ts),
        "SCBG05b: 16:00 EST on early-close day is well after early close — must NOT be session"
    );
}

// ---------------------------------------------------------------------------
// SCBG06 — Gap from early close to next Monday open has 0 missing bars
// ---------------------------------------------------------------------------

/// From Black Friday 13:00 ET (early close) to Mon 2024-12-02 09:35 ET:
/// all intermediate slots are non-session (after early close, weekend, pre-market).
/// missing_bars_between must return 0 — no false missing bar detection.
#[test]
fn scbg06_early_close_to_next_open_zero_missing_bars() {
    let early_close_end: i64 = 1_732_903_200; // 2024-11-29 13:00 EST
    let monday_open5: i64 = 1_733_150_100; // 2024-12-02 09:35 EST
    let missing =
        CalendarSpec::NyseWeekdays.missing_bars_between(early_close_end, monday_open5, 300);
    assert_eq!(
        missing, 0,
        "SCBG06: gap from early close (13:00 ET Fri) to Mon open must have 0 missing bars, got {missing}"
    );
}

/// evaluate_bar with NyseWeekdays does not halt going from early close to Monday open.
#[test]
fn scbg06_bar_gap_early_close_to_monday_does_not_halt() {
    let early_close_end: i64 = 1_732_903_200; // 2024-11-29 13:00 EST (Black Friday early close)
    let monday_open5: i64 = 1_733_150_100; // 2024-12-02 09:35 EST (Mon first bar)

    let mut st = IntegrityState::new();
    let cfg = nyse_cfg();

    let d1 = evaluate_bar(&cfg, &mut st, &feed(), 1, &bar(early_close_end));
    assert_eq!(
        d1.action,
        IntegrityAction::Allow,
        "SCBG06: early-close bar must be allowed"
    );

    let d2 = evaluate_bar(&cfg, &mut st, &feed(), 2, &bar(monday_open5));
    assert_eq!(
        d2.action,
        IntegrityAction::Allow,
        "SCBG06: Monday first bar after early-close Friday must NOT trigger halt; got {:?}",
        d2.reason
    );
    assert!(
        !st.halted,
        "SCBG06: halted must be false after early-close gap"
    );
}

// ---------------------------------------------------------------------------
// SCBG07 — Intra-session gap on early-close day still halts
// ---------------------------------------------------------------------------

/// Real gap within Black Friday session (10:00 ET to 10:25 ET = 4 missing bars) still halts.
#[test]
fn scbg07_intra_session_gap_on_early_close_day_halts() {
    // 2024-11-29 10:00 EST = 15:00 UTC
    let bar_10_00: i64 = 1_732_892_400; // 2024-11-29T15:00:00Z = 10:00 EST
                                        // 2024-11-29 10:25 EST = 15:25 UTC (4 bars skipped)
    let bar_10_25: i64 = 1_732_893_900; // 2024-11-29T15:25:00Z = 10:25 EST

    let mut st = IntegrityState::new();
    let cfg = nyse_cfg();

    let d1 = evaluate_bar(&cfg, &mut st, &feed(), 1, &bar(bar_10_00));
    assert_eq!(
        d1.action,
        IntegrityAction::Allow,
        "SCBG07: first bar must be allowed"
    );

    let d2 = evaluate_bar(&cfg, &mut st, &feed(), 2, &bar(bar_10_25));
    assert_eq!(
        d2.action,
        IntegrityAction::Halt,
        "SCBG07: real intra-session gap on early-close day must still halt"
    );
    assert_eq!(
        d2.reason,
        IntegrityReason::GapDetected,
        "SCBG07: reason must be GapDetected"
    );
    assert!(st.halted, "SCBG07: halted flag must be set");
}

// ---------------------------------------------------------------------------
// SCBG08 — classify_market_session agrees with is_session_bar_end
// ---------------------------------------------------------------------------

/// For every timestamp used in this suite, `classify_market_session == "regular"`
/// must match `is_session_bar_end == true` and vice versa.
///
/// This proves that both functions read the same early-close truth and do not drift.
#[test]
fn scbg08_classify_agrees_with_is_session_bar_end() {
    let cases: &[(i64, bool, &str)] = &[
        // (timestamp, expected_is_session, label)
        (1_713_211_200, true, "SCBG01 EDT 16:00 (normal close)"),
        (1_704_747_600, true, "SCBG02 EST 16:00 (normal close)"),
        (1_720_026_000, true, "SCBG03 EDT 13:00 early-close"),
        (1_720_026_300, false, "SCBG03 EDT 13:05 after early-close"),
        (1_732_903_200, true, "SCBG04 EST 13:00 early-close"),
        (1_732_903_500, false, "SCBG05 EST 13:05 after early-close"),
        (1_732_914_000, false, "SCBG05b EST 16:00 on early-close day"),
    ];

    for &(ts, expected_session, label) in cases {
        let is_session = CalendarSpec::NyseWeekdays.is_session_bar_end(ts);
        assert_eq!(
            is_session, expected_session,
            "SCBG08 is_session_bar_end mismatch at {label} (ts={ts})"
        );

        let classify = CalendarSpec::NyseWeekdays.classify_market_session(ts);
        let classify_says_regular = classify == "regular";
        assert_eq!(
            classify_says_regular, expected_session,
            "SCBG08 classify_market_session={classify:?} disagrees with is_session_bar_end={expected_session} at {label} (ts={ts})"
        );
    }
}
