use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, StressProfile};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

/// FlipOnce: buys at bar 1, flattens at bar 2.
struct FlipOnce {
    bar_idx: u64,
}

impl FlipOnce {
    fn new() -> Self {
        Self { bar_idx: 0 }
    }
}

impl Strategy for FlipOnce {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("FlipOnce", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", 100)]),
            2 => StrategyOutput::new(vec![TargetPosition::new("SPY", 0)]),
            _ => StrategyOutput::new(vec![]),
        }
    }
}

fn make_bars() -> Vec<BacktestBar> {
    (0..3)
        .map(|i| {
            BacktestBar::new(
                "SPY",
                1_700_000_000 + (i + 1) * 60,
                500_000_000, // open
                510_000_000, // high
                490_000_000, // low
                505_000_000, // close
                1000,
            )
        })
        .collect()
}

#[test]
fn stress_slippage_worsens_equity() {
    let bars = make_bars();

    // Base config: no slippage
    let mut base_cfg = BacktestConfig::test_defaults();
    base_cfg.stress = StressProfile {
        slippage_bps: 0,
        volatility_mult_bps: 0,
    };

    let mut engine_base = BacktestEngine::new(base_cfg);
    engine_base.add_strategy(Box::new(FlipOnce::new())).unwrap();
    let base_report = engine_base.run(&bars).unwrap();

    // Stressed config: 200 bps slippage (2%)
    let mut stressed_cfg = BacktestConfig::test_defaults();
    stressed_cfg.stress = StressProfile {
        slippage_bps: 200,
        volatility_mult_bps: 0,
    };

    let mut engine_stressed = BacktestEngine::new(stressed_cfg);
    engine_stressed
        .add_strategy(Box::new(FlipOnce::new()))
        .unwrap();
    let stressed_report = engine_stressed.run(&bars).unwrap();

    // Both should have fills
    assert!(!base_report.fills.is_empty());
    assert!(!stressed_report.fills.is_empty());

    // Final equity (last point in curve)
    let base_final = base_report.equity_curve.last().unwrap().1;
    let stressed_final = stressed_report.equity_curve.last().unwrap().1;

    // Stressed equity must be strictly worse (lower) than base
    assert!(
        stressed_final < base_final,
        "stressed equity ({}) should be lower than base equity ({})",
        stressed_final,
        base_final,
    );
}
