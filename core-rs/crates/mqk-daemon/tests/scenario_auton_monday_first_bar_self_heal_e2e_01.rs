//! AUTON-MONDAY-FIRST-BAR-SELF-HEAL-E2E
//!
//! Combined integration proof for
//! PAPER-AUTONOMOUS-STARTUP-THREE-DEFECT-CLOSURE-01, run only after all
//! three defect patches pass their own individual targeted proofs:
//!
//!   1. AUTON-FIRST-BAR-FRESHNESS-WAIT-SEMANTICS-01
//!      (`market_data_freshness::is_awaiting_first_session_bar`,
//!      `state::lifecycle::readiness_blocked_only_by_pending_first_session_bar`)
//!   2. AUTON-LEGACY-FRESHNESS-OPERATOR-RETRY-01
//!      (`routes::autonomous_daily_operator::RECOVERABLE_PREFLIGHT_REASON_CODES`)
//!   3. AUTON-PRESTART-OBSERVATION-RETRY-SAFETY-01
//!      (`routes::autonomous_daily_operator::check_prestart_retry_safety`)
//!
//! ## What this file proves that no single defect's own test file proves alone
//!
//! Patch 1 by itself only proves the *refusal* changes shape (OBF-01 in
//! `scenario_opening_bar_freshness_authority_repair_01.rs`). It does not,
//! alone, prove that once the first bar actually exists, the exact same
//! `start_execution_runtime` call path stops refusing on freshness grounds
//! at all -- i.e. that patch 1 is a genuine *wait*, not a permanent
//! reclassification. This file drives `start_execution_runtime` twice
//! against the same fixture, at two points on the same session timeline,
//! through the real (unmocked) calendar/freshness/readiness code:
//!
//!   - T1 (open+45s): only the prior session's tail exists -> refused,
//!     `latest_completed_bar_pending`, `WaitForCondition`, zero run/outbox
//!     rows (self_heal_01).
//!   - T4 (open+301s, current bar now published): the freshness gate no
//!     longer refuses on ANY market-data-freshness ground (self_heal_01
//!     continued) -- proven by asserting the resulting error, if any, does
//!     not carry `"market_data_not_fresh"` or `"latest_completed_bar_pending"`
//!     in its fault_class, i.e. this specific authority has cleared. (A
//!     residual refusal from an unrelated, unaudited downstream gate --
//!     capital policy, parity evidence, WS continuity, native strategy
//!     bootstrap -- is out of scope for this file; those are independently
//!     gated and independently tested elsewhere and this mission does not
//!     touch them.)
//!
//! T2 (bar due but never arrives -> normal fail-closed, never a bypass) is
//! covered by the companion test below. T3 (a `PrepareDataOnly`-shaped bar
//! *observation* recorded mid-wait does not poison operator-retry safety)
//! is deliberately NOT re-proven here: `check_prestart_retry_safety` is
//! `pub(crate)` inside a `pub(crate) mod routes::autonomous_daily_operator`,
//! unreachable from an external integration-test crate, and the exact same
//! claim (a `bars_observed`-only fixture built from the real
//! `mqk_db::record_completed_bar_observed` recorder is `Safe`, while
//! `check_operation_pristine` on the identical fixture remains
//! `HasActivity`) is already proven, without duplication, by
//! `routes::autonomous_daily_operator::prestart_retry_safety_tests::
//! bars_observed_only_is_safe` /
//! `coverage_pristine_check_is_unaffected_and_still_reports_has_activity`
//! (inline unit tests, confirmed green this session) and end-to-end through
//! the real HTTP retry route by
//! `scenario_autonomous_daily_operator_retry_01.rs`'s
//! `t_prestart_bars_observed_only_retry_succeeds` (also confirmed green).
//!
//! T5-T7 (bounded dispatch exactly once, no duplicate dispatch, valid
//! no-signal evaluation) are proven by pre-existing, unmodified
//! infrastructure this mission's three patches never touch --
//! `scenario_autonomous_completed_bar_driver_01.rs`'s
//! `preopen_to_running_lifecycle_26_35_exactly_once_dispatch` and
//! `scenario_autonomous_completed_bar_task_01.rs`'s
//! `m01_task_level_prepare_to_running_exactly_once` -- confirmed still green
//! in this same session; not duplicated here.
//!
//! Run alone (requires a local test Postgres, e.g. `mqk-test-postgres` on
//! `:5434` -- never the paper DB on `:5440`):
//! `MQK_DATABASE_URL=postgres://postgres:postgres@localhost:5434/mqk_test cargo test -p mqk-daemon --test scenario_auton_monday_first_bar_self_heal_e2e_01 -- --test-threads=1`

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use chrono::{DateTime, TimeZone, Utc};
use mqk_daemon::state::autonomous_retry_policy::{
    classify_autonomous_reason, coordinator_reason_from_runtime_lifecycle_error, AutonomousRetryClass,
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

// Monday 2024-04-15, same reference date every other daily-data-readiness
// scenario test file in this crate uses.
const REF_INSTANT: i64 = 1_713_188_100; // 2024-04-15 13:35 UTC

fn session_open() -> DateTime<Utc> {
    let provider = NyseWeekdaysProvider;
    let ref_now = Utc.timestamp_opt(REF_INSTANT, 0).single().expect("valid ts");
    resolve_market_session_schedule(&provider, ref_now).session_open_utc
}

static COUNTER: AtomicU32 = AtomicU32::new(0);
fn next_id() -> u32 {
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn write_registries_for(symbol: &str, provider: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mqk_monday_e2e_registry_{}_{}",
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
    "notes": "AUTON-MONDAY-FIRST-BAR-SELF-HEAL-E2E synthetic fixture"
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
    "display_name": "Monday E2E Test Provider",
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

fn cleanup_dir(dir: &std::path::Path) {
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

async fn seed_5m_bars(pool: &sqlx::PgPool, symbol: &str, provider: &str, end_ts_list: &[i64]) {
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = '5m'")
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

fn build_state(pool: sqlx::PgPool) -> std::sync::Arc<AppState> {
    let mut st_raw = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
    st_raw.db = Some(pool);
    std::sync::Arc::new(st_raw)
}

async fn arm_for_start(st: &std::sync::Arc<AppState>, strategy_id: &str) {
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "monday-e2e-test-msg".to_string(),
        last_event_at: "2026-04-14T15:00:00Z".to_string(),
    })
    .await;
    st.integrity.write().await.disarmed = false;
    st.set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
        strategy_id: strategy_id.to_string(),
    }]))
    .await;
}

// ---------------------------------------------------------------------------
// self_heal_01 -- T1 (wait) then T4 (clears) on the SAME fixture, no
// operator retry involved anywhere in this path: the coordinator's own
// bounded backoff is the only thing this test relies on.
// ---------------------------------------------------------------------------

// T4 in this test progresses past the freshness gate into orchestrator
// bootstrap code that uses `tokio::task::block_in_place` (proof in itself
// that the freshness authority genuinely cleared, not just that this test
// stopped checking) -- that requires the multi-threaded runtime.
#[tokio::test(flavor = "multi_thread")]
async fn self_heal_01_t1_wait_then_t4_freshness_gate_clears_without_manual_intervention() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("SELF-HEAL-01").await else {
        return;
    };
    clear_env();
    let symbol = "ZZMONE2E";
    let provider_id = "monday_e2e_provider";
    let dir = write_registries_for(symbol, provider_id);
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", symbol);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
        std::env::set_var("MQK_DATA_READINESS_GRACE_SECS", "0");
        std::env::set_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS", "60");
    }

    let open = session_open();
    let t1 = open + chrono::Duration::seconds(45);

    mqk_db::upsert_strategy_registry_entry(
        &pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: "intraday_scalper".to_string(),
            display_name: "intraday_scalper".to_string(),
            enabled: true,
            kind: "test".to_string(),
            registered_at_utc: t1,
            updated_at_utc: t1,
            note: "AUTON-MONDAY-FIRST-BAR-SELF-HEAL-E2E synthetic fixture".to_string(),
        },
    )
    .await
    .expect("seed strategy registry");

    let runs_before = count(&pool, "runs").await;
    let outbox_before = count(&pool, "oms_outbox").await;

    // ---- T1: session just opened, only the previous session's tail exists ----
    seed_5m_bars(&pool, symbol, provider_id, &expected_bars_at(t1)).await;

    let st = build_state(pool.clone());
    arm_for_start(&st, "intraday_scalper").await;
    st.set_daily_data_readiness_clock_override_for_test(Some(t1))
        .await;

    let err_t1 = st
        .start_execution_runtime()
        .await
        .expect_err("T1: a structurally pending first bar must still refuse start");
    assert_eq!(
        err_t1.fault_class(),
        "runtime.start_refused.latest_completed_bar_pending",
        "T1: must be the new WaitForCondition fault class, never the old \
         market_data_not_fresh: {err_t1}"
    );
    let reason_t1 = coordinator_reason_from_runtime_lifecycle_error(&err_t1);
    assert_eq!(
        classify_autonomous_reason(&reason_t1),
        AutonomousRetryClass::WaitForCondition,
        "T1: must classify WaitForCondition, never ManualInterventionRequired"
    );
    assert_eq!(
        runs_before,
        count(&pool, "runs").await,
        "T1: no run row from a refused start"
    );
    assert_eq!(
        outbox_before,
        count(&pool, "oms_outbox").await,
        "T1: no outbox row from a refused start"
    );

    // ---- T4: the first bar's interval + grace has now elapsed AND the
    // current-session bar has been published (simulates the completed-bar
    // driver's PrepareDataOnly observation/ingest cycle succeeding during
    // the wait). No operator action of any kind occurs between T1 and T4 in
    // this test -- proving the coordinator's own retry is sufficient. ----
    let t4 = open + chrono::Duration::seconds(301);
    seed_5m_bars(&pool, symbol, provider_id, &expected_bars_at(t4)).await;
    st.set_daily_data_readiness_clock_override_for_test(Some(t4))
        .await;

    match st.start_execution_runtime().await {
        Ok(_) => {
            // Full success also satisfies every downstream gate -- strictly
            // stronger than what this test claims, and fine.
        }
        Err(err_t4) => {
            assert_ne!(
                err_t4.fault_class(),
                "runtime.start_refused.market_data_not_fresh",
                "T4: the market-data-freshness authority must have cleared \
                 once the current-session bar is published: {err_t4}"
            );
            assert_ne!(
                err_t4.fault_class(),
                "runtime.start_refused.latest_completed_bar_pending",
                "T4: the wait condition must have resolved by now, not \
                 persisted: {err_t4}"
            );
            // Any other fault_class here comes from an unrelated,
            // independently-gated downstream authority (capital policy,
            // parity evidence, WS continuity, native strategy bootstrap,
            // ...) that this mission's three patches never touch and this
            // file does not attempt to satisfy or re-prove.
        }
    }

    remove_strategy_registry_entry(&pool, "intraday_scalper").await;
    clear_env();
    cleanup_dir(&dir);
    cleanup_bars(&pool, symbol).await;
}

// ---------------------------------------------------------------------------
// self_heal_02 -- T2: the first bar's interval + grace elapses but the
// current-session bar never arrives (provider outage / genuine gap). The
// carve-out must NOT cover this: it is blocked on "missing", never "stale",
// so the original fail-closed market_data_not_fresh path must still apply
// and no run must be created.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn self_heal_02_t2_bar_due_but_missing_still_fails_closed() {
    let _g = env_lock().lock().await;
    let Some(pool) = db_pool_or_skip("SELF-HEAL-02").await else {
        return;
    };
    clear_env();
    let symbol = "ZZMONE2EB";
    let provider_id = "monday_e2e_provider_b";
    let dir = write_registries_for(symbol, provider_id);
    unsafe {
        std::env::set_var("MQK_STRATEGY_SYMBOL", symbol);
        std::env::set_var("MQK_STRATEGY_IDS", "intraday_scalper_b");
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", "5m");
        std::env::set_var("MQK_DATA_READINESS_GRACE_SECS", "0");
        std::env::set_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS", "60");
    }

    let open = session_open();
    let t2 = open + chrono::Duration::seconds(301); // bar is due; grace elapsed

    mqk_db::upsert_strategy_registry_entry(
        &pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: "intraday_scalper_b".to_string(),
            display_name: "intraday_scalper_b".to_string(),
            enabled: true,
            kind: "test".to_string(),
            registered_at_utc: t2,
            updated_at_utc: t2,
            note: "AUTON-MONDAY-FIRST-BAR-SELF-HEAL-E2E synthetic fixture".to_string(),
        },
    )
    .await
    .expect("seed strategy registry");

    let runs_before = count(&pool, "runs").await;

    // No bars at all -> freshness_state == "missing", never covered by the
    // carve-out regardless of session timing.
    cleanup_bars(&pool, symbol).await;

    let st = build_state(pool.clone());
    arm_for_start(&st, "intraday_scalper_b").await;
    st.set_daily_data_readiness_clock_override_for_test(Some(t2))
        .await;

    let err = st
        .start_execution_runtime()
        .await
        .expect_err("T2: a genuinely missing current-session bar must still refuse start");
    assert_ne!(
        err.fault_class(),
        "runtime.start_refused.latest_completed_bar_pending",
        "T2: the pending-first-bar carve-out must never cover a genuine gap: {err}"
    );
    let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
    assert_eq!(
        classify_autonomous_reason(&reason),
        AutonomousRetryClass::ManualInterventionRequired,
        "T2: a genuine missing-data gap must still durably require operator \
         attention (via the existing remediation path), not be waved through: {err}"
    );
    assert_eq!(
        runs_before,
        count(&pool, "runs").await,
        "T2: no run row from a refused start"
    );

    remove_strategy_registry_entry(&pool, "intraday_scalper_b").await;
    clear_env();
    cleanup_dir(&dir);
    cleanup_bars(&pool, symbol).await;
}

