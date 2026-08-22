//! STRATEGY-PROMOTION-REGISTRY-01C: Daemon strategy promotion control
//! surface proof tests.
//!
//! Every evidence fixture is hand-constructed in-memory and written to a
//! fresh OS temp directory via `mqk_backtest::write_review_artifacts` (the
//! same function the CLI's `mqk backtest review-scan` command calls) — no
//! test opens a provider/broker/network call. Tests that construct state
//! mutate `MQK_STRATEGY_REVIEW_ARTIFACT_ROOT` process-wide, so this file
//! must run with `--test-threads=1`.
//!
//! No-DB / field-validation tests run unconditionally. Every test that
//! touches the promotion registry itself requires `MQK_DATABASE_URL` and is
//! marked `#[ignore]`. Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_strategy_promotion_routes_01 \
//!     -- --include-ignored --test-threads=1
//!
//! # Proof matrix
//!
//! | Test                                             | What it proves                                   |
//! |---------------------------------------------------|---------------------------------------------------|
//! | mutation_requires_operator_auth                   | POST .../transition with no Bearer token -> 401  |
//! | mutation_wrong_token_rejected                     | wrong Bearer token -> 401                        |
//! | mutation_field_validation_before_db                | blank/invalid fields rejected before any DB call |
//! | valid_paper_candidate_creates_first_transition     | matching paper_candidate + P7C research evidence -> transitioned |
//! | valid_scanner_evidence_without_research_evidence_is_rejected | scanner evidence alone (no P7C fields) -> evidence_invalid |
//! | valid_research_evidence_without_scanner_evidence_is_rejected | P7C evidence alone (rejected scanner row) -> evidence_invalid |
//! | rejected_candidate_cannot_be_approved              | review_state=rejected -> evidence_invalid        |
//! | watchlist_candidate_cannot_be_approved             | review_state=watchlist_candidate -> evidence_invalid |
//! | missing_decision_cannot_be_approved                | no matching row -> evidence_invalid              |
//! | duplicate_matching_rows_fail_closed                | 2 matching rows -> evidence_invalid (ambiguous)  |
//! | mismatched_identity_fails                          | symbol/timeframe mismatch -> evidence_invalid    |
//! | path_traversal_rejected                            | review_dir outside root -> evidence_invalid      |
//! | malformed_manifest_json_fails                      | broken manifest.json -> evidence_invalid         |
//! | no_db_produces_unavailable                         | no DB pool -> 503 unavailable                    |
//! | valid_transition_visible_on_read_routes            | promotions/check + promotions list reflect it    |
//! | history_remains_visible_after_later_transition     | 2 transitions -> both rows in history, newest first |
//! | illegal_transition_creates_no_row                  | no-state -> active_paper rejected, no DB row     |
//! | duplicate_transition_request_is_idempotent         | identical POST replayed -> disposition=duplicate |
//! | tradable_live_always_false                         | every response's tradable_live is false          |

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::{header, Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_backtest::{
    write_review_artifacts, ReviewManifest, ReviewRunOutput, ReviewSummary,
    StrategyScanReviewDecision, StrategyScanReviewState,
};
use mqk_daemon::{routes, state};
use tower::ServiceExt;
use uuid::Uuid;

const TEST_TOKEN: &str = "promo-test-token";

// ---------------------------------------------------------------------------
// Fixtures / helpers
// ---------------------------------------------------------------------------

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mqk_daemon_strategy_promotion_routes_{label}_{}",
        Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

/// Writes a fixture review artifact directory containing exactly the given
/// decisions, via the same `write_review_artifacts` the CLI uses.
fn write_fixture(out_dir: &Path, decisions: Vec<StrategyScanReviewDecision>) -> PathBuf {
    let paper_candidate_count = decisions
        .iter()
        .filter(|d| d.review_state == StrategyScanReviewState::PaperCandidate)
        .count();
    let review_id = Uuid::new_v4();
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
        watchlist_candidate_count: decisions
            .iter()
            .filter(|d| d.review_state == StrategyScanReviewState::WatchlistCandidate)
            .count(),
        paper_candidate_count,
        rejected_count: decisions
            .iter()
            .filter(|d| d.review_state == StrategyScanReviewState::Rejected)
            .count(),
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

fn rejected_candidate(
    strategy_id: &str,
    symbol: &str,
    timeframe: &str,
) -> StrategyScanReviewDecision {
    StrategyScanReviewDecision {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        strategy_id: strategy_id.to_string(),
        scanner_rank: Some(1),
        scanner_score: Some(1.0),
        review_state: StrategyScanReviewState::Rejected,
        reason_codes: vec!["negative_total_return".to_string()],
        blockers: vec!["total_return_pct is negative".to_string()],
        warnings: Vec::new(),
    }
}

fn watchlist_candidate(
    strategy_id: &str,
    symbol: &str,
    timeframe: &str,
) -> StrategyScanReviewDecision {
    StrategyScanReviewDecision {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        strategy_id: strategy_id.to_string(),
        scanner_rank: Some(1),
        scanner_score: Some(5.0),
        review_state: StrategyScanReviewState::WatchlistCandidate,
        reason_codes: vec!["weak_profit_factor".to_string()],
        blockers: Vec::new(),
        warnings: vec!["profit_factor below required minimum".to_string()],
    }
}

/// PROMOTION-WALKFORWARD-GATE-WIRING-01: research-evidence env config shared
/// by every state builder below. The registry file need not exist yet at
/// env-set time -- the gate fails closed with its own clear message if it's
/// missing when actually queried; tests that need the P7C gate to PASS call
/// `write_research_evidence_fixture` first, which creates this exact path.
/// Thresholds are set generously permissive (not `None`/unset) so ordinary
/// fixture evidence (dsr=0.85, pbo=0.15) passes -- tests that specifically
/// exercise threshold rejection override these two vars themselves.
fn set_research_evidence_env(root: &Path) {
    std::env::set_var(
        "MQK_RESEARCH_REGISTRY_DB",
        root.join("research_registry.sqlite3"),
    );
    std::env::set_var("MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT", root);
    std::env::set_var("MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO", "0.0");
    std::env::set_var("MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING", "1.0");
}

/// A fully self-consistent, registry-anchored evidence bundle for a
/// synthetic trial named after `seed`, written under `root` (must be inside
/// whatever root `set_research_evidence_env` was called with). Mirrors
/// `mqk-promotion/tests/common`'s exact artifact shapes.
struct ResearchEvidenceFixture {
    registry_db_path: PathBuf,
    evidence_dir: PathBuf,
    judge_path: PathBuf,
    trial_id: String,
}

fn write_research_evidence_fixture(root: &Path, seed: &str) -> ResearchEvidenceFixture {
    write_research_evidence_fixture_with_strategy(root, seed, seed)
}

/// PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01: like
/// [`write_research_evidence_fixture`], but registers the trial under an
/// explicit `strategy_id` distinct from `seed` -- lets a test register TWO
/// genuinely distinct, individually real, registry-anchored trials under
/// the SAME `strategy_id` (calling this twice with the same `root` but
/// different `seed`s accumulates both trials in the one shared
/// `research_registry.sqlite3`, since the registry path is not
/// `seed`-dependent).
fn write_research_evidence_fixture_with_strategy(
    root: &Path,
    seed: &str,
    strategy_id: &str,
) -> ResearchEvidenceFixture {
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
    // hand-rolled `rusqlite` schema. A hand-rolled minimal schema (trial_id/
    // experiment_id/hypothesis_id/strategy_id columns only) satisfies
    // `verify_promotion_oos_evidence`'s own narrow reads, but silently
    // diverges from the REAL registry schema `dsr_pbo_sensitivity_cli.py`
    // (via `ResearchResultStore`) reads when P9 evidence for this SAME
    // trial is finalized against this SAME registry file -- e.g. a missing
    // `protocol_id` column breaks the real reader outright. Using the real
    // store here means both P7C (this function) and P9
    // (`write_real_backtest_evidence`) evidence for one trial are always
    // read from the one real, production-shaped registry.
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
         attempt_id, _ = store.begin_attempt(trial_id={trial_id}, origin='daemon_route_it_fixture')\n\
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
        strategy_id = py_str_literal(strategy_id),
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
        registry_db_path,
        evidence_dir,
        judge_path,
        trial_id,
    }
}

/// CANONICAL-ROBUSTNESS-PROMOTION-GATE-01: trusted daemon config for the
/// backtest-evidence gate (`backtest_evidence_gate::evaluate_backtest_evidence_gate`),
/// alongside the Research env vars above. `root` doubles as the artifact
/// root `write_real_backtest_evidence` below writes real candidates into --
/// the SAME convention `mqk_artifacts::init_run_artifacts` always uses.
fn set_backtest_evidence_env(root: &Path) {
    std::env::set_var("MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT", root);
    std::env::set_var("MQK_PROMOTION_MIN_SHARPE", "0.0");
    std::env::set_var("MQK_PROMOTION_MAX_MDD", "1.0");
    std::env::set_var("MQK_PROMOTION_MIN_CAGR", "0.0");
    std::env::set_var("MQK_PROMOTION_MIN_PROFIT_FACTOR", "0.0");
    std::env::set_var("MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT", "0.0");
}

// ---------------------------------------------------------------------------
// CANONICAL-ROBUSTNESS-PROMOTION-GATE-01 / PRODUCTION-PROMOTION-DB-E2E-01:
// real Backtest-side promotion evidence, via the SAME production functions
// mqk-cli's `backtest csv` + `finalize-robustness-sensitivity` call --
// never a hand-built JSON fixture, never a fabricated RobustnessEvidence.
// ---------------------------------------------------------------------------

/// 182 daily bars, ~0.35%/day compounding growth: manually verified (see
/// BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01's own commit) to make
/// `swing_momentum` genuinely trade AND clear every real P9 scenario
/// (execution-delay, month/regime concentration, parameter-neighborhood,
/// placebo, conservative-capacity) -- not tuned to force a pass on a
/// scenario that would otherwise fail; a candidate that behaves badly still
/// fails these for real (see `bars_that_fail_concentration` below).
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

/// 90 daily bars, front-loaded then flat-then-jump growth: the SAME shape
/// proven (BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01's own manual
/// smoke test) to make `month_and_regime_concentration` genuinely FAIL
/// (>50% of total gain concentrated in one calendar month) -- real evidence
/// of a real defect, not a fabricated failure.
fn bars_that_fail_concentration(symbol: &str) -> Vec<mqk_backtest::BacktestBar> {
    let m: i64 = 1_000_000;
    let start: i64 = 1_704_229_200; // 2024-01-02T21:00:00Z
    let mut price = 500.0_f64;
    let mut bars = Vec::with_capacity(90);
    for i in 0..90i64 {
        let ts = start + i * 86_400;
        price *= 1.002;
        let o = (price * m as f64) as i64;
        let h = (price * 1.01 * m as f64) as i64;
        let l = (price * 0.99 * m as f64) as i64;
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

/// Safe Python string-literal encoding for embedding a Rust `&str` into an
/// inline `python -c` script -- JSON string escaping is a valid subset of
/// Python string-literal escaping for the plain paths/identifiers used here.
fn py_str_literal(s: &str) -> String {
    serde_json::to_string(s).expect("string always serializes")
}

/// Options for [`write_real_backtest_evidence`] -- every knob a caller
/// might deliberately vary to construct a genuine negative control (never
/// by hand-editing the resulting JSON).
struct RealEvidenceOptions {
    bars: Vec<mqk_backtest::BacktestBar>,
    /// If `false`, never call `write_canonical_stress_suite` at all
    /// (missing stress evidence).
    write_stress: bool,
    /// If `false`, never call `write_canonical_robustness_gauntlet` at all
    /// (missing P9 evidence).
    write_p9: bool,
    /// If `false`, skip the real DSR/PBO sensitivity finalize step (P9
    /// stays incomplete: `dsr_pbo_sensitivity` remains deferred).
    finalize_sensitivity: bool,
}

impl Default for RealEvidenceOptions {
    fn default() -> Self {
        Self {
            bars: Vec::new(), // caller always overrides
            write_stress: true,
            write_p9: true,
            finalize_sensitivity: true,
        }
    }
}

/// Run a REAL `BacktestEngine`, write the REAL full evidence set (manifest,
/// audit, `backtest_report.json`, `stress_suite.json`,
/// `robustness_gauntlet.json` -- finalized with a REAL DSR/PBO sensitivity
/// result via a real Python subprocess against a real disposable SQLite
/// registry) via the exact same production functions
/// `mqk-cli`'s `backtest csv` + `finalize-robustness-sensitivity` call.
/// Returns the candidate's `run_id`. `artifact_root` must be the SAME root
/// `set_backtest_evidence_env` configured.
///
/// PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01: `research_trial_id`/
/// `research_registry_db` MUST be the EXACT SAME trial_id and registry a
/// paired [`write_research_evidence_fixture`] call already registered --
/// this function no longer registers a separate, differently-named trial
/// in a separate registry file for P9 (the confirmed defect: P7C and P9
/// evidence silently came from two different trials). The trial is
/// expected to already exist in `research_registry_db` (with its
/// `strategy_id` matching `report.strategy_name`, always `"swing_momentum"`
/// for the real plugin this function always runs) by the time this is
/// called.
fn write_real_backtest_evidence(
    artifact_root: &Path,
    research_trial_id: &str,
    research_registry_db: &Path,
    symbol: &str,
    opts: RealEvidenceOptions,
) -> Uuid {
    use mqk_strategy::engines::register_builtin_strategies_with_sizing;
    use mqk_strategy::PluginRegistry;

    // Mirrors `mqk-cli`'s own `backtest csv` config construction exactly
    // (`conservative_defaults()` + timeframe + integrity threshold) -- the
    // SAME config shape manually validated (BKT-PROMOTION-EVIDENCE-
    // PRODUCTION-FINALIZER-01) to make `swing_momentum` genuinely trade and
    // clear every real P9 scenario on daily bars.
    let mut cfg = mqk_backtest::BacktestConfig::conservative_defaults();
    cfg.timeframe_secs = 86_400;
    cfg.integrity_stale_threshold_ticks = 200_000;
    let initial_cash = cfg.initial_cash_micros;

    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, symbol, 1, None, None)
        .expect("register_builtin_strategies_with_sizing");
    // The engine always runs the real `swing_momentum` strategy; its own
    // `report.strategy_name` (== "swing_momentum") is what
    // `evaluate_backtest_evidence_gate`'s cross-candidate check below
    // compares the daemon route's `strategy_id` against, and what
    // `dsr_pbo_sensitivity_scenario`'s own cross-candidate check compares
    // the Research trial's `strategy_id` against.

    let mut engine = mqk_backtest::BacktestEngine::new(cfg.clone());
    engine
        .add_strategy(reg.instantiate("swing_momentum").expect("swing_momentum registered"))
        .expect("add_strategy");
    let report = engine.run(&opts.bars).expect("engine.run");

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
        git_hash: "daemon_route_it_git_hash",
        config_hash: &config_hash,
        host_fingerprint: "daemon_route_it_host",
        now_utc: Utc::now(),
    })
    .expect("init_run_artifacts");

    mqk_artifacts::write_backtest_report(&init_result.run_dir, &report, initial_cash)
        .expect("write_backtest_report");

    if opts.write_stress {
        let stress_output =
            mqk_backtest::run_backtest_stress_suite(&report, &cfg, &opts.bars, || {
                reg.instantiate("swing_momentum").expect("swing_momentum registered")
            });
        mqk_artifacts::write_canonical_stress_suite(&init_result.run_dir, &stress_output)
            .expect("write_canonical_stress_suite");
    }

    if opts.write_p9 {
        let gauntlet_output =
            mqk_backtest::run_robustness_gauntlet(&report, &cfg, &opts.bars, || {
                reg.instantiate("swing_momentum").expect("swing_momentum registered")
            });
        mqk_artifacts::write_canonical_robustness_gauntlet(&init_result.run_dir, &gauntlet_output)
            .expect("write_canonical_robustness_gauntlet");

        if opts.finalize_sensitivity {
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
        }
    }

    report.run_id
}

fn make_state_no_db(root: &Path, auth: state::OperatorAuthMode) -> Arc<state::AppState> {
    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", root);
    set_research_evidence_env(root);
    set_backtest_evidence_env(root);
    Arc::new(state::AppState::new_with_operator_auth(auth))
}

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_strategy_promotion_routes_01 \
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

fn make_state_with_db(
    root: &Path,
    pool: sqlx::PgPool,
    auth: state::OperatorAuthMode,
) -> Arc<state::AppState> {
    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", root);
    set_research_evidence_env(root);
    set_backtest_evidence_env(root);
    Arc::new(state::AppState::new_with_db_and_operator_auth(pool, auth))
}

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, body)
}

fn parse_json(body: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&body).expect("response body must be valid JSON")
}

fn router_with(st: &Arc<state::AppState>) -> axum::Router {
    routes::build_router(Arc::clone(st))
}

fn get_req(uri: &str) -> Request<axum::body::Body> {
    Request::builder()
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

fn post_json_req(
    uri: &str,
    token: Option<&str>,
    body: serde_json::Value,
) -> Request<axum::body::Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    builder
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn urlencoding_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn transition_body(
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    target_state: &str,
    review_dir: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "strategy_id": strategy_id,
        "symbol": symbol,
        "timeframe_secs": timeframe_secs,
        "target_state": target_state,
        "review_dir": review_dir,
        "effective_at_utc": Utc::now().to_rfc3339(),
        "expires_at_utc": null,
        "initiated_by": "test-operator",
        "reason": "scenario test",
    })
}

/// PROMOTION-WALKFORWARD-GATE-WIRING-01 / CANONICAL-ROBUSTNESS-PROMOTION-
/// GATE-01: like `transition_body`, but also carries the three P7C
/// research-evidence fields AND `backtest_run_id` required for any
/// evidence-requiring transition to actually succeed (Gate 4c/4d). Tests
/// that only exercise Gate 4 (scanner evidence) rejection paths never reach
/// Gate 4c and keep using plain `transition_body`; for those, an empty/
/// irrelevant `backtest_run_id` is harmless since Gate 4 rejects first.
#[allow(clippy::too_many_arguments)]
fn transition_body_with_research(
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    target_state: &str,
    review_dir: Option<&str>,
    research_trial_id: &str,
    research_evidence_dir: &str,
    research_judge_artifact_path: &str,
    backtest_run_id: &str,
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
        "effective_at_utc": Utc::now().to_rfc3339(),
        "expires_at_utc": null,
        "initiated_by": "test-operator",
        "reason": "scenario test",
    })
}

const TRANSITION_ROUTE: &str = "/api/v1/strategy/promotions/transition";

// ---------------------------------------------------------------------------
// Auth (no DB required — middleware runs before the handler)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mutation_requires_operator_auth() {
    let root = temp_dir("auth_missing");
    let st = make_state_no_db(
        &root,
        state::OperatorAuthMode::TokenRequired(TEST_TOKEN.to_string()),
    );
    let router = routes::build_router(st);

    let body = transition_body("strat", "AAPL", 86400, "shadow_approved", None);
    let (status, _) = call(router, post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mutation_wrong_token_rejected() {
    let root = temp_dir("auth_wrong");
    let st = make_state_no_db(
        &root,
        state::OperatorAuthMode::TokenRequired(TEST_TOKEN.to_string()),
    );
    let router = routes::build_router(st);

    let body = transition_body("strat", "AAPL", 86400, "shadow_approved", None);
    let (status, _) = call(
        router,
        post_json_req(TRANSITION_ROUTE, Some("not-the-token"), body),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Field validation (no DB required — Gate 0 runs before Gate 1's DB check)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mutation_field_validation_before_db() {
    let root = temp_dir("field_validation");
    let st = make_state_no_db(&root, state::OperatorAuthMode::ExplicitDevNoToken);
    let router = routes::build_router(Arc::clone(&st));

    // Blank strategy_id.
    let body = transition_body("", "AAPL", 86400, "shadow_approved", None);
    let (status, resp_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["disposition"], "rejected");

    // Wildcard symbol.
    let body = transition_body("strat", "*", 86400, "shadow_approved", None);
    let (status, resp_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert!(json["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b.as_str().unwrap().contains("wildcard")));

    // Non-positive timeframe.
    let body = transition_body("strat", "AAPL", 0, "shadow_approved", None);
    let (status, _) = call(router, post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Evidence validation (DB required — Gate 1 runs before Gate 4's evidence
// check, so these must have a DB configured to reach that gate)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn valid_paper_candidate_creates_first_transition() {
    // Gate 4c/4d's backtest-evidence gate binds candidate identity via the
    // REAL engine's own `strategy_name` (always "swing_momentum" for the
    // real `swing_momentum` plugin -- see `write_real_backtest_evidence`),
    // so `strategy_id` here must be the real name, not a synthetic unique
    // id. The symbol is varied instead to keep this test's DB identity
    // (strategy_id, symbol, timeframe_secs) disjoint from other tests.
    let root = temp_dir("valid_paper_candidate");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            ..Default::default()
        },
    );
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let router = routes::build_router(st);

    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        &run_id.to_string(),
    );
    let (status, resp_body) = call(router, post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(resp_body);
    assert_eq!(json["accepted"], true);
    assert_eq!(json["disposition"], "transitioned");
    assert_eq!(json["previous_state"], serde_json::Value::Null);
    assert_eq!(json["target_state"], "shadow_approved");
    assert!(json["transition_id"].is_string());
}

/// PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01 (REQUIRED NEGATIVE CONTROL,
/// REAL-RESEARCH-TO-PROMOTION-E2E-01 item 1): Trial A and Trial B are both
/// real, independently registered Research trials under the SAME
/// `strategy_id`, in the SAME real registry (via `ResearchResultStore`,
/// never hand-inserted rows). Trial A supplies the P7C/OOS evidence; Trial
/// B supplies the P9 `dsr_pbo_sensitivity` evidence. Through the REAL HTTP
/// route against a real disposable Postgres, this must be rejected -- proof
/// that the confirmed defect (P7C and P9 evidence silently coming from two
/// different trials) is closed end to end, not merely at the unit-test
/// level (see `mqk-promotion`'s own
/// `scenario_research_backtest_promotion_v1_acceptance_01.rs::p10d`).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn same_strategy_different_research_trial_for_p9_vs_p7c_is_rejected() {
    let root = temp_dir("trial_binding_mismatch");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);

    // Trial A: real, registered, supplies P7C/OOS evidence.
    let research_a =
        write_research_evidence_fixture_with_strategy(&root, "trialbind_a", &strategy_id);
    // Trial B: ALSO real, ALSO independently registered under the SAME
    // strategy_id (same shared registry file) -- supplies P9 sensitivity.
    let research_b =
        write_research_evidence_fixture_with_strategy(&root, "trialbind_b", &strategy_id);
    assert_ne!(research_a.trial_id, research_b.trial_id);

    let run_id = write_real_backtest_evidence(
        &root,
        &research_b.trial_id,
        &research_b.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            ..Default::default()
        },
    );

    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let router = routes::build_router(st);

    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research_a.trial_id, // P7C evidence: Trial A
        research_a.evidence_dir.to_str().unwrap(),
        research_a.judge_path.to_str().unwrap(),
        &run_id.to_string(), // P9 evidence (bound above): Trial B
    );
    let (status, resp_body) = call(router, post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "evidence_invalid");
    assert!(
        json["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b.as_str().unwrap().contains("Research trial binding mismatch")),
        "got: {json}"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn valid_scanner_evidence_without_research_evidence_is_rejected() {
    // PROMOTION-WALKFORWARD-GATE-WIRING-01, mission Section 4E item 15:
    // scanner/review evidence alone (Gate 4) must never be sufficient --
    // Gate 4c (P7C Research OOS evidence) is REQUIRED ADDITIONALLY, not
    // optionally. Same valid scanner evidence as
    // `valid_paper_candidate_creates_first_transition`, but the three
    // research_* fields are simply omitted.
    let root = temp_dir("scanner_only_no_research");
    let strategy_id = unique_id("promo_route_scanner_only");
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, "AAPL", "1D")]);
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let router = routes::build_router(st);

    let body = transition_body(
        &strategy_id,
        "AAPL",
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
    );
    let (status, resp_body) = call(router, post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "evidence_invalid");
    assert!(json["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b.as_str().unwrap().contains("research_trial_id")));
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn valid_research_evidence_without_scanner_evidence_is_rejected() {
    // Mission Section 4E item 16: P7C evidence alone must never be
    // sufficient either -- Gate 4 (scanner/review evidence) still gates
    // first. Genuine research evidence, but `review_dir` points at a
    // fixture containing a REJECTED (not paper_candidate) decision.
    let root = temp_dir("research_only_no_scanner");
    let strategy_id = unique_id("promo_route_research_only");
    let review_dir = write_fixture(&root, vec![rejected_candidate(&strategy_id, "AAPL", "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let router = routes::build_router(st);

    let body = transition_body_with_research(
        &strategy_id,
        "AAPL",
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        // Gate 4 (scanner/review evidence) rejects this request before Gate
        // 4c/4d is ever reached (review_state=rejected, not
        // paper_candidate) -- no real backtest evidence is needed for this
        // negative control.
        "",
    );
    let (status, resp_body) = call(router, post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["disposition"], "evidence_invalid");
    assert!(json["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b.as_str().unwrap().contains("not 'paper_candidate'")));
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn rejected_candidate_cannot_be_approved() {
    let root = temp_dir("rejected_candidate");
    let strategy_id = unique_id("promo_route_rejected");
    let review_dir = write_fixture(&root, vec![rejected_candidate(&strategy_id, "AAPL", "1D")]);
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let router = routes::build_router(st);

    let body = transition_body(
        &strategy_id,
        "AAPL",
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
    );
    let (status, resp_body) = call(router, post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "evidence_invalid");
    assert!(json["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b.as_str().unwrap().contains("not 'paper_candidate'")));
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn watchlist_candidate_cannot_be_approved() {
    let root = temp_dir("watchlist_candidate");
    let strategy_id = unique_id("promo_route_watchlist");
    let review_dir = write_fixture(&root, vec![watchlist_candidate(&strategy_id, "AAPL", "1D")]);
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let router = routes::build_router(st);

    let body = transition_body(
        &strategy_id,
        "AAPL",
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
    );
    let (status, resp_body) = call(router, post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["disposition"], "evidence_invalid");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn missing_decision_cannot_be_approved() {
    let root = temp_dir("missing_decision");
    let strategy_id = unique_id("promo_route_missing");
    // Fixture exists but contains no row for this identity at all.
    let review_dir = write_fixture(
        &root,
        vec![paper_candidate("some_other_strategy", "MSFT", "1D")],
    );
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let router = routes::build_router(st);

    let body = transition_body(
        &strategy_id,
        "AAPL",
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
    );
    let (status, resp_body) = call(router, post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["disposition"], "evidence_invalid");
    assert!(json["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b.as_str().unwrap().contains("no matching evidence row")));
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn duplicate_matching_rows_fail_closed() {
    let root = temp_dir("duplicate_rows");
    let strategy_id = unique_id("promo_route_dup_rows");
    // Two rows for the exact same identity — ambiguous evidence.
    let review_dir = write_fixture(
        &root,
        vec![
            paper_candidate(&strategy_id, "AAPL", "1D"),
            paper_candidate(&strategy_id, "AAPL", "1D"),
        ],
    );
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let router = routes::build_router(st);

    let body = transition_body(
        &strategy_id,
        "AAPL",
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
    );
    let (status, resp_body) = call(router, post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["disposition"], "evidence_invalid");
    assert!(json["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b.as_str().unwrap().contains("ambiguous")));
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn mismatched_symbol_and_timeframe_fail() {
    let root = temp_dir("mismatched_identity_02");
    let strategy_id = unique_id("promo_route_mismatch2");
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, "AAPL", "1D")]);
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    // Wrong symbol.
    let body = transition_body(
        &strategy_id,
        "MSFT",
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
    );
    let (status, resp_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["disposition"], "evidence_invalid");

    // Wrong timeframe (3600 secs = "1H", fixture only has "1D" = 86400).
    let body = transition_body(
        &strategy_id,
        "AAPL",
        3600,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
    );
    let (status, resp_body) = call(
        routes::build_router(st),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["disposition"], "evidence_invalid");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn path_traversal_rejected() {
    let root = temp_dir("path_traversal_root");
    let outside = temp_dir("path_traversal_outside");
    let strategy_id = unique_id("promo_route_traversal");
    // Evidence exists, but OUTSIDE the configured root.
    let review_dir = write_fixture(&outside, vec![paper_candidate(&strategy_id, "AAPL", "1D")]);
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let router = routes::build_router(st);

    let body = transition_body(
        &strategy_id,
        "AAPL",
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
    );
    let (status, resp_body) = call(router, post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["disposition"], "evidence_invalid");
    assert!(json["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b.as_str().unwrap().contains("does not resolve inside")));
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn malformed_manifest_json_fails() {
    let root = temp_dir("malformed_manifest_root");
    let bad_dir = root.join("bad-review");
    std::fs::create_dir_all(&bad_dir).unwrap();
    std::fs::write(bad_dir.join("manifest.json"), "{ not valid json").unwrap();
    std::fs::write(bad_dir.join("review_decisions.json"), "[]").unwrap();
    let strategy_id = unique_id("promo_route_malformed");
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let router = routes::build_router(st);

    let body = transition_body(
        &strategy_id,
        "AAPL",
        86400,
        "shadow_approved",
        Some(bad_dir.to_str().unwrap()),
    );
    let (status, resp_body) = call(router, post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["disposition"], "evidence_invalid");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn no_db_produces_unavailable() {
    let root = temp_dir("no_db_unavailable");
    let strategy_id = unique_id("promo_route_no_db");
    let st = make_state_no_db(&root, state::OperatorAuthMode::ExplicitDevNoToken);
    let router = routes::build_router(st);

    let body = transition_body(&strategy_id, "AAPL", 86400, "shadow_approved", None);
    let (status, resp_body) = call(router, post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let json = parse_json(resp_body);
    assert_eq!(json["disposition"], "unavailable");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn valid_transition_visible_on_read_routes() {
    let root = temp_dir("visible_on_read");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            ..Default::default()
        },
    );
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        &run_id.to_string(),
    );
    let (status, _) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // GET .../promotions/check reflects it.
    let uri = format!(
        "/api/v1/strategy/promotions/check?strategy_id={}&symbol={}&timeframe_secs=86400",
        urlencoding_encode(&strategy_id),
        urlencoding_encode(&symbol)
    );
    let (status, resp_body) = call(routes::build_router(Arc::clone(&st)), get_req(&uri)).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(resp_body);
    assert_eq!(json["truth_state"], "active");
    assert_eq!(json["current_state"], "shadow_approved");
    assert_eq!(json["tradable_paper"], false);
    assert_eq!(json["tradable_live"], false);
    assert_eq!(json["reason_code"], "promotion_shadow_only");

    // GET .../promotions (list) also reflects it.
    let (status, resp_body) = call(
        routes::build_router(st),
        get_req("/api/v1/strategy/promotions"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(resp_body);
    assert_eq!(json["truth_state"], "active");
    let rows = json["rows"].as_array().unwrap();
    assert!(rows
        .iter()
        .any(|r| r["strategy_id"] == strategy_id && r["new_state"] == "shadow_approved"));
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn history_remains_visible_after_later_transition() {
    let root = temp_dir("history_visible");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            ..Default::default()
        },
    );
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    // shadow_approved (requires evidence).
    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        &run_id.to_string(),
    );
    let (status, _) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // paper_approved (no new evidence required for this edge).
    let body = transition_body(&strategy_id, &symbol, 86400, "paper_approved", None);
    let (status, _) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let uri = format!(
        "/api/v1/strategy/promotions/history?strategy_id={}&symbol={}&timeframe_secs=86400",
        urlencoding_encode(&strategy_id),
        urlencoding_encode(&symbol)
    );
    let (status, resp_body) = call(routes::build_router(st), get_req(&uri)).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(resp_body);
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "both transitions must remain in history");
    // Newest first.
    assert_eq!(rows[0]["new_state"], "paper_approved");
    assert_eq!(rows[1]["new_state"], "shadow_approved");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn illegal_transition_creates_no_row() {
    let root = temp_dir("illegal_no_row");
    let strategy_id = unique_id("promo_route_illegal");
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    // no-state -> active_paper is never legal.
    let body = transition_body(&strategy_id, "AAPL", 86400, "active_paper", None);
    let (status, resp_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let json = parse_json(resp_body);
    assert_eq!(json["disposition"], "illegal_transition");

    let uri = format!(
        "/api/v1/strategy/promotions/check?strategy_id={}&symbol=AAPL&timeframe_secs=86400",
        urlencoding_encode(&strategy_id)
    );
    let (_, resp_body) = call(routes::build_router(st), get_req(&uri)).await;
    let json = parse_json(resp_body);
    assert_eq!(
        json["current_state"],
        serde_json::Value::Null,
        "rejected transition must leave no row"
    );
    assert_eq!(json["reason_code"], "promotion_missing");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn duplicate_transition_request_is_idempotent() {
    let root = temp_dir("duplicate_request");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            ..Default::default()
        },
    );
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    // Build one fixed request body (fixed effective_at_utc so the replay is
    // byte-identical, not just logically identical).
    let effective_at = Utc::now().to_rfc3339();
    let body = serde_json::json!({
        "strategy_id": strategy_id,
        "symbol": symbol,
        "timeframe_secs": 86400,
        "target_state": "shadow_approved",
        "review_dir": review_dir.to_str().unwrap(),
        "research_trial_id": research.trial_id,
        "research_evidence_dir": research.evidence_dir.to_str().unwrap(),
        "research_judge_artifact_path": research.judge_path.to_str().unwrap(),
        "backtest_run_id": run_id.to_string(),
        "effective_at_utc": effective_at,
        "expires_at_utc": null,
        "initiated_by": "test-operator",
        "reason": "scenario test",
    });

    let (status1, body1) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body.clone()),
    )
    .await;
    assert_eq!(status1, StatusCode::OK);
    let json1 = parse_json(body1);
    assert_eq!(json1["disposition"], "transitioned");
    let tid1 = json1["transition_id"].as_str().unwrap().to_string();

    let (status2, body2) = call(
        routes::build_router(st),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status2, StatusCode::OK);
    let json2 = parse_json(body2);
    assert_eq!(json2["disposition"], "duplicate");
    assert_eq!(json2["transition_id"].as_str().unwrap(), tid1);
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn tradable_live_always_false() {
    let root = temp_dir("live_always_false");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            ..Default::default()
        },
    );
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        &run_id.to_string(),
    );
    let (status, _) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "real evidence must produce a genuine transition");

    let uri = format!(
        "/api/v1/strategy/promotions/check?strategy_id={}&symbol={}&timeframe_secs=86400",
        urlencoding_encode(&strategy_id),
        urlencoding_encode(&symbol)
    );
    let (_, resp_body) = call(routes::build_router(Arc::clone(&st)), get_req(&uri)).await;
    assert_eq!(parse_json(resp_body)["tradable_live"], false);

    let (_, resp_body) = call(
        routes::build_router(st),
        get_req("/api/v1/strategy/promotions"),
    )
    .await;
    let json = parse_json(resp_body);
    for row in json["rows"].as_array().unwrap() {
        assert_eq!(
            row["tradable_live"], false,
            "tradable_live must always be false"
        );
    }
}

// ---------------------------------------------------------------------------
// PRODUCTION-PROMOTION-DB-E2E-01: negative controls over the REAL Gate
// 4c/4d backtest-evidence path, exercised through the real Axum route with
// a real Postgres pool -- each fixture is genuine production evidence with
// exactly one deliberate defect, never a hand-edited JSON file.
// ---------------------------------------------------------------------------

/// Asserts a transition request was rejected before any row could be
/// committed: BAD_REQUEST/evidence_invalid, and the identity's current
/// state (via the real `/promotions/check` route) is still null.
async fn assert_evidence_rejected_no_row(
    st: Arc<state::AppState>,
    body: serde_json::Value,
    strategy_id: &str,
    symbol: &str,
) {
    let (status, resp_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "evidence_invalid");

    let uri = format!(
        "/api/v1/strategy/promotions/check?strategy_id={}&symbol={}&timeframe_secs=86400",
        urlencoding_encode(strategy_id),
        urlencoding_encode(symbol)
    );
    let (_, resp_body) = call(routes::build_router(st), get_req(&uri)).await;
    assert_eq!(
        parse_json(resp_body)["current_state"],
        serde_json::Value::Null,
        "rejected evidence must leave no promotion row"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn cross_candidate_backtest_evidence_strategy_mismatch_is_rejected() {
    // Real evidence is genuinely produced (engine always runs the real
    // `swing_momentum` plugin, so `report.strategy_name == "swing_momentum"`
    // no matter what `strategy_id` is passed in here) -- but every OTHER
    // fixture (scanner review, research OOS) plus the promotion request
    // itself claims a DIFFERENT strategy_id. The backtest-evidence gate's
    // own cross-candidate check (`backtest_evidence_gate.rs`) must catch
    // this: a real, structurally valid Backtest candidate must never be
    // accepted for a promotion identity it was not produced for.
    let root = temp_dir("cross_candidate_mismatch");
    let strategy_id = unique_id("promo_route_cross_candidate").to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    // Real evidence, but produced by the real engine under its own real
    // name ("swing_momentum"), never under `strategy_id` above.
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            ..Default::default()
        },
    );
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        &run_id.to_string(),
    );
    let (status, resp_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["disposition"], "evidence_invalid");
    assert!(json["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b.as_str().unwrap().contains("cross-candidate")));

    let uri = format!(
        "/api/v1/strategy/promotions/check?strategy_id={}&symbol={}&timeframe_secs=86400",
        urlencoding_encode(&strategy_id),
        urlencoding_encode(&symbol)
    );
    let (_, resp_body) = call(routes::build_router(st), get_req(&uri)).await;
    assert_eq!(
        parse_json(resp_body)["current_state"],
        serde_json::Value::Null,
        "rejected cross-candidate evidence must leave no promotion row"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn missing_backtest_run_id_is_rejected() {
    let root = temp_dir("missing_backtest_run_id");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        "", // no real evidence was ever written for this candidate
    );
    let (status, resp_body) = call(router_with(&st), post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["disposition"], "evidence_invalid");
    assert!(json["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b.as_str().unwrap().contains("backtest_run_id")));
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn missing_backtest_evidence_artifact_root_is_rejected() {
    // Same fully-valid fixtures as the positive-path tests, but the daemon
    // itself has no `MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT` configured --
    // trusted daemon config, never request content, is the only source of
    // authority for the artifact root (`backtest_evidence_gate.rs`).
    let root = temp_dir("missing_artifact_root_config");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            ..Default::default()
        },
    );
    let pool = make_db_pool().await;
    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", &root);
    set_research_evidence_env(&root);
    std::env::remove_var("MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT");
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    // Restore for subsequent tests sharing this process (--test-threads=1).
    set_backtest_evidence_env(&root);

    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        &run_id.to_string(),
    );
    let (status, resp_body) = call(router_with(&st), post_json_req(TRANSITION_ROUTE, None, body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp_body);
    assert_eq!(json["disposition"], "evidence_invalid");
    assert!(json["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b.as_str().unwrap().contains("MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT")));
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn missing_stress_evidence_is_rejected() {
    let root = temp_dir("missing_stress_evidence");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            write_stress: false,
            ..Default::default()
        },
    );
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        &run_id.to_string(),
    );
    assert_evidence_rejected_no_row(st, body, &strategy_id, &symbol).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn missing_p9_robustness_evidence_is_rejected() {
    let root = temp_dir("missing_p9");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            write_p9: false,
            ..Default::default()
        },
    );
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        &run_id.to_string(),
    );
    assert_evidence_rejected_no_row(st, body, &strategy_id, &symbol).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn incomplete_p9_missing_dsr_pbo_sensitivity_is_rejected() {
    // P9 (robustness_gauntlet.json) is written, but the DSR/PBO sensitivity
    // finalize step never runs -- `dsr_pbo_sensitivity` stays a deferred
    // scenario, so P9 is structurally present but NOT complete
    // (`is_complete=false`). `evaluate_promotion` must require full P9
    // completeness, not merely its presence.
    let root = temp_dir("incomplete_p9");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            finalize_sensitivity: false,
            ..Default::default()
        },
    );
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        &run_id.to_string(),
    );
    assert_evidence_rejected_no_row(st, body, &strategy_id, &symbol).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn failed_p9_scenario_is_rejected() {
    // A genuinely bad candidate: `month_and_regime_concentration` really
    // fails (>50% of total gain concentrated in one calendar month) on
    // `bars_that_fail_concentration`. `is_complete=true` (every scenario
    // ran, including a real DSR/PBO sensitivity finalize) but
    // `all_applicable_passed=false` -- `evaluate_promotion` must require
    // both.
    let root = temp_dir("failed_p9_scenario");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: bars_that_fail_concentration(&symbol),
            ..Default::default()
        },
    );
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        &run_id.to_string(),
    );
    assert_evidence_rejected_no_row(st, body, &strategy_id, &symbol).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn artifact_tamper_is_rejected() {
    // Real, genuinely complete evidence -- then one byte of
    // `stress_suite.json` is corrupted after the fact, breaking its
    // recorded hash. `resolve_backtest_evidence`'s hash-chain verification
    // must catch this; a route that only checked file *presence* would not.
    let root = temp_dir("artifact_tamper");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            ..Default::default()
        },
    );
    let stress_path = root.join(run_id.to_string()).join("stress_suite.json");
    let original = std::fs::read_to_string(&stress_path).expect("read stress_suite.json");
    std::fs::write(&stress_path, format!("{original} ")).expect("tamper stress_suite.json");

    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);
    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        &run_id.to_string(),
    );
    assert_evidence_rejected_no_row(st, body, &strategy_id, &symbol).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn dsr_below_threshold_is_rejected() {
    // Genuine end-to-end fixtures, but with the Research judge's own
    // dsr=0.85 fixture value now failing an unusually strict daemon policy
    // threshold override -- proves the ROUTE actually reads and enforces
    // `MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO`, not merely that the judge
    // artifact contains a dsr value.
    let root = temp_dir("dsr_below_threshold");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            ..Default::default()
        },
    );
    let pool = make_db_pool().await;
    // `AppState` captures this threshold ONCE at construction time, and
    // `make_state_with_db` itself unconditionally resets it to the
    // permissive default via `set_research_evidence_env` -- so the override
    // must be applied AFTER that call but BEFORE `AppState` is constructed,
    // not by calling `make_state_with_db` at all. Fixture dsr=0.85 must now
    // fail a 0.99 floor.
    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", &root);
    set_research_evidence_env(&root);
    std::env::set_var("MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO", "0.99");
    set_backtest_evidence_env(&root);
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    std::env::set_var("MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO", "0.0");
    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        &run_id.to_string(),
    );
    assert_evidence_rejected_no_row(st, body, &strategy_id, &symbol).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn pbo_above_threshold_is_rejected() {
    // Same rationale as `dsr_below_threshold_is_rejected`, for
    // `MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING` against the
    // fixture's pbo=0.15.
    let root = temp_dir("pbo_above_threshold");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            ..Default::default()
        },
    );
    let pool = make_db_pool().await;
    // Same rationale as `dsr_below_threshold_is_rejected`: bypass
    // `make_state_with_db` (which unconditionally resets this var to its
    // permissive default) and apply the override directly before
    // constructing `AppState`.
    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", &root);
    set_research_evidence_env(&root);
    std::env::set_var("MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING", "0.01");
    set_backtest_evidence_env(&root);
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    std::env::set_var("MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING", "1.0");
    let body = transition_body_with_research(
        &strategy_id,
        &symbol,
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
        &run_id.to_string(),
    );
    assert_evidence_rejected_no_row(st, body, &strategy_id, &symbol).await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn duplicate_retry_with_mismatched_backtest_run_id_is_rejected() {
    // PRODUCTION-PROMOTION-DB-E2E-01: a retry that is byte-identical to an
    // already-accepted request except for a DIFFERENT (never-validated)
    // `backtest_run_id` must never be short-circuited as
    // `disposition: "duplicate"` -- that would silently skip Gate 4c/4d
    // re-validation of the second request's own claimed evidence. Proves
    // the `backtest_run_id` fix to the deterministic `transition_id` seed
    // in `routes/strategy_promotions.rs`.
    let root = temp_dir("duplicate_mismatched_run_id");
    let strategy_id = "swing_momentum".to_string();
    let symbol = unique_id("SYM").to_uppercase();
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, &symbol, "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let run_id = write_real_backtest_evidence(
        &root,
        &research.trial_id,
        &research.registry_db_path,
        &symbol,
        RealEvidenceOptions {
            bars: smooth_uptrend_bars(&symbol),
            ..Default::default()
        },
    );
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    let effective_at = Utc::now().to_rfc3339();
    let mut body = serde_json::json!({
        "strategy_id": strategy_id,
        "symbol": symbol,
        "timeframe_secs": 86400,
        "target_state": "shadow_approved",
        "review_dir": review_dir.to_str().unwrap(),
        "research_trial_id": research.trial_id,
        "research_evidence_dir": research.evidence_dir.to_str().unwrap(),
        "research_judge_artifact_path": research.judge_path.to_str().unwrap(),
        "backtest_run_id": run_id.to_string(),
        "effective_at_utc": effective_at,
        "expires_at_utc": null,
        "initiated_by": "test-operator",
        "reason": "scenario test",
    });

    let (status1, body1) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body.clone()),
    )
    .await;
    assert_eq!(status1, StatusCode::OK);
    let json1 = parse_json(body1);
    assert_eq!(json1["disposition"], "transitioned");
    let tid1 = json1["transition_id"].as_str().unwrap().to_string();

    // Same request, but a syntactically well-formed, never-registered
    // `backtest_run_id` -- everything else (including `effective_at_utc`)
    // byte-identical. Because `backtest_run_id` is now part of the
    // deterministic `transition_id` seed, this computes a DIFFERENT
    // `transition_id` than the first call, so Gate 1b's idempotency
    // short-circuit does not fire; the request is independently re-decided
    // from Gate 2 onward against the identity's now-current state
    // (shadow_approved), which correctly rejects a second
    // shadow_approved->shadow_approved request as `illegal_transition`
    // regardless of evidence -- the exact outcome this test exists to
    // prove is absent: it must NEVER be silently accepted as
    // `disposition: "duplicate"` carrying the first call's already-verified
    // lineage.
    body["backtest_run_id"] = serde_json::Value::String(Uuid::new_v4().to_string());
    let (status2, body2) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_ne!(status2, StatusCode::OK, "a mismatched-lineage retry must never succeed");
    let json2 = parse_json(body2);
    assert_ne!(
        json2["disposition"], "duplicate",
        "a mismatched-lineage retry must never be reported as a duplicate of the original request"
    );
    assert_ne!(
        json2["transition_id"].as_str(),
        Some(tid1.as_str()),
        "a rejected mismatched-lineage retry must never claim the original transition_id"
    );

    // No contradictory second row: history for this identity still shows
    // exactly the one, originally-verified transition.
    let uri = format!(
        "/api/v1/strategy/promotions/history?strategy_id={}&symbol={}&timeframe_secs=86400",
        urlencoding_encode(&strategy_id),
        urlencoding_encode(&symbol)
    );
    let (_, resp_body) = call(routes::build_router(st), get_req(&uri)).await;
    let history = parse_json(resp_body);
    let rows = history["rows"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        1,
        "a rejected mismatched-lineage retry must never create a second row"
    );
    assert_eq!(rows[0]["transition_id"].as_str().unwrap(), tid1);
}
