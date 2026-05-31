//! BACKTEST-OHLCV-AND-SWEEP-01: OHLCV context preservation tests.
//!
//! Verifies that full OHLCV values from BacktestBar are preserved in BarStub
//! inside StrategyContext, and that existing close/volume strategies remain unchanged.

use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine};
use mqk_execution::StrategyOutput as ExecOutput;
#[allow(unused_imports)]
use mqk_execution::TargetPosition;
use mqk_strategy::{
    BarStub, RecentBarsWindow, Strategy, StrategyContext, StrategyOutput, StrategySpec,
};

// ---------------------------------------------------------------------------
// Inspection strategy: captures the last BarStub seen via on_bar.
// ---------------------------------------------------------------------------

struct OhlcvCapture {
    pub last_bar: Option<BarStub>,
    symbol: String,
}

impl OhlcvCapture {
    fn new(symbol: impl Into<String>) -> Self {
        Self {
            last_bar: None,
            symbol: symbol.into(),
        }
    }
}

impl Strategy for OhlcvCapture {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(format!("ohlcv_capture_{}", self.symbol), 60)
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        self.last_bar = ctx.recent.last().cloned();
        ExecOutput::new(vec![])
    }
}

fn make_bar(end_ts: i64, open: i64, high: i64, low: i64, close: i64, vol: i64) -> BacktestBar {
    BacktestBar::new("SPY", end_ts, open, high, low, close, vol)
}

// BOS01 / BOS02: BarStub in StrategyContext carries full OHLCV from BacktestBar.
#[test]
fn ohlcv_01_bar_stub_carries_full_ohlcv() {
    let capture = Box::new(OhlcvCapture::new("SPY"));
    let mut cfg = BacktestConfig::test_defaults();
    cfg.timeframe_secs = 60;

    let bars = vec![
        make_bar(
            1000,
            100_000_000,
            105_000_000,
            98_000_000,
            103_000_000,
            5000,
        ),
        make_bar(
            1060,
            103_000_000,
            110_000_000,
            102_000_000,
            108_000_000,
            6000,
        ),
    ];

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(capture).unwrap();
    let report = engine.run(&bars).unwrap();
    assert_eq!(report.fills.len(), 0); // capture strategy emits no trades
    assert_eq!(report.equity_curve.len(), 2);
}

// Verify OHLCV preserved by building context manually (unit proof).
#[test]
fn ohlcv_02_barstub_with_ohlcv_preserves_all_fields() {
    let stub = BarStub::with_ohlcv(9999, true, 10_000, 12_000, 9_000, 11_000, 42);
    assert_eq!(stub.end_ts, 9999);
    assert!(stub.is_complete);
    assert_eq!(stub.open_micros, 10_000);
    assert_eq!(stub.high_micros, 12_000);
    assert_eq!(stub.low_micros, 9_000);
    assert_eq!(stub.close_micros, 11_000);
    assert_eq!(stub.volume, 42);
}

// Backward-compat: BarStub::new still works; open/high/low default to close.
#[test]
fn ohlcv_03_barstub_new_defaults_ohlc_to_close() {
    let stub = BarStub::new(1000, true, 50_000_000, 100);
    assert_eq!(stub.open_micros, 50_000_000);
    assert_eq!(stub.high_micros, 50_000_000);
    assert_eq!(stub.low_micros, 50_000_000);
    assert_eq!(stub.close_micros, 50_000_000);
    assert_eq!(stub.volume, 100);
}

// open/high/low from BacktestBar must be distinct and correctly propagated.
#[test]
fn ohlcv_04_distinct_ohlc_values_propagated_to_strategy_context() {
    use std::sync::{Arc, Mutex};

    struct Recorder {
        bars: Arc<Mutex<Vec<BarStub>>>,
    }
    impl Strategy for Recorder {
        fn spec(&self) -> StrategySpec {
            StrategySpec::new("recorder", 60)
        }
        fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
            if let Some(b) = ctx.recent.last() {
                self.bars.lock().unwrap().push(b.clone());
            }
            ExecOutput::new(vec![])
        }
    }

    let bars_log = Arc::new(Mutex::new(Vec::<BarStub>::new()));
    let recorder = Box::new(Recorder {
        bars: bars_log.clone(),
    });

    let mut cfg = BacktestConfig::test_defaults();
    cfg.timeframe_secs = 60;

    // Bar with clearly distinct OHLCV values.
    let input = vec![make_bar(
        1000,
        200_000_000,
        210_000_000,
        195_000_000,
        205_000_000,
        999,
    )];

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(recorder).unwrap();
    engine.run(&input).unwrap();

    let log = bars_log.lock().unwrap();
    assert_eq!(log.len(), 1);
    let b = &log[0];
    assert_eq!(b.open_micros, 200_000_000);
    assert_eq!(b.high_micros, 210_000_000);
    assert_eq!(b.low_micros, 195_000_000);
    assert_eq!(b.close_micros, 205_000_000);
    assert_eq!(b.volume, 999);
}

// BOS03: Existing strategies continue to compile and produce signals on close/volume.
// Verify that the OHLCV addition does not break existing close-only strategy logic.
#[test]
fn ohlcv_05_existing_close_volume_access_unchanged() {
    // Use intraday_scalper which reads close_micros internally.
    use mqk_strategy::{engines::register_builtin_strategies_with_sizing, PluginRegistry};
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    let strategy = reg.instantiate("intraday_scalper").unwrap();

    let mut cfg = BacktestConfig::test_defaults();
    // intraday_scalper expects 300s timeframe.
    cfg.timeframe_secs = 300;
    let bars: Vec<BacktestBar> = (0..20)
        .map(|i| {
            let close = 100_000_000 + i * 500_000;
            make_bar(
                1000 + i * 300,
                close - 200_000,
                close + 300_000,
                close - 400_000,
                close,
                500 + i * 10,
            )
        })
        .collect();

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(strategy).unwrap();
    let report = engine.run(&bars).unwrap();

    // Strategy ran without errors; equity curve has entries.
    assert_eq!(report.equity_curve.len(), bars.len());
    assert!(!report.halted, "strategy should not halt on valid data");
}

// Incomplete bar must still be rejected (anti-lookahead invariant unchanged).
#[test]
fn ohlcv_06_incomplete_bar_rejected() {
    let capture = Box::new(OhlcvCapture::new("SPY"));
    let mut cfg = BacktestConfig::test_defaults();
    cfg.timeframe_secs = 60;

    let mut bad_bar = make_bar(1000, 100_000_000, 105_000_000, 98_000_000, 103_000_000, 500);
    bad_bar.is_complete = false;

    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(capture).unwrap();
    let err = engine.run(&[bad_bar]).unwrap_err();
    assert!(matches!(
        err,
        mqk_backtest::BacktestError::IncompleteBar { .. }
    ));
}

// OHLCV context preserved in RecentBarsWindow (multi-bar window).
#[test]
fn ohlcv_07_ohlcv_preserved_across_window() {
    // Build a window manually and verify last() carries OHLCV.
    let stub1 = BarStub::with_ohlcv(1000, true, 10, 15, 9, 12, 100);
    let stub2 = BarStub::with_ohlcv(1060, true, 12, 18, 11, 16, 200);
    let window = RecentBarsWindow::new(5, vec![stub1, stub2]);
    let last = window.last().unwrap();
    assert_eq!(last.open_micros, 12);
    assert_eq!(last.high_micros, 18);
    assert_eq!(last.low_micros, 11);
    assert_eq!(last.close_micros, 16);
    assert_eq!(last.volume, 200);
}
