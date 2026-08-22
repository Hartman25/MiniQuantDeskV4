use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::research_registry::load_research_authority;

// ============================================================================
// P7C-REPAIR-01 (PROMOTION-OOS-EVIDENCE-GATE-01-REPAIR-01)
// P7C-REPAIR-02 (PROMOTION-OOS-EVIDENCE-GATE-01-REPAIR-02)
// P7C-REPAIR-03 (FINAL WAVE-2 BLOCKER REPAIR, Patch B)
// P7C-REPAIR-04 (FINAL WAVE-2 + MASTER-LEDGER CONSOLIDATION, Patch A)
// ============================================================================
//
// DEFECT FIXED (REPAIR-01): the original P7C (`PromotionOosEvidence`, now
// removed) was a public, all-`pub`-field, `Deserialize`-able struct
// populated ENTIRELY by the caller. `check_oos_evidence` verified that its
// ten fields equalled expected strings/booleans, but it never authenticated
// those claims against real Research artifacts -- a caller could manually
// type `economic_protocol_id: "economic_walk_forward_v1".to_string()` (etc.)
// and satisfy the gate without a single real artifact existing anywhere.
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
//
// DEFECT FIXED (REPAIR-02, mission Section 5A-5G): REPAIR-01's hash-binding
// proved INTEGRITY (the three artifacts are mutually consistent) but never
// AUTHORITY (that the bundle corresponds to any REAL registered Research
// trial/attempt/judge run) -- a caller could still fabricate all three
// artifacts, keep every hash internally consistent, and pass. Fixed by
// requiring an authority record that the supplied artifacts must match,
// including the FULL judge artifact's own SHA-256 (catching a mutated
// DSR/PBO numeric OUTPUT, not merely mutated inputs -- input-hash binding
// alone cannot). Also closes: the missing `discrete_share_economic_path_v1`
// requirement (an evidence-only P7B artifact, lacking real discrete
// economics, could otherwise still satisfy this gate) and incomplete judge
// structural validation (schema_version/protocol_id/comparison_scope were
// never checked). REPAIR-02 also removes the `valid_for_testing` production
// bypass entirely (mission Section 5A).
//
// DEFECT FIXED (REPAIR-03): REPAIR-02's `ResearchAttemptAuthority` was
// itself a public, all-`pub`-field struct constructible by any caller with
// plain struct-literal syntax -- the verifier trusted it, but nothing
// stopped a caller from fabricating an internally self-consistent bundle
// (matching `trial_id`/`economic_eval_id`/judge SHA-256, every hash
// checking out) and simply TYPING those same three strings into an
// authority literal. FIX: authority is no longer caller-supplied at all.
// `verify_promotion_oos_evidence` now takes a `registry_db_path` and
// establishes authority itself by querying the durable Research SQLite
// registry read-only -- see [`crate::research_registry::load_research_authority`]
// for exactly what is checked (trial registered, a succeeded attempt with
// matching `result_id`, a registered judge artifact whose canonical content
// matches the ACTUAL supplied judge JSON and whose experiment/hypothesis
// belongs to this trial). There is no public constructor for an authority
// value anywhere in this crate; a caller cannot manufacture authority by
// typing strings, only by having a real row in the real registry.
//
// DEFECT FIXED (REPAIR-04): REPAIR-03's authority match compared a
// Rust-recomputed hash of the SUPPLIED judge JSON (re-serialized via this
// crate's own `serde_json`) against the registry's stored hash. Python's
// `json.dumps` (which produced the hash actually registered) and Rust's
// `serde_json` are not guaranteed to format every float identically -- the
// SAME value can be spelled `1e-06` by Python and `1e-6` by Rust -- so a
// genuinely authoritative, unmutated judge artifact could hash differently
// and be falsely rejected. FIX: this function no longer computes or passes
// a Rust-side hash of the supplied judge JSON for lookup purposes at all;
// it hands the PARSED judge `Value` to `load_research_authority`, which
// compares it against the registry's own stored canonical text (itself
// integrity-checked against its own hash) using ONE canonicalization --
// see that module's own docs.

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

/// Required `discrete_economics_protocol_id` value, present on EVERY entry
/// of the economic artifact's `folds[]` array whenever discrete shares
/// genuinely drove that fold's economics -- see
/// `mqk_research.ml.weight_to_share.DISCRETE_ECONOMICS_PROTOCOL_ID_V1`.
/// P7C-REPAIR-02 (mission Section 5B, defect C): REQUIRED in addition to
/// `weight_to_share_protocol_id` -- an evidence-only P7B artifact (the
/// translation exists but discrete shares never actually drove the P&L)
/// carries `weight_to_share_protocol_id` but NOT this marker, and must not
/// satisfy promotion.
pub const REQUIRED_DISCRETE_ECONOMICS_PROTOCOL_ID: &str = "discrete_share_economic_path_v1";

/// Required judge artifact `schema_version` -- see
/// `mqk_research.ml.multiple_testing_judge.SCHEMA_VERSION`.
pub const REQUIRED_JUDGE_SCHEMA_VERSION: &str = "multiple_testing_judge_v1";

/// Required judge artifact `protocol.protocol_id` -- see
/// `mqk_research.ml.multiple_testing_judge.PROTOCOL_ID`.
pub const REQUIRED_JUDGE_PROTOCOL_ID: &str = "research_multiple_testing_judge_v1";

// P7C-REPAIR-03: the caller-suppliable `ResearchAttemptAuthority` struct
// that used to live here is gone. Authority is now established exclusively
// by `crate::research_registry::load_research_authority`, which reads the
// durable Research SQLite registry read-only -- see that module's own docs.

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
    /// PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE: this trial's own
    /// registered `strategy_id`, read from the durable Research registry
    /// (never a caller claim) -- see
    /// `crate::research_registry::VerifiedResearchAuthority::strategy_id`.
    strategy_id: String,
}

impl VerifiedPromotionOosEvidence {
    pub fn trial_id(&self) -> &str {
        &self.trial_id
    }

    /// This trial's own registered `strategy_id` (`research_trials.strategy_id`).
    pub fn strategy_id(&self) -> &str {
        &self.strategy_id
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
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
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

/// Fail-closed structural verification + hash-binding + AUTHORITY-anchoring
/// + statistical-threshold extraction (P7C-REPAIR-01/-02 CORE RULE). Parses
/// `economic_walk_forward_json` and `judge_json` (the RAW text content of
/// those real Research artifacts) and `economic_daily_returns_csv` (the raw
/// bytes of the daily-returns CSV the economic artifact claims to have
/// produced), and returns `Ok(VerifiedPromotionOosEvidence)` ONLY when
/// every one of the following holds -- otherwise `Err` with one
/// human-readable reason per failing check (stable order, never a single
/// opaque failure):
///
/// AUTHORITY (P7C-REPAIR-02/-03, established as soon as each fact it needs
/// has been extracted -- mission Section 5C/5D): established by
/// [`crate::research_registry::load_research_authority`] reading the durable
/// Research SQLite registry at `registry_db_path` READ-ONLY -- never from a
/// caller-suppliable struct:
///   - `research_trials` contains `trial_id`
///   - a `research_attempts` row for `trial_id` has `status = 'succeeded'`
///     and `result_id` equal to the economic artifact's own
///     `ids.economic_eval_id`
///   - a `research_judge_artifacts` row's registered canonical content
///     (integrity-verified against its own `judge_artifact_sha256`) is
///     semantically identical to the FULL supplied `judge_json`, and
///     belongs to `trial_id`'s own registered experiment/hypothesis -- this
///     is what a caller CANNOT satisfy merely by keeping the three
///     artifacts internally self-consistent; it requires a durable registry
///     row produced when the judge actually ran (see
///     [`crate::research_registry::load_research_authority`])
///
/// STRUCTURAL (economic artifact):
///   - `protocol.protocol_id` == [`REQUIRED_ECONOMIC_PROTOCOL_ID`]
///   - `aggregate.folds_used` > 0
///   - `holdout.status` == [`REQUIRED_HOLDOUT_STATUS`]
///   - `execution_pricing.pricing_model_id` == [`REQUIRED_EXECUTION_PRICING_PROTOCOL_ID`]
///   - `weight_to_share.weight_to_share_protocol_id` == `Some(`[`REQUIRED_WEIGHT_TO_SHARE_PROTOCOL_ID`]`)`
///   - `folds` is a non-empty array and EVERY entry's
///     `discrete_economics_protocol_id` == [`REQUIRED_DISCRETE_ECONOMICS_PROTOCOL_ID`]
///     (P7C-REPAIR-02, mission Section 5B -- an evidence-only P7B artifact
///     that never actually let discrete shares drive the P&L carries the
///     weight_to_share marker above but not this one)
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
///   - `schema_version` == [`REQUIRED_JUDGE_SCHEMA_VERSION`] (P7C-REPAIR-02,
///     mission Section 5G)
///   - `protocol.protocol_id` == [`REQUIRED_JUDGE_PROTOCOL_ID`] (P7C-REPAIR-02)
///   - `comparison_scope` is structurally present (a non-null, non-empty
///     JSON object) -- an unresolved/absent comparison population cannot
///     back a promotion decision (P7C-REPAIR-02)
///   - `judge_status` == [`REQUIRED_JUDGE_STATUS`]
///   - `holdout.status` == [`REQUIRED_HOLDOUT_STATUS`]
///   - `trial_id` is present in `included_trial_ids` (never merely absent
///     from `excluded_trial_ids` -- must be POSITIVELY included)
///   - `economic_eval_id` is present in `input_economic_result_ids`
///   - `dsr_results[]` has an entry for `trial_id` with `evaluable == true`
///     and a finite `deflated_sharpe_ratio`
///   - `pbo_result.status` == [`REQUIRED_PBO_STATUS`] with a finite `pbo`
///
/// Performs ZERO holdout scoring, but DOES perform one read-only database
/// query (P7C-REPAIR-03): opening `registry_db_path` to establish AUTHORITY
/// -- see [`crate::research_registry::load_research_authority`]. No
/// filesystem write, network access, or holdout access ever occurs. The
/// caller is responsible for reading the three artifact byte/string buffers
/// from disk exactly once each (this function never re-reads or re-resolves
/// an artifact path; `registry_db_path` is the one path this function does
/// open itself, strictly read-only).
pub fn verify_promotion_oos_evidence(
    registry_db_path: &Path,
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

    // P7C-REPAIR-02 (mission Section 5B, defect C): require the DISCRETE
    // economics marker on every fold, not merely the weight_to_share
    // TRANSLATION marker checked above -- proves discrete shares actually
    // drove the economics, not merely that the translation exists.
    let folds = econ.get("folds").and_then(Value::as_array).cloned().unwrap_or_default();
    if folds.is_empty() {
        errs.push(
            "OOS evidence rejected: economic artifact has no folds[] -- no discrete economics \
             evidence to check"
                .to_string(),
        );
    } else {
        for (i, fold) in folds.iter().enumerate() {
            let marker = fold
                .get("discrete_economics_protocol_id")
                .and_then(Value::as_str);
            if marker != Some(REQUIRED_DISCRETE_ECONOMICS_PROTOCOL_ID) {
                errs.push(format!(
                    "OOS evidence rejected: folds[{i}].discrete_economics_protocol_id {marker:?} \
                     != required Some({REQUIRED_DISCRETE_ECONOMICS_PROTOCOL_ID:?}) -- an \
                     evidence-only weight_to_share translation is not sufficient; discrete \
                     shares must have actually driven this fold's economics"
                ));
            }
        }
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

    // P7C-REPAIR-03/-04: AUTHORITY -- establish, from the durable Research
    // registry's own rows (never from a caller-suppliable claim), that
    // `trial_id` is registered, that a succeeded attempt of it has
    // `result_id == economic_eval_id`, and that a registered judge artifact
    // whose registered canonical content matches the ACTUAL supplied judge
    // JSON belongs to this trial's own experiment/hypothesis. REPAIR-04:
    // the supplied judge `Value` is passed through directly rather than
    // hashed with this crate's own serializer first -- Python's
    // `json.dumps` and Rust's `serde_json` are not guaranteed to format
    // every float identically (e.g. `1e-06` vs `1e-6` for the same value),
    // so a Rust-side re-hash of the supplied JSON could never reliably
    // match a hash Python computed from its own canonical text. See
    // `crate::research_registry::load_research_authority`, which instead
    // reads the registry's OWN stored canonical text, verifies its
    // integrity against its own `judge_artifact_sha256`, and compares
    // PARSED JSON VALUES using a single (Rust-side) canonicalization.
    let mut research_strategy_id: Option<String> = None;
    match load_research_authority(registry_db_path, trial_id, &economic_eval_id, &judge) {
        Ok(authority) => research_strategy_id = Some(authority.strategy_id),
        Err(authority_errs) => errs.extend(authority_errs),
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
    // P7C-REPAIR-02 (mission Section 5G): reject an unknown judge
    // schema/protocol outright -- REPAIR-01 never checked these, so a
    // caller supplying an arbitrary JSON shape (as long as it happened to
    // carry the specific fields REPAIR-01 read) could satisfy the gate.
    let judge_schema_version = get_str(&judge, &["schema_version"]).unwrap_or_default();
    if judge_schema_version != REQUIRED_JUDGE_SCHEMA_VERSION {
        errs.push(format!(
            "OOS evidence rejected: judge schema_version {judge_schema_version:?} != required \
             {REQUIRED_JUDGE_SCHEMA_VERSION:?} -- unknown/unsupported judge artifact shape"
        ));
    }
    let judge_protocol_id = get_str(&judge, &["protocol", "protocol_id"]).unwrap_or_default();
    if judge_protocol_id != REQUIRED_JUDGE_PROTOCOL_ID {
        errs.push(format!(
            "OOS evidence rejected: judge protocol.protocol_id {judge_protocol_id:?} != required \
             {REQUIRED_JUDGE_PROTOCOL_ID:?} -- unknown/unsupported judge methodology"
        ));
    }
    match judge.get("comparison_scope") {
        Some(Value::Object(map)) if !map.is_empty() => {}
        other => errs.push(format!(
            "OOS evidence rejected: judge comparison_scope is missing, null, or an empty/\
             malformed object ({other:?}) -- no valid comparison population was resolved for \
             this judge run"
        )),
    }

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
        strategy_id: research_strategy_id
            .expect("no errs means load_research_authority returned Ok above"),
    })
}
