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

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    ingest_jobs::{IngestJobRecord, IngestJobStatus},
    routes, state,
};
use sqlx::Row;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// FakeProvider — zero-network HistoricalProvider for testing the real sync path.
//
// Used in PD-04, PD-10..PD-13.  Never makes HTTP calls; returns configurable bars.
// ---------------------------------------------------------------------------

struct FakeProvider {
    bars: Vec<mqk_md::ProviderBar>,
    fail_symbols: Vec<String>,
}

struct SlowCountingProvider {
    calls: Arc<AtomicUsize>,
    delay: std::time::Duration,
}

#[async_trait::async_trait]
impl mqk_md::HistoricalProvider for SlowCountingProvider {
    fn source_name(&self) -> &'static str {
        "slow_counting_test_provider"
    }

    async fn fetch_bars(
        &self,
        _req: mqk_md::FetchBarsRequest,
    ) -> anyhow::Result<Vec<mqk_md::ProviderBar>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        Ok(vec![])
    }
}

impl FakeProvider {
    fn empty() -> Self {
        Self {
            bars: vec![],
            fail_symbols: vec![],
        }
    }

    #[allow(dead_code)]
    fn with_bars(bars: Vec<mqk_md::ProviderBar>) -> Self {
        Self {
            bars,
            fail_symbols: vec![],
        }
    }

    fn failing(fail_symbols: Vec<String>) -> Self {
        Self {
            bars: vec![],
            fail_symbols,
        }
    }
}

#[async_trait::async_trait]
impl mqk_md::HistoricalProvider for FakeProvider {
    fn source_name(&self) -> &'static str {
        "fake_test_provider"
    }

    async fn fetch_bars(
        &self,
        req: mqk_md::FetchBarsRequest,
    ) -> anyhow::Result<Vec<mqk_md::ProviderBar>> {
        for sym in &req.symbols {
            if self.fail_symbols.contains(sym) {
                return Err(anyhow::anyhow!(
                    "fake provider: intentional failure for symbol {}",
                    sym
                ));
            }
        }
        Ok(self.bars.clone())
    }
}

/// Build AppState (not Arc-wrapped) pointing at the real canonical registry.
/// Caller can mutate it (e.g. inject a provider client) before wrapping in Arc.
fn make_provider_router_with_registry_raw() -> (state::AppState, ()) {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let registry_path = std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/equities.json")
        .canonicalize()
        .expect("registry must resolve")
        .to_string_lossy()
        .to_string();
    let provider_registry_path = std::path::PathBuf::from(manifest_dir)
        .join("../../../config/providers/providers.json")
        .canonicalize()
        .expect("provider registry must resolve")
        .to_string_lossy()
        .to_string();

    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = registry_path;
    st.provider_registry_path = provider_registry_path;
    (st, ())
}

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

fn cancel_ingest_request(job_id: uuid::Uuid) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/api/v1/ingest/jobs/{}/cancel", job_id))
        .body(axum::body::Body::empty())
        .unwrap()
}

async fn post_cancel_job(
    router: axum::Router,
    job_id: uuid::Uuid,
) -> (StatusCode, serde_json::Value) {
    let (status, body) = call(router, cancel_ingest_request(job_id)).await;
    (status, parse_json(body))
}

fn seed_job_record(job_id: uuid::Uuid, status: IngestJobStatus) -> IngestJobRecord {
    let now = chrono::Utc::now();
    let completed_at_utc = if status.is_terminal() {
        Some(now)
    } else {
        None
    };

    IngestJobRecord {
        job_id,
        source: "twelvedata".to_string(),
        mode: Some("sync_provider".to_string()),
        csv_path: None,
        timeframe: "1D".to_string(),
        source_label: "twelvedata".to_string(),
        out_dir: "exports/md_ingest".to_string(),
        status,
        created_at_utc: now,
        started_at_utc: None,
        completed_at_utc,
        rows_read: None,
        rows_inserted: None,
        rows_rejected: None,
        quality_report_path: None,
        error: None,
        dry_run: true,
        provider_api_calls_allowed: false,
        api_calls_made: 0,
        symbols_source: Some("registry".to_string()),
        registry_path_used: None,
        provider_registry_path_used: None,
        symbols_count: Some(88),
        planned_first_symbol: Some("AAL".to_string()),
        planned_last_symbol: Some("XOM".to_string()),
        asset_class: "equity".to_string(),
        provider_enabled: Some(true),
        provider_verification_status: Some("verified".to_string()),
        symbols_completed: Some(0),
        symbols_failed: Some(0),
    }
}

fn insert_seed_job(st: &Arc<state::AppState>, record: IngestJobRecord) {
    let mut store = st.ingest_jobs.lock().expect("ingest_jobs lock poisoned");
    store.insert(record.job_id, record);
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

#[test]
fn cancel_01_cancelled_status_is_terminal() {
    assert!(
        IngestJobStatus::Cancelled.is_terminal(),
        "cancelled must be terminal"
    );
    assert_eq!(IngestJobStatus::Cancelled.as_str(), "cancelled");
}

#[tokio::test]
async fn cancel_02_unknown_job_returns_404_truthful_error() {
    let router = make_router();
    let phantom_id = uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000198")
        .expect("static uuid must parse");

    let (status, body) = post_cancel_job(router, phantom_id).await;

    assert_eq!(status, StatusCode::NOT_FOUND, "unknown cancel must → 404");
    assert_eq!(json_str(&body, "truth_state"), "not_found");
    assert!(
        !json_bool(&body, "accepted"),
        "unknown cancel must not be accepted: {body}"
    );
    let err = json_str(&body, "error");
    assert!(
        err.contains("not found") && err.contains(&phantom_id.to_string()),
        "error must identify unknown job_id, got: {err}"
    );
}

#[tokio::test]
async fn cancel_03_queued_job_becomes_cancelled_terminal() {
    let (st, _) = make_router_with_state();
    let job_id = uuid::Uuid::new_v4();
    insert_seed_job(&st, seed_job_record(job_id, IngestJobStatus::Queued));

    let router_cancel = routes::build_router(Arc::clone(&st));
    let (status, body) = post_cancel_job(router_cancel, job_id).await;

    assert_eq!(status, StatusCode::ACCEPTED, "queued cancel must → 202");
    assert!(json_bool(&body, "accepted"), "cancel must be accepted");
    assert_eq!(json_str(&body, "truth_state"), "cancel_accepted");
    assert_eq!(json_str(&body, "status"), "cancelled");
    let err = json_str(&body, "error");
    assert!(
        err.contains("cancel requested by operator"),
        "cancel reason must be truthful, got: {err}"
    );

    let router_get = routes::build_router(Arc::clone(&st));
    let (status, body) = get_job_status(router_get, &job_id.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_str(&body, "status"), "cancelled");
}

#[tokio::test]
async fn cancel_04_already_terminal_job_returns_truthful_response_without_mutation() {
    let (st, _) = make_router_with_state();
    let job_id = uuid::Uuid::new_v4();
    insert_seed_job(
        &st,
        seed_job_record(job_id, IngestJobStatus::DryRunCompleted),
    );

    let router_cancel = routes::build_router(Arc::clone(&st));
    let (status, body) = post_cancel_job(router_cancel, job_id).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "already-terminal cancel must return a truth response"
    );
    assert_eq!(json_str(&body, "truth_state"), "already_terminal");
    assert!(
        !json_bool(&body, "accepted"),
        "already-terminal cancel must not be accepted: {body}"
    );
    assert_eq!(
        json_str(&body, "status"),
        "dry_run_completed",
        "terminal status must not be mutated"
    );
}

#[tokio::test]
async fn cancel_05_fake_provider_stops_after_cancel_between_symbols() {
    let (mut st_raw, _) = make_provider_router_with_registry_raw();
    let calls = Arc::new(AtomicUsize::new(0));
    st_raw.set_provider_client_for_test(Arc::new(SlowCountingProvider {
        calls: Arc::clone(&calls),
        delay: std::time::Duration::from_millis(50),
    }));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2026-01-01",
            "end": "2026-01-05"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "real job must queue: {body}");
    let job_id: uuid::Uuid = json_str(&body, "job_id")
        .parse()
        .expect("job_id must parse");

    for _ in 0..50 {
        if calls.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        calls.load(Ordering::SeqCst) > 0,
        "fake provider must have entered first fetch before cancellation"
    );

    let router_cancel = routes::build_router(Arc::clone(&st));
    let (status, body) = post_cancel_job(router_cancel, job_id).await;
    assert_eq!(status, StatusCode::ACCEPTED, "running cancel must → 202");
    assert_eq!(json_str(&body, "status"), "cancelled");

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    let total_calls = calls.load(Ordering::SeqCst);
    assert!(
        total_calls < 88,
        "provider must not continue through all registry symbols after cancel; calls={total_calls}"
    );

    let router_get = routes::build_router(Arc::clone(&st));
    let (status, body) = get_job_status(router_get, &job_id.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_str(&body, "status"), "cancelled");
    let err = json_str(&body, "error");
    assert!(
        err.contains("cancel requested by operator"),
        "cancelled job must preserve cancel reason, got: {err}"
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

// ---------------------------------------------------------------------------
// DATA-INGEST-GUI-SYNC-ALL-01: GET /api/v1/ingest/tracked-equities
//
// | Test   | What it proves                                                      |
// |--------|---------------------------------------------------------------------|
// | TE-01  | Returns count=88 from the canonical registry (AAPL+SPY present)     |
// | TE-02  | Missing registry path → truth_state=registry_unavailable + error    |
// | TE-03  | Response symbols are in deterministic alphabetical order            |
// | TE-04  | Execution state and arm_state are untouched                         |
// | TE-05  | No DB pool required; route works without a database                 |
//
// No provider API calls. No DB writes. No TwelveData credits consumed.
// ---------------------------------------------------------------------------

/// Return an Axum router pointed at the real canonical registry file.
///
/// CARGO_MANIFEST_DIR is the `mqk-daemon` crate directory.
/// The registry lives three levels up: ../../../config/instruments/equities.json.
fn make_router_with_real_registry() -> axum::Router {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let registry_path = std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/equities.json")
        .canonicalize()
        .expect("registry path must resolve from CARGO_MANIFEST_DIR")
        .to_string_lossy()
        .to_string();

    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = registry_path;
    let st = Arc::new(st);
    routes::build_router(st)
}

/// Return a router pointing at a guaranteed-nonexistent registry path.
fn make_router_with_missing_registry() -> axum::Router {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = "/nonexistent/path/that/cannot/exist/equities.json".to_string();
    let st = Arc::new(st);
    routes::build_router(st)
}

async fn get_tracked_equities(router: axum::Router) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/ingest/tracked-equities")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    (status, parse_json(body))
}

// TE-01: real registry → count=88, AAPL and SPY present, truth_state=active
#[tokio::test]
async fn te_01_tracked_equities_count_88_aapl_spy_present() {
    let router = make_router_with_real_registry();
    let (status, body) = get_tracked_equities(router).await;

    assert_eq!(status, StatusCode::OK, "must return 200");
    assert_eq!(
        body["truth_state"], "active",
        "truth_state must be active: {body}"
    );
    assert_eq!(
        body["canonical_route"], "/api/v1/ingest/tracked-equities",
        "canonical_route must be set"
    );

    let count = body["count"].as_u64().expect("count must be a number");
    assert_eq!(count, 88, "count must equal 88 enabled equities");

    let symbols = body["symbols"]
        .as_array()
        .expect("symbols must be an array");
    assert_eq!(symbols.len(), 88, "symbols array length must equal count");

    let symbol_strings: Vec<&str> = symbols
        .iter()
        .map(|s| s["symbol"].as_str().expect("symbol must be a string"))
        .collect();

    assert!(
        symbol_strings.contains(&"AAPL"),
        "AAPL must be in the symbol list"
    );
    assert!(
        symbol_strings.contains(&"SPY"),
        "SPY must be in the symbol list"
    );

    assert_eq!(
        body["error"],
        serde_json::Value::Null,
        "error must be null on success"
    );
    assert!(body["first_symbol"].is_string(), "first_symbol must be set");
    assert!(body["last_symbol"].is_string(), "last_symbol must be set");
}

// TE-02: missing registry path → truth_state=registry_unavailable, error populated
#[tokio::test]
async fn te_02_missing_registry_returns_unavailable() {
    let router = make_router_with_missing_registry();
    let (status, body) = get_tracked_equities(router).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "must still return 200 (truth envelope)"
    );
    assert_eq!(
        body["truth_state"], "registry_unavailable",
        "truth_state must be registry_unavailable: {body}"
    );
    assert_eq!(body["count"].as_u64().unwrap(), 0, "count must be 0");
    assert!(
        body["symbols"].as_array().unwrap().is_empty(),
        "symbols must be empty"
    );
    assert!(body["error"].is_string(), "error must be populated: {body}");
    assert_eq!(
        body["first_symbol"],
        serde_json::Value::Null,
        "first_symbol must be null"
    );
    assert_eq!(
        body["last_symbol"],
        serde_json::Value::Null,
        "last_symbol must be null"
    );
}

// TE-03: symbols are in deterministic alphabetical order
#[tokio::test]
async fn te_03_symbols_are_alphabetically_sorted() {
    let router = make_router_with_real_registry();
    let (status, body) = get_tracked_equities(router).await;
    assert_eq!(status, StatusCode::OK);

    let symbols = body["symbols"].as_array().unwrap();
    let syms: Vec<&str> = symbols
        .iter()
        .map(|s| s["symbol"].as_str().unwrap())
        .collect();

    for window in syms.windows(2) {
        assert!(
            window[0] <= window[1],
            "symbols must be sorted: '{}' before '{}'",
            window[0],
            window[1]
        );
    }

    // Sanity: first alphabetically should be AAL, last should start after W
    let first = syms.first().unwrap();
    let last = syms.last().unwrap();
    assert!(
        *first <= "AAL",
        "first symbol must sort at or before AAL, got {first}"
    );
    assert!(*first < *last, "first < last required for sorted list");

    // first_symbol and last_symbol fields must match the array
    assert_eq!(body["first_symbol"], *first);
    assert_eq!(body["last_symbol"], *last);
}

// TE-04: execution_snapshot and arm_state are untouched after calling the route
#[tokio::test]
async fn te_04_execution_state_untouched() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let registry_path = std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/equities.json")
        .canonicalize()
        .expect("registry must exist")
        .to_string_lossy()
        .to_string();

    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = registry_path;
    let st = Arc::new(st);

    // Record state before the call.
    let snap_before = st.execution_snapshot.read().await.is_none();
    let arm_before = st.integrity.read().await.disarmed;

    let router = routes::build_router(Arc::clone(&st));
    let (status, _) = get_tracked_equities(router).await;
    assert_eq!(status, StatusCode::OK);

    // Both must remain unchanged.
    let snap_after = st.execution_snapshot.read().await.is_none();
    let arm_after = st.integrity.read().await.disarmed;

    assert_eq!(
        snap_before, snap_after,
        "execution_snapshot must be untouched"
    );
    assert_eq!(arm_before, arm_after, "arm_state must be untouched");
}

// TE-05: route works without a DB pool (no database required)
#[tokio::test]
async fn te_05_no_db_required() {
    // make_router_with_real_registry uses new_with_operator_auth which has no DB pool.
    let router = make_router_with_real_registry();
    let (status, body) = get_tracked_equities(router).await;

    // Must succeed even with no DB configured.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "active");
    assert_eq!(body["count"].as_u64().unwrap(), 88);
}

// ---------------------------------------------------------------------------
// PAPER-SOAK-PROVIDER-SCOPED-INGEST-TEST-REPAIR-01
//
// TEST-ONLY: derive the expected provider/timeframe-scoped symbol set
// directly from the canonical registry file, independent of the production
// resolver (`resolve_provider_scoped_equities` in routes/ingest.rs). This
// filters explicitly (enabled + asset_class + provider + timeframe-string
// membership) so the assertions below prove the resolver's live behavior
// against an independently derived truth, not a restatement of the
// resolver's own logic.
// ---------------------------------------------------------------------------
fn expected_registry_symbols_for_provider_timeframe(
    provider: &str,
    asset_class: &str,
    timeframe: &str,
) -> Vec<String> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let registry_path = std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/equities.json")
        .canonicalize()
        .expect("registry path must resolve from CARGO_MANIFEST_DIR");
    let instruments = mqk_md::instrument_registry::load_instrument_registry(&registry_path)
        .expect("canonical registry must parse");

    let mut symbols: Vec<String> = instruments
        .into_iter()
        .filter(|i| {
            i.enabled
                && i.asset_class.eq_ignore_ascii_case(asset_class)
                && i.provider.eq_ignore_ascii_case(provider)
                && i.timeframes.iter().any(|tf| tf == timeframe)
        })
        .map(|i| i.symbol)
        .collect();
    symbols.sort();
    symbols
}

// PROV-SCOPE-01: canonical registry provider scoping intentionally excludes
// Alpaca-assigned AAPL from the TwelveData/1D provider-sync plan.
//
// This exists to prevent someone from later "fixing" the ingest resolver
// back to whole-registry behavior and silently destroying provider
// provenance (MARKET-DATA-PROVIDER-PROVENANCE-01 / -REPAIR-01).
#[test]
fn canonical_registry_provider_scoping_excludes_alpaca_aapl_from_twelvedata_1d() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let registry_path = std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/equities.json")
        .canonicalize()
        .expect("registry path must resolve from CARGO_MANIFEST_DIR");
    let instruments = mqk_md::instrument_registry::load_instrument_registry(&registry_path)
        .expect("canonical registry must parse");

    let enabled_count = instruments.iter().filter(|i| i.enabled).count();
    assert_eq!(
        enabled_count, 88,
        "canonical registry must still contain 88 enabled equities"
    );

    let aapl = instruments
        .iter()
        .find(|i| i.symbol == "AAPL")
        .expect("AAPL must be present in the registry");
    assert_eq!(
        aapl.provider, "alpaca",
        "AAPL must be provider=alpaca (MARKET-DATA-PROVIDER-PROVENANCE-01-REPAIR-01)"
    );
    assert!(
        aapl.timeframes.iter().any(|tf| tf == "5m"),
        "AAPL must authorize 5m"
    );
    assert!(
        !aapl.timeframes.iter().any(|tf| tf == "1D"),
        "AAPL must NOT authorize 1D"
    );

    let td_1d = expected_registry_symbols_for_provider_timeframe("twelvedata", "equity", "1D");
    assert_eq!(
        td_1d.len(),
        87,
        "TwelveData/1D provider-scoped set must be 87"
    );
    assert!(
        !td_1d.contains(&"AAPL".to_string()),
        "AAPL must be excluded from the TwelveData/1D provider-scoped set"
    );

    let alpaca_5m = expected_registry_symbols_for_provider_timeframe("alpaca", "equity", "5m");
    assert_eq!(
        alpaca_5m,
        vec!["AAPL".to_string()],
        "Alpaca/5m provider-scoped set must be exactly [AAPL] at this registry revision"
    );
}

// ---------------------------------------------------------------------------
// DATA-INGEST-DAEMON-PROVIDER-JOBS-01: Provider sync job tests
//
// | Test   | What it proves                                                                |
// |--------|-------------------------------------------------------------------------------|
// | PD-01  | POST dry_run=true → 202, accepted=true, dry_run=true, api_calls_made=0      |
// | PD-02  | After wait, job is dry_run_completed, symbols_count=provider-scoped (87),   |
// |        | api_calls_made=0                                                             |
// | PD-03  | dry_run=false + allow_provider_api_calls=false → 400 refused                |
// | PD-04  | dry_run=false + allow_provider_api_calls=true + fake provider → 202 queued  |
// | PD-05  | source=twelvedata without mode → 400 refused (IJ-05 compat preserved)      |
// | PD-06  | invalid registry path → dry-run job fails truthfully (registry_load_failed) |
//
// No real TwelveData network calls in any test. No DB writes. No API credits consumed.
// Real-provider path (PD-04) uses an injectable fake provider (zero-network).
// All tests are fully in-process.
// ---------------------------------------------------------------------------

/// Return a router + shared state pointing at the real canonical registry.
fn make_provider_router_with_registry() -> (Arc<state::AppState>, axum::Router) {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let registry_path = std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/equities.json")
        .canonicalize()
        .expect("registry must resolve")
        .to_string_lossy()
        .to_string();
    let provider_registry_path = std::path::PathBuf::from(manifest_dir)
        .join("../../../config/providers/providers.json")
        .canonicalize()
        .expect("provider registry must resolve")
        .to_string_lossy()
        .to_string();

    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = registry_path;
    st.provider_registry_path = provider_registry_path;
    let st = Arc::new(st);
    let router = routes::build_router(Arc::clone(&st));
    (st, router)
}

/// POST a provider sync job and return (status, parsed JSON body).
async fn post_provider_job(
    router: axum::Router,
    payload: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/ingest/jobs")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&payload).unwrap(),
        ))
        .unwrap();
    let (status, body) = call(router, req).await;
    (status, parse_json(body))
}

/// GET job status by id and return (status code, parsed JSON body).
async fn get_job_status(router: axum::Router, job_id: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .uri(format!("/api/v1/ingest/jobs/{}", job_id))
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    (status, parse_json(body))
}

async fn ingest_job_db_pool_or_skip(label: &str) -> Option<sqlx::PgPool> {
    let Ok(url) = std::env::var("MQK_DATABASE_URL") else {
        eprintln!("{label}: skipped; MQK_DATABASE_URL is not set");
        return None;
    };

    let pool = match sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
    {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("{label}: skipped; could not connect to MQK_DATABASE_URL: {e}");
            return None;
        }
    };

    if let Err(e) = mqk_db::migrate(&pool).await {
        eprintln!("{label}: skipped; mqk_db::migrate failed: {e}");
        return None;
    }

    Some(pool)
}

async fn cleanup_ingest_job(pool: &sqlx::PgPool, job_id: uuid::Uuid) {
    sqlx::query("delete from sys_ingest_jobs where job_id = $1")
        .bind(job_id)
        .execute(pool)
        .await
        .expect("cleanup sys_ingest_jobs row");
}

async fn wait_for_persisted_status(
    pool: &sqlx::PgPool,
    job_id: uuid::Uuid,
    terminal_statuses: &[&str],
) -> mqk_daemon::ingest_jobs::IngestJobRecord {
    for _ in 0..50 {
        if let Some(record) = mqk_daemon::ingest_jobs::load_persisted_ingest_job(pool, job_id)
            .await
            .expect("load persisted ingest job")
        {
            if terminal_statuses.contains(&record.status.as_str()) {
                return record;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    mqk_daemon::ingest_jobs::load_persisted_ingest_job(pool, job_id)
        .await
        .expect("load persisted ingest job after timeout")
        .unwrap_or_else(|| panic!("persisted ingest job {job_id} not found after timeout"))
}

// PD-01: dry_run=true → 202, accepted=true, dry_run echoed, api_calls_made=0
#[tokio::test]
async fn pd_01_provider_dryryn_accepted() {
    let (_, router) = make_provider_router_with_registry();
    let (status, body) = post_provider_job(
        router,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "dry-run POST must → 202: {body}"
    );
    assert!(
        json_bool(&body, "accepted"),
        "accepted must be true: {body}"
    );
    assert_eq!(
        json_str(&body, "status"),
        "queued",
        "status must be queued: {body}"
    );
    assert_eq!(
        json_str(&body, "source"),
        "twelvedata",
        "source must echo: {body}"
    );

    // dry_run and api_calls_made must be reported at acceptance time.
    assert_eq!(
        body["dry_run"],
        serde_json::Value::Bool(true),
        "dry_run must be true: {body}"
    );
    assert_eq!(
        body["api_calls_made"].as_i64().unwrap_or(999),
        0,
        "api_calls_made must be 0 at acceptance: {body}"
    );
    assert_eq!(
        body["provider_api_calls_allowed"],
        serde_json::Value::Bool(false),
        "provider_api_calls_allowed must be false: {body}"
    );

    // job_id must be a non-nil UUID.
    let job_id_str = json_str(&body, "job_id");
    let job_id: uuid::Uuid = job_id_str.parse().expect("job_id must be a valid UUID");
    assert!(!job_id.is_nil(), "job_id must be non-nil");
}

// PD-02: after wait → dry_run_completed, symbols_count=provider-scoped (87), api_calls_made=0
#[tokio::test]
async fn pd_02_provider_dryrun_completes_provider_scoped_symbols_zero_api_calls() {
    let (st, _) = make_provider_router_with_registry();

    // POST the job.
    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "POST must → 202: {body}");
    let job_id = json_str(&body, "job_id").to_string();

    // Wait for the background task to resolve symbols (pure fs read — fast).
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Poll job status.
    let router_get = routes::build_router(Arc::clone(&st));
    let (status, body) = get_job_status(router_get, &job_id).await;
    assert_eq!(status, StatusCode::OK, "GET status must → 200");

    // Status must be dry_run_completed.
    let job_status = json_str(&body, "status");
    assert_eq!(
        job_status, "dry_run_completed",
        "job must reach dry_run_completed; got: {job_status}. body: {body}"
    );

    // symbols_count must equal the TwelveData/1D provider-scoped registry size
    // (AAPL is excluded: it is Alpaca/5m-only — see PROV-SCOPE-01 above).
    let expected =
        expected_registry_symbols_for_provider_timeframe("twelvedata", "equity", "1D").len()
            as u64;
    let symbols_count = body["symbols_count"]
        .as_u64()
        .expect("symbols_count must be a number in completed job");
    assert_eq!(
        symbols_count, expected,
        "symbols_count must be {expected} (TwelveData/1D provider-scoped registry), got: {symbols_count}"
    );

    // api_calls_made must be exactly 0 — the dry-run invariant.
    let api_calls = body["api_calls_made"]
        .as_i64()
        .expect("api_calls_made must be a number");
    assert_eq!(
        api_calls, 0,
        "api_calls_made must be 0 for dry-run; got: {api_calls}"
    );

    // dry_run and provider_api_calls_allowed must be correct.
    assert_eq!(body["dry_run"], serde_json::Value::Bool(true));
    assert_eq!(
        body["provider_api_calls_allowed"],
        serde_json::Value::Bool(false)
    );

    // symbols_source and registry_path_used must be populated.
    assert_eq!(body["symbols_source"], "registry");
    assert!(
        body["registry_path_used"].is_string(),
        "registry_path_used must be a string: {body}"
    );

    // planned_first_symbol and planned_last_symbol must be populated.
    assert!(
        body["planned_first_symbol"].is_string(),
        "planned_first_symbol must be populated: {body}"
    );
    assert!(
        body["planned_last_symbol"].is_string(),
        "planned_last_symbol must be populated: {body}"
    );

    // No DB rows. No CSV files. No error.
    assert_eq!(
        body["error"],
        serde_json::Value::Null,
        "error must be null on success"
    );
    assert_eq!(
        body["rows_read"],
        serde_json::Value::Null,
        "rows_read must be null for dry-run"
    );
    assert_eq!(body["rows_inserted"], serde_json::Value::Null);

    // source and mode must be correct.
    assert_eq!(json_str(&body, "source"), "twelvedata");
    assert_eq!(body["mode"], "sync_provider");
}

// PD-03: dry_run=false + allow_provider_api_calls=false → 400 refused
#[tokio::test]
async fn pd_03_provider_real_without_allow_refused() {
    let (_, router) = make_provider_router_with_registry();
    let (status, body) = post_provider_job(
        router,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": false
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "must → 400 when dry_run=false and allow=false: {body}"
    );
    assert!(
        !json_bool(&body, "accepted"),
        "accepted must be false: {body}"
    );

    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("allow_provider_api_calls")
            || err.contains("not allowed")
            || err.contains("dry_run"),
        "error must explain why provider calls are refused: {err}"
    );

    // api_calls_made must be 0 — never called provider.
    assert_eq!(
        body["api_calls_made"].as_i64().unwrap_or(999),
        0,
        "api_calls_made must be 0 on refusal: {body}"
    );
}

// PD-04: dry_run=false + allow_provider_api_calls=true + fake provider → 202 queued
//
// Verifies that the real provider-sync path is now implemented: the job is
// accepted (202) rather than refused.  A zero-network fake provider is
// injected so no TwelveData API calls are made.  No DB is wired, so the
// background task will fail at DB insert — but the key invariant proven here
// is that the route no longer returns "not_implemented".
#[tokio::test]
async fn pd_04_provider_real_with_allow_queued() {
    let (mut st_raw, _) = make_provider_router_with_registry_raw();
    st_raw.set_provider_client_for_test(Arc::new(FakeProvider::empty()));
    let st = Arc::new(st_raw);
    let router = routes::build_router(Arc::clone(&st));

    let (status, body) = post_provider_job(
        router,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2026-01-01",
            "end": "2026-01-10"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "real provider path must → 202 (queued): {body}"
    );
    assert!(
        json_bool(&body, "accepted"),
        "accepted must be true: {body}"
    );

    let job_status = json_str(&body, "status");
    assert_eq!(
        job_status, "queued",
        "status must be 'queued', got: {job_status}"
    );

    // dry_run must be false; provider_api_calls_allowed must be true.
    assert_eq!(
        body["dry_run"],
        serde_json::Value::Bool(false),
        "dry_run must be false: {body}"
    );
    assert_eq!(
        body["provider_api_calls_allowed"],
        serde_json::Value::Bool(true),
        "provider_api_calls_allowed must be true: {body}"
    );

    // api_calls_made must be 0 at acceptance time.
    assert_eq!(
        body["api_calls_made"].as_i64().unwrap_or(999),
        0,
        "api_calls_made must be 0 at acceptance: {body}"
    );

    // job_id must be a non-nil UUID.
    let job_id_str = json_str(&body, "job_id");
    let job_id: uuid::Uuid = job_id_str.parse().expect("job_id must be a valid UUID");
    assert!(!job_id.is_nil(), "job_id must be non-nil");
}

// PD-05: source=twelvedata without mode → 400 (IJ-05 behaviour preserved)
#[tokio::test]
async fn pd_05_twelvedata_without_mode_still_refused() {
    let (_, router) = make_provider_router_with_registry();
    let (status, body) = post_provider_job(
        router,
        serde_json::json!({
            "source": "twelvedata",
            "timeframe": "1D"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "must → 400 without mode: {body}"
    );
    assert!(
        !json_bool(&body, "accepted"),
        "accepted must be false: {body}"
    );
    assert_eq!(json_str(&body, "source"), "twelvedata", "source must echo");

    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("mode") || err.contains("twelvedata"),
        "error must mention mode or source: {err}"
    );
}

// PD-06: invalid registry path → job fails with registry_load_failed error
#[tokio::test]
async fn pd_06_invalid_registry_path_job_fails_truthfully() {
    // Use a router pointing at a guaranteed-nonexistent registry path.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let provider_registry_path = std::path::PathBuf::from(manifest_dir)
        .join("../../../config/providers/providers.json")
        .canonicalize()
        .expect("provider registry must resolve")
        .to_string_lossy()
        .to_string();

    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path =
        "/nonexistent/path/that/cannot/exist/equities_phantom.json".to_string();
    st.provider_registry_path = provider_registry_path;
    let st = Arc::new(st);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;

    // Job should be accepted (registry check is async).
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "POST must → 202 even with bad path: {body}"
    );
    let job_id = json_str(&body, "job_id").to_string();

    // Wait for background task to attempt and fail registry load.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let (status, body) = get_job_status(router_get, &job_id).await;
    assert_eq!(status, StatusCode::OK);

    let job_status = json_str(&body, "status");
    assert_eq!(
        job_status, "failed",
        "job must fail when registry path is invalid, got: {job_status}"
    );

    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("registry") || err.contains("failed") || err.contains("not found"),
        "error must describe registry load failure: {err}"
    );

    // api_calls_made must be 0 — never got as far as a provider call.
    assert_eq!(
        body["api_calls_made"].as_i64().unwrap_or(999),
        0,
        "api_calls_made must be 0 on registry failure: {body}"
    );
}

// ---------------------------------------------------------------------------
// DATA-PROVIDER-FOUNDATION-01: Provider registry + asset_class validation tests
//
// | Test   | What it proves                                                               |
// |--------|------------------------------------------------------------------------------|
// | PD-07  | asset_class="futures" for twelvedata → 400 refused (unsupported by provider) |
// | PD-08  | asset_class="invalid_xyz" → 400 refused (not a known asset class)           |
// | PD-09  | provider_enabled + verification_status reported in completed dry-run job     |
//
// Uses make_full_registry_router which sets BOTH instrument and provider registry paths.
// No provider API calls. No DB writes. No TwelveData credits consumed.
// ---------------------------------------------------------------------------

/// Return a router + state pointing at BOTH canonical registries (instruments + providers).
fn make_full_registry_router() -> (Arc<state::AppState>, axum::Router) {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let instrument_registry_path = std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/equities.json")
        .canonicalize()
        .expect("instrument registry must resolve")
        .to_string_lossy()
        .to_string();
    let provider_registry_path = std::path::PathBuf::from(manifest_dir)
        .join("../../../config/providers/providers.json")
        .canonicalize()
        .expect("provider registry must resolve")
        .to_string_lossy()
        .to_string();

    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = instrument_registry_path;
    st.provider_registry_path = provider_registry_path;
    let st = Arc::new(st);
    let router = routes::build_router(Arc::clone(&st));
    (st, router)
}

fn write_provider_registry(entries: serde_json::Value) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("temp provider registry dir");
    let path = dir.path().join("providers.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&entries).unwrap())
        .expect("write temp provider registry");
    (dir, path.to_string_lossy().to_string())
}

// PD-07: twelvedata + asset_class="futures" → 400 refused (provider does not support futures)
#[tokio::test]
async fn pd_07_unsupported_asset_class_refused() {
    let (_, router) = make_full_registry_router();
    let (status, body) = post_provider_job(
        router,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "asset_class": "futures",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unsupported asset_class must → 400: {body}"
    );
    assert!(
        !json_bool(&body, "accepted"),
        "accepted must be false: {body}"
    );

    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("futures") || err.contains("asset_class") || err.contains("not support"),
        "error must describe unsupported asset_class, got: {err}"
    );

    // api_calls_made must be 0 — no provider call was made.
    assert_eq!(
        body["api_calls_made"].as_i64().unwrap_or(999),
        0,
        "api_calls_made must be 0 on refusal: {body}"
    );
}

// DATA-PROVIDER-REGISTRY-FACTORY-01: Alpaca dry-run is selectable and makes zero provider calls.
#[tokio::test]
async fn provider_registry_alpaca_dry_run_completes_zero_provider_calls() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (mut st_raw, _) = make_provider_router_with_registry_raw();
    st_raw.set_provider_client_for_test(Arc::new(SlowCountingProvider {
        calls: Arc::clone(&calls),
        delay: std::time::Duration::from_millis(1),
    }));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "alpaca",
            "mode": "sync_provider",
            "timeframe": "5m",
            "symbols_source": "registry",
            "asset_class": "equity",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED, "POST must → 202: {body}");
    let job_id = json_str(&body, "job_id").to_string();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let (status, body) = get_job_status(router_get, &job_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_str(&body, "status"), "dry_run_completed");
    assert_eq!(body["source"], "alpaca");
    assert_eq!(body["api_calls_made"].as_i64().unwrap_or(999), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

// DATA-PROVIDER-REGISTRY-FACTORY-01: unknown provider ids are refused from registry.
#[tokio::test]
async fn provider_registry_unknown_provider_refused_truthfully() {
    let (_, router) = make_full_registry_router();
    let (status, body) = post_provider_job(
        router,
        serde_json::json!({
            "source": "not_registered_provider",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "asset_class": "equity",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "must → 400: {body}");
    assert!(!json_bool(&body, "accepted"));
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("not registered") || err.contains("provider"),
        "error must describe unknown provider: {err}"
    );
    assert_eq!(body["api_calls_made"].as_i64().unwrap_or(999), 0);
}

// DATA-PROVIDER-REGISTRY-FACTORY-01: disabled provider ids are refused from registry.
#[tokio::test]
async fn provider_registry_disabled_provider_refused_truthfully() {
    let (_, router) = make_full_registry_router();
    let (status, body) = post_provider_job(
        router,
        serde_json::json!({
            "source": "alphavantage",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "asset_class": "equity",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "must → 400: {body}");
    assert!(!json_bool(&body, "accepted"));
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("disabled") || err.contains("enabled=false"),
        "error must describe disabled provider: {err}"
    );
    assert_eq!(body["api_calls_made"].as_i64().unwrap_or(999), 0);
}

// DATA-PROVIDER-REGISTRY-FACTORY-01: provider without historical capability is refused.
#[tokio::test]
async fn provider_registry_provider_without_historical_capability_refused() {
    let (_dir, provider_registry_path) = write_provider_registry(serde_json::json!([
        {
            "provider_id": "fake",
            "display_name": "Fake No Historical",
            "asset_classes": ["equity"],
            "free_tier_available": true,
            "api_key_required": false,
            "credential_env_vars": [],
            "rate_limit_notes": "test",
            "supported_timeframes": [],
            "historical_depth_notes": "none",
            "realtime_support_notes": "none",
            "licensing_notes": "test",
            "implementation_status": "implemented_equity_provider",
            "enabled": true,
            "verification_status": "repo_implemented_official_limits_unverified",
            "docs_url": ""
        }
    ]));
    let (mut st_raw, _) = make_provider_router_with_registry_raw();
    st_raw.provider_registry_path = provider_registry_path;
    let st = Arc::new(st_raw);

    let (status, body) = post_provider_job(
        routes::build_router(Arc::clone(&st)),
        serde_json::json!({
            "source": "fake",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "asset_class": "equity",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "must → 400: {body}");
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("historical_bars"),
        "error must describe missing historical capability: {err}"
    );
    assert_eq!(body["api_calls_made"].as_i64().unwrap_or(999), 0);
}

// PD-08: unknown asset_class value → 400 refused immediately (not a valid class)
#[tokio::test]
async fn pd_08_invalid_asset_class_refused() {
    let (_, router) = make_full_registry_router();
    let (status, body) = post_provider_job(
        router,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "asset_class": "invalid_xyz",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "invalid asset_class must → 400: {body}"
    );
    assert!(
        !json_bool(&body, "accepted"),
        "accepted must be false: {body}"
    );

    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("invalid_xyz") || err.contains("asset_class") || err.contains("unsupported"),
        "error must describe invalid asset_class, got: {err}"
    );
}

// PD-09: completed dry-run job reports provider_enabled=true and verification_status
#[tokio::test]
async fn pd_09_dry_run_reports_provider_registry_fields() {
    let (st, _) = make_full_registry_router();

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "asset_class": "equity",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "POST must → 202: {body}");
    let job_id = json_str(&body, "job_id").to_string();

    // Wait for background task.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let (status, body) = get_job_status(router_get, &job_id).await;
    assert_eq!(status, StatusCode::OK);

    let job_status = json_str(&body, "status");
    assert_eq!(
        job_status, "dry_run_completed",
        "job must be dry_run_completed: {body}"
    );

    // provider_enabled must be true (twelvedata is enabled in registry).
    assert_eq!(
        body["provider_enabled"],
        serde_json::Value::Bool(true),
        "provider_enabled must be true for twelvedata: {body}"
    );

    // provider_verification_status must be populated.
    assert!(
        body["provider_verification_status"].is_string(),
        "provider_verification_status must be a string: {body}"
    );

    // asset_class must echo back correctly.
    assert_eq!(
        body["asset_class"], "equity",
        "asset_class must be echoed correctly: {body}"
    );

    // Safety: api_calls_made must remain 0.
    assert_eq!(
        body["api_calls_made"].as_i64().unwrap_or(999),
        0,
        "api_calls_made must be 0: {body}"
    );
}

// ---------------------------------------------------------------------------
// DATA-INGEST-DAEMON-PROVIDER-JOBS-01: Real provider path proof tests
//
// | Test   | What it proves                                                                |
// |--------|-------------------------------------------------------------------------------|
// | PD-10  | Real path with fake provider: job runs, api_calls_made > 0                   |
// | PD-11  | Missing API key without injected client: job queues then fails truthfully     |
// | PD-12  | api_credits_per_minute guardrail stops batch at cap                           |
// | PD-13  | Partial success: failing symbols tracked, non-zero symbols_failed             |
//
// All tests use FakeProvider (zero-network). No TwelveData credentials required.
// No DB is wired in these tests; jobs reach `failed` at the DB insert step
// (except PD-12 which is halted by guardrail before any DB call).
// This is expected and proves the fail-closed behavior on missing DB.
// ---------------------------------------------------------------------------

// PD-10: real provider path runs, makes api calls via fake provider, fails at DB (no pool)
#[tokio::test]
async fn pd_10_real_provider_job_runs_with_fake_provider() {
    let (mut st_raw, _) = make_provider_router_with_registry_raw();
    st_raw.set_provider_client_for_test(Arc::new(FakeProvider::empty()));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2026-01-01",
            "end": "2026-01-05"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED, "must → 202 (queued): {body}");
    let job_id = json_str(&body, "job_id").to_string();

    // Wait for background task to complete. Provider-scoped fake symbols × instant = fast.
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let (_, body) = get_job_status(router_get, &job_id).await;

    // Job must have finished (not still running).
    let job_status = json_str(&body, "status");
    assert!(
        job_status == "failed" || job_status == "completed" || job_status == "partial",
        "job must have reached terminal status, got: {job_status}. body: {body}"
    );

    // api_calls_made must be > 0 — the fake provider was called.
    let api_calls = body["api_calls_made"].as_i64().unwrap_or(0);
    assert!(
        api_calls > 0,
        "api_calls_made must be > 0; fake provider was called for each symbol. got: {api_calls}"
    );

    // symbols_count must be the TwelveData/1D provider-scoped registry size
    // (AAPL is excluded: it is Alpaca/5m-only — see PROV-SCOPE-01 above).
    let expected =
        expected_registry_symbols_for_provider_timeframe("twelvedata", "equity", "1D").len()
            as u64;
    let symbols_count = body["symbols_count"].as_u64().unwrap_or(0);
    assert_eq!(
        symbols_count, expected,
        "symbols_count must be {expected}, got: {symbols_count}"
    );

    // dry_run must be false; provider_api_calls_allowed must be true.
    assert_eq!(body["dry_run"], serde_json::Value::Bool(false));
    assert_eq!(
        body["provider_api_calls_allowed"],
        serde_json::Value::Bool(true)
    );
}

// DATA-PROVIDER-REGISTRY-FACTORY-01: non-TwelveData real path can use fake provider seam.
//
// DAILY-DATA-READINESS-01B-PROVIDER-CONTRACT-INTEGRATION-01 (§B2.3): the real
// canonical registry (`config/instruments/equities.json`) assigns every
// enabled equity to provider "twelvedata" — none to "alpaca". Historical
// provider-sync now correctly scopes symbol resolution to instruments
// actually assigned to the requested provider (never claims provenance for
// an instrument it was never configured to fetch), so this test must use its
// own registry fixture carrying an "alpaca"-assigned instrument to keep
// proving the real path with a non-TwelveData provider.
#[tokio::test]
async fn provider_registry_alpaca_real_provider_job_runs_with_fake_provider() {
    let registry = tempfile::NamedTempFile::new().expect("create fake instrument registry");
    std::fs::write(
        registry.path(),
        br#"[{
          "instrument_id": "equity:US:ZZALPACAFAKE",
          "symbol": "ZZALPACAFAKE",
          "asset_class": "equity",
          "provider": "alpaca",
          "provider_symbol": "ZZALPACAFAKE",
          "venue": "TEST",
          "currency": "USD",
          "enabled": true,
          "timeframes": ["5m"],
          "notes": "test fixture: alpaca-assigned instrument"
        }]"#,
    )
    .expect("write fake instrument registry");

    let (mut st_raw, _) = make_provider_router_with_registry_raw();
    st_raw.instrument_registry_path = registry.path().to_string_lossy().to_string();
    st_raw.set_provider_client_for_test(Arc::new(FakeProvider::empty()));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "alpaca",
            "mode": "sync_provider",
            "timeframe": "5m",
            "symbols_source": "registry",
            "asset_class": "equity",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2026-01-01",
            "end": "2026-01-05"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED, "must → 202 (queued): {body}");
    let job_id = json_str(&body, "job_id").to_string();

    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let (_, body) = get_job_status(router_get, &job_id).await;

    let job_status = json_str(&body, "status");
    assert!(
        job_status == "failed" || job_status == "completed" || job_status == "partial",
        "job must have reached terminal status, got: {job_status}. body: {body}"
    );
    assert_eq!(body["source"], "alpaca");
    assert!(
        body["api_calls_made"].as_i64().unwrap_or(0) > 0,
        "fake provider must be called on real path: {body}"
    );
}

// PD-11: missing API key without injected client → job is queued but then fails truthfully
#[tokio::test]
async fn pd_11_missing_api_key_job_fails_truthfully() {
    // No provider_client injected, no TWELVEDATA_API_KEY set in test environment.
    // Ensure the env var is absent for this test.
    std::env::remove_var("TWELVEDATA_API_KEY");

    let (st_raw, _) = make_provider_router_with_registry_raw();
    // Do NOT inject a provider client — this exercises the env-var fallback path.
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2026-01-01",
            "end": "2026-01-05"
        }),
    )
    .await;

    // Job must be accepted (202); the API key check is async.
    assert_eq!(status, StatusCode::ACCEPTED, "must → 202 (queued): {body}");
    let job_id = json_str(&body, "job_id").to_string();

    // Wait for background task to discover missing API key.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let (_, body) = get_job_status(router_get, &job_id).await;

    let job_status = json_str(&body, "status");
    assert_eq!(
        job_status, "failed",
        "job must fail when API key is missing, got: {job_status}"
    );

    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("TWELVEDATA_API_KEY") || err.contains("api key") || err.contains("missing"),
        "error must describe missing API key, got: {err}"
    );

    // api_calls_made must be 0 — never got to make a provider call.
    assert_eq!(
        body["api_calls_made"].as_i64().unwrap_or(999),
        0,
        "api_calls_made must be 0 on missing key: {body}"
    );
}

// PD-12: api_credits_per_minute guardrail stops batch before exceeding cap
#[tokio::test]
async fn pd_12_api_credits_per_minute_guardrail_stops_batch() {
    let (mut st_raw, _) = make_provider_router_with_registry_raw();
    st_raw.set_provider_client_for_test(Arc::new(FakeProvider::empty()));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    // Cap at 3 API calls — should stop after 3 symbols out of the provider-scoped set.
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2026-01-01",
            "end": "2026-01-05",
            "api_credits_per_minute": 3
        }),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED, "must → 202 (queued): {body}");
    let job_id = json_str(&body, "job_id").to_string();

    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let (_, body) = get_job_status(router_get, &job_id).await;

    let job_status = json_str(&body, "status");
    // After hitting the guardrail with some symbols done and some skipped:
    // - if no DB: status is "failed" (db error overrides partial)
    // - if guardrail fires before any symbol completes and DB OK: "failed"
    // - if some symbols completed before guardrail and DB OK: "partial"
    // Since we have no DB here, the terminal is "failed" (db error path).
    // The key invariant: api_calls_made <= 3 (guardrail enforced).
    let api_calls = body["api_calls_made"].as_i64().unwrap_or(999);
    assert!(
        api_calls <= 3,
        "api_calls_made must be <= 3 (guardrail cap), got: {api_calls}"
    );

    // symbols_count must be the TwelveData/1D provider-scoped registry size
    // (AAPL is excluded: it is Alpaca/5m-only — see PROV-SCOPE-01 above).
    let expected =
        expected_registry_symbols_for_provider_timeframe("twelvedata", "equity", "1D").len()
            as u64;
    let symbols_count = body["symbols_count"].as_u64().unwrap_or(0);
    assert_eq!(
        symbols_count, expected,
        "symbols_count must be {expected} (provider-scoped registry size), got: {symbols_count}"
    );

    // Job must have reached a terminal state.
    assert!(
        job_status == "failed" || job_status == "partial" || job_status == "completed",
        "job must be terminal, got: {job_status}"
    );
}

// PD-13: per-symbol failure tracking — symbols_failed is non-zero for failing symbols
#[tokio::test]
async fn pd_13_per_symbol_failure_tracked() {
    // The provider-sync plan processes the TwelveData/1D provider-scoped set
    // in alphabetical order (AAPL is excluded: it is Alpaca/5m-only — see
    // PROV-SCOPE-01 above), and the guardrail below caps the batch at 2 API
    // calls. Fail exactly the first two symbols that will actually be
    // attempted so this test keeps proving per-symbol failure tracking
    // regardless of future registry edits, instead of hardcoding symbols
    // that may no longer be reached within the guardrail cap.
    let scoped_symbols =
        expected_registry_symbols_for_provider_timeframe("twelvedata", "equity", "1D");
    let first_two_symbols: Vec<String> = scoped_symbols.into_iter().take(2).collect();
    assert_eq!(
        first_two_symbols.len(),
        2,
        "provider-scoped registry must have at least 2 symbols to exercise this guardrail"
    );

    let (mut st_raw, _) = make_provider_router_with_registry_raw();
    // FakeProvider that fails for the first two symbols the guardrail will
    // actually reach (small cap so the test runs quickly; both calls fail).
    st_raw.set_provider_client_for_test(Arc::new(FakeProvider::failing(
        first_two_symbols.clone(),
    )));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    // Cap to 2 symbols via per-day guardrail to keep test fast.
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2026-01-01",
            "end": "2026-01-05",
            "api_credits_per_day": 2
        }),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED, "must → 202 (queued): {body}");
    let job_id = json_str(&body, "job_id").to_string();

    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let (_, body) = get_job_status(router_get, &job_id).await;

    // symbols_count must be the TwelveData/1D provider-scoped registry size
    // (AAPL is excluded: it is Alpaca/5m-only — see PROV-SCOPE-01 above).
    let expected_symbols_count =
        expected_registry_symbols_for_provider_timeframe("twelvedata", "equity", "1D").len()
            as u64;
    let symbols_count = body["symbols_count"].as_u64().unwrap_or(0);
    assert_eq!(
        symbols_count, expected_symbols_count,
        "symbols_count must be {expected_symbols_count}: {body}"
    );

    // api_calls_made must be <= 2 (guardrail).
    let api_calls = body["api_calls_made"].as_i64().unwrap_or(999);
    assert!(
        api_calls <= 2,
        "api_calls_made must be <= 2 (per_day guardrail), got: {api_calls}"
    );

    // symbols_failed must be > 0 (at least one of the first two provider-scoped
    // symbols, which FakeProvider was configured to fail, was attempted).
    let symbols_failed = body["symbols_failed"].as_u64().unwrap_or(0);
    assert!(
        symbols_failed > 0,
        "symbols_failed must be > 0 (fake provider fails for {first_two_symbols:?}), got: {symbols_failed}"
    );

    // Job must have reached a terminal state.
    let job_status = json_str(&body, "status");
    assert!(
        job_status == "failed" || job_status == "partial",
        "job must be terminal (failed or partial), got: {job_status}"
    );
}

// DB-01: provider dry-run job persists create/progress/terminal state and fresh state can read it.
#[tokio::test]
async fn db_01_provider_dry_run_persists_and_fresh_state_reads() {
    let Some(pool) = ingest_job_db_pool_or_skip("DB-01").await else {
        return;
    };

    let (mut st_raw, _) = make_provider_router_with_registry_raw();
    st_raw.db = Some(pool.clone());
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;

    assert_eq!(status, StatusCode::ACCEPTED, "dry-run must queue: {body}");
    let job_id: uuid::Uuid = json_str(&body, "job_id")
        .parse()
        .expect("job_id must parse");

    let persisted =
        wait_for_persisted_status(&pool, job_id, &["dry_run_completed", "failed"]).await;
    assert_eq!(
        persisted.status.as_str(),
        "dry_run_completed",
        "dry-run must complete without provider calls"
    );
    assert_eq!(persisted.api_calls_made, 0, "dry-run must make zero calls");
    // TwelveData/1D provider-scoped registry size (AAPL is excluded: it is
    // Alpaca/5m-only — see PROV-SCOPE-01 above).
    let expected_symbols_count =
        expected_registry_symbols_for_provider_timeframe("twelvedata", "equity", "1D").len();
    assert_eq!(persisted.symbols_count, Some(expected_symbols_count));

    let mut fresh =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    fresh.db = Some(pool.clone());
    let fresh = Arc::new(fresh);

    let router_status = routes::build_router(Arc::clone(&fresh));
    let (status, body) = get_job_status(router_status, &job_id.to_string()).await;
    assert_eq!(status, StatusCode::OK, "fresh state status must read DB");
    assert_eq!(json_str(&body, "status"), "dry_run_completed");
    assert_eq!(body["api_calls_made"].as_i64().unwrap_or(-1), 0);

    let router_list = routes::build_router(fresh);
    let req = Request::builder()
        .uri("/api/v1/ingest/jobs")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, resp) = call(router_list, req).await;
    assert_eq!(status, StatusCode::OK, "fresh state list must read DB");
    let json = parse_json(resp);
    let jobs = json["jobs"].as_array().expect("jobs must be array");
    let job_id_str = job_id.to_string();
    assert!(
        jobs.iter()
            .any(|j| j["job_id"].as_str() == Some(job_id_str.as_str())),
        "fresh state list must include persisted job {job_id}: {json}"
    );

    cleanup_ingest_job(&pool, job_id).await;
}

// DB-02: fake-provider real sync uses no network and persists progress/final state.
#[tokio::test]
async fn db_02_fake_provider_real_sync_persists_progress_and_terminal_state() {
    let Some(pool) = ingest_job_db_pool_or_skip("DB-02").await else {
        return;
    };

    let (mut st_raw, _) = make_provider_router_with_registry_raw();
    st_raw.db = Some(pool.clone());
    st_raw.set_provider_client_for_test(Arc::new(FakeProvider::empty()));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2026-01-01",
            "end": "2026-01-05"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "real fake-provider job must queue: {body}"
    );
    let job_id: uuid::Uuid = json_str(&body, "job_id")
        .parse()
        .expect("job_id must parse");

    let persisted =
        wait_for_persisted_status(&pool, job_id, &["completed", "partial", "failed"]).await;
    assert_eq!(
        persisted.status.as_str(),
        "completed",
        "empty fake-provider bars should complete with zero inserted rows"
    );
    assert!(
        persisted.api_calls_made > 0,
        "fake provider must be called through the real sync path"
    );
    // TwelveData/1D provider-scoped registry size (AAPL is excluded: it is
    // Alpaca/5m-only — see PROV-SCOPE-01 above).
    let expected_symbols_count =
        expected_registry_symbols_for_provider_timeframe("twelvedata", "equity", "1D").len();
    assert_eq!(persisted.symbols_count, Some(expected_symbols_count));
    assert_eq!(persisted.symbols_completed, Some(expected_symbols_count));
    assert_eq!(persisted.rows_inserted, Some(0));
    assert_eq!(persisted.rows_rejected, Some(0));

    cleanup_ingest_job(&pool, job_id).await;
}

// DB-03: refused provider job persists refused status/reason when DB is configured.
#[tokio::test]
async fn db_03_refused_provider_job_persists_reason() {
    let Some(pool) = ingest_job_db_pool_or_skip("DB-03").await else {
        return;
    };

    let (mut st_raw, _) = make_provider_router_with_registry_raw();
    st_raw.db = Some(pool.clone());
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": false
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "job must be refused: {body}"
    );
    let job_id: uuid::Uuid = json_str(&body, "job_id")
        .parse()
        .expect("job_id must parse");
    assert!(!job_id.is_nil(), "refused persisted jobs need a durable id");

    let persisted = mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
        .await
        .expect("load refused job")
        .expect("refused job must be persisted");
    assert_eq!(persisted.status.as_str(), "refused");
    let err = persisted.error.as_deref().unwrap_or("");
    assert!(
        err.contains("allow_provider_api_calls") || err.contains("provider API calls"),
        "refusal reason must be persisted, got: {err}"
    );
    assert_eq!(persisted.api_calls_made, 0);

    cleanup_ingest_job(&pool, job_id).await;
}

// DB-04: cancel route persists cancelled status/reason when DB is configured.
#[tokio::test]
async fn db_04_cancel_persists_cancelled_status_and_reason() {
    let Some(pool) = ingest_job_db_pool_or_skip("DB-04").await else {
        return;
    };

    let mut st_raw =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st_raw.db = Some(pool.clone());
    let st = Arc::new(st_raw);

    let job_id = uuid::Uuid::new_v4();
    let record = seed_job_record(job_id, IngestJobStatus::Queued);
    mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &record)
        .await
        .expect("persist queued ingest job");

    let router_cancel = routes::build_router(Arc::clone(&st));
    let (status, body) = post_cancel_job(router_cancel, job_id).await;
    assert_eq!(status, StatusCode::ACCEPTED, "DB cancel must → 202");
    assert_eq!(json_str(&body, "truth_state"), "cancel_accepted");
    assert_eq!(json_str(&body, "status"), "cancelled");

    let persisted = mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
        .await
        .expect("load cancelled job")
        .expect("cancelled job must be persisted");
    assert_eq!(persisted.status.as_str(), "cancelled");
    let err = persisted.error.as_deref().unwrap_or("");
    assert!(
        err.contains("cancel requested by operator"),
        "cancel reason must be persisted, got: {err}"
    );
    assert!(
        persisted.completed_at_utc.is_some(),
        "cancelled persisted job must have completed_at_utc"
    );

    cleanup_ingest_job(&pool, job_id).await;
}

// DB-05: after a DB-backed cancel, GET status and GET list both read the
// cancelled status back correctly (not just the raw persisted row).
#[tokio::test]
async fn db_05_get_and_list_read_cancelled_after_db_cancel() {
    let Some(pool) = ingest_job_db_pool_or_skip("DB-05").await else {
        return;
    };

    let mut st_raw =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st_raw.db = Some(pool.clone());
    let st = Arc::new(st_raw);

    let job_id = uuid::Uuid::new_v4();
    let record = seed_job_record(job_id, IngestJobStatus::Queued);
    mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &record)
        .await
        .expect("persist queued ingest job");

    let router_cancel = routes::build_router(Arc::clone(&st));
    let (status, _body) = post_cancel_job(router_cancel, job_id).await;
    assert_eq!(status, StatusCode::ACCEPTED, "DB cancel must → 202");

    // A brand new process-local state (empty in-memory store) forces the GET
    // routes below to prove the *durable* row, not a lucky in-memory cache.
    let mut fresh =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    fresh.db = Some(pool.clone());
    let fresh = Arc::new(fresh);

    let router_status = routes::build_router(Arc::clone(&fresh));
    let (status, body) = get_job_status(router_status, &job_id.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json_str(&body, "status"),
        "cancelled",
        "GET status must read cancelled from DB"
    );

    let router_list = routes::build_router(fresh);
    let req = Request::builder()
        .uri("/api/v1/ingest/jobs")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router_list, req).await;
    assert_eq!(status, StatusCode::OK);
    let list_body = parse_json(body);
    let jobs = list_body["jobs"]
        .as_array()
        .expect("list response must have a jobs array");
    let found = jobs
        .iter()
        .find(|j| j["job_id"].as_str() == Some(job_id.to_string().as_str()))
        .unwrap_or_else(|| panic!("cancelled job must appear in list: {list_body}"));
    assert_eq!(found["status"].as_str(), Some("cancelled"));

    cleanup_ingest_job(&pool, job_id).await;
}

// DB-06: repeated cancel of an already-cancelled DB-backed job is idempotent
// — second call returns already_terminal, accepted=false, and does not
// disturb the durable row.
#[tokio::test]
async fn db_06_repeated_cancel_on_db_job_is_idempotent() {
    let Some(pool) = ingest_job_db_pool_or_skip("DB-06").await else {
        return;
    };

    let mut st_raw =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st_raw.db = Some(pool.clone());
    let st = Arc::new(st_raw);

    let job_id = uuid::Uuid::new_v4();
    let record = seed_job_record(job_id, IngestJobStatus::Queued);
    mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &record)
        .await
        .expect("persist queued ingest job");

    let router_first = routes::build_router(Arc::clone(&st));
    let (status, body) = post_cancel_job(router_first, job_id).await;
    assert_eq!(status, StatusCode::ACCEPTED, "first cancel must → 202");
    assert_eq!(json_str(&body, "status"), "cancelled");

    let first_persisted = mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
        .await
        .expect("load after first cancel")
        .expect("row must exist");

    let router_second = routes::build_router(Arc::clone(&st));
    let (status, body) = post_cancel_job(router_second, job_id).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "repeated cancel must → 200, not 202"
    );
    assert_eq!(json_str(&body, "truth_state"), "already_terminal");
    assert!(
        !json_bool(&body, "accepted"),
        "repeated cancel must not be accepted: {body}"
    );
    assert_eq!(json_str(&body, "status"), "cancelled");

    let second_persisted = mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
        .await
        .expect("load after second cancel")
        .expect("row must still exist");
    assert_eq!(
        first_persisted.completed_at_utc, second_persisted.completed_at_utc,
        "repeated cancel must not rewrite the durable terminal row"
    );

    cleanup_ingest_job(&pool, job_id).await;
}

// DB-07: a job never seen by this process's in-memory store (loaded fresh
// from DB) can be cancelled, persisted, and cached coherently.
#[tokio::test]
async fn db_07_db_only_job_cancellation_caches_coherent_memory() {
    let Some(pool) = ingest_job_db_pool_or_skip("DB-07").await else {
        return;
    };

    let job_id = uuid::Uuid::new_v4();
    let record = seed_job_record(job_id, IngestJobStatus::Running);
    mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &record)
        .await
        .expect("persist running ingest job");

    // Fresh AppState: this job_id has never been inserted into its
    // in-memory store, so cancel must take the DB-only load-then-cancel path.
    let mut st_raw =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st_raw.db = Some(pool.clone());
    let st = Arc::new(st_raw);

    let router_cancel = routes::build_router(Arc::clone(&st));
    let (status, body) = post_cancel_job(router_cancel, job_id).await;
    assert_eq!(status, StatusCode::ACCEPTED, "DB-only cancel must → 202");
    assert_eq!(json_str(&body, "truth_state"), "cancel_accepted");
    assert_eq!(json_str(&body, "status"), "cancelled");

    let persisted = mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
        .await
        .expect("load cancelled job")
        .expect("cancelled job must be persisted");
    assert_eq!(persisted.status.as_str(), "cancelled");

    // The record must now also be cached in this process's in-memory store
    // (same st, same Arc<Mutex<HashMap>>), coherent with the durable row.
    {
        let cached = st
            .ingest_jobs
            .lock()
            .expect("ingest_jobs lock poisoned")
            .get(&job_id)
            .cloned()
            .expect("DB-only job must be cached in memory after cancel");
        assert_eq!(cached.status, IngestJobStatus::Cancelled);
        // Postgres truncates timestamptz to microsecond precision, so compare
        // at that precision rather than requiring exact nanosecond equality.
        assert_eq!(
            cached.completed_at_utc.map(|t| t.timestamp_micros()),
            persisted.completed_at_utc.map(|t| t.timestamp_micros())
        );
    }

    cleanup_ingest_job(&pool, job_id).await;
}

// DB-08: when durable persistence fails mid-cancel, the route returns
// 503/accepted=false and leaves BOTH memory and the durable row at their
// pre-request truth (no partial/false terminal state anywhere) — then a
// retry against a healthy DB produces one coherent cancelled state.
#[tokio::test]
async fn db_08_persistence_failure_then_retry_produces_one_coherent_cancel() {
    let Some(pool) = ingest_job_db_pool_or_skip("DB-08").await else {
        return;
    };

    let job_id = uuid::Uuid::new_v4();
    let record = seed_job_record(job_id, IngestJobStatus::Queued);
    // Seed the real, reachable durable row directly (not through the broken
    // AppState below) so DB pre-request truth is established independently.
    mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &record)
        .await
        .expect("persist queued ingest job");

    // A lazily-connecting pool against an unreachable address: connect()
    // never blocks (lazy), but any query against it fails — a self-contained
    // "DB became unreachable mid-request" fixture that never touches the
    // real isolated test pool. Mirrors the established idiom in
    // scenario_durable_paper_portfolio_snapshot_persistence_01.rs.
    let unreachable_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/mqk_test?sslmode=disable")
        .expect("lazy connect should not fail synchronously");

    let mut st_raw =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st_raw.db = Some(unreachable_pool);
    let st = Arc::new(st_raw);
    insert_seed_job(&st, record.clone());

    let router_fail = routes::build_router(Arc::clone(&st));
    let (status, body) = post_cancel_job(router_fail, job_id).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "cancel with broken DB must → 503: {body}"
    );
    assert_eq!(json_str(&body, "truth_state"), "backend_unavailable");
    assert!(
        !json_bool(&body, "accepted"),
        "failed persistence must not be accepted: {body}"
    );

    // Memory must be untouched: still 'queued', not a fabricated 'cancelled'.
    {
        let cached = st
            .ingest_jobs
            .lock()
            .expect("ingest_jobs lock poisoned")
            .get(&job_id)
            .cloned()
            .expect("job must still be present in memory");
        assert_eq!(
            cached.status,
            IngestJobStatus::Queued,
            "memory must not be mutated to cancelled when persistence fails"
        );
    }

    // The durable row (read through the healthy pool) must also be
    // untouched.
    let after_failure = mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
        .await
        .expect("load after failed cancel")
        .expect("row must still exist");
    assert_eq!(
        after_failure.status.as_str(),
        "queued",
        "durable row must remain at pre-request truth when persistence fails"
    );

    // Retry with a healthy DB pool, sharing the same in-memory store (same
    // process, same job — only the DB reachability changed, as it would
    // across a real reconnect).
    let mut st_retry_raw =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st_retry_raw.db = Some(pool.clone());
    st_retry_raw.ingest_jobs = Arc::clone(&st.ingest_jobs);
    let st_retry = Arc::new(st_retry_raw);

    let router_retry = routes::build_router(Arc::clone(&st_retry));
    let (status, body) = post_cancel_job(router_retry, job_id).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "retry cancel must → 202: {body}"
    );
    assert_eq!(json_str(&body, "status"), "cancelled");

    let after_retry = mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
        .await
        .expect("load after retry")
        .expect("row must exist");
    assert_eq!(after_retry.status.as_str(), "cancelled");

    let cached_after_retry = st_retry
        .ingest_jobs
        .lock()
        .expect("ingest_jobs lock poisoned")
        .get(&job_id)
        .cloned()
        .expect("job must be cached after retry");
    assert_eq!(cached_after_retry.status, IngestJobStatus::Cancelled);
    // Postgres truncates timestamptz to microsecond precision, so compare at
    // that precision rather than requiring exact nanosecond equality.
    assert_eq!(
        cached_after_retry
            .completed_at_utc
            .map(|t| t.timestamp_micros()),
        after_retry.completed_at_utc.map(|t| t.timestamp_micros()),
        "memory and DB must agree on the single coherent cancelled state"
    );

    cleanup_ingest_job(&pool, job_id).await;
}

// ---------------------------------------------------------------------------
// CAS guard: persist_ingest_job_record refuses to let a non-cancel write
// overwrite a durably cancelled row (INGEST-JOB-CANCEL-STATUS-CONSTRAINT-
// REPAIR-01 follow-up). These tests call the persistence function directly
// in a caller-controlled order — "cancel commits, then a stale write
// arrives" — which reproduces the exact interleaving a real race can
// produce, deterministically and without any timing dependency: the order
// of the two DB calls in test code IS the ordering under test.
// ---------------------------------------------------------------------------

/// A background progress write (status unchanged, only counters bumped)
/// that lands after cancellation has already committed must not revert the
/// durable row away from cancelled.
#[tokio::test]
async fn db_10_cas_blocks_stale_progress_write_after_cancel_commit() {
    let Some(pool) = ingest_job_db_pool_or_skip("DB-10").await else {
        return;
    };

    let job_id = uuid::Uuid::new_v4();
    let queued = seed_job_record(job_id, IngestJobStatus::Queued);
    mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &queued)
        .await
        .expect("seed queued row");

    // The cancel commit lands first.
    let mut cancelled = queued.clone();
    cancelled.status = IngestJobStatus::Cancelled;
    cancelled.completed_at_utc = Some(chrono::Utc::now());
    cancelled.error = Some("cancel requested by operator".to_string());
    mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &cancelled)
        .await
        .expect("cancel commit must persist");

    // A background progress write that read its in-memory record *before*
    // cancellation, then finally completes its own DB round trip *after*
    // the cancel commit above — same shape as the per-symbol progress
    // persist in run_real_provider_sync.
    let mut stale_progress = queued.clone();
    stale_progress.status = IngestJobStatus::Running;
    stale_progress.api_calls_made = 5;
    stale_progress.symbols_completed = Some(3);
    stale_progress.symbols_failed = Some(0);
    mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &stale_progress)
        .await
        .expect("stale write must not error, only no-op");

    let persisted = mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
        .await
        .expect("load")
        .expect("row must exist");
    assert_eq!(
        persisted.status,
        IngestJobStatus::Cancelled,
        "stale progress write must not replace the durable cancelled status"
    );
    let err = persisted.error.as_deref().unwrap_or("");
    assert!(
        err.contains("cancel requested by operator"),
        "operator cancellation reason must survive the stale write, got: {err}"
    );
    assert_eq!(
        persisted.api_calls_made, 0,
        "the blocked write's counters must not be applied either"
    );

    cleanup_ingest_job(&pool, job_id).await;
}

/// A terminal completion write (completed/failed/partial/dry_run_completed)
/// racing in after cancellation must not replace cancelled — the guard
/// applies to every non-cancel status value, not just in-progress ones.
#[tokio::test]
async fn db_11_cas_blocks_stale_terminal_completion_write_after_cancel_commit() {
    let Some(pool) = ingest_job_db_pool_or_skip("DB-11").await else {
        return;
    };

    for stale_status in [
        IngestJobStatus::Completed,
        IngestJobStatus::Failed,
        IngestJobStatus::Partial,
        IngestJobStatus::DryRunCompleted,
    ] {
        let job_id = uuid::Uuid::new_v4();
        let queued = seed_job_record(job_id, IngestJobStatus::Running);
        mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &queued)
            .await
            .expect("seed running row");

        let mut cancelled = queued.clone();
        cancelled.status = IngestJobStatus::Cancelled;
        cancelled.completed_at_utc = Some(chrono::Utc::now());
        cancelled.error = Some("cancel requested by operator".to_string());
        mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &cancelled)
            .await
            .expect("cancel commit must persist");

        let mut stale_terminal = queued.clone();
        stale_terminal.status = stale_status.clone();
        stale_terminal.completed_at_utc = Some(chrono::Utc::now());
        stale_terminal.rows_inserted = Some(999);
        mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &stale_terminal)
            .await
            .unwrap_or_else(|e| {
                panic!("stale terminal write ({stale_status:?}) must not error: {e}")
            });

        let persisted = mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
            .await
            .expect("load")
            .expect("row must exist");
        assert_eq!(
            persisted.status,
            IngestJobStatus::Cancelled,
            "stale terminal write ({stale_status:?}) must not replace cancelled"
        );
        assert_ne!(
            persisted.rows_inserted,
            Some(999),
            "blocked terminal write's fields must not be applied ({stale_status:?})"
        );

        cleanup_ingest_job(&pool, job_id).await;
    }
}

/// Repeated stale writes (simulating a flaky/retrying background task)
/// never accumulate into flipping the row back to a non-terminal status —
/// each one independently no-ops against the same guard.
#[tokio::test]
async fn db_12_cas_blocks_repeated_stale_writes_after_cancel_commit() {
    let Some(pool) = ingest_job_db_pool_or_skip("DB-12").await else {
        return;
    };

    let job_id = uuid::Uuid::new_v4();
    let queued = seed_job_record(job_id, IngestJobStatus::Running);
    mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &queued)
        .await
        .expect("seed running row");

    let mut cancelled = queued.clone();
    cancelled.status = IngestJobStatus::Cancelled;
    cancelled.completed_at_utc = Some(chrono::Utc::now());
    cancelled.error = Some("cancel requested by operator".to_string());
    mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &cancelled)
        .await
        .expect("cancel commit must persist");

    for attempt in 0..5 {
        let mut stale = queued.clone();
        stale.status = IngestJobStatus::Running;
        stale.api_calls_made = attempt;
        mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &stale)
            .await
            .expect("stale retry write must not error");

        let persisted = mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
            .await
            .expect("load")
            .expect("row must exist");
        assert_eq!(
            persisted.status,
            IngestJobStatus::Cancelled,
            "attempt {attempt}: repeated stale write must never revert cancelled"
        );
    }

    cleanup_ingest_job(&pool, job_id).await;
}

/// The guard must not block normal writes when the job was never
/// cancelled: queued -> running -> completed all apply as before.
#[tokio::test]
async fn db_13_cas_does_not_block_normal_uncancelled_progression() {
    let Some(pool) = ingest_job_db_pool_or_skip("DB-13").await else {
        return;
    };

    let job_id = uuid::Uuid::new_v4();
    let queued = seed_job_record(job_id, IngestJobStatus::Queued);
    mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &queued)
        .await
        .expect("seed queued row");

    let mut running = queued.clone();
    running.status = IngestJobStatus::Running;
    running.api_calls_made = 3;
    mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &running)
        .await
        .expect("normal running write must apply");

    let after_running = mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
        .await
        .expect("load")
        .expect("row must exist");
    assert_eq!(after_running.status, IngestJobStatus::Running);
    assert_eq!(after_running.api_calls_made, 3);

    let mut completed = running.clone();
    completed.status = IngestJobStatus::Completed;
    completed.completed_at_utc = Some(chrono::Utc::now());
    completed.rows_inserted = Some(42);
    mqk_daemon::ingest_jobs::persist_ingest_job_record(&pool, &completed)
        .await
        .expect("normal completion write must apply");

    let after_completed = mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
        .await
        .expect("load")
        .expect("row must exist");
    assert_eq!(after_completed.status, IngestJobStatus::Completed);
    assert_eq!(after_completed.rows_inserted, Some(42));

    cleanup_ingest_job(&pool, job_id).await;
}

// ---------------------------------------------------------------------------
// Full-stack barrier proofs: a real background provider-sync task, paused
// deterministically (via `tokio::sync::Notify` barriers, never a sleep) at
// two different points relative to a real, fully-awaited cancel request.
// Proves the invariant end-to-end (HTTP route -> background task -> DB ->
// memory), not just at the persistence-function level above.
//
// db_14 pauses *before* the background task's own in-memory read (inside
// the injected provider's fetch call), so cancellation's in-memory publish
// always happens first -- this exercises the pre-existing `mutate_job`
// terminal-status guard and proves end-to-end coherence in that ordering.
//
// db_15 pauses *after* the background task's own in-memory read but
// *before* its durable persist (using `ingest_job_persist_barrier_for_test`,
// wired into `persist_job_update` in routes/ingest.rs), so the background
// write's captured record is a stale, non-cancelled snapshot by the time
// cancellation commits to the DB first -- this is the exact DB-layer race
// (a background write racing in *after* the cancel DB commit) that
// `persist_ingest_job_record`'s conditional `where` clause exists to close,
// reproduced through the real HTTP + background-task path rather than by
// calling the persistence function directly.
// ---------------------------------------------------------------------------

/// A `HistoricalProvider` that deterministically signals when it has been
/// entered and then blocks until the test releases it — the barrier that
/// makes the race window controllable instead of timing-dependent.
struct BarrierProvider {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl mqk_md::HistoricalProvider for BarrierProvider {
    fn source_name(&self) -> &'static str {
        "barrier_test_provider"
    }

    async fn fetch_bars(
        &self,
        _req: mqk_md::FetchBarsRequest,
    ) -> anyhow::Result<Vec<mqk_md::ProviderBar>> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(vec![])
    }
}

#[tokio::test]
async fn db_14_barrier_proves_background_write_cannot_overwrite_cancel_commit() {
    let Some(pool) = ingest_job_db_pool_or_skip("DB-14").await else {
        return;
    };

    let (mut st_raw, _) = make_provider_router_with_registry_raw();
    st_raw.db = Some(pool.clone());
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    st_raw.set_provider_client_for_test(Arc::new(BarrierProvider {
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
    }));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2026-01-01",
            "end": "2026-01-05"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "real job must queue: {body}");
    let job_id: uuid::Uuid = json_str(&body, "job_id")
        .parse()
        .expect("job_id must parse");

    // Deterministic barrier: block until the background task is inside
    // fetch_bars for the first symbol. It has already checked
    // ingest_job_is_cancelled()==false and is now paused exactly in the
    // check-then-act gap this patch closes -- not a timing guess.
    entered.notified().await;

    // Cancel and fully await the response: guarantees both the durable row
    // and the in-memory store are committed 'cancelled' before the
    // background task is allowed to resume.
    let router_cancel = routes::build_router(Arc::clone(&st));
    let (status, body) = post_cancel_job(router_cancel, job_id).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "cancel during in-flight fetch must → 202: {body}"
    );
    assert_eq!(json_str(&body, "status"), "cancelled");

    let persisted_before_release =
        mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
            .await
            .expect("load before release")
            .expect("row must exist");
    assert_eq!(persisted_before_release.status.as_str(), "cancelled");

    // Release the paused background task. It will complete the fetch,
    // process the (empty) result as a completed symbol, and attempt its
    // own progress persist -- exactly the racing write this patch's CAS
    // guard blocks -- before its own cancellation re-check makes it return.
    release.notify_one();

    // The background task's only remaining work after release is one more
    // DB write (blocked by the guard) and its own cancellation check before
    // returning; the race window itself was already deterministically
    // resolved by the barrier above. Poll repeatedly through that window
    // and require the durable row to read 'cancelled' on *every* single
    // observation, so even a transient overwrite would fail this test.
    for _ in 0..40 {
        let persisted = mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
            .await
            .expect("load during settle window")
            .expect("row must exist");
        assert_eq!(
            persisted.status.as_str(),
            "cancelled",
            "durable row must never be observed as anything but cancelled once cancel has committed"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // In-memory must agree with the durable row.
    let cached = st
        .ingest_jobs
        .lock()
        .expect("ingest_jobs lock poisoned")
        .get(&job_id)
        .cloned()
        .expect("job must be cached");
    assert_eq!(cached.status, IngestJobStatus::Cancelled);
    let cached_error = cached.error.as_deref().unwrap_or("");
    assert!(
        cached_error.contains("cancel requested by operator"),
        "in-memory cancellation reason must be preserved, got: {cached_error}"
    );

    cleanup_ingest_job(&pool, job_id).await;
}

/// The true DB-write race: a background progress write's `mutate_job` read
/// (capturing status=Running) completes *before* cancellation exists at
/// all, but its durable persist is deterministically held back until
/// *after* a concurrent cancel request has fully committed to the DB. This
/// reproduces "a background progress write racing after the cancel DB
/// commit" end-to-end, not just via direct sequential calls (db_10).
#[tokio::test]
async fn db_15_barrier_proves_stale_write_committed_after_cancel_db_commit_is_blocked() {
    let Some(pool) = ingest_job_db_pool_or_skip("DB-15").await else {
        return;
    };

    let (mut st_raw, _) = make_provider_router_with_registry_raw();
    st_raw.db = Some(pool.clone());
    // No delay needed here -- the pause point is the persist barrier below,
    // not the provider call, so a plain zero-network fake is sufficient.
    st_raw.set_provider_client_for_test(Arc::new(FakeProvider::empty()));
    let persist_entered = Arc::new(tokio::sync::Notify::new());
    let persist_release = Arc::new(tokio::sync::Notify::new());
    st_raw.set_ingest_job_persist_barrier_entered_for_test(Arc::clone(&persist_entered));
    st_raw.set_ingest_job_persist_barrier_for_test(Arc::clone(&persist_release));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2026-01-01",
            "end": "2026-01-05"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "real job must queue: {body}");
    let job_id: uuid::Uuid = json_str(&body, "job_id")
        .parse()
        .expect("job_id must parse");

    // Deterministic barrier: block until the background task's per-symbol
    // progress write has already read its in-memory record (status=Running,
    // captured *before* any cancellation exists) and is now paused
    // immediately before its own durable persist call.
    persist_entered.notified().await;

    // Cancel and fully await the response: the durable row and in-memory
    // store are BOTH committed 'cancelled' while the background write is
    // still paused, holding a stale non-cancelled record it hasn't
    // persisted yet.
    let router_cancel = routes::build_router(Arc::clone(&st));
    let (status, body) = post_cancel_job(router_cancel, job_id).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "cancel while background write is paused must → 202: {body}"
    );
    assert_eq!(json_str(&body, "status"), "cancelled");

    let persisted_before_release =
        mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
            .await
            .expect("load before release")
            .expect("row must exist");
    assert_eq!(persisted_before_release.status.as_str(), "cancelled");

    // Release the paused background write: it now attempts to persist its
    // stale, captured-before-cancellation record (status=Running) — landing
    // *after* the cancel commit above. This is exactly the race
    // `persist_ingest_job_record`'s conditional `where` clause blocks.
    persist_release.notify_one();

    // The background write's stale persist attempt is the only remaining
    // work; poll repeatedly through that window and require the durable row
    // to read 'cancelled' on every single observation.
    for _ in 0..40 {
        let persisted = mqk_daemon::ingest_jobs::load_persisted_ingest_job(&pool, job_id)
            .await
            .expect("load during settle window")
            .expect("row must exist");
        assert_eq!(
            persisted.status.as_str(),
            "cancelled",
            "the stale background write (captured before cancellation) must never overwrite \
             the durable cancelled row, even though it lands after the cancel DB commit"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // In-memory must also stay coherent with the durable row.
    let cached = st
        .ingest_jobs
        .lock()
        .expect("ingest_jobs lock poisoned")
        .get(&job_id)
        .cloned()
        .expect("job must be cached");
    assert_eq!(cached.status, IngestJobStatus::Cancelled);

    cleanup_ingest_job(&pool, job_id).await;
}

// ---------------------------------------------------------------------------
// DAILY-DATA-READINESS-01B2-INGEST-TRUTHFULNESS-REPAIR-01 (Repair 1):
// historical-sync symbol-mismatch truthfulness.
// ---------------------------------------------------------------------------

/// Build an `AppState` (not Arc-wrapped) pointing at a single-instrument
/// custom instrument registry (real `config/providers/providers.json`
/// unchanged), for isolating one instrument's mismatch behavior instead of
/// the real 88-symbol registry.
fn make_provider_router_with_single_instrument(
    symbol: &str,
    provider: &str,
    timeframe: &str,
) -> (state::AppState, tempfile::NamedTempFile) {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let provider_registry_path = std::path::PathBuf::from(manifest_dir)
        .join("../../../config/providers/providers.json")
        .canonicalize()
        .expect("provider registry must resolve")
        .to_string_lossy()
        .to_string();

    let registry = tempfile::NamedTempFile::new().expect("create fake instrument registry");
    let json = format!(
        r#"[{{
          "instrument_id": "equity:US:{symbol}",
          "symbol": "{symbol}",
          "asset_class": "equity",
          "provider": "{provider}",
          "provider_symbol": "{symbol}",
          "venue": "TEST",
          "currency": "USD",
          "enabled": true,
          "timeframes": ["{timeframe}"],
          "notes": "test fixture"
        }}]"#
    );
    std::fs::write(registry.path(), json).expect("write fake instrument registry");

    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = registry.path().to_string_lossy().to_string();
    st.provider_registry_path = provider_registry_path;
    (st, registry)
}

fn provider_bar(symbol: &str, timeframe: &str, end_ts: i64) -> mqk_md::ProviderBar {
    mqk_md::ProviderBar {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        end_ts,
        open: "100".to_string(),
        high: "101".to_string(),
        low: "99".to_string(),
        close: "100.5".to_string(),
        volume: 1000,
        is_complete: true,
    }
}

/// A provider response containing *only* wrong-symbol bars is a rejection,
/// never a silent success: it must not count as completed, must count as
/// failed, must not increase `rows_inserted`, and must increase
/// `rows_rejected` by the mismatched-bar count.
#[tokio::test]
async fn hist_sync_only_mismatched_bars_are_rejected_not_completed() {
    let Some(pool) = ingest_job_db_pool_or_skip("HIST-MISMATCH-01").await else {
        return;
    };

    let (mut st_raw, _registry) =
        make_provider_router_with_single_instrument("ZZHISTMISMATCH", "twelvedata", "1D");
    st_raw.db = Some(pool.clone());
    st_raw.set_provider_client_for_test(Arc::new(FakeProvider::with_bars(vec![provider_bar(
        "SOME_OTHER_SYMBOL",
        "1D",
        1_704_153_600,
    )])));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2023-01-01",
            "end": "2023-01-05"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "job must queue: {body}");
    let job_id: uuid::Uuid = json_str(&body, "job_id")
        .parse()
        .expect("job_id must parse");

    let persisted =
        wait_for_persisted_status(&pool, job_id, &["completed", "partial", "failed"]).await;
    assert_eq!(
        persisted.status.as_str(),
        "failed",
        "a response containing ONLY wrong-symbol bars must never report success: {persisted:?}"
    );
    assert_eq!(
        persisted.symbols_completed,
        Some(0),
        "must not count as completed"
    );
    assert_eq!(persisted.symbols_failed, Some(1), "must count as failed");
    assert_eq!(persisted.rows_inserted, Some(0));
    assert_eq!(
        persisted.rows_rejected,
        Some(1),
        "the mismatched bar must be counted as rejected"
    );
    let err = persisted.error.as_deref().unwrap_or("");
    assert!(
        err.contains("mismatch") || err.contains("symbol"),
        "a bounded, operator-visible mismatch reason must be recorded: {err}"
    );

    let count: i64 = sqlx::query_scalar("select count(*) from md_bars where symbol like 'ZZHISTMISMATCH%' or symbol = 'SOME_OTHER_SYMBOL'")
        .fetch_one(&pool)
        .await
        .expect("count rows for mismatched response");
    assert_eq!(
        count, 0,
        "a wrong-symbol-only response must write zero rows"
    );

    cleanup_ingest_job(&pool, job_id).await;
}

/// A provider response mixing valid matched bars with wrong-symbol bars
/// still inserts the valid rows (never discarded), still counts the
/// mismatched bars as rejected, and reports the job `partial` rather than
/// `completed`. Stored valid rows retain canonical local symbol, exact
/// provider ID, canonical provider symbol, and `historical_backfill`
/// ingest mode.
#[tokio::test]
async fn hist_sync_mixed_bars_insert_valid_rows_and_report_partial() {
    let Some(pool) = ingest_job_db_pool_or_skip("HIST-MISMATCH-02").await else {
        return;
    };

    let (mut st_raw, _registry) =
        make_provider_router_with_single_instrument("ZZHISTMIXED", "twelvedata", "1D");
    st_raw.db = Some(pool.clone());
    st_raw.set_provider_client_for_test(Arc::new(FakeProvider::with_bars(vec![
        provider_bar("ZZHISTMIXED", "1D", 1_704_153_600), // matches provider_symbol
        provider_bar("SOME_OTHER_SYMBOL", "1D", 1_704_240_000), // mismatched
    ])));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2023-01-01",
            "end": "2023-01-05"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "job must queue: {body}");
    let job_id: uuid::Uuid = json_str(&body, "job_id")
        .parse()
        .expect("job_id must parse");

    let persisted =
        wait_for_persisted_status(&pool, job_id, &["completed", "partial", "failed"]).await;
    assert_eq!(
        persisted.status.as_str(),
        "partial",
        "valid + mismatched bars together must report partial, never completed: {persisted:?}"
    );
    assert_eq!(
        persisted.rows_inserted,
        Some(1),
        "the valid matched row must not be discarded"
    );
    assert_eq!(
        persisted.rows_rejected,
        Some(1),
        "the mismatched bar must be counted as rejected"
    );

    let row = sqlx::query(
        "select symbol, provider_id, provider_symbol, ingest_mode from md_bars \
         where symbol = 'ZZHISTMIXED' and timeframe = '1D' and end_ts = $1",
    )
    .bind(1_704_153_600_i64)
    .fetch_one(&pool)
    .await
    .expect("fetch inserted valid row");
    assert_eq!(row.try_get::<String, _>("symbol").unwrap(), "ZZHISTMIXED");
    assert_eq!(
        row.try_get::<String, _>("provider_id").unwrap(),
        "twelvedata"
    );
    assert_eq!(
        row.try_get::<Option<String>, _>("provider_symbol").unwrap(),
        Some("ZZHISTMIXED".to_string()),
        "provenance must retain the canonical provider symbol"
    );
    assert_eq!(
        row.try_get::<Option<String>, _>("ingest_mode").unwrap(),
        Some("historical_backfill".to_string())
    );

    let mismatched_count: i64 =
        sqlx::query_scalar("select count(*) from md_bars where symbol = 'SOME_OTHER_SYMBOL'")
            .fetch_one(&pool)
            .await
            .expect("count mismatched rows");
    assert_eq!(
        mismatched_count, 0,
        "the mismatched bar must never be written to md_bars"
    );

    sqlx::query("delete from md_bars where symbol = 'ZZHISTMIXED'")
        .execute(&pool)
        .await
        .ok();
    cleanup_ingest_job(&pool, job_id).await;
}

// ---------------------------------------------------------------------------
// DAILY-DATA-READINESS-01B2-REGISTRY-ADMISSION-CLOSURE-01:
// registry validation + per-instrument timeframe admission for historical
// sync and dry run.
// ---------------------------------------------------------------------------

/// Build an `AppState` (not Arc-wrapped) pointing at an arbitrary instrument
/// registry JSON body (real `config/providers/providers.json` unchanged),
/// for constructing invalid or multi-instrument registry fixtures.
fn make_provider_router_with_registry_json(
    json: &str,
) -> (state::AppState, tempfile::NamedTempFile) {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let provider_registry_path = std::path::PathBuf::from(manifest_dir)
        .join("../../../config/providers/providers.json")
        .canonicalize()
        .expect("provider registry must resolve")
        .to_string_lossy()
        .to_string();

    let registry = tempfile::NamedTempFile::new().expect("create fake instrument registry");
    std::fs::write(registry.path(), json).expect("write fake instrument registry");

    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = registry.path().to_string_lossy().to_string();
    st.provider_registry_path = provider_registry_path;
    (st, registry)
}

/// A registry with a duplicate `symbol` across two entries — parseable JSON,
/// but rejected by `validate_registry`.
fn duplicate_symbol_registry_json(symbol: &str) -> String {
    format!(
        r#"[
          {{
            "instrument_id": "equity:US:{symbol}A",
            "symbol": "{symbol}",
            "asset_class": "equity",
            "provider": "twelvedata",
            "provider_symbol": "{symbol}",
            "venue": "TEST",
            "currency": "USD",
            "enabled": true,
            "timeframes": ["1D"],
            "notes": "duplicate symbol fixture 1"
          }},
          {{
            "instrument_id": "equity:US:{symbol}B",
            "symbol": "{symbol}",
            "asset_class": "equity",
            "provider": "twelvedata",
            "provider_symbol": "{symbol}B",
            "venue": "TEST",
            "currency": "USD",
            "enabled": true,
            "timeframes": ["1D"],
            "notes": "duplicate symbol fixture 2"
          }}
        ]"#
    )
}

/// A registry with one `"1D"`-only instrument and one `"5m"`-only
/// instrument, both assigned to provider `"twelvedata"` — proves timeframe
/// admission actually discriminates between instruments rather than
/// resolving the whole enabled/provider-scoped set.
fn mixed_timeframe_registry_json() -> String {
    r#"[
      {
        "instrument_id": "equity:US:ZZTFDAILY",
        "symbol": "ZZTFDAILY",
        "asset_class": "equity",
        "provider": "twelvedata",
        "provider_symbol": "ZZTFDAILY",
        "venue": "TEST",
        "currency": "USD",
        "enabled": true,
        "timeframes": ["1D"],
        "notes": "daily-only fixture"
      },
      {
        "instrument_id": "equity:US:ZZTFFIVEMIN",
        "symbol": "ZZTFFIVEMIN",
        "asset_class": "equity",
        "provider": "twelvedata",
        "provider_symbol": "ZZTFFIVEMIN",
        "venue": "TEST",
        "currency": "USD",
        "enabled": true,
        "timeframes": ["5m"],
        "notes": "5m-only fixture"
      }
    ]"#
    .to_string()
}

// REG-ADM-01: a parseable-but-invalid (duplicate symbol) registry fails a
// real historical sync job truthfully before any provider call.
#[tokio::test]
async fn reg_adm_01_duplicate_symbols_fail_historical_sync_before_provider_call() {
    let (mut st_raw, _registry) =
        make_provider_router_with_registry_json(&duplicate_symbol_registry_json("ZZREGADMDUP"));
    st_raw.set_provider_client_for_test(Arc::new(FakeProvider::empty()));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2026-01-01",
            "end": "2026-01-05"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "job must queue: {body}");
    let job_id = json_str(&body, "job_id").to_string();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let (_, body) = get_job_status(router_get, &job_id).await;

    assert_eq!(
        json_str(&body, "status"),
        "failed",
        "a duplicate-symbol registry must fail the sync job: {body}"
    );
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("duplicate") || err.contains("registry"),
        "error must describe the registry validation failure: {err}"
    );
    assert_eq!(
        body["api_calls_made"].as_i64().unwrap_or(999),
        0,
        "api_calls_made must be 0 on invalid registry: {body}"
    );
}

// REG-ADM-02: the same parseable-but-invalid registry fails provider-sync
// dry run truthfully, never reporting a planned admissible symbol count.
#[tokio::test]
async fn reg_adm_02_duplicate_symbols_fail_dry_run_truthfully() {
    let (st_raw, _registry) =
        make_provider_router_with_registry_json(&duplicate_symbol_registry_json("ZZREGADMDUP2"));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "job must queue: {body}");
    let job_id = json_str(&body, "job_id").to_string();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let (_, body) = get_job_status(router_get, &job_id).await;

    assert_eq!(
        json_str(&body, "status"),
        "failed",
        "a duplicate-symbol registry must fail the dry-run job: {body}"
    );
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("duplicate") || err.contains("registry"),
        "error must describe the registry validation failure: {err}"
    );
    assert_eq!(
        body["api_calls_made"].as_i64().unwrap_or(999),
        0,
        "api_calls_made must be 0 on invalid registry dry run: {body}"
    );
    assert_eq!(
        body["symbols_count"],
        serde_json::Value::Null,
        "symbols_count must not be populated when the registry itself is invalid: {body}"
    );
}

// REG-ADM-03: an instrument authorized only for "1D" is admitted by a "1D"
// sync, and the mixed registry resolves only the timeframe-authorized
// instrument (never the whole enabled/provider-scoped set).
#[tokio::test]
async fn reg_adm_03_timeframe_admission_1d_sync_includes_daily_instrument_only() {
    let (st_raw, _registry) =
        make_provider_router_with_registry_json(&mixed_timeframe_registry_json());
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "job must queue: {body}");
    let job_id = json_str(&body, "job_id").to_string();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let (_, body) = get_job_status(router_get, &job_id).await;
    assert_eq!(
        json_str(&body, "status"),
        "dry_run_completed",
        "body: {body}"
    );
    assert_eq!(
        body["symbols_count"].as_u64(),
        Some(1),
        "only the 1D-authorized instrument must be admitted: {body}"
    );
    assert_eq!(body["planned_first_symbol"], "ZZTFDAILY");
    assert_eq!(body["planned_last_symbol"], "ZZTFDAILY");
}

// REG-ADM-04: the same mixed registry, requested at "5m", excludes the
// "1D"-only instrument and admits only the "5m"-authorized instrument.
#[tokio::test]
async fn reg_adm_04_timeframe_admission_5m_sync_excludes_daily_instrument() {
    let (st_raw, _registry) =
        make_provider_router_with_registry_json(&mixed_timeframe_registry_json());
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "5m",
            "symbols_source": "registry",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "job must queue: {body}");
    let job_id = json_str(&body, "job_id").to_string();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let (_, body) = get_job_status(router_get, &job_id).await;
    assert_eq!(
        json_str(&body, "status"),
        "dry_run_completed",
        "body: {body}"
    );
    assert_eq!(
        body["symbols_count"].as_u64(),
        Some(1),
        "only the 5m-authorized instrument must be admitted, never the 1D-only one: {body}"
    );
    assert_eq!(body["planned_first_symbol"], "ZZTFFIVEMIN");
    assert_eq!(body["planned_last_symbol"], "ZZTFFIVEMIN");
}

// REG-ADM-05: dry-run and real-sync planned symbol counts agree for the same
// provider/asset_class/timeframe admission set.
#[tokio::test]
async fn reg_adm_05_dry_run_and_real_sync_planned_sets_agree() {
    let (mut st_raw, _registry) =
        make_provider_router_with_registry_json(&mixed_timeframe_registry_json());
    st_raw.set_provider_client_for_test(Arc::new(FakeProvider::empty()));
    let st = Arc::new(st_raw);

    let router_dry = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_dry,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": true,
            "allow_provider_api_calls": false
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "dry-run job must queue: {body}"
    );
    let dry_job_id = json_str(&body, "job_id").to_string();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let router_get = routes::build_router(Arc::clone(&st));
    let (_, dry_body) = get_job_status(router_get, &dry_job_id).await;
    assert_eq!(json_str(&dry_body, "status"), "dry_run_completed");
    let dry_count = dry_body["symbols_count"].as_u64();

    let router_real = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_real,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2026-01-01",
            "end": "2026-01-05"
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "real sync job must queue: {body}"
    );
    let real_job_id = json_str(&body, "job_id").to_string();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let router_get2 = routes::build_router(Arc::clone(&st));
    let (_, real_body) = get_job_status(router_get2, &real_job_id).await;
    let real_count = real_body["symbols_count"].as_u64();

    assert_eq!(dry_count, Some(1), "dry-run body: {dry_body}");
    assert_eq!(
        dry_count, real_count,
        "dry-run and real-sync planned symbol counts must agree: dry={dry_body}, real={real_body}"
    );
}

// REG-ADM-06: when no registry instrument authorizes the requested
// provider/timeframe combination, the real sync job fails truthfully and
// makes zero fake-provider calls.
#[tokio::test]
async fn reg_adm_06_no_admissible_instrument_makes_zero_provider_calls() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (mut st_raw, _registry) =
        make_provider_router_with_single_instrument("ZZTFNOADMIT", "twelvedata", "1D");
    st_raw.set_provider_client_for_test(Arc::new(SlowCountingProvider {
        calls: Arc::clone(&calls),
        delay: std::time::Duration::from_millis(1),
    }));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    // The fixture instrument only authorizes "1D"; request "5m" instead so
    // no instrument is admitted, even though twelvedata itself supports 5m.
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "5m",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2026-01-01",
            "end": "2026-01-05"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "job must queue: {body}");
    let job_id = json_str(&body, "job_id").to_string();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let router_get = routes::build_router(Arc::clone(&st));
    let (_, body) = get_job_status(router_get, &job_id).await;
    assert_eq!(
        json_str(&body, "status"),
        "failed",
        "a no-admissible-instrument result must fail truthfully: {body}"
    );
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("no enabled registry instruments") || err.contains("timeframe"),
        "error must explain the provider/timeframe admission gap: {err}"
    );
    assert_eq!(
        body["api_calls_made"].as_i64().unwrap_or(999),
        0,
        "api_calls_made must be 0: {body}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the fake provider must never be called: {body}"
    );
}

// REG-ADM-07: no invalid-registry case writes an md_bars row. Skips
// truthfully when no DB is configured.
#[tokio::test]
async fn reg_adm_07_invalid_registry_writes_zero_md_bars_rows() {
    let Some(pool) = ingest_job_db_pool_or_skip("REG-ADM-07").await else {
        return;
    };

    let (mut st_raw, _registry) =
        make_provider_router_with_registry_json(&duplicate_symbol_registry_json("ZZREGADMDUP3"));
    st_raw.db = Some(pool.clone());
    st_raw.set_provider_client_for_test(Arc::new(FakeProvider::empty()));
    let st = Arc::new(st_raw);

    let router_post = routes::build_router(Arc::clone(&st));
    let (status, body) = post_provider_job(
        router_post,
        serde_json::json!({
            "source": "twelvedata",
            "mode": "sync_provider",
            "timeframe": "1D",
            "symbols_source": "registry",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "start": "2026-01-01",
            "end": "2026-01-05"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "job must queue: {body}");
    let job_id: uuid::Uuid = json_str(&body, "job_id")
        .parse()
        .expect("job_id must parse");

    let persisted =
        wait_for_persisted_status(&pool, job_id, &["completed", "partial", "failed"]).await;
    assert_eq!(
        persisted.status.as_str(),
        "failed",
        "invalid registry must fail: {persisted:?}"
    );

    let count: i64 =
        sqlx::query_scalar("select count(*) from md_bars where symbol like 'ZZREGADMDUP3%'")
            .fetch_one(&pool)
            .await
            .expect("count rows for invalid-registry job");
    assert_eq!(count, 0, "an invalid registry must write zero md_bars rows");

    cleanup_ingest_job(&pool, job_id).await;
}
