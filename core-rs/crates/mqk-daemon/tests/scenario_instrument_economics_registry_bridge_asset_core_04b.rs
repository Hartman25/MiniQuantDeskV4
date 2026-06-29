//! ASSET-CORE-04B — registry-v2 -> `mqk_portfolio::InstrumentEconomics` bridge.
//!
//! Proves `mqk_daemon::state::instrument_economics_bridge` (pure bridge
//! functions) and `GET /api/v1/system/instrument-economics/status` (the
//! read-only diagnostic route): the real production registry bridges
//! cleanly with equity multiplier=1/USD, futures/options/crypto/forex
//! fixtures bridge or refuse truthfully per the model's currency-safety and
//! multiplier rules, and the bridge never enables, implies, or decides
//! trading permission for any instrument (`trading_enabled_by_bridge` is
//! always `false`).
//!
//! Futures/options/crypto/forex cannot be exercised through the route
//! itself: the v1 registry (`config/instruments/equities.json`) has no
//! derivative contract representation, so `convert_v1_registry_to_v2` only
//! ever produces `Equity`/`Etf` contracts (see
//! `mqk_md::instrument_registry_v2::convert_tracked_instrument_to_v2`).
//! Those cases are proven as pure-function tests directly against
//! `state::instrument_v2_to_economics`, mirroring
//! `scenario_instrument_session_status_asset_core_05b.rs`'s precedent for
//! the same limitation.
//!
//! ## CI command
//!
//! ```sh
//! cargo test -p mqk-daemon --test scenario_instrument_economics_registry_bridge_asset_core_04b
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use mqk_md::instrument_registry_v2::{
    ContractDefinitionV2, InstrumentDefinitionV2, InstrumentMetadataV2,
};
use mqk_portfolio::MICROS_SCALE;
use tower::ServiceExt;

const ROUTE: &str = "/api/v1/system/instrument-economics/status";

/// The 14 ETF-tagged symbols in the real committed production registry.
/// Mirrors `V1_ETF_SYMBOLS` in
/// `mqk-md/src/instrument_registry_v2.rs`'s own test module.
const V1_ETF_SYMBOLS: &[&str] = &[
    "SPY", "QQQ", "IWM", "DIA", "XLK", "XLF", "XLE", "XLI", "XLP", "XLU", "TLT", "IEF", "SHY",
    "GLD",
];

// ---------------------------------------------------------------------------
// Router helpers
// ---------------------------------------------------------------------------

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
            "mqk_missing_instrument_economics_{}.json",
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
        "mqk_test_instrument_economics_{}_{}.json",
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

// ---------------------------------------------------------------------------
// Pure-function fixtures (futures/options/crypto/forex; v1 cannot represent
// these, so they are never exercised through the route -- see module docs).
// ---------------------------------------------------------------------------

fn base_equity(symbol: &str) -> InstrumentDefinitionV2 {
    InstrumentDefinitionV2 {
        instrument_id: format!("equity:US:{symbol}"),
        symbol: symbol.to_string(),
        asset_class: "equity".to_string(),
        instrument_kind: None,
        venue: Some("NASDAQ".to_string()),
        currency: "USD".to_string(),
        quote_currency: None,
        provider_symbols: BTreeMap::from([("twelvedata".to_string(), symbol.to_string())]),
        enabled: true,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        timeframes: vec!["1D".to_string()],
        contract: Some(ContractDefinitionV2::Equity),
        metadata: InstrumentMetadataV2::default(),
        notes: None,
        allow_enabled_non_equity_for_testing: false,
        economics: None,
    }
}

fn base_etf(symbol: &str) -> InstrumentDefinitionV2 {
    InstrumentDefinitionV2 {
        instrument_kind: Some("etf".to_string()),
        contract: Some(ContractDefinitionV2::Etf),
        metadata: InstrumentMetadataV2 {
            sector: Some("broad_market".to_string()),
            category: Some("index_equity".to_string()),
            tags: Vec::new(),
        },
        venue: Some("NYSEARCA".to_string()),
        ..base_equity(symbol)
    }
}

fn base_future(symbol: &str, multiplier: i64) -> InstrumentDefinitionV2 {
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
            multiplier,
            tick_size_micros: 250_000,
        }),
        metadata: InstrumentMetadataV2::default(),
        notes: Some("fixture only; not enabled".to_string()),
        allow_enabled_non_equity_for_testing: false,
        economics: None,
    }
}

fn base_option(symbol: &str, multiplier: i64) -> InstrumentDefinitionV2 {
    InstrumentDefinitionV2 {
        instrument_id: format!("option:US:{symbol}"),
        symbol: symbol.to_string(),
        asset_class: "option".to_string(),
        instrument_kind: None,
        venue: Some("OPRA".to_string()),
        currency: "USD".to_string(),
        quote_currency: None,
        provider_symbols: BTreeMap::new(),
        enabled: false,
        paper_trading_enabled: false,
        live_trading_enabled: false,
        timeframes: vec!["1D".to_string()],
        contract: Some(ContractDefinitionV2::Option {
            underlying: "AAPL".to_string(),
            expiry: "2026-09-18".to_string(),
            strike_micros: 150_000_000,
            right: "call".to_string(),
            multiplier,
        }),
        metadata: InstrumentMetadataV2::default(),
        notes: Some("fixture only; not enabled".to_string()),
        allow_enabled_non_equity_for_testing: false,
        economics: None,
    }
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

/// Same-currency forex fixture: `currency == quote_currency == "USD"`. The
/// economics model has only one currency field, so this is the case the
/// bridge can safely represent.
fn base_forex_same_currency(symbol: &str) -> InstrumentDefinitionV2 {
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

/// Mismatched-currency forex fixture: settlement `currency` ("EUR") differs
/// from `quote_currency` ("USD") -- the genuinely cross-currency case the
/// model cannot represent with a single currency field.
fn base_forex_mismatched_currency(symbol: &str) -> InstrumentDefinitionV2 {
    let mut inst = base_forex_same_currency(symbol);
    inst.currency = "EUR".to_string();
    inst.quote_currency = Some("USD".to_string());
    inst
}

// ---------------------------------------------------------------------------
// 1-4, 14-16: route tests against the real production registry + failure paths
// ---------------------------------------------------------------------------

// ACB04B-01: the real v1 registry converts to v2, validates, and every real
// row bridges successfully (bridged_count == instrument_count == 88).
#[tokio::test]
async fn ac04b_01_real_registry_converts_validates_and_bridges_every_row() {
    let router = make_router_with_real_registry();
    let (status, body) = call_json(router, ROUTE).await;

    assert_eq!(status, StatusCode::OK, "must return 200: {body}");
    assert_eq!(body["truth_state"], "active", "truth_state: {body}");
    assert_eq!(body["registry_v2_valid"], true);
    assert_eq!(body["instrument_count"].as_u64(), Some(88));
    assert_eq!(body["bridged_count"].as_u64(), Some(88));
    assert_eq!(body["failed_count"].as_u64(), Some(0));
    assert_eq!(
        body["rows"]
            .as_array()
            .expect("rows must be an array")
            .len(),
        88
    );
}

// ACB04B-02: every real registry row is equity, multiplier=1 (MICROS_SCALE),
// USD quote_currency, and never trading_enabled_by_bridge.
#[tokio::test]
async fn ac04b_02_real_registry_rows_all_have_equity_multiplier_one_and_usd() {
    let router = make_router_with_real_registry();
    let (_, body) = call_json(router, ROUTE).await;
    let rows = body["rows"].as_array().expect("rows must be an array");
    assert!(!rows.is_empty());

    for row in rows {
        assert_eq!(row["asset_class"], "equity", "row={row}");
        assert_eq!(row["truth_state"], "active", "row={row}");
        assert_eq!(
            row["contract_multiplier_micros"].as_i64(),
            Some(MICROS_SCALE),
            "row={row}"
        );
        assert_eq!(row["quote_currency"], "USD", "row={row}");
        assert_eq!(row["trading_enabled_by_bridge"], false, "row={row}");
    }
}

// ACB04B-03: all 14 ETF-tagged rows stay asset_class=equity with
// instrument_kind=etf, multiplier=1, and model_only=false (equity-equivalent).
#[tokio::test]
async fn ac04b_03_real_registry_etf_rows_stay_equity_with_etf_kind_and_multiplier_one() {
    let router = make_router_with_real_registry();
    let (_, body) = call_json(router, ROUTE).await;
    let rows = body["rows"].as_array().expect("rows must be an array");

    for symbol in V1_ETF_SYMBOLS {
        let row = rows
            .iter()
            .find(|r| r["symbol"] == *symbol)
            .unwrap_or_else(|| panic!("etf row for {symbol} must be present"));
        assert_eq!(row["asset_class"], "equity", "symbol={symbol}");
        assert_eq!(row["instrument_kind"], "etf", "symbol={symbol}");
        assert_eq!(
            row["contract_multiplier_micros"].as_i64(),
            Some(MICROS_SCALE),
            "symbol={symbol}"
        );
        assert_eq!(
            row["model_only"], false,
            "etf rows are equity-equivalent, not model_only: symbol={symbol}"
        );
    }
}

// ACB04B-04: non_equity_enabled_count and model_only_count are both zero for
// the real (equities-only) production registry.
#[tokio::test]
async fn ac04b_04_real_registry_non_equity_enabled_and_model_only_counts_zero() {
    let router = make_router_with_real_registry();
    let (_, body) = call_json(router, ROUTE).await;

    assert_eq!(body["non_equity_enabled_count"].as_u64(), Some(0));
    assert_eq!(body["model_only_count"].as_u64(), Some(0));
}

// ACB04B-14: the route's honesty flags are always false/true as documented,
// even on the happy path with a fully-bridged real registry.
#[tokio::test]
async fn ac04b_14_route_reports_bridge_honesty_flags_for_real_registry() {
    let router = make_router_with_real_registry();
    let (status, body) = call_json(router, ROUTE).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["bridge_model_only"], true);
    assert_eq!(body["trading_uses_instrument_economics"], false);
    assert_eq!(body["runtime_uses_instrument_economics"], false);
    assert_eq!(body["risk_uses_instrument_economics"], false);
    assert_eq!(body["order_path_uses_instrument_economics"], false);
}

// ACB04B-15: the symbol query param filters rows to an exact match,
// case-insensitively, without affecting the full-registry counts.
#[tokio::test]
async fn ac04b_15_symbol_filter_returns_single_matching_row_counts_unaffected() {
    let router = make_router_with_real_registry();
    let (status, body) = call_json(router, &format!("{ROUTE}?symbol=aapl")).await;

    assert_eq!(status, StatusCode::OK);
    let rows = body["rows"].as_array().expect("rows must be an array");
    assert_eq!(rows.len(), 1, "rows={rows:?}");
    assert_eq!(rows[0]["symbol"], "AAPL");
    assert_eq!(
        body["instrument_count"].as_u64(),
        Some(88),
        "instrument_count must stay full-registry, unaffected by symbol filter"
    );
}

// ACB04B-15b: the limit query param truncates rows without affecting the
// full-registry counts.
#[tokio::test]
async fn ac04b_15b_limit_param_truncates_rows_counts_unaffected() {
    let router = make_router_with_real_registry();
    let (status, body) = call_json(router, &format!("{ROUTE}?limit=3")).await;

    assert_eq!(status, StatusCode::OK);
    let rows = body["rows"].as_array().expect("rows must be an array");
    assert_eq!(rows.len(), 3);
    assert_eq!(body["instrument_count"].as_u64(), Some(88));
    assert_eq!(body["bridged_count"].as_u64(), Some(88));
}

// ACB04B-16: a missing registry file returns a fail-closed envelope
// (truth_state=unavailable, HTTP 200) -- not a panic, not a 5xx.
#[tokio::test]
async fn ac04b_16_missing_registry_returns_fail_closed_envelope_no_panic() {
    let router = make_router_with_missing_registry();
    let (status, body) = call_json(router, ROUTE).await;

    assert_eq!(status, StatusCode::OK, "must still return 200: {body}");
    assert_eq!(body["truth_state"], "unavailable");
    assert_eq!(body["registry_v2_valid"], false);
    assert_eq!(body["instrument_count"].as_u64(), Some(0));
    assert_eq!(body["bridge_model_only"], true);
    assert!(
        body["errors"].as_array().is_some_and(|v| !v.is_empty()),
        "errors must be populated: {body}"
    );
}

// ACB04B-16b: malformed (unparseable) v1 JSON content returns a fail-closed
// envelope (truth_state=v1_load_failed) -- not a panic.
#[tokio::test]
async fn ac04b_16b_malformed_registry_returns_fail_closed_envelope_no_panic() {
    let (router, path) = make_router_with_registry_content("{not valid json", "malformed");
    let (status, body) = call_json(router, ROUTE).await;
    std::fs::remove_file(&path).ok();

    assert_eq!(status, StatusCode::OK, "must still return 200: {body}");
    assert_eq!(body["truth_state"], "v1_load_failed");
    assert_eq!(body["instrument_count"].as_u64(), Some(0));
}

// ---------------------------------------------------------------------------
// 5-13: pure-function tests (futures/options/crypto/forex; v1 cannot
// represent these asset classes -- see module docs).
// ---------------------------------------------------------------------------

// ACB04B-05: a futures fixture bridges its explicit raw multiplier (50) into
// `contract_multiplier_micros = 50 * MICROS_SCALE`, carries tick_size_micros
// through directly, and is model_only / never trading_enabled_by_bridge.
#[test]
fn ac04b_05_futures_fixture_bridges_explicit_multiplier() {
    let inst = base_future("ES2026U", 50);
    let result = state::instrument_v2_to_economics(&inst);

    assert_eq!(result.truth_state, "active", "result={result:?}");
    let econ = result.economics.clone().expect("economics must be present");
    assert_eq!(econ.contract_multiplier_micros, 50 * MICROS_SCALE);
    assert_eq!(econ.quote_currency, "USD");
    assert_eq!(econ.tick_size_micros, Some(250_000));
    assert!(result.model_only);
    assert!(!result.trading_enabled_by_bridge);
    assert!(!result.enabled, "fixture must stay disabled");
}

// ACB04B-06: an options fixture bridges its explicit raw multiplier (100)
// into `contract_multiplier_micros = 100 * MICROS_SCALE`.
#[test]
fn ac04b_06_options_fixture_bridges_explicit_multiplier() {
    let inst = base_option("AAPL20260918C150", 100);
    let result = state::instrument_v2_to_economics(&inst);

    assert_eq!(result.truth_state, "active", "result={result:?}");
    let econ = result.economics.clone().expect("economics must be present");
    assert_eq!(econ.contract_multiplier_micros, 100 * MICROS_SCALE);
    assert_eq!(econ.quote_currency, "USD");
    assert!(result.model_only);
    assert!(!result.trading_enabled_by_bridge);
}

// ACB04B-07: a crypto fixture bridges as model-only with a fractional-capable
// quantity_scale (1, the finest micros-grain) and is never trading-enabled.
#[test]
fn ac04b_07_crypto_fixture_bridges_fractional_quantity_model_only() {
    let inst = base_crypto("BTC/USD");
    let result = state::instrument_v2_to_economics(&inst);

    assert_eq!(result.truth_state, "active", "result={result:?}");
    let econ = result.economics.clone().expect("economics must be present");
    assert_eq!(
        econ.quantity_scale, 1,
        "crypto must demonstrate fractional-capable quantity_scale"
    );
    assert_eq!(econ.contract_multiplier_micros, MICROS_SCALE);
    assert!(result.model_only);
    assert!(!result.trading_enabled_by_bridge);
    assert!(!result.enabled);
    assert!(!result.paper_trading_enabled);
    assert!(!result.live_trading_enabled);
}

// ACB04B-08: a same-currency forex fixture (currency == quote_currency)
// bridges safely; a mismatched-currency forex fixture refuses with
// currency_conversion_unsupported -- the model has no base-currency field,
// so it cannot represent a genuinely cross-currency instrument.
#[test]
fn ac04b_08_forex_same_currency_bridges_mismatched_currency_refuses() {
    let same = base_forex_same_currency("EUR/USD");
    let result_same = state::instrument_v2_to_economics(&same);
    assert_eq!(
        result_same.truth_state, "active",
        "same-currency forex must bridge: result={result_same:?}"
    );
    assert!(result_same.model_only);
    assert!(!result_same.trading_enabled_by_bridge);
    let econ = result_same.economics.expect("economics must be present");
    assert_eq!(econ.quote_currency, "USD");
    assert_eq!(econ.contract_multiplier_micros, MICROS_SCALE);

    let mismatched = base_forex_mismatched_currency("EUR/USD");
    let result_mismatched = state::instrument_v2_to_economics(&mismatched);
    assert_eq!(
        result_mismatched.truth_state, "currency_conversion_unsupported",
        "result={result_mismatched:?}"
    );
    assert!(result_mismatched.economics.is_none());
    assert!(!result_mismatched.trading_enabled_by_bridge);
}

// ACB04B-09: a future/option fixture with no contract attached at all (the
// Rust-typed equivalent of "missing multiplier data") fails closed with
// truth_state=missing_multiplier rather than fabricating a default.
#[test]
fn ac04b_09_missing_contract_for_future_and_option_fails_closed() {
    for mut inst in [
        base_future("ES2026U", 50),
        base_option("AAPL20260918C150", 100),
    ] {
        inst.contract = None;
        let result = state::instrument_v2_to_economics(&inst);
        assert_eq!(
            result.truth_state, "missing_multiplier",
            "symbol={} result={result:?}",
            inst.symbol
        );
        assert!(result.economics.is_none());
        assert!(!result.trading_enabled_by_bridge);
    }
}

// ACB04B-10: zero/negative multipliers on future and option contracts fail
// closed with truth_state=invalid_multiplier.
#[test]
fn ac04b_10_invalid_multiplier_fails_closed_for_future_and_option() {
    for bad in [0i64, -1, -50] {
        let inst = base_future("ES2026U", bad);
        let result = state::instrument_v2_to_economics(&inst);
        assert_eq!(
            result.truth_state, "invalid_multiplier",
            "future multiplier={bad} result={result:?}"
        );
        assert!(result.economics.is_none());
    }
    for bad in [0i64, -1, -100] {
        let inst = base_option("AAPL20260918C150", bad);
        let result = state::instrument_v2_to_economics(&inst);
        assert_eq!(
            result.truth_state, "invalid_multiplier",
            "option multiplier={bad} result={result:?}"
        );
        assert!(result.economics.is_none());
    }
}

// ACB04B-11: an empty currency fails closed with truth_state=missing_currency
// (defensive: validate_registry_v2 already forbids this in any validated
// registry, but the bridge is a standalone pure function and must not trust
// unvalidated input).
#[test]
fn ac04b_11_missing_currency_fails_closed() {
    let mut inst = base_equity("AAPL");
    inst.currency = String::new();
    let result = state::instrument_v2_to_economics(&inst);

    assert_eq!(result.truth_state, "missing_currency", "result={result:?}");
    assert!(result.economics.is_none());
    assert!(!result.trading_enabled_by_bridge);
}

// ACB04B-12: an unknown/unsupported asset_class string fails closed with
// truth_state=unsupported_instrument rather than guessing a contract shape.
#[test]
fn ac04b_12_unsupported_asset_class_fails_closed() {
    let mut inst = base_equity("AAPL");
    inst.asset_class = "stock".to_string();
    let result = state::instrument_v2_to_economics(&inst);

    assert_eq!(
        result.truth_state, "unsupported_instrument",
        "result={result:?}"
    );
    assert!(result.economics.is_none());
    assert!(!result.trading_enabled_by_bridge);
}

// ACB04B-13: trading_enabled_by_bridge is always false, across every asset
// class and every success/failure outcome -- the single hardest invariant
// this bridge must never violate.
#[test]
fn ac04b_13_trading_enabled_by_bridge_always_false_across_all_fixtures() {
    let fixtures = vec![
        base_equity("AAPL"),
        base_etf("SPY"),
        base_future("ES2026U", 50),
        base_option("AAPL20260918C150", 100),
        base_crypto("BTC/USD"),
        base_forex_same_currency("EUR/USD"),
        base_forex_mismatched_currency("GBP/JPY"),
    ];
    for inst in &fixtures {
        let result = state::instrument_v2_to_economics(inst);
        assert!(
            !result.trading_enabled_by_bridge,
            "symbol={} result={result:?}",
            inst.symbol
        );
    }

    // Also fail-closed paths must report false, not merely omit the field.
    let mut bad_currency = base_equity("AAPL");
    bad_currency.currency = String::new();
    let mut bad_class = base_equity("AAPL");
    bad_class.asset_class = "stock".to_string();
    let bad_multiplier = base_future("ES2026U", -1);
    for inst in [bad_currency, bad_class, bad_multiplier] {
        let result = state::instrument_v2_to_economics(&inst);
        assert!(result.economics.is_none());
        assert!(!result.trading_enabled_by_bridge, "result={result:?}");
    }
}
