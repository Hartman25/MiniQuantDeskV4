//! PROMOTION-STRESS-SUITE-AUTHORITY-01 — durable, candidate-bound,
//! tamper-evident artifact for [`mqk_backtest::StressSuiteRunOutput`].
//!
//! `stress_suite.json` is the durable, schema-versioned record of a real
//! [`mqk_backtest::run_backtest_stress_suite`] execution. Two independent
//! layers of protection back [`load_canonical_stress_suite`]:
//!
//! 1. **Candidate binding** — `run_id`/`strategy_name`/`config_id` are
//!    cross-checked against the run's `manifest.json`, exactly like
//!    [`crate::backtest_report_artifact`]. A stress result written for one
//!    candidate can never be loaded as evidence for a different one.
//! 2. **Integrity proof** — writing the artifact also appends a
//!    `stress_suite_completed` event to the run's hash-chained
//!    `audit.jsonl`, carrying the artifact's own SHA-256. Loading
//!    recomputes that hash from the on-disk file and requires it to match
//!    the value recorded in the (separately tamper-evident, hash-chained)
//!    audit event — hand-editing `stress_suite.json` alone, without also
//!    defeating the audit hash chain, is detected and rejected.
//!
//! `mqk-artifacts` cannot depend on `mqk-promotion` (the reverse dependency
//! already exists), so this module returns the artifact-level
//! [`StressSuiteArtifact`] type rather than `mqk_promotion::StressSuiteResult`
//! directly; bridging the two is the evidence resolver's job
//! (`PROMOTION-BACKTEST-EVIDENCE-SEAM-01`).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mqk_audit::{verify_hash_chain_str, AuditEvent, AuditWriter, VerifyResult};
use mqk_backtest::StressSuiteRunOutput;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::RunManifest;

/// Current schema version of `stress_suite.json`.
pub const STRESS_SUITE_ARTIFACT_SCHEMA_VERSION: u32 = 1;

/// The audit event type recorded by [`write_canonical_stress_suite`] and
/// required by [`load_canonical_stress_suite`].
const STRESS_SUITE_AUDIT_EVENT_TYPE: &str = "stress_suite_completed";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StressScenarioOutcomeDto {
    name: String,
    passed: bool,
    reason: Option<String>,
    final_equity_micros: i64,
    max_drawdown_fraction: f64,
}

impl From<&mqk_backtest::StressScenarioOutcome> for StressScenarioOutcomeDto {
    fn from(s: &mqk_backtest::StressScenarioOutcome) -> Self {
        Self {
            name: s.name.clone(),
            passed: s.passed,
            reason: s.reason.clone(),
            final_equity_micros: s.final_equity_micros,
            max_drawdown_fraction: s.max_drawdown_fraction,
        }
    }
}

/// Durable, schema-versioned mirror of [`StressSuiteRunOutput`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StressSuiteArtifact {
    pub schema_version: u32,
    pub protocol_version: String,
    pub run_id: Uuid,
    pub config_id: Uuid,
    pub strategy_name: String,
    scenarios: Vec<StressScenarioOutcomeDto>,
}

impl StressSuiteArtifact {
    /// Number of scenarios recorded. Never trust a zero-scenario artifact as
    /// evidence -- see [`StressSuiteArtifactError::ZeroScenarios`].
    pub fn scenarios_run(&self) -> usize {
        self.scenarios.len()
    }

    /// True iff every recorded scenario passed (and at least one ran).
    pub fn all_passed(&self) -> bool {
        !self.scenarios.is_empty() && self.scenarios.iter().all(|s| s.passed)
    }

    /// Number of recorded scenarios that passed.
    pub fn scenarios_passed(&self) -> usize {
        self.scenarios.iter().filter(|s| s.passed).count()
    }

    /// Human-readable descriptions of failed scenarios (empty when all passed).
    pub fn failed_scenario_descriptions(&self) -> Vec<String> {
        self.scenarios
            .iter()
            .filter(|s| !s.passed)
            .map(|s| {
                format!(
                    "{}: {}",
                    s.name,
                    s.reason.as_deref().unwrap_or("failed (no reason recorded)")
                )
            })
            .collect()
    }
}

impl From<&StressSuiteRunOutput> for StressSuiteArtifact {
    fn from(o: &StressSuiteRunOutput) -> Self {
        Self {
            schema_version: STRESS_SUITE_ARTIFACT_SCHEMA_VERSION,
            protocol_version: o.protocol_version.clone(),
            run_id: o.run_id,
            config_id: o.config_id,
            strategy_name: o.strategy_name.clone(),
            scenarios: o.scenarios.iter().map(StressScenarioOutcomeDto::from).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StressSuiteArtifactError {
    ManifestMissing,
    ManifestParse(String),
    /// `stress_suite.json` is missing from the run directory.
    MissingArtifact,
    MalformedJson(String),
    UnsupportedSchemaVersion(u32),
    /// `protocol_version` does not equal
    /// [`mqk_backtest::STRESS_SUITE_PROTOCOL_VERSION`] -- an artifact
    /// produced under a different (older, newer, or fabricated) stress
    /// protocol can never satisfy the current protocol's evidence
    /// requirement. PROMOTION-STRESS-AUTHORITY-REPAIR-01.
    UnsupportedProtocolVersion(String),
    /// The artifact's `protocol_version` matched, but at least one scenario
    /// name required by [`mqk_backtest::REQUIRED_SCENARIO_NAMES`] is absent
    /// -- an incomplete scenario set can never satisfy that protocol's
    /// evidence requirement, regardless of how many other scenarios ran.
    /// PROMOTION-STRESS-AUTHORITY-REPAIR-01.
    MissingRequiredScenario(String),
    /// The artifact recorded zero scenarios -- never valid evidence.
    ZeroScenarios,
    RunIdMismatch { manifest: Uuid, artifact: Uuid },
    StrategyNameMismatch { manifest: String, artifact: String },
    ConfigIdMismatch { manifest_config_hash: String, artifact_config_id: String },
    /// `audit.jsonl` does not contain a `stress_suite_completed` event, or
    /// is missing entirely.
    AuditEventMissing,
    /// The audit hash chain itself is broken (tamper anywhere in the file).
    AuditChainBroken(String),
    /// The `stress_suite_completed` event's recorded SHA-256 disagrees with
    /// the SHA-256 of the actual on-disk `stress_suite.json` -- the file was
    /// edited after the audit event was written.
    ContentHashMismatch,
}

impl std::fmt::Display for StressSuiteArtifactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ManifestMissing => write!(f, "manifest.json missing"),
            Self::ManifestParse(e) => write!(f, "manifest.json parse failed: {e}"),
            Self::MissingArtifact => write!(f, "stress_suite.json missing"),
            Self::MalformedJson(e) => write!(f, "stress_suite.json parse failed: {e}"),
            Self::UnsupportedSchemaVersion(v) => {
                write!(f, "stress_suite.json schema_version {v} unsupported")
            }
            Self::UnsupportedProtocolVersion(v) => write!(
                f,
                "stress_suite.json protocol_version {v:?} is not the required production stress protocol"
            ),
            Self::MissingRequiredScenario(name) => write!(
                f,
                "stress_suite.json is missing required scenario {name:?}"
            ),
            Self::ZeroScenarios => write!(f, "stress_suite.json recorded zero scenarios"),
            Self::RunIdMismatch { manifest, artifact } => write!(
                f,
                "run_id mismatch: manifest={manifest} stress_suite.json={artifact}"
            ),
            Self::StrategyNameMismatch { manifest, artifact } => write!(
                f,
                "strategy_name mismatch: manifest={manifest:?} stress_suite.json={artifact:?}"
            ),
            Self::ConfigIdMismatch {
                manifest_config_hash,
                artifact_config_id,
            } => write!(
                f,
                "config identity mismatch: manifest.config_hash={manifest_config_hash:?} stress_suite.json.config_id={artifact_config_id:?}"
            ),
            Self::AuditEventMissing => write!(
                f,
                "audit.jsonl has no '{STRESS_SUITE_AUDIT_EVENT_TYPE}' event"
            ),
            Self::AuditChainBroken(reason) => write!(f, "audit hash chain broken: {reason}"),
            Self::ContentHashMismatch => write!(
                f,
                "stress_suite.json content hash disagrees with the audited hash (tampered after write)"
            ),
        }
    }
}

impl std::error::Error for StressSuiteArtifactError {}

// ---------------------------------------------------------------------------
// Write
// ---------------------------------------------------------------------------

/// Write `stress_suite.json` and append its integrity-proof audit event.
///
/// Must be called after the run's `audit.jsonl` already carries the
/// backtest completion event (i.e. after
/// [`crate::write_backtest_report`]) -- this function resumes the existing
/// hash chain rather than starting a new one, and never truncates or
/// reorders prior events.
///
/// Idempotent: if `audit.jsonl` already carries a `stress_suite_completed`
/// event, this is a no-op (the file is still (re)written, but no second,
/// chain-breaking event is appended).
pub fn write_canonical_stress_suite(
    run_dir: &Path,
    output: &StressSuiteRunOutput,
) -> Result<PathBuf> {
    fs::create_dir_all(run_dir)
        .with_context(|| format!("create run dir failed: {}", run_dir.display()))?;
    let artifact = StressSuiteArtifact::from(output);
    let json = serde_json::to_string_pretty(&artifact)
        .context("serialize stress_suite.json failed")?;
    let contents = format!("{json}\n");
    let path = run_dir.join("stress_suite.json");
    fs::write(&path, &contents)
        .with_context(|| format!("write stress_suite.json failed: {}", path.display()))?;

    let audit_path = run_dir.join("audit.jsonl");
    let existing = fs::read_to_string(&audit_path).unwrap_or_default();
    let lines: Vec<&str> = existing.lines().filter(|l| !l.trim().is_empty()).collect();

    for line in &lines {
        if let Ok(ev) = serde_json::from_str::<AuditEvent>(line) {
            if ev.event_type == STRESS_SUITE_AUDIT_EVENT_TYPE {
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
    let stress_suite_sha256 = hex::encode(hasher.finalize());

    let payload = serde_json::json!({
        "run_id": output.run_id,
        "config_id": output.config_id,
        "strategy_name": output.strategy_name,
        "protocol_version": output.protocol_version,
        "scenarios_run": output.scenarios.len(),
        "all_passed": output.all_passed(),
        "stress_suite_sha256": stress_suite_sha256,
        "stress_suite_schema_version": STRESS_SUITE_ARTIFACT_SCHEMA_VERSION,
    });

    writer
        .append(output.run_id, "backtest", STRESS_SUITE_AUDIT_EVENT_TYPE, payload)
        .with_context(|| {
            format!(
                "append stress suite completion audit event failed: {}",
                audit_path.display()
            )
        })?;

    Ok(path)
}

// ---------------------------------------------------------------------------
// Load
// ---------------------------------------------------------------------------

/// Load and validate the canonical stress suite result for `run_dir`.
///
/// Fail-closed on: missing manifest/artifact, malformed JSON, unsupported
/// schema version, zero scenarios, any candidate-binding mismatch against
/// `manifest.json`, a broken audit hash chain, a missing
/// `stress_suite_completed` audit event, or a content-hash disagreement
/// between that event and the actual on-disk `stress_suite.json`.
pub fn load_canonical_stress_suite(
    run_dir: &Path,
) -> Result<StressSuiteArtifact, StressSuiteArtifactError> {
    let manifest_path = run_dir.join("manifest.json");
    if !manifest_path.exists() {
        return Err(StressSuiteArtifactError::ManifestMissing);
    }
    let manifest_raw = fs::read_to_string(&manifest_path)
        .map_err(|e| StressSuiteArtifactError::ManifestParse(e.to_string()))?;
    let manifest: RunManifest = serde_json::from_str(&manifest_raw)
        .map_err(|e| StressSuiteArtifactError::ManifestParse(e.to_string()))?;

    let artifact_path = run_dir.join("stress_suite.json");
    if !artifact_path.exists() {
        return Err(StressSuiteArtifactError::MissingArtifact);
    }
    let artifact_bytes = fs::read(&artifact_path)
        .map_err(|e| StressSuiteArtifactError::MalformedJson(e.to_string()))?;
    let artifact: StressSuiteArtifact = serde_json::from_slice(&artifact_bytes)
        .map_err(|e| StressSuiteArtifactError::MalformedJson(e.to_string()))?;

    if artifact.schema_version != STRESS_SUITE_ARTIFACT_SCHEMA_VERSION {
        return Err(StressSuiteArtifactError::UnsupportedSchemaVersion(
            artifact.schema_version,
        ));
    }
    if artifact.scenarios.is_empty() {
        return Err(StressSuiteArtifactError::ZeroScenarios);
    }
    if artifact.protocol_version != mqk_backtest::STRESS_SUITE_PROTOCOL_VERSION {
        return Err(StressSuiteArtifactError::UnsupportedProtocolVersion(
            artifact.protocol_version.clone(),
        ));
    }
    let present_names: std::collections::BTreeSet<&str> =
        artifact.scenarios.iter().map(|s| s.name.as_str()).collect();
    for required in mqk_backtest::REQUIRED_SCENARIO_NAMES {
        if !present_names.contains(required) {
            return Err(StressSuiteArtifactError::MissingRequiredScenario(
                required.to_string(),
            ));
        }
    }
    if artifact.run_id != manifest.run_id {
        return Err(StressSuiteArtifactError::RunIdMismatch {
            manifest: manifest.run_id,
            artifact: artifact.run_id,
        });
    }
    if artifact.strategy_name != manifest.strategy_name {
        return Err(StressSuiteArtifactError::StrategyNameMismatch {
            manifest: manifest.strategy_name,
            artifact: artifact.strategy_name,
        });
    }
    let artifact_config_id = artifact.config_id.to_string();
    if artifact_config_id != manifest.config_hash {
        return Err(StressSuiteArtifactError::ConfigIdMismatch {
            manifest_config_hash: manifest.config_hash,
            artifact_config_id,
        });
    }

    // Integrity proof: audit chain must verify AND must contain a
    // stress_suite_completed event whose recorded hash matches the actual
    // on-disk bytes.
    let audit_path = run_dir.join("audit.jsonl");
    let audit_raw = fs::read_to_string(&audit_path).unwrap_or_default();
    match verify_hash_chain_str(&audit_raw) {
        Ok(VerifyResult::Valid { .. }) => {}
        Ok(VerifyResult::Broken { line, reason }) => {
            return Err(StressSuiteArtifactError::AuditChainBroken(format!(
                "line {line}: {reason}"
            )))
        }
        Err(e) => return Err(StressSuiteArtifactError::AuditChainBroken(e.to_string())),
    }

    let mut hasher = Sha256::new();
    hasher.update(&artifact_bytes);
    let actual_sha256 = hex::encode(hasher.finalize());

    let mut found = false;
    for line in audit_raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let ev: AuditEvent = match serde_json::from_str(trimmed) {
            Ok(ev) => ev,
            Err(_) => continue,
        };
        if ev.event_type != STRESS_SUITE_AUDIT_EVENT_TYPE {
            continue;
        }
        found = true;
        let recorded = ev
            .payload
            .get("stress_suite_sha256")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if recorded != actual_sha256 {
            return Err(StressSuiteArtifactError::ContentHashMismatch);
        }
    }
    if !found {
        return Err(StressSuiteArtifactError::AuditEventMissing);
    }

    Ok(artifact)
}
