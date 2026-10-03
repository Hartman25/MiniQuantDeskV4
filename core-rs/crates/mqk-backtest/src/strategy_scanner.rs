//! STRATEGY-LAB-SCANNER-01B — local-data-only strategy/symbol/timeframe
//! scanner core (pure).
//!
//! Given already-resolved inputs (bars the caller already loaded from
//! disk, and a strategy instance the caller already constructed), decides
//! a deterministic `truth_state`/`reason_code` for one
//! `(symbol, timeframe, strategy_id)` candidate and — only when every
//! precondition passes — runs the bars through the existing deterministic
//! [`crate::engine::BacktestEngine`] and reduces the resulting
//! [`crate::types::BacktestReport`] into [`StrategyScanMetrics`] via the
//! existing, already-tested [`crate::sweep::sweep_row_from_report`].
//!
//! This module performs **no file IO, no network IO, and no DB access**.
//! Every side effect (resolving the instrument registry, reading a bars
//! CSV, instantiating a strategy from the plugin registry) happens in the
//! caller (the `mqk backtest scan-strategies` CLI command). This module
//! also does not import any broker, provider, or OMS-write type — it only
//! reuses the same in-memory, replay-only backtest engine already used by
//! every other backtest CLI command in this repo.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use mqk_strategy::Strategy;

use crate::engine::BacktestEngine;
use crate::sweep::{sweep_row_from_report, SweepPoint};
use crate::types::{BacktestBar, BacktestConfig, StrategySizingConfig};

/// Minimum number of bars required to attempt a scan run. Below this, a
/// candidate is reported `insufficient_data` rather than run through the
/// engine — a handful of bars cannot produce a meaningful trade/return
/// sample and running them anyway would fabricate a misleadingly precise
/// score.
pub const DEFAULT_MIN_BARS: usize = 60;

// ---------------------------------------------------------------------------
// Truth states / reason codes
// ---------------------------------------------------------------------------

/// Outcome of evaluating one `(symbol, timeframe, strategy_id)` candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyScanTruthState {
    /// The candidate ran successfully and carries a rankable score.
    CandidateRanked,
    /// Local bars were found but there were too few to evaluate.
    InsufficientData,
    /// The backtest engine returned an error while running the candidate.
    BacktestFailed,
    /// `strategy_id` is not registered with the scanner.
    UnsupportedStrategy,
    /// The requested timeframe does not match the strategy's required timeframe.
    UnsupportedTimeframe,
    /// No local bars file was found for this `(symbol, timeframe)`.
    DataMissing,
    /// The engine produced a report but metrics could not be derived from it.
    MetricsUnavailable,
}

impl StrategyScanTruthState {
    pub fn code(&self) -> &'static str {
        match self {
            Self::CandidateRanked => "candidate_ranked",
            Self::InsufficientData => "insufficient_data",
            Self::BacktestFailed => "backtest_failed",
            Self::UnsupportedStrategy => "unsupported_strategy",
            Self::UnsupportedTimeframe => "unsupported_timeframe",
            Self::DataMissing => "data_missing",
            Self::MetricsUnavailable => "metrics_unavailable",
        }
    }
}

/// Machine-readable reason paired with each [`StrategyScanTruthState`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyScanReasonCode {
    Ranked,
    NotEnoughBars,
    MissingBarsFile,
    StrategyNotSupportedByScanner,
    TimeframeNotSupportedByScanner,
    BacktestError,
    MetricsParseError,
    /// The policy-required benchmark could not be computed or bound (fail
    /// closed; never substituted by another benchmark).
    BenchmarkUnavailable,
}

impl StrategyScanReasonCode {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Ranked => "ranked",
            Self::NotEnoughBars => "not_enough_bars",
            Self::MissingBarsFile => "missing_bars_file",
            Self::StrategyNotSupportedByScanner => "strategy_not_supported_by_scanner",
            Self::TimeframeNotSupportedByScanner => "timeframe_not_supported_by_scanner",
            Self::BacktestError => "backtest_error",
            Self::MetricsParseError => "metrics_parse_error",
            Self::BenchmarkUnavailable => "benchmark_unavailable",
        }
    }
}

// ---------------------------------------------------------------------------
// Metrics / candidate / report schema
// ---------------------------------------------------------------------------

/// Deterministic starter metrics for a ranked candidate. Every field is
/// derived from the existing, already-tested
/// [`crate::sweep::sweep_row_from_report`] reduction of a
/// [`crate::types::BacktestReport`] — no new metric derivation logic is
/// introduced here. Fields are `None`/absent for skipped candidates.
///
/// An `exposure` metric (fraction of bars holding an open position) was
/// deliberately **not** added: `BacktestReport` does not expose a per-bar
/// position size, only fills and the equity curve, and computing exposure
/// honestly would require re-deriving per-bar position state — out of
/// scope for this foundation patch. Omitted rather than fabricated.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StrategyScanMetrics {
    pub total_return_pct: Option<f64>,
    pub benchmark_return_pct: Option<f64>,
    pub alpha_pct: Option<f64>,
    pub max_drawdown_pct: Option<f64>,
    pub trade_count: Option<usize>,
    pub win_rate_pct: Option<f64>,
    pub profit_factor: Option<f64>,
    pub fill_count: Option<usize>,
    pub bars_used: usize,
    pub data_start_ts: Option<i64>,
    pub data_end_ts: Option<i64>,
    pub halted: bool,
    /// Present only for a candidate scanned under
    /// [`ScanBenchmarkPolicy::CapitalMatchedExactTargetV2`]; absent in every
    /// legacy artifact (and omitted from serialization when `None`, so
    /// legacy artifacts keep their exact historical bytes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_v2: Option<ScanBenchmarkV2Evidence>,
    /// Present only for a candidate scanned under
    /// [`ScanBenchmarkPolicy::CapitalFractionMatchedPassiveV1`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_capital_fraction: Option<ScanCapitalFractionBenchmarkEvidence>,
}

/// Benchmark policy a scan run (and the review of it) evaluates alpha against.
///
/// `LegacyFullyInvested` is the historical raw fully-invested price return
/// from [`crate::sweep::sweep_row_from_report`]. `CapitalMatchedExactTargetV2`
/// is [`crate::benchmark_v2::BENCHMARK_V2_POLICY_ID`]: a candidate-quantity,
/// candidate-capital, candidate-eligibility passive benchmark for fixed-Q
/// candidates. `CapitalFractionMatchedPassiveV1` is
/// [`crate::benchmark_capital_fraction::BENCHMARK_CAPITAL_FRACTION_POLICY_ID`],
/// the benchmark for capital-fraction-sized candidates. There is no fallback
/// between policies: a scan that cannot compute its own policy's benchmark
/// fails the candidate closed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanBenchmarkPolicy {
    #[default]
    LegacyFullyInvested,
    CapitalMatchedExactTargetV2,
    CapitalFractionMatchedPassiveV1,
}

impl ScanBenchmarkPolicy {
    /// `None` for the legacy policy (historical artifacts carry no id).
    pub fn policy_id(&self) -> Option<&'static str> {
        match self {
            Self::LegacyFullyInvested => None,
            Self::CapitalMatchedExactTargetV2 => Some(crate::benchmark_v2::BENCHMARK_V2_POLICY_ID),
            Self::CapitalFractionMatchedPassiveV1 => {
                Some(crate::benchmark_capital_fraction::BENCHMARK_CAPITAL_FRACTION_POLICY_ID)
            }
        }
    }

    /// Parse an explicit CLI/policy id. The legacy policy is selected by
    /// omission, never by a string.
    pub fn from_policy_id(id: &str) -> Option<Self> {
        if id == crate::benchmark_v2::BENCHMARK_V2_POLICY_ID {
            Some(Self::CapitalMatchedExactTargetV2)
        } else if id == crate::benchmark_capital_fraction::BENCHMARK_CAPITAL_FRACTION_POLICY_ID {
            Some(Self::CapitalFractionMatchedPassiveV1)
        } else {
            None
        }
    }

    /// Resolve a manifest-recorded policy id (`None` = legacy).
    pub fn from_manifest_id(id: Option<&str>) -> Result<Self, String> {
        match id {
            None => Ok(Self::LegacyFullyInvested),
            Some(s) => Self::from_policy_id(s)
                .ok_or_else(|| format!("unrecognized benchmark_policy_id '{s}'")),
        }
    }
}

/// Provenance of one Benchmark V2 alpha evaluation, carried on a candidate
/// scanned under [`ScanBenchmarkPolicy::CapitalMatchedExactTargetV2`]. Binds
/// the candidate run and the benchmark run to the same strategy semantics,
/// capital, quantity, cost config, bars and evaluation endpoint, so a
/// reviewer (and promotion) can verify the alpha without re-running either.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScanBenchmarkV2Evidence {
    pub policy_id: String,
    pub strategy_id: String,
    pub strategy_semantic_fingerprint: String,
    pub symbol: String,
    pub timeframe: String,
    pub candidate_run_id: String,
    pub candidate_config_id: String,
    pub candidate_initial_cash_micros: i64,
    pub candidate_target_qty_micros: i64,
    pub candidate_execution_model_id: String,
    /// `(ending - initial_cash) / initial_cash * 100`, the same formula as
    /// `benchmark_account_return_pct`.
    pub candidate_total_return_pct: f64,
    pub required_history_bars: usize,
    pub benchmark_run_id: String,
    pub benchmark_config_id: String,
    pub benchmark_target_qty_micros: i64,
    pub benchmark_eligibility_bar_index: usize,
    pub benchmark_eligibility_decision_ts: i64,
    pub benchmark_initial_cash_micros: i64,
    pub benchmark_execution_model_id: String,
    pub benchmark_account_return_pct: f64,
    pub alpha_pct: f64,
    /// `derive_input_data_hash` over the exact bars both runs consumed.
    pub input_data_hash: String,
    /// `end_ts` of the last evaluated bar.
    pub evaluation_end_ts: i64,
    /// INFORMATIONAL ONLY: the legacy fully-invested price return. Never an
    /// input to alpha or to any review decision under this policy.
    pub legacy_buy_and_hold_return_pct: Option<f64>,
}

impl ScanBenchmarkV2Evidence {
    /// Self-consistency of the evidence, independent of any scanner metrics
    /// row. Fails closed on any field that would let a mismatched benchmark
    /// pass as capital/quantity/eligibility/cost matched.
    pub fn verify_internal(&self) -> Result<(), String> {
        let bad = |what: &str| Err(format!("benchmark_v2 evidence inconsistent: {what}"));
        if self.policy_id != crate::benchmark_v2::BENCHMARK_V2_POLICY_ID {
            return bad("policy_id is not the accepted Benchmark V2 policy");
        }
        for (name, v) in [
            ("strategy_id", &self.strategy_id),
            (
                "strategy_semantic_fingerprint",
                &self.strategy_semantic_fingerprint,
            ),
            ("symbol", &self.symbol),
            ("timeframe", &self.timeframe),
            ("candidate_run_id", &self.candidate_run_id),
            ("benchmark_run_id", &self.benchmark_run_id),
            ("candidate_config_id", &self.candidate_config_id),
            ("benchmark_config_id", &self.benchmark_config_id),
            (
                "candidate_execution_model_id",
                &self.candidate_execution_model_id,
            ),
            (
                "benchmark_execution_model_id",
                &self.benchmark_execution_model_id,
            ),
            ("input_data_hash", &self.input_data_hash),
        ] {
            if v.trim().is_empty() {
                return bad(&format!("{name} is empty"));
            }
        }
        if self.candidate_run_id == self.benchmark_run_id {
            return bad("benchmark_run_id equals candidate_run_id");
        }
        if uuid::Uuid::parse_str(&self.candidate_run_id).is_err()
            || uuid::Uuid::parse_str(&self.benchmark_run_id).is_err()
        {
            return bad("candidate/benchmark run id is not a UUID");
        }
        if self.candidate_initial_cash_micros <= 0
            || self.candidate_initial_cash_micros != self.benchmark_initial_cash_micros
        {
            return bad("candidate and benchmark initial cash differ or are not positive");
        }
        if self.candidate_target_qty_micros <= 0
            || self.candidate_target_qty_micros != self.benchmark_target_qty_micros
        {
            return bad("candidate and benchmark target quantity differ or are not positive");
        }
        if self.candidate_execution_model_id != self.benchmark_execution_model_id {
            return bad("candidate and benchmark execution model differ");
        }
        if self.candidate_config_id != self.benchmark_config_id {
            return bad("candidate and benchmark cost/capital config differ");
        }
        if self.required_history_bars == 0
            || self.benchmark_eligibility_bar_index != self.required_history_bars - 1
        {
            return bad("benchmark eligibility is not the candidate's first causally eligible bar");
        }
        if !self.candidate_total_return_pct.is_finite()
            || !self.benchmark_account_return_pct.is_finite()
            || !self.alpha_pct.is_finite()
            || (self.candidate_total_return_pct
                - self.benchmark_account_return_pct
                - self.alpha_pct)
                .abs()
                > 1e-9
        {
            return bad("alpha_pct is not candidate return minus benchmark return");
        }
        Ok(())
    }
}

/// Provenance of one capital-fraction-matched passive benchmark evaluation.
/// Binds the candidate's policy-resolved entry (capital, fraction, budget,
/// causal reference, quantity) and the benchmark run to one data window,
/// execution model and cost basis, so substitution of any element is detectable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScanCapitalFractionBenchmarkEvidence {
    pub policy_id: String,
    pub strategy_id: String,
    pub strategy_semantic_fingerprint: String,
    pub symbol: String,
    pub timeframe: String,
    pub candidate_run_id: String,
    pub candidate_config_id: String,
    /// `config_id` of the candidate config with only the sizing policy swapped
    /// to fixed-quantity: the identity the benchmark run must share.
    pub candidate_cost_basis_config_id: String,
    pub sizing_policy_id: String,
    pub allocation_fraction_bps: i64,
    pub initial_allocated_capital_micros: i64,
    pub position_budget_micros: i64,
    pub reference_bar_end_ts: i64,
    pub reference_price_micros: i64,
    pub candidate_target_qty_micros: i64,
    pub candidate_initial_cash_micros: i64,
    pub candidate_execution_model_id: String,
    pub candidate_total_return_pct: f64,
    pub benchmark_run_id: String,
    pub benchmark_config_id: String,
    pub benchmark_target_qty_micros: i64,
    pub benchmark_entry_bar_index: usize,
    pub benchmark_initial_cash_micros: i64,
    pub benchmark_execution_model_id: String,
    pub benchmark_account_return_pct: f64,
    pub alpha_pct: f64,
    pub input_data_hash: String,
    pub evaluation_end_ts: i64,
    /// INFORMATIONAL ONLY: never an input to alpha or any review decision.
    pub legacy_buy_and_hold_return_pct: Option<f64>,
}

impl ScanCapitalFractionBenchmarkEvidence {
    /// Self-consistency independent of any metrics row; fails closed.
    pub fn verify_internal(&self) -> Result<(), String> {
        let bad = |what: &str| {
            Err(format!(
                "capital-fraction benchmark evidence inconsistent: {what}"
            ))
        };
        if self.policy_id != crate::benchmark_capital_fraction::BENCHMARK_CAPITAL_FRACTION_POLICY_ID
        {
            return bad("policy_id is not the accepted capital-fraction benchmark policy");
        }
        for (name, v) in [
            ("strategy_id", &self.strategy_id),
            (
                "strategy_semantic_fingerprint",
                &self.strategy_semantic_fingerprint,
            ),
            ("symbol", &self.symbol),
            ("timeframe", &self.timeframe),
            ("candidate_run_id", &self.candidate_run_id),
            ("benchmark_run_id", &self.benchmark_run_id),
            ("candidate_config_id", &self.candidate_config_id),
            (
                "candidate_cost_basis_config_id",
                &self.candidate_cost_basis_config_id,
            ),
            ("benchmark_config_id", &self.benchmark_config_id),
            (
                "candidate_execution_model_id",
                &self.candidate_execution_model_id,
            ),
            (
                "benchmark_execution_model_id",
                &self.benchmark_execution_model_id,
            ),
            ("input_data_hash", &self.input_data_hash),
        ] {
            if v.trim().is_empty() {
                return bad(&format!("{name} is empty"));
            }
        }
        if self.sizing_policy_id != mqk_strategy::SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1 {
            return bad("sizing_policy_id is not the capital-fraction policy");
        }
        if self.candidate_run_id == self.benchmark_run_id {
            return bad("benchmark_run_id equals candidate_run_id");
        }
        if uuid::Uuid::parse_str(&self.candidate_run_id).is_err()
            || uuid::Uuid::parse_str(&self.benchmark_run_id).is_err()
        {
            return bad("candidate/benchmark run id is not a UUID");
        }
        if !(1..=mqk_strategy::ALLOCATION_FRACTION_BPS_DENOMINATOR)
            .contains(&self.allocation_fraction_bps)
        {
            return bad("allocation_fraction_bps outside 1..=10000");
        }
        if self.initial_allocated_capital_micros <= 0
            || self.initial_allocated_capital_micros != self.candidate_initial_cash_micros
            || self.candidate_initial_cash_micros != self.benchmark_initial_cash_micros
        {
            return bad(
                "allocated capital, candidate cash and benchmark cash differ or are not positive",
            );
        }
        let expected_budget = (self.initial_allocated_capital_micros as i128
            * self.allocation_fraction_bps as i128)
            / mqk_strategy::ALLOCATION_FRACTION_BPS_DENOMINATOR as i128;
        if expected_budget <= 0 || self.position_budget_micros as i128 != expected_budget {
            return bad("position budget is not floor(capital * fraction)");
        }
        if self.reference_price_micros <= 0 {
            return bad("reference price is not positive");
        }
        let scale = mqk_execution::QTY_MICROS_SCALE as i128;
        let q = self.candidate_target_qty_micros as i128;
        if q <= 0
            || q % scale != 0
            || q != self.benchmark_target_qty_micros as i128
            || (q / scale) * self.reference_price_micros as i128
                > self.position_budget_micros as i128
        {
            return bad("benchmark quantity differs from the candidate's resolved whole-share quantity or exceeds the budget");
        }
        if self.candidate_execution_model_id != self.benchmark_execution_model_id {
            return bad("candidate and benchmark execution model differ");
        }
        if self.candidate_cost_basis_config_id != self.benchmark_config_id {
            return bad("candidate and benchmark cost/capital config differ");
        }
        if !self.candidate_total_return_pct.is_finite()
            || !self.benchmark_account_return_pct.is_finite()
            || !self.alpha_pct.is_finite()
            || (self.candidate_total_return_pct
                - self.benchmark_account_return_pct
                - self.alpha_pct)
                .abs()
                > 1e-9
        {
            return bad("alpha_pct is not candidate return minus benchmark return");
        }
        // The benchmark run id is recomputed from the benchmark's own
        // behavior-bearing inputs (exact quantity and causal entry included), so
        // a substituted run id, or a quantity/entry/timeframe that differs from
        // the run that produced it, is refused.
        let (Some(timeframe_secs), Ok(config_id)) = (
            resolve_timeframe_secs(&self.timeframe),
            uuid::Uuid::parse_str(&self.benchmark_config_id),
        ) else {
            return bad(
                "benchmark timeframe or config id cannot be resolved to recompute the run id",
            );
        };
        let expected =
            crate::benchmark_capital_fraction::expected_capital_fraction_benchmark_run_id(
                &config_id,
                &self.input_data_hash,
                &self.benchmark_execution_model_id,
                &self.symbol,
                timeframe_secs,
                self.benchmark_target_qty_micros,
                self.reference_bar_end_ts,
                self.benchmark_entry_bar_index,
            );
        if uuid::Uuid::parse_str(&self.benchmark_run_id).ok() != Some(expected) {
            return bad(
                "benchmark_run_id does not match the run identity derived from the benchmark's quantity, entry and inputs",
            );
        }
        Ok(())
    }
}

/// One evaluated `(symbol, timeframe, strategy_id)` candidate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyScanCandidate {
    pub symbol: String,
    pub timeframe: String,
    pub strategy_id: String,
    pub bars_available: usize,
    pub truth_state: StrategyScanTruthState,
    pub reason_code: StrategyScanReasonCode,
    pub score: Option<f64>,
    pub rank: Option<usize>,
    pub metrics: StrategyScanMetrics,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
}

impl StrategyScanCandidate {
    fn skipped(
        symbol: &str,
        timeframe: &str,
        strategy_id: &str,
        bars_available: usize,
        truth_state: StrategyScanTruthState,
        reason_code: StrategyScanReasonCode,
        blockers: Vec<String>,
    ) -> Self {
        Self {
            symbol: symbol.to_string(),
            timeframe: timeframe.to_string(),
            strategy_id: strategy_id.to_string(),
            bars_available,
            truth_state,
            reason_code,
            score: None,
            rank: None,
            metrics: StrategyScanMetrics {
                bars_used: bars_available,
                ..Default::default()
            },
            warnings: Vec::new(),
            blockers,
        }
    }
}

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

/// Deterministic scan policy. `base_config` seeds every candidate's
/// [`BacktestConfig`] (timeframe is overwritten per candidate; sizing resets to
/// the legacy default unless a capital-fraction policy is selected);
/// all other fields (risk limits, commission, stress) are preserved as-is.
///
/// `base_config.integrity_enabled` is forced off by [`StrategyScanPolicy::default`]:
/// `BacktestConfig::conservative_defaults()`'s `integrity_stale_threshold_ticks`
/// (120) is calibrated for intraday bars and would spuriously flag every
/// daily-bar gap (86,400s apart) as stale. Because one scan invocation may
/// cover multiple timeframes, no single hardcoded threshold is correct for
/// all of them, so the scanner's own internal engine runs disable the
/// integrity gate for themselves — this does not affect any live, paper,
/// or single-timeframe backtest CLI path, which keep their own defaults.
#[derive(Clone, Debug)]
pub struct StrategyScanPolicy {
    pub min_bars: usize,
    pub base_config: BacktestConfig,
    pub benchmark_policy: ScanBenchmarkPolicy,
}

impl Default for StrategyScanPolicy {
    fn default() -> Self {
        let mut base_config = BacktestConfig::conservative_defaults();
        base_config.integrity_enabled = false;
        Self {
            min_bars: DEFAULT_MIN_BARS,
            base_config,
            benchmark_policy: ScanBenchmarkPolicy::LegacyFullyInvested,
        }
    }
}

// ---------------------------------------------------------------------------
// Timeframe resolution
// ---------------------------------------------------------------------------

/// Resolve a scanner timeframe label (e.g. `"1D"`) to seconds. Returns
/// `None` for an unrecognized label — the caller reports this as
/// `unsupported_timeframe`, never as a silent default.
pub fn resolve_timeframe_secs(timeframe: &str) -> Option<i64> {
    match timeframe {
        "1m" => Some(60),
        "5m" => Some(300),
        "15m" => Some(900),
        "1H" => Some(3_600),
        "1D" => Some(86_400),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// evaluate_scan_candidate — pure, no IO
// ---------------------------------------------------------------------------

/// Evaluate one `(symbol, timeframe, strategy_id)` candidate.
///
/// # Arguments
/// - `strategy_supported_timeframe_secs`: `Some(secs)` if `strategy_id` is
///   registered with the scanner and requires timeframe `secs`; `None` if
///   the scanner does not know this `strategy_id` at all. The caller
///   derives this once from `PluginRegistry::list()` (already in-memory,
///   no IO).
/// - `strategy`: an already-instantiated strategy for `symbol`, or `None`.
///   Only consulted when every other precondition (supported strategy,
///   matching timeframe, bars present, enough bars) already passed.
/// - `bars`: `None` means the caller found no local bars file.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_scan_candidate(
    symbol: &str,
    timeframe: &str,
    strategy_id: &str,
    strategy_supported_timeframe_secs: Option<i64>,
    strategy: Option<Box<dyn Strategy>>,
    bars: Option<&[BacktestBar]>,
    policy: &StrategyScanPolicy,
) -> StrategyScanCandidate {
    evaluate_scan_candidate_with_emission(
        symbol,
        timeframe,
        strategy_id,
        strategy_supported_timeframe_secs,
        strategy,
        None,
        bars,
        policy,
    )
}

/// As [`evaluate_scan_candidate`], additionally taking a SECOND, independent
/// instance of the same strategy. Under
/// [`ScanBenchmarkPolicy::CapitalMatchedExactTargetV2`] that instance is
/// consumed to emit the candidate's exact native target stream (the engine
/// consumes the first instance for the candidate run itself). Under the
/// legacy policy it is ignored. A V2 policy with no emission instance fails
/// the candidate closed -- it never degrades to the legacy benchmark.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_scan_candidate_with_emission(
    symbol: &str,
    timeframe: &str,
    strategy_id: &str,
    strategy_supported_timeframe_secs: Option<i64>,
    strategy: Option<Box<dyn Strategy>>,
    emission_strategy: Option<Box<dyn Strategy>>,
    bars: Option<&[BacktestBar]>,
    policy: &StrategyScanPolicy,
) -> StrategyScanCandidate {
    let bars_available = bars.map(|b| b.len()).unwrap_or(0);

    let Some(required_secs) = strategy_supported_timeframe_secs else {
        return StrategyScanCandidate::skipped(
            symbol,
            timeframe,
            strategy_id,
            bars_available,
            StrategyScanTruthState::UnsupportedStrategy,
            StrategyScanReasonCode::StrategyNotSupportedByScanner,
            vec![format!(
                "strategy_id '{strategy_id}' is not registered with the scanner"
            )],
        );
    };

    let Some(requested_secs) = resolve_timeframe_secs(timeframe) else {
        return StrategyScanCandidate::skipped(
            symbol,
            timeframe,
            strategy_id,
            bars_available,
            StrategyScanTruthState::UnsupportedTimeframe,
            StrategyScanReasonCode::TimeframeNotSupportedByScanner,
            vec![format!(
                "timeframe '{timeframe}' is not recognized by the scanner"
            )],
        );
    };

    if requested_secs != required_secs {
        return StrategyScanCandidate::skipped(
            symbol,
            timeframe,
            strategy_id,
            bars_available,
            StrategyScanTruthState::UnsupportedTimeframe,
            StrategyScanReasonCode::TimeframeNotSupportedByScanner,
            vec![format!(
                "strategy '{strategy_id}' requires timeframe_secs={required_secs}, but '{timeframe}' resolves to {requested_secs}"
            )],
        );
    }

    let Some(bars) = bars else {
        return StrategyScanCandidate::skipped(
            symbol,
            timeframe,
            strategy_id,
            0,
            StrategyScanTruthState::DataMissing,
            StrategyScanReasonCode::MissingBarsFile,
            vec![format!(
                "no local bars file found for symbol={symbol} timeframe={timeframe}"
            )],
        );
    };

    if bars.len() < policy.min_bars {
        return StrategyScanCandidate::skipped(
            symbol,
            timeframe,
            strategy_id,
            bars.len(),
            StrategyScanTruthState::InsufficientData,
            StrategyScanReasonCode::NotEnoughBars,
            vec![format!(
                "{} bars available, {} required",
                bars.len(),
                policy.min_bars
            )],
        );
    }

    let Some(strategy) = strategy else {
        // Fail closed: supported-strategy precondition passed but the
        // caller could not actually instantiate it (should not happen in
        // practice — defense in depth against a caller bug).
        return StrategyScanCandidate::skipped(
            symbol,
            timeframe,
            strategy_id,
            bars.len(),
            StrategyScanTruthState::UnsupportedStrategy,
            StrategyScanReasonCode::StrategyNotSupportedByScanner,
            vec![format!(
                "strategy_id '{strategy_id}' declared supported but no instance was provided"
            )],
        );
    };

    let mut cfg = policy.base_config.clone();
    cfg.timeframe_secs = required_secs;
    if !cfg.sizing_policy.is_capital_fraction() {
        cfg.sizing = StrategySizingConfig::default_sizing();
    }

    // A capital-fraction candidate is judged only by the capital-fraction
    // benchmark and a fixed-quantity candidate never by it; there is no
    // cross-policy substitution in either direction.
    if cfg.sizing_policy.is_capital_fraction()
        != (policy.benchmark_policy == ScanBenchmarkPolicy::CapitalFractionMatchedPassiveV1)
    {
        return StrategyScanCandidate::skipped(
            symbol,
            timeframe,
            strategy_id,
            bars.len(),
            StrategyScanTruthState::MetricsUnavailable,
            StrategyScanReasonCode::BenchmarkUnavailable,
            vec![format!(
                "sizing policy '{}' is incompatible with benchmark policy {:?}",
                cfg.sizing_policy.policy_id(),
                policy.benchmark_policy.policy_id()
            )],
        );
    }

    let mut engine = BacktestEngine::new(cfg.clone());
    if let Err(e) = engine.add_strategy(strategy) {
        return StrategyScanCandidate::skipped(
            symbol,
            timeframe,
            strategy_id,
            bars.len(),
            StrategyScanTruthState::BacktestFailed,
            StrategyScanReasonCode::BacktestError,
            vec![format!("add_strategy failed: {e:?}")],
        );
    }

    let report = match engine.run(bars) {
        Ok(r) => r,
        Err(e) => {
            return StrategyScanCandidate::skipped(
                symbol,
                timeframe,
                strategy_id,
                bars.len(),
                StrategyScanTruthState::BacktestFailed,
                StrategyScanReasonCode::BacktestError,
                vec![format!("engine run failed: {e}")],
            );
        }
    };

    let point = SweepPoint {
        target_qty: cfg.sizing.target_qty,
        max_target_qty: cfg.sizing.max_target_qty,
        max_position_notional_usd: cfg.sizing.max_position_notional_usd,
        slippage_bps: cfg.stress.slippage_bps,
        volatility_mult_bps: cfg.stress.volatility_mult_bps,
        max_participation_rate_bps: cfg.liquidity.max_participation_rate_bps,
    };
    let row = sweep_row_from_report(&report, &point, None);

    let data_start_ts = bars.first().map(|b| b.end_ts);
    let data_end_ts = bars.last().map(|b| b.end_ts);

    let mut metrics = StrategyScanMetrics {
        total_return_pct: Some(row.total_return_pct),
        benchmark_return_pct: row.buy_and_hold_return_pct,
        alpha_pct: row.alpha_pct,
        max_drawdown_pct: Some(row.max_drawdown_pct),
        trade_count: Some(row.trade_count),
        win_rate_pct: row.win_rate_pct,
        profit_factor: row.profit_factor,
        fill_count: Some(row.fill_count),
        bars_used: bars.len(),
        data_start_ts,
        data_end_ts,
        halted: row.halted,
        benchmark_v2: None,
        benchmark_capital_fraction: None,
    };

    if policy.benchmark_policy == ScanBenchmarkPolicy::CapitalMatchedExactTargetV2 {
        // Alpha authority for this policy is Benchmark V2 ONLY. The legacy
        // fully-invested return is retained as informational evidence and
        // never feeds alpha, score or review.
        match benchmark_v2_evidence(
            symbol,
            timeframe,
            strategy_id,
            &cfg,
            bars,
            &report,
            emission_strategy,
            row.buy_and_hold_return_pct,
        ) {
            Ok(evidence) => {
                metrics.total_return_pct = Some(evidence.candidate_total_return_pct);
                metrics.benchmark_return_pct = Some(evidence.benchmark_account_return_pct);
                metrics.alpha_pct = Some(evidence.alpha_pct);
                metrics.benchmark_v2 = Some(evidence);
            }
            Err(reason) => {
                return StrategyScanCandidate::skipped(
                    symbol,
                    timeframe,
                    strategy_id,
                    bars.len(),
                    StrategyScanTruthState::MetricsUnavailable,
                    StrategyScanReasonCode::BenchmarkUnavailable,
                    vec![format!(
                        "{} unavailable: {reason}",
                        crate::benchmark_v2::BENCHMARK_V2_POLICY_ID
                    )],
                );
            }
        }
    }

    if policy.benchmark_policy == ScanBenchmarkPolicy::CapitalFractionMatchedPassiveV1 {
        match capital_fraction_benchmark_evidence(
            symbol,
            timeframe,
            strategy_id,
            required_secs,
            &cfg,
            bars,
            &report,
            row.buy_and_hold_return_pct,
        ) {
            Ok(evidence) => {
                metrics.total_return_pct = Some(evidence.candidate_total_return_pct);
                metrics.benchmark_return_pct = Some(evidence.benchmark_account_return_pct);
                metrics.alpha_pct = Some(evidence.alpha_pct);
                metrics.benchmark_capital_fraction = Some(evidence);
            }
            Err(reason) => {
                return StrategyScanCandidate::skipped(
                    symbol,
                    timeframe,
                    strategy_id,
                    bars.len(),
                    StrategyScanTruthState::MetricsUnavailable,
                    StrategyScanReasonCode::BenchmarkUnavailable,
                    vec![format!(
                        "{} unavailable: {reason}",
                        crate::benchmark_capital_fraction::BENCHMARK_CAPITAL_FRACTION_POLICY_ID
                    )],
                );
            }
        }
    }

    let mut warnings = Vec::new();
    if row.halted {
        warnings.push("backtest halted before processing all bars".to_string());
    }
    if row.trade_count == 0 {
        warnings.push("no completed round-trip trades".to_string());
    }

    let score = metrics.alpha_pct.or(metrics.total_return_pct);

    StrategyScanCandidate {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        strategy_id: strategy_id.to_string(),
        bars_available: bars.len(),
        truth_state: StrategyScanTruthState::CandidateRanked,
        reason_code: StrategyScanReasonCode::Ranked,
        score,
        rank: None,
        metrics,
        warnings,
        blockers: Vec::new(),
    }
}

/// Compute the Benchmark V2 evidence for one already-run candidate. Every
/// failure is an `Err(reason)`; the caller fails the candidate closed.
#[allow(clippy::too_many_arguments)]
fn benchmark_v2_evidence(
    symbol: &str,
    timeframe: &str,
    strategy_id: &str,
    cfg: &BacktestConfig,
    bars: &[BacktestBar],
    report: &crate::types::BacktestReport,
    emission_strategy: Option<Box<dyn Strategy>>,
    legacy_buy_and_hold_return_pct: Option<f64>,
) -> Result<ScanBenchmarkV2Evidence, String> {
    let emission = emission_strategy
        .ok_or_else(|| "no emission strategy instance was provided".to_string())?;
    let stream = crate::native_signals::emit_native_signal_stream(cfg.clone(), bars, emission)
        .map_err(|e| format!("native signal emission failed: {e}"))?;

    // The emitting run must be the same strategy/config/bars/execution model
    // as the candidate run (documented equal to a plain run of the unwrapped
    // strategy); otherwise the stream does not describe this candidate.
    if stream.run_id != report.run_id {
        return Err(format!(
            "emission run_id {} != candidate run_id {}",
            stream.run_id, report.run_id
        ));
    }
    if stream.semantic_fingerprint != report.strategy_semantic_fingerprint {
        return Err("emission semantic fingerprint != candidate semantic fingerprint".to_string());
    }
    if stream.symbol != symbol {
        return Err(format!(
            "emitted target symbol '{}' != candidate symbol '{symbol}'",
            stream.symbol
        ));
    }
    let candidate_target_qty_micros = stream
        .rows
        .iter()
        .map(|r| r.target_qty_micros)
        .find(|q| *q > 0)
        .ok_or_else(|| "candidate never emitted a positive target quantity".to_string())?;

    // Candidate return on the SAME initial-cash basis Benchmark V2 uses.
    let initial = cfg.initial_cash_micros;
    if initial <= 0 {
        return Err("initial_cash_micros must be positive".to_string());
    }
    let ending = report
        .equity_curve
        .last()
        .map(|(_, eq)| *eq)
        .unwrap_or(initial);
    let candidate_total_return_pct = (ending - initial) as f64 / initial as f64 * 100.0;

    let bench = crate::benchmark_v2::compute_benchmark_v2(
        &stream,
        bars,
        cfg.clone(),
        candidate_total_return_pct,
    )
    .map_err(|e| e.to_string())?;

    Ok(ScanBenchmarkV2Evidence {
        policy_id: bench.policy_id.clone(),
        strategy_id: strategy_id.to_string(),
        strategy_semantic_fingerprint: report.strategy_semantic_fingerprint.clone(),
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        candidate_run_id: report.run_id.to_string(),
        candidate_config_id: report.config_id.to_string(),
        candidate_initial_cash_micros: initial,
        candidate_target_qty_micros,
        candidate_execution_model_id: report.execution_model_id.clone(),
        candidate_total_return_pct,
        required_history_bars: stream.required_history_bars,
        benchmark_run_id: bench.benchmark_run_id.clone(),
        benchmark_config_id: bench.config_id.clone(),
        benchmark_target_qty_micros: bench.target_qty_micros,
        benchmark_eligibility_bar_index: bench.eligibility_bar_index,
        benchmark_eligibility_decision_ts: bench.eligibility_decision_ts,
        benchmark_initial_cash_micros: bench.initial_cash_micros,
        benchmark_execution_model_id: bench.execution_model_id.clone(),
        benchmark_account_return_pct: bench.account_return_pct,
        alpha_pct: bench.alpha_pct,
        input_data_hash: report.input_data_hash.clone(),
        evaluation_end_ts: bars.last().map(|b| b.end_ts).unwrap_or(0),
        legacy_buy_and_hold_return_pct,
    })
}

/// Compute the capital-fraction benchmark evidence for one already-run
/// candidate. Every failure is an `Err(reason)`; the caller fails closed.
#[allow(clippy::too_many_arguments)]
fn capital_fraction_benchmark_evidence(
    symbol: &str,
    timeframe: &str,
    strategy_id: &str,
    timeframe_secs: i64,
    cfg: &BacktestConfig,
    bars: &[BacktestBar],
    report: &crate::types::BacktestReport,
    legacy_buy_and_hold_return_pct: Option<f64>,
) -> Result<ScanCapitalFractionBenchmarkEvidence, String> {
    let initial = cfg.initial_cash_micros;
    if initial <= 0 {
        return Err("initial_cash_micros must be positive".to_string());
    }
    let ending = report
        .equity_curve
        .last()
        .map(|(_, eq)| *eq)
        .unwrap_or(initial);
    let candidate_total_return_pct = (ending - initial) as f64 / initial as f64 * 100.0;

    let bench = crate::benchmark_capital_fraction::compute_capital_fraction_benchmark(
        report,
        bars,
        cfg,
        timeframe_secs,
        candidate_total_return_pct,
    )
    .map_err(|e| e.to_string())?;
    if bench.symbol != symbol {
        return Err(format!(
            "benchmark symbol '{}' != candidate symbol '{symbol}'",
            bench.symbol
        ));
    }

    let mut cost_basis_cfg = cfg.clone();
    cost_basis_cfg.sizing_policy = mqk_strategy::SizingPolicy::FixedQuantityV1;

    Ok(ScanCapitalFractionBenchmarkEvidence {
        policy_id: bench.policy_id.clone(),
        strategy_id: strategy_id.to_string(),
        strategy_semantic_fingerprint: report.strategy_semantic_fingerprint.clone(),
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        candidate_run_id: report.run_id.to_string(),
        candidate_config_id: report.config_id.to_string(),
        candidate_cost_basis_config_id: cost_basis_cfg.config_id().to_string(),
        sizing_policy_id: bench.sizing_policy_id.clone(),
        allocation_fraction_bps: bench.allocation_fraction_bps,
        initial_allocated_capital_micros: bench.initial_allocated_capital_micros,
        position_budget_micros: bench.position_budget_micros,
        reference_bar_end_ts: bench.reference_bar_end_ts,
        reference_price_micros: bench.reference_price_micros,
        candidate_target_qty_micros: bench.target_qty_micros,
        candidate_initial_cash_micros: initial,
        candidate_execution_model_id: report.execution_model_id.clone(),
        candidate_total_return_pct,
        benchmark_run_id: bench.benchmark_run_id.clone(),
        benchmark_config_id: bench.config_id.clone(),
        benchmark_target_qty_micros: bench.target_qty_micros,
        benchmark_entry_bar_index: bench.entry_bar_index,
        benchmark_initial_cash_micros: bench.initial_cash_micros,
        benchmark_execution_model_id: bench.execution_model_id.clone(),
        benchmark_account_return_pct: bench.account_return_pct,
        alpha_pct: bench.alpha_pct,
        input_data_hash: report.input_data_hash.clone(),
        evaluation_end_ts: bars.last().map(|b| b.end_ts).unwrap_or(0),
        legacy_buy_and_hold_return_pct,
    })
}

// ---------------------------------------------------------------------------
// Deterministic ranking
// ---------------------------------------------------------------------------

/// Sort candidates and assign 1-based `rank` to every `candidate_ranked`
/// row (in sorted order). Skipped candidates always have `rank = None`.
///
/// Order:
/// 1. `candidate_ranked` rows before any skipped row.
/// 2. Higher `score` first (`None` score sorts after any `Some` score).
/// 3. `symbol` ascending.
/// 4. `timeframe` ascending.
/// 5. `strategy_id` ascending.
///
/// No randomness; a stable sort over a fully-ordered key, so re-running
/// the same candidate set always produces the same order.
pub fn rank_scan_candidates(candidates: &mut [StrategyScanCandidate]) {
    candidates.sort_by(|a, b| {
        let a_group = u8::from(a.truth_state != StrategyScanTruthState::CandidateRanked);
        let b_group = u8::from(b.truth_state != StrategyScanTruthState::CandidateRanked);
        a_group
            .cmp(&b_group)
            .then_with(|| match (a.score, b.score) {
                (Some(sa), Some(sb)) => sb.partial_cmp(&sa).unwrap_or(Ordering::Equal),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            })
            .then_with(|| a.symbol.cmp(&b.symbol))
            .then_with(|| a.timeframe.cmp(&b.timeframe))
            .then_with(|| a.strategy_id.cmp(&b.strategy_id))
    });

    let mut next_rank = 1usize;
    for candidate in candidates.iter_mut() {
        if candidate.truth_state == StrategyScanTruthState::CandidateRanked {
            candidate.rank = Some(next_rank);
            next_rank += 1;
        } else {
            candidate.rank = None;
        }
    }
}

// ---------------------------------------------------------------------------
// STRATEGY-SCANNER-JOBS-GUI-01B: shared scan-run + artifact schema.
//
// Moved here from `mqk-cli/src/commands/bkt.rs::run_strategy_scan` so both
// the CLI (`mqk backtest scan-strategies`) and the daemon
// (`POST /api/v1/strategy-scans/jobs`) run the identical local-data-only
// scan and write the identical artifact schema, without the daemon shelling
// out to the CLI binary. No provider, broker, or DB import here — the only
// IO is: read `registry_path`, read `{bars_root}/{timeframe}/
// {symbol}_{timeframe}.csv` files, and (via `write_scan_artifacts`) write
// the artifact directory.
// ---------------------------------------------------------------------------

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mqk_md::instrument_registry::{enabled_equity_symbols, load_instrument_registry};
use mqk_strategy::{engines::register_builtin_strategies_with_sizing, PluginRegistry};

/// Deterministic scan-run manifest. Field-identical to the CLI's prior
/// private `ScanManifest`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanManifest {
    pub schema_version: u32,
    pub scan_id: String,
    pub created_at_utc: String,
    pub git_hash: String,
    pub registry_path: String,
    pub bars_root: String,
    pub timeframe: String,
    pub strategies: Vec<String>,
    pub universe_count: usize,
    pub ranked_count: usize,
    pub skipped_count: usize,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
    /// Benchmark policy this scan evaluated alpha under. `None` = the legacy
    /// fully-invested policy (every historical artifact).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_policy_id: Option<String>,
}

/// Count of skipped candidates sharing one `reason_code`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanSkipReasonCount {
    pub reason_code: String,
    pub count: usize,
}

/// Deterministic scan-run summary. `top_ranked` is owned (not borrowed) so
/// this type can be constructed by either the CLI (single-process, one
/// `Vec<StrategyScanCandidate>` in scope) or the daemon (candidates stored
/// in a job record, summary computed once and cloned into the response).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScanSummary {
    pub scan_id: String,
    pub universe_count: usize,
    pub ranked_count: usize,
    pub skipped_count: usize,
    pub top_ranked: Vec<StrategyScanCandidate>,
    pub top_skip_reasons: Vec<ScanSkipReasonCount>,
}

/// Bounded request describing one local-data scan run. Every path is
/// caller-resolved (relative to the caller's working directory); this
/// function performs no path-escape validation of its own — callers that
/// accept `out_dir`/`registry_path`/`bars_root` from an untrusted operator
/// surface (e.g. the daemon's `POST /api/v1/strategy-scans/jobs`) must apply
/// their own bounds/validation before calling `execute_strategy_scan`.
#[derive(Clone, Debug)]
pub struct ScanRunRequest {
    pub registry_path: String,
    pub bars_root: String,
    pub timeframe: String,
    pub strategies: Vec<String>,
    pub top: usize,
    pub limit_symbols: Option<usize>,
    /// Caller-resolved short git hash (e.g. via `git rev-parse --short HEAD`,
    /// falling back to `"UNKNOWN"`). Kept caller-supplied rather than
    /// re-invoked here so this pure-computation module never spawns a
    /// subprocess itself.
    pub git_hash: String,
    /// Caller-resolved RFC3339 creation timestamp (e.g. `Utc::now()`). Kept
    /// caller-supplied so this function remains deterministic given a fixed
    /// clock reading, matching the existing CLI's own inline `Utc::now()`
    /// call pattern (see `mqk-cli/src/commands/bkt.rs`).
    pub created_at_utc: String,
}

/// Result of running a scan (before any artifact file is written).
#[derive(Clone, Debug)]
pub struct ScanRunOutput {
    pub scan_id: uuid::Uuid,
    pub manifest: ScanManifest,
    pub candidates: Vec<StrategyScanCandidate>,
    pub summary: ScanSummary,
}

/// Deterministic UUIDv5 scan identity: re-running with identical inputs
/// (registry path, bars root, timeframe, strategies, resolved universe)
/// always produces the same `scan_id`. Never `Uuid::new_v4()`.
pub fn derive_scan_id(
    registry_path: &str,
    bars_root: &str,
    timeframe: &str,
    strategies: &[String],
    universe: &[String],
) -> uuid::Uuid {
    let canonical = format!(
        "mqk-scan.v1|registry={registry_path}|bars_root={bars_root}|timeframe={timeframe}|strategies={}|universe={}",
        strategies.join(","),
        universe.join(","),
    );
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, canonical.as_bytes())
}

/// [`derive_scan_id`] with the benchmark policy AND the scan's base
/// [`BacktestConfig`] identity bound in, so a V2 scan can never share a
/// `scan_id` (and artifact directory) with a legacy scan of the same universe,
/// nor with a V2 scan evaluated under a different execution/economic config.
pub fn derive_scan_id_with_benchmark(
    registry_path: &str,
    bars_root: &str,
    timeframe: &str,
    strategies: &[String],
    universe: &[String],
    benchmark_policy_id: &str,
    base_config_id: &uuid::Uuid,
) -> uuid::Uuid {
    let canonical = format!(
        "mqk-scan.v3|registry={registry_path}|bars_root={bars_root}|timeframe={timeframe}|strategies={}|universe={}|benchmark_policy={benchmark_policy_id}|base_config={base_config_id}",
        strategies.join(","),
        universe.join(","),
    );
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, canonical.as_bytes())
}

fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Render the full candidate set as CSV. Identical schema to the CLI's
/// prior private `candidates_to_csv`.
pub fn candidates_to_csv(candidates: &[StrategyScanCandidate]) -> String {
    let mut out = String::from(
        "rank,symbol,timeframe,strategy_id,bars_available,truth_state,reason_code,score,total_return_pct,alpha_pct,max_drawdown_pct,trade_count,win_rate_pct,profit_factor,warnings,blockers\n",
    );
    for c in candidates {
        let row = [
            c.rank.map(|r| r.to_string()).unwrap_or_default(),
            csv_field(&c.symbol),
            csv_field(&c.timeframe),
            csv_field(&c.strategy_id),
            c.bars_available.to_string(),
            c.truth_state.code().to_string(),
            c.reason_code.code().to_string(),
            c.score.map(|v| format!("{v:.4}")).unwrap_or_default(),
            c.metrics
                .total_return_pct
                .map(|v| format!("{v:.4}"))
                .unwrap_or_default(),
            c.metrics
                .alpha_pct
                .map(|v| format!("{v:.4}"))
                .unwrap_or_default(),
            c.metrics
                .max_drawdown_pct
                .map(|v| format!("{v:.4}"))
                .unwrap_or_default(),
            c.metrics
                .trade_count
                .map(|v| v.to_string())
                .unwrap_or_default(),
            c.metrics
                .win_rate_pct
                .map(|v| format!("{v:.2}"))
                .unwrap_or_default(),
            c.metrics
                .profit_factor
                .map(|v| format!("{v:.4}"))
                .unwrap_or_default(),
            csv_field(&c.warnings.join(";")),
            csv_field(&c.blockers.join(";")),
        ];
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

/// Run a local-data-only scan: load the instrument registry, resolve the
/// enabled-equity universe (optionally truncated by `limit_symbols`), read
/// each symbol's local bars CSV under `{bars_root}/{timeframe}/`, evaluate
/// every `(symbol, strategy)` candidate via [`evaluate_scan_candidate`], and
/// rank the results via [`rank_scan_candidates`].
///
/// No provider call, no broker call, no live/paper order, no DB connection.
/// The only IO is: read `req.registry_path`, read
/// `{req.bars_root}/{req.timeframe}/{symbol}_{req.timeframe}.csv` files.
/// Does not write any artifact file — see [`write_scan_artifacts`].
pub fn execute_strategy_scan(req: &ScanRunRequest) -> Result<ScanRunOutput, String> {
    execute_strategy_scan_with_benchmark(req, ScanBenchmarkPolicy::LegacyFullyInvested)
}

/// As [`execute_strategy_scan`], evaluating alpha under an explicit benchmark
/// policy. The policy is recorded in the scan manifest and bound into
/// `scan_id` (legacy ids are unchanged).
pub fn execute_strategy_scan_with_benchmark(
    req: &ScanRunRequest,
    benchmark_policy: ScanBenchmarkPolicy,
) -> Result<ScanRunOutput, String> {
    execute_strategy_scan_with_policy(
        req,
        StrategyScanPolicy {
            benchmark_policy,
            ..StrategyScanPolicy::default()
        },
    )
}

/// As [`execute_strategy_scan_with_benchmark`], under an explicit full
/// [`StrategyScanPolicy`]. A promotion-grade V2 scan passes the canonical
/// Backtest config as `policy.base_config` so every candidate's evidence is
/// produced under the same execution/economic contract as the canonical
/// Backtest evidence Promotion consumes.
pub fn execute_strategy_scan_with_policy(
    req: &ScanRunRequest,
    policy: StrategyScanPolicy,
) -> Result<ScanRunOutput, String> {
    let benchmark_policy = policy.benchmark_policy;
    if policy.base_config.sizing_policy.is_capital_fraction()
        != (benchmark_policy == ScanBenchmarkPolicy::CapitalFractionMatchedPassiveV1)
    {
        return Err(format!(
            "sizing policy '{}' requires its own benchmark policy; got {:?}",
            policy.base_config.sizing_policy.policy_id(),
            benchmark_policy.policy_id()
        ));
    }
    if req.strategies.is_empty() {
        return Err("strategies must name at least one strategy_id".to_string());
    }
    if req.top == 0 {
        return Err("top must be > 0".to_string());
    }

    let instruments = load_instrument_registry(Path::new(&req.registry_path)).map_err(|e| {
        format!(
            "load instrument registry failed: {}: {}",
            req.registry_path, e
        )
    })?;
    let mut universe = enabled_equity_symbols(&instruments);
    if let Some(limit) = req.limit_symbols {
        universe.truncate(limit);
    }

    let bars_root_path = Path::new(&req.bars_root);
    let timeframe_dir = bars_root_path.join(&req.timeframe);

    let mut candidates: Vec<StrategyScanCandidate> = Vec::new();
    for symbol in &universe {
        // Fresh per-symbol registry: register_builtin_strategies_with_sizing
        // binds each strategy factory to this symbol via closure capture.
        let mut reg = PluginRegistry::new();
        register_builtin_strategies_with_sizing(&mut reg, symbol.as_str(), 1, None, None)
            .map_err(|e| format!("register_builtin_strategies failed for symbol={symbol}: {e}"))?;

        let bars_path = timeframe_dir.join(format!("{symbol}_{}.csv", req.timeframe));
        // A malformed local bars file is reported the same as a missing one
        // (data_missing) -- honest limitation, not a crash.
        let bars: Option<Vec<BacktestBar>> = if bars_path.is_file() {
            crate::loader::load_csv_file(&bars_path).ok()
        } else {
            None
        };

        for strategy_id in &req.strategies {
            let strategy_timeframe_secs = reg.lookup(strategy_id).ok().map(|m| m.timeframe_secs);
            let strategy_instance = if strategy_timeframe_secs.is_some() {
                reg.instantiate(strategy_id).ok()
            } else {
                None
            };
            let emission_instance = if strategy_timeframe_secs.is_some()
                && benchmark_policy == ScanBenchmarkPolicy::CapitalMatchedExactTargetV2
            {
                reg.instantiate(strategy_id).ok()
            } else {
                None
            };
            candidates.push(evaluate_scan_candidate_with_emission(
                symbol,
                &req.timeframe,
                strategy_id,
                strategy_timeframe_secs,
                strategy_instance,
                emission_instance,
                bars.as_deref(),
                &policy,
            ));
        }
    }

    rank_scan_candidates(&mut candidates);

    let ranked_count = candidates
        .iter()
        .filter(|c| c.truth_state == StrategyScanTruthState::CandidateRanked)
        .count();
    let skipped_count = candidates.len() - ranked_count;

    let mut warnings = Vec::new();
    if !timeframe_dir.is_dir() {
        warnings.push(format!(
            "bars timeframe directory not found: {}",
            timeframe_dir.display()
        ));
    }

    let scan_id = match benchmark_policy.policy_id() {
        None => derive_scan_id(
            &req.registry_path,
            &req.bars_root,
            &req.timeframe,
            &req.strategies,
            &universe,
        ),
        Some(policy_id) => derive_scan_id_with_benchmark(
            &req.registry_path,
            &req.bars_root,
            &req.timeframe,
            &req.strategies,
            &universe,
            policy_id,
            &policy.base_config.config_id(),
        ),
    };
    let manifest = ScanManifest {
        schema_version: 1,
        scan_id: scan_id.to_string(),
        created_at_utc: req.created_at_utc.clone(),
        git_hash: req.git_hash.clone(),
        registry_path: req.registry_path.clone(),
        bars_root: req.bars_root.clone(),
        timeframe: req.timeframe.clone(),
        strategies: req.strategies.clone(),
        universe_count: universe.len(),
        ranked_count,
        skipped_count,
        blockers: Vec::new(),
        warnings,
        benchmark_policy_id: benchmark_policy.policy_id().map(str::to_string),
    };

    let top_ranked: Vec<StrategyScanCandidate> = candidates
        .iter()
        .filter(|c| c.rank.is_some())
        .take(req.top)
        .cloned()
        .collect();

    let mut reason_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for c in candidates
        .iter()
        .filter(|c| c.truth_state != StrategyScanTruthState::CandidateRanked)
    {
        *reason_counts.entry(c.reason_code.code()).or_insert(0) += 1;
    }
    let top_skip_reasons: Vec<ScanSkipReasonCount> = reason_counts
        .into_iter()
        .map(|(reason_code, count)| ScanSkipReasonCount {
            reason_code: reason_code.to_string(),
            count,
        })
        .collect();

    let summary = ScanSummary {
        scan_id: scan_id.to_string(),
        universe_count: universe.len(),
        ranked_count,
        skipped_count,
        top_ranked,
        top_skip_reasons,
    };

    Ok(ScanRunOutput {
        scan_id,
        manifest,
        candidates,
        summary,
    })
}

/// Write `manifest.json` / `candidates.json` / `candidates.csv` /
/// `summary.json` for a completed [`ScanRunOutput`] into
/// `{out_dir}/{scan_id}/`. Returns the created run directory.
pub fn write_scan_artifacts(out_dir: &Path, output: &ScanRunOutput) -> Result<PathBuf, String> {
    let run_dir = out_dir.join(output.scan_id.to_string());
    std::fs::create_dir_all(&run_dir).map_err(|e| {
        format!(
            "create scan artifact dir failed: {}: {e}",
            run_dir.display()
        )
    })?;
    std::fs::write(
        run_dir.join("manifest.json"),
        serde_json::to_string_pretty(&output.manifest)
            .map_err(|e| format!("serialize scan manifest failed: {e}"))?,
    )
    .map_err(|e| format!("write manifest.json failed: {}: {e}", run_dir.display()))?;
    std::fs::write(
        run_dir.join("candidates.json"),
        serde_json::to_string_pretty(&output.candidates)
            .map_err(|e| format!("serialize scan candidates failed: {e}"))?,
    )
    .map_err(|e| format!("write candidates.json failed: {}: {e}", run_dir.display()))?;
    std::fs::write(
        run_dir.join("candidates.csv"),
        candidates_to_csv(&output.candidates),
    )
    .map_err(|e| format!("write candidates.csv failed: {}: {e}", run_dir.display()))?;
    std::fs::write(
        run_dir.join("summary.json"),
        serde_json::to_string_pretty(&output.summary)
            .map_err(|e| format!("serialize scan summary failed: {e}"))?,
    )
    .map_err(|e| format!("write summary.json failed: {}: {e}", run_dir.display()))?;

    Ok(run_dir)
}
