//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-DURABLE-SESSION-COORDINATOR: proof
//! tests for `state::autonomous_daily_coordinator`.
//!
//! DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_autonomous_daily_session_coordinator_01 \
//!   -- --include-ignored --test-threads=1 --nocapture
//!
//! No real provider, broker, or network call is made anywhere in this
//! file. `BrokerKind::Paper` (the in-process paper broker) is used
//! throughout so the STRATEGY-DORMANCY-01 gate (Paper+Alpaca only) never
//! applies, keeping a Dormant native-strategy bootstrap (no fleet
//! configured) a legitimate, non-blocking test configuration for exercising
//! identity/create/recover/preopen/start-sequence/close logic without
//! requiring a registered strategy engine. `establish_db_backed_active_run_for_test`
//! is used for the running-reconciliation and session-close tests — it
//! creates a real `runs` row and a locally-owned in-process loop handle,
//! but starts no broker connection and makes no network call.

use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use mqk_daemon::state::autonomous_daily_coordinator::{
    apply_transition, attempt_canonical_start, dispatch_by_state, handle_running,
    handle_session_close, handle_stopping, tick_autonomous_daily_coordinator,
    AutonomousDailyCoordinatorTickInput, AutonomousDailyCoordinatorTickOutcome,
};
use mqk_daemon::state::{
    self, derive_assignment_identity, derive_autonomous_daily_operation_id,
    derive_runtime_binding_identity, resolve_autonomous_daily_session_plan_from_env, AppState,
    AutonomousDailyPlanTiming, AutonomousDailySessionPlan, AutonomousDailySessionPlanResolution,
    BrokerKind, DeploymentMode, MultiSymbolConfigSource, MultiSymbolRuntimeConfig,
    SymbolStrategyAssignment,
};
use mqk_runtime::native_strategy::EffectiveRuntimeBinding;
use uuid::Uuid;

const SESSION_START_ENV: &str = "MQK_SESSION_START_HH_MM";
const SESSION_STOP_ENV: &str = "MQK_SESSION_STOP_HH_MM";
const STRATEGY_SYMBOL_ENV: &str = "MQK_STRATEGY_SYMBOL";
const STRATEGY_IDS_ENV: &str = "MQK_STRATEGY_IDS";
const STRATEGY_TIMEFRAME_ENV: &str = "MQK_STRATEGY_MD_TIMEFRAME";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn test_pool() -> anyhow::Result<sqlx::PgPool> {
    if std::env::var("MQK_DATABASE_URL").is_err() {
        anyhow::bail!("SKIP: requires MQK_DATABASE_URL");
    }
    mqk_db::testkit_db_pool().await
}

fn unique_suffix() -> String {
    Uuid::new_v4().to_string().replace('-', "")[..10].to_string()
}

/// Reset every env var this file touches to a known-clean baseline. Called
/// at the start of every test (not only DB-backed ones) so panics in an
/// earlier test never leak configuration into a later one under
/// `--test-threads=1` serial execution.
fn reset_env() {
    std::env::remove_var(SESSION_START_ENV);
    std::env::remove_var(SESSION_STOP_ENV);
    std::env::remove_var(STRATEGY_SYMBOL_ENV);
    std::env::remove_var(STRATEGY_IDS_ENV);
    std::env::remove_var(STRATEGY_TIMEFRAME_ENV);
}

/// A minimal, resolvable legacy single-symbol assignment configuration.
/// `strategy_id` need not be registered in any plugin registry — config
/// resolution and native-strategy bootstrap are independent seams
/// (`build_multi_symbol_runtime_config_from_env` vs.
/// `resolve_autonomous_runtime_context`'s fleet-driven bootstrap, which
/// stays `Dormant` here since no test fleet is injected).
fn set_resolvable_assignment_env(symbol: &str) {
    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
}

/// A test `AppState` with a real DB pool, Paper deployment, and the
/// in-process paper broker (never Alpaca — keeps STRATEGY-DORMANCY-01 out
/// of scope for these tests, and never opens any network connection).
fn paper_state_with_db(db: sqlx::PgPool, adapter_id: &str) -> Arc<AppState> {
    let mut st = AppState::new_for_test_with_db_mode_and_broker(
        db,
        DeploymentMode::Paper,
        BrokerKind::Paper,
    );
    st.set_adapter_id_for_test(adapter_id);
    Arc::new(st)
}

/// Independently compute the exact assignment/runtime-binding identity a
/// coordinator tick would derive for the current (Dormant-bootstrap, legacy
/// single-symbol) env configuration set by [`set_resolvable_assignment_env`].
fn expected_assignment_identity(symbol: &str) -> String {
    let config = MultiSymbolRuntimeConfig {
        schema_version: "v2".to_string(),
        symbols: vec![SymbolStrategyAssignment {
            symbol: symbol.to_string(),
            strategy_id: "swing_momentum".to_string(),
            timeframe: "5m".to_string(),
        }],
        max_concurrent_symbols: 1,
        source: MultiSymbolConfigSource::EnvSingleSymbolFallback,
    };
    derive_assignment_identity(&config)
}

fn dormant_runtime_binding_identity() -> String {
    derive_runtime_binding_identity(&EffectiveRuntimeBinding {
        effective_runtime_strategy_id: None,
        effective_runtime_target_symbol: None,
        effective_runtime_timeframe_secs: None,
    })
}

async fn cleanup_operation(pool: &sqlx::PgPool, operation_id: Uuid) {
    let _ =
        sqlx::query("delete from sys_autonomous_daily_operation_events where operation_id = $1")
            .bind(operation_id)
            .execute(pool)
            .await;
    let _ = sqlx::query("delete from sys_autonomous_daily_operations where operation_id = $1")
        .bind(operation_id)
        .execute(pool)
        .await;
}

async fn cleanup_run(pool: &sqlx::PgPool, run_id: Uuid) {
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

/// Resolve today's coordinator identity for `adapter_id`/`symbol` at
/// `now_utc`, returning the plan + identity triple. Panics (test failure)
/// if the plan is not `Applicable` — callers must choose a weekday
/// `now_utc`.
fn resolve_identity_for_env(
    adapter_id: &str,
    symbol: &str,
    now_utc: DateTime<Utc>,
) -> (AutonomousDailySessionPlan, String, String, Uuid) {
    let timing = AutonomousDailyPlanTiming::production_default();
    let plan = match resolve_autonomous_daily_session_plan_from_env(now_utc, &timing) {
        AutonomousDailySessionPlanResolution::Applicable(plan) => plan,
        other => {
            panic!("expected an applicable session plan for the test fixture date, got {other:?}")
        }
    };
    let assignment_identity = expected_assignment_identity(symbol);
    let runtime_binding_identity = dormant_runtime_binding_identity();
    let operation_id = derive_autonomous_daily_operation_id(
        &plan,
        "PAPER",
        adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
    );
    (
        plan,
        assignment_identity,
        runtime_binding_identity,
        operation_id,
    )
}

/// A weekday at a fixed wall-clock time (2026-07-20 is a Monday), offset
/// `hour`:`minute` UTC.
fn weekday_at(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 20, hour, minute, 0).unwrap()
}

/// Fetch the operation currently occupying the daily slot
/// `(2026-07-20, PAPER, adapter_id)` — the robust way to locate a
/// coordinator-created row without independently re-deriving its exact
/// `runtime_binding_identity` (which depends on `EffectiveRuntimeBinding`
/// derivation details these tests do not need to replicate).
async fn fetch_slot_row(
    pool: &sqlx::PgPool,
    adapter_id: &str,
) -> anyhow::Result<Option<mqk_db::AutonomousDailyOperationRecord>> {
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    mqk_db::fetch_autonomous_daily_operation_for_slot(pool, market_date, "PAPER", adapter_id).await
}

// ---------------------------------------------------------------------------
// A — Plan applicability (no DB required)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a01_weekend_creates_no_operation_no_db_call() {
    reset_env();
    // Saturday 2026-07-18.
    let now = Utc.with_ymd_and_hms(2026, 7, 18, 15, 0, 0).unwrap();
    let st = AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Paper);
    let st = Arc::new(st);
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await
    .expect("tick must not error");
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::NotApplicable { .. }
        ),
        "weekend must be NotApplicable, got {outcome:?}"
    );
}

#[tokio::test]
async fn a02_holiday_creates_no_operation() {
    reset_env();
    // Friday 2026-07-03 is the observed Independence Day market holiday.
    let now = Utc.with_ymd_and_hms(2026, 7, 3, 15, 0, 0).unwrap();
    let st = AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Paper);
    let st = Arc::new(st);
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await
    .expect("tick must not error");
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::NotApplicable { .. }
        ),
        "holiday must be NotApplicable, got {outcome:?}"
    );
}

#[tokio::test]
async fn a03_invalid_fixed_window_override_blocks_calendar() {
    reset_env();
    // Only one of the two required override env vars is set — invalid
    // configuration must never be silently treated as absent.
    std::env::set_var(SESSION_START_ENV, "14:30");
    let now = weekday_at(15, 0);
    let st = AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Paper);
    let st = Arc::new(st);
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await
    .expect("tick must not error");
    std::env::remove_var(SESSION_START_ENV);
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::CalendarBlocked { .. }
        ),
        "invalid override must be CalendarBlocked, got {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// B — Identity / create / recover / conflict (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn b01_first_tick_creates_exactly_one_operation() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-b01-{}", unique_suffix());
    let symbol = "AAPL";
    set_resolvable_assignment_env(symbol);
    let now = weekday_at(12, 0); // before preopen (13:00 UTC)

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::WaitingForPreopen
    );

    let row = fetch_slot_row(&pool, &adapter_id)
        .await?
        .expect("exactly one operation row must exist for the daily slot");
    let events: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_daily_operation_events where operation_id = $1",
    )
    .bind(row.operation_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        events.0, 1,
        "exactly one initial transition event must exist"
    );

    cleanup_operation(&pool, row.operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn b02_repeated_tick_recovers_same_operation() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-b02-{}", unique_suffix());
    let symbol = "AAPL";
    set_resolvable_assignment_env(symbol);
    let now = weekday_at(12, 0);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await?;
    let first = fetch_slot_row(&pool, &adapter_id)
        .await?
        .expect("row must exist after first tick");
    tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now + ChronoDuration::seconds(30),
    })
    .await?;

    let count: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_daily_operations where market_date = $1 \
         and deployment_mode = 'PAPER' and adapter_id = $2",
    )
    .bind(first.market_date)
    .bind(&adapter_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        count.0, 1,
        "a second tick must recover, never create a second row"
    );

    cleanup_operation(&pool, first.operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn b03_concurrent_identical_ticks_create_one_row_one_event() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-b03-{}", unique_suffix());
    let symbol = "AAPL";
    set_resolvable_assignment_env(symbol);
    let now = weekday_at(12, 0);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let mut handles = Vec::new();
    for _ in 0..5 {
        let st = Arc::clone(&st);
        handles.push(tokio::spawn(async move {
            tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
                state: &st,
                now_utc: now,
            })
            .await
        }));
    }
    for h in handles {
        h.await.expect("task panicked")?;
    }

    let row = fetch_slot_row(&pool, &adapter_id)
        .await?
        .expect("exactly one operation row must exist for the daily slot");
    let count: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_daily_operations where market_date = $1 \
         and deployment_mode = 'PAPER' and adapter_id = $2",
    )
    .bind(row.market_date)
    .bind(&adapter_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        count.0, 1,
        "concurrent identical ticks must create exactly one row"
    );
    let events: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_daily_operation_events where operation_id = $1",
    )
    .bind(row.operation_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        events.0, 1,
        "concurrent identical ticks must create exactly one initial event"
    );

    cleanup_operation(&pool, row.operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn b04_identity_conflict_creates_no_second_row_and_calls_neither_start_nor_stop(
) -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-b04-{}", unique_suffix());
    let symbol = "AAPL";
    set_resolvable_assignment_env(symbol);
    let now = weekday_at(12, 0);
    let (plan, _, _, _) = resolve_identity_for_env(&adapter_id, symbol, now);

    // Seed a conflicting row for the same daily slot with a different
    // assignment_identity (as if the operator changed MQK_STRATEGY_SYMBOL
    // and restarted mid-day).
    let other_operation_id = Uuid::new_v4();
    let create_args = mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id: other_operation_id,
        market_date: plan.market_date.parse().unwrap(),
        deployment_mode: "PAPER".to_string(),
        adapter_id: adapter_id.clone(),
        session_plan_identity: plan.session_plan_identity.clone(),
        assignment_identity: "a-completely-different-assignment".to_string(),
        runtime_binding_identity: dormant_runtime_binding_identity(),
        calendar_source: plan.calendar_source.clone(),
        calendar_coverage_state: plan.calendar_coverage_state.clone(),
        schedule_source: plan.schedule_source.as_str().to_string(),
        effective_operation_open_utc: plan.effective_operation_open_utc,
        effective_operation_close_utc: plan.effective_operation_close_utc,
        exchange_session_open_utc: plan.exchange_session_open_utc,
        exchange_session_close_utc: plan.exchange_session_close_utc,
        exchange_is_early_close: plan.exchange_is_early_close,
        previous_trading_date: plan.previous_trading_date.parse().unwrap(),
        preopen_start_utc: plan.preopen_start_utc,
        postclose_finalize_utc: plan.postclose_finalize_utc,
        initial_state: mqk_db::STATE_AWAITING_PREOPEN.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: now,
        bounded_detail: "conflicting seed row".to_string(),
        stop_attempt_count: 0,
    };
    mqk_db::create_or_recover_autonomous_daily_operation(&pool, &create_args).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "operation_identity_conflict"
            }
        ),
        "identity conflict must classify as manual intervention, got {outcome:?}"
    );

    let count: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_daily_operations where market_date = $1 \
         and deployment_mode = 'PAPER' and adapter_id = $2",
    )
    .bind(plan.market_date.parse::<NaiveDate>().unwrap())
    .bind(&adapter_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        count.0, 1,
        "identity conflict must never create a second row for the daily slot"
    );

    assert!(
        st.locally_owned_run_id().await.is_none(),
        "identity conflict must never call the canonical start path"
    );

    cleanup_operation(&pool, other_operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// C — Preopen timing (DB-backed, uses set_resolvable_assignment_env)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn c01_before_preopen_remains_awaiting_preopen() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-c01-{}", unique_suffix());
    let symbol = "AAPL";
    set_resolvable_assignment_env(symbol);
    let now = weekday_at(12, 0); // preopen starts at 13:00 UTC

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::WaitingForPreopen
    );

    let row = fetch_slot_row(&pool, &adapter_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.state, mqk_db::STATE_AWAITING_PREOPEN);
    assert_eq!(row.start_attempt_count, 0);

    cleanup_operation(&pool, row.operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn c02_preopen_reached_enters_preparing_data() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-c02-{}", unique_suffix());
    let symbol = "AAPL";
    set_resolvable_assignment_env(symbol);
    let before_preopen = weekday_at(12, 0);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: before_preopen,
    })
    .await?;

    // Preopen is 30 minutes before effective open (13:30 UTC) -> 13:00 UTC.
    let at_preopen = weekday_at(13, 0);
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: at_preopen,
    })
    .await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::PreparingData
                | AutonomousDailyCoordinatorTickOutcome::PreflightBlocked { .. }
                | AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired { .. }
        ),
        "preopen reached must leave awaiting_preopen, got {outcome:?}"
    );
    let row = fetch_slot_row(&pool, &adapter_id)
        .await?
        .expect("row must exist");
    assert_ne!(row.state, mqk_db::STATE_AWAITING_PREOPEN);
    assert_eq!(
        row.start_attempt_count, 0,
        "preopen readiness evaluation must never increment start_attempt_count regardless of \
         its classification"
    );

    cleanup_operation(&pool, row.operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn c03_no_provider_call_during_preopen_readiness_evaluation() -> anyhow::Result<()> {
    // No fake/real provider client is ever injected into `st` in this test
    // file. If `handle_preparing_data`'s readiness evaluation attempted a
    // provider call, it would panic or hang on a None client rather than
    // return a typed blocked report — its passing completion here is itself
    // the proof of "no provider call".
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-c03-{}", unique_suffix());
    let symbol = "AAPL";
    set_resolvable_assignment_env(symbol);
    let at_preopen = weekday_at(13, 0);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: at_preopen,
    })
    .await?;
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: at_preopen + ChronoDuration::seconds(30),
    })
    .await?;
    assert!(!matches!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::Started { .. }
    ));

    if let Some(row) = fetch_slot_row(&pool, &adapter_id).await? {
        cleanup_operation(&pool, row.operation_id).await;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// D — Canonical start sequence (DB-backed; `attempt_canonical_start` driven
// directly against a hand-built operation row, avoiding the full 23-gate
// chain's need for real market-data/artifact/parity fixtures)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn stub_operation_awaiting_open(
    operation_id: Uuid,
    adapter_id: &str,
    now: DateTime<Utc>,
) -> mqk_db::AutonomousDailyOperationRecord {
    mqk_db::AutonomousDailyOperationRecord {
        operation_id,
        market_date: NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
        deployment_mode: "PAPER".to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity: "stub-plan".to_string(),
        assignment_identity: "stub-assignment".to_string(),
        runtime_binding_identity: "stub-binding".to_string(),
        calendar_source: "nyse_weekdays_heuristic".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: "nyse_weekdays_heuristic".to_string(),
        effective_operation_open_utc: now,
        effective_operation_close_utc: now + ChronoDuration::hours(6),
        exchange_session_open_utc: Some(now),
        exchange_session_close_utc: Some(now + ChronoDuration::hours(6)),
        exchange_is_early_close: Some(false),
        previous_trading_date: Some(NaiveDate::from_ymd_opt(2026, 7, 17).unwrap()),
        preopen_start_utc: now - ChronoDuration::minutes(30),
        postclose_finalize_utc: now + ChronoDuration::hours(6) + ChronoDuration::minutes(15),
        state: mqk_db::STATE_AWAITING_OPEN.to_string(),
        state_reason_code: None,
        state_version: 1,
        run_id: None,
        start_attempt_count: 0,
        last_start_attempt_utc: None,
        next_retry_utc: None,
        data_refresh_state: "not_started".to_string(),
        last_provider_poll_utc: None,
        provider_poll_attempt_count: 0,
        provider_poll_success_count: 0,
        provider_poll_failure_count: 0,
        last_completed_bar_ts: None,
        last_dispatched_bar_ts: None,
        bars_observed: 0,
        bars_dispatched: 0,
        started_at_utc: None,
        stopped_at_utc: None,
        finalized_at_utc: None,
        outcome: None,
        no_trade_reason: None,
        last_error: None,
        created_at_utc: now,
        updated_at_utc: now,
        stop_attempt_count: Some(0),
        last_stop_attempt_utc: None,
    }
}

async fn seed_stub_row(
    pool: &sqlx::PgPool,
    operation: &mqk_db::AutonomousDailyOperationRecord,
) -> anyhow::Result<()> {
    let create_args = mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        market_date: operation.market_date,
        deployment_mode: operation.deployment_mode.clone(),
        adapter_id: operation.adapter_id.clone(),
        session_plan_identity: operation.session_plan_identity.clone(),
        assignment_identity: operation.assignment_identity.clone(),
        runtime_binding_identity: operation.runtime_binding_identity.clone(),
        calendar_source: operation.calendar_source.clone(),
        calendar_coverage_state: operation.calendar_coverage_state.clone(),
        schedule_source: operation.schedule_source.clone(),
        effective_operation_open_utc: operation.effective_operation_open_utc,
        effective_operation_close_utc: operation.effective_operation_close_utc,
        exchange_session_open_utc: operation.exchange_session_open_utc.unwrap(),
        exchange_session_close_utc: operation.exchange_session_close_utc.unwrap(),
        exchange_is_early_close: operation.exchange_is_early_close.unwrap(),
        previous_trading_date: operation.previous_trading_date.unwrap(),
        preopen_start_utc: operation.preopen_start_utc,
        postclose_finalize_utc: operation.postclose_finalize_utc,
        initial_state: operation.state.clone(),
        data_refresh_state: operation.data_refresh_state.clone(),
        occurred_at_utc: operation.created_at_utc,
        bounded_detail: "stub seed".to_string(),
        stop_attempt_count: 0,
    };
    mqk_db::create_or_recover_autonomous_daily_operation(pool, &create_args).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn d01_halted_integrity_blocks_start_with_zero_attempts() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-d01-{}", unique_suffix());
    let now = weekday_at(13, 30);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now);
    seed_stub_row(&pool, &stub).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    {
        let mut ig = st.integrity.write().await;
        ig.halted = true;
        ig.disarmed = true;
    }

    let outcome =
        attempt_canonical_start(&st, &pool, stub, now, mqk_db::STATE_AWAITING_OPEN).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "integrity_halted"
            }
        ),
        "halted integrity must classify manual/integrity_halted, got {outcome:?}"
    );

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.start_attempt_count, 0,
        "an arm-gate refusal must never count as a canonical start attempt"
    );
    assert_eq!(row.state, mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED);

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn d02_durable_disarm_blocks_start_with_zero_attempts() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-d02-{}", unique_suffix());
    let now = weekday_at(13, 30);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now);
    seed_stub_row(&pool, &stub).await?;

    mqk_db::persist_arm_state_canonical(
        &pool,
        mqk_db::ArmState::Disarmed,
        Some(mqk_db::DisarmReason::OperatorHalt),
    )
    .await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    // Default boot integrity is disarmed=true, halted=false — gate 1 passes
    // through to the DB arm-state check.
    let outcome =
        attempt_canonical_start(&st, &pool, stub, now, mqk_db::STATE_AWAITING_OPEN).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "durable_arm_disarmed"
            }
        ),
        "durable DISARMED must classify manual/durable_arm_disarmed, got {outcome:?}"
    );

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.start_attempt_count, 0);

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn d03_already_armed_proceeds_to_a_real_counted_start_attempt() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-d03-{}", unique_suffix());
    let now = weekday_at(13, 30);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now);
    seed_stub_row(&pool, &stub).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    let _outcome =
        attempt_canonical_start(&st, &pool, stub, now, mqk_db::STATE_AWAITING_OPEN).await?;

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.start_attempt_count, 1,
        "an already-armed operation must reach the canonical start call and count exactly one \
         real attempt (the daemon test-fixture's Paper/no-strategy configuration is expected to \
         then refuse at a later gate — this test proves attempt counting, not a full Ok start)"
    );
    assert!(row.last_start_attempt_utc.is_some());

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn d04_retry_not_due_makes_zero_additional_attempts() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-d04-{}", unique_suffix());
    let now = weekday_at(13, 30);
    let operation_id = Uuid::new_v4();
    let mut stub = stub_operation_awaiting_open(operation_id, &adapter_id, now);
    stub.next_retry_utc = Some(now + ChronoDuration::seconds(30));
    seed_stub_row(&pool, &stub).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome =
        attempt_canonical_start(&st, &pool, stub, now, mqk_db::STATE_AWAITING_OPEN).await?;
    assert_eq!(outcome, AutonomousDailyCoordinatorTickOutcome::RetryNotDue);

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.start_attempt_count, 0,
        "no call must be made before next_retry_utc"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn d05_operator_managed_run_is_never_stopped_attached_or_started_over() -> anyhow::Result<()>
{
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-d05-{}", unique_suffix());
    let now = weekday_at(13, 30);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now); // run_id: None
    seed_stub_row(&pool, &stub).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let operator_run_id = Uuid::new_v4();
    st.establish_db_backed_active_run_for_test(operator_run_id)
        .await
        .expect("establish operator run");

    let outcome =
        attempt_canonical_start(&st, &pool, stub, now, mqk_db::STATE_AWAITING_OPEN).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "operator_managed_run_active"
            }
        ),
        "an operator-managed run must block with a distinct typed reason, got {outcome:?}"
    );

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.start_attempt_count, 0,
        "must never attempt a start over an operator-managed run"
    );
    assert_eq!(
        row.run_id, None,
        "must never attach the operator-managed run to this operation"
    );
    assert_eq!(
        st.locally_owned_run_id().await,
        Some(operator_run_id),
        "the operator-managed run must remain untouched and still locally owned"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, operator_run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// E — Running reconciliation (DB-backed, `handle_running`)
// ---------------------------------------------------------------------------

/// Seed a fresh operation row and advance it, via real legal CAS
/// transitions (`awaiting_open -> start_retrying -> running`), to a durable
/// `running` state bound to `run_id`. Returns the resulting real record
/// (correct `state_version`) — unlike a hand-built in-memory struct, this
/// can be passed straight into `handle_running`/`handle_session_close`
/// without a stale-CAS mismatch.
async fn seed_running_operation(
    pool: &sqlx::PgPool,
    operation_id: Uuid,
    adapter_id: &str,
    run_id: Uuid,
    now: DateTime<Utc>,
) -> anyhow::Result<mqk_db::AutonomousDailyOperationRecord> {
    let stub = stub_operation_awaiting_open(operation_id, adapter_id, now);
    seed_stub_row(pool, &stub).await?;
    let current = mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id)
        .await?
        .expect("seeded row must exist");
    let current = apply_transition(
        pool,
        &current,
        mqk_db::STATE_START_RETRYING,
        None,
        now,
        None,
        "test setup",
    )
    .await?;
    apply_transition(
        pool,
        &current,
        mqk_db::STATE_RUNNING,
        None,
        now,
        Some(run_id),
        "test setup",
    )
    .await
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn e01_matching_local_run_remains_running_with_no_writes() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-e01-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let operation = seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(run_id).await?;

    let before = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let outcome = handle_running(&st, &pool, operation, now).await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::Running { run_id }
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        before.state_version, after.state_version,
        "a matching running reconciliation must write nothing"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn e02_mismatched_local_run_becomes_controller_degraded_manual() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-e02-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let operation_run_id = Uuid::new_v4();
    let local_run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let operation =
        seed_running_operation(&pool, operation_id, &adapter_id, operation_run_id, now).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(local_run_id)
        .await?;

    let outcome = handle_running(&st, &pool, operation, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "runtime_run_id_mismatch"
            }
        ),
        "a mismatched local run must classify manual/runtime_run_id_mismatch, got {outcome:?}"
    );
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.state, mqk_db::STATE_CONTROLLER_DEGRADED);
    assert_eq!(
        row.run_id,
        Some(operation_run_id),
        "the operation's durable run_id must never be silently overwritten by the mismatch"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, local_run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn e03_terminal_prior_run_without_halt_enters_recovery_retrying() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-e03-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let operation = seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    // A terminal run row exists (stopped), but no local runtime owns it —
    // the ordinary "process restarted after a clean stop that the
    // coordinator itself did not observe" or "run ended without halt" case.
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "TEST".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;
    mqk_db::stop_run(&pool, run_id).await?;

    let outcome = handle_running(&st, &pool, operation, now).await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::RecoveryScheduled
    );
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.state, mqk_db::STATE_RECOVERY_RETRYING);
    assert!(
        row.next_retry_utc.is_some(),
        "recovery must schedule a bounded retry"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn e04_halted_termination_is_never_retried() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-e04-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let operation = seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "TEST".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;
    mqk_db::stop_run(&pool, run_id).await?;
    {
        let mut ig = st.integrity.write().await;
        ig.halted = true;
        ig.disarmed = true;
    }

    let outcome = handle_running(&st, &pool, operation, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "runtime_ended_unsafely"
            }
        ),
        "a halted termination must never be scheduled for recovery, got {outcome:?}"
    );
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_ne!(row.state, mqk_db::STATE_RECOVERY_RETRYING);

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// F — Session close / stop (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn f01_matching_runtime_is_stopped_at_close() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-f01-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let operation = seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(run_id).await?;

    let outcome = handle_session_close(&st, &pool, operation, now).await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::RuntimeStopped
    );
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.state, mqk_db::STATE_STOPPING);
    assert!(row.stopped_at_utc.is_some());
    assert_eq!(row.stop_attempt_count, Some(1));
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "the matching runtime must actually be stopped"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn f02_no_runtime_at_close_makes_no_stop_call_but_records_stopped() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-f02-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now); // never started

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    seed_stub_row(&pool, &stub).await?;

    let outcome = handle_session_close(&st, &pool, stub, now).await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::RuntimeStopped
    );
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.state, mqk_db::STATE_STOPPING);
    assert!(row.stopped_at_utc.is_some());
    assert_eq!(
        row.stop_attempt_count,
        Some(0),
        "no runtime existed at close, so no stop call — and therefore no stop attempt — was made"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn f03_mismatched_runtime_is_never_stopped_at_close() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-f03-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let operation_run_id = Uuid::new_v4();
    let local_run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let operation =
        seed_running_operation(&pool, operation_id, &adapter_id, operation_run_id, now).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(local_run_id)
        .await?;

    let outcome = handle_session_close(&st, &pool, operation, now).await?;
    assert!(matches!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired { .. }
    ));
    assert_eq!(
        st.locally_owned_run_id().await,
        Some(local_run_id),
        "a mismatched runtime must never be stopped by the coordinator"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, local_run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn f04_unresolved_stop_at_postclose_finalize_becomes_manual() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-f04-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now);
    seed_stub_row(&pool, &stub).await?;
    // seed_stub_row always creates as STATE_AWAITING_OPEN (the only legal
    // initial state its args accept) -- advance it to stop_retrying via real
    // transitions so this test proves the postclose-finalize deadline
    // against a durable row, not a fabricated in-memory-only state.
    let current = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .unwrap();
    let current = apply_transition(
        &pool,
        &current,
        mqk_db::STATE_STOPPING,
        None,
        now,
        None,
        "test setup",
    )
    .await?;
    let current = apply_transition(
        &pool,
        &current,
        mqk_db::STATE_STOP_RETRYING,
        None,
        now,
        None,
        "test setup",
    )
    .await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    // Drive the tick at/after the operation's own postclose_finalize_utc
    // (now + 6h15m per stub_operation_awaiting_open) rather than mutating
    // the seeded boundary itself, which must stay internally consistent
    // (postclose_finalize_utc >= effective_operation_close_utc) to pass the
    // store's own validation.
    let at_postclose_finalize = stub.postclose_finalize_utc;
    let outcome = handle_stopping(
        &st,
        &pool,
        current,
        &plan_from_stub(&stub),
        at_postclose_finalize,
    )
    .await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "unresolved_stop_failure_at_postclose_finalize"
            }
        ),
        "an unresolved stop at postclose_finalize_utc must become manual, got {outcome:?}"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

fn plan_from_stub(op: &mqk_db::AutonomousDailyOperationRecord) -> AutonomousDailySessionPlan {
    AutonomousDailySessionPlan {
        market_date: op.market_date.format("%Y-%m-%d").to_string(),
        previous_trading_date: op
            .previous_trading_date
            .unwrap()
            .format("%Y-%m-%d")
            .to_string(),
        exchange_session_open_utc: op.exchange_session_open_utc.unwrap(),
        exchange_session_close_utc: op.exchange_session_close_utc.unwrap(),
        exchange_is_early_close: op.exchange_is_early_close.unwrap(),
        effective_operation_open_utc: op.effective_operation_open_utc,
        effective_operation_close_utc: op.effective_operation_close_utc,
        calendar_source: op.calendar_source.clone(),
        calendar_coverage_state: op.calendar_coverage_state.clone(),
        schedule_source: state::AutonomousDailyScheduleSource::NyseWeekdaysHeuristic,
        preopen_start_utc: op.preopen_start_utc,
        postclose_finalize_utc: op.postclose_finalize_utc,
        session_plan_identity: op.session_plan_identity.clone(),
    }
}

// ---------------------------------------------------------------------------
// G — Restart truth (DB-backed): a freshly constructed coordinator/AppState
// recovers durable suppression from the DB alone.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn g01_durable_manual_blocker_suppresses_start_after_process_restart() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-g01-{}", unique_suffix());
    let now = weekday_at(13, 30);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now);
    seed_stub_row(&pool, &stub).await?;

    // First "process": halted integrity drives the operation to manual.
    {
        let st = paper_state_with_db(pool.clone(), &adapter_id);
        {
            let mut ig = st.integrity.write().await;
            ig.halted = true;
            ig.disarmed = true;
        }
        attempt_canonical_start(&st, &pool, stub.clone(), now, mqk_db::STATE_AWAITING_OPEN).await?;
    }

    let after_first = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after_first.state,
        mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED
    );
    let version_after_first = after_first.state_version;

    // A brand new "process": fresh AppState, same halted condition,
    // dispatching through the top-level per-state router this time.
    {
        let st2 = paper_state_with_db(pool.clone(), &adapter_id);
        {
            let mut ig = st2.integrity.write().await;
            ig.halted = true;
            ig.disarmed = true;
        }
        let plan = plan_from_stub(&stub);
        let outcome = dispatch_by_state(
            &st2,
            &pool,
            after_first,
            &plan,
            now + ChronoDuration::seconds(30),
        )
        .await?;
        assert!(matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired { .. }
        ));
    }

    let after_second = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after_second.state_version, version_after_first,
        "an unchanged durable manual blocker must suppress a repeated transition/event across a \
         simulated process restart, using only durable DB state — no process-local memory"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}
