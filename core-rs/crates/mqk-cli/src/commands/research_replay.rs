//! W06-P9-CANONICAL-RESEARCH-REPLAY-CLI-01
//!
//! Produces a REAL canonical Backtest/stress/P9 artifact tree for a
//! registered Wave06 Research trial, using
//! `mqk_backtest::ResearchOosReplayStrategy` (W06-P9-RUST-REPLAY-STRATEGY-01)
//! as the actual Backtest candidate -- through the EXISTING production
//! artifact pipeline (`mqk_artifacts::init_run_artifacts` /
//! `write_backtest_report` / `write_canonical_stress_suite` /
//! `write_canonical_robustness_gauntlet`) and the EXISTING deferred-scenario
//! finalizers already wired for every other backtest candidate
//! (`run_finalize_robustness_sensitivity`, `run_finalize_p7a_p7b_replay_stress`,
//! `run_finalize_genuine_shuffled_placebo`, all in `super::bkt`) -- no second
//! artifact format, no second P9 verifier, no Paper/Live/OMS/broker call.
//!
//! Consumes W06-P9-REPLAY-AUTHORITY-01's `manifest.json` bundle: this module
//! owns ALL file I/O and content-hash re-verification of that bundle (Patch
//! A/B deliberately do none) -- a wrong/mutated/missing bundle file, a wrong
//! trial/strategy/economic_eval_id, or a replay schedule that does not match
//! the resolved bars all fail closed here, before any engine run.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::DateTime;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEngine, ReplaySemanticSpec, ResearchOosReplayStrategy,
};
use mqk_execution::TargetPosition;
use mqk_strategy::Strategy;

use super::bkt::{
    run_finalize_genuine_shuffled_placebo, run_finalize_p7a_p7b_replay_stress,
    run_finalize_robustness_sensitivity,
};

// ---------------------------------------------------------------------------
// manifest.json DTOs (mirrors mqk_research.ml.oos_replay_bundle's schema)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct FileHashRecord {
    #[serde(default)]
    path: Option<String>,
    sha256: Option<String>,
    #[allow(dead_code)]
    bytes: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct ScheduleFileRecord {
    file: String,
    sha256: String,
    #[allow(dead_code)]
    bytes: i64,
    #[allow(dead_code)]
    row_count: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestLineage {
    trial_id: String,
    #[allow(dead_code)]
    experiment_id: String,
    #[allow(dead_code)]
    hypothesis_id: String,
    strategy_id: String,
    #[allow(dead_code)]
    attempt_id: String,
    economic_eval_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ReplayWeightToShareDto {
    equity_usd: f64,
    max_target_qty: Option<i64>,
    max_position_notional_usd: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReplaySemanticSpecDto {
    replay_protocol_version: String,
    strategy_id: String,
    feature_columns: Vec<String>,
    feature_transform: String,
    direction_policy: String,
    rank_side_count: i64,
    long_only: bool,
    borrow_model: Option<String>,
    max_gross_exposure: f64,
    timeframe: String,
    weight_to_share: ReplayWeightToShareDto,
}

impl ReplaySemanticSpecDto {
    /// `trial_id` is deliberately NOT a field of `ReplaySemanticSpecDto`
    /// (mirrors Python's `replay_semantic_spec` JSON section, which
    /// structurally excludes it -- see oos_replay_bundle.py's TEST 12) --
    /// the caller must supply it from `manifest.lineage.trial_id`
    /// (R1.4/R2.2).
    fn into_semantic(self, trial_id: String) -> ReplaySemanticSpec {
        ReplaySemanticSpec {
            replay_protocol_version: self.replay_protocol_version,
            strategy_id: self.strategy_id,
            feature_columns: self.feature_columns,
            feature_transform: self.feature_transform,
            direction_policy: self.direction_policy,
            rank_side_count: self.rank_side_count,
            long_only: self.long_only,
            borrow_model: self.borrow_model,
            max_gross_exposure: self.max_gross_exposure,
            timeframe: self.timeframe,
            equity_usd: self.weight_to_share.equity_usd,
            max_target_qty: self.weight_to_share.max_target_qty,
            max_position_notional_usd: self.weight_to_share.max_position_notional_usd,
            trial_id,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ReplayManifestDto {
    schema_version: String,
    protocol_version: String,
    lineage: ManifestLineage,
    replay_semantic_spec: ReplaySemanticSpecDto,
    #[allow(dead_code)]
    holdout_start_utc: String,
    source_file_hashes: std::collections::BTreeMap<String, FileHashRecord>,
    baseline_schedule: ScheduleFileRecord,
    symbol_loo_schedules: std::collections::BTreeMap<String, ScheduleFileRecord>,
}

const REQUIRED_MANIFEST_PROTOCOL_VERSION: &str = "research_oos_replay_bundle_v1";

fn sha256_hex_of_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read failed: {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}

fn require_hash_match(label: &str, path: &Path, expected_sha256: &str) -> Result<()> {
    let actual = sha256_hex_of_file(path)?;
    if actual != expected_sha256 {
        bail!(
            "Fail-closed: {label} at {} no longer matches its manifest-recorded sha256 \
             (expected {expected_sha256}, got {actual}) -- refusing a mutated replay bundle",
            path.display()
        );
    }
    Ok(())
}

/// Resolved, re-verified replay bundle -- every file's content has already
/// been checked against `manifest.json`'s own recorded hashes.
#[derive(Debug)]
pub struct ResolvedReplayBundle {
    #[allow(dead_code)]
    pub bundle_dir: PathBuf,
    pub trial_id: String,
    pub strategy_id: String,
    pub economic_eval_id: String,
    pub semantic: ReplaySemanticSpec,
    pub bars_csv_path: PathBuf,
    pub baseline_schedule: BTreeMap<i64, Vec<TargetPosition>>,
    pub loo_schedules: BTreeMap<String, BTreeMap<i64, Vec<TargetPosition>>>,
}

/// Load `manifest.json` from `bundle_dir`, re-verify every recorded source
/// file's content hash (features/targets/schema/bars), cross-check the
/// caller's expected `trial_id`/`strategy_id`/`economic_eval_id` against the
/// manifest's own lineage, and load + hash-verify the baseline and every
/// leave-one-out schedule CSV. Fails closed on any mismatch (mission C4/C6).
pub fn resolve_replay_bundle(
    bundle_dir: &Path,
    expected_trial_id: &str,
    expected_strategy_id: &str,
    expected_economic_eval_id: &str,
) -> Result<ResolvedReplayBundle> {
    let manifest_path = bundle_dir.join("manifest.json");
    let manifest_text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("missing replay bundle manifest: {}", manifest_path.display()))?;
    let manifest: ReplayManifestDto = serde_json::from_str(&manifest_text)
        .with_context(|| format!("malformed replay bundle manifest: {}", manifest_path.display()))?;

    if manifest.schema_version != REQUIRED_MANIFEST_PROTOCOL_VERSION
        || manifest.protocol_version != REQUIRED_MANIFEST_PROTOCOL_VERSION
    {
        bail!(
            "Fail-closed: unsupported replay bundle protocol_version {:?} (expected {:?})",
            manifest.protocol_version,
            REQUIRED_MANIFEST_PROTOCOL_VERSION
        );
    }
    if manifest.lineage.trial_id != expected_trial_id {
        bail!(
            "Fail-closed: replay bundle trial_id {:?} != expected {:?}",
            manifest.lineage.trial_id,
            expected_trial_id
        );
    }
    if manifest.lineage.strategy_id != expected_strategy_id {
        bail!(
            "Fail-closed: replay bundle strategy_id {:?} != expected {:?}",
            manifest.lineage.strategy_id,
            expected_strategy_id
        );
    }
    if manifest.lineage.economic_eval_id != expected_economic_eval_id {
        bail!(
            "Fail-closed: replay bundle economic_eval_id {:?} != expected {:?}",
            manifest.lineage.economic_eval_id,
            expected_economic_eval_id
        );
    }
    if manifest.replay_semantic_spec.strategy_id != expected_strategy_id {
        bail!(
            "Fail-closed: replay bundle replay_semantic_spec.strategy_id {:?} != expected {:?}",
            manifest.replay_semantic_spec.strategy_id,
            expected_strategy_id
        );
    }

    // Re-verify EVERY recorded source input's content hash (features,
    // targets, feature_schema, bars) before trusting anything downstream.
    let mut bars_csv_path: Option<PathBuf> = None;
    for (label, record) in &manifest.source_file_hashes {
        let path = record
            .path
            .as_ref()
            .with_context(|| format!("replay bundle source_file_hashes.{label} missing a path"))?;
        let sha256 = record
            .sha256
            .as_ref()
            .with_context(|| format!("replay bundle source_file_hashes.{label} missing a sha256"))?;
        require_hash_match(label, Path::new(path), sha256)?;
        if label == "bars_csv" {
            bars_csv_path = Some(PathBuf::from(path));
        }
    }
    let bars_csv_path = bars_csv_path
        .context("Fail-closed: replay bundle manifest has no source_file_hashes.bars_csv entry")?;

    let baseline_path = bundle_dir.join(&manifest.baseline_schedule.file);
    require_hash_match(
        "baseline_schedule",
        &baseline_path,
        &manifest.baseline_schedule.sha256,
    )?;
    let baseline_schedule = load_schedule_csv(&baseline_path)?;

    let mut loo_schedules = BTreeMap::new();
    for (symbol, record) in &manifest.symbol_loo_schedules {
        let path = bundle_dir.join(&record.file);
        require_hash_match(&format!("loo_schedule[{symbol}]"), &path, &record.sha256)?;
        loo_schedules.insert(symbol.clone(), load_schedule_csv(&path)?);
    }

    let trial_id = manifest.lineage.trial_id;
    Ok(ResolvedReplayBundle {
        bundle_dir: bundle_dir.to_path_buf(),
        trial_id: trial_id.clone(),
        strategy_id: manifest.lineage.strategy_id,
        economic_eval_id: manifest.lineage.economic_eval_id,
        semantic: manifest.replay_semantic_spec.into_semantic(trial_id),
        bars_csv_path,
        baseline_schedule,
        loo_schedules,
    })
}

/// Parses `decision_ts,symbol,target_qty` (mission A6 schedule CSV shape,
/// written by `mqk_research.ml.oos_replay_bundle._write_bundle`). Manual,
/// dependency-free parser: this format is fully controlled by Patch A and
/// never contains embedded commas/quoting.
fn load_schedule_csv(path: &Path) -> Result<BTreeMap<i64, Vec<TargetPosition>>> {
    let text = fs::read_to_string(path).with_context(|| format!("read failed: {}", path.display()))?;
    let mut lines = text.lines();
    let header = lines.next().context("empty schedule csv")?;
    let cols: Vec<&str> = header.split(',').collect();
    let ts_idx = cols
        .iter()
        .position(|c| *c == "decision_ts")
        .context("schedule csv missing decision_ts column")?;
    let sym_idx = cols
        .iter()
        .position(|c| *c == "symbol")
        .context("schedule csv missing symbol column")?;
    let qty_idx = cols
        .iter()
        .position(|c| *c == "target_qty")
        .context("schedule csv missing target_qty column")?;

    let mut schedule: BTreeMap<i64, Vec<TargetPosition>> = BTreeMap::new();
    for (line_no, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        let ts_str = fields
            .get(ts_idx)
            .with_context(|| format!("schedule csv line {}: missing decision_ts", line_no + 2))?;
        let symbol = fields
            .get(sym_idx)
            .with_context(|| format!("schedule csv line {}: missing symbol", line_no + 2))?
            .to_string();
        let qty: i64 = fields
            .get(qty_idx)
            .with_context(|| format!("schedule csv line {}: missing target_qty", line_no + 2))?
            .parse()
            .with_context(|| format!("schedule csv line {}: invalid target_qty", line_no + 2))?;
        let end_ts = DateTime::parse_from_rfc3339(ts_str)
            .with_context(|| format!("schedule csv line {}: invalid decision_ts {ts_str:?}", line_no + 2))?
            .timestamp();
        schedule.entry(end_ts).or_default().push(TargetPosition::new(symbol, qty));
    }
    Ok(schedule)
}

/// Loads a Research-produced bars CSV (`symbol,end_ts,[open,high,low,]close[,volume]`,
/// ISO8601 `end_ts`, float prices) into `BacktestBar`s by converting it to
/// the format `mqk_backtest::parse_csv_bars` already accepts
/// (`end_ts` epoch seconds, `*_micros` integer prices) -- reusing that
/// EXISTING parser for sorting/day_id/reject_window_id derivation rather
/// than reimplementing it. Missing open/high/low fall back to close (the
/// same documented close-only convention `mqk_strategy::BarStub::new`
/// already uses); missing volume falls back to 0.
pub fn load_research_bars_csv(path: &Path) -> Result<Vec<BacktestBar>> {
    let text = fs::read_to_string(path).with_context(|| format!("read failed: {}", path.display()))?;
    let mut lines = text.lines();
    let header = lines.next().context("empty bars csv")?;
    let cols: Vec<&str> = header.split(',').collect();
    let find = |name: &str| cols.iter().position(|c| *c == name);
    let sym_idx = find("symbol").context("bars csv missing symbol column")?;
    let ts_idx = find("end_ts").context("bars csv missing end_ts column")?;
    let close_idx = find("close").context("bars csv missing close column")?;
    let open_idx = find("open");
    let high_idx = find("high");
    let low_idx = find("low");
    let vol_idx = find("volume");

    let mut converted = String::from("symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume\n");
    for (line_no, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        let symbol = fields
            .get(sym_idx)
            .with_context(|| format!("bars csv line {}: missing symbol", line_no + 2))?;
        let ts_str = fields
            .get(ts_idx)
            .with_context(|| format!("bars csv line {}: missing end_ts", line_no + 2))?;
        let end_ts = DateTime::parse_from_rfc3339(ts_str)
            .with_context(|| format!("bars csv line {}: invalid end_ts {ts_str:?}", line_no + 2))?
            .timestamp();
        let close: f64 = fields
            .get(close_idx)
            .context("missing close")?
            .parse()
            .with_context(|| format!("bars csv line {}: invalid close", line_no + 2))?;
        let open = open_idx.and_then(|i| fields.get(i)).and_then(|s| s.parse::<f64>().ok()).unwrap_or(close);
        let high = high_idx.and_then(|i| fields.get(i)).and_then(|s| s.parse::<f64>().ok()).unwrap_or(close);
        let low = low_idx.and_then(|i| fields.get(i)).and_then(|s| s.parse::<f64>().ok()).unwrap_or(close);
        let volume: i64 = vol_idx
            .and_then(|i| fields.get(i))
            .and_then(|s| s.parse::<f64>().ok())
            .map(|v| v as i64)
            .unwrap_or(0);

        let open_micros = mqk_execution::price_to_micros(open).context("open price out of range")?;
        let high_micros = mqk_execution::price_to_micros(high).context("high price out of range")?;
        let low_micros = mqk_execution::price_to_micros(low).context("low price out of range")?;
        let close_micros = mqk_execution::price_to_micros(close).context("close price out of range")?;
        converted.push_str(&format!(
            "{symbol},{end_ts},{open_micros},{high_micros},{low_micros},{close_micros},{volume}\n"
        ));
    }
    mqk_backtest::parse_csv_bars(&converted).map_err(|e| anyhow::anyhow!("parse_csv_bars failed: {e}"))
}

// ---------------------------------------------------------------------------
// Top-level orchestration
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub struct ResearchReplayArgs {
    pub registry_db: String,
    pub trial_id: String,
    pub strategy_id: String,
    pub economic_eval_id: String,
    pub replay_bundle_dir: String,
    pub out_dir: String,
    pub research_py_root: String,
    pub python: String,
    pub judge_artifact_sha256: String,
    pub block_counts: String,
    pub dsr_max_sensitivity_range: f64,
    pub pbo_max_sensitivity_range: f64,
    pub stress_out_dir: String,
    pub stress_execution_slippage_bps: u32,
    pub stress_execution_volatility_mult_bps: u32,
    pub stress_max_target_qty: Option<u32>,
    pub stress_max_position_notional_usd: Option<f64>,
    pub max_drawdown_ceiling: f64,
    pub placebo_out_dir: String,
}

pub struct ResearchReplaySummary {
    pub trial_id: String,
    pub strategy_id: String,
    pub economic_eval_id: String,
    pub run_id: uuid::Uuid,
    pub run_dir: PathBuf,
    pub canonical_robustness_artifact_sha256: String,
    pub is_complete: bool,
    pub all_applicable_passed: bool,
}

/// Builds a `ResearchOosReplayStrategy` for `bars` from `bundle`'s schedule
/// for `excluded_symbol` (`None` == the baseline, full-universe schedule).
/// Fails closed if a non-baseline exclusion is requested for a symbol the
/// bundle never computed a leave-one-out schedule for (mission C4 "wrong
/// bars: refuse" -- an incomplete bundle must never silently fall back to
/// the baseline schedule for a universe it does not describe).
fn strategy_for(
    bundle: &ResolvedReplayBundle,
    excluded_symbol: Option<&str>,
    bars: &[BacktestBar],
) -> Result<Box<dyn Strategy>> {
    let schedule = match excluded_symbol {
        None => bundle.baseline_schedule.clone(),
        Some(sym) => bundle
            .loo_schedules
            .get(sym)
            .with_context(|| {
                format!(
                    "Fail-closed: replay bundle has no leave-one-out schedule for excluded symbol \
                     {sym:?} -- refusing to fall back to the baseline (wrong-universe) schedule"
                )
            })?
            .clone(),
    };
    Ok(Box::new(ResearchOosReplayStrategy::new(
        bundle.semantic.clone(),
        schedule,
        bars,
    )))
}

/// C2/C3/C4/C5: end-to-end canonical Backtest/stress/P9 production for one
/// registered Wave06 Research trial, entirely via existing production
/// artifact/finalizer seams.
pub fn run_research_replay_backtest(args: ResearchReplayArgs) -> Result<ResearchReplaySummary> {
    let bundle = resolve_replay_bundle(
        Path::new(&args.replay_bundle_dir),
        &args.trial_id,
        &args.strategy_id,
        &args.economic_eval_id,
    )
    .context("resolve_replay_bundle failed")?;

    let bars = load_research_bars_csv(&bundle.bars_csv_path).context("load_research_bars_csv failed")?;
    if bars.is_empty() {
        bail!("Fail-closed: replay bundle's bars_csv resolved to zero bars");
    }

    let timeframe_secs = match bundle.semantic.timeframe.as_str() {
        "1D" | "1Day" => 86_400,
        other => bail!("Fail-closed: unsupported replay timeframe {other:?}"),
    };
    let base_config = BacktestConfig {
        timeframe_secs,
        ..BacktestConfig::conservative_defaults()
    };

    let baseline_strategy = strategy_for(&bundle, None, &bars)?;
    let mut engine = BacktestEngine::new(base_config.clone());
    engine
        .add_strategy(baseline_strategy)
        .context("add_strategy failed for baseline replay strategy")?;
    let report = engine.run(&bars).context("baseline replay backtest run failed")?;

    let run_id = report.run_id;
    let git_hash = super::bkt::bkt_git_hash();
    let host_fp = super::bkt::bkt_host_fingerprint();
    let init_result = mqk_artifacts::init_run_artifacts(mqk_artifacts::InitRunArtifactsArgs {
        exports_root: Path::new(&args.out_dir),
        schema_version: 1,
        run_id,
        strategy_name: &report.strategy_name,
        engine_id: "mqk-backtest",
        mode: "research_replay",
        timeframe: Some(&bundle.semantic.timeframe),
        timeframe_secs: Some(timeframe_secs),
        git_hash: &git_hash,
        config_hash: report.config_id.to_string().as_str(),
        host_fingerprint: &host_fp,
        now_utc: chrono::Utc::now(),
    })
    .context("init_run_artifacts failed")?;

    mqk_artifacts::write_backtest_report(&init_result.run_dir, &report, base_config.initial_cash_micros)
        .context("write_backtest_report failed")?;

    let make_strategy = || -> Box<dyn Strategy> {
        strategy_for(&bundle, None, &bars).expect("baseline strategy construction cannot fail here")
    };
    let make_strategy_for_bars = |filtered: &[BacktestBar]| -> Box<dyn Strategy> {
        let full: BTreeSet<&str> = bars.iter().map(|b| b.symbol.as_str()).collect();
        let remaining: BTreeSet<&str> = filtered.iter().map(|b| b.symbol.as_str()).collect();
        let excluded: Vec<&&str> = full.difference(&remaining).collect();
        let excluded_symbol = excluded
            .first()
            .unwrap_or_else(|| panic!("symbol_leave_one_out must exclude exactly one symbol"));
        strategy_for(&bundle, Some(excluded_symbol), filtered)
            .unwrap_or_else(|e| panic!("{e}"))
    };

    let stress_output =
        mqk_backtest::run_backtest_stress_suite(&report, &base_config, &bars, &make_strategy);
    mqk_artifacts::write_canonical_stress_suite(&init_result.run_dir, &stress_output)
        .context("write_canonical_stress_suite failed")?;

    let gauntlet_output = mqk_backtest::run_robustness_gauntlet_with_symbol_loo_factory(
        &report,
        &base_config,
        &bars,
        make_strategy,
        make_strategy_for_bars,
    );
    mqk_artifacts::write_canonical_robustness_gauntlet(&init_result.run_dir, &gauntlet_output)
        .context("write_canonical_robustness_gauntlet failed")?;

    // The three EXISTING deferred cross-language scenario finalizers --
    // called exactly as any other backtest candidate would call them.
    run_finalize_robustness_sensitivity(
        args.out_dir.clone(),
        run_id.to_string(),
        args.registry_db.clone(),
        args.trial_id.clone(),
        args.judge_artifact_sha256.clone(),
        args.research_py_root.clone(),
        args.python.clone(),
        args.block_counts.clone(),
        args.dsr_max_sensitivity_range,
        args.pbo_max_sensitivity_range,
    )
    .context("run_finalize_robustness_sensitivity failed")?;

    run_finalize_p7a_p7b_replay_stress(
        args.out_dir.clone(),
        run_id.to_string(),
        args.registry_db.clone(),
        args.trial_id.clone(),
        args.economic_eval_id.clone(),
        args.research_py_root.clone(),
        args.python.clone(),
        args.stress_out_dir.clone(),
        args.stress_execution_slippage_bps,
        args.stress_execution_volatility_mult_bps,
        args.stress_max_target_qty,
        args.stress_max_position_notional_usd,
        args.max_drawdown_ceiling,
    )
    .context("run_finalize_p7a_p7b_replay_stress failed")?;

    run_finalize_genuine_shuffled_placebo(
        args.out_dir.clone(),
        run_id.to_string(),
        args.registry_db.clone(),
        args.trial_id.clone(),
        args.economic_eval_id.clone(),
        args.research_py_root.clone(),
        args.python.clone(),
        args.placebo_out_dir.clone(),
    )
    .context("run_finalize_genuine_shuffled_placebo failed")?;

    let finalized = mqk_artifacts::load_canonical_robustness_gauntlet(&init_result.run_dir)
        .context("re-loading the finalized canonical robustness gauntlet failed")?;
    let artifact_sha256 = sha256_hex_of_file(&init_result.run_dir.join("robustness_gauntlet.json"))?;

    Ok(ResearchReplaySummary {
        trial_id: bundle.trial_id,
        strategy_id: bundle.strategy_id,
        economic_eval_id: bundle.economic_eval_id,
        run_id,
        run_dir: init_result.run_dir,
        canonical_robustness_artifact_sha256: artifact_sha256,
        is_complete: finalized.is_complete(),
        all_applicable_passed: finalized.all_applicable_passed(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("mqk_cli_research_replay_test_{label}_{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn rfc3339(epoch: i64) -> String {
        chrono::DateTime::<chrono::Utc>::from_timestamp(epoch, 0).unwrap().to_rfc3339()
    }

    fn write_bars(dir: &Path, symbols: &[&str], days: i64) -> PathBuf {
        let path = dir.join("bars.csv");
        let mut s = String::from("symbol,end_ts,open,high,low,close,volume\n");
        for d in 0..days {
            let ts = 86_400 * (d + 1);
            for (i, sym) in symbols.iter().enumerate() {
                let px = 100.0 + 10.0 * i as f64;
                s.push_str(&format!("{sym},{},{px},{px},{px},{px},1000\n", rfc3339(ts)));
            }
        }
        fs::write(&path, s).unwrap();
        path
    }

    fn write_schedule(dir: &Path, name: &str, rows: &[(i64, &str, i64)]) -> PathBuf {
        let path = dir.join(name);
        let mut s = String::from("decision_ts,symbol,target_qty\n");
        for (ts, sym, qty) in rows {
            s.push_str(&format!("{},{sym},{qty}\n", rfc3339(*ts)));
        }
        fs::write(&path, s).unwrap();
        path
    }

    fn file_hash_record(path: &Path) -> serde_json::Value {
        serde_json::json!({
            "path": path.to_string_lossy(),
            "sha256": sha256_hex_of_file(path).unwrap(),
            "bytes": fs::metadata(path).unwrap().len(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn write_manifest(
        dir: &Path,
        trial_id: &str,
        strategy_id: &str,
        economic_eval_id: &str,
        bars_path: &Path,
        baseline_path: &Path,
        loo: &[(&str, PathBuf)],
    ) {
        let mut loo_obj = serde_json::Map::new();
        for (sym, path) in loo {
            loo_obj.insert(
                sym.to_string(),
                serde_json::json!({
                    "file": path.file_name().unwrap().to_string_lossy(),
                    "sha256": sha256_hex_of_file(path).unwrap(),
                    "bytes": fs::metadata(path).unwrap().len(),
                    "row_count": 1,
                }),
            );
        }
        let manifest = serde_json::json!({
            "schema_version": REQUIRED_MANIFEST_PROTOCOL_VERSION,
            "protocol_version": REQUIRED_MANIFEST_PROTOCOL_VERSION,
            "lineage": {
                "trial_id": trial_id, "experiment_id": "exp", "hypothesis_id": "hyp",
                "strategy_id": strategy_id, "attempt_id": "att0001", "economic_eval_id": economic_eval_id,
            },
            "replay_semantic_spec": {
                "replay_protocol_version": REQUIRED_MANIFEST_PROTOCOL_VERSION,
                "strategy_id": strategy_id,
                "feature_columns": ["test_xs_rank"],
                "feature_transform": "cross_sectional_percentile_rank_rerank_of_authenticated_feature_v1",
                "direction_policy": "cross_sectional_rank_long_only_v1",
                "rank_side_count": 1,
                "long_only": true,
                "borrow_model": null,
                "max_gross_exposure": 1.0,
                "timeframe": "1D",
                "weight_to_share": {"equity_usd": 100000.0, "max_target_qty": null, "max_position_notional_usd": null},
            },
            "holdout_start_utc": rfc3339(86_400 * 1_000),
            "source_file_hashes": { "bars_csv": file_hash_record(bars_path) },
            "baseline_schedule": {
                "file": baseline_path.file_name().unwrap().to_string_lossy(),
                "sha256": sha256_hex_of_file(baseline_path).unwrap(),
                "bytes": fs::metadata(baseline_path).unwrap().len(),
                "row_count": 1,
            },
            "symbol_loo_schedules": loo_obj,
        });
        fs::write(dir.join("manifest.json"), serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
    }

    /// REQUIRED TEST 2: wrong trial_id -> refusal.
    #[test]
    fn wrong_trial_id_refused() {
        let dir = unique_dir("wrong_trial");
        let bars = write_bars(&dir, &["AAA", "BBB"], 2);
        let baseline = write_schedule(&dir, "baseline.csv", &[(86_400, "AAA", 5)]);
        write_manifest(&dir, "trial-real", "strat-1", "econ-1", &bars, &baseline, &[]);
        let err = resolve_replay_bundle(&dir, "trial-WRONG", "strat-1", "econ-1").unwrap_err();
        assert!(err.to_string().contains("trial_id"), "{err}");
    }

    /// REQUIRED TEST 3: wrong economic_eval_id -> refusal.
    #[test]
    fn wrong_economic_eval_id_refused() {
        let dir = unique_dir("wrong_econ");
        let bars = write_bars(&dir, &["AAA", "BBB"], 2);
        let baseline = write_schedule(&dir, "baseline.csv", &[(86_400, "AAA", 5)]);
        write_manifest(&dir, "trial-1", "strat-1", "econ-real", &bars, &baseline, &[]);
        let err = resolve_replay_bundle(&dir, "trial-1", "strat-1", "econ-WRONG").unwrap_err();
        assert!(err.to_string().contains("economic_eval_id"), "{err}");
    }

    /// REQUIRED TEST 4: wrong strategy_id -> refusal.
    #[test]
    fn wrong_strategy_id_refused() {
        let dir = unique_dir("wrong_strategy");
        let bars = write_bars(&dir, &["AAA", "BBB"], 2);
        let baseline = write_schedule(&dir, "baseline.csv", &[(86_400, "AAA", 5)]);
        write_manifest(&dir, "trial-1", "strat-real", "econ-1", &bars, &baseline, &[]);
        let err = resolve_replay_bundle(&dir, "trial-1", "strat-WRONG", "econ-1").unwrap_err();
        assert!(err.to_string().contains("strategy_id"), "{err}");
    }

    /// REQUIRED TEST 5: mutated source/replay file -> refusal.
    #[test]
    fn mutated_bars_file_refused() {
        let dir = unique_dir("mutated_bars");
        let bars = write_bars(&dir, &["AAA", "BBB"], 2);
        let baseline = write_schedule(&dir, "baseline.csv", &[(86_400, "AAA", 5)]);
        write_manifest(&dir, "trial-1", "strat-1", "econ-1", &bars, &baseline, &[]);
        let original = fs::read_to_string(&bars).unwrap();
        fs::write(&bars, original + "MUTATED,9999999999,1,1,1,1,1\n").unwrap();
        let err = resolve_replay_bundle(&dir, "trial-1", "strat-1", "econ-1").unwrap_err();
        assert!(err.to_string().contains("no longer matches"), "{err}");
    }

    /// REQUIRED TEST: missing schedule file -> refusal.
    #[test]
    fn missing_baseline_schedule_file_refused() {
        let dir = unique_dir("missing_schedule");
        let bars = write_bars(&dir, &["AAA", "BBB"], 2);
        let baseline = write_schedule(&dir, "baseline.csv", &[(86_400, "AAA", 5)]);
        write_manifest(&dir, "trial-1", "strat-1", "econ-1", &bars, &baseline, &[]);
        fs::remove_file(&baseline).unwrap();
        let err = resolve_replay_bundle(&dir, "trial-1", "strat-1", "econ-1").unwrap_err();
        assert!(format!("{err:#}").to_lowercase().contains("read failed"), "{err:#}");
    }

    /// Bars CSV round-trip: ISO8601 -> epoch seconds, float prices -> micros.
    #[test]
    fn bars_csv_round_trip_converts_prices_and_timestamps() {
        let dir = unique_dir("bars_roundtrip");
        let bars_path = write_bars(&dir, &["AAA"], 2);
        let bars = load_research_bars_csv(&bars_path).unwrap();
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].symbol, "AAA");
        assert_eq!(bars[0].close_micros, 100_000_000);
        assert_eq!(bars[0].end_ts, 86_400);
        assert_eq!(bars[1].end_ts, 172_800);
    }

    /// REQUIRED TESTS 7/9/10: a real baseline replay run through the actual
    /// production artifact pipeline (init_run_artifacts / write_backtest_report
    /// / write_canonical_stress_suite / run_robustness_gauntlet_with_symbol_loo_factory
    /// / write_canonical_robustness_gauntlet) produces an artifact that (a) is
    /// NOT complete before the three Research-registry-anchored scenarios are
    /// merged, (b) reloads successfully through the existing
    /// load_canonical_robustness_gauntlet, and (c) used the bar-aware
    /// leave-one-out factory (proven applicable+passed for a genuinely
    /// distinct per-symbol LOO schedule).
    #[test]
    fn baseline_gauntlet_artifact_is_incomplete_and_reloads() {
        let bundle_dir = unique_dir("bundle_full");
        let symbols = ["AAA", "BBB", "CCC"];
        let bars_path = write_bars(&bundle_dir, &symbols, 5);
        let bars = load_research_bars_csv(&bars_path).unwrap();
        let all_ts: BTreeSet<i64> = bars.iter().map(|b| b.end_ts).collect();

        let mut baseline_schedule: BTreeMap<i64, Vec<TargetPosition>> = BTreeMap::new();
        for ts in &all_ts {
            baseline_schedule.insert(
                *ts,
                vec![
                    TargetPosition::new("AAA", 5),
                    TargetPosition::new("BBB", 0),
                    TargetPosition::new("CCC", 0),
                ],
            );
        }
        let mut loo_schedules: BTreeMap<String, BTreeMap<i64, Vec<TargetPosition>>> = BTreeMap::new();
        for excluded in symbols {
            let mut sched = BTreeMap::new();
            for ts in &all_ts {
                let rows: Vec<TargetPosition> = symbols
                    .iter()
                    .filter(|s| **s != excluded)
                    .map(|s| TargetPosition::new(*s, 5))
                    .collect();
                sched.insert(*ts, rows);
            }
            loo_schedules.insert(excluded.to_string(), sched);
        }

        let semantic = ReplaySemanticSpec {
            replay_protocol_version: REQUIRED_MANIFEST_PROTOCOL_VERSION.to_string(),
            strategy_id: "strat-1".to_string(),
            feature_columns: vec!["test_xs_rank".to_string()],
            feature_transform: "cross_sectional_percentile_rank_rerank_of_authenticated_feature_v1"
                .to_string(),
            direction_policy: "cross_sectional_rank_long_only_v1".to_string(),
            rank_side_count: 1,
            long_only: true,
            borrow_model: None,
            max_gross_exposure: 1.0,
            timeframe: "1D".to_string(),
            equity_usd: 100_000.0,
            max_target_qty: None,
            max_position_notional_usd: None,
            trial_id: "trial-1".to_string(),
        };
        let bundle = ResolvedReplayBundle {
            bundle_dir: bundle_dir.clone(),
            trial_id: "trial-1".to_string(),
            strategy_id: "strat-1".to_string(),
            economic_eval_id: "econ-1".to_string(),
            semantic,
            bars_csv_path: bars_path,
            baseline_schedule,
            loo_schedules,
        };

        let base_config = BacktestConfig {
            timeframe_secs: 86_400,
            ..BacktestConfig::conservative_defaults()
        };
        let baseline_strategy = strategy_for(&bundle, None, &bars).unwrap();
        let mut engine = BacktestEngine::new(base_config.clone());
        engine.add_strategy(baseline_strategy).unwrap();
        let report = engine.run(&bars).unwrap();

        let out_dir = unique_dir("bundle_full_artifacts");
        let init_result = mqk_artifacts::init_run_artifacts(mqk_artifacts::InitRunArtifactsArgs {
            exports_root: &out_dir,
            schema_version: 1,
            run_id: report.run_id,
            strategy_name: &report.strategy_name,
            engine_id: "mqk-backtest",
            mode: "research_replay",
            timeframe: Some("1D"),
            timeframe_secs: Some(86_400),
            git_hash: "test",
            config_hash: &report.config_id.to_string(),
            host_fingerprint: "test",
            now_utc: chrono::Utc::now(),
        })
        .unwrap();
        mqk_artifacts::write_backtest_report(&init_result.run_dir, &report, base_config.initial_cash_micros)
            .unwrap();

        let make_strategy = || -> Box<dyn Strategy> { strategy_for(&bundle, None, &bars).unwrap() };
        let make_strategy_for_bars = |filtered: &[BacktestBar]| -> Box<dyn Strategy> {
            let full: BTreeSet<&str> = bars.iter().map(|b| b.symbol.as_str()).collect();
            let remaining: BTreeSet<&str> = filtered.iter().map(|b| b.symbol.as_str()).collect();
            let excluded = *full.difference(&remaining).next().unwrap();
            strategy_for(&bundle, Some(excluded), filtered).unwrap()
        };

        let stress_output =
            mqk_backtest::run_backtest_stress_suite(&report, &base_config, &bars, &make_strategy);
        mqk_artifacts::write_canonical_stress_suite(&init_result.run_dir, &stress_output).unwrap();

        let gauntlet_output = mqk_backtest::run_robustness_gauntlet_with_symbol_loo_factory(
            &report,
            &base_config,
            &bars,
            make_strategy,
            make_strategy_for_bars,
        );
        let loo_outcome = gauntlet_output
            .scenarios
            .iter()
            .find(|s| s.name == "symbol_leave_one_out")
            .expect("symbol_leave_one_out scenario present");
        assert!(loo_outcome.applicable);
        assert!(loo_outcome.passed, "{:?}", loo_outcome.reason);

        mqk_artifacts::write_canonical_robustness_gauntlet(&init_result.run_dir, &gauntlet_output).unwrap();

        // REQUIRED TEST 7: incomplete P9 (three deferred Research-anchored
        // scenarios never merged in this test) remains is_complete=false.
        let loaded = mqk_artifacts::load_canonical_robustness_gauntlet(&init_result.run_dir).unwrap();
        assert!(!loaded.is_complete());
        assert_eq!(loaded.scenarios_run(), 6);
    }
}
