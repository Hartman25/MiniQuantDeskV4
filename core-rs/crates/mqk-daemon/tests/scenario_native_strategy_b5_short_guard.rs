//! B5: Short-sale guard for the native strategy decision path.
//!
//! Closes the B5 hardening gap: `bar_result_to_decisions()` previously allowed
//! a native strategy to produce sell decisions that exceeded existing long
//! holdings, or to open short positions from flat.  The broker would reject or
//! silently fill these, leaving the runtime tracking a position it cannot manage.
//!
//! # Gap closed
//!
//! The native strategy runtime does not support short-position lifecycle
//! (margin, borrow, cover semantics).  A sell intent that would result in a
//! net-short position must be dropped fail-closed at the translation layer,
//! not forwarded to the broker.
//!
//! # Guard logic
//!
//! When `delta < 0` (sell direction), two blocking conditions are checked
//! before the decision is created:
//!   (a) `current <= 0` — flat or already short; any sell opens/extends short.
//!   (b) `abs(delta) > current` — sell exceeds long holdings; drives net-short.
//!
//! Both cases return `None` from the `filter_map`, silently dropping the intent.
//!
//! Buy decisions (delta > 0) are unaffected by this guard.
//!
//! # Test inventory
//!
//! | ID   | Condition                                                   | Expected                                        |
//! |------|-------------------------------------------------------------|-------------------------------------------------|
//! | S01  | Sell from flat (target=-N, current=0)                       | blocked (would open short)                      |
//! | S02  | Sell exceeds holdings (abs(delta) > current > 0)            | blocked (would go net-short)                    |
//! | S03  | Close long exactly (target=0, current=N)                    | allowed (sell qty == holdings)                  |
//! | S04  | Partial reduce long (target=M, current=N, 0<M<N)            | allowed (sell qty < holdings)                   |
//! | S05  | Buy from flat — guard does not apply to buy direction       | allowed (unaffected)                             |
//! | S06  | Sell exactly at holdings boundary                           | allowed (qty_to_sell == current, boundary case) |
//! | S07  | Mixed bar: valid close-long + invalid short-from-flat       | valid passes; invalid blocked; one decision out |
//! | S08  | Already short, targeting deeper short (going more negative) | blocked (current <= 0; deepening short)         |

use std::collections::BTreeMap;

use uuid::Uuid;

use mqk_daemon::decision::bar_result_to_decisions;
use mqk_strategy::{
    IntentMode, StrategyBarResult, StrategyIntents, StrategyOutput, StrategySpec, TargetPosition,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn live_result(targets: Vec<TargetPosition>) -> StrategyBarResult {
    StrategyBarResult {
        spec: StrategySpec::new("test_strategy", 300),
        intents: StrategyIntents {
            mode: IntentMode::Live,
            output: StrategyOutput { targets },
        },
    }
}

fn fixed_run_id() -> Uuid {
    Uuid::nil()
}

const FIXED_NOW_MICROS: i64 = 1_700_000_000_000_000;

fn flat() -> BTreeMap<String, i64> {
    BTreeMap::new()
}

fn pos(symbol: &str, qty: i64) -> BTreeMap<String, i64> {
    let mut m = BTreeMap::new();
    m.insert(symbol.to_string(), qty);
    m
}

// ---------------------------------------------------------------------------
// S01 — Sell from flat position → blocked (would open short)
// ---------------------------------------------------------------------------

/// S01: Strategy targets a short position from flat → blocked by B5 guard.
///
/// Before B5: this produced `side="sell", qty=10`.
/// After B5: no decision (selling from flat = opening short; not supported).
///
/// Rationale: the runtime has no position to sell against.  Forwarding this
/// to the broker would either open a short (unmanageable) or produce a broker
/// rejection (visible error noise).  Fail-closed at the source.
#[test]
fn b5_s01_sell_from_flat_is_blocked() {
    // target=-10, current=0 → delta=-10 → would sell 10 (short from flat)
    let result = live_result(vec![TargetPosition::new("AAPL", -10)]);
    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &flat());

    assert!(
        decisions.is_empty(),
        "S01: sell from flat must be blocked by B5 short-sale guard; \
         got: {:?}",
        decisions
            .iter()
            .map(|d| (&d.symbol, &d.side, d.qty))
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// S02 — Sell exceeds holdings → blocked (would go net-short)
// ---------------------------------------------------------------------------

/// S02: Strategy targets a position more negative than current long holdings →
/// blocked.
///
/// Example: current=5, target=-8 → delta=-13.  Selling 13 shares when only
/// 5 are held leaves the position at -8 (a short of 8 shares).
#[test]
fn b5_s02_sell_exceeds_holdings_is_blocked() {
    // current=5 MSFT, target=-8 → delta=-13 → qty_to_sell=13 > current(5) → blocked
    let result = live_result(vec![TargetPosition::new("MSFT", -8)]);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("MSFT", 5));

    assert!(
        decisions.is_empty(),
        "S02: sell(13) > current holdings(5) must be blocked; \
         got: {:?}",
        decisions
            .iter()
            .map(|d| (&d.symbol, &d.side, d.qty))
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// S03 — Close long exactly → allowed
// ---------------------------------------------------------------------------

/// S03: Strategy targets flat from a long position → sell exactly current
/// holdings.  B5 guard passes (qty_to_sell == current, not exceeding).
#[test]
fn b5_s03_close_long_exactly_is_allowed() {
    // current=7 GOOG, target=0 → delta=-7 → sell 7; 7 <= 7 → guard passes
    let result = live_result(vec![TargetPosition::new("GOOG", 0)]);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("GOOG", 7));

    assert_eq!(
        decisions.len(),
        1,
        "S03: close-long target → one sell decision"
    );
    let d = &decisions[0];
    assert_eq!(d.side, "sell", "S03: negative delta → sell");
    assert_eq!(d.qty, 7, "S03: sell qty = current holdings (7)");
}

// ---------------------------------------------------------------------------
// S04 — Partial reduce long → allowed
// ---------------------------------------------------------------------------

/// S04: Strategy reduces a long position without going flat → allowed.
///
/// Example: current=10, target=4 → delta=-6 → sell 6.  Resulting position
/// is 4 (still long).  qty_to_sell(6) <= current(10): guard passes.
#[test]
fn b5_s04_partial_reduce_long_is_allowed() {
    // current=10 NVDA, target=4 → delta=-6 → sell 6; 6 <= 10 → passes
    let result = live_result(vec![TargetPosition::new("NVDA", 4)]);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("NVDA", 10));

    assert_eq!(
        decisions.len(),
        1,
        "S04: partial reduce → one sell decision"
    );
    let d = &decisions[0];
    assert_eq!(d.side, "sell", "S04: negative delta → sell");
    assert_eq!(
        d.qty, 6,
        "S04: sell qty = delta = current(10) - target(4) = 6"
    );
}

// ---------------------------------------------------------------------------
// S05 — Buy from flat → guard does not apply, decision produced
// ---------------------------------------------------------------------------

/// S05: Buy direction is unaffected by the B5 short-sale guard.
///
/// The guard only fires for `delta < 0` (sell direction).  A buy from flat
/// is a normal opening purchase; it must still produce a decision.
#[test]
fn b5_s05_buy_from_flat_is_unaffected() {
    // current=0, target=+12 → delta=+12 → buy 12; guard does not apply (delta > 0)
    let result = live_result(vec![TargetPosition::new("TSLA", 12)]);
    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &flat());

    assert_eq!(decisions.len(), 1, "S05: buy from flat → one buy decision");
    let d = &decisions[0];
    assert_eq!(d.side, "buy", "S05: positive delta → buy");
    assert_eq!(d.qty, 12, "S05: qty = target (delta from flat)");
}

// ---------------------------------------------------------------------------
// S06 — Sell at exact holdings boundary → allowed (boundary case)
// ---------------------------------------------------------------------------

/// S06: qty_to_sell == current (selling every share held) is the valid
/// boundary case.  This proves the guard uses `>`, not `>=`.
///
/// current=3, target=-3 would be checked as: delta = -3-3 = -6 → qty_to_sell=6
/// That exceeds current(3), so it's blocked.
///
/// Instead: current=3, target=0 → delta=-3 → qty_to_sell=3 == current(3) → allowed.
#[test]
fn b5_s06_sell_exactly_at_holdings_boundary_is_allowed() {
    // current=3 AMD, target=0 → delta=-3 → qty_to_sell=3 == current(3) → guard: 3 > 3 is false → passes
    let result = live_result(vec![TargetPosition::new("AMD", 0)]);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("AMD", 3));

    assert_eq!(
        decisions.len(),
        1,
        "S06: sell exactly at holdings boundary → one decision"
    );
    let d = &decisions[0];
    assert_eq!(d.side, "sell", "S06: negative delta → sell");
    assert_eq!(d.qty, 3, "S06: qty = current holdings (boundary)");
}

// ---------------------------------------------------------------------------
// S07 — Mixed bar: valid close-long + invalid short-from-flat → selective guard
// ---------------------------------------------------------------------------

/// S07: A bar result with multiple targets where some are valid (close long)
/// and some are invalid (short from flat or exceeds holdings) → the guard
/// selectively blocks only the invalid ones.
///
/// Proves the guard does not affect unrelated targets in the same bar.
#[test]
fn b5_s07_mixed_bar_guard_is_selective() {
    // Position map: AAPL held 10, TSLA flat, MSFT flat.
    let mut positions = BTreeMap::new();
    positions.insert("AAPL".to_string(), 10i64);

    let result = live_result(vec![
        TargetPosition::new("AAPL", 0), // close long: target=0, current=10 → sell 10 → VALID
        TargetPosition::new("TSLA", -5), // short from flat: target=-5, current=0 → BLOCKED
        TargetPosition::new("MSFT", 8), // buy from flat: target=+8, current=0 → VALID
    ]);
    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &positions);

    assert_eq!(
        decisions.len(),
        2,
        "S07: two valid targets (AAPL close, MSFT buy); TSLA short-from-flat blocked; \
         got: {:?}",
        decisions
            .iter()
            .map(|d| (&d.symbol, &d.side, d.qty))
            .collect::<Vec<_>>()
    );

    let syms: Vec<&str> = decisions.iter().map(|d| d.symbol.as_str()).collect();
    assert!(syms.contains(&"AAPL"), "S07: AAPL close-long must pass");
    assert!(syms.contains(&"MSFT"), "S07: MSFT buy must pass");
    assert!(
        !syms.contains(&"TSLA"),
        "S07: TSLA short-from-flat must be blocked"
    );

    // Confirm the AAPL sell is correct.
    let aapl = decisions.iter().find(|d| d.symbol == "AAPL").unwrap();
    assert_eq!(aapl.side, "sell", "S07: AAPL → sell to close long");
    assert_eq!(aapl.qty, 10, "S07: AAPL sell qty = current holdings");
}

// ---------------------------------------------------------------------------
// S08 — Already short, targeting deeper short → blocked
// ---------------------------------------------------------------------------

/// S08: The position map records an existing short (negative qty).  The
/// strategy targets going deeper into short.  `delta < 0` and `current <= 0`
/// → guard blocks the intent.
///
/// Cover/close of an existing short is a BUY (delta > 0) and is unaffected.
/// This test proves the sell-direction guard fires even when an existing short
/// position is present.
#[test]
fn b5_s08_deepen_existing_short_is_blocked() {
    // current=-5 AMZN (existing short), target=-10 → delta = -10 - (-5) = -5 → sell 5
    // guard: current(-5) <= 0 → blocked
    let result = live_result(vec![TargetPosition::new("AMZN", -10)]);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("AMZN", -5));

    assert!(
        decisions.is_empty(),
        "S08: deepening an existing short (current=-5, target=-10) must be blocked; \
         current <= 0 triggers B5 guard; got: {:?}",
        decisions
            .iter()
            .map(|d| (&d.symbol, &d.side, d.qty))
            .collect::<Vec<_>>()
    );
}
