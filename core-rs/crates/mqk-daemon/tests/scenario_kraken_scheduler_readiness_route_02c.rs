//! CRYPTO-DATA-02C-KRAKEN-SCHEDULER-READINESS-STATUS-SURFACE-01:
//! GET /api/v1/market-data/kraken-scheduler/readiness.
//!
//! Proves the read-only daemon re-exposure of `CRYPTO-DATA-02B`'s
//! `mqk-cli md kraken-scheduler-readiness` classification:
//! - Returns truth_state="active" /
//!   scheduler_readiness_state="scheduler_ready_manual_registration_blocked"
//!   against the real, committed policy/registry/providers files -- and
//!   `scheduler_registration_status="not_registered"` (never implying a
//!   scheduler is actually registered).
//! - Returns truth_state="policy_missing" when the policy file is absent.
//! - Returns truth_state="policy_invalid" when the policy violates its own
//!   contract (too-fast cadence).
//! - Returns truth_state="provider_unsafe" when kraken.enabled=true.
//! - Returns truth_state="trading_flags_unsafe" when a symbol carries
//!   paper_trading_enabled=true or live_trading_enabled=true.
//! - Returns truth_state="registry_unsafe" when a symbol is missing its
//!   Kraken alias.
//! - Never opens a DB connection (no DB pool configured anywhere in this file).
//!
//! # Proof matrix
//!
//! | Test    | What it proves                                                       |
//! |---------|-----------------------------------------------------------------------|
//! | KSR-01  | Real fixtures -> active, scheduler_ready_manual_registration_blocked |
//! | KSR-02  | Missing policy file -> truth_state=policy_missing                    |
//! | KSR-03  | Too-fast cadence policy -> truth_state=policy_invalid                 |
//! | KSR-04  | kraken.enabled=true -> truth_state=provider_unsafe                    |
//! | KSR-05  | paper_trading_enabled=true -> truth_state=trading_flags_unsafe        |
//! | KSR-06  | live_trading_enabled=true -> truth_state=trading_flags_unsafe         |
//! | KSR-07  | missing BTC/USD alias -> truth_state=registry_unsafe                  |
//! | KSR-08  | safety flags always true; route never opens a DB connection          |
//!
//! All tests use synthetic temp-dir fixture files except KSR-01/KSR-08,
//! which read the real committed config/policy files. No network call. No
//! DB pool configured anywhere in this file. No CLI subprocess. No config
//! or policy file mutated. No scheduler registered.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

const ROUTE: &str = "/api/v1/market-data/kraken-scheduler/readiness";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn real_policy_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest_dir)
        .join("../../../docs/specs/crypto_data_02a_kraken_scheduler_rate_limit_decision.json")
        .canonicalize()
        .expect("committed CRYPTO-DATA-02A policy fixture must resolve from CARGO_MANIFEST_DIR")
        .to_string_lossy()
        .to_string()
}

fn real_registry_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/instruments_v2.crypto_local_marks.example.json")
        .canonicalize()
        .expect("committed crypto local marks fixture must resolve from CARGO_MANIFEST_DIR")
        .to_string_lossy()
        .to_string()
}

fn real_providers_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest_dir)
        .join("../../../config/providers/providers.json")
        .canonicalize()
        .expect("committed providers.json must resolve from CARGO_MANIFEST_DIR")
        .to_string_lossy()
        .to_string()
}

fn make_router(
    policy_path: String,
    registry_path: Option<String>,
    providers_path: Option<String>,
) -> axum::Router {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.kraken_scheduler_policy_path = policy_path;
    st.instrument_registry_v2_path = registry_path;
    if let Some(p) = providers_path {
        st.provider_registry_path = p;
    }
    routes::build_router(Arc::new(st))
}

fn get_readiness() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri(ROUTE)
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

fn write_temp_json(tag: &str, content: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_test_kraken_scheduler_readiness_route_{}_{}.json",
        tag,
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp fixture");
    path
}

fn providers_json(kraken_enabled: bool) -> String {
    format!(
        r#"[
  {{
    "provider_id": "kraken",
    "display_name": "Kraken (public OHLC)",
    "asset_classes": ["crypto"],
    "free_tier_available": true,
    "api_key_required": false,
    "credential_env_vars": [],
    "rate_limit_notes": "test fixture",
    "supported_timeframes": ["1D"],
    "historical_depth_notes": "test fixture",
    "realtime_support_notes": "test fixture",
    "licensing_notes": "test fixture",
    "implementation_status": "ohlcv_adapter_fixture_proven_network_opt_in_only",
    "enabled": {kraken_enabled},
    "verification_status": "fixture_parser_proven_live_network_not_exercised_by_this_patch",
    "docs_url": "https://docs.kraken.com/api/docs/rest-api/get-ohlc-data"
  }}
]"#
    )
}

fn registry_json_both_symbols(
    btc_alias: bool,
    paper_trading_enabled: bool,
    live_trading_enabled: bool,
) -> String {
    let btc_symbols = if btc_alias {
        r#"{"kraken_pair": "XBTUSD", "kraken_result_key": "XXBTZUSD"}"#
    } else {
        "{}"
    };
    format!(
        r#"{{
  "schema_version": 1,
  "instruments": [
    {{
      "instrument_id": "crypto:GLOBAL:BTCUSD",
      "symbol": "BTC/USD",
      "asset_class": "crypto",
      "currency": "USD",
      "quote_currency": "USD",
      "provider_symbols": {btc_symbols},
      "enabled": false,
      "paper_trading_enabled": {paper_trading_enabled},
      "live_trading_enabled": {live_trading_enabled},
      "timeframes": ["1D"],
      "contract": {{"kind": "crypto_pair", "base": "BTC", "quote": "USD"}}
    }},
    {{
      "instrument_id": "crypto:GLOBAL:ETHUSD",
      "symbol": "ETH/USD",
      "asset_class": "crypto",
      "currency": "USD",
      "quote_currency": "USD",
      "provider_symbols": {{"kraken_pair": "ETHUSD", "kraken_result_key": "XETHZUSD"}},
      "enabled": false,
      "paper_trading_enabled": {paper_trading_enabled},
      "live_trading_enabled": {live_trading_enabled},
      "timeframes": ["1D"],
      "contract": {{"kind": "crypto_pair", "base": "ETH", "quote": "USD"}}
    }}
  ]
}}"#
    )
}

fn policy_json(
    min_seconds_between_pair_calls: i64,
    min_seconds_between_scheduled_runs: i64,
    scheduler_registration_status: &str,
) -> String {
    format!(
        r#"{{
  "schema_version": "crypto-data-02a-kraken-scheduler-rate-limit-decision-v1",
  "patch_id": "CRYPTO-DATA-02A-KRAKEN-SCHEDULER-RATE-LIMIT-DECISION-01",
  "provider": "kraken",
  "symbols": ["BTC/USD", "ETH/USD"],
  "recommended_default_cadence": "daily",
  "min_seconds_between_pair_calls": {min_seconds_between_pair_calls},
  "min_seconds_between_scheduled_runs": {min_seconds_between_scheduled_runs},
  "max_ohlc_calls_per_run": 2,
  "max_total_network_calls_per_run": 2,
  "concurrency": "sequential_only",
  "scheduler_registration_status": "{scheduler_registration_status}",
  "safety": {{
    "no_scheduled_task_registered": true,
    "no_daemon_job_added": true,
    "no_network_call_to_kraken_api": true,
    "no_db_write": true,
    "no_trading_enabled": true,
    "kraken_provider_enabled": false,
    "paper_trading_enabled": false,
    "live_trading_enabled": false
  }}
}}"#
    )
}

// KSR-01: real, committed policy/registry/providers files -> truth_state=active,
// scheduler_readiness_state=scheduler_ready_manual_registration_blocked,
// scheduler_registration_status=not_registered.
#[tokio::test]
async fn ksr_01_real_fixtures_are_active_but_not_registered() {
    let router = make_router(
        real_policy_path(),
        Some(real_registry_path()),
        Some(real_providers_path()),
    );
    let (status, v) = call(router, get_readiness()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
    assert_eq!(
        str_field(&v, "scheduler_readiness_state"),
        "scheduler_ready_manual_registration_blocked"
    );
    assert_eq!(
        str_field(&v, "scheduler_registration_status"),
        "not_registered"
    );
    assert_eq!(str_field(&v, "daemon_job_status"), "absent");
    assert!(!bool_field(&v, "network_call_made"));
    assert!(!bool_field(&v, "db_write"));
    assert!(!bool_field(&v, "trading_enabled"));
    assert!(bool_field(&v, "all_passed"));

    let safety = v.get("safety").expect("safety object must be present");
    assert!(bool_field(safety, "no_scheduled_task_registered"));
    assert!(bool_field(safety, "no_daemon_job_added"));
    assert!(bool_field(safety, "no_network_call_made"));
    assert!(bool_field(safety, "no_db_connection"));
    assert!(bool_field(safety, "no_trading_enabled"));
    assert!(bool_field(safety, "no_config_file_mutated"));

    let symbols = v
        .get("symbols")
        .and_then(|s| s.as_array())
        .expect("symbols must be an array");
    assert_eq!(symbols.len(), 2);
}

// KSR-02: missing policy file -> truth_state=policy_missing.
#[tokio::test]
async fn ksr_02_missing_policy_file_fails_closed() {
    let missing_policy = std::env::temp_dir().join(format!(
        "mqk_test_kraken_scheduler_readiness_route_missing_policy_{}.json",
        std::process::id()
    ));

    let router = make_router(
        missing_policy.to_string_lossy().to_string(),
        Some(real_registry_path()),
        Some(real_providers_path()),
    );
    let (status, v) = call(router, get_readiness()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "policy_missing");
    assert!(!bool_field(&v, "all_passed"));
    assert_eq!(str_field(&v, "scheduler_readiness_state"), "policy_blocked");
}

// KSR-03: policy with too-fast cadence -> truth_state=policy_invalid.
#[tokio::test]
async fn ksr_03_policy_too_fast_cadence_is_invalid() {
    let policy_path = write_temp_json("ksr03_policy", &policy_json(1, 86_400, "not_registered"));

    let router = make_router(
        policy_path.to_string_lossy().to_string(),
        Some(real_registry_path()),
        Some(real_providers_path()),
    );
    let (status, v) = call(router, get_readiness()).await;

    let _ = std::fs::remove_file(&policy_path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "policy_invalid");
    assert_eq!(str_field(&v, "rate_limit_policy_state"), "invalid");
    assert!(!bool_field(&v, "all_passed"));
}

// KSR-04: kraken.enabled=true -> truth_state=provider_unsafe.
#[tokio::test]
async fn ksr_04_kraken_provider_enabled_true_is_unsafe() {
    let policy_path = write_temp_json("ksr04_policy", &policy_json(2, 86_400, "not_registered"));
    let registry_path = write_temp_json(
        "ksr04_registry",
        &registry_json_both_symbols(true, false, false),
    );
    let providers_path = write_temp_json("ksr04_providers", &providers_json(true));

    let router = make_router(
        policy_path.to_string_lossy().to_string(),
        Some(registry_path.to_string_lossy().to_string()),
        Some(providers_path.to_string_lossy().to_string()),
    );
    let (status, v) = call(router, get_readiness()).await;

    let _ = std::fs::remove_file(&policy_path);
    let _ = std::fs::remove_file(&registry_path);
    let _ = std::fs::remove_file(&providers_path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "provider_unsafe");
    assert!(bool_field(&v, "provider_enabled"));
    assert!(!bool_field(&v, "all_passed"));
}

// KSR-05: paper_trading_enabled=true -> truth_state=trading_flags_unsafe.
#[tokio::test]
async fn ksr_05_paper_trading_enabled_true_is_unsafe() {
    let policy_path = write_temp_json("ksr05_policy", &policy_json(2, 86_400, "not_registered"));
    let registry_path = write_temp_json(
        "ksr05_registry",
        &registry_json_both_symbols(true, true, false),
    );
    let providers_path = write_temp_json("ksr05_providers", &providers_json(false));

    let router = make_router(
        policy_path.to_string_lossy().to_string(),
        Some(registry_path.to_string_lossy().to_string()),
        Some(providers_path.to_string_lossy().to_string()),
    );
    let (status, v) = call(router, get_readiness()).await;

    let _ = std::fs::remove_file(&policy_path);
    let _ = std::fs::remove_file(&registry_path);
    let _ = std::fs::remove_file(&providers_path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "trading_flags_unsafe");
    assert!(!bool_field(&v, "all_passed"));
}

// KSR-06: live_trading_enabled=true -> truth_state=trading_flags_unsafe.
#[tokio::test]
async fn ksr_06_live_trading_enabled_true_is_unsafe() {
    let policy_path = write_temp_json("ksr06_policy", &policy_json(2, 86_400, "not_registered"));
    let registry_path = write_temp_json(
        "ksr06_registry",
        &registry_json_both_symbols(true, false, true),
    );
    let providers_path = write_temp_json("ksr06_providers", &providers_json(false));

    let router = make_router(
        policy_path.to_string_lossy().to_string(),
        Some(registry_path.to_string_lossy().to_string()),
        Some(providers_path.to_string_lossy().to_string()),
    );
    let (status, v) = call(router, get_readiness()).await;

    let _ = std::fs::remove_file(&policy_path);
    let _ = std::fs::remove_file(&registry_path);
    let _ = std::fs::remove_file(&providers_path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "trading_flags_unsafe");
    assert!(!bool_field(&v, "all_passed"));
}

// KSR-07: missing BTC/USD Kraken alias -> truth_state=registry_unsafe.
#[tokio::test]
async fn ksr_07_missing_btc_alias_is_registry_unsafe() {
    let policy_path = write_temp_json("ksr07_policy", &policy_json(2, 86_400, "not_registered"));
    let registry_path = write_temp_json(
        "ksr07_registry",
        &registry_json_both_symbols(false, false, false),
    );
    let providers_path = write_temp_json("ksr07_providers", &providers_json(false));

    let router = make_router(
        policy_path.to_string_lossy().to_string(),
        Some(registry_path.to_string_lossy().to_string()),
        Some(providers_path.to_string_lossy().to_string()),
    );
    let (status, v) = call(router, get_readiness()).await;

    let _ = std::fs::remove_file(&policy_path);
    let _ = std::fs::remove_file(&registry_path);
    let _ = std::fs::remove_file(&providers_path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "registry_unsafe");
    assert!(!bool_field(&v, "all_passed"));

    let symbols = v.get("symbols").and_then(|s| s.as_array()).unwrap();
    let btc = symbols
        .iter()
        .find(|s| str_field(s, "symbol") == "BTC/USD")
        .expect("BTC/USD entry must be present in symbols array");
    assert!(!bool_field(btc, "alias_ok"));
}

// KSR-08: no DB pool is configured anywhere in this file. If the route under
// test ever attempted to open a DB connection, every test above would have
// failed with a connection/panic error rather than a clean 200 JSON
// response. This test re-asserts explicitly against the real fixtures.
#[tokio::test]
async fn ksr_08_route_never_opens_a_db_connection() {
    let router = make_router(
        real_policy_path(),
        Some(real_registry_path()),
        Some(real_providers_path()),
    );
    let (status, v) = call(router, get_readiness()).await;

    assert_eq!(status, StatusCode::OK);
    let safety = v.get("safety").expect("safety object must be present");
    assert!(bool_field(safety, "no_db_connection"));
}
