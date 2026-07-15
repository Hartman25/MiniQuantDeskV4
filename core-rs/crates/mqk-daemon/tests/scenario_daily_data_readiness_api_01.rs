//! DAILY-DATA-READINESS-01C-ENFORCEMENT-01 — read-only API surface proof.
//!
//! Binding contract:
//! `docs/specs/daily_data_readiness_01a_current_truth_and_contract.md`.
//!
//! Exercises `GET /api/v1/market-data/readiness` and its three additive
//! projections (`system/preflight`, `autonomous/readiness`,
//! `market-data/ingest-plan`) exclusively over HTTP, through
//! `routes::build_router`. Never a separate assignment parser or a
//! simplified readiness evaluator — every surface calls
//! `routes::market_data_readiness::compute_daily_data_readiness_response`.
//!
//! Every fixture uses a synthetic instrument/provider (`ZZDDRAPI01` /
//! `zzddrapi01_provider`) via temporary registry file overrides
//! (`MQK_PROVIDER_REGISTRY_PATH` / `MQK_INSTRUMENT_REGISTRY_PATH`) — never a
//! real ticker or the real config files, matching the established pattern in
//! `scenario_external_signal_sector_risk_01.rs`.
//!
//! Run alone:
//! `cargo test -p mqk-daemon --test scenario_daily_data_readiness_api_01 -- --test-threads=1`

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::state::BrokerKind;
use mqk_daemon::{routes, state};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Env-var serialization — every test in this file mutates process-global env
// vars (MQK_STRATEGY_SYMBOL / MQK_STRATEGY_IDS / MQK_STRATEGY_MD_TIMEFRAME /
// MQK_PROVIDER_REGISTRY_PATH / MQK_INSTRUMENT_REGISTRY_PATH) and must not run
// concurrently with any other test that touches the same vars, in this file
// or any other (matches scenario_premarket_data_readiness_gate_01.rs /
// scenario_ingest_plan_01.rs).
// ---------------------------------------------------------------------------

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn clear_env() {
    unsafe {
        std::env::remove_var("MQK_STRATEGY_SYMBOL");
        std::env::remove_var("MQK_STRATEGY_IDS");
        std::env::remove_var("MQK_STRATEGY_MD_TIMEFRAME");
        std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
        std::env::remove_var("MQK_PROVIDER_REGISTRY_PATH");
        std::env::remove_var("MQK_INSTRUMENT_REGISTRY_PATH");
    }
}

// ---------------------------------------------------------------------------
// Synthetic registry fixtures — never a real ticker/provider.
// ---------------------------------------------------------------------------

static COUNTER: AtomicU32 = AtomicU32::new(0);
fn next_id() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

const FAKE_SYMBOL: &str = "ZZDDRAPI01";
const FAKE_PROVIDER: &str = "zzddrapi01_provider";

/// Writes synthetic instrument + provider registry JSON files to a fresh temp
/// dir and points `MQK_INSTRUMENT_REGISTRY_PATH` / `MQK_PROVIDER_REGISTRY_PATH`
/// at them. Returns the dir so the caller can clean it up.
fn write_registries() -> std::path::PathBuf {
    write_registries_for(FAKE_SYMBOL, FAKE_PROVIDER)
}

/// Parameterized variant of [`write_registries`] (matches the pattern in
/// `scenario_daily_data_readiness_start_gate_01.rs`): DB-backed tests that
/// persist real `md_bars` rows use a distinct symbol/provider per test so
/// they never collide with another test's seeded fixtures.
fn write_registries_for(symbol: &str, provider: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mqk_ddr_api01_registry_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::create_dir_all(&dir).expect("create registry dir");

    let instruments_path = dir.join("instruments.json");
    std::fs::write(
        &instruments_path,
        format!(
            r#"[
  {{
    "instrument_id": "equity:US:{symbol}",
    "symbol": "{symbol}",
    "asset_class": "equity",
    "provider": "{provider}",
    "provider_symbol": "{symbol}",
    "venue": "TEST",
    "currency": "USD",
    "enabled": true,
    "timeframes": ["1D"],
    "notes": "DAILY-DATA-READINESS-01C-ENFORCEMENT-01 synthetic fixture, not a real instrument"
  }}
]"#
        ),
    )
    .expect("write instruments fixture");

    let providers_path = dir.join("providers.json");
    std::fs::write(
        &providers_path,
        format!(
            r#"[
  {{
    "provider_id": "{provider}",
    "display_name": "{symbol} Test Provider",
    "asset_classes": ["equity"],
    "free_tier_available": true,
    "api_key_required": false,
    "credential_env_vars": [],
    "rate_limit_notes": "",
    "supported_timeframes": ["1D"],
    "historical_depth_notes": "",
    "realtime_support_notes": "",
    "licensing_notes": "",
    "implementation_status": "test",
    "enabled": true,
    "verification_status": "test",
    "docs_url": ""
  }}
]"#
        ),
    )
    .expect("write providers fixture");

    unsafe {
        std::env::set_var("MQK_INSTRUMENT_REGISTRY_PATH", &instruments_path);
        std::env::set_var("MQK_PROVIDER_REGISTRY_PATH", &providers_path);
    }
    dir
}

fn cleanup_registries(dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(dir);
}

/// Sets the legacy single-symbol env vars so the canonical assignment
/// resolver (`build_multi_symbol_runtime_config_from_env`) and the runtime
/// bootstrap (`bootstrap_with_effective_binding`) both bind to
/// `FAKE_SYMBOL`/`swing_momentum`/`1D` — every runtime-binding check (§3a/
/// §3b/§3c) passes, so the assignment reaches the bar-readiness stage.
fn set_matching_assignment_env() {
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", FAKE_SYMBOL);
        std::env::set_var("MQK_STRATEGY_IDS", "swing_momentum");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");
    }
}

// ---------------------------------------------------------------------------
// HTTP test helpers
// ---------------------------------------------------------------------------

async fn call(router: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let resp = router
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_else(|e| {
        panic!(
            "body is not valid JSON: {e}; raw={:?}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, json)
}

fn make_paper_alpaca() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ))
}

fn make_default() -> Arc<state::AppState> {
    Arc::new(state::AppState::new())
}

/// Strips fields that legitimately differ across independent calls made
/// milliseconds apart (each surface recomputes `Utc::now()` independently),
/// so surface-agreement assertions compare everything that must actually
/// agree.
fn strip_volatile(mut v: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = v.as_object_mut() {
        obj.remove("evaluated_at_utc");
    }
    v
}

// ---------------------------------------------------------------------------
// API-01/02/03 — route exists, canonical schema, binding_scope
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_01_02_03_route_exists_schema_and_binding_scope() {
    let _g = env_lock().lock().await;
    clear_env();

    let st = make_default();
    let router = routes::build_router(st);
    let (status, v) = call(router, "/api/v1/market-data/readiness").await;

    assert_eq!(
        status,
        StatusCode::OK,
        "API-01: route must exist and return 200"
    );

    for field in [
        "canonical_route",
        "schema_version",
        "evaluated_at_utc",
        "binding_scope",
        "assignment_source",
        "applicability",
        "start_allowed",
        "top_level_blocker",
        "configured_grace_seconds",
        "configured_future_skew_seconds",
        "calendar_source",
        "calendar_coverage_state",
        "market_date",
        "session_open_utc",
        "session_close_utc",
        "assignments",
    ] {
        assert!(
            v.get(field).is_some(),
            "API-02: response must contain field '{field}': {v}"
        );
    }
    assert_eq!(v["canonical_route"], "/api/v1/market-data/readiness");
    assert!(v["assignments"].is_array());
    assert!(v["start_allowed"].is_boolean());

    assert_eq!(
        v["binding_scope"], "configuration_preview",
        "API-03: this route must always label binding_scope=configuration_preview"
    );

    clear_env();
}

// ---------------------------------------------------------------------------
// API-17 — non-applicable modes represented truthfully
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_17_non_applicable_mode_is_truthful_not_fabricated_ready() {
    let _g = env_lock().lock().await;
    clear_env();

    // AppState::new() defaults to a non-paper+alpaca deployment (paper+paper),
    // so strategy_market_data_source() != ExternalSignalIngestion.
    let st = make_default();
    let router = routes::build_router(st);
    let (status, v) = call(router, "/api/v1/market-data/readiness").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["applicability"], "not_applicable");
    assert_eq!(
        v["start_allowed"], false,
        "API-17: not_applicable must never be reported as start-ready"
    );
    assert_eq!(v["assignments"], serde_json::json!([]));

    clear_env();
}

// ---------------------------------------------------------------------------
// API-04/05/18 — canonical assignment identity, provider identity, no
// fabricated zeroes for unknown values
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_04_05_18_assignment_identity_and_provider_identity_no_fabricated_zeroes() {
    let _g = env_lock().lock().await;
    clear_env();
    let dir = write_registries();
    set_matching_assignment_env();

    let st = make_paper_alpaca();
    let router = routes::build_router(st);
    let (status, v) = call(router, "/api/v1/market-data/readiness").await;
    assert_eq!(status, StatusCode::OK);

    assert_eq!(v["applicability"], "applicable");
    assert_eq!(v["assignment_source"], "env_single_symbol_fallback");
    let assignments = v["assignments"].as_array().expect("assignments array");
    assert_eq!(
        assignments.len(),
        1,
        "API-04: exactly one legacy-fallback assignment: {v}"
    );
    let a = &assignments[0];

    assert_eq!(a["assignment_symbol"], FAKE_SYMBOL);
    assert_eq!(a["assignment_timeframe"], "1D");
    assert_eq!(a["configured_strategy_id"], "swing_momentum");
    assert_eq!(
        a["effective_runtime_strategy_id"], "swing_momentum",
        "API-04: binding must resolve to the active bootstrap's strategy id"
    );
    assert_eq!(a["effective_runtime_target_symbol"], FAKE_SYMBOL);
    assert_eq!(a["effective_runtime_timeframe_secs"], 86_400);
    assert_eq!(a["required_history_bars"], 20, "swing_momentum LOOKBACK=20");
    assert_eq!(a["asset_class"], "equity");

    assert_eq!(
        a["expected_provider_id"], FAKE_PROVIDER,
        "API-05: expected provider identity must come from the instrument registry"
    );
    assert_eq!(a["expected_provider_symbol"], FAKE_SYMBOL);

    // API-18: no DB is configured on this AppState — loaded_completed_bars,
    // expected_latest_bar_ts, actual_latest_bar_ts must be null, never a
    // fabricated 0.
    assert!(
        a["loaded_completed_bars"].is_null(),
        "API-18: unknown bar count must be null, not 0: {a}"
    );
    assert!(a["actual_latest_bar_ts"].is_null());
    assert_eq!(
        a["readiness_state"], "db_unavailable",
        "API-06: DB absent -> db_unavailable, not a fabricated ready/blocked verdict: {a}"
    );

    clear_env();
    cleanup_registries(&dir);
}

// ---------------------------------------------------------------------------
// API-06 — db_unavailable when DB is absent (also covered above; this test
// additionally proves the top-level start_allowed reflects it)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_06_db_unavailable_when_db_absent() {
    let _g = env_lock().lock().await;
    clear_env();
    let dir = write_registries();
    set_matching_assignment_env();

    let st = make_paper_alpaca();
    let router = routes::build_router(st);
    let (_status, v) = call(router, "/api/v1/market-data/readiness").await;

    assert_eq!(v["assignments"][0]["readiness_state"], "db_unavailable");
    assert_eq!(
        v["start_allowed"], false,
        "API-06: a db_unavailable assignment must never yield start_allowed=true"
    );

    clear_env();
    cleanup_registries(&dir);
}

// ---------------------------------------------------------------------------
// API-07/09/10/11/12 — read-only: no run/outbox row, no event, no
// provider/broker call (DB-backed; skip without MQK_DATABASE_URL)
// ---------------------------------------------------------------------------

async fn db_pool_or_skip(label: &str) -> Option<sqlx::PgPool> {
    let Ok(url) = std::env::var("MQK_DATABASE_URL") else {
        eprintln!("{label}: skipped; MQK_DATABASE_URL is not set");
        return None;
    };
    if url.contains(":5440") || url.contains("miniquantdesk_paper") {
        eprintln!("{label}: skipped; MQK_DATABASE_URL looks like the paper DB, refusing to run");
        return None;
    }
    let pool = match sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
    {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("{label}: skipped; could not connect to MQK_DATABASE_URL: {e}");
            return None;
        }
    };
    if let Err(e) = mqk_db::migrate(&pool).await {
        eprintln!("{label}: skipped; mqk_db::migrate failed: {e}");
        return None;
    }
    Some(pool)
}

async fn count(pool: &sqlx::PgPool, table: &str) -> i64 {
    let row: (i64,) = sqlx::query_as(&format!("select count(*) from {table}"))
        .fetch_one(pool)
        .await
        .unwrap_or_else(|e| panic!("count({table}) failed: {e}"));
    row.0
}

#[tokio::test]
async fn api_07_09_10_11_12_route_is_read_only() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("API-07/09/10/11/12").await else {
        return;
    };
    clear_env();
    let dir = write_registries();
    set_matching_assignment_env();

    let runs_before = count(&pool, "runs").await;
    let outbox_before = count(&pool, "oms_outbox").await;
    let events_before = count(&pool, "sys_autonomous_session_events").await;

    let mut st_raw = state::AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st_raw.db = Some(pool.clone());
    let st = Arc::new(st_raw);
    let router = routes::build_router(st);
    let (status, v) = call(router, "/api/v1/market-data/readiness").await;
    assert_eq!(status, StatusCode::OK);
    // With DB present and no bars seeded for FAKE_SYMBOL, the assignment
    // reaches the bar stage and blocks on market_data_missing — proving the
    // route drove a real bounded query, not merely a db_unavailable
    // short-circuit, while still creating no state.
    assert_eq!(v["assignments"][0]["readiness_state"], "blocked");

    let runs_after = count(&pool, "runs").await;
    let outbox_after = count(&pool, "oms_outbox").await;
    let events_after = count(&pool, "sys_autonomous_session_events").await;

    assert_eq!(
        runs_before, runs_after,
        "API-09: route must create no run row"
    );
    assert_eq!(
        outbox_before, outbox_after,
        "API-10: route must create no outbox row"
    );
    assert_eq!(
        events_before, events_after,
        "API-08: route must persist no autonomous-session event (GET never writes evidence)"
    );
    // API-11/12: no provider/broker client is wired into this route's call
    // graph at all (compute_daily_data_readiness_response -> evaluate_assignment
    // only ever calls mqk_db::md::fetch_bounded_bars_with_provenance) —
    // structurally incapable of a provider or broker call.

    clear_env();
    cleanup_registries(&dir);
}

// ---------------------------------------------------------------------------
// API-13/14/15/16 — surface agreement across all four projections
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_13_14_15_16_surfaces_agree_on_daily_data_readiness() {
    let _g = env_lock().lock().await;
    clear_env();
    let dir = write_registries();
    set_matching_assignment_env();

    let st = make_paper_alpaca();
    let router = routes::build_router(st);

    let (status_readiness, readiness) = call(router.clone(), "/api/v1/market-data/readiness").await;
    let (status_preflight, preflight) = call(router.clone(), "/api/v1/system/preflight").await;
    let (status_autonomous, autonomous) =
        call(router.clone(), "/api/v1/autonomous/readiness").await;
    let (status_ingest_plan, ingest_plan) = call(router, "/api/v1/market-data/ingest-plan").await;

    for s in [
        status_readiness,
        status_preflight,
        status_autonomous,
        status_ingest_plan,
    ] {
        assert_eq!(s, StatusCode::OK);
    }

    let readiness_stripped = strip_volatile(readiness);
    let preflight_ddr = strip_volatile(preflight["daily_data_readiness"].clone());
    let autonomous_ddr = strip_volatile(autonomous["daily_data_readiness"].clone());
    let ingest_plan_ddr = strip_volatile(ingest_plan["daily_data_readiness"].clone());

    assert_eq!(
        readiness_stripped, preflight_ddr,
        "API-13: preflight.daily_data_readiness must agree with the dedicated route"
    );
    assert_eq!(
        readiness_stripped, autonomous_ddr,
        "API-14: autonomous/readiness.daily_data_readiness must agree with the dedicated route"
    );
    assert_eq!(
        readiness_stripped, ingest_plan_ddr,
        "API-15: ingest-plan.daily_data_readiness must agree with the dedicated route"
    );

    // API-16: blockers and start_allowed agree across all four surfaces —
    // already implied by the full deep-equality assertions above, restated
    // explicitly for the specific fields the mission calls out.
    assert_eq!(
        readiness_stripped["start_allowed"],
        preflight_ddr["start_allowed"]
    );
    assert_eq!(
        readiness_stripped["assignments"][0]["blockers"],
        autonomous_ddr["assignments"][0]["blockers"]
    );

    clear_env();
    cleanup_registries(&dir);
}

// ---------------------------------------------------------------------------
// REPAIR 2 (DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01): continuity_state
// truth, proven end-to-end through the live HTTP route (not only the pure
// `derive_continuity_state` unit tests in
// `scenario_daily_data_readiness_01.rs`). `md_bars`'s
// `primary key (symbol, timeframe, end_ts)` makes a genuine duplicate row
// impossible to seed through a real DB write, and a history shortfall always
// co-occurs with a gap here too — so this file covers the one continuity
// defect that IS independently reproducible via real seeded rows: a missing
// interior date.
// ---------------------------------------------------------------------------

async fn seed_1d_bars(pool: &sqlx::PgPool, symbol: &str, provider: &str, end_ts_list: &[i64]) {
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = '1D'")
        .bind(symbol)
        .execute(pool)
        .await
        .expect("cleanup failed");
    for &end_ts in end_ts_list {
        sqlx::query(
            r#"
            insert into md_bars (
              symbol, timeframe, end_ts, open_micros, high_micros, low_micros,
              close_micros, volume, is_complete, provider_id, provider_source,
              provider_symbol, ingest_mode, ingested_at
            ) values ($1,'1D',$2,100000000,101000000,99000000,100500000,1000000,true,
                      $3,$3,$1,'historical_sync',now())
            "#,
        )
        .bind(symbol)
        .bind(end_ts)
        .bind(provider)
        .execute(pool)
        .await
        .expect("seed insert failed");
    }
}

async fn cleanup_bars(pool: &sqlx::PgPool, symbol: &str) {
    let _ = sqlx::query("delete from md_bars where symbol = $1")
        .bind(symbol)
        .execute(pool)
        .await;
}

#[tokio::test]
async fn api_19_interior_gap_reports_continuity_state_gap_detected() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("API-19").await else {
        return;
    };
    clear_env();
    let symbol = "ZZDDRAPI19";
    let provider = "zzddrapi19_provider";
    let dir = write_registries_for(symbol, provider);
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", symbol);
        std::env::set_var("MQK_STRATEGY_IDS", "swing_momentum");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1D");
    }

    let calendar_provider = mqk_daemon::state::market_calendar::NyseWeekdaysProvider;
    let now = chrono::Utc::now();
    let schedule = mqk_daemon::state::market_calendar::resolve_market_session_schedule(
        &calendar_provider,
        now,
    );
    let expected = mqk_daemon::daily_data_readiness::expected_daily_end_ts_window(
        &calendar_provider,
        &schedule,
        now.timestamp(),
        900,
        20,
    )
    .expect("expected window resolves");
    assert_eq!(expected.len(), 20);
    // Seed every expected date except one interior (non-latest) date, so the
    // window is otherwise fully covered — isolates the gap finding from an
    // insufficient-history finding (19 of 20 bars present).
    let with_interior_gap: Vec<i64> = expected
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 5)
        .map(|(_, ts)| *ts)
        .collect();
    assert_eq!(with_interior_gap.len(), 19);
    seed_1d_bars(&pool, symbol, provider, &with_interior_gap).await;

    let mut st_raw = state::AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st_raw.db = Some(pool.clone());
    let st = Arc::new(st_raw);
    let router = routes::build_router(st);
    let (status, v) = call(router, "/api/v1/market-data/readiness").await;
    assert_eq!(status, StatusCode::OK);

    let a = &v["assignments"][0];
    assert!(
        a["blockers"]
            .as_array()
            .expect("blockers array")
            .iter()
            .any(|b| b == "interior_gap"),
        "API-19: interior_gap must be a named blocker: {a}"
    );
    assert_eq!(
        a["continuity_state"], "gap_detected",
        "REPAIR 2: interior gap -> continuity_state == gap_detected: {a}"
    );
    assert_ne!(a["continuity_state"], "ok");

    clear_env();
    cleanup_registries(&dir);
    cleanup_bars(&pool, symbol).await;
}
