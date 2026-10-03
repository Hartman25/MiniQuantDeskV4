//! Native-strategy signal stream emitter.
//!
//! Runs a native [`Strategy`] through the real [`BacktestEngine`] and records
//! the target the strategy emitted at every bar it was evaluated on. The
//! executable strategy implementation therefore stays the single source of
//! truth: Research consumes this stream as the strategy's out-of-sample
//! decision series and never re-implements the rule.
//!
//! Protocol `native_strategy_signal_stream_v2` supports exactly one target per
//! evaluated bar, for one symbol. Anything else fails closed rather than being
//! interpreted. `target_qty_micros` is the strategy's ABSOLUTE portfolio target
//! (`TargetPosition.qty`; production derives `delta = target - current`), carried
//! as an exact quantity and never as a direction or a weight. v2 supersedes v1,
//! whose `bar_history_len` recorded the configured length even when the engine
//! supplied a longer window.

use std::sync::{Arc, Mutex};

use uuid::Uuid;

use mqk_execution::StrategyOutput;
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

use crate::{effective_history_len, BacktestBar, BacktestConfig, BacktestEngine, BacktestError};

pub const NATIVE_SIGNAL_STREAM_PROTOCOL_ID: &str = "native_strategy_signal_stream_v2";
/// Meaning of `target_qty_micros` in this protocol.
pub const NATIVE_SIGNAL_QUANTITY_SEMANTICS_ID: &str = "absolute_target_qty_micros_v1";

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
    /// `BacktestConfig::bar_history_len` as configured.
    pub configured_bar_history_len: usize,
    /// `Strategy::required_history_bars()`.
    pub required_history_bars: usize,
    /// The window length the engine actually supplied
    /// (`effective_history_len(configured, required)`).
    pub effective_bar_history_len: usize,
    /// The largest window the strategy was actually handed during the run.
    pub observed_max_window_len: usize,
    /// `BacktestConfig::initial_cash_micros` of the emitting run.
    pub initial_cash_micros: i64,
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
    window_len: usize,
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

    fn required_history_bars(&self) -> usize {
        self.inner.required_history_bars()
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
                window_len: ctx.recent.bars.len(),
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
    if config.sizing_policy.is_capital_fraction() {
        return Err(NativeSignalError::Backtest(
            BacktestError::InvalidSizingPolicy {
                reason: "native signal streams carry fixed-quantity targets and cannot describe a capital-fraction run".to_string(),
            },
        ));
    }
    let spec = strategy.spec();
    let semantic_fingerprint = strategy.semantic_fingerprint();
    let timeframe_secs = config.timeframe_secs;
    let configured_bar_history_len = config.bar_history_len;
    let required_history_bars = strategy.required_history_bars();
    let effective_bar_history_len =
        effective_history_len(configured_bar_history_len, required_history_bars);
    let initial_cash_micros = config.initial_cash_micros;

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
    let observed_max_window_len = recorded.iter().map(|r| r.window_len).max().unwrap_or(0);
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
        configured_bar_history_len,
        required_history_bars,
        effective_bar_history_len,
        observed_max_window_len,
        initial_cash_micros,
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
        closes.extend([120_000_000_i64; 10]);
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

    fn rising(n: usize) -> Vec<BacktestBar> {
        let closes: Vec<i64> = (0..n as i64).map(|i| 100_000_000 + i * 10_000).collect();
        series(&closes)
    }

    /// IR-3: the recorded history length is the window the engine actually
    /// supplied, not the configured default.
    #[test]
    fn history_provenance_is_the_effective_window_the_strategy_received() {
        for (name, required) in [
            ("absolute_momentum_252", 253),
            ("near_high_momentum_252_3pct", 252),
            ("trend_pullback_5d_4pct_hold5", 204),
            ("dual_sma_50_200_trend", 200),
        ] {
            let stream = emit_native_signal_stream(cfg(), &rising(320), strategy(name)).unwrap();
            assert_eq!(stream.configured_bar_history_len, 50, "{name}");
            assert_eq!(stream.required_history_bars, required, "{name}");
            assert_eq!(stream.effective_bar_history_len, required, "{name}");
            assert_eq!(
                stream.observed_max_window_len, required,
                "{name}: the window actually handed to the strategy"
            );
            assert_eq!(stream.initial_cash_micros, cfg().initial_cash_micros);
        }
    }

    struct ShortLookback;
    impl Strategy for ShortLookback {
        fn spec(&self) -> StrategySpec {
            StrategySpec::new("short_lookback", 86_400)
        }
        fn required_history_bars(&self) -> usize {
            10
        }
        fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
            use mqk_execution::QtyMicros;
            use mqk_strategy::TargetPosition;
            StrategyOutput {
                targets: vec![TargetPosition::new("SPY", QtyMicros::ZERO)],
            }
        }
    }

    /// A strategy whose requirement is below the configured default keeps the
    /// configured window (and says so).
    #[test]
    fn a_requirement_below_the_configured_default_keeps_the_configured_window() {
        let stream =
            emit_native_signal_stream(cfg(), &rising(120), Box::new(ShortLookback)).unwrap();
        assert_eq!(stream.required_history_bars, 10);
        assert_eq!(stream.configured_bar_history_len, 50);
        assert_eq!(stream.effective_bar_history_len, 50);
        assert_eq!(stream.observed_max_window_len, 50);
    }

    #[test]
    fn effective_history_len_is_the_larger_of_configured_and_required() {
        assert_eq!(effective_history_len(50, 253), 253);
        assert_eq!(effective_history_len(50, 20), 50);
        assert_eq!(effective_history_len(50, 0), 50);
        assert_eq!(effective_history_len(50, 50), 50);
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
