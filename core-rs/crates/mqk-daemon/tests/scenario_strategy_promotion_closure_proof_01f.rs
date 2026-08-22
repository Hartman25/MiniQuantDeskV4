//! STRATEGY-PROMOTION-REGISTRY-01F: bounded end-to-end closure proof.
//!
//! One consolidated proof, run against the isolated local test DB, that
//! walks the full operator-facing lifecycle through the real daemon router
//! (in-process `axum`/`tower` requests via `.oneshot()` — no real daemon
//! process is started, no broker/provider/network call is made anywhere in
//! this file):
//!
//! 1. Register a test strategy in `sys_strategy_registry`.
//! 2. Write a committed-shape `paper_candidate` review-artifact fixture
//!    (same `write_review_artifacts` function the CLI uses).
//! 3. Transition no-state -> shadow_approved -> paper_approved ->
//!    active_paper, each via the real `POST /api/v1/strategy/promotions/
//!    transition` route (evidence independently validated by the route,
//!    never trusted from the request).
//! 4. Prove current-state and history readback (`GET .../check`,
//!    `GET .../history`) after every transition.
//! 5. Prove no paper outbox row can be created before `active_paper`.
//! 6. Prove exactly one synthetic outbox row is created once
//!    `active_paper` is reached and every other existing gate
//!    (registered+enabled, armed, active run) is satisfied.
//! 7. Transition `active_paper -> demoted`.
//! 8. Prove a new decision (different `decision_id`) is refused after
//!    demotion.
//! 9. Clean up only this test's own rows.
//!
//! Requires `MQK_DATABASE_URL` and is marked `#[ignore]`. Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_strategy_promotion_closure_proof_01f \
//!     -- --include-ignored --nocapture

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_backtest::{
    write_review_artifacts, ReviewManifest, ReviewRunOutput, ReviewSummary,
    StrategyScanReviewDecision, StrategyScanReviewState,
};
use mqk_daemon::{
    decision::{submit_internal_strategy_decision, InternalStrategyDecision},
    routes, state,
};
use tower::ServiceExt;
use uuid::Uuid;

const TIMEFRAME_SECS: i64 = 86400;
const TRANSITION_ROUTE: &str = "/api/v1/strategy/promotions/transition";

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_strategy_promotion_closure_proof_01f \
             -- --include-ignored --nocapture"
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

/// Writes a committed-shape `paper_candidate` review artifact fixture via
/// the exact same `write_review_artifacts` function the CLI's
/// `mqk backtest review-scan` command calls — this is a real artifact
/// directory, not a hand-rolled JSON blob.
fn write_paper_candidate_fixture(
    out_dir: &std::path::Path,
    strategy_id: &str,
    symbol: &str,
) -> PathBuf {
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
    let review_id = Uuid::new_v4();
    let manifest = ReviewManifest {
        schema_version: 1,
        review_id: review_id.to_string(),
        scanner_scan_id: "closure-proof-scan".to_string(),
        source_artifact_dir: "fixture-source-not-on-disk".to_string(),
        created_at_utc: Utc::now().to_rfc3339(),
        git_hash: "closure-proof-fixture".to_string(),
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
        scanner_scan_id: "closure-proof-scan".to_string(),
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
    write_review_artifacts(out_dir, &output).expect("write closure-proof fixture")
}

/// PROMOTION-WALKFORWARD-GATE-WIRING-01: a fully self-consistent,
/// registry-anchored P7C evidence bundle for `strategy_id`, written under
/// `root`. Mirrors `mqk-promotion/tests/common`'s exact artifact shapes
/// (also duplicated, for the same "no cross-crate test visibility" reason,
/// in `scenario_strategy_promotion_routes_01.rs`).
struct ResearchEvidenceFixture {
    trial_id: String,
    registry_db_path: PathBuf,
    evidence_dir: PathBuf,
    judge_path: PathBuf,
}

fn write_research_evidence_fixture(root: &std::path::Path, seed: &str) -> ResearchEvidenceFixture {
    use sha2::{Digest, Sha256};

    let trial_id = format!("trial_{seed}");
    let experiment_id = format!("exp_{seed}");
    let economic_eval_id = format!("econ_eval_{seed}");

    let evidence_dir = root.join(format!("research_evidence_{seed}"));
    std::fs::create_dir_all(&evidence_dir).expect("create research evidence dir");

    let daily_csv = b"date,net_daily_return\n2021-01-01,0.0010\n2021-01-02,0.0021\n".to_vec();
    let daily_sha = hex::encode(Sha256::digest(&daily_csv));
    std::fs::write(evidence_dir.join("economic_daily_returns.csv"), &daily_csv)
        .expect("write daily returns csv");

    let economic_json = format!(
        r#"{{"protocol":{{"protocol_id":"economic_walk_forward_v1"}},"aggregate":{{"folds_used":3}},"holdout":{{"status":"reserved_not_evaluated"}},"execution_pricing":{{"pricing_model_id":"rust_conservative_bar_range_v1"}},"weight_to_share":{{"weight_to_share_protocol_id":"weight_to_share_v1"}},"outputs":{{"economic_daily_returns_csv":{{"sha256":"{daily_sha}"}}}},"ids":{{"economic_eval_id":"{economic_eval_id}"}},"folds":[{{"discrete_economics_protocol_id":"discrete_share_economic_path_v1"}}]}}"#
    );
    let economic_sha = hex::encode(Sha256::digest(economic_json.as_bytes()));
    std::fs::write(evidence_dir.join("economic_walk_forward.json"), &economic_json)
        .expect("write economic artifact");

    let judge_json = format!(
        r#"{{"schema_version":"multiple_testing_judge_v1","protocol":{{"protocol_id":"research_multiple_testing_judge_v1"}},"comparison_scope":{{"experiment_id":"{experiment_id}"}},"judge_status":"evaluated","holdout":{{"status":"reserved_not_evaluated"}},"included_trial_ids":["{trial_id}"],"input_economic_result_ids":["{economic_eval_id}"],"input_artifacts":[{{"trial_id":"{trial_id}","economic_walk_forward_json_sha256":"{economic_sha}","economic_daily_returns_csv_sha256":"{daily_sha}"}}],"dsr_results":[{{"trial_id":"{trial_id}","evaluable":true,"deflated_sharpe_ratio":0.85}}],"pbo_result":{{"status":"evaluated","pbo":0.15}}}}"#
    );
    let judge_sha = hex::encode(Sha256::digest(judge_json.as_bytes()));
    let judge_path = root.join(format!("judge_{seed}.json"));
    std::fs::write(&judge_path, &judge_json).expect("write judge artifact");

    // PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01 / REAL-RESEARCH-TO-
    // PROMOTION-E2E-01: registered via the REAL `ResearchResultStore`
    // production methods (research-py, real Python subprocess), never a
    // hand-rolled `rusqlite` schema -- see the identical fix and rationale
    // in `scenario_strategy_promotion_routes_01.rs`'s own copy of this
    // function (a hand-rolled minimal schema silently diverges from the
    // real registry schema `dsr_pbo_sensitivity_cli.py` reads for P9).
    let registry_db_path = root.join("research_registry.sqlite3");
    let hypothesis_id = format!("hyp_{seed}");
    let judge_id = format!("judge_{seed}");
    let script = format!(
        "from pathlib import Path\n\
         from mqk_research.exp_distributed.storage import ResearchResultStore\n\
         from mqk_research.ml.economic_walkforward import PROTOCOL_ID as ECON_PROTOCOL_ID\n\
         store = ResearchResultStore(Path({registry_db}))\n\
         store.register_hypothesis(hypothesis_id={hypothesis_id}, experiment_id={experiment_id})\n\
         store.register_trial(\n\
         \ttrial_id={trial_id}, experiment_id={experiment_id}, hypothesis_id={hypothesis_id},\n\
         \tstrategy_id={strategy_id}, protocol_id=ECON_PROTOCOL_ID, identity={{'minimal': True}},\n\
         )\n\
         attempt_id, _ = store.begin_attempt(trial_id={trial_id}, origin='closure_proof_it_fixture')\n\
         store.finalize_attempt(\n\
         \tattempt_id, status='succeeded', result_id={economic_eval_id},\n\
         \tartifact_paths={{'economic_walk_forward': {economic_path}}},\n\
         \tresult_summary={{'folds_used': 3}},\n\
         )\n\
         judge_json = Path({judge_path}).read_text(encoding='utf-8')\n\
         store.register_judge_artifact(\n\
         \tjudge_id={judge_id}, experiment_id={experiment_id}, hypothesis_id=None,\n\
         \tartifact_path={judge_path}, judge_artifact_sha256={judge_sha},\n\
         \tcanonical_judge_json=judge_json, schema_version='multiple_testing_judge_v1',\n\
         \tprotocol_id='research_multiple_testing_judge_v1',\n\
         )\n",
        registry_db = py_str_literal(&registry_db_path.display().to_string()),
        hypothesis_id = py_str_literal(&hypothesis_id),
        experiment_id = py_str_literal(&experiment_id),
        trial_id = py_str_literal(&trial_id),
        strategy_id = py_str_literal(seed),
        economic_eval_id = py_str_literal(&economic_eval_id),
        economic_path = py_str_literal(
            &evidence_dir.join("economic_walk_forward.json").display().to_string()
        ),
        judge_path = py_str_literal(&judge_path.display().to_string()),
        judge_id = py_str_literal(&judge_id),
        judge_sha = py_str_literal(&judge_sha),
    );
    let src_dir = research_py_root().join("src");
    let output = std::process::Command::new("python")
        .env("PYTHONPATH", &src_dir)
        .arg("-c")
        .arg(&script)
        .output()
        .expect("failed to spawn python real-registry fixture-builder");
    assert!(
        output.status.success(),
        "real-registry fixture-builder script failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    ResearchEvidenceFixture {
        trial_id,
        registry_db_path,
        evidence_dir,
        judge_path,
    }
}

// ---------------------------------------------------------------------------
// PRODUCTION-PROMOTION-DB-E2E-01: real Backtest-side promotion evidence, via
// the SAME production functions mqk-cli's `backtest csv` +
// `finalize-robustness-sensitivity` call -- duplicated here for the same
// "no cross-crate test visibility" reason `write_research_evidence_fixture`
// above already is (this file is an independent test binary).
// ---------------------------------------------------------------------------

/// 182 daily bars, ~0.35%/day compounding growth: manually verified (see
/// BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01's own commit, and
/// `scenario_strategy_promotion_routes_01.rs`'s identical fixture) to make
/// `swing_momentum` genuinely trade AND clear every real P9 scenario.
fn smooth_uptrend_bars(symbol: &str) -> Vec<mqk_backtest::BacktestBar> {
    let m: i64 = 1_000_000;
    let start: i64 = 1_704_229_200; // 2024-01-02T21:00:00Z
    let mut price = 500.0_f64;
    let mut bars = Vec::with_capacity(182);
    for i in 0..182i64 {
        let ts = start + i * 86_400;
        price *= 1.0035;
        let o = (price * m as f64) as i64;
        let h = (price * 1.005 * m as f64) as i64;
        let l = (price * 0.995 * m as f64) as i64;
        let c = (price * m as f64) as i64;
        bars.push(mqk_backtest::BacktestBar::new(symbol, ts, o, h, l, c, 10_000));
    }
    bars
}

fn research_py_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("research-py")
}

fn py_str_literal(s: &str) -> String {
    serde_json::to_string(s).expect("string always serializes")
}

/// Run a REAL `BacktestEngine`, write the REAL, genuinely complete evidence
/// set (manifest, audit, `backtest_report.json`, `stress_suite.json`,
/// `robustness_gauntlet.json` -- finalized with a REAL DSR/PBO sensitivity
/// result via a real Python subprocess against a real disposable SQLite
/// registry). Returns the candidate's `run_id`. The engine always runs the
/// real `swing_momentum` plugin, so `report.strategy_name ==
/// "swing_momentum"` regardless of `dsr_trial_seed` -- callers that need
/// Gate 4c/4d to pass must use `strategy_id = "swing_momentum"` for the
/// promotion identity itself (see the test below).
fn write_real_backtest_evidence(
    artifact_root: &std::path::Path,
    research_trial_id: &str,
    research_registry_db: &std::path::Path,
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
        git_hash: "closure_proof_it_git_hash",
        config_hash: &config_hash,
        host_fingerprint: "closure_proof_it_host",
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
        &[8, 10],
        0.25, // test-fixture-only threshold; not asserted as accepted policy
        0.25,
    );
    mqk_artifacts::finalize_canonical_robustness_gauntlet_with_sensitivity(
        &init_result.run_dir,
        &sensitivity,
    )
    .expect("finalize_canonical_robustness_gauntlet_with_sensitivity");

    // P7A-P7B-ECONOMIC-REPLAY-STRESS-01: SAME trial/registry as
    // dsr_pbo_sensitivity above -- see the identical rationale in
    // `scenario_strategy_promotion_routes_01.rs`'s own copy of this
    // function (this fixture's hand-registered economic_walk_forward.json
    // has no recorded `inputs`, so this genuinely reports
    // `applicable: false`, fast, and does not block `is_complete()`).
    let stress = mqk_backtest::p7a_p7b_economic_replay_stress_scenario(
        "python",
        &research_py_root(),
        research_registry_db,
        research_trial_id,
        &report.strategy_name,
        &artifact_root.join(format!("p7a_p7b_stress_{}", report.run_id)),
        20,   // test-fixture-only stress knob; not asserted as accepted policy
        50,   // test-fixture-only stress knob; not asserted as accepted policy
        None, // test-fixture-only stress knob; not asserted as accepted policy
        None, // test-fixture-only stress knob; not asserted as accepted policy
        0.30, // test-fixture-only threshold; not asserted as accepted policy
    );
    mqk_artifacts::finalize_canonical_robustness_gauntlet_with_sensitivity(
        &init_result.run_dir,
        &stress,
    )
    .expect("finalize_canonical_robustness_gauntlet_with_sensitivity (p7a_p7b_economic_replay_stress)");

    report.run_id
}

async fn seed_registry(pool: &sqlx::PgPool, strategy_id: &str) {
    let ts = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: format!("Closure Proof Strategy {strategy_id}"),
            enabled: true,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("seed_registry failed");
}

async fn seed_active_run(st: &Arc<state::AppState>) -> Uuid {
    let pool = st.db.as_ref().expect("db configured");
    let run_id = Uuid::new_v4();
    let now = Utc::now();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({"source": "scenario_strategy_promotion_closure_proof_01f"}),
            host_fingerprint: "test-host".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::arm_run(pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(pool, run_id).await.expect("begin_run");
    mqk_db::heartbeat_run(pool, run_id, now)
        .await
        .expect("heartbeat_run");
    st.inject_running_loop_for_test(run_id).await;
    run_id
}

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
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

fn get_req(uri: &str) -> Request<axum::body::Body> {
    Request::builder()
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

fn make_decision(decision_id: &str, strategy_id: &str, symbol: &str) -> InternalStrategyDecision {
    InternalStrategyDecision {
        decision_id: decision_id.to_string(),
        strategy_id: strategy_id.to_string(),
        symbol: symbol.to_string(),
        timeframe_secs: TIMEFRAME_SECS,
        side: "buy".to_string(),
        qty: 10,
        order_type: "market".to_string(),
        time_in_force: "day".to_string(),
        limit_price: None,
    }
}

async fn outbox_row_count(pool: &sqlx::PgPool, decision_id: &str) -> i64 {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM oms_outbox WHERE idempotency_key = $1")
        .bind(decision_id)
        .fetch_one(pool)
        .await
        .expect("count outbox rows");
    row.0
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn closure_proof_full_lifecycle_through_real_routes() {
    let pool = make_db_pool().await;
    // Gate 4c/4d's backtest-evidence gate binds candidate identity via the
    // REAL engine's own `strategy_name` (always "swing_momentum" for the
    // real `swing_momentum` plugin -- see `write_real_backtest_evidence`),
    // so `strategy_id` here must be the real name, not a synthetic unique
    // id. The symbol is varied instead to keep this test's DB identity
    // (strategy_id, symbol, timeframe_secs) -- and therefore this test's own
    // repeated-run idempotency -- disjoint across runs.
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let root = std::env::temp_dir().join(format!("mqk_closure_proof_{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("create fixture root");
    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", &root);
    // PROMOTION-WALKFORWARD-GATE-WIRING-01: trusted config for the P7C
    // research-evidence gate (Gate 4c), required in addition to the
    // review-artifact evidence above for the shadow_approved transition
    // below.
    std::env::set_var(
        "MQK_RESEARCH_REGISTRY_DB",
        root.join("research_registry.sqlite3"),
    );
    std::env::set_var("MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT", &root);
    std::env::set_var("MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO", "0.0");
    std::env::set_var("MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING", "1.0");
    // PRODUCTION-PROMOTION-DB-E2E-01: trusted config for the Backtest-
    // evidence gate (Gate 4d) + the canonical `evaluate_promotion` metrics
    // policy, required in addition to the above for the same transition.
    std::env::set_var("MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT", &root);
    std::env::set_var("MQK_PROMOTION_MIN_SHARPE", "0.0");
    std::env::set_var("MQK_PROMOTION_MAX_MDD", "1.0");
    std::env::set_var("MQK_PROMOTION_MIN_CAGR", "0.0");
    std::env::set_var("MQK_PROMOTION_MIN_PROFIT_FACTOR", "0.0");
    std::env::set_var("MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT", "0.0");

    // --- Step 1: register the strategy. -----------------------------------
    seed_registry(&pool, &strategy_id).await;

    // --- Step 2: write a real paper_candidate review artifact fixture,
    // real Research OOS evidence, and a real Backtest evidence bundle
    // (genuine engine run + genuine, fully-complete P9 gauntlet). -----------
    let review_dir = write_paper_candidate_fixture(&root, &strategy_id, &symbol);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let backtest_run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
    );

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // --- Step 3a: no-state -> shadow_approved, via the real route. --------
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": strategy_id,
            "symbol": symbol,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "shadow_approved",
            "review_dir": review_dir.to_str().unwrap(),
            "research_trial_id": research.trial_id,
            "research_evidence_dir": research.evidence_dir.to_str().unwrap(),
            "research_judge_artifact_path": research.judge_path.to_str().unwrap(),
            "backtest_run_id": backtest_run_id.to_string(),
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "closure-proof-operator",
            "reason": "closure proof step 1",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "shadow_approved transition must succeed: {json}"
    );
    assert_eq!(json["disposition"], "transitioned");
    println!("[closure-proof] step 1: no-state -> shadow_approved: {json}");

    // Readback: current state + history after step 1.
    let check_uri = format!(
        "/api/v1/strategy/promotions/check?strategy_id={strategy_id}&symbol={symbol}&timeframe_secs={TIMEFRAME_SECS}"
    );
    let (_, check1) = call(routes::build_router(Arc::clone(&st)), get_req(&check_uri)).await;
    assert_eq!(check1["current_state"], "shadow_approved");
    assert_eq!(check1["tradable_paper"], false);
    assert_eq!(check1["tradable_live"], false);
    println!("[closure-proof] readback after step 1: {check1}");

    // --- Step 3b: shadow_approved -> paper_approved. -----------------------
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": strategy_id,
            "symbol": symbol,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "paper_approved",
            "review_dir": null,
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "closure-proof-operator",
            "reason": "closure proof step 2",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "paper_approved transition must succeed: {json}"
    );
    println!("[closure-proof] step 2: shadow_approved -> paper_approved: {json}");

    // --- Step 4a: no outbox row can be created before active_paper. -------
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");
    let _run_id = seed_active_run(&st).await;

    let pre_active_decision_id = unique_id("dec_pre_active");
    let pre_active_outcome = submit_internal_strategy_decision(
        &st,
        make_decision(&pre_active_decision_id, &strategy_id, &symbol),
    )
    .await;
    assert!(
        !pre_active_outcome.accepted,
        "paper_approved (not yet active) must never create an outbox row"
    );
    assert_eq!(pre_active_outcome.disposition, "promotion_not_active");
    assert_eq!(outbox_row_count(&pool, &pre_active_decision_id).await, 0);
    println!("[closure-proof] step 3: confirmed no outbox row before active_paper");

    // --- Step 3c: paper_approved -> active_paper. --------------------------
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": strategy_id,
            "symbol": symbol,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "active_paper",
            "review_dir": null,
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "closure-proof-operator",
            "reason": "closure proof step 4",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "active_paper transition must succeed: {json}"
    );
    println!("[closure-proof] step 4: paper_approved -> active_paper: {json}");

    let (_, check2) = call(routes::build_router(Arc::clone(&st)), get_req(&check_uri)).await;
    assert_eq!(check2["current_state"], "active_paper");
    assert_eq!(check2["tradable_paper"], true);
    assert_eq!(check2["tradable_live"], false);
    println!("[closure-proof] readback after step 4: {check2}");

    let history_uri = format!(
        "/api/v1/strategy/promotions/history?strategy_id={strategy_id}&symbol={symbol}&timeframe_secs={TIMEFRAME_SECS}"
    );
    let (_, history1) = call(routes::build_router(Arc::clone(&st)), get_req(&history_uri)).await;
    let history_rows = history1["rows"].as_array().expect("history rows array");
    assert_eq!(
        history_rows.len(),
        3,
        "history must show all 3 transitions so far"
    );
    println!(
        "[closure-proof] history after step 4 ({} rows): {history1}",
        history_rows.len()
    );

    // --- Step 5: exactly one synthetic outbox row after active_paper. -----
    let active_decision_id = unique_id("dec_active");
    let active_outcome =
        submit_internal_strategy_decision(
            &st,
            make_decision(&active_decision_id, &strategy_id, &symbol),
        )
        .await;
    assert!(
        active_outcome.accepted,
        "active_paper + every other gate satisfied must create an outbox row; disposition={:?} blockers={:?}",
        active_outcome.disposition, active_outcome.blockers
    );
    assert_eq!(outbox_row_count(&pool, &active_decision_id).await, 1);
    println!("[closure-proof] step 5: one synthetic outbox row created for decision_id={active_decision_id}");

    // --- Step 6: active_paper -> demoted. -----------------------------------
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": strategy_id,
            "symbol": symbol,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "demoted",
            "review_dir": null,
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "closure-proof-operator",
            "reason": "closure proof step 6",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "demoted transition must succeed: {json}"
    );
    println!("[closure-proof] step 6: active_paper -> demoted: {json}");

    // --- Step 7: demotion blocks a NEW decision. ---------------------------
    let post_demote_decision_id = unique_id("dec_post_demote");
    let post_demote_outcome = submit_internal_strategy_decision(
        &st,
        make_decision(&post_demote_decision_id, &strategy_id, &symbol),
    )
    .await;
    assert!(
        !post_demote_outcome.accepted,
        "demoted must block a new decision"
    );
    assert_eq!(post_demote_outcome.disposition, "promotion_demoted");
    assert_eq!(outbox_row_count(&pool, &post_demote_decision_id).await, 0);
    println!("[closure-proof] step 7: confirmed demotion blocks a new decision");

    // Final history readback: all 4 transitions remain visible.
    let (_, history2) = call(routes::build_router(Arc::clone(&st)), get_req(&history_uri)).await;
    let final_rows = history2["rows"].as_array().expect("history rows array");
    assert_eq!(
        final_rows.len(),
        4,
        "history must show all 4 transitions, newest first"
    );
    assert_eq!(final_rows[0]["new_state"], "demoted");
    println!(
        "[closure-proof] final history ({} rows): {history2}",
        final_rows.len()
    );

    // --- Cleanup: only this test's own rows. --------------------------------
    sqlx::query("DELETE FROM oms_outbox WHERE idempotency_key = $1")
        .bind(&active_decision_id)
        .execute(&pool)
        .await
        .expect("cleanup oms_outbox");
    sqlx::query("DELETE FROM sys_strategy_promotion_transitions WHERE strategy_id = $1")
        .bind(&strategy_id)
        .execute(&pool)
        .await
        .expect("cleanup sys_strategy_promotion_transitions");
    sqlx::query("DELETE FROM sys_strategy_registry WHERE strategy_id = $1")
        .bind(&strategy_id)
        .execute(&pool)
        .await
        .expect("cleanup sys_strategy_registry");
    sqlx::query("DELETE FROM runs WHERE run_id = $1")
        .bind(_run_id)
        .execute(&pool)
        .await
        .expect("cleanup runs");
    let _ = std::fs::remove_dir_all(&root);

    println!(
        "[closure-proof] cleanup complete; no broker/provider/network call was made at any step"
    );
}
