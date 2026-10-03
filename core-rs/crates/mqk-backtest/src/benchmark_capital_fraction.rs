//! Capital-fraction-matched passive buy-and-hold benchmark
//! ("CapitalFractionMatchedPassiveBuyHoldV1").
//!
//! For a candidate sized by `FixedInitialCapitalFractionV1`, the benchmark
//! buys the candidate's exact FIRST resolved entry quantity `Q` once, at the
//! candidate's first causal long-decision bar, and holds to the end. It runs
//! through the real, unmodified [`crate::engine::BacktestEngine`] under the
//! candidate's own capital, commission, slippage and liquidity config (only the
//! sizing policy is swapped to fixed-quantity, because `Q` is already
//! resolved), so alpha compares two account returns on one basis.
//!
//! `Q`, the reference bar and the sizing parameters come ONLY from the
//! candidate report's recorded sizing provenance. A candidate that never
//! entered has no benchmark: the result is an error, never a fallback to the
//! legacy benchmark, Benchmark V2, a one-share position or a direction-only
//! comparison.

use std::fmt;

use mqk_execution::{QtyMicros, StrategyOutput, TargetPosition};
use mqk_strategy::{SizingPolicy, Strategy, StrategyContext, StrategySpec};

use crate::{BacktestBar, BacktestConfig, BacktestEngine, BacktestError, BacktestReport};

pub const BENCHMARK_CAPITAL_FRACTION_POLICY_ID: &str =
    "capital_fraction_matched_passive_buy_hold_v1";

#[derive(Debug, Clone, PartialEq)]
pub struct CapitalFractionBenchmarkSection {
    pub policy_id: String,
    pub symbol: String,
    pub sizing_policy_id: String,
    pub allocation_fraction_bps: i64,
    pub initial_allocated_capital_micros: i64,
    pub position_budget_micros: i64,
    pub reference_bar_end_ts: i64,
    pub reference_price_micros: i64,
    pub target_qty_micros: i64,
    pub entry_bar_index: usize,
    pub initial_cash_micros: i64,
    pub execution_model_id: String,
    pub config_id: String,
    pub benchmark_run_id: String,
    pub account_return_pct: f64,
    pub alpha_pct: f64,
}

#[derive(Debug)]
pub enum CapitalFractionBenchmarkError {
    /// The candidate config does not select the capital-fraction policy.
    CandidateNotCapitalFraction,
    /// The candidate never resolved a valid entry: nothing to match.
    CandidateNeverEntered,
    EmptyBars,
    /// Report provenance disagrees with the config (capital, fraction, policy).
    ProvenanceMismatch(&'static str),
    /// The recorded reference bar is not in the evaluated bars.
    ReferenceBarNotFound {
        reference_bar_end_ts: i64,
    },
    Backtest(BacktestError),
}

impl fmt::Display for CapitalFractionBenchmarkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CandidateNotCapitalFraction => write!(
                f,
                "candidate config does not select the capital-fraction sizing policy"
            ),
            Self::CandidateNeverEntered => write!(
                f,
                "candidate never resolved a valid capital-fraction entry; no benchmark quantity exists"
            ),
            Self::EmptyBars => write!(f, "empty bar sequence"),
            Self::ProvenanceMismatch(what) => {
                write!(f, "candidate sizing provenance disagrees with config: {what}")
            }
            Self::ReferenceBarNotFound {
                reference_bar_end_ts,
            } => write!(
                f,
                "reference bar end_ts {reference_bar_end_ts} not found in evaluated bars"
            ),
            Self::Backtest(e) => write!(f, "benchmark engine run failed: {e}"),
        }
    }
}

impl std::error::Error for CapitalFractionBenchmarkError {}

struct BuyHoldFromBar {
    symbol: String,
    target: QtyMicros,
    entry_bar_index: usize,
    timeframe_secs: i64,
    seen: usize,
}

impl Strategy for BuyHoldFromBar {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(
            "benchmark_capital_fraction_passive_buy_hold",
            self.timeframe_secs,
        )
    }

    fn required_history_bars(&self) -> usize {
        0
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        let idx = self.seen;
        self.seen += 1;
        let qty = if idx >= self.entry_bar_index {
            self.target
        } else {
            QtyMicros::ZERO
        };
        StrategyOutput::new(vec![TargetPosition::new(self.symbol.clone(), qty)])
    }
}

/// `config` and `bars` MUST be the exact config and bars of the candidate run
/// that produced `candidate_report`.
pub fn compute_capital_fraction_benchmark(
    candidate_report: &BacktestReport,
    bars: &[BacktestBar],
    config: &BacktestConfig,
    timeframe_secs: i64,
    candidate_total_return_pct: f64,
) -> Result<CapitalFractionBenchmarkSection, CapitalFractionBenchmarkError> {
    let SizingPolicy::FixedInitialCapitalFractionV1 {
        allocation_fraction_bps,
    } = config.sizing_policy
    else {
        return Err(CapitalFractionBenchmarkError::CandidateNotCapitalFraction);
    };
    if bars.is_empty() {
        return Err(CapitalFractionBenchmarkError::EmptyBars);
    }
    let prov = &candidate_report.sizing_provenance;
    if prov.policy != config.sizing_policy {
        return Err(CapitalFractionBenchmarkError::ProvenanceMismatch(
            "report sizing policy differs from config",
        ));
    }
    let first = prov
        .entries
        .iter()
        .min_by_key(|e| e.reference_bar_end_ts)
        .ok_or(CapitalFractionBenchmarkError::CandidateNeverEntered)?;
    if first.allocation_fraction_bps != allocation_fraction_bps {
        return Err(CapitalFractionBenchmarkError::ProvenanceMismatch(
            "entry fraction differs from config",
        ));
    }
    if first.initial_allocated_capital_micros != config.initial_cash_micros {
        return Err(CapitalFractionBenchmarkError::ProvenanceMismatch(
            "entry capital differs from config initial cash",
        ));
    }
    if first.resolved_target_qty_micros <= 0 {
        return Err(CapitalFractionBenchmarkError::ProvenanceMismatch(
            "entry resolved quantity is not positive",
        ));
    }
    let entry_bar_index = bars
        .iter()
        .position(|b| b.end_ts == first.reference_bar_end_ts && b.symbol == first.symbol)
        .ok_or(CapitalFractionBenchmarkError::ReferenceBarNotFound {
            reference_bar_end_ts: first.reference_bar_end_ts,
        })?;
    if bars[entry_bar_index].close_micros != first.causal_reference_price_micros {
        return Err(CapitalFractionBenchmarkError::ProvenanceMismatch(
            "entry reference price is not the reference bar close",
        ));
    }

    let mut bench_cfg = config.clone();
    bench_cfg.sizing_policy = SizingPolicy::FixedQuantityV1;

    let initial_cash_micros = config.initial_cash_micros;
    let strategy = BuyHoldFromBar {
        symbol: first.symbol.clone(),
        target: QtyMicros::new(first.resolved_target_qty_micros),
        entry_bar_index,
        timeframe_secs,
        seen: 0,
    };
    let mut engine = BacktestEngine::new(bench_cfg);
    engine
        .add_strategy(Box::new(strategy))
        .map_err(CapitalFractionBenchmarkError::Backtest)?;
    let report = engine
        .run(bars)
        .map_err(CapitalFractionBenchmarkError::Backtest)?;

    let ending = report
        .equity_curve
        .last()
        .map(|(_, eq)| *eq)
        .unwrap_or(initial_cash_micros);
    let account_return_pct = if initial_cash_micros != 0 {
        (ending - initial_cash_micros) as f64 / initial_cash_micros as f64 * 100.0
    } else {
        0.0
    };

    Ok(CapitalFractionBenchmarkSection {
        policy_id: BENCHMARK_CAPITAL_FRACTION_POLICY_ID.to_string(),
        symbol: first.symbol.clone(),
        sizing_policy_id: config.sizing_policy.policy_id().to_string(),
        allocation_fraction_bps,
        initial_allocated_capital_micros: first.initial_allocated_capital_micros,
        position_budget_micros: first.position_budget_micros,
        reference_bar_end_ts: first.reference_bar_end_ts,
        reference_price_micros: first.causal_reference_price_micros,
        target_qty_micros: first.resolved_target_qty_micros,
        entry_bar_index,
        initial_cash_micros,
        execution_model_id: report.execution_model_id.clone(),
        config_id: report.config_id.to_string(),
        benchmark_run_id: report.run_id.to_string(),
        account_return_pct,
        alpha_pct: candidate_total_return_pct - account_return_pct,
    })
}
