//! STRATEGY-SCANNER-PROMOTION-01B/01C — scanner-candidate research-review
//! classifier and review-artifact IO.
//!
//! Given an already-evaluated [`crate::strategy_scanner::StrategyScanCandidate`]
//! (itself already pure and deterministic), classifies it into a
//! deterministic [`StrategyScanReviewState`] research state via
//! [`evaluate_scan_review_decision`]/[`build_review_decisions`] — pure, **no
//! file IO, no network IO, no DB access**, and no broker, provider, OMS,
//! outbox/inbox, admission, or strategy-router import anywhere in this repo.
//! The bottom half of this file (from [`ReviewRunRequest`] onward, mirroring
//! `strategy_scanner.rs`'s own `execute_strategy_scan`/`write_scan_artifacts`
//! split) reads an existing scanner artifact directory and writes a review
//! artifact directory — the only IO in this module, and still no provider,
//! broker, or DB call.
//!
//! `PaperCandidate` is **not** trading approval of any kind. It only means
//! "eligible for a later, separately authorized paper-promotion patch to
//! consider" — nothing in this module, or anywhere else in this repo,
//! consumes a `PaperCandidate` decision to submit, route, or admit an order.
//!
//! The scanner's own known caveat — `score` is `alpha_pct.or(total_return_pct)`,
//! so a candidate can rank well (high alpha vs. benchmark) while still losing
//! money in absolute terms — is the load-bearing safety rule enforced here:
//! a candidate with a negative `total_return_pct` can never reach
//! `PaperCandidate`, regardless of how positive its `alpha_pct` is.

use serde::{Deserialize, Serialize};

use crate::strategy_scanner::{ScanManifest, StrategyScanCandidate, StrategyScanTruthState};

// ---------------------------------------------------------------------------
// Review state
// ---------------------------------------------------------------------------

/// Research-review classification of one scanner candidate. Not a trading
/// readiness state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyScanReviewState {
    /// Missing required evidence, or the underlying scanner candidate never
    /// reached `candidate_ranked`. Never promotable.
    Blocked,
    /// Enough evidence exists to inspect, but not enough to call it a
    /// candidate (e.g. too few completed trades).
    NeedsReview,
    /// Can be watched or retested, but not traded (e.g. a present-but-weak
    /// profit factor).
    WatchlistCandidate,
    /// Eligible for a later, separately authorized paper-promotion patch to
    /// consider. NOT trading approval, NOT automatically tradable.
    PaperCandidate,
    /// Explicitly fails an evidence requirement (halted run, drawdown over
    /// policy, or a negative absolute total return).
    Rejected,
}

impl StrategyScanReviewState {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::NeedsReview => "needs_review",
            Self::WatchlistCandidate => "watchlist_candidate",
            Self::PaperCandidate => "paper_candidate",
            Self::Rejected => "rejected",
        }
    }
}

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

/// Deterministic review-classification thresholds. These are research review
/// filters, not strategy optimization targets — do not tune these to make a
/// particular candidate pass.
#[derive(Clone, Debug, PartialEq)]
pub struct StrategyScanReviewPolicy {
    pub min_bars_used: usize,
    pub min_trade_count: usize,
    pub min_total_return_pct: f64,
    pub min_alpha_pct: f64,
    pub max_drawdown_pct: f64,
    pub min_profit_factor: f64,
}

impl Default for StrategyScanReviewPolicy {
    fn default() -> Self {
        Self {
            min_bars_used: 252,
            min_trade_count: 5,
            min_total_return_pct: 0.0,
            min_alpha_pct: 0.0,
            max_drawdown_pct: 25.0,
            min_profit_factor: 1.05,
        }
    }
}

// ---------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------

/// Research-review decision for one scanner candidate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyScanReviewDecision {
    pub symbol: String,
    pub timeframe: String,
    pub strategy_id: String,
    pub scanner_rank: Option<usize>,
    pub scanner_score: Option<f64>,
    pub review_state: StrategyScanReviewState,
    pub reason_codes: Vec<String>,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
}

fn decision(
    candidate: &StrategyScanCandidate,
    review_state: StrategyScanReviewState,
    reason_codes: Vec<String>,
    blockers: Vec<String>,
    warnings: Vec<String>,
) -> StrategyScanReviewDecision {
    StrategyScanReviewDecision {
        symbol: candidate.symbol.clone(),
        timeframe: candidate.timeframe.clone(),
        strategy_id: candidate.strategy_id.clone(),
        scanner_rank: candidate.rank,
        scanner_score: candidate.score,
        review_state,
        reason_codes,
        blockers,
        warnings,
    }
}

// ---------------------------------------------------------------------------
// evaluate_scan_review_decision — pure, no IO
// ---------------------------------------------------------------------------

/// Classify one already-evaluated scanner candidate. Pure and deterministic:
/// identical `(candidate, policy)` inputs always produce an identical
/// `StrategyScanReviewDecision`.
pub fn evaluate_scan_review_decision(
    candidate: &StrategyScanCandidate,
    policy: &StrategyScanReviewPolicy,
) -> StrategyScanReviewDecision {
    // A candidate the scanner itself never ranked (data_missing,
    // insufficient_data, unsupported_strategy/timeframe, backtest_failed,
    // metrics_unavailable) can never be promoted — fail closed before
    // looking at any metric field.
    if candidate.truth_state != StrategyScanTruthState::CandidateRanked {
        return decision(
            candidate,
            StrategyScanReviewState::Blocked,
            vec!["not_candidate_ranked".to_string()],
            vec![format!(
                "scanner truth_state '{}' is not candidate_ranked",
                candidate.truth_state.code()
            )],
            Vec::new(),
        );
    }

    let metrics = &candidate.metrics;
    let mut reason_codes = Vec::new();
    let mut blockers = Vec::new();

    if candidate.score.is_none() {
        reason_codes.push("missing_score".to_string());
        blockers.push("scanner score is missing".to_string());
    }
    if metrics.total_return_pct.is_none() {
        reason_codes.push("missing_total_return".to_string());
        blockers.push("total_return_pct is missing".to_string());
    }
    if metrics.alpha_pct.is_none() {
        reason_codes.push("missing_alpha".to_string());
        blockers.push("alpha_pct is missing".to_string());
    }
    if metrics.max_drawdown_pct.is_none() {
        reason_codes.push("missing_drawdown".to_string());
        blockers.push("max_drawdown_pct is missing".to_string());
    }
    if metrics.trade_count.is_none() {
        reason_codes.push("missing_trade_count".to_string());
        blockers.push("trade_count is missing".to_string());
    }
    if metrics.bars_used < policy.min_bars_used {
        reason_codes.push("below_min_bars".to_string());
        blockers.push(format!(
            "bars_used {} below required minimum {}",
            metrics.bars_used, policy.min_bars_used
        ));
    }

    // Missing required evidence (or too few bars) always blocks -- never
    // promotes, regardless of how any other field looks.
    if !blockers.is_empty() {
        return decision(
            candidate,
            StrategyScanReviewState::Blocked,
            reason_codes,
            blockers,
            Vec::new(),
        );
    }

    // Every field below is proven `Some`/present by the checks above.
    let total_return = metrics.total_return_pct.expect("checked above");
    let alpha = metrics.alpha_pct.expect("checked above");
    let drawdown = metrics.max_drawdown_pct.expect("checked above");
    let trade_count = metrics.trade_count.expect("checked above");

    if metrics.halted {
        reason_codes.push("halted".to_string());
        blockers.push("backtest halted before processing all bars".to_string());
        return decision(
            candidate,
            StrategyScanReviewState::Rejected,
            reason_codes,
            blockers,
            Vec::new(),
        );
    }

    if drawdown > policy.max_drawdown_pct {
        reason_codes.push("excess_drawdown".to_string());
        blockers.push(format!(
            "max_drawdown_pct {drawdown:.4} exceeds policy max {:.4}",
            policy.max_drawdown_pct
        ));
        return decision(
            candidate,
            StrategyScanReviewState::Rejected,
            reason_codes,
            blockers,
            Vec::new(),
        );
    }

    // Load-bearing safety rule: a negative absolute total return can never
    // become paper_candidate, even when alpha (return vs. benchmark) is
    // positive -- ranking well is not the same as making money.
    if total_return < policy.min_total_return_pct {
        reason_codes.push("negative_total_return".to_string());
        blockers.push(format!(
            "total_return_pct {total_return:.4} is below required minimum {:.4} -- \
             a candidate cannot be promoted on rank/alpha alone while losing money \
             in absolute terms",
            policy.min_total_return_pct
        ));
        return decision(
            candidate,
            StrategyScanReviewState::Rejected,
            reason_codes,
            blockers,
            Vec::new(),
        );
    }

    if alpha < policy.min_alpha_pct {
        reason_codes.push("non_positive_alpha".to_string());
        blockers.push(format!(
            "alpha_pct {alpha:.4} is below required minimum {:.4}",
            policy.min_alpha_pct
        ));
        return decision(
            candidate,
            StrategyScanReviewState::Rejected,
            reason_codes,
            blockers,
            Vec::new(),
        );
    }

    // Marginal gates: a candidate can still be watched/reviewed here, but is
    // not eligible to become paper_candidate.
    let mut warnings = Vec::new();
    let mut needs_review = false;
    let mut watchlist = false;

    if trade_count < policy.min_trade_count {
        reason_codes.push("below_min_trade_count".to_string());
        warnings.push(format!(
            "trade_count {trade_count} below required minimum {}",
            policy.min_trade_count
        ));
        needs_review = true;
    }

    match metrics.profit_factor {
        Some(pf) if pf < policy.min_profit_factor => {
            reason_codes.push("weak_profit_factor".to_string());
            warnings.push(format!(
                "profit_factor {pf:.4} below required minimum {:.4}",
                policy.min_profit_factor
            ));
            watchlist = true;
        }
        None => {
            reason_codes.push("profit_factor_unavailable".to_string());
            warnings.push("profit_factor is not available for this candidate".to_string());
            watchlist = true;
        }
        Some(_) => {}
    }

    let review_state = if needs_review {
        reason_codes.push("needs_more_evidence_for_paper_candidate".to_string());
        StrategyScanReviewState::NeedsReview
    } else if watchlist {
        reason_codes.push("needs_more_evidence_for_paper_candidate".to_string());
        StrategyScanReviewState::WatchlistCandidate
    } else {
        reason_codes.push("eligible_paper_candidate".to_string());
        StrategyScanReviewState::PaperCandidate
    };

    decision(candidate, review_state, reason_codes, blockers, warnings)
}

/// Classify every candidate in `candidates`, preserving input order (the
/// scanner's own [`crate::strategy_scanner::rank_scan_candidates`] ordering
/// is already deterministic, so this function introduces no reordering of
/// its own).
pub fn build_review_decisions(
    candidates: &[StrategyScanCandidate],
    policy: &StrategyScanReviewPolicy,
) -> Vec<StrategyScanReviewDecision> {
    candidates
        .iter()
        .map(|c| evaluate_scan_review_decision(c, policy))
        .collect()
}

// ---------------------------------------------------------------------------
// STRATEGY-SCANNER-PROMOTION-01C: review-artifact schema + IO.
//
// Reads an existing scanner artifact directory (manifest.json/summary.json/
// candidates.json, written by `crate::strategy_scanner::write_scan_artifacts`)
// and writes a review artifact directory (manifest.json/
// review_decisions.json/review_decisions.csv/summary.json). No provider,
// broker, or DB call -- the only IO is reading the three named scanner
// artifact files and writing the four named review artifact files.
// ---------------------------------------------------------------------------

use std::path::{Path, PathBuf};

const RESEARCH_ONLY_WARNING: &str = "Scanner ranking is research evidence only.";
const NOT_TRADING_APPROVAL_WARNING: &str = "Scanner output is not autonomous trading approval.";
const PROMOTION_NOT_APPROVAL_WARNING: &str = "promotion-ready is not trading-approved.";
const PAPER_CANDIDATE_WARNING: &str = "paper_candidate requires a later, separately authorized \
     paper-promotion patch before any paper trading.";
const NO_AUTONOMOUS_APPROVAL_WARNING: &str =
    "No autonomous trading approval is granted by this review.";

fn fixed_review_warnings() -> Vec<String> {
    vec![
        RESEARCH_ONLY_WARNING.to_string(),
        NOT_TRADING_APPROVAL_WARNING.to_string(),
        PROMOTION_NOT_APPROVAL_WARNING.to_string(),
        PAPER_CANDIDATE_WARNING.to_string(),
        NO_AUTONOMOUS_APPROVAL_WARNING.to_string(),
    ]
}

/// Deterministic review-run manifest. Carries every policy threshold used
/// for this review so a reader can judge the classification without
/// re-deriving it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewManifest {
    pub schema_version: u32,
    pub review_id: String,
    pub scanner_scan_id: String,
    pub source_artifact_dir: String,
    pub created_at_utc: String,
    pub git_hash: String,
    pub policy_min_bars_used: usize,
    pub policy_min_trade_count: usize,
    pub policy_min_total_return_pct: f64,
    pub policy_min_alpha_pct: f64,
    pub policy_max_drawdown_pct: f64,
    pub policy_min_profit_factor: f64,
    pub candidate_count: usize,
    pub blocked_count: usize,
    pub needs_review_count: usize,
    pub watchlist_candidate_count: usize,
    pub paper_candidate_count: usize,
    pub rejected_count: usize,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
}

/// Deterministic review-run summary.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewSummary {
    pub scanner_scan_id: String,
    pub review_id: String,
    pub candidate_count: usize,
    pub blocked_count: usize,
    pub needs_review_count: usize,
    pub watchlist_candidate_count: usize,
    pub paper_candidate_count: usize,
    pub rejected_count: usize,
    pub top_paper_candidates: Vec<StrategyScanReviewDecision>,
    pub top_watchlist_candidates: Vec<StrategyScanReviewDecision>,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
}

/// Bounded request describing one review run. Every path is caller-resolved
/// (relative to the caller's working directory); this function performs no
/// path-escape validation of its own -- callers that accept `artifact_dir`/
/// `out_dir` from an untrusted operator surface (e.g. a future daemon route)
/// must apply their own bounds/validation before calling
/// `execute_strategy_scan_review`, mirroring the existing
/// `GET /api/v1/strategy-scans/artifact` route's canonicalize+root-prefix
/// pattern.
#[derive(Clone, Debug)]
pub struct ReviewRunRequest {
    pub artifact_dir: String,
    pub top: usize,
    pub policy: StrategyScanReviewPolicy,
    /// Caller-resolved short git hash, kept caller-supplied for the same
    /// reason as `ScanRunRequest::git_hash`.
    pub git_hash: String,
    /// Caller-resolved RFC3339 creation timestamp, kept caller-supplied for
    /// the same reason as `ScanRunRequest::created_at_utc`.
    pub created_at_utc: String,
}

/// Result of running a review (before any artifact file is written).
#[derive(Clone, Debug)]
pub struct ReviewRunOutput {
    pub review_id: uuid::Uuid,
    pub manifest: ReviewManifest,
    pub decisions: Vec<StrategyScanReviewDecision>,
    pub summary: ReviewSummary,
}

/// Deterministic UUIDv5 review identity: re-running review over the same
/// scanner `scan_id` with an identical policy always produces the same
/// `review_id`. Never `Uuid::new_v4()`.
pub fn derive_review_id(scan_id: &str, policy: &StrategyScanReviewPolicy) -> uuid::Uuid {
    let canonical = format!(
        "mqk-scan-review.v1|scan_id={scan_id}|min_bars_used={}|min_trade_count={}|\
         min_total_return_pct={}|min_alpha_pct={}|max_drawdown_pct={}|min_profit_factor={}",
        policy.min_bars_used,
        policy.min_trade_count,
        policy.min_total_return_pct,
        policy.min_alpha_pct,
        policy.max_drawdown_pct,
        policy.min_profit_factor,
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

/// Render the full review-decision set as CSV.
pub fn review_decisions_to_csv(decisions: &[StrategyScanReviewDecision]) -> String {
    let mut out = String::from(
        "symbol,timeframe,strategy_id,scanner_rank,scanner_score,review_state,reason_codes,blockers,warnings\n",
    );
    for d in decisions {
        let row = [
            csv_field(&d.symbol),
            csv_field(&d.timeframe),
            csv_field(&d.strategy_id),
            d.scanner_rank.map(|r| r.to_string()).unwrap_or_default(),
            d.scanner_score
                .map(|v| format!("{v:.4}"))
                .unwrap_or_default(),
            d.review_state.code().to_string(),
            csv_field(&d.reason_codes.join(";")),
            csv_field(&d.blockers.join(";")),
            csv_field(&d.warnings.join(";")),
        ];
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

/// Read an existing scanner artifact directory
/// (`manifest.json`/`summary.json`/`candidates.json`, as written by
/// [`crate::strategy_scanner::write_scan_artifacts`]), classify every
/// candidate via [`build_review_decisions`], and return the review result
/// (not yet written to disk -- see [`write_review_artifacts`]).
///
/// No provider call, no broker call, no live/paper order, no DB connection.
/// The only IO is reading `{req.artifact_dir}/manifest.json`,
/// `{req.artifact_dir}/summary.json`, and `{req.artifact_dir}/candidates.json`
/// (all three required to exist and parse).
pub fn execute_strategy_scan_review(req: &ReviewRunRequest) -> Result<ReviewRunOutput, String> {
    if req.top == 0 {
        return Err("top must be > 0".to_string());
    }

    let artifact_dir = Path::new(&req.artifact_dir);
    let manifest_path = artifact_dir.join("manifest.json");
    let summary_path = artifact_dir.join("summary.json");
    let candidates_path = artifact_dir.join("candidates.json");

    let scanner_manifest: ScanManifest = read_scanner_artifact_json(&manifest_path)?;
    // summary.json is required to exist and parse (proves the scanner
    // artifact directory is complete) but its content is not otherwise
    // consumed here -- candidates.json already carries every field needed.
    let _scanner_summary: crate::strategy_scanner::ScanSummary =
        read_scanner_artifact_json(&summary_path)?;
    let candidates: Vec<StrategyScanCandidate> = read_scanner_artifact_json(&candidates_path)?;

    let decisions = build_review_decisions(&candidates, &req.policy);

    let mut blocked_count = 0usize;
    let mut needs_review_count = 0usize;
    let mut watchlist_candidate_count = 0usize;
    let mut paper_candidate_count = 0usize;
    let mut rejected_count = 0usize;
    for d in &decisions {
        match d.review_state {
            StrategyScanReviewState::Blocked => blocked_count += 1,
            StrategyScanReviewState::NeedsReview => needs_review_count += 1,
            StrategyScanReviewState::WatchlistCandidate => watchlist_candidate_count += 1,
            StrategyScanReviewState::PaperCandidate => paper_candidate_count += 1,
            StrategyScanReviewState::Rejected => rejected_count += 1,
        }
    }

    let review_id = derive_review_id(&scanner_manifest.scan_id, &req.policy);
    let warnings = fixed_review_warnings();

    let manifest = ReviewManifest {
        schema_version: 1,
        review_id: review_id.to_string(),
        scanner_scan_id: scanner_manifest.scan_id.clone(),
        source_artifact_dir: req.artifact_dir.clone(),
        created_at_utc: req.created_at_utc.clone(),
        git_hash: req.git_hash.clone(),
        policy_min_bars_used: req.policy.min_bars_used,
        policy_min_trade_count: req.policy.min_trade_count,
        policy_min_total_return_pct: req.policy.min_total_return_pct,
        policy_min_alpha_pct: req.policy.min_alpha_pct,
        policy_max_drawdown_pct: req.policy.max_drawdown_pct,
        policy_min_profit_factor: req.policy.min_profit_factor,
        candidate_count: decisions.len(),
        blocked_count,
        needs_review_count,
        watchlist_candidate_count,
        paper_candidate_count,
        rejected_count,
        blockers: Vec::new(),
        warnings: warnings.clone(),
    };

    let top_paper_candidates: Vec<StrategyScanReviewDecision> = decisions
        .iter()
        .filter(|d| d.review_state == StrategyScanReviewState::PaperCandidate)
        .take(req.top)
        .cloned()
        .collect();
    let top_watchlist_candidates: Vec<StrategyScanReviewDecision> = decisions
        .iter()
        .filter(|d| d.review_state == StrategyScanReviewState::WatchlistCandidate)
        .take(req.top)
        .cloned()
        .collect();

    let summary = ReviewSummary {
        scanner_scan_id: scanner_manifest.scan_id.clone(),
        review_id: review_id.to_string(),
        candidate_count: decisions.len(),
        blocked_count,
        needs_review_count,
        watchlist_candidate_count,
        paper_candidate_count,
        rejected_count,
        top_paper_candidates,
        top_watchlist_candidates,
        blockers: Vec::new(),
        warnings,
    };

    Ok(ReviewRunOutput {
        review_id,
        manifest,
        decisions,
        summary,
    })
}

fn read_scanner_artifact_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("read failed: {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse failed: {}: {e}", path.display()))
}

/// Write `manifest.json` / `review_decisions.json` / `review_decisions.csv`
/// / `summary.json` for a completed [`ReviewRunOutput`] into
/// `{out_dir}/{review_id}/`. Returns the created run directory.
pub fn write_review_artifacts(out_dir: &Path, output: &ReviewRunOutput) -> Result<PathBuf, String> {
    let run_dir = out_dir.join(output.review_id.to_string());
    std::fs::create_dir_all(&run_dir).map_err(|e| {
        format!(
            "create review artifact dir failed: {}: {e}",
            run_dir.display()
        )
    })?;
    std::fs::write(
        run_dir.join("manifest.json"),
        serde_json::to_string_pretty(&output.manifest)
            .map_err(|e| format!("serialize review manifest failed: {e}"))?,
    )
    .map_err(|e| format!("write manifest.json failed: {}: {e}", run_dir.display()))?;
    std::fs::write(
        run_dir.join("review_decisions.json"),
        serde_json::to_string_pretty(&output.decisions)
            .map_err(|e| format!("serialize review decisions failed: {e}"))?,
    )
    .map_err(|e| {
        format!(
            "write review_decisions.json failed: {}: {e}",
            run_dir.display()
        )
    })?;
    std::fs::write(
        run_dir.join("review_decisions.csv"),
        review_decisions_to_csv(&output.decisions),
    )
    .map_err(|e| {
        format!(
            "write review_decisions.csv failed: {}: {e}",
            run_dir.display()
        )
    })?;
    std::fs::write(
        run_dir.join("summary.json"),
        serde_json::to_string_pretty(&output.summary)
            .map_err(|e| format!("serialize review summary failed: {e}"))?,
    )
    .map_err(|e| format!("write summary.json failed: {}: {e}", run_dir.display()))?;

    Ok(run_dir)
}
