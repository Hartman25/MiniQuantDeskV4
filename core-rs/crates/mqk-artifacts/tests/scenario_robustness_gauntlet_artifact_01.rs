//! P9 `BKT-ROBUSTNESS-GAUNTLET-01` — durable, tamper-evident,
//! candidate-bound artifact authority for `robustness_gauntlet.json`.
//!
//! Mirrors `scenario_stress_suite_authority_01.rs`'s structure; see that
//! file for the fuller rationale behind each control.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use mqk_artifacts::{
    finalize_canonical_robustness_gauntlet_with_sensitivity, load_canonical_robustness_gauntlet,
    InitRunArtifactsArgs, RobustnessGauntletArtifactError, RobustnessGauntletFinalizeError,
};
use mqk_backtest::{run_robustness_gauntlet, BacktestBar, BacktestConfig, BacktestEngine};
use mqk_strategy::{Strategy, StrategyContext, StrategyOutput, StrategySpec, TargetPosition};

const M: i64 = 1_000_000;
static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct HoldFlat {
    name: &'static str,
}

impl Strategy for HoldFlat {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(self.name, 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput::new(vec![TargetPosition::new("ES", 0)])
    }
}

fn flat_bar(end_ts: i64, price_usd: i64) -> BacktestBar {
    let p = price_usd * M;
    BacktestBar::new("ES", end_ts, p, p, p, p, 1_000)
}

fn cfg_with_wide_cap() -> BacktestConfig {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 100_000_000;
    cfg
}

fn bars() -> Vec<BacktestBar> {
    vec![flat_bar(1_700_000_060, 500), flat_bar(1_700_000_120, 501)]
}

fn run_and_persist(
    label: &str,
    strategy_name: &'static str,
) -> (mqk_backtest::BacktestReport, PathBuf, BacktestConfig, Vec<BacktestBar>) {
    let config = cfg_with_wide_cap();
    let initial_cash = config.initial_cash_micros;
    let bars = bars();

    let mut engine = BacktestEngine::new(config.clone());
    engine.add_strategy(Box::new(HoldFlat { name: strategy_name })).unwrap();
    let report = engine.run(&bars).expect("engine.run must succeed");

    let seq = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("mqk_rga01_{}_{}_{}", label, std::process::id(), seq));
    let _ = fs::remove_dir_all(&root);

    let config_hash = report.config_id.to_string();
    let init_result = mqk_artifacts::init_run_artifacts(InitRunArtifactsArgs {
        exports_root: &root,
        schema_version: 1,
        run_id: report.run_id,
        strategy_name: &report.strategy_name,
        engine_id: "mqk-backtest",
        mode: "backtest",
        timeframe: None,
        timeframe_secs: Some(60),
        git_hash: "rga01_test_git_hash",
        config_hash: &config_hash,
        host_fingerprint: "rga01_test_host",
        now_utc: chrono::Utc::now(),
    })
    .expect("init_run_artifacts must succeed");

    mqk_artifacts::write_backtest_report(&init_result.run_dir, &report, initial_cash)
        .expect("write_backtest_report must succeed");

    (report, init_result.run_dir, config, bars)
}

fn cleanup(root: &Path) {
    let _ = fs::remove_dir_all(root);
}

fn tamper(run_dir: &Path, mutate: impl FnOnce(&mut serde_json::Value)) {
    let path = run_dir.join("robustness_gauntlet.json");
    let raw = fs::read_to_string(&path).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    mutate(&mut v);
    fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

#[test]
fn rga01a_real_round_trip() {
    let (report, run_dir, config, bars) = run_and_persist("valid", "RgaValid");
    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(HoldFlat { name: "RgaValid" })
    });
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();

    let loaded = load_canonical_robustness_gauntlet(&run_dir).expect("must load");
    assert_eq!(loaded.scenarios_run(), output.scenarios.len());
    assert_eq!(
        loaded.deferred_scenario_names().len(),
        1,
        "only dsr_pbo_sensitivity is deferred by this pure, engine-only run -- \
         conservative_capacity_stress is now a real scenario"
    );

    cleanup(&run_dir);
}

#[test]
fn rga01b_missing_artifact_rejected() {
    let (_report, run_dir, _config, _bars) = run_and_persist("missing", "RgaMissing");
    let err = load_canonical_robustness_gauntlet(&run_dir).unwrap_err();
    assert_eq!(err, RobustnessGauntletArtifactError::MissingArtifact);
    cleanup(&run_dir);
}

#[test]
fn rga01c_zero_scenarios_rejected() {
    let (report, run_dir, config, bars) = run_and_persist("zero", "RgaZero");
    let output = run_robustness_gauntlet(&report, &config, &bars, || Box::new(HoldFlat { name: "RgaZero" }));
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();

    tamper(&run_dir, |v| {
        v["scenarios"] = serde_json::Value::Array(vec![]);
    });

    let err = load_canonical_robustness_gauntlet(&run_dir).unwrap_err();
    assert_eq!(err, RobustnessGauntletArtifactError::ZeroScenarios);
    cleanup(&run_dir);
}

#[test]
fn rga01d_content_tampered_after_write_rejected() {
    let (report, run_dir, config, bars) = run_and_persist("tamper", "RgaTamper");
    let output = run_robustness_gauntlet(&report, &config, &bars, || Box::new(HoldFlat { name: "RgaTamper" }));
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();

    tamper(&run_dir, |v| {
        v["scenarios"][0]["passed"] = serde_json::Value::Bool(!v["scenarios"][0]["passed"].as_bool().unwrap());
    });

    let err = load_canonical_robustness_gauntlet(&run_dir).unwrap_err();
    assert_eq!(err, RobustnessGauntletArtifactError::ContentHashMismatch);
    cleanup(&run_dir);
}

#[test]
fn rga01e_cross_candidate_swap_rejected() {
    let (report_a, run_dir_a, config_a, bars_a) = run_and_persist("cross_a", "RgaCrossA");
    let (report_b, run_dir_b, _config_b, _bars_b) = run_and_persist("cross_b", "RgaCrossB");
    assert_ne!(report_a.run_id, report_b.run_id);

    let output_a = run_robustness_gauntlet(&report_a, &config_a, &bars_a, || {
        Box::new(HoldFlat { name: "RgaCrossA" })
    });
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir_a, &output_a).unwrap();

    fs::copy(
        run_dir_a.join("robustness_gauntlet.json"),
        run_dir_b.join("robustness_gauntlet.json"),
    )
    .unwrap();

    let err = load_canonical_robustness_gauntlet(&run_dir_b).unwrap_err();
    assert!(matches!(err, RobustnessGauntletArtifactError::RunIdMismatch { .. }));

    cleanup(&run_dir_a);
    cleanup(&run_dir_b);
}

#[test]
fn rga01f_write_is_idempotent() {
    let (report, run_dir, config, bars) = run_and_persist("idempotent", "RgaIdempotent");
    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(HoldFlat { name: "RgaIdempotent" })
    });

    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();

    let audit_jsonl = fs::read_to_string(run_dir.join("audit.jsonl")).unwrap();
    let count = audit_jsonl
        .lines()
        .filter(|l| l.contains("robustness_gauntlet_completed"))
        .count();
    assert_eq!(count, 1);

    let verify = mqk_audit::verify_hash_chain_str(&audit_jsonl).unwrap();
    assert!(matches!(verify, mqk_audit::VerifyResult::Valid { .. }));

    cleanup(&run_dir);
}

/// Same "consistent, not tampered" construction as
/// `scenario_stress_suite_authority_01.rs`'s `bsaa01l`: mutating
/// `RobustnessGauntletOutput.protocol_version` BEFORE writing means the
/// resulting JSON and its audit hash agree perfectly; only the structural
/// protocol-identity check can reject it.
#[test]
fn rga01g_wrong_protocol_version_rejected() {
    let (report, run_dir, config, bars) = run_and_persist("wrong_protocol", "RgaWrongProtocol");
    let mut output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(HoldFlat { name: "RgaWrongProtocol" })
    });
    output.protocol_version = "bkt_robustness_gauntlet_v0_fabricated".to_string();
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();

    let err = load_canonical_robustness_gauntlet(&run_dir).unwrap_err();
    assert_eq!(
        err,
        RobustnessGauntletArtifactError::UnsupportedProtocolVersion(
            "bkt_robustness_gauntlet_v0_fabricated".to_string()
        )
    );
    cleanup(&run_dir);
}

/// A loaded artifact from `run_robustness_gauntlet` alone (before
/// `dsr_pbo_sensitivity` is merged in) is structurally VALID (loads
/// successfully) but not COMPLETE -- `is_complete()` is a separate,
/// explicit check, never conflated with "loads without error".
#[test]
fn rga01h_loaded_artifact_incomplete_until_dsr_pbo_merged() {
    let (report, run_dir, config, bars) = run_and_persist("incomplete", "RgaIncomplete");
    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(HoldFlat { name: "RgaIncomplete" })
    });
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();

    let loaded = load_canonical_robustness_gauntlet(&run_dir).expect("must load");
    assert!(
        !loaded.is_complete(),
        "must be incomplete: dsr_pbo_sensitivity was never merged before writing"
    );

    cleanup(&run_dir);
}

/// Once `dsr_pbo_sensitivity` is merged in before writing, the loaded
/// artifact reports `is_complete() == true`.
#[test]
fn rga01i_loaded_artifact_complete_after_dsr_pbo_merged() {
    use mqk_backtest::RobustnessScenarioOutcome;

    let (report, run_dir, config, bars) = run_and_persist("complete", "RgaComplete");
    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(HoldFlat { name: "RgaComplete" })
    })
    .merge_dsr_pbo_sensitivity(RobustnessScenarioOutcome {
        name: "dsr_pbo_sensitivity".to_string(),
        applicable: true,
        passed: true,
        reason: None,
        detail: "test-fabricated evaluated outcome".to_string(),
        research_trial_id: Some("rga01i_test_trial".to_string()),
    });
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();

    let loaded = load_canonical_robustness_gauntlet(&run_dir).expect("must load");
    assert!(loaded.is_complete(), "must be complete once dsr_pbo_sensitivity is merged in");
    assert!(loaded.deferred_scenario_names().is_empty());

    cleanup(&run_dir);
}

// ---------------------------------------------------------------------------
// BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01: finalize seam
// ---------------------------------------------------------------------------

fn fake_sensitivity(passed: bool) -> mqk_backtest::RobustnessScenarioOutcome {
    mqk_backtest::RobustnessScenarioOutcome {
        name: mqk_backtest::DSR_PBO_SENSITIVITY_SCENARIO_NAME.to_string(),
        applicable: true,
        passed,
        reason: if passed { None } else { Some("dsr_range exceeded ceiling".to_string()) },
        detail: "test-fabricated finalize-seam outcome".to_string(),
        research_trial_id: Some("fake_sensitivity_trial".to_string()),
    }
}

/// The real production two-phase flow: `write_canonical_robustness_gauntlet`
/// (inline, at backtest-execution time) produces a structurally valid but
/// INCOMPLETE artifact; `finalize_canonical_robustness_gauntlet_with_sensitivity`
/// (a later, separate phase) merges the real sensitivity result in --
/// the loaded artifact then reports `is_complete() == true`.
#[test]
fn rga01j_finalize_merges_sensitivity_into_incomplete_artifact() {
    let (report, run_dir, config, bars) = run_and_persist("finalize_basic", "RgaFinalizeBasic");
    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(HoldFlat { name: "RgaFinalizeBasic" })
    });
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();
    assert!(!load_canonical_robustness_gauntlet(&run_dir).unwrap().is_complete());

    let sensitivity = fake_sensitivity(true);
    finalize_canonical_robustness_gauntlet_with_sensitivity(&run_dir, &sensitivity)
        .expect("finalize must succeed against a structurally valid existing artifact");

    let loaded = load_canonical_robustness_gauntlet(&run_dir).expect("must load after finalize");
    assert!(loaded.is_complete(), "must be complete after merging dsr_pbo_sensitivity");
    assert!(
        !loaded.failed_scenario_descriptions().iter().any(|d| d.starts_with("dsr_pbo_sensitivity")),
        "the merged sensitivity scenario passed and must not appear as failed: {:?}",
        loaded.failed_scenario_descriptions()
    );
    assert_eq!(loaded.scenarios_run(), 7, "6 pure scenarios + 1 finalized sensitivity");

    cleanup(&run_dir);
}

/// A finalization retry with IDENTICAL sensitivity content is a genuine
/// no-op -- never a duplicate scenario, never a second audit event.
#[test]
fn rga01k_finalize_is_idempotent_on_identical_replay() {
    let (report, run_dir, config, bars) = run_and_persist("finalize_idem", "RgaFinalizeIdem");
    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(HoldFlat { name: "RgaFinalizeIdem" })
    });
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();

    let sensitivity = fake_sensitivity(true);
    finalize_canonical_robustness_gauntlet_with_sensitivity(&run_dir, &sensitivity).unwrap();
    finalize_canonical_robustness_gauntlet_with_sensitivity(&run_dir, &sensitivity)
        .expect("identical replay must be idempotent, not an error");

    let loaded = load_canonical_robustness_gauntlet(&run_dir).expect("must still load");
    assert_eq!(loaded.scenarios_run(), 7, "replay must not duplicate the scenario");

    let audit_jsonl = fs::read_to_string(run_dir.join("audit.jsonl")).unwrap();
    let finalized_count = audit_jsonl
        .lines()
        .filter(|l| l.contains("robustness_gauntlet_finalized"))
        .count();
    assert_eq!(finalized_count, 1, "replay must not append a second finalized audit event");

    cleanup(&run_dir);
}

/// A finalization retry with a DIFFERENT sensitivity result for the same
/// already-finalized artifact is refused -- real evidence is never silently
/// overwritten by a conflicting later claim.
#[test]
fn rga01l_finalize_rejects_conflicting_sensitivity() {
    let (report, run_dir, config, bars) = run_and_persist("finalize_conflict", "RgaFinalizeConflict");
    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(HoldFlat { name: "RgaFinalizeConflict" })
    });
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();

    finalize_canonical_robustness_gauntlet_with_sensitivity(&run_dir, &fake_sensitivity(true)).unwrap();

    let err = finalize_canonical_robustness_gauntlet_with_sensitivity(&run_dir, &fake_sensitivity(false))
        .unwrap_err();
    assert_eq!(err, RobustnessGauntletFinalizeError::ConflictingSensitivityResult);

    // The original, genuinely-finalized evidence must remain untouched --
    // still the PASSED sensitivity result, never the rejected failed one.
    let loaded = load_canonical_robustness_gauntlet(&run_dir).expect("must still load");
    assert!(
        !loaded.failed_scenario_descriptions().iter().any(|d| d.starts_with("dsr_pbo_sensitivity")),
        "the rejected conflicting (failed) result must never have applied: {:?}",
        loaded.failed_scenario_descriptions()
    );

    cleanup(&run_dir);
}

/// PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01: a finalization retry with
/// otherwise-IDENTICAL applicable/passed/reason/detail but a DIFFERENT
/// `research_trial_id` must still be refused as conflicting -- silently
/// keeping the first trial's binding would defeat the whole point of
/// recording which Research trial produced this evidence.
#[test]
fn rga01p_finalize_rejects_same_content_different_trial_id() {
    let (report, run_dir, config, bars) =
        run_and_persist("finalize_trial_conflict", "RgaFinalizeTrialConflict");
    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(HoldFlat { name: "RgaFinalizeTrialConflict" })
    });
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();

    let mut first = fake_sensitivity(true);
    first.research_trial_id = Some("trial_a".to_string());
    finalize_canonical_robustness_gauntlet_with_sensitivity(&run_dir, &first).unwrap();

    let mut second = fake_sensitivity(true); // same applicable/passed/reason/detail
    second.research_trial_id = Some("trial_b".to_string()); // different trial
    let err =
        finalize_canonical_robustness_gauntlet_with_sensitivity(&run_dir, &second).unwrap_err();
    assert_eq!(err, RobustnessGauntletFinalizeError::ConflictingSensitivityResult);

    // The original binding (trial_a) must remain untouched.
    let loaded = load_canonical_robustness_gauntlet(&run_dir).expect("must still load");
    assert_eq!(
        loaded.dsr_pbo_sensitivity_research_trial_id(),
        Some("trial_a"),
        "the rejected conflicting trial binding must never have applied"
    );

    cleanup(&run_dir);
}

/// The canonical resolver accessor round-trips the exact trial_id recorded
/// at finalization time, and reports `None` before finalization.
#[test]
fn rga01q_dsr_pbo_sensitivity_research_trial_id_accessor() {
    let (report, run_dir, config, bars) =
        run_and_persist("finalize_trial_accessor", "RgaFinalizeTrialAccessor");
    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(HoldFlat { name: "RgaFinalizeTrialAccessor" })
    });
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();
    assert_eq!(
        load_canonical_robustness_gauntlet(&run_dir)
            .unwrap()
            .dsr_pbo_sensitivity_research_trial_id(),
        None,
        "no binding proof exists before dsr_pbo_sensitivity is finalized"
    );

    let mut sensitivity = fake_sensitivity(true);
    sensitivity.research_trial_id = Some("rga01q_trial".to_string());
    finalize_canonical_robustness_gauntlet_with_sensitivity(&run_dir, &sensitivity).unwrap();

    let loaded = load_canonical_robustness_gauntlet(&run_dir).expect("must load after finalize");
    assert_eq!(
        loaded.dsr_pbo_sensitivity_research_trial_id(),
        Some("rga01q_trial")
    );

    // The audit event also carries it explicitly (not merely inside the
    // hashed JSON body), directly queryable without parsing the artifact.
    let audit_jsonl = fs::read_to_string(run_dir.join("audit.jsonl")).unwrap();
    let finalized_line = audit_jsonl
        .lines()
        .find(|l| l.contains("robustness_gauntlet_finalized"))
        .expect("finalized audit event must exist");
    assert!(
        finalized_line.contains("\"dsr_pbo_sensitivity_research_trial_id\":\"rga01q_trial\""),
        "audit payload must carry the trial binding explicitly: {finalized_line}"
    );

    cleanup(&run_dir);
}

/// A caller bug (wrong scenario name) is refused before touching disk.
#[test]
fn rga01m_finalize_rejects_wrong_scenario_name() {
    let (report, run_dir, config, bars) = run_and_persist("finalize_wrong_name", "RgaFinalizeWrongName");
    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(HoldFlat { name: "RgaFinalizeWrongName" })
    });
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();

    let mut wrong = fake_sensitivity(true);
    wrong.name = "not_dsr_pbo_sensitivity".to_string();
    let err = finalize_canonical_robustness_gauntlet_with_sensitivity(&run_dir, &wrong).unwrap_err();
    assert_eq!(
        err,
        RobustnessGauntletFinalizeError::WrongScenarioName("not_dsr_pbo_sensitivity".to_string())
    );

    cleanup(&run_dir);
}

/// Finalization requires a structurally valid EXISTING artifact -- it can
/// never create one from scratch. A candidate that never had its inline
/// evidence written at all is refused.
#[test]
fn rga01n_finalize_on_missing_artifact_fails() {
    let (_report, run_dir, _config, _bars) = run_and_persist("finalize_missing", "RgaFinalizeMissing");
    // Deliberately never call write_canonical_robustness_gauntlet.

    let err = finalize_canonical_robustness_gauntlet_with_sensitivity(&run_dir, &fake_sensitivity(true))
        .unwrap_err();
    assert_eq!(
        err,
        RobustnessGauntletFinalizeError::ExistingArtifactInvalid(
            RobustnessGauntletArtifactError::MissingArtifact
        )
    );

    cleanup(&run_dir);
}

/// Interrupted finalization: if the process died after the JSON file was
/// updated but before the new audit event was appended, the artifact must
/// NEVER read back as valid (let alone complete) -- proven here by
/// reproducing exactly that half-written state directly (bypassing the
/// finalize function's own atomicity) and confirming the loader fails
/// closed with `ContentHashMismatch`, never silently accepting the
/// unaudited content.
#[test]
fn rga01o_interrupted_finalization_leaves_no_falsely_complete_artifact() {
    let (report, run_dir, config, bars) = run_and_persist("finalize_interrupt", "RgaFinalizeInterrupt");
    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(HoldFlat { name: "RgaFinalizeInterrupt" })
    });
    mqk_artifacts::write_canonical_robustness_gauntlet(&run_dir, &output).unwrap();

    // Simulate the exact interrupted state: rewrite robustness_gauntlet.json
    // with genuinely more-complete content (as finalize would), but WITHOUT
    // appending the corresponding robustness_gauntlet_finalized audit event.
    tamper(&run_dir, |v| {
        let arr = v["scenarios"].as_array_mut().unwrap();
        arr.push(serde_json::json!({
            "name": "dsr_pbo_sensitivity",
            "applicable": true,
            "passed": true,
            "reason": serde_json::Value::Null,
            "detail": "interrupted -- audit event never appended"
        }));
        let deferred = v["deferred"].as_array_mut().unwrap();
        deferred.clear();
    });

    let err = load_canonical_robustness_gauntlet(&run_dir).unwrap_err();
    assert_eq!(
        err,
        RobustnessGauntletArtifactError::ContentHashMismatch,
        "an artifact updated without its audit event must never read back as valid"
    );

    cleanup(&run_dir);
}
