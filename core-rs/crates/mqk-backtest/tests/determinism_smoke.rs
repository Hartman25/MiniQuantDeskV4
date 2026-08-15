use mqk_backtest::{BacktestConfig, BacktestEngine};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

const BARS_CSV: &str = include_str!("fixtures/bkt4_bars.csv");

struct BuyHoldExit {
    spec: StrategySpec,
}

impl BuyHoldExit {
    fn new(timeframe_secs: i64) -> Self {
        Self {
            spec: StrategySpec::new("bkt4_buy_hold_exit", timeframe_secs),
        }
    }
}

impl Strategy for BuyHoldExit {
    fn spec(&self) -> StrategySpec {
        self.spec.clone()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let target_qty = if ctx.now_tick == 0 {
            10
        } else if ctx.now_tick >= 2 {
            0
        } else {
            10
        };

        StrategyOutput::new(vec![TargetPosition {
            symbol: "TEST".to_string(),
            qty: target_qty,
        }])
    }
}

#[test]
fn determinism_equity_curve_and_fills_are_stable() {
    let bars = mqk_backtest::parse_csv_bars(BARS_CSV).expect("parse fixture csv");

    let mut cfg = BacktestConfig::test_defaults();
    cfg.timeframe_secs = 60;
    cfg.initial_cash_micros = 100_000_000_000;
    cfg.shadow_mode = false;
    cfg.integrity_enabled = false;
    cfg.stress.slippage_bps = 0;
    cfg.stress.volatility_mult_bps = 0;

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BuyHoldExit::new(60))).unwrap();

    let report = engine.run(&bars).expect("run backtest");

    // BKT-FUTURE-EXECUTION-01: the tick-1 buy signal (bar end_ts=60) is
    // priced from bar end_ts=120 (the first later bar) at its HIGH
    // (1_030_000), not from bar 60's HIGH. The tick-2 sell signal (bar
    // end_ts=120, generated once the buy has resolved) is priced from bar
    // end_ts=180 at its LOW (1_000_000).
    assert_eq!(report.fills.len(), 2);

    assert_eq!(report.fills[0].symbol, "TEST");
    assert_eq!(format!("{:?}", report.fills[0].side), "Buy");
    assert_eq!(report.fills[0].qty, 10);
    assert_eq!(report.fills[0].signal_ts, 60);
    assert_eq!(report.fills[0].fill_ts, 120);
    assert_eq!(report.fills[0].price_micros, 1_030_000);
    assert_eq!(report.fills[0].fee_micros, 0);

    assert_eq!(report.fills[1].symbol, "TEST");
    assert_eq!(format!("{:?}", report.fills[1].side), "Sell");
    assert_eq!(report.fills[1].qty, 10);
    assert_eq!(report.fills[1].signal_ts, 120);
    assert_eq!(report.fills[1].fill_ts, 180);
    assert_eq!(report.fills[1].price_micros, 1_000_000);
    assert_eq!(report.fills[1].fee_micros, 0);

    let expected = vec![
        (60, 100_000_000_000),
        (120, 99_999_900_000),
        (180, 99_999_700_000),
    ];
    assert_eq!(report.equity_curve, expected);

    assert_eq!(report.last_prices.get("TEST").copied(), Some(1_015_000));
}
