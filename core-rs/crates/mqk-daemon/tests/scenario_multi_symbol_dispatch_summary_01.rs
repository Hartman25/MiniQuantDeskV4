//! MULTI-SYMBOL-DISPATCH-SUMMARY-01: read-only API summary route proof tests.
//!
//! The route exposes the in-memory `PerSymbolTargetState` map recorded by
//! Patch 8. It must not persist state, call broker/OMS paths, or authorize
//! live routing.

use std::sync::{Arc, OnceLock};

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    api_types::MultiSymbolDispatchSummaryResponse,
    routes,
    state::{self, AppState, PerSymbolTargetState},
};
use tokio::sync::Mutex;
use tower::ServiceExt;

const ROUTE: &str = "/api/v1/strategy/multi-symbol-dispatch-summary";

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvVarGuard {
    key: &'static str,
    prior: Option<String>,
}

impl EnvVarGuard {
    fn remove(key: &'static str) -> Self {
        let prior = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, prior }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

fn no_runtime_config_env() -> Vec<EnvVarGuard> {
    vec![
        EnvVarGuard::remove("MQK_PAPER_WATCHLIST_PATH"),
        EnvVarGuard::remove("MQK_STRATEGY_SYMBOL"),
        EnvVarGuard::remove("MQK_STRATEGY_IDS"),
        EnvVarGuard::remove("MQK_STRATEGY_MD_TIMEFRAME"),
    ]
}

fn bare_state() -> Arc<AppState> {
    Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ))
}

fn target_state(
    symbol: &str,
    strategy_id: &str,
    current_qty: i64,
    target_qty: i64,
    no_order_reason: &str,
) -> PerSymbolTargetState {
    PerSymbolTargetState {
        symbol: symbol.to_string(),
        strategy_id: strategy_id.to_string(),
        current_qty,
        target_qty,
        delta: target_qty - current_qty,
        no_order_reason: no_order_reason.to_string(),
        last_decision_id: None,
        last_decision_disposition: None,
        updated_at_utc: "2026-01-01T00:00:00Z".to_string(),
    }
}

async fn request_summary(st: Arc<AppState>) -> MultiSymbolDispatchSummaryResponse {
    let router = routes::build_router(st);
    let req = Request::builder()
        .method("GET")
        .uri(ROUTE)
        .body(axum::body::Body::empty())
        .expect("request");
    let resp = router.oneshot(req).await.expect("oneshot failed");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect")
        .to_bytes();
    serde_json::from_slice(&body).expect("summary response json")
}

#[tokio::test]
async fn s01_no_target_states_returns_no_snapshot_and_empty_per_symbol() {
    let _lock = env_lock().lock().await;
    let _env = no_runtime_config_env();
    let response = request_summary(bare_state()).await;

    assert_eq!(response.truth_state, "no_snapshot");
    assert!(response.per_symbol.is_empty());
    assert_eq!(response.configured_symbol_count, 0);
}

#[tokio::test]
async fn s02_response_includes_canonical_route() {
    let _lock = env_lock().lock().await;
    let _env = no_runtime_config_env();
    let response = request_summary(bare_state()).await;

    assert_eq!(response.canonical_route, ROUTE);
}

#[tokio::test]
async fn s03_response_backend_is_daemon_runtime_state() {
    let _lock = env_lock().lock().await;
    let _env = no_runtime_config_env();
    let response = request_summary(bare_state()).await;

    assert_eq!(response.backend, "daemon.runtime_state");
}

#[tokio::test]
async fn s04_recorded_target_state_rows_appear_in_per_symbol() {
    let _lock = env_lock().lock().await;
    let _env = no_runtime_config_env();
    let st = bare_state();
    st.record_per_symbol_target_state(target_state(
        "AAPL",
        "swing_momentum",
        0,
        10,
        "order_will_be_submitted",
    ))
    .await;

    let response = request_summary(st).await;

    assert_eq!(response.truth_state, "active");
    assert_eq!(response.runtime_execution_mode, "single_symbol");
    assert_eq!(response.configured_symbol_count, 1);
    assert_eq!(response.per_symbol.len(), 1);
    assert_eq!(response.per_symbol[0].symbol, "AAPL");
}

#[tokio::test]
async fn s05_rows_preserve_deterministic_symbol_ordering() {
    let _lock = env_lock().lock().await;
    let _env = no_runtime_config_env();
    let st = bare_state();
    st.record_per_symbol_target_state(target_state("TSLA", "s3", 0, 1, "hold"))
        .await;
    st.record_per_symbol_target_state(target_state("AAPL", "s1", 0, 1, "hold"))
        .await;
    st.record_per_symbol_target_state(target_state("MSFT", "s2", 0, 1, "hold"))
        .await;

    let response = request_summary(st).await;
    let symbols: Vec<&str> = response
        .per_symbol
        .iter()
        .map(|row| row.symbol.as_str())
        .collect();

    assert_eq!(symbols, vec!["AAPL", "MSFT", "TSLA"]);
}

#[tokio::test]
async fn s06_row_fields_copy_qty_delta_and_no_order_reason() {
    let _lock = env_lock().lock().await;
    let _env = no_runtime_config_env();
    let st = bare_state();
    st.record_per_symbol_target_state(target_state(
        "AAPL",
        "swing_momentum",
        3,
        8,
        "already_at_target",
    ))
    .await;

    let mut rows = request_summary(st).await.per_symbol;
    let row = rows.remove(0);

    assert_eq!(row.current_qty, 3);
    assert_eq!(row.target_qty, 8);
    assert_eq!(row.delta, 5);
    assert_eq!(row.no_order_reason, "already_at_target");
}

#[tokio::test]
async fn s07_row_fields_copy_last_decision_id_and_disposition() {
    let _lock = env_lock().lock().await;
    let _env = no_runtime_config_env();
    let st = bare_state();
    let mut row = target_state("AAPL", "swing_momentum", 0, 5, "order_will_be_submitted");
    row.last_decision_id = Some("decision-001".to_string());
    row.last_decision_disposition = Some("accepted".to_string());
    st.record_per_symbol_target_state(row).await;

    let mut rows = request_summary(st).await.per_symbol;
    let row = rows.remove(0);

    assert_eq!(row.last_decision_id.as_deref(), Some("decision-001"));
    assert_eq!(row.last_decision_disposition.as_deref(), Some("accepted"));
}

#[tokio::test]
async fn s08_max_new_orders_per_tick_reached_is_preserved_as_no_order_reason() {
    let _lock = env_lock().lock().await;
    let _env = no_runtime_config_env();
    let st = bare_state();
    st.record_per_symbol_target_state(target_state(
        "MSFT",
        "swing_momentum",
        3,
        3,
        "max_new_orders_per_tick_reached",
    ))
    .await;

    let mut rows = request_summary(st).await.per_symbol;
    let row = rows.remove(0);

    assert_eq!(row.no_order_reason, "max_new_orders_per_tick_reached");
}

#[tokio::test]
async fn s09_no_order_reason_values_are_not_remapped() {
    let _lock = env_lock().lock().await;
    let _env = no_runtime_config_env();
    let st = bare_state();
    st.record_per_symbol_target_state(target_state("AAPL", "s1", 0, -5, "b5_short_sale_guard"))
        .await;
    st.record_per_symbol_target_state(target_state("MSFT", "s2", 7, 7, "already_at_target"))
        .await;
    st.record_per_symbol_target_state(target_state("TSLA", "s3", 0, 1, "order_will_be_submitted"))
        .await;

    let reasons: Vec<String> = request_summary(st)
        .await
        .per_symbol
        .into_iter()
        .map(|row| row.no_order_reason)
        .collect();

    assert_eq!(
        reasons,
        vec![
            "b5_short_sale_guard".to_string(),
            "already_at_target".to_string(),
            "order_will_be_submitted".to_string(),
        ]
    );
}

#[tokio::test]
async fn s10_day_order_count_and_limit_are_wired_values() {
    let _lock = env_lock().lock().await;
    let _env = no_runtime_config_env();
    let st = bare_state();
    st.set_symbol_day_order_count_for_test("AAPL", 7);
    st.set_per_symbol_day_order_limit_for_test(Some(9));
    st.record_per_symbol_target_state(target_state("AAPL", "s1", 0, 1, "order_will_be_submitted"))
        .await;

    let mut rows = request_summary(st).await.per_symbol;
    let row = rows.remove(0);

    assert_eq!(row.day_order_count, 7);
    assert_eq!(row.day_order_limit, Some(9));
}

#[tokio::test]
async fn s11_bar_staleness_secs_is_option_and_not_fabricated() {
    let _lock = env_lock().lock().await;
    let _env = no_runtime_config_env();
    let st = bare_state();
    st.record_per_symbol_target_state(target_state("AAPL", "s1", 0, 1, "order_will_be_submitted"))
        .await;

    let mut rows = request_summary(st).await.per_symbol;
    let row = rows.remove(0);

    assert_eq!(row.bar_staleness_secs, None);
}

#[tokio::test]
async fn s12_route_handler_is_read_only_and_needs_no_db_persistence() {
    let _lock = env_lock().lock().await;
    let _env = no_runtime_config_env();
    let st = bare_state();
    assert!(st.db.is_none(), "precondition: no DB pool configured");
    st.record_per_symbol_target_state(target_state("AAPL", "s1", 0, 1, "order_will_be_submitted"))
        .await;

    let response = request_summary(st).await;
    assert_eq!(response.per_symbol.len(), 1);

    let strategy_rs = include_str!("../src/routes/strategy.rs");
    let start = strategy_rs
        .find("pub(crate) async fn multi_symbol_dispatch_summary")
        .expect("summary handler exists");
    let end = strategy_rs[start..]
        .find("// ---------------------------------------------------------------------------\n// POST /api/v1/strategy/signal")
        .map(|idx| start + idx)
        .expect("handler block end exists");
    let handler_block = &strategy_rs[start..end];
    for forbidden in [
        "mqk_db::",
        "outbox_enqueue",
        "submit_order",
        "cancel_order",
        "replace_order",
        "broker_snapshot.write",
    ] {
        assert!(
            !handler_block.contains(forbidden),
            "summary handler must not contain read/write path: {forbidden}"
        );
    }
}

#[test]
fn s13_no_live_routing_authority_exists_in_response_type() {
    let api_types = include_str!("../src/api_types.rs");
    let start = api_types
        .find("pub struct MultiSymbolDispatchSummaryResponse")
        .expect("summary response type exists");
    let end = api_types[start..]
        .find("// ---------------------------------------------------------------------------")
        .map(|idx| start + idx)
        .expect("response block end exists");
    let block = &api_types[start..end];

    assert!(!block.contains("approved_for_live"));
    assert!(!block.contains("live_routing"));
    assert!(!block.contains("live-routing"));
}

#[test]
fn s14_patch_10_gui_and_oms_expansion_remains_not_started() {
    let api_types = include_str!("../src/api_types.rs");
    let routes_rs = include_str!("../src/routes.rs");

    assert!(!api_types.contains("pub per_symbol_status"));
    assert!(!api_types.contains("pub per_symbol_exposure"));
    assert!(!routes_rs.contains("/api/v1/gui"));
}
