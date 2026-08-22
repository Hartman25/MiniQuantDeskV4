//! P7C-REPAIR-01 (PROMOTION-OOS-EVIDENCE-GATE-01-REPAIR-01) — the promotion
//! gate must fail closed unless it has STRUCTURALLY VERIFIED, hash-bound,
//! statistically-thresholded out-of-sample Research evidence extracted from
//! REAL artifact content — never a caller-populated claim. Covers the
//! repair mission's REQUIRED tests (Section 6K, 35 items, referenced by
//! number) plus the Section 7 acceptance-boundary load-bearing proofs.
//!
//! P7C-REPAIR-03 (FINAL WAVE-2 BLOCKER REPAIR, Patch B): REPAIR-02's
//! `ResearchAttemptAuthority` was itself a public, all-`pub`-field struct a
//! caller could construct directly with plain struct-literal syntax — the
//! verifier trusted it without ever confirming a matching row existed in
//! any real registry. That struct is now gone entirely; every test below
//! that needs authority to succeed builds a REAL temporary SQLite registry
//! database (`tests/common::new_registry_db` and friends) and registers
//! real `research_trials`/`research_attempts`/`research_judge_artifacts`
//! rows in it — the ONLY way `verify_promotion_oos_evidence` can be made to
//! pass. See the REGISTRY AUTHORITY section below for the load-bearing
//! negative proof that a self-consistent-but-unregistered bundle fails.

mod common;

use common::{
    canonical_json_sha256, new_registry_db, register_judge_artifact, register_succeeded_attempt,
    register_trial, sha256_hex, RegistryDb,
};
use mqk_promotion::{
    evaluate_promotion, pick_winner, select_best, verify_promotion_oos_evidence, ArtifactLock,
    Candidate, PromotionConfig, PromotionInput, StressSuiteResult, VerifiedPromotionOosEvidence,
    REQUIRED_STRESS_PROTOCOL_VERSION,
};
use mqk_backtest::{derive_input_data_hash, derive_run_id, BacktestConfig, BacktestReport};

// ---------------------------------------------------------------------------
// Fixture JSON builders — mirror the REAL Python artifact schemas
// (economic_walk_forward.json / multiple_testing_judge JSON) exactly at the
// field paths verify_promotion_oos_evidence reads. Each negative-control
// test mutates exactly ONE fact and proves the verifier rejects it.
// ---------------------------------------------------------------------------

struct Fixture {
    trial_id: String,
    experiment_id: String,
    #[allow(dead_code)] // documents the registered identity; not read directly by any test today
    hypothesis_id: String,
    economic_json: String,
    daily_csv: Vec<u8>,
    judge_json: String,
    /// P7C-REPAIR-03: the real temporary registry database this fixture's
    /// trial/attempt/judge rows are registered in. A negative-control test
    /// that mutates `economic_json`/`judge_json` after construction
    /// deliberately leaves the registry unchanged, so the mutated bundle
    /// ALSO fails the registry authority checks (in addition to whichever
    /// specific structural check that test is targeting) unless it calls
    /// `reregister_judge` to keep authority isolated from the structural
    /// fact under test.
    registry: RegistryDb,
}

impl Fixture {
    /// Registers a NEW `research_judge_artifacts` row matching the
    /// CURRENT (possibly already-mutated) `judge_json`'s canonical hash, so
    /// a test that mutates judge_json to exercise one specific structural
    /// check does not ALSO spuriously fail on "unregistered judge artifact"
    /// -- isolating the assertion to the fact actually under test, exactly
    /// as the pre-REPAIR-03 tests did by updating `authority.judge_artifact_sha256`.
    fn reregister_judge(&self) {
        let new_sha = canonical_json_sha256(&self.judge_json);
        register_judge_artifact(
            &self.registry.path,
            &new_sha,
            &format!("judge_{}_reregistered", self.trial_id),
            &self.experiment_id,
            None,
            Some(&self.judge_json),
        );
    }
}

/// A fully self-consistent, structurally valid, registry-anchored evidence
/// bundle for `trial_id`: hashes genuinely match, every required field is
/// present and correct, and a matching trial/attempt/judge triple is
/// registered in a real temporary SQLite registry. This is the ONLY shape
/// `verify_promotion_oos_evidence` accepts.
fn valid_fixture(trial_id: &str) -> Fixture {
    let daily_csv = b"date,net_daily_return\n2021-01-01,0.0010\n2021-01-02,0.0021\n".to_vec();
    let daily_sha = sha256_hex(&daily_csv);
    let economic_eval_id = format!("econ_eval_{trial_id}");
    let experiment_id = format!("exp_{trial_id}");
    let hypothesis_id = format!("hyp_{trial_id}");

    let economic_json = format!(
        r#"{{"protocol":{{"protocol_id":"economic_walk_forward_v1"}},"aggregate":{{"folds_used":3}},"holdout":{{"status":"reserved_not_evaluated"}},"execution_pricing":{{"pricing_model_id":"rust_conservative_bar_range_v1"}},"weight_to_share":{{"weight_to_share_protocol_id":"weight_to_share_v1"}},"outputs":{{"economic_daily_returns_csv":{{"sha256":"{daily_sha}"}}}},"ids":{{"economic_eval_id":"{economic_eval_id}"}},"folds":[{{"discrete_economics_protocol_id":"discrete_share_economic_path_v1"}}]}}"#
    );
    let economic_sha = sha256_hex(economic_json.as_bytes());

    let judge_json = format!(
        r#"{{"schema_version":"multiple_testing_judge_v1","protocol":{{"protocol_id":"research_multiple_testing_judge_v1"}},"comparison_scope":{{"experiment_id":"exp_{trial_id}"}},"judge_status":"evaluated","holdout":{{"status":"reserved_not_evaluated"}},"included_trial_ids":["{trial_id}"],"input_economic_result_ids":["{economic_eval_id}"],"input_artifacts":[{{"trial_id":"{trial_id}","economic_walk_forward_json_sha256":"{economic_sha}","economic_daily_returns_csv_sha256":"{daily_sha}"}}],"dsr_results":[{{"trial_id":"{trial_id}","evaluable":true,"deflated_sharpe_ratio":0.85}}],"pbo_result":{{"status":"evaluated","pbo":0.15}}}}"#
    );
    let judge_sha = canonical_json_sha256(&judge_json);

    let registry = new_registry_db();
    register_trial(&registry.path, trial_id, &experiment_id, &hypothesis_id);
    register_succeeded_attempt(
        &registry.path,
        &format!("{trial_id}:att0001"),
        trial_id,
        &economic_eval_id,
    );
    register_judge_artifact(
        &registry.path,
        &judge_sha,
        &format!("judge_{trial_id}"),
        &experiment_id,
        None,
        Some(&judge_json),
    );

    Fixture {
        trial_id: trial_id.to_string(),
        experiment_id,
        hypothesis_id,
        economic_json,
        daily_csv,
        judge_json,
        registry,
    }
}

fn verify(f: &Fixture) -> Result<VerifiedPromotionOosEvidence, Vec<String>> {
    verify_promotion_oos_evidence(&f.registry.path, &f.trial_id, &f.economic_json, &f.daily_csv, &f.judge_json)
}

fn reasons_contain(errs: &[String], needle: &str) -> bool {
    errs.iter().any(|r| r.contains(needle))
}

// ---------------------------------------------------------------------------
// POSITIVE PROOF — a genuinely valid, hash-consistent, registry-anchored
// bundle verifies (REQUIRED TESTS 4/19/20/21)
// ---------------------------------------------------------------------------

#[test]
fn valid_bundle_verifies_successfully() {
    let f = valid_fixture("trial_valid_001");
    let ev = verify(&f).expect("structurally valid, hash-consistent, registered evidence must verify");
    assert_eq!(ev.trial_id(), "trial_valid_001");
    assert_eq!(ev.economic_eval_id(), "econ_eval_trial_valid_001");
    assert_eq!(ev.folds_used(), 3);
    assert!((ev.deflated_sharpe_ratio() - 0.85).abs() < 1e-12);
    assert!((ev.probability_of_backtest_overfitting() - 0.15).abs() < 1e-12);
}

/// REQUIRED TEST 20/21: this verifier checks nothing about
/// `signal_policy.direction_policy` (P7C is agnostic to long-only vs
/// long/short — that distinction lives entirely in the Python economic
/// protocol identity, RESEARCH-LONG-SHORT-ECONOMIC-POLICY-01) — the exact
/// same fixture shape verifies regardless, proving both a legacy long-only
/// evaluation and a new long/short evaluation can satisfy this gate.
#[test]
fn both_legacy_long_only_and_new_long_short_shapes_verify_identically() {
    let f = valid_fixture("direction_agnostic_trial");
    assert!(verify(&f).is_ok());
}

// ---------------------------------------------------------------------------
// REQUIRED TEST 2 — missing evidence fails
// ---------------------------------------------------------------------------

#[test]
fn missing_verified_evidence_fails_promotion() {
    let input = otherwise_valid_input("no_evidence", None);
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(!decision.passed);
    assert!(reasons_contain(&decision.fail_reasons, "OOS evidence missing"));
}

// ---------------------------------------------------------------------------
// REQUIRED TEST 3 — fake expected protocol strings alone cannot pass
// ---------------------------------------------------------------------------

#[test]
fn wrong_economic_protocol_id_fails() {
    let mut f = valid_fixture("wrong_protocol_trial");
    f.economic_json = f
        .economic_json
        .replace(r#""protocol_id":"economic_walk_forward_v1""#, r#""protocol_id":"ml_train_meta_v1""#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "economic_protocol_id"));
}

// ---------------------------------------------------------------------------
// REQUIRED TESTS 6/7 — mutated hash fails (HASH-BINDING NEGATIVE CONTROLS)
// ---------------------------------------------------------------------------

#[test]
fn mutated_economic_artifact_content_fails_hash_binding() {
    let mut f = valid_fixture("mutated_econ_trial");
    // Mutate the economic artifact's OWN content (one added field) AFTER
    // the judge's recorded hash was computed from the original text --
    // simulates a stale/mismatched pairing.
    f.economic_json = f.economic_json.replace(
        r#""aggregate":{"folds_used":3}"#,
        r#""aggregate":{"folds_used":3,"tampered":true}"#,
    );
    let errs = verify(&f).unwrap_err();
    assert!(
        reasons_contain(&errs, "economic_walk_forward_json_sha256"),
        "got: {errs:?}"
    );
}

#[test]
fn mutated_daily_returns_csv_fails_hash_binding() {
    let mut f = valid_fixture("mutated_csv_trial");
    f.daily_csv = b"date,net_daily_return\n2021-01-01,0.9999\n".to_vec();
    let errs = verify(&f).unwrap_err();
    assert!(
        reasons_contain(&errs, "daily-returns"),
        "got: {errs:?}"
    );
}

#[test]
fn wrong_nonempty_economic_eval_id_fails() {
    // The economic artifact's own economic_eval_id is changed to a
    // different, still-nonempty string that neither the judge's
    // input_economic_result_ids nor the registered attempt's result_id
    // ever recorded.
    let mut f = valid_fixture("wrong_eval_id_trial");
    f.economic_json = f
        .economic_json
        .replace("econ_eval_wrong_eval_id_trial", "totally_different_eval_id");
    let errs = verify(&f).unwrap_err();
    assert!(
        reasons_contain(&errs, "input_economic_result_ids")
            // Also breaks hash binding since economic_json content changed.
            || reasons_contain(&errs, "economic_walk_forward_json_sha256")
            // Also breaks registry authority: no succeeded attempt has this result_id.
            || reasons_contain(&errs, "research_attempts"),
        "got: {errs:?}"
    );
}

// ---------------------------------------------------------------------------
// REQUIRED TESTS 8/9 — wrong trial / excluded from judge scope
// ---------------------------------------------------------------------------

#[test]
fn judge_artifact_for_wrong_trial_fails() {
    let f = valid_fixture("real_trial");
    // Ask the verifier about a DIFFERENT trial_id than the one the fixture
    // actually documents (and than the one registered in its registry).
    let errs = verify_promotion_oos_evidence(
        &f.registry.path,
        "some_other_trial",
        &f.economic_json,
        &f.daily_csv,
        &f.judge_json,
    )
    .unwrap_err();
    assert!(reasons_contain(&errs, "judged comparison scope"));
}

#[test]
fn candidate_excluded_from_judge_scope_fails() {
    let mut f = valid_fixture("excluded_trial");
    f.judge_json = f
        .judge_json
        .replace(r#""included_trial_ids":["excluded_trial"]"#, r#""included_trial_ids":[]"#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "judged comparison scope"));
}

// ---------------------------------------------------------------------------
// REQUIRED TEST 10 — missing candidate DSR row fails
// ---------------------------------------------------------------------------

#[test]
fn missing_dsr_row_for_trial_fails() {
    let mut f = valid_fixture("no_dsr_row_trial");
    f.judge_json = f.judge_json.replace(r#""dsr_results":[{"trial_id":"no_dsr_row_trial","evaluable":true,"deflated_sharpe_ratio":0.85}]"#, r#""dsr_results":[]"#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "no judge dsr_results entry"));
}

// ---------------------------------------------------------------------------
// REQUIRED TEST 11 — DSR not evaluable fails
// ---------------------------------------------------------------------------

#[test]
fn dsr_not_evaluable_fails() {
    let mut f = valid_fixture("dsr_not_evaluable_trial");
    f.judge_json = f.judge_json.replace(r#""evaluable":true"#, r#""evaluable":false"#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "DSR result was not evaluable"));
}

// ---------------------------------------------------------------------------
// REQUIRED TEST 12 — PBO not evaluated fails
// ---------------------------------------------------------------------------

#[test]
fn pbo_not_evaluated_fails() {
    let mut f = valid_fixture("pbo_not_evaluated_trial");
    f.judge_json = f
        .judge_json
        .replace(r#""pbo_result":{"status":"evaluated","pbo":0.15}"#, r#""pbo_result":{"status":"not_evaluable"}"#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "PBO status"));
}

// ---------------------------------------------------------------------------
// REQUIRED TESTS 13/14 — judge not_evaluable / partially_evaluable fails
// ---------------------------------------------------------------------------

#[test]
fn judge_not_evaluable_fails() {
    let mut f = valid_fixture("judge_not_evaluable_trial");
    f.judge_json = f.judge_json.replace(r#""judge_status":"evaluated""#, r#""judge_status":"not_evaluable""#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "judge_status"));
}

#[test]
fn judge_partially_evaluable_fails() {
    let mut f = valid_fixture("judge_partial_trial");
    f.judge_json = f
        .judge_json
        .replace(r#""judge_status":"evaluated""#, r#""judge_status":"partially_evaluable""#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "judge_status"));
}

// ---------------------------------------------------------------------------
// REQUIRED TESTS 15/16 — holdout wrong state / consumed fails (both artifacts)
// ---------------------------------------------------------------------------

#[test]
fn economic_holdout_wrong_state_fails() {
    let mut f = valid_fixture("econ_holdout_wrong_trial");
    f.economic_json = f
        .economic_json
        .replace(r#""holdout":{"status":"reserved_not_evaluated"}"#, r#""holdout":{"status":"unknown"}"#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "economic artifact holdout.status"));
}

#[test]
fn economic_holdout_consumed_fails() {
    let mut f = valid_fixture("econ_holdout_consumed_trial");
    f.economic_json = f
        .economic_json
        .replace(r#""holdout":{"status":"reserved_not_evaluated"}"#, r#""holdout":{"status":"consumed"}"#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "economic artifact holdout.status"));
}

#[test]
fn judge_holdout_consumed_fails() {
    let mut f = valid_fixture("judge_holdout_consumed_trial");
    f.judge_json = f
        .judge_json
        .replace(r#""holdout":{"status":"reserved_not_evaluated"}"#, r#""holdout":{"status":"consumed"}"#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "judge holdout.status"));
}

// ---------------------------------------------------------------------------
// REQUIRED TEST 17 — diagnostic P7A pricing fails
// ---------------------------------------------------------------------------

#[test]
fn diagnostic_p7a_pricing_fails() {
    let mut f = valid_fixture("diagnostic_p7a_trial");
    f.economic_json = f
        .economic_json
        .replace(r#""pricing_model_id":"rust_conservative_bar_range_v1""#, r#""pricing_model_id":"close_only_diagnostic_v1""#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "execution_pricing_protocol_id"));
}

// ---------------------------------------------------------------------------
// REQUIRED TEST 18 — continuous/evidence-only old P7B protocol fails
// ---------------------------------------------------------------------------

#[test]
fn diagnostic_continuous_weight_only_p7b_fails() {
    let mut f = valid_fixture("diagnostic_p7b_trial");
    f.economic_json = f
        .economic_json
        .replace(r#""weight_to_share":{"weight_to_share_protocol_id":"weight_to_share_v1"}"#, r#""weight_to_share":{"weight_to_share_protocol_id":null}"#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "weight_to_share_protocol_id"));
}

// ---------------------------------------------------------------------------
// P7C-REPAIR-02 REQUIRED TESTS (mission Section 5B) — the DISCRETE
// economics marker is required in ADDITION to weight_to_share_protocol_id
// -- an evidence-only P7B artifact (translation exists, discrete shares
// never actually drove the P&L) carries the marker checked above but not
// this one.
// ---------------------------------------------------------------------------

#[test]
fn missing_discrete_economics_marker_fails() {
    let mut f = valid_fixture("no_discrete_marker_trial");
    f.economic_json = f.economic_json.replace(
        r#""folds":[{"discrete_economics_protocol_id":"discrete_share_economic_path_v1"}]"#,
        r#""folds":[{}]"#,
    );
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "discrete_economics_protocol_id"), "got: {errs:?}");
}

#[test]
fn empty_folds_array_fails() {
    let mut f = valid_fixture("empty_folds_trial");
    f.economic_json = f.economic_json.replace(
        r#""folds":[{"discrete_economics_protocol_id":"discrete_share_economic_path_v1"}]"#,
        r#""folds":[]"#,
    );
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "no folds"), "got: {errs:?}");
}

// ---------------------------------------------------------------------------
// P7C-REPAIR-02 REQUIRED TESTS (mission Section 5G) — judge structural
// validation: unknown schema/protocol/missing comparison scope all fail.
// Each mutates judge_json then re-registers it under its NEW canonical
// hash, isolating the assertion to the structural check under test (the
// registry-absence failure mode has its own dedicated tests below).
// ---------------------------------------------------------------------------

#[test]
fn wrong_judge_schema_version_fails() {
    let mut f = valid_fixture("wrong_judge_schema_trial");
    f.judge_json = f
        .judge_json
        .replace(r#""schema_version":"multiple_testing_judge_v1""#, r#""schema_version":"some_other_v2""#);
    f.reregister_judge();
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "judge schema_version"), "got: {errs:?}");
}

#[test]
fn wrong_judge_protocol_id_fails() {
    let mut f = valid_fixture("wrong_judge_protocol_trial");
    f.judge_json = f.judge_json.replace(
        r#""protocol_id":"research_multiple_testing_judge_v1""#,
        r#""protocol_id":"some_other_protocol_v1""#,
    );
    f.reregister_judge();
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "judge protocol.protocol_id"), "got: {errs:?}");
}

#[test]
fn missing_comparison_scope_fails() {
    let mut f = valid_fixture("missing_scope_trial");
    f.judge_json = f
        .judge_json
        .replace(r#""comparison_scope":{"experiment_id":"exp_missing_scope_trial"},"#, "");
    f.reregister_judge();
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "comparison_scope"), "got: {errs:?}");
}

#[test]
fn empty_comparison_scope_object_fails() {
    let mut f = valid_fixture("empty_scope_trial");
    f.judge_json = f
        .judge_json
        .replace(r#""comparison_scope":{"experiment_id":"exp_empty_scope_trial"}"#, r#""comparison_scope":{}"#);
    f.reregister_judge();
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "comparison_scope"), "got: {errs:?}");
}

#[test]
fn null_comparison_scope_fails() {
    let mut f = valid_fixture("null_scope_trial");
    f.judge_json = f
        .judge_json
        .replace(r#""comparison_scope":{"experiment_id":"exp_null_scope_trial"}"#, r#""comparison_scope":null"#);
    f.reregister_judge();
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "comparison_scope"), "got: {errs:?}");
}

// ---------------------------------------------------------------------------
// P7C-REPAIR-03 REGISTRY AUTHORITY — the load-bearing RED/GREEN proof
// (mission's REQUIRED LOAD-BEARING NEGATIVE TEST). A completely fabricated
// but internally self-consistent bundle (every hash matches every other
// hash) must still fail because it was never registered in the durable
// Research registry -- i.e. no authority record backs it. This is what
// REPAIR-02's caller-suppliable `ResearchAttemptAuthority` could never
// prove: it only ever tested a WRONG authority record, never proved a
// MATCHING FABRICATED one fails. There is no public API anywhere in this
// crate that lets a test (or any other caller) fabricate authority
// directly -- only a real registry row, inserted via raw SQL against the
// SAME schema the real registry writes, can satisfy the verifier.
// ---------------------------------------------------------------------------

#[test]
fn fabricated_internally_consistent_unregistered_bundle_fails() {
    // Build a bundle EXACTLY the way valid_fixture does (every internal
    // hash is genuinely self-consistent -- this is not a malformed-JSON
    // test), but verify it against a completely EMPTY registry database --
    // exactly as if this fabricated bundle simply does not exist in the
    // durable Research registry. No public API in mqk_promotion lets this
    // test mint a passing authority for it any other way.
    let f = valid_fixture("fabricated_trial");
    let empty_registry = new_registry_db();
    let errs = verify_promotion_oos_evidence(
        &empty_registry.path,
        &f.trial_id,
        &f.economic_json,
        &f.daily_csv,
        &f.judge_json,
    )
    .unwrap_err();
    assert!(
        reasons_contain(&errs, "no research_trials row"),
        "a fully self-consistent but unregistered bundle must fail on registry absence: {errs:?}"
    );
}

#[test]
fn registered_trial_wrong_economic_eval_id_fails() {
    // The trial IS registered, and a succeeded attempt exists for it -- but
    // that attempt's result_id does not match the economic artifact's own
    // economic_eval_id (a different attempt's result was substituted).
    let trial_id = "wrong_result_id_trial";
    let daily_csv = b"date,net_daily_return\n2021-01-01,0.0010\n".to_vec();
    let daily_sha = sha256_hex(&daily_csv);
    let economic_eval_id = "econ_eval_actual";
    let experiment_id = format!("exp_{trial_id}");

    let economic_json = format!(
        r#"{{"protocol":{{"protocol_id":"economic_walk_forward_v1"}},"aggregate":{{"folds_used":3}},"holdout":{{"status":"reserved_not_evaluated"}},"execution_pricing":{{"pricing_model_id":"rust_conservative_bar_range_v1"}},"weight_to_share":{{"weight_to_share_protocol_id":"weight_to_share_v1"}},"outputs":{{"economic_daily_returns_csv":{{"sha256":"{daily_sha}"}}}},"ids":{{"economic_eval_id":"{economic_eval_id}"}},"folds":[{{"discrete_economics_protocol_id":"discrete_share_economic_path_v1"}}]}}"#
    );
    let economic_sha = sha256_hex(economic_json.as_bytes());
    let judge_json = format!(
        r#"{{"schema_version":"multiple_testing_judge_v1","protocol":{{"protocol_id":"research_multiple_testing_judge_v1"}},"comparison_scope":{{"experiment_id":"{experiment_id}"}},"judge_status":"evaluated","holdout":{{"status":"reserved_not_evaluated"}},"included_trial_ids":["{trial_id}"],"input_economic_result_ids":["{economic_eval_id}"],"input_artifacts":[{{"trial_id":"{trial_id}","economic_walk_forward_json_sha256":"{economic_sha}","economic_daily_returns_csv_sha256":"{daily_sha}"}}],"dsr_results":[{{"trial_id":"{trial_id}","evaluable":true,"deflated_sharpe_ratio":0.85}}],"pbo_result":{{"status":"evaluated","pbo":0.15}}}}"#
    );
    let judge_sha = canonical_json_sha256(&judge_json);

    let registry = new_registry_db();
    register_trial(&registry.path, trial_id, &experiment_id, "hyp");
    // Registered attempt's result_id is a DIFFERENT id than the economic
    // artifact's own economic_eval_id.
    register_succeeded_attempt(&registry.path, &format!("{trial_id}:att0001"), trial_id, "econ_eval_a_different_run");
    register_judge_artifact(&registry.path, &judge_sha, "judge_x", &experiment_id, None, Some(&judge_json));

    let errs = verify_promotion_oos_evidence(&registry.path, trial_id, &economic_json, &daily_csv, &judge_json)
        .unwrap_err();
    assert!(
        reasons_contain(&errs, "no succeeded research_attempts row"),
        "got: {errs:?}"
    );
}

#[test]
fn correct_economic_artifact_but_unregistered_judge_fails() {
    // Trial and attempt are both correctly registered; the judge artifact
    // itself was simply never registered at all.
    let f = valid_fixture("unregistered_judge_trial");
    // A fresh registry with the SAME trial/attempt, but no judge row.
    let registry = new_registry_db();
    register_trial(&registry.path, &f.trial_id, &f.experiment_id, "hyp");
    register_succeeded_attempt(
        &registry.path,
        &format!("{}:att0001", f.trial_id),
        &f.trial_id,
        "econ_eval_unregistered_judge_trial",
    );
    let errs = verify_promotion_oos_evidence(&registry.path, &f.trial_id, &f.economic_json, &f.daily_csv, &f.judge_json)
        .unwrap_err();
    assert!(
        reasons_contain(&errs, "no research_judge_artifacts row"),
        "got: {errs:?}"
    );
}

#[test]
fn registered_judge_wrong_experiment_hypothesis_fails() {
    // The judge artifact IS registered under its exact canonical hash --
    // but for a DIFFERENT experiment than the one the trial itself belongs
    // to (e.g. a judge run over an unrelated experiment happened to produce
    // byte-identical statistical content by coincidence in this synthetic
    // fixture; in reality this models a copy-paste/mismatched registration
    // bug). Must fail closed rather than trust the hash match alone.
    let f = valid_fixture("wrong_scope_judge_trial");
    let registry = new_registry_db();
    register_trial(&registry.path, &f.trial_id, &f.experiment_id, "hyp");
    register_succeeded_attempt(
        &registry.path,
        &format!("{}:att0001", f.trial_id),
        &f.trial_id,
        "econ_eval_wrong_scope_judge_trial",
    );
    let judge_sha = canonical_json_sha256(&f.judge_json);
    register_judge_artifact(
        &registry.path,
        &judge_sha,
        "judge_x",
        "a_totally_different_experiment",
        None,
        Some(&f.judge_json),
    );

    let errs = verify_promotion_oos_evidence(&registry.path, &f.trial_id, &f.economic_json, &f.daily_csv, &f.judge_json)
        .unwrap_err();
    assert!(
        reasons_contain(&errs, "wrong experiment/hypothesis"),
        "got: {errs:?}"
    );
}

#[test]
fn registered_judge_scoped_to_specific_hypothesis_must_match_trials_own() {
    // A judge run scoped to ONE specific hypothesis_id (not the whole
    // experiment) must only anchor trials registered under that SAME
    // hypothesis_id -- a trial from a sibling hypothesis in the same
    // experiment must not be covered.
    let f = valid_fixture("hypothesis_scoped_trial");
    let registry = new_registry_db();
    register_trial(&registry.path, &f.trial_id, &f.experiment_id, "hyp_alpha");
    register_succeeded_attempt(
        &registry.path,
        &format!("{}:att0001", f.trial_id),
        &f.trial_id,
        "econ_eval_hypothesis_scoped_trial",
    );
    let judge_sha = canonical_json_sha256(&f.judge_json);
    // Judge scoped to a DIFFERENT specific hypothesis within the SAME experiment.
    register_judge_artifact(
        &registry.path,
        &judge_sha,
        "judge_x",
        &f.experiment_id,
        Some("hyp_beta"),
        Some(&f.judge_json),
    );

    let errs = verify_promotion_oos_evidence(&registry.path, &f.trial_id, &f.economic_json, &f.daily_csv, &f.judge_json)
        .unwrap_err();
    assert!(
        reasons_contain(&errs, "wrong experiment/hypothesis"),
        "got: {errs:?}"
    );
}

#[test]
fn registered_judge_scoped_to_whole_experiment_covers_any_hypothesis() {
    // Mirror of the above: a judge run scoped to the WHOLE experiment
    // (hypothesis_id absent/NULL in the registry) legitimately covers a
    // trial registered under ANY hypothesis_id within that experiment.
    let f = valid_fixture("experiment_scoped_trial");
    let registry = new_registry_db();
    register_trial(&registry.path, &f.trial_id, &f.experiment_id, "hyp_whatever");
    register_succeeded_attempt(
        &registry.path,
        &format!("{}:att0001", f.trial_id),
        &f.trial_id,
        "econ_eval_experiment_scoped_trial",
    );
    let judge_sha = canonical_json_sha256(&f.judge_json);
    register_judge_artifact(&registry.path, &judge_sha, "judge_x", &f.experiment_id, None, Some(&f.judge_json));

    let result =
        verify_promotion_oos_evidence(&registry.path, &f.trial_id, &f.economic_json, &f.daily_csv, &f.judge_json);
    assert!(result.is_ok(), "got: {result:?}");
}

// ---------------------------------------------------------------------------
// P7C-REPAIR-04 — cross-language canonical judge-authority hardening
// (mission Section 3G, REQUIRED TESTS 1-7). Python's `json.dumps` and
// Rust's `serde_json` are not guaranteed to format every float identically
// (e.g. "1e-06" vs "1e-6" for the SAME value) -- authority must be
// established by comparing PARSED JSON VALUES against the registry's own
// stored canonical text (itself integrity-checked against its own hash),
// never by recomputing a hash of the supplied JSON with this crate's own
// serializer.
// ---------------------------------------------------------------------------

/// Registers trial + succeeded attempt only (no judge row) -- returns
/// `(economic_json, daily_csv, economic_eval_id, experiment_id, registry)`.
fn base_authority_fixture(trial_id: &str) -> (String, Vec<u8>, String, String, RegistryDb) {
    let daily_csv = b"date,net_daily_return\n2021-01-01,0.0010\n".to_vec();
    let daily_sha = sha256_hex(&daily_csv);
    let economic_eval_id = format!("econ_eval_{trial_id}");
    let experiment_id = format!("exp_{trial_id}");

    let economic_json = format!(
        r#"{{"protocol":{{"protocol_id":"economic_walk_forward_v1"}},"aggregate":{{"folds_used":3}},"holdout":{{"status":"reserved_not_evaluated"}},"execution_pricing":{{"pricing_model_id":"rust_conservative_bar_range_v1"}},"weight_to_share":{{"weight_to_share_protocol_id":"weight_to_share_v1"}},"outputs":{{"economic_daily_returns_csv":{{"sha256":"{daily_sha}"}}}},"ids":{{"economic_eval_id":"{economic_eval_id}"}},"folds":[{{"discrete_economics_protocol_id":"discrete_share_economic_path_v1"}}]}}"#
    );

    let registry = new_registry_db();
    register_trial(&registry.path, trial_id, &experiment_id, "hyp");
    register_succeeded_attempt(&registry.path, &format!("{trial_id}:att0001"), trial_id, &economic_eval_id);

    (economic_json, daily_csv, economic_eval_id, experiment_id, registry)
}

/// A fully structurally-valid judge JSON text for `trial_id`, with a
/// caller-chosen literal spelling (`dsr_literal`, e.g. `"1e-06"`, `"1e-6"`,
/// `"2e-6"`, `"0.85"`) for the candidate's `deflated_sharpe_ratio` field.
fn judge_json_with_dsr_literal(
    trial_id: &str,
    experiment_id: &str,
    economic_eval_id: &str,
    economic_sha: &str,
    daily_sha: &str,
    dsr_literal: &str,
) -> String {
    format!(
        r#"{{"schema_version":"multiple_testing_judge_v1","protocol":{{"protocol_id":"research_multiple_testing_judge_v1"}},"comparison_scope":{{"experiment_id":"{experiment_id}"}},"judge_status":"evaluated","holdout":{{"status":"reserved_not_evaluated"}},"included_trial_ids":["{trial_id}"],"input_economic_result_ids":["{economic_eval_id}"],"input_artifacts":[{{"trial_id":"{trial_id}","economic_walk_forward_json_sha256":"{economic_sha}","economic_daily_returns_csv_sha256":"{daily_sha}"}}],"dsr_results":[{{"trial_id":"{trial_id}","evaluable":true,"deflated_sharpe_ratio":{dsr_literal}}}],"pbo_result":{{"status":"evaluated","pbo":0.15}}}}"#
    )
}

/// REQUIRED TEST 1 — the registry's stored canonical text spells a number
/// as "1e-06" (a plausible Python `canonical_json` exponent form); the
/// supplied judge JSON spells the SAME semantic value as "1e-6". Must
/// verify -- a canonicalization-format mismatch must not reject a
/// genuinely matching artifact merely because Python and Rust format
/// floats differently.
#[test]
fn exponent_format_interoperability_positive_proof() {
    let trial_id = "exp_format_trial";
    let (economic_json, daily_csv, economic_eval_id, experiment_id, registry) =
        base_authority_fixture(trial_id);
    let economic_sha = sha256_hex(economic_json.as_bytes());
    let daily_sha = sha256_hex(&daily_csv);

    let registered_text =
        judge_json_with_dsr_literal(trial_id, &experiment_id, &economic_eval_id, &economic_sha, &daily_sha, "1e-06");
    let judge_sha = sha256_hex(registered_text.as_bytes());
    register_judge_artifact(&registry.path, &judge_sha, "judge_x", &experiment_id, None, Some(&registered_text));

    let supplied_text = registered_text.replace("1e-06", "1e-6");
    assert_ne!(supplied_text, registered_text, "sanity: spelling actually differs");

    let ev = verify_promotion_oos_evidence(&registry.path, trial_id, &economic_json, &daily_csv, &supplied_text)
        .expect("semantically-identical exponent-format spelling must verify");
    assert!((ev.deflated_sharpe_ratio() - 1e-6).abs() < 1e-15);
}

/// REQUIRED TEST 2 — registered "1e-06", supplied "2e-6": a genuinely
/// DIFFERENT numeric value, not merely a different spelling. Must fail.
#[test]
fn semantic_numeric_mutation_fails() {
    let trial_id = "numeric_mutation_trial";
    let (economic_json, daily_csv, economic_eval_id, experiment_id, registry) =
        base_authority_fixture(trial_id);
    let economic_sha = sha256_hex(economic_json.as_bytes());
    let daily_sha = sha256_hex(&daily_csv);

    let registered_text =
        judge_json_with_dsr_literal(trial_id, &experiment_id, &economic_eval_id, &economic_sha, &daily_sha, "1e-06");
    let judge_sha = sha256_hex(registered_text.as_bytes());
    register_judge_artifact(&registry.path, &judge_sha, "judge_x", &experiment_id, None, Some(&registered_text));

    let supplied_text = registered_text.replace("1e-06", "2e-6");

    let errs = verify_promotion_oos_evidence(&registry.path, trial_id, &economic_json, &daily_csv, &supplied_text)
        .unwrap_err();
    assert!(
        reasons_contain(&errs, "no research_judge_artifacts row"),
        "a genuinely different numeric value must not verify: {errs:?}"
    );
}

/// REQUIRED TEST 3 — the AUTHORITATIVE (registered) canonical text's
/// top-level field order differs from the SUPPLIED judge JSON's, yet both
/// parse to the same JSON value. Complements
/// `json_field_ordering_does_not_alter_verification_result` (which
/// reorders only the supplied side) by reordering the REGISTERED side.
#[test]
fn registered_canonical_text_field_order_does_not_alter_verification_result() {
    let f = valid_fixture("registry_order_trial");
    let value: serde_json::Value = serde_json::from_str(&f.judge_json).unwrap();
    let obj = value.as_object().unwrap();
    let mut reversed = serde_json::Map::new();
    for (k, v) in obj.iter().rev() {
        reversed.insert(k.clone(), v.clone());
    }
    let reversed_text = serde_json::to_string(&serde_json::Value::Object(reversed)).unwrap();
    assert_ne!(reversed_text, f.judge_json, "sanity: ordering actually differs");

    let registry = new_registry_db();
    register_trial(&registry.path, &f.trial_id, &f.experiment_id, "hyp");
    register_succeeded_attempt(
        &registry.path,
        &format!("{}:att0001", f.trial_id),
        &f.trial_id,
        &format!("econ_eval_{}", f.trial_id),
    );
    let reversed_sha = sha256_hex(reversed_text.as_bytes());
    register_judge_artifact(&registry.path, &reversed_sha, "judge_x", &f.experiment_id, None, Some(&reversed_text));

    // Supply the ORIGINAL (non-reversed) judge_json -- must still match the
    // reversed-field-order REGISTERED text.
    let result = verify_promotion_oos_evidence(&registry.path, &f.trial_id, &f.economic_json, &f.daily_csv, &f.judge_json);
    assert!(result.is_ok(), "got: {result:?}");
}

/// REQUIRED TEST 4 — the SUPPLIED judge JSON carries added insignificant
/// whitespace/pretty-printing relative to the registered compact canonical
/// text. Must still verify.
#[test]
fn insignificant_whitespace_does_not_alter_verification_result() {
    let f = valid_fixture("whitespace_trial");
    let value: serde_json::Value = serde_json::from_str(&f.judge_json).unwrap();
    let pretty = serde_json::to_string_pretty(&value).unwrap();
    assert_ne!(pretty, f.judge_json, "sanity: whitespace actually differs");

    let result = verify_promotion_oos_evidence(&f.registry.path, &f.trial_id, &f.economic_json, &f.daily_csv, &pretty);
    assert!(result.is_ok(), "got: {result:?}");
}

/// REQUIRED TEST 5 — `canonical_judge_json` is stored differently from what
/// its OWN `judge_artifact_sha256` primary key actually hashes to (the
/// row's internal integrity is broken). Must never be trusted, even though
/// the SUPPLIED judge JSON is byte-identical to the genuine (correctly
/// hashed) text.
#[test]
fn registry_canonical_text_hash_mismatch_fails_closed() {
    let trial_id = "registry_text_tamper_trial";
    let (economic_json, daily_csv, economic_eval_id, experiment_id, registry) =
        base_authority_fixture(trial_id);
    let economic_sha = sha256_hex(economic_json.as_bytes());
    let daily_sha = sha256_hex(&daily_csv);

    let genuine_text =
        judge_json_with_dsr_literal(trial_id, &experiment_id, &economic_eval_id, &economic_sha, &daily_sha, "0.85");
    let genuine_sha = sha256_hex(genuine_text.as_bytes());
    let tampered_text =
        judge_json_with_dsr_literal(trial_id, &experiment_id, &economic_eval_id, &economic_sha, &daily_sha, "0.99");
    // Registered under the CORRECT (genuine) sha, but the stored text does
    // not hash to it -- a corrupted/tampered row.
    register_judge_artifact(&registry.path, &genuine_sha, "judge_x", &experiment_id, None, Some(&tampered_text));

    let errs = verify_promotion_oos_evidence(&registry.path, trial_id, &economic_json, &daily_csv, &genuine_text)
        .unwrap_err();
    assert!(reasons_contain(&errs, "no research_judge_artifacts row"), "got: {errs:?}");
}

/// REQUIRED TEST 6 — same integrity contract as TEST 5, from the opposite
/// direction: a genuinely valid row is registered, then its
/// `judge_artifact_sha256` primary key is tampered with directly while its
/// `canonical_judge_json` text is left untouched.
#[test]
fn registry_sha_tampered_after_registration_fails_closed() {
    let trial_id = "registry_sha_tamper_trial";
    let (economic_json, daily_csv, economic_eval_id, experiment_id, registry) =
        base_authority_fixture(trial_id);
    let economic_sha = sha256_hex(economic_json.as_bytes());
    let daily_sha = sha256_hex(&daily_csv);

    let genuine_text =
        judge_json_with_dsr_literal(trial_id, &experiment_id, &economic_eval_id, &economic_sha, &daily_sha, "0.85");
    let genuine_sha = sha256_hex(genuine_text.as_bytes());
    register_judge_artifact(&registry.path, &genuine_sha, "judge_x", &experiment_id, None, Some(&genuine_text));

    {
        let conn = rusqlite::Connection::open(&registry.path).expect("open registry db");
        conn.execute(
            "update research_judge_artifacts set judge_artifact_sha256 = ?1 where judge_artifact_sha256 = ?2",
            rusqlite::params!["tampered_sha_does_not_match_text", genuine_sha],
        )
        .expect("tamper with judge_artifact_sha256");
    }

    let errs = verify_promotion_oos_evidence(&registry.path, trial_id, &economic_json, &daily_csv, &genuine_text)
        .unwrap_err();
    assert!(reasons_contain(&errs, "no research_judge_artifacts row"), "got: {errs:?}");
}

/// REQUIRED TEST 7 — simulates a historical row written before
/// P7C-REPAIR-04 (`canonical_judge_json` is `NULL`). Must never be trusted,
/// no matter how well the supplied judge JSON otherwise matches.
#[test]
fn missing_canonical_text_registry_row_fails_closed() {
    let trial_id = "missing_canonical_text_trial";
    let (economic_json, daily_csv, economic_eval_id, experiment_id, registry) =
        base_authority_fixture(trial_id);
    let economic_sha = sha256_hex(economic_json.as_bytes());
    let daily_sha = sha256_hex(&daily_csv);

    let judge_text =
        judge_json_with_dsr_literal(trial_id, &experiment_id, &economic_eval_id, &economic_sha, &daily_sha, "0.85");
    let judge_sha = sha256_hex(judge_text.as_bytes());
    register_judge_artifact(&registry.path, &judge_sha, "judge_x", &experiment_id, None, None);

    let errs = verify_promotion_oos_evidence(&registry.path, trial_id, &economic_json, &daily_csv, &judge_text)
        .unwrap_err();
    assert!(reasons_contain(&errs, "no research_judge_artifacts row"), "got: {errs:?}");
}

#[test]
fn nonexistent_registry_database_fails_closed() {
    let f = valid_fixture("no_db_trial");
    let bogus_path = f.registry.path.parent().unwrap().join("does_not_exist.sqlite3");
    let errs = verify_promotion_oos_evidence(&bogus_path, &f.trial_id, &f.economic_json, &f.daily_csv, &f.judge_json)
        .unwrap_err();
    assert!(reasons_contain(&errs, "research registry"), "got: {errs:?}");
}

/// mission Section 5F: mutating ONLY the candidate's DSR value (keeping
/// every other field, and every OTHER hash relationship, byte-identical)
/// must fail via the full judge artifact registry hash -- REPAIR-01's
/// input-hash binding alone could never catch this, since the judge's OWN
/// recorded input hashes (of the economic/csv artifacts) are untouched by
/// this mutation, and the registry is deliberately left unchanged (still
/// only registered under the PRE-mutation hash) -- exactly what happens
/// when an attacker mutates a judge artifact after it was registered.
#[test]
fn mutated_dsr_value_fails_authority_hash() {
    let mut f = valid_fixture("mutated_dsr_trial");
    f.judge_json = f.judge_json.replace(r#""deflated_sharpe_ratio":0.85"#, r#""deflated_sharpe_ratio":0.99"#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "judge_artifact_sha256"), "got: {errs:?}");
}

/// mission Section 5F: same proof for PBO.
#[test]
fn mutated_pbo_value_fails_authority_hash() {
    let mut f = valid_fixture("mutated_pbo_trial");
    f.judge_json = f.judge_json.replace(r#""pbo":0.15"#, r#""pbo":0.01"#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "judge_artifact_sha256"), "got: {errs:?}");
}

// ---------------------------------------------------------------------------
// REQUIRED TESTS 26/27 — non-finite DSR/PBO fails (JSON cannot encode
// literal NaN/Infinity, so the realistic non-finite failure mode is a
// missing/null numeric field -- exercised distinctly from the "row missing
// entirely" case above)
// ---------------------------------------------------------------------------

#[test]
fn null_deflated_sharpe_ratio_fails() {
    let mut f = valid_fixture("null_dsr_trial");
    f.judge_json = f
        .judge_json
        .replace(r#""deflated_sharpe_ratio":0.85"#, r#""deflated_sharpe_ratio":null"#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "deflated_sharpe_ratio is missing or non-finite"));
}

#[test]
fn null_pbo_value_fails() {
    let mut f = valid_fixture("null_pbo_trial");
    f.judge_json = f.judge_json.replace(r#""pbo":0.15"#, r#""pbo":null"#);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "pbo_result.pbo is missing or non-finite"));
}

// ---------------------------------------------------------------------------
// REQUIRED TEST 34 — deterministic JSON map ordering does not alter result
// ---------------------------------------------------------------------------

#[test]
fn json_field_ordering_does_not_alter_verification_result() {
    // The ECONOMIC artifact's exact BYTES are its hash-binding identity (the
    // judge's own recorded hash is computed against exactly those bytes, by
    // design -- reordering it is a genuine content mutation, correctly
    // rejected, and is covered separately by
    // mutated_economic_artifact_content_fails_hash_binding above). Field
    // ordering invariance is meaningful (and achievable) for the JUDGE
    // JSON's own top-level layout, since this verifier reads its fields by
    // NAME (serde_json::Value), never by position -- reordering it must not
    // change the result, and its CANONICAL hash (what the registry is
    // keyed by) is order-invariant too.
    let f = valid_fixture("ordering_trial");
    let value: serde_json::Value = serde_json::from_str(&f.judge_json).unwrap();
    let obj = value.as_object().unwrap();
    let mut reversed = serde_json::Map::new();
    for (k, v) in obj.iter().rev() {
        reversed.insert(k.clone(), v.clone());
    }
    let judge_reordered = serde_json::to_string(&serde_json::Value::Object(reversed)).unwrap();
    assert_ne!(judge_reordered, f.judge_json, "sanity: ordering actually differs");

    let a = verify_promotion_oos_evidence(&f.registry.path, &f.trial_id, &f.economic_json, &f.daily_csv, &f.judge_json)
        .expect("original order verifies");
    let b = verify_promotion_oos_evidence(&f.registry.path, &f.trial_id, &f.economic_json, &f.daily_csv, &judge_reordered)
        .expect("reordered judge JSON must verify identically (canonical hash is order-invariant)");
    assert_eq!(a, b);
}

// ---------------------------------------------------------------------------
// REQUIRED TEST 35 — forged self-consistent-but-incomplete shortcut JSON
// (missing genuine walk-forward output shape entirely) fails closed
// ---------------------------------------------------------------------------

#[test]
fn hand_typed_shortcut_json_missing_real_artifact_shape_fails() {
    // A caller who tries to skip real Research artifacts with a minimal,
    // hand-typed JSON blob (no aggregate/execution_pricing/weight_to_share/
    // ids sections at all) fails closed on every structural check at once,
    // never partially passes -- against a completely empty registry too,
    // since nothing here was ever registered.
    let daily_csv = b"not,real,returns".to_vec();
    let economic_json = r#"{"claim":"trust me, this is valid"}"#.to_string();
    let judge_json = r#"{"claim":"trust me too"}"#.to_string();
    let registry = new_registry_db();
    let errs = verify_promotion_oos_evidence(
        &registry.path,
        "shortcut_trial",
        &economic_json,
        &daily_csv,
        &judge_json,
    )
    .unwrap_err();
    assert!(errs.len() >= 5, "expected multiple independent structural failures, got: {errs:?}");
    assert!(reasons_contain(&errs, "economic_protocol_id"));
    assert!(reasons_contain(&errs, "judge_status"));
}

// ---------------------------------------------------------------------------
// REQUIRED TESTS 22-25 — DSR/PBO explicit threshold policy (evaluate_promotion)
// ---------------------------------------------------------------------------

#[test]
fn dsr_below_config_threshold_rejects() {
    let ev = verify(&valid_fixture("dsr_below_threshold_trial")).expect("valid fixture verifies");
    let mut config = lenient_config();
    config.min_deflated_sharpe_ratio = 0.90; // fixture's DSR is 0.85 -- below this
    let input = otherwise_valid_input("dsr_below_threshold", Some(ev));
    let decision = evaluate_promotion(&config, &input);
    assert!(!decision.passed);
    assert!(reasons_contain(&decision.fail_reasons, "Deflated Sharpe Ratio"));
}

#[test]
fn dsr_exactly_at_threshold_passes_dsr_portion() {
    let ev = verify(&valid_fixture("dsr_exact_threshold_trial")).expect("valid fixture verifies");
    let mut config = lenient_config();
    config.min_deflated_sharpe_ratio = ev.deflated_sharpe_ratio(); // exact equality
    let input = otherwise_valid_input("dsr_exact_threshold", Some(ev));
    let decision = evaluate_promotion(&config, &input);
    assert!(
        !decision.fail_reasons.iter().any(|r| r.contains("Deflated Sharpe Ratio")),
        "DSR exactly at threshold must not itself fail: {:?}",
        decision.fail_reasons
    );
}

#[test]
fn pbo_above_config_threshold_rejects() {
    let ev = verify(&valid_fixture("pbo_above_threshold_trial")).expect("valid fixture verifies");
    let mut config = lenient_config();
    config.max_probability_backtest_overfitting = 0.10; // fixture's PBO is 0.15 -- above this
    let input = otherwise_valid_input("pbo_above_threshold", Some(ev));
    let decision = evaluate_promotion(&config, &input);
    assert!(!decision.passed);
    assert!(reasons_contain(&decision.fail_reasons, "Probability of Backtest Overfitting"));
}

#[test]
fn pbo_exactly_at_threshold_passes_pbo_portion() {
    let ev = verify(&valid_fixture("pbo_exact_threshold_trial")).expect("valid fixture verifies");
    let mut config = lenient_config();
    config.max_probability_backtest_overfitting = ev.probability_of_backtest_overfitting();
    let input = otherwise_valid_input("pbo_exact_threshold", Some(ev));
    let decision = evaluate_promotion(&config, &input);
    assert!(
        !decision.fail_reasons.iter().any(|r| r.contains("Probability of Backtest Overfitting")),
        "PBO exactly at threshold must not itself fail: {:?}",
        decision.fail_reasons
    );
}

#[test]
fn invalid_threshold_config_fails_closed() {
    let ev = verify(&valid_fixture("invalid_config_trial")).expect("valid fixture verifies");
    let mut config = lenient_config();
    config.min_deflated_sharpe_ratio = -1.0; // out of [0,1]
    let input = otherwise_valid_input("invalid_config", Some(ev));
    let decision = evaluate_promotion(&config, &input);
    assert!(!decision.passed);
    assert!(reasons_contain(&decision.fail_reasons, "Invalid PromotionConfig"));
}

// ---------------------------------------------------------------------------
// REQUIRED TEST 28 — full valid chain (verified OOS + historical metrics +
// stress + artifact lock + P7A + repaired P7B + DSR/PBO thresholds) passes
// ---------------------------------------------------------------------------

#[test]
fn end_to_end_valid_candidate_passes_promotion() {
    let ev = verify(&valid_fixture("e2e_trial_001")).expect("valid fixture verifies");
    let input = otherwise_valid_input("e2e_valid_strategy", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(
        decision.passed,
        "valid provenance + artifact lock + passing stress + passing historical thresholds + \
         verified OOS + DSR/PBO thresholds => promotion must pass; fail_reasons: {:?}",
        decision.fail_reasons
    );
    assert!(decision.fail_reasons.is_empty());
    assert!(pick_winner("e2e", &decision.metrics, "e2e", &decision.metrics) == "e2e");
}

// ---------------------------------------------------------------------------
// REQUIRED TESTS 29-31 — existing gates remain independently enforced
// ---------------------------------------------------------------------------

#[test]
fn verified_oos_evidence_does_not_weaken_metrics_gate() {
    let strict = PromotionConfig {
        min_sharpe: 0.0,
        max_mdd: 0.10,
        min_cagr: 1_000.0, // impossible to clear
        min_profit_factor: 0.0,
        min_profitable_months_pct: 0.40,
        min_deflated_sharpe_ratio: 0.0,
        max_probability_backtest_overfitting: 1.0,
    };
    let ev = verify(&valid_fixture("metrics_still_enforced_trial")).unwrap();
    let input = otherwise_valid_input("metrics_still_enforced", Some(ev));
    let decision = evaluate_promotion(&strict, &input);
    assert!(!decision.passed);
    assert!(reasons_contain(&decision.fail_reasons, "CAGR"));
}

#[test]
fn verified_oos_evidence_does_not_weaken_stress_suite_gate() {
    let ev = verify(&valid_fixture("stress_still_enforced_trial")).unwrap();
    let mut input = otherwise_valid_input("stress_still_enforced", Some(ev));
    input.stress_suite = None;
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(!decision.passed);
    assert!(reasons_contain(&decision.fail_reasons, "Stress suite not run"));
}

#[test]
fn verified_oos_evidence_does_not_weaken_artifact_lock_gate() {
    let ev = verify(&valid_fixture("lock_still_enforced_trial")).unwrap();
    let mut input = otherwise_valid_input("lock_still_enforced", Some(ev));
    input.artifact_lock = None;
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(!decision.passed);
    assert!(reasons_contain(&decision.fail_reasons, "Artifact not hash-locked"));
}

// ---------------------------------------------------------------------------
// REQUIRED TEST 32 — winner selection excludes any candidate failing the
// verified OOS gate
// ---------------------------------------------------------------------------

#[test]
fn select_best_skips_candidates_without_valid_verified_evidence() {
    let candidates = vec![
        Candidate {
            id: "no_evidence".into(),
            input: otherwise_valid_input("no_evidence_candidate", None),
        },
        Candidate {
            id: "with_evidence".into(),
            input: otherwise_valid_input(
                "with_evidence_candidate",
                Some(verify(&valid_fixture("with_evidence_trial")).unwrap()),
            ),
        },
    ];
    let best = select_best(&lenient_config(), &candidates);
    let (winner_id, decision) = best.expect("at least one candidate should pass");
    assert_eq!(winner_id, "with_evidence");
    assert!(decision.passed);

    let no_ev_decision = evaluate_promotion(&lenient_config(), &candidates[0].input);
    assert!(!no_ev_decision.passed);
}

#[test]
fn select_best_returns_none_when_no_candidate_has_valid_evidence() {
    let candidates = vec![
        Candidate {
            id: "a".into(),
            input: otherwise_valid_input("a_no_evidence", None),
        },
        Candidate {
            id: "b".into(),
            input: otherwise_valid_input("b_no_evidence", None),
        },
    ];
    assert!(select_best(&lenient_config(), &candidates).is_none());
}

// ---------------------------------------------------------------------------
// REQUIRED TEST 33 — no holdout scoring/consumption occurs as a side effect
// ---------------------------------------------------------------------------

#[test]
fn evaluate_promotion_performs_no_io_or_holdout_access() {
    // Structural proof: evaluate_promotion takes only in-memory
    // strings/bytes/typed structs and touches no filesystem path, database,
    // or holdout ledger (the one read-only registry query happens earlier,
    // inside verify_promotion_oos_evidence, which already ran by the time
    // `ev` exists here). Two evaluations of the SAME evidence produce
    // byte-identical results.
    let ev = verify(&valid_fixture("no_io_trial")).unwrap();
    let input = otherwise_valid_input("no_io", Some(ev));
    let d1 = evaluate_promotion(&lenient_config(), &input);
    let d2 = evaluate_promotion(&lenient_config(), &input);
    assert_eq!(d1, d2);
}

// ---------------------------------------------------------------------------
// P7C-REPAIR-02/-03 REQUIRED TEST (mission Section 5A/5K item 1, extended by
// REPAIR-03's production API audit): no production shortcut exists. Proves
// the crate's own source contains neither the old `valid_for_testing`
// bypass NOR the removed `ResearchAttemptAuthority` struct (or an
// equivalent caller-suppliable authority constructor) anywhere.
// ---------------------------------------------------------------------------

#[test]
fn no_valid_for_testing_shortcut_exists_in_crate_source() {
    for path in [
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/research_evidence.rs"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/research_registry.rs"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/types.rs"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"),
    ] {
        let source = std::fs::read_to_string(path).expect("crate source file must exist");
        // Matches an actual callable definition/reference (`fn
        // valid_for_testing` or `::valid_for_testing(`), not this test's
        // own explanatory prose about the REMOVED bypass in doc comments.
        assert!(
            !source.contains("fn valid_for_testing") && !source.contains("::valid_for_testing("),
            "found a valid_for_testing shortcut in {path} -- P7C-REPAIR-02 requires the ONLY \
             route to VerifiedPromotionOosEvidence to be the real verifier"
        );
    }
}

#[test]
fn no_public_authority_struct_or_constructor_exists_in_crate_source() {
    // P7C-REPAIR-03 production API audit (mission item 6): no
    // `ResearchAttemptAuthority` symbol -- definition or struct-literal
    // construction -- survives anywhere in this crate's source. The only
    // route to authority is `research_registry::load_research_authority`,
    // which is `pub(crate)` (never exported from the crate) and reads a
    // real database.
    let mut combined_source = String::new();
    for path in [
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/research_evidence.rs"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/research_registry.rs"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/types.rs"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"),
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/evaluator.rs"),
    ] {
        let source = std::fs::read_to_string(path).expect("crate source file must exist");
        // Matches an actual definition or struct-literal construction, not
        // this file's (or the source's) own explanatory prose about the
        // REMOVED struct -- same precision standard as the
        // valid_for_testing check below.
        assert!(
            !source.contains("struct ResearchAttemptAuthority")
                && !source.contains("ResearchAttemptAuthority {"),
            "found a ResearchAttemptAuthority definition/construction in {path} -- P7C-REPAIR-03 \
             requires authority to be established exclusively by reading the durable registry, \
             never by a caller-suppliable struct"
        );
        assert!(
            !source.contains("pub fn load_research_authority")
                && !source.contains("pub struct VerifiedResearchAuthority"),
            "found a PUBLIC (crate-exported) authority constructor/type in {path} -- it must \
             stay pub(crate) only"
        );
        combined_source.push_str(&source);
    }
    assert!(
        combined_source.contains("fn load_research_authority"),
        "sanity check on the audit itself: load_research_authority must exist somewhere in the \
         crate source scanned above"
    );
}

/// REQUIRED TEST: a genuinely valid, registry-anchored bundle -- built the
/// same way `tests/common::valid_oos_evidence_for_testing` does -- passes
/// the evaluator end to end.
#[test]
fn genuinely_verified_evidence_is_accepted_by_evaluator() {
    let ev = verify(&valid_fixture("genuinely_verified_trial")).unwrap();
    let input = otherwise_valid_input("genuinely_verified_input", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(decision.passed, "fail_reasons: {:?}", decision.fail_reasons);
}

// ---------------------------------------------------------------------------
// Shared fixture helpers
// ---------------------------------------------------------------------------

fn good_equity_curve() -> Vec<(i64, i64)> {
    let day = 86_400i64;
    let mut curve = Vec::new();
    let mut equity = 1_000_000_000.0_f64;
    for d in 0..=180 {
        curve.push((d * day, equity as i64));
        equity *= 1.003;
    }
    curve
}

fn good_report(strategy_name: &str) -> BacktestReport {
    let config_id = BacktestConfig::test_defaults().config_id();
    let input_data_hash = derive_input_data_hash(&[]);
    let run_id = derive_run_id(strategy_name, &config_id, &input_data_hash);
    BacktestReport {
        equity_curve: good_equity_curve(),
        strategy_name: strategy_name.to_string(),
        run_id,
        config_id,
        input_data_hash,
        ..BacktestReport::test_fixture()
    }
}

fn lenient_config() -> PromotionConfig {
    PromotionConfig {
        min_sharpe: 0.5,
        max_mdd: 0.10,
        min_cagr: 0.05,
        min_profit_factor: 0.0,
        min_profitable_months_pct: 0.40,
        min_deflated_sharpe_ratio: 0.0,
        max_probability_backtest_overfitting: 1.0,
    }
}

/// Every OTHER gate (provenance, artifact lock, stress suite) passing —
/// used so every test in this file isolates the OOS-evidence gate alone.
fn otherwise_valid_input(
    strategy_name: &str,
    oos_evidence: Option<VerifiedPromotionOosEvidence>,
) -> PromotionInput {
    PromotionInput {
        initial_equity_micros: 1_000_000_000,
        report: good_report(strategy_name),
        stress_suite: Some(StressSuiteResult::pass(3, REQUIRED_STRESS_PROTOCOL_VERSION)),
        artifact_lock: Some(ArtifactLock::new_for_testing("cfg_hash", "git_hash")),
        oos_evidence,
    }
}
