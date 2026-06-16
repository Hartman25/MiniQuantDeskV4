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
// DATA-INGEST-DAEMON-PROVIDER-JOBS-01: Provider sync job tests
//
// | Test   | What it proves                                                                |
// |--------|-------------------------------------------------------------------------------|
// | PD-01  | POST dry_run=true → 202, accepted=true, dry_run=true, api_calls_made=0      |
// | PD-02  | After wait, job is dry_run_completed, symbols_count=88, api_calls_made=0    |
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

// PD-02: after wait → dry_run_completed, symbols_count=88, api_calls_made=0
#[tokio::test]
async fn pd_02_provider_dryryn_completes_88_symbols_zero_api_calls() {
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

    // symbols_count must equal the canonical registry size.
    let symbols_count = body["symbols_count"]
        .as_u64()
        .expect("symbols_count must be a number in completed job");
    assert_eq!(
        symbols_count, 88,
        "symbols_count must be 88 (canonical registry), got: {symbols_count}"
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

    // Wait for background task to complete.  88 fake symbols × instant = fast.
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

    // symbols_count must be 88 (full registry).
    let symbols_count = body["symbols_count"].as_u64().unwrap_or(0);
    assert_eq!(
        symbols_count, 88,
        "symbols_count must be 88, got: {symbols_count}"
    );

    // dry_run must be false; provider_api_calls_allowed must be true.
    assert_eq!(body["dry_run"], serde_json::Value::Bool(false));
    assert_eq!(
        body["provider_api_calls_allowed"],
        serde_json::Value::Bool(true)
    );
}

// DATA-PROVIDER-REGISTRY-FACTORY-01: non-TwelveData real path can use fake provider seam.
#[tokio::test]
async fn provider_registry_alpaca_real_provider_job_runs_with_fake_provider() {
    let (mut st_raw, _) = make_provider_router_with_registry_raw();
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
    // Cap at 3 API calls — should stop after 3 symbols out of 88.
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

    // symbols_count must be 88 (all symbols were resolved).
    let symbols_count = body["symbols_count"].as_u64().unwrap_or(0);
    assert_eq!(
        symbols_count, 88,
        "symbols_count must be 88 (registry size), got: {symbols_count}"
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
    let (mut st_raw, _) = make_provider_router_with_registry_raw();
    // FakeProvider that fails for all symbols (simulates provider errors).
    st_raw.set_provider_client_for_test(Arc::new(FakeProvider::failing(vec![
        // We use a small cap so the test runs quickly.
        // The guardrail will stop after 2 calls, both of which fail.
        "AAPL".to_string(),
        "MSFT".to_string(),
    ])));
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

    // symbols_count must be 88 (all resolved from registry).
    let symbols_count = body["symbols_count"].as_u64().unwrap_or(0);
    assert_eq!(symbols_count, 88, "symbols_count must be 88: {body}");

    // api_calls_made must be <= 2 (guardrail).
    let api_calls = body["api_calls_made"].as_i64().unwrap_or(999);
    assert!(
        api_calls <= 2,
        "api_calls_made must be <= 2 (per_day guardrail), got: {api_calls}"
    );

    // symbols_failed must be > 0 (at least AAPL or MSFT was attempted and failed).
    let symbols_failed = body["symbols_failed"].as_u64().unwrap_or(0);
    assert!(
        symbols_failed > 0,
        "symbols_failed must be > 0 (fake provider fails for AAPL/MSFT), got: {symbols_failed}"
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
    assert_eq!(persisted.symbols_count, Some(88));

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
    assert_eq!(persisted.symbols_count, Some(88));
    assert_eq!(persisted.symbols_completed, Some(88));
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
