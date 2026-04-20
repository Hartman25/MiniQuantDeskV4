//! EVENT-RISK-CALENDAR-01: Earnings-calendar gate — pure evaluator proof tests.
//!
//! ## What this proves
//!
//! Gate 1h-calendar was added to `POST /api/v1/strategy/signal` by
//! EVENT-RISK-CALENDAR-01.  It checks the signal's symbol against operator-declared
//! earnings events in an `earnings-calendar-v1` JSON file, using the per-entry
//! pre/post proximity windows to derive the effective blackout interval.
//!
//! This file proves the pure evaluator (`evaluate_earnings_calendar`) that backs the
//! gate.  All tests are pure in-process with no DB, no network, and no environment
//! variables required.
//!
//! ## Tests
//!
//! - EC-01: Authorized when symbol not in any entry
//! - EC-02: EarningsBlackout when signal is within pre-window (before earnings_ts)
//! - EC-03: EarningsBlackout when signal is within post-window (after earnings_ts)
//! - EC-04: EarningsBlackout when signal is at exactly earnings_ts
//! - EC-05: Authorized when signal is before the pre-window start (start − 1)
//! - EC-06: Authorized when signal is after the post-window end (end + 1)
//! - EC-07: Symbol-specificity — another symbol's entry does not affect the checked symbol
//! - EC-08: Wrong schema_version → Unavailable (fail-closed)
//! - EC-09: Malformed JSON → Unavailable (fail-closed)
//! - EC-10: `is_signal_safe` contract — true only for NotConfigured and Authorized
//! - EC-11: `earnings_calendar_surface_label` returns "not_connected" without env var

use mqk_daemon::earnings_calendar::{
    earnings_calendar_surface_label, evaluate_earnings_calendar, EarningsCalendarOutcome,
};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// earnings_ts used across boundary tests.
const TS_EARNINGS: i64 = 1_700_043_600;
const PRE_WINDOW: i64 = 86_400; // 24 hours
const POST_WINDOW: i64 = 14_400; // 4 hours

fn calendar_json(symbol: &str) -> String {
    format!(
        r#"{{"schema_version":"earnings-calendar-v1","entries":[{{"symbol":"{symbol}","earnings_ts":{TS_EARNINGS},"pre_window_secs":{PRE_WINDOW},"post_window_secs":{POST_WINDOW}}}]}}"#
    )
}

// ---------------------------------------------------------------------------
// EC-01: Authorized — symbol not in any entry
//
// Proves: when the checked symbol has no entry in the calendar, the gate
// returns Authorized and is_signal_safe is true.
// ---------------------------------------------------------------------------

#[test]
fn ec_01_authorized_when_symbol_not_in_calendar() {
    let json = calendar_json("AAPL");
    let outcome = evaluate_earnings_calendar(&json, "SPY", TS_EARNINGS);
    assert_eq!(
        outcome,
        EarningsCalendarOutcome::Authorized,
        "EC-01: symbol with no calendar entry must be Authorized; got: {outcome:?}"
    );
    assert!(
        outcome.is_signal_safe(),
        "EC-01: Authorized must be signal-safe"
    );
}

// ---------------------------------------------------------------------------
// EC-02: EarningsBlackout — signal within pre-window
//
// Proves: a signal at (earnings_ts − pre_window_secs / 2) is refused because
// it falls within the effective pre-earnings blackout window.
// ---------------------------------------------------------------------------

#[test]
fn ec_02_earnings_blackout_within_pre_window() {
    let json = calendar_json("AAPL");
    let ts_mid_pre = TS_EARNINGS - PRE_WINDOW / 2;
    let outcome = evaluate_earnings_calendar(&json, "AAPL", ts_mid_pre);
    assert!(
        matches!(outcome, EarningsCalendarOutcome::EarningsBlackout { .. }),
        "EC-02: signal within pre-window must be EarningsBlackout; got: {outcome:?}"
    );
    assert!(
        !outcome.is_signal_safe(),
        "EC-02: EarningsBlackout must not be signal-safe (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// EC-03: EarningsBlackout — signal within post-window
//
// Proves: a signal at (earnings_ts + post_window_secs / 2) is refused because
// it falls within the effective post-earnings blackout window.
// ---------------------------------------------------------------------------

#[test]
fn ec_03_earnings_blackout_within_post_window() {
    let json = calendar_json("AAPL");
    let ts_mid_post = TS_EARNINGS + POST_WINDOW / 2;
    let outcome = evaluate_earnings_calendar(&json, "AAPL", ts_mid_post);
    assert!(
        matches!(outcome, EarningsCalendarOutcome::EarningsBlackout { .. }),
        "EC-03: signal within post-window must be EarningsBlackout; got: {outcome:?}"
    );
    assert!(
        !outcome.is_signal_safe(),
        "EC-03: EarningsBlackout must not be signal-safe (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// EC-04: EarningsBlackout — signal at exactly earnings_ts
//
// Proves: a signal at the exact earnings timestamp is refused (included in
// both pre and post windows; the interval is [start, end] inclusive).
// ---------------------------------------------------------------------------

#[test]
fn ec_04_earnings_blackout_at_exact_earnings_ts() {
    let json = calendar_json("AAPL");
    let outcome = evaluate_earnings_calendar(&json, "AAPL", TS_EARNINGS);
    assert!(
        matches!(outcome, EarningsCalendarOutcome::EarningsBlackout { .. }),
        "EC-04: signal at exact earnings_ts must be EarningsBlackout; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// EC-05: Authorized — signal one second before the pre-window start
//
// Proves: the pre-window start bound is exclusive on its outer side:
// ts = earnings_ts − pre_window_secs − 1 is Authorized (outside the window).
// ---------------------------------------------------------------------------

#[test]
fn ec_05_authorized_before_pre_window_start() {
    let json = calendar_json("AAPL");
    let blackout_start = TS_EARNINGS - PRE_WINDOW;
    let just_before = blackout_start - 1;
    let outcome = evaluate_earnings_calendar(&json, "AAPL", just_before);
    assert_eq!(
        outcome,
        EarningsCalendarOutcome::Authorized,
        "EC-05: signal at blackout_start−1 must be Authorized; got: {outcome:?}"
    );

    // Also verify the start boundary itself is blocked (inclusive).
    let at_start = evaluate_earnings_calendar(&json, "AAPL", blackout_start);
    assert!(
        matches!(at_start, EarningsCalendarOutcome::EarningsBlackout { .. }),
        "EC-05: signal at exact blackout_start must be EarningsBlackout (inclusive); got: {at_start:?}"
    );
}

// ---------------------------------------------------------------------------
// EC-06: Authorized — signal one second after the post-window end
//
// Proves: the post-window end bound is exclusive on its outer side:
// ts = earnings_ts + post_window_secs + 1 is Authorized (outside the window).
// ---------------------------------------------------------------------------

#[test]
fn ec_06_authorized_after_post_window_end() {
    let json = calendar_json("AAPL");
    let blackout_end = TS_EARNINGS + POST_WINDOW;
    let just_after = blackout_end + 1;
    let outcome = evaluate_earnings_calendar(&json, "AAPL", just_after);
    assert_eq!(
        outcome,
        EarningsCalendarOutcome::Authorized,
        "EC-06: signal at blackout_end+1 must be Authorized; got: {outcome:?}"
    );

    // Also verify the end boundary itself is blocked (inclusive).
    let at_end = evaluate_earnings_calendar(&json, "AAPL", blackout_end);
    assert!(
        matches!(at_end, EarningsCalendarOutcome::EarningsBlackout { .. }),
        "EC-06: signal at exact blackout_end must be EarningsBlackout (inclusive); got: {at_end:?}"
    );
}

// ---------------------------------------------------------------------------
// EC-07: Symbol-specificity
//
// Proves: an entry for "AAPL" does not block "MSFT" signals even when the
// timestamp falls inside the AAPL earnings window.
// ---------------------------------------------------------------------------

#[test]
fn ec_07_symbol_specificity() {
    let json = calendar_json("AAPL");
    let outcome = evaluate_earnings_calendar(&json, "MSFT", TS_EARNINGS);
    assert_eq!(
        outcome,
        EarningsCalendarOutcome::Authorized,
        "EC-07: AAPL entry must not affect MSFT signals; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// EC-08: Wrong schema_version → Unavailable (fail-closed)
//
// Proves: an earnings calendar file with an unrecognized schema_version is not
// silently treated as authorizing or as NotConfigured — it fails closed.
// ---------------------------------------------------------------------------

#[test]
fn ec_08_wrong_schema_version_is_unavailable() {
    let json = r#"{"schema_version":"earnings-calendar-v2","entries":[]}"#;
    let outcome = evaluate_earnings_calendar(json, "AAPL", TS_EARNINGS);
    assert!(
        matches!(outcome, EarningsCalendarOutcome::Unavailable { .. }),
        "EC-08: wrong schema_version must yield Unavailable; got: {outcome:?}"
    );
    assert!(
        !outcome.is_signal_safe(),
        "EC-08: Unavailable must not be signal-safe (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// EC-09: Malformed JSON → Unavailable (fail-closed)
//
// Proves: an earnings calendar file that cannot be parsed fails closed rather
// than silently passing or returning NotConfigured.
// ---------------------------------------------------------------------------

#[test]
fn ec_09_malformed_json_is_unavailable() {
    let outcome = evaluate_earnings_calendar("not valid json {{{", "AAPL", TS_EARNINGS);
    assert!(
        matches!(outcome, EarningsCalendarOutcome::Unavailable { .. }),
        "EC-09: malformed JSON must yield Unavailable; got: {outcome:?}"
    );
    assert!(
        !outcome.is_signal_safe(),
        "EC-09: Unavailable must not be signal-safe (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// EC-10: is_signal_safe contract
//
// Proves: only NotConfigured and Authorized satisfy is_signal_safe.
// EarningsBlackout and Unavailable both fail closed.
// ---------------------------------------------------------------------------

#[test]
fn ec_10_is_signal_safe_contract() {
    assert!(
        EarningsCalendarOutcome::NotConfigured.is_signal_safe(),
        "EC-10: NotConfigured must be signal-safe"
    );
    assert!(
        EarningsCalendarOutcome::Authorized.is_signal_safe(),
        "EC-10: Authorized must be signal-safe"
    );
    assert!(
        !EarningsCalendarOutcome::EarningsBlackout {
            note: "test".to_string()
        }
        .is_signal_safe(),
        "EC-10: EarningsBlackout must not be signal-safe"
    );
    assert!(
        !EarningsCalendarOutcome::Unavailable {
            reason: "test".to_string()
        }
        .is_signal_safe(),
        "EC-10: Unavailable must not be signal-safe"
    );
}

// ---------------------------------------------------------------------------
// EC-11: earnings_calendar_surface_label without MQK_EARNINGS_CALENDAR_PATH
//
// Proves: when the env var is not set (standard CI environment), the surface
// label is "not_connected".  When it IS set to a non-empty path, the label
// is "operator_file".
// ---------------------------------------------------------------------------

#[test]
fn ec_11_surface_label_not_connected_without_env_var() {
    // In standard CI the env var is not set; must be "not_connected".
    if std::env::var("MQK_EARNINGS_CALENDAR_PATH").is_err() {
        assert_eq!(
            earnings_calendar_surface_label(),
            "not_connected",
            "EC-11: without env var, surface label must be 'not_connected'"
        );
    }
    // Either value is valid; neither panics.
    let label = earnings_calendar_surface_label();
    assert!(
        label == "not_connected" || label == "operator_file",
        "EC-11: surface label must be 'not_connected' or 'operator_file'; got: '{label}'"
    );
}
