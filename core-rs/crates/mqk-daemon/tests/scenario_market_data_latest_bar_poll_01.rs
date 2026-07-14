//! DATA-PROVIDER-LATEST-BAR-POLL-01: latest closed-bar poll-once route tests.
//!
//! Pure route safety tests require no DB and no network. DB-backed insert/idempotency
//! tests skip truthfully when MQK_DATABASE_URL is not configured.

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
use sqlx::Row;
use tempfile::NamedTempFile;
use tower::ServiceExt;

#[derive(Clone)]
enum FakeLatestOutcome {
    Bar(mqk_md::CanonicalBar),
    Error(mqk_md::MarketDataProviderError),
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
            supported_timeframes: vec![mqk_md::Timeframe::M1, mqk_md::Timeframe::M5],
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
            FakeLatestOutcome::Error(error) => Err(error),
            FakeLatestOutcome::None => Ok(None),
        }
    }
}

fn bar(symbol: &str, end_ts: i64, is_complete: bool) -> mqk_md::CanonicalBar {
    mqk_md::CanonicalBar {
        symbol: symbol.to_string(),
        timeframe: "5m".to_string(),
        end_ts,
        open: "100".to_string(),
        high: "101".to_string(),
        low: "99".to_string(),
        close: "100.5".to_string(),
        volume: 1000,
        is_complete,
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
          "supported_timeframes": ["1m", "5m"],
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

/// Fixture instrument registry carrying one entry so B2.4's canonical
/// provider-symbol resolution has a real mapping to resolve against (never
/// a fabricated/unregistered symbol accepted implicitly).
fn fake_instrument_registry_file(symbol: &str, provider: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("create fake instrument registry");
    let json = format!(
        r#"[{{
          "instrument_id": "equity:US:{symbol}",
          "symbol": "{symbol}",
          "asset_class": "equity",
          "provider": "{provider}",
          "provider_symbol": "{symbol}",
          "venue": "TEST",
          "currency": "USD",
          "enabled": true,
          "timeframes": ["5m"],
          "notes": "test fixture"
        }}]"#
    );
    file.write_all(json.as_bytes())
        .expect("write fake instrument registry");
    file
}

/// General form of [`fake_instrument_registry_file`] — lets tests set a
/// `provider_symbol` distinct from the local `symbol`, a blank
/// `provider_symbol`, a different `provider`, or `enabled: false`.
fn fake_instrument_registry_file_ex(
    symbol: &str,
    provider: &str,
    provider_symbol: &str,
    enabled: bool,
) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("create fake instrument registry");
    let json = format!(
        r#"[{{
          "instrument_id": "equity:US:{symbol}",
          "symbol": "{symbol}",
          "asset_class": "equity",
          "provider": "{provider}",
          "provider_symbol": "{provider_symbol}",
          "venue": "TEST",
          "currency": "USD",
          "enabled": {enabled},
          "timeframes": ["5m"],
          "notes": "test fixture"
        }}]"#
    );
    file.write_all(json.as_bytes())
        .expect("write fake instrument registry");
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
    sqlx::query("delete from md_bars where symbol like 'ZZPOLL%'")
        .execute(&pool)
        .await
        .expect("clean md_bars test rows");
    Some(pool)
}

#[tokio::test]
async fn market_data_dry_run_poll_once_makes_zero_provider_calls_and_zero_db_writes() {
    let registry = fake_registry_file();
    let provider = Arc::new(FakeLatestProvider::new(HashMap::new()));
    let mut st = state::AppState::new();
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLDRY"],
            "timeframe": "5m",
            "dry_run": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "dry_run");
    assert_eq!(body["api_calls_made"], 0);
    assert_eq!(body["inserted_count"], 0);
    assert_eq!(body["updated_count"], 0);
    assert_eq!(provider.calls(), 0);
}

#[tokio::test]
async fn market_data_real_poll_without_allowance_is_refused_before_provider_call() {
    let provider = Arc::new(FakeLatestProvider::new(HashMap::new()));
    let mut st = state::AppState::new();
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLNOALLOW"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": false,
            "now_utc": "2024-01-01T00:10:30Z"
        })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["truth_state"], "refused");
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("allow_provider_api_calls"));
    assert_eq!(provider.calls(), 0);
}

#[tokio::test]
async fn market_data_fake_provider_poll_once_inserts_and_second_poll_is_idempotent() {
    let Some(pool) = maybe_db("market_data_fake_provider_poll_once_inserts").await else {
        return;
    };
    let registry = fake_registry_file();
    let instrument_registry = fake_instrument_registry_file("ZZPOLLINS", "fake");
    let mut outcomes = HashMap::new();
    outcomes.insert(
        "ZZPOLLINS".to_string(),
        FakeLatestOutcome::Bar(bar("ZZPOLLINS", 1_704_067_200, true)),
    );
    let provider = Arc::new(FakeLatestProvider::new(outcomes));
    let mut st = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );
    st.set_latest_bar_provider_client_for_test(provider);
    let app = routes::build_router(Arc::new(st));
    let body = serde_json::json!({
        "provider_id": "fake",
        "symbols": ["ZZPOLLINS"],
        "timeframe": "5m",
        "dry_run": false,
        "allow_provider_api_calls": true,
        "now_utc": "2024-01-01T00:10:30Z",
        "provider_registry_path": registry.path().to_string_lossy(),
        "instrument_registry_path": instrument_registry.path().to_string_lossy()
    });

    let (first_status, first_body) = call_json(
        app.clone(),
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(body.clone()),
    )
    .await;
    let (second_status, second_body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(body),
    )
    .await;

    assert_eq!(first_status, StatusCode::OK);
    assert_eq!(first_body["inserted_count"], 1);
    assert_eq!(second_status, StatusCode::OK);
    assert_eq!(second_body["updated_count"], 1);

    let count: i64 = sqlx::query_scalar(
        "select count(*) from md_bars where symbol = $1 and timeframe = $2 and end_ts = $3",
    )
    .bind("ZZPOLLINS")
    .bind("5m")
    .bind(1_704_067_200_i64)
    .fetch_one(&pool)
    .await
    .expect("count md_bars latest poll row");
    assert_eq!(count, 1);

    let row = sqlx::query(
        "select provider_id, provider_source, provider_symbol, ingest_mode from md_bars where symbol = $1 and timeframe = $2 and end_ts = $3",
    )
    .bind("ZZPOLLINS")
    .bind("5m")
    .bind(1_704_067_200_i64)
    .fetch_one(&pool)
    .await
    .expect("fetch md_bars provider metadata");
    assert_eq!(row.try_get::<String, _>("provider_id").unwrap(), "fake");
    assert_eq!(
        row.try_get::<Option<String>, _>("provider_source").unwrap(),
        Some("fake".to_string())
    );
    assert_eq!(
        row.try_get::<Option<String>, _>("provider_symbol").unwrap(),
        Some("ZZPOLLINS".to_string())
    );
    assert_eq!(
        row.try_get::<Option<String>, _>("ingest_mode").unwrap(),
        Some("latest_poll".to_string())
    );
}

#[tokio::test]
async fn market_data_provider_error_reports_partial_failure_without_hiding_success() {
    let Some(pool) = maybe_db("market_data_provider_error_reports_partial_failure").await else {
        return;
    };
    let registry = fake_registry_file();
    // Repair 2: admission now happens before any provider call, so ZZPOLLERR
    // must also be a real, enabled, "fake"-provider instrument to reach the
    // provider at all — otherwise this test would stop proving a genuine
    // provider-error outcome and would instead (coincidentally, with the
    // same aggregate JSON shape) prove an admission rejection.
    let instrument_registry =
        tempfile::NamedTempFile::new().expect("create fake instrument registry");
    std::fs::write(
        instrument_registry.path(),
        br#"[
          {
            "instrument_id": "equity:US:ZZPOLLOK",
            "symbol": "ZZPOLLOK",
            "asset_class": "equity",
            "provider": "fake",
            "provider_symbol": "ZZPOLLOK",
            "venue": "TEST",
            "currency": "USD",
            "enabled": true,
            "timeframes": ["5m"],
            "notes": "test fixture"
          },
          {
            "instrument_id": "equity:US:ZZPOLLERR",
            "symbol": "ZZPOLLERR",
            "asset_class": "equity",
            "provider": "fake",
            "provider_symbol": "ZZPOLLERR",
            "venue": "TEST",
            "currency": "USD",
            "enabled": true,
            "timeframes": ["5m"],
            "notes": "test fixture"
          }
        ]"#,
    )
    .expect("write fake instrument registry");
    let mut outcomes = HashMap::new();
    outcomes.insert(
        "ZZPOLLOK".to_string(),
        FakeLatestOutcome::Bar(bar("ZZPOLLOK", 1_704_067_200, true)),
    );
    outcomes.insert(
        "ZZPOLLERR".to_string(),
        FakeLatestOutcome::Error(mqk_md::MarketDataProviderError::ProviderUnavailable {
            provider_id: "fake".to_string(),
            message: "test provider outage".to_string(),
        }),
    );
    let provider = Arc::new(FakeLatestProvider::new(outcomes));
    let mut st =
        state::AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    st.set_latest_bar_provider_client_for_test(provider);
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLOK", "ZZPOLLERR"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy(),
            "instrument_registry_path": instrument_registry.path().to_string_lossy()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "partial");
    assert_eq!(body["inserted_count"], 1);
    assert_eq!(body["error_count"], 1);
    assert_eq!(body["symbols"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn market_data_returned_forming_or_future_bar_is_skipped() {
    let Some(pool) = maybe_db("market_data_returned_forming_or_future_bar_is_skipped").await else {
        return;
    };
    let registry = fake_registry_file();
    let instrument_registry = fake_instrument_registry_file("ZZPOLLFUT", "fake");
    let mut outcomes = HashMap::new();
    outcomes.insert(
        "ZZPOLLFUT".to_string(),
        FakeLatestOutcome::Bar(bar("ZZPOLLFUT", 1_704_067_500, false)),
    );
    let provider = Arc::new(FakeLatestProvider::new(outcomes));
    let mut st = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );
    st.set_latest_bar_provider_client_for_test(provider);
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLFUT"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy(),
            "instrument_registry_path": instrument_registry.path().to_string_lossy()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "completed");
    assert_eq!(body["skipped_count"], 1);
    assert_eq!(body["inserted_count"], 0);

    let count: i64 = sqlx::query_scalar("select count(*) from md_bars where symbol = $1")
        .bind("ZZPOLLFUT")
        .fetch_one(&pool)
        .await
        .expect("count skipped future row");
    assert_eq!(count, 0);
}

// ---------------------------------------------------------------------------
// DAILY-DATA-READINESS-01B2-INGEST-TRUTHFULNESS-REPAIR-01 (Repair 2):
// pre-call admission — the canonical instrument/provider mapping must be
// proven before any provider call is made.
// ---------------------------------------------------------------------------

/// An invalid/nonexistent instrument registry path refuses the whole
/// request before any provider call — never `unwrap_or_default()` into an
/// empty registry that silently skips every symbol individually.
#[tokio::test]
async fn market_data_invalid_instrument_registry_path_makes_zero_provider_calls() {
    let Some(pool) = maybe_db("market_data_invalid_instrument_registry_path").await else {
        return;
    };
    let registry = fake_registry_file();
    let provider = Arc::new(FakeLatestProvider::new(HashMap::new()));
    let mut st =
        state::AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLBADREG"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy(),
            "instrument_registry_path": "/nonexistent/path/that/cannot/exist/equities.json"
        })),
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["truth_state"], "refused");
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("instrument_registry_unavailable"));
    assert_eq!(
        provider.calls(),
        0,
        "an unreadable instrument registry must make zero provider calls"
    );
}

/// A local symbol absent from the instrument registry blocks before any
/// provider call.
#[tokio::test]
async fn market_data_unknown_local_symbol_makes_zero_provider_calls() {
    let Some(pool) = maybe_db("market_data_unknown_local_symbol").await else {
        return;
    };
    let registry = fake_registry_file();
    // Registry has an entry, but not for the symbol requested below.
    let instrument_registry = fake_instrument_registry_file("ZZPOLLSOMEOTHER", "fake");
    let provider = Arc::new(FakeLatestProvider::new(HashMap::new()));
    let mut st =
        state::AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLUNKNOWN"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy(),
            "instrument_registry_path": instrument_registry.path().to_string_lossy()
        })),
    )
    .await;

    // The sole requested symbol fails admission, so the aggregate response
    // truthfully reports `failed` (mirrors the existing `provider_error`
    // single-symbol-failure precedent elsewhere in this file).
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["truth_state"], "failed");
    assert_eq!(
        body["symbols"][0]["status"],
        "skipped_instrument_not_in_registry"
    );
    assert_eq!(
        provider.calls(),
        0,
        "an unregistered local symbol must make zero provider calls"
    );
}

/// A disabled instrument blocks before any provider call.
#[tokio::test]
async fn market_data_disabled_instrument_makes_zero_provider_calls() {
    let Some(pool) = maybe_db("market_data_disabled_instrument").await else {
        return;
    };
    let registry = fake_registry_file();
    let instrument_registry =
        fake_instrument_registry_file_ex("ZZPOLLDISABLED", "fake", "ZZPOLLDISABLED", false);
    let provider = Arc::new(FakeLatestProvider::new(HashMap::new()));
    let mut st =
        state::AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLDISABLED"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy(),
            "instrument_registry_path": instrument_registry.path().to_string_lossy()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["truth_state"], "failed");
    assert_eq!(body["symbols"][0]["status"], "skipped_instrument_disabled");
    assert_eq!(
        provider.calls(),
        0,
        "a disabled instrument must make zero provider calls"
    );
}

/// A canonical provider mismatch (instrument configured for a different
/// provider than the poll's selected provider) blocks before any provider
/// call.
#[tokio::test]
async fn market_data_provider_id_mismatch_makes_zero_provider_calls() {
    let Some(pool) = maybe_db("market_data_provider_id_mismatch").await else {
        return;
    };
    let registry = fake_registry_file();
    let instrument_registry =
        fake_instrument_registry_file("ZZPOLLWRONGPROV", "some_other_provider");
    let provider = Arc::new(FakeLatestProvider::new(HashMap::new()));
    let mut st =
        state::AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLWRONGPROV"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy(),
            "instrument_registry_path": instrument_registry.path().to_string_lossy()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["truth_state"], "failed");
    assert_eq!(body["symbols"][0]["status"], "skipped_provider_mismatch");
    assert_eq!(
        provider.calls(),
        0,
        "a provider-id mismatch must make zero provider calls"
    );
}

/// A blank canonical `provider_symbol` blocks before any provider call.
#[tokio::test]
async fn market_data_blank_provider_symbol_makes_zero_provider_calls() {
    let Some(pool) = maybe_db("market_data_blank_provider_symbol").await else {
        return;
    };
    let registry = fake_registry_file();
    // DAILY-DATA-READINESS-01B2: a blank `provider_symbol` is now a
    // registry-level `validate_registry` violation (Repair 1), so the whole
    // registry is refused before any per-symbol admission check ever runs —
    // this instrument no longer reaches the (now unreachable for this case)
    // `skipped_blank_provider_symbol` per-symbol outcome.
    let instrument_registry = fake_instrument_registry_file_ex("ZZPOLLBLANKPS", "fake", "", true);
    let provider = Arc::new(FakeLatestProvider::new(HashMap::new()));
    let mut st =
        state::AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLBLANKPS"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy(),
            "instrument_registry_path": instrument_registry.path().to_string_lossy()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["truth_state"], "refused");
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("empty provider_symbol") || err.contains("instrument_registry_invalid"),
        "error must describe the registry validation failure: {body}"
    );
    assert_eq!(
        provider.calls(),
        0,
        "a blank canonical provider_symbol must make zero provider calls"
    );
}

/// When the canonical `provider_symbol` differs from the local symbol, the
/// request sent to the provider uses the provider symbol — never the local
/// symbol — and the accepted bar is stored under the local symbol with the
/// provider symbol preserved as provenance.
#[tokio::test]
async fn market_data_distinct_provider_symbol_is_sent_and_local_symbol_is_stored() {
    let Some(pool) = maybe_db("market_data_distinct_provider_symbol").await else {
        return;
    };
    let registry = fake_registry_file();
    let instrument_registry =
        fake_instrument_registry_file_ex("ZZPOLLLOCAL", "fake", "PROVIDERSIDE-ZZPOLLLOCAL", true);
    let mut outcomes = HashMap::new();
    // Keyed by the *provider* symbol — proves the request was sent using
    // `instrument.provider_symbol`, not the local canonical symbol.
    outcomes.insert(
        "PROVIDERSIDE-ZZPOLLLOCAL".to_string(),
        FakeLatestOutcome::Bar(bar("PROVIDERSIDE-ZZPOLLLOCAL", 1_704_067_200, true)),
    );
    let provider = Arc::new(FakeLatestProvider::new(outcomes));
    let mut st = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLLOCAL"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy(),
            "instrument_registry_path": instrument_registry.path().to_string_lossy()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "completed");
    assert_eq!(body["inserted_count"], 1);
    assert_eq!(
        provider.calls(),
        1,
        "exactly one real provider call must have been made"
    );

    let row = sqlx::query(
        "select symbol, provider_id, provider_symbol from md_bars \
         where symbol = 'ZZPOLLLOCAL' and timeframe = '5m' and end_ts = $1",
    )
    .bind(1_704_067_200_i64)
    .fetch_one(&pool)
    .await
    .expect("fetch stored bar");
    assert_eq!(
        row.try_get::<String, _>("symbol").unwrap(),
        "ZZPOLLLOCAL",
        "md_bars.symbol must be the canonical LOCAL symbol, never the provider symbol"
    );
    assert_eq!(row.try_get::<String, _>("provider_id").unwrap(), "fake");
    assert_eq!(
        row.try_get::<Option<String>, _>("provider_symbol").unwrap(),
        Some("PROVIDERSIDE-ZZPOLLLOCAL".to_string()),
        "provenance must retain the canonical provider symbol"
    );

    sqlx::query("delete from md_bars where symbol = 'ZZPOLLLOCAL'")
        .execute(&pool)
        .await
        .ok();
}

/// A bar returned under a symbol other than the one actually requested
/// (the canonical provider symbol) is rejected — never accepted under a
/// mismatched label, and never written to the DB.
#[tokio::test]
async fn market_data_wrong_returned_provider_symbol_is_rejected_and_writes_nothing() {
    let Some(pool) = maybe_db("market_data_wrong_returned_provider_symbol").await else {
        return;
    };
    let registry = fake_registry_file();
    let instrument_registry =
        fake_instrument_registry_file_ex("ZZPOLLMISMATCH", "fake", "ZZPOLLMISMATCH", true);
    let mut outcomes = HashMap::new();
    // The fake provider is queried with "ZZPOLLMISMATCH" but returns a bar
    // labeled under a completely different symbol.
    outcomes.insert(
        "ZZPOLLMISMATCH".to_string(),
        FakeLatestOutcome::Bar(bar("SOME_OTHER_SYMBOL", 1_704_067_200, true)),
    );
    let provider = Arc::new(FakeLatestProvider::new(outcomes));
    let mut st = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLMISMATCH"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy(),
            "instrument_registry_path": instrument_registry.path().to_string_lossy()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["inserted_count"], 0);
    assert_eq!(
        body["symbols"][0]["status"],
        "skipped_unclosed_or_unexpected_bar"
    );
    assert_eq!(
        provider.calls(),
        1,
        "the provider is called once (admission passed); the mismatch is caught on the response"
    );

    let count: i64 = sqlx::query_scalar(
        "select count(*) from md_bars where symbol = $1 or symbol = 'SOME_OTHER_SYMBOL'",
    )
    .bind("ZZPOLLMISMATCH")
    .fetch_one(&pool)
    .await
    .expect("count rows for mismatched response");
    assert_eq!(count, 0, "a wrong-symbol response must write zero DB bars");
}

// ---------------------------------------------------------------------------
// DAILY-DATA-READINESS-01B2-REGISTRY-ADMISSION-CLOSURE-01:
// registry validation + per-instrument timeframe admission for latest-bar
// polling.
// ---------------------------------------------------------------------------

/// A registry with two entries sharing the same local `symbol` — parseable
/// JSON, but rejected by `validate_registry` (duplicate symbol).
fn duplicate_symbol_instrument_registry_json(symbol: &str, provider: &str) -> String {
    format!(
        r#"[
          {{
            "instrument_id": "equity:US:{symbol}A",
            "symbol": "{symbol}",
            "asset_class": "equity",
            "provider": "{provider}",
            "provider_symbol": "{symbol}",
            "venue": "TEST",
            "currency": "USD",
            "enabled": true,
            "timeframes": ["5m"],
            "notes": "duplicate local symbol fixture 1"
          }},
          {{
            "instrument_id": "equity:US:{symbol}B",
            "symbol": "{symbol}",
            "asset_class": "equity",
            "provider": "{provider}",
            "provider_symbol": "{symbol}B",
            "venue": "TEST",
            "currency": "USD",
            "enabled": true,
            "timeframes": ["5m"],
            "notes": "duplicate local symbol fixture 2"
          }}
        ]"#
    )
}

/// A registry with two distinct local symbols that share the same enabled
/// `provider_symbol` for the same `provider` — parseable JSON, but rejected
/// by `validate_registry` (duplicate enabled provider_symbol).
fn duplicate_provider_symbol_instrument_registry_json(
    provider: &str,
    provider_symbol: &str,
) -> String {
    format!(
        r#"[
          {{
            "instrument_id": "equity:US:{provider_symbol}LOCALA",
            "symbol": "{provider_symbol}LOCALA",
            "asset_class": "equity",
            "provider": "{provider}",
            "provider_symbol": "{provider_symbol}",
            "venue": "TEST",
            "currency": "USD",
            "enabled": true,
            "timeframes": ["5m"],
            "notes": "duplicate provider_symbol fixture 1"
          }},
          {{
            "instrument_id": "equity:US:{provider_symbol}LOCALB",
            "symbol": "{provider_symbol}LOCALB",
            "asset_class": "equity",
            "provider": "{provider}",
            "provider_symbol": "{provider_symbol}",
            "venue": "TEST",
            "currency": "USD",
            "enabled": true,
            "timeframes": ["5m"],
            "notes": "duplicate provider_symbol fixture 2"
          }}
        ]"#
    )
}

/// Like [`fake_instrument_registry_file_ex`] but lets tests set an explicit
/// `timeframes` list, for proving per-instrument timeframe admission.
fn fake_instrument_registry_file_with_timeframes(
    symbol: &str,
    provider: &str,
    timeframes: &[&str],
) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("create fake instrument registry");
    let timeframes_json = serde_json::to_string(timeframes).expect("serialize timeframes");
    let json = format!(
        r#"[{{
          "instrument_id": "equity:US:{symbol}",
          "symbol": "{symbol}",
          "asset_class": "equity",
          "provider": "{provider}",
          "provider_symbol": "{symbol}",
          "venue": "TEST",
          "currency": "USD",
          "enabled": true,
          "timeframes": {timeframes_json},
          "notes": "test fixture"
        }}]"#
    );
    file.write_all(json.as_bytes())
        .expect("write fake instrument registry");
    file
}

/// A parseable-but-invalid (duplicate local symbol) instrument registry
/// refuses the poll before any provider call.
#[tokio::test]
async fn market_data_duplicate_local_symbols_refuse_poll_before_any_provider_call() {
    let Some(pool) = maybe_db("market_data_duplicate_local_symbols").await else {
        return;
    };
    let registry = fake_registry_file();
    let instrument_registry =
        tempfile::NamedTempFile::new().expect("create fake instrument registry");
    std::fs::write(
        instrument_registry.path(),
        duplicate_symbol_instrument_registry_json("ZZPOLLDUPSYM", "fake").as_bytes(),
    )
    .expect("write fake instrument registry");
    let provider = Arc::new(FakeLatestProvider::new(HashMap::new()));
    let mut st =
        state::AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLDUPSYM"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy(),
            "instrument_registry_path": instrument_registry.path().to_string_lossy()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["truth_state"], "refused");
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("duplicate") || err.contains("instrument_registry_invalid"),
        "error must describe the registry validation failure: {body}"
    );
    assert_eq!(
        provider.calls(),
        0,
        "a duplicate-symbol registry must make zero provider calls"
    );
}

/// A parseable-but-invalid (duplicate enabled provider_symbol) instrument
/// registry refuses the poll before any provider call.
#[tokio::test]
async fn market_data_duplicate_provider_symbols_refuse_poll_before_any_provider_call() {
    let Some(pool) = maybe_db("market_data_duplicate_provider_symbols").await else {
        return;
    };
    let registry = fake_registry_file();
    let instrument_registry =
        tempfile::NamedTempFile::new().expect("create fake instrument registry");
    std::fs::write(
        instrument_registry.path(),
        duplicate_provider_symbol_instrument_registry_json("fake", "ZZPOLLDUPPS").as_bytes(),
    )
    .expect("write fake instrument registry");
    let provider = Arc::new(FakeLatestProvider::new(HashMap::new()));
    let mut st =
        state::AppState::new_with_db_and_operator_auth(pool, OperatorAuthMode::ExplicitDevNoToken);
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLDUPPSLOCALA"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy(),
            "instrument_registry_path": instrument_registry.path().to_string_lossy()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["truth_state"], "refused");
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("duplicate") || err.contains("instrument_registry_invalid"),
        "error must describe the registry validation failure: {body}"
    );
    assert_eq!(
        provider.calls(),
        0,
        "a duplicate-provider_symbol registry must make zero provider calls"
    );
}

/// An instrument whose `timeframes` list does not include the requested
/// timeframe blocks admission before any provider call, makes zero API
/// calls, and writes zero DB bars.
#[tokio::test]
async fn market_data_instrument_timeframe_not_authorized_blocks_poll_before_provider_call() {
    let Some(pool) = maybe_db("market_data_instrument_timeframe_not_authorized").await else {
        return;
    };
    let registry = fake_registry_file();
    let instrument_registry =
        fake_instrument_registry_file_with_timeframes("ZZPOLLTFNO", "fake", &["1D"]);
    let provider = Arc::new(FakeLatestProvider::new(HashMap::new()));
    let mut st = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLTFNO"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy(),
            "instrument_registry_path": instrument_registry.path().to_string_lossy()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["truth_state"], "failed");
    assert_eq!(
        body["symbols"][0]["status"],
        "skipped_instrument_timeframe_unsupported"
    );
    assert_eq!(
        body["api_calls_made"], 0,
        "api_calls_made must remain zero on instrument-timeframe rejection: {body}"
    );
    assert_eq!(
        provider.calls(),
        0,
        "an instrument not authorized for the requested timeframe must make zero provider calls"
    );

    let count: i64 = sqlx::query_scalar("select count(*) from md_bars where symbol = $1")
        .bind("ZZPOLLTFNO")
        .fetch_one(&pool)
        .await
        .expect("count rows for timeframe-rejected symbol");
    assert_eq!(count, 0, "no DB bar must be written on rejection");
}

/// An instrument whose `timeframes` list includes the requested timeframe
/// (alongside another timeframe) is admitted and polled normally.
#[tokio::test]
async fn market_data_instrument_timeframe_authorized_permits_poll() {
    let Some(pool) = maybe_db("market_data_instrument_timeframe_authorized").await else {
        return;
    };
    let registry = fake_registry_file();
    let instrument_registry =
        fake_instrument_registry_file_with_timeframes("ZZPOLLTFYES", "fake", &["1D", "5m"]);
    let mut outcomes = HashMap::new();
    outcomes.insert(
        "ZZPOLLTFYES".to_string(),
        FakeLatestOutcome::Bar(bar("ZZPOLLTFYES", 1_704_067_200, true)),
    );
    let provider = Arc::new(FakeLatestProvider::new(outcomes));
    let mut st = state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        OperatorAuthMode::ExplicitDevNoToken,
    );
    st.set_latest_bar_provider_client_for_test(provider.clone());
    let app = routes::build_router(Arc::new(st));

    let (status, body) = call_json(
        app,
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLTFYES"],
            "timeframe": "5m",
            "dry_run": false,
            "allow_provider_api_calls": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy(),
            "instrument_registry_path": instrument_registry.path().to_string_lossy()
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "completed");
    assert_eq!(body["inserted_count"], 1);
    assert_eq!(
        provider.calls(),
        1,
        "an instrument whose timeframes list authorizes the requested timeframe must be polled"
    );

    sqlx::query("delete from md_bars where symbol = 'ZZPOLLTFYES'")
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
async fn market_data_feed_status_returns_last_poll_state() {
    let registry = fake_registry_file();
    let st = state::AppState::new();
    let app = routes::build_router(Arc::new(st));

    let (initial_status, initial_body) =
        call_json(app.clone(), "GET", "/api/v1/market-data/feed/status", None).await;
    assert_eq!(initial_status, StatusCode::OK);
    assert_eq!(initial_body["truth_state"], "no_poll");

    let (_poll_status, _poll_body) = call_json(
        app.clone(),
        "POST",
        "/api/v1/market-data/feed/poll-once",
        Some(serde_json::json!({
            "provider_id": "fake",
            "symbols": ["ZZPOLLSTATUS"],
            "timeframe": "5m",
            "dry_run": true,
            "now_utc": "2024-01-01T00:10:30Z",
            "provider_registry_path": registry.path().to_string_lossy()
        })),
    )
    .await;
    let (status, body) = call_json(app, "GET", "/api/v1/market-data/feed/status", None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "active");
    assert_eq!(body["last_poll"]["truth_state"], "dry_run");
    assert_eq!(body["last_poll"]["provider_id"], "fake");
}
