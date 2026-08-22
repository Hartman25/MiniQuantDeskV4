//! P9 `BKT-ROBUSTNESS-GAUNTLET-01` — durable, candidate-bound,
//! tamper-evident artifact for [`mqk_backtest::RobustnessGauntletOutput`].
//!
//! `robustness_gauntlet.json` mirrors [`crate::stress_suite_artifact`]'s
//! design exactly: candidate binding via cross-check against `manifest.json`
//! and an integrity proof via a hash-chained `robustness_gauntlet_completed`
//! audit event carrying the artifact's own SHA-256. See that module's docs
//! for the full rationale, not repeated here.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mqk_audit::{verify_hash_chain_str, AuditEvent, AuditWriter, VerifyResult};
use mqk_backtest::RobustnessGauntletOutput;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::RunManifest;

/// Current schema version of `robustness_gauntlet.json`.
pub const ROBUSTNESS_GAUNTLET_ARTIFACT_SCHEMA_VERSION: u32 = 1;

const ROBUSTNESS_GAUNTLET_AUDIT_EVENT_TYPE: &str = "robustness_gauntlet_completed";
/// BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01: appended by
/// `finalize_canonical_robustness_gauntlet_with_sensitivity` when it merges
/// a separately-computed `dsr_pbo_sensitivity` scenario into an already-real
/// artifact. Distinct from [`ROBUSTNESS_GAUNTLET_AUDIT_EVENT_TYPE`] (never
/// reused for the same event type) because the artifact's content genuinely
/// changes on finalization -- reusing the original event type would make
/// its OWN recorded hash disagree with the new file content the next time
/// the artifact is loaded.
const ROBUSTNESS_GAUNTLET_FINALIZED_AUDIT_EVENT_TYPE: &str = "robustness_gauntlet_finalized";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RobustnessScenarioOutcomeDto {
    name: String,
    applicable: bool,
    passed: bool,
    reason: Option<String>,
    detail: String,
    /// PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01: durable mirror of
    /// [`mqk_backtest::RobustnessScenarioOutcome::research_trial_id`].
    /// `#[serde(default)]` so an artifact written before this field existed
    /// still loads (as `None`) instead of failing malformed-JSON parsing --
    /// [`RobustnessGauntletArtifact::dsr_pbo_sensitivity_research_trial_id`]
    /// then correctly reports "no binding proof" for that legacy artifact,
    /// which the promotion gate treats as fail-closed exactly like a
    /// genuine mismatch.
    #[serde(default)]
    research_trial_id: Option<String>,
}

impl From<&mqk_backtest::RobustnessScenarioOutcome> for RobustnessScenarioOutcomeDto {
    fn from(s: &mqk_backtest::RobustnessScenarioOutcome) -> Self {
        Self {
            name: s.name.clone(),
            applicable: s.applicable,
            passed: s.passed,
            reason: s.reason.clone(),
            detail: s.detail.clone(),
            research_trial_id: s.research_trial_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct DeferredScenarioDto {
    name: String,
    reason: String,
}

impl From<&mqk_backtest::DeferredScenario> for DeferredScenarioDto {
    fn from(d: &mqk_backtest::DeferredScenario) -> Self {
        Self {
            name: d.name.clone(),
            reason: d.reason.clone(),
        }
    }
}

/// Durable, schema-versioned mirror of [`RobustnessGauntletOutput`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RobustnessGauntletArtifact {
    pub schema_version: u32,
    pub protocol_version: String,
    pub run_id: Uuid,
    pub config_id: Uuid,
    pub strategy_name: String,
    scenarios: Vec<RobustnessScenarioOutcomeDto>,
    deferred: Vec<DeferredScenarioDto>,
}

impl RobustnessGauntletArtifact {
    pub fn scenarios_run(&self) -> usize {
        self.scenarios.len()
    }

    /// True iff every applicable scenario passed, and at least one scenario
    /// was applicable.
    pub fn all_applicable_passed(&self) -> bool {
        let applicable: Vec<&RobustnessScenarioOutcomeDto> =
            self.scenarios.iter().filter(|s| s.applicable).collect();
        !applicable.is_empty() && applicable.iter().all(|s| s.passed)
    }

    pub fn failed_scenario_descriptions(&self) -> Vec<String> {
        self.scenarios
            .iter()
            .filter(|s| s.applicable && !s.passed)
            .map(|s| {
                format!(
                    "{}: {}",
                    s.name,
                    s.reason.as_deref().unwrap_or("failed (no reason recorded)")
                )
            })
            .collect()
    }

    pub fn deferred_scenario_names(&self) -> Vec<String> {
        self.deferred.iter().map(|d| d.name.clone()).collect()
    }

    /// PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01: the exact Research
    /// `trial_id` the `dsr_pbo_sensitivity` scenario's evidence was computed
    /// against, if that scenario is present and carries one. `None` when the
    /// scenario is absent/deferred, or when it was recorded before this
    /// field existed -- either way, "no binding proof", never a bare claim
    /// promotion can rely on.
    pub fn dsr_pbo_sensitivity_research_trial_id(&self) -> Option<&str> {
        self.scenarios
            .iter()
            .find(|s| s.name == mqk_backtest::DSR_PBO_SENSITIVITY_SCENARIO_NAME)
            .and_then(|s| s.research_trial_id.as_deref())
    }

    /// Mirrors `mqk_backtest::RobustnessGauntletOutput::is_complete`: true
    /// iff `deferred` is empty and every
    /// `mqk_backtest::REQUIRED_ROBUSTNESS_SCENARIO_NAMES` entry is present
    /// in `scenarios` by name. A structurally valid (protocol-checked,
    /// hash-verified) but INCOMPLETE artifact -- e.g. written before
    /// `dsr_pbo_sensitivity` was merged in -- still loads successfully;
    /// completeness is a separate, explicit check callers gating promotion
    /// on P9 evidence must make themselves (mirrors this artifact-vs-policy
    /// separation already used by `stress_suite_artifact`/`evaluate_promotion`).
    pub fn is_complete(&self) -> bool {
        if !self.deferred.is_empty() {
            return false;
        }
        let present: std::collections::BTreeSet<&str> =
            self.scenarios.iter().map(|s| s.name.as_str()).collect();
        mqk_backtest::REQUIRED_ROBUSTNESS_SCENARIO_NAMES
            .iter()
            .all(|required| present.contains(required))
    }
}

impl From<&RobustnessGauntletOutput> for RobustnessGauntletArtifact {
    fn from(o: &RobustnessGauntletOutput) -> Self {
        Self {
            schema_version: ROBUSTNESS_GAUNTLET_ARTIFACT_SCHEMA_VERSION,
            protocol_version: o.protocol_version.clone(),
            run_id: o.run_id,
            config_id: o.config_id,
            strategy_name: o.strategy_name.clone(),
            scenarios: o.scenarios.iter().map(RobustnessScenarioOutcomeDto::from).collect(),
            deferred: o.deferred.iter().map(DeferredScenarioDto::from).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RobustnessGauntletArtifactError {
    ManifestMissing,
    ManifestParse(String),
    MissingArtifact,
    MalformedJson(String),
    UnsupportedSchemaVersion(u32),
    /// `protocol_version` does not equal
    /// [`mqk_backtest::ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION`] -- an
    /// artifact produced under a different (older, newer, or fabricated)
    /// gauntlet protocol can never satisfy the current protocol's evidence
    /// requirement. PROMOTION-STRESS-AUTHORITY-REPAIR-01 wave / P9
    /// completion repair.
    UnsupportedProtocolVersion(String),
    ZeroScenarios,
    RunIdMismatch { manifest: Uuid, artifact: Uuid },
    StrategyNameMismatch { manifest: String, artifact: String },
    ConfigIdMismatch { manifest_config_hash: String, artifact_config_id: String },
    AuditEventMissing,
    AuditChainBroken(String),
    ContentHashMismatch,
}

impl std::fmt::Display for RobustnessGauntletArtifactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ManifestMissing => write!(f, "manifest.json missing"),
            Self::ManifestParse(e) => write!(f, "manifest.json parse failed: {e}"),
            Self::MissingArtifact => write!(f, "robustness_gauntlet.json missing"),
            Self::MalformedJson(e) => write!(f, "robustness_gauntlet.json parse failed: {e}"),
            Self::UnsupportedSchemaVersion(v) => {
                write!(f, "robustness_gauntlet.json schema_version {v} unsupported")
            }
            Self::UnsupportedProtocolVersion(v) => write!(
                f,
                "robustness_gauntlet.json protocol_version {v:?} is not the required P9 protocol"
            ),
            Self::ZeroScenarios => write!(f, "robustness_gauntlet.json recorded zero scenarios"),
            Self::RunIdMismatch { manifest, artifact } => write!(
                f,
                "run_id mismatch: manifest={manifest} robustness_gauntlet.json={artifact}"
            ),
            Self::StrategyNameMismatch { manifest, artifact } => write!(
                f,
                "strategy_name mismatch: manifest={manifest:?} robustness_gauntlet.json={artifact:?}"
            ),
            Self::ConfigIdMismatch {
                manifest_config_hash,
                artifact_config_id,
            } => write!(
                f,
                "config identity mismatch: manifest.config_hash={manifest_config_hash:?} robustness_gauntlet.json.config_id={artifact_config_id:?}"
            ),
            Self::AuditEventMissing => write!(
                f,
                "audit.jsonl has no '{ROBUSTNESS_GAUNTLET_AUDIT_EVENT_TYPE}' event"
            ),
            Self::AuditChainBroken(reason) => write!(f, "audit hash chain broken: {reason}"),
            Self::ContentHashMismatch => write!(
                f,
                "robustness_gauntlet.json content hash disagrees with the audited hash (tampered after write)"
            ),
        }
    }
}

impl std::error::Error for RobustnessGauntletArtifactError {}

// ---------------------------------------------------------------------------
// Write
// ---------------------------------------------------------------------------

/// Write `robustness_gauntlet.json` and append its integrity-proof audit
/// event. Must be called after the run's `audit.jsonl` already carries at
/// least the backtest completion event -- resumes the existing hash chain,
/// never starts a new one. Idempotent: a second call is a no-op if
/// `audit.jsonl` already carries a `robustness_gauntlet_completed` event.
pub fn write_canonical_robustness_gauntlet(
    run_dir: &Path,
    output: &RobustnessGauntletOutput,
) -> Result<PathBuf> {
    fs::create_dir_all(run_dir)
        .with_context(|| format!("create run dir failed: {}", run_dir.display()))?;
    let artifact = RobustnessGauntletArtifact::from(output);
    let json = serde_json::to_string_pretty(&artifact)
        .context("serialize robustness_gauntlet.json failed")?;
    let contents = format!("{json}\n");
    let path = run_dir.join("robustness_gauntlet.json");
    fs::write(&path, &contents)
        .with_context(|| format!("write robustness_gauntlet.json failed: {}", path.display()))?;

    let audit_path = run_dir.join("audit.jsonl");
    let existing = fs::read_to_string(&audit_path).unwrap_or_default();
    let lines: Vec<&str> = existing.lines().filter(|l| !l.trim().is_empty()).collect();

    for line in &lines {
        if let Ok(ev) = serde_json::from_str::<AuditEvent>(line) {
            if ev.event_type == ROBUSTNESS_GAUNTLET_AUDIT_EVENT_TYPE {
                return Ok(path); // already recorded -- idempotent no-op
            }
        }
    }

    let mut writer = AuditWriter::new(&audit_path, true)
        .with_context(|| format!("open audit writer failed: {}", audit_path.display()))?;
    if let Some(last_line) = lines.last() {
        let last_ev: AuditEvent = serde_json::from_str(last_line)
            .context("parse last audit event for chain resume failed")?;
        writer.set_last_hash(last_ev.hash_self);
        writer.set_seq(lines.len() as u64);
    }

    let mut hasher = Sha256::new();
    hasher.update(contents.as_bytes());
    let robustness_gauntlet_sha256 = hex::encode(hasher.finalize());

    let payload = serde_json::json!({
        "run_id": output.run_id,
        "config_id": output.config_id,
        "strategy_name": output.strategy_name,
        "protocol_version": output.protocol_version,
        "scenarios_run": output.scenarios.len(),
        "all_applicable_passed": output.all_applicable_passed(),
        "robustness_gauntlet_sha256": robustness_gauntlet_sha256,
        "robustness_gauntlet_schema_version": ROBUSTNESS_GAUNTLET_ARTIFACT_SCHEMA_VERSION,
    });

    writer
        .append(output.run_id, "backtest", ROBUSTNESS_GAUNTLET_AUDIT_EVENT_TYPE, payload)
        .with_context(|| {
            format!(
                "append robustness gauntlet completion audit event failed: {}",
                audit_path.display()
            )
        })?;

    Ok(path)
}

// ---------------------------------------------------------------------------
// Finalize (BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01)
// ---------------------------------------------------------------------------

/// Every way [`finalize_canonical_robustness_gauntlet_with_sensitivity`]
/// fails closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RobustnessGauntletFinalizeError {
    /// The existing artifact must already structurally validate (protocol,
    /// hash, candidate binding) before it can be finalized -- finalization
    /// only ADDS evidence to an already-real artifact.
    ExistingArtifactInvalid(RobustnessGauntletArtifactError),
    /// `sensitivity.name` was not
    /// [`mqk_backtest::DSR_PBO_SENSITIVITY_SCENARIO_NAME`] -- this seam
    /// only ever merges that one scenario; any other name is a caller bug,
    /// never silently accepted.
    WrongScenarioName(String),
    /// A `dsr_pbo_sensitivity` scenario is ALREADY present on this artifact
    /// with DIFFERENT content -- refusing to silently overwrite real,
    /// already-durable evidence (mirrors PROMOTION-LINEAGE-ATOMICITY-01's
    /// own mismatched-retry-refusal pattern).
    ConflictingSensitivityResult,
    /// A filesystem or serialization failure while writing the updated
    /// artifact or its audit event. An error here means the finalization
    /// did not complete -- see this function's own docs for exactly what
    /// state that leaves on disk.
    Io(String),
}

impl std::fmt::Display for RobustnessGauntletFinalizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExistingArtifactInvalid(e) => {
                write!(f, "existing robustness_gauntlet.json invalid: {e}")
            }
            Self::WrongScenarioName(name) => write!(
                f,
                "finalize_canonical_robustness_gauntlet_with_sensitivity only merges \
                 dsr_pbo_sensitivity, got scenario name {name:?}"
            ),
            Self::ConflictingSensitivityResult => write!(
                f,
                "a dsr_pbo_sensitivity scenario is already finalized on this artifact with \
                 different content -- refusing to overwrite"
            ),
            Self::Io(e) => write!(f, "finalize I/O failed: {e}"),
        }
    }
}

impl std::error::Error for RobustnessGauntletFinalizeError {}

/// Merge a separately-computed `dsr_pbo_sensitivity` scenario
/// (`mqk_backtest::dsr_pbo_sensitivity_scenario`) into an EXISTING, already-
/// real `robustness_gauntlet.json` (written by
/// [`write_canonical_robustness_gauntlet`] immediately after the real
/// backtest run), and durably re-record the merged artifact's new content
/// hash via a SECOND, distinct, hash-chained audit event
/// ([`ROBUSTNESS_GAUNTLET_FINALIZED_AUDIT_EVENT_TYPE`]) -- the original
/// `robustness_gauntlet_completed` event is never modified or removed, so
/// the artifact's full history (pre- and post-finalization) stays
/// auditable.
///
/// # Interrupted finalization
///
/// If this function is interrupted after writing the updated JSON but
/// before the new audit event is appended, [`load_canonical_robustness_gauntlet`]
/// will detect that the file's actual content hash no longer matches the
/// latest recorded audit hash and fail closed with `ContentHashMismatch` --
/// there is no window in which a partially-finalized artifact reads back as
/// valid, let alone complete.
///
/// # Idempotency
///
/// If a `dsr_pbo_sensitivity` scenario with IDENTICAL content is already
/// present, this is a no-op (`Ok`, no new write, no new audit event). A
/// DIFFERENT `dsr_pbo_sensitivity` scenario for an already-finalized
/// artifact is refused (`ConflictingSensitivityResult`).
pub fn finalize_canonical_robustness_gauntlet_with_sensitivity(
    run_dir: &Path,
    sensitivity: &mqk_backtest::RobustnessScenarioOutcome,
) -> Result<PathBuf, RobustnessGauntletFinalizeError> {
    if sensitivity.name != mqk_backtest::DSR_PBO_SENSITIVITY_SCENARIO_NAME {
        return Err(RobustnessGauntletFinalizeError::WrongScenarioName(
            sensitivity.name.clone(),
        ));
    }

    let existing = load_canonical_robustness_gauntlet(run_dir)
        .map_err(RobustnessGauntletFinalizeError::ExistingArtifactInvalid)?;
    let path = run_dir.join("robustness_gauntlet.json");

    if let Some(prev) = existing
        .scenarios
        .iter()
        .find(|s| s.name == mqk_backtest::DSR_PBO_SENSITIVITY_SCENARIO_NAME)
    {
        let prev_matches = prev.applicable == sensitivity.applicable
            && prev.passed == sensitivity.passed
            && prev.reason == sensitivity.reason
            && prev.detail == sensitivity.detail
            // PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01: two finalizations
            // with coincidentally identical applicable/passed/reason/detail
            // but a DIFFERENT research_trial_id are never the "same" replay
            // -- silently keeping the first trial's id would defeat the
            // whole point of binding this evidence to a specific trial.
            && prev.research_trial_id == sensitivity.research_trial_id;
        if prev_matches {
            return Ok(path); // idempotent replay -- already finalized identically
        }
        return Err(RobustnessGauntletFinalizeError::ConflictingSensitivityResult);
    }

    // Merge: existing scenarios + the new one, dropping the matching
    // deferred entry -- mirrors
    // RobustnessGauntletOutput::merge_dsr_pbo_sensitivity exactly, at the
    // artifact level.
    let mut merged = existing;
    merged.deferred.retain(|d| d.name != sensitivity.name);
    merged.scenarios.push(RobustnessScenarioOutcomeDto::from(sensitivity));

    let json = serde_json::to_string_pretty(&merged)
        .map_err(|e| RobustnessGauntletFinalizeError::Io(e.to_string()))?;
    let contents = format!("{json}\n");
    fs::write(&path, &contents)
        .map_err(|e| RobustnessGauntletFinalizeError::Io(e.to_string()))?;

    let audit_path = run_dir.join("audit.jsonl");
    let audit_raw = fs::read_to_string(&audit_path).unwrap_or_default();
    let lines: Vec<&str> = audit_raw.lines().filter(|l| !l.trim().is_empty()).collect();

    let mut writer = AuditWriter::new(&audit_path, true)
        .map_err(|e| RobustnessGauntletFinalizeError::Io(e.to_string()))?;
    if let Some(last_line) = lines.last() {
        let last_ev: AuditEvent = serde_json::from_str(last_line)
            .map_err(|e: serde_json::Error| RobustnessGauntletFinalizeError::Io(e.to_string()))?;
        writer.set_last_hash(last_ev.hash_self);
        writer.set_seq(lines.len() as u64);
    }

    let mut hasher = Sha256::new();
    hasher.update(contents.as_bytes());
    let robustness_gauntlet_sha256 = hex::encode(hasher.finalize());

    let payload = serde_json::json!({
        "run_id": merged.run_id,
        "config_id": merged.config_id,
        "strategy_name": merged.strategy_name,
        "protocol_version": merged.protocol_version,
        "scenarios_run": merged.scenarios.len(),
        "all_applicable_passed": merged.all_applicable_passed(),
        "is_complete": merged.is_complete(),
        // PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01: explicit in the
        // audit payload (not merely inside the hashed JSON body) so the
        // exact Research trial this finalization bound is directly
        // queryable from audit.jsonl without loading/parsing the artifact.
        "dsr_pbo_sensitivity_research_trial_id": sensitivity.research_trial_id,
        "robustness_gauntlet_sha256": robustness_gauntlet_sha256,
        "robustness_gauntlet_schema_version": ROBUSTNESS_GAUNTLET_ARTIFACT_SCHEMA_VERSION,
    });

    writer
        .append(
            merged.run_id,
            "backtest",
            ROBUSTNESS_GAUNTLET_FINALIZED_AUDIT_EVENT_TYPE,
            payload,
        )
        .map_err(|e| RobustnessGauntletFinalizeError::Io(e.to_string()))?;

    Ok(path)
}

// ---------------------------------------------------------------------------
// Load
// ---------------------------------------------------------------------------

pub fn load_canonical_robustness_gauntlet(
    run_dir: &Path,
) -> Result<RobustnessGauntletArtifact, RobustnessGauntletArtifactError> {
    let manifest_path = run_dir.join("manifest.json");
    if !manifest_path.exists() {
        return Err(RobustnessGauntletArtifactError::ManifestMissing);
    }
    let manifest_raw = fs::read_to_string(&manifest_path)
        .map_err(|e| RobustnessGauntletArtifactError::ManifestParse(e.to_string()))?;
    let manifest: RunManifest = serde_json::from_str(&manifest_raw)
        .map_err(|e| RobustnessGauntletArtifactError::ManifestParse(e.to_string()))?;

    let artifact_path = run_dir.join("robustness_gauntlet.json");
    if !artifact_path.exists() {
        return Err(RobustnessGauntletArtifactError::MissingArtifact);
    }
    let artifact_bytes = fs::read(&artifact_path)
        .map_err(|e| RobustnessGauntletArtifactError::MalformedJson(e.to_string()))?;
    let artifact: RobustnessGauntletArtifact = serde_json::from_slice(&artifact_bytes)
        .map_err(|e| RobustnessGauntletArtifactError::MalformedJson(e.to_string()))?;

    if artifact.schema_version != ROBUSTNESS_GAUNTLET_ARTIFACT_SCHEMA_VERSION {
        return Err(RobustnessGauntletArtifactError::UnsupportedSchemaVersion(
            artifact.schema_version,
        ));
    }
    if artifact.scenarios.is_empty() {
        return Err(RobustnessGauntletArtifactError::ZeroScenarios);
    }
    if artifact.protocol_version != mqk_backtest::ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION {
        return Err(RobustnessGauntletArtifactError::UnsupportedProtocolVersion(
            artifact.protocol_version.clone(),
        ));
    }
    if artifact.run_id != manifest.run_id {
        return Err(RobustnessGauntletArtifactError::RunIdMismatch {
            manifest: manifest.run_id,
            artifact: artifact.run_id,
        });
    }
    if artifact.strategy_name != manifest.strategy_name {
        return Err(RobustnessGauntletArtifactError::StrategyNameMismatch {
            manifest: manifest.strategy_name,
            artifact: artifact.strategy_name,
        });
    }
    let artifact_config_id = artifact.config_id.to_string();
    if artifact_config_id != manifest.config_hash {
        return Err(RobustnessGauntletArtifactError::ConfigIdMismatch {
            manifest_config_hash: manifest.config_hash,
            artifact_config_id,
        });
    }

    let audit_path = run_dir.join("audit.jsonl");
    let audit_raw = fs::read_to_string(&audit_path).unwrap_or_default();
    match verify_hash_chain_str(&audit_raw) {
        Ok(VerifyResult::Valid { .. }) => {}
        Ok(VerifyResult::Broken { line, reason }) => {
            return Err(RobustnessGauntletArtifactError::AuditChainBroken(format!(
                "line {line}: {reason}"
            )))
        }
        Err(e) => return Err(RobustnessGauntletArtifactError::AuditChainBroken(e.to_string())),
    }

    let mut hasher = Sha256::new();
    hasher.update(&artifact_bytes);
    let actual_sha256 = hex::encode(hasher.finalize());

    // BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01: a `_finalized` event
    // (if present) always comes AFTER the original `_completed` event in
    // file order and describes the CURRENT file content -- so the LATEST
    // matching event (of either type) is authoritative, not "every matching
    // event must agree" (which `_completed` alone could never satisfy once
    // finalization has genuinely changed the file). `_completed` must still
    // be present somewhere -- a `_finalized` event can never stand alone.
    let mut found_completed = false;
    let mut latest_recorded_hash: Option<String> = None;
    for line in audit_raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let ev: AuditEvent = match serde_json::from_str(trimmed) {
            Ok(ev) => ev,
            Err(_) => continue,
        };
        match ev.event_type.as_str() {
            ROBUSTNESS_GAUNTLET_AUDIT_EVENT_TYPE => {
                found_completed = true;
                latest_recorded_hash = ev
                    .payload
                    .get("robustness_gauntlet_sha256")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
            ROBUSTNESS_GAUNTLET_FINALIZED_AUDIT_EVENT_TYPE => {
                latest_recorded_hash = ev
                    .payload
                    .get("robustness_gauntlet_sha256")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
            _ => {}
        }
    }
    if !found_completed {
        return Err(RobustnessGauntletArtifactError::AuditEventMissing);
    }
    if latest_recorded_hash.as_deref() != Some(actual_sha256.as_str()) {
        return Err(RobustnessGauntletArtifactError::ContentHashMismatch);
    }

    Ok(artifact)
}
