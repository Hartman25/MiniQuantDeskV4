use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

// ============================================================================
// P7C-REPAIR-01 (PROMOTION-OOS-EVIDENCE-GATE-01-REPAIR-01)
// ============================================================================
//
// DEFECT FIXED: the original P7C (`PromotionOosEvidence`, now removed) was a
// public, all-`pub`-field, `Deserialize`-able struct populated ENTIRELY by
// the caller. `check_oos_evidence` verified that its ten fields equalled
// expected strings/booleans, but it never authenticated those claims
// against real Research artifacts -- a caller could manually type
// `economic_protocol_id: "economic_walk_forward_v1".to_string()` (etc.) and
// satisfy the gate without a single real artifact existing anywhere. The
// crate's own original positive test demonstrated exactly this weakness by
// hand-constructing a "valid" evidence struct.
//
// FIX: [`VerifiedPromotionOosEvidence`] has PRIVATE fields, derives
// `Serialize` (for audit/report output) but deliberately NOT `Deserialize`
// (a `#[derive(Deserialize)]` impl would itself be a construction path that
// bypasses field privacy -- see `serde`'s codegen, which is expanded in the
// struct's own defining scope and can set private fields regardless of
// external visibility rules). The ONLY way to construct a valid instance in
// production code is [`verify_promotion_oos_evidence`], which parses the
// REAL `economic_walk_forward.json` / multiple-testing-judge JSON / daily
// -returns CSV bytes, hash-binds them to each other, and extracts every
// fact from that verified content -- never from caller-supplied claims.
// [`VerifiedPromotionOosEvidence::valid_for_testing`] is a plain, clearly
// named test-only escape hatch (mirrors [`crate::ArtifactLock::new_for_testing`]'s
// existing convention exactly) for tests that need a passing candidate and
// are not themselves testing the OOS-evidence gate.

/// Required `economic_protocol_id` value -- see
/// `mqk_research.ml.economic_walkforward.PROTOCOL_ID`. A candidate whose
/// evidence carries a different (or legacy global-fit `ml_train_meta_v1`)
/// protocol_id cannot satisfy this gate -- mirrors P6C's evidence-boundary
/// structural distinction.
pub const REQUIRED_ECONOMIC_PROTOCOL_ID: &str = "economic_walk_forward_v1";

/// Required holdout `status` value, checked independently against BOTH the
/// economic artifact's own `"holdout": {"status": ...}` and the judge
/// artifact's own `"holdout": {"status": ...}` -- see
/// `mqk_research.ml.multiple_testing_judge`'s `holdout_status`/`holdout`
/// fields and the holdout consumption ledger
/// (RESEARCH-HOLDOUT-CONSUMPTION-LEDGER-01). Any other value (including a
/// literal "consumed" status) fails closed -- this verifier never scores or
/// consumes the reserved holdout, and never trusts evidence that claims it
/// did.
pub const REQUIRED_HOLDOUT_STATUS: &str = "reserved_not_evaluated";

/// Required `execution_pricing_protocol_id` value (P7A) -- see
/// `mqk_research.ml.execution_pricing.EXECUTION_PRICING_MODEL_ID_RUST_CONSERVATIVE_V1`.
pub const REQUIRED_EXECUTION_PRICING_PROTOCOL_ID: &str = "rust_conservative_bar_range_v1";

/// Required `weight_to_share_protocol_id` value (P7B) -- see
/// `mqk_research.ml.weight_to_share.WEIGHT_TO_SHARE_PROTOCOL_ID_V1`. `None`
/// (missing/null) is the diagnostic/legacy continuous-weight-only state and
/// can never satisfy this gate (mirrors
/// `require_official_weight_to_share_parity`).
pub const REQUIRED_WEIGHT_TO_SHARE_PROTOCOL_ID: &str = "weight_to_share_v1";

/// Required `judge_status` value -- see
/// `mqk_research.ml.multiple_testing_judge`'s own `"judge_status"` field.
/// Deliberately the STRONGEST unambiguous evaluability state
/// (`"evaluated"`), not `"partially_evaluable"`: the judge module itself
/// documents that "partially_evaluable" means either DSR or PBO failed for
/// the population as a whole -- never accepted as promotion-grade.
pub const REQUIRED_JUDGE_STATUS: &str = "evaluated";

/// Required PBO `status` value -- see the judge's own `pbo_result.status`.
pub const REQUIRED_PBO_STATUS: &str = "evaluated";

/// Non-forgeable, structurally VERIFIED OOS evidence for one promotion
/// candidate. Every field was extracted from, and cross-checked against,
/// real `economic_walk_forward.json` / multiple-testing-judge JSON content
/// by [`verify_promotion_oos_evidence`] -- there is no caller-populated
/// construction path in production code. See module docs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VerifiedPromotionOosEvidence {
    trial_id: String,
    economic_eval_id: String,
    folds_used: u32,
    /// Deflated/Probabilistic Sharpe Ratio for THIS trial, extracted from
    /// the judge's own `dsr_results[]` entry matched by `trial_id` (that
    /// entry's `evaluable` was verified `true` and `deflated_sharpe_ratio`
    /// verified finite before this struct could ever be constructed).
    deflated_sharpe_ratio: f64,
    /// Probability of Backtest Overfitting for the comparison POPULATION
    /// this trial belongs to, extracted from the judge's own
    /// `pbo_result.pbo` (a population-level CSCV result, not per-trial;
    /// `pbo_result.status` was verified `"evaluated"` first).
    probability_of_backtest_overfitting: f64,
}

impl VerifiedPromotionOosEvidence {
    pub fn trial_id(&self) -> &str {
        &self.trial_id
    }

    pub fn economic_eval_id(&self) -> &str {
        &self.economic_eval_id
    }

    pub fn folds_used(&self) -> u32 {
        self.folds_used
    }

    pub fn deflated_sharpe_ratio(&self) -> f64 {
        self.deflated_sharpe_ratio
    }

    pub fn probability_of_backtest_overfitting(&self) -> f64 {
        self.probability_of_backtest_overfitting
    }

    /// **Test-only bypass** -- constructs a structurally-passing evidence
    /// bundle WITHOUT running the verifier, for tests that need a passing
    /// candidate and are not themselves testing the OOS-evidence gate.
    /// Mirrors [`crate::ArtifactLock::new_for_testing`]'s naming convention
    /// exactly. Do **not** use in production code.
    pub fn valid_for_testing(trial_id: impl Into<String>) -> Self {
        Self {
            trial_id: trial_id.into(),
            economic_eval_id: "test_synthetic_economic_eval_id".to_string(),
            folds_used: 3,
            deflated_sharpe_ratio: 0.9,
            probability_of_backtest_overfitting: 0.1,
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn get_str<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str()
}

fn get_u64(v: &Value, path: &[&str]) -> Option<u64> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_u64()
}

fn get_f64(v: &Value, path: &[&str]) -> Option<f64> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_f64()
}

fn str_array<'a>(v: &'a Value, path: &[&str]) -> Vec<&'a str> {
    let mut cur = v;
    for key in path {
        match cur.get(key) {
            Some(next) => cur = next,
            None => return Vec::new(),
        }
    }
    cur.as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// Fail-closed structural verification + hash-binding + statistical-
/// threshold extraction (P7C-REPAIR-01 CORE RULE). Parses `economic_walk_
/// forward_json` and `judge_json` (the RAW text content of those real
/// Research artifacts) and `economic_daily_returns_csv` (the raw bytes of
/// the daily-returns CSV the economic artifact claims to have produced),
/// and returns `Ok(VerifiedPromotionOosEvidence)` ONLY when every one of
/// the following holds -- otherwise `Err` with one human-readable reason
/// per failing check (stable order, never a single opaque failure):
///
/// STRUCTURAL (economic artifact):
///   - `protocol.protocol_id` == [`REQUIRED_ECONOMIC_PROTOCOL_ID`]
///   - `aggregate.folds_used` > 0
///   - `holdout.status` == [`REQUIRED_HOLDOUT_STATUS`]
///   - `execution_pricing.pricing_model_id` == [`REQUIRED_EXECUTION_PRICING_PROTOCOL_ID`]
///   - `weight_to_share.weight_to_share_protocol_id` == `Some(`[`REQUIRED_WEIGHT_TO_SHARE_PROTOCOL_ID`]`)`
///   - `ids.economic_eval_id` is non-empty
///
/// HASH BINDING:
///   - the economic artifact's own recorded
///     `outputs.economic_daily_returns_csv.sha256` matches the SHA-256 of
///     the actually-supplied `economic_daily_returns_csv` bytes
///   - the judge artifact's `input_artifacts[]` entry for `trial_id` records
///     an `economic_walk_forward_json_sha256` matching the SHA-256 of the
///     actually-supplied `economic_walk_forward_json` text, AND an
///     `economic_daily_returns_csv_sha256` matching the same daily-returns
///     bytes hash above -- this is what makes it structurally impossible
///     for a caller to pair mismatched/stale/mutated artifacts and still
///     pass.
///
/// STRUCTURAL (judge artifact) + CANDIDATE/TRIAL BINDING:
///   - `judge_status` == [`REQUIRED_JUDGE_STATUS`]
///   - `holdout.status` == [`REQUIRED_HOLDOUT_STATUS`]
///   - `trial_id` is present in `included_trial_ids` (never merely absent
///     from `excluded_trial_ids` -- must be POSITIVELY included)
///   - `economic_eval_id` is present in `input_economic_result_ids`
///   - `dsr_results[]` has an entry for `trial_id` with `evaluable == true`
///     and a finite `deflated_sharpe_ratio`
///   - `pbo_result.status` == [`REQUIRED_PBO_STATUS`] with a finite `pbo`
///
/// Performs ZERO holdout scoring and ZERO filesystem/network I/O -- pure
/// parsing of caller-supplied in-memory content. The caller is responsible
/// for reading these three byte/string buffers from disk exactly once each
/// (this function never re-reads or re-resolves a path).
pub fn verify_promotion_oos_evidence(
    trial_id: &str,
    economic_walk_forward_json: &str,
    economic_daily_returns_csv: &[u8],
    judge_json: &str,
) -> Result<VerifiedPromotionOosEvidence, Vec<String>> {
    let mut errs: Vec<String> = Vec::new();
    let trial_id = trial_id.trim();
    if trial_id.is_empty() {
        errs.push("OOS evidence rejected: trial_id is empty".to_string());
    }

    let econ: Value = match serde_json::from_str(economic_walk_forward_json) {
        Ok(v) => v,
        Err(e) => {
            errs.push(format!(
                "OOS evidence rejected: economic_walk_forward.json is not valid JSON: {e}"
            ));
            return Err(errs);
        }
    };
    let judge: Value = match serde_json::from_str(judge_json) {
        Ok(v) => v,
        Err(e) => {
            errs.push(format!(
                "OOS evidence rejected: multiple-testing judge JSON is not valid JSON: {e}"
            ));
            return Err(errs);
        }
    };

    // ---- economic_walk_forward.json structural checks ----
    let economic_protocol_id = get_str(&econ, &["protocol", "protocol_id"]).unwrap_or_default();
    if economic_protocol_id != REQUIRED_ECONOMIC_PROTOCOL_ID {
        errs.push(format!(
            "OOS evidence rejected: economic_protocol_id {economic_protocol_id:?} != required \
             {REQUIRED_ECONOMIC_PROTOCOL_ID:?} -- not the accepted economic_walk_forward_v1 \
             protocol (may be a legacy/global-fit artifact mistaken for OOS evidence)"
        ));
    }

    let folds_used = get_u64(&econ, &["aggregate", "folds_used"]).unwrap_or(0);
    if folds_used == 0 {
        errs.push(
            "OOS evidence rejected: aggregate.folds_used = 0 or missing -- no discovery-fold \
             OOS evidence exists"
                .to_string(),
        );
    }

    let econ_holdout_status = get_str(&econ, &["holdout", "status"]).unwrap_or_default();
    if econ_holdout_status != REQUIRED_HOLDOUT_STATUS {
        errs.push(format!(
            "OOS evidence rejected: economic artifact holdout.status {econ_holdout_status:?} != \
             required {REQUIRED_HOLDOUT_STATUS:?} -- the reserved final holdout must remain \
             unconsumed for this promotion decision"
        ));
    }

    let execution_pricing_protocol_id =
        get_str(&econ, &["execution_pricing", "pricing_model_id"]).unwrap_or_default();
    if execution_pricing_protocol_id != REQUIRED_EXECUTION_PRICING_PROTOCOL_ID {
        errs.push(format!(
            "OOS evidence rejected: execution_pricing_protocol_id \
             {execution_pricing_protocol_id:?} != required \
             {REQUIRED_EXECUTION_PRICING_PROTOCOL_ID:?} -- P7A official execution-pricing parity \
             is not satisfied (diagnostic/close-only pricing cannot count as promotion-grade \
             parity)"
        ));
    }

    let weight_to_share_protocol_id =
        get_str(&econ, &["weight_to_share", "weight_to_share_protocol_id"]);
    if weight_to_share_protocol_id != Some(REQUIRED_WEIGHT_TO_SHARE_PROTOCOL_ID) {
        errs.push(format!(
            "OOS evidence rejected: weight_to_share_protocol_id {weight_to_share_protocol_id:?} \
             != required Some({REQUIRED_WEIGHT_TO_SHARE_PROTOCOL_ID:?}) -- P7B official \
             weight-to-share parity is not satisfied (continuous-weight-only evidence cannot \
             count as promotion-grade parity)"
        ));
    }

    let economic_eval_id = get_str(&econ, &["ids", "economic_eval_id"])
        .unwrap_or_default()
        .to_string();
    if economic_eval_id.trim().is_empty() {
        errs.push(
            "OOS evidence rejected: economic_eval_id is empty -- cannot cross-check this \
             evidence bundle against a specific economic_walk_forward.json artifact"
                .to_string(),
        );
    }

    // ---- hash binding #1: economic artifact <-> actual daily-returns bytes ----
    let recorded_daily_sha256 =
        get_str(&econ, &["outputs", "economic_daily_returns_csv", "sha256"]).unwrap_or_default();
    let actual_daily_sha256 = sha256_hex(economic_daily_returns_csv);
    if recorded_daily_sha256 != actual_daily_sha256 {
        errs.push(format!(
            "OOS evidence rejected: economic artifact's recorded daily-returns sha256 \
             {recorded_daily_sha256:?} does not match the actual supplied daily-returns bytes \
             sha256 {actual_daily_sha256:?}"
        ));
    }

    let actual_economic_json_sha256 = sha256_hex(economic_walk_forward_json.as_bytes());

    // ---- judge.json structural checks ----
    let judge_status = get_str(&judge, &["judge_status"]).unwrap_or_default();
    if judge_status != REQUIRED_JUDGE_STATUS {
        errs.push(format!(
            "OOS evidence rejected: multiple-testing judge_status {judge_status:?} != required \
             {REQUIRED_JUDGE_STATUS:?} (missing, not_evaluable, or only partially evaluable \
             judge evidence cannot count as promotion-grade)"
        ));
    }

    let judge_holdout_status = get_str(&judge, &["holdout", "status"]).unwrap_or_default();
    if judge_holdout_status != REQUIRED_HOLDOUT_STATUS {
        errs.push(format!(
            "OOS evidence rejected: judge holdout.status {judge_holdout_status:?} != required \
             {REQUIRED_HOLDOUT_STATUS:?}"
        ));
    }

    let included_trial_ids = str_array(&judge, &["included_trial_ids"]);
    if !included_trial_ids.contains(&trial_id) {
        errs.push(format!(
            "OOS evidence rejected: trial_id {trial_id:?} is not part of the judged comparison \
             scope (not present in included_trial_ids) -- wrong/incompatible comparison \
             population"
        ));
    }

    let input_economic_result_ids = str_array(&judge, &["input_economic_result_ids"]);
    if !economic_eval_id.is_empty() && !input_economic_result_ids.contains(&economic_eval_id.as_str()) {
        errs.push(format!(
            "OOS evidence rejected: economic_eval_id {economic_eval_id:?} is not among judge \
             input_economic_result_ids -- this candidate's economic result was not actually \
             scored by the judge"
        ));
    }

    // ---- hash binding #2: judge's recorded artifact hashes for THIS trial ----
    let input_artifacts = judge
        .get("input_artifacts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let matching_artifact = input_artifacts
        .iter()
        .find(|a| a.get("trial_id").and_then(Value::as_str) == Some(trial_id));
    match matching_artifact {
        None => errs.push(format!(
            "OOS evidence rejected: no judge input_artifacts entry for trial_id {trial_id:?}"
        )),
        Some(a) => {
            let recorded_econ_sha256 = a
                .get("economic_walk_forward_json_sha256")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if recorded_econ_sha256 != actual_economic_json_sha256 {
                errs.push(format!(
                    "OOS evidence rejected: judge's recorded economic_walk_forward_json_sha256 \
                     {recorded_econ_sha256:?} for trial_id {trial_id:?} does not match the \
                     actual supplied economic artifact's sha256 {actual_economic_json_sha256:?}"
                ));
            }
            let recorded_csv_sha256 = a
                .get("economic_daily_returns_csv_sha256")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if recorded_csv_sha256 != actual_daily_sha256 {
                errs.push(format!(
                    "OOS evidence rejected: judge's recorded economic_daily_returns_csv_sha256 \
                     {recorded_csv_sha256:?} for trial_id {trial_id:?} does not match the actual \
                     supplied daily-returns bytes sha256 {actual_daily_sha256:?}"
                ));
            }
        }
    }

    // ---- dsr_results[trial_id] ----
    let dsr_results = judge
        .get("dsr_results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let matching_dsr = dsr_results
        .iter()
        .find(|r| r.get("trial_id").and_then(Value::as_str) == Some(trial_id));
    let deflated_sharpe_ratio: Option<f64> = match matching_dsr {
        None => {
            errs.push(format!(
                "OOS evidence rejected: no judge dsr_results entry for trial_id {trial_id:?}"
            ));
            None
        }
        Some(r) => {
            let evaluable = r.get("evaluable").and_then(Value::as_bool).unwrap_or(false);
            if !evaluable {
                errs.push(format!(
                    "OOS evidence rejected: this candidate's own multiple-testing DSR result \
                     was not evaluable (dsr_results[trial_id={trial_id:?}].evaluable = false)"
                ));
                None
            } else {
                match r.get("deflated_sharpe_ratio").and_then(Value::as_f64) {
                    Some(v) if v.is_finite() => Some(v),
                    _ => {
                        errs.push(format!(
                            "OOS evidence rejected: dsr_results[trial_id={trial_id:?}].\
                             deflated_sharpe_ratio is missing or non-finite"
                        ));
                        None
                    }
                }
            }
        }
    };

    // ---- pbo_result ----
    let pbo_status = get_str(&judge, &["pbo_result", "status"]).unwrap_or_default();
    let probability_of_backtest_overfitting: Option<f64> = if pbo_status != REQUIRED_PBO_STATUS {
        errs.push(format!(
            "OOS evidence rejected: PBO status {pbo_status:?} != required {REQUIRED_PBO_STATUS:?}"
        ));
        None
    } else {
        match get_f64(&judge, &["pbo_result", "pbo"]) {
            Some(v) if v.is_finite() => Some(v),
            _ => {
                errs.push(
                    "OOS evidence rejected: pbo_result.pbo is missing or non-finite".to_string(),
                );
                None
            }
        }
    };

    if !errs.is_empty() {
        return Err(errs);
    }

    Ok(VerifiedPromotionOosEvidence {
        trial_id: trial_id.to_string(),
        economic_eval_id,
        folds_used: folds_used as u32,
        deflated_sharpe_ratio: deflated_sharpe_ratio
            .expect("no errs means every field above was successfully extracted"),
        probability_of_backtest_overfitting: probability_of_backtest_overfitting
            .expect("no errs means every field above was successfully extracted"),
    })
}
