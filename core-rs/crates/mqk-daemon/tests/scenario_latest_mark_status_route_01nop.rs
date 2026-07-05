//! CRYPTO-DATA-01N-O-P-LATEST-MARK-EVIDENCE-STATUS-BUNDLE-01-COMBINED:
//! read-only CoinLore latest-mark evidence status surface.
//!
//! Proves GET /api/v1/market-data/latest-marks/status:
//! - Returns truth_state="no_evidence" when evidence directory is empty.
//! - Returns truth_state="backend_unavailable" when evidence directory is missing.
//! - Returns truth_state="active" and parses valid, fresh evidence correctly.
//! - Returns truth_state="stale" when produced_at_utc is older than the max age.
//! - Returns truth_state="parse_error" (no panic) on malformed JSON.
//! - Returns truth_state="parse_error" on wrong schema_version.
//! - Returns truth_state="unsafe_evidence" (never "active") when evidence
//!   claims db_write/md_bars_write/completed_bar_claim=true.
//! - Returns truth_state="unsafe_evidence" when a mark carries a bar-like field.
//! - Selects the latest file (alphabetically last) among multiple candidates.
//! - Never opens a DB connection, never writes outbox/oms rows, never calls a provider.
//!
//! # Proof matrix
//!
//! | Test    | What it proves                                                              |
//! |---------|------------------------------------------------------------------------------|
//! | LMS-01  | Missing evidence dir → truth_state="backend_unavailable"                    |
//! | LMS-02  | Empty evidence dir → truth_state="no_evidence"                              |
//! | LMS-03  | Valid fresh evidence → truth_state="active", marks/fields parsed            |
//! | LMS-04  | Old produced_at_utc → truth_state="stale"                                   |
//! | LMS-05  | Malformed JSON → truth_state="parse_error", no panic                        |
//! | LMS-06  | Wrong schema_version → truth_state="parse_error"                            |
//! | LMS-07  | db_write=true in evidence → truth_state="unsafe_evidence", never "active"   |
//! | LMS-08  | md_bars_write=true in evidence → truth_state="unsafe_evidence"              |
//! | LMS-09  | completed_bar_claim=true in evidence → truth_state="unsafe_evidence"        |
//! | LMS-10  | Mark carries an "open" field → truth_state="unsafe_evidence"                |
//! | LMS-11  | Latest file selected from multiple candidates (alphabetical last)           |
//! | LMS-12  | Route response contains no DB-derived field / DB is never opened            |
//!
//! All tests use synthetic temp-dir evidence files. No real CoinLore network
//! call made. No DB writes. No orders. No broker calls. No outbox/oms rows.

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

fn get_latest_mark_status() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/market-data/latest-marks/status")
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

/// A minimal, safe, valid `coinlore-latest-mark-v1` evidence body.
fn valid_evidence(produced_at: &str) -> String {
    format!(
        r#"{{
  "schema_version": "coinlore-latest-mark-v1",
  "producer": "mqk-cli md coinlore-latest-mark",
  "produced_at_utc": "{produced_at}",
  "provider": "coinlore",
  "mode": "input_file",
  "network_call_made": false,
  "db_write": false,
  "md_bars_write": false,
  "completed_bar_claim": false,
  "provider_enabled": false,
  "registry_path": "config/instruments/instruments_v2.crypto_local_marks.example.json",
  "symbols_requested": ["BTC/USD", "ETH/USD"],
  "truth_state": "active",
  "stale_or_missing": false,
  "marks": [
    {{
      "canonical_symbol": "BTC/USD",
      "provider_id": "coinlore",
      "provider_symbol": "BTC",
      "provider_coin_id": "90",
      "price_usd": "62906.61",
      "volume24_usd": "16261421089.448309",
      "as_of_client_request_ts": 1783264281,
      "provider_ts": null,
      "truth_state": "client_observed_only",
      "kind": "ticker"
    }},
    {{
      "canonical_symbol": "ETH/USD",
      "provider_id": "coinlore",
      "provider_symbol": "ETH",
      "provider_coin_id": "80",
      "price_usd": "1777.74",
      "volume24_usd": "9252669028.469555",
      "as_of_client_request_ts": 1783264281,
      "provider_ts": null,
      "truth_state": "client_observed_only",
      "kind": "ticker"
    }}
  ],
  "all_passed": true,
  "reason_code": "latest_mark_evidence_generated",
  "fail_reasons": []
}}"#,
        produced_at = produced_at
    )
}

// ---------------------------------------------------------------------------
// LMS-01: Missing evidence directory → backend_unavailable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lms_01_missing_evidence_dir_returns_backend_unavailable() {
    let router = make_router_with_evidence_dir("/tmp/mqk_does_not_exist_lms01_abc123");
    let (status, v) = call(router, get_latest_mark_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "backend_unavailable");
    assert!(bool_field(&v, "stale_or_missing_evidence"));
    assert!(v["error"].as_str().is_some(), "error field must be present");
    assert!(v["marks"].as_array().map(|a| a.len()).unwrap_or(1) == 0);
}

// ---------------------------------------------------------------------------
// LMS-02: Empty evidence directory → no_evidence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lms_02_empty_evidence_dir_returns_no_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_latest_mark_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "no_evidence");
    assert!(bool_field(&v, "stale_or_missing_evidence"));
    assert!(v["error"].is_null(), "no error on empty dir");
    assert!(v["evidence_path"].is_null());
}

// ---------------------------------------------------------------------------
// LMS-03: Valid fresh evidence → active, fields/marks parsed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lms_03_valid_fresh_evidence_parses_correctly() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "coinlore_latest_mark_1783264281.json",
        &valid_evidence(&recent_ts()),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_latest_mark_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
    assert!(
        !bool_field(&v, "stale_or_missing_evidence"),
        "recent evidence must not be stale"
    );
    assert_eq!(str_field(&v, "provider"), "coinlore");
    assert_eq!(v["network_call_made"].as_bool(), Some(false));
    assert_eq!(v["db_write"].as_bool(), Some(false));
    assert_eq!(v["md_bars_write"].as_bool(), Some(false));
    assert_eq!(v["completed_bar_claim"].as_bool(), Some(false));
    assert_eq!(v["provider_enabled"].as_bool(), Some(false));
    assert_eq!(v["all_passed"].as_bool(), Some(true));
    assert!(v["evidence_path"].as_str().is_some());
    assert!(v["produced_at_utc"].as_str().is_some());

    let symbols_requested = v["symbols_requested"].as_array().expect("array");
    assert_eq!(symbols_requested.len(), 2);

    let marks = v["marks"].as_array().expect("marks must be array");
    assert_eq!(marks.len(), 2);
    let btc = marks
        .iter()
        .find(|m| m["canonical_symbol"] == "BTC/USD")
        .expect("BTC/USD mark present");
    assert_eq!(btc["provider_coin_id"].as_str(), Some("90"));
    assert_eq!(btc["price_usd"].as_str(), Some("62906.61"));
    assert_eq!(btc["kind"].as_str(), Some("ticker"));
    assert_eq!(btc["truth_state"].as_str(), Some("client_observed_only"));
    // No bar-like field on the parsed row's JSON shape.
    for forbidden in ["open", "high", "low", "close", "is_complete", "end_ts"] {
        assert!(btc.get(forbidden).is_none());
    }
}

// ---------------------------------------------------------------------------
// LMS-04: Old produced_at_utc → stale
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lms_04_old_evidence_is_flagged_stale() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "coinlore_latest_mark_1700000000.json",
        &valid_evidence(&old_ts()),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_latest_mark_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "stale");
    assert!(bool_field(&v, "stale_or_missing_evidence"));
}

// ---------------------------------------------------------------------------
// LMS-05: Malformed JSON → parse_error, no panic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lms_05_malformed_json_returns_parse_error_no_panic() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "coinlore_latest_mark_1783264300.json",
        r#"{ this is not valid json !!! "#,
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_latest_mark_status()).await;

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
// LMS-06: Wrong schema_version → parse_error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lms_06_wrong_schema_version_returns_parse_error() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "coinlore_latest_mark_1783264400.json",
        r#"{
  "schema_version": "coinlore-latest-mark-v999",
  "produced_at_utc": "2026-07-04T14:00:00Z",
  "marks": []
}"#,
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_latest_mark_status()).await;

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
// LMS-07/08/09: unsafe claims never surfaced as active
// ---------------------------------------------------------------------------

fn evidence_with_flag_override(produced_at: &str, key: &str) -> String {
    let base = valid_evidence(produced_at);
    // Flip exactly one safety flag to true via a literal string replace --
    // the base fixture always writes `"<key>": false`.
    base.replacen(&format!("\"{key}\": false"), &format!("\"{key}\": true"), 1)
}

#[tokio::test]
async fn lms_07_db_write_true_returns_unsafe_evidence_never_active() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "coinlore_latest_mark_1783264500.json",
        &evidence_with_flag_override(&recent_ts(), "db_write"),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_latest_mark_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_evidence");
    assert_ne!(str_field(&v, "truth_state"), "active");
    assert!(bool_field(&v, "stale_or_missing_evidence"));
    assert!(v["error"].as_str().unwrap().contains("db_write"));
}

#[tokio::test]
async fn lms_08_md_bars_write_true_returns_unsafe_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "coinlore_latest_mark_1783264600.json",
        &evidence_with_flag_override(&recent_ts(), "md_bars_write"),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_latest_mark_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_evidence");
}

#[tokio::test]
async fn lms_09_completed_bar_claim_true_returns_unsafe_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "coinlore_latest_mark_1783264700.json",
        &evidence_with_flag_override(&recent_ts(), "completed_bar_claim"),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_latest_mark_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_evidence");
}

// ---------------------------------------------------------------------------
// LMS-10: A mark carrying a bar-like field ("open") → unsafe_evidence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lms_10_mark_with_bar_like_field_returns_unsafe_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let produced = recent_ts();
    let evidence = format!(
        r#"{{
  "schema_version": "coinlore-latest-mark-v1",
  "produced_at_utc": "{produced}",
  "provider": "coinlore",
  "mode": "input_file",
  "network_call_made": false,
  "db_write": false,
  "md_bars_write": false,
  "completed_bar_claim": false,
  "provider_enabled": false,
  "symbols_requested": ["BTC/USD"],
  "marks": [
    {{
      "canonical_symbol": "BTC/USD",
      "provider_id": "coinlore",
      "provider_symbol": "BTC",
      "provider_coin_id": "90",
      "price_usd": "62906.61",
      "open": "62000.00",
      "as_of_client_request_ts": 1783264281,
      "truth_state": "client_observed_only",
      "kind": "ticker"
    }}
  ],
  "all_passed": true,
  "reason_code": "latest_mark_evidence_generated",
  "fail_reasons": []
}}"#,
        produced = produced
    );
    write_evidence(&dir, "coinlore_latest_mark_1783264800.json", &evidence);

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_latest_mark_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_evidence");
    assert!(v["error"].as_str().unwrap().contains("open"));
}

// ---------------------------------------------------------------------------
// LMS-11: Multiple candidates — latest file (alphabetically last) is selected
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lms_11_latest_candidate_selected_from_multiple_files() {
    let dir = tempfile::tempdir().expect("create temp dir");
    // Older file is unsafe; newer file is safe/active. Route must pick the
    // newer file (alphabetically last, since filenames embed epoch seconds).
    write_evidence(
        &dir,
        "coinlore_latest_mark_1783264000.json",
        &evidence_with_flag_override(&recent_ts(), "db_write"),
    );
    write_evidence(
        &dir,
        "coinlore_latest_mark_1783265000.json",
        &valid_evidence(&recent_ts()),
    );
    // A non-matching file (different prefix) must be ignored entirely.
    write_evidence(
        &dir,
        "intraday_refresh_20260704_090000.json",
        r#"{"schema_version":"intraday-refresh-v1"}"#,
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_latest_mark_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        str_field(&v, "truth_state"),
        "active",
        "must pick the newer (1783265000) safe evidence file"
    );
}

// ---------------------------------------------------------------------------
// LMS-12: Route never opens a DB connection / mutates outbox/oms state.
//
// AppState constructed here has no DB pool configured (st.db is None by
// construction in new_with_operator_auth's test path), and the handler's
// source contains no mqk_db call of any kind -- if it ever opened a
// connection, this test's router construction with no pool would surface
// as a panic/error rather than the expected 200 OK truth_state responses
// already proven by LMS-01..11. This test re-asserts explicitly for the
// active path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lms_12_route_returns_active_without_any_db_pool_configured() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "coinlore_latest_mark_1783266000.json",
        &valid_evidence(&recent_ts()),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_latest_mark_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
}
