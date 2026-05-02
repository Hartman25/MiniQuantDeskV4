//! BACKTEST-DAEMON-JOBS-01: Daemon backtest job API proof tests.
//!
//! Proves that:
//! - POST /api/v1/backtests/jobs rejects invalid requests (missing fields, bad path)
//! - POST /api/v1/backtests/jobs accepts a valid CSV job and returns job_id
//! - GET /api/v1/backtests/jobs returns job summaries
//! - GET /api/v1/backtests/jobs/:job_id returns full job status
//! - A completed job has artifact_dir, manifest_path, metrics_path populated
//! - No live/paper execution state is touched
//!
//! # Proof matrix
//!
//! | Test   | What it proves                                                            |
//! |--------|---------------------------------------------------------------------------|
//! | BJ-01  | POST with empty bars_path → 400, accepted=false                          |
//! | BJ-02  | POST with timeframe_secs=0 → 400, accepted=false                         |
//! | BJ-03  | POST with nonexistent bars_path → 400, accepted=false                    |
//! | BJ-04  | GET /backtests/jobs with empty store → 200, truth_state=active, empty    |
//! | BJ-05  | GET /backtests/jobs/:id with unknown id → 404, truth_state=not_found     |
//! | BJ-06  | POST valid job → 202, accepted=true, job_id is a non-nil UUID            |
//! | BJ-07  | After POST, GET /backtests/jobs returns the submitted job                 |
//! | BJ-08  | After POST + wait, job status = completed, artifact paths populated       |
//! | BJ-09  | Completed job artifact files exist on disk (manifest.json, metrics.json) |
//! | BJ-10  | No live routing enabled; execution_snapshot untouched after backtest      |
//!
//! BJ-01 to BJ-07 are fully in-process (no disk I/O beyond fixture read).
//! BJ-08 and BJ-09 require disk write to a temp dir; they always run.
//! BJ-10 is a pure in-memory safety check.
//!
//! All tests require no database and no network.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_router() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

fn make_router_with_state() -> (Arc<state::AppState>, axum::Router) {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));
    (st, router)
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

fn json_str<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_else(|| panic!("key '{}' must be a string in: {}", key, v))
}

fn json_bool(v: &serde_json::Value, key: &str) -> bool {
    v.get(key)
        .and_then(|x| x.as_bool())
        .unwrap_or_else(|| panic!("key '{}' must be a bool in: {}", key, v))
}

/// Absolute path to the bkt4_bars.csv fixture.
fn fixture_bars_path() -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mqk-backtest/tests/fixtures/bkt4_bars.csv");
    p.canonicalize()
        .expect("bkt4_bars.csv fixture must exist")
        .display()
        .to_string()
}

fn post_json_body(payload: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/backtests/jobs")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&payload).unwrap(),
        ))
        .unwrap()
}

// ---------------------------------------------------------------------------
// BJ-01: empty bars_path → 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bj01_empty_bars_path_refused() {
    let router = make_router();
    let body = post_json_body(serde_json::json!({
        "bars_path": "",
        "strategy": "swing_momentum",
        "symbol": "TEST",
        "timeframe_secs": 60,
        "initial_cash_micros": 100_000_000_000i64
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "empty bars_path must → 400"
    );
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"), "accepted must be false");
    let err = json_str(&json, "error");
    assert!(
        err.contains("bars_path"),
        "error must mention bars_path, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// BJ-02: timeframe_secs=0 → 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bj02_zero_timeframe_refused() {
    let router = make_router();
    let body = post_json_body(serde_json::json!({
        "bars_path": fixture_bars_path(),
        "strategy": "swing_momentum",
        "symbol": "TEST",
        "timeframe_secs": 0,
        "initial_cash_micros": 100_000_000_000i64
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "timeframe_secs=0 must → 400"
    );
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"), "accepted must be false");
}

// ---------------------------------------------------------------------------
// BJ-03: nonexistent bars_path → 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bj03_nonexistent_bars_path_refused() {
    let router = make_router();
    let body = post_json_body(serde_json::json!({
        "bars_path": "/does/not/exist/bkt_phantom.csv",
        "strategy": "swing_momentum",
        "symbol": "TEST",
        "timeframe_secs": 60,
        "initial_cash_micros": 100_000_000_000i64
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "nonexistent bars_path must → 400"
    );
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"), "accepted must be false");
    let err = json_str(&json, "error");
    assert!(
        err.contains("not found") || err.contains("bars_path"),
        "error must describe missing file, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// BJ-04: GET /backtests/jobs on fresh state → 200, empty list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bj04_list_jobs_empty_on_fresh_state() {
    let router = make_router();
    let req = Request::builder()
        .uri("/api/v1/backtests/jobs")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, resp) = call(router, req).await;
    assert_eq!(status, StatusCode::OK, "GET /backtests/jobs must → 200");
    let json = parse_json(resp);
    assert_eq!(
        json_str(&json, "truth_state"),
        "active",
        "truth_state must be 'active'"
    );
    let jobs = json
        .get("jobs")
        .and_then(|v| v.as_array())
        .expect("jobs array must be present");
    assert!(jobs.is_empty(), "jobs list must be empty on fresh state");
}

// ---------------------------------------------------------------------------
// BJ-05: GET /backtests/jobs/:id with unknown id → 404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bj05_get_unknown_job_id_returns_404() {
    let router = make_router();
    let phantom_id = "00000000-0000-0000-0000-000000000042";
    let req = Request::builder()
        .uri(format!("/api/v1/backtests/jobs/{}", phantom_id))
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, resp) = call(router, req).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "unknown job_id must → 404");
    let json = parse_json(resp);
    assert_eq!(
        json_str(&json, "truth_state"),
        "not_found",
        "truth_state must be 'not_found'"
    );
}

// ---------------------------------------------------------------------------
// BJ-06: POST valid job → 202, accepted=true, non-nil job_id
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bj06_valid_post_returns_accepted() {
    let router = make_router();
    let body = post_json_body(serde_json::json!({
        "bars_path": fixture_bars_path(),
        "strategy": "swing_momentum",
        "symbol": "TEST",
        "timeframe_secs": 60,
        "initial_cash_micros": 100_000_000_000i64
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "valid POST must → 202 Accepted"
    );
    let json = parse_json(resp);
    assert!(json_bool(&json, "accepted"), "accepted must be true");
    let job_id_str = json_str(&json, "job_id");
    let job_id: uuid::Uuid = job_id_str.parse().expect("job_id must be a valid UUID");
    assert!(!job_id.is_nil(), "job_id must be non-nil after acceptance");
    assert_eq!(
        json_str(&json, "status"),
        "queued",
        "status must be 'queued' immediately after POST"
    );
}

// ---------------------------------------------------------------------------
// BJ-07: After POST, GET /backtests/jobs returns the submitted job
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bj07_list_returns_submitted_job() {
    let (st, _) = make_router_with_state();

    // POST via a fresh router bound to the same state.
    let router_post = routes::build_router(Arc::clone(&st));
    let body = post_json_body(serde_json::json!({
        "bars_path": fixture_bars_path(),
        "strategy": "swing_momentum",
        "symbol": "TEST",
        "timeframe_secs": 60,
        "initial_cash_micros": 100_000_000_000i64
    }));
    let (status, resp) = call(router_post, body).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let submitted_id = parse_json(resp)
        .get("job_id")
        .and_then(|v| v.as_str())
        .expect("job_id must be present in POST response")
        .to_string();

    // Give the background task a moment to register in the store.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // GET list via a fresh router bound to the same state.
    let router_get = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .uri("/api/v1/backtests/jobs")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, resp) = call(router_get, req).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(resp);
    let jobs = json
        .get("jobs")
        .and_then(|v| v.as_array())
        .expect("jobs array must be present");
    assert!(!jobs.is_empty(), "jobs list must contain the submitted job");
    let found = jobs
        .iter()
        .any(|j| j.get("job_id").and_then(|v| v.as_str()) == Some(&submitted_id));
    assert!(
        found,
        "submitted job_id {submitted_id} must appear in jobs list"
    );
}

// ---------------------------------------------------------------------------
// BJ-08: After POST + wait, job status = completed and artifact paths set
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bj08_job_completes_with_artifact_paths() {
    let tmp = tempfile::tempdir().expect("temp dir must be creatable");
    let out_dir = tmp.path().display().to_string();

    let (st, _) = make_router_with_state();

    // Submit job. swing_momentum requires timeframe_secs=86400 (daily).
    let router_post = routes::build_router(Arc::clone(&st));
    let body = post_json_body(serde_json::json!({
        "bars_path": fixture_bars_path(),
        "strategy": "swing_momentum",
        "symbol": "TEST",
        "timeframe_secs": 86400,
        "initial_cash_micros": 100_000_000_000i64,
        "out_dir": out_dir
    }));
    let (status, resp) = call(router_post, body).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let job_id = parse_json(resp)
        .get("job_id")
        .and_then(|v| v.as_str())
        .expect("job_id must be present")
        .to_string();

    // Poll until completed or timeout (max 10 s).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let router_get = routes::build_router(Arc::clone(&st));
        let req = Request::builder()
            .uri(format!("/api/v1/backtests/jobs/{}", job_id))
            .body(axum::body::Body::empty())
            .unwrap();
        let (status, resp) = call(router_get, req).await;
        assert_eq!(status, StatusCode::OK);
        let json = parse_json(resp);
        let job_status = json_str(&json, "status").to_string();

        if job_status == "completed" {
            let artifact_dir = json
                .get("artifact_dir")
                .and_then(|v| v.as_str())
                .expect("completed job must have artifact_dir");
            assert!(
                !artifact_dir.is_empty(),
                "artifact_dir must be non-empty for completed job"
            );
            let manifest = json
                .get("manifest_path")
                .and_then(|v| v.as_str())
                .expect("completed job must have manifest_path");
            let metrics = json
                .get("metrics_path")
                .and_then(|v| v.as_str())
                .expect("completed job must have metrics_path");
            assert!(
                !manifest.is_empty(),
                "manifest_path must be non-empty for completed job"
            );
            assert!(
                !metrics.is_empty(),
                "metrics_path must be non-empty for completed job"
            );
            return;
        }

        if job_status == "failed" {
            let err = json
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("(no error)");
            panic!("job failed unexpectedly: {err}");
        }

        assert!(
            std::time::Instant::now() < deadline,
            "job did not complete within 10 seconds (last status: {job_status})"
        );
    }
}

// ---------------------------------------------------------------------------
// BJ-09: Completed job artifact files exist on disk
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bj09_completed_job_artifacts_on_disk() {
    let tmp = tempfile::tempdir().expect("temp dir must be creatable");
    let out_dir = tmp.path().display().to_string();

    let (st, _) = make_router_with_state();

    // swing_momentum requires timeframe_secs=86400 (daily).
    let router_post = routes::build_router(Arc::clone(&st));
    let body = post_json_body(serde_json::json!({
        "bars_path": fixture_bars_path(),
        "strategy": "swing_momentum",
        "symbol": "TEST",
        "timeframe_secs": 86400,
        "initial_cash_micros": 100_000_000_000i64,
        "out_dir": out_dir
    }));
    let (_, resp) = call(router_post, body).await;
    let job_id = parse_json(resp)
        .get("job_id")
        .and_then(|v| v.as_str())
        .expect("job_id must be present")
        .to_string();

    // Wait for completion.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let artifact_dir;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let router_get = routes::build_router(Arc::clone(&st));
        let req = Request::builder()
            .uri(format!("/api/v1/backtests/jobs/{}", job_id))
            .body(axum::body::Body::empty())
            .unwrap();
        let (_, resp) = call(router_get, req).await;
        let json = parse_json(resp);
        let job_status = json_str(&json, "status").to_string();
        if job_status == "completed" {
            artifact_dir = json
                .get("artifact_dir")
                .and_then(|v| v.as_str())
                .expect("artifact_dir must be present")
                .to_string();
            break;
        }
        if job_status == "failed" {
            let err = json
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("(no error)");
            panic!("job failed: {err}");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "job timed out waiting for completion"
        );
    }

    // Verify artifact files exist.
    let base = std::path::Path::new(&artifact_dir);
    for filename in &[
        "manifest.json",
        "metrics.json",
        "orders.csv",
        "fills.csv",
        "equity_curve.csv",
    ] {
        let p = base.join(filename);
        assert!(p.exists(), "artifact file must exist: {}", p.display());
    }
}

// ---------------------------------------------------------------------------
// BJ-10: No live routing or execution state touched after backtest
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bj10_backtest_does_not_touch_live_execution_state() {
    let (st, router) = make_router_with_state();

    // Snapshot execution state before backtest job.
    let execution_snapshot_before = st.execution_snapshot.read().await.clone();
    let broker_snapshot_before = st.broker_snapshot.read().await.clone();

    // Submit a backtest job.
    let body = post_json_body(serde_json::json!({
        "bars_path": fixture_bars_path(),
        "strategy": "swing_momentum",
        "symbol": "TEST",
        "timeframe_secs": 60,
        "initial_cash_micros": 100_000_000_000i64
    }));
    call(router, body).await;

    // Wait briefly for the background task to run.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Verify execution and broker state were NOT modified.
    let execution_snapshot_after = st.execution_snapshot.read().await.clone();
    let broker_snapshot_after = st.broker_snapshot.read().await.clone();

    assert!(
        execution_snapshot_before.is_none() && execution_snapshot_after.is_none(),
        "execution_snapshot must remain None after backtest — live execution not started"
    );
    assert!(
        broker_snapshot_before.is_none() && broker_snapshot_after.is_none(),
        "broker_snapshot must remain None after backtest — no broker called"
    );

    // System status must still show idle (no run started).
    let status = st.status.read().await;
    assert_eq!(
        status.state, "idle",
        "daemon state must remain idle after backtest job"
    );
    assert!(
        status.active_run_id.is_none(),
        "active_run_id must remain None after backtest — no live run started"
    );
}
