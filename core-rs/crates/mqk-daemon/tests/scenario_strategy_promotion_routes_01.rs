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
//! | valid_paper_candidate_creates_first_transition     | matching paper_candidate evidence -> transitioned |
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

fn make_state_no_db(root: &Path, auth: state::OperatorAuthMode) -> Arc<state::AppState> {
    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", root);
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
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    let body = transition_body(
        &strategy_id,
        "AAPL",
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
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
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    // shadow_approved (requires evidence).
    let body = transition_body(
        &strategy_id,
        "AAPL",
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
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
    let pool = make_db_pool().await;
    let st = make_state_with_db(&root, pool, state::OperatorAuthMode::ExplicitDevNoToken);

    let body = transition_body(
        &strategy_id,
        "AAPL",
        86400,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
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
