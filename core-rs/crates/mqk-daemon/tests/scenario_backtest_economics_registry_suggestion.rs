//! BACKTEST-ECONOMICS-GUI-REGISTRY-01-COMBINED.
//!
//! Proves the read-only backtest economics suggestion route. The route loads
//! the configured v1 registry, converts it to InstrumentRegistryV2 in memory,
//! validates it, and returns only operator-facing backtest economics hints. It
//! does not require a DB pool and does not touch providers, brokers, orders, or
//! live/paper runtime state.

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
