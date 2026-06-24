//! SHORT-SIDE-EXTERNAL-SIGNAL-WIRING-01 — shortable preflight HTTP surface.
//!
//! Proves `GET /api/v1/broker/assets/:symbol/shortable-preflight` is read-only,
//! truthful when unconfigured, and testable without broker network calls.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::state::{
    AppState, BrokerAssetShortablePreflight, BrokerAssetShortablePreflightFetcher, BrokerKind,
    DeploymentMode,
};
use mqk_daemon::{routes, state};
use tokio::sync::Mutex;
use tower::ServiceExt;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn remove(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

#[derive(Debug)]
struct FakeFetcher {
    result: Result<Option<BrokerAssetShortablePreflight>, String>,
}

impl BrokerAssetShortablePreflightFetcher for FakeFetcher {
    fn fetch_asset_shortable_preflight(
        &self,
        _symbol: &str,
    ) -> Result<Option<BrokerAssetShortablePreflight>, String> {
        self.result.clone()
    }
}

async fn call(router: axum::Router, symbol: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri(format!(
            "/api/v1/broker/assets/{symbol}/shortable-preflight"
        ))
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
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body is valid JSON");
    (status, json)
}

fn paper_alpaca_without_creds() -> Arc<AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ))
}

fn state_with_fetcher(
    result: Result<Option<BrokerAssetShortablePreflight>, String>,
) -> Arc<AppState> {
    let mut st = state::AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st.set_asset_shortable_preflight_fetcher_for_test(Arc::new(FakeFetcher { result }));
    Arc::new(st)
}

fn asset(symbol: &str, shortable: bool) -> BrokerAssetShortablePreflight {
    BrokerAssetShortablePreflight {
        symbol: symbol.to_string(),
        asset_class: "equity".to_string(),
        tradable: true,
        shortable,
        marginable: Some(true),
        easy_to_borrow: None,
        source: "test_asset".to_string(),
    }
}

#[tokio::test]
async fn spr01_no_fetcher_reports_not_configured() {
    let _lock = ENV_LOCK.lock().await;
    let _key = EnvGuard::remove("ALPACA_API_KEY_PAPER");
    let _secret = EnvGuard::remove("ALPACA_API_SECRET_PAPER");
    let router = routes::build_router(paper_alpaca_without_creds());

    let (status, json) = call(router, "AAPL").await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["truth_state"], "not_configured", "{json}");
    assert_eq!(json["symbol"], "AAPL", "{json}");
    assert_eq!(json["shortable"], serde_json::Value::Null, "{json}");
}

#[tokio::test]
async fn spr02_active_shortable_true_response_is_truthful() {
    let router = routes::build_router(state_with_fetcher(Ok(Some(asset("AAPL", true)))));

    let (status, json) = call(router, "aapl").await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["truth_state"], "active", "{json}");
    assert_eq!(json["symbol"], "AAPL", "{json}");
    assert_eq!(json["asset_class"], "equity", "{json}");
    assert_eq!(json["tradable"], true, "{json}");
    assert_eq!(json["shortable"], true, "{json}");
    assert_eq!(json["source"], "test_asset", "{json}");
}

#[tokio::test]
async fn spr03_active_shortable_false_response_is_truthful() {
    let router = routes::build_router(state_with_fetcher(Ok(Some(asset("AAPL", false)))));

    let (status, json) = call(router, "AAPL").await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["truth_state"], "active", "{json}");
    assert_eq!(json["shortable"], false, "{json}");
}

#[tokio::test]
async fn spr04_query_failure_reports_query_failed_without_order_surface() {
    let router = routes::build_router(state_with_fetcher(Err(
        "simulated broker read failure".to_string()
    )));

    let (status, json) = call(router, "AAPL").await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["truth_state"], "query_failed", "{json}");
    assert!(
        json["message"]
            .as_str()
            .unwrap_or("")
            .contains("simulated broker read failure"),
        "{json}"
    );
    assert!(json.get("order_id").is_none(), "{json}");
    assert!(json.get("accepted").is_none(), "{json}");
}

#[tokio::test]
async fn spr05_symbol_not_found_is_distinct_truth_state() {
    let router = routes::build_router(state_with_fetcher(Ok(None)));

    let (status, json) = call(router, "AAPL").await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["truth_state"], "symbol_not_found", "{json}");
    assert_eq!(json["shortable"], serde_json::Value::Null, "{json}");
}

#[tokio::test]
async fn spr06_broker_unavailable_reports_distinct_truth_state() {
    let router = routes::build_router(state_with_fetcher(Err(
        "broker_unavailable: simulated outage".to_string(),
    )));

    let (status, json) = call(router, "AAPL").await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["truth_state"], "broker_unavailable", "{json}");
    assert_eq!(json["shortable"], serde_json::Value::Null, "{json}");
}
