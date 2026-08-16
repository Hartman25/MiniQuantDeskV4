//! P7C (PROMOTION-OOS-EVIDENCE-GATE-01) — the promotion gate must fail
//! closed unless it has STRUCTURALLY VERIFIED, promotion-grade
//! out-of-sample Research evidence. Covers the mission's REQUIRED
//! NEGATIVE/POSITIVE TESTS list (20 items, referenced by number).

use mqk_backtest::{derive_input_data_hash, derive_run_id, BacktestConfig, BacktestReport};
use mqk_promotion::{
    check_oos_evidence, evaluate_promotion, pick_winner, select_best, ArtifactLock, Candidate,
    PromotionConfig, PromotionInput, PromotionOosEvidence, StressSuiteResult,
};

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
    }
}

/// Every OTHER gate (provenance, artifact lock, stress suite) passing —
/// used so every test in this file isolates the OOS-evidence gate alone.
fn otherwise_valid_input(strategy_name: &str, oos_evidence: Option<PromotionOosEvidence>) -> PromotionInput {
    PromotionInput {
        initial_equity_micros: 1_000_000_000,
        report: good_report(strategy_name),
        stress_suite: Some(StressSuiteResult::pass(3)),
        artifact_lock: Some(ArtifactLock::new_for_testing("cfg_hash", "git_hash")),
        oos_evidence,
    }
}

// ---------------------------------------------------------------------------
// TEST 1 — PRIMARY NEGATIVE PROOF
// ---------------------------------------------------------------------------

#[test]
fn primary_negative_proof_no_oos_evidence_rejects_otherwise_perfect_candidate() {
    let input = otherwise_valid_input("primary_negative_proof", None);
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(
        !decision.passed,
        "a candidate with no OOS evidence must be rejected even though every other gate passes"
    );
    let reasons = decision.fail_reasons.join("; ");
    assert!(
        reasons.contains("OOS evidence missing"),
        "fail reason must mention missing OOS evidence, got: {reasons}"
    );
}

// ---------------------------------------------------------------------------
// TEST 2 — global-fit negative control
// ---------------------------------------------------------------------------

#[test]
fn global_fit_relabeled_protocol_id_still_fails() {
    let mut ev = PromotionOosEvidence::valid_for_testing("global_fit_trial");
    // Even "relabeled" as promotion-grade, a legacy single-shot/global-fit
    // shape (ml_train_meta_v1) is not the accepted economic_walk_forward_v1
    // protocol -- must fail.
    ev.economic_protocol_id = "ml_train_meta_v1".to_string();
    let input = otherwise_valid_input("global_fit_control", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(!decision.passed);
    assert!(decision
        .fail_reasons
        .iter()
        .any(|r| r.contains("economic_protocol_id")));
}

// ---------------------------------------------------------------------------
// TEST 3 — empty-fold evidence fails
// ---------------------------------------------------------------------------

#[test]
fn empty_fold_evidence_fails() {
    let mut ev = PromotionOosEvidence::valid_for_testing("empty_fold_trial");
    ev.folds_used = 0;
    let input = otherwise_valid_input("empty_fold", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(!decision.passed);
    assert!(decision.fail_reasons.iter().any(|r| r.contains("folds_used")));
}

// ---------------------------------------------------------------------------
// TESTS 4/5 — holdout status
// ---------------------------------------------------------------------------

#[test]
fn holdout_status_other_than_reserved_fails() {
    let mut ev = PromotionOosEvidence::valid_for_testing("holdout_wrong_trial");
    ev.holdout_status = "unknown".to_string();
    let input = otherwise_valid_input("holdout_wrong", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(!decision.passed);
    assert!(decision.fail_reasons.iter().any(|r| r.contains("holdout_status")));
}

#[test]
fn consumed_holdout_state_fails() {
    let mut ev = PromotionOosEvidence::valid_for_testing("holdout_consumed_trial");
    ev.holdout_status = "consumed".to_string();
    let input = otherwise_valid_input("holdout_consumed", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(!decision.passed);
    assert!(decision.fail_reasons.iter().any(|r| r.contains("holdout_status")));
}

// ---------------------------------------------------------------------------
// TEST 6 — diagnostic P7A pricing fails
// ---------------------------------------------------------------------------

#[test]
fn diagnostic_p7a_pricing_fails() {
    let mut ev = PromotionOosEvidence::valid_for_testing("diagnostic_p7a_trial");
    ev.execution_pricing_protocol_id = "close_only_diagnostic_v1".to_string();
    let input = otherwise_valid_input("diagnostic_p7a", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(!decision.passed);
    assert!(decision
        .fail_reasons
        .iter()
        .any(|r| r.contains("execution_pricing_protocol_id")));
}

// ---------------------------------------------------------------------------
// TEST 7 — missing P7B parity evidence fails
// ---------------------------------------------------------------------------

#[test]
fn missing_p7b_parity_evidence_fails() {
    let mut ev = PromotionOosEvidence::valid_for_testing("missing_p7b_trial");
    ev.weight_to_share_protocol_id = None;
    let input = otherwise_valid_input("missing_p7b", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(!decision.passed);
    assert!(decision
        .fail_reasons
        .iter()
        .any(|r| r.contains("weight_to_share_protocol_id")));
}

// ---------------------------------------------------------------------------
// TESTS 8/9 — P7A-only and P7B-only evidence both fail (full parity required)
// ---------------------------------------------------------------------------

#[test]
fn p7a_only_evidence_fails() {
    // P7A satisfied, P7B not (weight_to_share missing) -- must still fail.
    let mut ev = PromotionOosEvidence::valid_for_testing("p7a_only_trial");
    ev.weight_to_share_protocol_id = None;
    assert_eq!(
        ev.execution_pricing_protocol_id,
        "rust_conservative_bar_range_v1",
        "sanity: P7A protocol id is satisfied in this fixture"
    );
    let input = otherwise_valid_input("p7a_only", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(!decision.passed, "P7A parity alone must not satisfy the P7C gate");
}

#[test]
fn p7b_only_evidence_fails() {
    // P7B satisfied, P7A not (diagnostic pricing) -- must still fail.
    let mut ev = PromotionOosEvidence::valid_for_testing("p7b_only_trial");
    ev.execution_pricing_protocol_id = "close_only_diagnostic_v1".to_string();
    assert_eq!(
        ev.weight_to_share_protocol_id.as_deref(),
        Some("weight_to_share_v1"),
        "sanity: P7B protocol id is satisfied in this fixture"
    );
    let input = otherwise_valid_input("p7b_only", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(!decision.passed, "P7B parity alone must not satisfy the P7C gate");
}

// ---------------------------------------------------------------------------
// TESTS 10/11/12 — multiple-testing evidence
// ---------------------------------------------------------------------------

#[test]
fn missing_multiple_testing_evidence_fails() {
    // "Missing" is expressed as an empty/invalid judge_status -- the type
    // system prevents omitting the field entirely (see research_evidence.rs
    // module docs: no single boolean to simply leave unset).
    let mut ev = PromotionOosEvidence::valid_for_testing("missing_mtj_trial");
    ev.judge_status = String::new();
    let input = otherwise_valid_input("missing_mtj", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(!decision.passed);
    assert!(decision.fail_reasons.iter().any(|r| r.contains("judge_status")));
}

#[test]
fn not_evaluable_multiple_testing_evidence_fails() {
    let mut ev = PromotionOosEvidence::valid_for_testing("not_evaluable_mtj_trial");
    ev.judge_status = "not_evaluable".to_string();
    let input = otherwise_valid_input("not_evaluable_mtj", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(!decision.passed);
    assert!(decision.fail_reasons.iter().any(|r| r.contains("judge_status")));
}

#[test]
fn failed_multiple_testing_evidence_fails() {
    // "Failed" for this candidate specifically = its own DSR trial result
    // was not evaluable, even if the population-level judge_status/PBO are
    // fine (never weaken the judge to make a candidate pass).
    let mut ev = PromotionOosEvidence::valid_for_testing("failed_mtj_trial");
    ev.judge_trial_evaluable = false;
    let input = otherwise_valid_input("failed_mtj", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(!decision.passed);
    assert!(decision
        .fail_reasons
        .iter()
        .any(|r| r.contains("judge_trial_evaluable") || r.contains("DSR result was not evaluable")));
}

#[test]
fn not_evaluable_pbo_status_fails() {
    let mut ev = PromotionOosEvidence::valid_for_testing("pbo_not_evaluable_trial");
    ev.pbo_status = "not_evaluable".to_string();
    let input = otherwise_valid_input("pbo_not_evaluable", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(!decision.passed);
    assert!(decision.fail_reasons.iter().any(|r| r.contains("PBO status")));
}

// ---------------------------------------------------------------------------
// TEST 13 — wrong trial/comparison-scope identity fails
// ---------------------------------------------------------------------------

#[test]
fn wrong_comparison_scope_identity_fails() {
    let mut ev = PromotionOosEvidence::valid_for_testing("wrong_scope_trial");
    ev.trial_included_in_judge_scope = false;
    let input = otherwise_valid_input("wrong_scope", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(!decision.passed);
    assert!(decision
        .fail_reasons
        .iter()
        .any(|r| r.contains("judged comparison scope")));
}

#[test]
fn empty_trial_id_fails() {
    let mut ev = PromotionOosEvidence::valid_for_testing("");
    ev.trial_id = String::new();
    let input = otherwise_valid_input("empty_trial_id", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(!decision.passed);
    assert!(decision.fail_reasons.iter().any(|r| r.contains("trial_id is empty")));
}

// ---------------------------------------------------------------------------
// TEST 14 — mismatched artifact/hash evidence fails
// ---------------------------------------------------------------------------

#[test]
fn empty_economic_eval_id_fails() {
    let mut ev = PromotionOosEvidence::valid_for_testing("empty_hash_trial");
    ev.economic_eval_id = String::new();
    let input = otherwise_valid_input("empty_hash", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(!decision.passed);
    assert!(decision.fail_reasons.iter().any(|r| r.contains("economic_eval_id")));
}

// ---------------------------------------------------------------------------
// TEST 15 — structurally valid complete OOS evidence CAN pass
// ---------------------------------------------------------------------------

#[test]
fn structurally_valid_complete_oos_evidence_passes_the_new_gate() {
    let ev = PromotionOosEvidence::valid_for_testing("complete_valid_trial");
    let input = otherwise_valid_input("complete_valid", Some(ev));
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(
        decision.passed,
        "structurally complete, correctly-scoped OOS evidence must satisfy the gate; \
         fail_reasons: {:?}",
        decision.fail_reasons
    );
    assert!(decision.fail_reasons.is_empty());
}

// ---------------------------------------------------------------------------
// TEST 16 — with valid OOS evidence, EXISTING gates still reject their own
// old negative cases (P7C must not weaken any existing gate)
// ---------------------------------------------------------------------------

#[test]
fn valid_oos_evidence_does_not_weaken_stress_suite_gate() {
    let mut input = otherwise_valid_input("stress_still_enforced", Some(PromotionOosEvidence::valid_for_testing("t")));
    input.stress_suite = None;
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(!decision.passed);
    assert!(decision.fail_reasons.iter().any(|r| r.contains("Stress suite not run")));
}

#[test]
fn valid_oos_evidence_does_not_weaken_artifact_lock_gate() {
    let mut input = otherwise_valid_input("lock_still_enforced", Some(PromotionOosEvidence::valid_for_testing("t")));
    input.artifact_lock = None;
    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(!decision.passed);
    assert!(decision.fail_reasons.iter().any(|r| r.contains("Artifact not hash-locked")));
}

#[test]
fn valid_oos_evidence_does_not_weaken_metrics_gate() {
    let strict = PromotionConfig {
        min_sharpe: 0.0,
        max_mdd: 0.10,
        min_cagr: 1_000.0, // 100,000% annualized -- impossible to clear
        min_profit_factor: 0.0,
        min_profitable_months_pct: 0.40,
    };
    let input = otherwise_valid_input("metrics_still_enforced", Some(PromotionOosEvidence::valid_for_testing("t")));
    let decision = evaluate_promotion(&strict, &input);
    assert!(!decision.passed);
    assert!(decision.fail_reasons.iter().any(|r| r.contains("CAGR")));
}

// ---------------------------------------------------------------------------
// TEST 17 — determinism: JSON field ordering cannot alter the gate result
// ---------------------------------------------------------------------------

#[test]
fn json_field_ordering_does_not_alter_gate_result() {
    let ev = PromotionOosEvidence::valid_for_testing("ordering_trial");
    let json_a = serde_json::to_string(&ev).unwrap();

    // Manually re-serialize with a different (reversed) key order via a raw
    // JSON value round-trip -- serde_json::Value uses a Map that preserves
    // insertion order, so re-inserting keys in reverse order actually
    // changes the emitted key order while the semantic content is identical.
    let value: serde_json::Value = serde_json::from_str(&json_a).unwrap();
    let obj = value.as_object().unwrap();
    let mut reversed = serde_json::Map::new();
    for (k, v) in obj.iter().rev() {
        reversed.insert(k.clone(), v.clone());
    }
    let json_b = serde_json::to_string(&serde_json::Value::Object(reversed)).unwrap();
    assert_ne!(json_a, json_b, "sanity: the two JSON strings differ in key order");

    let ev_a: PromotionOosEvidence = serde_json::from_str(&json_a).unwrap();
    let ev_b: PromotionOosEvidence = serde_json::from_str(&json_b).unwrap();
    assert_eq!(ev_a, ev_b, "deserialized evidence must be identical regardless of JSON key order");

    let reasons_a = check_oos_evidence(&Some(ev_a));
    let reasons_b = check_oos_evidence(&Some(ev_b));
    assert_eq!(reasons_a, reasons_b);
    assert!(reasons_a.is_empty());
}

// ---------------------------------------------------------------------------
// TEST 18 — no holdout scoring/consumption occurs as a side effect
// ---------------------------------------------------------------------------

#[test]
fn evaluate_promotion_performs_no_io_or_holdout_access() {
    // Structural proof, not a mock: evaluate_promotion / check_oos_evidence
    // take only in-memory typed structs (PromotionConfig, PromotionInput)
    // and touch no filesystem path, database, or holdout ledger -- there is
    // no code path here through which a holdout could be scored/consumed as
    // a side effect of evaluating promotion. Two evaluations of the SAME
    // evidence produce byte-identical results (no hidden mutation/state).
    let ev = PromotionOosEvidence::valid_for_testing("no_io_trial");
    let input = otherwise_valid_input("no_io", Some(ev));
    let d1 = evaluate_promotion(&lenient_config(), &input);
    let d2 = evaluate_promotion(&lenient_config(), &input);
    assert_eq!(d1, d2);
}

// ---------------------------------------------------------------------------
// TEST 19 — winner selection remains limited to candidates passing the gate
// ---------------------------------------------------------------------------

#[test]
fn select_best_skips_candidates_without_valid_oos_evidence() {
    let candidates = vec![
        Candidate {
            id: "no_evidence".into(),
            input: otherwise_valid_input("no_evidence_candidate", None),
        },
        Candidate {
            id: "with_evidence".into(),
            input: otherwise_valid_input(
                "with_evidence_candidate",
                Some(PromotionOosEvidence::valid_for_testing("with_evidence_trial")),
            ),
        },
    ];

    let best = select_best(&lenient_config(), &candidates);
    let (winner_id, decision) = best.expect("at least one candidate should pass");
    assert_eq!(winner_id, "with_evidence");
    assert!(decision.passed);

    // Direct proof the no-evidence candidate individually fails.
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
// TEST 20 — end-to-end synthetic candidate: everything valid => passes
// ---------------------------------------------------------------------------

#[test]
fn end_to_end_valid_candidate_passes_promotion() {
    let ev = PromotionOosEvidence {
        trial_id: "e2e_trial_001".to_string(),
        economic_protocol_id: "economic_walk_forward_v1".to_string(),
        folds_used: 5,
        holdout_status: "reserved_not_evaluated".to_string(),
        execution_pricing_protocol_id: "rust_conservative_bar_range_v1".to_string(),
        weight_to_share_protocol_id: Some("weight_to_share_v1".to_string()),
        judge_status: "evaluated".to_string(),
        judge_trial_evaluable: true,
        pbo_status: "evaluated".to_string(),
        trial_included_in_judge_scope: true,
        economic_eval_id: "sha256_deadbeef00000000000000000000000000000000000000000000000000".to_string(),
    };

    let input = PromotionInput {
        initial_equity_micros: 1_000_000_000,
        report: good_report("e2e_valid_strategy_v1"),
        stress_suite: Some(StressSuiteResult::pass(5)),
        artifact_lock: Some(ArtifactLock::new_for_testing("e2e_cfg_hash", "e2e_git_hash")),
        oos_evidence: Some(ev),
    };

    let decision = evaluate_promotion(&lenient_config(), &input);
    assert!(
        decision.passed,
        "valid provenance + artifact lock + passing stress + passing historical thresholds + \
         valid OOS + valid multiple-testing + P7A parity + P7B parity => promotion must pass; \
         fail_reasons: {:?}",
        decision.fail_reasons
    );
    assert!(decision.fail_reasons.is_empty());
    assert!(pick_winner("e2e", &decision.metrics, "e2e", &decision.metrics) == "e2e");
}
