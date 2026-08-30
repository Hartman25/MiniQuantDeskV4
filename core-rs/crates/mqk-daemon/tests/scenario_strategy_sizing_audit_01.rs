//! STRATEGY-SIZING-AND-EXIT-AUDIT-01: sizing and exit behavior audit tests.
//!
//! Proves the exact code path from bar tick → signal direction → target qty →
//! delta → order qty, and documents why:
//!
//! 1. The bot only buys 1 AAPL per run-start (default target_qty=1).
//! 2. Subsequent ticks with the same signal produce delta=0 → no order.
//! 3. The strategy's -1 signal (bearish) does NOT produce a sell because B5 blocks
//!    a sell that would go net-short (target=-1, current=1 → delta=-2 → qty_to_sell=2
//!    > current=1 → blocked).
//! 4. A configurable target_qty (e.g. 5) changes the buy size.
//! 5. A flat-exit (target=0 while long) produces a valid sell (B5 passes).
//!    This is the correct exit mechanic; the current intraday_scalper never emits
//!    target=0 on a bearish signal — it emits -1, which B5 blocks.
//!    Fix is deferred to STRATEGY-EXIT-RULES-01.
//! 6. Risk/capital gates are independent of sizing — they approve or reject orders
//!    but do not size them.
//! 7. Buying power/account balance is never consulted for sizing.
//!
//! # Follow-up patches documented here
//!
//! - STRATEGY-EXIT-RULES-01: make intraday_scalper emit target=0 (flat) on
//!   bearish signal rather than target=-N (net-short), so a long position can be
//!   closed naturally via the existing B5-passing sell path.
//! - STRATEGY-POSITION-SIZING-01: add risk-cap enforcement (max notional, max
//!   equity fraction) around MQK_STRATEGY_TARGET_QTY so larger sizes are bounded.
//! - STRATEGY-REPEATED-TRADE-CYCLE-01: prove entry → fill → exit → entry cycling
//!   works end-to-end in a DB-backed scenario.

use std::collections::BTreeMap;

use uuid::Uuid;

use mqk_daemon::decision::bar_result_to_decisions;
use mqk_strategy::{
    IntentMode, StrategyBarResult, StrategyIntents, StrategyOutput, StrategySpec, TargetPosition,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn live_result_with_target(symbol: &str, target: i64) -> StrategyBarResult {
    StrategyBarResult {
        spec: StrategySpec::new("intraday_scalper", 300),
        semantic_fingerprint: "test-fixture:intraday_scalper".to_string(),
        intents: StrategyIntents {
            mode: IntentMode::Live,
            output: StrategyOutput {
                targets: vec![TargetPosition::new(symbol, target)],
            },
        },
    }
}

fn fixed_run_id() -> Uuid {
    Uuid::nil()
}

const FIXED_NOW_MICROS: i64 = 1_748_000_000_000_000;

fn flat() -> BTreeMap<String, i64> {
    BTreeMap::new()
}

fn pos(symbol: &str, qty: i64) -> BTreeMap<String, i64> {
    let mut m = BTreeMap::new();
    m.insert(symbol.to_string(), qty);
    m
}

// ---------------------------------------------------------------------------
// SA-01: First signal with current_position=0 and target=1 → buy delta=1
// ---------------------------------------------------------------------------

/// SA-01: First bar tick, flat position, target=1 → buy 1 share.
///
/// Proves the normal entry path: strategy emits target=+1, current=0,
/// delta=+1 → buy 1.  This is what the live smoke produced: ordered_qty=1.
#[test]
fn sa01_first_signal_flat_target1_buys_1() {
    let result = live_result_with_target("AAPL", 1);
    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &flat());

    assert_eq!(decisions.len(), 1, "SA-01: one target → one decision");
    let d = &decisions[0];
    assert_eq!(d.side, "buy", "SA-01: positive delta → buy");
    assert_eq!(d.qty, 1, "SA-01: qty = delta = target(1) - current(0)");
    assert_eq!(d.order_type, "market", "SA-01: market order");
}

// ---------------------------------------------------------------------------
// SA-02: Second signal with current_position=1 and target=1 → delta=0, no order
// ---------------------------------------------------------------------------

/// SA-02: Second bar tick, already holding 1 share, target=1 → no order.
///
/// Proves the root cause of "only 1 share bought and no further orders":
/// after the first fill, current_position=1 and strategy still emits target=1,
/// so delta = 1 - 1 = 0.  No_order_reason = already_at_target.
#[test]
fn sa02_second_signal_already_at_target_no_order() {
    let result = live_result_with_target("AAPL", 1);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("AAPL", 1));

    assert!(
        decisions.is_empty(),
        "SA-02: target == current → delta=0 → no_order_reason=already_at_target; \
         got: {:?}",
        decisions
            .iter()
            .map(|d| (&d.symbol, &d.side, d.qty))
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// SA-03: Configurable target_qty=5 with flat position → buy delta=5
// ---------------------------------------------------------------------------

/// SA-03: target_qty=5 (via MQK_STRATEGY_TARGET_QTY), flat position → buy 5.
///
/// Proves that setting target_qty=5 produces a buy of 5 shares from flat.
/// Risk/capital gates are independent and still apply after this point.
#[test]
fn sa03_target_qty_5_flat_buys_5() {
    let result = live_result_with_target("AAPL", 5);
    let decisions = bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &flat());

    assert_eq!(decisions.len(), 1, "SA-03: one target → one decision");
    let d = &decisions[0];
    assert_eq!(d.side, "buy", "SA-03: positive delta → buy");
    assert_eq!(d.qty, 5, "SA-03: qty = delta = target(5) - current(0)");
}

// ---------------------------------------------------------------------------
// SA-04: target_qty=5, current_position=2 → buy delta=3
// ---------------------------------------------------------------------------

/// SA-04: target=5, current=2 → buy 3 (delta only, not full target).
///
/// Proves partial-position delta logic: if we already hold 2 and target is 5,
/// we buy only 3 more.
#[test]
fn sa04_target_qty_5_partial_position_buys_delta() {
    let result = live_result_with_target("AAPL", 5);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("AAPL", 2));

    assert_eq!(decisions.len(), 1, "SA-04: one target → one decision");
    let d = &decisions[0];
    assert_eq!(d.side, "buy", "SA-04: positive delta → buy");
    assert_eq!(d.qty, 3, "SA-04: qty = delta = target(5) - current(2)");
}

// ---------------------------------------------------------------------------
// SA-05: Bearish signal (target=-1), current_position=1 → B5 BLOCKS sell
// ---------------------------------------------------------------------------

/// SA-05: Bearish signal emits target=-1; current=1; B5 guard blocks the sell.
///
/// Proves WHY no sell is observed in the live smoke:
/// - direction = -1, target_qty = 1 → TargetPosition.qty = -1
/// - current = 1 (one share held from the buy fill)
/// - delta = -1 - 1 = -2
/// - qty_to_sell = 2
/// - B5 guard: qty_to_sell(2) > current(1) → blocked (would go net-short)
///
/// The strategy CANNOT exit a long position via a bearish signal because the
/// runtime interprets target=-1 as "hold -1 share" (net-short), which is
/// blocked fail-closed.  Fix: STRATEGY-EXIT-RULES-01.
#[test]
fn sa05_bearish_signal_b5_blocks_sell_would_go_short() {
    // target=-1 is what intraday_scalper emits on a bearish bar
    let result = live_result_with_target("AAPL", -1);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("AAPL", 1));

    assert!(
        decisions.is_empty(),
        "SA-05: target=-1, current=1 → delta=-2, qty_to_sell=2 > current(1) → \
         B5 short-sale guard blocks; no sell produced; \
         STRATEGY-EXIT-RULES-01 required for proper exit; got: {:?}",
        decisions
            .iter()
            .map(|d| (&d.symbol, &d.side, d.qty))
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// SA-06: Flat-exit target=0, current_position=1 → sell 1 (B5 PASSES)
// ---------------------------------------------------------------------------

/// SA-06: target=0 (flat-exit), current=1 → sell 1; B5 guard passes.
///
/// Proves that the CORRECT exit mechanic exists in the runtime: if the strategy
/// returned target=0 instead of target=-1 on a bearish signal, the B5 guard
/// would pass (qty_to_sell=1 == current=1, not exceeding) and a sell would be
/// produced.
///
/// STRATEGY-EXIT-RULES-01 should make intraday_scalper emit target=0 (go flat)
/// on bearish signals rather than target=-1 (go short).
#[test]
fn sa06_flat_exit_target_0_current_1_sells_1() {
    let result = live_result_with_target("AAPL", 0);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("AAPL", 1));

    assert_eq!(
        decisions.len(),
        1,
        "SA-06: target=0, current=1 → delta=-1 → sell 1 (flat exit); B5 passes"
    );
    let d = &decisions[0];
    assert_eq!(d.side, "sell", "SA-06: negative delta → sell");
    assert_eq!(d.qty, 1, "SA-06: sell qty = current holdings = 1");
}

// ---------------------------------------------------------------------------
// SA-07: Flat-exit target=0, current_position=5 → sell 5
// ---------------------------------------------------------------------------

/// SA-07: target=0, current=5 → sell 5; proves flat-exit scales to position size.
#[test]
fn sa07_flat_exit_target_0_current_5_sells_5() {
    let result = live_result_with_target("AAPL", 0); // target=0 (go flat)
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("AAPL", 5));

    assert_eq!(
        decisions.len(),
        1,
        "SA-07: target=0, current=5 → sell 5 (B5 passes: qty_to_sell=5 == current)"
    );
    let d = &decisions[0];
    assert_eq!(d.side, "sell", "SA-07: sell to close long");
    assert_eq!(d.qty, 5, "SA-07: sell qty = current position");
}

// ---------------------------------------------------------------------------
// SA-08: target_qty=5, current_position=5, second tick → delta=0, no order
// ---------------------------------------------------------------------------

/// SA-08: target=5, current=5 → no order (already at target).
///
/// Proves the same already_at_target behavior with a configurable size:
/// once holding 5 shares at target=5, subsequent ticks produce no orders.
#[test]
fn sa08_already_at_target_5_no_order() {
    let result = live_result_with_target("AAPL", 5);
    let decisions =
        bar_result_to_decisions(&result, fixed_run_id(), FIXED_NOW_MICROS, &pos("AAPL", 5));

    assert!(
        decisions.is_empty(),
        "SA-08: target=5, current=5 → delta=0 → no_order_reason=already_at_target"
    );
}

// ---------------------------------------------------------------------------
// SA-09: Strategy emit test — with_target_qty(5) bullish → target=5, not 1
// ---------------------------------------------------------------------------

/// SA-09: IntradayScalperStrategy::with_target_qty(5) on bullish bars emits target=5.
///
/// Proves the strategy-level sizing change propagates to TargetPosition.qty.
/// The decision layer then computes delta against current position.
#[test]
fn sa09_strategy_with_target_qty_5_emits_target_5() {
    use mqk_strategy::engines::intraday_scalper::IntradayScalperStrategy;
    use mqk_strategy::Strategy;
    use mqk_strategy::{BarStub, RecentBarsWindow, StrategyContext};

    let base = 200_000_000i64;
    let bars: Vec<BarStub> = (0..5)
        .map(|i| {
            BarStub::new(
                i,
                true,
                if i == 4 { base + base / 400 } else { base }, // last bar +25 bps
                1,
            )
        })
        .collect();
    let ctx = StrategyContext::new(300, 0, RecentBarsWindow::new(10, bars));
    let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 5);
    let out = s.on_bar(&ctx);

    assert_eq!(out.targets.len(), 1, "SA-09: one target");
    assert_eq!(
        out.targets[0].qty, 5,
        "SA-09: bullish direction(+1) × target_qty(5) = target(5)"
    );
}

// ---------------------------------------------------------------------------
// SA-10: target_qty_from_env returns default 1 when env is absent
// ---------------------------------------------------------------------------

/// SA-10: target_qty_from_env returns 1 when MQK_STRATEGY_TARGET_QTY is not set.
///
/// Proves conservative default: absent env → default 1 (not 0, not panic).
#[test]
fn sa10_target_qty_from_env_default_is_1() {
    use mqk_strategy::engines::intraday_scalper::target_qty_from_env;

    // Remove the env var if present in the test process.
    std::env::remove_var("MQK_STRATEGY_TARGET_QTY");
    let qty = target_qty_from_env();
    assert_eq!(qty, 1, "SA-10: absent MQK_STRATEGY_TARGET_QTY → default 1");
}
