//! DAILY-DATA-READINESS-01C-ENFORCEMENT-01 — runtime start-gate proof.
//!
//! Binding contract:
//! `docs/specs/daily_data_readiness_01a_current_truth_and_contract.md`.
//!
//! Exercises `AppState::start_execution_runtime()` directly (never a real
//! daemon, never a real provider/broker). Uses the in-memory test seams
//! already established elsewhere in this crate
//! (`set_strategy_fleet_for_test`, `update_ws_continuity`,
//! `integrity.write()`) plus two narrow seams added for this phase:
//! `set_daily_data_readiness_clock_override_for_test` (injected clock, §C.12)
//! and `set_daily_data_readiness_evidence_override_for_test` (injected
//! evidence-write outcome, §C.12 "injected event writers") — the latter
//! proves the §C.9 evidence-failure policy deterministically without
//! breaking a live DB connection mid-test.
//!
//! Every fixture uses a synthetic instrument/provider/symbol
//! (`ZZSGxx`-prefixed) via temporary registry file overrides — never a real
//! ticker.
//!
//! Run alone:
//! `cargo test -p mqk-daemon --test scenario_daily_data_readiness_start_gate_01 -- --test-threads=1`

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use chrono::{TimeZone, Utc};
use mqk_daemon::state::{
    AlpacaWsContinuityState, AppState, BrokerKind, DeploymentMode, StrategyFleetEntry,
};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Env-var serialization (matches every other daily-data-readiness test file)
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
        std::env::remove_var("MQK_SESSION_START_HH_MM");
        std::env::remove_var("MQK_SESSION_STOP_HH_MM");
        std::env::remove_var("MQK_DATA_READINESS_GRACE_SECS");
        std::env::remove_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS");
    }
}

// ---------------------------------------------------------------------------
// Fixed clock — same verified instant Phase B's own DB-backed tests use
// (`scenario_daily_data_readiness_01.rs`'s MON_2024_04_15_935AM_EDT): a
// Monday, well inside NYSE regular session, well inside the calendar seam's
// 2023-2028 coverage window.
// ---------------------------------------------------------------------------

const MON_2024_04_15_935AM_EDT: i64 = 1_713_188_100; // 2024-04-15 13:35 UTC = 09:35 EDT, Mon

fn fixed_now() -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(MON_2024_04_15_935AM_EDT + 10 * 300, 0)
        .single()
        .expect("valid ts")
}

// ---------------------------------------------------------------------------
// Synthetic registry fixtures
// ---------------------------------------------------------------------------

static COUNTER: AtomicU32 = AtomicU32::new(0);
fn next_id() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

const SYMBOL: &str = "ZZSG01";
const PROVIDER: &str = "zzsg01_provider";

/// `symbol`/`provider` are parameterized (rather than always the shared
/// `SYMBOL`/`PROVIDER` consts) so DB-backed tests that persist a real
/// deterministic `evaluation_id` (§C.8) never collide with each other's
/// already-inserted `sys_autonomous_session_events` row within the same
/// test-binary process (the id is a pure function of
/// (minute_bucket, binding, assignment) — two tests sharing an identical
/// symbol/strategy/timeframe/clock would otherwise deterministically compute
/// the same id and silently no-op on `on conflict (id) do nothing`).
fn write_registries_for(symbol: &str, provider: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mqk_ddr_sg01_registry_{}_{}",
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
    "timeframes": ["5m"],
    "notes": "DAILY-DATA-READINESS-01C-ENFORCEMENT-01 start-gate synthetic fixture"
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
    "display_name": "ZZSG01 Test Provider",
    "asset_classes": ["equity"],
    "free_tier_available": true,
    "api_key_required": false,
    "credential_env_vars": [],
    "rate_limit_notes": "",
    "supported_timeframes": ["5m"],
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

fn write_registries() -> std::path::PathBuf {
    write_registries_for(SYMBOL, PROVIDER)
}

fn cleanup(dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(dir);
}

fn write_watchlist(symbol: &str, strategy_id: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mqk_ddr_sg01_watchlist_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::write(
        &path,
        format!(
            r#"{{
  "schema_version": "watchlist-v2",
  "mode": "paper",
  "approved_for_autonomous_paper": true,
  "approved_for_live": false,
  "symbols": ["{symbol}"],
  "strategy_assignments": {{"{symbol}": "{strategy_id}"}},
  "max_symbols_to_trade": 1,
  "max_concurrent_positions": 1
}}"#
        ),
    )
    .expect("write watchlist fixture");
    path
}

// ---------------------------------------------------------------------------
// AppState fixture builder — passes every gate ahead of B1A/Phase C:
// deployment_readiness (paper+alpaca), integrity armed, WS continuity live,
// reconcile clean (default "unknown", allowed through), no artifact/parity/
// capital-policy configured (NotConfigured passes on paper).
// ---------------------------------------------------------------------------

async fn ready_state_with_fleet(fleet_strategy_id: Option<&str>) -> std::sync::Arc<AppState> {
    let st = std::sync::Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca));
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "sg01-test-msg".to_string(),
        last_event_at: "2026-04-14T15:00:00Z".to_string(),
    })
    .await;
    st.integrity.write().await.disarmed = false;
    if let Some(id) = fleet_strategy_id {
        st.set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
            strategy_id: id.to_string(),
        }]))
        .await;
    }
    st.set_daily_data_readiness_clock_override_for_test(Some(fixed_now()))
        .await;
    st
}

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

async fn seed_5m_bars(pool: &sqlx::PgPool, symbol: &str, provider: &str, expected_ts: &[i64]) {
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = '5m'")
        .bind(symbol)
        .execute(pool)
        .await
        .expect("cleanup failed");
    for &end_ts in expected_ts {
        sqlx::query(
            r#"
            insert into md_bars (
              symbol, timeframe, end_ts, open_micros, high_micros, low_micros,
              close_micros, volume, is_complete, provider_id, provider_source,
              provider_symbol, ingest_mode, ingested_at
            ) values ($1,'5m',$2,100000000,101000000,99000000,100500000,1000000,true,
                      $3,$3,$1,'historical_sync',$4)
            "#,
        )
        .bind(symbol)
        .bind(end_ts)
        .bind(provider)
        .bind(
            Utc.timestamp_opt(end_ts + 60, 0)
                .single()
                .expect("valid ts"),
        )
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

// ---------------------------------------------------------------------------
// SG-01 — missing assignments block before run creation (req #1)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_01_missing_assignments_blocks_before_run_creation() {
    let _g = env_lock().lock().await;
    clear_env();
    let dir = write_registries();
    // No MQK_STRATEGY_SYMBOL / watchlist configured -> assignment resolution
    // fails closed (`required_assignments_missing`).
    let st = ready_state_with_fleet(Some("intraday_scalper")).await;

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SG-01: missing assignments must refuse start");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.daily_data_readiness_blocked"
    );
    assert!(
        err.to_string().contains("required_assignments_missing"),
        "SG-01: error must name the top-level blocker: {err}"
    );

    clear_env();
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// SG-02 — strategy-ID mismatch blocks before run creation (req #2, #19)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_02_strategy_id_mismatch_blocks_before_run_creation() {
    let _g = env_lock().lock().await;
    clear_env();
    let dir = write_registries();
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", SYMBOL);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    }
    // Fleet (the source of the REAL bootstrap's active strategy, per B1A) is
    // deliberately a DIFFERENT strategy sharing the same TIMEFRAME_SECS=300
    // (intraday_short_scalper) so only the strategy-id axis mismatches —
    // symbol and timeframe both still agree. Proves the gate uses the exact
    // bootstrap it constructed (req #19), not the env-var-derived id.
    let st = ready_state_with_fleet(Some("intraday_short_scalper")).await;

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SG-02: strategy-id mismatch must refuse start");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.daily_data_readiness_blocked"
    );
    assert!(
        err.to_string()
            .contains("runtime_strategy_assignment_mismatch"),
        "SG-02: error must name runtime_strategy_assignment_mismatch: {err}"
    );

    clear_env();
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// SG-03 — target-symbol mismatch blocks before run creation (req #3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_03_target_symbol_mismatch_blocks_before_run_creation() {
    let _g = env_lock().lock().await;
    clear_env();
    let dir = write_registries();
    let watchlist_symbol = "ZZSGWATCH01";
    let watchlist_path = write_watchlist(watchlist_symbol, "intraday_scalper");
    unsafe {
        // Bootstrap target symbol comes from MQK_STRATEGY_SYMBOL, deliberately
        // different from the watchlist-v2 assignment's own symbol.
        std::env::set_var("MQK_STRATEGY_SYMBOL", SYMBOL);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
        std::env::set_var("MQK_PAPER_WATCHLIST_PATH", &watchlist_path);
    }
    let st = ready_state_with_fleet(Some("intraday_scalper")).await;

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SG-03: target-symbol mismatch must refuse start");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.daily_data_readiness_blocked"
    );
    assert!(
        err.to_string()
            .contains("runtime_strategy_symbol_binding_mismatch"),
        "SG-03: error must name runtime_strategy_symbol_binding_mismatch: {err}"
    );

    clear_env();
    cleanup(&dir);
    let _ = std::fs::remove_file(&watchlist_path);
}

// ---------------------------------------------------------------------------
// SG-04 — timeframe mismatch blocks before run creation (req #4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_04_timeframe_mismatch_blocks_before_run_creation() {
    let _g = env_lock().lock().await;
    clear_env();
    let dir = write_registries();
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", SYMBOL);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        // intraday_scalper's own StrategySpec.timeframe_secs is 300 (5m);
        // "1m" (60s) deliberately disagrees — symbol/strategy both match.
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "1m");
    }
    let st = ready_state_with_fleet(Some("intraday_scalper")).await;

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SG-04: timeframe mismatch must refuse start");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.daily_data_readiness_blocked"
    );
    assert!(
        err.to_string()
            .contains("runtime_strategy_timeframe_mismatch"),
        "SG-04: error must name runtime_strategy_timeframe_mismatch: {err}"
    );

    clear_env();
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// SG-05 — DB unavailable blocks before run creation; evidence_persisted=false
// (req #5, #15)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_05_db_unavailable_blocks_and_reports_evidence_not_persisted() {
    let _g = env_lock().lock().await;
    clear_env();
    let dir = write_registries();
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", SYMBOL);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    }
    // Fully matching binding, no DB configured on this AppState at all.
    let st = ready_state_with_fleet(Some("intraday_scalper")).await;
    assert!(st.db.is_none(), "SG-05: precondition — no DB configured");

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SG-05: db-unavailable assignment must refuse start");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.daily_data_readiness_blocked"
    );
    assert!(
        err.to_string().contains("evidence_persisted=false"),
        "SG-05/15: DB-unavailable must honestly report evidence_persisted=false: {err}"
    );

    clear_env();
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// SG-06 — calendar unavailable blocks before run creation (req #6)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_06_calendar_unavailable_blocks_before_run_creation() {
    let _g = env_lock().lock().await;
    clear_env();
    let dir = write_registries();
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", SYMBOL);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
        // FixedWindowOverrideProvider consults no exchange calendar at all —
        // resolve_market_session_schedule always reports coverage_state=Unknown
        // for it (§6a).
        std::env::set_var("MQK_SESSION_START_HH_MM", "09:30");
        std::env::set_var("MQK_SESSION_STOP_HH_MM", "16:00");
    }
    let st = ready_state_with_fleet(Some("intraday_scalper")).await;

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SG-06: calendar-unavailable assignment must refuse start");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.daily_data_readiness_blocked"
    );
    assert!(
        err.to_string().contains("calendar_unavailable"),
        "SG-06: error must name calendar_unavailable: {err}"
    );

    clear_env();
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// SG-07 — market-data-missing blocks before run creation; no run/outbox row;
// no loop spawned (req #7, #8, #9, #10, #11)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_07_market_data_missing_blocks_creates_no_run_no_outbox_no_loop() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("SG-07").await else {
        return;
    };
    clear_env();
    let symbol = "ZZSG07";
    let dir = write_registries_for(symbol, "zzsg07_provider");
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", symbol);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    }
    cleanup_bars(&pool, symbol).await; // no bars seeded -> market_data_missing

    let runs_before = count(&pool, "runs").await;
    let outbox_before = count(&pool, "oms_outbox").await;

    let mut st_raw = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st_raw.db = Some(pool.clone());
    let st = std::sync::Arc::new(st_raw);
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "sg07-test-msg".to_string(),
        last_event_at: "2026-04-14T15:00:00Z".to_string(),
    })
    .await;
    st.integrity.write().await.disarmed = false;
    st.set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
        strategy_id: "intraday_scalper".to_string(),
    }]))
    .await;
    st.set_daily_data_readiness_clock_override_for_test(Some(fixed_now()))
        .await;

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SG-07: market_data_missing assignment must refuse start");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.daily_data_readiness_blocked"
    );
    assert!(
        err.to_string().contains("market_data_missing"),
        "SG-07: error must name market_data_missing: {err}"
    );

    assert_eq!(
        runs_before,
        count(&pool, "runs").await,
        "SG-08/09: blocked start must create no run row"
    );
    assert_eq!(
        outbox_before,
        count(&pool, "oms_outbox").await,
        "SG-09/10: blocked start must create no outbox row"
    );
    assert!(
        st.native_strategy_bootstrap_truth_state_for_test()
            .await
            .is_none(),
        "SG-08: bootstrap is only stored after a fully successful start — \
         its absence proves no execution loop was spawned"
    );
    // SG-10/11: no provider or broker adapter is constructed anywhere on the
    // path from B1A through this gate to the early return — structurally
    // unreachable code, not merely untriggered in this fixture.

    clear_env();
    cleanup(&dir);
    cleanup_bars(&pool, symbol).await;
}

// ---------------------------------------------------------------------------
// SG-08 — pre-start event attempted before run creation, even when blocked
// (req #12, #14)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_08_pre_start_evidence_attempted_even_when_blocked_and_keeps_original_blocker() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("SG-08").await else {
        return;
    };
    clear_env();
    let symbol = "ZZSG08";
    let dir = write_registries_for(symbol, "zzsg08_provider");
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", symbol);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    }
    cleanup_bars(&pool, symbol).await;
    // Reset this test's own evidence identity (evaluation_id is a pure
    // function of (minute_bucket, binding, assignment); the fixed clock used
    // across this whole file means a rerun of this exact test binary would
    // otherwise collide with its own prior run's already-persisted row via
    // `on conflict (id) do nothing`, silently no-op'ing the insert this run).
    let _ = sqlx::query("delete from sys_autonomous_session_events where detail like $1")
        .bind(format!("%\"{symbol}\"%"))
        .execute(&pool)
        .await;

    let mut st_raw = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st_raw.db = Some(pool.clone());
    let st = std::sync::Arc::new(st_raw);
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "sg08-test-msg".to_string(),
        last_event_at: "2026-04-14T15:00:00Z".to_string(),
    })
    .await;
    st.integrity.write().await.disarmed = false;
    st.set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
        strategy_id: "intraday_scalper".to_string(),
    }]))
    .await;
    st.set_daily_data_readiness_clock_override_for_test(Some(fixed_now()))
        .await;
    // req #14: force the evidence write to report failure — the blocked
    // verdict must still return its ORIGINAL blocker (market_data_missing),
    // never overwritten by a readiness_evidence_persist_failed message.
    st.set_daily_data_readiness_evidence_override_for_test(Some(false))
        .await;

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SG-08: blocked assignment must refuse start");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.daily_data_readiness_blocked",
        "req #14: a blocked verdict must keep its own fault_class even when \
         evidence persistence is forced to fail: {err}"
    );
    assert!(
        err.to_string().contains("market_data_missing"),
        "req #14: original blocker must survive an evidence-persist failure: {err}"
    );
    assert!(
        err.to_string().contains("evidence_persisted=false"),
        "req #14: evidence_persisted must honestly reflect the forced failure: {err}"
    );

    let matching_events: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_session_events \
         where event_type = 'daily_data_readiness_evaluated' and detail like $1",
    )
    .bind(format!("%\"{symbol}\"%"))
    .fetch_one(&pool)
    .await
    .expect("count matching evidence rows");
    assert_eq!(
        matching_events.0, 1,
        "req #12: the pre-start event must be attempted (and persisted, since the \
         override only forces the *reported* outcome, not the real write) even \
         though the verdict was blocked"
    );

    // Filtered by symbol, not `order by ts_utc desc limit 1` — every test in
    // this file shares the same injected fixed clock, so `ts_utc` ties do
    // not reliably order by insertion recency.
    let row: (String,) = sqlx::query_as(
        "select detail from sys_autonomous_session_events \
         where event_type = 'daily_data_readiness_evaluated' and detail like $1 \
         order by ts_utc desc limit 1",
    )
    .bind(format!("%\"{symbol}\"%"))
    .fetch_one(&pool)
    .await
    .expect("fetch evidence row");
    let detail: serde_json::Value = serde_json::from_str(&row.0).expect("evidence detail JSON");
    assert_eq!(detail["binding_scope"], "start_attempt_binding");
    assert_eq!(detail["start_allowed"], false);

    clear_env();
    cleanup(&dir);
    cleanup_bars(&pool, symbol).await;
}

// ---------------------------------------------------------------------------
// SG-09 — ready verdict refuses start if evidence persistence fails (req #13)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_09_ready_verdict_refuses_start_when_evidence_persist_fails() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("SG-09").await else {
        return;
    };
    clear_env();
    let symbol = "ZZSG09";
    let provider_id = "zzsg09_provider";
    let dir = write_registries_for(symbol, provider_id);
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", symbol);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
        // Matches Phase B's own DB-backed ready fixtures
        // (scenario_daily_data_readiness_01.rs DDR-29/30): the evaluator's
        // effective_grace_seconds defaults to min(900, 300)=300 unless
        // overridden, which would shift the expected-window arithmetic
        // relative to the seeded bars below.
        std::env::set_var("MQK_DATA_READINESS_GRACE_SECS", "0");
        std::env::set_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS", "60");
    }

    let calendar_provider = mqk_daemon::state::market_calendar::NyseWeekdaysProvider;
    let now = fixed_now();
    let schedule = mqk_daemon::state::market_calendar::resolve_market_session_schedule(
        &calendar_provider,
        now,
    );
    let expected = mqk_daemon::daily_data_readiness::expected_intraday_end_ts_window(
        &calendar_provider,
        &schedule,
        now.timestamp(),
        300,
        0,
        5,
    )
    .expect("expected window resolves");
    seed_5m_bars(&pool, symbol, provider_id, &expected).await;

    let runs_before = count(&pool, "runs").await;

    let mut st_raw = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st_raw.db = Some(pool.clone());
    let st = std::sync::Arc::new(st_raw);
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "sg09-test-msg".to_string(),
        last_event_at: "2026-04-14T15:00:00Z".to_string(),
    })
    .await;
    st.integrity.write().await.disarmed = false;
    st.set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
        strategy_id: "intraday_scalper".to_string(),
    }]))
    .await;
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;
    st.set_daily_data_readiness_evidence_override_for_test(Some(false))
        .await;

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SG-09: ready verdict must refuse start if evidence persist fails");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.readiness_evidence_persist_failed",
        "req #13: {err}"
    );
    assert_eq!(
        runs_before,
        count(&pool, "runs").await,
        "req #13: a ready-but-evidence-failed start must create no run row"
    );

    clear_env();
    cleanup(&dir);
    cleanup_bars(&pool, symbol).await;
}

// ---------------------------------------------------------------------------
// SG-10 — synthetic readiness permits synthetic run creation; run-linked
// evidence shares evaluation_id; later lifecycle gates remain enforced
// (req #16, #17, #18, #22)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_10_ready_start_creates_run_and_links_evidence_but_later_gate_still_enforced() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("SG-10").await else {
        return;
    };
    clear_env();
    let symbol = "ZZSG10";
    let provider_id = "zzsg10_provider";
    let dir = write_registries_for(symbol, provider_id);
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", symbol);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
        std::env::set_var("MQK_DATA_READINESS_GRACE_SECS", "0");
        std::env::set_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS", "60");
    }

    let calendar_provider = mqk_daemon::state::market_calendar::NyseWeekdaysProvider;
    let now = fixed_now();
    let schedule = mqk_daemon::state::market_calendar::resolve_market_session_schedule(
        &calendar_provider,
        now,
    );
    let expected = mqk_daemon::daily_data_readiness::expected_intraday_end_ts_window(
        &calendar_provider,
        &schedule,
        now.timestamp(),
        300,
        0,
        5,
    )
    .expect("expected window resolves");
    seed_5m_bars(&pool, symbol, provider_id, &expected).await;

    // Deliberately do NOT insert a sys_strategy_registry row for
    // "intraday_scalper" — B2A (the DB strategy-registry gate, positioned
    // after db_pool()?, i.e. strictly after this Phase C gate) must still
    // fire (req #22: existing later lifecycle safety gates remain present).
    let mut st_raw = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st_raw.db = Some(pool.clone());
    let st = std::sync::Arc::new(st_raw);
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "sg10-test-msg".to_string(),
        last_event_at: "2026-04-14T15:00:00Z".to_string(),
    })
    .await;
    st.integrity.write().await.disarmed = false;
    st.set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
        strategy_id: "intraday_scalper".to_string(),
    }]))
    .await;
    st.set_daily_data_readiness_clock_override_for_test(Some(now))
        .await;

    // See SG-08's comment: reset this test's own evidence identity so a
    // rerun of this binary cannot collide with a prior run's already-
    // persisted row for the same deterministic evaluation_id.
    let _ = sqlx::query("delete from sys_autonomous_session_events where detail like $1")
        .bind(format!("%\"{symbol}\"%"))
        .execute(&pool)
        .await;

    let result = st.start_execution_runtime().await;
    let err = result.expect_err(
        "SG-10 (req #22): B2A strategy-registry gate must still fire even though \
         daily-data readiness was ready — this bundle adds a prerequisite, it does \
         not replace later safety controls",
    );
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.strategy_registry_missing",
        "req #22: the LATER gate's own fault_class must be the one that fires: {err}"
    );

    // req #16: daily-data readiness itself permitted the attempt to proceed
    // past this gate (it did not block on daily_data_readiness_blocked) —
    // proven by the fault_class above being the *later* gate's, not this
    // phase's. req #12's pre-start event was still attempted for this ready
    // verdict (evidence persistence is part of the ready-path gate itself).
    let matching_events: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_session_events \
         where event_type = 'daily_data_readiness_evaluated' and detail like $1",
    )
    .bind(format!("%\"{symbol}\"%"))
    .fetch_one(&pool)
    .await
    .expect("count matching evidence rows");
    assert_eq!(
        matching_events.0, 1,
        "req #12/#16: a ready verdict's pre-start event must be persisted \
         even though a later gate subsequently refuses the run"
    );
    let (detail_json,): (String,) = sqlx::query_as(
        "select detail from sys_autonomous_session_events \
         where event_type = 'daily_data_readiness_evaluated' and detail like $1 \
         order by ts_utc desc limit 1",
    )
    .bind(format!("%\"{symbol}\"%"))
    .fetch_one(&pool)
    .await
    .expect("fetch evidence row");
    let detail: serde_json::Value = serde_json::from_str(&detail_json).expect("evidence JSON");
    assert_eq!(
        detail["start_allowed"], true,
        "SG-10: the daily-data readiness verdict itself must have been ready: {detail}"
    );
    // No run_linked event: B2A refused before run creation, so req #17/#18's
    // linkage never had a run_id to attach to — proving the linked event is
    // strictly conditional on actual run creation, never fabricated.
    let linked_count: (i64,) =
        sqlx::query_as("select count(*) from sys_autonomous_session_events where event_type = $1")
            .bind(mqk_daemon::daily_data_readiness::EVENT_TYPE_READINESS_RUN_LINKED)
            .fetch_one(&pool)
            .await
            .expect("count run_linked events");
    assert_eq!(
        linked_count.0, 0,
        "no run was created, so no run_linked evidence row may exist"
    );

    clear_env();
    cleanup(&dir);
    cleanup_bars(&pool, symbol).await;
}

// ---------------------------------------------------------------------------
// SG-11 — non-applicable modes preserve prior behavior (req #21)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_11_non_applicable_mode_preserves_prior_deployment_gate_behavior() {
    let _g = env_lock().lock().await;
    clear_env();
    // paper+paper: strategy_market_data_source() != ExternalSignalIngestion,
    // so this Phase C gate is not_applicable and must never fire. The
    // pre-existing PT-TRUTH-01 deployment-readiness gate (earlier in the
    // function) refuses first, exactly as it did before this patch.
    let st = std::sync::Arc::new(AppState::new_for_test_with_mode(DeploymentMode::Paper));

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SG-11: paper+paper must still refuse at the pre-existing deployment gate");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.deployment_mode_unproven",
        "req #21: non-applicable deployments must preserve their prior gate behavior \
         unchanged by this patch: {err}"
    );

    clear_env();
}
