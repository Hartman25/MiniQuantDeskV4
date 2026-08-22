use std::io;
use std::path::{Path, PathBuf};

use mqk_backtest::BacktestReport;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::artifact_gate::ArtifactLock;
use crate::research_evidence::VerifiedPromotionOosEvidence;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Thresholds for promotion gating.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PromotionConfig {
    /// Minimum annualized Sharpe ratio.
    pub min_sharpe: f64,
    /// Maximum drawdown as fraction (e.g. 0.20 = 20%).
    pub max_mdd: f64,
    /// Minimum CAGR as fraction (e.g. 0.10 = 10%).
    pub min_cagr: f64,
    /// Minimum profit factor (sum profits / abs sum losses).
    pub min_profit_factor: f64,
    /// Minimum fraction of profitable months (0..1).
    pub min_profitable_months_pct: f64,
    /// P7C-REPAIR-01 (mission Section 6G): minimum required Deflated/
    /// Probabilistic Sharpe Ratio (a probability, [0,1]) from the verified
    /// multiple-testing judge evidence for THIS candidate. Explicit,
    /// versioned, required -- no hidden threshold constant.
    /// `evaluate_promotion` fails closed if this is outside `[0,1]`.
    pub min_deflated_sharpe_ratio: f64,
    /// P7C-REPAIR-01: maximum allowed Probability of Backtest Overfitting
    /// (`[0,1]`) from the verified multiple-testing judge evidence for the
    /// comparison population this candidate belongs to. `evaluate_promotion`
    /// fails closed if this is outside `[0,1]`.
    pub max_probability_backtest_overfitting: f64,
}

impl PromotionConfig {
    /// PROMOTION-EVIDENCE-LINEAGE-V3: a deterministic SHA-256 fingerprint
    /// over the exact threshold values a `PromotionDecision` was judged
    /// against. Unlike every other durable lineage hash this patch reuses
    /// (stress/robustness artifact content hashes, the Research judge
    /// artifact hash), there is no PRE-EXISTING artifact or audit event
    /// hashing `PromotionConfig` -- it is assembled fresh from trusted
    /// daemon/deployment configuration on every request, never durably
    /// written anywhere else. This is therefore the one genuinely NEW
    /// fingerprint this patch computes, following the same
    /// canonical-byte-buffer-then-SHA-256 pattern already established by
    /// `mqk-daemon::promotion_evidence_validation::compute_evidence_fingerprint_v2`
    /// -- each `f64` is hashed via its raw IEEE-754 big-endian bytes
    /// (`to_bits().to_be_bytes()`), never a lossy decimal string
    /// representation, so two configs with the same bit pattern always
    /// fingerprint identically and no rounding ambiguity can hide a
    /// genuine threshold change.
    pub fn deterministic_fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut buf = Vec::with_capacity(8 * 7);
        for v in [
            self.min_sharpe,
            self.max_mdd,
            self.min_cagr,
            self.min_profit_factor,
            self.min_profitable_months_pct,
            self.min_deflated_sharpe_ratio,
            self.max_probability_backtest_overfitting,
        ] {
            buf.extend_from_slice(&v.to_bits().to_be_bytes());
        }
        let mut hasher = Sha256::new();
        hasher.update(&buf);
        hex::encode(hasher.finalize())
    }
}

// ---------------------------------------------------------------------------
// Patch B2 — Stress suite result
// ---------------------------------------------------------------------------

/// The exact versioned production stress protocol
/// [`evaluate_promotion`](crate::evaluate_promotion) requires
/// [`StressSuiteResult::protocol_version`] to equal. Anchored directly to
/// [`mqk_backtest::STRESS_SUITE_PROTOCOL_VERSION`] -- never duplicated as a
/// separate literal -- so promotion authority always tracks whatever
/// protocol the real stress suite implementation
/// (`mqk_backtest::run_backtest_stress_suite`) actually emits.
///
/// PROMOTION-STRESS-AUTHORITY-REPAIR-01: the original Patch B2 doc comment
/// on this type named "adversarial partial-fill + cancel/replace" stress as
/// the required evidence. Investigation (see
/// `mqk_backtest::stress_suite` module docs) found that wording was
/// unimplemented scaffold -- the causal execution engine has no partial-fill
/// or cancel/replace order lifecycle, and building one would rewrite the
/// accepted `BKT-FUTURE-EXECUTION-01` causal-execution invariant. The real,
/// accepted production protocol is `cost_stress_2x` / `cost_stress_3x` /
/// `conservative_risk_limits` (`bkt_stress_suite_v1`); this constant --
/// checked by both the durable artifact loader
/// (`mqk_artifacts::load_canonical_stress_suite`) and `evaluate_promotion`
/// itself -- is what makes that the ONE exact supported protocol rather than
/// an unenforced convention.
pub const REQUIRED_STRESS_PROTOCOL_VERSION: &str = mqk_backtest::STRESS_SUITE_PROTOCOL_VERSION;

/// Results of the real, versioned production Backtest stress suite
/// (`mqk_backtest::run_backtest_stress_suite`, protocol
/// [`REQUIRED_STRESS_PROTOCOL_VERSION`]).
///
/// Promotion is **blocked** when:
/// - `PromotionInput.stress_suite` is `None` (suite not run).
/// - `protocol_version` does not equal [`REQUIRED_STRESS_PROTOCOL_VERSION`]
///   exactly (unknown, blank, stale, or fabricated protocol).
/// - The suite ran but `passed == false`.
/// - The suite ran with zero scenarios (`scenarios_run == 0`), which is invalid.
///
/// Build with [`StressSuiteResult::pass`] or [`StressSuiteResult::fail`].
/// Constructing a value here (even via these helpers) does not by itself
/// grant promotion authority -- only a `protocol_version` that matches
/// [`REQUIRED_STRESS_PROTOCOL_VERSION`] can pass `evaluate_promotion`'s
/// stress gate; production code only ever obtains a matching value from
/// [`mqk_artifacts::load_canonical_stress_suite`] via
/// `crate::evidence_bundle::resolve_backtest_evidence`, which itself
/// verifies protocol identity before this type is ever constructed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StressSuiteResult {
    /// True if all scenarios passed.
    pub passed: bool,
    /// Number of stress scenarios that ran. Must be ≥ 1 for the suite to be valid.
    pub scenarios_run: u32,
    /// Number of scenarios that passed.
    pub scenarios_passed: u32,
    /// Human-readable descriptions of failed scenarios (empty when passed).
    pub failed_scenarios: Vec<String>,
    /// The exact stress protocol this result was produced under. Checked
    /// against [`REQUIRED_STRESS_PROTOCOL_VERSION`] by `evaluate_promotion`
    /// -- see that constant's docs. PROMOTION-STRESS-AUTHORITY-REPAIR-01.
    pub protocol_version: String,
}

impl StressSuiteResult {
    /// All `scenarios_run` scenarios passed, under `protocol_version`.
    pub fn pass(scenarios_run: u32, protocol_version: impl Into<String>) -> Self {
        Self {
            passed: true,
            scenarios_run,
            scenarios_passed: scenarios_run,
            failed_scenarios: Vec::new(),
            protocol_version: protocol_version.into(),
        }
    }

    /// Some scenarios failed, under `protocol_version`.
    pub fn fail(
        scenarios_run: u32,
        scenarios_passed: u32,
        failed_scenarios: Vec<String>,
        protocol_version: impl Into<String>,
    ) -> Self {
        Self {
            passed: false,
            scenarios_run,
            scenarios_passed,
            failed_scenarios,
            protocol_version: protocol_version.into(),
        }
    }
}

/// The exact versioned P9 robustness-gauntlet protocol
/// [`evaluate_promotion`](crate::evaluate_promotion) requires
/// [`RobustnessEvidence::protocol_version`] to equal. Anchored directly to
/// [`mqk_backtest::ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION`] -- never
/// duplicated as a separate literal -- so promotion authority always
/// tracks whatever protocol `mqk_backtest::run_robustness_gauntlet` /
/// `mqk_backtest::dsr_pbo_sensitivity_scenario` actually emit.
pub const REQUIRED_ROBUSTNESS_PROTOCOL_VERSION: &str =
    mqk_backtest::ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION;

/// P9 (`BKT-ROBUSTNESS-GAUNTLET-01`) robustness evidence for one candidate.
/// CANONICAL-ROBUSTNESS-PROMOTION-GATE-01: bridges
/// `mqk_artifacts::RobustnessGauntletArtifact` (that crate cannot depend on
/// `mqk-promotion`, the reverse dependency already exists) into the type
/// `evaluate_promotion` consumes, exactly mirroring how [`StressSuiteResult`]
/// bridges `mqk_artifacts::StressSuiteArtifact`.
///
/// Deliberately carries only the JUDGMENT already computed by
/// `mqk_artifacts::RobustnessGauntletArtifact::{is_complete,
/// all_applicable_passed, failed_scenario_descriptions}` -- never the full
/// per-scenario detail (that stays in the durable artifact for audit; this
/// type is promotion-policy input only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RobustnessEvidence {
    /// The exact gauntlet protocol this evidence was produced under.
    /// Checked against [`REQUIRED_ROBUSTNESS_PROTOCOL_VERSION`] by
    /// `evaluate_promotion`.
    pub protocol_version: String,
    /// Every scenario required under `protocol_version`
    /// (`mqk_backtest::REQUIRED_ROBUSTNESS_SCENARIO_NAMES`) was genuinely
    /// evaluated -- present in `scenarios` and nothing left in `deferred`.
    /// Distinct from `all_applicable_passed`: a candidate can be complete
    /// and still fail.
    pub is_complete: bool,
    /// Every APPLICABLE required scenario passed (mirrors
    /// `RobustnessGauntletArtifact::all_applicable_passed`).
    pub all_applicable_passed: bool,
    /// Human-readable descriptions of failed applicable scenarios (empty
    /// when `all_applicable_passed`).
    pub failed_scenarios: Vec<String>,
    /// Names of scenarios still deferred (empty when `is_complete`).
    pub deferred_scenarios: Vec<String>,
    /// PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01: the exact Research
    /// `trial_id` the `dsr_pbo_sensitivity` scenario's evidence was computed
    /// against (mirrors
    /// `mqk_artifacts::RobustnessGauntletArtifact::dsr_pbo_sensitivity_research_trial_id`).
    /// `None` means no binding proof exists for this evidence (scenario
    /// absent, or recorded before this field existed) -- `evaluate_promotion`
    /// treats that identically to a genuine mismatch against
    /// `oos_evidence.trial_id()`: fail closed, never assume agreement.
    pub dsr_pbo_sensitivity_research_trial_id: Option<String>,
    /// P7A-P7B-ECONOMIC-REPLAY-STRESS-01: the exact Research `trial_id` the
    /// `p7a_p7b_economic_replay_stress` scenario's evidence was computed
    /// against (mirrors `mqk_artifacts::RobustnessGauntletArtifact::
    /// p7a_p7b_economic_replay_stress_research_trial_id`). Recorded
    /// unconditionally whenever the scenario is present -- whether it was
    /// genuinely evaluated, found inapplicable to this candidate, or
    /// errored -- so `None` here means only "scenario not yet finalized",
    /// never "no trial to bind". Same fail-closed contract as
    /// `dsr_pbo_sensitivity_research_trial_id`: `evaluate_promotion` treats
    /// `None`, and a mismatch against `oos_evidence.trial_id()`, identically.
    pub p7a_p7b_economic_replay_stress_research_trial_id: Option<String>,
    /// FINAL-P7A-P7B-REPLAY-AUTHORITY-01 Section B: the exact
    /// `economic_eval_id` the `p7a_p7b_economic_replay_stress` scenario's
    /// evidence was bound to (mirrors `mqk_artifacts::RobustnessGauntletArtifact::
    /// p7a_p7b_economic_replay_stress_baseline_economic_eval_id`). Must equal
    /// the P7C/OOS evidence's own verified `economic_eval_id` -- same trial
    /// but a DIFFERENT economic result is a mismatch, never assumed to
    /// agree. `None` means no binding proof exists (scenario absent, or
    /// recorded before this field existed) -- `evaluate_promotion` treats
    /// that identically to a genuine mismatch.
    pub p7a_p7b_economic_replay_stress_baseline_economic_eval_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// Input bundle for promotion evaluation.
pub struct PromotionInput {
    /// Initial equity in micros (required for CAGR denominator).
    pub initial_equity_micros: i64,
    /// The backtest report to evaluate.
    pub report: BacktestReport,
    /// Results of the partial-fill + cancel/replace stress suite.
    ///
    /// **Promotion is blocked if `None`** (suite not run) or if the suite
    /// ran but failed. Set to `Some(StressSuiteResult::pass(n))` after the
    /// stress suite completes successfully.
    pub stress_suite: Option<StressSuiteResult>, // Patch B2
    /// Patch B6 — proof that the artifact has been hash-locked.
    ///
    /// **Promotion is blocked if `None`** (artifact not validated).
    /// Set to `Some(lock)` after calling [`crate::lock_artifact_from_str`]
    /// successfully on the run's manifest + audit log.
    pub artifact_lock: Option<ArtifactLock>, // Patch B6
    /// P7C-REPAIR-01/-02/-03 — structurally VERIFIED, AUTHORITY-anchored
    /// promotion-grade out-of-sample Research evidence
    /// (PROMOTION-OOS-EVIDENCE-GATE-01-REPAIR-03).
    ///
    /// **Promotion is blocked if `None`** (no OOS evidence). The only way
    /// to obtain `Some(VerifiedPromotionOosEvidence)` in production code is
    /// [`crate::research_evidence::verify_promotion_oos_evidence`], which
    /// hash-binds, structurally verifies, and anchors AUTHORITY by reading
    /// the durable Research SQLite registry read-only (never from a
    /// caller-suppliable struct — see
    /// `crate::research_registry::load_research_authority`) — there is no
    /// compatibility default equivalent to "evidence passed", and no
    /// caller-populated struct that can satisfy this field.
    pub oos_evidence: Option<VerifiedPromotionOosEvidence>, // P7C-REPAIR-01/-02/-03
    /// CANONICAL-ROBUSTNESS-PROMOTION-GATE-01 — P9 (`BKT-ROBUSTNESS-GAUNTLET-01`)
    /// robustness evidence for this SAME candidate.
    ///
    /// **Promotion is blocked if `None`** (gauntlet not run), if
    /// `protocol_version` does not match [`REQUIRED_ROBUSTNESS_PROTOCOL_VERSION`],
    /// if `is_complete` is `false` (a required scenario is missing/deferred
    /// without becoming genuinely inapplicable), or if
    /// `all_applicable_passed` is `false`. Only a complete, valid,
    /// protocol-matching, fully-passed P9 proof satisfies this gate.
    pub robustness_evidence: Option<RobustnessEvidence>,
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// Computed promotion metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionMetrics {
    pub sharpe: f64,
    pub mdd: f64,
    pub cagr: f64,
    pub profit_factor: f64,
    pub profitable_months_pct: f64,
    // Intermediate reporting values
    pub start_equity_micros: i64,
    pub end_equity_micros: i64,
    pub duration_days: f64,
    pub num_months: usize,
    pub num_trades: usize,
    /// Whether integrity checks blocked execution during the backtest run.
    ///
    /// When `true`, all fills were produced before the integrity gate fired.
    /// A run where execution was blocked from bar 1 onward has zero fills and
    /// zero trades, which typically causes CAGR, profit_factor, and
    /// profitable_months_pct to all be 0 — promotion will fail on metrics alone.
    /// Surfaces `BacktestReport.execution_blocked` in the promotion record.
    pub execution_blocked: bool,
}

// ---------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------

/// Gate result for a single candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionDecision {
    pub passed: bool,
    /// Stable-ordered list of human-readable fail reasons (empty when passed).
    pub fail_reasons: Vec<String>,
    pub metrics: PromotionMetrics,
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

/// Backtest run provenance carried through to the promotion report.
///
/// These fields link a promotion report back to the exact backtest run
/// that was evaluated. They are extracted from [`BacktestReport`] and
/// stored in the promotion artifact so that a stored `promotion_report.json`
/// is unambiguously traceable to its source run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunProvenance {
    /// Deterministic run identity UUID from [`BacktestReport::run_id`].
    /// Non-nil when the report was produced by the backtest engine.
    pub run_id: Uuid,
    /// Strategy name from [`BacktestReport::strategy_name`].
    pub strategy_name: String,
    /// Config identity UUID from [`BacktestReport::config_id`].
    pub config_id: Uuid,
    /// Whether the run halted early.
    pub halted: bool,
    /// Halt reason, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub halt_reason: Option<String>,
}

/// Full promotion report artifact (serializable to JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionReport {
    pub config: PromotionConfig,
    /// Provenance of the backtest run that was promoted/rejected.
    ///
    /// Enables a stored `promotion_report.json` to be traced back to the
    /// exact run_id, strategy, and config that generated it.
    pub provenance: RunProvenance,
    pub metrics: PromotionMetrics,
    pub decision: PromotionDecision,
    /// Winner candidate id (set by select_best, None for single evaluation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winner_id: Option<String>,
}

/// Write the report as pretty-printed JSON to `out_dir/promotion_report.json`.
/// Returns the path written.
pub fn write_promotion_report_json(
    out_dir: &Path,
    report: &PromotionReport,
) -> io::Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let path = out_dir.join("promotion_report.json");
    let json = serde_json::to_string_pretty(report).map_err(io::Error::other)?;
    std::fs::write(&path, json)?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Tie-break types
// ---------------------------------------------------------------------------

/// A candidate for comparative evaluation.
pub struct Candidate {
    pub id: String,
    pub input: PromotionInput,
}
