//! PROMOTION-EVIDENCE-SEMANTIC-BINDING-01 (R2): DB-backed proof that Gate 4d
//! refuses a fresh evidence-bearing promotion transition when the resolved
//! Backtest evidence's own durably-authenticated
//! `strategy_semantic_fingerprint` (captured directly from the exact boxed
//! `Strategy` instance the real `BacktestEngine` ran -- see
//! `mqk_backtest::BacktestReport::strategy_semantic_fingerprint`) does not
//! match the current server-resolved semantic configuration for the EXACT
//! identity being promoted -- even when `strategy_id` (the existing
//! cross-candidate check) agrees.
//!
//! `swing_momentum`'s semantic fingerprint is a pure function of `symbol`
//! (see `mqk_daemon::strategy_config_identity`'s own
//! `symbol_change_changes_the_resolved_fingerprint` test), so real backtest
//! evidence genuinely produced for one symbol, submitted against a promotion
//! request for a DIFFERENT symbol, is a genuine "evidence produced under a
//! different configuration" scenario -- not a synthetic fixture value. The
//! positive control (evidence config A + runtime config A -> eligible) is
//! already exhaustively proven by
//! `scenario_strategy_promotion_closure_proof_01f.rs`'s full lifecycle test;
//! this file proves only the negative side of the same binding.
//!
//! Requires `MQK_DATABASE_URL` and is marked `#[ignore]`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_promotion_evidence_semantic_binding_01 \
//!     -- --include-ignored --test-threads=1

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_backtest::{
    write_review_artifacts, ReviewManifest, ReviewRunOutput, ReviewSummary,
    StrategyScanReviewDecision, StrategyScanReviewState,
};
use mqk_daemon::strategy_config_identity::resolve_server_semantic_fingerprint;
use mqk_daemon::{routes, state};
use tower::ServiceExt;
use uuid::Uuid;

const STRATEGY_ID: &str = "swing_momentum";
const TIMEFRAME_SECS: i64 = 86_400;
const TRANSITION_ROUTE: &str = "/api/v1/strategy/promotions/transition";
/// The symbol the shared evidence bundle is genuinely built for.
const EVIDENCE_SYMBOL: &str = "AAPL";
/// A DIFFERENT symbol whose current runtime `swing_momentum` fingerprint is
/// therefore guaranteed to differ from `EVIDENCE_SYMBOL`'s.
const DRIFTED_SYMBOL: &str = "MSFT";

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_promotion_evidence_semantic_binding_01 \
             -- --include-ignored --test-threads=1"
        )
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to test DB");
    mqk_db::migrate(&pool).await.expect("run migrations");
    pool
}

// ---------------------------------------------------------------------------
// Real Research + Backtest evidence fixture -- duplicated verbatim from
// scenario_strategy_promotion_closure_proof_01f.rs (same "no cross-crate test
// visibility" reason already documented there; also mirrored in
// scenario_dynamic_selection_evidence_validation_01.rs's R0 repair).
// ---------------------------------------------------------------------------

struct ResearchEvidenceFixture {
    trial_id: String,
    economic_eval_id: String,
    registry_db_path: PathBuf,
    evidence_dir: PathBuf,
    judge_path: PathBuf,
    judge_artifact_sha256: String,
}

fn research_py_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("research-py")
}

fn smooth_uptrend_bars(symbol: &str) -> Vec<mqk_backtest::BacktestBar> {
    let m: i64 = 1_000_000;
    let start: i64 = 1_704_229_200; // 2024-01-02T21:00:00Z
    let mut price = 500.0_f64;
    let mut bars = Vec::with_capacity(240);
    for i in 0..240i64 {
        let ts = start + i * 86_400;
        let leg = (i / 30) % 3; // 0 = calm bull, 1 = wide-range bull, 2 = decline
        match leg {
            0 => price *= 1.0055,
            1 => price *= 1.0017,
            _ => price *= 0.9960,
        }
        let (hi_mult, lo_mult) = if leg == 1 { (1.09, 0.91) } else { (1.005, 0.995) };
        let o = (price * m as f64) as i64;
        let h = (price * hi_mult * m as f64) as i64;
        let l = (price * lo_mult * m as f64) as i64;
        let c = (price * m as f64) as i64;
        bars.push(mqk_backtest::BacktestBar::new(symbol, ts, o, h, l, c, 10_000));
    }
    bars
}

fn write_real_research_evidence_via_production_pipeline(
    root: &Path,
    seed: &str,
    strategy_id: &str,
    entry_thresholds: &[f64],
) -> Vec<ResearchEvidenceFixture> {
    let registry_db_path = root.join("research_registry.sqlite3");
    let run_root = root.join(format!("real_research_runs_{seed}"));
    let judge_path = root.join(format!("real_research_judge_{seed}.json"));
    let experiment_id = format!("exp.{seed}");
    let hypothesis_id = format!("hyp.{seed}");
    let thresholds_arg = entry_thresholds
        .iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let src_dir = research_py_root().join("src");
    let output = std::process::Command::new("python")
        .env("PYTHONPATH", &src_dir)
        .args([
            "-m",
            "mqk_research.ml.real_research_promotion_e2e_cli",
            "--registry-db",
            &registry_db_path.display().to_string(),
            "--run-root",
            &run_root.display().to_string(),
            "--experiment-id",
            &experiment_id,
            "--hypothesis-id",
            &hypothesis_id,
            "--strategy-id",
            strategy_id,
            "--entry-thresholds",
            &thresholds_arg,
            "--periods-days",
            "560",
            "--steps",
            "10",
            "--judge-out",
            &judge_path.display().to_string(),
        ])
        .output()
        .expect("failed to spawn real_research_promotion_e2e_cli");
    assert!(
        output.status.success(),
        "real_research_promotion_e2e_cli failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "real_research_promotion_e2e_cli produced unparseable stdout: {e}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(
        parsed["status"], "ok",
        "real_research_promotion_e2e_cli reported non-ok status: {parsed}"
    );
    assert_eq!(
        parsed["judge_status"], "evaluated",
        "fixture precondition: the real judge must be genuinely 'evaluated': {parsed}"
    );

    let judge_artifact_sha256 = parsed["judge_artifact_sha256"]
        .as_str()
        .expect("judge_artifact_sha256")
        .to_string();

    let trials = parsed["trials"].as_array().expect("trials array");
    assert_eq!(trials.len(), entry_thresholds.len());
    trials
        .iter()
        .map(|t| {
            assert_eq!(
                t["dsr_evaluable"], true,
                "fixture precondition: every real trial's DSR must be evaluable: {t}"
            );
            let trial_id = t["trial_id"].as_str().expect("trial_id").to_string();
            let economic_eval_id = t["economic_eval_id"].as_str().expect("economic_eval_id").to_string();
            let economic_json_path =
                PathBuf::from(t["economic_walk_forward_json"].as_str().expect("path"));
            let evidence_dir = economic_json_path
                .parent()
                .expect("economic_walk_forward_json has a parent dir")
                .to_path_buf();
            ResearchEvidenceFixture {
                trial_id,
                economic_eval_id,
                registry_db_path: registry_db_path.clone(),
                evidence_dir,
                judge_path: judge_path.clone(),
                judge_artifact_sha256: judge_artifact_sha256.clone(),
            }
        })
        .collect()
}

/// Real `BacktestEngine` run for `symbol`, with a genuinely complete P9
/// gauntlet -- see `scenario_strategy_promotion_closure_proof_01f.rs`'s
/// identical function for the full rationale. Always runs the real
/// `swing_momentum` plugin for `symbol`, so the resulting
/// `report.strategy_semantic_fingerprint` is the REAL, genuine fingerprint
/// for `(STRATEGY_ID, symbol, TIMEFRAME_SECS)` -- never a hand-computed
/// stand-in.
fn write_real_backtest_evidence(
    artifact_root: &Path,
    research_trial_id: &str,
    research_economic_eval_id: &str,
    research_registry_db: &Path,
    research_judge_artifact_sha256: &str,
    symbol: &str,
) -> Uuid {
    use mqk_strategy::engines::register_builtin_strategies_with_sizing;
    use mqk_strategy::PluginRegistry;

    let mut cfg = mqk_backtest::BacktestConfig::conservative_defaults();
    cfg.timeframe_secs = 86_400;
    cfg.integrity_stale_threshold_ticks = 200_000;
    let initial_cash = cfg.initial_cash_micros;

    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, symbol, 1, None, None)
        .expect("register_builtin_strategies_with_sizing");

    let mut engine = mqk_backtest::BacktestEngine::new(cfg.clone());
    engine
        .add_strategy(reg.instantiate("swing_momentum").expect("swing_momentum registered"))
        .expect("add_strategy");
    let bars = smooth_uptrend_bars(symbol);
    let report = engine.run(&bars).expect("engine.run");

    let config_hash = report.config_id.to_string();
    let init_result = mqk_artifacts::init_run_artifacts(mqk_artifacts::InitRunArtifactsArgs {
        exports_root: artifact_root,
        schema_version: 1,
        run_id: report.run_id,
        strategy_name: &report.strategy_name,
        engine_id: "mqk-backtest",
        mode: "backtest",
        timeframe: None,
        timeframe_secs: Some(86_400),
        git_hash: "sem_binding_it_git_hash",
        config_hash: &config_hash,
        host_fingerprint: "sem_binding_it_host",
        now_utc: Utc::now(),
    })
    .expect("init_run_artifacts");

    mqk_artifacts::write_backtest_report(&init_result.run_dir, &report, initial_cash)
        .expect("write_backtest_report");

    let stress_output = mqk_backtest::run_backtest_stress_suite(&report, &cfg, &bars, || {
        reg.instantiate("swing_momentum").expect("swing_momentum registered")
    });
    mqk_artifacts::write_canonical_stress_suite(&init_result.run_dir, &stress_output)
        .expect("write_canonical_stress_suite");

    let gauntlet_output = mqk_backtest::run_robustness_gauntlet(&report, &cfg, &bars, || {
        reg.instantiate("swing_momentum").expect("swing_momentum registered")
    });
    mqk_artifacts::write_canonical_robustness_gauntlet(&init_result.run_dir, &gauntlet_output)
        .expect("write_canonical_robustness_gauntlet");

    let sensitivity = mqk_backtest::dsr_pbo_sensitivity_scenario(
        "python",
        &research_py_root(),
        research_registry_db,
        research_trial_id,
        &report.strategy_name,
        research_judge_artifact_sha256,
        &[8, 10],
        0.25,
        0.25,
    );
    mqk_artifacts::finalize_canonical_robustness_gauntlet_with_sensitivity(
        &init_result.run_dir,
        &sensitivity,
    )
    .expect("finalize_canonical_robustness_gauntlet_with_sensitivity");

    let stress = mqk_backtest::p7a_p7b_economic_replay_stress_scenario(
        "python",
        &research_py_root(),
        research_registry_db,
        research_trial_id,
        research_economic_eval_id,
        &report.strategy_name,
        &artifact_root.join(format!("p7a_p7b_stress_{}", report.run_id)),
        20,
        50,
        Some(1000),
        None,
        0.30,
    );
    mqk_artifacts::finalize_canonical_robustness_gauntlet_with_sensitivity(
        &init_result.run_dir,
        &stress,
    )
    .expect("finalize_canonical_robustness_gauntlet_with_sensitivity (p7a_p7b_economic_replay_stress)");

    let placebo = mqk_backtest::genuine_shuffled_placebo_scenario(
        "python",
        &research_py_root(),
        research_registry_db,
        research_trial_id,
        research_economic_eval_id,
        &report.strategy_name,
        &artifact_root.join(format!("genuine_shuffled_placebo_{}", report.run_id)),
    );
    mqk_artifacts::finalize_canonical_robustness_gauntlet_with_sensitivity(
        &init_result.run_dir,
        &placebo,
    )
    .expect("finalize_canonical_robustness_gauntlet_with_sensitivity (genuine_shuffled_placebo)");

    report.run_id
}

/// Everything Gate 4c/4d needs, built ONCE per test binary (real Research
/// trial + real Backtest run are expensive) and reused by both tests here --
/// both submit the SAME evidence, targeting different promotion identities.
struct SharedEvidence {
    research_trial_id: String,
    research_evidence_dir: PathBuf,
    research_judge_artifact_path: PathBuf,
    backtest_run_id: Uuid,
    research_registry_db: PathBuf,
    evidence_root: PathBuf,
}

static SHARED_EVIDENCE: OnceLock<SharedEvidence> = OnceLock::new();

fn det_uuid(seed: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, seed.as_bytes())
}

fn shared_evidence() -> &'static SharedEvidence {
    SHARED_EVIDENCE.get_or_init(|| {
        let root = std::env::temp_dir().join(format!(
            "mqk_daemon_sem_binding_shared_fixture_{}",
            det_uuid("scenario_promotion_evidence_semantic_binding_01::shared_evidence_fixture")
        ));
        // Backtest run artifacts are content-hash-locked once written; this
        // deterministic path must start empty every process run so a prior
        // run's artifacts never collide with a fresh build (see the
        // identical note in scenario_dynamic_selection_evidence_validation_01.rs).
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create shared evidence fixture root");

        let research_trials = write_real_research_evidence_via_production_pipeline(
            &root,
            "real_e2e_positive",
            STRATEGY_ID,
            &[0.45, 0.55],
        );
        let research = &research_trials[0];
        let backtest_run_id = write_real_backtest_evidence(
            &root,
            &research.trial_id,
            &research.economic_eval_id,
            &research.registry_db_path,
            &research.judge_artifact_sha256,
            EVIDENCE_SYMBOL,
        );

        SharedEvidence {
            research_trial_id: research.trial_id.clone(),
            research_evidence_dir: research.evidence_dir.clone(),
            research_judge_artifact_path: research.judge_path.clone(),
            backtest_run_id,
            research_registry_db: research.registry_db_path.clone(),
            evidence_root: root,
        }
    })
}

fn write_paper_candidate_fixture(out_dir: &Path, strategy_id: &str, symbol: &str) -> PathBuf {
    let decision = StrategyScanReviewDecision {
        symbol: symbol.to_string(),
        timeframe: "1D".to_string(),
        strategy_id: strategy_id.to_string(),
        scanner_rank: Some(1),
        scanner_score: Some(9.5),
        review_state: StrategyScanReviewState::PaperCandidate,
        reason_codes: vec!["eligible_paper_candidate".to_string()],
        blockers: Vec::new(),
        warnings: Vec::new(),
    };
    let review_id = det_uuid(&format!(
        "scenario_promotion_evidence_semantic_binding_01::write_paper_candidate_fixture::{}",
        out_dir.display()
    ));
    let manifest = ReviewManifest {
        schema_version: 1,
        review_id: review_id.to_string(),
        scanner_scan_id: "sem-binding-scan".to_string(),
        source_artifact_dir: "fixture-source-not-on-disk".to_string(),
        created_at_utc: "2026-07-01T00:00:00Z".to_string(),
        git_hash: "test-git-hash".to_string(),
        policy_min_bars_used: 252,
        policy_min_trade_count: 5,
        policy_min_total_return_pct: 0.0,
        policy_min_alpha_pct: 0.0,
        policy_max_drawdown_pct: 25.0,
        policy_min_profit_factor: 1.05,
        candidate_count: 1,
        blocked_count: 0,
        needs_review_count: 0,
        watchlist_candidate_count: 0,
        paper_candidate_count: 1,
        rejected_count: 0,
        blockers: Vec::new(),
        warnings: Vec::new(),
    };
    let summary = ReviewSummary {
        scanner_scan_id: "sem-binding-scan".to_string(),
        review_id: review_id.to_string(),
        candidate_count: 1,
        blocked_count: 0,
        needs_review_count: 0,
        watchlist_candidate_count: 0,
        paper_candidate_count: 1,
        rejected_count: 0,
        top_paper_candidates: vec![decision.clone()],
        top_watchlist_candidates: Vec::new(),
        blockers: Vec::new(),
        warnings: Vec::new(),
    };
    let output = ReviewRunOutput {
        review_id,
        manifest,
        decisions: vec![decision],
        summary,
    };
    write_review_artifacts(out_dir, &output).expect("write review artifacts")
}

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON body");
    (status, json)
}

fn transition_req(body: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri(TRANSITION_ROUTE)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn row_count_for(pool: &sqlx::PgPool, symbol: &str) -> i64 {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM sys_strategy_promotion_transitions WHERE strategy_id = $1 AND symbol = $2",
    )
    .bind(STRATEGY_ID)
    .bind(symbol)
    .fetch_one(pool)
    .await
    .expect("count rows");
    row.0
}

/// THE load-bearing negative control: real evidence genuinely produced for
/// `EVIDENCE_SYMBOL`, submitted against a promotion request for
/// `DRIFTED_SYMBOL` -- same `strategy_id` (the existing cross-candidate check
/// trivially passes), but a materially different semantic configuration
/// (swing_momentum's fingerprint is keyed on symbol). Must be refused with
/// zero promotion transition rows, even though every other gate (review
/// artifact, Research OOS evidence, promotion policy thresholds) would
/// otherwise accept it.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn cross_symbol_evidence_is_rejected_despite_matching_strategy_id() {
    let pool = make_db_pool().await;
    let evidence = shared_evidence();

    let real_fp_drifted =
        resolve_server_semantic_fingerprint(STRATEGY_ID, DRIFTED_SYMBOL, TIMEFRAME_SECS)
            .expect("swing_momentum must resolve for DRIFTED_SYMBOL");
    let real_fp_evidence =
        resolve_server_semantic_fingerprint(STRATEGY_ID, EVIDENCE_SYMBOL, TIMEFRAME_SECS)
            .expect("swing_momentum must resolve for EVIDENCE_SYMBOL");
    assert_ne!(
        real_fp_drifted, real_fp_evidence,
        "sanity: swing_momentum's fingerprint must actually depend on symbol"
    );

    let review_root = std::env::temp_dir().join(format!(
        "mqk_daemon_sem_binding_review_{}",
        det_uuid("scenario_promotion_evidence_semantic_binding_01::cross_symbol_review_root")
    ));
    std::fs::create_dir_all(&review_root).expect("create review root");
    // The review artifact and Research evidence both genuinely claim
    // DRIFTED_SYMBOL / STRATEGY_ID -- only the BACKTEST evidence was produced
    // under a different (EVIDENCE_SYMBOL) semantic configuration, isolating
    // this test to Gate 4d alone.
    let review_dir = write_paper_candidate_fixture(&review_root, STRATEGY_ID, DRIFTED_SYMBOL);

    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", &review_root);
    std::env::set_var("MQK_RESEARCH_REGISTRY_DB", &evidence.research_registry_db);
    std::env::set_var("MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT", &evidence.evidence_root);
    std::env::set_var("MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO", "0.0");
    std::env::set_var("MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING", "1.0");
    std::env::set_var("MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT", &evidence.evidence_root);
    std::env::set_var("MQK_PROMOTION_MIN_SHARPE", "0.0");
    std::env::set_var("MQK_PROMOTION_MAX_MDD", "1.0");
    std::env::set_var("MQK_PROMOTION_MIN_CAGR", "0.0");
    std::env::set_var("MQK_PROMOTION_MIN_PROFIT_FACTOR", "0.0");
    std::env::set_var("MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT", "0.0");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let before = row_count_for(&pool, DRIFTED_SYMBOL).await;
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": STRATEGY_ID,
            "symbol": DRIFTED_SYMBOL,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "shadow_approved",
            "review_dir": review_dir.to_str().unwrap(),
            "research_trial_id": evidence.research_trial_id,
            "research_evidence_dir": evidence.research_evidence_dir.to_str().unwrap(),
            "research_judge_artifact_path": evidence.research_judge_artifact_path.to_str().unwrap(),
            "backtest_run_id": evidence.backtest_run_id.to_string(),
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "sem-binding-test",
            "reason": "cross-symbol evidence must be refused",
        })),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "evidence produced for a different symbol's semantic config must be refused: {json}"
    );
    assert_eq!(json["disposition"], "evidence_invalid");
    assert!(
        json["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b.as_str().unwrap_or_default().contains("strategy_semantic_fingerprint")),
        "expected a semantic-fingerprint blocker: {json}"
    );
    assert_eq!(
        row_count_for(&pool, DRIFTED_SYMBOL).await,
        before,
        "a rejected fresh-evidence transition must create no new row"
    );

    sqlx::query("DELETE FROM sys_strategy_promotion_transitions WHERE strategy_id = $1 AND symbol = $2")
        .bind(STRATEGY_ID)
        .bind(DRIFTED_SYMBOL)
        .execute(&pool)
        .await
        .ok();
}

/// Caller-forgery proof: `StrategyPromotionTransitionRequest` has no field
/// through which a caller could claim a `strategy_semantic_fingerprint` --
/// an extra, unrecognized JSON field is silently ignored by serde (same
/// empirical proof pattern as
/// `scenario_promotion_config_identity_01.rs::caller_supplied_fingerprint_field_is_ignored`).
/// The exact same cross-symbol mismatch as above must still be refused even
/// when the request body also forges a `strategy_semantic_fingerprint`
/// field claiming the (real) DRIFTED_SYMBOL value.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn caller_supplied_semantic_fingerprint_field_has_no_effect() {
    let pool = make_db_pool().await;
    let evidence = shared_evidence();

    let real_fp_drifted =
        resolve_server_semantic_fingerprint(STRATEGY_ID, DRIFTED_SYMBOL, TIMEFRAME_SECS)
            .expect("swing_momentum must resolve for DRIFTED_SYMBOL");

    let review_root = std::env::temp_dir().join(format!(
        "mqk_daemon_sem_binding_forged_review_{}",
        det_uuid("scenario_promotion_evidence_semantic_binding_01::forged_review_root")
    ));
    std::fs::create_dir_all(&review_root).expect("create review root");
    let review_dir = write_paper_candidate_fixture(&review_root, STRATEGY_ID, DRIFTED_SYMBOL);

    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", &review_root);
    std::env::set_var("MQK_RESEARCH_REGISTRY_DB", &evidence.research_registry_db);
    std::env::set_var("MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT", &evidence.evidence_root);
    std::env::set_var("MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO", "0.0");
    std::env::set_var("MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING", "1.0");
    std::env::set_var("MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT", &evidence.evidence_root);
    std::env::set_var("MQK_PROMOTION_MIN_SHARPE", "0.0");
    std::env::set_var("MQK_PROMOTION_MAX_MDD", "1.0");
    std::env::set_var("MQK_PROMOTION_MIN_CAGR", "0.0");
    std::env::set_var("MQK_PROMOTION_MIN_PROFIT_FACTOR", "0.0");
    std::env::set_var("MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT", "0.0");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let before = row_count_for(&pool, DRIFTED_SYMBOL).await;
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": STRATEGY_ID,
            "symbol": DRIFTED_SYMBOL,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "shadow_approved",
            "review_dir": review_dir.to_str().unwrap(),
            "research_trial_id": evidence.research_trial_id,
            "research_evidence_dir": evidence.research_evidence_dir.to_str().unwrap(),
            "research_judge_artifact_path": evidence.research_judge_artifact_path.to_str().unwrap(),
            "backtest_run_id": evidence.backtest_run_id.to_string(),
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "sem-binding-forgery-test",
            "reason": "forged fingerprint field must have no effect",
            // Not a real request field -- must be silently ignored.
            "strategy_semantic_fingerprint": real_fp_drifted,
        })),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a forged strategy_semantic_fingerprint field must not rescue a real config mismatch: {json}"
    );
    assert_eq!(json["disposition"], "evidence_invalid");
    assert_eq!(
        row_count_for(&pool, DRIFTED_SYMBOL).await,
        before,
        "a rejected fresh-evidence transition must create no new row"
    );

    sqlx::query("DELETE FROM sys_strategy_promotion_transitions WHERE strategy_id = $1 AND symbol = $2")
        .bind(STRATEGY_ID)
        .bind(DRIFTED_SYMBOL)
        .execute(&pool)
        .await
        .ok();
}
