//! SHORT-SIDE-FLATTEN-PROOF-01 — Proves flatten close order construction is
//! short-position-aware.
//!
//! Calls `build_flatten_close_order_json` and
//! `build_operator_flatten_close_order_json` directly (pure) to prove:
//!
//! - A long position produces a sell order for the same absolute quantity.
//! - A short position produces a buy order for the absolute quantity (buy-to-cover).
//! - The absolute-quantity invariant holds in both cases (qty > 0 always).
//! - Short flatten never emits sell side; long flatten never emits buy side.
//! - Signal sources are `"pre_event_flatten"` / `"operator_flatten"` as appropriate.
//! - Idempotency key is deterministic: same inputs → same key (UUIDv5, no randomness).
//! - The operator-flatten variant mirrors the same sign logic.
//!
//! Also proves the intent-classification seam for the flatten use case:
//! - `current = -N, delta = +N` → `BuyToFlat` (not `ShortOpen`).
//! - `BuyToFlat` does not require short-entry policy authorization.
//! - `current = -N, delta < N` → `BuyToCover` → `NotShortOpen`.
//!
//! All tests are pure in-process (no DB, no IO, no network, no broker routes).
//!
//! # Safety invariants enforced in this file
//!
//! - Short trading is NOT enabled; no strategy emits short signals.
//! - B5 guard remains intact (no changes to `bar_result_to_decisions`).
//! - No broker submit code is called.
//! - No DB migrations; no `.env.local` modifications.
//!
//! # Test inventory
//!
//! | ID   | Scenario                                                                    |
//! |------|-----------------------------------------------------------------------------|
//! | SF01 | Long position (+N) flatten → side=sell, qty=N                              |
//! | SF02 | Short position (-N) flatten → side=buy, qty=N (buy-to-cover)               |
//! | SF03 | Short flatten qty is abs value (positive, never negative)                   |
//! | SF04 | Short flatten does NOT emit sell side                                       |
//! | SF05 | Long flatten does NOT emit buy side                                         |
//! | SF06 | Idempotency key is deterministic (same inputs → same key; no new_v4)        |
//! | SF07 | operator_flatten variant handles short position identically                  |
//! | SF08 | Signal source fields are correct for pre_event and operator flatten          |
//! | SF09 | classify_order_intent: current=-N, delta=+N → BuyToFlat (not ShortOpen)     |
//! | SF10 | BuyToFlat does not require short-entry policy authorization → NotShortOpen   |
//! | SF11 | Partial short cover → BuyToCover → NotShortOpen (risk-reducing)             |
//! | SF12 | Zero qty is degenerate: callers must not call with zero (documents contract)  |

use uuid::Uuid;

use mqk_daemon::capital_policy::{
    classify_order_intent, evaluate_short_entry_policy, order_intent_to_short_entry_intent,
    OrderIntent, ShortEntryConfig, ShortEntryPolicyDecision,
};
use mqk_daemon::pre_event_flatten::{
    build_flatten_close_order_json, build_operator_flatten_close_order_json, FLATTEN_SIGNAL_SOURCE,
    OPERATOR_FLATTEN_SIGNAL_SOURCE,
};

fn fixed_run_id() -> Uuid {
    Uuid::nil()
}

const FIXED_TS_SECS: i64 = 1_750_000_000;

// ---------------------------------------------------------------------------
// SF01 — Long position (+N) flatten → side=sell, qty=N
// ---------------------------------------------------------------------------

#[test]
fn sf01_flatten_long_emits_sell() {
    let (_, order) = build_flatten_close_order_json("AAPL", 100, FIXED_TS_SECS, fixed_run_id());
    assert_eq!(
        order["side"].as_str(),
        Some("sell"),
        "SF01: long position (+100) must produce side=sell; got: {order}"
    );
    assert_eq!(
        order["qty"].as_i64(),
        Some(100),
        "SF01: qty must equal the long position size (100); got: {order}"
    );
}

// ---------------------------------------------------------------------------
// SF02 — Short position (-N) flatten → side=buy, qty=N (buy-to-cover)
// ---------------------------------------------------------------------------

#[test]
fn sf02_flatten_short_emits_buy_to_cover() {
    let (_, order) = build_flatten_close_order_json("AAPL", -50, FIXED_TS_SECS, fixed_run_id());
    assert_eq!(
        order["side"].as_str(),
        Some("buy"),
        "SF02: short position (-50) must produce side=buy (buy-to-cover); got: {order}"
    );
    assert_eq!(
        order["qty"].as_i64(),
        Some(50),
        "SF02: qty must equal abs(short position) = 50; got: {order}"
    );
}

// ---------------------------------------------------------------------------
// SF03 — Short flatten qty is abs value (always positive, never negative)
// ---------------------------------------------------------------------------

#[test]
fn sf03_flatten_short_uses_abs_qty() {
    let (_, order) = build_flatten_close_order_json("SPY", -200, FIXED_TS_SECS, fixed_run_id());
    let qty = order["qty"]
        .as_i64()
        .expect("SF03: qty field must be present");
    assert!(
        qty > 0,
        "SF03: flatten qty must always be positive (abs of signed qty); got qty={qty}"
    );
    assert_eq!(qty, 200, "SF03: qty must be abs(-200) = 200; got: {order}");
}

// ---------------------------------------------------------------------------
// SF04 — Short flatten does NOT emit sell side
// ---------------------------------------------------------------------------

#[test]
fn sf04_flatten_short_never_emits_sell() {
    for signed_qty in &[-1_i64, -75, -500] {
        let (_, order) =
            build_flatten_close_order_json("TSLA", *signed_qty, FIXED_TS_SECS, fixed_run_id());
        assert_ne!(
            order["side"].as_str(),
            Some("sell"),
            "SF04: short position ({signed_qty}) flatten must never emit sell side \
             (sell would increase short exposure, not reduce it); got: {order}"
        );
        assert_eq!(
            order["side"].as_str(),
            Some("buy"),
            "SF04: short position ({signed_qty}) flatten must emit buy; got: {order}"
        );
    }
}

// ---------------------------------------------------------------------------
// SF05 — Long flatten does NOT emit buy side
// ---------------------------------------------------------------------------

#[test]
fn sf05_flatten_long_never_emits_buy() {
    for signed_qty in &[1_i64, 50, 1000] {
        let (_, order) =
            build_flatten_close_order_json("MSFT", *signed_qty, FIXED_TS_SECS, fixed_run_id());
        assert_ne!(
            order["side"].as_str(),
            Some("buy"),
            "SF05: long position ({signed_qty}) flatten must never emit buy side \
             (buy would increase long exposure, not reduce it); got: {order}"
        );
        assert_eq!(
            order["side"].as_str(),
            Some("sell"),
            "SF05: long position ({signed_qty}) flatten must emit sell; got: {order}"
        );
    }
}

// ---------------------------------------------------------------------------
// SF06 — Idempotency key is deterministic (same inputs → same key; no new_v4)
// ---------------------------------------------------------------------------

#[test]
fn sf06_flatten_idempotency_key_is_deterministic() {
    // Same inputs for a short position → same key on repeated calls.
    let (key1, _) = build_flatten_close_order_json("AAPL", -50, FIXED_TS_SECS, fixed_run_id());
    let (key2, _) = build_flatten_close_order_json("AAPL", -50, FIXED_TS_SECS, fixed_run_id());
    assert_eq!(
        key1, key2,
        "SF06: same inputs must produce the same idempotency key (UUIDv5, not random new_v4)"
    );

    // Same inputs for a long position → same key on repeated calls.
    let (key_long1, _) = build_flatten_close_order_json("AAPL", 50, FIXED_TS_SECS, fixed_run_id());
    let (key_long2, _) = build_flatten_close_order_json("AAPL", 50, FIXED_TS_SECS, fixed_run_id());
    assert_eq!(
        key_long1, key_long2,
        "SF06: long flatten same inputs must also produce the same idempotency key"
    );

    // Different symbol → different key.
    let (key_spy, _) = build_flatten_close_order_json("SPY", -50, FIXED_TS_SECS, fixed_run_id());
    assert_ne!(
        key1, key_spy,
        "SF06: AAPL(-50) and SPY(-50) must produce different idempotency keys"
    );

    // Different run_id → different key.
    let alt_run = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"sf06-alt-run");
    let (key_alt_run, _) = build_flatten_close_order_json("AAPL", -50, FIXED_TS_SECS, alt_run);
    assert_ne!(
        key1, key_alt_run,
        "SF06: different run_id must produce different idempotency key"
    );

    // Different minute bucket → different key.
    let next_minute_ts = FIXED_TS_SECS + 60; // advances to the next minute bucket
    let (key_later, _) =
        build_flatten_close_order_json("AAPL", -50, next_minute_ts, fixed_run_id());
    assert_ne!(
        key1, key_later,
        "SF06: different minute bucket must produce different idempotency key"
    );
}

// ---------------------------------------------------------------------------
// SF07 — operator_flatten variant handles short position identically
// ---------------------------------------------------------------------------

#[test]
fn sf07_operator_flatten_short_emits_buy_with_abs_qty() {
    let (_, order) =
        build_operator_flatten_close_order_json("NVDA", -30, FIXED_TS_SECS, fixed_run_id());
    assert_eq!(
        order["side"].as_str(),
        Some("buy"),
        "SF07: operator_flatten short position (-30) must produce side=buy; got: {order}"
    );
    assert_eq!(
        order["qty"].as_i64(),
        Some(30),
        "SF07: operator_flatten qty must be abs(-30) = 30; got: {order}"
    );
    // Operator flatten key must differ from pre-event flatten key for the same inputs
    // (different namespace: "mqk-op-flatten.v1." vs "mqk-flatten.v1.").
    let (pre_key, _) = build_flatten_close_order_json("NVDA", -30, FIXED_TS_SECS, fixed_run_id());
    let (op_key, _) =
        build_operator_flatten_close_order_json("NVDA", -30, FIXED_TS_SECS, fixed_run_id());
    assert_ne!(
        pre_key, op_key,
        "SF07: operator_flatten and pre_event_flatten must produce different idempotency keys \
         (distinct namespaces prevent key collision)"
    );
}

// ---------------------------------------------------------------------------
// SF08 — Signal source fields are correct for pre_event and operator flatten
// ---------------------------------------------------------------------------

#[test]
fn sf08_signal_source_correct_for_short_flatten() {
    // pre_event_flatten path for short position.
    let (_, pre_order) = build_flatten_close_order_json("AAPL", -40, FIXED_TS_SECS, fixed_run_id());
    assert_eq!(
        pre_order["signal_source"].as_str(),
        Some(FLATTEN_SIGNAL_SOURCE),
        "SF08: pre_event flatten signal_source must be '{}'; got: {pre_order}",
        FLATTEN_SIGNAL_SOURCE
    );
    assert_eq!(
        FLATTEN_SIGNAL_SOURCE, "pre_event_flatten",
        "SF08: FLATTEN_SIGNAL_SOURCE constant value check"
    );

    // operator_flatten path for short position.
    let (_, op_order) =
        build_operator_flatten_close_order_json("AAPL", -40, FIXED_TS_SECS, fixed_run_id());
    assert_eq!(
        op_order["signal_source"].as_str(),
        Some(OPERATOR_FLATTEN_SIGNAL_SOURCE),
        "SF08: operator flatten signal_source must be '{}'; got: {op_order}",
        OPERATOR_FLATTEN_SIGNAL_SOURCE
    );
    assert_eq!(
        OPERATOR_FLATTEN_SIGNAL_SOURCE, "operator_flatten",
        "SF08: OPERATOR_FLATTEN_SIGNAL_SOURCE constant value check"
    );

    // Both must use market orders (not limit) — flatten is always market.
    assert_eq!(
        pre_order["order_type"].as_str(),
        Some("market"),
        "SF08: pre_event flatten must use market order; got: {pre_order}"
    );
    assert_eq!(
        op_order["order_type"].as_str(),
        Some("market"),
        "SF08: operator flatten must use market order; got: {op_order}"
    );
}

// ---------------------------------------------------------------------------
// SF09 — classify_order_intent: current=-N, delta=+N → BuyToFlat (not ShortOpen)
// ---------------------------------------------------------------------------

#[test]
fn sf09_buy_to_flat_from_short_is_not_short_open() {
    // Exact short cover: current=-50, delta=+50 → BuyToFlat (closes short exactly).
    let intent = classify_order_intent(-50, 50);
    assert_eq!(
        intent,
        OrderIntent::BuyToFlat,
        "SF09: current=-50, delta=+50 must classify as BuyToFlat (exact short cover); \
         got: {intent:?}"
    );
    assert!(
        !intent.requires_short_entry_policy(),
        "SF09: BuyToFlat must not require short-entry policy (it is risk-reducing, not short-open)"
    );

    // Larger short: current=-200, delta=+200 → BuyToFlat.
    let intent_large = classify_order_intent(-200, 200);
    assert_eq!(
        intent_large,
        OrderIntent::BuyToFlat,
        "SF09: current=-200, delta=+200 must also be BuyToFlat; got: {intent_large:?}"
    );
    assert!(
        !intent_large.requires_short_entry_policy(),
        "SF09: BuyToFlat for large short must not require short-entry policy"
    );
}

// ---------------------------------------------------------------------------
// SF10 — BuyToFlat does not require short-entry policy authorization → NotShortOpen
// ---------------------------------------------------------------------------

#[test]
fn sf10_buy_to_flat_passes_policy_gate_as_not_short_open() {
    // Use the fail-closed default policy (strictest possible config).
    let policy = ShortEntryConfig::fail_closed();

    let intent = classify_order_intent(-50, 50); // BuyToFlat
    let short_entry_intent = order_intent_to_short_entry_intent(intent);
    let decision = evaluate_short_entry_policy(
        &policy,
        short_entry_intent,
        true, // paper mode
        None,
        None,
        None,
    );

    assert_eq!(
        decision,
        ShortEntryPolicyDecision::NotShortOpen,
        "SF10: BuyToFlat intent through fail-closed policy must return NotShortOpen \
         (gate does not apply to risk-reducing cover orders); got: {decision:?}"
    );
    assert!(
        !decision.is_short_open_allowed(),
        "SF10: NotShortOpen must not report is_short_open_allowed=true \
         (cover is not a short-open permission, it is a non-applicable gate)"
    );
}

// ---------------------------------------------------------------------------
// SF11 — Partial short cover → BuyToCover → NotShortOpen (risk-reducing)
// ---------------------------------------------------------------------------

#[test]
fn sf11_partial_short_cover_is_buy_to_cover_not_short_entry() {
    // Partial cover: current=-100, delta=+40 → stays short at -60.
    let intent = classify_order_intent(-100, 40);
    assert_eq!(
        intent,
        OrderIntent::BuyToCover,
        "SF11: partial cover (current=-100, delta=+40) must be BuyToCover; got: {intent:?}"
    );
    assert!(
        !intent.requires_short_entry_policy(),
        "SF11: BuyToCover must not require short-entry policy (risk-reducing partial cover)"
    );

    let short_entry_intent = order_intent_to_short_entry_intent(intent);
    let decision = evaluate_short_entry_policy(
        &ShortEntryConfig::fail_closed(),
        short_entry_intent,
        true, // paper mode
        None,
        None,
        None,
    );
    assert_eq!(
        decision,
        ShortEntryPolicyDecision::NotShortOpen,
        "SF11: BuyToCover through fail-closed policy must return NotShortOpen; got: {decision:?}"
    );
}

// ---------------------------------------------------------------------------
// SF12 — Zero qty is degenerate: documents caller contract (must not pass zero)
// ---------------------------------------------------------------------------

#[test]
fn sf12_zero_qty_is_degenerate_caller_contract() {
    // The function does not guard against zero — this is a caller contract.
    // A zero-qty flatten produces a market buy for 0 shares (from the else branch
    // when net_qty_signed == 0), which would be rejected by the broker at submit time.
    // Callers in the loop must check net_qty_signed != 0 before calling.
    //
    // This test documents the degenerate behavior without asserting it is "safe" —
    // it is the caller's responsibility to skip flat positions.
    let (_, order) = build_flatten_close_order_json("AAPL", 0, FIXED_TS_SECS, fixed_run_id());
    let qty = order["qty"].as_i64().expect("SF12: qty must be present");
    assert_eq!(
        qty, 0,
        "SF12: zero net_qty_signed produces qty=0 (degenerate; callers must not pass zero)"
    );
    // Documents which side is chosen: the else branch → "buy".
    // This is not a meaningful order; it exists only to document the behavior.
    let side = order["side"].as_str().unwrap_or("");
    assert!(
        side == "buy" || side == "sell",
        "SF12: zero qty produces some side string; callers must guard against zero before calling"
    );
}
