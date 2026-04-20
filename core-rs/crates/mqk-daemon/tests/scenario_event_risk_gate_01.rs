//! EVENT-RISK-GATE-01: Event-risk blackout gate — pure evaluator proof tests.
//!
//! ## What this proves
//!
//! Gate 1h was added to `POST /api/v1/strategy/signal` by EVENT-RISK-GATE-01.
//! It checks the signal's symbol against operator-declared blackout periods
//! at the current session timestamp, using `MQK_EVENT_RISK_BLACKOUT_PATH`.
//!
//! This file proves the pure evaluator (`evaluate_event_risk_blackout`) that
//! backs the gate.  All tests are pure in-process with no DB, no network, and
//! no environment variables required.
//!
//! ## Tests
//!
//! - EG-01: `evaluate_event_risk_blackout` — authorized when symbol not in any period
//! - EG-02: `evaluate_event_risk_blackout` — blackout when symbol exactly in period
//! - EG-03: Symbol-specificity — another symbol in the same period → Authorized
//! - EG-04: Timestamp start boundary — at `start_ts` → Blackout; `start_ts − 1` → Authorized
//! - EG-05: Timestamp end boundary   — at `end_ts`   → Blackout; `end_ts + 1`   → Authorized
//! - EG-06: Wrong `schema_version` → Unavailable (fail-closed)
//! - EG-07: Malformed JSON → Unavailable (fail-closed)
//! - EG-08: Multiple periods — only matching symbol+ts triggers Blackout
//! - EG-09: `is_signal_safe` contract — true only for `NotConfigured` and `Authorized`
//! - EG-10: `blackout_surface_label` returns `"not_configured"` without env var

use mqk_daemon::event_risk_blackout::{
    blackout_surface_label, evaluate_event_risk_blackout, EventRiskBlackoutOutcome,
};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

const TS_MID: i64 = 1_700_050_000;
const TS_START: i64 = 1_700_000_000;
const TS_END: i64 = 1_700_086_400;

fn blackout_json(symbol: &str, start: i64, end: i64) -> String {
    format!(
        r#"{{"schema_version":"blackout-v1","periods":[{{"symbol":"{symbol}","start_ts":{start},"end_ts":{end}}}]}}"#
    )
}

// ---------------------------------------------------------------------------
// EG-01: Authorized — symbol not in any period
//
// Proves: when the symbol has no matching period the gate returns Authorized
// and `is_signal_safe` is true.
// ---------------------------------------------------------------------------

#[test]
fn eg_01_authorized_when_symbol_not_in_period() {
    let json = blackout_json("AAPL", TS_START, TS_END);
    let outcome = evaluate_event_risk_blackout(&json, "SPY", TS_MID);
    assert_eq!(
        outcome,
        EventRiskBlackoutOutcome::Authorized,
        "EG-01: different symbol must be Authorized; got: {outcome:?}"
    );
    assert!(
        outcome.is_signal_safe(),
        "EG-01: Authorized must be signal-safe"
    );
}

// ---------------------------------------------------------------------------
// EG-02: Blackout — symbol matches period at mid-window timestamp
//
// Proves: when the symbol is in a declared period the gate returns Blackout
// and `is_signal_safe` is false (fail-closed).
// ---------------------------------------------------------------------------

#[test]
fn eg_02_blackout_when_symbol_in_period() {
    let json = blackout_json("AAPL", TS_START, TS_END);
    let outcome = evaluate_event_risk_blackout(&json, "AAPL", TS_MID);
    assert!(
        matches!(outcome, EventRiskBlackoutOutcome::Blackout { .. }),
        "EG-02: matching symbol+ts must be Blackout; got: {outcome:?}"
    );
    assert!(
        !outcome.is_signal_safe(),
        "EG-02: Blackout must not be signal-safe (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// EG-03: Symbol-specificity — other symbol in same period → Authorized
//
// Proves: a period declared for "AAPL" does not block "MSFT" signals even
// when the timestamp falls inside the AAPL window.
// ---------------------------------------------------------------------------

#[test]
fn eg_03_symbol_specificity() {
    let json = blackout_json("AAPL", TS_START, TS_END);
    let outcome = evaluate_event_risk_blackout(&json, "MSFT", TS_MID);
    assert_eq!(
        outcome,
        EventRiskBlackoutOutcome::Authorized,
        "EG-03: AAPL period must not affect MSFT; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// EG-04: Timestamp start boundary (inclusive)
//
// Proves: a signal at exactly `start_ts` is refused (inclusive lower bound);
// a signal at `start_ts − 1` is authorized.
// ---------------------------------------------------------------------------

#[test]
fn eg_04_timestamp_start_boundary() {
    let json = blackout_json("AAPL", TS_START, TS_END);

    // At start_ts — must be Blackout (inclusive).
    let at_start = evaluate_event_risk_blackout(&json, "AAPL", TS_START);
    assert!(
        matches!(at_start, EventRiskBlackoutOutcome::Blackout { .. }),
        "EG-04: signal at start_ts must be Blackout (inclusive); got: {at_start:?}"
    );

    // One second before start_ts — must be Authorized.
    let before_start = evaluate_event_risk_blackout(&json, "AAPL", TS_START - 1);
    assert_eq!(
        before_start,
        EventRiskBlackoutOutcome::Authorized,
        "EG-04: signal at start_ts−1 must be Authorized; got: {before_start:?}"
    );
}

// ---------------------------------------------------------------------------
// EG-05: Timestamp end boundary (inclusive)
//
// Proves: a signal at exactly `end_ts` is refused (inclusive upper bound);
// a signal at `end_ts + 1` is authorized.
// ---------------------------------------------------------------------------

#[test]
fn eg_05_timestamp_end_boundary() {
    let json = blackout_json("AAPL", TS_START, TS_END);

    // At end_ts — must be Blackout (inclusive).
    let at_end = evaluate_event_risk_blackout(&json, "AAPL", TS_END);
    assert!(
        matches!(at_end, EventRiskBlackoutOutcome::Blackout { .. }),
        "EG-05: signal at end_ts must be Blackout (inclusive); got: {at_end:?}"
    );

    // One second after end_ts — must be Authorized.
    let after_end = evaluate_event_risk_blackout(&json, "AAPL", TS_END + 1);
    assert_eq!(
        after_end,
        EventRiskBlackoutOutcome::Authorized,
        "EG-05: signal at end_ts+1 must be Authorized; got: {after_end:?}"
    );
}

// ---------------------------------------------------------------------------
// EG-06: Wrong schema_version → Unavailable (fail-closed)
//
// Proves: a blackout file with an unrecognized schema_version is not silently
// treated as authorizing or as NotConfigured — it fails closed with Unavailable.
// ---------------------------------------------------------------------------

#[test]
fn eg_06_wrong_schema_version_is_unavailable() {
    let json = r#"{"schema_version":"blackout-v2","periods":[]}"#;
    let outcome = evaluate_event_risk_blackout(json, "AAPL", TS_MID);
    assert!(
        matches!(outcome, EventRiskBlackoutOutcome::Unavailable { .. }),
        "EG-06: wrong schema_version must yield Unavailable; got: {outcome:?}"
    );
    assert!(
        !outcome.is_signal_safe(),
        "EG-06: Unavailable must not be signal-safe (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// EG-07: Malformed JSON → Unavailable (fail-closed)
//
// Proves: a blackout file that cannot be parsed fails closed rather than
// silently passing or returning NotConfigured.
// ---------------------------------------------------------------------------

#[test]
fn eg_07_malformed_json_is_unavailable() {
    let outcome = evaluate_event_risk_blackout("not valid json {{{", "AAPL", TS_MID);
    assert!(
        matches!(outcome, EventRiskBlackoutOutcome::Unavailable { .. }),
        "EG-07: malformed JSON must yield Unavailable; got: {outcome:?}"
    );
    assert!(
        !outcome.is_signal_safe(),
        "EG-07: Unavailable must not be signal-safe (fail-closed)"
    );
}

// ---------------------------------------------------------------------------
// EG-08: Multiple periods — only matching symbol+ts triggers Blackout
//
// Proves: with multiple periods in the file, only the one matching both
// symbol and timestamp produces Blackout; others do not interfere.
// ---------------------------------------------------------------------------

#[test]
fn eg_08_multiple_periods_correct_matching() {
    let json = r#"{
        "schema_version": "blackout-v1",
        "periods": [
            {"symbol": "AAPL", "start_ts": 1700000000, "end_ts": 1700086400},
            {"symbol": "MSFT", "start_ts": 1700100000, "end_ts": 1700186400},
            {"symbol": "SPY",  "start_ts": 1700200000, "end_ts": 1700286400}
        ]
    }"#;

    // AAPL at AAPL window → Blackout.
    assert!(
        matches!(
            evaluate_event_risk_blackout(json, "AAPL", 1_700_050_000),
            EventRiskBlackoutOutcome::Blackout { .. }
        ),
        "EG-08: AAPL at AAPL window must be Blackout"
    );

    // MSFT at AAPL window → Authorized (wrong symbol).
    assert_eq!(
        evaluate_event_risk_blackout(json, "MSFT", 1_700_050_000),
        EventRiskBlackoutOutcome::Authorized,
        "EG-08: MSFT at AAPL window must be Authorized"
    );

    // AAPL at MSFT window → Authorized (wrong symbol for that period).
    assert_eq!(
        evaluate_event_risk_blackout(json, "AAPL", 1_700_150_000),
        EventRiskBlackoutOutcome::Authorized,
        "EG-08: AAPL at MSFT window must be Authorized"
    );

    // SPY at SPY window → Blackout.
    assert!(
        matches!(
            evaluate_event_risk_blackout(json, "SPY", 1_700_250_000),
            EventRiskBlackoutOutcome::Blackout { .. }
        ),
        "EG-08: SPY at SPY window must be Blackout"
    );
}

// ---------------------------------------------------------------------------
// EG-09: is_signal_safe contract
//
// Proves: only NotConfigured and Authorized satisfy is_signal_safe.
// Blackout and Unavailable both fail closed.
// ---------------------------------------------------------------------------

#[test]
fn eg_09_is_signal_safe_contract() {
    assert!(
        EventRiskBlackoutOutcome::NotConfigured.is_signal_safe(),
        "EG-09: NotConfigured must be signal-safe"
    );
    assert!(
        EventRiskBlackoutOutcome::Authorized.is_signal_safe(),
        "EG-09: Authorized must be signal-safe"
    );
    assert!(
        !EventRiskBlackoutOutcome::Blackout {
            note: "test".to_string()
        }
        .is_signal_safe(),
        "EG-09: Blackout must not be signal-safe"
    );
    assert!(
        !EventRiskBlackoutOutcome::Unavailable {
            reason: "test".to_string()
        }
        .is_signal_safe(),
        "EG-09: Unavailable must not be signal-safe"
    );
}

// ---------------------------------------------------------------------------
// EG-10: blackout_surface_label without MQK_EVENT_RISK_BLACKOUT_PATH
//
// Proves: when the env var is not set (standard CI environment), the surface
// label is "not_configured" — distinct from "not_wired" (gate absent) and
// "configured" (gate enforcing).
// ---------------------------------------------------------------------------

#[test]
fn eg_10_surface_label_not_configured_without_env_var() {
    // This test relies on MQK_EVENT_RISK_BLACKOUT_PATH not being set in CI.
    // If the var is set, the label is "configured" — that is also correct.
    let label = blackout_surface_label();
    assert!(
        label == "not_configured" || label == "configured",
        "EG-10: surface label must be 'not_configured' or 'configured'; got: '{label}'"
    );
    // In standard CI (env var absent), must be "not_configured".
    if std::env::var("MQK_EVENT_RISK_BLACKOUT_PATH").is_err() {
        assert_eq!(
            label, "not_configured",
            "EG-10: without env var, surface label must be 'not_configured'"
        );
    }
}
