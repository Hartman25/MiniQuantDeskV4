//! PROMOTION-BACKTEST-EVIDENCE-SEAM-01 — canonical, candidate-bound backtest
//! evidence resolver.
//!
//! Now that real durable authorities exist for
//! [`mqk_backtest::BacktestReport`] (`BKT-PROMOTION-ARTIFACT-AUTHORITY-01`),
//! [`crate::ArtifactLock`] (Patch B6), and stress evidence
//! (`PROMOTION-STRESS-SUITE-AUTHORITY-01`), this module is the seam that
//! resolves all three for exactly one, unambiguous promotion candidate.
//!
//! # Candidate identity
//!
//! The caller supplies a candidate's **identity** -- an artifact root and a
//! `run_id` -- never the evidence itself. `run_id` is
//! [`mqk_backtest::BacktestReport::run_id`]'s existing, already-accepted
//! deterministic identity (BKT-PROV-01: derived from strategy name, config,
//! input bar sequence, and execution model -- never from a result/metric
//! value), and `artifact_root.join(run_id.to_string())` is the exact
//! directory convention [`mqk_artifacts::init_run_artifacts`] already
//! writes every run to. Reusing that convention exactly (rather than
//! searching, globbing, or picking "latest") makes evidence ambiguity
//! structurally impossible: a filesystem directory name maps to at most one
//! directory, so there is no "duplicate evidence" case to disambiguate in
//! the first place -- the resolver only ever inspects the one location the
//! requested identity names.
//!
//! # What this resolver does NOT do
//!
//! It does not call [`crate::evaluate_promotion`] or otherwise judge
//! whether the candidate is promotion-worthy -- a real stress suite that
//! genuinely failed some scenarios still resolves successfully (as
//! `stress_suite.passed == false`); only a *structural* problem (missing,
//! malformed, mismatched-identity, root-escaping, or tampered evidence)
//! makes this resolver fail. Policy judgment belongs to `evaluate_promotion`
//! alone.

use std::fs;
use std::path::Path;

use uuid::Uuid;

use crate::artifact_gate::{lock_artifact_from_str, ArtifactLock, LockError};
use crate::types::{RobustnessEvidence, StressSuiteResult};
use mqk_artifacts::{
    load_canonical_backtest_report, load_canonical_robustness_gauntlet,
    load_canonical_stress_suite, BacktestReportArtifactError, RobustnessGauntletArtifact,
    RobustnessGauntletArtifactError, StressSuiteArtifact, StressSuiteArtifactError,
};
use mqk_backtest::BacktestReport;

/// The audit event type [`mqk_artifacts::write_backtest_report`] appends on
/// completion, carrying `canonical_report_sha256` -- see that function's
/// docs. Checked here (never by `load_canonical_backtest_report` itself,
/// which only cross-validates *identity* fields) to give
/// `backtest_report.json`'s numeric content the same tamper-evidence
/// `stress_suite.json` already has.
const BACKTEST_REPORT_AUDIT_EVENT_TYPE: &str = "backtest_run_completed";

/// The complete, candidate-bound evidence bundle
/// [`crate::PromotionInput`] needs.
#[derive(Debug, Clone)]
pub struct BacktestEvidenceBundle {
    pub run_id: Uuid,
    pub report: BacktestReport,
    pub artifact_lock: ArtifactLock,
    pub stress_suite: StressSuiteResult,
    /// PROMOTION-EVIDENCE-LINEAGE-V3: the EXACT `stress_suite.json` content
    /// hash [`crate::resolve_backtest_evidence`] verified for this
    /// candidate (`mqk_artifacts::StressSuiteArtifact::content_sha256`,
    /// reproducing the existing `stress_suite_sha256` audit hash -- never a
    /// new parallel hash).
    pub stress_artifact_sha256: String,
    /// CANONICAL-ROBUSTNESS-PROMOTION-GATE-01 — P9 robustness evidence for
    /// this SAME candidate.
    pub robustness_evidence: RobustnessEvidence,
    /// PROMOTION-EVIDENCE-LINEAGE-V3: the EXACT `robustness_gauntlet.json`
    /// content hash [`crate::resolve_backtest_evidence`] verified for this
    /// candidate (`mqk_artifacts::RobustnessGauntletArtifact::content_sha256`,
    /// reproducing the existing `robustness_gauntlet_sha256` audit hash --
    /// never a new parallel hash).
    pub finalized_robustness_artifact_sha256: String,
    /// The run's starting cash, for [`crate::PromotionInput::initial_equity_micros`].
    /// `BacktestReport` itself carries no such field (it's a config value,
    /// not engine output), so this is read from the SAME hash-chained
    /// `backtest_run_completed` audit event already verified for
    /// [`BacktestEvidenceResolveError::ReportContentHashMismatch`] --
    /// tamper-evident via the same mechanism, not a second trust path.
    pub initial_equity_micros: i64,
}

/// Every way [`resolve_backtest_evidence`] fails closed. Each variant names
/// a distinct structural problem -- never a generic catch-all -- so a
/// caller (and this module's own tests) can tell exactly which evidence
/// requirement was not met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BacktestEvidenceResolveError {
    /// `artifact_root` itself does not exist / cannot be canonicalized.
    ArtifactRootUnavailable,
    /// `artifact_root/<run_id>` does not exist.
    CandidateNotFound,
    /// `artifact_root/<run_id>` canonicalizes to a path outside
    /// `artifact_root` (e.g. a symlink planted at that exact name).
    CandidateRootEscape,
    /// `backtest_report.json` is missing, malformed, or its identity fields
    /// disagree with `manifest.json` -- see
    /// [`mqk_artifacts::BacktestReportArtifactError`].
    Report(BacktestReportArtifactError),
    /// The loaded report's `run_id` disagrees with the identity the caller
    /// actually requested. Defense-in-depth on top of
    /// [`Self::Report`]'s own manifest cross-check: this resolver's own
    /// entry point independently re-verifies the exact identity it was
    /// asked to resolve, so candidate A can never be satisfied by candidate
    /// B's evidence even if some future change ever loosened the loader.
    ReportIdentityMismatch { requested: Uuid, report: Uuid },
    /// `audit.jsonl` has no `backtest_run_completed` event recording the
    /// report's content hash.
    ReportAuditEventMissing,
    /// The `backtest_run_completed` event's recorded SHA-256 disagrees with
    /// the SHA-256 of the actual on-disk `backtest_report.json` -- the file
    /// was edited after the audit event was written.
    ReportContentHashMismatch,
    /// The `backtest_run_completed` event has no valid (non-negative
    /// integer) `initial_cash_micros` field.
    ReportInitialEquityMissing,
    /// `manifest.json` + `audit.jsonl` did not pass
    /// [`crate::lock_artifact_from_str`] -- see [`LockError`].
    ArtifactLockFailed(LockError),
    /// `stress_suite.json` is missing, malformed, or fails its own
    /// candidate-binding / integrity checks -- see
    /// [`mqk_artifacts::StressSuiteArtifactError`].
    StressSuite(StressSuiteArtifactError),
    /// `robustness_gauntlet.json` is missing, malformed, or fails its own
    /// candidate-binding / integrity checks -- see
    /// [`mqk_artifacts::RobustnessGauntletArtifactError`].
    /// CANONICAL-ROBUSTNESS-PROMOTION-GATE-01.
    RobustnessGauntlet(RobustnessGauntletArtifactError),
}

impl std::fmt::Display for BacktestEvidenceResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ArtifactRootUnavailable => write!(f, "artifact root unavailable"),
            Self::CandidateNotFound => write!(f, "candidate evidence directory not found"),
            Self::CandidateRootEscape => {
                write!(f, "candidate evidence directory resolves outside artifact root")
            }
            Self::Report(e) => write!(f, "backtest report evidence invalid: {e}"),
            Self::ReportIdentityMismatch { requested, report } => write!(
                f,
                "resolved report run_id={report} does not match requested candidate run_id={requested}"
            ),
            Self::ReportAuditEventMissing => write!(
                f,
                "audit.jsonl has no '{BACKTEST_REPORT_AUDIT_EVENT_TYPE}' event"
            ),
            Self::ReportContentHashMismatch => write!(
                f,
                "backtest_report.json content hash disagrees with the audited hash (tampered after write)"
            ),
            Self::ReportInitialEquityMissing => write!(
                f,
                "backtest_run_completed audit event has no valid initial_cash_micros"
            ),
            Self::ArtifactLockFailed(e) => write!(f, "artifact lock failed: {e}"),
            Self::StressSuite(e) => write!(f, "stress suite evidence invalid: {e}"),
            Self::RobustnessGauntlet(e) => write!(f, "robustness gauntlet evidence invalid: {e}"),
        }
    }
}

impl std::error::Error for BacktestEvidenceResolveError {}

/// Resolve the complete, candidate-bound backtest evidence bundle for the
/// candidate identified by `run_id`, searched under `artifact_root`.
///
/// The caller supplies only the candidate's identity -- never a raw
/// evidence path or the evidence itself -- exactly
/// `artifact_root.join(run_id.to_string())`, the same convention
/// `mqk_artifacts::init_run_artifacts` always writes to. Fails closed on
/// any missing, malformed, cross-candidate, root-escaping, or
/// post-write-tampered evidence. Never judges promotion eligibility -- a
/// real stress suite that genuinely failed scenarios still resolves
/// successfully; only structural evidence problems are errors here.
pub fn resolve_backtest_evidence(
    artifact_root: &Path,
    run_id: Uuid,
) -> Result<BacktestEvidenceBundle, BacktestEvidenceResolveError> {
    let root_canon = fs::canonicalize(artifact_root)
        .map_err(|_| BacktestEvidenceResolveError::ArtifactRootUnavailable)?;

    let candidate_dir = artifact_root.join(run_id.to_string());
    let candidate_canon = fs::canonicalize(&candidate_dir)
        .map_err(|_| BacktestEvidenceResolveError::CandidateNotFound)?;
    if !candidate_canon.starts_with(&root_canon) {
        return Err(BacktestEvidenceResolveError::CandidateRootEscape);
    }

    // --- BacktestReport authority (identity-checked against manifest.json
    // by the loader itself) ---
    let report =
        load_canonical_backtest_report(&candidate_canon).map_err(BacktestEvidenceResolveError::Report)?;
    if report.run_id != run_id {
        return Err(BacktestEvidenceResolveError::ReportIdentityMismatch {
            requested: run_id,
            report: report.run_id,
        });
    }

    // --- ArtifactLock (Patch B6's existing, unmodified verifier) ---
    let manifest_json = fs::read_to_string(candidate_canon.join("manifest.json"))
        .map_err(|_| BacktestEvidenceResolveError::Report(BacktestReportArtifactError::ManifestMissing))?;
    let audit_jsonl = fs::read_to_string(candidate_canon.join("audit.jsonl")).unwrap_or_default();
    let artifact_lock = lock_artifact_from_str(&manifest_json, &audit_jsonl)
        .map_err(BacktestEvidenceResolveError::ArtifactLockFailed)?;

    // --- Report content integrity: cross-check the audited hash against
    // the actual on-disk bytes (never verified by the loader itself). ---
    let report_bytes = fs::read(candidate_canon.join("backtest_report.json"))
        .map_err(|_| BacktestEvidenceResolveError::Report(BacktestReportArtifactError::MissingCanonicalReport))?;
    let actual_sha256 = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&report_bytes);
        hex::encode(hasher.finalize())
    };
    let mut found_report_event = false;
    let mut initial_equity_micros: Option<i64> = None;
    for line in audit_jsonl.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let ev: mqk_audit::AuditEvent = match serde_json::from_str(trimmed) {
            Ok(ev) => ev,
            Err(_) => continue,
        };
        if ev.event_type != BACKTEST_REPORT_AUDIT_EVENT_TYPE {
            continue;
        }
        found_report_event = true;
        let recorded = ev
            .payload
            .get("canonical_report_sha256")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if recorded != actual_sha256 {
            return Err(BacktestEvidenceResolveError::ReportContentHashMismatch);
        }
        initial_equity_micros = ev.payload.get("initial_cash_micros").and_then(|v| v.as_i64());
    }
    if !found_report_event {
        return Err(BacktestEvidenceResolveError::ReportAuditEventMissing);
    }
    let initial_equity_micros =
        initial_equity_micros.ok_or(BacktestEvidenceResolveError::ReportInitialEquityMissing)?;

    // --- StressSuiteResult authority ---
    let stress_artifact =
        load_canonical_stress_suite(&candidate_canon).map_err(BacktestEvidenceResolveError::StressSuite)?;
    let stress_artifact_sha256 = stress_artifact.content_sha256();
    let stress_suite = stress_result_from_artifact(&stress_artifact);

    // --- RobustnessEvidence authority (P9) ---
    let robustness_artifact = load_canonical_robustness_gauntlet(&candidate_canon)
        .map_err(BacktestEvidenceResolveError::RobustnessGauntlet)?;
    let finalized_robustness_artifact_sha256 = robustness_artifact.content_sha256();
    let robustness_evidence = robustness_evidence_from_artifact(&robustness_artifact);

    Ok(BacktestEvidenceBundle {
        run_id,
        report,
        artifact_lock,
        stress_suite,
        stress_artifact_sha256,
        robustness_evidence,
        finalized_robustness_artifact_sha256,
        initial_equity_micros,
    })
}

/// Bridge `mqk_artifacts::StressSuiteArtifact` (artifact-level; that crate
/// cannot depend on `mqk-promotion`, the reverse dependency already exists)
/// to `mqk_promotion::StressSuiteResult` (the type `evaluate_promotion`
/// consumes).
fn stress_result_from_artifact(a: &StressSuiteArtifact) -> StressSuiteResult {
    // `a.protocol_version` was already verified by `load_canonical_stress_suite`
    // to equal `mqk_backtest::STRESS_SUITE_PROTOCOL_VERSION` (== `mqk_promotion`'s
    // `REQUIRED_STRESS_PROTOCOL_VERSION` -- the two are the same constant by
    // construction) before this resolver ever runs. It is still carried
    // through here (never dropped/re-derived) so `evaluate_promotion`'s own
    // protocol check is a real, independent verification rather than a
    // rubber stamp -- PROMOTION-STRESS-AUTHORITY-REPAIR-01.
    if a.all_passed() {
        StressSuiteResult::pass(a.scenarios_run() as u32, a.protocol_version.clone())
    } else {
        StressSuiteResult::fail(
            a.scenarios_run() as u32,
            a.scenarios_passed() as u32,
            a.failed_scenario_descriptions(),
            a.protocol_version.clone(),
        )
    }
}

/// Bridge `mqk_artifacts::RobustnessGauntletArtifact` to
/// `mqk_promotion::RobustnessEvidence` -- mirrors [`stress_result_from_artifact`].
/// `a.protocol_version` was already verified by
/// `load_canonical_robustness_gauntlet` to equal
/// `mqk_backtest::ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION`; still carried
/// through (never dropped/re-derived) so `evaluate_promotion`'s own
/// protocol check is a real, independent verification.
fn robustness_evidence_from_artifact(a: &RobustnessGauntletArtifact) -> RobustnessEvidence {
    RobustnessEvidence {
        protocol_version: a.protocol_version.clone(),
        is_complete: a.is_complete(),
        all_applicable_passed: a.all_applicable_passed(),
        failed_scenarios: a.failed_scenario_descriptions(),
        deferred_scenarios: a.deferred_scenario_names(),
        dsr_pbo_sensitivity_research_trial_id: a
            .dsr_pbo_sensitivity_research_trial_id()
            .map(|s| s.to_string()),
        p7a_p7b_economic_replay_stress_research_trial_id: a
            .p7a_p7b_economic_replay_stress_research_trial_id()
            .map(|s| s.to_string()),
    }
}
