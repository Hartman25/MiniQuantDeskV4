use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use mqk_portfolio::Side;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub mod backtest_report_artifact; // BKT-PROMOTION-ARTIFACT-AUTHORITY-01
pub use backtest_report_artifact::{
    load_canonical_backtest_report, write_canonical_backtest_report, BacktestReportArtifact,
    BacktestReportArtifactError, BACKTEST_REPORT_ARTIFACT_SCHEMA_VERSION,
};

pub mod stress_suite_artifact; // PROMOTION-STRESS-SUITE-AUTHORITY-01
pub use stress_suite_artifact::{
    load_canonical_stress_suite, write_canonical_stress_suite, StressSuiteArtifact,
    StressSuiteArtifactError, STRESS_SUITE_ARTIFACT_SCHEMA_VERSION,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunManifest {
    pub schema_version: i32,
    pub run_id: Uuid,
    pub strategy_name: String,
    pub engine_id: String,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeframe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeframe_secs: Option<i64>,
    pub git_hash: String,
    pub config_hash: String,
    pub host_fingerprint: String,
    pub created_at_utc: DateTime<Utc>,
    pub artifacts: ArtifactList,
    /// BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01: truthful economics metadata.
    ///
    /// Always written for new manifests (never silently omitted) -- see
    /// [`ManifestEconomics`]. `#[serde(default)]` lets manifests written
    /// before this field existed keep parsing: a missing key defaults to
    /// [`ManifestEconomics::default_equity`], which is exactly the implicit
    /// behavior every pre-existing manifest's run actually had.
    #[serde(default)]
    pub economics: ManifestEconomics,
    /// BKT-PROMOTION-ARTIFACT-AUTHORITY-01: the execution/replay semantic
    /// that produced this run's fills (see
    /// `mqk_backtest::BACKTEST_EXECUTION_MODEL_ID`), mirrored from
    /// [`mqk_backtest::BacktestReport::execution_model_id`] so
    /// `backtest_report.json` can be cross-validated against the manifest
    /// without opening the canonical report first. `#[serde(default)]` lets
    /// manifests written before this field existed keep parsing (empty
    /// string means "not recorded", not "empty by real value" -- see
    /// [`backtest_report_artifact::load_canonical_backtest_report`]).
    /// Empty for live/paper run manifests, which have no backtest execution
    /// model and never call `write_backtest_report`.
    #[serde(default)]
    pub execution_model_id: String,
}

/// Truthful backtest-economics metadata recorded on `manifest.json`.
///
/// BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01: mirrors the economics already
/// carried by `metrics.json` (see `EconomicsSection` below) so the manifest
/// is self-describing without requiring a consumer to also read
/// `metrics.json`. `source` is derived purely from whether the values equal
/// the implicit default (multiplier=1, no margin) -- the same
/// value-equality semantics as `BacktestInstrumentEconomics::is_default_equity`,
/// not a record of operator intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEconomics {
    pub contract_multiplier: i64,
    #[serde(default)]
    pub initial_margin_micros: Option<i64>,
    #[serde(default)]
    pub maintenance_margin_micros: Option<i64>,
    pub margin_enforced: bool,
    pub source: String,
}

impl ManifestEconomics {
    /// Default equity economics: multiplier=1, no margin, not enforced.
    ///
    /// Used by [`init_run_artifacts`] (which has no backtest-specific
    /// economics input -- it is shared by the live/paper run CLI too) and as
    /// the `#[serde(default)]` fallback for manifests written before this
    /// field existed.
    pub fn default_equity() -> Self {
        Self {
            contract_multiplier: 1,
            initial_margin_micros: None,
            maintenance_margin_micros: None,
            margin_enforced: false,
            source: "default_equity".to_string(),
        }
    }

    /// Build manifest economics from a backtest run's actual economics.
    pub fn from_backtest_economics(economics: &mqk_backtest::BacktestEconomicsReport) -> Self {
        let is_default = economics.contract_multiplier == 1
            && economics.initial_margin_micros.is_none()
            && economics.maintenance_margin_micros.is_none();
        Self {
            contract_multiplier: economics.contract_multiplier,
            initial_margin_micros: economics.initial_margin_micros,
            maintenance_margin_micros: economics.maintenance_margin_micros,
            margin_enforced: economics.margin_enforced,
            source: if is_default {
                "default_equity"
            } else {
                "explicit_request"
            }
            .to_string(),
        }
    }
}

impl Default for ManifestEconomics {
    fn default() -> Self {
        Self::default_equity()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactList {
    pub audit_jsonl: String,
    pub manifest_json: String,
    pub orders_csv: String,
    pub fills_csv: String,
    pub equity_curve_csv: String,
    pub metrics_json: String,
}

pub struct InitRunArtifactsArgs<'a> {
    pub exports_root: &'a Path, // e.g. ../exports
    pub schema_version: i32,
    pub run_id: Uuid,
    pub strategy_name: &'a str,
    pub engine_id: &'a str,
    pub mode: &'a str,
    pub timeframe: Option<&'a str>,
    pub timeframe_secs: Option<i64>,
    pub git_hash: &'a str,
    pub config_hash: &'a str,
    pub host_fingerprint: &'a str,
    /// I9-4: injected creation time for deterministic manifest.
    ///
    /// Pass `time_source.now_utc()` in production or a fixed timestamp in
    /// tests.  Eliminates `Utc::now()` from the artifacts path so that two
    /// independent runs with identical inputs produce byte-identical manifests.
    pub now_utc: DateTime<Utc>,
}

pub struct InitRunArtifactsResult {
    pub run_dir: PathBuf,
    pub manifest_path: PathBuf,
}

pub fn init_run_artifacts(args: InitRunArtifactsArgs<'_>) -> Result<InitRunArtifactsResult> {
    // exports/<run_id>/
    let run_dir = args.exports_root.join(args.run_id.to_string());
    fs::create_dir_all(&run_dir)
        .with_context(|| format!("create exports dir failed: {}", run_dir.display()))?;

    // Create placeholder files if missing (do not overwrite existing).
    ensure_file_exists_with(&run_dir.join("audit.jsonl"), "")?;
    ensure_file_exists_with(
        &run_dir.join("orders.csv"),
        "ts_utc,order_id,symbol,side,qty,order_type,limit_price,stop_price,status\n",
    )?;
    ensure_file_exists_with(
        &run_dir.join("fills.csv"),
        "ts_utc,fill_id,order_id,symbol,side,qty,price,fee\n",
    )?;
    ensure_file_exists_with(&run_dir.join("equity_curve.csv"), "ts_utc,equity\n")?;
    ensure_file_exists_with(&run_dir.join("metrics.json"), "{}\n")?;

    // Write manifest.json (overwrite is OK; it's deterministic for a run start).
    let manifest = RunManifest {
        schema_version: args.schema_version,
        run_id: args.run_id,
        strategy_name: args.strategy_name.to_string(),
        engine_id: args.engine_id.to_string(),
        mode: args.mode.to_string(),
        timeframe: args
            .timeframe
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string),
        timeframe_secs: args.timeframe_secs,
        git_hash: args.git_hash.to_string(),
        config_hash: args.config_hash.to_string(),
        host_fingerprint: args.host_fingerprint.to_string(),
        created_at_utc: args.now_utc, // I9-4: injected, not Utc::now()
        artifacts: ArtifactList {
            audit_jsonl: "audit.jsonl".to_string(),
            manifest_json: "manifest.json".to_string(),
            orders_csv: "orders.csv".to_string(),
            fills_csv: "fills.csv".to_string(),
            equity_curve_csv: "equity_curve.csv".to_string(),
            metrics_json: "metrics.json".to_string(),
        },
        // This function has no backtest-specific economics input (it is
        // shared by the live/paper run CLI). Backtest callers correct this
        // to the run's real economics via `write_backtest_report`.
        economics: ManifestEconomics::default_equity(),
        // Likewise unknown at init time (this function runs before/without
        // a BacktestReport for live/paper callers); backtest callers correct
        // this via `write_backtest_report`.
        execution_model_id: String::new(),
    };

    let manifest_path = run_dir.join("manifest.json");
    let json = serde_json::to_string_pretty(&manifest).context("serialize manifest failed")?;
    fs::write(&manifest_path, format!("{json}\n"))
        .with_context(|| format!("write manifest failed: {}", manifest_path.display()))?;

    Ok(InitRunArtifactsResult {
        run_dir,
        manifest_path,
    })
}

fn ensure_file_exists_with(path: &Path, contents_if_create: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    fs::write(path, contents_if_create)
        .with_context(|| format!("create placeholder failed: {}", path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Strategy Lab artifact evaluator (read-only)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct StrategyLabArtifactMetrics {
    #[serde(default)]
    strategy_name: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    symbols: Vec<String>,
    #[serde(default)]
    timeframe: Option<String>,
    #[serde(default)]
    timeframe_secs: Option<i64>,
    #[serde(default)]
    total_return_pct: Option<f64>,
    #[serde(default)]
    max_drawdown_pct: Option<f64>,
    #[serde(default)]
    trade_count: Option<usize>,
    #[serde(default)]
    win_rate_pct: Option<f64>,
    #[serde(default)]
    profit_factor: Option<f64>,
    #[serde(default)]
    expectancy: Option<f64>,
    #[serde(default)]
    trade_frequency: Option<f64>,
    #[serde(default)]
    exposure_time_pct: Option<f64>,
    #[serde(default)]
    sharpe_ratio: Option<f64>,
    #[serde(default)]
    buy_and_hold_return_pct: Option<f64>,
    #[serde(default)]
    alpha_pct: Option<f64>,
    #[serde(default)]
    benchmark: Option<StrategyLabArtifactBenchmark>,
}

#[derive(Debug, Clone, Deserialize)]
struct StrategyLabArtifactBenchmark {
    #[serde(default)]
    buy_and_hold_return_pct: Option<f64>,
    #[serde(default)]
    alpha_pct: Option<f64>,
}

/// Build a Strategy Lab input from an existing completed backtest artifact folder.
///
/// This is read-only: it only reads `metrics.json` and, when present,
/// `manifest.json`. It does not run a backtest, create jobs, write files, call
/// providers, or make paper/live promotion decisions.
pub fn strategy_lab_input_from_artifact_dir(
    artifact_dir: impl AsRef<Path>,
) -> Result<mqk_backtest::StrategyLabInput> {
    let artifact_dir = artifact_dir.as_ref();
    if !artifact_dir.is_dir() {
        anyhow::bail!(
            "artifact directory does not exist or is not a directory: {}",
            artifact_dir.display()
        );
    }

    let metrics_path = artifact_dir.join("metrics.json");
    let metrics_json = fs::read_to_string(&metrics_path)
        .with_context(|| format!("read metrics.json failed: {}", metrics_path.display()))?;
    let metrics: StrategyLabArtifactMetrics = serde_json::from_str(&metrics_json)
        .with_context(|| format!("parse metrics.json failed: {}", metrics_path.display()))?;

    let manifest_path = artifact_dir.join("manifest.json");
    let manifest = if manifest_path.exists() {
        let manifest_json = fs::read_to_string(&manifest_path)
            .with_context(|| format!("read manifest.json failed: {}", manifest_path.display()))?;
        Some(
            serde_json::from_str::<RunManifest>(&manifest_json).with_context(|| {
                format!("parse manifest.json failed: {}", manifest_path.display())
            })?,
        )
    } else {
        None
    };

    Ok(strategy_lab_input_from_artifact_parts(
        metrics,
        manifest.as_ref(),
    ))
}

/// Evaluate an existing completed backtest artifact folder with the pure
/// Strategy Lab contract.
pub fn evaluate_strategy_lab_artifact_dir(
    artifact_dir: impl AsRef<Path>,
) -> Result<mqk_backtest::StrategyLabEvaluation> {
    let input = strategy_lab_input_from_artifact_dir(artifact_dir)?;
    Ok(mqk_backtest::evaluate_strategy_lab(&input))
}

/// Options for read-only Strategy Lab artifact tree ranking.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StrategyLabArtifactRankOptions {
    pub top_n: Option<usize>,
}

/// Deterministic read-only ranking report for completed artifact folders.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StrategyLabArtifactRankReport {
    pub root_path: PathBuf,
    pub candidates_scanned: usize,
    pub evaluations_count: usize,
    pub failures_count: usize,
    pub ranked: Vec<StrategyLabArtifactRankRow>,
    pub failures: Vec<StrategyLabArtifactRankFailure>,
}

/// One ranked Strategy Lab artifact evaluation row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StrategyLabArtifactRankRow {
    pub rank: usize,
    pub artifact_path: PathBuf,
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe: String,
    pub score: f64,
    pub grade: String,
    pub decision: String,
    pub reason_codes: Vec<String>,
    pub total_return: Option<f64>,
    pub max_drawdown: Option<f64>,
    pub trade_count: Option<usize>,
    pub win_rate: Option<f64>,
    pub profit_factor: Option<f64>,
    pub expectancy: Option<f64>,
    pub exposure: Option<f64>,
    pub sharpe: Option<f64>,
    pub buy_hold_return: Option<f64>,
    pub alpha_vs_benchmark: Option<f64>,
}

/// One artifact candidate that could not be evaluated.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StrategyLabArtifactRankFailure {
    pub artifact_path: PathBuf,
    pub error: String,
}

/// Scan an artifact tree and rank completed Strategy Lab artifact folders.
///
/// This is read-only: it identifies folders containing `metrics.json`, evaluates
/// each candidate with the existing single-artifact Strategy Lab evaluator, and
/// returns a deterministic report. It does not run backtests, write files, call
/// providers, touch the database, or make paper/live promotion decisions.
pub fn rank_strategy_lab_artifact_tree(
    root: impl AsRef<Path>,
    options: StrategyLabArtifactRankOptions,
) -> Result<StrategyLabArtifactRankReport> {
    let root = root.as_ref();
    if !root.is_dir() {
        anyhow::bail!(
            "artifact root does not exist or is not a directory: {}",
            root.display()
        );
    }

    let mut candidates = Vec::new();
    collect_strategy_lab_artifact_candidates(root, &mut candidates)?;
    candidates.sort();

    let mut evaluated = Vec::new();
    let mut failures = Vec::new();
    for artifact_path in candidates.iter() {
        match evaluate_strategy_lab_artifact_dir(artifact_path) {
            Ok(evaluation) => evaluated.push((artifact_path.clone(), evaluation)),
            Err(err) => failures.push(StrategyLabArtifactRankFailure {
                artifact_path: artifact_path.clone(),
                error: format!("{err:#}"),
            }),
        }
    }

    rank_strategy_lab_artifact_evaluations(&mut evaluated);
    let evaluations_count = evaluated.len();
    if let Some(top_n) = options.top_n {
        evaluated.truncate(top_n);
    }

    let ranked = evaluated
        .into_iter()
        .enumerate()
        .map(|(idx, (artifact_path, evaluation))| {
            StrategyLabArtifactRankRow::from_evaluation(idx + 1, artifact_path, evaluation)
        })
        .collect::<Vec<_>>();

    let failures_count = failures.len();

    Ok(StrategyLabArtifactRankReport {
        root_path: root.to_path_buf(),
        candidates_scanned: candidates.len(),
        evaluations_count,
        failures_count,
        ranked,
        failures,
    })
}

impl StrategyLabArtifactRankRow {
    fn from_evaluation(
        rank: usize,
        artifact_path: PathBuf,
        evaluation: mqk_backtest::StrategyLabEvaluation,
    ) -> Self {
        Self {
            rank,
            artifact_path,
            strategy_id: evaluation.strategy_id,
            symbol: evaluation.symbol,
            timeframe: evaluation.timeframe,
            score: evaluation.score,
            grade: evaluation.grade.code().to_string(),
            decision: evaluation.decision.code().to_string(),
            reason_codes: evaluation
                .reason_codes
                .iter()
                .map(|reason| reason.code().to_string())
                .collect(),
            total_return: evaluation.total_return,
            max_drawdown: evaluation.max_drawdown,
            trade_count: evaluation.trade_count,
            win_rate: evaluation.win_rate,
            profit_factor: evaluation.profit_factor,
            expectancy: evaluation.expectancy,
            exposure: evaluation.exposure,
            sharpe: evaluation.sharpe,
            buy_hold_return: evaluation.buy_hold_return,
            alpha_vs_benchmark: evaluation.alpha_vs_benchmark,
        }
    }
}

fn collect_strategy_lab_artifact_candidates(
    root: &Path,
    candidates: &mut Vec<PathBuf>,
) -> Result<()> {
    if root.join("metrics.json").is_file() {
        candidates.push(root.to_path_buf());
    }

    let mut child_dirs = Vec::new();
    for entry in fs::read_dir(root)
        .with_context(|| format!("read artifact tree failed: {}", root.display()))?
    {
        let entry = entry
            .with_context(|| format!("read artifact tree entry failed: {}", root.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read artifact tree file type failed: {}", path.display()))?;
        if file_type.is_dir() {
            child_dirs.push(path);
        }
    }
    child_dirs.sort();

    for child in child_dirs {
        collect_strategy_lab_artifact_candidates(&child, candidates)?;
    }

    Ok(())
}

fn rank_strategy_lab_artifact_evaluations(
    rows: &mut [(PathBuf, mqk_backtest::StrategyLabEvaluation)],
) {
    rows.sort_by(|(path_a, a), (path_b, b)| {
        strategy_lab_decision_priority(a.decision)
            .cmp(&strategy_lab_decision_priority(b.decision))
            .then_with(|| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal))
            .then_with(|| {
                strategy_lab_grade_priority(a.grade).cmp(&strategy_lab_grade_priority(b.grade))
            })
            .then_with(|| a.strategy_id.cmp(&b.strategy_id))
            .then_with(|| a.symbol.cmp(&b.symbol))
            .then_with(|| a.timeframe.cmp(&b.timeframe))
            .then_with(|| path_a.cmp(path_b))
    });
}

fn strategy_lab_decision_priority(decision: mqk_backtest::StrategyLabDecision) -> u8 {
    match decision {
        mqk_backtest::StrategyLabDecision::ResearchPass => 0,
        mqk_backtest::StrategyLabDecision::ResearchWatch => 1,
        mqk_backtest::StrategyLabDecision::ResearchFail => 2,
        mqk_backtest::StrategyLabDecision::InsufficientData => 3,
    }
}

fn strategy_lab_grade_priority(grade: mqk_backtest::StrategyLabGrade) -> u8 {
    match grade {
        mqk_backtest::StrategyLabGrade::A => 0,
        mqk_backtest::StrategyLabGrade::B => 1,
        mqk_backtest::StrategyLabGrade::C => 2,
        mqk_backtest::StrategyLabGrade::D => 3,
        mqk_backtest::StrategyLabGrade::F => 4,
        mqk_backtest::StrategyLabGrade::Insufficient => 5,
    }
}

fn strategy_lab_input_from_artifact_parts(
    metrics: StrategyLabArtifactMetrics,
    manifest: Option<&RunManifest>,
) -> mqk_backtest::StrategyLabInput {
    let strategy_id = first_non_empty([
        metrics.strategy_name.as_deref(),
        manifest.map(|m| m.strategy_name.as_str()),
    ])
    .unwrap_or("unknown")
    .to_string();

    let symbol = metrics
        .symbol
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            if metrics.symbols.is_empty() {
                None
            } else {
                Some(metrics.symbols.join(","))
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    let timeframe = manifest
        .and_then(|m| m.timeframe.as_deref())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .or_else(|| manifest.and_then(|m| timeframe_from_secs(m.timeframe_secs)))
        .or_else(|| {
            metrics
                .timeframe
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
        })
        .or_else(|| timeframe_from_secs(metrics.timeframe_secs))
        .unwrap_or_else(|| "unknown".to_string());

    let benchmark_buy_hold = metrics
        .benchmark
        .as_ref()
        .and_then(|b| b.buy_and_hold_return_pct)
        .or(metrics.buy_and_hold_return_pct);
    let benchmark_alpha = metrics
        .benchmark
        .as_ref()
        .and_then(|b| b.alpha_pct)
        .or(metrics.alpha_pct);

    mqk_backtest::StrategyLabInput {
        strategy_id,
        symbol,
        timeframe,
        metrics: mqk_backtest::StrategyLabMetrics {
            total_return: metrics.total_return_pct,
            max_drawdown: metrics.max_drawdown_pct,
            trade_count: metrics.trade_count,
            win_rate: metrics.win_rate_pct,
            profit_factor: metrics.profit_factor,
            expectancy: metrics.expectancy,
            trade_frequency: metrics.trade_frequency,
            exposure: metrics.exposure_time_pct,
            sharpe: metrics.sharpe_ratio,
            buy_hold_return: benchmark_buy_hold,
            alpha_vs_benchmark: benchmark_alpha,
        },
    }
}

fn first_non_empty<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Option<&'a str> {
    values.into_iter().flatten().find(|s| !s.trim().is_empty())
}

fn timeframe_from_secs(timeframe_secs: Option<i64>) -> Option<String> {
    let secs = timeframe_secs?;
    let timeframe = match secs {
        60 => "1m".to_string(),
        300 => "5m".to_string(),
        900 => "15m".to_string(),
        3_600 => "1h".to_string(),
        86_400 => "1D".to_string(),
        secs if secs > 0 => format!("{secs}s"),
        _ => return None,
    };
    Some(timeframe)
}

// ---------------------------------------------------------------------------
// Backtest report writer (deterministic outputs)
// ---------------------------------------------------------------------------

/// Buy-and-hold benchmark comparison fields.
///
/// Computed from the first and last bar prices recorded by the engine.
/// None if fewer than 2 bars were processed or prices are invalid.
#[derive(Debug, Clone, Serialize)]
struct BenchmarkSection {
    /// Open price of the first bar (micros). Basis for benchmark entry price.
    first_bar_open_micros: i64,
    /// Close price of the last bar (micros). Basis for benchmark exit price.
    last_bar_close_micros: i64,
    /// Buy-and-hold return: ((last_close / first_open) - 1) * 100.
    buy_and_hold_return_pct: f64,
    /// Strategy total return percent (same as top-level total_return_pct).
    strategy_total_return_pct: f64,
    /// Alpha: strategy_total_return_pct - buy_and_hold_return_pct.
    alpha_pct: f64,
    /// Assumption note for transparency.
    assumption: &'static str,
}

/// Compute benchmark section. Returns None if prices are absent or first_open <= 0.
fn compute_benchmark(
    first_open: Option<i64>,
    last_close: Option<i64>,
    strategy_return_pct: f64,
) -> Option<BenchmarkSection> {
    let first = first_open?;
    let last = last_close?;
    if first <= 0 {
        return None;
    }
    let bah = (last as f64 / first as f64 - 1.0) * 100.0;
    Some(BenchmarkSection {
        first_bar_open_micros: first,
        last_bar_close_micros: last,
        buy_and_hold_return_pct: bah,
        strategy_total_return_pct: strategy_return_pct,
        alpha_pct: strategy_return_pct - bah,
        assumption: "single-symbol buy-and-hold; first bar open → last bar close; no commissions",
    })
}

/// Deep performance metrics fields, computed by pure helper functions.
#[derive(Debug, Clone, Serialize)]
struct BacktestMetrics<'a> {
    // --- Existing fields (preserved; schema_version stays 1 — additive change) ---
    schema_version: i32,
    run_id: Uuid,
    /// BKT-FUTURE-EXECUTION-01-REPAIR-01: the execution/replay semantic
    /// that actually produced this run's fills (see
    /// `mqk_backtest::BACKTEST_EXECUTION_MODEL_ID`), folded into `run_id`.
    execution_model_id: &'a str,
    strategy_name: &'a str,
    halted: bool,
    halt_reason: Option<&'a str>,
    execution_blocked: bool,
    bars: usize,
    orders: usize,
    orders_filled: usize,
    orders_rejected: usize,
    fills: usize,
    /// Preserved for backward compatibility. Equals `ending_equity_micros`.
    final_equity_micros: i64,
    symbols: Vec<&'a str>,
    last_prices_micros: std::collections::BTreeMap<&'a str, i64>,

    // --- BACKTEST-METRICS-01: equity performance ---
    starting_equity_micros: i64,
    /// Alias for final_equity_micros; both carry the same value.
    ending_equity_micros: i64,
    /// ending_equity - starting_equity (signed; negative = net loss).
    total_return_micros: i64,
    /// (ending - starting) / starting * 100. E.g. 12.34 means 12.34%.
    total_return_pct: f64,
    /// Peak equity seen over the run (including starting equity as baseline).
    equity_high_water_mark_micros: i64,
    /// Largest peak-to-trough decline in equity (always >= 0).
    max_drawdown_micros: i64,
    /// max_drawdown_micros / hwm * 100. E.g. 5.0 means 5%.
    max_drawdown_pct: f64,

    // --- BACKTEST-METRICS-01: costs ---
    /// Sum of all fill fee_micros across the run.
    total_commission_micros: i64,

    // --- BACKTEST-METRICS-01: FIFO round-trip trade metrics ---
    /// Number of completed round-trip trades (FIFO pairing of BUY/SELL per symbol).
    /// Open positions at run end are NOT counted (no closing price available for trade PnL).
    trade_count: usize,
    winning_trade_count: usize,
    losing_trade_count: usize,
    /// Trades where FIFO-paired PnL == 0 after fees.
    flat_trade_count: usize,
    /// winning / trade_count * 100. None if trade_count == 0.
    win_rate_pct: Option<f64>,
    /// Sum of PnL micros across winning trades (>= 0).
    gross_profit_micros: i64,
    /// Sum of PnL micros across losing trades (<= 0; negative value represents losses).
    gross_loss_micros: i64,
    /// gross_profit / abs(gross_loss). None if no losing trades (infinite).
    profit_factor: Option<f64>,
    /// Average PnL micros of winning trades. None if no winning trades.
    average_win_micros: Option<i64>,
    /// Average PnL micros of losing trades (negative value). None if no losing trades.
    average_loss_micros: Option<i64>,
    /// win_rate * avg_win + (1 - win_rate) * avg_loss (expected PnL per trade).
    /// None if trade_count == 0.
    expectancy_micros: Option<i64>,
    /// Best (highest) single trade PnL micros. None if trade_count == 0.
    best_trade_micros: Option<i64>,
    /// Worst (lowest) single trade PnL micros. None if trade_count == 0.
    worst_trade_micros: Option<i64>,

    // --- BACKTEST-METRICS-01: risk-adjusted ratios ---
    /// Per-bar Sharpe ratio (non-annualized): mean(returns) / std(returns).
    /// Returns are computed bar-by-bar from equity_curve (first return vs starting_equity).
    /// None if fewer than 2 return observations or zero variance.
    sharpe_ratio: Option<f64>,
    /// Per-bar Sortino ratio (non-annualized): mean(returns) / downside_std.
    /// Downside std uses negative returns only (MAR = 0). None if no downside observations.
    sortino_ratio: Option<f64>,

    // --- BACKTEST-METRICS-01: exposure ---
    /// Number of bars where at least one position was non-zero at bar close.
    exposure_bars: usize,
    /// exposure_bars / bars * 100. E.g. 50.0 means 50%.
    exposure_time_pct: f64,

    // --- Benchmark comparison ---
    /// Buy-and-hold benchmark. None if fewer than 2 bars or invalid prices.
    #[serde(skip_serializing_if = "Option::is_none")]
    benchmark: Option<BenchmarkSection>,

    // --- BACKTEST-CONFIG-DETERMINISM-SIZING-01: strategy sizing ---
    strategy_sizing: SizingSection,

    // --- BACKTEST-REPORT-ECONOMICS-ARTIFACT-01: instrument economics ---
    economics: EconomicsSection,
}

/// Strategy sizing configuration as captured in the backtest run.
///
/// Included in metrics.json so artifact consumers can verify the sizing
/// parameters used without needing to re-derive from config_id.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SizingSection {
    target_qty: i64,
    max_target_qty: Option<i64>,
    max_position_notional_usd: Option<i64>,
}

/// Instrument economics as captured in the backtest run.
///
/// BACKTEST-REPORT-ECONOMICS-ARTIFACT-01: included in metrics.json so artifact
/// consumers can see truthfully whether (and how) a run used multiplier/margin
/// economics. Default equity runs (the overwhelming majority today) always
/// report `contract_multiplier=1`, both margins `null`, and `margin_enforced=false`.
#[derive(Debug, Clone, Serialize)]
struct EconomicsSection {
    contract_multiplier: i64,
    initial_margin_micros: Option<i64>,
    maintenance_margin_micros: Option<i64>,
    realized_pnl_micros: i64,
    margin_enforced: bool,
}

// ---------------------------------------------------------------------------
// Pure metric computation helpers
// ---------------------------------------------------------------------------

/// Compute equity curve performance metrics from starting equity and curve data.
///
/// Returns (hwm, max_drawdown_micros, max_drawdown_pct, total_return_micros, total_return_pct).
fn compute_equity_metrics(starting_equity: i64, curve: &[(i64, i64)]) -> (i64, i64, f64, i64, f64) {
    let ending = curve.last().map(|(_, eq)| *eq).unwrap_or(starting_equity);
    let total_return_micros = ending.saturating_sub(starting_equity);
    let total_return_pct = if starting_equity != 0 {
        total_return_micros as f64 / starting_equity as f64 * 100.0
    } else {
        0.0
    };

    // High water mark starts at starting_equity, then tracks curve peaks.
    let mut hwm = starting_equity;
    let mut max_dd_micros: i64 = 0;
    let mut max_dd_pct: f64 = 0.0;

    for &(_, eq) in curve {
        if eq > hwm {
            hwm = eq;
        }
        let dd = hwm.saturating_sub(eq);
        if dd > max_dd_micros {
            max_dd_micros = dd;
            max_dd_pct = if hwm > 0 {
                dd as f64 / hwm as f64 * 100.0
            } else {
                0.0
            };
        }
    }

    (
        hwm,
        max_dd_micros,
        max_dd_pct,
        total_return_micros,
        total_return_pct,
    )
}

/// Compute per-bar Sharpe and Sortino ratios (non-annualized).
///
/// Bar returns: r[0] = (curve[0] - starting) / starting;
///              r[i] = (curve[i] - curve[i-1]) / curve[i-1] for i >= 1.
/// Returns (sharpe, sortino). Both None if fewer than 2 return observations.
fn compute_sharpe_sortino(
    starting_equity: i64,
    curve: &[(i64, i64)],
) -> (Option<f64>, Option<f64>) {
    if curve.is_empty() {
        return (None, None);
    }

    let mut returns: Vec<f64> = Vec::with_capacity(curve.len());

    if starting_equity != 0 {
        returns.push((curve[0].1 - starting_equity) as f64 / starting_equity as f64);
    }
    for i in 1..curve.len() {
        let prev = curve[i - 1].1;
        if prev != 0 {
            returns.push((curve[i].1 - prev) as f64 / prev as f64);
        }
    }

    if returns.len() < 2 {
        return (None, None);
    }

    let n = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / n;

    // Sample standard deviation (n-1 denominator).
    let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let std_dev = variance.sqrt();

    let sharpe = if std_dev > 0.0 {
        Some(mean / std_dev)
    } else {
        None
    };

    // Sortino: downside deviation from MAR=0 (uses all negative returns, population denominator).
    let downside: Vec<f64> = returns.iter().filter(|&&r| r < 0.0).copied().collect();
    let sortino = if downside.is_empty() {
        None
    } else {
        let down_var = downside.iter().map(|r| r.powi(2)).sum::<f64>() / downside.len() as f64;
        let down_std = down_var.sqrt();
        if down_std > 0.0 {
            Some(mean / down_std)
        } else {
            None
        }
    };

    (sharpe, sortino)
}

/// FIFO round-trip trade pairing.
///
/// Returns one `i64` (pnl_micros) per completed round-trip trade.
/// Long trades: opened by BUY, closed by SELL. pnl = (sell - buy) * qty - fees.
/// Short trades: opened by SELL, closed by BUY. pnl = (buy_short_entry - buy_exit) * qty - fees.
/// Open positions remaining at run end are excluded (no exit price for PnL).
/// Fills must already be in chronological order (engine guarantees this).
fn compute_fifo_trade_pnls(fills: &[mqk_backtest::BacktestFill]) -> Vec<i64> {
    use std::collections::{HashMap, VecDeque};

    // Lot: (open_qty, price_micros, remaining_fee_micros).
    type Lot = (i64, i64, i64);
    let mut long_lots: HashMap<String, VecDeque<Lot>> = HashMap::new();
    let mut short_lots: HashMap<String, VecDeque<Lot>> = HashMap::new();
    let mut pnls: Vec<i64> = Vec::new();

    for fill in fills {
        let sym = fill.symbol.clone();
        let qty = fill.qty;
        let price = fill.price_micros;
        let fee = fill.fee_micros;

        match &fill.inner.side {
            Side::Buy => {
                // Close existing short lots first (FIFO), then open long with remainder.
                let mut rem_qty = qty;
                let mut rem_fee = fee;
                {
                    let shorts = short_lots.entry(sym.clone()).or_default();
                    while rem_qty > 0 && !shorts.is_empty() {
                        let lot = shorts.front_mut().unwrap();
                        let close_qty = rem_qty.min(lot.0);
                        let open_fee_part = if lot.0 > 0 {
                            lot.2 * close_qty / lot.0
                        } else {
                            0
                        };
                        let close_fee_part = if qty > 0 { fee * close_qty / qty } else { 0 };
                        // Short PnL: entry = lot price (sell), exit = current buy price.
                        let pnl = (lot.1 - price)
                            .saturating_mul(close_qty)
                            .saturating_sub(open_fee_part)
                            .saturating_sub(close_fee_part);
                        pnls.push(pnl);
                        lot.0 -= close_qty;
                        lot.2 -= open_fee_part;
                        rem_qty -= close_qty;
                        rem_fee = rem_fee.saturating_sub(close_fee_part);
                        if lot.0 == 0 {
                            shorts.pop_front();
                        }
                    }
                }
                if rem_qty > 0 {
                    long_lots
                        .entry(sym)
                        .or_default()
                        .push_back((rem_qty, price, rem_fee));
                }
            }
            Side::Sell => {
                // Close existing long lots first (FIFO), then open short with remainder.
                let mut rem_qty = qty;
                let mut rem_fee = fee;
                {
                    let longs = long_lots.entry(sym.clone()).or_default();
                    while rem_qty > 0 && !longs.is_empty() {
                        let lot = longs.front_mut().unwrap();
                        let close_qty = rem_qty.min(lot.0);
                        let open_fee_part = if lot.0 > 0 {
                            lot.2 * close_qty / lot.0
                        } else {
                            0
                        };
                        let close_fee_part = if qty > 0 { fee * close_qty / qty } else { 0 };
                        // Long PnL: entry = lot price (buy), exit = current sell price.
                        let pnl = (price - lot.1)
                            .saturating_mul(close_qty)
                            .saturating_sub(open_fee_part)
                            .saturating_sub(close_fee_part);
                        pnls.push(pnl);
                        lot.0 -= close_qty;
                        lot.2 -= open_fee_part;
                        rem_qty -= close_qty;
                        rem_fee = rem_fee.saturating_sub(close_fee_part);
                        if lot.0 == 0 {
                            longs.pop_front();
                        }
                    }
                }
                if rem_qty > 0 {
                    short_lots
                        .entry(sym)
                        .or_default()
                        .push_back((rem_qty, price, rem_fee));
                }
            }
        }
    }

    pnls
}

/// Count equity-curve bars where at least one position was non-zero at bar close.
///
/// Reconstructs net position per symbol by replaying fills in order.
/// Fills must be sorted by fill_ts (engine guarantees this).
fn count_exposure_bars(fills: &[mqk_backtest::BacktestFill], equity_curve: &[(i64, i64)]) -> usize {
    use std::collections::HashMap;

    let mut net_positions: HashMap<String, i64> = HashMap::new();
    let mut fill_idx = 0;
    let mut exposed = 0usize;

    for &(bar_ts, _) in equity_curve {
        // Apply all fills whose fill_ts <= this bar's timestamp.
        while fill_idx < fills.len() && fills[fill_idx].fill_ts <= bar_ts {
            let f = &fills[fill_idx];
            let delta = match &f.inner.side {
                Side::Buy => f.qty,
                Side::Sell => -f.qty,
            };
            *net_positions.entry(f.symbol.clone()).or_insert(0) += delta;
            fill_idx += 1;
        }
        if net_positions.values().any(|&q| q != 0) {
            exposed += 1;
        }
    }

    exposed
}

/// Write deterministic backtest artifacts into an existing run directory.
///
/// This function performs explicit IO. No wall-clock time is used;
/// timestamps are derived from `report.equity_curve` / bar end_ts.
///
/// `initial_cash_micros` is the starting cash value from `BacktestConfig`
/// at the time of the run. It is required for accurate return and drawdown metrics.
///
/// Files written (overwritten):
/// - `orders.csv`
/// - `fills.csv`
/// - `equity_curve.csv`
/// - `metrics.json`
pub fn write_backtest_report(
    run_dir: &Path,
    report: &mqk_backtest::BacktestReport,
    initial_cash_micros: i64,
) -> Result<()> {
    fs::create_dir_all(run_dir).with_context(|| {
        format!(
            "create backtest artifacts dir failed: {}",
            run_dir.display()
        )
    })?;

    // orders.csv — BKT-04P: one row per intent (filled AND rejected).
    let mut orders_csv =
        String::from("ts_utc,order_id,symbol,side,qty,order_type,limit_price,stop_price,status\n");
    for o in &report.orders {
        let side_str = match o.side {
            mqk_backtest::BacktestOrderSide::Buy => "BUY",
            mqk_backtest::BacktestOrderSide::Sell => "SELL",
        };
        let status_str = match o.status {
            mqk_backtest::OrderStatus::Filled => "FILLED",
            mqk_backtest::OrderStatus::Rejected => "REJECTED",
            mqk_backtest::OrderStatus::HaltTriggered => "HALT_TRIGGERED",
            mqk_backtest::OrderStatus::UnfilledEndOfData => "UNFILLED_END_OF_DATA",
            mqk_backtest::OrderStatus::CanceledOnHalt => "CANCELED_ON_HALT",
        };
        // 9 columns: ts_utc,order_id,symbol,side,qty,order_type,limit_price,stop_price,status
        // Backtest orders are always MARKET with no limit or stop price (both empty).
        // ts_utc is signal_ts (decision/order-creation time) -- see fills.csv
        // below for fill_ts (actual execution time), which can be later.
        orders_csv.push_str(&format!(
            "{},{},{},{},{},MARKET,,,{}\n",
            o.signal_ts, o.order_id, o.symbol, side_str, o.qty, status_str,
        ));
    }
    let orders_path = run_dir.join("orders.csv");
    fs::write(&orders_path, orders_csv)
        .with_context(|| format!("write orders.csv failed: {}", orders_path.display()))?;

    // fills.csv — BKT-01P: per-fill provenance with real bar timestamp and deterministic UUIDs.
    let mut fills_csv = String::from("ts_utc,fill_id,order_id,symbol,side,qty,price,fee\n");
    for f in &report.fills {
        let side = format!("{:?}", f.side).to_uppercase(); // BUY / SELL deterministically
        fills_csv.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            f.fill_ts, // bar end timestamp that actually priced this fill (BKT-FUTURE-EXECUTION-01: may be later than the order's signal_ts)
            f.fill_id, // deterministic UUIDv5 per fill
            f.order_id,   // deterministic UUIDv5 per originating order intent
            f.symbol,
            side,
            f.qty,
            f.price_micros,
            f.fee_micros
        ));
    }
    let fills_path = run_dir.join("fills.csv");
    fs::write(&fills_path, fills_csv)
        .with_context(|| format!("write fills.csv failed: {}", fills_path.display()))?;

    // equity_curve.csv
    let mut eq_csv = String::from("ts_utc,equity\n");
    for (ts, eq) in &report.equity_curve {
        eq_csv.push_str(&format!("{},{}\n", ts, eq));
    }
    let eq_path = run_dir.join("equity_curve.csv");
    fs::write(&eq_path, eq_csv)
        .with_context(|| format!("write equity_curve.csv failed: {}", eq_path.display()))?;

    // metrics.json — compute deep performance metrics from report data.
    let final_equity = report
        .equity_curve
        .last()
        .map(|(_, eq)| *eq)
        .unwrap_or(initial_cash_micros);

    // Deterministic symbol listing (sorted).
    let mut symbols: Vec<&str> = report.last_prices.keys().map(|s| s.as_str()).collect();
    symbols.sort();

    let last_prices_micros = report
        .last_prices
        .iter()
        .map(|(k, v)| (k.as_str(), *v))
        .collect::<std::collections::BTreeMap<_, _>>();

    let orders_filled = report
        .orders
        .iter()
        .filter(|o| o.status == mqk_backtest::OrderStatus::Filled)
        .count();
    let orders_rejected = report
        .orders
        .iter()
        .filter(|o| o.status == mqk_backtest::OrderStatus::Rejected)
        .count();

    // Equity performance.
    let (hwm, max_dd_micros, max_dd_pct, total_return_micros, total_return_pct) =
        compute_equity_metrics(initial_cash_micros, &report.equity_curve);

    // Risk-adjusted ratios.
    let (sharpe_ratio, sortino_ratio) =
        compute_sharpe_sortino(initial_cash_micros, &report.equity_curve);

    // Total commissions from fills.
    let total_commission_micros: i64 = report.fills.iter().map(|f| f.fee_micros).sum();

    // FIFO trade-level metrics.
    let trade_pnls = compute_fifo_trade_pnls(&report.fills);
    let trade_count = trade_pnls.len();
    let winning_trade_count = trade_pnls.iter().filter(|&&p| p > 0).count();
    let losing_trade_count = trade_pnls.iter().filter(|&&p| p < 0).count();
    let flat_trade_count = trade_pnls.iter().filter(|&&p| p == 0).count();

    let gross_profit_micros: i64 = trade_pnls.iter().filter(|&&p| p > 0).sum();
    let gross_loss_micros: i64 = trade_pnls.iter().filter(|&&p| p < 0).sum();

    let win_rate_pct = if trade_count > 0 {
        Some(winning_trade_count as f64 / trade_count as f64 * 100.0)
    } else {
        None
    };

    let profit_factor = if gross_loss_micros < 0 {
        Some(gross_profit_micros as f64 / (-gross_loss_micros) as f64)
    } else {
        None // no losing trades — infinite profit factor, report as null
    };

    let average_win_micros = if winning_trade_count > 0 {
        Some(gross_profit_micros / winning_trade_count as i64)
    } else {
        None
    };
    let average_loss_micros = if losing_trade_count > 0 {
        Some(gross_loss_micros / losing_trade_count as i64)
    } else {
        None
    };

    let expectancy_micros = if trade_count > 0 {
        let wr = winning_trade_count as f64 / trade_count as f64;
        let avg_win = average_win_micros.unwrap_or(0) as f64;
        let avg_loss = average_loss_micros.unwrap_or(0) as f64;
        Some((wr * avg_win + (1.0 - wr) * avg_loss) as i64)
    } else {
        None
    };

    let best_trade_micros = trade_pnls.iter().copied().max();
    let worst_trade_micros = trade_pnls.iter().copied().min();

    // Exposure bars from fill replay.
    let exposure_bars = count_exposure_bars(&report.fills, &report.equity_curve);
    let exposure_time_pct = if report.equity_curve.is_empty() {
        0.0
    } else {
        exposure_bars as f64 / report.equity_curve.len() as f64 * 100.0
    };

    let metrics = BacktestMetrics {
        schema_version: 1,
        run_id: report.run_id,
        execution_model_id: report.execution_model_id.as_str(),
        strategy_name: report.strategy_name.as_str(),
        halted: report.halted,
        halt_reason: report.halt_reason.as_deref(),
        execution_blocked: report.execution_blocked,
        bars: report.equity_curve.len(),
        orders: report.orders.len(),
        orders_filled,
        orders_rejected,
        fills: report.fills.len(),
        final_equity_micros: final_equity,
        symbols,
        last_prices_micros,
        starting_equity_micros: initial_cash_micros,
        ending_equity_micros: final_equity,
        total_return_micros,
        total_return_pct,
        equity_high_water_mark_micros: hwm,
        max_drawdown_micros: max_dd_micros,
        max_drawdown_pct: max_dd_pct,
        total_commission_micros,
        trade_count,
        winning_trade_count,
        losing_trade_count,
        flat_trade_count,
        win_rate_pct,
        gross_profit_micros,
        gross_loss_micros,
        profit_factor,
        average_win_micros,
        average_loss_micros,
        expectancy_micros,
        best_trade_micros,
        worst_trade_micros,
        sharpe_ratio,
        sortino_ratio,
        exposure_bars,
        exposure_time_pct,
        benchmark: compute_benchmark(
            report.first_bar_open_micros,
            report.last_bar_close_micros,
            total_return_pct,
        ),
        strategy_sizing: SizingSection {
            target_qty: report.sizing.target_qty,
            max_target_qty: report.sizing.max_target_qty,
            max_position_notional_usd: report.sizing.max_position_notional_usd,
        },
        economics: EconomicsSection {
            contract_multiplier: report.economics.contract_multiplier,
            initial_margin_micros: report.economics.initial_margin_micros,
            maintenance_margin_micros: report.economics.maintenance_margin_micros,
            realized_pnl_micros: report.economics.realized_pnl_micros,
            margin_enforced: report.economics.margin_enforced,
        },
    };

    let metrics_path = run_dir.join("metrics.json");
    let json = serde_json::to_string_pretty(&metrics).context("serialize metrics failed")?;
    fs::write(&metrics_path, format!("{json}\n"))
        .with_context(|| format!("write metrics.json failed: {}", metrics_path.display()))?;

    // report.md — human-readable operator summary.
    let report_md = build_report_md(&metrics, report, initial_cash_micros);
    let report_md_path = run_dir.join("report.md");
    fs::write(&report_md_path, report_md)
        .with_context(|| format!("write report.md failed: {}", report_md_path.display()))?;

    // BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01 / BKT-PROMOTION-ARTIFACT-
    // AUTHORITY-01: correct manifest.json's `economics` and
    // `execution_model_id` fields to this run's real values. Skipped (not
    // fabricated) when no manifest.json exists yet -- some callers in this
    // crate's own tests write artifacts directly into a bare temp dir
    // without first calling `init_run_artifacts`.
    let manifest_path = run_dir.join("manifest.json");
    let manifest_present = manifest_path.exists();
    if manifest_present {
        merge_manifest_completion_metadata(
            &manifest_path,
            &ManifestEconomics::from_backtest_economics(&report.economics),
            &report.execution_model_id,
        )
        .with_context(|| {
            format!(
                "merge manifest completion metadata failed: {}",
                manifest_path.display()
            )
        })?;
    }

    // BKT-PROMOTION-ARTIFACT-AUTHORITY-01: canonical, schema-versioned,
    // lossless BacktestReport authority. Always written -- does not require
    // manifest.json, so ad-hoc test callers that write straight into a bare
    // temp dir still get a loadable canonical artifact.
    let canonical_report_path = backtest_report_artifact::write_canonical_backtest_report(
        run_dir, report,
    )
    .with_context(|| {
        format!(
            "write canonical backtest_report.json failed: {}",
            run_dir.display()
        )
    })?;

    // Completion audit event -- written LAST, only once every other
    // canonical artifact this run needs for promotion evidence (orders,
    // fills, equity curve, metrics, manifest, canonical report) has been
    // durably written. Only emitted when this run_dir was initialized via
    // `init_run_artifacts` (manifest.json + audit.jsonl present); ad-hoc
    // bare-temp-dir test callers do not get a synthesized audit trail.
    if manifest_present {
        write_backtest_completion_audit_event(run_dir, report, &canonical_report_path)
            .with_context(|| {
                format!(
                    "write backtest completion audit event failed: {}",
                    run_dir.display()
                )
            })?;
    }

    Ok(())
}

/// Read-parse-merge-write `manifest.json`'s `economics` and
/// `execution_model_id` keys in place. Additive only -- every other field
/// `init_run_artifacts` (or a later provenance augmentation such as the
/// daemon's md_bars source merge) wrote is left untouched.
fn merge_manifest_completion_metadata(
    manifest_path: &Path,
    economics: &ManifestEconomics,
    execution_model_id: &str,
) -> Result<()> {
    let raw = fs::read_to_string(manifest_path)
        .with_context(|| format!("read manifest failed: {}", manifest_path.display()))?;
    let mut value: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("parse manifest failed: {}", manifest_path.display()))?;
    let obj = value.as_object_mut().with_context(|| {
        format!(
            "manifest.json is not a JSON object: {}",
            manifest_path.display()
        )
    })?;
    obj.insert(
        "economics".to_string(),
        serde_json::to_value(economics).context("serialize manifest economics failed")?,
    );
    obj.insert(
        "execution_model_id".to_string(),
        serde_json::Value::String(execution_model_id.to_string()),
    );
    let pretty = serde_json::to_string_pretty(&value).context("serialize manifest failed")?;
    fs::write(manifest_path, format!("{pretty}\n"))
        .with_context(|| format!("write manifest failed: {}", manifest_path.display()))?;
    Ok(())
}

/// Append the single completion audit event for a finished backtest run.
///
/// Idempotent: if `audit.jsonl` already carries content (e.g. a prior call
/// to `write_backtest_report` for this same `run_dir` already appended the
/// event), this is a no-op rather than a second chain-breaking append.
fn write_backtest_completion_audit_event(
    run_dir: &Path,
    report: &mqk_backtest::BacktestReport,
    canonical_report_path: &Path,
) -> Result<()> {
    let audit_path = run_dir.join("audit.jsonl");
    let existing = fs::read_to_string(&audit_path).unwrap_or_default();
    if !existing.trim().is_empty() {
        return Ok(());
    }

    let canonical_bytes = fs::read(canonical_report_path).with_context(|| {
        format!(
            "read canonical report for audit hash: {}",
            canonical_report_path.display()
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(&canonical_bytes);
    let canonical_report_sha256 = hex::encode(hasher.finalize());

    let mut writer = mqk_audit::AuditWriter::new(&audit_path, true)
        .with_context(|| format!("open audit writer failed: {}", audit_path.display()))?;

    let payload = serde_json::json!({
        "run_id": report.run_id,
        "strategy_name": report.strategy_name,
        "config_id": report.config_id,
        "input_data_hash": report.input_data_hash,
        "execution_model_id": report.execution_model_id,
        "halted": report.halted,
        "halt_reason": report.halt_reason,
        "canonical_report_sha256": canonical_report_sha256,
        "canonical_report_schema_version": BACKTEST_REPORT_ARTIFACT_SCHEMA_VERSION,
    });

    writer
        .append(report.run_id, "backtest", "backtest_run_completed", payload)
        .with_context(|| {
            format!(
                "append completion audit event failed: {}",
                audit_path.display()
            )
        })?;

    Ok(())
}

/// Build a deterministic Markdown operator summary from computed metrics.
fn build_report_md(
    m: &BacktestMetrics<'_>,
    report: &mqk_backtest::BacktestReport,
    initial_cash_micros: i64,
) -> String {
    fn micros_to_dollars(v: i64) -> String {
        format!("${:.2}", v as f64 / 1_000_000.0)
    }
    fn opt_micros(v: Option<i64>) -> String {
        match v {
            Some(x) => micros_to_dollars(x),
            None => "N/A".to_string(),
        }
    }
    fn opt_pct(v: Option<f64>) -> String {
        match v {
            Some(x) => format!("{:.4}%", x),
            None => "N/A".to_string(),
        }
    }
    fn opt_f64(v: Option<f64>) -> String {
        match v {
            Some(x) => format!("{:.4}", x),
            None => "N/A".to_string(),
        }
    }

    let mut out = String::new();
    out.push_str("# Backtest Report\n\n");
    out.push_str("## Identity\n\n");
    out.push_str("| Field | Value |\n|---|---|\n");
    out.push_str(&format!("| Strategy | {} |\n", m.strategy_name));
    out.push_str(&format!("| Run ID | {} |\n", m.run_id));
    out.push_str(&format!("| Config ID | {} |\n", report.config_id));
    out.push_str(&format!(
        "| Input Data Hash | {} |\n",
        report.input_data_hash
    ));
    out.push_str(&format!(
        "| Execution Model | {} |\n",
        m.execution_model_id
    ));
    out.push_str(&format!("| Halted | {} |\n", m.halted));
    if let Some(r) = m.halt_reason {
        out.push_str(&format!("| Halt Reason | {} |\n", r));
    }
    if m.execution_blocked {
        out.push_str("| Execution Blocked | true (integrity disarm/halt) |\n");
    }
    out.push('\n');

    // BACKTEST-CONFIG-DETERMINISM-SIZING-01: sizing section
    out.push_str("## Strategy Sizing\n\n");
    out.push_str("| Parameter | Value |\n|---|---|\n");
    out.push_str(&format!(
        "| Target Qty | {} |\n",
        m.strategy_sizing.target_qty
    ));
    out.push_str(&format!(
        "| Max Target Qty | {} |\n",
        m.strategy_sizing
            .max_target_qty
            .map(|v| v.to_string())
            .unwrap_or_else(|| "none (no cap)".to_string())
    ));
    out.push_str(&format!(
        "| Max Position Notional USD | {} |\n",
        m.strategy_sizing
            .max_position_notional_usd
            .map(|v| format!("${}", v))
            .unwrap_or_else(|| "none (no cap)".to_string())
    ));
    out.push('\n');

    // BACKTEST-REPORT-ECONOMICS-ARTIFACT-01: instrument economics section.
    out.push_str("## Instrument Economics\n\n");
    out.push_str("| Parameter | Value |\n|---|---|\n");
    out.push_str(&format!(
        "| Contract Multiplier | {} |\n",
        m.economics.contract_multiplier
    ));
    out.push_str(&format!(
        "| Initial Margin | {} |\n",
        opt_micros(m.economics.initial_margin_micros)
    ));
    out.push_str(&format!(
        "| Maintenance Margin | {} |\n",
        opt_micros(m.economics.maintenance_margin_micros)
    ));
    out.push_str(&format!(
        "| Economics Realized P&L | {} |\n",
        micros_to_dollars(m.economics.realized_pnl_micros)
    ));
    out.push_str(&format!(
        "| Margin Enforced | {} |\n",
        m.economics.margin_enforced
    ));
    if m.economics.contract_multiplier == 1 {
        out.push_str("\n_Equity economics (multiplier=1): equity curve above is the multiplier-aware curve, identical to the standard portfolio curve._\n");
    } else {
        out.push_str("\n_Multiplier economics active: the Equity Performance curve above is the multiplier-aware economics curve, not the un-multiplied share-count curve._\n");
    }
    out.push('\n');

    out.push_str("## Equity Performance\n\n");
    out.push_str("| Metric | Value |\n|---|---|\n");
    out.push_str(&format!(
        "| Starting Equity | {} |\n",
        micros_to_dollars(initial_cash_micros)
    ));
    out.push_str(&format!(
        "| Ending Equity | {} |\n",
        micros_to_dollars(m.ending_equity_micros)
    ));
    out.push_str(&format!(
        "| Total Return | {} ({:.4}%) |\n",
        micros_to_dollars(m.total_return_micros),
        m.total_return_pct
    ));
    out.push_str(&format!(
        "| High Water Mark | {} |\n",
        micros_to_dollars(m.equity_high_water_mark_micros)
    ));
    out.push_str(&format!(
        "| Max Drawdown | {} ({:.4}%) |\n",
        micros_to_dollars(m.max_drawdown_micros),
        m.max_drawdown_pct
    ));
    out.push_str(&format!(
        "| Total Commission | {} |\n",
        micros_to_dollars(m.total_commission_micros)
    ));
    out.push('\n');

    out.push_str("## Benchmark Comparison\n\n");
    match &m.benchmark {
        Some(b) => {
            out.push_str("| Metric | Value |\n|---|---|\n");
            out.push_str(&format!(
                "| Buy-and-Hold Return | {:.4}% |\n",
                b.buy_and_hold_return_pct
            ));
            out.push_str(&format!(
                "| Strategy Total Return | {:.4}% |\n",
                b.strategy_total_return_pct
            ));
            out.push_str(&format!("| Alpha vs Benchmark | {:.4}% |\n", b.alpha_pct));
            out.push_str(&format!(
                "| First Bar Open | {} |\n",
                micros_to_dollars(b.first_bar_open_micros)
            ));
            out.push_str(&format!(
                "| Last Bar Close | {} |\n",
                micros_to_dollars(b.last_bar_close_micros)
            ));
            out.push_str(&format!("| Assumption | {} |\n", b.assumption));
        }
        None => {
            out.push_str("Benchmark not available (fewer than 2 bars or invalid prices).\n");
        }
    }
    out.push('\n');

    out.push_str("## Trade Statistics\n\n");
    out.push_str("| Metric | Value |\n|---|---|\n");
    out.push_str(&format!("| Bars Processed | {} |\n", m.bars));
    out.push_str(&format!("| Orders Submitted | {} |\n", m.orders));
    out.push_str(&format!("| Orders Filled | {} |\n", m.orders_filled));
    out.push_str(&format!("| Orders Rejected | {} |\n", m.orders_rejected));
    out.push_str(&format!("| Fills | {} |\n", m.fills));
    out.push_str(&format!(
        "| Completed Trades (FIFO) | {} |\n",
        m.trade_count
    ));
    out.push_str(&format!("| Winning Trades | {} |\n", m.winning_trade_count));
    out.push_str(&format!("| Losing Trades | {} |\n", m.losing_trade_count));
    out.push_str(&format!("| Win Rate | {} |\n", opt_pct(m.win_rate_pct)));
    out.push_str(&format!(
        "| Profit Factor | {} |\n",
        opt_f64(m.profit_factor)
    ));
    out.push_str(&format!(
        "| Gross Profit | {} |\n",
        micros_to_dollars(m.gross_profit_micros)
    ));
    out.push_str(&format!(
        "| Gross Loss | {} |\n",
        micros_to_dollars(m.gross_loss_micros)
    ));
    out.push_str(&format!(
        "| Best Trade | {} |\n",
        opt_micros(m.best_trade_micros)
    ));
    out.push_str(&format!(
        "| Worst Trade | {} |\n",
        opt_micros(m.worst_trade_micros)
    ));
    out.push_str(&format!(
        "| Expectancy | {} |\n",
        opt_micros(m.expectancy_micros)
    ));
    out.push('\n');

    out.push_str("## Risk-Adjusted Ratios\n\n");
    out.push_str("| Metric | Value |\n|---|---|\n");
    out.push_str(&format!(
        "| Sharpe Ratio (per-bar) | {} |\n",
        opt_f64(m.sharpe_ratio)
    ));
    out.push_str(&format!(
        "| Sortino Ratio (per-bar) | {} |\n",
        opt_f64(m.sortino_ratio)
    ));
    out.push('\n');

    out.push_str("## Exposure\n\n");
    out.push_str("| Metric | Value |\n|---|---|\n");
    out.push_str(&format!("| Exposure Bars | {} |\n", m.exposure_bars));
    out.push_str(&format!(
        "| Time in Market | {:.4}% |\n",
        m.exposure_time_pct
    ));
    out.push('\n');

    out.push_str("## Assumptions & Disclaimers\n\n");
    out.push_str("- Fill model: conservative worst-case (BUY@HIGH, SELL@LOW).\n");
    out.push_str("- Slippage and commission are applied per fill as configured.\n");
    out.push_str("- Benchmark uses first bar open and last bar close for a single symbol.\n");
    out.push_str("- Ratios are per-bar (non-annualized); interpret with caution.\n");
    out.push_str("- **Backtest results do not guarantee future profitability.**\n");
    out.push_str("- Past performance is not indicative of future results.\n");

    out
}

// ---------------------------------------------------------------------------
// Schema-correctness tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mqk_backtest::{
        derive_input_data_hash, BacktestFill, BacktestOrder, BacktestOrderSide, BacktestReport,
        OrderStatus,
    };
    use mqk_portfolio::{Fill, Side};
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn test_report_with_orders() -> BacktestReport {
        let order_id = Uuid::new_v5(&Uuid::from_bytes([0u8; 16]), b"test_order");
        let fill_id = Uuid::new_v5(&Uuid::from_bytes([1u8; 16]), order_id.as_bytes());
        BacktestReport {
            strategy_name: "test_strategy".to_string(),
            run_id: Uuid::new_v5(&Uuid::from_bytes([2u8; 16]), b"run"),
            config_id: Uuid::new_v5(&Uuid::from_bytes([3u8; 16]), b"cfg"),
            input_data_hash: derive_input_data_hash(&[]),
            equity_curve: vec![(1_000, 1_000_000_000), (2_000, 1_010_000_000)],
            orders: vec![
                BacktestOrder {
                    order_id,
                    signal_ts: 1_000,
                    symbol: "SPY".to_string(),
                    side: BacktestOrderSide::Buy,
                    qty: 10,
                    status: OrderStatus::Filled,
                },
                BacktestOrder {
                    order_id: Uuid::new_v5(&Uuid::from_bytes([0u8; 16]), b"order2"),
                    signal_ts: 2_000,
                    symbol: "SPY".to_string(),
                    side: BacktestOrderSide::Sell,
                    qty: 10,
                    status: OrderStatus::Rejected,
                },
            ],
            fills: vec![BacktestFill {
                fill_id,
                order_id,
                signal_ts: 1_000,
                fill_ts: 1_000,
                inner: Fill::new("SPY", Side::Buy, 10, 150_000_000, 5_000),
            }],
            last_prices: {
                let mut m = BTreeMap::new();
                m.insert("SPY".to_string(), 151_000_000i64);
                m
            },
            first_bar_open_micros: Some(150_000_000),
            last_bar_close_micros: Some(151_000_000),
            ..BacktestReport::test_fixture()
        }
    }

    /// ART-01: orders.csv header column count must match every data row.
    #[test]
    fn orders_csv_header_matches_row_column_count() {
        let tmp = std::env::temp_dir().join(format!("mqk_art_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let report = test_report_with_orders();
        write_backtest_report(&tmp, &report, 1_000_000_000).unwrap();

        let orders_csv = std::fs::read_to_string(tmp.join("orders.csv")).unwrap();
        let lines: Vec<&str> = orders_csv.lines().filter(|l| !l.is_empty()).collect();

        assert!(!lines.is_empty(), "orders.csv must not be empty");
        let header = lines[0];
        let header_cols: usize = header.split(',').count();
        assert_eq!(
            header_cols, 9,
            "header must declare exactly 9 columns, got {}: '{}'",
            header_cols, header
        );

        for (i, row) in lines[1..].iter().enumerate() {
            let row_cols = row.split(',').count();
            assert_eq!(
                row_cols,
                header_cols,
                "data row {} has {} columns but header has {}; row: '{}'",
                i + 1,
                row_cols,
                header_cols,
                row
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// ART-02: fills.csv header column count must match every data row.
    #[test]
    fn fills_csv_header_matches_row_column_count() {
        let tmp = std::env::temp_dir().join(format!("mqk_art_test_fills_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let report = test_report_with_orders();
        write_backtest_report(&tmp, &report, 1_000_000_000).unwrap();

        let fills_csv = std::fs::read_to_string(tmp.join("fills.csv")).unwrap();
        let lines: Vec<&str> = fills_csv.lines().filter(|l| !l.is_empty()).collect();

        assert!(!lines.is_empty(), "fills.csv must not be empty");
        let header = lines[0];
        let header_cols = header.split(',').count();
        assert_eq!(
            header_cols, 8,
            "fills.csv header must declare 8 columns, got {}: '{}'",
            header_cols, header
        );

        for (i, row) in lines[1..].iter().enumerate() {
            let row_cols = row.split(',').count();
            assert_eq!(
                row_cols,
                header_cols,
                "fills row {} has {} columns but header has {}; row: '{}'",
                i + 1,
                row_cols,
                header_cols,
                row
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// ART-03: equity_curve.csv header/row agreement.
    #[test]
    fn equity_csv_header_matches_row_column_count() {
        let tmp = std::env::temp_dir().join(format!("mqk_art_test_eq_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let report = test_report_with_orders();
        write_backtest_report(&tmp, &report, 1_000_000_000).unwrap();

        let eq_csv = std::fs::read_to_string(tmp.join("equity_curve.csv")).unwrap();
        let lines: Vec<&str> = eq_csv.lines().filter(|l| !l.is_empty()).collect();

        assert!(!lines.is_empty());
        let header_cols = lines[0].split(',').count();
        assert_eq!(header_cols, 2, "equity_curve header must be 2 columns");
        for (i, row) in lines[1..].iter().enumerate() {
            let c = row.split(',').count();
            assert_eq!(c, header_cols, "equity_curve row {} mismatch", i + 1);
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// ART-04: metrics.json is valid JSON with schema_version=1 and required fields.
    #[test]
    fn metrics_json_is_valid_and_versioned() {
        let tmp = std::env::temp_dir().join(format!("mqk_art_test_metrics_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let report = test_report_with_orders();
        write_backtest_report(&tmp, &report, 1_000_000_000).unwrap();

        let contents = std::fs::read_to_string(tmp.join("metrics.json")).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&contents).expect("metrics.json must be valid JSON");
        assert_eq!(v["schema_version"], 1, "schema_version must be 1");
        assert!(v["fills"].is_number(), "fills count must be present");
        assert!(v["orders"].is_number(), "orders count must be present");
        assert!(
            v["run_id"].is_string(),
            "run_id must be present in metrics.json"
        );
        assert!(
            v["strategy_name"].is_string(),
            "strategy_name must be present in metrics.json"
        );
        assert_eq!(
            v["strategy_name"], "test_strategy",
            "strategy_name must match report"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// BKT-FUTURE-EXECUTION-01-REPAIR-01: metrics.json and report.md must
    /// truthfully surface the execution model that actually produced the
    /// run -- not a placeholder, and never a value implying same-bar
    /// execution when the run actually used future-target-symbol-bar
    /// execution.
    #[test]
    fn execution_model_id_is_written_truthfully_to_metrics_and_report_md() {
        let tmp = std::env::temp_dir().join(format!(
            "mqk_art_test_exec_model_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        let report = BacktestReport {
            execution_model_id: mqk_backtest::BACKTEST_EXECUTION_MODEL_ID.to_string(),
            ..test_report_with_orders()
        };
        write_backtest_report(&tmp, &report, 1_000_000_000).unwrap();

        let metrics_contents = std::fs::read_to_string(tmp.join("metrics.json")).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&metrics_contents).expect("metrics.json must be valid JSON");
        assert_eq!(
            v["execution_model_id"], mqk_backtest::BACKTEST_EXECUTION_MODEL_ID,
            "metrics.json must truthfully carry the execution model that produced the run"
        );

        let report_md = std::fs::read_to_string(tmp.join("report.md")).unwrap();
        assert!(
            report_md.contains(mqk_backtest::BACKTEST_EXECUTION_MODEL_ID),
            "report.md must surface the execution model in its Identity section"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// ART-05: orders.csv status column carries correct values (not in stop_price slot).
    #[test]
    fn orders_csv_status_column_is_correct() {
        let tmp = std::env::temp_dir().join(format!("mqk_art_test_status_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let report = test_report_with_orders();
        write_backtest_report(&tmp, &report, 1_000_000_000).unwrap();

        let csv = std::fs::read_to_string(tmp.join("orders.csv")).unwrap();
        let mut lines = csv.lines().filter(|l| !l.is_empty());
        let header: Vec<&str> = lines.next().unwrap().split(',').collect();

        let status_idx = header
            .iter()
            .position(|&h| h == "status")
            .expect("status column must exist in header");
        let stop_price_idx = header
            .iter()
            .position(|&h| h == "stop_price")
            .expect("stop_price column must exist in header");

        for row_str in lines {
            let cols: Vec<&str> = row_str.split(',').collect();
            let status_val = cols[status_idx];
            let stop_price_val = cols[stop_price_idx];

            assert!(
                matches!(
                    status_val,
                    "FILLED" | "REJECTED" | "HALT_TRIGGERED" | "UNFILLED_END_OF_DATA"
                ),
                "status column must be a valid status string, got '{}'",
                status_val
            );
            assert_eq!(
                stop_price_val, "",
                "stop_price must be empty for MARKET backtest orders, got '{}'",
                stop_price_val
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // BACKTEST-METRICS-01: focused deep-metric computation tests
    // -----------------------------------------------------------------------

    fn make_report_no_fills(equity_curve: Vec<(i64, i64)>) -> BacktestReport {
        BacktestReport {
            strategy_name: "test".to_string(),
            run_id: Uuid::new_v5(&Uuid::from_bytes([0u8; 16]), b"r"),
            config_id: Uuid::new_v5(&Uuid::from_bytes([0u8; 16]), b"c"),
            input_data_hash: derive_input_data_hash(&[]),
            equity_curve,
            ..BacktestReport::test_fixture()
        }
    }

    fn make_fill(
        bar_ts: i64,
        symbol: &str,
        side: Side,
        qty: i64,
        price_micros: i64,
        fee_micros: i64,
    ) -> BacktestFill {
        let ns = Uuid::from_bytes([0xBBu8; 16]);
        let key = format!("{}:{}:{}:{}", bar_ts, symbol, qty, price_micros);
        let order_id = Uuid::new_v5(&ns, key.as_bytes());
        let fill_id = Uuid::new_v5(&ns, order_id.as_bytes());
        BacktestFill {
            fill_id,
            order_id,
            signal_ts: bar_ts,
            fill_ts: bar_ts,
            inner: Fill::new(symbol, side, qty, price_micros, fee_micros),
        }
    }

    /// BM-01: flat/no-trade run — equity unchanged, all trade metrics are zero/null.
    #[test]
    fn bm01_flat_no_trade_run() {
        let starting = 100_000_000_000i64; // $100k
        let report = make_report_no_fills(vec![(1, starting)]);

        let (hwm, max_dd, max_dd_pct, ret_micros, ret_pct) =
            compute_equity_metrics(starting, &report.equity_curve);

        assert_eq!(hwm, starting);
        assert_eq!(max_dd, 0);
        assert_eq!(max_dd_pct, 0.0);
        assert_eq!(ret_micros, 0);
        assert_eq!(ret_pct, 0.0);

        let pnls = compute_fifo_trade_pnls(&report.fills);
        assert!(pnls.is_empty(), "no fills → no closed trades");

        let exposure = count_exposure_bars(&report.fills, &report.equity_curve);
        assert_eq!(exposure, 0, "no fills → no exposure bars");

        let (sharpe, sortino) = compute_sharpe_sortino(starting, &report.equity_curve);
        assert!(sharpe.is_none(), "only 1 return observation → sharpe None");
        assert!(sortino.is_none());
    }

    /// BM-02: equity curve with a clear drawdown — HWM, max_drawdown_micros, max_drawdown_pct.
    #[test]
    fn bm02_equity_curve_drawdown() {
        let starting = 100_000_000i64;
        // Equity goes 100 → 110 → 95 → 105 (peak=110, trough=95, dd=15).
        let curve = vec![
            (1, 100_000_000),
            (2, 110_000_000),
            (3, 95_000_000),
            (4, 105_000_000),
        ];

        let (hwm, max_dd, max_dd_pct, ret_micros, ret_pct) =
            compute_equity_metrics(starting, &curve);

        assert_eq!(hwm, 110_000_000, "HWM must be the peak equity");
        assert_eq!(max_dd, 15_000_000, "drawdown = peak(110) - trough(95) = 15");
        // 15/110 * 100 ≈ 13.636%
        assert!(
            (max_dd_pct - 13.636).abs() < 0.001,
            "max_dd_pct ≈ 13.636%, got {}",
            max_dd_pct
        );
        assert_eq!(ret_micros, 5_000_000, "final=105 - starting=100 = 5");
        assert_eq!(ret_pct, 5.0, "5/100 * 100 = 5.0%");
    }

    /// BM-03: total commission is the sum of all fill fees.
    #[test]
    fn bm03_commission_aggregation() {
        let fills = [
            make_fill(1, "SPY", Side::Buy, 10, 100_000_000, 1_000_000),
            make_fill(2, "SPY", Side::Buy, 5, 100_000_000, 500_000),
        ];
        let total_fee: i64 = fills.iter().map(|f| f.fee_micros).sum();
        assert_eq!(total_fee, 1_500_000);
    }

    /// BM-04: FIFO pairing — simple winning long trade (BUY then SELL).
    #[test]
    fn bm04_winning_long_trade() {
        // BUY 100 @ $10 ($10 each, price=10_000_000 micros), fee=1_000 micros.
        // SELL 100 @ $11 ($11 each, price=11_000_000 micros), fee=1_000 micros.
        // PnL = (11_000_000 - 10_000_000) * 100 - 1_000 - 1_000 = 100_000_000 - 2_000 = 99_998_000.
        let fills = vec![
            make_fill(1, "SPY", Side::Buy, 100, 10_000_000, 1_000),
            make_fill(2, "SPY", Side::Sell, 100, 11_000_000, 1_000),
        ];

        let pnls = compute_fifo_trade_pnls(&fills);
        assert_eq!(pnls.len(), 1, "one completed round-trip");
        assert_eq!(pnls[0], 99_998_000, "pnl = 100M - 2k fees = 99_998_000");

        let winning = pnls.iter().filter(|&&p| p > 0).count();
        let losing = pnls.iter().filter(|&&p| p < 0).count();
        assert_eq!(winning, 1);
        assert_eq!(losing, 0);

        let gross_profit: i64 = pnls.iter().filter(|&&p| p > 0).sum();
        let gross_loss: i64 = pnls.iter().filter(|&&p| p < 0).sum();
        assert_eq!(gross_profit, 99_998_000);
        assert_eq!(gross_loss, 0);

        // profit_factor: no losses → None
        assert!(
            gross_loss == 0,
            "no losing trades → profit_factor should be None"
        );

        let win_rate = winning as f64 / pnls.len() as f64 * 100.0;
        assert_eq!(win_rate, 100.0);
    }

    /// BM-05: FIFO pairing — simple losing long trade (BUY then SELL at lower price).
    #[test]
    fn bm05_losing_long_trade() {
        // BUY 100 @ $11, fee=1_000. SELL 100 @ $10, fee=1_000.
        // PnL = (10M - 11M) * 100 - 1_000 - 1_000 = -100M - 2_000 = -100_002_000.
        let fills = vec![
            make_fill(1, "SPY", Side::Buy, 100, 11_000_000, 1_000),
            make_fill(2, "SPY", Side::Sell, 100, 10_000_000, 1_000),
        ];

        let pnls = compute_fifo_trade_pnls(&fills);
        assert_eq!(pnls.len(), 1, "one completed round-trip");
        assert_eq!(pnls[0], -100_002_000, "loss = -100M - 2k fees");

        let winning = pnls.iter().filter(|&&p| p > 0).count();
        let losing = pnls.iter().filter(|&&p| p < 0).count();
        assert_eq!(winning, 0);
        assert_eq!(losing, 1);

        let gross_profit: i64 = pnls.iter().filter(|&&p| p > 0).sum();
        let gross_loss: i64 = pnls.iter().filter(|&&p| p < 0).sum();
        assert_eq!(gross_profit, 0);
        assert_eq!(gross_loss, -100_002_000);

        // profit_factor = gross_profit / abs(gross_loss) = 0 / 100_002_000 = 0.0
        let pf = gross_profit as f64 / (-gross_loss) as f64;
        assert_eq!(pf, 0.0);

        let win_rate = winning as f64 / pnls.len() as f64 * 100.0;
        assert_eq!(win_rate, 0.0);

        let avg_loss = gross_loss / losing as i64;
        assert_eq!(avg_loss, -100_002_000);

        // Expectancy = 0 * 0 + 1 * avg_loss = avg_loss.
        let wr = winning as f64 / pnls.len() as f64;
        let expectancy = (wr * gross_profit as f64 + (1.0 - wr) * avg_loss as f64) as i64;
        assert_eq!(expectancy, -100_002_000);
    }

    /// BM-06: Sharpe/Sortino require at least 2 return observations.
    #[test]
    fn bm06_sharpe_sortino_minimum_observations() {
        let starting = 100_000_000i64;

        // Single-bar equity curve → only 1 return observation → both None.
        let (s1, so1) = compute_sharpe_sortino(starting, &[(1, 100_000_000)]);
        assert!(s1.is_none(), "1 return obs → sharpe None");
        assert!(so1.is_none());

        // Two bars: r0=0.0, r1=0.1 → variance is non-zero, sharpe computable; no downside.
        let (s2, so2) = compute_sharpe_sortino(starting, &[(1, 100_000_000), (2, 110_000_000)]);
        // r0 = 0.0, r1 = 0.1 → mean=0.05, std≈0.0707, sharpe≈0.707
        assert!(s2.is_some(), "non-zero variance → sharpe computable");
        assert!(so2.is_none(), "no downside returns → sortino None");

        // Positive then negative return → both computable.
        let (s3, so3) = compute_sharpe_sortino(starting, &[(1, 110_000_000), (2, 95_000_000)]);
        // r0 = 0.1, r1 = -15/110 ≈ -0.1364
        assert!(s3.is_some(), "mixed returns → sharpe computable");
        assert!(
            so3.is_some(),
            "negative return present → sortino computable"
        );
    }

    /// BM-07: exposure_bars counts bars where net position is non-zero.
    #[test]
    fn bm07_exposure_bars() {
        // Bar 1: BUY 10 shares → position = +10 (exposed).
        // Bar 2: no fill → position still +10 (exposed).
        // Bar 3: SELL 10 shares → position = 0 (flat at bar close).
        let fills = vec![
            make_fill(1, "SPY", Side::Buy, 10, 100_000_000, 0),
            make_fill(3, "SPY", Side::Sell, 10, 105_000_000, 0),
        ];
        let curve = vec![(1, 100_000_000), (2, 101_000_000), (3, 105_000_000)];
        let exposed = count_exposure_bars(&fills, &curve);
        assert_eq!(
            exposed, 2,
            "bars 1 and 2 are exposed; bar 3 position closes to 0"
        );
    }

    /// ART-06: metrics.json includes all deep performance fields from BACKTEST-METRICS-01.
    #[test]
    fn art06_metrics_json_includes_deep_performance_fields() {
        let tmp = std::env::temp_dir().join(format!("mqk_art_test_deep_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let report = test_report_with_orders();
        write_backtest_report(&tmp, &report, 1_000_000_000).unwrap();

        let contents = std::fs::read_to_string(tmp.join("metrics.json")).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&contents).expect("metrics.json must be valid JSON");

        // Equity performance fields.
        assert!(v["starting_equity_micros"].is_number());
        assert!(v["ending_equity_micros"].is_number());
        assert!(v["total_return_micros"].is_number());
        assert!(v["total_return_pct"].is_number());
        assert!(v["equity_high_water_mark_micros"].is_number());
        assert!(v["max_drawdown_micros"].is_number());
        assert!(v["max_drawdown_pct"].is_number());

        // Cost field.
        assert!(v["total_commission_micros"].is_number());

        // Trade-level fields (trade_count is a number; win_rate_pct may be null if no trades).
        assert!(v["trade_count"].is_number());
        assert!(v["winning_trade_count"].is_number());
        assert!(v["losing_trade_count"].is_number());
        assert!(v["flat_trade_count"].is_number());
        assert!(v["gross_profit_micros"].is_number());
        assert!(v["gross_loss_micros"].is_number());
        assert!(v["exposure_bars"].is_number());
        assert!(v["exposure_time_pct"].is_number());

        // Backward-compat field still present.
        assert!(v["final_equity_micros"].is_number());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01: manifest.json economics tests
    // -----------------------------------------------------------------------

    /// Run `init_run_artifacts` then `write_backtest_report` for `report` into
    /// a fresh temp dir, then read back and parse the resulting manifest.json.
    fn init_and_write_manifest(tag: &str, report: &BacktestReport) -> (PathBuf, RunManifest) {
        let tmp =
            std::env::temp_dir().join(format!("mqk_art_test_mre_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);

        let init = init_run_artifacts(InitRunArtifactsArgs {
            exports_root: &tmp,
            schema_version: 1,
            run_id: report.run_id,
            strategy_name: &report.strategy_name,
            engine_id: "mqk-backtest",
            mode: "backtest",
            timeframe: None,
            timeframe_secs: Some(60),
            git_hash: "test",
            config_hash: &report.config_id.to_string(),
            host_fingerprint: "test-host",
            now_utc: Utc::now(), // allow: test-only manifest timestamp
        })
        .expect("init_run_artifacts must succeed");

        write_backtest_report(&init.run_dir, report, 1_000_000_000)
            .expect("write_backtest_report must succeed");

        let manifest_json =
            std::fs::read_to_string(&init.manifest_path).expect("manifest.json must be readable");
        let manifest: RunManifest =
            serde_json::from_str(&manifest_json).expect("manifest.json must parse as RunManifest");

        (init.run_dir, manifest)
    }

    /// MRE-01: a default (equity) report writes truthful default_equity
    /// economics into manifest.json.
    #[test]
    fn mre01_manifest_default_equity_economics_is_truthful() {
        let report = test_report_with_orders();
        let (run_dir, manifest) = init_and_write_manifest("mre01", &report);

        assert_eq!(manifest.economics.contract_multiplier, 1);
        assert_eq!(manifest.economics.initial_margin_micros, None);
        assert_eq!(manifest.economics.maintenance_margin_micros, None);
        assert!(!manifest.economics.margin_enforced);
        assert_eq!(manifest.economics.source, "default_equity");

        let _ = std::fs::remove_dir_all(run_dir.parent().unwrap_or(&run_dir));
    }

    /// MRE-02: an explicit multiplier=50 report writes truthful explicit
    /// economics into manifest.json (source="explicit_request").
    #[test]
    fn mre02_manifest_explicit_multiplier_50_economics_is_truthful() {
        let mut report = test_report_with_orders();
        report.run_id = Uuid::new_v5(&Uuid::from_bytes([4u8; 16]), b"mre02");
        report.economics = mqk_backtest::BacktestEconomicsReport {
            contract_multiplier: 50,
            initial_margin_micros: None,
            maintenance_margin_micros: None,
            realized_pnl_micros: 12_345,
            margin_enforced: false,
        };
        let (run_dir, manifest) = init_and_write_manifest("mre02", &report);

        assert_eq!(manifest.economics.contract_multiplier, 50);
        assert!(!manifest.economics.margin_enforced);
        assert_eq!(manifest.economics.source, "explicit_request");

        let _ = std::fs::remove_dir_all(run_dir.parent().unwrap_or(&run_dir));
    }

    /// MRE-03: margin metadata reaches manifest.json truthfully, and
    /// margin_enforced stays false -- the margin scaffold is never enforced.
    #[test]
    fn mre03_manifest_margin_metadata_is_truthful_with_margin_not_enforced() {
        let mut report = test_report_with_orders();
        report.run_id = Uuid::new_v5(&Uuid::from_bytes([5u8; 16]), b"mre03");
        report.economics = mqk_backtest::BacktestEconomicsReport {
            contract_multiplier: 50,
            initial_margin_micros: Some(10_000_000_000),
            maintenance_margin_micros: Some(5_000_000_000),
            realized_pnl_micros: 0,
            margin_enforced: false,
        };
        let (run_dir, manifest) = init_and_write_manifest("mre03", &report);

        assert_eq!(manifest.economics.contract_multiplier, 50);
        assert_eq!(
            manifest.economics.initial_margin_micros,
            Some(10_000_000_000)
        );
        assert_eq!(
            manifest.economics.maintenance_margin_micros,
            Some(5_000_000_000)
        );
        assert!(
            !manifest.economics.margin_enforced,
            "margin metadata must never claim enforcement"
        );
        assert_eq!(manifest.economics.source, "explicit_request");

        let _ = std::fs::remove_dir_all(run_dir.parent().unwrap_or(&run_dir));
    }

    /// MRE-04: a minimal manifest.json written before this field existed (no
    /// `economics` key at all) still parses, defaulting to default_equity --
    /// never a parse failure, never a fabricated non-default value.
    #[test]
    fn mre04_old_manifest_without_economics_still_parses_with_default() {
        let old_manifest_json = serde_json::json!({
            "schema_version": 1,
            "run_id": "00000000-0000-0000-0000-000000000000",
            "strategy_name": "legacy",
            "engine_id": "mqk-backtest",
            "mode": "backtest",
            "git_hash": "deadbeef",
            "config_hash": "cafef00d",
            "host_fingerprint": "old-host",
            "created_at_utc": "2026-01-01T00:00:00Z",
            "artifacts": {
                "audit_jsonl": "audit.jsonl",
                "manifest_json": "manifest.json",
                "orders_csv": "orders.csv",
                "fills_csv": "fills.csv",
                "equity_curve_csv": "equity_curve.csv",
                "metrics_json": "metrics.json"
            }
        })
        .to_string();

        let manifest: RunManifest = serde_json::from_str(&old_manifest_json)
            .expect("manifest.json without economics must still parse");

        assert_eq!(manifest.economics, ManifestEconomics::default_equity());
    }
}
