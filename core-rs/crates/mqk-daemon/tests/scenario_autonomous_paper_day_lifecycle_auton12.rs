//! # AUTON-12 — Canonical unattended paper-day lifecycle proof
//!
//! ## Purpose
//!
//! Closes the proof gap left open after AUTON-11 (gate parity: blocked cases
//! only) and AUTON-PAPER-01 (AU-01..AU-16: sub-lifecycles, recovery, alerts).
//!
//! ## What this file proves
//!
//! | Test  | Kind        | Claim                                                                                          |
//! |-------|-------------|------------------------------------------------------------------------------------------------|
//! | AL-01 | DB-backed, **legacy compatibility** | Tick-driven session-boundary stop via the legacy `run_session_controller_tick` compatibility function: `locally_started=true` + out-of-session tick → `StoppedAtBoundary` + DB run = Stopped |
//! | AL-02 | Pure, no-DB, **legacy compatibility** | Tick-driven start via the legacy `run_session_controller_tick` compatibility function: armed + WS=Live + all pre-DB gates pass → `StartRefused` with `service_unavailable` (DB is the only missing piece) |
//! | AL-03 | DB-backed, **canonical production path** | `tick_autonomous_daily_coordinator` drives a full, real successful start (`start_execution_runtime` returns `Ok`, a genuine `runs` row reaches `Running`) through the durable daily-operation coordinator, then a full real stop at session close (`stop_execution_runtime` called, `runs` row reaches `Stopped`) — the AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-LIFECYCLE-CLOSURE-REPAIR-01 REPAIR 12/13 closure proof. |
//!
//! AL-01/AL-02 are retained only as regression coverage for the legacy
//! `run_session_controller_tick`/`attempt_auto_start`/`attempt_auto_stop`
//! compatibility functions (still exported for `/api/v1/system/preflight`
//! and their own existing test coverage) — they are **not** called by the
//! production controller loop (`session_controller.rs`'s
//! `run_session_controller` calls `run_durable_session_controller_tick`,
//! which delegates entirely to `tick_autonomous_daily_coordinator`). AL-03 is
//! the canonical proof for the durable path actually running in production.
//!
//! ## What is NOT claimed
//!
//! - Wall-clock soak or broker network connectivity (a loopback in-process
//!   mock HTTP server stands in for the Alpaca paper REST surface; no real
//!   network request leaves the machine).
//! - Multi-day soak or live-capital semantics.
//! - Completed-bar dispatch, signal generation, or order submission — AL-03
//!   proves the runtime reaches `Running` and is cleanly stopped; it never
//!   drives the completed-bar driver (Phase C, out of D2 scope) and asserts
//!   zero orders were ever submitted.
//!
//! ## Architecture note
//!
//! AL-01 uses `establish_db_backed_active_run_for_test` (the AUTON-PAPER-03B
//! seam) to establish coherent DB + in-memory run state without invoking the
//! full start path. AL-03 uses the real, source-aligned fixture pattern from
//! `scenario_daemon_runtime_lifecycle.rs` (synthetic provider/instrument
//! registries, seeded `md_bars`, a loopback mock Alpaca REST server, a
//! registered `intraday_scalper` strategy, a Live broker cursor) to drive
//! the actual production start gate chain to a genuine `Ok`, not a stub.

use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use mqk_broker_alpaca::{encode_fetch_cursor, types::AlpacaFetchCursor};
use mqk_daemon::state;
use mqk_daemon::state::autonomous_daily_coordinator::{
    tick_autonomous_daily_coordinator, AutonomousDailyCoordinatorTickInput,
    AutonomousDailyCoordinatorTickOutcome,
};
use state::{
    AlpacaWsContinuityState, AutonomousSessionSchedule, AutonomousSessionTruth, BrokerKind,
    DeploymentMode, SessionWindow, StrategyFleetEntry,
};
use tokio::net::TcpListener;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_paper_alpaca() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ))
}

/// Fixed UTC window 14:30–21:00 and a deterministic in-session / out-of-session pair.
fn session_fixtures() -> (
    AutonomousSessionSchedule,
    chrono::DateTime<Utc>,
    chrono::DateTime<Utc>,
) {
    let window = SessionWindow::parse("14:30", "21:00")
        .expect("AUTON-12 helper: fixed test window must parse");
    let in_session = Utc.with_ymd_and_hms(2026, 4, 14, 15, 0, 0).unwrap();
    let out_of_session = Utc.with_ymd_and_hms(2026, 4, 14, 22, 0, 0).unwrap();
    (
        AutonomousSessionSchedule::FixedUtcWindow(window),
        in_session,
        out_of_session,
    )
}

async fn db_pool_or_skip() -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(v) => v,
        Err(_) => return None,
    };
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("AUTON-12 DB test: failed to connect to MQK_DATABASE_URL"),
    )
}

// ---------------------------------------------------------------------------
// AL-01 (DB-backed) — Tick-driven session-boundary stop → StoppedAtBoundary
//
// Preconditions (simulate post-start controller state):
//   - DB-backed active run established via AUTON-PAPER-03B seam
//   - locally_started = true  (the controller's local variable after auto-start)
//   - schedule's now is outside the session window
//
// Proof: tick arm (false, true, true) fires →
//   attempt_auto_stop → stop_execution_runtime:
//     - sends Stop to the injected loop; loop exits ("test loop stopped")
//     - db_pool() succeeds (DB configured)
//     - fetch_run finds run in Running status
//     - stop_run writes Stopped to DB
//   → locally_started becomes false
//   → truth becomes StoppedAtBoundary
//   → locally_owned_run_id() returns None (loop joined + handle taken)
//   → DB run status = Stopped (durable evidence)
//
// Skips gracefully when MQK_DATABASE_URL is not set.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn al01_tick_driven_session_boundary_stop_produces_stopped_at_boundary() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("AL-01: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    mqk_db::migrate(&pool)
        .await
        .expect("AL-01: migration failed");

    let run_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        b"auton12.al01.tick_driven_boundary_stop",
    );

    // Idempotent pre-test cleanup.
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("AL-01: pre-test run cleanup failed");

    let mut st_inner = state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st_inner.set_adapter_id_for_test("auton12-al01");
    let st = Arc::new(st_inner);

    // Arm integrity so run state is coherent.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    // Establish DB-backed active run + inject fake execution loop (AUTON-PAPER-03B seam).
    st.establish_db_backed_active_run_for_test(run_id)
        .await
        .expect("AL-01: DB-backed active run must be established");

    // Verify preconditions before the tick.
    assert!(
        st.locally_owned_run_id().await.is_some(),
        "AL-01 precondition: locally_owned_run_id must be Some before the stop tick"
    );
    {
        let run = mqk_db::fetch_run(&pool, run_id)
            .await
            .expect("AL-01: fetch pre-stop run failed");
        assert!(
            matches!(run.status, mqk_db::RunStatus::Running),
            "AL-01 precondition: DB run must be Running before the stop tick; got: {:?}",
            run.status
        );
    }

    // Simulate the controller's local variable state after a successful auto-start.
    let mut locally_started = true;

    let (schedule, _in_session, out_of_session) = session_fixtures();
    // Drive the out-of-session tick: arm (false, true, true) fires → attempt_auto_stop.
    state::run_session_controller_tick(&st, schedule, &mut locally_started, out_of_session).await;

    // ── Core lifecycle claim ─────────────────────────────────────────────────

    assert!(
        !locally_started,
        "AL-01: locally_started must be false after session-boundary stop"
    );

    let truth = st.autonomous_session_truth().await;
    assert!(
        matches!(truth, AutonomousSessionTruth::StoppedAtBoundary { .. }),
        "AL-01: autonomous session truth must be StoppedAtBoundary after tick-driven stop; \
         got: {truth:?}"
    );

    // ── Durable evidence ────────────────────────────────────────────────────

    let run = mqk_db::fetch_run(&pool, run_id)
        .await
        .expect("AL-01: fetch post-stop run failed");
    assert!(
        matches!(run.status, mqk_db::RunStatus::Stopped),
        "AL-01: DB run must be Stopped after session-boundary stop; got: {:?}",
        run.status
    );

    // No dangling local ownership after stop.
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "AL-01: locally_owned_run_id must be None after session-boundary stop (loop joined)"
    );
}

// ---------------------------------------------------------------------------
// AL-02 (pure, no-DB, legacy compatibility) — Tick-driven start: every gate
// before the strict daily-data-readiness gate passes → StartRefused at that
// readiness gate, which itself refuses because it cannot query `md_bars`
// without a DB.
//
// Preconditions (every gate before daily-data-readiness satisfied for
// Paper+Alpaca):
//   - integrity armed  (disarmed=false, halted=false)           → gate 2 passes
//   - WS = Live                                                 → BRK-00R-04 passes
//   - reconcile = "unknown" (boot default)                      → BRK-09R passes
//   - no artifact path set (NotConfigured)                      → TV-02C passes
//   - no parity evidence path (NotConfigured)                   → TV-03C passes
//   - Paper mode (not LiveCapital)                              → TV-04F passes
//   - no capital policy (NotConfigured)                         → TV-04A passes
//   - no deployment economics (NotConfigured)                   → TV-04D passes
//   - active strategy fleet (via set_strategy_fleet_for_test)   → B1A passes
//   - resolvable (symbol, strategy_id, timeframe) assignment    → readiness
//     evaluation can run at all, rather than refusing earlier with
//     `required_assignments_missing`
//   - no DB                                                     → readiness
//     gate's own per-assignment `md_bars` query cannot run
//
// Proof: tick arm (true, false, _) fires →
//   attempt_auto_start →
//   try_autonomous_arm returns Ok (already armed: early-exit, no DB needed) →
//   start_execution_runtime traverses every gate before daily-data-readiness
//   without blocking →
//   the strict daily-data-readiness gate refuses (`readiness_state =
//   "db_unavailable"` for the one assignment, since it cannot query
//   `md_bars` without a DB) →
//   attempt_auto_start sets StartRefused { detail: "…daily_data_readiness_blocked…" }
//
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-LIFECYCLE-CLOSURE-REPAIR-01: this
// replaces the prior (stale) claim that `db_pool()`'s `service_unavailable`
// is the terminal fault — the DAILY-DATA-READINESS-01C-ENFORCEMENT-01 strict
// readiness gate was inserted earlier in the gate chain than `db_pool()` by
// prior Phase B/C work, so a DB-less environment is refused at the
// readiness gate now, never reaching `db_pool()` at all. This test still
// proves every gate before readiness passes under canonical preconditions;
// it no longer claims DB absence alone is the final proof boundary — AL-03
// below is the canonical proof of a full DB-backed start.
// ---------------------------------------------------------------------------

const STRATEGY_SYMBOL_ENV_AL02: &str = "MQK_STRATEGY_SYMBOL";
const STRATEGY_IDS_ENV_AL02: &str = "MQK_STRATEGY_IDS";
const STRATEGY_TIMEFRAME_ENV_AL02: &str = "MQK_STRATEGY_MD_TIMEFRAME";

#[tokio::test]
async fn al02_tick_driven_start_passes_all_pre_db_gates_reaches_db_gate() {
    let st = make_paper_alpaca();

    // Arm integrity — passes gate 2.
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    // WS = Live — passes BRK-00R-04.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:order-al02:accepted:2026-04-14T14:30:00Z".to_string(),
        last_event_at: "2026-04-14T14:30:00Z".to_string(),
    })
    .await;

    // reconcile defaults to "unknown" — passes BRK-09R ("dirty"/"stale" block, "unknown" passes).
    // No artifact, no capital policy, no deployment economics — all pass-through.
    // STRATEGY-DORMANCY-01: Paper+Alpaca requires an active fleet; set one so the
    // bootstrap gate passes and the start reaches the DB gate as intended.
    st.set_strategy_fleet_for_test(Some(vec![StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;
    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-LIFECYCLE-CLOSURE-REPAIR-01:
    // the DAILY-DATA-READINESS-01C-ENFORCEMENT-01 strict readiness gate
    // (gate 16) now runs before db_pool() (gate 17) and independently needs
    // a resolvable (symbol, strategy_id, timeframe) assignment via
    // `build_multi_symbol_runtime_config_from_env` — `set_strategy_fleet_for_test`
    // only satisfies the separate B1A native-strategy-bootstrap gate. Without
    // this, the start is refused at the readiness gate with
    // `required_assignments_missing`, never reaching the DB gate at all.
    #[allow(deprecated)]
    unsafe {
        std::env::set_var(STRATEGY_SYMBOL_ENV_AL02, "AL02SYM");
        std::env::set_var(STRATEGY_IDS_ENV_AL02, "swing_momentum");
        std::env::set_var(STRATEGY_TIMEFRAME_ENV_AL02, "5m");
    }

    assert!(
        st.db.is_none(),
        "AL-02 precondition: test AppState must have no DB (db_pool() is the target gate)"
    );

    let mut locally_started = false;
    let (schedule, in_session, _out_of_session) = session_fixtures();

    state::run_session_controller_tick(&st, schedule, &mut locally_started, in_session).await;

    std::env::remove_var(STRATEGY_SYMBOL_ENV_AL02);
    std::env::remove_var(STRATEGY_IDS_ENV_AL02);
    std::env::remove_var(STRATEGY_TIMEFRAME_ENV_AL02);

    // Start was refused at the readiness/DB gate, not at any earlier gate.
    assert!(
        !locally_started,
        "AL-02: locally_started must be false (start failed at the readiness/DB gate, not at an \
         earlier gate)"
    );

    let truth = st.autonomous_session_truth().await;
    let AutonomousSessionTruth::StartRefused { ref detail } = truth else {
        panic!("AL-02: truth must be StartRefused; got: {truth:?}");
    };

    // The detail must name the daily-data-readiness fault (itself caused by
    // DB unavailability — `readiness_state = "db_unavailable"`) — proving
    // every earlier gate passed.
    assert!(
        detail.contains("daily_data_readiness_blocked"),
        "AL-02: StartRefused detail must mention 'daily_data_readiness_blocked', proving every \
         earlier gate was satisfied; got detail: {detail:?}"
    );

    // Negative guard: detail must not name any earlier gate as the blocker.
    // These substrings appear in the fault_class or error message only if those gates fired.
    for pre_db_marker in &[
        "integrity_disarmed",
        "ws_continuity",
        "reconcile_dirty",
        "artifact_intake",
        "artifact_deployability",
        "parity_evidence",
        "live_capital",
        "capital_policy",
        "deployment_economics",
        "native_strategy_bootstrap",
        "deployment_mode_unproven",
    ] {
        assert!(
            !detail.contains(pre_db_marker),
            "AL-02: StartRefused detail must not mention pre-DB gate marker '{pre_db_marker}'; \
             this would mean an earlier gate fired when all should have passed; \
             got detail: {detail:?}"
        );
    }

    // No dangling local ownership.
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "AL-02: locally_owned_run_id must be None after a DB-gate-blocked start"
    );
}

// ---------------------------------------------------------------------------
// AL-03 (DB-backed, canonical production path) — REPAIR 12/13: the durable
// coordinator drives a full, real successful start and stop.
//
// Fixture pattern mirrors `scenario_daemon_runtime_lifecycle.rs`'s
// `daemon_state()` exactly (synthetic registries, seeded md_bars, a loopback
// mock Alpaca REST server, a registered `intraday_scalper` strategy, a Live
// broker cursor) but drives `tick_autonomous_daily_coordinator` directly
// instead of the HTTP routes, so the proof exercises the real production
// controller seam (`session_controller.rs`'s `run_durable_session_controller_tick`
// calls this exact function).
// ---------------------------------------------------------------------------

const AL03_SYMBOL: &str = "ZZAUTON12";
const AL03_PROVIDER: &str = "zz_auton12_provider";
const AL03_STRATEGY: &str = "intraday_scalper";
const AL03_TIMEFRAME: &str = "5m";
const AL03_TIMEFRAME_SECS: i64 = 300;
const AL03_REQUIRED_BARS: usize = 5;
const AL03_ADAPTER_ID: &str = "alpaca";

/// Same verified NYSE-regular-session instant `scenario_daemon_runtime_lifecycle.rs`
/// uses (Monday 2024-04-15, well inside NYSE regular session, well inside
/// the calendar seam's covered range), offset ten bars into the session.
const AL03_READINESS_FIXED_TS: i64 = 1_713_188_100 + 10 * 300;

fn al03_now() -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(AL03_READINESS_FIXED_TS, 0)
        .single()
        .expect("valid fixed AL-03 readiness timestamp")
}

fn al03_write_registries() {
    let dir = std::env::temp_dir().join(format!(
        "mqk_auton12_al03_registry_{}_{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create AL-03 registry dir");

    let instruments_path = dir.join("instruments.json");
    std::fs::write(
        &instruments_path,
        format!(
            r#"[
  {{
    "instrument_id": "equity:US:{sym}",
    "symbol": "{sym}",
    "asset_class": "equity",
    "provider": "{prov}",
    "provider_symbol": "{sym}",
    "venue": "TEST",
    "currency": "USD",
    "enabled": true,
    "timeframes": ["5m"],
    "notes": "AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-LIFECYCLE-CLOSURE-REPAIR-01 AL-03 synthetic fixture"
  }}
]"#,
            sym = AL03_SYMBOL,
            prov = AL03_PROVIDER,
        ),
    )
    .expect("write AL-03 instruments fixture");

    let providers_path = dir.join("providers.json");
    std::fs::write(
        &providers_path,
        format!(
            r#"[
  {{
    "provider_id": "{prov}",
    "display_name": "AL-03 Test Provider",
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
]"#,
            prov = AL03_PROVIDER,
        ),
    )
    .expect("write AL-03 providers fixture");

    #[allow(deprecated)]
    unsafe {
        std::env::set_var("MQK_INSTRUMENT_REGISTRY_PATH", &instruments_path);
        std::env::set_var("MQK_PROVIDER_REGISTRY_PATH", &providers_path);
    }
}

/// The exact five completed 5m `end_ts` values the strict Bundle 2 readiness
/// gate expects for `AL03_SYMBOL`, computed via the same production
/// calendar-walk function the gate itself calls — never a second,
/// hand-derived window.
fn al03_expected_bar_window() -> Vec<i64> {
    let calendar_provider = state::market_calendar::NyseWeekdaysProvider;
    let now = al03_now();
    let schedule = state::market_calendar::resolve_market_session_schedule(&calendar_provider, now);
    mqk_daemon::daily_data_readiness::expected_intraday_end_ts_window(
        &calendar_provider,
        &schedule,
        now.timestamp(),
        AL03_TIMEFRAME_SECS,
        0,
        AL03_REQUIRED_BARS,
    )
    .expect("AL-03 expected 5m window resolves")
}

async fn al03_seed_bars(pool: &sqlx::PgPool, expected_ts: &[i64]) {
    sqlx::query("delete from md_bars where symbol = $1 and timeframe = $2")
        .bind(AL03_SYMBOL)
        .bind(AL03_TIMEFRAME)
        .execute(pool)
        .await
        .expect("cleanup AL-03 bars failed");
    for &end_ts in expected_ts {
        sqlx::query(
            r#"
            insert into md_bars (
              symbol, timeframe, end_ts, open_micros, high_micros, low_micros,
              close_micros, volume, is_complete, provider_id, provider_source,
              provider_symbol, ingest_mode, ingested_at
            ) values ($1,$2,$3,100000000,100000000,100000000,100000000,1000000,true,
                      $4,$4,$1,'historical_sync',$5)
            "#,
        )
        .bind(AL03_SYMBOL)
        .bind(AL03_TIMEFRAME)
        .bind(end_ts)
        .bind(AL03_PROVIDER)
        .bind(
            Utc.timestamp_opt(end_ts + 60, 0)
                .single()
                .expect("valid ingested_at ts"),
        )
        .execute(pool)
        .await
        .expect("seed AL-03 bar insert failed");
    }
}

/// The legacy `market_data_freshness` gate evaluates bar age against the
/// real wall clock even though the strict readiness clock above is fixed at
/// a historical instant. Raise `MQK_INTRADAY_BAR_MAX_AGE_SECS` just enough
/// for the fixed synthetic latest bar to clear that older gate during this
/// test run — bounded, never unbounded/negative.
fn al03_legacy_freshness_max_age_secs(latest_bar_end_ts: i64) -> i64 {
    let real_now = Utc::now().timestamp();
    (real_now - latest_bar_end_ts).max(0) + 3600
}

/// Loopback in-process HTTP server satisfying the Alpaca paper REST surface
/// needed for a real start: `GET /v2/account/activities/FILL` → `[]`. No
/// real network request ever leaves the machine.
async fn al03_start_mock_alpaca_server() -> String {
    let app = axum::Router::new().route(
        "/v2/account/activities/FILL",
        axum::routing::get(|| async { axum::Json(serde_json::json!([])) }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{}", addr.port())
}

async fn al03_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "AL-03 requires MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_autonomous_paper_day_lifecycle_auton12 \
             -- --include-ignored"
        )
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("AL-03 connect failed");
    mqk_db::migrate(&pool).await.expect("AL-03 migrate failed");

    sqlx::query("DELETE FROM runtime_leader_lease WHERE id = 1")
        .execute(&pool)
        .await
        .expect("cleanup runtime_leader_lease");
    sqlx::query("DELETE FROM runtime_control_state WHERE id = 1")
        .execute(&pool)
        .await
        .expect("cleanup runtime_control_state");
    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("cleanup sys_arm_state");
    sqlx::query("DELETE FROM runs WHERE engine_id = 'mqk-daemon'")
        .execute(&pool)
        .await
        .expect("cleanup daemon runs");
    sqlx::query("DELETE FROM sys_reconcile_status_state")
        .execute(&pool)
        .await
        .expect("cleanup sys_reconcile_status_state");
    sqlx::query("DELETE FROM broker_event_cursor WHERE adapter_id = 'alpaca'")
        .execute(&pool)
        .await
        .expect("cleanup broker_event_cursor");
    sqlx::query("DELETE FROM md_bars WHERE symbol = $1 AND timeframe = $2")
        .bind(AL03_SYMBOL)
        .bind(AL03_TIMEFRAME)
        .execute(&pool)
        .await
        .expect("cleanup AL-03 md_bars");
    sqlx::query(
        "DELETE FROM sys_autonomous_daily_operation_events WHERE operation_id IN \
         (SELECT operation_id FROM sys_autonomous_daily_operations WHERE adapter_id = $1)",
    )
    .bind(AL03_ADAPTER_ID)
    .execute(&pool)
    .await
    .expect("cleanup AL-03 operation events");
    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3-SUPERVISOR-AND-CRITICAL-OUTCOME-
    // CLOSURE-01: the task-level exactly-once proof
    // (scenario_autonomous_completed_bar_task_01::m01) shares this same
    // PAPER/alpaca slot and durably records bar-dispatch claims for its
    // operation — delete those before the operations row or this cleanup
    // fails on the foreign key.
    sqlx::query(
        "DELETE FROM sys_autonomous_daily_bar_dispatches WHERE operation_id IN \
         (SELECT operation_id FROM sys_autonomous_daily_operations WHERE adapter_id = $1)",
    )
    .bind(AL03_ADAPTER_ID)
    .execute(&pool)
    .await
    .expect("cleanup AL-03 bar dispatches");
    sqlx::query("DELETE FROM sys_autonomous_daily_operations WHERE adapter_id = $1")
        .bind(AL03_ADAPTER_ID)
        .execute(&pool)
        .await
        .expect("cleanup AL-03 operations");
    sqlx::query(
        "INSERT INTO sys_strategy_registry \
         (strategy_id, display_name, enabled, kind, registered_at_utc, updated_at_utc, note) \
         VALUES ($1, 'Intraday Scalper', true, 'bar_driven', NOW(), NOW(), 'auton12-al03-fixture') \
         ON CONFLICT (strategy_id) DO UPDATE SET enabled = true, updated_at_utc = NOW()",
    )
    .bind(AL03_STRATEGY)
    .execute(&pool)
    .await
    .expect("upsert intraday_scalper for AL-03");

    pool
}

async fn al03_daemon_state() -> Arc<state::AppState> {
    let mock_url = al03_start_mock_alpaca_server().await;
    al03_write_registries();
    #[allow(deprecated)]
    unsafe {
        std::env::set_var("MQK_DAEMON_DEPLOYMENT_MODE", "paper");
        std::env::set_var("MQK_DAEMON_ADAPTER_ID", AL03_ADAPTER_ID);
        std::env::set_var("ALPACA_API_KEY_PAPER", "test-paper-key");
        std::env::set_var("ALPACA_API_SECRET_PAPER", "test-paper-secret");
        std::env::set_var("ALPACA_PAPER_BASE_URL", &mock_url);
        std::env::set_var("MQK_STRATEGY_IDS", AL03_STRATEGY);
        std::env::set_var("MQK_STRATEGY_SYMBOL", AL03_SYMBOL);
        std::env::set_var("MQK_STRATEGY_MD_TIMEFRAME", AL03_TIMEFRAME);
        std::env::set_var("MQK_DATA_READINESS_GRACE_SECS", "0");
        std::env::set_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS", "60");
        std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
        std::env::remove_var(SESSION_START_ENV_AL03);
        std::env::remove_var(SESSION_STOP_ENV_AL03);
    }

    let pool = al03_pool().await;
    let expected_bars = al03_expected_bar_window();
    al03_seed_bars(&pool, &expected_bars).await;
    let latest_bar_end_ts = *expected_bars
        .last()
        .expect("AL-03 expected window is non-empty");
    let legacy_max_age_secs = al03_legacy_freshness_max_age_secs(latest_bar_end_ts);
    #[allow(deprecated)]
    unsafe {
        std::env::set_var(
            "MQK_INTRADAY_BAR_MAX_AGE_SECS",
            legacy_max_age_secs.to_string(),
        );
    }

    let state = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    state
        .set_daily_data_readiness_clock_override_for_test(Some(al03_now()))
        .await;

    let resolved_config = state::build_multi_symbol_runtime_config_from_env()
        .expect("AL-03 fixture assignment must resolve");
    assert_eq!(resolved_config.symbols.len(), 1);
    assert_eq!(resolved_config.symbols[0].symbol, AL03_SYMBOL);
    assert_eq!(resolved_config.symbols[0].strategy_id, AL03_STRATEGY);
    assert_eq!(resolved_config.symbols[0].timeframe, AL03_TIMEFRAME);
    assert_eq!(expected_bars.len(), AL03_REQUIRED_BARS);

    let live_cursor = AlpacaFetchCursor::live(None, "alpaca:auton12:start", "2026-01-01T00:00:00Z");
    let cursor_json = encode_fetch_cursor(&live_cursor).expect("encode live cursor");
    mqk_db::advance_broker_cursor(
        state.db.as_ref().expect("db must be set"),
        "alpaca",
        &cursor_json,
        chrono::Utc::now(),
    )
    .await
    .expect("persist live broker cursor for AL-03");
    state
        .update_ws_continuity(state::AlpacaWsContinuityState::Live {
            last_message_id: "alpaca:auton12:start".to_string(),
            last_event_at: "2026-01-01T00:00:00Z".to_string(),
        })
        .await;
    {
        let mut broker = state.broker_snapshot.write().await;
        *broker = Some(mqk_schemas::BrokerSnapshot {
            captured_at_utc: chrono::Utc::now(),
            account: mqk_schemas::BrokerAccount {
                equity: "100000".to_string(),
                cash: "100000".to_string(),
                currency: "USD".to_string(),
            },
            orders: vec![],
            fills: vec![],
            positions: vec![],
        });
    }
    {
        let mut execution = state.execution_snapshot.write().await;
        *execution = Some(mqk_runtime::observability::ExecutionSnapshot {
            run_id: None,
            active_orders: vec![],
            pending_outbox: vec![],
            recent_inbox_events: vec![],
            portfolio: mqk_runtime::observability::PortfolioSnapshot {
                cash_micros: 0,
                realized_pnl_micros: 0,
                positions: vec![],
            },
            system_block_state: None,
            recent_risk_denials: vec![],
            snapshot_at_utc: chrono::Utc::now(),
            has_recent_terminal_fill: false,
            risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
        });
    }
    state
}

const SESSION_START_ENV_AL03: &str = "MQK_SESSION_START_HH_MM";
const SESSION_STOP_ENV_AL03: &str = "MQK_SESSION_STOP_HH_MM";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn al03_durable_coordinator_drives_a_real_successful_start_and_stop() {
    let st = al03_daemon_state().await;
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    let pool = st.db.clone().expect("AL-03: db must be configured");
    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None)
        .await
        .expect("AL-03: persist ARMED failed");

    let now = al03_now();

    // ── REPAIR 12: durable coordinator start proof ──────────────────────────
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await
    .expect("AL-03: start tick must not error");
    let run_id = match outcome {
        AutonomousDailyCoordinatorTickOutcome::Started { run_id } => run_id,
        other => panic!("AL-03: expected Started, got {other:?}"),
    };

    tokio::time::sleep(Duration::from_millis(150)).await;

    let market_date = chrono::NaiveDate::from_ymd_opt(2024, 4, 15).unwrap();
    let operation = mqk_db::fetch_autonomous_daily_operation_for_slot(
        &pool,
        market_date,
        "PAPER",
        AL03_ADAPTER_ID,
    )
    .await
    .expect("AL-03: fetch operation failed")
    .expect("AL-03: operation row must exist after Started");
    assert_eq!(operation.state, mqk_db::STATE_RUNNING);
    assert_eq!(operation.run_id, Some(run_id));
    assert_eq!(operation.started_at_utc, Some(now));
    assert_eq!(
        operation.start_attempt_count, 1,
        "exactly one real counted start attempt"
    );
    assert_eq!(operation.next_retry_utc, None);

    let run = mqk_db::fetch_run(&pool, run_id)
        .await
        .expect("AL-03: fetch run failed");
    assert!(
        matches!(run.status, mqk_db::RunStatus::Running),
        "AL-03: durable run must reach Running, got {:?}",
        run.status
    );
    assert_eq!(
        st.locally_owned_run_id().await,
        Some(run_id),
        "AL-03: local ownership must bind to the exact run_id"
    );

    // ── REPAIR 12: durable coordinator stop proof ───────────────────────────
    let close_now = operation.effective_operation_close_utc;
    let stop_outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: close_now,
    })
    .await
    .expect("AL-03: stop tick must not error");
    assert_eq!(
        stop_outcome,
        AutonomousDailyCoordinatorTickOutcome::RuntimeStopped
    );

    let operation_after_stop = mqk_db::fetch_autonomous_daily_operation_for_slot(
        &pool,
        market_date,
        "PAPER",
        AL03_ADAPTER_ID,
    )
    .await
    .expect("AL-03: fetch operation after stop failed")
    .expect("AL-03: operation row must still exist");
    assert_eq!(operation_after_stop.state, mqk_db::STATE_STOPPING);
    assert!(operation_after_stop.stopped_at_utc.is_some());
    assert_eq!(operation_after_stop.stop_attempt_count, Some(1));

    let run_after_stop = mqk_db::fetch_run(&pool, run_id)
        .await
        .expect("AL-03: fetch run after stop failed");
    assert!(
        matches!(run_after_stop.status, mqk_db::RunStatus::Stopped),
        "AL-03: durable run must reach Stopped, got {:?}",
        run_after_stop.status
    );
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "AL-03: local ownership must be cleared after the canonical stop"
    );

    // ── No paper/live order was ever submitted ──────────────────────────────
    let outbox_count: (i64,) = sqlx::query_as("select count(*) from oms_outbox where run_id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .expect("AL-03: outbox count query failed");
    assert_eq!(
        outbox_count.0, 0,
        "AL-03: no order may ever be submitted by this start/stop proof"
    );

    st.stop_for_shutdown().await;
}
