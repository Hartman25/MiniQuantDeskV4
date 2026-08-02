//! CRYPTO-DATA-03C-KRAKEN-SCHEDULER-TASK-STATUS-SURFACE-01: read-only
//! task-registration evidence status surface.
//!
//! Proves GET /api/v1/market-data/kraken-scheduler/task-status:
//! - Returns truth_state="no_evidence" when the evidence file is absent.
//! - Returns truth_state="backend_unavailable" when the evidence directory is missing.
//! - Returns truth_state="active" and parses valid check-only evidence correctly.
//! - Returns truth_state="active" and parses valid register evidence, registered=true.
//! - Returns truth_state="parse_error" (no panic) on malformed JSON.
//! - Returns truth_state="parse_error" on wrong schema_version.
//! - Returns truth_state="unsafe_evidence" when network_call_made=true.
//! - Returns truth_state="unsafe_evidence" when db_write=true.
//! - Returns truth_state="unsafe_evidence" when md_bars_write=true.
//! - Returns truth_state="unsafe_evidence" when env_vars_embedded is non-empty.
//! - Returns truth_state="unsafe_evidence" when check-only evidence claims
//!   scheduled_task_mutation=true.
//! - Returns truth_state="unsafe_evidence" when task_action is missing the
//!   expected Run-KrakenOhlcSync.ps1 reference.
//! - Never opens a DB connection, never mutates outbox/oms rows, never calls
//!   Kraken, never shells out to PowerShell, never touches the Windows Task
//!   Scheduler.
//!
//! # Proof matrix
//!
//! | Test    | What it proves                                                              |
//! |---------|------------------------------------------------------------------------------|
//! | KTS-01  | Missing evidence dir → truth_state="backend_unavailable"                    |
//! | KTS-02  | No evidence file (empty dir) → truth_state="no_evidence"                    |
//! | KTS-03  | Valid check-only evidence → truth_state="active", fields round-trip         |
//! | KTS-04  | Valid register evidence → truth_state="active", registered=true            |
//! | KTS-05  | Malformed JSON → truth_state="parse_error", no panic                       |
//! | KTS-06  | Wrong schema_version → truth_state="parse_error"                           |
//! | KTS-07  | network_call_made=true → truth_state="unsafe_evidence"                     |
//! | KTS-08  | db_write=true → truth_state="unsafe_evidence"                              |
//! | KTS-09  | md_bars_write=true → truth_state="unsafe_evidence"                         |
//! | KTS-10  | env_vars_embedded non-empty → truth_state="unsafe_evidence"                |
//! | KTS-11  | check_only evidence claiming scheduled_task_mutation=true → unsafe_evidence |
//! | KTS-12  | task_action missing Run-KrakenOhlcSync.ps1 reference → unsafe_evidence      |
//! | KTS-13  | Route returns active without any DB pool configured (no DB dependency)     |
//!
//! All tests use synthetic temp-dir evidence files. No real Kraken network
//! call made. No DB writes. No orders. No broker calls. No outbox/oms rows.
//! No CLI execution, no PowerShell subprocess, no scheduler registered.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

const EVIDENCE_FILENAME: &str = "kraken_ohlc_task_registration.json";

fn make_router_with_evidence_dir(dir: &str) -> axum::Router {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.md_refresh_evidence_dir = dir.to_string();
    routes::build_router(Arc::new(st))
}

fn get_task_status() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/market-data/kraken-scheduler/task-status")
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

fn write_evidence(dir: &tempfile::TempDir, content: &str) {
    let path = dir.path().join(EVIDENCE_FILENAME);
    std::fs::write(path, content).expect("write test evidence file");
}

/// A minimal, safe, valid `kraken-ohlc-task-registration-v1` check-only evidence body.
fn valid_check_only_evidence() -> String {
    r#"{
  "schema_version": "kraken-ohlc-task-registration-v1",
  "patch_id": "CRYPTO-DATA-03B-KRAKEN-SCHEDULER-TASK-SCRIPTS-01",
  "producer": "Register-KrakenOhlcSyncTask.ps1",
  "produced_at_utc": "2026-07-04T14:00:00.000Z",
  "mode": "check_only",
  "task_name": "MiniQuantDesk-KrakenOhlcSync",
  "task_path": "\\",
  "task_exists_before": false,
  "task_exists_after": false,
  "registered": false,
  "unregistered": false,
  "check_only": true,
  "task_action": "-ExecutionPolicy Bypass -NonInteractive -WindowStyle Hidden -Command \"& Run-KrakenOhlcSync.ps1 -CheckOnly:$false\"",
  "runner_path": "C:\\repo\\scripts\\windows\\Run-KrakenOhlcSync.ps1",
  "policy_path": "docs/specs/crypto_data_02a_kraken_scheduler_rate_limit_decision.json",
  "registry_path": "config/instruments/instruments_v2.crypto_local_marks.example.json",
  "providers_path": "config/providers/providers.json",
  "symbols": ["BTC/USD", "ETH/USD"],
  "timeframe": "1D",
  "at": "07:45",
  "scheduled_task_mutation": false,
  "network_call_made": false,
  "db_write": false,
  "md_bars_write": false,
  "env_vars_embedded": [],
  "env_vars_required": ["MQK_DATABASE_URL", "MQK_ALLOW_KRAKEN_SCHEDULED_SYNC"],
  "all_passed": true,
  "reason_code": "task_registration_preview_generated",
  "fail_reasons": [],
  "warnings": [],
  "safety": {
    "calls_runner_script_only": true,
    "no_daemon_runtime_broker_provider_order_references": true,
    "no_env_vars_embedded_in_task_action": true,
    "no_env_local_read": true,
    "task_never_started_by_this_script": true
  }
}"#
    .to_string()
}

/// A minimal, safe, valid `register` evidence body (registered=true, no
/// route-side mutation implied -- this simulates the operator having
/// already run `-Register` separately from this route).
fn valid_register_evidence() -> String {
    r#"{
  "schema_version": "kraken-ohlc-task-registration-v1",
  "patch_id": "CRYPTO-DATA-03B-KRAKEN-SCHEDULER-TASK-SCRIPTS-01",
  "producer": "Register-KrakenOhlcSyncTask.ps1",
  "produced_at_utc": "2026-07-04T14:05:00.000Z",
  "mode": "register",
  "task_name": "MiniQuantDesk-KrakenOhlcSync",
  "task_path": "\\",
  "task_exists_before": false,
  "task_exists_after": true,
  "registered": true,
  "unregistered": false,
  "check_only": false,
  "task_action": "-ExecutionPolicy Bypass -NonInteractive -WindowStyle Hidden -Command \"& Run-KrakenOhlcSync.ps1 -CheckOnly:$false\"",
  "runner_path": "C:\\repo\\scripts\\windows\\Run-KrakenOhlcSync.ps1",
  "policy_path": "docs/specs/crypto_data_02a_kraken_scheduler_rate_limit_decision.json",
  "registry_path": "config/instruments/instruments_v2.crypto_local_marks.example.json",
  "providers_path": "config/providers/providers.json",
  "symbols": ["BTC/USD", "ETH/USD"],
  "timeframe": "1D",
  "at": "07:45",
  "scheduled_task_mutation": true,
  "network_call_made": false,
  "db_write": false,
  "md_bars_write": false,
  "env_vars_embedded": [],
  "env_vars_required": ["MQK_DATABASE_URL", "MQK_ALLOW_KRAKEN_SCHEDULED_SYNC"],
  "all_passed": true,
  "reason_code": "task_registered",
  "fail_reasons": [],
  "warnings": [],
  "safety": {
    "calls_runner_script_only": true,
    "no_daemon_runtime_broker_provider_order_references": true,
    "no_env_vars_embedded_in_task_action": true,
    "no_env_local_read": true,
    "task_never_started_by_this_script": true
  }
}"#
    .to_string()
}

// ---------------------------------------------------------------------------
// KTS-01: Missing evidence directory → backend_unavailable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kts_01_missing_evidence_dir_returns_backend_unavailable() {
    let router = make_router_with_evidence_dir("/tmp/mqk_does_not_exist_kts01_abc123");
    let (status, v) = call(router, get_task_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "backend_unavailable");
    assert!(v["error"].as_str().is_some(), "error field must be present");
}

// ---------------------------------------------------------------------------
// KTS-02: No evidence file (empty dir) → no_evidence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kts_02_no_evidence_file_returns_no_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_task_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "no_evidence");
    assert!(v["error"].is_null(), "no error when evidence file absent");
    assert!(v["evidence_path"].is_null());
}

// ---------------------------------------------------------------------------
// KTS-03: Valid check-only evidence → active, fields round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kts_03_valid_check_only_evidence_parses_correctly() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(&dir, &valid_check_only_evidence());

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_task_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
    assert_eq!(str_field(&v, "mode"), "check_only");
    assert_eq!(str_field(&v, "task_name"), "MiniQuantDesk-KrakenOhlcSync");
    assert_eq!(v["task_exists_before"].as_bool(), Some(false));
    assert_eq!(v["task_exists_after"].as_bool(), Some(false));
    assert_eq!(v["registered"].as_bool(), Some(false));
    assert_eq!(v["unregistered"].as_bool(), Some(false));
    assert_eq!(v["check_only"].as_bool(), Some(true));
    assert_eq!(v["scheduled_task_mutation"].as_bool(), Some(false));
    assert_eq!(v["network_call_made"].as_bool(), Some(false));
    assert_eq!(v["db_write"].as_bool(), Some(false));
    assert_eq!(v["md_bars_write"].as_bool(), Some(false));
    assert_eq!(v["all_passed"].as_bool(), Some(true));
    let symbols = v["symbols"].as_array().expect("symbols array");
    assert_eq!(symbols.len(), 2);
    let env_required = v["env_vars_required"].as_array().expect("array");
    assert_eq!(env_required.len(), 2);
    assert!(v["env_vars_embedded"].as_array().expect("array").is_empty());
    assert_eq!(
        v["safety"]["calls_runner_script_only"].as_bool(),
        Some(true)
    );
    assert!(v["evidence_path"].as_str().is_some());
    assert!(v["error"].is_null());
}

// ---------------------------------------------------------------------------
// KTS-04: Valid register evidence → active, registered=true
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kts_04_valid_register_evidence_surfaces_registered_true() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(&dir, &valid_register_evidence());

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_task_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
    assert_eq!(str_field(&v, "mode"), "register");
    assert_eq!(v["registered"].as_bool(), Some(true));
    assert_eq!(v["task_exists_after"].as_bool(), Some(true));
    assert_eq!(v["check_only"].as_bool(), Some(false));
    // Route itself performed no mutation -- it only reported the evidence's
    // own historical claim truthfully.
    assert!(v["error"].is_null());
}

// ---------------------------------------------------------------------------
// KTS-05: Malformed JSON → parse_error, no panic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kts_05_malformed_json_returns_parse_error_no_panic() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(&dir, r#"{ this is not valid json !!! "#);

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_task_status()).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "malformed evidence must still return 200"
    );
    assert_eq!(str_field(&v, "truth_state"), "parse_error");
    assert!(v["error"].as_str().is_some());
}

// ---------------------------------------------------------------------------
// KTS-06: Wrong schema_version → parse_error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kts_06_wrong_schema_version_returns_parse_error() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(
        &dir,
        r#"{
  "schema_version": "kraken-ohlc-task-registration-v0",
  "produced_at_utc": "2026-07-04T14:00:00Z",
  "mode": "check_only"
}"#,
    );

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_task_status()).await;

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
// KTS-07: network_call_made=true → unsafe_evidence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kts_07_network_call_made_returns_unsafe_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let evidence = valid_check_only_evidence().replacen(
        "\"network_call_made\": false,",
        "\"network_call_made\": true,",
        1,
    );
    write_evidence(&dir, &evidence);

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_task_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_evidence");
    assert!(v["error"].as_str().unwrap().contains("network_call_made"));
}

// ---------------------------------------------------------------------------
// KTS-08: db_write=true → unsafe_evidence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kts_08_db_write_true_returns_unsafe_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let evidence =
        valid_check_only_evidence().replacen("\"db_write\": false,", "\"db_write\": true,", 1);
    write_evidence(&dir, &evidence);

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_task_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_evidence");
    assert!(v["error"].as_str().unwrap().contains("db_write"));
}

// ---------------------------------------------------------------------------
// KTS-09: md_bars_write=true → unsafe_evidence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kts_09_md_bars_write_true_returns_unsafe_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let evidence = valid_check_only_evidence().replacen(
        "\"md_bars_write\": false,",
        "\"md_bars_write\": true,",
        1,
    );
    write_evidence(&dir, &evidence);

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_task_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_evidence");
    assert!(v["error"].as_str().unwrap().contains("md_bars_write"));
}

// ---------------------------------------------------------------------------
// KTS-10: env_vars_embedded non-empty → unsafe_evidence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kts_10_env_vars_embedded_nonempty_returns_unsafe_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let evidence = valid_check_only_evidence().replacen(
        "\"env_vars_embedded\": [],",
        "\"env_vars_embedded\": [\"MQK_DATABASE_URL\"],",
        1,
    );
    write_evidence(&dir, &evidence);

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_task_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_evidence");
    assert!(v["error"].as_str().unwrap().contains("env_vars_embedded"));
}

// ---------------------------------------------------------------------------
// KTS-11: check_only evidence claiming scheduled_task_mutation=true → unsafe_evidence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kts_11_check_only_claiming_mutation_returns_unsafe_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let evidence = valid_check_only_evidence().replacen(
        "\"scheduled_task_mutation\": false,",
        "\"scheduled_task_mutation\": true,",
        1,
    );
    write_evidence(&dir, &evidence);

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_task_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_evidence");
    assert!(v["error"]
        .as_str()
        .unwrap()
        .contains("scheduled_task_mutation"));
}

// ---------------------------------------------------------------------------
// KTS-12: task_action missing Run-KrakenOhlcSync.ps1 reference → unsafe_evidence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kts_12_task_action_missing_runner_reference_returns_unsafe_evidence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let evidence = valid_check_only_evidence().replacen(
        "-Command \\\"& Run-KrakenOhlcSync.ps1 -CheckOnly:$false\\\"",
        "-Command \\\"& SomeOtherScript.ps1\\\"",
        1,
    );
    write_evidence(&dir, &evidence);

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_task_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_evidence");
    assert!(v["error"]
        .as_str()
        .unwrap()
        .contains("Run-KrakenOhlcSync.ps1"));
}

// ---------------------------------------------------------------------------
// KTS-13: Route returns active without any DB pool configured.
//
// AppState constructed here has no DB pool (st.db is None by construction in
// new_with_operator_auth's test path), and the handler's source contains no
// mqk_db call of any kind -- if it ever opened a connection, this test's
// router construction with no pool would surface as a panic/error rather
// than the expected 200 OK truth_state response already proven above.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kts_13_route_returns_active_without_any_db_pool_configured() {
    let dir = tempfile::tempdir().expect("create temp dir");
    write_evidence(&dir, &valid_check_only_evidence());

    let router = make_router_with_evidence_dir(dir.path().to_str().unwrap());
    let (status, v) = call(router, get_task_status()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
}
