//! STRATEGY-SCANNER-PROMOTION-01D: Daemon strategy scan review-artifact
//! readback proof tests.
//!
//! Every fixture is hand-constructed in-memory and written to a fresh OS
//! temp directory via `mqk_backtest::write_review_artifacts` (the same
//! function the CLI's `mqk backtest review-scan` command calls) — no test
//! opens a database connection, and no test makes a provider/broker/network
//! call. Tests mutate `MQK_STRATEGY_REVIEW_ARTIFACT_ROOT` process-wide, so
//! this file must run with `--test-threads=1`.
//!
//! # Proof matrix
//!
//! | Test                                       | What it proves                                              |
//! |---------------------------------------------|---------------------------------------------------------------|
//! | review_artifact_active_for_valid_directory  | valid review artifact -> active, manifest/summary/decisions populated |
//! | review_artifact_missing_directory           | dir under root that was never created -> missing_artifact     |
//! | review_artifact_path_escape_rejected        | dir outside the configured root -> path_rejected               |
//! | review_artifact_invalid_json                | manifest.json malformed -> invalid_artifact                    |
//! | review_artifact_decisions_capped            | >200 decisions in the artifact -> response capped at 200        |
//! | review_artifact_requires_query_param        | no review_dir query param -> 400                                |
//! | review_artifact_warnings_present             | every response (any truth_state) carries the fixed research-only warning set |

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_backtest::{
    write_review_artifacts, ReviewManifest, ReviewRunOutput, ReviewSummary,
    StrategyScanReviewDecision, StrategyScanReviewState,
};
use mqk_daemon::{routes, state};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mqk_daemon_strategy_scan_review_artifact_{label}_{}",
        Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn sample_decisions(count: usize) -> Vec<StrategyScanReviewDecision> {
    let mut decisions = vec![
        StrategyScanReviewDecision {
            symbol: "AAPL".to_string(),
            timeframe: "1D".to_string(),
            strategy_id: "swing_momentum".to_string(),
            scanner_rank: Some(1),
            scanner_score: Some(8.5),
            review_state: StrategyScanReviewState::PaperCandidate,
            reason_codes: vec!["eligible_paper_candidate".to_string()],
            blockers: Vec::new(),
            warnings: Vec::new(),
        },
        StrategyScanReviewDecision {
            symbol: "MSFT".to_string(),
            timeframe: "1D".to_string(),
            strategy_id: "swing_momentum".to_string(),
            scanner_rank: Some(2),
            scanner_score: Some(6.0),
            review_state: StrategyScanReviewState::Rejected,
            reason_codes: vec!["negative_total_return".to_string()],
            blockers: vec!["total_return_pct is negative".to_string()],
            warnings: Vec::new(),
        },
    ];
    let mut i = decisions.len();
    while decisions.len() < count {
        decisions.push(StrategyScanReviewDecision {
            symbol: format!("SYM{i}"),
            timeframe: "1D".to_string(),
            strategy_id: "swing_momentum".to_string(),
            scanner_rank: Some(i + 1),
            scanner_score: Some(1.0),
            review_state: StrategyScanReviewState::Blocked,
            reason_codes: vec!["missing_score".to_string()],
            blockers: vec!["scanner score is missing".to_string()],
            warnings: Vec::new(),
        });
        i += 1;
    }
    decisions
}

/// Builds a `ReviewRunOutput` directly (no dependency on a real scanner
/// artifact or the scan pipeline) and writes it via the same
/// `mqk_backtest::write_review_artifacts` function the CLI uses. Returns the
/// created `{out_dir}/{review_id}/` directory.
fn write_fixture_review_artifact(out_dir: &Path, decision_count: usize) -> PathBuf {
    let decisions = sample_decisions(decision_count);
    let paper_candidate_count = decisions
        .iter()
        .filter(|d| d.review_state == StrategyScanReviewState::PaperCandidate)
        .count();
    let rejected_count = decisions
        .iter()
        .filter(|d| d.review_state == StrategyScanReviewState::Rejected)
        .count();
    let blocked_count = decisions
        .iter()
        .filter(|d| d.review_state == StrategyScanReviewState::Blocked)
        .count();

    let review_id = Uuid::new_v4();
    let scanner_scan_id = "11111111-1111-1111-1111-111111111111".to_string();
    let fixed_warnings = vec![
        "Scanner ranking is research evidence only.".to_string(),
        "Scanner output is not autonomous trading approval.".to_string(),
        "promotion-ready is not trading-approved.".to_string(),
    ];

    let manifest = ReviewManifest {
        schema_version: 1,
        review_id: review_id.to_string(),
        scanner_scan_id: scanner_scan_id.clone(),
        source_artifact_dir: "fixture-source-not-on-disk".to_string(),
        created_at_utc: "2026-07-01T00:00:00Z".to_string(),
        git_hash: "test".to_string(),
        policy_min_bars_used: 252,
        policy_min_trade_count: 5,
        policy_min_total_return_pct: 0.0,
        policy_min_alpha_pct: 0.0,
        policy_max_drawdown_pct: 25.0,
        policy_min_profit_factor: 1.05,
        candidate_count: decisions.len(),
        blocked_count,
        needs_review_count: 0,
        watchlist_candidate_count: 0,
        paper_candidate_count,
        rejected_count,
        blockers: Vec::new(),
        warnings: fixed_warnings.clone(),
    };

    let top_paper_candidates: Vec<_> = decisions
        .iter()
        .filter(|d| d.review_state == StrategyScanReviewState::PaperCandidate)
        .cloned()
        .collect();

    let summary = ReviewSummary {
        scanner_scan_id,
        review_id: review_id.to_string(),
        candidate_count: decisions.len(),
        blocked_count,
        needs_review_count: 0,
        watchlist_candidate_count: 0,
        paper_candidate_count,
        rejected_count,
        top_paper_candidates,
        top_watchlist_candidates: Vec::new(),
        blockers: Vec::new(),
        warnings: fixed_warnings,
    };

    let output = ReviewRunOutput {
        review_id,
        manifest,
        decisions,
        summary,
    };

    write_review_artifacts(out_dir, &output).expect("write fixture review artifacts")
}

/// Sets `MQK_STRATEGY_REVIEW_ARTIFACT_ROOT` to `root` before constructing
/// state, so the review artifact route's root confinement matches this
/// fixture. Process-global -- callers must run this test file with
/// `--test-threads=1`.
fn make_state(root: &Path) -> Arc<state::AppState> {
    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", root);
    Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ))
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

// Minimal percent-encoding for query values (mirrors the sibling scanner
// artifact test file's own helper -- avoids a new crate dependency).
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn review_artifact_active_for_valid_directory() {
    let root = temp_dir("valid_root");
    let review_dir = write_fixture_review_artifact(&root, 2);
    let st = make_state(&root);
    let router = routes::build_router(st);

    let uri = format!(
        "/api/v1/strategy-scans/review-artifact?review_dir={}",
        urlencoding_encode(review_dir.to_str().unwrap())
    );
    let (status, body) = call(router, get_req(&uri)).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "active");
    assert!(json["manifest"].is_object());
    assert!(json["summary"].is_object());
    let decisions = json["decisions"].as_array().expect("decisions array present");
    assert_eq!(decisions.len(), 2);

    let top_paper = json["top_paper_candidates"].as_array().unwrap();
    assert_eq!(top_paper.len(), 1);
    assert_eq!(top_paper[0]["symbol"], "AAPL");
    assert_eq!(top_paper[0]["review_state"], "paper_candidate");
}

#[tokio::test]
async fn review_artifact_missing_directory() {
    let root = temp_dir("missing_root");
    std::fs::create_dir_all(&root).unwrap();
    let st = make_state(&root);
    let router = routes::build_router(st);

    let never_created = root.join("00000000-0000-0000-0000-000000000000");
    let uri = format!(
        "/api/v1/strategy-scans/review-artifact?review_dir={}",
        urlencoding_encode(never_created.to_str().unwrap())
    );
    let (status, body) = call(router, get_req(&uri)).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "missing_artifact");
    assert!(json["manifest"].is_null());
}

#[tokio::test]
async fn review_artifact_path_escape_rejected() {
    let root = temp_dir("escape_root");
    std::fs::create_dir_all(&root).unwrap();
    let st = make_state(&root);
    let router = routes::build_router(Arc::clone(&st));

    // A directory that exists but sits outside the configured root.
    let outside = temp_dir("escape_outside");
    let uri = format!(
        "/api/v1/strategy-scans/review-artifact?review_dir={}",
        urlencoding_encode(outside.to_str().unwrap())
    );
    let (status, body) = call(router, get_req(&uri)).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "path_rejected");
}

#[tokio::test]
async fn review_artifact_invalid_json() {
    let root = temp_dir("invalid_root");
    let bad_dir = root.join("bad-review");
    std::fs::create_dir_all(&bad_dir).unwrap();
    std::fs::write(bad_dir.join("manifest.json"), "{ not valid json").unwrap();
    std::fs::write(bad_dir.join("summary.json"), "{}").unwrap();
    std::fs::write(bad_dir.join("review_decisions.json"), "[]").unwrap();

    let st = make_state(&root);
    let router = routes::build_router(st);
    let uri = format!(
        "/api/v1/strategy-scans/review-artifact?review_dir={}",
        urlencoding_encode(bad_dir.to_str().unwrap())
    );
    let (status, body) = call(router, get_req(&uri)).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "invalid_artifact");
}

#[tokio::test]
async fn review_artifact_decisions_capped() {
    let root = temp_dir("capped_root");
    let review_dir = write_fixture_review_artifact(&root, 300);
    let st = make_state(&root);
    let router = routes::build_router(st);

    let uri = format!(
        "/api/v1/strategy-scans/review-artifact?review_dir={}",
        urlencoding_encode(review_dir.to_str().unwrap())
    );
    let (status, body) = call(router, get_req(&uri)).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "active");
    let decisions = json["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 200, "decisions must be capped at 200 rows");
}

#[tokio::test]
async fn review_artifact_requires_query_param() {
    let root = temp_dir("no_param_root");
    std::fs::create_dir_all(&root).unwrap();
    let st = make_state(&root);
    let router = routes::build_router(st);
    let (status, body) = call(router, get_req("/api/v1/strategy-scans/review-artifact")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "path_rejected");
}

#[tokio::test]
async fn review_artifact_warnings_present() {
    let root = temp_dir("warnings_root");
    std::fs::create_dir_all(&root).unwrap();
    let st = make_state(&root);
    let router = routes::build_router(st);
    let (_, body) = call(router, get_req("/api/v1/strategy-scans/review-artifact")).await;
    let json = parse_json(body);
    let warnings: Vec<&str> = json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    assert!(warnings.iter().any(|w| w.contains("research evidence only")));
    assert!(warnings.iter().any(|w| w.contains("not autonomous trading approval")));
}
