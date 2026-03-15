//! BKT-01P: Per-fill provenance proof.
//!
//! Proves that every fill produced by the backtest engine carries:
//! - A non-nil `fill_id` (deterministic UUIDv5)
//! - A non-nil `order_id` (deterministic UUIDv5)
//! - `fill_id != order_id` (the two IDs are derived from different namespaces)
//! - `bar_end_ts == bar.end_ts` for the bar that triggered the fill
//! - Identical replay → identical (fill_id, order_id, bar_end_ts) — determinism
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

fn run_two_bar() -> mqk_backtest::BacktestReport {
    let bars = vec![bar(1_700_000_060), bar2(1_700_000_120)];
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
    let report = run_two_bar();
    assert_eq!(report.fills.len(), 2, "expected buy + sell");

    for f in &report.fills {
        assert_ne!(
            f.fill_id,
            Uuid::nil(),
            "fill_id must not be nil (symbol={}, ts={})",
            f.symbol,
            f.bar_end_ts
        );
        assert_ne!(
            f.order_id,
            Uuid::nil(),
            "order_id must not be nil (symbol={}, ts={})",
            f.symbol,
            f.bar_end_ts
        );
    }
}

// ---------------------------------------------------------------------------
// P2: fill_id != order_id (different namespace derivation)
// ---------------------------------------------------------------------------

#[test]
fn fill_id_differs_from_order_id() {
    let report = run_two_bar();
    assert_eq!(report.fills.len(), 2);

    for f in &report.fills {
        assert_ne!(
            f.fill_id, f.order_id,
            "fill_id and order_id must be distinct UUIDs"
        );
    }
}

// ---------------------------------------------------------------------------
// P3: bar_end_ts matches the bar that triggered the fill
// ---------------------------------------------------------------------------

#[test]
fn bar_end_ts_matches_triggering_bar() {
    let ts_bar1: i64 = 1_700_000_060;
    let ts_bar2: i64 = 1_700_000_120;

    let report = run_two_bar();
    assert_eq!(report.fills.len(), 2);

    // Fill 0: BUY triggered on bar 1
    assert_eq!(
        report.fills[0].bar_end_ts, ts_bar1,
        "BUY fill bar_end_ts should match bar 1 ts"
    );

    // Fill 1: SELL triggered on bar 2
    assert_eq!(
        report.fills[1].bar_end_ts, ts_bar2,
        "SELL fill bar_end_ts should match bar 2 ts"
    );
}

// ---------------------------------------------------------------------------
// P4: deterministic replay — identical IDs across two independent runs
// ---------------------------------------------------------------------------

#[test]
fn ids_are_stable_across_identical_replays() {
    let r1 = run_two_bar();
    let r2 = run_two_bar();

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
            f1.bar_end_ts, f2.bar_end_ts,
            "bar_end_ts must be identical across replays"
        );
    }
}

// ---------------------------------------------------------------------------
// P5: different bars produce different order_ids
// ---------------------------------------------------------------------------

#[test]
fn different_bars_produce_different_order_ids() {
    let report = run_two_bar();
    assert_eq!(report.fills.len(), 2);

    assert_ne!(
        report.fills[0].order_id, report.fills[1].order_id,
        "fills on different bars must have distinct order_ids"
    );
    assert_ne!(
        report.fills[0].fill_id, report.fills[1].fill_id,
        "fills on different bars must have distinct fill_ids"
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
