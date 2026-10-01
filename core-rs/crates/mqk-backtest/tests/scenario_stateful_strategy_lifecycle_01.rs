//! A stateful strategy must keep one instance across sequential bars inside the
//! real BacktestEngine and the native signal emitter, and the registered
//! pullback engine must reproduce its own bar-by-bar decisions through them.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use mqk_backtest::{emit_native_signal_stream, BacktestBar, BacktestConfig, BacktestEngine};
use mqk_strategy::engines::register_builtin_strategies_with_sizing;
use mqk_strategy::{
    PluginRegistry, RecentBarsWindow, Strategy, StrategyContext, StrategyOutput, StrategySpec,
    TargetPosition,
};

const DAY: i64 = 86_400;

fn cfg() -> BacktestConfig {
    let mut c = BacktestConfig::conservative_defaults();
    c.timeframe_secs = DAY;
    c.integrity_enabled = false;
    c
}

/// Quiet noise around 100M with a sharp dip every 23 bars and a slow recovery.
fn wave_closes(n: usize) -> Vec<i64> {
    (0..n)
        .map(|i| {
            let noise = (((i * 41) % 31) as i64 - 15) * 300_000;
            match i % 23 {
                5 => 70_000_000,
                6 => 85_000_000,
                7 => 92_000_000,
                _ => 100_000_000 + noise,
            }
        })
        .collect()
}

fn bars(closes: &[i64]) -> Vec<BacktestBar> {
    closes
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            BacktestBar::new(
                "SPY",
                DAY * (i as i64 + 1),
                c,
                c + 500_000,
                c - 500_000,
                c,
                1_000,
            )
        })
        .collect()
}

/// Counts its own calls: any reconstruction between bars would reset it.
struct CallCounter {
    calls: usize,
    seen: Arc<AtomicUsize>,
}

impl Strategy for CallCounter {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("call_counter", DAY)
    }
    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.calls += 1;
        self.seen.store(self.calls, Ordering::SeqCst);
        StrategyOutput {
            targets: vec![TargetPosition::new(
                "SPY",
                mqk_execution::QtyMicros::from_whole_units((self.calls % 2) as i64).unwrap(),
            )],
        }
    }
}

#[test]
fn engine_and_emitter_keep_one_instance_across_sequential_bars() {
    let b = bars(&wave_closes(60));
    let seen = Arc::new(AtomicUsize::new(0));
    let stream = emit_native_signal_stream(
        cfg(),
        &b,
        Box::new(CallCounter {
            calls: 0,
            seen: Arc::clone(&seen),
        }),
    )
    .unwrap();
    assert_eq!(
        seen.load(Ordering::SeqCst),
        60,
        "every bar reached the same instance"
    );
    for (i, row) in stream.rows.iter().enumerate() {
        // call number i+1: odd -> long. A per-bar rebuild would always emit 1.
        assert_eq!(
            row.target_qty_micros,
            if (i + 1) % 2 == 1 { 1_000_000 } else { 0 },
            "bar {i}"
        );
    }

    let seen2 = Arc::new(AtomicUsize::new(0));
    let mut engine = BacktestEngine::new(cfg());
    engine
        .add_strategy(Box::new(CallCounter {
            calls: 0,
            seen: Arc::clone(&seen2),
        }))
        .unwrap();
    engine.run(&b).unwrap();
    assert_eq!(seen2.load(Ordering::SeqCst), 60);
}

fn registered(name: &str) -> Box<dyn Strategy> {
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    reg.instantiate(name).unwrap()
}

#[test]
fn pullback_stream_through_the_real_engine_equals_its_own_sequential_decisions() {
    let closes = wave_closes(260);
    let b = bars(&closes);
    let stream =
        emit_native_signal_stream(cfg(), &b, registered("pullback_mean_reversion_20_2")).unwrap();
    assert_eq!(stream.rows.len(), closes.len());

    // Direct sequential evaluation of the same registered engine with the window the engine uses.
    let history = cfg()
        .bar_history_len
        .max(registered("pullback_mean_reversion_20_2").required_history_bars());
    let mut direct = registered("pullback_mean_reversion_20_2");
    let mut window: Vec<mqk_strategy::BarStub> = Vec::new();
    let mut expect = Vec::new();
    for x in &b {
        window.push(mqk_strategy::BarStub::with_ohlcv(
            x.end_ts,
            true,
            x.open_micros,
            x.high_micros,
            x.low_micros,
            x.close_micros,
            x.volume,
        ));
        if window.len() > history {
            window.remove(0);
        }
        let ctx = StrategyContext::new(DAY, 0, RecentBarsWindow::new(history, window.clone()));
        expect.push(direct.on_bar(&ctx).targets[0].qty.raw());
    }
    let got: Vec<i64> = stream.rows.iter().map(|r| r.target_qty_micros).collect();
    assert_eq!(got, expect);
    assert!(
        got.iter().any(|&q| q > 0) && got.contains(&0),
        "fixture exercises both states"
    );
    assert!(got.iter().all(|&q| q == 0 || q == 1_000_000), "never short");
    assert_eq!(
        stream.semantic_fingerprint,
        registered("pullback_mean_reversion_20_2").semantic_fingerprint()
    );
}

#[test]
fn the_registered_pullback_requirement_is_the_true_lookback() {
    let s = registered("pullback_mean_reversion_20_2");
    assert_eq!(s.required_history_bars(), 20);
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
    let meta = reg
        .list()
        .into_iter()
        .find(|m| m.name == "pullback_mean_reversion_20_2")
        .unwrap()
        .clone();
    assert_eq!(meta.data_requirements.unwrap().minimum_completed_bars, 20);
    assert_eq!(meta.timeframe_secs, DAY);
}
