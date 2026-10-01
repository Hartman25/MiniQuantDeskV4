//! Native-strategy signal stream emitter.
//!
//! Runs a native [`Strategy`] through the real [`BacktestEngine`] and records
//! the target the strategy emitted at every bar it was evaluated on. The
//! executable strategy implementation therefore stays the single source of
//! truth: Research consumes this stream as the strategy's out-of-sample
//! decision series and never re-implements the rule.
//!
//! Protocol `native_strategy_signal_stream_v1` supports exactly one target per
//! evaluated bar, for one symbol. Anything else fails closed rather than being
//! interpreted.

use std::sync::{Arc, Mutex};

use uuid::Uuid;

use mqk_execution::StrategyOutput;
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

use crate::{BacktestBar, BacktestConfig, BacktestEngine, BacktestError};

pub const NATIVE_SIGNAL_STREAM_PROTOCOL_ID: &str = "native_strategy_signal_stream_v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeSignalRow {
    /// Epoch seconds of the evaluated bar's end: the decision timestamp.
    pub decision_ts: i64,
    pub target_qty_micros: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeSignalStream {
    pub strategy_name: String,
    pub semantic_fingerprint: String,
    pub symbol: String,
    pub timeframe_secs: i64,
    pub bar_history_len: usize,
    /// The engine run identity for the same strategy/config/bars/execution
    /// model; equal to a plain `BacktestEngine` run of the unwrapped strategy.
    pub run_id: Uuid,
    pub rows: Vec<NativeSignalRow>,
}

#[derive(Debug)]
pub enum NativeSignalError {
    Backtest(BacktestError),
    /// A bar produced a target vector that is not exactly one entry.
    UnsupportedTargetShape {
        decision_ts: i64,
        target_count: usize,
    },
    /// Targets named more than one symbol across the run.
    MultipleSymbols {
        first: String,
        other: String,
    },
    /// The strategy was never evaluated on any bar.
    NoSignals,
    /// The recorder lock was poisoned.
    RecorderPoisoned,
}

impl std::fmt::Display for NativeSignalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for NativeSignalError {}

struct Recorded {
    decision_ts: i64,
    symbol: String,
    qty_micros: i64,
    target_count: usize,
}

struct SignalRecorder {
    inner: Box<dyn Strategy>,
    log: Arc<Mutex<Vec<Recorded>>>,
}

impl Strategy for SignalRecorder {
    fn spec(&self) -> StrategySpec {
        self.inner.spec()
    }

    fn semantic_fingerprint(&self) -> String {
        self.inner.semantic_fingerprint()
    }

    fn empty_output_is_noop(&self) -> bool {
        self.inner.empty_output_is_noop()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let out = self.inner.on_bar(ctx);
        let decision_ts = ctx.recent.bars.last().map(|b| b.end_ts).unwrap_or(0);
        let (symbol, qty_micros) = out
            .targets
            .first()
            .map(|t| (t.symbol.clone(), t.qty.raw()))
            .unwrap_or_default();
        if let Ok(mut log) = self.log.lock() {
            log.push(Recorded {
                decision_ts,
                symbol,
                qty_micros,
                target_count: out.targets.len(),
            });
        }
        out
    }
}

/// Run `strategy` over `bars` through the real engine and return its emitted
/// target stream.
pub fn emit_native_signal_stream(
    config: BacktestConfig,
    bars: &[BacktestBar],
    strategy: Box<dyn Strategy>,
) -> Result<NativeSignalStream, NativeSignalError> {
    let spec = strategy.spec();
    let semantic_fingerprint = strategy.semantic_fingerprint();
    let timeframe_secs = config.timeframe_secs;
    let bar_history_len = config.bar_history_len;

    let log: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = SignalRecorder {
        inner: strategy,
        log: Arc::clone(&log),
    };

    let mut engine = BacktestEngine::new(config);
    engine
        .add_strategy(Box::new(recorder))
        .map_err(NativeSignalError::Backtest)?;
    let report = engine.run(bars).map_err(NativeSignalError::Backtest)?;

    let recorded = log
        .lock()
        .map_err(|_| NativeSignalError::RecorderPoisoned)?;
    let mut symbol: Option<String> = None;
    let mut rows = Vec::with_capacity(recorded.len());
    for r in recorded.iter() {
        if r.target_count != 1 {
            return Err(NativeSignalError::UnsupportedTargetShape {
                decision_ts: r.decision_ts,
                target_count: r.target_count,
            });
        }
        match &symbol {
            None => symbol = Some(r.symbol.clone()),
            Some(first) if *first != r.symbol => {
                return Err(NativeSignalError::MultipleSymbols {
                    first: first.clone(),
                    other: r.symbol.clone(),
                })
            }
            Some(_) => {}
        }
        rows.push(NativeSignalRow {
            decision_ts: r.decision_ts,
            target_qty_micros: r.qty_micros,
        });
    }
    let symbol = symbol.ok_or(NativeSignalError::NoSignals)?;

    Ok(NativeSignalStream {
        strategy_name: spec.name,
        semantic_fingerprint,
        symbol,
        timeframe_secs,
        bar_history_len,
        run_id: report.run_id,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mqk_strategy::engines::register_builtin_strategies_with_sizing;
    use mqk_strategy::PluginRegistry;

    const DAY: i64 = 86_400;

    fn series(closes: &[i64]) -> Vec<BacktestBar> {
        closes
            .iter()
            .enumerate()
            .map(|(i, &c)| {
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

    fn strategy(name: &str) -> Box<dyn Strategy> {
        let mut reg = PluginRegistry::new();
        register_builtin_strategies_with_sizing(&mut reg, "SPY", 1, None, None).unwrap();
        reg.instantiate(name).unwrap()
    }

    /// 60 flat closes at $100 then a step up to $120: the trend filter is
    /// flat through the flat segment and long from the step bar onward.
    fn flat_then_step() -> Vec<BacktestBar> {
        let mut closes = vec![100_000_000; 60];
        closes.extend(std::iter::repeat(120_000_000).take(10));
        series(&closes)
    }

    #[test]
    fn stream_matches_the_unwrapped_engine_run_identity() {
        let bars = flat_then_step();
        let stream = emit_native_signal_stream(cfg(), &bars, strategy("trend_sma50")).unwrap();

        let mut engine = BacktestEngine::new(cfg());
        engine.add_strategy(strategy("trend_sma50")).unwrap();
        let plain = engine.run(&bars).unwrap();

        assert_eq!(
            stream.run_id, plain.run_id,
            "wrapper must not alter identity"
        );
        assert_eq!(stream.strategy_name, "trend_sma50");
        assert_eq!(stream.symbol, "SPY");
        assert_eq!(stream.rows.len(), bars.len());
        assert_eq!(
            stream.semantic_fingerprint,
            strategy("trend_sma50").semantic_fingerprint()
        );
    }

    #[test]
    fn stream_is_the_strategy_decision_series() {
        let bars = flat_then_step();
        let stream = emit_native_signal_stream(cfg(), &bars, strategy("trend_sma50")).unwrap();
        for (i, row) in stream.rows.iter().enumerate() {
            assert_eq!(row.decision_ts, DAY * (i as i64 + 1));
            let expect_long = i >= 60;
            assert_eq!(
                row.target_qty_micros,
                if expect_long { 1_000_000 } else { 0 },
                "bar {i}"
            );
        }
    }

    #[test]
    fn stream_is_deterministic() {
        let bars = flat_then_step();
        let a = emit_native_signal_stream(cfg(), &bars, strategy("trend_sma50")).unwrap();
        let b = emit_native_signal_stream(cfg(), &bars, strategy("trend_sma50")).unwrap();
        assert_eq!(a, b);
    }

    struct TwoTargets;
    impl Strategy for TwoTargets {
        fn spec(&self) -> StrategySpec {
            StrategySpec::new("two_targets", 86_400)
        }
        fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
            use mqk_execution::QtyMicros;
            use mqk_strategy::TargetPosition;
            StrategyOutput {
                targets: vec![
                    TargetPosition::new("SPY", QtyMicros::ZERO),
                    TargetPosition::new("EFA", QtyMicros::ZERO),
                ],
            }
        }
    }

    #[test]
    fn multi_target_output_fails_closed() {
        let bars = flat_then_step();
        let err = emit_native_signal_stream(cfg(), &bars, Box::new(TwoTargets)).unwrap_err();
        assert!(matches!(
            err,
            NativeSignalError::UnsupportedTargetShape {
                target_count: 2,
                ..
            }
        ));
    }
}
