//! CANONICAL-ROBUSTNESS-PROMOTION-GATE-01 — P9 robustness evidence gate
//! tests for promotion evaluation.
//!
//! Validates:
//! - Promotion is blocked when `robustness_evidence` is `None` (gauntlet
//!   not run).
//! - Promotion is blocked when the evidence's protocol does not match the
//!   required P9 protocol -- even when everything else about it claims
//!   success.
//! - Promotion is blocked when the evidence is incomplete (a required
//!   scenario remains deferred).
//! - Promotion is blocked when a required scenario genuinely failed.
//! - Promotion passes when the evidence is complete, protocol-matching,
//!   and every applicable scenario passed.

mod common;

use mqk_backtest::{derive_input_data_hash, derive_run_id, BacktestConfig, BacktestReport};
use mqk_promotion::{
    evaluate_promotion, ArtifactLock, PromotionConfig, PromotionInput, RobustnessEvidence,
    StressSuiteResult, REQUIRED_ROBUSTNESS_PROTOCOL_VERSION, REQUIRED_STRESS_PROTOCOL_VERSION,
};

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

fn good_report() -> BacktestReport {
    let config_id = BacktestConfig::test_defaults().config_id();
    let input_data_hash = derive_input_data_hash(&[]);
    let run_id = derive_run_id(
        "robustness_gate_test_strategy_v1",
        &config_id,
        &input_data_hash,
    );
    BacktestReport {
        equity_curve: good_equity_curve(),
        strategy_name: "robustness_gate_test_strategy_v1".to_string(),
        run_id,
        config_id,
        input_data_hash,
        ..BacktestReport::test_fixture()
    }
}

fn base_input(robustness_evidence: Option<RobustnessEvidence>) -> PromotionInput {
    PromotionInput {
        initial_equity_micros: 1_000_000_000,
        report: good_report(),
        stress_suite: Some(StressSuiteResult::pass(3, REQUIRED_STRESS_PROTOCOL_VERSION)),
        artifact_lock: Some(ArtifactLock::new_for_testing("cfg_hash", "git_hash")),
        oos_evidence: Some(common::valid_oos_evidence_for_testing("robustness_gate_trial")),
        robustness_evidence,
    }
}

#[test]
fn robustness_evidence_missing_blocks_promotion() {
    let input = base_input(None);
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(!decision.passed, "promotion must be blocked when robustness_evidence is None");
    let reasons = decision.fail_reasons.join("; ");
    assert!(
        reasons.contains("Robustness evidence missing"),
        "got: {reasons}"
    );
}

#[test]
fn robustness_evidence_wrong_protocol_blocks_promotion() {
    let input = base_input(Some(RobustnessEvidence {
        protocol_version: "bkt_robustness_gauntlet_v0_fabricated".to_string(),
        is_complete: true,
        all_applicable_passed: true,
        failed_scenarios: Vec::new(),
        deferred_scenarios: Vec::new(),
        dsr_pbo_sensitivity_research_trial_id: Some("robustness_gate_trial".to_string()),
        p7a_p7b_economic_replay_stress_research_trial_id: Some("robustness_gate_trial".to_string()),
        p7a_p7b_economic_replay_stress_baseline_economic_eval_id: Some(
            "econ_eval_robustness_gate_trial".to_string(),
        ),
        p7a_p7b_economic_replay_stress_missing_required_evidence_fields: Vec::new(),
        genuine_shuffled_placebo_research_trial_id: Some("robustness_gate_trial".to_string()),
        genuine_shuffled_placebo_baseline_economic_eval_id: Some(
            "econ_eval_robustness_gate_trial".to_string(),
        ),
        genuine_shuffled_placebo_protocol_id: Some(
            mqk_backtest::GENUINE_SHUFFLED_PLACEBO_PROTOCOL_ID.to_string(),
        ),
        dsr_pbo_sensitivity_authoritative_judge_artifact_sha256: Some(
            common::deterministic_judge_artifact_sha256_for_testing("robustness_gate_trial"),
        ),
    }));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(
        !decision.passed,
        "a complete, passed evidence bundle under the WRONG protocol must still block promotion"
    );
    let reasons = decision.fail_reasons.join("; ");
    assert!(reasons.contains("Robustness evidence protocol mismatch"), "got: {reasons}");
    assert!(reasons.contains(REQUIRED_ROBUSTNESS_PROTOCOL_VERSION), "got: {reasons}");
}

#[test]
fn robustness_evidence_incomplete_blocks_promotion() {
    let input = base_input(Some(RobustnessEvidence {
        protocol_version: REQUIRED_ROBUSTNESS_PROTOCOL_VERSION.to_string(),
        is_complete: false,
        all_applicable_passed: true, // every scenario that DID run passed
        failed_scenarios: Vec::new(),
        deferred_scenarios: vec!["dsr_pbo_sensitivity".to_string()],
        dsr_pbo_sensitivity_research_trial_id: None, // scenario itself is still deferred
        p7a_p7b_economic_replay_stress_research_trial_id: None, // scenario itself is still deferred
        p7a_p7b_economic_replay_stress_baseline_economic_eval_id: None, // scenario itself is still deferred
        p7a_p7b_economic_replay_stress_missing_required_evidence_fields: vec![
            "<scenario evidence entirely absent>".to_string(),
        ],
        genuine_shuffled_placebo_research_trial_id: None, // scenario itself is still deferred
        genuine_shuffled_placebo_baseline_economic_eval_id: None, // scenario itself is still deferred
        genuine_shuffled_placebo_protocol_id: None, // scenario itself is still deferred
        dsr_pbo_sensitivity_authoritative_judge_artifact_sha256: None, // scenario itself is still deferred
    }));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(
        !decision.passed,
        "a deferred required scenario must block promotion even if everything else passed"
    );
    let reasons = decision.fail_reasons.join("; ");
    assert!(reasons.contains("Robustness evidence incomplete"), "got: {reasons}");
    assert!(reasons.contains("dsr_pbo_sensitivity"), "got: {reasons}");
}

#[test]
fn robustness_evidence_failed_scenario_blocks_promotion() {
    let input = base_input(Some(RobustnessEvidence {
        protocol_version: REQUIRED_ROBUSTNESS_PROTOCOL_VERSION.to_string(),
        is_complete: true,
        all_applicable_passed: false,
        failed_scenarios: vec!["symbol_leave_one_out: excluding ES breaches conservative bar".to_string()],
        deferred_scenarios: Vec::new(),
        dsr_pbo_sensitivity_research_trial_id: Some("robustness_gate_trial".to_string()),
        p7a_p7b_economic_replay_stress_research_trial_id: Some("robustness_gate_trial".to_string()),
        p7a_p7b_economic_replay_stress_baseline_economic_eval_id: Some(
            "econ_eval_robustness_gate_trial".to_string(),
        ),
        p7a_p7b_economic_replay_stress_missing_required_evidence_fields: Vec::new(),
        genuine_shuffled_placebo_research_trial_id: Some("robustness_gate_trial".to_string()),
        genuine_shuffled_placebo_baseline_economic_eval_id: Some(
            "econ_eval_robustness_gate_trial".to_string(),
        ),
        genuine_shuffled_placebo_protocol_id: Some(
            mqk_backtest::GENUINE_SHUFFLED_PLACEBO_PROTOCOL_ID.to_string(),
        ),
        dsr_pbo_sensitivity_authoritative_judge_artifact_sha256: Some(
            common::deterministic_judge_artifact_sha256_for_testing("robustness_gate_trial"),
        ),
    }));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(!decision.passed, "a genuinely failed required scenario must block promotion");
    let reasons = decision.fail_reasons.join("; ");
    assert!(reasons.contains("Robustness evidence failed"), "got: {reasons}");
    assert!(reasons.contains("symbol_leave_one_out"), "got: {reasons}");
}

#[test]
fn robustness_evidence_complete_and_passed_allows_promotion() {
    let input = base_input(Some(RobustnessEvidence {
        protocol_version: REQUIRED_ROBUSTNESS_PROTOCOL_VERSION.to_string(),
        is_complete: true,
        all_applicable_passed: true,
        failed_scenarios: Vec::new(),
        deferred_scenarios: Vec::new(),
        dsr_pbo_sensitivity_research_trial_id: Some("robustness_gate_trial".to_string()),
        p7a_p7b_economic_replay_stress_research_trial_id: Some("robustness_gate_trial".to_string()),
        p7a_p7b_economic_replay_stress_baseline_economic_eval_id: Some(
            "econ_eval_robustness_gate_trial".to_string(),
        ),
        p7a_p7b_economic_replay_stress_missing_required_evidence_fields: Vec::new(),
        genuine_shuffled_placebo_research_trial_id: Some("robustness_gate_trial".to_string()),
        genuine_shuffled_placebo_baseline_economic_eval_id: Some(
            "econ_eval_robustness_gate_trial".to_string(),
        ),
        genuine_shuffled_placebo_protocol_id: Some(
            mqk_backtest::GENUINE_SHUFFLED_PLACEBO_PROTOCOL_ID.to_string(),
        ),
        dsr_pbo_sensitivity_authoritative_judge_artifact_sha256: Some(
            common::deterministic_judge_artifact_sha256_for_testing("robustness_gate_trial"),
        ),
    }));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(
        decision.passed,
        "complete, protocol-matching, fully-passed robustness evidence must not block promotion; \
         fail_reasons: {:?}",
        decision.fail_reasons
    );
    assert!(decision.fail_reasons.is_empty());
}

/// P7A-P7B-ECONOMIC-REPLAY-STRESS-01 negative controls #6/#9: the
/// `p7a_p7b_economic_replay_stress` trial-binding gate must be enforced
/// INDEPENDENTLY of the `dsr_pbo_sensitivity` one -- `dsr_pbo_sensitivity_
/// research_trial_id` here correctly matches the P7C/OOS trial
/// (`robustness_gate_trial`), isolating this failure to the NEW gate alone
/// (a genuinely distinct trial's stress evidence, e.g. trial B's replay
/// result, supplied for trial A's promotion).
#[test]
fn p7a_p7b_replay_stress_trial_mismatch_blocks_promotion_even_when_dsr_pbo_matches() {
    let input = base_input(Some(RobustnessEvidence {
        protocol_version: REQUIRED_ROBUSTNESS_PROTOCOL_VERSION.to_string(),
        is_complete: true,
        all_applicable_passed: true,
        failed_scenarios: Vec::new(),
        deferred_scenarios: Vec::new(),
        dsr_pbo_sensitivity_research_trial_id: Some("robustness_gate_trial".to_string()),
        p7a_p7b_economic_replay_stress_research_trial_id: Some("a_different_trial_b".to_string()),
        p7a_p7b_economic_replay_stress_baseline_economic_eval_id: Some(
            "econ_eval_a_different_trial_b".to_string(),
        ),
        p7a_p7b_economic_replay_stress_missing_required_evidence_fields: Vec::new(),
        genuine_shuffled_placebo_research_trial_id: Some("robustness_gate_trial".to_string()),
        genuine_shuffled_placebo_baseline_economic_eval_id: Some(
            "econ_eval_robustness_gate_trial".to_string(),
        ),
        genuine_shuffled_placebo_protocol_id: Some(
            mqk_backtest::GENUINE_SHUFFLED_PLACEBO_PROTOCOL_ID.to_string(),
        ),
        dsr_pbo_sensitivity_authoritative_judge_artifact_sha256: Some(
            common::deterministic_judge_artifact_sha256_for_testing("robustness_gate_trial"),
        ),
    }));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(
        !decision.passed,
        "a p7a_p7b_economic_replay_stress result bound to a DIFFERENT research trial must block \
         promotion even though dsr_pbo_sensitivity is correctly bound"
    );
    let reasons = decision.fail_reasons.join("; ");
    assert!(reasons.contains("p7a_p7b_economic_replay_stress"), "got: {reasons}");
    assert!(reasons.contains("a_different_trial_b"), "got: {reasons}");
    assert!(reasons.contains("robustness_gate_trial"), "got: {reasons}");
}

/// Same invariant, missing-binding form: `p7a_p7b_economic_replay_stress`
/// evidence present but carrying no `research_trial_id` at all must block
/// promotion just as an explicit mismatch does -- `None` is never treated
/// as an assumed match.
#[test]
fn p7a_p7b_replay_stress_missing_trial_binding_blocks_promotion_even_when_dsr_pbo_matches() {
    let input = base_input(Some(RobustnessEvidence {
        protocol_version: REQUIRED_ROBUSTNESS_PROTOCOL_VERSION.to_string(),
        is_complete: true,
        all_applicable_passed: true,
        failed_scenarios: Vec::new(),
        deferred_scenarios: Vec::new(),
        dsr_pbo_sensitivity_research_trial_id: Some("robustness_gate_trial".to_string()),
        p7a_p7b_economic_replay_stress_research_trial_id: None,
        p7a_p7b_economic_replay_stress_baseline_economic_eval_id: None,
        p7a_p7b_economic_replay_stress_missing_required_evidence_fields: vec![
            "<scenario evidence entirely absent>".to_string(),
        ],
        genuine_shuffled_placebo_research_trial_id: Some("robustness_gate_trial".to_string()),
        genuine_shuffled_placebo_baseline_economic_eval_id: Some(
            "econ_eval_robustness_gate_trial".to_string(),
        ),
        genuine_shuffled_placebo_protocol_id: Some(
            mqk_backtest::GENUINE_SHUFFLED_PLACEBO_PROTOCOL_ID.to_string(),
        ),
        dsr_pbo_sensitivity_authoritative_judge_artifact_sha256: Some(
            common::deterministic_judge_artifact_sha256_for_testing("robustness_gate_trial"),
        ),
    }));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(
        !decision.passed,
        "p7a_p7b_economic_replay_stress evidence with no bound research_trial_id must block \
         promotion even though dsr_pbo_sensitivity is correctly bound"
    );
    let reasons = decision.fail_reasons.join("; ");
    assert!(reasons.contains("p7a_p7b_economic_replay_stress"), "got: {reasons}");
    assert!(reasons.contains("Research trial binding missing"), "got: {reasons}");
}

// ---------------------------------------------------------------------------
// FINAL-P9-AUTHORITY-BINDING-REPAIR-01: Sections 1, 3, 4 negative controls
// ---------------------------------------------------------------------------

/// A fully valid, complete, protocol-matching, fully-bound P9 robustness
/// evidence bundle for `research_trial_id = "robustness_gate_trial"` (the
/// SAME trial `base_input` uses) -- the baseline every negative control
/// below mutates exactly ONE field of via struct-update syntax, isolating
/// each new gate's failure to that ONE field.
fn full_valid_robustness_evidence() -> RobustnessEvidence {
    RobustnessEvidence {
        protocol_version: REQUIRED_ROBUSTNESS_PROTOCOL_VERSION.to_string(),
        is_complete: true,
        all_applicable_passed: true,
        failed_scenarios: Vec::new(),
        deferred_scenarios: Vec::new(),
        dsr_pbo_sensitivity_research_trial_id: Some("robustness_gate_trial".to_string()),
        p7a_p7b_economic_replay_stress_research_trial_id: Some("robustness_gate_trial".to_string()),
        p7a_p7b_economic_replay_stress_baseline_economic_eval_id: Some(
            "econ_eval_robustness_gate_trial".to_string(),
        ),
        p7a_p7b_economic_replay_stress_missing_required_evidence_fields: Vec::new(),
        genuine_shuffled_placebo_research_trial_id: Some("robustness_gate_trial".to_string()),
        genuine_shuffled_placebo_baseline_economic_eval_id: Some(
            "econ_eval_robustness_gate_trial".to_string(),
        ),
        genuine_shuffled_placebo_protocol_id: Some(
            mqk_backtest::GENUINE_SHUFFLED_PLACEBO_PROTOCOL_ID.to_string(),
        ),
        dsr_pbo_sensitivity_authoritative_judge_artifact_sha256: Some(
            common::deterministic_judge_artifact_sha256_for_testing("robustness_gate_trial"),
        ),
    }
}

/// Section 1 positive control is `robustness_evidence_complete_and_passed_allows_promotion`
/// above. Negative control: a `dsr_pbo_sensitivity` judge-scope binding that
/// disagrees with the P7C/OOS evidence's own verified judge scope must block
/// promotion, even though every OTHER binding (trial, economic result,
/// placebo) is correct.
#[test]
fn dsr_pbo_sensitivity_judge_scope_mismatch_blocks_promotion() {
    let input = base_input(Some(RobustnessEvidence {
        dsr_pbo_sensitivity_authoritative_judge_artifact_sha256: Some(
            "a_completely_different_judge_sha256".to_string(),
        ),
        ..full_valid_robustness_evidence()
    }));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(
        !decision.passed,
        "a dsr_pbo_sensitivity judge scope bound to a DIFFERENT judge_artifact_sha256 than the \
         P7C/OOS evidence must block promotion"
    );
    let reasons = decision.fail_reasons.join("; ");
    assert!(reasons.contains("Judge scope binding mismatch"), "got: {reasons}");
}

/// Missing-binding form: no `dsr_pbo_sensitivity_authoritative_judge_artifact_sha256`
/// at all must block promotion just as an explicit mismatch does.
#[test]
fn dsr_pbo_sensitivity_missing_judge_scope_binding_blocks_promotion() {
    let input = base_input(Some(RobustnessEvidence {
        dsr_pbo_sensitivity_authoritative_judge_artifact_sha256: None,
        ..full_valid_robustness_evidence()
    }));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(!decision.passed);
    let reasons = decision.fail_reasons.join("; ");
    assert!(reasons.contains("Judge scope binding missing"), "got: {reasons}");
}

/// Section 3: `genuine_shuffled_placebo` bound to a DIFFERENT research trial
/// than the P7C/OOS evidence must block promotion.
#[test]
fn genuine_shuffled_placebo_trial_mismatch_blocks_promotion() {
    let input = base_input(Some(RobustnessEvidence {
        genuine_shuffled_placebo_research_trial_id: Some("a_different_trial_c".to_string()),
        ..full_valid_robustness_evidence()
    }));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(!decision.passed);
    let reasons = decision.fail_reasons.join("; ");
    assert!(reasons.contains("genuine_shuffled_placebo"), "got: {reasons}");
    assert!(reasons.contains("Research trial binding mismatch"), "got: {reasons}");
    assert!(reasons.contains("a_different_trial_c"), "got: {reasons}");
}

/// Section 3: `genuine_shuffled_placebo` bound to a DIFFERENT economic
/// result than the P7C/OOS evidence, under the SAME trial, must block
/// promotion (two valid trials/evals under the same strategy scenario).
#[test]
fn genuine_shuffled_placebo_economic_eval_mismatch_blocks_promotion() {
    let input = base_input(Some(RobustnessEvidence {
        genuine_shuffled_placebo_baseline_economic_eval_id: Some(
            "econ_eval_a_different_trial_b".to_string(),
        ),
        ..full_valid_robustness_evidence()
    }));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(!decision.passed);
    let reasons = decision.fail_reasons.join("; ");
    assert!(reasons.contains("genuine_shuffled_placebo"), "got: {reasons}");
    assert!(reasons.contains("Economic result binding mismatch"), "got: {reasons}");
}

/// Section 3: `genuine_shuffled_placebo` declaring the WRONG protocol_id
/// must block promotion -- only the exact accepted placebo protocol counts.
#[test]
fn genuine_shuffled_placebo_wrong_protocol_blocks_promotion() {
    let input = base_input(Some(RobustnessEvidence {
        genuine_shuffled_placebo_protocol_id: Some("temporal_offset_placebo_v0_fabricated".to_string()),
        ..full_valid_robustness_evidence()
    }));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(!decision.passed);
    let reasons = decision.fail_reasons.join("; ");
    assert!(reasons.contains("genuine_shuffled_placebo protocol mismatch"), "got: {reasons}");
}

/// Section 4: a `p7a_p7b_economic_replay_stress` scenario missing REQUIRED
/// structured evidence fields (mirroring a fabricated scenario carrying only
/// `baseline_economic_eval_id`, the ONLY field the prior canonical check
/// required) must block promotion even though every trial/economic-result
/// binding is otherwise correct.
#[test]
fn p7a_p7b_replay_stress_missing_required_evidence_fields_blocks_promotion() {
    let input = base_input(Some(RobustnessEvidence {
        p7a_p7b_economic_replay_stress_missing_required_evidence_fields: vec![
            "protocol_id".to_string(),
            "stressed_max_drawdown".to_string(),
        ],
        ..full_valid_robustness_evidence()
    }));
    let decision = evaluate_promotion(&lenient_config(), &input);

    assert!(
        !decision.passed,
        "incomplete p7a_p7b_economic_replay_stress evidence must block promotion even with \
         correct trial/economic-result binding"
    );
    let reasons = decision.fail_reasons.join("; ");
    assert!(
        reasons.contains("p7a_p7b_economic_replay_stress evidence incomplete"),
        "got: {reasons}"
    );
    assert!(reasons.contains("protocol_id"), "got: {reasons}");
    assert!(reasons.contains("stressed_max_drawdown"), "got: {reasons}");
}
