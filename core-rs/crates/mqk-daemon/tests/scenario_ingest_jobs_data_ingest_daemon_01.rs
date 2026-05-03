//! DATA-INGEST-DAEMON-JOBS-01: Daemon ingest job API proof tests.
//!
//! Proves that:
//! - POST /api/v1/ingest/jobs rejects invalid requests (empty path, bad timeframe,
//!   nonexistent path, unsupported source)
//! - POST /api/v1/ingest/jobs accepts a valid CSV job and returns job_id + queued
//! - GET /api/v1/ingest/jobs returns an empty list on fresh state (truth_state=active)
//! - GET /api/v1/ingest/jobs returns submitted jobs after a POST
//! - GET /api/v1/ingest/jobs/:job_id returns 404 for an unknown id
//! - GET /api/v1/ingest/jobs/:job_id returns the submitted job with correct fields
//! - A valid CSV job transitions to failed (no_db) when no DB pool is configured
//! - No live/paper execution state is touched by any ingest job operation
//!
//! # Proof matrix
//!
//! | Test   | What it proves                                                              |
//! |--------|-----------------------------------------------------------------------------|
//! | IJ-01  | POST with empty csv_path → 400, accepted=false                             |
//! | IJ-02  | POST with missing csv_path field → 400, accepted=false                     |
//! | IJ-03  | POST with invalid timeframe → 400, accepted=false                          |
//! | IJ-04  | POST with nonexistent csv_path → 400, accepted=false                       |
//! | IJ-05  | POST with unsupported source ("twelvedata") → 400, accepted=false          |
//! | IJ-06  | GET /ingest/jobs on fresh state → 200, truth_state=active, empty list     |
//! | IJ-07  | GET /ingest/jobs/:id with unknown id → 404, truth_state=not_found         |
//! | IJ-08  | POST valid CSV job → 202, accepted=true, job_id non-nil, status="queued"  |
//! | IJ-09  | After POST, GET /ingest/jobs returns the submitted job                     |
//! | IJ-10  | After POST + wait, job transitions to failed with "no_db" (no pool)        |
//! | IJ-11  | GET /ingest/jobs/:job_id returns full job status for existing job           |
//! | IJ-12  | No live routing; execution_snapshot untouched after ingest job             |
//!
//! | IJ-13  | md_backup schema (`open_micros`) with PostgreSQL `t`/`f` bools → no_db (parse ok) |
//!
//! IJ-01 to IJ-09 are fully in-process (no DB, no disk I/O beyond temp file).
//! IJ-10 proves the honest no_db failure path (DB pool absent → job fails truthfully).
//! IJ-11 uses the same shared state as IJ-10.
//! IJ-12 is a pure in-memory safety check.
//! IJ-13 proves the db_backup schema parses successfully (no "deserialize ProviderBar failed").
//!
//! All tests require no database and no network.
//! No TwelveData API credits are consumed.

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

fn post_ingest_json(payload: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/ingest/jobs")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&payload).unwrap(),
        ))
        .unwrap()
}

/// Create a temporary CSV file with md_backup schema and `true`/`false` is_complete.
/// Returns the path to the temp file (caller should keep the TempPath alive).
fn temp_csv_fixture() -> (tempfile::NamedTempFile, String) {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().expect("create temp file");
    writeln!(
        f,
        "symbol,timeframe,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete,ingested_at"
    )
    .unwrap();
    writeln!(
        f,
        "SPY,1D,1708041600,100500000,101000000,99500000,100750000,1000,true,2024-02-16T00:00:00Z"
    )
    .unwrap();
    let path = f.path().to_string_lossy().to_string();
    (f, path)
}

/// Create a temporary CSV file using PostgreSQL `t`/`f` boolean notation (actual AAPL_1D.csv format).
fn temp_csv_fixture_pg_bool() -> (tempfile::NamedTempFile, String) {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().expect("create temp file");
    writeln!(
        f,
        "symbol,timeframe,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete,ingested_at"
    )
    .unwrap();
    // Exact format from exports/md_backup/1D/AAPL_1D.csv
    writeln!(
        f,
        "AAPL,1D,726105600,531250,535713,515625,520088,129136000,t,2026-04-19 12:21:42.508004-10"
    )
    .unwrap();
    writeln!(
        f,
        "AAPL,1D,726192000,517857,529017,511161,529017,186256000,t,2026-04-19 12:21:42.686801-10"
    )
    .unwrap();
    let path = f.path().to_string_lossy().to_string();
    (f, path)
}

// ---------------------------------------------------------------------------
// IJ-01: empty csv_path → 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ij01_empty_csv_path_refused() {
    let router = make_router();
    let body = post_ingest_json(serde_json::json!({
        "source": "csv",
        "csv_path": "",
        "timeframe": "1D"
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "empty csv_path must → 400");
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"), "accepted must be false");
    let err = json_str(&json, "error");
    assert!(
        err.contains("csv_path"),
        "error must mention csv_path, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// IJ-02: csv_path field absent → 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ij02_absent_csv_path_refused() {
    let router = make_router();
    let body = post_ingest_json(serde_json::json!({
        "source": "csv",
        "timeframe": "1D"
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "absent csv_path must → 400"
    );
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"), "accepted must be false");
}

// ---------------------------------------------------------------------------
// IJ-03: invalid timeframe → 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ij03_invalid_timeframe_refused() {
    let (_tmp, path) = temp_csv_fixture();
    let router = make_router();
    let body = post_ingest_json(serde_json::json!({
        "source": "csv",
        "csv_path": path,
        "timeframe": "15m"
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "invalid timeframe must → 400"
    );
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"), "accepted must be false");
    let err = json_str(&json, "error");
    assert!(
        err.contains("timeframe") || err.contains("15m"),
        "error must describe invalid timeframe, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// IJ-04: nonexistent csv_path → 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ij04_nonexistent_csv_path_refused() {
    let router = make_router();
    let body = post_ingest_json(serde_json::json!({
        "source": "csv",
        "csv_path": "/does/not/exist/phantom_bars.csv",
        "timeframe": "1D"
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "nonexistent csv_path must → 400"
    );
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"), "accepted must be false");
    let err = json_str(&json, "error");
    assert!(
        err.contains("not found") || err.contains("csv_path"),
        "error must describe missing file, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// IJ-05: unsupported source ("twelvedata") → 400, not_implemented
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ij05_unsupported_source_refused() {
    let router = make_router();
    let body = post_ingest_json(serde_json::json!({
        "source": "twelvedata",
        "csv_path": null,
        "timeframe": "1D"
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unsupported source must → 400"
    );
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"), "accepted must be false");
    let err = json_str(&json, "error");
    assert!(
        err.contains("not implemented") || err.contains("twelvedata"),
        "error must name the unsupported source, got: {err}"
    );
    // Source field must echo back what was requested.
    let src = json_str(&json, "source");
    assert_eq!(src, "twelvedata", "source must echo the requested value");
}

// ---------------------------------------------------------------------------
// IJ-06: GET /ingest/jobs on fresh state → 200, truth_state=active, empty list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ij06_list_jobs_empty_on_fresh_state() {
    let router = make_router();
    let req = Request::builder()
        .uri("/api/v1/ingest/jobs")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, resp) = call(router, req).await;
    assert_eq!(status, StatusCode::OK, "GET /ingest/jobs must → 200");
    let json = parse_json(resp);
    assert_eq!(
        json_str(&json, "truth_state"),
        "active",
        "truth_state must be 'active' on fresh state"
    );
    let jobs = json
        .get("jobs")
        .and_then(|v| v.as_array())
        .expect("jobs array must be present");
    assert!(jobs.is_empty(), "jobs list must be empty on fresh state");
}

// ---------------------------------------------------------------------------
// IJ-07: GET /ingest/jobs/:id with unknown id → 404, truth_state=not_found
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ij07_get_unknown_job_id_returns_404() {
    let router = make_router();
    let phantom_id = "00000000-0000-0000-0000-000000000099";
    let req = Request::builder()
        .uri(format!("/api/v1/ingest/jobs/{}", phantom_id))
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
// IJ-08: POST valid CSV job → 202, accepted=true, non-nil job_id, status=queued
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ij08_valid_post_returns_accepted() {
    let (_tmp, path) = temp_csv_fixture();
    let router = make_router();
    let body = post_ingest_json(serde_json::json!({
        "source": "csv",
        "csv_path": path,
        "timeframe": "1D"
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
    assert!(!job_id.is_nil(), "job_id must be non-nil");
    assert_eq!(
        json_str(&json, "status"),
        "queued",
        "status must be 'queued' immediately after POST"
    );
    assert_eq!(json_str(&json, "source"), "csv", "source must echo 'csv'");
}

// ---------------------------------------------------------------------------
// IJ-09: After POST, GET /ingest/jobs returns the submitted job
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ij09_list_returns_submitted_job() {
    let (_tmp, path) = temp_csv_fixture();
    let (st, _) = make_router_with_state();

    // POST via a fresh router bound to the same state.
    let router_post = routes::build_router(Arc::clone(&st));
    let body = post_ingest_json(serde_json::json!({
        "source": "csv",
        "csv_path": path,
        "timeframe": "1D"
    }));
    let (status, resp) = call(router_post, body).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let submitted_id = parse_json(resp)
        .get("job_id")
        .and_then(|v| v.as_str())
        .expect("job_id must be present in POST response")
        .to_string();

    // Give the background task a moment to register.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // GET list via a fresh router bound to the same state.
    let router_get = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .uri("/api/v1/ingest/jobs")
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
// IJ-10: Valid CSV job → transitions to failed with "no_db" (no DB pool)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ij10_no_db_job_fails_truthfully() {
    let (_tmp, path) = temp_csv_fixture();
    let (st, _) = make_router_with_state();

    // Confirm no DB pool is configured (AppState::new() has db=None).
    // The ingest job must fail honestly rather than fabricating success.
    let router_post = routes::build_router(Arc::clone(&st));
    let body = post_ingest_json(serde_json::json!({
        "source": "csv",
        "csv_path": path,
        "timeframe": "1D"
    }));
    let (status, resp) = call(router_post, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "POST must → 202");
    let submitted_id = parse_json(resp)
        .get("job_id")
        .and_then(|v| v.as_str())
        .expect("job_id must be present")
        .to_string();

    // Wait for background task to complete (no_db path is synchronous and fast).
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Poll job status.
    let router_get = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .uri(format!("/api/v1/ingest/jobs/{}", submitted_id))
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, resp) = call(router_get, req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(resp);

    let job_status = json_str(&json, "status");
    assert_eq!(
        job_status, "failed",
        "job must be 'failed' when no DB is available, got: {job_status}"
    );

    // Error must mention no_db — never fabricate success.
    let err = json.get("error").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        err.contains("no_db") || err.contains("database"),
        "failed job error must indicate DB unavailability, got: {err}"
    );

    // truth_state must be 'active' (job is in store; DB absence is a job-level error).
    assert_eq!(json_str(&json, "truth_state"), "active");
}

// ---------------------------------------------------------------------------
// IJ-11: GET /ingest/jobs/:job_id returns full status for existing job
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ij11_get_job_id_returns_full_status() {
    let (_tmp, path) = temp_csv_fixture();
    let (st, _) = make_router_with_state();

    let router_post = routes::build_router(Arc::clone(&st));
    let body = post_ingest_json(serde_json::json!({
        "source": "csv",
        "csv_path": path,
        "timeframe": "1D",
        "source_label": "test_run"
    }));
    let (_, resp) = call(router_post, body).await;
    let submitted_id = parse_json(resp)
        .get("job_id")
        .and_then(|v| v.as_str())
        .expect("job_id must be present")
        .to_string();

    // Wait for background task to settle.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .uri(format!("/api/v1/ingest/jobs/{}", submitted_id))
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, resp) = call(router_get, req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(resp);

    // Required fields must be present.
    assert_eq!(json_str(&json, "truth_state"), "active");
    assert_eq!(json_str(&json, "job_id"), &submitted_id);
    assert_eq!(json_str(&json, "source"), "csv");
    assert_eq!(json_str(&json, "timeframe"), "1D");
    assert!(
        json.get("created_at_utc")
            .and_then(|v| v.as_str())
            .is_some(),
        "created_at_utc must be present"
    );
    // Status must be terminal (completed or failed) — not stuck in queued.
    let status_val = json_str(&json, "status");
    assert!(
        status_val == "completed" || status_val == "failed",
        "job must reach terminal state, got: {status_val}"
    );
}

// ---------------------------------------------------------------------------
// IJ-13: md_backup schema with PostgreSQL t/f booleans → no_db (parse succeeds)
// ---------------------------------------------------------------------------
//
// Proves DATA-INGEST-DAEMON-JOBS-02: the db_backup CSV schema (open_micros columns,
// PostgreSQL t/f booleans, ingested_at extra column) parses correctly.
// Without the fix, the job would fail with "deserialize ProviderBar failed".
// With the fix, the job parses successfully and fails only at the DB step ("no_db").

#[tokio::test]
async fn ij13_md_backup_pg_bool_schema_parses_to_no_db() {
    let (_tmp, path) = temp_csv_fixture_pg_bool();
    let (st, _) = make_router_with_state();

    let router_post = routes::build_router(Arc::clone(&st));
    let body = post_ingest_json(serde_json::json!({
        "source": "csv",
        "csv_path": path,
        "timeframe": "1D",
        "source_label": "ij13-pg-bool-smoke"
    }));
    let (status, resp) = call(router_post, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "POST must → 202");
    let submitted_id = parse_json(resp)
        .get("job_id")
        .and_then(|v| v.as_str())
        .expect("job_id must be present")
        .to_string();

    // Wait for background task to complete (no_db is fast).
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let req = axum::http::Request::builder()
        .uri(format!("/api/v1/ingest/jobs/{}", submitted_id))
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, resp) = call(router_get, req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(resp);

    let job_status = json_str(&json, "status");
    assert_eq!(
        job_status, "failed",
        "job must be 'failed' (no DB configured), got: {job_status}"
    );

    let err = json.get("error").and_then(|v| v.as_str()).unwrap_or("");
    // Key assertion: error must be no_db, NOT "deserialize ProviderBar failed".
    assert!(
        err.contains("no_db") || err.contains("database"),
        "error must be no_db (parse succeeded), got: '{err}'"
    );
    assert!(
        !err.contains("ProviderBar"),
        "error must NOT mention ProviderBar (db_backup schema must be detected), got: '{err}'"
    );
}

// ---------------------------------------------------------------------------
// IJ-12: No live routing; execution_snapshot untouched after ingest job
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ij12_ingest_job_does_not_touch_execution_snapshot() {
    let (_tmp, path) = temp_csv_fixture();
    let (st, _) = make_router_with_state();

    // Record execution_snapshot before ingest job.
    let snapshot_before = {
        let snap = st.execution_snapshot.read().await;
        snap.is_none()
    };

    let router_post = routes::build_router(Arc::clone(&st));
    let body = post_ingest_json(serde_json::json!({
        "source": "csv",
        "csv_path": path,
        "timeframe": "1D"
    }));
    let (status, _) = call(router_post, body).await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // Wait for background task to settle.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // execution_snapshot must remain untouched.
    let snapshot_after = {
        let snap = st.execution_snapshot.read().await;
        snap.is_none()
    };
    assert_eq!(
        snapshot_before, snapshot_after,
        "ingest job must not touch execution_snapshot"
    );
    assert!(
        snapshot_after,
        "execution_snapshot must remain None (no live execution started)"
    );
}
