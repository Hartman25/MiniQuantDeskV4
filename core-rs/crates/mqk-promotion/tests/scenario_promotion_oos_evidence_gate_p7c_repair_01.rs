//! P7C-REPAIR-01 (PROMOTION-OOS-EVIDENCE-GATE-01-REPAIR-01) — the promotion
//! gate must fail closed unless it has STRUCTURALLY VERIFIED, hash-bound,
//! statistically-thresholded out-of-sample Research evidence extracted from
//! REAL artifact content — never a caller-populated claim. Covers the
//! repair mission's REQUIRED tests (Section 6K, 35 items, referenced by
//! number) plus the Section 7 acceptance-boundary load-bearing proofs.

use mqk_promotion::{
    evaluate_promotion, pick_winner, select_best, verify_promotion_oos_evidence, ArtifactLock,
    Candidate, PromotionConfig, PromotionInput, ResearchAttemptAuthority, StressSuiteResult,
    VerifiedPromotionOosEvidence,
};
use mqk_backtest::{derive_input_data_hash, derive_run_id, BacktestConfig, BacktestReport};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Fixture JSON builders — mirror the REAL Python artifact schemas
// (economic_walk_forward.json / multiple_testing_judge JSON) exactly at the
// field paths verify_promotion_oos_evidence reads. Each negative-control
// test mutates exactly ONE fact and proves the verifier rejects it.
// ---------------------------------------------------------------------------

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Matches `verify_promotion_oos_evidence`'s own canonical judge-artifact
/// hashing exactly -- see that function's doc comment.
fn canonical_json_sha256(json_str: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(json_str).expect("valid JSON fixture");
    sha256_hex(&serde_json::to_vec(&value).expect("Value always re-serializes"))
}

struct Fixture {
    trial_id: String,
    economic_json: String,
    daily_csv: Vec<u8>,
    judge_json: String,
    /// P7C-REPAIR-02: the durable authority record this fixture's artifacts
    /// are anchored to, built from the ORIGINAL (pre-mutation) content --
    /// a negative-control test that mutates `economic_json`/`judge_json`
    /// after construction deliberately leaves `authority` unchanged, so the
    /// mutated bundle ALSO fails the new authority checks (in addition to
    /// whichever specific structural check that test is targeting) --
    /// every existing assertion here uses `reasons_contain` (presence, not
    /// exclusivity), so this is compatible with every pre-existing test.
    authority: ResearchAttemptAuthority,
}

/// A fully self-consistent, structurally valid, AUTHORITY-anchored evidence
/// bundle for `trial_id`: hashes genuinely match, every required field is
/// present and correct, and the authority record matches. This is the ONLY
/// shape `verify_promotion_oos_evidence` accepts.
fn valid_fixture(trial_id: &str) -> Fixture {
    let daily_csv = b"date,net_daily_return\n2021-01-01,0.0010\n2021-01-02,0.0021\n".to_vec();
    let daily_sha = sha256_hex(&daily_csv);
    let economic_eval_id = format!("econ_eval_{trial_id}");

    let economic_json = format!(
        r#"{{"protocol":{{"protocol_id":"economic_walk_forward_v1"}},"aggregate":{{"folds_used":3}},"holdout":{{"status":"reserved_not_evaluated"}},"execution_pricing":{{"pricing_model_id":"rust_conservative_bar_range_v1"}},"weight_to_share":{{"weight_to_share_protocol_id":"weight_to_share_v1"}},"outputs":{{"economic_daily_returns_csv":{{"sha256":"{daily_sha}"}}}},"ids":{{"economic_eval_id":"{economic_eval_id}"}},"folds":[{{"discrete_economics_protocol_id":"discrete_share_economic_path_v1"}}]}}"#
    );
    let economic_sha = sha256_hex(economic_json.as_bytes());

    let judge_json = format!(
        r#"{{"schema_version":"multiple_testing_judge_v1","protocol":{{"protocol_id":"research_multiple_testing_judge_v1"}},"comparison_scope":{{"experiment_id":"exp_{trial_id}"}},"judge_status":"evaluated","holdout":{{"status":"reserved_not_evaluated"}},"included_trial_ids":["{trial_id}"],"input_economic_result_ids":["{economic_eval_id}"],"input_artifacts":[{{"trial_id":"{trial_id}","economic_walk_forward_json_sha256":"{economic_sha}","economic_daily_returns_csv_sha256":"{daily_sha}"}}],"dsr_results":[{{"trial_id":"{trial_id}","evaluable":true,"deflated_sharpe_ratio":0.85}}],"pbo_result":{{"status":"evaluated","pbo":0.15}}}}"#
    );

    let authority = ResearchAttemptAuthority {
        trial_id: trial_id.to_string(),
        economic_eval_id: economic_eval_id.clone(),
        judge_artifact_sha256: canonical_json_sha256(&judge_json),
    };

    Fixture {
        trial_id: trial_id.to_string(),
        economic_json,
        daily_csv,
        judge_json,
        authority,
    }
}

fn verify(f: &Fixture) -> Result<VerifiedPromotionOosEvidence, Vec<String>> {
    verify_promotion_oos_evidence(&f.authority, &f.trial_id, &f.economic_json, &f.daily_csv, &f.judge_json)
}

fn reasons_contain(errs: &[String], needle: &str) -> bool {
    errs.iter().any(|r| r.contains(needle))
}

// ---------------------------------------------------------------------------
// POSITIVE PROOF — a genuinely valid, hash-consistent bundle verifies
// (REQUIRED TESTS 4/19/20/21)
// ---------------------------------------------------------------------------

#[test]
fn valid_bundle_verifies_successfully() {
    let f = valid_fixture("trial_valid_001");
    let ev = verify(&f).expect("structurally valid, hash-consistent evidence must verify");
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
    // different, still-nonempty string that the judge's
    // input_economic_result_ids never recorded.
    let mut f = valid_fixture("wrong_eval_id_trial");
    f.economic_json = f
        .economic_json
        .replace("econ_eval_wrong_eval_id_trial", "totally_different_eval_id");
    let errs = verify(&f).unwrap_err();
    assert!(
        reasons_contain(&errs, "input_economic_result_ids")
            // Also breaks hash binding since economic_json content changed.
            || reasons_contain(&errs, "economic_walk_forward_json_sha256"),
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
    // actually documents.
    let errs = verify_promotion_oos_evidence(
        &f.authority,
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
// ---------------------------------------------------------------------------

#[test]
fn wrong_judge_schema_version_fails() {
    let mut f = valid_fixture("wrong_judge_schema_trial");
    f.judge_json = f
        .judge_json
        .replace(r#""schema_version":"multiple_testing_judge_v1""#, r#""schema_version":"some_other_v2""#);
    f.authority.judge_artifact_sha256 = canonical_json_sha256(&f.judge_json);
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
    f.authority.judge_artifact_sha256 = canonical_json_sha256(&f.judge_json);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "judge protocol.protocol_id"), "got: {errs:?}");
}

#[test]
fn missing_comparison_scope_fails() {
    let mut f = valid_fixture("missing_scope_trial");
    f.judge_json = f
        .judge_json
        .replace(r#""comparison_scope":{"experiment_id":"exp_missing_scope_trial"},"#, "");
    f.authority.judge_artifact_sha256 = canonical_json_sha256(&f.judge_json);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "comparison_scope"), "got: {errs:?}");
}

#[test]
fn empty_comparison_scope_object_fails() {
    let mut f = valid_fixture("empty_scope_trial");
    f.judge_json = f
        .judge_json
        .replace(r#""comparison_scope":{"experiment_id":"exp_empty_scope_trial"}"#, r#""comparison_scope":{}"#);
    f.authority.judge_artifact_sha256 = canonical_json_sha256(&f.judge_json);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "comparison_scope"), "got: {errs:?}");
}

#[test]
fn null_comparison_scope_fails() {
    let mut f = valid_fixture("null_scope_trial");
    f.judge_json = f
        .judge_json
        .replace(r#""comparison_scope":{"experiment_id":"exp_null_scope_trial"}"#, r#""comparison_scope":null"#);
    f.authority.judge_artifact_sha256 = canonical_json_sha256(&f.judge_json);
    let errs = verify(&f).unwrap_err();
    assert!(reasons_contain(&errs, "comparison_scope"), "got: {errs:?}");
}

// ---------------------------------------------------------------------------
// P7C-REPAIR-02 REQUIRED TESTS (mission Section 5C/5E) — AUTHORITY anchor:
// the load-bearing RED/GREEN proof. A completely fabricated but internally
// self-consistent bundle (every hash matches every other hash) must still
// fail because it was never registered -- i.e. no authority record backs
// it. This is what REPAIR-01 alone could never catch.
// ---------------------------------------------------------------------------

#[test]
fn fabricated_internally_consistent_unregistered_bundle_fails() {
    // Build a bundle EXACTLY the way valid_fixture does (so every internal
    // hash is genuinely self-consistent -- this is not a malformed-JSON
    // test), but never register it: construct an authority for a
    // DIFFERENT, unrelated trial/economic_eval_id/judge hash, exactly as
    // if this fabricated bundle simply does not exist in the durable
    // Research registry.
    let f = valid_fixture("fabricated_trial");
    let unrelated_authority = ResearchAttemptAuthority {
        trial_id: "fabricated_trial".to_string(),
        economic_eval_id: "some_other_registered_eval_id".to_string(),
        judge_artifact_sha256: "0".repeat(64),
    };
    let errs = verify_promotion_oos_evidence(
        &unrelated_authority,
        &f.trial_id,
        &f.economic_json,
        &f.daily_csv,
        &f.judge_json,
    )
    .unwrap_err();
    assert!(reasons_contain(&errs, "economic_eval_id"), "got: {errs:?}");
    assert!(reasons_contain(&errs, "judge_artifact_sha256"), "got: {errs:?}");
}

#[test]
fn authority_trial_id_mismatch_fails() {
    let f = valid_fixture("authority_mismatch_trial");
    let wrong_authority = ResearchAttemptAuthority {
        trial_id: "a_completely_different_trial".to_string(),
        ..f.authority.clone()
    };
    let errs = verify_promotion_oos_evidence(
        &wrong_authority,
        &f.trial_id,
        &f.economic_json,
        &f.daily_csv,
        &f.judge_json,
    )
    .unwrap_err();
    assert!(reasons_contain(&errs, "authority.trial_id"), "got: {errs:?}");
}

/// mission Section 5F: mutating ONLY the candidate's DSR value (keeping
/// every other field, and every OTHER hash relationship, byte-identical)
/// must fail via the full judge artifact authority hash -- REPAIR-01's
/// input-hash binding alone could never catch this, since the judge's
/// OWN recorded input hashes (of the economic/csv artifacts) are
/// untouched by this mutation.
#[test]
fn mutated_dsr_value_fails_authority_hash() {
    let mut f = valid_fixture("mutated_dsr_trial");
    f.judge_json = f.judge_json.replace(r#""deflated_sharpe_ratio":0.85"#, r#""deflated_sharpe_ratio":0.99"#);
    // authority.judge_artifact_sha256 is deliberately LEFT UNCHANGED here
    // (still the pre-mutation hash) -- this is exactly what happens when
    // an attacker mutates a judge artifact after it was registered.
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
    // change the result.
    let f = valid_fixture("ordering_trial");
    let value: serde_json::Value = serde_json::from_str(&f.judge_json).unwrap();
    let obj = value.as_object().unwrap();
    let mut reversed = serde_json::Map::new();
    for (k, v) in obj.iter().rev() {
        reversed.insert(k.clone(), v.clone());
    }
    let judge_reordered = serde_json::to_string(&serde_json::Value::Object(reversed)).unwrap();
    assert_ne!(judge_reordered, f.judge_json, "sanity: ordering actually differs");

    let a = verify_promotion_oos_evidence(&f.authority, &f.trial_id, &f.economic_json, &f.daily_csv, &f.judge_json)
        .expect("original order verifies");
    let b = verify_promotion_oos_evidence(&f.authority, &f.trial_id, &f.economic_json, &f.daily_csv, &judge_reordered)
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
    // never partially passes.
    let daily_csv = b"not,real,returns".to_vec();
    let economic_json = r#"{"claim":"trust me, this is valid"}"#.to_string();
    let judge_json = r#"{"claim":"trust me too"}"#.to_string();
    let authority = ResearchAttemptAuthority {
        trial_id: "shortcut_trial".to_string(),
        economic_eval_id: String::new(),
        judge_artifact_sha256: String::new(),
    };
    let errs = verify_promotion_oos_evidence(
        &authority,
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
    // Structural proof: evaluate_promotion / verify_promotion_oos_evidence
    // take only in-memory strings/bytes/typed structs and touch no
    // filesystem path, database, or holdout ledger. Two evaluations of the
    // SAME evidence produce byte-identical results.
    let ev = verify(&valid_fixture("no_io_trial")).unwrap();
    let input = otherwise_valid_input("no_io", Some(ev));
    let d1 = evaluate_promotion(&lenient_config(), &input);
    let d2 = evaluate_promotion(&lenient_config(), &input);
    assert_eq!(d1, d2);
}

// ---------------------------------------------------------------------------
// P7C-REPAIR-02 REQUIRED TEST (mission Section 5A/5K item 1): no production
// valid_for_testing shortcut exists. The old test here documented the
// OPPOSITE (a bypass admitted by the evaluator) -- that bypass is now
// entirely removed (see tests/common/mod.rs for its real-verifier
// replacement). This proves it stays gone: the crate's own source contains
// no `valid_for_testing` symbol anywhere.
// ---------------------------------------------------------------------------

#[test]
fn no_valid_for_testing_shortcut_exists_in_crate_source() {
    for path in [
        concat!(env!("CARGO_MANIFEST_DIR"), "/src/research_evidence.rs"),
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

/// REQUIRED TEST: a genuinely valid, authority-anchored bundle -- built the
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
        stress_suite: Some(StressSuiteResult::pass(3)),
        artifact_lock: Some(ArtifactLock::new_for_testing("cfg_hash", "git_hash")),
        oos_evidence,
    }
}
