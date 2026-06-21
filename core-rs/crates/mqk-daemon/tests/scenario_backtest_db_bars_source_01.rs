//! BACKTEST-DB-BARS-SOURCE-01: daemon/GUI backtest jobs can run from canonical
//! `md_bars` directly, not only from an operator-supplied CSV file.
//!
//! Proves:
//! - `source` defaults to `"csv"` when omitted — existing CSV requests are
//!   unaffected (byte-for-byte backward compatible at the wire level).
//! - `source="md_bars"` is validated additively: timeframe/start/end required,
//!   start/end must be valid RFC3339, end must be >= start, bars_path is not
//!   required.
//! - Shared validation (strategy/symbol/timeframe_secs/initial_cash_micros)
//!   still applies to the md_bars branch.
//! - An unrecognized `source` value is refused explicitly — never silently
//!   coerced to csv.
//! - With no DB configured, an md_bars job is accepted (validation only
//!   inspects the request) but the background run fails closed with a
//!   `no_db` error — never panics, never fabricates success.
//! - (DB-backed, skipped without `MQK_DATABASE_URL`) a real md_bars-sourced
//!   job completes and the written manifest.json records
//!   source/symbol/start/end/bar_count provenance; a query with zero matching
//!   rows fails closed with a `no_data` error.
//!
//! # Proof matrix
//!
//! | Test  | What it proves                                                          |
//! |-------|--------------------------------------------------------------------------|
//! | DB01  | unknown source -> 400, accepted=false, error names the bad value        |
//! | DB02  | md_bars missing timeframe -> 400                                        |
//! | DB03  | md_bars missing start -> 400                                            |
//! | DB04  | md_bars missing end -> 400                                              |
//! | DB05  | md_bars invalid RFC3339 start -> 400                                    |
//! | DB06  | md_bars end < start -> 400                                              |
//! | DB07  | md_bars still enforces shared timeframe_secs>0 check                    |
//! | DB08  | md_bars request with bars_path key fully absent -> 202 Accepted         |
//! | DB09  | csv request with no `source` key at all -> 202 Accepted (default csv)   |
//! | DB10  | csv request with explicit `source="csv"` -> 202 Accepted                |
//! | DB11  | md_bars job with no DB configured -> job fails with `no_db` error       |
//! | DB12  | `source="CSV"` (mixed case) normalizes and still works                  |
//! | DBDB01| (DB-backed) md_bars job completes; manifest records provenance          |
//! | DBDB02| (DB-backed) md_bars job with zero matching rows fails with `no_data`    |
//!
//! DB01-DB12 are fully in-process (no real Postgres, no network).
//! DBDB01/DBDB02 require `MQK_DATABASE_URL` and are `#[ignore]`d by default.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
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

/// Absolute path to the bkt4_bars.csv fixture (shared with scenario_backtest_jobs_01.rs).
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

async fn poll_job_status(
    st: &Arc<state::AppState>,
    job_id: &str,
    timeout: std::time::Duration,
) -> serde_json::Value {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let router = routes::build_router(Arc::clone(st));
        let req = Request::builder()
            .uri(format!("/api/v1/backtests/jobs/{}", job_id))
            .body(axum::body::Body::empty())
            .unwrap();
        let (status, resp) = call(router, req).await;
        assert_eq!(status, StatusCode::OK);
        let json = parse_json(resp);
        let job_status = json_str(&json, "status").to_string();
        if job_status == "completed" || job_status == "failed" {
            return json;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "job did not reach a terminal status within {:?} (last status: {})",
            timeout,
            job_status
        );
    }
}

// ---------------------------------------------------------------------------
// DB01: unknown source -> 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn db01_unknown_source_refused() {
    let router = make_router();
    let body = post_json_body(serde_json::json!({
        "source": "xml",
        "bars_path": fixture_bars_path(),
        "strategy": "swing_momentum",
        "symbol": "TEST",
        "timeframe_secs": 60,
        "initial_cash_micros": 100_000_000_000i64
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown source must -> 400");
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"));
    let err = json_str(&json, "error");
    assert!(
        err.contains("unknown source"),
        "error must name the bad source, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// DB02-DB04: md_bars missing timeframe/start/end -> 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn db02_md_bars_missing_timeframe_refused() {
    let router = make_router();
    let body = post_json_body(serde_json::json!({
        "source": "md_bars",
        "strategy": "swing_momentum",
        "symbol": "AAPL",
        "timeframe_secs": 86400,
        "initial_cash_micros": 100_000_000_000i64,
        "start": "2026-06-01T00:00:00Z",
        "end": "2026-06-20T00:00:00Z"
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"));
    assert!(json_str(&json, "error").contains("timeframe"));
}

#[tokio::test]
async fn db03_md_bars_missing_start_refused() {
    let router = make_router();
    let body = post_json_body(serde_json::json!({
        "source": "md_bars",
        "strategy": "swing_momentum",
        "symbol": "AAPL",
        "timeframe_secs": 86400,
        "initial_cash_micros": 100_000_000_000i64,
        "timeframe": "1D",
        "end": "2026-06-20T00:00:00Z"
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"));
    assert!(json_str(&json, "error").contains("start"));
}

#[tokio::test]
async fn db04_md_bars_missing_end_refused() {
    let router = make_router();
    let body = post_json_body(serde_json::json!({
        "source": "md_bars",
        "strategy": "swing_momentum",
        "symbol": "AAPL",
        "timeframe_secs": 86400,
        "initial_cash_micros": 100_000_000_000i64,
        "timeframe": "1D",
        "start": "2026-06-01T00:00:00Z"
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"));
    assert!(json_str(&json, "error").contains("end"));
}

// ---------------------------------------------------------------------------
// DB05: md_bars invalid RFC3339 start -> 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn db05_md_bars_invalid_start_format_refused() {
    let router = make_router();
    let body = post_json_body(serde_json::json!({
        "source": "md_bars",
        "strategy": "swing_momentum",
        "symbol": "AAPL",
        "timeframe_secs": 86400,
        "initial_cash_micros": 100_000_000_000i64,
        "timeframe": "1D",
        "start": "not-a-date",
        "end": "2026-06-20T00:00:00Z"
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"));
    let err = json_str(&json, "error");
    assert!(
        err.contains("RFC3339") && err.contains("start"),
        "error must name start as the invalid RFC3339 field, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// DB06: md_bars end < start -> 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn db06_md_bars_end_before_start_refused() {
    let router = make_router();
    let body = post_json_body(serde_json::json!({
        "source": "md_bars",
        "strategy": "swing_momentum",
        "symbol": "AAPL",
        "timeframe_secs": 86400,
        "initial_cash_micros": 100_000_000_000i64,
        "timeframe": "1D",
        "start": "2026-06-20T00:00:00Z",
        "end": "2026-06-01T00:00:00Z"
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"));
    assert!(json_str(&json, "error").contains("end must be >= start"));
}

// ---------------------------------------------------------------------------
// DB07: md_bars still enforces shared timeframe_secs>0 check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn db07_md_bars_shared_validation_still_applies() {
    let router = make_router();
    let body = post_json_body(serde_json::json!({
        "source": "md_bars",
        "strategy": "swing_momentum",
        "symbol": "AAPL",
        "timeframe_secs": 0,
        "initial_cash_micros": 100_000_000_000i64,
        "timeframe": "1D",
        "start": "2026-06-01T00:00:00Z",
        "end": "2026-06-20T00:00:00Z"
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "timeframe_secs=0 must still be refused on the md_bars branch"
    );
    let json = parse_json(resp);
    assert!(!json_bool(&json, "accepted"));
}

// ---------------------------------------------------------------------------
// DB08: md_bars request with bars_path key fully absent -> 202 Accepted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn db08_md_bars_does_not_require_bars_path() {
    let router = make_router();
    // Note: no "bars_path" key at all (not even empty string) — proves the
    // #[serde(default)] on bars_path lets md_bars requests omit it entirely.
    let body = post_json_body(serde_json::json!({
        "source": "md_bars",
        "strategy": "swing_momentum",
        "symbol": "AAPL",
        "timeframe_secs": 86400,
        "initial_cash_micros": 100_000_000_000i64,
        "timeframe": "1D",
        "start": "2026-06-01T00:00:00Z",
        "end": "2026-06-20T00:00:00Z"
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "valid md_bars request without bars_path must -> 202 Accepted"
    );
    let json = parse_json(resp);
    assert!(json_bool(&json, "accepted"));
}

// ---------------------------------------------------------------------------
// DB09-DB10: CSV backward compatibility
// ---------------------------------------------------------------------------

#[tokio::test]
async fn db09_csv_request_without_source_field_still_works() {
    let router = make_router();
    // Identical shape to the pre-existing BJ-06 payload — no "source" key.
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
        "omitted source must default to csv and behave exactly as before"
    );
    let json = parse_json(resp);
    assert!(json_bool(&json, "accepted"));
    assert_eq!(json_str(&json, "status"), "queued");
}

#[tokio::test]
async fn db10_csv_request_explicit_source_still_works() {
    let router = make_router();
    let body = post_json_body(serde_json::json!({
        "source": "csv",
        "bars_path": fixture_bars_path(),
        "strategy": "swing_momentum",
        "symbol": "TEST",
        "timeframe_secs": 60,
        "initial_cash_micros": 100_000_000_000i64
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "explicit source=csv must work identically");
    let json = parse_json(resp);
    assert!(json_bool(&json, "accepted"));
}

#[tokio::test]
async fn db12_source_mixed_case_normalizes() {
    let router = make_router();
    let body = post_json_body(serde_json::json!({
        "source": "CSV",
        "bars_path": fixture_bars_path(),
        "strategy": "swing_momentum",
        "symbol": "TEST",
        "timeframe_secs": 60,
        "initial_cash_micros": 100_000_000_000i64
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "mixed-case 'CSV' must normalize to csv");
    let json = parse_json(resp);
    assert!(json_bool(&json, "accepted"));
}

// ---------------------------------------------------------------------------
// DB11: md_bars job with no DB configured -> job fails with no_db error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn db11_md_bars_no_db_configured_fails_closed() {
    let (st, router) = make_router_with_state();

    let body = post_json_body(serde_json::json!({
        "source": "md_bars",
        "strategy": "swing_momentum",
        "symbol": "AAPL",
        "timeframe_secs": 86400,
        "initial_cash_micros": 100_000_000_000i64,
        "timeframe": "1D",
        "start": "2026-06-01T00:00:00Z",
        "end": "2026-06-20T00:00:00Z"
    }));
    let (status, resp) = call(router, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "validation alone must accept the request");
    let job_id = parse_json(resp)
        .get("job_id")
        .and_then(|v| v.as_str())
        .expect("job_id must be present")
        .to_string();

    let json = poll_job_status(&st, &job_id, std::time::Duration::from_secs(10)).await;
    assert_eq!(
        json_str(&json, "status"),
        "failed",
        "md_bars job must fail closed when no DB pool is configured, never panic or fabricate success"
    );
    let err = json_str(&json, "error");
    assert!(
        err.contains("no_db"),
        "error must explicitly name no_db, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// DBDB01/DBDB02 — DB-backed (skip without MQK_DATABASE_URL)
// ---------------------------------------------------------------------------

async fn maybe_pool() -> Option<sqlx::PgPool> {
    match mqk_db::connect_from_env().await {
        Ok(pool) => Some(pool),
        Err(e) => {
            eprintln!("SKIP: requires working MQK_DATABASE_URL ({e})");
            None
        }
    }
}

async fn insert_test_bar(
    pool: &sqlx::PgPool,
    symbol: &str,
    timeframe: &str,
    end_ts: i64,
    price_micros: i64,
) {
    sqlx::query(
        r#"
        insert into md_bars (
          symbol, timeframe, end_ts, open_micros, high_micros, low_micros, close_micros, volume, is_complete
        ) values
          ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        on conflict (symbol, timeframe, end_ts) do update set
          open_micros = excluded.open_micros,
          high_micros = excluded.high_micros,
          low_micros = excluded.low_micros,
          close_micros = excluded.close_micros,
          volume = excluded.volume,
          is_complete = excluded.is_complete
        "#,
    )
    .bind(symbol)
    .bind(timeframe)
    .bind(end_ts)
    .bind(price_micros)
    .bind(price_micros + 10_000)
    .bind(price_micros - 10_000)
    .bind(price_micros + 5_000)
    .bind(1_000_i64)
    .bind(true)
    .execute(pool)
    .await
    .expect("insert test md_bars row");
}

async fn delete_test_bars(pool: &sqlx::PgPool, symbol: &str) {
    let _ = sqlx::query("delete from md_bars where symbol = $1")
        .bind(symbol)
        .execute(pool)
        .await;
}

#[tokio::test]
#[ignore]
async fn dbdb01_md_bars_job_completes_and_manifest_records_provenance() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    mqk_db::migrate(&pool).await.expect("migrate failed");

    let symbol = "ZZBTDBSRC01TST";
    delete_test_bars(&pool, symbol).await;

    let base_ts: i64 = 1_790_000_000;
    let bar_tss = [base_ts, base_ts + 86_400, base_ts + 172_800];
    for (i, ts) in bar_tss.iter().enumerate() {
        insert_test_bar(&pool, symbol, "1D", *ts, 100_000_000 + (i as i64) * 1_000_000).await;
    }

    let start_ts = base_ts - 86_400;
    let end_ts = base_ts + 172_800 + 86_400;
    let start_rfc3339 = DateTime::<Utc>::from_timestamp(start_ts, 0)
        .unwrap()
        .to_rfc3339();
    let end_rfc3339 = DateTime::<Utc>::from_timestamp(end_ts, 0)
        .unwrap()
        .to_rfc3339();

    let tmp = tempfile::tempdir().expect("temp dir must be creatable");
    let out_dir = tmp.path().display().to_string();

    let (st, _) = (
        Arc::new(state::AppState::new_with_db_and_operator_auth(
            pool.clone(),
            state::OperatorAuthMode::ExplicitDevNoToken,
        )),
        (),
    );

    let router_post = routes::build_router(Arc::clone(&st));
    let body = post_json_body(serde_json::json!({
        "source": "md_bars",
        "strategy": "swing_momentum",
        "symbol": symbol,
        "timeframe_secs": 86400,
        "initial_cash_micros": 100_000_000_000i64,
        "out_dir": out_dir,
        "timeframe": "1D",
        "start": start_rfc3339,
        "end": end_rfc3339
    }));
    let (status, resp) = call(router_post, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "DBDB01: valid md_bars job must be accepted");
    let job_id = parse_json(resp)
        .get("job_id")
        .and_then(|v| v.as_str())
        .expect("job_id must be present")
        .to_string();

    let json = poll_job_status(&st, &job_id, std::time::Duration::from_secs(20)).await;
    if json_str(&json, "status") == "failed" {
        let err = json.get("error").and_then(|v| v.as_str()).unwrap_or("(no error)");
        panic!("DBDB01: job failed unexpectedly: {err}");
    }
    assert_eq!(json_str(&json, "status"), "completed");

    let artifact_dir = json
        .get("artifact_dir")
        .and_then(|v| v.as_str())
        .expect("completed job must have artifact_dir");
    let manifest_path = std::path::Path::new(artifact_dir).join("manifest.json");
    let manifest_raw = std::fs::read_to_string(&manifest_path).expect("manifest.json must be readable");
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest_raw).expect("manifest.json must be valid JSON");

    assert_eq!(manifest["source"], "md_bars", "manifest must record source=md_bars");
    assert_eq!(manifest["symbol"], symbol, "manifest must record the queried symbol");
    assert_eq!(manifest["start"], start_rfc3339, "manifest must record the requested start");
    assert_eq!(manifest["end"], end_rfc3339, "manifest must record the requested end");
    assert_eq!(manifest["bar_count"], 3, "manifest must record the loaded bar count");
    assert_eq!(manifest["timeframe"], "1D", "manifest must record the md_bars timeframe string");
    assert_eq!(manifest["timeframe_secs"], 86400, "manifest must record timeframe_secs");

    delete_test_bars(&pool, symbol).await;
}

#[tokio::test]
#[ignore]
async fn dbdb02_md_bars_job_with_zero_matching_rows_fails_with_no_data() {
    let Some(pool) = maybe_pool().await else {
        return;
    };
    mqk_db::migrate(&pool).await.expect("migrate failed");

    // Unique symbol with deliberately no inserted rows.
    let symbol = "ZZBTDBSRC01NODATA";
    delete_test_bars(&pool, symbol).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let router_post = routes::build_router(Arc::clone(&st));
    let body = post_json_body(serde_json::json!({
        "source": "md_bars",
        "strategy": "swing_momentum",
        "symbol": symbol,
        "timeframe_secs": 86400,
        "initial_cash_micros": 100_000_000_000i64,
        "timeframe": "1D",
        "start": "2026-06-01T00:00:00Z",
        "end": "2026-06-20T00:00:00Z"
    }));
    let (status, resp) = call(router_post, body).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let job_id = parse_json(resp)
        .get("job_id")
        .and_then(|v| v.as_str())
        .expect("job_id must be present")
        .to_string();

    let json = poll_job_status(&st, &job_id, std::time::Duration::from_secs(10)).await;
    assert_eq!(
        json_str(&json, "status"),
        "failed",
        "DBDB02: zero matching md_bars rows must fail the job, never fabricate an equity curve"
    );
    let err = json_str(&json, "error");
    assert!(err.contains("no_data"), "error must explicitly name no_data, got: {err}");
}
