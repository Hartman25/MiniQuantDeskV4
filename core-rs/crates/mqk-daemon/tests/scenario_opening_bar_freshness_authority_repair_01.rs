//! OPENING-BAR-FRESHNESS-AUTHORITY-REPAIR-01 — end-to-end proof.
//!
//! ## Background
//!
//! 2026-08-14 (Friday): the autonomous coordinator attempted its first
//! canonical runtime start at T+45s after the regular session opened. No
//! 5-minute bar of the new session could possibly exist yet (the earliest
//! possible completed bar is T+300s). `daily_data_readiness` correctly fell
//! back to expecting the *previous* session's tail and reported ready, but
//! the older `market_data_freshness` gate inside `start_execution_runtime`
//! independently re-checked wall-clock bar age against a flat 900s
//! threshold, saw the (necessarily prior-session) latest bar as stale, and
//! refused with `runtime.start_refused.market_data_not_fresh` — a fault
//! class the autonomous retry policy classifies `ManualInterventionRequired`.
//! The day was durably stuck: `POST /api/v1/autonomous/daily-operation/retry`
//! does not recognize this reason code either (its recoverable set is keyed
//! off `daily_data_readiness` reason codes, not this legacy gate's).
//!
//! ## This patch
//!
//! Neither readiness authority is removed or weakened. At the exact point
//! `start_execution_runtime` would return `market_data_not_fresh`, it now
//! additionally checks whether every blocking symbol is blocked *only* by
//! `"stale"` (never `"missing"`/`"insufficient"`) and structurally cannot
//! have a fresher bar yet
//! (`market_data_freshness::is_awaiting_first_session_bar`, reusing
//! `daily_data_readiness`'s own grace/timeframe helpers — no new duration
//! constant). When that holds, it returns the new
//! `runtime.start_refused.latest_completed_bar_pending` fault class instead,
//! which `autonomous_retry_policy` classifies `WaitForCondition` — the
//! coordinator retries on its own bounded backoff instead of durably parking
//! the day in `manual_intervention_required`. Every other case (missing/
//! insufficient data, or stale outside this narrow window) is completely
//! unchanged.
//!
//! ## Test
//!
//! | Test    | Claim |
//! |---------|-------|
//! | OBF-01  | T+45s after open, only prior-session bars exist: `start_execution_runtime` returns `latest_completed_bar_pending`, NOT `market_data_not_fresh`; the autonomous retry policy classifies it `WaitForCondition` |
//! | OBF-02  | No run/outbox row is created by the softened refusal (still a refusal, not a bypass) |
//!
//! Run alone (requires a local test Postgres, e.g. `mqk-test-postgres` on
//! `:5434` — never the paper DB on `:5440`):
//! `MQK_DATABASE_URL=postgres://postgres:postgres@localhost:5434/mqk_test cargo test -p mqk-daemon --test scenario_opening_bar_freshness_authority_repair_01 -- --test-threads=1`

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use chrono::{DateTime, TimeZone, Utc};
use mqk_daemon::state::autonomous_retry_policy::{
    classify_autonomous_reason, coordinator_reason_from_runtime_lifecycle_error,
    AutonomousCoordinatorReason, AutonomousRetryClass,
};
use mqk_daemon::state::{
    market_calendar::{resolve_market_session_schedule, NyseWeekdaysProvider},
    AlpacaWsContinuityState, AppState, BrokerKind, StrategyFleetEntry,
};
use tokio::sync::Mutex;

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
        std::env::remove_var("MQK_DATA_READINESS_GRACE_SECS");
        std::env::remove_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS");
    }
}

// Monday 2024-04-15, well inside NyseWeekdaysProvider's covered range —
// same reference date every other daily-data-readiness scenario test file
// in this crate uses.
const REF_INSTANT: i64 = 1_713_188_100; // 2024-04-15 13:35 UTC

/// `now` = the regular session's own open instant + 45 seconds — the exact
/// temporal shape of the 2026-08-14 production incident (T+45s).
fn now_45s_after_open() -> DateTime<Utc> {
    let provider = NyseWeekdaysProvider;
    let ref_now = Utc.timestamp_opt(REF_INSTANT, 0).single().expect("valid ts");
    let schedule = resolve_market_session_schedule(&provider, ref_now);
    schedule.session_open_utc + chrono::Duration::seconds(45)
}

/// The exact bar `end_ts` window `daily_data_readiness` itself expects at
/// `now` — reusing its own canonical helper (never hand-rolled arithmetic)
/// so the seeded bars are byte-identical to what the strict readiness
/// evaluator will check for. At T+45s after open this falls back entirely
/// to the previous trading session's tail (mirrors
/// `scenario_daily_data_readiness_start_gate_01.rs`'s SG-09/SG-16 pattern).
fn expected_bars_at(now: DateTime<Utc>) -> Vec<i64> {
    let provider = NyseWeekdaysProvider;
    let schedule = resolve_market_session_schedule(&provider, now);
    mqk_daemon::daily_data_readiness::expected_intraday_end_ts_window(
        &provider,
        &schedule,
        now.timestamp(),
        300,
        0,
        5,
    )
    .expect("expected window resolves")
}

static COUNTER: AtomicU32 = AtomicU32::new(0);
fn next_id() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn write_registries_for(symbol: &str, provider: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mqk_obf01_registry_{}_{}",
        std::process::id(),
        next_id()
    ));
    std::fs::create_dir_all(&dir).expect("create registry dir");

    std::fs::write(
        dir.join("instruments.json"),
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
    "notes": "OPENING-BAR-FRESHNESS-AUTHORITY-REPAIR-01 synthetic fixture"
  }}
]"#
        ),
    )
    .expect("write instruments fixture");

    std::fs::write(
        dir.join("providers.json"),
        format!(
            r#"[
  {{
    "provider_id": "{provider}",
    "display_name": "OBF01 Test Provider",
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
        std::env::set_var("MQK_INSTRUMENT_REGISTRY_PATH", dir.join("instruments.json"));
        std::env::set_var("MQK_PROVIDER_REGISTRY_PATH", dir.join("providers.json"));
    }
    dir
}

fn cleanup(dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(dir);
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

async fn remove_strategy_registry_entry(pool: &sqlx::PgPool, strategy_id: &str) {
    let _ = sqlx::query("delete from sys_strategy_registry where strategy_id = $1")
        .bind(strategy_id)
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// OBF-01/OBF-02
// ---------------------------------------------------------------------------

#[tokio::test]
async fn obf_01_45s_after_open_returns_latest_completed_bar_pending_and_coordinator_waits() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("OBF-01").await else {
        return;
    };
    clear_env();
    let symbol = "ZZOBF01";
    let provider_id = "obf01_provider";
    let dir = write_registries_for(symbol, provider_id);
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", symbol);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
        std::env::set_var("MQK_DATA_READINESS_GRACE_SECS", "0");
        std::env::set_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS", "60");
    }

    let now = now_45s_after_open();
    // daily_data_readiness's own expected window at T+45s — falls back
    // entirely to the previous trading session's tail (no current-session
    // grid slot can qualify this early regardless of grace).
    let expected = expected_bars_at(now);
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
            note: "OBF-01 synthetic fixture".to_string(),
        },
    )
    .await
    .expect("seed strategy registry");

    let runs_before = count(&pool, "runs").await;
    let outbox_before = count(&pool, "oms_outbox").await;

    let mut st_raw = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st_raw.db = Some(pool.clone());
    let st = std::sync::Arc::new(st_raw);
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "obf01-test-msg".to_string(),
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

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("OBF-01: a structurally pending first bar must still refuse start");

    assert_eq!(
        err.fault_class(),
        "runtime.start_refused.latest_completed_bar_pending",
        "OBF-01: must be the NEW fault class, never the old \
         market_data_not_fresh (which durably traps the day in \
         manual_intervention_required): {err}"
    );
    assert!(
        err.to_string()
            .contains("OPENING-BAR-FRESHNESS-AUTHORITY-REPAIR-01"),
        "OBF-01: error message must cite this patch: {err}"
    );

    // The coordinator's own retry classification for this exact error must
    // be WaitForCondition, never ManualInterventionRequired — this is the
    // real proof that today's autonomous soak would NOT get durably stuck.
    let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
    assert_eq!(reason, AutonomousCoordinatorReason::LatestCompletedBarPending);
    assert_eq!(
        classify_autonomous_reason(&reason),
        AutonomousRetryClass::WaitForCondition,
        "OBF-01: the autonomous coordinator must retry on its own bounded \
         backoff, never park in manual_intervention_required"
    );

    // OBF-02: still a genuine refusal, not a bypass — no run/outbox row.
    assert_eq!(
        runs_before,
        count(&pool, "runs").await,
        "OBF-02: a softened-but-still-refused start must create no run row"
    );
    assert_eq!(
        outbox_before,
        count(&pool, "oms_outbox").await,
        "OBF-02: a softened-but-still-refused start must create no outbox row"
    );

    remove_strategy_registry_entry(&pool, "intraday_scalper").await;
    clear_env();
    cleanup(&dir);
    cleanup_bars(&pool, symbol).await;
}
