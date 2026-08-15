use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_portfolio::Side as PfSide;
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

/// BuyOnce: purchases 10 shares at bar 1 (fills at bar 2, the first later
/// SPY bar — BKT-FUTURE-EXECUTION-01).
struct BuyOnce {
    bar_idx: u64,
}

impl BuyOnce {
    fn new() -> Self {
        Self { bar_idx: 0 }
    }
}

impl Strategy for BuyOnce {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BuyOnce", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", 10)]),
            _ => StrategyOutput::new(vec![]),
        }
    }
}

#[test]
fn buy_fills_at_high_not_close() {
    // bar 1 signals the buy; it has no bearing on the fill price.
    let bar1 = BacktestBar::new(
        "SPY",
        1_700_000_060,
        500_000_000,
        500_000_000,
        500_000_000,
        500_000_000,
        1000,
    );

    // bar 2 is the first later SPY bar — this is what must price the fill.
    let high_micros = 510_000_000;
    let close_micros = 505_000_000;
    let bar2 = BacktestBar::new(
        "SPY",
        1_700_000_120,
        500_000_000, // open
        high_micros, // high
        490_000_000, // low
        close_micros, // close
        1000,
    );

    let cfg = BacktestConfig::test_defaults();
    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BuyOnce::new())).unwrap();
    let report = engine.run(&[bar1.clone(), bar2.clone()]).unwrap();

    // Should have exactly one fill (buy)
    assert_eq!(report.fills.len(), 1, "expected exactly 1 fill");
    let fill = &report.fills[0];
    assert_eq!(fill.side, PfSide::Buy);
    assert_eq!(fill.qty, 10);
    assert_eq!(fill.signal_ts, bar1.end_ts, "signal made on bar 1");
    assert_eq!(fill.fill_ts, bar2.end_ts, "priced from bar 2, not bar 1");

    // Ambiguity worst-case: BUY fills at bar 2's HIGH, not CLOSE
    assert_eq!(
        fill.price_micros, high_micros,
        "BUY should fill at bar 2's HIGH price ({}) not CLOSE ({})",
        high_micros, close_micros,
    );
}

/// SellOnce: buys 10 shares at bar 1 (fills bar 2), sells them at bar 2
/// (fills bar 3).
struct SellOnce {
    bar_idx: u64,
}

impl SellOnce {
    fn new() -> Self {
        Self { bar_idx: 0 }
    }
}

impl Strategy for SellOnce {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("SellOnce", 60)
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

#[test]
fn sell_fills_at_low_not_close() {
    let low_micros = 490_000_000;
    let close_micros = 505_000_000;

    let bars = vec![
        // bar 1: buy signal (fills bar 2).
        BacktestBar::new(
            "SPY",
            1_700_000_060,
            500_000_000,
            510_000_000,
            500_000_000,
            close_micros,
            1000,
        ),
        // bar 2: buy fills here; also carries the sell signal (fills bar 3).
        BacktestBar::new(
            "SPY",
            1_700_000_120,
            505_000_000,
            515_000_000,
            500_000_000,
            510_000_000,
            1000,
        ),
        // bar 3: the only later SPY bar after the sell signal — must price the sell fill.
        BacktestBar::new(
            "SPY",
            1_700_000_180,
            505_000_000,
            512_000_000,
            low_micros,
            close_micros,
            1000,
        ),
    ];

    let cfg = BacktestConfig::test_defaults();
    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(SellOnce::new())).unwrap();
    let report = engine.run(&bars).unwrap();

    // Should have two fills: buy (bar 2) and sell (bar 3).
    assert_eq!(report.fills.len(), 2, "expected exactly 2 fills");

    let sell_fill = &report.fills[1];
    assert_eq!(sell_fill.side, PfSide::Sell);
    assert_eq!(sell_fill.qty, 10);
    assert_eq!(sell_fill.signal_ts, bars[1].end_ts, "sell signalled on bar 2");
    assert_eq!(sell_fill.fill_ts, bars[2].end_ts, "sell priced from bar 3");

    // Ambiguity worst-case: SELL fills at bar 3's LOW, not CLOSE
    assert_eq!(
        sell_fill.price_micros, low_micros,
        "SELL should fill at bar 3's LOW price ({}) not CLOSE ({})",
        low_micros, close_micros,
    );
}
