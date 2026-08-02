//! STRATEGY-SCANNER-JOBS-GUI-01B/01C: Daemon strategy scanner job + artifact
//! readback proof tests.
//!
//! Every fixture is a tiny, hand-written local file under a fresh temp
//! directory — no test depends on the repo-root `exports/` tree, no test
//! opens a database connection, and no test makes a provider/broker/network
//! call. Tests mutate `MQK_STRATEGY_SCAN_ARTIFACT_ROOT` process-wide, so
//! this file must run with `--test-threads=1` (see the mission's Phase B/C
//! validation command).
//!
//! # Proof matrix
//!
//! | Test                                              | What it proves                                            |
//! |----------------------------------------------------|-----------------------------------------------------------|
//! | submit_valid_scan_job_completes_and_writes_artifacts | fixture scan completes, all 4 artifact files written, ranked/skipped counts honest |
//! | job_list_and_status_return_submitted_job           | GET jobs list + GET job status both surface the submitted job |
//! | unknown_job_id_returns_404_not_found                | GET unknown job_id -> 404, truth_state=not_found          |
//! | blank_timeframe_rejected                            | POST blank timeframe -> 400, accepted=false                |
//! | blank_strategy_rejected                             | POST blank strategy -> 400, accepted=false                 |
//! | top_out_of_bounds_rejected                          | POST top=0 and top=101 -> 400                              |
//! | limit_symbols_out_of_bounds_rejected                | POST limit_symbols=0 and 201 -> 400                        |
//! | no_db_configured_for_scan_job                       | AppState.db is None; job still completes (no DB required)  |
//! | artifact_route_active_for_completed_job             | GET artifact for a completed job's dir -> active, matches summary |
//! | artifact_route_missing_directory                    | GET artifact for a dir under root that was never created -> missing_artifact |
//! | artifact_route_path_escape_rejected                 | GET artifact for a dir outside the configured root -> path_rejected |
//! | artifact_route_invalid_json                         | GET artifact where manifest.json is malformed -> invalid_artifact |
//! | artifact_route_requires_query_param                 | GET artifact with no artifact_dir -> 400                    |
//! | artifact_route_warnings_present                     | every artifact response (any truth_state) carries the fixed research-only warning set |

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mqk_daemon_strategy_scan_jobs_{label}_{}",
        Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn daily_bars_csv(symbol: &str, count: usize) -> String {
    let mut out = String::from(
        "symbol,timeframe,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n",
    );
    let start_ts: i64 = 1_700_000_000;
    for i in 0..count {
        let end_ts = start_ts + (i as i64) * 86_400;
        let base = 100_000_000i64 + (i as i64) * 100_000;
        let wiggle = if i % 2 == 0 { 50_000 } else { -50_000 };
        let open = base;
        let close = base + wiggle;
        let high = open.max(close) + 20_000;
        let low = open.min(close) - 20_000;
        out.push_str(&format!(
            "{symbol},1D,{end_ts},{open},{high},{low},{close},1000000,t\n"
        ));
    }
    out
}

fn registry_json(symbols: &[&str]) -> String {
    let entries: Vec<String> = symbols
        .iter()
        .map(|s| {
            format!(
                r#"{{"instrument_id":"equity:US:{s}","symbol":"{s}","asset_class":"equity","provider":"twelvedata","provider_symbol":"{s}","venue":"NASDAQ","currency":"USD","enabled":true,"timeframes":["1D"],"notes":"fixture"}}"#
            )
        })
        .collect();
    format!("[{}]", entries.join(","))
}

/// Fixture layout: registry with AAPL, MSFT, NODATA (all enabled); local
/// `1D/AAPL_1D.csv` and `1D/MSFT_1D.csv` (80 bars each); `NODATA` has no
/// bars file at all. `out_dir` is pre-created (empty) so the artifact
/// route's root-confinement canonicalization always succeeds regardless of
/// test ordering.
struct Fixture {
    _dir: PathBuf,
    registry_path: PathBuf,
    bars_root: PathBuf,
    out_dir: PathBuf,
}

fn build_fixture() -> Fixture {
    let dir = temp_dir("fixture");
    let registry_path = dir.join("equities.json");
    std::fs::write(&registry_path, registry_json(&["AAPL", "MSFT", "NODATA"]))
        .expect("write registry fixture");

    let bars_root = dir.join("md_backup");
    let tf_dir = bars_root.join("1D");
    std::fs::create_dir_all(&tf_dir).expect("create 1D bars dir");
    std::fs::write(tf_dir.join("AAPL_1D.csv"), daily_bars_csv("AAPL", 80))
        .expect("write AAPL bars");
    std::fs::write(tf_dir.join("MSFT_1D.csv"), daily_bars_csv("MSFT", 80))
        .expect("write MSFT bars");
    // NODATA intentionally has no bars file.

    let out_dir = dir.join("strategy_scans");
    std::fs::create_dir_all(&out_dir).expect("create out_dir");

    Fixture {
        _dir: dir,
        registry_path,
        bars_root,
        out_dir,
    }
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

/// Sets `MQK_STRATEGY_SCAN_ARTIFACT_ROOT` to `fx.out_dir` before constructing
/// state, so the artifact route's root confinement matches this fixture.
/// Process-global — callers must run this test file with `--test-threads=1`.
fn make_state(fx: &Fixture) -> Arc<state::AppState> {
    std::env::set_var("MQK_STRATEGY_SCAN_ARTIFACT_ROOT", &fx.out_dir);
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

fn post_json_body(uri: &str, payload: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&payload).unwrap(),
        ))
        .unwrap()
}

fn get_req(uri: &str) -> Request<axum::body::Body> {
    Request::builder()
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

async fn submit_scan(
    st: &Arc<state::AppState>,
    fx: &Fixture,
    timeframe: &str,
    strategy: &str,
) -> serde_json::Value {
    let router = routes::build_router(Arc::clone(st));
    let payload = serde_json::json!({
        "registry_path": fx.registry_path.to_str().unwrap(),
        "bars_root": fx.bars_root.to_str().unwrap(),
        "timeframe": timeframe,
        "strategy": strategy,
        "top": 20,
        "out_dir": fx.out_dir.to_str().unwrap(),
    });
    let (status, body) = call(
        router,
        post_json_body("/api/v1/strategy-scans/jobs", payload),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "submit failed: {:?}",
        parse_json(body.clone())
    );
    parse_json(body)
}

async fn wait_for_terminal_job(st: Arc<state::AppState>, job_id: &str) -> serde_json::Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let router = routes::build_router(Arc::clone(&st));
        let (status, body) = call(
            router,
            get_req(&format!("/api/v1/strategy-scans/jobs/{}", job_id)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let json = parse_json(body);
        let job_status = json["status"].as_str().unwrap();
        if job_status == "completed" || job_status == "failed" {
            return json;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "job did not complete within 10 seconds (last status: {job_status})"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase B: job submit / list / status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn submit_valid_scan_job_completes_and_writes_artifacts() {
    let fx = build_fixture();
    let st = make_state(&fx);

    let accepted = submit_scan(&st, &fx, "1D", "swing_momentum").await;
    assert!(accepted["accepted"].as_bool().unwrap());
    let job_id = accepted["job_id"].as_str().unwrap().to_string();
    assert_ne!(job_id, Uuid::nil().to_string());

    let status = wait_for_terminal_job(Arc::clone(&st), &job_id).await;
    assert_eq!(
        status["status"], "completed",
        "job did not complete: {status}"
    );

    let artifact_dir = status["artifact_dir"]
        .as_str()
        .expect("completed job must carry artifact_dir");
    let dir = std::path::Path::new(artifact_dir);
    for file in [
        "manifest.json",
        "candidates.json",
        "candidates.csv",
        "summary.json",
    ] {
        assert!(
            dir.join(file).is_file(),
            "expected artifact file missing: {}",
            dir.join(file).display()
        );
    }

    let summary = status["summary"]
        .as_object()
        .expect("summary present on completion");
    assert_eq!(summary["universe_count"], 3);
    assert_eq!(summary["ranked_count"], 2);
    assert_eq!(summary["skipped_count"], 1);

    // Fixed research-only warning set must be present on every completed job.
    let warnings: Vec<&str> = status["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    assert!(warnings
        .iter()
        .any(|w| w.contains("research evidence only")));
    assert!(warnings
        .iter()
        .any(|w| w.contains("not autonomous trading approval")));
    assert!(warnings
        .iter()
        .any(|w| w.contains("negative absolute returns")));
}

#[tokio::test]
async fn job_list_and_status_return_submitted_job() {
    let fx = build_fixture();
    let st = make_state(&fx);

    let accepted = submit_scan(&st, &fx, "1D", "swing_momentum").await;
    let job_id = accepted["job_id"].as_str().unwrap().to_string();
    let _ = wait_for_terminal_job(Arc::clone(&st), &job_id).await;

    let router = routes::build_router(Arc::clone(&st));
    let (status, body) = call(router, get_req("/api/v1/strategy-scans/jobs")).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "active");
    let jobs = json["jobs"].as_array().unwrap();
    assert!(
        jobs.iter().any(|j| j["job_id"] == job_id),
        "submitted job not found in jobs list: {jobs:?}"
    );

    let router2 = routes::build_router(Arc::clone(&st));
    let (status2, body2) = call(
        router2,
        get_req(&format!("/api/v1/strategy-scans/jobs/{job_id}")),
    )
    .await;
    assert_eq!(status2, StatusCode::OK);
    let json2 = parse_json(body2);
    assert_eq!(json2["job_id"], job_id);
    assert_eq!(json2["request"]["timeframe"], "1D");
    assert_eq!(json2["request"]["strategy"], "swing_momentum");
}

#[tokio::test]
async fn unknown_job_id_returns_404_not_found() {
    let fx = build_fixture();
    let st = make_state(&fx);
    let router = routes::build_router(st);
    let unknown = Uuid::new_v4();
    let (status, body) = call(
        router,
        get_req(&format!("/api/v1/strategy-scans/jobs/{unknown}")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "not_found");
}

#[tokio::test]
async fn blank_timeframe_rejected() {
    let fx = build_fixture();
    let st = make_state(&fx);
    let router = routes::build_router(Arc::clone(&st));
    let payload = serde_json::json!({
        "registry_path": fx.registry_path.to_str().unwrap(),
        "bars_root": fx.bars_root.to_str().unwrap(),
        "timeframe": "   ",
        "strategy": "swing_momentum",
        "top": 20,
        "out_dir": fx.out_dir.to_str().unwrap(),
    });
    let (status, body) = call(
        router,
        post_json_body("/api/v1/strategy-scans/jobs", payload),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(body);
    assert!(!json["accepted"].as_bool().unwrap());
}

#[tokio::test]
async fn blank_strategy_rejected() {
    let fx = build_fixture();
    let st = make_state(&fx);
    let router = routes::build_router(Arc::clone(&st));
    let payload = serde_json::json!({
        "registry_path": fx.registry_path.to_str().unwrap(),
        "bars_root": fx.bars_root.to_str().unwrap(),
        "timeframe": "1D",
        "strategy": "",
        "top": 20,
        "out_dir": fx.out_dir.to_str().unwrap(),
    });
    let (status, body) = call(
        router,
        post_json_body("/api/v1/strategy-scans/jobs", payload),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(body);
    assert!(!json["accepted"].as_bool().unwrap());
}

#[tokio::test]
async fn top_out_of_bounds_rejected() {
    let fx = build_fixture();
    let st = make_state(&fx);
    for top in [0, 101] {
        let router = routes::build_router(Arc::clone(&st));
        let payload = serde_json::json!({
            "timeframe": "1D",
            "strategy": "swing_momentum",
            "top": top,
        });
        let (status, _) = call(
            router,
            post_json_body("/api/v1/strategy-scans/jobs", payload),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "top={top} should be rejected"
        );
    }
}

#[tokio::test]
async fn limit_symbols_out_of_bounds_rejected() {
    let fx = build_fixture();
    let st = make_state(&fx);
    for limit in [0, 201] {
        let router = routes::build_router(Arc::clone(&st));
        let payload = serde_json::json!({
            "timeframe": "1D",
            "strategy": "swing_momentum",
            "top": 20,
            "limit_symbols": limit,
        });
        let (status, _) = call(
            router,
            post_json_body("/api/v1/strategy-scans/jobs", payload),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "limit_symbols={limit} should be rejected"
        );
    }
}

#[tokio::test]
async fn no_db_configured_for_scan_job() {
    let fx = build_fixture();
    let st = make_state(&fx);
    assert!(
        st.db.is_none(),
        "test AppState must not have a DB pool configured"
    );

    let accepted = submit_scan(&st, &fx, "1D", "swing_momentum").await;
    let job_id = accepted["job_id"].as_str().unwrap().to_string();
    let status = wait_for_terminal_job(Arc::clone(&st), &job_id).await;
    assert_eq!(
        status["status"], "completed",
        "scan job must complete without a DB pool"
    );
}

// ---------------------------------------------------------------------------
// Phase C: read-only artifact readback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn artifact_route_active_for_completed_job() {
    let fx = build_fixture();
    let st = make_state(&fx);

    let accepted = submit_scan(&st, &fx, "1D", "swing_momentum").await;
    let job_id = accepted["job_id"].as_str().unwrap().to_string();
    let job_status = wait_for_terminal_job(Arc::clone(&st), &job_id).await;
    let artifact_dir = job_status["artifact_dir"].as_str().unwrap().to_string();

    let router = routes::build_router(Arc::clone(&st));
    let uri = format!(
        "/api/v1/strategy-scans/artifact?artifact_dir={}",
        urlencoding_encode(&artifact_dir)
    );
    let (status, body) = call(router, get_req(&uri)).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "active");
    assert!(json["manifest"].is_object());
    assert!(json["summary"].is_object());
    let candidates = json["candidates"]
        .as_array()
        .expect("candidates array present");
    assert_eq!(candidates.len(), 3);

    let top_candidates = json["top_candidates"].as_array().unwrap();
    assert_eq!(top_candidates.len(), 2, "both AAPL and MSFT should rank");
    assert_eq!(top_candidates[0]["rank"], 1);
    assert_eq!(top_candidates[1]["rank"], 2);
    let score0 = top_candidates[0]["score"].as_f64().unwrap();
    let score1 = top_candidates[1]["score"].as_f64().unwrap();
    assert!(
        score0 >= score1,
        "top_candidates must be in descending score order"
    );

    let skip_reasons = json["skip_reasons"].as_array().unwrap();
    assert!(
        skip_reasons
            .iter()
            .any(|r| r["reason_code"] == "missing_bars_file"),
        "NODATA's missing_bars_file skip reason must be summarized: {skip_reasons:?}"
    );
}

#[tokio::test]
async fn artifact_route_missing_directory() {
    let fx = build_fixture();
    let st = make_state(&fx);
    let router = routes::build_router(st);

    let never_created = fx.out_dir.join("00000000-0000-0000-0000-000000000000");
    let uri = format!(
        "/api/v1/strategy-scans/artifact?artifact_dir={}",
        urlencoding_encode(never_created.to_str().unwrap())
    );
    let (status, body) = call(router, get_req(&uri)).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "missing_artifact");
    assert!(json["manifest"].is_null());
}

#[tokio::test]
async fn artifact_route_path_escape_rejected() {
    let fx = build_fixture();
    let st = make_state(&fx);
    let router = routes::build_router(Arc::clone(&st));

    // A directory that exists but sits outside the configured root.
    let outside = fx._dir.join("evil_outside_root");
    std::fs::create_dir_all(&outside).unwrap();

    let uri = format!(
        "/api/v1/strategy-scans/artifact?artifact_dir={}",
        urlencoding_encode(outside.to_str().unwrap())
    );
    let (status, body) = call(router, get_req(&uri)).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "path_rejected");
}

#[tokio::test]
async fn artifact_route_invalid_json() {
    let fx = build_fixture();
    let st = make_state(&fx);

    let bad_dir = fx.out_dir.join("bad-scan");
    std::fs::create_dir_all(&bad_dir).unwrap();
    std::fs::write(bad_dir.join("manifest.json"), "{ not valid json").unwrap();
    std::fs::write(bad_dir.join("summary.json"), "{}").unwrap();
    std::fs::write(bad_dir.join("candidates.json"), "[]").unwrap();

    let router = routes::build_router(st);
    let uri = format!(
        "/api/v1/strategy-scans/artifact?artifact_dir={}",
        urlencoding_encode(bad_dir.to_str().unwrap())
    );
    let (status, body) = call(router, get_req(&uri)).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "invalid_artifact");
}

#[tokio::test]
async fn artifact_route_requires_query_param() {
    let fx = build_fixture();
    let st = make_state(&fx);
    let router = routes::build_router(st);
    let (status, body) = call(router, get_req("/api/v1/strategy-scans/artifact")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(body);
    assert_eq!(json["truth_state"], "path_rejected");
}

#[tokio::test]
async fn artifact_route_warnings_present() {
    let fx = build_fixture();
    let st = make_state(&fx);
    let router = routes::build_router(st);
    let (_, body) = call(router, get_req("/api/v1/strategy-scans/artifact")).await;
    let json = parse_json(body);
    let warnings: Vec<&str> = json["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    assert!(warnings
        .iter()
        .any(|w| w.contains("research evidence only")));
    assert!(warnings
        .iter()
        .any(|w| w.contains("not autonomous trading approval")));
    assert!(warnings
        .iter()
        .any(|w| w.contains("negative absolute returns")));
}

// ---------------------------------------------------------------------------
// Minimal percent-encoding for query values (avoids a new crate dependency
// just for this test file — backslashes and colons in Windows paths must
// survive the query string round trip).
// ---------------------------------------------------------------------------

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
