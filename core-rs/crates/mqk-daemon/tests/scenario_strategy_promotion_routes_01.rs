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
    #[allow(dead_code)]
    registry_db_path: PathBuf,
    evidence_dir: PathBuf,
    judge_path: PathBuf,
    trial_id: String,
}

fn write_research_evidence_fixture(root: &Path, seed: &str) -> ResearchEvidenceFixture {
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

    let registry_db_path = root.join("research_registry.sqlite3");
    let conn = rusqlite::Connection::open(&registry_db_path).expect("open registry db");
    conn.execute_batch(
        "
        create table if not exists research_trials (
            trial_id text primary key,
            experiment_id text not null,
            hypothesis_id text not null,
            strategy_id text not null
        );
        create table if not exists research_attempts (
            attempt_id text primary key,
            trial_id text not null,
            status text not null,
            result_id text
        );
        create table if not exists research_judge_artifacts (
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
        rusqlite::params![trial_id, experiment_id, format!("hyp_{seed}"), seed],
    )
    .expect("insert research_trials row");
    conn.execute(
        "insert into research_attempts (attempt_id, trial_id, status, result_id) \
         values (?1, ?2, 'succeeded', ?3)",
        rusqlite::params![format!("{trial_id}:att0001"), trial_id, economic_eval_id],
    )
    .expect("insert research_attempts row");
    conn.execute(
        "insert into research_judge_artifacts \
         (judge_artifact_sha256, judge_id, experiment_id, hypothesis_id, canonical_judge_json) \
         values (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            judge_sha,
            format!("judge_{seed}"),
            experiment_id,
            Option::<String>::None,
            judge_json,
        ],
    )
    .expect("insert research_judge_artifacts row");
    drop(conn);

    ResearchEvidenceFixture {
        registry_db_path,
        evidence_dir,
        judge_path,
        trial_id,
    }
}

fn make_state_no_db(root: &Path, auth: state::OperatorAuthMode) -> Arc<state::AppState> {
    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", root);
    set_research_evidence_env(root);
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

/// PROMOTION-WALKFORWARD-GATE-WIRING-01: like `transition_body`, but also
/// carries the three P7C research-evidence fields required for any
/// evidence-requiring transition to actually succeed (Gate 4c). Tests that
/// only exercise Gate 4 (scanner evidence) rejection paths never reach Gate
/// 4c and keep using plain `transition_body`.
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
    let root = temp_dir("valid_paper_candidate");
    let strategy_id = unique_id("promo_route_valid");
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, "AAPL", "1D")]);
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
    let strategy_id = unique_id("promo_route_readback");
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, "AAPL", "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    let body = transition_body_with_research(
        &strategy_id,
        "AAPL",
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
    );
    let (status, _) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // GET .../promotions/check reflects it.
    let uri = format!(
        "/api/v1/strategy/promotions/check?strategy_id={}&symbol=AAPL&timeframe_secs=86400",
        urlencoding_encode(&strategy_id)
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
    let strategy_id = unique_id("promo_route_history");
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, "AAPL", "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    // shadow_approved (requires evidence).
    let body = transition_body_with_research(
        &strategy_id,
        "AAPL",
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
    );
    let (status, _) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // paper_approved (no new evidence required for this edge).
    let body = transition_body(&strategy_id, "AAPL", 86400, "paper_approved", None);
    let (status, _) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let uri = format!(
        "/api/v1/strategy/promotions/history?strategy_id={}&symbol=AAPL&timeframe_secs=86400",
        urlencoding_encode(&strategy_id)
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
    let strategy_id = unique_id("promo_route_replay");
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, "AAPL", "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    // Build one fixed request body (fixed effective_at_utc so the replay is
    // byte-identical, not just logically identical).
    let effective_at = Utc::now().to_rfc3339();
    let body = serde_json::json!({
        "strategy_id": strategy_id,
        "symbol": "AAPL",
        "timeframe_secs": 86400,
        "target_state": "shadow_approved",
        "review_dir": review_dir.to_str().unwrap(),
        "research_trial_id": research.trial_id,
        "research_evidence_dir": research.evidence_dir.to_str().unwrap(),
        "research_judge_artifact_path": research.judge_path.to_str().unwrap(),
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
    let strategy_id = unique_id("promo_route_live_check");
    let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, "AAPL", "1D")]);
    let research = write_research_evidence_fixture(&root, &strategy_id);
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    let body = transition_body_with_research(
        &strategy_id,
        "AAPL",
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        &research.trial_id,
        research.evidence_dir.to_str().unwrap(),
        research.judge_path.to_str().unwrap(),
    );
    call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, None, body),
    )
    .await;

    let uri = format!(
        "/api/v1/strategy/promotions/check?strategy_id={}&symbol=AAPL&timeframe_secs=86400",
        urlencoding_encode(&strategy_id)
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
