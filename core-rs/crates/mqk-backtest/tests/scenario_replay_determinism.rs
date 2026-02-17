use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

/// BuyHoldSell: buys 10 shares at bar 1, sells at bar 3.
struct BuyHoldSell {
    bar_idx: u64,
}

impl BuyHoldSell {
    fn new() -> Self {
        Self { bar_idx: 0 }
    }
}

impl Strategy for BuyHoldSell {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BuyHoldSell", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", 10)]),
            3 => StrategyOutput::new(vec![TargetPosition::new("SPY", 0)]),
            _ => StrategyOutput::new(vec![]),
        }
    }
}

fn make_bars() -> Vec<BacktestBar> {
    (0..5)
        .map(|i| {
            BacktestBar::new(
                "SPY",
                1_700_000_000 + (i + 1) * 60,
                500_000_000,  // open
                510_000_000,  // high
                490_000_000,  // low
                505_000_000,  // close
                1000,
            )
        })
        .collect()
}

#[test]
fn replay_determinism_identical_results() {
    let bars = make_bars();
    let cfg = BacktestConfig::test_defaults();

    // Run 1
    let mut engine1 = BacktestEngine::new(cfg.clone());
    engine1
        .add_strategy(Box::new(BuyHoldSell::new()))
        .unwrap();
    let report1 = engine1.run(&bars).unwrap();

    // Run 2
    let mut engine2 = BacktestEngine::new(cfg);
    engine2
        .add_strategy(Box::new(BuyHoldSell::new()))
        .unwrap();
    let report2 = engine2.run(&bars).unwrap();

    // Determinism: identical results
    assert_eq!(report1.halted, report2.halted);
    assert_eq!(report1.halt_reason, report2.halt_reason);
    assert_eq!(report1.fills, report2.fills);
    assert_eq!(report1.equity_curve, report2.equity_curve);

    // Sanity: we should have fills (buy + sell)
    assert!(
        report1.fills.len() >= 2,
        "expected at least 2 fills, got {}",
        report1.fills.len()
    );
}
