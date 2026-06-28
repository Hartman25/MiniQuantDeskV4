//! ASSET-CORE-05B — read-only per-instrument session-profile diagnostics.
//!
//! Proves that `GET /api/v1/system/instrument-sessions/status` loads the
//! configured v1 registry, converts it to v2 in memory, validates it, maps
//! each instrument to the existing ASSET-CORE-05A/05B session-profile seam,
//! and reports deterministic session state at an injected timestamp without
//! any DB pool, provider/broker call, daemon runtime start, order submission,
//! or trading cutover.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use mqk_md::instrument_registry_v2::{
    validate_registry_v2, ContractDefinitionV2, InstrumentDefinitionV2, InstrumentMetadataV2,
    InstrumentRegistryV2, SCHEMA_VERSION_V2,
};
use tower::ServiceExt;

const ROUTE: &str = "/api/v1/system/instrument-sessions/status?now_utc=2026-06-25T15:00:00Z";

fn real_registry_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/equities.json")
        .canonicalize()
        .expect("registry path must resolve from CARGO_MANIFEST_DIR")
        .to_string_lossy()
        .to_string()
}

fn make_router_with_real_registry() -> axum::Router {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = real_registry_path();
    routes::build_router(Arc::new(st))
}

fn make_router_with_missing_registry() -> axum::Router {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = std::env::temp_dir()
        .join(format!(
            "mqk_missing_instrument_sessions_{}.json",
            std::process::id()
        ))
        .to_string_lossy()
        .to_string();
    routes::build_router(Arc::new(st))
}

fn make_router_with_registry_content(
    content: &str,
    tag: &str,
) -> (axum::Router, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "mqk_test_instrument_sessions_{}_{}.json",
        tag,
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp registry fixture");

    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = path.to_string_lossy().to_string();
    (routes::build_router(Arc::new(st)), path)
}

async fn call_json(router: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json = serde_json::from_slice(&body).expect("body is valid JSON");
    (status, json)
}

fn base_crypto(symbol: &str) -> InstrumentDefinitionV2 {
    InstrumentDefinitionV2 {
        instrument_id: format!("crypto:GLOBAL:{symbol}"),
        symbol: symbol.to_string(),
        asset_class: "crypto".to_string(),
        instrument_kind: None,
        venue: None,
        currency: "USD".to_string(),
        quote_currency: Some("USD".to_string()),
        provider_symbols: BTreeMap::new(),
        enabled: false,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        timeframes: vec!["1D".to_string()],
        contract: Some(ContractDefinitionV2::CryptoPair {
            base: "BTC".to_string(),
            quote: "USD".to_string(),
        }),
        metadata: InstrumentMetadataV2::default(),
        notes: Some("fixture only; not enabled".to_string()),
        allow_enabled_non_equity_for_testing: false,
        economics: None,
    }
}

fn base_future(symbol: &str) -> InstrumentDefinitionV2 {
    InstrumentDefinitionV2 {
        instrument_id: format!("future:CME:{symbol}"),
        symbol: symbol.to_string(),
        asset_class: "future".to_string(),
        instrument_kind: None,
        venue: Some("CME".to_string()),
        currency: "USD".to_string(),
        quote_currency: None,
        provider_symbols: BTreeMap::new(),
        enabled: false,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        timeframes: vec!["1D".to_string()],
        contract: Some(ContractDefinitionV2::Future {
            root: "ES".to_string(),
            expiry: "2026-09".to_string(),
            multiplier: 50,
            tick_size_micros: 250_000,
        }),
        metadata: InstrumentMetadataV2::default(),
        notes: Some("fixture only; not enabled".to_string()),
        allow_enabled_non_equity_for_testing: false,
        economics: None,
    }
}

fn base_forex(symbol: &str) -> InstrumentDefinitionV2 {
    InstrumentDefinitionV2 {
        instrument_id: format!("forex:GLOBAL:{symbol}"),
        symbol: symbol.to_string(),
        asset_class: "forex".to_string(),
        instrument_kind: None,
        venue: None,
        currency: "USD".to_string(),
        quote_currency: Some("USD".to_string()),
        provider_symbols: BTreeMap::new(),
        enabled: false,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        timeframes: vec!["1D".to_string()],
        contract: Some(ContractDefinitionV2::ForexPair {
            base: "EUR".to_string(),
            quote: "USD".to_string(),
        }),
        metadata: InstrumentMetadataV2::default(),
        notes: Some("fixture only; not enabled".to_string()),
        allow_enabled_non_equity_for_testing: false,
        economics: None,
    }
}

fn registry_of(instruments: Vec<InstrumentDefinitionV2>) -> InstrumentRegistryV2 {
    InstrumentRegistryV2 {
        schema_version: SCHEMA_VERSION_V2,
        instruments,
    }
}

fn assert_fixture_maps_model_only(
    inst: InstrumentDefinitionV2,
    expected_profile: state::MarketSessionProfile,
) {
    let registry = registry_of(vec![inst.clone()]);
    validate_registry_v2(&registry).expect("fixture v2 registry must validate");

    let resolution = state::resolve_session_profile_for_instrument_metadata(
        &inst.asset_class,
        inst.instrument_kind.as_deref(),
    );
    assert_eq!(
        resolution.truth_state,
        state::SessionProfileResolutionTruth::UnsupportedAssetClass
    );
    assert_eq!(resolution.profile, Some(expected_profile));
    assert_eq!(expected_profile.implementation_status(), "model_only");
}

#[tokio::test]
async fn ac05b_01_real_registry_all_symbols_map_to_equity_profile() {
    let (status, body) = call_json(make_router_with_real_registry(), ROUTE).await;

    assert_eq!(status, StatusCode::OK, "route must return 200: {body}");
    assert_eq!(body["truth_state"], "active");
    assert_eq!(body["registry_v2_valid"], true);
    assert_eq!(body["v1_count"].as_u64(), Some(88));
    assert_eq!(body["v2_count"].as_u64(), Some(88));
    assert_eq!(body["instrument_count"].as_u64(), Some(88));

    let rows = body["instruments"].as_array().expect("instruments array");
    assert_eq!(rows.len(), 88);
    assert!(rows.iter().all(|row| {
        row["asset_class"] == "equity" && row["session_profile_id"] == "equity_us_regular"
    }));

    let profiles = body["profiles"].as_array().expect("profiles array");
    assert_eq!(profiles.len(), 1, "real registry has one profile: {body}");
    assert_eq!(profiles[0]["profile_id"], "equity_us_regular");
    assert_eq!(profiles[0]["instrument_count"].as_u64(), Some(88));
}

#[tokio::test]
async fn ac05b_02_real_registry_reports_zero_non_equity_enabled_rows() {
    let (_, body) = call_json(make_router_with_real_registry(), ROUTE).await;

    assert_eq!(body["non_equity_enabled"], false);
    let rows = body["instruments"].as_array().expect("instruments array");
    assert!(rows.iter().all(|row| row["asset_class"] == "equity"));
}

#[tokio::test]
async fn ac05b_03_equity_rows_are_production_backed_but_not_trading_inputs() {
    let (_, body) = call_json(make_router_with_real_registry(), ROUTE).await;

    assert_eq!(body["production_cutover_enabled"], false);
    assert_eq!(body["trading_uses_session_v2"], false);
    assert_eq!(body["runtime_uses_session_v2"], false);

    let rows = body["instruments"].as_array().expect("instruments array");
    assert!(rows.iter().all(|row| row["production_backed"] == true));
    assert!(rows.iter().all(|row| row["model_only"] == false));
    assert!(rows.iter().all(|row| row["trading_uses_this"] == false));
}

#[tokio::test]
async fn ac05b_04_fixed_now_returns_deterministic_equity_session_state() {
    let (_, body) = call_json(make_router_with_real_registry(), ROUTE).await;

    let rows = body["instruments"].as_array().expect("instruments array");
    assert!(rows.iter().all(|row| {
        row["session_state"] == "regular_open"
            && row["reason_code"] == "classified_by_market_calendar_provider"
    }));
}

#[tokio::test]
async fn ac05b_05_missing_registry_reports_registry_missing_no_panic() {
    let (status, body) = call_json(make_router_with_missing_registry(), ROUTE).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "registry_missing");
    assert_eq!(body["registry_v2_valid"], false);
    assert!(body["errors"].as_array().is_some_and(|v| !v.is_empty()));
    assert_eq!(body["production_cutover_enabled"], false);
    assert_eq!(body["trading_uses_session_v2"], false);
}

#[tokio::test]
async fn ac05b_06_malformed_registry_reports_registry_invalid_no_panic() {
    let (router, path) = make_router_with_registry_content("{not valid json", "malformed");
    let (status, body) = call_json(router, ROUTE).await;
    std::fs::remove_file(&path).ok();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "registry_invalid");
    assert_eq!(body["registry_v2_valid"], false);
    assert!(body["errors"].as_array().is_some_and(|v| !v.is_empty()));
}

#[test]
fn ac05b_07_crypto_fixture_maps_to_24x7_model_only_profile() {
    assert_fixture_maps_model_only(
        base_crypto("BTCUSD_TEST"),
        state::MarketSessionProfile::CryptoContinuous,
    );
    let kind = state::classify_crypto_continuous_session(
        chrono::DateTime::parse_from_rfc3339("2026-06-25T15:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    assert_eq!(kind.as_str(), "continuous");
    assert!(kind.is_open());
}

#[test]
fn ac05b_08_futures_fixture_maps_to_model_only_futures_profile() {
    assert_fixture_maps_model_only(
        base_future("ES_TEST"),
        state::MarketSessionProfile::FuturesGlobex,
    );
}

#[test]
fn ac05b_09_forex_fixture_maps_to_model_only_forex_profile() {
    assert_fixture_maps_model_only(
        base_forex("EURUSD_TEST"),
        state::MarketSessionProfile::ForexWeekdayContinuous,
    );
    let kind = state::classify_forex_weekday_continuous_session(
        chrono::DateTime::parse_from_rfc3339("2026-06-25T15:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    assert_eq!(kind.as_str(), "continuous");
    assert!(kind.is_open());
}

#[test]
fn ac05b_10_unknown_fail_closed_behavior_remains_unchanged() {
    let unknown = state::MarketSessionTruth::unknown();
    assert_eq!(unknown.state, state::MarketSessionState::Unknown);
    assert!(!unknown.is_in_session());
    assert!(!state::MarketVenueSessionKind::Unknown.is_open());
}

#[tokio::test]
async fn ac05b_11_route_is_mounted_public_and_timestamp_validation_is_fail_closed() {
    let (status, body) = call_json(
        make_router_with_real_registry(),
        "/api/v1/system/instrument-sessions/status?now_utc=not-a-timestamp",
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "timestamp_invalid");
    assert_eq!(body["production_cutover_enabled"], false);
    assert_eq!(body["trading_uses_session_v2"], false);
    assert_eq!(body["runtime_uses_session_v2"], false);
}
