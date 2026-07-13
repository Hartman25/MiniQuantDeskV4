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
    if req.strategies.is_empty() {
        return Err("strategies must name at least one strategy_id".to_string());
    }
    if req.top == 0 {
        return Err("top must be > 0".to_string());
    }

    let instruments = load_instrument_registry(Path::new(&req.registry_path))
        .map_err(|e| format!("load instrument registry failed: {}: {}", req.registry_path, e))?;
    let mut universe = enabled_equity_symbols(&instruments);
    if let Some(limit) = req.limit_symbols {
        universe.truncate(limit);
    }

    let policy = StrategyScanPolicy::default();
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
            candidates.push(evaluate_scan_candidate(
                symbol,
                &req.timeframe,
                strategy_id,
                strategy_timeframe_secs,
                strategy_instance,
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

    let scan_id = derive_scan_id(
        &req.registry_path,
        &req.bars_root,
        &req.timeframe,
        &req.strategies,
        &universe,
    );
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
    std::fs::create_dir_all(&run_dir)
        .map_err(|e| format!("create scan artifact dir failed: {}: {e}", run_dir.display()))?;
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
