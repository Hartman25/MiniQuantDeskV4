//! Scenario: No same-bar fills, ever — BKT-FUTURE-EXECUTION-01 (supersedes Patch B1)
//!
//! # Background
//!
//! Patch B1 originally guarded against "same-bar lookahead" only in its
//! narrowest form — using a signal bar's OPEN or CLOSE price for the fill
//! that produced the signal. It still filled from the *signal bar itself*
//! (worst-case OHLC within that bar), which is what BKT-FUTURE-EXECUTION-01
//! closes: a strategy that observes a completed bar at timestamp T must
//! never receive an economic fill priced from that same bar. The fill must
//! come from the first strictly-later bar for the order's own symbol.
//!
//! The engine guards against lookahead in layers that cannot be bypassed:
//!
//! 1. **Incomplete-bar gate**: bars with `is_complete = false` are rejected with
//!    `Err(BacktestError::IncompleteBar)`. The strategy never sees partial data.
//! 2. **No same-bar fill**: an order generated from bar T's signal can only be
//!    filled by a later bar (`fill_ts > signal_ts`) for the exact same symbol.
//! 3. **Conservative fill pricing** on that later bar: fills always use the
//!    worst-case price within the completed bar (BUY @ HIGH, SELL @ LOW), not
//!    OPEN, not CLOSE.
//!
//! # Invariants under test
//!
//! 1. Incomplete bar at position 0 → `Err(IncompleteBar)` (no fills, no report).
//! 2. Incomplete bar sandwiched between complete bars → `Err(IncompleteBar)`.
//! 3. Negative timestamp → `Err(NegativeTimestamp)`.
//! 4. BUY signal_ts != fill_ts (no same-bar fill).
//! 5. BUY fill price ≠ open_micros of the *fill* bar (OPEN lookahead is blocked).
//! 6. BUY fill price ≠ close_micros of the *fill* bar (CLOSE lookahead is blocked).
//! 7. SELL signal_ts != fill_ts (no same-bar fill).
//! 8. SELL fill price ≠ open_micros of the *fill* bar.
//! 9. SELL fill price ≠ close_micros of the *fill* bar.
//!
//! The companion file `scenario_ambiguity_worst_case_enforced.rs` proves the
//! positive side: BUY fills AT HIGH and SELL fills AT LOW of the fill bar.
//!
//! All tests are pure in-process; no DB or network required.

use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, BacktestError};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_portfolio::Side as PfSide;
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

// ---------------------------------------------------------------------------
// Strategy helpers
// ---------------------------------------------------------------------------

/// Emits a BUY target on bar 1, nothing after.
struct BuyOnBar1 {
    bar_idx: u64,
}
impl BuyOnBar1 {
    fn new() -> Self {
        Self { bar_idx: 0 }
    }
}
impl Strategy for BuyOnBar1 {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BuyOnBar1", 60)
    }
    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", 10)]),
            _ => StrategyOutput::new(vec![]),
        }
    }
}

/// Buys 10 on bar 1, sells all on bar 2 (once the buy has resolved).
struct BuyBar1SellBar2 {
    bar_idx: u64,
}
impl BuyBar1SellBar2 {
    fn new() -> Self {
        Self { bar_idx: 0 }
    }
}
impl Strategy for BuyBar1SellBar2 {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BuyBar1SellBar2", 60)
    }
    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", 10)]),
            2 => StrategyOutput::new(vec![TargetPosition::new("SPY", 0)]),
            _ => StrategyOutput::new(vec![]),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A bar where all four prices are deliberately different so tests can
/// distinguish OPEN / HIGH / LOW / CLOSE unambiguously.
///
/// OPEN=500, HIGH=515, LOW=488, CLOSE=505 (all in micros × 1_000_000)
fn distinct_ohlc_bar(symbol: &str, end_ts: i64) -> BacktestBar {
    BacktestBar::new(
        symbol,
        end_ts,
        500_000_000, // open  — $500.00
        515_000_000, // high  — $515.00   ← BUY fill expected here
        488_000_000, // low   — $488.00   ← SELL fill expected here
        505_000_000, // close — $505.00
        1_000,
    )
}

/// A second bar with a different OPEN/CLOSE than `distinct_ohlc_bar` but the
/// same HIGH/LOW, so a same-bar-fill regression (accidentally reusing bar 1's
/// price) is still distinguishable from a correct fill against this bar.
fn distinct_ohlc_bar2(symbol: &str, end_ts: i64) -> BacktestBar {
    BacktestBar::new(
        symbol,
        end_ts,
        501_000_000, // open  — $501.00
        516_000_000, // high  — $516.00   ← BUY fill expected here
        487_000_000, // low   — $487.00   ← SELL fill expected here
        506_000_000, // close — $506.00
        1_000,
    )
}

fn default_cfg() -> BacktestConfig {
    BacktestConfig::test_defaults()
}

// ---------------------------------------------------------------------------
// 1. Incomplete bar at position 0 → Err(IncompleteBar)
// ---------------------------------------------------------------------------

#[test]
fn incomplete_bar_at_position_one_is_always_rejected() {
    let mut bar = distinct_ohlc_bar("SPY", 1_700_000_060);
    bar.is_complete = false; // mark incomplete — this must be rejected

    let mut engine = BacktestEngine::new(default_cfg());
    engine.add_strategy(Box::new(BuyOnBar1::new())).unwrap();
    let result = engine.run(&[bar]);

    match result {
        Err(BacktestError::IncompleteBar { symbol, end_ts }) => {
            assert_eq!(symbol, "SPY");
            assert_eq!(end_ts, 1_700_000_060);
        }
        Ok(report) => panic!(
            "expected Err(IncompleteBar) but got Ok with {} fills",
            report.fills.len()
        ),
        Err(other) => panic!("expected IncompleteBar but got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// 2. Incomplete bar sandwiched between complete bars → Err(IncompleteBar)
// ---------------------------------------------------------------------------

#[test]
fn incomplete_bar_sandwiched_between_complete_bars_is_rejected() {
    let bar1 = distinct_ohlc_bar("SPY", 1_700_000_060); // complete
    let mut bar2 = distinct_ohlc_bar("SPY", 1_700_000_120);
    bar2.is_complete = false; // incomplete — must be rejected mid-run
    let bar3 = distinct_ohlc_bar("SPY", 1_700_000_180); // complete (never reached)

    let mut engine = BacktestEngine::new(default_cfg());
    engine.add_strategy(Box::new(BuyOnBar1::new())).unwrap();
    let result = engine.run(&[bar1, bar2, bar3]);

    match result {
        Err(BacktestError::IncompleteBar { end_ts, .. }) => {
            assert_eq!(
                end_ts, 1_700_000_120,
                "incomplete bar should be identified by its timestamp"
            );
        }
        Ok(report) => panic!(
            "expected Err(IncompleteBar) for middle bar but got Ok with {} fills",
            report.fills.len()
        ),
        Err(other) => panic!("expected IncompleteBar but got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// 3. Negative timestamp → Err(NegativeTimestamp)
// ---------------------------------------------------------------------------

#[test]
fn negative_timestamp_is_always_rejected() {
    let mut bar = distinct_ohlc_bar("SPY", -1);
    bar.end_ts = -1; // negative — must be rejected

    let mut engine = BacktestEngine::new(default_cfg());
    engine.add_strategy(Box::new(BuyOnBar1::new())).unwrap();
    let result = engine.run(&[bar]);

    match result {
        Err(BacktestError::NegativeTimestamp { end_ts }) => {
            assert_eq!(end_ts, -1);
        }
        Ok(report) => panic!(
            "expected Err(NegativeTimestamp) but got Ok with {} fills",
            report.fills.len()
        ),
        Err(other) => panic!("expected NegativeTimestamp but got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// 4/5/6: BUY — no same-bar fill; fill price ≠ OPEN/CLOSE of the fill bar
// ---------------------------------------------------------------------------

#[test]
fn buy_signal_and_fill_are_never_the_same_bar() {
    // bar1 signals; bar2 is the only later SPY bar, so it must price the fill.
    let bar1 = distinct_ohlc_bar("SPY", 1_700_000_060);
    let bar2 = distinct_ohlc_bar2("SPY", 1_700_000_120);

    let mut engine = BacktestEngine::new(default_cfg());
    engine.add_strategy(Box::new(BuyOnBar1::new())).unwrap();
    let report = engine.run(&[bar1.clone(), bar2.clone()]).unwrap();

    assert_eq!(report.fills.len(), 1, "expected exactly 1 buy fill");
    let fill = &report.fills[0];
    assert_eq!(fill.signal_ts, bar1.end_ts, "signal made on bar 1");
    assert_ne!(
        fill.fill_ts, fill.signal_ts,
        "fill_ts must never equal signal_ts — same-bar fill is forbidden"
    );
    assert_eq!(fill.fill_ts, bar2.end_ts, "fill priced from bar 2");
}

#[test]
fn buy_fill_does_not_use_open_price_of_fill_bar() {
    let bar1 = distinct_ohlc_bar("SPY", 1_700_000_060);
    let bar2 = distinct_ohlc_bar2("SPY", 1_700_000_120);
    let bar2_open = bar2.open_micros;

    let mut engine = BacktestEngine::new(default_cfg());
    engine.add_strategy(Box::new(BuyOnBar1::new())).unwrap();
    let report = engine.run(&[bar1, bar2]).unwrap();

    assert_eq!(report.fills.len(), 1);
    let fill = &report.fills[0];
    assert_eq!(fill.side, PfSide::Buy);
    assert_ne!(
        fill.price_micros, bar2_open,
        "BUY fill must NOT use the fill bar's OPEN price ({})",
        bar2_open
    );
}

#[test]
fn buy_fill_does_not_use_close_price_of_fill_bar() {
    let bar1 = distinct_ohlc_bar("SPY", 1_700_000_060);
    let bar2 = distinct_ohlc_bar2("SPY", 1_700_000_120);
    let bar2_close = bar2.close_micros;

    let mut engine = BacktestEngine::new(default_cfg());
    engine.add_strategy(Box::new(BuyOnBar1::new())).unwrap();
    let report = engine.run(&[bar1, bar2]).unwrap();

    assert_eq!(report.fills.len(), 1);
    let fill = &report.fills[0];
    assert_eq!(fill.side, PfSide::Buy);
    assert_ne!(
        fill.price_micros, bar2_close,
        "BUY fill must NOT use the fill bar's CLOSE price ({})",
        bar2_close
    );
}

// ---------------------------------------------------------------------------
// 7/8/9: SELL — no same-bar fill; fill price ≠ OPEN/CLOSE of the fill bar
// ---------------------------------------------------------------------------

#[test]
fn sell_signal_and_fill_are_never_the_same_bar() {
    // bar1: buy signal (fills bar2). bar2: sell signal, once the buy has
    // resolved (fills bar3).
    let bar1 = distinct_ohlc_bar("SPY", 1_700_000_060);
    let bar2 = distinct_ohlc_bar2("SPY", 1_700_000_120);
    let bar3 = BacktestBar::new(
        "SPY",
        1_700_000_180,
        506_000_000, // open
        517_000_000, // high
        486_000_000, // low   ← SELL fill expected here
        507_000_000, // close
        1_000,
    );

    let mut engine = BacktestEngine::new(default_cfg());
    engine
        .add_strategy(Box::new(BuyBar1SellBar2::new()))
        .unwrap();
    let report = engine
        .run(&[bar1.clone(), bar2.clone(), bar3.clone()])
        .unwrap();

    assert_eq!(report.fills.len(), 2, "expected buy + sell fills");
    let sell_fill = report
        .fills
        .iter()
        .find(|f| f.side == PfSide::Sell)
        .expect("no sell fill found");
    assert_eq!(sell_fill.signal_ts, bar2.end_ts, "sell decision made on bar 2");
    assert_ne!(
        sell_fill.fill_ts, sell_fill.signal_ts,
        "fill_ts must never equal signal_ts — same-bar fill is forbidden"
    );
    assert_eq!(sell_fill.fill_ts, bar3.end_ts, "sell priced from bar 3");
}

#[test]
fn sell_fill_does_not_use_open_price_of_fill_bar() {
    let bar1 = distinct_ohlc_bar("SPY", 1_700_000_060);
    let bar2 = distinct_ohlc_bar2("SPY", 1_700_000_120);
    let bar3 = BacktestBar::new(
        "SPY",
        1_700_000_180,
        506_000_000, // open  ← must not be used for the fill
        517_000_000, // high
        486_000_000, // low   ← SELL fill expected here
        507_000_000, // close
        1_000,
    );
    let bar3_open = bar3.open_micros;

    let mut engine = BacktestEngine::new(default_cfg());
    engine
        .add_strategy(Box::new(BuyBar1SellBar2::new()))
        .unwrap();
    let report = engine.run(&[bar1, bar2, bar3]).unwrap();

    let sell_fill = report
        .fills
        .iter()
        .find(|f| f.side == PfSide::Sell)
        .expect("no sell fill found");
    assert_ne!(
        sell_fill.price_micros, bar3_open,
        "SELL fill must NOT use the fill bar's OPEN price ({})",
        bar3_open
    );
}

#[test]
fn sell_fill_does_not_use_close_price_of_fill_bar() {
    let bar1 = distinct_ohlc_bar("SPY", 1_700_000_060);
    let bar2 = distinct_ohlc_bar2("SPY", 1_700_000_120);
    let bar3 = BacktestBar::new(
        "SPY",
        1_700_000_180,
        506_000_000, // open
        517_000_000, // high
        486_000_000, // low   ← SELL fill expected here
        507_000_000, // close ← must not be used for the fill
        1_000,
    );
    let bar3_close = bar3.close_micros;

    let mut engine = BacktestEngine::new(default_cfg());
    engine
        .add_strategy(Box::new(BuyBar1SellBar2::new()))
        .unwrap();
    let report = engine.run(&[bar1, bar2, bar3]).unwrap();

    let sell_fill = report
        .fills
        .iter()
        .find(|f| f.side == PfSide::Sell)
        .expect("no sell fill found");
    assert_ne!(
        sell_fill.price_micros, bar3_close,
        "SELL fill must NOT use the fill bar's CLOSE price ({})",
        bar3_close
    );
}
