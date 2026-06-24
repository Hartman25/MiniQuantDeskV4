//! SHORT-SIDE-EXTERNAL-SIGNAL-WIRING-01 — external strategy/signal short gate.
//!
//! Proves the external HTTP signal route classifies sell-side transitions using
//! current position truth and denies short entries before outbox unless both
//! short-entry policy and read-only shortable preflight allow them.

use std::path::PathBuf;
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
use uuid::Uuid;

const ENV_CAPITAL_POLICY_PATH: &str = "MQK_CAPITAL_POLICY_PATH";
const SYMBOL: &str = "AAPL";
const STRATEGY_ID: &str = "strat-short-ext-01";

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, prev }
    }

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
struct FakeAssetFetcher {
    result: Result<Option<BrokerAssetShortablePreflight>, String>,
}

impl BrokerAssetShortablePreflightFetcher for FakeAssetFetcher {
    fn fetch_asset_shortable_preflight(
        &self,
        _symbol: &str,
    ) -> Result<Option<BrokerAssetShortablePreflight>, String> {
        self.result.clone()
    }
}

fn fake_asset(shortable: bool) -> BrokerAssetShortablePreflight {
    BrokerAssetShortablePreflight {
        symbol: SYMBOL.to_string(),
        asset_class: "equity".to_string(),
        tradable: true,
        shortable,
        marginable: Some(true),
        easy_to_borrow: Some(true),
        source: "test_asset".to_string(),
    }
}

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body is not valid JSON");
    (status, json)
}

fn signal_req(signal_id: &str, side: &str, qty: i64) -> Request<axum::body::Body> {
    let body = serde_json::json!({
        "signal_id": signal_id,
        "strategy_id": STRATEGY_ID,
        "symbol": SYMBOL,
        "side": side,
        "qty": qty,
        "asset_class": "equity"
    });
    Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

fn paper_alpaca_state() -> AppState {
    state::AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Alpaca)
}

fn state_with_fetcher(
    result: Result<Option<BrokerAssetShortablePreflight>, String>,
) -> Arc<AppState> {
    let mut st = paper_alpaca_state();
    st.set_asset_shortable_preflight_fetcher_for_test(Arc::new(FakeAssetFetcher { result }));
    Arc::new(st)
}

async fn set_position(st: &Arc<AppState>, symbol: &str, qty: i64) {
    *st.execution_snapshot.write().await = Some(mqk_runtime::observability::ExecutionSnapshot {
        run_id: None,
        active_orders: vec![],
        pending_outbox: vec![],
        recent_inbox_events: vec![],
        portfolio: mqk_runtime::observability::PortfolioSnapshot {
            cash_micros: 0,
            realized_pnl_micros: 0,
            positions: vec![mqk_runtime::observability::PositionSnapshot {
                symbol: symbol.to_string(),
                net_qty: qty,
            }],
        },
        system_block_state: None,
        recent_risk_denials: vec![],
        has_recent_terminal_fill: false,
        risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
        snapshot_at_utc: chrono::Utc::now(),
    });
}

fn write_policy(allow_short_entries: bool, require_shortable_check: bool) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mqk_short_ext_policy_{}_{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create policy dir");
    let path = dir.join("capital_allocation_policy.json");
    let json = serde_json::json!({
        "schema_version": "policy-v1",
        "policy_id": "short-ext-test",
        "enabled": true,
        "per_strategy_budgets": [{
            "strategy_id": STRATEGY_ID,
            "budget_authorized": true,
            "risk_bucket": "equity_short_test"
        }],
        "short_entry_policy": {
            "allow_short_entries": allow_short_entries,
            "paper_short_entries_enabled": allow_short_entries,
            "live_short_entries_enabled": false,
            "require_shortable_check": require_shortable_check,
            "max_short_notional_usd": null,
            "max_short_shares": null
        }
    });
    std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).expect("write policy");
    (path, dir)
}

fn cleanup_dir(dir: PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

fn disposition(json: &serde_json::Value) -> &str {
    json["disposition"].as_str().unwrap_or("")
}

fn blockers(json: &serde_json::Value) -> String {
    json["blockers"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

#[tokio::test]
async fn esg01_sell_from_flat_denied_by_default_before_outbox() {
    let _lock = ENV_LOCK.lock().await;
    let _policy = EnvGuard::remove(ENV_CAPITAL_POLICY_PATH);
    let st = Arc::new(paper_alpaca_state());

    let (status, json) = call(routes::build_router(st), signal_req("esg01", "sell", 1)).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    assert_eq!(disposition(&json), "short_entry_disabled", "{json}");
    assert_eq!(json["intent_placed"], false, "{json}");
    assert!(blockers(&json).contains("ShortOpen"), "{json}");
}

#[tokio::test]
async fn esg02_enabled_sell_from_flat_denied_when_preflight_unavailable() {
    let _lock = ENV_LOCK.lock().await;
    let (path, dir) = write_policy(true, true);
    let _policy = EnvGuard::set(ENV_CAPITAL_POLICY_PATH, path.to_str().unwrap());
    let st = Arc::new(paper_alpaca_state());

    let (status, json) = call(routes::build_router(st), signal_req("esg02", "sell", 1)).await;
    cleanup_dir(dir);

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    assert_eq!(
        disposition(&json),
        "shortable_preflight_unavailable",
        "{json}"
    );
    assert_eq!(json["intent_placed"], false, "{json}");
}

#[tokio::test]
async fn esg03_enabled_sell_from_flat_denied_when_symbol_not_shortable() {
    let _lock = ENV_LOCK.lock().await;
    let (path, dir) = write_policy(true, true);
    let _policy = EnvGuard::set(ENV_CAPITAL_POLICY_PATH, path.to_str().unwrap());
    let st = state_with_fetcher(Ok(Some(fake_asset(false))));

    let (status, json) = call(routes::build_router(st), signal_req("esg03", "sell", 1)).await;
    cleanup_dir(dir);

    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    assert_eq!(disposition(&json), "symbol_not_shortable", "{json}");
    assert_eq!(json["intent_placed"], false, "{json}");
}

#[tokio::test]
async fn esg04_enabled_shortable_true_passes_short_gate_to_existing_ws_gate() {
    let _lock = ENV_LOCK.lock().await;
    let (path, dir) = write_policy(true, true);
    let _policy = EnvGuard::set(ENV_CAPITAL_POLICY_PATH, path.to_str().unwrap());
    let st = state_with_fetcher(Ok(Some(fake_asset(true))));

    let (status, json) = call(routes::build_router(st), signal_req("esg04", "sell", 1)).await;
    cleanup_dir(dir);

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    assert_ne!(disposition(&json), "short_entry_disabled", "{json}");
    assert_ne!(
        disposition(&json),
        "shortable_preflight_unavailable",
        "{json}"
    );
    assert!(
        blockers(&json).to_lowercase().contains("continuity"),
        "{json}"
    );
}

#[tokio::test]
async fn esg05_sell_to_reduce_long_remains_allowed_without_short_policy() {
    let _lock = ENV_LOCK.lock().await;
    let _policy = EnvGuard::remove(ENV_CAPITAL_POLICY_PATH);
    let st = Arc::new(paper_alpaca_state());
    set_position(&st, SYMBOL, 10).await;

    let (status, json) = call(routes::build_router(st), signal_req("esg05", "sell", 5)).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{json}");
    assert_ne!(disposition(&json), "short_entry_disabled", "{json}");
    assert!(
        blockers(&json).to_lowercase().contains("continuity"),
        "{json}"
    );
}

#[tokio::test]
async fn esg06_sell_beyond_long_follows_short_entry_policy() {
    let _lock = ENV_LOCK.lock().await;
    let _policy = EnvGuard::remove(ENV_CAPITAL_POLICY_PATH);
    let st = Arc::new(paper_alpaca_state());
    set_position(&st, SYMBOL, 5).await;

    let (status, json) = call(routes::build_router(st), signal_req("esg06", "sell", 10)).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    assert_eq!(disposition(&json), "short_entry_disabled", "{json}");
    assert!(blockers(&json).contains("SellBeyondLongToShort"), "{json}");
}
