//! CRYPTO-REGISTRY-04-KRAKEN-DATA-ONLY-REGISTRY-STATUS-SURFACE-01:
//! GET /api/v1/market-data/crypto-registry/readiness.
//!
//! Proves the read-only daemon re-exposure of `CRYPTO-REGISTRY-03`'s
//! `mqk-cli md crypto-registry-readiness` classification:
//! - Returns truth_state="active"/data_readiness_state="data_ready_manual_only"
//!   against the real, committed fixture files (default disabled state is
//!   expected, not a failure).
//! - Returns truth_state="missing_provider" when the provider is absent
//!   from the providers registry.
//! - Returns truth_state="missing_alias" when a symbol is missing its
//!   kraken_pair/kraken_result_key alias.
//! - Returns truth_state="missing_symbol" when a requested symbol is absent
//!   from the registry entirely.
//! - Returns truth_state="unsafe_trading_enabled" when
//!   paper_trading_enabled=true or live_trading_enabled=true.
//! - Returns truth_state="unsafe_provider_enabled" when kraken.enabled=true.
//! - Never opens a DB connection (no DB pool configured anywhere in this file).
//!
//! # Proof matrix
//!
//! | Test    | What it proves                                                       |
//! |---------|-----------------------------------------------------------------------|
//! | CRR-01  | Real fixtures -> truth_state=active, data_ready_manual_only          |
//! | CRR-02  | Missing provider entry -> truth_state=missing_provider               |
//! | CRR-03  | Missing kraken alias -> truth_state=missing_alias                    |
//! | CRR-04  | Missing symbol entirely -> truth_state=missing_symbol                |
//! | CRR-05  | paper_trading_enabled=true -> truth_state=unsafe_trading_enabled     |
//! | CRR-06  | live_trading_enabled=true -> truth_state=unsafe_trading_enabled      |
//! | CRR-07  | kraken.enabled=true -> truth_state=unsafe_provider_enabled           |
//! | CRR-08  | safety flags always true; route never opens a DB connection         |
//!
//! All tests use synthetic temp-dir fixture files except CRR-01/CRR-08,
//! which read the real committed config files. No network call. No DB pool
//! configured anywhere in this file. No CLI subprocess. No config file
//! mutated.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

const ROUTE: &str = "/api/v1/market-data/crypto-registry/readiness";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

fn make_router(registry_path: Option<String>, providers_path: Option<String>) -> axum::Router {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
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
        "mqk_test_crypto_registry_readiness_route_{}_{}.json",
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

fn registry_json_single_btc(
    paper_trading_enabled: bool,
    live_trading_enabled: bool,
    include_kraken_alias: bool,
) -> String {
    let provider_symbols = if include_kraken_alias {
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
      "provider_symbols": {provider_symbols},
      "enabled": false,
      "paper_trading_enabled": {paper_trading_enabled},
      "live_trading_enabled": {live_trading_enabled},
      "timeframes": ["1D"],
      "contract": {{"kind": "crypto_pair", "base": "BTC", "quote": "USD"}}
    }}
  ]
}}"#
    )
}

fn registry_json_both_symbols(
    paper_trading_enabled: bool,
    live_trading_enabled: bool,
) -> String {
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
      "provider_symbols": {{"kraken_pair": "XBTUSD", "kraken_result_key": "XXBTZUSD"}},
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

// CRR-01: real, committed fixtures -> truth_state=active,
// data_readiness_state=data_ready_manual_only, trading/scheduler disabled/absent.
#[tokio::test]
async fn crr_01_real_fixtures_are_active_and_data_ready_manual_only() {
    let router = make_router(Some(real_registry_path()), Some(real_providers_path()));
    let (status, v) = call(router, get_readiness()).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "active");
    assert_eq!(str_field(&v, "data_readiness_state"), "data_ready_manual_only");
    assert_eq!(str_field(&v, "trading_readiness_state"), "disabled");
    assert_eq!(str_field(&v, "scheduler_readiness_state"), "absent");
    assert_eq!(str_field(&v, "provider"), "kraken");
    assert!(!bool_field(&v, "provider_enabled"));
    assert!(!bool_field(&v, "api_key_required"));
    assert!(bool_field(&v, "all_passed"));

    let safety = v.get("safety").expect("safety object must be present");
    assert!(bool_field(safety, "no_scheduler"));
    assert!(bool_field(safety, "no_db_connection"));
    assert!(bool_field(safety, "no_network_call"));
    assert!(bool_field(safety, "no_trading_enabled"));
    assert!(bool_field(safety, "no_config_file_mutated"));

    let symbols = v
        .get("symbols")
        .and_then(|s| s.as_array())
        .expect("symbols must be an array");
    assert_eq!(symbols.len(), 2);
    for sym in symbols {
        assert!(bool_field(sym, "found"));
        assert!(bool_field(sym, "asset_class_ok"));
        assert!(bool_field(sym, "alias_ok"));
        assert!(bool_field(sym, "trading_flags_safe"));
        assert!(bool_field(sym, "passed"));
        assert_eq!(sym.get("enabled").and_then(|e| e.as_bool()), Some(false));
    }
}

// CRR-02: missing provider entry -> truth_state=missing_provider.
#[tokio::test]
async fn crr_02_missing_provider_fails_closed() {
    let registry_path =
        write_temp_json("crr02_registry", &registry_json_single_btc(false, false, true));
    let providers_path = write_temp_json("crr02_providers", "[]");

    let router = make_router(
        Some(registry_path.to_string_lossy().to_string()),
        Some(providers_path.to_string_lossy().to_string()),
    );
    let (status, v) = call(router, get_readiness()).await;

    let _ = std::fs::remove_file(&registry_path);
    let _ = std::fs::remove_file(&providers_path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "missing_provider");
    assert!(!bool_field(&v, "all_passed"));
    assert_eq!(str_field(&v, "data_readiness_state"), "blocked");
}

// CRR-03: missing kraken alias on BTC/USD -> truth_state=missing_alias.
#[tokio::test]
async fn crr_03_missing_kraken_alias_fails_closed() {
    let registry_path = write_temp_json(
        "crr03_registry",
        &registry_json_single_btc(false, false, false),
    );
    let providers_path = write_temp_json("crr03_providers", &providers_json(false));

    let router = make_router(
        Some(registry_path.to_string_lossy().to_string()),
        Some(providers_path.to_string_lossy().to_string()),
    );
    let (status, v) = call(router, get_readiness()).await;

    let _ = std::fs::remove_file(&registry_path);
    let _ = std::fs::remove_file(&providers_path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "missing_alias");
    assert!(!bool_field(&v, "all_passed"));
}

// CRR-04: ETH/USD entirely absent from the registry -> truth_state=missing_symbol.
#[tokio::test]
async fn crr_04_missing_symbol_fails_closed() {
    let registry_path = write_temp_json(
        "crr04_registry",
        &registry_json_single_btc(false, false, true),
    );
    let providers_path = write_temp_json("crr04_providers", &providers_json(false));

    let router = make_router(
        Some(registry_path.to_string_lossy().to_string()),
        Some(providers_path.to_string_lossy().to_string()),
    );
    let (status, v) = call(router, get_readiness()).await;

    let _ = std::fs::remove_file(&registry_path);
    let _ = std::fs::remove_file(&providers_path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "missing_symbol");
    assert!(!bool_field(&v, "all_passed"));

    let symbols = v.get("symbols").and_then(|s| s.as_array()).unwrap();
    let eth = symbols
        .iter()
        .find(|s| str_field(s, "symbol") == "ETH/USD")
        .expect("ETH/USD entry must be present in symbols array");
    assert!(!bool_field(eth, "found"));
}

// CRR-05: paper_trading_enabled=true -> truth_state=unsafe_trading_enabled.
#[tokio::test]
async fn crr_05_paper_trading_enabled_true_is_unsafe() {
    let registry_path =
        write_temp_json("crr05_registry", &registry_json_both_symbols(true, false));
    let providers_path = write_temp_json("crr05_providers", &providers_json(false));

    let router = make_router(
        Some(registry_path.to_string_lossy().to_string()),
        Some(providers_path.to_string_lossy().to_string()),
    );
    let (status, v) = call(router, get_readiness()).await;

    let _ = std::fs::remove_file(&registry_path);
    let _ = std::fs::remove_file(&providers_path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_trading_enabled");
    assert!(!bool_field(&v, "all_passed"));
}

// CRR-06: live_trading_enabled=true -> truth_state=unsafe_trading_enabled.
#[tokio::test]
async fn crr_06_live_trading_enabled_true_is_unsafe() {
    let registry_path =
        write_temp_json("crr06_registry", &registry_json_both_symbols(false, true));
    let providers_path = write_temp_json("crr06_providers", &providers_json(false));

    let router = make_router(
        Some(registry_path.to_string_lossy().to_string()),
        Some(providers_path.to_string_lossy().to_string()),
    );
    let (status, v) = call(router, get_readiness()).await;

    let _ = std::fs::remove_file(&registry_path);
    let _ = std::fs::remove_file(&providers_path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_trading_enabled");
    assert!(!bool_field(&v, "all_passed"));
}

// CRR-07: kraken.enabled=true -> truth_state=unsafe_provider_enabled, per
// CRYPTO-REGISTRY-02's decision not to treat provider-enablement as safe yet.
#[tokio::test]
async fn crr_07_kraken_provider_enabled_true_is_unsafe() {
    let registry_path =
        write_temp_json("crr07_registry", &registry_json_both_symbols(false, false));
    let providers_path = write_temp_json("crr07_providers", &providers_json(true));

    let router = make_router(
        Some(registry_path.to_string_lossy().to_string()),
        Some(providers_path.to_string_lossy().to_string()),
    );
    let (status, v) = call(router, get_readiness()).await;

    let _ = std::fs::remove_file(&registry_path);
    let _ = std::fs::remove_file(&providers_path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(str_field(&v, "truth_state"), "unsafe_provider_enabled");
    assert!(bool_field(&v, "provider_enabled"));
    assert!(!bool_field(&v, "all_passed"));
}

// CRR-08: no DB pool is configured anywhere in this file. If the route under
// test ever attempted to open a DB connection, every test above would have
// failed with a connection/panic error rather than a clean 200 JSON
// response. This test re-asserts explicitly against the real fixtures.
#[tokio::test]
async fn crr_08_route_never_opens_a_db_connection() {
    let router = make_router(Some(real_registry_path()), Some(real_providers_path()));
    let (status, v) = call(router, get_readiness()).await;

    assert_eq!(status, StatusCode::OK);
    let safety = v.get("safety").expect("safety object must be present");
    assert!(bool_field(safety, "no_db_connection"));
}
