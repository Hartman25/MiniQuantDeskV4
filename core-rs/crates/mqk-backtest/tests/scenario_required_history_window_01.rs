//! A strategy that declares `required_history_bars()` must be shown at least that
//! many completed bars by the backtest engine, through every wrapper the
//! emitter and robustness runs put around it. Without this a 200-bar rule would
//! see the default 50-bar window and stay silently flat.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use mqk_backtest::{emit_native_signal_stream, BacktestBar, BacktestConfig, BacktestEngine};
use mqk_strategy::{Strategy, StrategyContext, StrategyOutput, StrategySpec, TargetPosition};

const DAY: i64 = 86_400;

struct NeedsHistory {
    required: usize,
    widest_seen: Arc<AtomicUsize>,
}

impl Strategy for NeedsHistory {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("needs_history", DAY)
    }
    fn required_history_bars(&self) -> usize {
        self.required
    }
    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        self.widest_seen
            .fetch_max(ctx.recent.len(), Ordering::SeqCst);
        StrategyOutput {
            targets: vec![TargetPosition::new("SPY", mqk_execution::QtyMicros::ZERO)],
        }
    }
}

fn bars(n: usize) -> Vec<BacktestBar> {
    (0..n)
        .map(|i| {
            let c = 100_000_000 + (i as i64 % 9) * 1_000_000;
            BacktestBar::new(
                "SPY",
                DAY * (i as i64 + 1),
                c,
                c + 1_000_000,
                c - 1_000_000,
                c,
                1_000,
            )
        })
        .collect()
}

fn cfg() -> BacktestConfig {
    let mut c = BacktestConfig::conservative_defaults();
    c.timeframe_secs = DAY;
    c.integrity_enabled = false;
    c
}

fn widest(required: usize, wrap_with_emitter: bool) -> usize {
    let seen = Arc::new(AtomicUsize::new(0));
    let s = Box::new(NeedsHistory {
        required,
        widest_seen: Arc::clone(&seen),
    });
    if wrap_with_emitter {
        emit_native_signal_stream(cfg(), &bars(300), s).unwrap();
    } else {
        let mut e = BacktestEngine::new(cfg());
        e.add_strategy(s).unwrap();
        e.run(&bars(300)).unwrap();
    }
    seen.load(Ordering::SeqCst)
}

#[test]
fn engine_window_widens_to_the_declared_requirement() {
    assert_eq!(widest(200, false), 200);
}

#[test]
fn undeclared_or_smaller_requirement_keeps_the_configured_window() {
    // Mutation control: with no requirement the old 50-bar window is unchanged.
    assert_eq!(
        widest(0, false),
        BacktestConfig::conservative_defaults().bar_history_len
    );
    assert_eq!(
        widest(10, false),
        BacktestConfig::conservative_defaults().bar_history_len
    );
}

#[test]
fn the_native_signal_recorder_forwards_the_requirement() {
    assert_eq!(widest(200, true), 200);
}
