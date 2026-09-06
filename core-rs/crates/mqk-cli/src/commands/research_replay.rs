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
//!
//! W06-A-P9-CANONICAL-CLI-AUTHORITY-REPAIR-01 (Patch R3): the replay bundle
//! is never a caller-selected pre-existing directory -- this module invokes
//! the R1 Python builder (`mqk_research.ml.oos_replay_bundle_cli`) itself
//! into a fresh, command-controlled work directory, anchors to the
//! builder's own machine-readable `manifest_sha256`, and sources every
//! Wave06 campaign policy value (block counts, sensitivity ranges, P7A/P7B
//! stress knobs, max-drawdown ceiling, the comparison judge) from the
//! committed, frozen `PREDECLARED_CAMPAIGN.json` / the existing campaign
//! judge path -- never an operator-tunable outcome input.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

// ---------------------------------------------------------------------------
// R3.1 -- build the replay bundle from durable Research authority ourselves.
// ---------------------------------------------------------------------------

/// Machine-readable result of `mqk_research.ml.oos_replay_bundle_cli`
/// (R1.5) -- the authority seam this command anchors to instead of trusting
/// a caller-selected bundle directory.
#[derive(Debug, Clone, Deserialize)]
struct ReplayBuilderResult {
    status: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    manifest_path: Option<String>,
    #[serde(default)]
    manifest_sha256: Option<String>,
    #[serde(default)]
    trial_id: Option<String>,
    #[serde(default)]
    strategy_id: Option<String>,
    #[serde(default)]
    economic_eval_id: Option<String>,
}

/// R3.1 (Finding C): invokes the R1 Python builder
/// (`mqk_research.ml.oos_replay_bundle_cli`) itself, into a fresh,
/// command-controlled `work_dir` -- a caller may choose WHERE the bundle is
/// built, but never hand this command a pre-existing `manifest.json` and
/// have it trusted as evidence. Returns the builder's own parsed
/// machine-readable report; the caller (`run_research_replay_backtest`)
/// still independently anchors to `manifest_sha256` via
/// [`resolve_replay_bundle`] before trusting any bundle content.
fn build_replay_bundle_via_python(
    python: &str,
    research_py_root: &Path,
    registry_db: &Path,
    trial_id: &str,
    economic_eval_id: &str,
    work_dir: &Path,
) -> Result<ReplayBuilderResult> {
    if work_dir.join("manifest.json").exists() {
        bail!(
            "Fail-closed: replay work_dir {} already contains a manifest.json -- this command \
             builds its own bundle from durable Research authority every run; a caller-seeded \
             manifest is never trusted as evidence",
            work_dir.display()
        );
    }
    fs::create_dir_all(work_dir)
        .with_context(|| format!("failed to create replay work_dir {}", work_dir.display()))?;

    let src_dir = research_py_root.join("src");
    let output = Command::new(python)
        .env("PYTHONPATH", &src_dir)
        .args([
            "-m",
            "mqk_research.ml.oos_replay_bundle_cli",
            "--registry-db",
            &registry_db.display().to_string(),
            "--trial-id",
            trial_id,
            "--economic-eval-id",
            economic_eval_id,
            "--out-dir",
            &work_dir.display().to_string(),
        ])
        .output()
        .with_context(|| {
            format!("failed to spawn {python} -m mqk_research.ml.oos_replay_bundle_cli")
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result: ReplayBuilderResult = serde_json::from_str(stdout.trim()).with_context(|| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        format!(
            "oos_replay_bundle_cli produced unparseable output (exit={:?}): stdout={stdout:?} \
             stderr={stderr:?}",
            output.status.code()
        )
    })?;
    if result.status != "ok" {
        bail!(
            "Fail-closed: oos_replay_bundle_cli refused: {}",
            result.reason.as_deref().unwrap_or("unknown reason")
        );
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// R3.3 -- frozen Wave06 campaign policy, never an operator-tunable knob.
// ---------------------------------------------------------------------------

const PREDECLARED_CAMPAIGN_RELATIVE_PATH: &str = "experiments/wave06_campaign/PREDECLARED_CAMPAIGN.json";

/// R3.3: `block_counts`/`dsr_max_sensitivity_range`/`pbo_max_sensitivity_range`/
/// P7A-P7B stress knobs/`max_drawdown_ceiling` are PREDECLARED Wave06
/// campaign policy, frozen BEFORE any Wave06 result -- never
/// operator-tunable outcome inputs. Sourced here from the committed,
/// git-tracked `PREDECLARED_CAMPAIGN.json`, never a CLI flag (Finding H).
#[derive(Debug, Clone)]
struct Wave06CampaignPolicy {
    block_counts: Vec<u32>,
    dsr_max_sensitivity_range: f64,
    pbo_max_sensitivity_range: f64,
    stress_execution_slippage_bps: u32,
    stress_execution_volatility_mult_bps: u32,
    stress_max_target_qty: Option<u32>,
    stress_max_position_notional_usd: Option<f64>,
    max_drawdown_ceiling: f64,
}

impl Wave06CampaignPolicy {
    fn load(research_py_root: &Path) -> Result<Self> {
        let path = research_py_root.join(PREDECLARED_CAMPAIGN_RELATIVE_PATH);
        let text = fs::read_to_string(&path).with_context(|| {
            format!(
                "Fail-closed: cannot read frozen Wave06 campaign policy at {}",
                path.display()
            )
        })?;
        let value: serde_json::Value = serde_json::from_str(&text).with_context(|| {
            format!("Fail-closed: malformed Wave06 campaign policy at {}", path.display())
        })?;
        let policy = value
            .get("advancement_policy")
            .context("Fail-closed: PREDECLARED_CAMPAIGN.json missing advancement_policy")?;
        let sensitivity = policy
            .get("dsr_pbo_block_count_sensitivity_requirement")
            .context(
                "Fail-closed: PREDECLARED_CAMPAIGN.json missing \
                 dsr_pbo_block_count_sensitivity_requirement",
            )?;
        let block_counts: Vec<u32> = sensitivity
            .get("block_counts")
            .and_then(|v| v.as_array())
            .context("Fail-closed: missing/malformed block_counts")?
            .iter()
            .map(|v| {
                v.as_u64()
                    .map(|n| n as u32)
                    .context("Fail-closed: block_counts entry is not a non-negative integer")
            })
            .collect::<Result<Vec<u32>>>()?;
        let dsr_max_sensitivity_range = sensitivity
            .get("dsr_max_sensitivity_range")
            .and_then(|v| v.as_f64())
            .context("Fail-closed: missing/malformed dsr_max_sensitivity_range")?;
        let pbo_max_sensitivity_range = sensitivity
            .get("pbo_max_sensitivity_range")
            .and_then(|v| v.as_f64())
            .context("Fail-closed: missing/malformed pbo_max_sensitivity_range")?;

        let stress = policy.get("p7a_p7b_economic_replay_stress_requirement").context(
            "Fail-closed: PREDECLARED_CAMPAIGN.json missing p7a_p7b_economic_replay_stress_requirement",
        )?;
        let stress_execution_slippage_bps = stress
            .get("stress_execution_slippage_bps")
            .and_then(|v| v.as_u64())
            .context("Fail-closed: missing/malformed stress_execution_slippage_bps")?
            as u32;
        let stress_execution_volatility_mult_bps = stress
            .get("stress_execution_volatility_mult_bps")
            .and_then(|v| v.as_u64())
            .context("Fail-closed: missing/malformed stress_execution_volatility_mult_bps")?
            as u32;
        let stress_max_target_qty =
            stress.get("stress_max_target_qty").and_then(|v| v.as_u64()).map(|n| n as u32);
        let stress_max_position_notional_usd =
            stress.get("stress_max_position_notional_usd").and_then(|v| v.as_f64());
        let max_drawdown_ceiling = stress
            .get("max_drawdown_ceiling")
            .and_then(|v| v.as_f64())
            .context("Fail-closed: missing/malformed max_drawdown_ceiling")?;

        Ok(Self {
            block_counts,
            dsr_max_sensitivity_range,
            pbo_max_sensitivity_range,
            stress_execution_slippage_bps,
            stress_execution_volatility_mult_bps,
            stress_max_target_qty,
            stress_max_position_notional_usd,
            max_drawdown_ceiling,
        })
    }

    fn block_counts_csv(&self) -> String {
        self.block_counts.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",")
    }
}

// ---------------------------------------------------------------------------
// R3.4 -- canonical Wave06 campaign judge authority, never a caller-selected
// judge_artifact_sha256.
// ---------------------------------------------------------------------------

const RUN_CAMPAIGN_JUDGE_RELATIVE_PATH: &str = "experiments/wave06_campaign/run_campaign_judge.py";

/// R3.4 (Finding H): resolves the canonical Wave06 campaign judge's
/// `judge_artifact_sha256` by invoking the EXISTING, already-accepted
/// campaign judge path (`wave06_campaign/run_campaign_judge.py`) itself --
/// never accepts an operator-supplied SHA identifying an arbitrary
/// registered judge. That script's own population logic already enforces
/// the canonical judge protocol/schema, the exact campaign comparison
/// population (union of every ACTUALLY-attempted campaign_order candidate's
/// complete real/placebo hypothesis families), and the accepted
/// `cscv_target_block_count=10` `JudgeSpec` default this function never
/// overrides.
fn resolve_campaign_judge_artifact_sha256(
    python: &str,
    research_py_root: &Path,
    registry_db: &Path,
) -> Result<String> {
    let script = research_py_root.join(RUN_CAMPAIGN_JUDGE_RELATIVE_PATH);
    let output = Command::new(python)
        .arg(&script)
        .args(["--execute", "--json", "--registry-db", &registry_db.display().to_string()])
        .output()
        .with_context(|| format!("failed to spawn {python} {}", script.display()))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).with_context(|| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        format!(
            "run_campaign_judge.py produced unparseable output (exit={:?}): stdout={stdout:?} \
             stderr={stderr:?}",
            output.status.code()
        )
    })?;
    if value.get("status").and_then(|v| v.as_str()) != Some("ok") {
        let reason = value.get("reason").and_then(|v| v.as_str()).unwrap_or("unknown reason");
        bail!("Fail-closed: run_campaign_judge.py refused/failed: {reason}");
    }
    value
        .get("judge_artifact_sha256")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .context("Fail-closed: run_campaign_judge.py JSON missing judge_artifact_sha256")
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
///
/// R3.1/R3.2: `expected_manifest_sha256` anchors this manifest to
/// independent authority (the R1 Python builder's own machine-readable
/// report -- see [`build_replay_bundle_via_python`]) BEFORE any of its
/// content is trusted. Without this anchor, a caller could mutate a
/// schedule CSV and consistently rewrite the manifest's own recorded child
/// hash for it, and every check below would still pass (the manifest would
/// remain internally self-consistent) -- this is precisely the
/// self-attestation gap Finding C identified.
pub fn resolve_replay_bundle(
    bundle_dir: &Path,
    expected_trial_id: &str,
    expected_strategy_id: &str,
    expected_economic_eval_id: &str,
    expected_manifest_sha256: &str,
) -> Result<ResolvedReplayBundle> {
    let manifest_path = bundle_dir.join("manifest.json");
    let actual_manifest_sha256 = sha256_hex_of_file(&manifest_path)
        .with_context(|| format!("missing replay bundle manifest: {}", manifest_path.display()))?;
    if actual_manifest_sha256 != expected_manifest_sha256 {
        bail!(
            "Fail-closed: replay bundle manifest.json at {} does not match the replay builder's \
             own reported manifest_sha256 (expected {expected_manifest_sha256}, got \
             {actual_manifest_sha256}) -- refusing a mutated/inconsistent replay bundle",
            manifest_path.display()
        );
    }
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

/// R3.3: no `judge_artifact_sha256`/`block_counts`/`dsr_max_sensitivity_range`/
/// `pbo_max_sensitivity_range`/P7A-P7B stress knobs/`max_drawdown_ceiling`
/// fields -- those are frozen Wave06 campaign policy, resolved internally
/// from `PREDECLARED_CAMPAIGN.json` (see [`Wave06CampaignPolicy::load`]) and
/// the canonical campaign judge path (see
/// [`resolve_campaign_judge_artifact_sha256`]), never operator-tunable
/// outcome inputs (Finding H). `replay_work_dir` is a fresh/empty
/// destination directory this command builds its own replay bundle into
/// (R3.1) -- never a pre-existing bundle directory to trust.
#[allow(clippy::too_many_arguments)]
pub struct ResearchReplayArgs {
    pub registry_db: String,
    pub trial_id: String,
    pub strategy_id: String,
    pub economic_eval_id: String,
    pub replay_work_dir: String,
    pub out_dir: String,
    pub research_py_root: String,
    pub python: String,
    pub stress_out_dir: String,
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

/// C2/C3/C4/C5/R3: end-to-end canonical Backtest/stress/P9 production for
/// one registered Wave06 Research trial, entirely via existing production
/// artifact/finalizer seams. R3.1: builds the replay bundle itself (never a
/// caller-selected pre-existing directory); R3.3/R3.4: sources every
/// campaign policy value and the comparison judge from frozen, existing
/// authority, never a caller-supplied outcome-affecting flag.
pub fn run_research_replay_backtest(args: ResearchReplayArgs) -> Result<ResearchReplaySummary> {
    let research_py_root = Path::new(&args.research_py_root);
    let registry_db = Path::new(&args.registry_db);

    let builder = build_replay_bundle_via_python(
        &args.python,
        research_py_root,
        registry_db,
        &args.trial_id,
        &args.economic_eval_id,
        Path::new(&args.replay_work_dir),
    )
    .context("build_replay_bundle_via_python failed")?;

    let builder_trial_id = builder
        .trial_id
        .context("Fail-closed: oos_replay_bundle_cli JSON missing trial_id")?;
    let builder_strategy_id = builder
        .strategy_id
        .context("Fail-closed: oos_replay_bundle_cli JSON missing strategy_id")?;
    let builder_economic_eval_id = builder
        .economic_eval_id
        .context("Fail-closed: oos_replay_bundle_cli JSON missing economic_eval_id")?;
    let manifest_path: PathBuf = builder
        .manifest_path
        .context("Fail-closed: oos_replay_bundle_cli JSON missing manifest_path")?
        .into();
    let manifest_sha256 = builder
        .manifest_sha256
        .context("Fail-closed: oos_replay_bundle_cli JSON missing manifest_sha256")?;

    if builder_trial_id != args.trial_id {
        bail!(
            "Fail-closed: replay builder resolved trial_id {builder_trial_id:?} != requested \
             {:?}",
            args.trial_id
        );
    }
    if builder_strategy_id != args.strategy_id {
        bail!(
            "Fail-closed: replay builder resolved strategy_id {builder_strategy_id:?} != \
             requested {:?}",
            args.strategy_id
        );
    }
    if builder_economic_eval_id != args.economic_eval_id {
        bail!(
            "Fail-closed: replay builder resolved economic_eval_id {builder_economic_eval_id:?} \
             != requested {:?}",
            args.economic_eval_id
        );
    }
    if !manifest_path.exists() {
        bail!(
            "Fail-closed: replay builder reported manifest_path {} but it does not exist",
            manifest_path.display()
        );
    }
    let bundle_dir = manifest_path
        .parent()
        .context("Fail-closed: replay builder's manifest_path has no parent directory")?
        .to_path_buf();

    let bundle = resolve_replay_bundle(
        &bundle_dir,
        &args.trial_id,
        &args.strategy_id,
        &args.economic_eval_id,
        &manifest_sha256,
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

    // R3.3/R3.4: frozen Wave06 campaign policy + canonical campaign judge --
    // resolved from existing authority, never a caller-supplied flag.
    let policy = Wave06CampaignPolicy::load(research_py_root)
        .context("Wave06CampaignPolicy::load failed")?;
    let judge_artifact_sha256 =
        resolve_campaign_judge_artifact_sha256(&args.python, research_py_root, registry_db)
            .context("resolve_campaign_judge_artifact_sha256 failed")?;

    // The three EXISTING deferred cross-language scenario finalizers --
    // called exactly as any other backtest candidate would call them.
    run_finalize_robustness_sensitivity(
        args.out_dir.clone(),
        run_id.to_string(),
        args.registry_db.clone(),
        args.trial_id.clone(),
        judge_artifact_sha256,
        args.research_py_root.clone(),
        args.python.clone(),
        policy.block_counts_csv(),
        policy.dsr_max_sensitivity_range,
        policy.pbo_max_sensitivity_range,
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
        policy.stress_execution_slippage_bps,
        policy.stress_execution_volatility_mult_bps,
        policy.stress_max_target_qty,
        policy.stress_max_position_notional_usd,
        policy.max_drawdown_ceiling,
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
    ) -> String {
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
        let manifest_path = dir.join("manifest.json");
        fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
        sha256_hex_of_file(&manifest_path).unwrap()
    }

    /// REQUIRED TEST 2: wrong trial_id -> refusal.
    #[test]
    fn wrong_trial_id_refused() {
        let dir = unique_dir("wrong_trial");
        let bars = write_bars(&dir, &["AAA", "BBB"], 2);
        let baseline = write_schedule(&dir, "baseline.csv", &[(86_400, "AAA", 5)]);
        let sha = write_manifest(&dir, "trial-real", "strat-1", "econ-1", &bars, &baseline, &[]);
        let err = resolve_replay_bundle(&dir, "trial-WRONG", "strat-1", "econ-1", &sha).unwrap_err();
        assert!(err.to_string().contains("trial_id"), "{err}");
    }

    /// REQUIRED TEST 3: wrong economic_eval_id -> refusal.
    #[test]
    fn wrong_economic_eval_id_refused() {
        let dir = unique_dir("wrong_econ");
        let bars = write_bars(&dir, &["AAA", "BBB"], 2);
        let baseline = write_schedule(&dir, "baseline.csv", &[(86_400, "AAA", 5)]);
        let sha = write_manifest(&dir, "trial-1", "strat-1", "econ-real", &bars, &baseline, &[]);
        let err = resolve_replay_bundle(&dir, "trial-1", "strat-1", "econ-WRONG", &sha).unwrap_err();
        assert!(err.to_string().contains("economic_eval_id"), "{err}");
    }

    /// REQUIRED TEST 4: wrong strategy_id -> refusal.
    #[test]
    fn wrong_strategy_id_refused() {
        let dir = unique_dir("wrong_strategy");
        let bars = write_bars(&dir, &["AAA", "BBB"], 2);
        let baseline = write_schedule(&dir, "baseline.csv", &[(86_400, "AAA", 5)]);
        let sha = write_manifest(&dir, "trial-1", "strat-real", "econ-1", &bars, &baseline, &[]);
        let err = resolve_replay_bundle(&dir, "trial-1", "strat-WRONG", "econ-1", &sha).unwrap_err();
        assert!(err.to_string().contains("strategy_id"), "{err}");
    }

    /// REQUIRED TEST 5: mutated source/replay file -> refusal.
    #[test]
    fn mutated_bars_file_refused() {
        let dir = unique_dir("mutated_bars");
        let bars = write_bars(&dir, &["AAA", "BBB"], 2);
        let baseline = write_schedule(&dir, "baseline.csv", &[(86_400, "AAA", 5)]);
        let sha = write_manifest(&dir, "trial-1", "strat-1", "econ-1", &bars, &baseline, &[]);
        let original = fs::read_to_string(&bars).unwrap();
        fs::write(&bars, original + "MUTATED,9999999999,1,1,1,1,1\n").unwrap();
        let err = resolve_replay_bundle(&dir, "trial-1", "strat-1", "econ-1", &sha).unwrap_err();
        assert!(err.to_string().contains("no longer matches"), "{err}");
    }

    /// REQUIRED TEST: missing schedule file -> refusal.
    #[test]
    fn missing_baseline_schedule_file_refused() {
        let dir = unique_dir("missing_schedule");
        let bars = write_bars(&dir, &["AAA", "BBB"], 2);
        let baseline = write_schedule(&dir, "baseline.csv", &[(86_400, "AAA", 5)]);
        let sha = write_manifest(&dir, "trial-1", "strat-1", "econ-1", &bars, &baseline, &[]);
        fs::remove_file(&baseline).unwrap();
        let err = resolve_replay_bundle(&dir, "trial-1", "strat-1", "econ-1", &sha).unwrap_err();
        assert!(format!("{err:#}").to_lowercase().contains("read failed"), "{err:#}");
    }

    /// R3.2 (Finding C): the missing negative control -- alter a schedule
    /// AND consistently rewrite the manifest's own recorded child hash for
    /// it (so the manifest stays internally self-consistent). Refused
    /// because the manifest's OWN bytes no longer match the anchor the
    /// caller (the replay builder) originally reported.
    #[test]
    fn manifest_mutation_with_consistent_child_hash_rewrite_refused() {
        let dir = unique_dir("manifest_mutation");
        let bars = write_bars(&dir, &["AAA", "BBB"], 2);
        let baseline = write_schedule(&dir, "baseline.csv", &[(86_400, "AAA", 5)]);
        let original_sha = write_manifest(&dir, "trial-1", "strat-1", "econ-1", &bars, &baseline, &[]);

        // Mutate the schedule CSV...
        let original = fs::read_to_string(&baseline).unwrap();
        fs::write(&baseline, original + "9999999999,ZZZ,999\n").unwrap();
        // ...and consistently rewrite the manifest's own recorded hash for
        // it, so the manifest is internally self-consistent (every
        // per-file check inside `resolve_replay_bundle` would otherwise
        // pass) -- but its own bytes are now different from what
        // `original_sha` anchors to.
        write_manifest(&dir, "trial-1", "strat-1", "econ-1", &bars, &baseline, &[]);

        let err = resolve_replay_bundle(&dir, "trial-1", "strat-1", "econ-1", &original_sha).unwrap_err();
        assert!(err.to_string().contains("does not match the replay builder's own reported manifest_sha256"), "{err}");
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

    // -------------------------------------------------------------------
    // R3.5 -- full canonical completion synthetic E2E proof.
    // -------------------------------------------------------------------

    /// R3.5: a load-bearing synthetic end-to-end proof using REAL production
    /// wrappers throughout -- a real Research registry trial (registered via
    /// the real `run_registered_economic_walkforward_eval`, through
    /// `research-py/tests/support/build_r3_e2e_fixture.py`, never a
    /// hand-authored registry row), the real R1 Python replay-bundle
    /// builder, the real canonical judge path, and the real
    /// `run_research_replay_backtest` production command -- no hand-authored
    /// `BacktestReport`, no hand-written canonical robustness JSON, no
    /// bypass of any production finalizer.
    ///
    /// `#[ignore]`d by default (needs a working `python` + research-py deps
    /// on PATH and takes real wall-clock time for five real subprocess
    /// invocations) -- run explicitly with `cargo test -p mqk-cli --bin
    /// mqk-cli -- --ignored r3_5_full_canonical_completion_synthetic_e2e_proof`.
    ///
    /// NOT asserting `all_applicable_passed() == true`: two genuine,
    /// deterministic, OUT-OF-SCOPE-for-this-wave findings, both discovered
    /// while constructing this exact fixture and confirmed independent of
    /// this fixture's own parameters, make an honest "every scenario
    /// passes" synthetic candidate infeasible within this wave's scope:
    ///   1. `p7a_p7b_economic_replay_stress`/`genuine_shuffled_placebo`'s
    ///      `_reconstruct_baseline_spec` could not even round-trip a
    ///      cross_sectional_rank_* (LIQ-01/VOL-01's own family) trial's
    ///      persisted `signal_policy` before this wave's fix (a genuine,
    ///      narrow, already-fixed bug: `tie_policy` is persisted as an
    ///      identity-only field for that direction-policy family but was
    ///      never a `SignalPolicySpec.__init__` parameter) -- now fixed as
    ///      part of this same patch, and `p7a_p7b_economic_replay_stress`
    ///      genuinely evaluates and passes for this fixture.
    ///   2. `genuine_shuffled_placebo`'s fold-wide score shuffle combined
    ///      with `_resolve_rank_direction_for_frame`'s exact
    ///      (`tie_tol=1e-9`) boundary-tie refusal is structurally
    ///      incompatible with ANY cross_sectional_percentile_rank feature
    ///      (LIQ-01/VOL-01's own feature-transform family, R1.3): that
    ///      transform always maps a decision date's cross-section onto the
    ///      SAME small, fixed value set (`{1/N, ..., N/N}`); a fold spanning
    ///      more than one decision date therefore shuffles many EXACT
    ///      repeats of that fixed set, making a boundary-adjacent exact
    ///      duplicate on some shuffled date combinatorially near-certain --
    ///      while a fold spanning exactly one decision date (the only way to
    ///      avoid the collision) leaves no later bar for economic_walk_forward_v1's
    ///      causal next-bar execution to fill any order against, making
    ///      every position size zero. This is a genuine, reproducible,
    ///      deterministic finding, NOT a fixture defect -- reported here per
    ///      mission instruction ("if an all-pass synthetic fixture cannot
    ///      honestly make every scenario pass ... report the exact
    ///      deterministic reason") rather than weakened or fabricated.
    ///      `genuine_shuffled_placebo` therefore genuinely, honestly reports
    ///      `applicable: true, passed: false` for this fixture, which is
    ///      sufficient for `is_complete()` (evidence coverage) but not for
    ///      `all_applicable_passed()`.
    #[test]
    #[ignore = "needs a working python + research-py deps on PATH; real subprocess E2E, run explicitly"]
    fn r3_5_full_canonical_completion_synthetic_e2e_proof() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let research_py_root = repo_root.join("research-py");
        let fixture_script = research_py_root.join("tests/support/build_r3_e2e_fixture.py");
        assert!(fixture_script.exists(), "{}", fixture_script.display());

        let dir = unique_dir("r3_5_e2e");
        let registry_db = dir.join("registry.sqlite3");
        let run_root = dir.join("fixture_runs");

        let python = "python";
        let output = std::process::Command::new(python)
            .arg(&fixture_script)
            .arg(&registry_db)
            .arg(&run_root)
            .output()
            .expect("failed to spawn build_r3_e2e_fixture.py");
        assert!(
            output.status.success(),
            "fixture build failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let fixture: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("fixture builder produced invalid JSON");
        assert_eq!(fixture["status"], "ok");
        let strategy_id = fixture["strategy_id"].as_str().unwrap().to_string();
        let trial_id = fixture["primary"]["trial_id"].as_str().unwrap().to_string();
        let economic_eval_id =
            fixture["primary"]["economic_eval_id"].as_str().unwrap().to_string();

        let summary = run_research_replay_backtest(ResearchReplayArgs {
            registry_db: registry_db.display().to_string(),
            trial_id: trial_id.clone(),
            strategy_id: strategy_id.clone(),
            economic_eval_id: economic_eval_id.clone(),
            replay_work_dir: dir.join("replay_work").display().to_string(),
            out_dir: dir.join("artifacts").display().to_string(),
            research_py_root: research_py_root.display().to_string(),
            python: python.to_string(),
            stress_out_dir: dir.join("stress").display().to_string(),
            placebo_out_dir: dir.join("placebo").display().to_string(),
        })
        .expect("run_research_replay_backtest failed");

        assert_eq!(summary.trial_id, trial_id);
        assert_eq!(summary.strategy_id, strategy_id);
        assert_eq!(summary.economic_eval_id, economic_eval_id);

        let report = mqk_artifacts::load_canonical_backtest_report(&summary.run_dir)
            .expect("load_canonical_backtest_report failed");
        assert_eq!(report.strategy_name, strategy_id, "R2.1: strategy_name == Research strategy_id");

        let gauntlet = mqk_artifacts::load_canonical_robustness_gauntlet(&summary.run_dir)
            .expect("load_canonical_robustness_gauntlet failed");
        assert_eq!(gauntlet.protocol_version, mqk_backtest::ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION);
        assert!(gauntlet.is_complete(), "every required scenario must be present");
        assert_eq!(
            gauntlet.scenarios_run(),
            mqk_backtest::REQUIRED_ROBUSTNESS_SCENARIO_NAMES.len(),
            "exactly the 9 required scenarios, no duplicates, no extras"
        );

        // Research-registry-anchored scenarios bind to the SAME trial this
        // fixture registered -- never a different/unrelated trial.
        assert_eq!(gauntlet.dsr_pbo_sensitivity_research_trial_id(), Some(trial_id.as_str()));
        assert_eq!(
            gauntlet.p7a_p7b_economic_replay_stress_research_trial_id(),
            Some(trial_id.as_str())
        );
        assert_eq!(
            gauntlet.p7a_p7b_economic_replay_stress_baseline_economic_eval_id(),
            Some(economic_eval_id.as_str())
        );

        // See this test's own doc comment: two genuine, deterministic,
        // out-of-scope findings make `all_applicable_passed() == true`
        // infeasible for an honest synthetic fixture in this wave -- the
        // truthful value is asserted here, not fabricated.
        assert!(
            !gauntlet.all_applicable_passed(),
            "expected NOT all-applicable-passed for this fixture (see doc comment); if this now \
             fails, either a real regression was introduced, or the two documented findings have \
             genuinely been resolved and this assertion (and its doc comment) should be updated"
        );

        // R3.5: resolve_backtest_evidence consumes the resulting artifact
        // tree end-to-end through the REAL production evidence-resolution
        // seam (never itself requiring all_applicable_passed()).
        let evidence = mqk_promotion::resolve_backtest_evidence(&dir.join("artifacts"), summary.run_id)
            .expect("resolve_backtest_evidence failed");
        assert_eq!(evidence.robustness_evidence.is_complete, true);
        assert_eq!(evidence.robustness_evidence.all_applicable_passed, false);

        // R3.6: no Paper/Live/OMS/broker call is reachable from this
        // command -- structurally true by this module's own dependency
        // graph (research_replay.rs never imports mqk-broker-*/mqk-runtime/
        // mqk-portfolio), verified here by construction rather than by a
        // runtime probe.
    }
}
