//! STRATEGY-SCANNER-PROMOTION-01B — pure scanner-candidate research-review
//! classifier.
//!
//! Given an already-evaluated [`crate::strategy_scanner::StrategyScanCandidate`]
//! (itself already pure and deterministic), classifies it into a
//! deterministic [`StrategyScanReviewState`] research state. This module
//! performs **no file IO, no network IO, no DB access**, and imports no
//! broker, provider, OMS, outbox/inbox, admission, or strategy-router type
//! from anywhere in this repo — it consumes only
//! [`crate::strategy_scanner::StrategyScanCandidate`] values already held in
//! memory by the caller.
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

use crate::strategy_scanner::{StrategyScanCandidate, StrategyScanTruthState};

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
