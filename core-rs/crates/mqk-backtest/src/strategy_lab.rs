//! Pure research-only evaluation contract for completed backtest results.
//!
//! This module classifies already-computed backtest metrics. It does not run
//! strategies, submit orders, call data providers, or make paper/live promotion
//! decisions.

use std::cmp::Ordering;

use crate::sweep::SweepRowResult;

/// Research-only input identifying a completed backtest result.
#[derive(Clone, Debug, PartialEq)]
pub struct StrategyLabInput {
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe: String,
    pub metrics: StrategyLabMetrics,
}

/// Metrics consumed by the strategy lab evaluator.
///
/// Return, drawdown, win rate, exposure, buy/hold return, and alpha fields are
/// percentage-point values. Optional values may be absent in older artifacts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrategyLabMetrics {
    pub total_return: Option<f64>,
    pub max_drawdown: Option<f64>,
    pub trade_count: Option<usize>,
    pub win_rate: Option<f64>,
    pub profit_factor: Option<f64>,
    pub expectancy: Option<f64>,
    pub trade_frequency: Option<f64>,
    pub exposure: Option<f64>,
    pub sharpe: Option<f64>,
    pub buy_hold_return: Option<f64>,
    pub alpha_vs_benchmark: Option<f64>,
}

/// Thresholds used by the deterministic research classifier.
#[derive(Clone, Debug, PartialEq)]
pub struct StrategyLabPolicy {
    pub min_trade_count: usize,
    pub max_drawdown: f64,
    pub min_profit_factor: f64,
    pub high_exposure: f64,
}

impl StrategyLabPolicy {
    pub const fn conservative_defaults() -> Self {
        Self {
            min_trade_count: 10,
            max_drawdown: 25.0,
            min_profit_factor: 1.20,
            high_exposure: 95.0,
        }
    }
}

impl Default for StrategyLabPolicy {
    fn default() -> Self {
        Self::conservative_defaults()
    }
}

/// Research-only evaluation output.
#[derive(Clone, Debug, PartialEq)]
pub struct StrategyLabEvaluation {
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe: String,
    pub total_return: Option<f64>,
    pub max_drawdown: Option<f64>,
    pub trade_count: Option<usize>,
    pub win_rate: Option<f64>,
    pub profit_factor: Option<f64>,
    pub expectancy: Option<f64>,
    pub trade_frequency: Option<f64>,
    pub exposure: Option<f64>,
    pub sharpe: Option<f64>,
    pub buy_hold_return: Option<f64>,
    pub alpha_vs_benchmark: Option<f64>,
    pub score: f64,
    pub grade: StrategyLabGrade,
    pub decision: StrategyLabDecision,
    pub reason_codes: Vec<StrategyLabReasonCode>,
}

/// Research-only decision values. These are not paper/live readiness states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrategyLabDecision {
    ResearchPass,
    ResearchWatch,
    ResearchFail,
    InsufficientData,
}

impl StrategyLabDecision {
    pub const fn code(self) -> &'static str {
        match self {
            Self::ResearchPass => "research_pass",
            Self::ResearchWatch => "research_watch",
            Self::ResearchFail => "research_fail",
            Self::InsufficientData => "insufficient_data",
        }
    }
}

/// Coarse score grade for display/reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrategyLabGrade {
    A,
    B,
    C,
    D,
    F,
    Insufficient,
}

impl StrategyLabGrade {
    pub const fn code(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::F => "F",
            Self::Insufficient => "insufficient",
        }
    }
}

/// Deterministic reason codes explaining the research classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum StrategyLabReasonCode {
    NoTrades,
    TooFewTrades,
    NegativeReturn,
    BenchmarkUnderperform,
    DrawdownTooHigh,
    ProfitFactorWeak,
    WinRateMissing,
    ExpectancyMissing,
    HighExposure,
    MetricsMissing,
    ResearchPassMinimumsMet,
}

impl StrategyLabReasonCode {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NoTrades => "no_trades",
            Self::TooFewTrades => "too_few_trades",
            Self::NegativeReturn => "negative_return",
            Self::BenchmarkUnderperform => "benchmark_underperform",
            Self::DrawdownTooHigh => "drawdown_too_high",
            Self::ProfitFactorWeak => "profit_factor_weak",
            Self::WinRateMissing => "win_rate_missing",
            Self::ExpectancyMissing => "expectancy_missing",
            Self::HighExposure => "high_exposure",
            Self::MetricsMissing => "metrics_missing",
            Self::ResearchPassMinimumsMet => "research_pass_minimums_met",
        }
    }
}

/// Evaluate with conservative default policy.
pub fn evaluate_strategy_lab(input: &StrategyLabInput) -> StrategyLabEvaluation {
    evaluate_strategy_lab_with_policy(input, &StrategyLabPolicy::conservative_defaults())
}

/// Evaluate with an explicit policy. The result is deterministic and pure.
pub fn evaluate_strategy_lab_with_policy(
    input: &StrategyLabInput,
    policy: &StrategyLabPolicy,
) -> StrategyLabEvaluation {
    let total_return = finite(input.metrics.total_return);
    let max_drawdown = non_negative_finite(input.metrics.max_drawdown);
    let trade_count = input.metrics.trade_count;
    let win_rate = bounded_percent(input.metrics.win_rate);
    let profit_factor = valid_profit_factor(input.metrics.profit_factor);
    let expectancy = finite(input.metrics.expectancy);
    let trade_frequency = non_negative_finite(input.metrics.trade_frequency);
    let exposure = bounded_percent(input.metrics.exposure);
    let sharpe = finite(input.metrics.sharpe);
    let buy_hold_return = finite(input.metrics.buy_hold_return);
    let alpha_vs_benchmark = finite(input.metrics.alpha_vs_benchmark).or({
        match (total_return, buy_hold_return) {
            (Some(total), Some(benchmark)) => Some(total - benchmark),
            _ => None,
        }
    });

    let mut reason_codes = Vec::new();

    if total_return.is_none() || max_drawdown.is_none() || trade_count.is_none() {
        reason_codes.push(StrategyLabReasonCode::MetricsMissing);
    }
    if win_rate.is_none() {
        reason_codes.push(StrategyLabReasonCode::WinRateMissing);
    }
    if expectancy.is_none() {
        reason_codes.push(StrategyLabReasonCode::ExpectancyMissing);
    }
    if matches!(trade_count, Some(0)) {
        reason_codes.push(StrategyLabReasonCode::NoTrades);
    }
    if matches!(trade_count, Some(count) if count > 0 && count < policy.min_trade_count) {
        reason_codes.push(StrategyLabReasonCode::TooFewTrades);
    }
    if matches!(total_return, Some(value) if value < 0.0) {
        reason_codes.push(StrategyLabReasonCode::NegativeReturn);
    }
    if matches!(max_drawdown, Some(value) if value > policy.max_drawdown) {
        reason_codes.push(StrategyLabReasonCode::DrawdownTooHigh);
    }
    if matches!(profit_factor, Some(value) if value < policy.min_profit_factor) {
        reason_codes.push(StrategyLabReasonCode::ProfitFactorWeak);
    }
    if matches!(alpha_vs_benchmark, Some(value) if value < 0.0) {
        reason_codes.push(StrategyLabReasonCode::BenchmarkUnderperform);
    }
    if matches!(exposure, Some(value) if value > policy.high_exposure) {
        reason_codes.push(StrategyLabReasonCode::HighExposure);
    }

    let score = score_metrics(
        ScoreMetricsInput {
            total_return,
            max_drawdown,
            trade_count,
            win_rate,
            profit_factor,
            expectancy,
            exposure,
            sharpe,
        },
        policy,
    );
    let decision = classify(score, &reason_codes);
    if decision == StrategyLabDecision::ResearchPass {
        reason_codes.push(StrategyLabReasonCode::ResearchPassMinimumsMet);
    }
    reason_codes.sort();
    reason_codes.dedup();

    let grade = grade(score, decision);

    StrategyLabEvaluation {
        strategy_id: input.strategy_id.clone(),
        symbol: input.symbol.clone(),
        timeframe: input.timeframe.clone(),
        total_return,
        max_drawdown,
        trade_count,
        win_rate,
        profit_factor,
        expectancy,
        trade_frequency,
        exposure,
        sharpe,
        buy_hold_return,
        alpha_vs_benchmark,
        score,
        grade,
        decision,
        reason_codes,
    }
}

/// Build strategy lab input from an existing sweep row.
pub fn strategy_lab_input_from_sweep_row(
    strategy_id: impl Into<String>,
    symbol: impl Into<String>,
    timeframe: impl Into<String>,
    row: &SweepRowResult,
) -> StrategyLabInput {
    StrategyLabInput {
        strategy_id: strategy_id.into(),
        symbol: symbol.into(),
        timeframe: timeframe.into(),
        metrics: StrategyLabMetrics {
            total_return: Some(row.total_return_pct),
            max_drawdown: Some(row.max_drawdown_pct),
            trade_count: Some(row.trade_count),
            win_rate: row.win_rate_pct,
            profit_factor: row.profit_factor,
            expectancy: None,
            trade_frequency: None,
            exposure: None,
            sharpe: None,
            buy_hold_return: row.buy_and_hold_return_pct,
            alpha_vs_benchmark: row.alpha_pct,
        },
    }
}

/// Deterministic ranking for already-evaluated research results.
pub fn rank_strategy_lab_evaluations(rows: &mut [StrategyLabEvaluation]) {
    rows.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then(a.strategy_id.cmp(&b.strategy_id))
            .then(a.symbol.cmp(&b.symbol))
            .then(a.timeframe.cmp(&b.timeframe))
    });
}

fn classify(score: f64, reason_codes: &[StrategyLabReasonCode]) -> StrategyLabDecision {
    if reason_codes.contains(&StrategyLabReasonCode::MetricsMissing)
        || reason_codes.contains(&StrategyLabReasonCode::NoTrades)
    {
        return StrategyLabDecision::InsufficientData;
    }
    if reason_codes.contains(&StrategyLabReasonCode::NegativeReturn)
        || reason_codes.contains(&StrategyLabReasonCode::DrawdownTooHigh)
        || reason_codes.contains(&StrategyLabReasonCode::ProfitFactorWeak)
    {
        return StrategyLabDecision::ResearchFail;
    }
    if reason_codes.contains(&StrategyLabReasonCode::TooFewTrades) || score < 70.0 {
        return StrategyLabDecision::ResearchWatch;
    }
    StrategyLabDecision::ResearchPass
}

fn grade(score: f64, decision: StrategyLabDecision) -> StrategyLabGrade {
    if decision == StrategyLabDecision::InsufficientData {
        return StrategyLabGrade::Insufficient;
    }
    if score >= 90.0 {
        StrategyLabGrade::A
    } else if score >= 80.0 {
        StrategyLabGrade::B
    } else if score >= 70.0 {
        StrategyLabGrade::C
    } else if score >= 60.0 {
        StrategyLabGrade::D
    } else {
        StrategyLabGrade::F
    }
}

/// Sanitized metric inputs consumed by [`score_metrics`].
///
/// Each field mirrors the validated local binding of the same name in
/// `evaluate_strategy_lab_with_policy` (already passed through
/// `finite`/`bounded_percent`/`valid_profit_factor`), not the raw
/// `StrategyLabMetrics` input.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ScoreMetricsInput {
    total_return: Option<f64>,
    max_drawdown: Option<f64>,
    trade_count: Option<usize>,
    win_rate: Option<f64>,
    profit_factor: Option<f64>,
    expectancy: Option<f64>,
    exposure: Option<f64>,
    sharpe: Option<f64>,
}

fn score_metrics(metrics: ScoreMetricsInput, policy: &StrategyLabPolicy) -> f64 {
    let mut score = 0.0;

    if let Some(value) = metrics.total_return {
        score += clamp(value, 0.0, 35.0);
    }
    if let Some(value) = metrics.max_drawdown {
        score += clamp(policy.max_drawdown - value, 0.0, 20.0);
    }
    if let Some(count) = metrics.trade_count {
        if count >= policy.min_trade_count {
            score += 10.0;
        } else if count > 0 {
            score += 5.0 * count as f64 / policy.min_trade_count as f64;
        }
    }
    if let Some(value) = metrics.profit_factor {
        if value.is_infinite() && value.is_sign_positive() {
            score += 15.0;
        } else {
            score += clamp((value - 1.0) * 20.0, 0.0, 15.0);
        }
    }
    if let Some(value) = metrics.win_rate {
        score += clamp((value - 40.0) / 60.0 * 5.0, 0.0, 5.0);
    }
    if let Some(value) = metrics.expectancy {
        score += clamp(value * 10.0, 0.0, 10.0);
    }
    if let Some(value) = metrics.sharpe {
        score += clamp(value * 5.0, 0.0, 10.0);
    }
    if let Some(value) = metrics.exposure {
        if value <= policy.high_exposure {
            score += 5.0;
        }
    }

    clamp(score, 0.0, 100.0)
}

fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite())
}

fn non_negative_finite(value: Option<f64>) -> Option<f64> {
    finite(value).filter(|v| *v >= 0.0)
}

fn bounded_percent(value: Option<f64>) -> Option<f64> {
    finite(value).filter(|v| (0.0..=100.0).contains(v))
}

fn valid_profit_factor(value: Option<f64>) -> Option<f64> {
    value.filter(|v| (*v >= 0.0 && v.is_finite()) || (*v == f64::INFINITY))
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
}
