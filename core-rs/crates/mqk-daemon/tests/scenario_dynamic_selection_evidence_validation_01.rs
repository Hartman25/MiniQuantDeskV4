//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 3: DB-backed proof for
//! `promotion_evidence_validation::validate_active_paper_candidate` — the
//! shared read-side evidence validator Bundle 7's plan builder (Phase 4)
//! will consume.
//!
//! Every identity is driven to its target promotion state through the real
//! `POST /api/v1/strategy/promotions/transition` route (same as
//! `scenario_strategy_promotion_routes_01.rs`), so these tests exercise the
//! validator against genuine durable evidence lineage, never a hand-rolled
//! DB row.
//!
//! All tests require `MQK_DATABASE_URL` and are marked `#[ignore]`. Each test
//! runs against its own disposable per-test database (`mqk_db::run_isolated`,
//! FULL-AUDIT-FAIL-017) and holds a process-global async lock for the
//! duration of its `MQK_STRATEGY_REVIEW_ARTIFACT_ROOT` env-var-sensitive
//! section, so this file is safe under the default (parallel) test runner as
//! well as `--test-threads=1`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --features testkit \
//!     --test scenario_dynamic_selection_evidence_validation_01 -- --include-ignored
//!
//! # Proof matrix
//!
//! | Test                              | What it proves                                    |
//! |------------------------------------|----------------------------------------------------|
//! | valid_active_paper_candidate_succeeds | active_paper + matching evidence -> Ok with correct score/rank |
//! | no_promotion_record_is_refused    | never-promoted identity -> NoPromotionRecord        |
//! | shadow_approved_is_not_active_paper | shadow_approved only -> PromotionNotActivePaper   |
//! | not_yet_effective_is_refused      | authority_ts before effective_at_utc -> PromotionNotYetEffective |
//! | expired_is_refused                | authority_ts after expires_at_utc -> PromotionExpired |
//! | tampered_durable_fingerprint_is_refused | durable evidence_fingerprint UPDATE'd -> FingerprintMismatch |
//! | missing_score_is_refused          | evidence row with scanner_score=None -> ScoreMissing |

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use axum::http::{header, Request, StatusCode};
use chrono::{DateTime, Duration, Utc};
use http_body_util::BodyExt;
use mqk_backtest::{
    write_review_artifacts, ReviewManifest, ReviewRunOutput, ReviewSummary,
    StrategyScanReviewDecision, StrategyScanReviewState,
};
use mqk_daemon::promotion_evidence_validation::{
    validate_active_paper_candidate, CandidateEvidenceReason,
};
use mqk_daemon::{routes, state};
use tower::ServiceExt;
use uuid::Uuid;

/// DYNAMIC-SELECTION-EVIDENCE-TEST-FIXTURE-REPAIR-01: every fresh
/// evidence-bearing transition (`no_state -> shadow_approved`) in this file
/// must resolve a real config identity (`strategy_config_identity::
/// resolve_server_semantic_fingerprint` only knows built-in registered
/// engines, never a synthetic per-test id) AND clear Gate 4c/4d's real
/// Research+Backtest evidence gates -- so every test uses the one real
/// built-in engine whose backtest evidence this file's fixture produces.
const STRATEGY_ID: &str = "swing_momentum";

// ---------------------------------------------------------------------------
// Fixtures / helpers (deliberately duplicated from
// scenario_strategy_promotion_routes_01.rs -- integration test binaries
// cannot share code without a support crate, and this repo's convention is
// one self-contained fixture set per scenario file).
// ---------------------------------------------------------------------------

/// Deterministic UUIDv5, namespaced by an explicit per-call-site seed string
/// -- never `Uuid::new_v4()`. Fixture uniqueness comes from the seed text
/// (always the caller-supplied label/prefix, itself unique per call site in
/// this file), not from randomness.
fn det_uuid(seed: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, seed.as_bytes())
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mqk_daemon_dyn_sel_evidence_{label}_{}",
        det_uuid(&format!(
            "scenario_dynamic_selection_evidence_validation_01::temp_dir::{label}"
        ))
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_fixture(out_dir: &Path, decisions: Vec<StrategyScanReviewDecision>) -> PathBuf {
    let paper_candidate_count = decisions
        .iter()
        .filter(|d| d.review_state == StrategyScanReviewState::PaperCandidate)
        .count();
    let review_id = det_uuid(&format!(
        "scenario_dynamic_selection_evidence_validation_01::write_fixture::{}",
        out_dir.display()
    ));
    let manifest = ReviewManifest {
        schema_version: 1,
        review_id: review_id.to_string(),
        scanner_scan_id: "scan-fixture".to_string(),
        source_artifact_dir: "fixture-source-not-on-disk".to_string(),
        created_at_utc: "2026-07-01T00:00:00Z".to_string(),
        git_hash: "test-git-hash".to_string(),
        policy_min_bars_used: 252,
        policy_min_trade_count: 5,
        policy_min_total_return_pct: 0.0,
        policy_min_alpha_pct: 0.0,
        policy_max_drawdown_pct: 25.0,
        policy_min_profit_factor: 1.05,
        candidate_count: decisions.len(),
        blocked_count: 0,
        needs_review_count: 0,
        watchlist_candidate_count: 0,
        paper_candidate_count,
        rejected_count: 0,
        blockers: Vec::new(),
        warnings: Vec::new(),
    };
    let summary = ReviewSummary {
        scanner_scan_id: "scan-fixture".to_string(),
        review_id: review_id.to_string(),
        candidate_count: decisions.len(),
        blocked_count: 0,
        needs_review_count: 0,
        watchlist_candidate_count: 0,
        paper_candidate_count,
        rejected_count: 0,
        top_paper_candidates: Vec::new(),
        top_watchlist_candidates: Vec::new(),
        blockers: Vec::new(),
        warnings: Vec::new(),
    };
    let output = ReviewRunOutput {
        review_id,
        manifest,
        decisions,
        summary,
    };
    write_review_artifacts(out_dir, &output).expect("write fixture review artifacts")
}

fn paper_candidate(strategy_id: &str, symbol: &str, timeframe: &str) -> StrategyScanReviewDecision {
    StrategyScanReviewDecision {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        strategy_id: strategy_id.to_string(),
        scanner_rank: Some(1),
        scanner_score: Some(9.0),
        review_state: StrategyScanReviewState::PaperCandidate,
        reason_codes: vec!["eligible_paper_candidate".to_string()],
        blockers: Vec::new(),
        warnings: Vec::new(),
    }
}

fn paper_candidate_no_score(
    strategy_id: &str,
    symbol: &str,
    timeframe: &str,
) -> StrategyScanReviewDecision {
    StrategyScanReviewDecision {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        strategy_id: strategy_id.to_string(),
        scanner_rank: Some(1),
        scanner_score: None,
        review_state: StrategyScanReviewState::PaperCandidate,
        reason_codes: vec!["eligible_paper_candidate".to_string()],
        blockers: Vec::new(),
        warnings: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// PROMOTION-WALKFORWARD-GATE-WIRING-01 / PRODUCTION-PROMOTION-DB-E2E-01:
// Gate 4c/4d requires real, verified Research out-of-sample evidence AND a
// real canonical Backtest evidence bundle for the fresh `shadow_approved`
// transition, in addition to the scanner/review evidence above. Duplicated
// verbatim (same "no cross-crate test visibility" reason already documented
// at the top of this file) from `scenario_strategy_promotion_closure_proof_
// 01f.rs`, the canonical fixture for this exact evidence shape. Built ONCE
// per test binary via `shared_evidence()` below and reused by every test in
// this file: Gate 4c/4d binds identity only to `strategy_id` (never
// `symbol`), so one real evidence bundle authorizes every (STRATEGY_ID,
// "AAPL", ...) identity here, each already isolated by its own disposable
// `run_isolated` database.
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

/// See `scenario_strategy_promotion_closure_proof_01f.rs`'s identical
/// fixture for the full rationale: 240 daily bars across three deterministic
/// 30-day legs (calm uptrend / wide-range uptrend / decline) that produce
/// three genuinely distinct market regimes, each profitable for
/// `swing_momentum`, so every P9 concentration/regime gate genuinely passes
/// on real evidence.
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

/// Runs the REAL Research production pipeline
/// (`mqk_research.ml.real_research_promotion_e2e_cli`). See
/// `scenario_strategy_promotion_closure_proof_01f.rs`'s identical function
/// for the full rationale.
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

/// Run a REAL `BacktestEngine`, write the REAL, genuinely complete evidence
/// set (manifest, audit, report, stress suite, robustness gauntlet --
/// finalized with a REAL DSR/PBO sensitivity result via a real Python
/// subprocess against a real disposable SQLite registry). Returns the
/// candidate's `run_id`. See `scenario_strategy_promotion_closure_proof_
/// 01f.rs`'s identical function for the full rationale; always runs the
/// real `swing_momentum` plugin, so `report.strategy_name == STRATEGY_ID`.
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
        git_hash: "dyn_sel_evidence_it_git_hash",
        config_hash: &config_hash,
        host_fingerprint: "dyn_sel_evidence_it_host",
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

/// Everything Gate 4c/4d needs from a fresh evidence-bearing transition
/// request, resolved ONCE (real Research trial + real Backtest run are
/// expensive: one Python subprocess plus one full engine run each) and
/// reused by every test in this file -- see the module note above for why
/// this is safe (Gate 4c/4d binds only to `strategy_id`, never `symbol`, and
/// every test has its own disposable database).
struct SharedPromotionEvidence {
    research_trial_id: String,
    research_evidence_dir: PathBuf,
    research_judge_artifact_path: PathBuf,
    backtest_run_id: Uuid,
    research_registry_db: PathBuf,
    evidence_root: PathBuf,
}

static SHARED_EVIDENCE: OnceLock<SharedPromotionEvidence> = OnceLock::new();

fn shared_evidence() -> &'static SharedPromotionEvidence {
    SHARED_EVIDENCE.get_or_init(|| {
        let root = std::env::temp_dir().join(format!(
            "mqk_daemon_dyn_sel_evidence_shared_fixture_{}",
            det_uuid("scenario_dynamic_selection_evidence_validation_01::shared_evidence_fixture")
        ));
        // Backtest run artifacts are content-hash-locked once written
        // (`finalize_canonical_robustness_gauntlet_with_sensitivity` rejects
        // a mismatched rewrite) -- this deterministic path is stable for
        // debuggability across runs, but must start empty every process run
        // so a prior run's artifacts (e.g. from a different git checkout of
        // this same fixture) never collide with a fresh build.
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create shared evidence fixture root");

        // Seed matches `scenario_strategy_promotion_closure_proof_01f.rs`'s
        // proven-good fixture: the genuine shuffled-placebo P9 control
        // deterministically permutes THIS seed's own synthetic OOS score
        // stream, so an arbitrary seed can genuinely fail that control (real
        // signal indistinguishable from noise) without any production defect
        // -- this exact seed is already verified real evidence that clears
        // every mandatory P9/P7A-P7B gate.
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
            "AAPL",
        );

        SharedPromotionEvidence {
            research_trial_id: research.trial_id.clone(),
            research_evidence_dir: research.evidence_dir.clone(),
            research_judge_artifact_path: research.judge_path.clone(),
            backtest_run_id,
            research_registry_db: research.registry_db_path.clone(),
            evidence_root: root,
        }
    })
}

// `MQK_STRATEGY_REVIEW_ARTIFACT_ROOT` is process-global state. Every test in
// this file sets it to its own private temp dir and relies on route handlers
// reading it back later in the same test (not just at set-time), so the
// hazard isn't only a torn write -- it's any interleaving where test A's
// route call reads test B's root after B has already overwritten it. Real
// failure observed: 6 of 7 tests in this file fail under the default
// (parallel) libtest runner, each independently, non-deterministically,
// while every one passes cleanly under `--test-threads=1` -- textbook
// global-environment-mutation contamination, not a logic defect. A
// process-global async lock, held for each test's entire env-var-sensitive
// section (returned to the caller and dropped at end of the test function),
// serializes these tests against each other without requiring the whole
// binary to run single-threaded.
static ENV_MUTATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn make_state_with_db(
    root: &Path,
    pool: sqlx::PgPool,
) -> (Arc<state::AppState>, tokio::sync::MutexGuard<'static, ()>) {
    let guard = ENV_MUTATION_LOCK.lock().await;
    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", root);
    // Gate 4c/4d config, required in addition to MQK_STRATEGY_REVIEW_ARTIFACT_
    // ROOT for the fresh `shadow_approved` transition -- same env-var-
    // sensitive section as above, so guarded by the same lock. Thresholds are
    // permissive (0.0 / max) since these tests validate identity/lineage
    // plumbing, not promotion policy acceptance thresholds.
    let evidence = shared_evidence();
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
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    (st, guard)
}

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, body)
}

const TRANSITION_ROUTE: &str = "/api/v1/strategy/promotions/transition";

/// `research_*`/`backtest_run_id` are only required for a fresh
/// evidence-bearing transition (`no_state -> shadow_approved`) -- Gate 4c/4d
/// (`transition_requires_evidence`) does not run for continuity transitions,
/// so callers pass `None` for those hops.
#[allow(clippy::too_many_arguments)]
fn transition_body(
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    target_state: &str,
    review_dir: Option<&str>,
    research_trial_id: Option<&str>,
    research_evidence_dir: Option<&str>,
    research_judge_artifact_path: Option<&str>,
    backtest_run_id: Option<&str>,
    effective_at_utc: DateTime<Utc>,
    expires_at_utc: Option<DateTime<Utc>>,
) -> serde_json::Value {
    serde_json::json!({
        "strategy_id": strategy_id,
        "symbol": symbol,
        "timeframe_secs": timeframe_secs,
        "target_state": target_state,
        "review_dir": review_dir,
        "research_trial_id": research_trial_id,
        "research_evidence_dir": research_evidence_dir,
        "research_judge_artifact_path": research_judge_artifact_path,
        "backtest_run_id": backtest_run_id,
        "effective_at_utc": effective_at_utc.to_rfc3339(),
        "expires_at_utc": expires_at_utc.map(|t| t.to_rfc3339()),
        "initiated_by": "test-operator",
        "reason": "scenario test",
    })
}

fn post_json_req(uri: &str, body: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

/// Drives an identity through shadow_approved -> paper_approved ->
/// active_paper via the real POST route, asserting 200 OK at each step.
///
/// `effective_at_offset`/`expires_at_offset` are applied relative to
/// `Utc::now()` captured *immediately before the active_paper POST*, not
/// before the earlier shadow_approved/paper_approved calls -- current-state
/// resolution orders by `effective_at_utc desc` first, so an absolute
/// timestamp computed before the intermediate transitions can sort *behind*
/// them and leave `paper_approved` (not `active_paper`) as the identity's
/// resolved "current" row, even though the active_paper POST itself
/// returned 200.
#[allow(clippy::too_many_arguments)]
async fn promote_to_active_paper(
    st: Arc<state::AppState>,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    review_dir: &Path,
    evidence: &SharedPromotionEvidence,
    effective_at_offset: Duration,
    expires_at_offset: Option<Duration>,
) {
    let backtest_run_id_str = evidence.backtest_run_id.to_string();
    let body = transition_body(
        strategy_id,
        symbol,
        timeframe_secs,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        Some(&evidence.research_trial_id),
        Some(evidence.research_evidence_dir.to_str().unwrap()),
        Some(evidence.research_judge_artifact_path.to_str().unwrap()),
        Some(&backtest_run_id_str),
        Utc::now(),
        None,
    );
    let (status, resp_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "shadow_approved failed: {:?}",
        String::from_utf8_lossy(&resp_body)
    );

    let body = transition_body(
        strategy_id,
        symbol,
        timeframe_secs,
        "paper_approved",
        None,
        None,
        None,
        None,
        None,
        Utc::now(),
        None,
    );
    let (status, resp_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "paper_approved failed: {:?}",
        String::from_utf8_lossy(&resp_body)
    );

    let effective_at_utc = Utc::now() + effective_at_offset;
    let expires_at_utc = expires_at_offset.map(|d| effective_at_utc + d);
    let body = transition_body(
        strategy_id,
        symbol,
        timeframe_secs,
        "active_paper",
        None,
        None,
        None,
        None,
        None,
        effective_at_utc,
        expires_at_utc,
    );
    let (status, resp_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "active_paper failed: {:?}",
        String::from_utf8_lossy(&resp_body)
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn valid_active_paper_candidate_succeeds() {
    // `temp_dir` is deterministic (UUIDv5 over a fixed string, by design --
    // see its doc comment), so a shared, never-cleaned-up MQK_DATABASE_URL
    // database retains this test's promotion row forever after the first
    // successful run; every later run then finds the identity already at
    // `active_paper` and fails the very first shadow_approved transition as
    // "illegal_transition" (real failure observed). A disposable per-test
    // database removes that residue.
    mqk_db::run_isolated("dyn_sel_valid", |pool| async move {
        let root = temp_dir("valid");
        let strategy_id = STRATEGY_ID;
        let review_dir = write_fixture(&root, vec![paper_candidate(strategy_id, "AAPL", "1D")]);
        let (st, _env_guard) = make_state_with_db(&root, pool.clone()).await;

        promote_to_active_paper(
            Arc::clone(&st),
            strategy_id,
            "AAPL",
            86400,
            &review_dir,
            shared_evidence(),
            Duration::zero(),
            None,
        )
        .await;

        let result =
            validate_active_paper_candidate(&pool, &st, strategy_id, "AAPL", 86400, Utc::now())
                .await;
        let evidence = result.expect("expected Ok for a valid active_paper candidate");
        assert_eq!(evidence.canonical_score_decimal, "9");
        assert_eq!(evidence.canonical_score_micros, Some(9_000_000));
        assert_eq!(evidence.scanner_rank, Some(1));
        assert!(!evidence.recomputed_legacy_fingerprint.is_empty());
        assert!(!evidence.recomputed_exact_fingerprint_v2.is_empty());
        assert_eq!(
            evidence.durable_legacy_fingerprint,
            evidence.recomputed_legacy_fingerprint
        );
        assert_eq!(
            evidence.durable_exact_fingerprint_v2,
            evidence.recomputed_exact_fingerprint_v2
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn no_promotion_record_is_refused() {
    mqk_db::run_isolated("dyn_sel_no_record", |pool| async move {
        let root = temp_dir("no_record");
        let strategy_id = STRATEGY_ID;
        let (st, _env_guard) = make_state_with_db(&root, pool.clone()).await;

        let result =
            validate_active_paper_candidate(&pool, &st, strategy_id, "AAPL", 86400, Utc::now())
                .await;
        assert_eq!(result, Err(CandidateEvidenceReason::NoPromotionRecord));
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn shadow_approved_is_not_active_paper() {
    mqk_db::run_isolated("dyn_sel_shadow", |pool| async move {
        let root = temp_dir("shadow_only");
        let strategy_id = STRATEGY_ID;
        let review_dir = write_fixture(&root, vec![paper_candidate(strategy_id, "AAPL", "1D")]);
        let (st, _env_guard) = make_state_with_db(&root, pool.clone()).await;

        let evidence = shared_evidence();
        let backtest_run_id_str = evidence.backtest_run_id.to_string();
        let body = transition_body(
            strategy_id,
            "AAPL",
            86400,
            "shadow_approved",
            Some(review_dir.to_str().unwrap()),
            Some(&evidence.research_trial_id),
            Some(evidence.research_evidence_dir.to_str().unwrap()),
            Some(evidence.research_judge_artifact_path.to_str().unwrap()),
            Some(&backtest_run_id_str),
            Utc::now(),
            None,
        );
        let (status, resp_body) = call(
            routes::build_router(Arc::clone(&st)),
            post_json_req(TRANSITION_ROUTE, body),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "shadow_approved failed: {:?}",
            String::from_utf8_lossy(&resp_body)
        );

        let result =
            validate_active_paper_candidate(&pool, &st, strategy_id, "AAPL", 86400, Utc::now())
                .await;
        assert_eq!(
            result,
            Err(CandidateEvidenceReason::PromotionNotActivePaper)
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn not_yet_effective_is_refused() {
    mqk_db::run_isolated("dyn_sel_nye", |pool| async move {
        let root = temp_dir("not_yet_effective");
        let strategy_id = STRATEGY_ID;
        let review_dir = write_fixture(&root, vec![paper_candidate(strategy_id, "AAPL", "1D")]);
        let (st, _env_guard) = make_state_with_db(&root, pool.clone()).await;

        // effective_at_utc 2 minutes ahead -- inside the route's 5-minute
        // clock-skew tolerance, so the transition itself is accepted.
        promote_to_active_paper(
            Arc::clone(&st),
            strategy_id,
            "AAPL",
            86400,
            &review_dir,
            shared_evidence(),
            Duration::minutes(2),
            None,
        )
        .await;

        // authority_ts before effective_at_utc.
        let result =
            validate_active_paper_candidate(&pool, &st, strategy_id, "AAPL", 86400, Utc::now())
                .await;
        assert_eq!(
            result,
            Err(CandidateEvidenceReason::PromotionNotYetEffective)
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn expired_is_refused() {
    mqk_db::run_isolated("dyn_sel_expired", |pool| async move {
        let root = temp_dir("expired");
        let strategy_id = STRATEGY_ID;
        let review_dir = write_fixture(&root, vec![paper_candidate(strategy_id, "AAPL", "1D")]);
        let (st, _env_guard) = make_state_with_db(&root, pool.clone()).await;

        promote_to_active_paper(
            Arc::clone(&st),
            strategy_id,
            "AAPL",
            86400,
            &review_dir,
            shared_evidence(),
            Duration::zero(),
            Some(Duration::minutes(1)),
        )
        .await;

        // authority_ts far past expires_at (effective+1min, computed inside
        // the helper relative to its own call time) -- "+1 hour from now" is
        // safely past it regardless of the few milliseconds of test overhead
        // above.
        let authority_ts = Utc::now() + Duration::hours(1);
        let result =
            validate_active_paper_candidate(&pool, &st, strategy_id, "AAPL", 86400, authority_ts)
                .await;
        assert_eq!(result, Err(CandidateEvidenceReason::PromotionExpired));
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn tampered_durable_fingerprint_is_refused() {
    mqk_db::run_isolated("dyn_sel_tampered", |pool| async move {
        let root = temp_dir("tampered_fingerprint");
        let strategy_id = STRATEGY_ID;
        let review_dir = write_fixture(&root, vec![paper_candidate(strategy_id, "AAPL", "1D")]);
        let (st, _env_guard) = make_state_with_db(&root, pool.clone()).await;

        promote_to_active_paper(
            Arc::clone(&st),
            strategy_id,
            "AAPL",
            86400,
            &review_dir,
            shared_evidence(),
            Duration::zero(),
            None,
        )
        .await;

        // Simulate durable drift: the artifact on disk still matches what was
        // originally validated, but the DB's evidence_fingerprint column has
        // since diverged (e.g. corruption, manual tampering).
        sqlx::query(
            "update sys_strategy_promotion_transitions set evidence_fingerprint = 'deliberately-wrong-fingerprint' \
             where strategy_id = $1 and symbol = $2 and timeframe_secs = $3 and evidence_fingerprint is not null",
        )
        .bind(strategy_id)
        .bind("AAPL")
        .bind(86400i64)
        .execute(&pool)
        .await
        .expect("tamper update");

        let result =
            validate_active_paper_candidate(&pool, &st, strategy_id, "AAPL", 86400, Utc::now())
                .await;
        assert_eq!(result, Err(CandidateEvidenceReason::FingerprintMismatch));
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn missing_score_is_refused() {
    mqk_db::run_isolated("dyn_sel_no_score", |pool| async move {
        let root = temp_dir("missing_score");
        let strategy_id = STRATEGY_ID;
        let review_dir = write_fixture(
            &root,
            vec![paper_candidate_no_score(strategy_id, "AAPL", "1D")],
        );
        let (st, _env_guard) = make_state_with_db(&root, pool.clone()).await;

        promote_to_active_paper(
            Arc::clone(&st),
            strategy_id,
            "AAPL",
            86400,
            &review_dir,
            shared_evidence(),
            Duration::zero(),
            None,
        )
        .await;

        let result =
            validate_active_paper_candidate(&pool, &st, strategy_id, "AAPL", 86400, Utc::now())
                .await;
        assert_eq!(result, Err(CandidateEvidenceReason::ScoreMissing));
    })
    .await;
}
