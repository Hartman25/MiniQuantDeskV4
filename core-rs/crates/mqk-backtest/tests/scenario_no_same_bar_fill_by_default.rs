//! Scenario: No same-bar lookahead fills by default — Patch B1
//!
//! # Background
//!
//! "Same-bar lookahead" occurs when a signal generated on bar N fills using a
//! price that would only be knowable at (or before) the start of that bar — most
//! dangerously the bar's OPEN price (you saw the bar close, then got filled at a
//! price from the bar's opening, which is in the past) or the bar's CLOSE price
//! (the very price you used to generate the signal).
//!
//! The engine guards against lookahead in two layers that cannot be bypassed:
//!
//! 1. **Incomplete-bar gate**: bars with `is_complete = false` are rejected with
//!    `Err(BacktestError::IncompleteBar)`. The strategy never sees partial data.
//!
//! 2. **Conservative fill pricing**: fills always use the worst-case price within
//!    the completed bar (BUY @ HIGH, SELL @ LOW), not OPEN, not CLOSE.
//!    This ensures the strategy cannot "see" a close of $505 and receive a fill
//!    at $500 (OPEN), which would be a free lunch unavailable in real trading.
//!
//! # Invariants under test
//!
//! 1. Incomplete bar at position 0 → `Err(IncompleteBar)` (no fills, no report).
//! 2. Incomplete bar sandwiched between complete bars → `Err(IncompleteBar)`.
//! 3. Negative timestamp → `Err(NegativeTimestamp)`.
//! 4. BUY fill price ≠ open_micros  (OPEN lookahead is blocked).
//! 5. BUY fill price ≠ close_micros (CLOSE lookahead is blocked).
//! 6. SELL fill price ≠ open_micros.
//! 7. SELL fill price ≠ close_micros.
//!
//! The companion file `scenario_ambiguity_worst_case_enforced.rs` proves the
//! positive side: BUY fills AT HIGH and SELL fills AT LOW.
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

/// Buys 10 on bar 1, sells all on bar 2.
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
// 4. BUY fill price ≠ open_micros  (OPEN-price lookahead is blocked)
// ---------------------------------------------------------------------------

#[test]
fn buy_fill_does_not_use_open_price() {
    // open=$500, high=$515, low=$488, close=$505
    // If the engine were using OPEN, fill would be at 500_000_000.
    // Correct conservative fill is at HIGH = 515_000_000.
    let bar = distinct_ohlc_bar("SPY", 1_700_000_060);
    let open_micros = bar.open_micros;

    let mut engine = BacktestEngine::new(default_cfg());
    engine.add_strategy(Box::new(BuyOnBar1::new())).unwrap();
    let report = engine.run(&[bar]).unwrap();

    assert_eq!(report.fills.len(), 1, "expected exactly 1 buy fill");
    let fill = &report.fills[0];
    assert_eq!(fill.side, PfSide::Buy);
    assert_ne!(
        fill.price_micros, open_micros,
        "BUY fill must NOT use OPEN price ({}) — that would be same-bar lookahead",
        open_micros
    );
}

// ---------------------------------------------------------------------------
// 5. BUY fill price ≠ close_micros  (CLOSE-price lookahead is blocked)
// ---------------------------------------------------------------------------

#[test]
fn buy_fill_does_not_use_close_price() {
    // open=$500, high=$515, low=$488, close=$505
    // If the engine were using CLOSE, fill would be at 505_000_000.
    // Correct conservative fill is at HIGH = 515_000_000.
    let bar = distinct_ohlc_bar("SPY", 1_700_000_060);
    let close_micros = bar.close_micros;

    let mut engine = BacktestEngine::new(default_cfg());
    engine.add_strategy(Box::new(BuyOnBar1::new())).unwrap();
    let report = engine.run(&[bar]).unwrap();

    assert_eq!(report.fills.len(), 1);
    let fill = &report.fills[0];
    assert_eq!(fill.side, PfSide::Buy);
    assert_ne!(
        fill.price_micros, close_micros,
        "BUY fill must NOT use CLOSE price ({}) — that would be circular lookahead",
        close_micros
    );
}

// ---------------------------------------------------------------------------
// 6. SELL fill price ≠ open_micros
// ---------------------------------------------------------------------------

#[test]
fn sell_fill_does_not_use_open_price() {
    // Bar 1: buy 10 shares (any complete bar with distinct prices).
    // Bar 2: open=$506, high=$516, low=$492, close=$511
    //   If engine used OPEN for sell: fill = 506_000_000
    //   Correct conservative fill: LOW = 492_000_000
    let bar1 = distinct_ohlc_bar("SPY", 1_700_000_060);
    let bar2 = BacktestBar::new(
        "SPY",
        1_700_000_120,
        506_000_000, // open  — $506.00
        516_000_000, // high  — $516.00
        492_000_000, // low   — $492.00  ← SELL fill expected here
        511_000_000, // close — $511.00
        1_000,
    );
    let open_micros_bar2 = bar2.open_micros;

    let mut engine = BacktestEngine::new(default_cfg());
    engine
        .add_strategy(Box::new(BuyBar1SellBar2::new()))
        .unwrap();
    let report = engine.run(&[bar1, bar2]).unwrap();

    // Should have 2 fills: buy at bar1, sell at bar2
    assert_eq!(report.fills.len(), 2, "expected buy + sell fills");
    let sell_fill = report
        .fills
        .iter()
        .find(|f| f.side == PfSide::Sell)
        .expect("no sell fill found");
    assert_ne!(
        sell_fill.price_micros, open_micros_bar2,
        "SELL fill must NOT use OPEN price ({}) — that would be same-bar lookahead",
        open_micros_bar2
    );
}

// ---------------------------------------------------------------------------
// 7. SELL fill price ≠ close_micros
// ---------------------------------------------------------------------------

#[test]
fn sell_fill_does_not_use_close_price() {
    // Same setup as test 6; also verify fill ≠ close.
    let bar1 = distinct_ohlc_bar("SPY", 1_700_000_060);
    let bar2 = BacktestBar::new(
        "SPY",
        1_700_000_120,
        506_000_000, // open
        516_000_000, // high
        492_000_000, // low   ← SELL fill expected here
        511_000_000, // close
        1_000,
    );
    let close_micros_bar2 = bar2.close_micros;

    let mut engine = BacktestEngine::new(default_cfg());
    engine
        .add_strategy(Box::new(BuyBar1SellBar2::new()))
        .unwrap();
    let report = engine.run(&[bar1, bar2]).unwrap();

    let sell_fill = report
        .fills
        .iter()
        .find(|f| f.side == PfSide::Sell)
        .expect("no sell fill found");
    assert_ne!(
        sell_fill.price_micros, close_micros_bar2,
        "SELL fill must NOT use CLOSE price ({}) — that would be circular lookahead",
        close_micros_bar2
    );
}
