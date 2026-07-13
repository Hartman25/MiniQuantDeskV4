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
/// [`BacktestConfig`] (sizing/timeframe are overwritten per candidate);
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
}

impl Default for StrategyScanPolicy {
    fn default() -> Self {
        let mut base_config = BacktestConfig::conservative_defaults();
        base_config.integrity_enabled = false;
        Self {
            min_bars: DEFAULT_MIN_BARS,
            base_config,
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
            vec![format!("timeframe '{timeframe}' is not recognized by the scanner")],
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
    cfg.sizing = StrategySizingConfig::default_sizing();

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
    };
    let row = sweep_row_from_report(&report, &point, None);

    let data_start_ts = bars.first().map(|b| b.end_ts);
    let data_end_ts = bars.last().map(|b| b.end_ts);

    let metrics = StrategyScanMetrics {
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
    };

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
