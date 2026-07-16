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
/// test-binary process by relying on the assignment identity alone (the id
/// is a pure function of evaluated-at-nanos, attempt_seq, binding, and
/// assignment identity — REPAIR 1's `attempt_seq` already prevents same-id
/// collisions across separate start attempts, but tests in this file filter
/// evidence rows by symbol substring match, so a distinct symbol per test
/// keeps each test's own row query unambiguous).
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
// SG-06 — a valid fixed-window override no longer causes a false
// calendar-unavailable refusal (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-
// CANONICAL-CALENDAR-REPAIR-01, req #6 repair). Bundle 2's shared calendar
// context must never receive a `FixedWindowOverrideProvider` as its
// authoritative calendar, so a valid override alone must not surface
// `calendar_unavailable` — the refusal below comes only from no DB being
// attached (matches SG-05's precondition), never from calendar truth.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_06_valid_fixed_window_override_does_not_cause_false_calendar_unavailable_refusal() {
    let _g = env_lock().lock().await;
    clear_env();
    let dir = write_registries();
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", SYMBOL);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
        std::env::set_var("MQK_SESSION_START_HH_MM", "09:30");
        std::env::set_var("MQK_SESSION_STOP_HH_MM", "16:00");
    }
    let st = ready_state_with_fleet(Some("intraday_scalper")).await;

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SG-06: precondition — start still refused (no DB attached)");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.daily_data_readiness_blocked"
    );
    assert!(
        !err.to_string().contains("calendar_unavailable"),
        "SG-06: a valid runtime-window override must never cause a false \
         calendar_unavailable refusal: {err}"
    );

    clear_env();
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// SG-06B — an invalid fixed-window override is a narrow shared-calendar
// blocker input: it prevents an applicable autonomous start outright, before
// any readiness evaluation, run creation, or provider/broker call
// (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-CANONICAL-CALENDAR-REPAIR-01).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_06b_invalid_fixed_window_override_blocks_before_run_creation() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("SG-06B").await else {
        return;
    };
    clear_env();
    let symbol = "ZZSG06B";
    let dir = write_registries_for(symbol, "zzsg06b_provider");
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", symbol);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
        // Only one of the two required override variables set — invalid.
        std::env::set_var("MQK_SESSION_START_HH_MM", "09:30");
    }
    cleanup_bars(&pool, symbol).await;

    let runs_before = count(&pool, "runs").await;
    let outbox_before = count(&pool, "oms_outbox").await;

    let mut st_raw = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st_raw.db = Some(pool.clone());
    let st = std::sync::Arc::new(st_raw);
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "sg06b-test-msg".to_string(),
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
        .expect_err("SG-06B: invalid fixed-window override must refuse start");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.fixed_window_override_invalid"
    );
    assert!(
        err.to_string().contains("fixed_window_override_invalid"),
        "SG-06B: error must name fixed_window_override_invalid: {err}"
    );

    assert_eq!(
        runs_before,
        count(&pool, "runs").await,
        "SG-06B: invalid override must create no run row"
    );
    assert_eq!(
        outbox_before,
        count(&pool, "oms_outbox").await,
        "SG-06B: invalid override must create no outbox row"
    );

    clear_env();
    cleanup(&dir);
    cleanup_bars(&pool, symbol).await;
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
    // function of evaluated-at-nanos, attempt_seq, binding, and assignment;
    // the fixed clock used across this whole file plus the process-local
    // `attempt_seq` counter restarting at 0 on a fresh process means a rerun
    // of this exact test binary would otherwise collide with its own prior
    // run's already-persisted row via `on conflict (id) do nothing`,
    // silently no-op'ing the insert this run).
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
    // Defensively remove any row a differently-ordered or previously-panicked
    // run of this file's own REPAIR 3/4 tests (SG-16/SG-17) may have left
    // behind, rather than assuming this shared local Postgres starts pristine.
    remove_strategy_registry_entry(&pool, "intraday_scalper").await;
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

    // Baseline (before/after delta), not an assumed-pristine global count:
    // other tests in this file (SG-16/SG-17, REPAIR 3/4) legitimately create
    // their own run_linked evidence for their own runs, so a global
    // zero-count assumption would be a false positive/negative depending on
    // test execution order. This test only asserts that ITS OWN attempt
    // creates no new run_linked row.
    let linked_count_before: (i64,) =
        sqlx::query_as("select count(*) from sys_autonomous_session_events where event_type = $1")
            .bind(mqk_daemon::daily_data_readiness::EVENT_TYPE_READINESS_RUN_LINKED)
            .fetch_one(&pool)
            .await
            .expect("count run_linked events before");

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
    // No NEW run_linked event: B2A refused before run creation, so req
    // #17/#18's linkage never had a run_id to attach to — proving the linked
    // event is strictly conditional on actual run creation, never
    // fabricated.
    let linked_count_after: (i64,) =
        sqlx::query_as("select count(*) from sys_autonomous_session_events where event_type = $1")
            .bind(mqk_daemon::daily_data_readiness::EVENT_TYPE_READINESS_RUN_LINKED)
            .fetch_one(&pool)
            .await
            .expect("count run_linked events after");
    assert_eq!(
        linked_count_after.0, linked_count_before.0,
        "no run was created, so no new run_linked evidence row may exist"
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

// ---------------------------------------------------------------------------
// DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01
//
// SG-12/13 — REPAIR 1: distinct start-attempt identity. SG-16/17 — REPAIR
// 3/4: synthetic lifecycle proof + fail-closed run-linked evidence policy.
// ---------------------------------------------------------------------------

/// Distinct evidence rows persisted for `symbol`, newest-last.
async fn fetch_evidence_details(pool: &sqlx::PgPool, symbol: &str) -> Vec<serde_json::Value> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select detail from sys_autonomous_session_events \
         where event_type = 'daily_data_readiness_evaluated' and detail like $1 \
         order by ts_utc asc",
    )
    .bind(format!("%\"{symbol}\"%"))
    .fetch_all(pool)
    .await
    .expect("fetch evidence rows");
    rows.into_iter()
        .map(|(d,)| serde_json::from_str(&d).expect("evidence JSON"))
        .collect()
}

async fn delete_evidence_for(pool: &sqlx::PgPool, symbol: &str) {
    let _ = sqlx::query("delete from sys_autonomous_session_events where detail like $1")
        .bind(format!("%\"{symbol}\"%"))
        .execute(pool)
        .await;
}

/// Stops any pre-existing ARMED/RUNNING run for the daemon-paper engine/mode
/// left over from an unrelated earlier test run against this shared local
/// Postgres — `AppState::create_or_reuse_run_for_start`'s
/// `durable_active_without_local_owner` conflict check would otherwise
/// refuse every subsequent test in this file that needs to actually create a
/// run (SG-16/SG-17). Mirrors the `delete from runs where run_id = $1`
/// cleanup convention already used across this test suite.
async fn clear_any_preexisting_active_daemon_run(pool: &sqlx::PgPool) {
    // Loop rather than a single stop: this shared local Postgres can carry
    // more than one leftover ARMED/RUNNING row from unrelated earlier test
    // runs; `fetch_active_run_for_engine` only ever returns one at a time.
    for _ in 0..16 {
        match mqk_db::fetch_active_run_for_engine(pool, "mqk-daemon", "PAPER").await {
            Ok(Some(active)) => {
                let _ = mqk_db::stop_run(pool, active.run_id).await;
            }
            _ => break,
        }
    }
}

/// Restores `sys_strategy_registry` for `strategy_id` to "not registered" so
/// other tests in this file (e.g. SG-10, which asserts
/// `strategy_registry_missing`) are not polluted by a row this file's own
/// synthetic-lifecycle tests (SG-16/SG-17) had to insert to progress past
/// B2A.
async fn remove_strategy_registry_entry(pool: &sqlx::PgPool, strategy_id: &str) {
    let _ = sqlx::query("delete from sys_strategy_registry where strategy_id = $1")
        .bind(strategy_id)
        .execute(pool)
        .await;
}

/// Removes a synthetic-lifecycle test's own run row AND its
/// `sys_autonomous_session_events` rows. `AppState::next_daemon_run_id` is
/// deterministic (`UUIDv5(node_id, engine_id, mode, count(*)+1)`,
/// `orchestrator_build.rs`) — deleting only the `runs` row resets the count
/// so a *later* test can deterministically regenerate this exact same
/// run_id, which would then collide with an orphaned `run_linked` evidence
/// row left behind from this run if that row were not also removed here.
async fn delete_run_and_its_events(pool: &sqlx::PgPool, run_id: uuid::Uuid) {
    let _ = sqlx::query("delete from sys_autonomous_session_events where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// SG-12 — REPAIR 1: two sequential identical attempts against an identical
// (fixed, overridden) clock must receive distinct evaluation_ids and persist
// two distinct evidence rows — the minute-bucket scheme this repair replaces
// would collide here (req #1, #3, #4).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_12_sequential_identical_attempts_produce_distinct_evaluation_ids() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("SG-12").await else {
        return;
    };
    clear_env();
    let symbol = "ZZSG12";
    let dir = write_registries_for(symbol, "zzsg12_provider");
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", symbol);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    }
    // No bars seeded -> market_data_missing (blocked), but the pre-start
    // event is still attempted for every applicable evaluation regardless
    // of verdict (§C.8/§C.9) — exactly what this test needs.
    cleanup_bars(&pool, symbol).await;
    delete_evidence_for(&pool, symbol).await;

    let mut st_raw = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st_raw.db = Some(pool.clone());
    let st = std::sync::Arc::new(st_raw);
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "sg12-test-msg".to_string(),
        last_event_at: "2026-04-14T15:00:00Z".to_string(),
    })
    .await;
    st.integrity.write().await.disarmed = false;
    st.set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
        strategy_id: "intraday_scalper".to_string(),
    }]))
    .await;
    // Fixed clock: both attempts see the IDENTICAL now_utc.
    st.set_daily_data_readiness_clock_override_for_test(Some(fixed_now()))
        .await;

    let err1 = st
        .start_execution_runtime()
        .await
        .expect_err("SG-12 attempt 1: market_data_missing must block");
    assert_eq!(
        err1.fault_class(),
        "runtime.start_refused.daily_data_readiness_blocked"
    );
    let err2 = st
        .start_execution_runtime()
        .await
        .expect_err("SG-12 attempt 2: market_data_missing must block");
    assert_eq!(
        err2.fault_class(),
        "runtime.start_refused.daily_data_readiness_blocked"
    );

    let details = fetch_evidence_details(&pool, symbol).await;
    assert_eq!(
        details.len(),
        2,
        "REPAIR 1 req #3/#4: two distinct attempts must persist two distinct evidence rows \
         — never overwritten via `on conflict (id) do nothing`: {details:?}"
    );
    let eval_ids: std::collections::HashSet<String> = details
        .iter()
        .map(|d| {
            d["evaluation_id"]
                .as_str()
                .expect("evaluation_id")
                .to_string()
        })
        .collect();
    assert_eq!(
        eval_ids.len(),
        2,
        "REPAIR 1 req #1: two identical attempts against an identical clock must receive \
         distinct evaluation_ids: {details:?}"
    );

    clear_env();
    cleanup(&dir);
    cleanup_bars(&pool, symbol).await;
}

// ---------------------------------------------------------------------------
// SG-13 — REPAIR 1: two concurrent identical attempts must also receive
// distinct evaluation_ids (req #2).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_13_concurrent_identical_attempts_produce_distinct_evaluation_ids() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("SG-13").await else {
        return;
    };
    clear_env();
    let symbol = "ZZSG13";
    let dir = write_registries_for(symbol, "zzsg13_provider");
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", symbol);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
    }
    cleanup_bars(&pool, symbol).await;
    delete_evidence_for(&pool, symbol).await;

    let mut st_raw = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st_raw.db = Some(pool.clone());
    let st = std::sync::Arc::new(st_raw);
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "sg13-test-msg".to_string(),
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

    // `AppState::lifecycle_op` serializes `start_execution_runtime` calls
    // internally (the same mutex it has always taken as its first action) —
    // that serialization is exactly what makes the process-local monotonic
    // attempt counter race-free. This test proves the OUTCOME two genuinely
    // concurrent callers observe (never colliding on evaluation_id), not the
    // internal serialization mechanism itself.
    let st_a = std::sync::Arc::clone(&st);
    let st_b = std::sync::Arc::clone(&st);
    let (res_a, res_b) = tokio::join!(
        async move { st_a.start_execution_runtime().await },
        async move { st_b.start_execution_runtime().await },
    );
    assert!(
        res_a.is_err() && res_b.is_err(),
        "SG-13: both concurrent attempts must be blocked (market_data_missing)"
    );

    let details = fetch_evidence_details(&pool, symbol).await;
    assert_eq!(
        details.len(),
        2,
        "REPAIR 1 req #2: two concurrent attempts must persist two distinct evidence rows: \
         {details:?}"
    );
    let eval_ids: std::collections::HashSet<String> = details
        .iter()
        .map(|d| {
            d["evaluation_id"]
                .as_str()
                .expect("evaluation_id")
                .to_string()
        })
        .collect();
    assert_eq!(
        eval_ids.len(),
        2,
        "REPAIR 1 req #2: concurrent attempts must never collide on evaluation_id: {details:?}"
    );

    clear_env();
    cleanup(&dir);
    cleanup_bars(&pool, symbol).await;
}

// ---------------------------------------------------------------------------
// REPAIR 3: synthetic `RuntimeStartEffects` fake — never constructs a
// broker, provider, or scheduler. `spawn_loop` genuinely spawns a no-op
// counter-based task (proving "loop spawned" is a real async task, not
// merely a log line) rather than a bar-driven/broker-driven execution loop.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeRuntimeStartEffects {
    start_runtime_effects_calls: std::sync::atomic::AtomicU32,
    spawn_loop_calls: std::sync::atomic::AtomicU32,
}

#[async_trait::async_trait]
impl mqk_daemon::daily_data_readiness::RuntimeStartEffects for FakeRuntimeStartEffects {
    async fn start_runtime_effects(
        &self,
        _run_id: uuid::Uuid,
    ) -> Result<(), mqk_daemon::daily_data_readiness::RuntimeStartEffectsError> {
        self.start_runtime_effects_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    async fn spawn_loop(
        &self,
        _run_id: uuid::Uuid,
    ) -> Result<(), mqk_daemon::daily_data_readiness::RuntimeStartEffectsError> {
        self.spawn_loop_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = std::sync::Arc::clone(&counter);
        let handle = tokio::spawn(async move {
            counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        handle.await.expect("synthetic loop task join");
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "synthetic loop task must actually run"
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SG-16 — REPAIR 3: synthetic lifecycle proof. Drives run creation, pre-start
// evidence, run-linked evidence, and loop spawn through the exact production
// sequencing (`AppState::create_or_reuse_run_for_start` +
// `daily_data_readiness::advance_run_to_active`) that
// `start_execution_runtime` itself now calls — never a separate test-only
// reimplementation of this ordering. Proves the required ordering trace, a
// shared evaluation_id across pre-start and run-linked evidence, and zero
// broker/provider/network activity (the fake is structurally incapable of
// either).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_16_synthetic_ready_start_proves_ordering_and_shared_evaluation_id() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("SG-16").await else {
        return;
    };
    clear_env();
    let symbol = "ZZSG16";
    let provider_id = "zzsg16_provider";
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

    // A plain DB upsert (no broker/provider involved) so this attempt can
    // progress past B2A — unlike SG-10, which deliberately stopped there.
    mqk_db::upsert_strategy_registry_entry(
        &pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: "intraday_scalper".to_string(),
            display_name: "intraday_scalper".to_string(),
            enabled: true,
            kind: "test".to_string(),
            registered_at_utc: now,
            updated_at_utc: now,
            note: "SG-16 synthetic fixture".to_string(),
        },
    )
    .await
    .expect("seed strategy registry");

    delete_evidence_for(&pool, symbol).await;

    // --- readiness_evaluated: the exact production composition every
    // readiness surface (routes + the runtime start gate) shares.
    let config =
        mqk_daemon::state::build_multi_symbol_runtime_config_from_env().expect("config resolves");
    let fleet_ids = Some(vec!["intraday_scalper".to_string()]);
    let (_bootstrap, binding) =
        mqk_runtime::native_strategy::bootstrap_with_effective_binding(fleet_ids.as_deref());
    let context = mqk_daemon::daily_data_readiness::load_readiness_context_from_env();
    let report = mqk_daemon::daily_data_readiness::evaluate_readiness_with_binding(
        Some(&pool),
        &config,
        &binding,
        &context,
        now,
    )
    .await;
    assert!(
        report.start_allowed,
        "SG-16 precondition: readiness must be ready: {report:?}"
    );
    let mut trace: Vec<&'static str> = Vec::new();
    trace.push("readiness_evaluated");

    // --- pre_start_event_persisted: the exact production evidence-write
    // call the start gate makes.
    let evaluation_id =
        mqk_daemon::daily_data_readiness::compute_evaluation_id(now, 0, &binding, &config);
    let persisted = mqk_daemon::daily_data_readiness::persist_pre_start_readiness_evidence(
        &pool,
        evaluation_id,
        now,
        "applicable",
        &report,
    )
    .await;
    assert!(persisted, "SG-16: pre-start evidence must persist");
    trace.push("pre_start_event_persisted");

    // --- run_created: the exact production run lookup/creation method.
    clear_any_preexisting_active_daemon_run(&pool).await;
    let st = std::sync::Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca));
    let run_id = st
        .create_or_reuse_run_for_start(&pool)
        .await
        .expect("SG-16: run creation must succeed");
    trace.push("run_created");

    // --- run_link_event_persisted -> loop_spawned: the exact shared
    // sequencing function production now calls, against a synthetic fake
    // that never constructs a broker/provider/scheduler.
    let fake = FakeRuntimeStartEffects::default();
    let result = mqk_daemon::daily_data_readiness::advance_run_to_active(
        &pool,
        &fake,
        run_id,
        Some((evaluation_id, chrono::Utc::now())),
        &mut trace,
    )
    .await;
    assert!(
        result.is_ok(),
        "SG-16: advance_run_to_active must succeed: {result:?}"
    );

    assert_eq!(
        trace,
        vec![
            "readiness_evaluated",
            "pre_start_event_persisted",
            "run_created",
            "run_link_event_persisted",
            "loop_spawned",
        ],
        "REPAIR 3: required ordering trace"
    );
    assert_eq!(
        fake.start_runtime_effects_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "REPAIR 3: runtime effects must be invoked exactly once"
    );
    assert_eq!(
        fake.spawn_loop_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "REPAIR 3: the loop must be spawned exactly once"
    );

    // req #17/#18/#19: shared evaluation_id + link-before-spawn ordering —
    // the run_linked row's evaluation_id must match the pre-start row's.
    let linked_row: (String,) = sqlx::query_as(
        "select detail from sys_autonomous_session_events \
         where event_type = $1 and run_id = $2",
    )
    .bind(mqk_daemon::daily_data_readiness::EVENT_TYPE_READINESS_RUN_LINKED)
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .expect("fetch run_linked row");
    let linked_detail: serde_json::Value =
        serde_json::from_str(&linked_row.0).expect("run_linked JSON");
    assert_eq!(
        linked_detail["evaluation_id"],
        evaluation_id.to_string(),
        "req #17: run-linked evidence must carry the same evaluation_id as the pre-start event"
    );
    assert_eq!(linked_detail["run_id"], run_id.to_string());

    let pre_start_details = fetch_evidence_details(&pool, symbol).await;
    assert_eq!(pre_start_details.len(), 1);
    assert_eq!(
        pre_start_details[0]["evaluation_id"],
        evaluation_id.to_string(),
        "req #17: pre-start evidence must carry the same evaluation_id"
    );

    // The fake never arms this run (it stays `Created` forever), so it must
    // be removed rather than left for a later test's `fetch_latest_run_for_engine`
    // to silently reuse (a `Created` run is eligible for reuse).
    delete_run_and_its_events(&pool, run_id).await;
    remove_strategy_registry_entry(&pool, "intraday_scalper").await;
    clear_env();
    cleanup(&dir);
    cleanup_bars(&pool, symbol).await;
}

// ---------------------------------------------------------------------------
// SG-17 — REPAIR 4: run-linked evidence write failure fails closed. No
// runtime effects, no loop spawn; the error carries the evaluation_id and
// run_id; pre-start evidence remains durable; no false run_linked record.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_17_run_link_persist_failure_fails_closed_no_effects_invoked() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("SG-17").await else {
        return;
    };
    clear_env();
    let symbol = "ZZSG17";
    let provider_id = "zzsg17_provider";
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

    mqk_db::upsert_strategy_registry_entry(
        &pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: "intraday_scalper".to_string(),
            display_name: "intraday_scalper".to_string(),
            enabled: true,
            kind: "test".to_string(),
            registered_at_utc: now,
            updated_at_utc: now,
            note: "SG-17 synthetic fixture".to_string(),
        },
    )
    .await
    .expect("seed strategy registry");

    delete_evidence_for(&pool, symbol).await;

    let config =
        mqk_daemon::state::build_multi_symbol_runtime_config_from_env().expect("config resolves");
    let fleet_ids = Some(vec!["intraday_scalper".to_string()]);
    let (_bootstrap, binding) =
        mqk_runtime::native_strategy::bootstrap_with_effective_binding(fleet_ids.as_deref());
    let context = mqk_daemon::daily_data_readiness::load_readiness_context_from_env();
    let report = mqk_daemon::daily_data_readiness::evaluate_readiness_with_binding(
        Some(&pool),
        &config,
        &binding,
        &context,
        now,
    )
    .await;
    assert!(
        report.start_allowed,
        "SG-17 precondition: readiness must be ready: {report:?}"
    );
    let evaluation_id =
        mqk_daemon::daily_data_readiness::compute_evaluation_id(now, 0, &binding, &config);
    let persisted = mqk_daemon::daily_data_readiness::persist_pre_start_readiness_evidence(
        &pool,
        evaluation_id,
        now,
        "applicable",
        &report,
    )
    .await;
    assert!(persisted, "SG-17: pre-start evidence must persist");

    clear_any_preexisting_active_daemon_run(&pool).await;
    let st = std::sync::Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca));
    let run_id = st
        .create_or_reuse_run_for_start(&pool)
        .await
        .expect("SG-17: run creation must succeed");

    // A genuinely separate pool, closed immediately — forces ONLY the
    // run-linked evidence write to fail, without touching the healthy
    // `pool` already used above for run creation / pre-start evidence.
    let db_url = std::env::var("MQK_DATABASE_URL").expect("MQK_DATABASE_URL");
    let bad_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await
        .expect("connect bad_pool");
    bad_pool.close().await;

    let fake = FakeRuntimeStartEffects::default();
    let mut trace: Vec<&'static str> = Vec::new();
    let result = mqk_daemon::daily_data_readiness::advance_run_to_active(
        &bad_pool,
        &fake,
        run_id,
        Some((evaluation_id, chrono::Utc::now())),
        &mut trace,
    )
    .await;

    match result {
        Err(
            mqk_daemon::daily_data_readiness::RuntimeStartSequenceError::RunLinkPersistFailed {
                evaluation_id: got_eval,
                run_id: got_run,
            },
        ) => {
            assert_eq!(
                got_eval, evaluation_id,
                "req #4: error must carry the evaluation_id"
            );
            assert_eq!(got_run, run_id, "req #4: error must carry the run_id");
        }
        other => panic!("SG-17 (REPAIR 4): expected RunLinkPersistFailed, got {other:?}"),
    }

    assert_eq!(
        fake.start_runtime_effects_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "req #3: a run-linked evidence failure must never invoke runtime effects \
         (no arm/begin/tick)"
    );
    assert_eq!(
        fake.spawn_loop_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "req #3: a run-linked evidence failure must never spawn the execution loop"
    );
    assert!(
        trace.is_empty(),
        "req #3: no ordered-trace tag may fire past the failed link: {trace:?}"
    );

    // req #6: no false run_linked record exists for this run.
    let linked_count: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_session_events where event_type = $1 and run_id = $2",
    )
    .bind(mqk_daemon::daily_data_readiness::EVENT_TYPE_READINESS_RUN_LINKED)
    .bind(run_id)
    .fetch_one(&pool)
    .await
    .expect("count run_linked events");
    assert_eq!(
        linked_count.0, 0,
        "req #6: no run_linked evidence may be claimed on a failed write"
    );

    // req #5: pre-start evidence remains durable and references the same
    // evaluation_id, unaffected by the later run-link failure.
    let pre_start_details = fetch_evidence_details(&pool, symbol).await;
    assert_eq!(pre_start_details.len(), 1);
    assert_eq!(
        pre_start_details[0]["evaluation_id"],
        evaluation_id.to_string(),
        "req #5: pre-start evidence must remain durable and reference the same evaluation_id"
    );

    delete_run_and_its_events(&pool, run_id).await;
    remove_strategy_registry_entry(&pool, "intraday_scalper").await;
    clear_env();
    cleanup(&dir);
    cleanup_bars(&pool, symbol).await;
}

// ---------------------------------------------------------------------------
// DAILY-DATA-READINESS-01C-MISSING-ASSIGNMENT-EVIDENCE-REPAIR-01
//
// SG-18..SG-21 — an applicable strict start-gate attempt whose assignment
// resolution itself fails (`build_multi_symbol_runtime_config_from_env()`
// returns `Err`) must still receive a distinct start-attempt sequence, a
// real `evaluation_id`, a truthful bounded blocked report, and a genuine
// pre-start evidence persist attempt — never an early return before any of
// that exists (the exact defect this repair closes).
// ---------------------------------------------------------------------------

/// Parses `evaluation_id=<uuid>;` out of a `RuntimeLifecycleError`'s display
/// string. This is the only channel available to this integration-test
/// binary for observing the real `evaluation_id` the production start gate
/// computed — `AppState::next_daily_data_readiness_attempt_seq` is
/// `pub(crate)`, not visible outside the `mqk-daemon` library crate.
fn extract_evaluation_id(err_display: &str) -> uuid::Uuid {
    let marker = "evaluation_id=";
    let start = err_display
        .find(marker)
        .unwrap_or_else(|| panic!("evaluation_id= not found in: {err_display}"))
        + marker.len();
    let rest = &err_display[start..];
    let end = rest.find(';').unwrap_or(rest.len());
    uuid::Uuid::parse_str(rest[..end].trim())
        .unwrap_or_else(|e| panic!("evaluation_id not a valid uuid in '{}': {e}", &rest[..end]))
}

async fn delete_evidence_row(pool: &sqlx::PgPool, evaluation_id: uuid::Uuid) {
    let _ = sqlx::query("delete from sys_autonomous_session_events where id = $1")
        .bind(format!("daily_data_readiness_evaluated:{evaluation_id}"))
        .execute(pool)
        .await;
}

/// Fresh AppState with a working DB, WS continuity live, integrity armed,
/// and an active (non-dormant) strategy fleet — every gate up to and
/// including B1A passes, but deliberately NO
/// `MQK_STRATEGY_SYMBOL`/`MQK_STRATEGY_IDS`/`MQK_STRATEGY_MD_TIMEFRAME`/
/// watchlist artifact is configured, so
/// `build_multi_symbol_runtime_config_from_env()` fails closed
/// (`MissingSymbol` -> `required_assignments_missing`) at the Phase C gate.
async fn db_backed_state_with_no_assignment_source(
    pool: &sqlx::PgPool,
) -> std::sync::Arc<AppState> {
    let mut st_raw = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st_raw.db = Some(pool.clone());
    let st = std::sync::Arc::new(st_raw);
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "ddr-missing-assignment-test-msg".to_string(),
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
    st
}

// ---------------------------------------------------------------------------
// SG-18 — missing assignments with a working DB: `required_assignments_missing`,
// a nonempty `evaluation_id`, `evidence_persisted=true`; the persisted event
// has the right event type/id/start_allowed/run_id/assignment-count; and no
// run/outbox/run-linked-event/loop is created (req #1, #2, #6, #7, #8, #9).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_18_missing_assignments_with_db_persists_evidence_identity_and_creates_no_run_or_outbox()
{
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("SG-18").await else {
        return;
    };
    clear_env();

    let runs_before = count(&pool, "runs").await;
    let outbox_before = count(&pool, "oms_outbox").await;

    let st = db_backed_state_with_no_assignment_source(&pool).await;

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SG-18: missing assignments must refuse start");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.daily_data_readiness_blocked"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("required_assignments_missing"),
        "SG-18 req #1: error must name the top-level blocker: {msg}"
    );
    assert!(
        msg.contains("evidence_persisted=true"),
        "SG-18 req #1: a working DB must honestly report evidence_persisted=true: {msg}"
    );
    let evaluation_id = extract_evaluation_id(&msg);

    let row: (String, Option<uuid::Uuid>) = sqlx::query_as(
        "select detail, run_id from sys_autonomous_session_events \
         where event_type = 'daily_data_readiness_evaluated' and id = $1",
    )
    .bind(format!("daily_data_readiness_evaluated:{evaluation_id}"))
    .fetch_one(&pool)
    .await
    .expect("SG-18 req #2: pre-start evidence row must exist for this evaluation_id");
    let detail: serde_json::Value = serde_json::from_str(&row.0).expect("evidence JSON");
    assert_eq!(detail["evaluation_id"], evaluation_id.to_string());
    assert_eq!(detail["binding_scope"], "start_attempt_binding");
    assert_eq!(detail["start_allowed"], false, "SG-18 req #2: {detail}");
    assert_eq!(
        detail["assignment_count"], 0,
        "SG-18 req #2: zero assignment summaries: {detail}"
    );
    assert_eq!(
        detail["assignments"].as_array().map(Vec::len),
        Some(0),
        "SG-18 req #2: zero assignment summaries: {detail}"
    );
    assert!(
        row.1.is_none(),
        "SG-18 req #2: run_id must be NULL for a pre-start event"
    );

    assert_eq!(
        runs_before,
        count(&pool, "runs").await,
        "SG-18 req #6: no run row"
    );
    assert_eq!(
        outbox_before,
        count(&pool, "oms_outbox").await,
        "SG-18 req #9: no outbox row"
    );
    assert!(
        st.native_strategy_bootstrap_truth_state_for_test()
            .await
            .is_none(),
        "SG-18 req #8: bootstrap is only stored after a fully successful start — \
         its absence proves no execution loop was spawned"
    );
    let linked_count: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_session_events where event_type = $1 and detail like $2",
    )
    .bind(mqk_daemon::daily_data_readiness::EVENT_TYPE_READINESS_RUN_LINKED)
    .bind(format!("%{evaluation_id}%"))
    .fetch_one(&pool)
    .await
    .expect("count run_linked events");
    assert_eq!(
        linked_count.0, 0,
        "SG-18 req #7: no run-linked evidence may exist for this evaluation_id"
    );

    delete_evidence_row(&pool, evaluation_id).await;
    clear_env();
}

// ---------------------------------------------------------------------------
// SG-19 — two sequential identical missing-assignment attempts under the
// same injected clock produce two different evaluation_ids and two distinct
// event rows (req #3).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_19_sequential_identical_missing_assignment_attempts_produce_distinct_evaluation_ids() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("SG-19").await else {
        return;
    };
    clear_env();

    let st = db_backed_state_with_no_assignment_source(&pool).await;

    let err1 = st
        .start_execution_runtime()
        .await
        .expect_err("SG-19 attempt 1: missing assignments must block");
    let err2 = st
        .start_execution_runtime()
        .await
        .expect_err("SG-19 attempt 2: missing assignments must block");

    let id1 = extract_evaluation_id(&err1.to_string());
    let id2 = extract_evaluation_id(&err2.to_string());
    assert_ne!(
        id1, id2,
        "SG-19 req #3: two identical attempts against an identical clock must receive \
         distinct evaluation_ids"
    );

    for id in [id1, id2] {
        let exists: (i64,) =
            sqlx::query_as("select count(*) from sys_autonomous_session_events where id = $1")
                .bind(format!("daily_data_readiness_evaluated:{id}"))
                .fetch_one(&pool)
                .await
                .expect("count evidence row");
        assert_eq!(
            exists.0, 1,
            "SG-19 req #3: each attempt must persist its own distinct evidence row: {id}"
        );
    }

    delete_evidence_row(&pool, id1).await;
    delete_evidence_row(&pool, id2).await;
    clear_env();
}

// ---------------------------------------------------------------------------
// SG-20 — two concurrent identical missing-assignment attempts must also
// receive distinct evaluation_ids (req #4).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_20_concurrent_identical_missing_assignment_attempts_produce_distinct_evaluation_ids() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("SG-20").await else {
        return;
    };
    clear_env();

    let st = db_backed_state_with_no_assignment_source(&pool).await;

    // `AppState::lifecycle_op` serializes `start_execution_runtime` calls
    // internally (SG-13's same reasoning applies here) — this test proves
    // the OUTCOME two genuinely concurrent callers observe.
    let st_a = std::sync::Arc::clone(&st);
    let st_b = std::sync::Arc::clone(&st);
    let (res_a, res_b) = tokio::join!(
        async move { st_a.start_execution_runtime().await },
        async move { st_b.start_execution_runtime().await },
    );
    let err_a = res_a.expect_err("SG-20: attempt A must block");
    let err_b = res_b.expect_err("SG-20: attempt B must block");

    let id_a = extract_evaluation_id(&err_a.to_string());
    let id_b = extract_evaluation_id(&err_b.to_string());
    assert_ne!(
        id_a, id_b,
        "SG-20 req #4: concurrent attempts must never collide on evaluation_id"
    );

    for id in [id_a, id_b] {
        let exists: (i64,) =
            sqlx::query_as("select count(*) from sys_autonomous_session_events where id = $1")
                .bind(format!("daily_data_readiness_evaluated:{id}"))
                .fetch_one(&pool)
                .await
                .expect("count evidence row");
        assert_eq!(
            exists.0, 1,
            "SG-20 req #4: each concurrent attempt must persist its own distinct evidence row: {id}"
        );
    }

    delete_evidence_row(&pool, id_a).await;
    delete_evidence_row(&pool, id_b).await;
    clear_env();
}

// ---------------------------------------------------------------------------
// SG-21 — missing assignments with no DB configured must still return the
// original blocker and a real evaluation_id, honestly reporting
// `evidence_persisted=false` (req #5).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sg_21_missing_assignments_no_db_returns_original_blocker_with_evaluation_id_and_evidence_not_persisted(
) {
    let _g = env_lock().lock().await;
    clear_env();
    let dir = write_registries();
    // Same fixture shape as SG-01/SG-05: no assignment source configured,
    // and no DB attached to this AppState at all.
    let st = ready_state_with_fleet(Some("intraday_scalper")).await;
    assert!(st.db.is_none(), "SG-21: precondition — no DB configured");

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("SG-21: missing assignments with no DB must refuse start");
    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.daily_data_readiness_blocked"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("required_assignments_missing"),
        "SG-21 req #5: error must name the original blocker: {msg}"
    );
    assert!(
        msg.contains("evidence_persisted=false"),
        "SG-21 req #5: no DB means evidence_persisted must honestly be false: {msg}"
    );
    // req #5: a real, well-formed evaluation_id must still be present —
    // `extract_evaluation_id` panics if it is missing or malformed.
    let _evaluation_id = extract_evaluation_id(&msg);

    clear_env();
    cleanup(&dir);
}
