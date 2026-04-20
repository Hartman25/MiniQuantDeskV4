//! EVENT-RISK-FLATTEN-01: Pre-event flatten trigger detection — pure evaluator proof tests.
//!
//! ## What this proves
//!
//! `pre_event_flatten` was added by EVENT-RISK-FLATTEN-01.  It is the
//! *detection sub-seam only*: a pure evaluator that answers whether positions
//! in a symbol should be flattened given an upcoming or currently active
//! event-risk blackout window.
//!
//! The orchestrator wiring (sub-seam B) is NOT closed here.  The status surface
//! `/api/v1/execution/event-risk-status` continues to report
//! `pre_event_flattening = "not_wired"` — correct, because the orchestrator has
//! not yet been taught to call this evaluator or emit close orders.
//!
//! All tests are pure in-process: no DB, no network, no environment variables.
//!
//! ## Tests — blackout-v1 source
//!
//! - PF-01: NoFlattenRequired when period is far in the future (beyond lead window)
//! - PF-02: FlattenRequired when period starts within the lead window
//! - PF-03: FlattenRequired when currently inside an active blackout period
//! - PF-04: NoFlattenRequired when blackout period is entirely in the past
//! - PF-05: Symbol-specificity — another symbol's period does not affect the checked symbol
//! - PF-06: Lead boundary — start_ts == ts + lead_secs triggers; start_ts == ts + lead_secs + 1 does not
//!
//! ## Tests — earnings-calendar-v1 source
//!
//! - PF-07: NoFlattenRequired when earnings blackout starts beyond lead window
//! - PF-08: FlattenRequired when earnings blackout starts within lead window
//! - PF-09: FlattenRequired when currently inside active earnings blackout
//! - PF-10: Earnings symbol-specificity — another symbol's entry does not affect the checked symbol
//!
//! ## Tests — fail-closed and contract
//!
//! - PF-11: Wrong schema_version (blackout) → Unavailable (fail-closed)
//! - PF-12: Wrong schema_version (earnings) → Unavailable (fail-closed)
//! - PF-13: Malformed JSON (blackout) → Unavailable (fail-closed)
//! - PF-14: Malformed JSON (earnings) → Unavailable (fail-closed)
//! - PF-15: is_flatten_required / is_unavailable contract
//!
//! ## Tests — combined evaluator logic
//!
//! - PF-16: NotConfigured when both sources return NotConfigured
//! - PF-17: FlattenRequired wins over NoFlattenRequired from the other source
//! - PF-18: Unavailable does not suppress FlattenRequired (flatten wins)
//! - PF-19: Unavailable returned when source unreadable and no FlattenRequired found

use mqk_daemon::pre_event_flatten::{
    build_flatten_close_order_json, evaluate_flatten_trigger_from_blackout,
    evaluate_flatten_trigger_from_earnings, evaluate_flatten_trigger_from_env,
    FlattenTriggerOutcome, DEFAULT_FLATTEN_LEAD_SECS, FLATTEN_SIGNAL_SOURCE,
};

// ---------------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------------

/// Base timestamp used across tests.  Arbitrary epoch value.
const TS_NOW: i64 = 1_700_050_000;

/// Default lead used in most tests (matches DEFAULT_FLATTEN_LEAD_SECS = 1800s).
const LEAD: i64 = DEFAULT_FLATTEN_LEAD_SECS;

/// Build a minimal blackout-v1 JSON with one period for `symbol`.
fn blackout_json(symbol: &str, start_ts: i64, end_ts: i64) -> String {
    format!(
        r#"{{"schema_version":"blackout-v1","periods":[{{"symbol":"{symbol}","start_ts":{start_ts},"end_ts":{end_ts}}}]}}"#
    )
}

/// Build a minimal earnings-calendar-v1 JSON with one entry for `symbol`.
fn earnings_json(
    symbol: &str,
    earnings_ts: i64,
    pre_window_secs: i64,
    post_window_secs: i64,
) -> String {
    format!(
        r#"{{"schema_version":"earnings-calendar-v1","entries":[{{"symbol":"{symbol}","earnings_ts":{earnings_ts},"pre_window_secs":{pre_window_secs},"post_window_secs":{post_window_secs}}}]}}"#
    )
}

// ============================================================================
// Blackout-v1 evaluator tests (PF-01 … PF-06)
// ============================================================================

// ---------------------------------------------------------------------------
// PF-01: NoFlattenRequired when period is far in the future (beyond lead window)
//
// Proves: a period starting at ts + lead + 1 does NOT trigger flatten.
// The evaluator must not fire for events outside the lead window.
// ---------------------------------------------------------------------------

#[test]
fn pf_01_no_flatten_when_period_beyond_lead_window() {
    // Period starts one second after the lead window closes.
    let start = TS_NOW + LEAD + 1;
    let end = start + 3600;
    let json = blackout_json("AAPL", start, end);

    let outcome = evaluate_flatten_trigger_from_blackout(&json, "AAPL", TS_NOW, LEAD);
    assert_eq!(
        outcome,
        FlattenTriggerOutcome::NoFlattenRequired,
        "PF-01: period beyond lead window must yield NoFlattenRequired; got: {outcome:?}"
    );
    assert!(
        !outcome.is_flatten_required(),
        "PF-01: is_flatten_required must be false; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// PF-02: FlattenRequired when period starts within the lead window
//
// Proves: a period starting at ts + lead / 2 (well within the window) fires.
// ---------------------------------------------------------------------------

#[test]
fn pf_02_flatten_required_when_period_within_lead_window() {
    let start = TS_NOW + LEAD / 2; // within lead
    let end = start + 3600;
    let json = blackout_json("AAPL", start, end);

    let outcome = evaluate_flatten_trigger_from_blackout(&json, "AAPL", TS_NOW, LEAD);
    assert!(
        matches!(outcome, FlattenTriggerOutcome::FlattenRequired { .. }),
        "PF-02: period within lead window must yield FlattenRequired; got: {outcome:?}"
    );
    assert!(
        outcome.is_flatten_required(),
        "PF-02: is_flatten_required must be true"
    );
}

// ---------------------------------------------------------------------------
// PF-03: FlattenRequired when currently inside an active blackout period
//
// Proves: a period that started in the past and ends in the future fires.
// (Existing positions must close even though new signals are already blocked.)
// ---------------------------------------------------------------------------

#[test]
fn pf_03_flatten_required_when_in_active_blackout_period() {
    // Period started 1 hour ago, ends in 1 hour.
    let start = TS_NOW - 3600;
    let end = TS_NOW + 3600;
    let json = blackout_json("AAPL", start, end);

    let outcome = evaluate_flatten_trigger_from_blackout(&json, "AAPL", TS_NOW, LEAD);
    assert!(
        matches!(outcome, FlattenTriggerOutcome::FlattenRequired { .. }),
        "PF-03: active blackout period must yield FlattenRequired; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// PF-04: NoFlattenRequired when blackout period is entirely in the past
//
// Proves: an expired period does not trigger flatten.
// ---------------------------------------------------------------------------

#[test]
fn pf_04_no_flatten_when_period_entirely_in_past() {
    // Period ended 1 second ago.
    let start = TS_NOW - 7200;
    let end = TS_NOW - 1;
    let json = blackout_json("AAPL", start, end);

    let outcome = evaluate_flatten_trigger_from_blackout(&json, "AAPL", TS_NOW, LEAD);
    assert_eq!(
        outcome,
        FlattenTriggerOutcome::NoFlattenRequired,
        "PF-04: expired period must yield NoFlattenRequired; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// PF-05: Symbol-specificity — another symbol's period does not affect the checked symbol
//
// Proves: a period declared for "AAPL" does not trigger flatten for "MSFT"
// even when the timestamp falls inside the AAPL window.
// ---------------------------------------------------------------------------

#[test]
fn pf_05_symbol_specificity_blackout() {
    let start = TS_NOW - 100;
    let end = TS_NOW + 3600;
    let json = blackout_json("AAPL", start, end); // period is for AAPL only

    let outcome = evaluate_flatten_trigger_from_blackout(&json, "MSFT", TS_NOW, LEAD);
    assert_eq!(
        outcome,
        FlattenTriggerOutcome::NoFlattenRequired,
        "PF-05: AAPL period must not trigger flatten for MSFT; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// PF-06: Lead boundary — inclusive at exactly ts + lead_secs; exclusive at ts + lead_secs + 1
//
// Proves the upper bound of the lead window is inclusive:
//   start_ts == ts + lead_secs    → FlattenRequired
//   start_ts == ts + lead_secs + 1 → NoFlattenRequired
// ---------------------------------------------------------------------------

#[test]
fn pf_06_lead_boundary_inclusive() {
    // Exactly at the lead boundary → FlattenRequired.
    let at_boundary = TS_NOW + LEAD;
    let end = at_boundary + 3600;
    let json_at = blackout_json("AAPL", at_boundary, end);
    let outcome_at = evaluate_flatten_trigger_from_blackout(&json_at, "AAPL", TS_NOW, LEAD);
    assert!(
        matches!(outcome_at, FlattenTriggerOutcome::FlattenRequired { .. }),
        "PF-06: start_ts == ts + lead_secs must be FlattenRequired; got: {outcome_at:?}"
    );

    // One second past the lead boundary → NoFlattenRequired.
    let past_boundary = TS_NOW + LEAD + 1;
    let json_past = blackout_json("AAPL", past_boundary, past_boundary + 3600);
    let outcome_past = evaluate_flatten_trigger_from_blackout(&json_past, "AAPL", TS_NOW, LEAD);
    assert_eq!(
        outcome_past,
        FlattenTriggerOutcome::NoFlattenRequired,
        "PF-06: start_ts == ts + lead_secs + 1 must be NoFlattenRequired; got: {outcome_past:?}"
    );
}

// ============================================================================
// Earnings-calendar-v1 evaluator tests (PF-07 … PF-10)
// ============================================================================

// ---------------------------------------------------------------------------
// PF-07: NoFlattenRequired when earnings blackout starts beyond lead window
//
// Earnings blackout start = earnings_ts - pre_window_secs.
// If that start is more than lead_secs away from ts, no flatten needed.
// ---------------------------------------------------------------------------

#[test]
fn pf_07_no_flatten_when_earnings_blackout_beyond_lead() {
    // blackout_start = earnings_ts - pre_window = TS_NOW + LEAD + 1
    let pre_window = 3600_i64;
    let earnings_ts = TS_NOW + LEAD + 1 + pre_window;
    let json = earnings_json("AAPL", earnings_ts, pre_window, 1800);

    let outcome = evaluate_flatten_trigger_from_earnings(&json, "AAPL", TS_NOW, LEAD);
    assert_eq!(
        outcome,
        FlattenTriggerOutcome::NoFlattenRequired,
        "PF-07: earnings blackout beyond lead must yield NoFlattenRequired; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// PF-08: FlattenRequired when earnings blackout starts within lead window
//
// blackout_start = earnings_ts - pre_window_secs is within (ts, ts + lead_secs].
// ---------------------------------------------------------------------------

#[test]
fn pf_08_flatten_required_when_earnings_blackout_within_lead() {
    // blackout_start = TS_NOW + LEAD / 2 (well within lead)
    let pre_window = 3600_i64;
    let earnings_ts = TS_NOW + LEAD / 2 + pre_window;
    let json = earnings_json("AAPL", earnings_ts, pre_window, 1800);

    let outcome = evaluate_flatten_trigger_from_earnings(&json, "AAPL", TS_NOW, LEAD);
    assert!(
        matches!(outcome, FlattenTriggerOutcome::FlattenRequired { .. }),
        "PF-08: earnings blackout within lead must yield FlattenRequired; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// PF-09: FlattenRequired when currently inside active earnings blackout
//
// ts is within [earnings_ts - pre_window, earnings_ts + post_window].
// ---------------------------------------------------------------------------

#[test]
fn pf_09_flatten_required_when_in_active_earnings_blackout() {
    // TS_NOW is between earnings_ts - pre_window and earnings_ts + post_window.
    let earnings_ts = TS_NOW + 1800; // earnings in 30 minutes
    let pre_window = 7200_i64; // pre-window = 2 hours → blackout started 90 min ago
    let post_window = 3600_i64;
    let json = earnings_json("AAPL", earnings_ts, pre_window, post_window);

    // Verify TS_NOW is inside the window.
    let blackout_start = earnings_ts - pre_window;
    let blackout_end = earnings_ts + post_window;
    assert!(
        TS_NOW >= blackout_start && TS_NOW <= blackout_end,
        "fixture check: TS_NOW must be inside earnings blackout"
    );

    let outcome = evaluate_flatten_trigger_from_earnings(&json, "AAPL", TS_NOW, LEAD);
    assert!(
        matches!(outcome, FlattenTriggerOutcome::FlattenRequired { .. }),
        "PF-09: active earnings blackout must yield FlattenRequired; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// PF-10: Earnings symbol-specificity
//
// Proves: an earnings entry for "AAPL" does not trigger flatten for "MSFT".
// ---------------------------------------------------------------------------

#[test]
fn pf_10_symbol_specificity_earnings() {
    let earnings_ts = TS_NOW + LEAD / 2 + 3600; // would trigger for AAPL
    let json = earnings_json("AAPL", earnings_ts, 3600, 1800);

    let outcome = evaluate_flatten_trigger_from_earnings(&json, "MSFT", TS_NOW, LEAD);
    assert_eq!(
        outcome,
        FlattenTriggerOutcome::NoFlattenRequired,
        "PF-10: AAPL earnings entry must not trigger flatten for MSFT; got: {outcome:?}"
    );
}

// ============================================================================
// Fail-closed and contract tests (PF-11 … PF-15)
// ============================================================================

// ---------------------------------------------------------------------------
// PF-11: Wrong schema_version (blackout) → Unavailable
//
// Proves: an unrecognized schema_version fails closed — not silently passed.
// ---------------------------------------------------------------------------

#[test]
fn pf_11_wrong_schema_version_blackout_is_unavailable() {
    let json = r#"{"schema_version":"blackout-v2","periods":[]}"#;
    let outcome = evaluate_flatten_trigger_from_blackout(json, "AAPL", TS_NOW, LEAD);
    assert!(
        matches!(outcome, FlattenTriggerOutcome::Unavailable { .. }),
        "PF-11: wrong schema_version must yield Unavailable; got: {outcome:?}"
    );
    assert!(
        outcome.is_unavailable(),
        "PF-11: is_unavailable must be true"
    );
    assert!(
        !outcome.is_flatten_required(),
        "PF-11: is_flatten_required must be false for Unavailable"
    );
}

// ---------------------------------------------------------------------------
// PF-12: Wrong schema_version (earnings) → Unavailable
// ---------------------------------------------------------------------------

#[test]
fn pf_12_wrong_schema_version_earnings_is_unavailable() {
    let json = r#"{"schema_version":"earnings-calendar-v2","entries":[]}"#;
    let outcome = evaluate_flatten_trigger_from_earnings(json, "AAPL", TS_NOW, LEAD);
    assert!(
        matches!(outcome, FlattenTriggerOutcome::Unavailable { .. }),
        "PF-12: wrong schema_version must yield Unavailable; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// PF-13: Malformed JSON (blackout) → Unavailable
// ---------------------------------------------------------------------------

#[test]
fn pf_13_malformed_json_blackout_is_unavailable() {
    let outcome = evaluate_flatten_trigger_from_blackout("{{not valid json", "AAPL", TS_NOW, LEAD);
    assert!(
        matches!(outcome, FlattenTriggerOutcome::Unavailable { .. }),
        "PF-13: malformed JSON must yield Unavailable; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// PF-14: Malformed JSON (earnings) → Unavailable
// ---------------------------------------------------------------------------

#[test]
fn pf_14_malformed_json_earnings_is_unavailable() {
    let outcome = evaluate_flatten_trigger_from_earnings("{{not valid json", "AAPL", TS_NOW, LEAD);
    assert!(
        matches!(outcome, FlattenTriggerOutcome::Unavailable { .. }),
        "PF-14: malformed JSON must yield Unavailable; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// PF-15: is_flatten_required / is_unavailable contract
//
// Proves the contract methods on FlattenTriggerOutcome are consistent with
// expected operator behavior.
// ---------------------------------------------------------------------------

#[test]
fn pf_15_outcome_contract() {
    assert!(
        !FlattenTriggerOutcome::NotConfigured.is_flatten_required(),
        "PF-15: NotConfigured must not be flatten-required"
    );
    assert!(
        !FlattenTriggerOutcome::NotConfigured.is_unavailable(),
        "PF-15: NotConfigured must not be unavailable"
    );

    assert!(
        !FlattenTriggerOutcome::NoFlattenRequired.is_flatten_required(),
        "PF-15: NoFlattenRequired must not be flatten-required"
    );
    assert!(
        !FlattenTriggerOutcome::NoFlattenRequired.is_unavailable(),
        "PF-15: NoFlattenRequired must not be unavailable"
    );

    let flatten = FlattenTriggerOutcome::FlattenRequired {
        note: "test".to_string(),
    };
    assert!(
        flatten.is_flatten_required(),
        "PF-15: FlattenRequired must be flatten-required"
    );
    assert!(
        !flatten.is_unavailable(),
        "PF-15: FlattenRequired must not be unavailable"
    );

    let unavail = FlattenTriggerOutcome::Unavailable {
        reason: "test".to_string(),
    };
    assert!(
        !unavail.is_flatten_required(),
        "PF-15: Unavailable must not be flatten-required"
    );
    assert!(
        unavail.is_unavailable(),
        "PF-15: Unavailable must be unavailable"
    );
}

// ============================================================================
// Combined evaluator logic tests (PF-16 … PF-19)
//
// evaluate_flatten_trigger_from_env reads both env vars.  These tests inject
// env vars pointing at temp files with controlled JSON content.
// Run with --test-threads=1 to avoid env-var races.
// ============================================================================

// ---------------------------------------------------------------------------
// PF-16: NotConfigured when both env vars are absent
//
// Proves: with no sources configured, the combined evaluator returns
// NotConfigured — not NoFlattenRequired.  These are semantically different:
// NotConfigured means no enforcement; NoFlattenRequired means checked-and-clear.
// ---------------------------------------------------------------------------

#[test]
fn pf_16_not_configured_when_no_env_vars() {
    // Remove both env vars (in case a prior test left them set).
    std::env::remove_var("MQK_EVENT_RISK_BLACKOUT_PATH");
    std::env::remove_var("MQK_EARNINGS_CALENDAR_PATH");

    let outcome = evaluate_flatten_trigger_from_env("AAPL", TS_NOW, LEAD);
    assert_eq!(
        outcome,
        FlattenTriggerOutcome::NotConfigured,
        "PF-16: with no env vars, combined evaluator must return NotConfigured; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// PF-17: FlattenRequired from one source wins over NoFlattenRequired from the other
//
// Proves: when one source says FlattenRequired and the other says
// NoFlattenRequired (or NotConfigured), the combined result is FlattenRequired.
// ---------------------------------------------------------------------------

#[test]
fn pf_17_flatten_required_wins_over_no_flatten() {
    use std::io::Write;

    // Blackout source: period starts within lead window → FlattenRequired.
    let blackout_start = TS_NOW + LEAD / 2;
    let blackout_json_content = blackout_json("AAPL", blackout_start, blackout_start + 7200);
    let mut blackout_file = tempfile::NamedTempFile::new().expect("tempfile");
    blackout_file
        .write_all(blackout_json_content.as_bytes())
        .expect("write");
    let blackout_path = blackout_file.path().to_string_lossy().to_string();

    // Earnings source: no entries for AAPL → NoFlattenRequired.
    let earnings_json_content = r#"{"schema_version":"earnings-calendar-v1","entries":[]}"#;
    let mut earnings_file = tempfile::NamedTempFile::new().expect("tempfile");
    earnings_file
        .write_all(earnings_json_content.as_bytes())
        .expect("write");
    let earnings_path = earnings_file.path().to_string_lossy().to_string();

    std::env::set_var("MQK_EVENT_RISK_BLACKOUT_PATH", &blackout_path);
    std::env::set_var("MQK_EARNINGS_CALENDAR_PATH", &earnings_path);

    let outcome = evaluate_flatten_trigger_from_env("AAPL", TS_NOW, LEAD);

    std::env::remove_var("MQK_EVENT_RISK_BLACKOUT_PATH");
    std::env::remove_var("MQK_EARNINGS_CALENDAR_PATH");

    assert!(
        matches!(outcome, FlattenTriggerOutcome::FlattenRequired { .. }),
        "PF-17: FlattenRequired from blackout source must win; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// PF-18: FlattenRequired wins even when the other source is Unavailable
//
// Proves: if one source returns FlattenRequired, the combined result is
// FlattenRequired even if the other source is misconfigured/unreadable.
// Rationale: we already know flattening is required; the unreadable source
// cannot override that.
// ---------------------------------------------------------------------------

#[test]
fn pf_18_flatten_required_wins_over_unavailable() {
    use std::io::Write;

    // Blackout source: period starts within lead window → FlattenRequired.
    let blackout_start = TS_NOW + LEAD / 2;
    let blackout_json_content = blackout_json("AAPL", blackout_start, blackout_start + 7200);
    let mut blackout_file = tempfile::NamedTempFile::new().expect("tempfile");
    blackout_file
        .write_all(blackout_json_content.as_bytes())
        .expect("write");
    let blackout_path = blackout_file.path().to_string_lossy().to_string();

    // Earnings source: path set but file does not exist → Unavailable.
    let bad_earnings_path = "/tmp/mqk_pf18_nonexistent_earnings_calendar.json";

    std::env::set_var("MQK_EVENT_RISK_BLACKOUT_PATH", &blackout_path);
    std::env::set_var("MQK_EARNINGS_CALENDAR_PATH", bad_earnings_path);

    let outcome = evaluate_flatten_trigger_from_env("AAPL", TS_NOW, LEAD);

    std::env::remove_var("MQK_EVENT_RISK_BLACKOUT_PATH");
    std::env::remove_var("MQK_EARNINGS_CALENDAR_PATH");

    assert!(
        matches!(outcome, FlattenTriggerOutcome::FlattenRequired { .. }),
        "PF-18: FlattenRequired must win over Unavailable from the other source; got: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// PF-19: Unavailable returned when source is unreadable and no FlattenRequired
//
// Proves: if a source is configured but unreadable and neither source found
// FlattenRequired, the combined evaluator returns Unavailable (fail-closed).
// The operator cannot assume it is safe to hold.
// ---------------------------------------------------------------------------

#[test]
fn pf_19_unavailable_when_source_unreadable_and_no_flatten_found() {
    // Earnings source: path set but file does not exist → Unavailable.
    let bad_path = "/tmp/mqk_pf19_nonexistent_earnings.json";
    std::env::remove_var("MQK_EVENT_RISK_BLACKOUT_PATH");
    std::env::set_var("MQK_EARNINGS_CALENDAR_PATH", bad_path);

    let outcome = evaluate_flatten_trigger_from_env("AAPL", TS_NOW, LEAD);

    std::env::remove_var("MQK_EARNINGS_CALENDAR_PATH");

    assert!(
        matches!(outcome, FlattenTriggerOutcome::Unavailable { .. }),
        "PF-19: unreadable source (no FlattenRequired) must yield Unavailable; got: {outcome:?}"
    );
}

// ============================================================================
// Close-order construction tests (PF-20 … PF-24)
//
// EVENT-RISK-FLATTEN-WIRE-01: prove build_flatten_close_order_json is correct
// and deterministic before it is called from the execution loop.
//
// All tests are pure: no DB, no network, no file I/O.
// ============================================================================

const RUN_ID: uuid::Uuid = uuid::Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10_u128);

// ---------------------------------------------------------------------------
// PF-20: long position → market sell close order
//
// Proves: a positive net_qty produces side="sell" and qty = abs(net_qty).
// ---------------------------------------------------------------------------

#[test]
fn pf_20_long_position_produces_sell_close_order() {
    let (key, json) = build_flatten_close_order_json("AAPL", 100, TS_NOW, RUN_ID);

    assert_eq!(json["symbol"], "AAPL", "PF-20: symbol must be AAPL");
    assert_eq!(
        json["side"], "sell",
        "PF-20: long position must produce side=sell"
    );
    assert_eq!(json["qty"], 100, "PF-20: qty must equal abs(net_qty)");
    assert_eq!(
        json["order_type"], "market",
        "PF-20: order_type must be market"
    );
    assert_eq!(
        json["signal_source"], FLATTEN_SIGNAL_SOURCE,
        "PF-20: signal_source must be FLATTEN_SIGNAL_SOURCE"
    );
    assert_eq!(
        json["request_type"], "submit",
        "PF-20: request_type must be submit"
    );
    assert!(!key.is_empty(), "PF-20: idempotency_key must be non-empty");
}

// ---------------------------------------------------------------------------
// PF-21: short position → market buy close order
//
// Proves: a negative net_qty produces side="buy" and qty = abs(net_qty).
// ---------------------------------------------------------------------------

#[test]
fn pf_21_short_position_produces_buy_close_order() {
    let (key, json) = build_flatten_close_order_json("MSFT", -50, TS_NOW, RUN_ID);

    assert_eq!(json["symbol"], "MSFT", "PF-21: symbol must be MSFT");
    assert_eq!(
        json["side"], "buy",
        "PF-21: short position must produce side=buy"
    );
    assert_eq!(json["qty"], 50, "PF-21: qty must equal abs(net_qty)");
    assert_eq!(
        json["order_type"], "market",
        "PF-21: order_type must be market"
    );
    assert!(!key.is_empty(), "PF-21: idempotency_key must be non-empty");
}

// ---------------------------------------------------------------------------
// PF-22: idempotency key is deterministic — same inputs produce same key
//
// Proves: no randomness in key derivation; a second call within the same minute
// with the same arguments produces an identical key (crash-safe retry).
// ---------------------------------------------------------------------------

#[test]
fn pf_22_idempotency_key_is_deterministic() {
    let (key_a, _) = build_flatten_close_order_json("AAPL", 100, TS_NOW, RUN_ID);
    let (key_b, _) = build_flatten_close_order_json("AAPL", 100, TS_NOW, RUN_ID);
    assert_eq!(
        key_a, key_b,
        "PF-22: same inputs must produce the same idempotency key"
    );
}

// ---------------------------------------------------------------------------
// PF-23: different minute → different key (allows new order after position re-entry)
//
// Proves: ts_secs in consecutive minutes produce distinct keys so a new close
// order can be enqueued after the previous one has been dispatched/consumed.
// ---------------------------------------------------------------------------

#[test]
fn pf_23_different_minute_produces_different_key() {
    // TS_NOW and TS_NOW + 60 are in different minute buckets.
    let ts_minute_1 = TS_NOW;
    let ts_minute_2 = TS_NOW + 60;
    assert_ne!(
        ts_minute_1 / 60,
        ts_minute_2 / 60,
        "fixture check: timestamps must be in different minutes"
    );
    let (key_a, _) = build_flatten_close_order_json("AAPL", 100, ts_minute_1, RUN_ID);
    let (key_b, _) = build_flatten_close_order_json("AAPL", 100, ts_minute_2, RUN_ID);
    assert_ne!(
        key_a, key_b,
        "PF-23: different minutes must produce different keys"
    );
}

// ---------------------------------------------------------------------------
// PF-24: different symbols in the same minute produce different keys
//
// Proves: symbol is part of the key — two simultaneous flatten triggers for
// different symbols do not collide.
// ---------------------------------------------------------------------------

#[test]
fn pf_24_different_symbols_produce_different_keys() {
    let (key_aapl, _) = build_flatten_close_order_json("AAPL", 100, TS_NOW, RUN_ID);
    let (key_msft, _) = build_flatten_close_order_json("MSFT", 100, TS_NOW, RUN_ID);
    assert_ne!(
        key_aapl, key_msft,
        "PF-24: different symbols must produce different idempotency keys"
    );
}
