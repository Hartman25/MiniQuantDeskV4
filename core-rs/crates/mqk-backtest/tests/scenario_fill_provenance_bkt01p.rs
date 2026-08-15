//! BKT-01P: Per-fill provenance proof.
//!
//! Proves that every fill produced by the backtest engine carries:
//! - A non-nil `fill_id` (deterministic UUIDv5)
//! - A non-nil `order_id` (deterministic UUIDv5)
//! - `fill_id != order_id` (the two IDs are derived from different namespaces)
//! - `signal_ts == ` the bar the decision was made on; `fill_ts == ` the
//!   later bar that actually priced it (BKT-FUTURE-EXECUTION-01: `fill_ts >
//!   signal_ts` for ordinary strategy orders)
//! - Identical replay → identical (fill_id, order_id, signal_ts, fill_ts) — determinism
//! - Different bars produce different order_ids — uniqueness
//! - Flatten-all fills carry distinct IDs from intent-driven fills for the same bar

use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn bar(ts: i64) -> BacktestBar {
    BacktestBar::new(
        "SPY",
        ts,
        100_000_000,
        105_000_000,
        95_000_000,
        100_000_000,
        1_000,
    )
}

fn bar2(ts: i64) -> BacktestBar {
    BacktestBar::new(
        "SPY",
        ts,
        110_000_000,
        115_000_000,
        105_000_000,
        110_000_000,
        1_000,
    )
}

fn bar3(ts: i64) -> BacktestBar {
    BacktestBar::new(
        "SPY",
        ts,
        120_000_000,
        125_000_000,
        115_000_000,
        120_000_000,
        1_000,
    )
}

/// Buys on bar 1 (fills on bar 2 -- the first later SPY bar), sells on bar 2
/// (fills on bar 3 -- the first later SPY bar after the sell signal).
struct BuyOnBar1ExitOnBar2;

impl Strategy for BuyOnBar1ExitOnBar2 {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("bkt01p_buy_exit", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        match _ctx.now_tick {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", 10)]),
            2 => StrategyOutput::new(vec![TargetPosition::new("SPY", 0)]),
            _ => StrategyOutput::new(vec![]),
        }
    }
}

const TS_BAR1: i64 = 1_700_000_060;
const TS_BAR2: i64 = 1_700_000_120;
const TS_BAR3: i64 = 1_700_000_180;

/// Three bars so both the buy (signalled on bar 1) and the sell (signalled
/// on bar 2, once the buy has resolved) each have a later bar of their own
/// symbol to fill against.
fn run_three_bar() -> mqk_backtest::BacktestReport {
    let bars = vec![bar(TS_BAR1), bar2(TS_BAR2), bar3(TS_BAR3)];
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 5_000_000; // 5x — permissive
    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BuyOnBar1ExitOnBar2)).unwrap();
    engine.run(&bars).unwrap()
}

// ---------------------------------------------------------------------------
// P1: fill_id and order_id are non-nil
// ---------------------------------------------------------------------------

#[test]
fn fill_and_order_ids_are_non_nil() {
    let report = run_three_bar();
    assert_eq!(report.fills.len(), 2, "expected buy + sell");

    for f in &report.fills {
        assert_ne!(
            f.fill_id,
            Uuid::nil(),
            "fill_id must not be nil (symbol={}, fill_ts={})",
            f.symbol,
            f.fill_ts
        );
        assert_ne!(
            f.order_id,
            Uuid::nil(),
            "order_id must not be nil (symbol={}, fill_ts={})",
            f.symbol,
            f.fill_ts
        );
    }
}

// ---------------------------------------------------------------------------
// P2: fill_id != order_id (different namespace derivation)
// ---------------------------------------------------------------------------

#[test]
fn fill_id_differs_from_order_id() {
    let report = run_three_bar();
    assert_eq!(report.fills.len(), 2);

    for f in &report.fills {
        assert_ne!(
            f.fill_id, f.order_id,
            "fill_id and order_id must be distinct UUIDs"
        );
    }
}

// ---------------------------------------------------------------------------
// P3: signal_ts is decision time, fill_ts is the later pricing bar
// ---------------------------------------------------------------------------

/// BKT-FUTURE-EXECUTION-01: the buy is signalled on bar 1 but priced from
/// bar 2 (the first later SPY bar); the sell is signalled on bar 2 (once the
/// buy has resolved and the strategy sees a position to exit) but priced
/// from bar 3. `fill_ts` must always be strictly greater than `signal_ts`.
#[test]
fn signal_ts_and_fill_ts_reflect_decision_and_pricing_bars_separately() {
    let report = run_three_bar();
    assert_eq!(report.fills.len(), 2);

    let buy = &report.fills[0];
    assert_eq!(buy.signal_ts, TS_BAR1, "buy decision made on bar 1");
    assert_eq!(buy.fill_ts, TS_BAR2, "buy priced from the first later SPY bar");
    assert!(buy.fill_ts > buy.signal_ts);

    let sell = &report.fills[1];
    assert_eq!(sell.signal_ts, TS_BAR2, "sell decision made on bar 2");
    assert_eq!(sell.fill_ts, TS_BAR3, "sell priced from the first later SPY bar");
    assert!(sell.fill_ts > sell.signal_ts);
}

// ---------------------------------------------------------------------------
// P4: deterministic replay — identical IDs across two independent runs
// ---------------------------------------------------------------------------

#[test]
fn ids_are_stable_across_identical_replays() {
    let r1 = run_three_bar();
    let r2 = run_three_bar();

    assert_eq!(r1.fills.len(), r2.fills.len());
    for (f1, f2) in r1.fills.iter().zip(r2.fills.iter()) {
        assert_eq!(
            f1.fill_id, f2.fill_id,
            "fill_id must be identical across replays"
        );
        assert_eq!(
            f1.order_id, f2.order_id,
            "order_id must be identical across replays"
        );
        assert_eq!(
            f1.signal_ts, f2.signal_ts,
            "signal_ts must be identical across replays"
        );
        assert_eq!(
            f1.fill_ts, f2.fill_ts,
            "fill_ts must be identical across replays"
        );
    }
}

// ---------------------------------------------------------------------------
// P5: different bars produce different order_ids
// ---------------------------------------------------------------------------

#[test]
fn different_bars_produce_different_order_ids() {
    let report = run_three_bar();
    assert_eq!(report.fills.len(), 2);

    assert_ne!(
        report.fills[0].order_id, report.fills[1].order_id,
        "fills signalled on different bars must have distinct order_ids"
    );
    assert_ne!(
        report.fills[0].fill_id, report.fills[1].fill_id,
        "fills signalled on different bars must have distinct fill_ids"
    );
}

// ---------------------------------------------------------------------------
// P6: flatten-all order_id namespace is distinct from intent order_id namespace
// ---------------------------------------------------------------------------

/// Pure-function proof: `make_flatten_order_id` uses a "flatten:..." name prefix,
/// making its UUIDv5 output distinct from `make_order_id` even when all other
/// inputs (ts, symbol, seq) are identical.
///
/// This proves the namespace separation without depending on the engine triggering
/// a specific risk-halt path. The property is structural: two functions with
/// different name formats under the same UUID namespace will always produce
/// different UUIDs (collision probability negligible for distinct inputs).
#[test]
fn flatten_order_id_namespace_is_distinct_from_intent_order_id() {
    use mqk_backtest::BacktestFill;

    let ts: i64 = 1_700_000_060;
    let symbol = "SPY";
    let seq: usize = 0;

    // Intent-driven order ID (BUY, seq 0)
    let intent_order_id = BacktestFill::make_order_id(ts, symbol, true, seq);
    // Flatten order ID (seq 0 — same symbol, same bar, same seq position)
    let flatten_order_id = BacktestFill::make_flatten_order_id(ts, symbol, seq);

    // Must differ — "flatten:ts:sym:seq" != "ts:sym:B:seq"
    assert_ne!(
        intent_order_id, flatten_order_id,
        "intent and flatten order IDs must differ for same ts/symbol/seq"
    );

    // fill_ids derived from distinct order_ids must also differ
    let intent_fill_id = BacktestFill::make_fill_id(&intent_order_id);
    let flatten_fill_id = BacktestFill::make_fill_id(&flatten_order_id);
    assert_ne!(
        intent_fill_id, flatten_fill_id,
        "fill IDs derived from distinct order IDs must also differ"
    );

    // Both must be non-nil
    assert_ne!(
        intent_order_id,
        Uuid::nil(),
        "intent order_id must not be nil"
    );
    assert_ne!(
        flatten_order_id,
        Uuid::nil(),
        "flatten order_id must not be nil"
    );

    // Both must be stable (same inputs → same UUID on repeated calls)
    assert_eq!(
        BacktestFill::make_order_id(ts, symbol, true, seq),
        intent_order_id,
        "make_order_id must be stable"
    );
    assert_eq!(
        BacktestFill::make_flatten_order_id(ts, symbol, seq),
        flatten_order_id,
        "make_flatten_order_id must be stable"
    );

    // SELL intent also differs from flatten at same position
    let sell_intent_order_id = BacktestFill::make_order_id(ts, symbol, false, seq);
    assert_ne!(
        sell_intent_order_id, flatten_order_id,
        "SELL intent order_id must differ from flatten order_id"
    );
}
