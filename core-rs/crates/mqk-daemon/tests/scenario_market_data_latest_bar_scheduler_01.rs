//! DATA-PROVIDER-LATEST-BAR-SCHEDULER-01: latest closed-bar scheduler route tests.
//!
//! Tests use injected fake providers and temporary provider registries. The
//! DB-backed immediate-poll proof skips truthfully unless MQK_DATABASE_URL is set.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::state::OperatorAuthMode;
use mqk_daemon::{routes, state};
use tempfile::NamedTempFile;
use tower::ServiceExt;

#[derive(Clone)]
enum FakeLatestOutcome {
    Bar(mqk_md::CanonicalBar),
    None,
}

struct FakeLatestProvider {
    outcomes: Mutex<HashMap<String, FakeLatestOutcome>>,
    calls: AtomicUsize,
}

impl FakeLatestProvider {
    fn new(outcomes: HashMap<String, FakeLatestOutcome>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes),
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl mqk_md::MarketDataProvider for FakeLatestProvider {
    fn provider_id(&self) -> &str {
        "fake"
    }

    fn display_name(&self) -> &str {
        "Fake Latest Provider"
    }

    fn capabilities(&self) -> mqk_md::MarketDataProviderCapabilities {
        mqk_md::MarketDataProviderCapabilities {
            historical_bars: false,
            latest_closed_bar: true,
            completed_bar_stream: false,
            supported_asset_classes: vec![mqk_md::ProviderAssetClass::Equity],
            supported_timeframes: vec![
                mqk_md::Timeframe::M1,
                mqk_md::Timeframe::M5,
                mqk_md::Timeframe::M15,
                mqk_md::Timeframe::H1,
                mqk_md::Timeframe::D1,
            ],
        }
    }

    fn health(&self) -> mqk_md::MarketDataProviderHealth {
        mqk_md::MarketDataProviderHealth::unknown()
    }

    fn rate_limits(&self) -> Option<mqk_md::MarketDataProviderRateLimits> {
        None
    }

    async fn fetch_historical_bars(
        &self,
        _request: mqk_md::HistoricalBarsRequest,
    ) -> Result<Vec<mqk_md::CanonicalBar>, mqk_md::MarketDataProviderError> {
        Err(mqk_md::MarketDataProviderError::UnsupportedCapability {
            provider_id: "fake".to_string(),
            capability: "historical_bars".to_string(),
        })
    }

    async fn fetch_latest_closed_bar(
        &self,
        request: mqk_md::LatestClosedBarRequest,
    ) -> Result<Option<mqk_md::CanonicalBar>, mqk_md::MarketDataProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self
            .outcomes
            .lock()
            .expect("fake latest provider outcomes lock poisoned")
            .get(&request.symbol)
            .cloned()
            .unwrap_or(FakeLatestOutcome::None)
        {
            FakeLatestOutcome::Bar(bar) => Ok(Some(bar)),
            FakeLatestOutcome::None => Ok(None),
        }
    }
}

fn bar(symbol: &str, timeframe: &str, end_ts: i64) -> mqk_md::CanonicalBar {
    mqk_md::CanonicalBar {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        end_ts,
        open: "100".to_string(),
        high: "101".to_string(),
        low: "99".to_string(),
        close: "100.5".to_string(),
        volume: 1000,
        is_complete: true,
    }
}

fn fake_registry_file() -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("create fake provider registry");
    file.write_all(
        br#"[{
          "provider_id": "fake",
          "display_name": "Fake",
          "asset_classes": ["equity"],
          "free_tier_available": true,
          "api_key_required": false,
          "credential_env_vars": [],
          "rate_limit_notes": "test",
          "supported_timeframes": ["1D", "1h", "1m", "5m", "15m"],
          "historical_depth_notes": "test",
          "realtime_support_notes": "test",
          "licensing_notes": "test",
          "implementation_status": "implemented_equity_provider",
          "enabled": true,
          "verification_status": "repo_implemented_official_limits_unverified",
          "docs_url": ""
        }]"#,
    )
    .expect("write fake provider registry");
    file
}

async fn call_json(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let req = builder
        .body(axum::body::Body::from(
            body.map(|value| value.to_string()).unwrap_or_default(),
        ))
        .expect("build request");
    let resp = app.oneshot(req).await.expect("route response");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("read response body")
        .to_bytes();
    let json = serde_json::from_slice(&bytes).expect("json response");
    (status, json)
}

async fn maybe_db(label: &str) -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            eprintln!("{label}: skipped DB-backed proof because MQK_DATABASE_URL is not set");
            return None;
        }
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connect MQK_DATABASE_URL");
    mqk_db::migrate(&pool).await.expect("run migrations");
    sqlx::query("delete from md_bars where symbol like 'ZZSCHED%'")
        .execute(&pool)
        .await
        .expect("clean scheduler md_bars test rows");
    Some(pool)
}

#[tokio::test]
async fn scheduler_dry_run_start_makes_no_provider_calls_and_schedules_next_poll() {
    let registry = fake_registry_file();
    let provider = Arc::new(FakeLatestProvider::new(HashMap::new()));
    let mut st = state::AppState::new();
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app.clone(),
        "POST",
        "/api/v1/market-data/feed/scheduler/start",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZSCHEDDRY"],
            "timeframe": "15m",
            "dry_run": true,
            "poll_immediately": false,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "started");
    assert_eq!(body["running"], true);
    assert_eq!(body["provider_id"], "fake");
    assert_eq!(body["timeframe"], "15m");
    assert_eq!(body["poll_count"], 0);
    assert_eq!(body["next_poll_utc"], "2024-01-01T00:15:00+00:00");
    assert_eq!(
        body["latest_expected_closed_bar_utc"],
        "2024-01-01T00:00:00+00:00"
    );
    assert_eq!(provider.calls(), 0);

    let (stop_status, stop_body) =
        call_json(app, "POST", "/api/v1/market-data/feed/scheduler/stop", None).await;
    assert_eq!(stop_status, StatusCode::OK);
    assert_eq!(stop_body["running"], false);
}

#[tokio::test]
async fn scheduler_real_start_without_allowance_is_refused_before_provider_call() {
    let provider = Arc::new(FakeLatestProvider::new(HashMap::new()));
    let mut st = state::AppState::new();
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/scheduler/start",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZSCHEDNOALLOW"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": false,
            "poll_immediately": true,
            "now_utc": "2024-01-01T00:10:30Z"
        })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["truth_state"], "refused");
    assert!(body["last_error"]
        .as_str()
        .unwrap()
        .contains("allow_provider_api_calls"));
    assert_eq!(provider.calls(), 0);
}

#[tokio::test]
async fn scheduler_fake_provider_poll_immediately_invokes_poll_once_once() {
    let Some(pool) = maybe_db("scheduler_fake_provider_poll_immediately").await else {
        return;
    };
    let registry = fake_registry_file();
    let mut outcomes = HashMap::new();
    outcomes.insert(
        "ZZSCHEDPOLL".to_string(),
        FakeLatestOutcome::Bar(bar("ZZSCHEDPOLL", "5m", 1_704_067_200)),
    );
    let provider = Arc::new(FakeLatestProvider::new(outcomes));
    let mut st =
        state::AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app.clone(),
        "POST",
        "/api/v1/market-data/feed/scheduler/start",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZSCHEDPOLL"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "poll_immediately": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["running"], true);
    assert_eq!(body["poll_count"], 1);
    assert_eq!(body["last_poll_utc"], "2024-01-01T00:10:30+00:00");
    assert_eq!(body["last_result"]["truth_state"], "completed");
    assert_eq!(body["last_result"]["api_calls_made"], 1);
    assert_eq!(provider.calls(), 1);

    let (stop_status, stop_body) =
        call_json(app, "POST", "/api/v1/market-data/feed/scheduler/stop", None).await;
    assert_eq!(stop_status, StatusCode::OK);
    assert_eq!(stop_body["running"], false);
}

#[tokio::test]
async fn scheduler_second_start_is_refused_while_running() {
    let registry = fake_registry_file();
    let st = state::AppState::new();
    let app = routes::build_router(Arc::new(st));
    let body = serde_json::json!({
        "provider_id": "fake",
        "symbols": ["ZZSCHEDSECOND"],
        "timeframe": "5m",
        "dry_run": true,
        "poll_immediately": false,
        "now_utc": "2024-01-01T00:10:30Z",
        "provider_registry_path": registry.path().to_string_lossy()
    });

    let (first_status, first_body) = call_json(
        app.clone(),
        "POST",
        "/api/v1/market-data/feed/scheduler/start",
        Some(body.clone()),
    )
    .await;
    let (second_status, second_body) = call_json(
        app.clone(),
        "POST",
        "/api/v1/market-data/feed/scheduler/start",
        Some(body),
    )
    .await;

    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(first_body["running"], true);
    assert_eq!(second_status, StatusCode::CONFLICT);
    assert_eq!(second_body["truth_state"], "already_running");
    assert_eq!(second_body["running"], true);

    let _ = call_json(app, "POST", "/api/v1/market-data/feed/scheduler/stop", None).await;
}

#[tokio::test]
async fn scheduler_stop_is_idempotent() {
    let st = state::AppState::new();
    let app = routes::build_router(Arc::new(st));

    let (first_status, first_body) = call_json(
        app.clone(),
        "POST",
        "/api/v1/market-data/feed/scheduler/stop",
        None,
    )
    .await;
    let (second_status, second_body) =
        call_json(app, "POST", "/api/v1/market-data/feed/scheduler/stop", None).await;

    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(first_body["truth_state"], "not_running");
    assert_eq!(first_body["running"], false);
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(second_body["truth_state"], "not_running");
    assert_eq!(second_body["running"], false);
}

#[tokio::test]
async fn scheduler_status_reports_last_poll_and_next_poll() {
    let registry = fake_registry_file();
    let st = state::AppState::new();
    let app = routes::build_router(Arc::new(st));

    let (_start_status, _start_body) = call_json(
        app.clone(),
        "POST",
        "/api/v1/market-data/feed/scheduler/start",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZSCHEDSTATUS"],
            "timeframe": "5m",
            "dry_run": true,
            "poll_immediately": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy()
        })),
    )
    .await;
    let (status, body) = call_json(
        app.clone(),
        "GET",
        "/api/v1/market-data/feed/scheduler/status",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "running");
    assert_eq!(body["running"], true);
    assert_eq!(body["last_poll_utc"], "2024-01-01T00:10:30+00:00");
    assert_eq!(body["next_poll_utc"], "2024-01-01T00:15:00+00:00");
    assert_eq!(body["last_result"]["truth_state"], "dry_run");
    assert_eq!(body["last_result"]["api_calls_made"], 0);

    let _ = call_json(app, "POST", "/api/v1/market-data/feed/scheduler/stop", None).await;
}
