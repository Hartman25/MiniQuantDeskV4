//! BACKTEST-ECONOMICS-GUI-REGISTRY-01-COMBINED /
//! INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED.
//!
//! Proves the read-only backtest economics suggestion route. By default (no
//! `MQK_INSTRUMENT_REGISTRY_V2_PATH`) the route loads the configured v1
//! registry, converts it to InstrumentRegistryV2 in memory, validates it, and
//! returns only operator-facing backtest economics hints. When a separate v2
//! registry source is configured, the route searches it first, falling back
//! to the v1 path only when the v2 source is healthy but does not carry the
//! requested symbol -- a configured-but-broken v2 source fails closed and
//! never silently falls back. The route does not require a DB pool and does
//! not touch providers, brokers, orders, or live/paper runtime state.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

const ROUTE: &str = "/api/v1/backtests/economics-suggestion";

fn real_registry_path() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest_dir)
        .join("../../../config/instruments/equities.json")
        .canonicalize()
        .expect("registry path must resolve from CARGO_MANIFEST_DIR")
        .to_string_lossy()
        .to_string()
}

fn make_router_with_registry_path(registry_path: String) -> axum::Router {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = registry_path;
    routes::build_router(Arc::new(st))
}

fn make_router_with_real_registry() -> axum::Router {
    make_router_with_registry_path(real_registry_path())
}

/// Router with both the v1 registry path and an explicit (possibly
/// nonexistent/invalid) v2 source path configured.
fn make_router_with_v1_and_v2(v1_path: String, v2_path: Option<String>) -> axum::Router {
    let mut st =
        state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    st.instrument_registry_path = v1_path;
    st.instrument_registry_v2_path = v2_path;
    routes::build_router(Arc::new(st))
}

/// Write `content` to a uniquely-tagged temp file and return its path. Callers
/// must remove the file after the request (`std::fs::remove_file(&path).ok()`),
/// mirroring `scenario_instrument_registry_v2_status_asset_core_01c.rs`.
fn write_temp_v2_fixture(content: &str, tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_test_econ_v2_suggestion_{}_{}.json",
        tag,
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp v2 registry fixture");
    path
}

/// A disabled future instrument with explicit economics metadata.
fn future_with_economics_fixture(symbol: &str, multiplier: i64) -> String {
    serde_json::json!({
        "schema_version": 1,
        "instruments": [{
            "instrument_id": format!("future:CME:{symbol}"),
            "symbol": symbol,
            "asset_class": "future",
            "venue": "CME",
            "currency": "USD",
            "enabled": false,
            "paper_trading_enabled": false,
            "live_trading_enabled": false,
            "timeframes": ["1D"],
            "contract": {
                "kind": "future",
                "root": "ES",
                "expiry": "2026-09-18",
                "multiplier": multiplier,
                "tick_size_micros": 250_000
            },
            "notes": "test fixture",
            "economics": {
                "contract_multiplier": multiplier,
                "initial_margin_micros": 500_000_000,
                "maintenance_margin_micros": 400_000_000
            }
        }]
    })
    .to_string()
}

/// A disabled future instrument with no `economics` metadata at all.
fn future_without_economics_fixture(symbol: &str) -> String {
    serde_json::json!({
        "schema_version": 1,
        "instruments": [{
            "instrument_id": format!("future:CME:{symbol}"),
            "symbol": symbol,
            "asset_class": "future",
            "venue": "CME",
            "currency": "USD",
            "enabled": false,
            "paper_trading_enabled": false,
            "live_trading_enabled": false,
            "timeframes": ["1D"],
            "contract": {
                "kind": "future",
                "root": "ES",
                "expiry": "2026-09-18",
                "multiplier": 50,
                "tick_size_micros": 250_000
            },
            "notes": "test fixture, no economics metadata"
        }]
    })
    .to_string()
}

/// A registry that parses as valid JSON but fails `validate_registry_v2`:
/// `enabled=true` on a non-equity instrument without the explicit
/// test-only `allow_enabled_non_equity_for_testing` escape hatch.
fn invalid_enabled_future_fixture(symbol: &str) -> String {
    serde_json::json!({
        "schema_version": 1,
        "instruments": [{
            "instrument_id": format!("future:CME:{symbol}"),
            "symbol": symbol,
            "asset_class": "future",
            "venue": "CME",
            "currency": "USD",
            "enabled": true,
            "timeframes": ["1D"],
            "contract": {
                "kind": "future",
                "root": "ES",
                "expiry": "2026-09-18",
                "multiplier": 50,
                "tick_size_micros": 250_000
            }
        }]
    })
    .to_string()
}

async fn call(router: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
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
    (
        status,
        serde_json::from_slice(&body).expect("body is not valid JSON"),
    )
}

#[tokio::test]
async fn ber01_active_equity_symbol_returns_default_multiplier_one() {
    let (status, body) = call(
        make_router_with_real_registry(),
        &format!("{ROUTE}?symbol=AAPL"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "must return 200: {body}");
    assert_eq!(body["truth_state"], "active", "must be active: {body}");
    assert_eq!(body["symbol"], "AAPL", "must echo registry symbol: {body}");
    assert_eq!(
        body["source"], "instrument_registry_v2",
        "must name source: {body}"
    );
    assert_eq!(
        body["contract_multiplier"].as_i64(),
        Some(1),
        "equity default multiplier must be explicit: {body}"
    );
    assert!(
        body["initial_margin_micros"].is_null(),
        "no margin metadata in v1 registry: {body}"
    );
    assert!(
        body["maintenance_margin_micros"].is_null(),
        "no margin metadata in v1 registry: {body}"
    );
    assert_eq!(
        body["reason"], "equity_default",
        "must explain default: {body}"
    );
}

#[tokio::test]
async fn ber02_active_etf_symbol_returns_default_multiplier_one() {
    let (status, body) = call(
        make_router_with_real_registry(),
        &format!("{ROUTE}?symbol=SPY"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "must return 200: {body}");
    assert_eq!(
        body["truth_state"], "active",
        "ETF trades as equity: {body}"
    );
    assert_eq!(
        body["contract_multiplier"].as_i64(),
        Some(1),
        "ETF default multiplier: {body}"
    );
    assert_eq!(
        body["reason"], "equity_default",
        "must explain default: {body}"
    );
}

#[tokio::test]
async fn ber03_unknown_symbol_returns_not_found_without_fabricating_economics() {
    let (status, body) = call(
        make_router_with_real_registry(),
        &format!("{ROUTE}?symbol=NO_SUCH_SYMBOL"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "must return 200: {body}");
    assert_eq!(
        body["truth_state"], "not_found",
        "must truthfully report missing symbol: {body}"
    );
    assert!(
        body["contract_multiplier"].is_null(),
        "must not fabricate multiplier: {body}"
    );
    assert!(
        body["initial_margin_micros"].is_null(),
        "must not fabricate margin: {body}"
    );
    assert!(
        body["maintenance_margin_micros"].is_null(),
        "must not fabricate margin: {body}"
    );
}

#[tokio::test]
async fn ber04_missing_registry_returns_registry_unavailable() {
    let router = make_router_with_registry_path(
        "/nonexistent/path/that/cannot/exist/equities.json".to_string(),
    );
    let (status, body) = call(router, &format!("{ROUTE}?symbol=AAPL")).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "must return 200 with truth_state: {body}"
    );
    assert_eq!(
        body["truth_state"], "registry_unavailable",
        "must truthfully report registry unavailable: {body}"
    );
    assert!(
        body["contract_multiplier"].is_null(),
        "must not fabricate multiplier: {body}"
    );
}

#[tokio::test]
async fn ber05_missing_symbol_query_is_bad_request() {
    let (status, body) = call(make_router_with_real_registry(), ROUTE).await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "missing query must fail closed: {body}"
    );
    assert_eq!(
        body["truth_state"], "error",
        "must report error state: {body}"
    );
    assert!(
        body["contract_multiplier"].is_null(),
        "must not fabricate multiplier: {body}"
    );
}

// ---------------------------------------------------------------------------
// INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED: separate v2 registry source.
// ---------------------------------------------------------------------------

// BER-06: with no MQK_INSTRUMENT_REGISTRY_V2_PATH configured (the default,
// `None`), behavior is exactly the pre-existing v1-only path, and the new
// asset_class/enabled/paper_trading_enabled/live_trading_enabled fields are
// populated truthfully from the v1-converted equity instrument.
#[tokio::test]
async fn ber06_no_v2_path_configured_preserves_v1_only_behavior_and_reports_equity_fields() {
    let (status, body) = call(
        make_router_with_real_registry(),
        &format!("{ROUTE}?symbol=AAPL"),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "must return 200: {body}");
    assert_eq!(body["truth_state"], "active");
    assert_eq!(body["contract_multiplier"].as_i64(), Some(1));
    assert_eq!(body["reason"], "equity_default");
    assert_eq!(
        body["asset_class"], "equity",
        "v1-converted equity must report asset_class: {body}"
    );
    assert_eq!(
        body["enabled"], true,
        "AAPL is enabled in the real v1 registry: {body}"
    );
    assert_eq!(body["paper_trading_enabled"], false, "{body}");
    assert_eq!(body["live_trading_enabled"], false, "{body}");
}

// BER-07: a configured, valid v2 source containing a disabled future with
// explicit economics returns that economics end-to-end, plus truthful
// disabled/non-tradable safety fields -- proving explicit v2 multiplier
// metadata through the route without enabling any trading.
#[tokio::test]
async fn ber07_configured_v2_disabled_future_with_economics_returns_explicit_metadata() {
    let v2_content = future_with_economics_fixture("ES_TEST", 50);
    let v2_path = write_temp_v2_fixture(&v2_content, "explicit_econ");

    let router = make_router_with_v1_and_v2(
        real_registry_path(),
        Some(v2_path.to_string_lossy().to_string()),
    );
    let (status, body) = call(router, &format!("{ROUTE}?symbol=ES_TEST")).await;
    std::fs::remove_file(&v2_path).ok();

    assert_eq!(status, StatusCode::OK, "must return 200: {body}");
    assert_eq!(body["truth_state"], "active", "{body}");
    assert_eq!(body["symbol"], "ES_TEST");
    assert_eq!(body["source"], "instrument_registry_v2");
    assert_eq!(
        body["contract_multiplier"].as_i64(),
        Some(50),
        "explicit v2 multiplier must win: {body}"
    );
    assert_eq!(body["initial_margin_micros"].as_i64(), Some(500_000_000));
    assert_eq!(body["maintenance_margin_micros"].as_i64(), Some(400_000_000));
    assert_eq!(body["reason"], "registry_v2_explicit");
    assert_eq!(body["asset_class"], "future");
    assert_eq!(
        body["enabled"], false,
        "fixture future must remain disabled: {body}"
    );
    assert_eq!(body["paper_trading_enabled"], false, "{body}");
    assert_eq!(body["live_trading_enabled"], false, "{body}");
}

// BER-08: a configured, valid v2 source containing a known instrument with no
// economics metadata truthfully reports no_contract_economics rather than
// fabricating a multiplier -- while still reporting the safety fields.
#[tokio::test]
async fn ber08_configured_v2_known_instrument_without_economics_returns_no_contract_economics() {
    let v2_content = future_without_economics_fixture("FUT_NO_ECON_TEST");
    let v2_path = write_temp_v2_fixture(&v2_content, "no_econ");

    let router = make_router_with_v1_and_v2(
        real_registry_path(),
        Some(v2_path.to_string_lossy().to_string()),
    );
    let (status, body) = call(router, &format!("{ROUTE}?symbol=FUT_NO_ECON_TEST")).await;
    std::fs::remove_file(&v2_path).ok();

    assert_eq!(status, StatusCode::OK, "must return 200: {body}");
    assert_eq!(body["truth_state"], "no_contract_economics", "{body}");
    assert!(
        body["contract_multiplier"].is_null(),
        "must not fabricate multiplier: {body}"
    );
    assert_eq!(body["asset_class"], "future");
    assert_eq!(body["enabled"], false);
}

// BER-09: a configured v2 path that does not exist fails closed with
// registry_unavailable and does NOT silently fall back to v1 -- even though
// the symbol exists and is healthy in the v1 registry. A bad explicit config
// must never look like success.
#[tokio::test]
async fn ber09_configured_v2_missing_path_returns_registry_unavailable_without_v1_fallback() {
    let router = make_router_with_v1_and_v2(
        real_registry_path(),
        Some("/nonexistent/path/that/cannot/exist/instruments_v2.json".to_string()),
    );
    let (status, body) = call(router, &format!("{ROUTE}?symbol=AAPL")).await;

    assert_eq!(status, StatusCode::OK, "must return 200 with truth_state: {body}");
    assert_eq!(
        body["truth_state"], "registry_unavailable",
        "must fail closed, not fall back to v1: {body}"
    );
    assert!(
        body["contract_multiplier"].is_null(),
        "must not fabricate multiplier: {body}"
    );
    assert!(body["asset_class"].is_null());
}

// BER-10: a configured v2 path whose content parses as JSON but fails
// validate_registry_v2 (enabled non-equity instrument without the test-only
// escape hatch) reports validation_failed -- distinct from
// registry_unavailable -- and does not fall back to v1.
#[tokio::test]
async fn ber10_configured_v2_invalid_registry_returns_validation_failed() {
    let v2_content = invalid_enabled_future_fixture("BAD_TEST");
    let v2_path = write_temp_v2_fixture(&v2_content, "invalid");

    let router = make_router_with_v1_and_v2(
        real_registry_path(),
        Some(v2_path.to_string_lossy().to_string()),
    );
    let (status, body) = call(router, &format!("{ROUTE}?symbol=AAPL")).await;
    std::fs::remove_file(&v2_path).ok();

    assert_eq!(status, StatusCode::OK, "must return 200 with truth_state: {body}");
    assert_eq!(
        body["truth_state"], "validation_failed",
        "must distinguish from registry_unavailable: {body}"
    );
    assert!(body["contract_multiplier"].is_null());
}

// BER-11: a configured, valid v2 source that simply does not carry the
// requested symbol falls through to the unchanged v1 lookup -- proving the
// "v2 first, then v1" chain rather than "v2 exclusive".
#[tokio::test]
async fn ber11_configured_v2_symbol_absent_falls_back_to_v1_default() {
    let v2_content = future_with_economics_fixture("ES_TEST", 50);
    let v2_path = write_temp_v2_fixture(&v2_content, "fallback");

    let router = make_router_with_v1_and_v2(
        real_registry_path(),
        Some(v2_path.to_string_lossy().to_string()),
    );
    let (status, body) = call(router, &format!("{ROUTE}?symbol=AAPL")).await;
    std::fs::remove_file(&v2_path).ok();

    assert_eq!(status, StatusCode::OK, "must return 200: {body}");
    assert_eq!(
        body["truth_state"], "active",
        "must fall back to v1 for a symbol absent from v2: {body}"
    );
    assert_eq!(body["contract_multiplier"].as_i64(), Some(1));
    assert_eq!(body["reason"], "equity_default");
}

// BER-12: a symbol absent from both a configured-valid v2 source and v1
// truthfully reports not_found.
#[tokio::test]
async fn ber12_configured_v2_and_v1_both_missing_symbol_returns_not_found() {
    let v2_content = future_with_economics_fixture("ES_TEST", 50);
    let v2_path = write_temp_v2_fixture(&v2_content, "bothmiss");

    let router = make_router_with_v1_and_v2(
        real_registry_path(),
        Some(v2_path.to_string_lossy().to_string()),
    );
    let (status, body) = call(router, &format!("{ROUTE}?symbol=NO_SUCH_SYMBOL_ZZZ")).await;
    std::fs::remove_file(&v2_path).ok();

    assert_eq!(status, StatusCode::OK, "must return 200: {body}");
    assert_eq!(body["truth_state"], "not_found", "{body}");
    assert!(body["contract_multiplier"].is_null());
    assert!(body["asset_class"].is_null());
}
