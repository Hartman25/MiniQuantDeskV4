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
