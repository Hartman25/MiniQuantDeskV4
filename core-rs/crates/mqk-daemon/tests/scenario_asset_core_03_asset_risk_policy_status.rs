//! ASSET-CORE-03B-ASSET-RISK-POLICY-SAFE-GAP-CLOSURE-01.
//!
//! Proves the read-only `GET /api/v1/system/asset-risk-policy` surface: it
//! must report the existing static `mqk_execution::asset_risk_policy` model
//! (equity/ETF enabled, crypto/future/option/forex disabled,
//! rates/fixed-income research-only) without requiring a DB pool, broker, or
//! provider, and must not change Gate 0 or the broker-submit routing guard.
//!
//! ## CI command
//!
//! ```sh
//! cargo test -p mqk-daemon --test scenario_asset_core_03_asset_risk_policy_status
//! ```

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

const ROUTE: &str = "/api/v1/system/asset-risk-policy";

fn make_router() -> axum::Router {
    let st = state::AppState::new_with_operator_auth(state::OperatorAuthMode::ExplicitDevNoToken);
    routes::build_router(Arc::new(st))
}

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    (status, body)
}

fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

async fn get_status(router: axum::Router) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri(ROUTE)
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    (status, parse_json(body))
}

fn entry_for<'a>(body: &'a serde_json::Value, asset_class: &str) -> &'a serde_json::Value {
    body["entries"]
        .as_array()
        .expect("entries must be an array")
        .iter()
        .find(|e| e["asset_class"] == asset_class)
        .unwrap_or_else(|| panic!("no entry for asset_class={asset_class}: {body}"))
}

// AC03B-01: route returns 200 with no DB pool, broker, or provider configured.
#[tokio::test]
async fn ac03b_01_status_route_returns_200_without_db_or_broker() {
    let (status, body) = get_status(make_router()).await;
    assert_eq!(status, StatusCode::OK, "must return 200: {body}");
}

// AC03B-02: top-level fields prove this is model-only.
#[tokio::test]
async fn ac03b_02_top_level_fields_prove_model_only() {
    let (_, body) = get_status(make_router()).await;
    assert_eq!(
        body["production_enforcement_enabled"], false,
        "production_enforcement_enabled must be false: {body}"
    );
    assert_eq!(
        body["non_equity_routing_enabled"], false,
        "non_equity_routing_enabled must be false: {body}"
    );
    assert!(
        body["policy_source"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "policy_source must be a non-empty string: {body}"
    );
    assert_eq!(
        body["schema_version"], "asset-risk-policy-v1",
        "schema_version must be asset-risk-policy-v1: {body}"
    );
}

// AC03B-03: equity policy reports enabled/paper-only.
#[tokio::test]
async fn ac03b_03_equity_policy_reports_enabled_paper_only() {
    let (_, body) = get_status(make_router()).await;
    let equity = entry_for(&body, "equity");
    assert_eq!(equity["state"], "enabled", "{equity}");
    assert_eq!(equity["paper_trading_enabled"], true, "{equity}");
    assert_eq!(equity["live_trading_enabled"], false, "{equity}");
}

// AC03B-04: ETF-as-equity policy reports enabled, distinct asset_class label.
#[tokio::test]
async fn ac03b_04_etf_as_equity_policy_reports_enabled() {
    let (_, body) = get_status(make_router()).await;
    let etf = entry_for(&body, "etf_as_equity");
    assert_eq!(etf["state"], "enabled", "{etf}");
    assert_eq!(etf["live_trading_enabled"], false, "{etf}");
}

// AC03B-05..08: every non-equity class reports disabled or research_only,
// never enabled, and live_trading_enabled is always false.
#[tokio::test]
async fn ac03b_05_non_equity_classes_never_report_enabled_or_live() {
    let (_, body) = get_status(make_router()).await;
    for asset_class in ["crypto", "future", "option", "forex"] {
        let entry = entry_for(&body, asset_class);
        assert_eq!(
            entry["state"], "disabled",
            "asset_class={asset_class} must be disabled: {entry}"
        );
        assert_eq!(
            entry["paper_trading_enabled"], false,
            "asset_class={asset_class} must not be paper-enabled: {entry}"
        );
        assert_eq!(
            entry["live_trading_enabled"], false,
            "asset_class={asset_class} must never be live-enabled: {entry}"
        );
    }
}

// AC03B-06: rates/fixed-income reports research_only, not enabled.
#[tokio::test]
async fn ac03b_06_rates_fixed_income_reports_research_only() {
    let (_, body) = get_status(make_router()).await;
    let rates = entry_for(&body, "rates_fixed_income");
    assert_eq!(rates["state"], "research_only", "{rates}");
    assert_eq!(rates["paper_trading_enabled"], false, "{rates}");
    assert_eq!(rates["live_trading_enabled"], false, "{rates}");
}

// AC03B-07: every entry carries a non-empty reason_code and message —
// operators must never see an unexplained policy row.
#[tokio::test]
async fn ac03b_07_every_entry_carries_reason_code_and_message() {
    let (_, body) = get_status(make_router()).await;
    let entries = body["entries"].as_array().expect("entries array");
    assert!(!entries.is_empty(), "entries must not be empty: {body}");
    for entry in entries {
        assert!(
            entry["reason_code"].as_str().is_some_and(|s| !s.is_empty()),
            "reason_code must be non-empty: {entry}"
        );
        assert!(
            entry["message"].as_str().is_some_and(|s| !s.is_empty()),
            "message must be non-empty: {entry}"
        );
    }
}

// AC03B-08: this read-only route must not itself require State/DB wiring
// beyond default AppState — proven implicitly by AC03B-01 above using a
// router built with no DB pool and no broker configured.
