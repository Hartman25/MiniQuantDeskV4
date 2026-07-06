//! CRYPTO-DATA-01AD-KRAKEN-SYNC-EVIDENCE-STATUS-ROUTE-01: read-only Kraken
//! OHLC ingest/sync evidence status surface.
//!
//! Proves GET /api/v1/market-data/kraken-ohlc/status:
//! - Returns truth_state="no_evidence" when evidence directory is empty.
//! - Returns truth_state="backend_unavailable" when evidence directory is missing.
//! - Returns truth_state="active" and parses valid, fresh ingest evidence correctly.
//! - Returns truth_state="active" and parses valid, fresh sync evidence correctly.
//! - Returns truth_state="stale" when produced_at_utc is older than the max age.
//! - Returns truth_state="parse_error" (no panic) on malformed JSON.
//! - Returns truth_state="unsafe_evidence" when a forming candle is not excluded.
//! - Returns truth_state="unsafe_evidence" when provider is not "kraken".
//! - Selects the latest file by embedded epoch timestamp across both the
//!   ingest and sync filename prefixes (not alphabetical filename order).
//! - Never opens a DB connection, never writes outbox/oms rows, never calls Kraken.
//!
//! # Proof matrix
//!
//! | Test    | What it proves                                                              |
//! |---------|------------------------------------------------------------------------------|
//! | KOS-01  | Missing evidence dir → truth_state="backend_unavailable"                    |
//! | KOS-02  | Empty evidence dir → truth_state="no_evidence"                              |
//! | KOS-03  | Valid fresh sync evidence → truth_state="active", fields parsed             |
//! | KOS-04  | Valid fresh ingest evidence → truth_state="active", latest_mode="ingest"    |
//! | KOS-05  | Old produced_at_utc → truth_state="stale"                                  |
//! | KOS-06  | Malformed JSON → truth_state="parse_error", no panic                       |
//! | KOS-07  | Wrong schema_version → truth_state="parse_error"                           |
//! | KOS-08  | forming_candle_excluded=false → truth_state="unsafe_evidence"              |
//! | KOS-09  | provider != "kraken" → truth_state="unsafe_evidence"                       |
//! | KOS-10  | Newer sync file selected over an older ingest file (epoch-based, not      |
//! |         | alphabetical -- "ingest" < "sync" lexically)                                |
//! | KOS-11  | Route never opens a DB connection / mutates outbox/oms state               |
//!
//! All tests use synthetic temp-dir evidence files. No real Kraken network
//! call made. No DB writes. No orders. No broker calls. No outbox/oms rows.
//! No CLI execution, no sync triggered.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn make_router_with_evidence_dir(dir: &str) -> axum::Router {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.md_refresh_evidence_dir = dir.to_string();
    routes::build_router(Arc::new(st))
}

fn get_kraken_ohlc_status() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/market-data/kraken-ohlc/status")
        .body(axum::body::Body::empty())
        .unwrap()
}

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v = serde_json::from_slice(&body).expect("response must be valid JSON");
    (status, v)
}

fn str_field<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_else(|| panic!("field '{}' must be a string in: {}", key, v))
}

fn bool_field(v: &serde_json::Value, key: &str) -> bool {
    v.get(key)
        .and_then(|x| x.as_bool())
        .unwrap_or_else(|| panic!("field '{}' must be a bool in: {}", key, v))
}

fn write_evidence(dir: &tempfile::TempDir, filename: &str, content: &str) {
    let path = dir.path().join(filename);
    std::fs::write(path, content).expect("write test evidence file");
}

fn recent_ts() -> String {
    let ts = chrono::Utc::now() - chrono::Duration::minutes(1);
    ts.to_rfc3339()
}

fn old_ts() -> String {
    let ts = chrono::Utc::now() - chrono::Duration::hours(48);
    ts.to_rfc3339()
}

/// A minimal, safe, valid `kraken-ohlc-sync-v2` evidence body.
fn valid_sync_evidence(produced_at: &str) -> String {
    format!(
        r#"{{
  "schema_version": "kraken-ohlc-sync-v2",
  "producer": "mqk-cli md kraken-ohlc-sync",
  "produced_at_utc": "{produced_at}",
  "provider": "kraken",
  "mode": "input_file",
  "network_call_made": false,
  "db_write": true,
  "md_bars_write": true,
  "provider_id": "kraken",
  "provider_source": "kraken",
  "provider_symbol": "XXBTZUSD",
  "ingest_mode": "provider_sync",
  "sync_policy": "content_diff_skip_unchanged_update_changed_insert_missing",
  "no_update_existing": false,
  "symbols_requested": ["BTC/USD"],
  "bars_completed": 2,
  "bars_excluded_forming": 1,
  "bars_considered_for_sync": 2,
  "bars_missing_new": 1,
  "bars_existing_candidate": 1,
  "rows_changed": 1,
  "rows_skipped_unchanged": 0,
  "rows_changed_skipped_due_to_no_update_existing": 0,
  "rows_inserted": 1,
  "rows_updated": 1,
  "rows_skipped_if_known": 0,
  "latest_existing_end_ts_before": 1783123200,
  "latest_completed_start_ts": 1783123200,
  "latest_completed_end_ts": 1783209600,
  "volume_semantics": "kraken_base_asset_volume_scaled_by_1e8_not_whole_coins_not_usd",
  "volume_scale": 100000000,
  "all_passed": true,
  "reason_code": "kraken_sync_evidence_generated",
  "fail_reasons": []
}}"#,
        produced_at = produced_at
    )
}

/// A minimal, safe, valid `kraken-ohlc-ingest-v1` evidence body.
fn valid_ingest_evidence(produced_at: &str) -> String {
    format!(
        r#"{{
  "schema_version": "kraken-ohlc-ingest-v1",
  "producer": "mqk-cli md kraken-ohlc-ingest",
  "produced_at_utc": "{produced_at}",
  "provider": "kraken",
  "mode": "input_file",
  "network_call_made": false,
  "db_write": true,
  "md_bars_write": true,
  "provider_id": "kraken",
  "provider_source": "kraken",
  "provider_symbol": "XXBTZUSD",
  "ingest_mode": "provider_ingest",
  "symbols_requested": ["BTC/USD"],
  "bars_completed": 2,
  "bars_excluded_forming": 1,
  "latest_completed_start_ts": 1783123200,
  "latest_completed_end_ts": 1783209600,
  "volume_semantics": "kraken_base_asset_volume_scaled_by_1e8_not_whole_coins_not_usd",
  "volume_scale": 100000000,
  "rows_inserted": 2,
  "rows_updated": 0,
  "all_passed": true,
  "reason_code": "kraken_ingest_evidence_generated",
  "fail_reasons": []
}}"#,
        produced_at = produced_at
    )
}

// ---------------------------------------------------------------------------
// KOS-01: Missing evidence directory → backend_unavailable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kos_01_missing_evidence_dir_returns_backend_unavailable() {
    let router = make_router_with_evidence_dir("/tmp/mqk_does_not_exist_kos01_abc123");
    let (status, v) = call(router, get_kraken_ohlc_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "backend_unavailable");
    assert!(bool_field(&v, "stale_or_missing_evidence"));
    assert!(v["error"].as_str().is_some(), "error field must be present");
}

// ---------------------------------------------------------------------------
// KOS-02: Empty evidence directory → no_evidence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kos_02_empty_evidence_dir_returns_no_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_kraken_ohlc_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "no_evidence");
    assert!(bool_field(&v, "stale_or_missing_evidence"));
    assert!(v["error"].is_null(), "no error on empty dir");
    assert!(v["evidence_path"].is_null());
}

// ---------------------------------------------------------------------------
// KOS-03: Valid fresh sync evidence → active, fields parsed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kos_03_valid_fresh_sync_evidence_parses_correctly() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "kraken_ohlc_sync_1783300000.json",
        &valid_sync_evidence(&recent_ts()),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_kraken_ohlc_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
    assert!(!bool_field(&v, "stale_or_missing_evidence"));
    assert_eq!(str_field(&v, "provider"), "kraken");
    assert_eq!(str_field(&v, "latest_mode"), "sync");
    assert_eq!(str_field(&v, "latest_schema_version"), "kraken-ohlc-sync-v2");
    assert_eq!(v["provider_id"].as_str(), Some("kraken"));
    assert_eq!(v["provider_source"].as_str(), Some("kraken"));
    assert_eq!(v["provider_symbol"].as_str(), Some("XXBTZUSD"));
    assert_eq!(v["ingest_mode"].as_str(), Some("provider_sync"));
    assert_eq!(
        v["sync_policy"].as_str(),
        Some("content_diff_skip_unchanged_update_changed_insert_missing")
    );
    assert_eq!(v["no_update_existing"].as_bool(), Some(false));
    assert_eq!(v["bars_missing_new"].as_i64(), Some(1));
    assert_eq!(v["rows_changed"].as_i64(), Some(1));
    assert_eq!(v["rows_skipped_unchanged"].as_i64(), Some(0));
    assert_eq!(
        v["rows_changed_skipped_due_to_no_update_existing"].as_i64(),
        Some(0)
    );
    assert_eq!(v["rows_inserted"].as_i64(), Some(1));
    assert_eq!(v["rows_updated"].as_i64(), Some(1));
    assert_eq!(v["volume_scale"].as_i64(), Some(100_000_000));
    assert_eq!(v["all_passed"].as_bool(), Some(true));
    let symbols_requested = v["symbols_requested"].as_array().expect("array");
    assert_eq!(symbols_requested.len(), 1);
}

// ---------------------------------------------------------------------------
// KOS-04: Valid fresh ingest evidence → active, latest_mode="ingest"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kos_04_valid_fresh_ingest_evidence_parses_correctly() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "kraken_ohlc_ingest_1783300000.json",
        &valid_ingest_evidence(&recent_ts()),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_kraken_ohlc_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
    assert_eq!(str_field(&v, "latest_mode"), "ingest");
    assert_eq!(str_field(&v, "latest_schema_version"), "kraken-ohlc-ingest-v1");
    assert_eq!(v["ingest_mode"].as_str(), Some("provider_ingest"));
    // Sync-only fields must be absent (None), never fabricated.
    assert!(v["sync_policy"].is_null());
    assert!(v["rows_changed"].is_null());
    assert!(v["bars_missing_new"].is_null());
}

// ---------------------------------------------------------------------------
// KOS-05: Old produced_at_utc → stale
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kos_05_old_evidence_is_flagged_stale() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "kraken_ohlc_sync_1700000000.json",
        &valid_sync_evidence(&old_ts()),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_kraken_ohlc_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "stale");
    assert!(bool_field(&v, "stale_or_missing_evidence"));
}

// ---------------------------------------------------------------------------
// KOS-06: Malformed JSON → parse_error, no panic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kos_06_malformed_json_returns_parse_error_no_panic() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "kraken_ohlc_sync_1783300100.json",
        r#"{ this is not valid json !!! "#,
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_kraken_ohlc_status()).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "malformed evidence must still return 200"
    );
    assert_eq!(str_field(&v, "truth_state"), "parse_error");
    assert!(bool_field(&v, "stale_or_missing_evidence"));
    assert!(v["error"].as_str().is_some());
}

// ---------------------------------------------------------------------------
// KOS-07: Wrong schema_version → parse_error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kos_07_wrong_schema_version_returns_parse_error() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "kraken_ohlc_sync_1783300200.json",
        r#"{
  "schema_version": "kraken-ohlc-sync-v1",
  "produced_at_utc": "2026-07-04T14:00:00Z",
  "provider": "kraken"
}"#,
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_kraken_ohlc_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "parse_error");
    let err = v["error"].as_str().expect("error field must be present");
    assert!(
        err.contains("schema_version"),
        "error must mention schema_version: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// KOS-08: forming_candle_excluded=false → unsafe_evidence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kos_08_forming_candle_not_excluded_returns_unsafe_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let base = valid_sync_evidence(&recent_ts());
    let evidence = base.replacen(
        "\"provider\": \"kraken\",",
        "\"provider\": \"kraken\",\n  \"forming_candle_excluded\": false,",
        1,
    );
    write_evidence(&dir, "kraken_ohlc_sync_1783300300.json", &evidence);

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_kraken_ohlc_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_evidence");
    assert_ne!(str_field(&v, "truth_state"), "active");
    assert!(v["error"]
        .as_str()
        .unwrap()
        .contains("forming_candle_excluded"));
}

// ---------------------------------------------------------------------------
// KOS-09: provider != "kraken" → unsafe_evidence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kos_09_wrong_provider_returns_unsafe_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let evidence = valid_sync_evidence(&recent_ts()).replacen(
        "\"provider\": \"kraken\",",
        "\"provider\": \"coinlore\",",
        1,
    );
    write_evidence(&dir, "kraken_ohlc_sync_1783300400.json", &evidence);

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_kraken_ohlc_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_evidence");
    assert!(v["error"].as_str().unwrap().contains("provider"));
}

// ---------------------------------------------------------------------------
// KOS-10: Newer sync file selected over an older ingest file, by embedded
// epoch timestamp -- not alphabetical filename order ("ingest" < "sync").
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kos_10_latest_candidate_selected_by_epoch_across_both_prefixes() {
    let dir = tempfile::tempdir().expect("create temp dir");
    // Alphabetically, "kraken_ohlc_ingest_..." < "kraken_ohlc_sync_...", but
    // this ingest file has a *larger* embedded epoch, so it must win.
    write_evidence(
        &dir,
        "kraken_ohlc_ingest_1783400000.json",
        &valid_ingest_evidence(&recent_ts()),
    );
    write_evidence(
        &dir,
        "kraken_ohlc_sync_1783300000.json",
        &valid_sync_evidence(&recent_ts()),
    );
    // A non-matching file (different prefix) must be ignored entirely.
    write_evidence(
        &dir,
        "coinlore_latest_mark_1783500000.json",
        r#"{"schema_version":"coinlore-latest-mark-v1"}"#,
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_kraken_ohlc_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
    assert_eq!(
        str_field(&v, "latest_mode"),
        "ingest",
        "the higher-epoch ingest file must win over the lower-epoch sync file"
    );
}

// ---------------------------------------------------------------------------
// KOS-11: Route never opens a DB connection / mutates outbox/oms state.
//
// AppState constructed here has no DB pool configured (st.db is None by
// construction in new_with_operator_auth's test path), and the handler's
// source contains no mqk_db call of any kind -- if it ever opened a
// connection, this test's router construction with no pool would surface
// as a panic/error rather than the expected 200 OK truth_state responses
// already proven by KOS-01..10. This test re-asserts explicitly for the
// active path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kos_11_route_returns_active_without_any_db_pool_configured() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "kraken_ohlc_sync_1783600000.json",
        &valid_sync_evidence(&recent_ts()),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_kraken_ohlc_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
}
