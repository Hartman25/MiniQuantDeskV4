//! INTRADAY-MD-REFRESHER-OPERATOR-SURFACE-01: operator-visible refresh evidence surface.
//!
//! Proves GET /api/v1/market-data/intraday-refresh/status:
//! - Returns truth_state="no_evidence" when evidence directory is empty.
//! - Returns truth_state="active" and parses success evidence correctly.
//! - Returns truth_state="active" and surfaces failure/fail_reasons correctly.
//! - Returns truth_state="parse_error" (no panic) on malformed JSON.
//! - Returns truth_state="parse_error" on wrong schema_version.
//! - Surfaces provider fields (rows_inserted, rows_updated, filtered counts) when present.
//! - Marks stale_or_missing_evidence=true when evidence is older than 24 h.
//! - No provider API calls, no DB mutation, no broker calls.
//!
//! # Proof matrix
//!
//! | Test      | What it proves                                                                |
//! |-----------|-------------------------------------------------------------------------------|
//! | IRS-01    | Missing evidence dir → truth_state="backend_unavailable", stale_or_missing   |
//! | IRS-02    | Empty evidence dir → truth_state="no_evidence", stale_or_missing=true        |
//! | IRS-03    | Valid success evidence → truth_state="active", all_passed=true, bar ts parsed |
//! | IRS-04    | Valid failure evidence → truth_state="active", all_passed=false, fail_reasons |
//! | IRS-05    | Malformed JSON → truth_state="parse_error", route does not panic              |
//! | IRS-06    | Wrong schema_version → truth_state="parse_error", error field present         |
//! | IRS-07    | Provider fields in evidence → rows_inserted/updated/filtered surfaced         |
//! | IRS-08    | Old produced_at_utc → stale_or_missing_evidence=true even on active response  |
//! | IRS-09    | Latest file selected from multiple candidates (alphabetical last)             |
//! | IRS-10    | Provider success + stale intraday bar forces all_passed=false                 |
//! | IRS-11    | Provider success + zero provider rows forces all_passed=false                 |
//! | IRS-12    | Overage snapshot (age 913>max 900), ~0s elapsed: staleness_overage_secs>=13, risk=high |
//! | IRS-13    | Near-expiry-after-elapsed (age 850, max 900), ~60s elapsed: effective age crosses cap, proof_window_ready=false |
//! | IRS-14    | Ample headroom (age 300, max 900), ~60s elapsed: still low risk, headroom shrinks by elapsed |
//! | IRS-15    | Missing age/max fields: risk=unknown, proof_window_ready=false (fail-safe)   |
//! | IRS-16    | effective_latest_completed_bar_age_secs field explicitly proven >= snapshot+elapsed for a near-cap symbol |
//! | IRS-17    | Ample-headroom symbol: effective age field present, proof_window_ready=true despite nonzero elapsed |
//! | IRS-18    | Near-expiry-from-elapsed: still-passing snapshot (age 780/max 900) becomes near_expiry after ~60s elapsed |
//! | IRS-19    | Already-stale snapshot (age 913/max 900) stays proof_window_ready=false after elapsed grows the overage further |
//! | IRS-20    | Missing/malformed produced_at_utc: evidence_elapsed_secs and effective age both None, risk=unknown |
//!
//! IRS-12..15 are INTRADAY-PROVIDER-CLOCK-SKEW-01B: pure evidence-field
//! classification of freshness headroom/overage risk. IRS-16..20 are
//! INTRADAY-PROVIDER-CLOCK-SKEW-01F: proof-window fields are recomputed from
//! *effective* (elapsed-time-adjusted) bar age, not the evidence snapshot
//! age alone -- IRS-12..15's assertions are also updated to honestly reflect
//! that `recent_ts()`-based fixtures now flow through the effective-age
//! recompute. No new provider/DB/broker calls -- same synthetic temp-dir
//! evidence fixtures as IRS-01..11.
//!
//! All tests use synthetic temp-dir evidence files.
//! No real providers called. No DB writes. No orders. No broker calls.

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

fn get_refresh_status() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/market-data/intraday-refresh/status")
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

/// Write a JSON file into `dir` with the given filename.
fn write_evidence(dir: &tempfile::TempDir, filename: &str, content: &str) {
    let path = dir.path().join(filename);
    std::fs::write(path, content).expect("write test evidence file");
}

/// Build a minimal success evidence JSON string.
fn success_evidence(produced_at: &str, symbol: &str, all_passed: bool) -> String {
    let gate = if all_passed { "PASS" } else { "FAIL" };
    let fail_reasons = if all_passed {
        "[]"
    } else {
        r#"["stale by 200min (threshold=15min)"]"#
    };
    format!(
        r#"{{
  "schema_version": "intraday-refresh-v1",
  "produced_at_utc": "{produced_at}",
  "mode": "once",
  "source": "twelvedata",
  "timeframe": "5m",
  "min_completed_bars_required": 30,
  "max_staleness_minutes": 15,
  "all_passed": {all_passed},
  "reason": "refresh complete",
  "symbols": [
    {{
      "symbol": "{symbol}",
      "gate": "{gate}",
      "completed_count": 5761,
      "max_ts_iso": "2026-06-12 17:00:00",
      "staleness_min": 5,
      "fail_reasons": {fail_reasons}
    }}
  ]
}}"#,
        produced_at = produced_at,
        symbol = symbol,
        all_passed = all_passed,
        gate = gate,
        fail_reasons = fail_reasons
    )
}

/// Current-ish ISO 8601 timestamp (1 minute ago).
fn recent_ts() -> String {
    let ts = chrono::Utc::now() - chrono::Duration::minutes(1);
    ts.to_rfc3339()
}

/// Effectively "now" -- produced_at_utc with ~0s elapsed by the time the
/// route evaluates it. Used where a test wants to isolate the pure
/// headroom/overage math from the elapsed-time recompute.
fn now_ts() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Old timestamp (48 h ago).
fn old_ts() -> String {
    let ts = chrono::Utc::now() - chrono::Duration::hours(48);
    ts.to_rfc3339()
}

// ---------------------------------------------------------------------------
// IRS-01: Missing evidence directory → backend_unavailable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_01_missing_evidence_dir_returns_backend_unavailable() {
    let router = make_router_with_evidence_dir("/tmp/mqk_does_not_exist_irs01_abc123");
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "backend_unavailable");
    assert!(bool_field(&v, "stale_or_missing_evidence"));
    assert!(v["error"].as_str().is_some(), "error field must be present");
    assert!(v["symbols"].as_array().map(|a| a.len()).unwrap_or(1) == 0);
}

// ---------------------------------------------------------------------------
// IRS-02: Empty evidence directory → no_evidence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_02_empty_evidence_dir_returns_no_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "no_evidence");
    assert!(bool_field(&v, "stale_or_missing_evidence"));
    assert!(v["error"].is_null(), "no error on empty dir");
    assert!(v["evidence_path"].is_null());
}

// ---------------------------------------------------------------------------
// IRS-03: Valid success evidence → active, all_passed=true, symbol parsed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_03_valid_success_evidence_parses_correctly() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "intraday_refresh_20260612_120000.json",
        &success_evidence(&recent_ts(), "AAPL", true),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
    assert!(
        !bool_field(&v, "stale_or_missing_evidence"),
        "recent evidence must not be stale"
    );
    assert_eq!(v["all_passed"].as_bool(), Some(true));
    assert_eq!(str_field(&v, "mode"), "once");
    assert_eq!(str_field(&v, "source"), "twelvedata");
    assert_eq!(str_field(&v, "timeframe"), "5m");
    assert!(v["evidence_path"].as_str().is_some());
    assert!(v["produced_at_utc"].as_str().is_some());

    let symbols = v["symbols"].as_array().expect("symbols must be array");
    assert_eq!(symbols.len(), 1);
    let sym = &symbols[0];
    assert_eq!(sym["symbol"].as_str(), Some("AAPL"));
    assert_eq!(sym["gate"].as_str(), Some("PASS"));
    assert_eq!(sym["completed_count"].as_i64(), Some(5761));
    assert_eq!(
        sym["latest_completed_bar_ts"].as_str(),
        Some("2026-06-12 17:00:00")
    );
    assert_eq!(sym["staleness_min"].as_i64(), Some(5));
    let fail_reasons = sym["fail_reasons"].as_array().expect("fail_reasons array");
    assert!(
        fail_reasons.is_empty(),
        "PASS symbol must have no fail reasons"
    );
}

// ---------------------------------------------------------------------------
// IRS-04: Valid failure evidence → active, all_passed=false, fail_reasons surfaced
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_04_valid_failure_evidence_surfaces_fail_reasons() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "intraday_refresh_20260612_121000.json",
        &success_evidence(&recent_ts(), "SPY", false),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
    assert_eq!(v["all_passed"].as_bool(), Some(false));

    let symbols = v["symbols"].as_array().expect("symbols array");
    assert_eq!(symbols.len(), 1);
    let sym = &symbols[0];
    assert_eq!(sym["gate"].as_str(), Some("FAIL"));
    let fail_reasons = sym["fail_reasons"].as_array().expect("fail_reasons array");
    assert!(
        !fail_reasons.is_empty(),
        "FAIL symbol must have at least one fail reason"
    );
    let first_reason = fail_reasons[0]
        .as_str()
        .expect("fail reason must be string");
    assert!(
        first_reason.contains("stale"),
        "fail reason must mention staleness: {}",
        first_reason
    );
}

// ---------------------------------------------------------------------------
// IRS-05: Malformed JSON → parse_error, no panic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_05_malformed_json_returns_parse_error_no_panic() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "intraday_refresh_20260612_130000.json",
        r#"{ this is not valid json !!! "#,
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "malformed evidence must still return 200"
    );
    assert_eq!(str_field(&v, "truth_state"), "parse_error");
    assert!(bool_field(&v, "stale_or_missing_evidence"));
    assert!(
        v["error"].as_str().is_some(),
        "error field must be present on parse_error"
    );
}

// ---------------------------------------------------------------------------
// IRS-06: Wrong schema_version → parse_error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_06_wrong_schema_version_returns_parse_error() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "intraday_refresh_20260612_140000.json",
        r#"{
  "schema_version": "intraday-refresh-v999",
  "produced_at_utc": "2026-06-12T14:00:00Z",
  "mode": "once",
  "all_passed": true,
  "symbols": []
}"#,
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "parse_error");
    assert!(bool_field(&v, "stale_or_missing_evidence"));
    let err = v["error"].as_str().expect("error field must be present");
    assert!(
        err.contains("schema_version") || err.contains("unsupported"),
        "error must mention schema_version: {}",
        err
    );
}

// ---------------------------------------------------------------------------
// IRS-07: Provider fields → surfaced in symbol status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_07_provider_fields_surfaced_when_present() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let produced = recent_ts();
    let evidence = format!(
        r#"{{
  "schema_version": "intraday-refresh-v1",
  "produced_at_utc": "{produced}",
  "mode": "once",
  "source": "twelvedata",
  "timeframe": "5m",
  "min_completed_bars_required": 30,
  "max_staleness_minutes": 15,
  "all_passed": true,
  "reason": "refresh complete",
  "symbols": [
    {{
      "symbol": "AAPL",
      "timeframe": "5m",
      "provider_source": "twelvedata",
      "attempted_at_utc": "{produced}",
      "provider_configured": true,
      "provider_attempted": true,
      "provider_success": true,
      "provider_exit_code": 0,
      "provider_rows_read": 100,
      "provider_rows_inserted": 12,
      "provider_rows_updated": 3,
      "provider_rows_kept_after_completion_filter": 95,
      "provider_rows_dropped_incomplete": 4,
      "provider_rows_dropped_current": 1,
      "provider_latest_completed_end_ts": "2026-06-12 17:00:00",
      "gate": "PASS",
      "completed_count": 5761,
      "max_ts_iso": "2026-06-12 17:00:00",
      "staleness_min": 5,
      "fail_reasons": []
    }}
  ]
}}"#,
        produced = produced
    );

    write_evidence(&dir, "intraday_refresh_20260612_150000.json", &evidence);
    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");

    let symbols = v["symbols"].as_array().expect("symbols array");
    let sym = &symbols[0];
    assert_eq!(sym["provider_source"].as_str(), Some("twelvedata"));
    assert_eq!(sym["provider_configured"].as_bool(), Some(true));
    assert_eq!(sym["provider_attempted"].as_bool(), Some(true));
    assert_eq!(sym["provider_success"].as_bool(), Some(true));
    assert_eq!(sym["rows_read"].as_i64(), Some(100));
    assert_eq!(sym["rows_inserted"].as_i64(), Some(12));
    assert_eq!(sym["rows_updated"].as_i64(), Some(3));
    assert_eq!(sym["rows_filtered_incomplete"].as_i64(), Some(4));
    assert_eq!(sym["rows_filtered_in_progress"].as_i64(), Some(1));
}

// ---------------------------------------------------------------------------
// IRS-10: Provider success + stale intraday bar → active, all_passed=false
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_10_stale_after_refresh_overrides_optimistic_all_passed() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let produced = recent_ts();
    let evidence = format!(
        r#"{{
  "schema_version": "intraday-refresh-v1",
  "produced_at_utc": "{produced}",
  "mode": "once",
  "source": "twelvedata",
  "timeframe": "5m",
  "min_completed_bars_required": 30,
  "max_staleness_minutes": 1440,
  "all_passed": true,
  "reason": "provider sync complete",
  "symbols": [
    {{
      "symbol": "AAPL",
      "timeframe": "5m",
      "provider_source": "twelvedata",
      "provider_configured": true,
      "provider_attempted": true,
      "provider_success": true,
      "provider_exit_code": 0,
      "provider_rows_read": 156,
      "provider_rows_ok": 156,
      "provider_rows_inserted": 0,
      "provider_rows_updated": 156,
      "gate": "PASS",
      "completed_count": 5761,
      "max_ts_iso": "2026-06-23T19:55:00Z",
      "latest_completed_bar_age_secs": 84000,
      "max_allowed_age_secs": 900,
      "staleness_min": 1400,
      "fail_reasons": []
    }}
  ]
}}"#,
        produced = produced
    );

    write_evidence(&dir, "intraday_refresh_20260625_120000.json", &evidence);
    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
    assert_eq!(
        v["all_passed"].as_bool(),
        Some(false),
        "route must not preserve optimistic all_passed=true when symbol evidence is stale"
    );

    let symbols = v["symbols"].as_array().expect("symbols array");
    let sym = &symbols[0];
    assert_eq!(sym["passed"].as_bool(), Some(false));
    assert_eq!(
        sym["freshness_truth_state"].as_str(),
        Some("stale_after_refresh")
    );
    assert_eq!(
        sym["reason_code"].as_str(),
        Some("provider_returned_stale_intraday_data")
    );
    assert_eq!(sym["latest_completed_bar_age_secs"].as_i64(), Some(84000));
    assert_eq!(sym["max_allowed_age_secs"].as_i64(), Some(900));
    assert_eq!(sym["rows_read"].as_i64(), Some(156));
    assert_eq!(sym["rows_ok"].as_i64(), Some(156));
}

// ---------------------------------------------------------------------------
// IRS-11: Provider success + no rows → active, all_passed=false
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_11_provider_success_no_rows_is_failed_reason() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let produced = recent_ts();
    let evidence = format!(
        r#"{{
  "schema_version": "intraday-refresh-v1",
  "produced_at_utc": "{produced}",
  "mode": "once",
  "source": "twelvedata",
  "timeframe": "5m",
  "all_passed": true,
  "reason": "provider sync complete",
  "symbols": [
    {{
      "symbol": "AAPL",
      "timeframe": "5m",
      "provider_source": "twelvedata",
      "provider_attempted": true,
      "provider_success": true,
      "provider_rows_read": 0,
      "provider_rows_ok": 0,
      "gate": "PASS",
      "completed_count": 30,
      "latest_completed_bar_age_secs": 60,
      "max_allowed_age_secs": 900,
      "staleness_min": 1,
      "fail_reasons": []
    }}
  ]
}}"#,
        produced = produced
    );

    write_evidence(&dir, "intraday_refresh_20260625_121000.json", &evidence);
    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
    assert_eq!(v["all_passed"].as_bool(), Some(false));
    let sym = &v["symbols"].as_array().expect("symbols array")[0];
    assert_eq!(sym["passed"].as_bool(), Some(false));
    assert_eq!(
        sym["reason_code"].as_str(),
        Some("provider_returned_no_rows")
    );
    assert_eq!(sym["rows_read"].as_i64(), Some(0));
}

// ---------------------------------------------------------------------------
// IRS-08: Old produced_at_utc → stale_or_missing_evidence=true even on active
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_08_old_evidence_is_flagged_stale() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "intraday_refresh_20260601_000000.json",
        &success_evidence(&old_ts(), "AAPL", true),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
    assert!(
        bool_field(&v, "stale_or_missing_evidence"),
        "evidence produced 48 h ago must be flagged stale"
    );
}

// ---------------------------------------------------------------------------
// IRS-09: Multiple candidates — latest file (alphabetically last) is selected
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_09_latest_candidate_selected_from_multiple_files() {
    let dir = tempfile::tempdir().expect("create temp dir");
    // Older file has all_passed=false; newer file has all_passed=true.
    // Route must pick the newer file (alphabetically last).
    write_evidence(
        &dir,
        "intraday_refresh_20260612_080000.json",
        &success_evidence(&recent_ts(), "AAPL", false),
    );
    write_evidence(
        &dir,
        "intraday_refresh_20260612_090000.json",
        &success_evidence(&recent_ts(), "AAPL", true),
    );
    // Also place a non-matching file to confirm it is ignored.
    write_evidence(
        &dir,
        "premarket_prep_20260612_090000.json",
        r#"{"schema_version":"other"}"#,
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
    assert_eq!(
        v["all_passed"].as_bool(),
        Some(true),
        "must pick latest file (090000) which has all_passed=true"
    );
}

// ---------------------------------------------------------------------------
// Shared helper for IRS-12..15: evidence with explicit age/max_age fields.
// ---------------------------------------------------------------------------

fn headroom_evidence(produced_at: &str, age_secs: Option<i64>, max_age_secs: Option<i64>) -> String {
    let age_field = match age_secs {
        Some(a) => format!(r#""latest_completed_bar_age_secs": {a},"#),
        None => String::new(),
    };
    let max_field = match max_age_secs {
        Some(m) => format!(r#""max_allowed_age_secs": {m},"#),
        None => String::new(),
    };
    format!(
        r#"{{
  "schema_version": "intraday-refresh-v1",
  "produced_at_utc": "{produced_at}",
  "mode": "once",
  "source": "twelvedata",
  "timeframe": "5m",
  "all_passed": true,
  "reason": "provider sync complete",
  "symbols": [
    {{
      "symbol": "AAPL",
      "timeframe": "5m",
      "provider_source": "twelvedata",
      "provider_attempted": true,
      "provider_success": true,
      {age_field}
      {max_field}
      "gate": "PASS",
      "completed_count": 5761,
      "fail_reasons": []
    }}
  ]
}}"#,
        produced_at = produced_at,
        age_field = age_field,
        max_field = max_field,
    )
}

// ---------------------------------------------------------------------------
// IRS-12: age 913 > max 900, ~0s elapsed -> staleness_overage_secs>=13, risk=high, not near_expiry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_12_overage_reports_staleness_overage_and_high_risk() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "intraday_refresh_20260710_090000.json",
        &headroom_evidence(&now_ts(), Some(913), Some(900)),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    let sym = &v["symbols"].as_array().expect("symbols array")[0];
    assert_eq!(sym["passed"].as_bool(), Some(false));
    assert_eq!(sym["freshness_headroom_secs"].as_i64(), None);
    let overage = sym["staleness_overage_secs"]
        .as_i64()
        .expect("staleness_overage_secs must be present");
    assert!(
        (13..20).contains(&overage),
        "expected overage in [13,20) (13 baseline + tiny test-runtime elapsed), got {overage}"
    );
    assert_eq!(sym["near_expiry"].as_bool(), Some(false));
    assert_eq!(sym["proof_window_risk"].as_str(), Some("high"));
    assert!(
        sym["operator_action"].as_str().is_some(),
        "operator_action must be present when already stale"
    );
    assert_eq!(v["proof_window_ready"].as_bool(), Some(false));
    assert_eq!(v["proof_window_risk"].as_str(), Some("high"));
}

// ---------------------------------------------------------------------------
// IRS-13: age 850, max 900, ~60s elapsed -> effective age crosses the cap,
// proof_window_ready=false even though the symbol's own snapshot-based
// `passed` verdict stays true. INTRADAY-PROVIDER-CLOCK-SKEW-01F.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_13_near_expiry_reports_headroom_and_high_risk() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "intraday_refresh_20260710_090100.json",
        &headroom_evidence(&recent_ts(), Some(850), Some(900)),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    let sym = &v["symbols"].as_array().expect("symbols array")[0];
    assert_eq!(
        sym["passed"].as_bool(),
        Some(true),
        "snapshot-based passed verdict is unaffected by effective-age recompute"
    );
    // 850 snapshot + ~60s elapsed pushes effective age past the 900s cap:
    // headroom is now None and staleness_overage_secs is now Some.
    assert!(sym["freshness_headroom_secs"].is_null());
    let overage = sym["staleness_overage_secs"]
        .as_i64()
        .expect("staleness_overage_secs must be present once effective age crosses the cap");
    assert!(
        (5..20).contains(&overage),
        "expected overage in [5,20) (~10 baseline + tiny test-runtime elapsed), got {overage}"
    );
    assert_eq!(sym["near_expiry"].as_bool(), Some(false));
    assert_eq!(sym["proof_window_risk"].as_str(), Some("high"));
    assert!(
        sym["operator_action"].as_str().is_some(),
        "operator_action must be present once effective age is past cap"
    );
    assert_eq!(
        v["proof_window_ready"].as_bool(),
        Some(false),
        "effective-age overage must block proof_window_ready even though the symbol currently passes"
    );
}

// ---------------------------------------------------------------------------
// IRS-14: age 300, max 900, ~60s elapsed -> still ample headroom, low risk;
// headroom shrinks by roughly the elapsed amount. INTRADAY-PROVIDER-CLOCK-SKEW-01F.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_14_ample_headroom_reports_low_risk() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "intraday_refresh_20260710_090200.json",
        &headroom_evidence(&recent_ts(), Some(300), Some(900)),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    let sym = &v["symbols"].as_array().expect("symbols array")[0];
    assert_eq!(sym["passed"].as_bool(), Some(true));
    let headroom = sym["freshness_headroom_secs"]
        .as_i64()
        .expect("freshness_headroom_secs must be present");
    assert!(
        (530..=600).contains(&headroom),
        "expected headroom in [530,600] (600 baseline minus ~60s elapsed), got {headroom}"
    );
    assert_eq!(sym["near_expiry"].as_bool(), Some(false));
    assert_eq!(sym["proof_window_risk"].as_str(), Some("low"));
    assert!(sym["operator_action"].is_null());
    assert_eq!(v["proof_window_ready"].as_bool(), Some(true));
    assert_eq!(v["proof_window_risk"].as_str(), Some("low"));
}

// ---------------------------------------------------------------------------
// IRS-15: missing age/max fields -> risk=unknown, proof_window_ready fails safe
// ---------------------------------------------------------------------------

#[tokio::test]
async fn irs_15_missing_age_fields_fails_safe_as_unknown() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "intraday_refresh_20260710_090300.json",
        &headroom_evidence(&recent_ts(), None, None),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    let sym = &v["symbols"].as_array().expect("symbols array")[0];
    assert_eq!(sym["freshness_headroom_secs"].as_i64(), None);
    assert_eq!(sym["staleness_overage_secs"].as_i64(), None);
    assert_eq!(sym["near_expiry"].as_bool(), Some(false));
    assert_eq!(sym["proof_window_risk"].as_str(), Some("unknown"));
    assert!(
        sym["operator_action"].as_str().is_some(),
        "operator_action must explain the evidence gap"
    );
    assert_eq!(
        v["proof_window_ready"].as_bool(),
        Some(false),
        "missing freshness fields must never be read as proof-window-ready"
    );
    assert_eq!(v["proof_window_risk"].as_str(), Some("unknown"));
}

// ---------------------------------------------------------------------------
// IRS-16..20: INTRADAY-PROVIDER-CLOCK-SKEW-01F -- proof-window fields are
// recomputed from *effective* (elapsed-time-adjusted) bar age, explicitly
// proving the new `evidence_elapsed_secs` / `effective_latest_completed_bar_age_secs`
// fields, not just the downstream headroom/overage/risk numbers IRS-12..15
// already cover. Same synthetic temp-dir evidence fixtures; no provider/DB/
// broker calls.
// ---------------------------------------------------------------------------

// IRS-16: age 850/max 900, ~60s elapsed -> effective_latest_completed_bar_age_secs
// explicitly proven >= snapshot age + elapsed, and >= the 900s cap.
#[tokio::test]
async fn irs_16_effective_age_field_reflects_snapshot_plus_elapsed() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "intraday_refresh_20260710_090400.json",
        &headroom_evidence(&recent_ts(), Some(850), Some(900)),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    let sym = &v["symbols"].as_array().expect("symbols array")[0];
    let elapsed = sym["evidence_elapsed_secs"]
        .as_i64()
        .expect("evidence_elapsed_secs must be present");
    assert!(
        elapsed >= 60,
        "expected evidence_elapsed_secs >= 60 (recent_ts is 60s ago), got {elapsed}"
    );
    let effective_age = sym["effective_latest_completed_bar_age_secs"]
        .as_i64()
        .expect("effective_latest_completed_bar_age_secs must be present");
    assert_eq!(
        effective_age,
        850 + elapsed,
        "effective age must equal snapshot age (850) plus evidence_elapsed_secs"
    );
    assert!(
        effective_age >= 910,
        "effective age must have crossed the 900s cap, got {effective_age}"
    );
    // Snapshot value is preserved unchanged for backward compatibility.
    assert_eq!(sym["latest_completed_bar_age_secs"].as_i64(), Some(850));
}

// IRS-17: age 300/max 900, ~60s elapsed -> effective age field present,
// proof_window_ready=true despite nonzero elapsed (ample headroom survives).
#[tokio::test]
async fn irs_17_effective_age_ample_headroom_stays_ready() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "intraday_refresh_20260710_090500.json",
        &headroom_evidence(&recent_ts(), Some(300), Some(900)),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    let sym = &v["symbols"].as_array().expect("symbols array")[0];
    let elapsed = sym["evidence_elapsed_secs"]
        .as_i64()
        .expect("evidence_elapsed_secs must be present");
    let effective_age = sym["effective_latest_completed_bar_age_secs"]
        .as_i64()
        .expect("effective_latest_completed_bar_age_secs must be present");
    assert_eq!(effective_age, 300 + elapsed);
    assert!(
        (355..=370).contains(&effective_age),
        "expected effective age around 360 (300 + ~60s elapsed), got {effective_age}"
    );
    assert_eq!(sym["proof_window_risk"].as_str(), Some("low"));
    assert_eq!(v["proof_window_ready"].as_bool(), Some(true));
}

// IRS-18: age 780/max 900, ~60s elapsed -> still-passing snapshot becomes
// near_expiry once effective age is considered.
#[tokio::test]
async fn irs_18_still_passing_snapshot_becomes_near_expiry_via_elapsed() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "intraday_refresh_20260710_090600.json",
        &headroom_evidence(&recent_ts(), Some(780), Some(900)),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    let sym = &v["symbols"].as_array().expect("symbols array")[0];
    assert_eq!(
        sym["passed"].as_bool(),
        Some(true),
        "snapshot age 780 < max 900 -- the snapshot-based passed verdict stays true"
    );
    let effective_age = sym["effective_latest_completed_bar_age_secs"]
        .as_i64()
        .expect("effective_latest_completed_bar_age_secs must be present");
    assert!(
        (835..=850).contains(&effective_age),
        "expected effective age around 840 (780 + ~60s elapsed), got {effective_age}"
    );
    assert_eq!(sym["near_expiry"].as_bool(), Some(true));
    assert_eq!(sym["proof_window_risk"].as_str(), Some("high"));
    assert_eq!(
        v["proof_window_ready"].as_bool(),
        Some(false),
        "near_expiry via effective age must block proof_window_ready"
    );
}

// IRS-19: age 913/max 900 (already stale at production time), ~60s elapsed
// -> proof_window_ready stays false; effective-age recompute only grows the
// overage further, proving the repair does not regress the already-stale case.
#[tokio::test]
async fn irs_19_already_stale_snapshot_stays_not_ready_after_elapsed() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "intraday_refresh_20260710_090700.json",
        &headroom_evidence(&recent_ts(), Some(913), Some(900)),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    let sym = &v["symbols"].as_array().expect("symbols array")[0];
    assert_eq!(sym["passed"].as_bool(), Some(false));
    let effective_age = sym["effective_latest_completed_bar_age_secs"]
        .as_i64()
        .expect("effective_latest_completed_bar_age_secs must be present");
    assert!(
        effective_age >= 913 + 60,
        "elapsed time must only grow the already-present overage, got effective_age={effective_age}"
    );
    let overage = sym["staleness_overage_secs"]
        .as_i64()
        .expect("staleness_overage_secs must be present");
    assert!(overage >= 13 + 60);
    assert_eq!(v["proof_window_ready"].as_bool(), Some(false));
    assert_eq!(v["proof_window_risk"].as_str(), Some("high"));
}

// IRS-20: missing/malformed produced_at_utc -> evidence_elapsed_secs and
// effective_latest_completed_bar_age_secs both None; proof-window risk fails
// safe to "unknown" exactly like a missing age/max field would.
#[tokio::test]
async fn irs_20_malformed_produced_at_utc_fails_safe_as_unknown() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        "intraday_refresh_20260710_090800.json",
        &headroom_evidence("not-a-real-timestamp", Some(300), Some(900)),
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_refresh_status()).await;

    assert_eq!(status, StatusCode::OK);
    let sym = &v["symbols"].as_array().expect("symbols array")[0];
    assert!(
        sym["evidence_elapsed_secs"].is_null(),
        "evidence_elapsed_secs must be None when produced_at_utc cannot be parsed"
    );
    assert!(
        sym["effective_latest_completed_bar_age_secs"].is_null(),
        "effective_latest_completed_bar_age_secs must be None when produced_at_utc cannot be parsed"
    );
    // Snapshot value is still surfaced unchanged even though the effective
    // recompute is unavailable.
    assert_eq!(sym["latest_completed_bar_age_secs"].as_i64(), Some(300));
    assert_eq!(sym["proof_window_risk"].as_str(), Some("unknown"));
    assert!(
        sym["operator_action"].as_str().is_some(),
        "operator_action must explain the missing effective-age input"
    );
    assert_eq!(
        v["proof_window_ready"].as_bool(),
        Some(false),
        "unknown effective-age risk must never be read as proof-window-ready"
    );
    assert!(
        v["stale_or_missing_evidence"]
            .as_bool()
            .expect("stale_or_missing_evidence must be a bool"),
        "malformed produced_at_utc must also mark the evidence itself as stale/missing"
    );
}
