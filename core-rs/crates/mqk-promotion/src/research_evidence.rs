use serde::{Deserialize, Serialize};

// ============================================================================
// P7C (PROMOTION-OOS-EVIDENCE-GATE-01)
// ============================================================================
//
// Structurally verified summary of the Python Research pipeline's
// out-of-sample evidence for ONE candidate, extracted from (never
// re-derived from) the existing accepted artifacts:
//
//   - economic_walk_forward.json (RESEARCH-ECONOMIC-WALKFORWARD-01 +
//     REPAIR-01/02/03) -- `economic_protocol_id`, `folds_used`,
//     `holdout_status`, `execution_pricing_protocol_id` (P7A),
//     `weight_to_share_protocol_id` (P7B), `economic_eval_id`.
//   - the multiple-testing judge artifact
//     (RESEARCH-MULTIPLE-TESTING-JUDGE-01 + REPAIR-01/02) --
//     `judge_status`, `judge_trial_evaluable`, `pbo_status`,
//     `trial_included_in_judge_scope`.
//
// This type NEVER recomputes DSR/PBO/economic P&L -- see the mission's
// explicit rule: "Do not recalculate DSR/PBO inside Rust. Do not port the
// statistics into mqk-promotion." It exists ONLY so `evaluate_promotion`
// can structurally verify that REAL, correctly-scoped, non-fabricated OOS
// evidence backs a candidate -- a hand-fabricated `passed=true` boolean
// cannot bypass this gate, because there is no single boolean field here to
// fabricate: every fact is checked independently against the exact accepted
// protocol/status strings the Python pipeline actually emits.
//
// SCOPE LIMIT (be honest about what this does NOT do): constructing a
// `PromotionOosEvidence` from the REAL economic_walk_forward.json / judge
// JSON files on disk is NOT implemented by this patch -- there is no
// Rust-side JSON-parsing bridge for those Python artifacts yet. This type
// and the gate that consumes it are the STRUCTURAL VERIFICATION layer;
// wiring a real file-parsing constructor is future work (tracked
// separately, not P7C's scope per the mission's "small verified Rust-side
// evidence summary ... constructed FROM those existing artifacts" framing
// -- construction is the caller's job, verification is this gate's job).

/// Required `economic_protocol_id` value -- see
/// `mqk_research.ml.economic_walkforward.PROTOCOL_ID`. A candidate whose
/// evidence carries a different (or legacy global-fit `ml_train_meta_v1`)
/// protocol_id cannot satisfy this gate -- mirrors P6C's evidence-boundary
/// structural distinction.
pub const REQUIRED_ECONOMIC_PROTOCOL_ID: &str = "economic_walk_forward_v1";

/// Required `holdout_status` value -- see economic_walk_forward.json's own
/// `"holdout": {"status": ...}` and the holdout consumption ledger
/// (RESEARCH-HOLDOUT-CONSUMPTION-LEDGER-01). Any other value (including a
/// literal "consumed" status) fails closed -- P7C never scores or consumes
/// the reserved holdout, and never trusts evidence that claims it did.
pub const REQUIRED_HOLDOUT_STATUS: &str = "reserved_not_evaluated";

/// Required `execution_pricing_protocol_id` value (P7A) -- see
/// `mqk_research.ml.execution_pricing.EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1`.
pub const REQUIRED_EXECUTION_PRICING_PROTOCOL_ID: &str = "rust_conservative_bar_range_v1";

/// Required `weight_to_share_protocol_id` value (P7B) -- see
/// `mqk_research.ml.weight_to_share.WEIGHT_TO_SHARE_PROTOCOL_ID_V1`. `None`
/// is the diagnostic/legacy continuous-weight-only state and can never
/// satisfy this gate (mirrors `require_official_weight_to_share_parity`).
pub const REQUIRED_WEIGHT_TO_SHARE_PROTOCOL_ID: &str = "weight_to_share_v1";

/// Required `judge_status` value -- see
/// `mqk_research.ml.multiple_testing_judge`'s own `"judge_status"` field.
/// Deliberately the STRONGEST unambiguous evaluability state
/// (`"evaluated"`), not `"partially_evaluable"`: the judge module itself
/// documents that "partially_evaluable" means either DSR or PBO failed for
/// the population as a whole, and the mission gives no accepted threshold
/// for treating a partial result as promotion-grade -- fail closed rather
/// than invent one.
pub const REQUIRED_JUDGE_STATUS: &str = "evaluated";

/// Required PBO `status` value -- see the judge's own `pbo_result.status`.
pub const REQUIRED_PBO_STATUS: &str = "evaluated";

/// Structurally verified OOS evidence for one promotion candidate. See
/// module docs for exactly which Python artifact each field is extracted
/// from and what it must equal to satisfy [`crate::evaluate_promotion`].
///
/// Every field here is a granular, independently-checked fact -- there is
/// no single `passed`/`oos_passed` boolean anywhere in this type. A caller
/// cannot bypass the gate by asserting success; each fact must match the
/// exact accepted protocol/status value the real Python pipeline emits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionOosEvidence {
    /// Research trial identity this evidence belongs to. Must be non-empty.
    pub trial_id: String,

    /// From `economic_walk_forward.json`'s `"protocol": {"protocol_id": ...}`.
    /// Must equal [`REQUIRED_ECONOMIC_PROTOCOL_ID`].
    pub economic_protocol_id: String,
    /// Number of discovery-fold OOS folds actually used to build this
    /// evidence (`economic_walk_forward.json`'s `"aggregate".folds_used`).
    /// Must be `> 0` -- zero folds means no real OOS evidence exists.
    pub folds_used: u32,
    /// From `economic_walk_forward.json`'s `"holdout": {"status": ...}`.
    /// Must equal [`REQUIRED_HOLDOUT_STATUS`].
    pub holdout_status: String,

    /// From `economic_walk_forward.json`'s
    /// `"execution_pricing": {"pricing_model_id": ...}` (P7A). Must equal
    /// [`REQUIRED_EXECUTION_PRICING_PROTOCOL_ID`].
    pub execution_pricing_protocol_id: String,

    /// From `economic_walk_forward.json`'s
    /// `"weight_to_share": {"weight_to_share_protocol_id": ...}` (P7B).
    /// Must equal `Some(`[`REQUIRED_WEIGHT_TO_SHARE_PROTOCOL_ID`]`)`.
    pub weight_to_share_protocol_id: Option<String>,

    /// From the multiple-testing judge artifact's own `"judge_status"`.
    /// Must equal [`REQUIRED_JUDGE_STATUS`].
    pub judge_status: String,
    /// Whether THIS candidate's own DSR result entry
    /// (`dsr_results[].evaluable`, matched by `trial_id`) was evaluable.
    /// Must be `true`.
    pub judge_trial_evaluable: bool,
    /// From the judge's own `pbo_result.status`. Must equal
    /// [`REQUIRED_PBO_STATUS`].
    pub pbo_status: String,
    /// Whether `trial_id` appears in the judge artifact's own
    /// `included_trial_ids` (never `excluded_trial_ids`) -- proves this
    /// candidate was actually part of the judged comparison population,
    /// not merely a same-shaped but unrelated evidence bundle. Must be
    /// `true`.
    pub trial_included_in_judge_scope: bool,

    /// Content identity of the underlying `economic_walk_forward.json`
    /// this evidence was extracted from (its own `ids.economic_eval_id`).
    /// Must be non-empty -- lets a caller cross-check this evidence bundle
    /// against the specific artifact it claims to summarize.
    pub economic_eval_id: String,
}

impl PromotionOosEvidence {
    /// Structurally VALID synthetic evidence for tests that need a
    /// passing candidate and are not themselves testing the OOS-evidence
    /// gate. Mirrors [`crate::ArtifactLock::new_for_testing`]'s naming
    /// convention exactly -- production code, but its ONLY legitimate use
    /// is a test fixture standing in for a real, verified artifact bundle.
    pub fn valid_for_testing(trial_id: impl Into<String>) -> Self {
        Self {
            trial_id: trial_id.into(),
            economic_protocol_id: REQUIRED_ECONOMIC_PROTOCOL_ID.to_string(),
            folds_used: 3,
            holdout_status: REQUIRED_HOLDOUT_STATUS.to_string(),
            execution_pricing_protocol_id: REQUIRED_EXECUTION_PRICING_PROTOCOL_ID.to_string(),
            weight_to_share_protocol_id: Some(REQUIRED_WEIGHT_TO_SHARE_PROTOCOL_ID.to_string()),
            judge_status: REQUIRED_JUDGE_STATUS.to_string(),
            judge_trial_evaluable: true,
            pbo_status: REQUIRED_PBO_STATUS.to_string(),
            trial_included_in_judge_scope: true,
            economic_eval_id: "test_synthetic_economic_eval_id".to_string(),
        }
    }
}

/// Fail-closed structural verification (P7C CORE RULE): checks every
/// granular fact in `evidence` against the exact accepted protocol/status
/// values the real Python pipeline emits. `None` means FAIL -- there is no
/// compatibility default equivalent to "evidence passed". Returns one
/// human-readable reason per failing check (stable order matching field
/// declaration order), empty when `evidence` fully satisfies the gate.
pub fn check_oos_evidence(evidence: &Option<PromotionOosEvidence>) -> Vec<String> {
    let mut reasons = Vec::new();

    let Some(ev) = evidence else {
        reasons.push(
            "OOS evidence missing: PromotionInput.oos_evidence is None -- promotion-grade \
             out-of-sample Research evidence is required (P7C, PROMOTION-OOS-EVIDENCE-GATE-01); \
             historical backtest metrics alone are never sufficient"
                .to_string(),
        );
        return reasons;
    };

    if ev.trial_id.trim().is_empty() {
        reasons.push("OOS evidence rejected: trial_id is empty".to_string());
    }

    if ev.economic_protocol_id != REQUIRED_ECONOMIC_PROTOCOL_ID {
        reasons.push(format!(
            "OOS evidence rejected: economic_protocol_id {:?} != required {:?} -- not the \
             accepted economic_walk_forward_v1 protocol (may be a legacy/global-fit artifact \
             mistaken for OOS evidence)",
            ev.economic_protocol_id, REQUIRED_ECONOMIC_PROTOCOL_ID
        ));
    }

    if ev.folds_used == 0 {
        reasons.push(
            "OOS evidence rejected: folds_used = 0 -- no discovery-fold OOS evidence exists"
                .to_string(),
        );
    }

    if ev.holdout_status != REQUIRED_HOLDOUT_STATUS {
        reasons.push(format!(
            "OOS evidence rejected: holdout_status {:?} != required {:?} -- the reserved final \
             holdout must remain unconsumed for this promotion decision",
            ev.holdout_status, REQUIRED_HOLDOUT_STATUS
        ));
    }

    if ev.execution_pricing_protocol_id != REQUIRED_EXECUTION_PRICING_PROTOCOL_ID {
        reasons.push(format!(
            "OOS evidence rejected: execution_pricing_protocol_id {:?} != required {:?} -- P7A \
             official execution-pricing parity is not satisfied (diagnostic/close-only pricing \
             cannot count as promotion-grade parity)",
            ev.execution_pricing_protocol_id, REQUIRED_EXECUTION_PRICING_PROTOCOL_ID
        ));
    }

    match &ev.weight_to_share_protocol_id {
        Some(id) if id == REQUIRED_WEIGHT_TO_SHARE_PROTOCOL_ID => {}
        other => reasons.push(format!(
            "OOS evidence rejected: weight_to_share_protocol_id {:?} != required Some({:?}) -- \
             P7B official weight-to-share parity is not satisfied (continuous-weight-only \
             evidence cannot count as promotion-grade parity)",
            other, REQUIRED_WEIGHT_TO_SHARE_PROTOCOL_ID
        )),
    }

    if ev.judge_status != REQUIRED_JUDGE_STATUS {
        reasons.push(format!(
            "OOS evidence rejected: multiple-testing judge_status {:?} != required {:?} \
             (missing, not_evaluable, or only partially evaluable judge evidence cannot count \
             as promotion-grade)",
            ev.judge_status, REQUIRED_JUDGE_STATUS
        ));
    }

    if !ev.judge_trial_evaluable {
        reasons.push(
            "OOS evidence rejected: this candidate's own multiple-testing DSR result was not \
             evaluable (judge_trial_evaluable = false)"
                .to_string(),
        );
    }

    if ev.pbo_status != REQUIRED_PBO_STATUS {
        reasons.push(format!(
            "OOS evidence rejected: PBO status {:?} != required {:?}",
            ev.pbo_status, REQUIRED_PBO_STATUS
        ));
    }

    if !ev.trial_included_in_judge_scope {
        reasons.push(
            "OOS evidence rejected: trial_id is not part of the judged comparison scope \
             (trial_included_in_judge_scope = false) -- wrong/incompatible comparison \
             population"
                .to_string(),
        );
    }

    if ev.economic_eval_id.trim().is_empty() {
        reasons.push(
            "OOS evidence rejected: economic_eval_id is empty -- cannot cross-check this \
             evidence bundle against a specific economic_walk_forward.json artifact"
                .to_string(),
        );
    }

    reasons
}
