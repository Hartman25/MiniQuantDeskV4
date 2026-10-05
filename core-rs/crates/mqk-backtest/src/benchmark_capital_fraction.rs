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
use mqk_strategy::{
    SemanticIdentityBuilder, SizingPolicy, Strategy, StrategyContext, StrategySpec,
    SEMANTIC_IDENTITY_SCHEMA_V1,
};
use uuid::Uuid;

use crate::{
    derive_run_id_with_quantity_semantics, BacktestBar, BacktestConfig, BacktestEngine,
    BacktestError, BacktestInstrumentEconomics, BacktestReport, QuantitySemanticsId,
    BACKTEST_EXECUTION_MODEL_ID,
};

pub const BENCHMARK_CAPITAL_FRACTION_POLICY_ID: &str =
    "capital_fraction_matched_passive_buy_hold_v1";
/// `StrategySpec` name of the benchmark strategy (part of its run identity).
pub const BENCHMARK_CAPITAL_FRACTION_STRATEGY_NAME: &str =
    "benchmark_capital_fraction_passive_buy_hold";
const BENCHMARK_BEHAVIOR_VERSION: &str = "v1";

/// The benchmark strategy's semantic identity: every behavior-bearing input
/// (policy/version, symbol, timeframe, exact target quantity, and the causal
/// entry: the reference bar's `end_ts` plus the positional index at which the
/// strategy starts holding). No result value participates. Without this the
/// default spec-only fingerprint would let two economically different passive
/// executions share one canonical benchmark `run_id`.
pub fn capital_fraction_benchmark_semantic_fingerprint(
    symbol: &str,
    timeframe_secs: i64,
    target_qty_micros: i64,
    entry_reference_bar_end_ts: i64,
    entry_bar_index: usize,
) -> String {
    let mut b = SemanticIdentityBuilder::new(
        SEMANTIC_IDENTITY_SCHEMA_V1,
        BENCHMARK_CAPITAL_FRACTION_STRATEGY_NAME,
        BENCHMARK_BEHAVIOR_VERSION,
    );
    b.push_str(BENCHMARK_CAPITAL_FRACTION_POLICY_ID)
        .push_str(symbol)
        .push_i64(timeframe_secs)
        .push_i64(target_qty_micros)
        .push_i64(entry_reference_bar_end_ts)
        .push_i64(i64::try_from(entry_bar_index).unwrap_or(i64::MAX));
    b.finish()
}

/// Recompute the canonical benchmark `run_id` from authoritative inputs alone:
/// the benchmark `config_id` (candidate config with only the sizing policy
/// swapped), the data identity, the execution model and the benchmark's own
/// behavior-bearing identity. Used by the evidence verifier so a substituted
/// `benchmark_run_id` (or a tampered quantity/entry/timeframe) is detected.
/// The benchmark always runs under default equity economics and whole-unit
/// quantity semantics.
#[allow(clippy::too_many_arguments)]
pub fn expected_capital_fraction_benchmark_run_id(
    benchmark_config_id: &Uuid,
    input_data_hash: &str,
    execution_model_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    target_qty_micros: i64,
    entry_reference_bar_end_ts: i64,
    entry_bar_index: usize,
) -> Uuid {
    derive_run_id_with_quantity_semantics(
        BENCHMARK_CAPITAL_FRACTION_STRATEGY_NAME,
        benchmark_config_id,
        input_data_hash,
        &BacktestInstrumentEconomics::equity(),
        execution_model_id,
        &capital_fraction_benchmark_semantic_fingerprint(
            symbol,
            timeframe_secs,
            target_qty_micros,
            entry_reference_bar_end_ts,
            entry_bar_index,
        ),
        QuantitySemanticsId::WholeUnitsV1,
    )
}

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
    pub candidate_max_drawdown_pct: f64,
    pub benchmark_max_drawdown_pct: f64,
    /// `benchmark_max_drawdown_pct - candidate_max_drawdown_pct`, in percentage points of peak
    /// equity: positive means the candidate's drawdown was smaller than the matched passive hold.
    pub drawdown_improvement_pct: f64,
}

/// Worst peak-to-trough equity decline as a percentage of the running peak, the peak starting at
/// `initial_cash_micros`. `None` when the curve is empty or the initial cash is not positive.
pub fn max_drawdown_pct_from_equity(initial_cash_micros: i64, curve: &[(i64, i64)]) -> Option<f64> {
    if curve.is_empty() || initial_cash_micros <= 0 {
        return None;
    }
    let mut peak = initial_cash_micros;
    let mut worst = 0.0f64;
    for &(_, equity) in curve {
        peak = peak.max(equity);
        let dd = peak.saturating_sub(equity) as f64 / peak as f64 * 100.0;
        worst = worst.max(dd);
    }
    Some(worst)
}

#[derive(Debug)]
pub enum CapitalFractionBenchmarkError {
    /// The candidate config does not select the capital-fraction policy.
    CandidateNotCapitalFraction,
    /// The candidate never resolved a valid entry: nothing to match.
    CandidateNeverEntered,
    EmptyBars,
    /// The candidate or benchmark run has no equity curve to measure drawdown on.
    EquityCurveUnavailable,
    /// Report provenance disagrees with the config (capital, fraction, policy).
    ProvenanceMismatch(&'static str),
    /// The recorded reference bar is not in the evaluated bars.
    ReferenceBarNotFound {
        reference_bar_end_ts: i64,
    },
    /// The engine's benchmark run id differs from the one derived from the
    /// benchmark's declared behavior-bearing inputs.
    RunIdentityDrift,
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
            Self::EquityCurveUnavailable => {
                write!(f, "candidate or benchmark equity curve is unavailable")
            }
            Self::ProvenanceMismatch(what) => {
                write!(f, "candidate sizing provenance disagrees with config: {what}")
            }
            Self::ReferenceBarNotFound {
                reference_bar_end_ts,
            } => write!(
                f,
                "reference bar end_ts {reference_bar_end_ts} not found in evaluated bars"
            ),
            Self::RunIdentityDrift => write!(
                f,
                "benchmark run id does not match its behavior-bearing inputs"
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
    entry_reference_bar_end_ts: i64,
    timeframe_secs: i64,
    seen: usize,
}

impl Strategy for BuyHoldFromBar {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(
            BENCHMARK_CAPITAL_FRACTION_STRATEGY_NAME,
            self.timeframe_secs,
        )
    }

    fn semantic_fingerprint(&self) -> String {
        capital_fraction_benchmark_semantic_fingerprint(
            &self.symbol,
            self.timeframe_secs,
            self.target.raw(),
            self.entry_reference_bar_end_ts,
            self.entry_bar_index,
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
        entry_reference_bar_end_ts: first.reference_bar_end_ts,
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
    if report.execution_model_id != BACKTEST_EXECUTION_MODEL_ID
        || report.run_id
            != expected_capital_fraction_benchmark_run_id(
                &report.config_id,
                &report.input_data_hash,
                &report.execution_model_id,
                &first.symbol,
                timeframe_secs,
                first.resolved_target_qty_micros,
                first.reference_bar_end_ts,
                entry_bar_index,
            )
    {
        return Err(CapitalFractionBenchmarkError::RunIdentityDrift);
    }

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

    let candidate_max_drawdown_pct =
        max_drawdown_pct_from_equity(initial_cash_micros, &candidate_report.equity_curve)
            .ok_or(CapitalFractionBenchmarkError::EquityCurveUnavailable)?;
    let benchmark_max_drawdown_pct =
        max_drawdown_pct_from_equity(initial_cash_micros, &report.equity_curve)
            .ok_or(CapitalFractionBenchmarkError::EquityCurveUnavailable)?;

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
        candidate_max_drawdown_pct,
        benchmark_max_drawdown_pct,
        drawdown_improvement_pct: benchmark_max_drawdown_pct - candidate_max_drawdown_pct,
    })
}
