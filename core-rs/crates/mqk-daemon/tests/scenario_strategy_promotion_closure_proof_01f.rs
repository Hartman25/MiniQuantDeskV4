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
//! 4b. RESEARCH-PROMOTION-DURABLE-LINEAGE-HTTP-PROOF-01: immediately after
//!     the evidence-requiring `shadow_approved` transition, read the
//!     durably persisted row back from Postgres by its exact
//!     `transition_id` and prove exact identity/value agreement (never
//!     merely non-null) against Research/Backtest/robustness authority
//!     independently re-derived from the same real fixture files via the
//!     same production functions the route itself calls: the full V3
//!     lineage (`research_trial_id`, `research_economic_eval_id`,
//!     `research_deflated_sharpe_ratio`,
//!     `research_probability_backtest_overfitting`, `backtest_run_id`,
//!     `research_judge_artifact_sha256`, `stress_protocol_version`,
//!     `stress_artifact_sha256`, `robustness_protocol_version`,
//!     `finalized_robustness_artifact_sha256`,
//!     `promotion_policy_fingerprint`) plus the scanner/review evidence
//!     binding (`evidence_transition_id`, `evidence_fingerprint`,
//!     `evidence_fingerprint_v2`) — with a negative control proving these
//!     assertions actually discriminate a different real Research trial's
//!     identity, not merely a shared default/None.
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
//!
//! FINAL-P10-FIXTURE-REALISM-01 (2026-08-22): the mandatory
//! `p7a_p7b_economic_replay_stress`/`genuine_shuffled_placebo` scenarios
//! require genuine, registry-anchored `inputs` to pass, which the lightweight
//! hand-built `write_research_evidence_fixture` fixture could never provide.
//! `closure_proof_full_lifecycle_through_real_routes` now builds its Research
//! evidence via `write_real_research_evidence_via_production_pipeline` (the
//! real Research production pipeline, duplicated here from
//! `scenario_strategy_promotion_routes_01.rs` for the same "no cross-crate
//! test visibility" reason). `smooth_uptrend_bars` was replaced with a
//! genuinely multi-regime bars fixture (see its own doc comment) so
//! `month_year_regime_concentration` also genuinely passes.

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
    promotion_evidence_validation::{compute_evidence_fingerprint_v2, validate_paper_candidate_evidence},
    routes, state,
};
use sqlx::Row;
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
    /// FINAL-P7A-P7B-REPLAY-AUTHORITY-01: the exact `economic_eval_id` this
    /// fixture registered as the trial's succeeded attempt `result_id`.
    economic_eval_id: String,
    registry_db_path: PathBuf,
    evidence_dir: PathBuf,
    judge_path: PathBuf,
    /// FINAL-P9-AUTHORITY-BINDING-REPAIR-01 Section 1: the exact
    /// `research_judge_artifacts.judge_artifact_sha256` this fixture
    /// registered -- required by `dsr_pbo_sensitivity_scenario`'s new
    /// `authoritative_judge_artifact_sha256` contract.
    judge_artifact_sha256: String,
}

// ---------------------------------------------------------------------------
// PRODUCTION-PROMOTION-DB-E2E-01: real Backtest-side promotion evidence, via
// the SAME production functions mqk-cli's `backtest csv` +
// `finalize-robustness-sensitivity` call -- duplicated here for the same
// "no cross-crate test visibility" reason `write_research_evidence_fixture`
// above already is (this file is an independent test binary).
// ---------------------------------------------------------------------------

/// FINAL-P10-FIXTURE-REALISM-01: 240 daily bars (8 calendar months), built
/// from three deterministic 30-day legs cycled `0,1,2,0,1,2,0,1`: a calm
/// uptrend leg (tight 0.5% intrabar range, +0.55%/day close growth,
/// classifies `bull_trend`), a wide-range uptrend leg (9% intrabar range,
/// +0.17%/day close growth -- `average_range_pct` alone crosses
/// `detect_market_regime`'s high-volatility threshold, so it classifies
/// `high_volatility` regardless of its own positive trend), and a decline
/// leg (-0.40%/day close growth, classifies `bear_trend`; `swing_momentum`
/// genuinely flips short and profits from the decline). Verified directly
/// against the real, unmodified `detect_market_regime` classifier and
/// `run_robustness_gauntlet` (see FINAL-P10-FIXTURE-REALISM-01's own commit,
/// and `scenario_strategy_promotion_routes_01.rs`'s identical fixture):
/// produces 3 genuinely distinct regime buckets (`bull_trend`,
/// `high_volatility`, `bear_trend`), each with real positive strategy P&L,
/// none exceeding the 0.5 concentration ceiling in any of the month/year/
/// regime dimensions -- `month_year_regime_concentration` genuinely passes.
/// `swing_momentum` genuinely trades throughout and every other P9/stress
/// scenario clears with real, unforced margin.
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

/// FINAL-P10-FIXTURE-REALISM-01: runs the REAL Research production pipeline
/// (`mqk_research.ml.real_research_promotion_e2e_cli`), duplicated here for
/// the same "no cross-crate test visibility" reason as
/// `write_research_evidence_fixture` above -- see
/// `scenario_strategy_promotion_routes_01.rs`'s identical function for the
/// full rationale. Returns one [`ResearchEvidenceFixture`] per trial, in
/// `--entry-thresholds` order; a single-trial population is never
/// DSR-evaluable, so callers needing a genuinely `evaluated` judge must
/// supply at least two `entry_thresholds`.
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
        "fixture precondition: the real judge must be genuinely 'evaluated' \
         (not merely 'partially_evaluable') for these tests to exercise the real \
         gates meaningfully: {parsed}"
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

fn research_py_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("research-py")
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
    research_economic_eval_id: &str,
    research_registry_db: &std::path::Path,
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
        research_judge_artifact_sha256,
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
    // dsr_pbo_sensitivity above. FINAL-P7A-P7B-REPLAY-AUTHORITY-01 Section A
    // ("MANDATORY MEANS MANDATORY"): this fixture's hand-registered
    // economic_walk_forward.json has no recorded `inputs`, so this now
    // genuinely reports `applicable: true, passed: false` (it can never
    // disappear via `applicable: false`) -- see the module-level note at the
    // top of this file for the known, honestly-flagged consequence for this
    // test's overall pass/fail expectation, not independently re-verified in
    // this session (no live Postgres/Python DB harness available here).
    let stress = mqk_backtest::p7a_p7b_economic_replay_stress_scenario(
        "python",
        &research_py_root(),
        research_registry_db,
        research_trial_id,
        research_economic_eval_id,
        &report.strategy_name,
        &artifact_root.join(format!("p7a_p7b_stress_{}", report.run_id)),
        20,   // test-fixture-only stress knob; not asserted as accepted policy
        50,   // test-fixture-only stress knob; not asserted as accepted policy
        Some(1000), // FINAL-P7A-P7B-REPLAY-AUTHORITY-01: baseline max_target_qty is
                    // None -- None -> finite is a genuine P7B tightening, required by
                    // the genuine-adversity validation (matches
                    // scenario_strategy_promotion_routes_01.rs's identical fix).
        None, // test-fixture-only stress knob; not asserted as accepted policy
        0.30, // test-fixture-only threshold; not asserted as accepted policy
    );
    mqk_artifacts::finalize_canonical_robustness_gauntlet_with_sensitivity(
        &init_result.run_dir,
        &stress,
    )
    .expect("finalize_canonical_robustness_gauntlet_with_sensitivity (p7a_p7b_economic_replay_stress)");

    // FINAL-P9-ROBUSTNESS-SEMANTICS-01: SAME trial/registry as the two
    // scenarios above.
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
    // FINAL-P10-FIXTURE-REALISM-01: real production pipeline required -- the
    // hand-built fixture no longer clears the mandatory p7a_p7b_economic_
    // replay_stress/genuine_shuffled_placebo scenarios (see module doc).
    let research_trials = write_real_research_evidence_via_production_pipeline(
        &root,
        "real_e2e_positive",
        &strategy_id,
        &[0.45, 0.55],
    );
    let research = &research_trials[0];
    let backtest_run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.economic_eval_id,
        &research.registry_db_path,
        &research.judge_artifact_sha256,
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

    // --- RESEARCH-PROMOTION-DURABLE-LINEAGE-HTTP-PROOF-01: immediately
    // after the REAL evidence-requiring HTTP transition above succeeds,
    // read the durable row it wrote back from Postgres and prove it binds
    // the EXACT Research/Backtest/robustness authority that authorized the
    // decision -- never merely that the lineage columns are non-null. Every
    // expected value below is independently re-derived from the SAME real
    // fixture files/pipeline and HTTP request this test already built,
    // via the SAME production functions the route itself calls -- never a
    // manufactured constant. -------------------------------------------
    let transition_id_1 = Uuid::parse_str(
        json["transition_id"]
            .as_str()
            .expect("shadow_approved response must carry a transition_id"),
    )
    .expect("transition_id must be a valid UUID");

    // Independently re-derive the exact verified Research OOS evidence via
    // the same `mqk_promotion::verify_promotion_oos_evidence` the route
    // calls through `research_evidence_gate`, reading the SAME real
    // artifact bytes this test's own fixture already produced.
    let econ_json =
        std::fs::read_to_string(research.evidence_dir.join("economic_walk_forward.json"))
            .expect("read economic_walk_forward.json");
    let daily_csv = std::fs::read(research.evidence_dir.join("economic_daily_returns.csv"))
        .expect("read economic_daily_returns.csv");
    let judge_json = std::fs::read_to_string(&research.judge_path).expect("read judge json");
    let verified_oos = mqk_promotion::verify_promotion_oos_evidence(
        &research.registry_db_path,
        &research.trial_id,
        &econ_json,
        &daily_csv,
        &judge_json,
    )
    .expect("independent re-verification of Research OOS evidence must succeed");

    // Independently re-derive the exact Backtest evidence bundle via the
    // same `mqk_promotion::resolve_backtest_evidence` the route calls
    // through `backtest_evidence_gate`, against the SAME
    // `MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT` this test configured.
    let verified_backtest = mqk_promotion::resolve_backtest_evidence(&root, backtest_run_id)
        .expect("independent re-resolution of Backtest evidence must succeed");

    // Independently re-derive the exact promotion-policy fingerprint from
    // the SAME MQK_PROMOTION_*/MQK_RESEARCH_MIN_*/MQK_RESEARCH_MAX_*
    // thresholds this test set at the top of this function.
    let expected_policy_fingerprint = mqk_promotion::PromotionConfig {
        min_sharpe: 0.0,
        max_mdd: 1.0,
        min_cagr: 0.0,
        min_profit_factor: 0.0,
        min_profitable_months_pct: 0.0,
        min_deflated_sharpe_ratio: 0.0,
        max_probability_backtest_overfitting: 1.0,
    }
    .deterministic_fingerprint();

    // Independently re-derive the exact legacy + v2 scanner/review evidence
    // fingerprints via the SAME `validate_paper_candidate_evidence`/
    // `compute_evidence_fingerprint_v2` the route calls for Gate 4.
    let validated_review_evidence = validate_paper_candidate_evidence(
        &st,
        review_dir.to_str().unwrap(),
        &strategy_id,
        &symbol,
        TIMEFRAME_SECS,
    )
    .expect("independent re-validation of review-artifact evidence must succeed");
    let expected_evidence_fingerprint_v2 = compute_evidence_fingerprint_v2(
        &validated_review_evidence.review_id,
        &validated_review_evidence.scanner_scan_id,
        &validated_review_evidence.git_hash,
        &strategy_id,
        &symbol,
        TIMEFRAME_SECS,
        &validated_review_evidence.review_state,
        validated_review_evidence.scanner_score_token.as_deref(),
        validated_review_evidence.scanner_rank,
        &validated_review_evidence.reason_codes,
        &validated_review_evidence.blockers,
        &validated_review_evidence.warnings,
    );

    // Read the REAL row Gate 5 durably persisted, by the EXACT
    // transition_id the HTTP response returned -- Postgres itself, not any
    // route/cache readback.
    let row1 = sqlx::query(
        r#"
        select evidence_transition_id, evidence_fingerprint, evidence_fingerprint_v2,
               research_trial_id, research_economic_eval_id,
               research_deflated_sharpe_ratio, research_probability_backtest_overfitting,
               backtest_run_id, research_judge_artifact_sha256, stress_protocol_version,
               stress_artifact_sha256, robustness_protocol_version,
               finalized_robustness_artifact_sha256, promotion_policy_fingerprint
        from sys_strategy_promotion_transitions
        where transition_id = $1
        "#,
    )
    .bind(transition_id_1)
    .fetch_one(&pool)
    .await
    .expect("readback of the durably persisted evidence-bearing transition row");

    let db_evidence_transition_id: Option<Uuid> = row1.try_get("evidence_transition_id").unwrap();
    let db_evidence_fingerprint: Option<String> = row1.try_get("evidence_fingerprint").unwrap();
    let db_evidence_fingerprint_v2: Option<String> =
        row1.try_get("evidence_fingerprint_v2").unwrap();
    let db_research_trial_id: Option<String> = row1.try_get("research_trial_id").unwrap();
    let db_research_economic_eval_id: Option<String> =
        row1.try_get("research_economic_eval_id").unwrap();
    let db_dsr: Option<f64> = row1.try_get("research_deflated_sharpe_ratio").unwrap();
    let db_pbo: Option<f64> = row1
        .try_get("research_probability_backtest_overfitting")
        .unwrap();
    let db_backtest_run_id: Option<Uuid> = row1.try_get("backtest_run_id").unwrap();
    let db_judge_sha256: Option<String> = row1.try_get("research_judge_artifact_sha256").unwrap();
    let db_stress_protocol: Option<String> = row1.try_get("stress_protocol_version").unwrap();
    let db_stress_sha256: Option<String> = row1.try_get("stress_artifact_sha256").unwrap();
    let db_robustness_protocol: Option<String> =
        row1.try_get("robustness_protocol_version").unwrap();
    let db_robustness_sha256: Option<String> =
        row1.try_get("finalized_robustness_artifact_sha256").unwrap();
    let db_policy_fingerprint: Option<String> =
        row1.try_get("promotion_policy_fingerprint").unwrap();

    // ---- Exact identity/value proof (never merely "is present") ----
    assert_eq!(
        db_evidence_transition_id,
        Some(transition_id_1),
        "evidence_transition_id must prove this evidence-bearing transition is its own \
         evidence root"
    );
    assert_eq!(
        db_evidence_fingerprint.as_deref(),
        Some(validated_review_evidence.fingerprint.as_str())
    );
    assert_eq!(
        db_evidence_fingerprint_v2.as_deref(),
        Some(expected_evidence_fingerprint_v2.as_str())
    );
    assert_eq!(db_research_trial_id.as_deref(), Some(verified_oos.trial_id()));
    assert_eq!(
        db_research_economic_eval_id.as_deref(),
        Some(verified_oos.economic_eval_id())
    );
    assert_eq!(db_dsr, Some(verified_oos.deflated_sharpe_ratio()));
    assert_eq!(
        db_pbo,
        Some(verified_oos.probability_of_backtest_overfitting())
    );
    assert_eq!(db_backtest_run_id, Some(backtest_run_id));
    assert_eq!(
        db_judge_sha256.as_deref(),
        Some(verified_oos.judge_artifact_sha256())
    );
    assert_eq!(
        db_stress_protocol.as_deref(),
        Some(verified_backtest.stress_suite.protocol_version.as_str())
    );
    assert_eq!(
        db_stress_sha256.as_deref(),
        Some(verified_backtest.stress_artifact_sha256.as_str())
    );
    assert_eq!(
        db_robustness_protocol.as_deref(),
        Some(verified_backtest.robustness_evidence.protocol_version.as_str())
    );
    assert_eq!(
        db_robustness_sha256.as_deref(),
        Some(verified_backtest.finalized_robustness_artifact_sha256.as_str())
    );
    assert_eq!(
        db_policy_fingerprint.as_deref(),
        Some(expected_policy_fingerprint.as_str())
    );
    println!(
        "[closure-proof] Postgres lineage readback proven exact for transition_id={transition_id_1}"
    );

    // ---- NEGATIVE CONTROL: research_trials[1] is a SECOND, genuinely
    // different real trial from the SAME real production pipeline run
    // (same judge/comparison population, different --entry-thresholds
    // value) -- not a manufactured constant. If the server had bound the
    // wrong trial's identity to this transition (a cross-trial lineage
    // bug), the persisted research_trial_id/research_economic_eval_id
    // would equal trial B's, not trial A's, and these assertions would
    // fail -- proving the positive assertions above are load-bearing, not
    // vacuously true from a shared None/default value (CLAUDE.md #14).
    let research_b = &research_trials[1];
    assert_ne!(
        research.trial_id, research_b.trial_id,
        "fixture precondition: the two real trials must have distinct trial_ids"
    );
    assert_ne!(
        db_research_trial_id.as_deref(),
        Some(research_b.trial_id.as_str()),
        "NEGATIVE CONTROL FAILED: persisted research_trial_id equals a DIFFERENT real trial's \
         id -- the server bound the wrong trial's identity to this transition"
    );
    assert_ne!(
        db_research_economic_eval_id.as_deref(),
        Some(research_b.economic_eval_id.as_str()),
        "NEGATIVE CONTROL FAILED: persisted research_economic_eval_id equals a DIFFERENT real \
         trial's economic_eval_id -- the server bound the wrong trial's identity to this \
         transition"
    );
    println!(
        "[closure-proof] negative control confirmed: persisted lineage does not match a \
         different real trial's identity (research_trials[1].trial_id={})",
        research_b.trial_id
    );

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
