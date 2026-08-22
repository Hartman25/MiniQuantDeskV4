//! P10 `RESEARCH-BACKTEST-FINAL-ACCEPTANCE-01` — load-bearing end-to-end
//! proof that a legitimate candidate flows:
//!
//!   real BacktestEngine run
//!     -> canonical artifacts (BKT-PROMOTION-ARTIFACT-AUTHORITY-01)
//!     -> real stress suite (PROMOTION-STRESS-SUITE-AUTHORITY-01)
//!     -> real robustness gauntlet (P9 BKT-ROBUSTNESS-GAUNTLET-01)
//!     -> canonical evidence resolution (PROMOTION-BACKTEST-EVIDENCE-SEAM-01)
//!     -> a real (test-constructed, schema-accurate) Research SQLite
//!        registry, verified via the accepted, FROZEN P7C mechanism
//!        (verify_promotion_oos_evidence)
//!     -> canonical evaluate_promotion
//!
//! without caller fabrication, cross-candidate evidence, or bypassing any
//! gate -- and that the negative paths fail closed.
//!
//! P10-RESEARCH-BACKTEST-FINAL-ACCEPTANCE-REPAIR-01: this file proves the
//! chain up to and including `evaluate_promotion` using synthetic-but-real
//! (non-fabricated engine, real artifact writers) evidence with a wide
//! matrix of positive/negative wiring checks. It deliberately does NOT
//! itself exercise the actual HTTP `POST /api/v1/strategy/promotions/
//! transition` route or a real Postgres instance -- that would duplicate,
//! not strengthen, coverage that already exists as real, DB-backed,
//! non-fabricated proof in `mqk-daemon`'s own test suite:
//!
//!   - `mqk-daemon/tests/scenario_strategy_promotion_routes_01.rs` --
//!     `valid_paper_candidate_creates_first_transition` and 26 sibling
//!     positive/negative-control tests, each running a REAL
//!     `BacktestEngine`/`swing_momentum` candidate through the REAL
//!     production evidence writers (`mqk_artifacts::write_backtest_report`/
//!     `write_canonical_stress_suite`/`write_canonical_robustness_gauntlet`/
//!     `finalize_canonical_robustness_gauntlet_with_sensitivity`, the last
//!     backed by a genuine Python DSR/PBO sensitivity subprocess -- never a
//!     fabricated `RobustnessScenarioOutcome`), then the REAL Axum route
//!     (`tower::oneshot`, no mock) against a real disposable Postgres.
//!   - `mqk-daemon/tests/scenario_strategy_promotion_closure_proof_01f.rs`
//!     -- `closure_proof_full_lifecycle_through_real_routes`: the complete
//!     no-state -> shadow_approved -> paper_approved -> active_paper ->
//!     demoted lifecycle through the same real route/DB, reading Postgres
//!     back after the evidence-requiring transition to confirm the
//!     persisted lineage (`evidence_transition_id`, `evidence_fingerprint`)
//!     identifies the exact evidence judged.
//!
//! Both landed in commit `37649200` ("promotion: prove canonical postgres
//! transition end to end", PRODUCTION-PROMOTION-DB-E2E-01), which also
//! contains this proof's own bypass audit: exactly one production caller of
//! `insert_strategy_promotion_transition_serialized`, mounted at exactly
//! one route. Together, this file plus that suite constitute the complete
//! P10 acceptance chain: Research registry/trial -> OOS/economic evidence
//! -> real Backtest -> production promotion-evidence finalization -> exact
//! stress protocol -> complete P9 -> candidate-bound evidence resolution ->
//! canonical `evaluate_promotion` -> real HTTP daemon promotion transition
//! -> atomic Postgres state + evidence lineage. Superseding the prior
//! `RESEARCH-BACKTEST-FINAL-ACCEPTANCE-01` (commit `41c19cc7`), whose own
//! scope note honestly flagged the HTTP/Postgres gap this now closes.
//!
//! Also out of scope, by design: "winner-only registration forbidden" and
//! "final holdout remains reserved" are `research-py` (Python)
//! invariants -- already covered by that codebase's own accepted test
//! suites (P6-CLOSURE's negative-control mutation test; P6B's 12-thread
//! concurrent-holdout-consume race proving exactly one winner; see prior
//! session memory / `docs/research/Research_Backtest_V1_Closeout_Audit.md`).
//! Re-deriving Python-side proof from a Rust test file would not be a
//! genuine test of that code -- this file does not fabricate one.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use mqk_backtest::{
    run_backtest_stress_suite, run_robustness_gauntlet, BacktestBar, BacktestConfig,
    BacktestEngine, BacktestReport,
};
use mqk_promotion::{
    evaluate_promotion, resolve_backtest_evidence, verify_promotion_oos_evidence, PromotionConfig,
    PromotionInput,
};
use mqk_strategy::{Strategy, StrategyContext, StrategyOutput, StrategySpec, TargetPosition};
use sha2::{Digest, Sha256};

const M: i64 = 1_000_000;
static SEQ: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

struct BuyHoldSell {
    name: &'static str,
    bar_idx: u64,
    qty: i64,
    sell_at_idx: u64,
}

impl Strategy for BuyHoldSell {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(self.name, 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        let target = if self.bar_idx < self.sell_at_idx { self.qty } else { 0 };
        StrategyOutput::new(vec![TargetPosition::new("ES", target)])
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

/// A profitable, non-fragile run: modest, steady gain across many bars.
fn profitable_bars() -> Vec<BacktestBar> {
    let mut bars = Vec::new();
    let mut price = 500i64;
    for i in 0..40 {
        bars.push(flat_bar(1_700_000_000 + i * 60, price));
        if i < 20 {
            price += 1;
        }
    }
    bars
}

/// A ~35% real collapse via actual fills -- fails the conservative
/// drawdown bar in the stress suite, matching PATCH B's own fragile
/// fixture pattern.
fn fragile_bars() -> Vec<BacktestBar> {
    vec![
        flat_bar(1_700_000_060, 500),
        flat_bar(1_700_000_120, 500),
        flat_bar(1_700_000_180, 150),
        flat_bar(1_700_000_240, 150),
    ]
}

/// Run a real engine, persist the FULL canonical evidence set (manifest,
/// audit, backtest_report.json, stress_suite.json, robustness_gauntlet.json)
/// into a fresh `<root>/<run_id>/` directory.
fn run_and_persist_full(
    label: &str,
    strategy_name: &'static str,
    bars: Vec<BacktestBar>,
    qty: i64,
    sell_at_idx: u64,
    research_trial_id: &str,
    research_economic_eval_id: &str,
) -> (BacktestReport, PathBuf) {
    let config = cfg_with_wide_cap();
    let initial_cash = config.initial_cash_micros;

    let mut engine = BacktestEngine::new(config.clone());
    engine
        .add_strategy(Box::new(BuyHoldSell { name: strategy_name, bar_idx: 0, qty, sell_at_idx }))
        .unwrap();
    let report = engine.run(&bars).expect("engine.run must succeed");

    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("mqk_p10_{}_{}_{}", label, std::process::id(), seq));
    let _ = fs::remove_dir_all(&root);

    let config_hash = report.config_id.to_string();
    let init_result = mqk_artifacts::init_run_artifacts(mqk_artifacts::InitRunArtifactsArgs {
        exports_root: &root,
        schema_version: 1,
        run_id: report.run_id,
        strategy_name: &report.strategy_name,
        engine_id: "mqk-backtest",
        mode: "backtest",
        timeframe: None,
        timeframe_secs: Some(60),
        git_hash: "p10_test_git_hash",
        config_hash: &config_hash,
        host_fingerprint: "p10_test_host",
        now_utc: chrono::Utc::now(),
    })
    .expect("init_run_artifacts must succeed");

    mqk_artifacts::write_backtest_report(&init_result.run_dir, &report, initial_cash)
        .expect("write_backtest_report must succeed");

    let stress_output = run_backtest_stress_suite(&report, &config, &bars, || {
        Box::new(BuyHoldSell { name: strategy_name, bar_idx: 0, qty, sell_at_idx })
    });
    mqk_artifacts::write_canonical_stress_suite(&init_result.run_dir, &stress_output)
        .expect("write_canonical_stress_suite must succeed");

    // CANONICAL-ROBUSTNESS-PROMOTION-GATE-01: merge a test-fabricated
    // dsr_pbo_sensitivity outcome before writing -- real cross-language
    // wiring is proven separately (mqk-backtest's own
    // scenario_dsr_pbo_sensitivity_01.rs + research-py's
    // test_dsr_pbo_sensitivity_cli.py); this P10 chain test only needs a
    // genuinely COMPLETE P9 artifact so the promotion-level gate this
    // patch adds does not itself become the reason the chain fails.
    let gauntlet_output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(BuyHoldSell { name: strategy_name, bar_idx: 0, qty, sell_at_idx })
    })
    .merge_dsr_pbo_sensitivity(mqk_backtest::RobustnessScenarioOutcome {
        name: mqk_backtest::DSR_PBO_SENSITIVITY_SCENARIO_NAME.to_string(),
        applicable: true,
        passed: true,
        reason: None,
        detail: "test-fabricated evaluated outcome".to_string(),
        research_trial_id: Some(research_trial_id.to_string()),
        evidence: None,
    })
    // P7A-P7B-ECONOMIC-REPLAY-STRESS-01: the real cross-language wiring is
    // proven separately (mqk-backtest's own
    // p7a_p7b_economic_replay_stress.rs unit tests + research-py's
    // p7a_p7b_economic_replay_stress_cli); this P10 chain test only needs a
    // genuinely COMPLETE P9 artifact, same rationale as dsr_pbo_sensitivity
    // above. FINAL-P7A-P7B-REPLAY-AUTHORITY-01 Section B: `evidence` carries
    // the `baseline_economic_eval_id` the new evaluator.rs gate requires to
    // match the P7C/OOS evidence's own `economic_eval_id`.
    .merge_dsr_pbo_sensitivity(mqk_backtest::RobustnessScenarioOutcome {
        name: mqk_backtest::P7A_P7B_ECONOMIC_REPLAY_STRESS_SCENARIO_NAME.to_string(),
        applicable: true,
        passed: true,
        reason: None,
        detail: "test-fabricated evaluated outcome".to_string(),
        research_trial_id: Some(research_trial_id.to_string()),
        evidence: Some(serde_json::json!({
            "baseline_economic_eval_id": research_economic_eval_id,
        })),
    });
    mqk_artifacts::write_canonical_robustness_gauntlet(&init_result.run_dir, &gauntlet_output)
        .expect("write_canonical_robustness_gauntlet must succeed");

    // Return the ARTIFACT ROOT (not the per-run directory) --
    // resolve_backtest_evidence joins run_id.to_string() itself.
    (report, root)
}

fn cleanup(root: &std::path::Path) {
    let _ = fs::remove_dir_all(root);
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Build a real, schema-accurate SQLite Research registry (mirroring
/// `research-py`'s actual `research_trials`/`research_attempts`/
/// `research_judge_artifacts` tables and `mqk-daemon::research_evidence_gate`'s
/// own test fixtures) for `strategy_id`, and return
/// `(registry_path, economic_walk_forward_json, economic_daily_returns_csv, judge_json, trial_id)`
/// ready for `verify_promotion_oos_evidence`.
#[allow(clippy::type_complexity)]
fn build_real_research_evidence(
    seed: &str,
    strategy_id: &str,
) -> (PathBuf, String, Vec<u8>, String, String) {
    let dir = std::env::temp_dir().join(format!(
        "mqk_p10_research_{}_{}",
        seed,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let trial_id = format!("trial_{seed}");
    let experiment_id = format!("exp_{seed}");
    let economic_eval_id = format!("econ_eval_{seed}");

    let daily_csv = b"date,net_daily_return\n2021-01-01,0.0010\n2021-01-02,0.0021\n".to_vec();
    let daily_sha = sha256_hex(&daily_csv);

    let economic_json = format!(
        r#"{{"protocol":{{"protocol_id":"economic_walk_forward_v1"}},"aggregate":{{"folds_used":3}},"holdout":{{"status":"reserved_not_evaluated"}},"execution_pricing":{{"pricing_model_id":"rust_conservative_bar_range_v1"}},"weight_to_share":{{"weight_to_share_protocol_id":"weight_to_share_v1"}},"outputs":{{"economic_daily_returns_csv":{{"sha256":"{daily_sha}"}}}},"ids":{{"economic_eval_id":"{economic_eval_id}"}},"folds":[{{"discrete_economics_protocol_id":"discrete_share_economic_path_v1"}}]}}"#
    );
    let economic_sha = sha256_hex(economic_json.as_bytes());

    let judge_json = format!(
        r#"{{"schema_version":"multiple_testing_judge_v1","protocol":{{"protocol_id":"research_multiple_testing_judge_v1"}},"comparison_scope":{{"experiment_id":"{experiment_id}"}},"judge_status":"evaluated","holdout":{{"status":"reserved_not_evaluated"}},"included_trial_ids":["{trial_id}"],"input_economic_result_ids":["{economic_eval_id}"],"input_artifacts":[{{"trial_id":"{trial_id}","economic_walk_forward_json_sha256":"{economic_sha}","economic_daily_returns_csv_sha256":"{daily_sha}"}}],"dsr_results":[{{"trial_id":"{trial_id}","evaluable":true,"deflated_sharpe_ratio":0.85}}],"pbo_result":{{"status":"evaluated","pbo":0.15}}}}"#
    );
    let judge_sha = sha256_hex(judge_json.as_bytes());

    let registry_path = dir.join("registry.sqlite3");
    let conn = rusqlite::Connection::open(&registry_path).expect("open registry db");
    conn.execute_batch(
        "
        create table research_trials (
            trial_id text primary key,
            experiment_id text not null,
            hypothesis_id text not null,
            strategy_id text not null
        );
        create table research_attempts (
            attempt_id text primary key,
            trial_id text not null,
            status text not null,
            result_id text
        );
        create table research_judge_artifacts (
            judge_artifact_sha256 text primary key,
            judge_id text not null,
            experiment_id text not null,
            hypothesis_id text,
            canonical_judge_json text
        );
        ",
    )
    .expect("create registry schema");
    conn.execute(
        "insert into research_trials (trial_id, experiment_id, hypothesis_id, strategy_id) \
         values (?1, ?2, ?3, ?4)",
        rusqlite::params![trial_id, experiment_id, format!("hyp_{seed}"), strategy_id],
    )
    .unwrap();
    conn.execute(
        "insert into research_attempts (attempt_id, trial_id, status, result_id) \
         values (?1, ?2, 'succeeded', ?3)",
        rusqlite::params![format!("{trial_id}:att0001"), trial_id, economic_eval_id],
    )
    .unwrap();
    conn.execute(
        "insert into research_judge_artifacts \
         (judge_artifact_sha256, judge_id, experiment_id, hypothesis_id, canonical_judge_json) \
         values (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![judge_sha, format!("judge_{seed}"), experiment_id, Option::<String>::None, judge_json],
    )
    .unwrap();
    drop(conn);

    (registry_path, economic_json, daily_csv, judge_json, trial_id)
}

fn lenient_promotion_config() -> PromotionConfig {
    PromotionConfig {
        min_sharpe: 0.0,
        max_mdd: 0.50,
        min_cagr: 0.0,
        min_profit_factor: 0.0,
        min_profitable_months_pct: 0.0,
        min_deflated_sharpe_ratio: 0.5,
        max_probability_backtest_overfitting: 0.5,
    }
}

// ---------------------------------------------------------------------------
// 1. Full positive chain
// ---------------------------------------------------------------------------

#[test]
fn p10a_full_chain_real_evidence_passes_canonical_evaluate_promotion() {
    let (report, root) =
        run_and_persist_full(
            "full_chain",
            "P10FullChain",
            profitable_bars(),
            5,
            25,
            "trial_full_chain",
            "econ_eval_full_chain",
        );

    // Backtest evidence (report + artifact_lock + stress_suite), resolved
    // by the exact candidate-bound seam the production route uses.
    let bundle = resolve_backtest_evidence(&root, report.run_id).expect("must resolve");
    assert!(bundle.stress_suite.passed, "profitable/non-fragile candidate must pass its own stress suite");

    // Robustness gauntlet: proves it is a genuinely resolvable, audit-
    // consumable part of the chain (P9) -- CANONICAL-ROBUSTNESS-PROMOTION-
    // GATE-01 wires it as a hard evaluate_promotion gate below via
    // bundle.robustness_evidence.
    let candidate_dir = root.join(report.run_id.to_string());
    let gauntlet = mqk_artifacts::load_canonical_robustness_gauntlet(&candidate_dir)
        .expect("robustness gauntlet must be resolvable as part of the evidence chain");
    assert_eq!(
        gauntlet.scenarios_run(),
        8,
        "6 pure scenarios + merged dsr_pbo_sensitivity + merged p7a_p7b_economic_replay_stress"
    );
    assert!(bundle.robustness_evidence.is_complete);
    assert!(bundle.robustness_evidence.all_applicable_passed);

    // Research OOS evidence: a real SQLite registry, strategy_id matching
    // this SAME candidate's report.strategy_name -- the cross-candidate
    // binding mqk-daemon's production gates independently enforce.
    let (registry_path, econ_json, daily_csv, judge_json, trial_id) =
        build_real_research_evidence("full_chain", &report.strategy_name);
    let oos_evidence = verify_promotion_oos_evidence(&registry_path, &trial_id, &econ_json, &daily_csv, &judge_json)
        .expect("real, registry-anchored Research evidence must verify");
    assert_eq!(oos_evidence.strategy_id(), report.strategy_name);

    // Canonical decision: exactly the fields PromotionInput/evaluate_promotion
    // define -- no parallel/partial policy re-implementation.
    let input = PromotionInput {
        initial_equity_micros: bundle.initial_equity_micros,
        report: bundle.report,
        stress_suite: Some(bundle.stress_suite),
        artifact_lock: Some(bundle.artifact_lock),
        oos_evidence: Some(oos_evidence),
        robustness_evidence: Some(bundle.robustness_evidence),
    };
    let decision = evaluate_promotion(&lenient_promotion_config(), &input);
    assert!(
        decision.passed,
        "a legitimate, fully-evidenced candidate must pass: {:?}",
        decision.fail_reasons
    );

    cleanup(&root);
}

// ---------------------------------------------------------------------------
// 2. Cross-candidate identity is genuinely distinguishable
// ---------------------------------------------------------------------------

#[test]
fn p10b_cross_candidate_identity_is_distinguishable() {
    // The exact identity check mqk-daemon::backtest_evidence_gate /
    // research_evidence_gate independently perform in production
    // (PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE) -- proven here
    // against REAL resolved data, not a mock.
    let (report, root) = run_and_persist_full(
        "cross_candidate",
        "P10CrossCandidate",
        profitable_bars(),
        5,
        25,
        "trial_cross_candidate",
        "econ_eval_cross_candidate",
    );
    let bundle = resolve_backtest_evidence(&root, report.run_id).expect("must resolve");

    // Research evidence registered under a DIFFERENT strategy_id.
    let (registry_path, econ_json, daily_csv, judge_json, trial_id) =
        build_real_research_evidence("cross_candidate", "SomeUnrelatedStrategy");
    let oos_evidence = verify_promotion_oos_evidence(&registry_path, &trial_id, &econ_json, &daily_csv, &judge_json)
        .expect("evidence is internally valid on its own");

    assert_ne!(
        oos_evidence.strategy_id(),
        bundle.report.strategy_name,
        "a real production route must reject this pairing as cross-candidate evidence"
    );

    cleanup(&root);
}

// ---------------------------------------------------------------------------
// 3. Valid Research evidence does not override a real failed stress suite
// ---------------------------------------------------------------------------

#[test]
fn p10c_valid_research_evidence_does_not_override_failed_stress_suite() {
    let (report, root) = run_and_persist_full(
        "fragile_with_research",
        "P10FragileWithResearch",
        fragile_bars(),
        100,
        3,
        "trial_fragile_with_research",
        "econ_eval_fragile_with_research",
    );
    let bundle = resolve_backtest_evidence(&root, report.run_id).expect("must resolve");
    assert!(!bundle.stress_suite.passed, "fixture precondition: this candidate's real stress suite must fail");

    let (registry_path, econ_json, daily_csv, judge_json, trial_id) =
        build_real_research_evidence("fragile_with_research", &report.strategy_name);
    let oos_evidence = verify_promotion_oos_evidence(&registry_path, &trial_id, &econ_json, &daily_csv, &judge_json)
        .expect("Research evidence itself is genuinely valid");

    let input = PromotionInput {
        initial_equity_micros: bundle.initial_equity_micros,
        report: bundle.report,
        stress_suite: Some(bundle.stress_suite),
        artifact_lock: Some(bundle.artifact_lock),
        oos_evidence: Some(oos_evidence),
        robustness_evidence: Some(bundle.robustness_evidence),
    };
    let decision = evaluate_promotion(&lenient_promotion_config(), &input);
    assert!(
        !decision.passed,
        "genuinely valid Research evidence must NOT compensate for a real failed stress suite"
    );

    cleanup(&root);
}

// ---------------------------------------------------------------------------
// 4. PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01: same strategy_id,
//    DIFFERENT Research trial for P9 vs P7C must be rejected
// ---------------------------------------------------------------------------

/// Confirmed defect this patch closes: the production finalizer only ever
/// verified `Research trial strategy_id == BacktestReport.strategy_name` --
/// never that the SAME trial produced both the P7C/OOS evidence and the P9
/// DSR/PBO sensitivity evidence. Trial A and Trial B here are both genuine,
/// independently valid, registry-anchored Research trials registered under
/// the SAME `strategy_id` -- proving the defect is not "an invalid trial
/// slipped through" but "two otherwise-legitimate trials under the same
/// strategy were silently mixed".
#[test]
fn p10d_same_strategy_different_trial_for_p9_vs_p7c_is_rejected() {
    let strategy_id = "P10TrialBindingSharedStrategy";

    // P9 dsr_pbo_sensitivity evidence bound to Trial B.
    let (report, root) = run_and_persist_full(
        "trial_binding",
        strategy_id,
        profitable_bars(),
        5,
        25,
        "trial_trial_binding_b",
        "econ_eval_trial_binding_b",
    );
    let bundle = resolve_backtest_evidence(&root, report.run_id).expect("must resolve");
    assert!(bundle.robustness_evidence.is_complete);
    assert!(bundle.robustness_evidence.all_applicable_passed);
    assert_eq!(
        bundle.robustness_evidence.dsr_pbo_sensitivity_research_trial_id.as_deref(),
        Some("trial_trial_binding_b")
    );

    // Trial A supplies the P7C/OOS evidence -- a real, independently valid,
    // registry-anchored Research trial under the SAME strategy_id as Trial B.
    let (registry_path_a, econ_a, daily_a, judge_a, trial_a_id) =
        build_real_research_evidence("trial_binding_a", strategy_id);
    let oos_evidence =
        verify_promotion_oos_evidence(&registry_path_a, &trial_a_id, &econ_a, &daily_a, &judge_a)
            .expect("Trial A's evidence is genuinely, independently valid");
    assert_eq!(oos_evidence.strategy_id(), strategy_id);

    // Trial B is ALSO real and independently registered under the SAME
    // strategy_id -- proving Trial B is not "invalid", merely not the SAME
    // trial that produced the P7C evidence above.
    let (registry_path_b, econ_b, daily_b, judge_b, trial_b_id) =
        build_real_research_evidence("trial_binding_b", strategy_id);
    let trial_b_evidence =
        verify_promotion_oos_evidence(&registry_path_b, &trial_b_id, &econ_b, &daily_b, &judge_b)
            .expect("Trial B's evidence is also genuinely, independently valid");
    assert_eq!(trial_b_evidence.strategy_id(), strategy_id);
    assert_ne!(trial_a_id, trial_b_id);

    let input = PromotionInput {
        initial_equity_micros: bundle.initial_equity_micros,
        report: bundle.report,
        stress_suite: Some(bundle.stress_suite),
        artifact_lock: Some(bundle.artifact_lock),
        oos_evidence: Some(oos_evidence),
        robustness_evidence: Some(bundle.robustness_evidence),
    };
    let decision = evaluate_promotion(&lenient_promotion_config(), &input);
    assert!(
        !decision.passed,
        "P9 evidence from a DIFFERENT Research trial than P7C, even under the SAME \
         strategy_id, must never satisfy promotion: {:?}",
        decision.fail_reasons
    );
    let reasons = decision.fail_reasons.join("; ");
    assert!(
        reasons.contains("Research trial binding mismatch"),
        "got: {reasons}"
    );

    cleanup(&root);
}

/// Positive control for the same gate: Trial A supplies BOTH the P7C
/// evidence AND the P9 sensitivity evidence -- promotion must not be blocked
/// by the trial-binding gate.
#[test]
fn p10e_same_trial_for_p9_and_p7c_is_accepted() {
    let strategy_id = "P10TrialBindingSameTrialStrategy";

    let (report, root) = run_and_persist_full(
        "trial_binding_same",
        strategy_id,
        profitable_bars(),
        5,
        25,
        "trial_trial_binding_same",
        "econ_eval_trial_binding_same",
    );
    let bundle = resolve_backtest_evidence(&root, report.run_id).expect("must resolve");

    let (registry_path, econ_json, daily_csv, judge_json, trial_id) =
        build_real_research_evidence("trial_binding_same", strategy_id);
    assert_eq!(trial_id, "trial_trial_binding_same");
    let oos_evidence =
        verify_promotion_oos_evidence(&registry_path, &trial_id, &econ_json, &daily_csv, &judge_json)
            .expect("evidence must be genuinely valid");

    let input = PromotionInput {
        initial_equity_micros: bundle.initial_equity_micros,
        report: bundle.report,
        stress_suite: Some(bundle.stress_suite),
        artifact_lock: Some(bundle.artifact_lock),
        oos_evidence: Some(oos_evidence),
        robustness_evidence: Some(bundle.robustness_evidence),
    };
    let decision = evaluate_promotion(&lenient_promotion_config(), &input);
    assert!(
        decision.passed,
        "the SAME trial supplying both P7C and P9 evidence must satisfy the trial-binding \
         gate: {:?}",
        decision.fail_reasons
    );

    cleanup(&root);
}
