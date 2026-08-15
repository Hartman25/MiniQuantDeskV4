use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

/// BigBuyOnce: attempts to buy far beyond allocation cap at bar 1.
struct BigBuyOnce {
    bar_idx: u64,
}

impl BigBuyOnce {
    fn new() -> Self {
        Self { bar_idx: 0 }
    }
}

impl Strategy for BigBuyOnce {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BigBuyOnce", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", 1000)]),
            _ => StrategyOutput::new(vec![]),
        }
    }
}

#[test]
fn allocation_cap_rejects_risk_increasing_intent() {
    // BKT-FUTURE-EXECUTION-01: the allocation cap is checked at fill time
    // (using the actual fill price), not at signal time -- so the pending
    // order needs a later bar of its own symbol to reach that check at all.
    // Two identical flat bars: bar 1 signals, bar 2 is where the cap check
    // (and the worst-case BUY-at-HIGH pricing) actually happens.
    let bars = vec![
        BacktestBar::new(
            "SPY",
            1_700_000_060,
            500_000_000,
            500_000_000,
            500_000_000,
            500_000_000,
            1000,
        ),
        BacktestBar::new(
            "SPY",
            1_700_000_120,
            500_000_000,
            500_000_000,
            500_000_000,
            500_000_000,
            1000,
        ),
    ];

    let mut cfg = BacktestConfig::test_defaults();
    // Initial equity is 100k (from defaults). Set cap to 10% (0.10).
    cfg.max_gross_exposure_mult_micros = 100_000; // 0.10 * 1e6

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BigBuyOnce::new())).unwrap();
    let report = engine.run(&bars).unwrap();

    // The intent should be rejected due to allocation cap. No fills.
    assert_eq!(report.fills.len(), 0);
    assert_eq!(report.orders.len(), 1);
    assert_eq!(
        report.orders[0].status,
        mqk_backtest::OrderStatus::Rejected
    );
}
