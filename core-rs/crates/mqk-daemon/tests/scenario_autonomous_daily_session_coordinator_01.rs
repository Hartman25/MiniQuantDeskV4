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
    handle_running_transition_store_error, handle_session_close, handle_stopping, retry_stop,
    tick_autonomous_daily_coordinator, AutonomousDailyCoordinatorTickInput,
    AutonomousDailyCoordinatorTickOutcome,
};
use mqk_daemon::state::{
    self, derive_assignment_identity, derive_autonomous_daily_operation_id,
    derive_runtime_binding_identity, resolve_autonomous_daily_session_plan_from_env,
    run_durable_session_controller_tick, AppState, AutonomousDailyPlanTiming,
    AutonomousDailySessionPlan, AutonomousDailySessionPlanResolution, AutonomousSessionTruth,
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
                reason_code: "operation_identity_conflict",
                ..
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

    // REPAIR 8: the existing (conflicting) operation must be durably
    // CAS-transitioned to manual_intervention_required exactly once, with a
    // full typed blocker signature persisted alongside the reason code.
    let existing_after_conflict =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, other_operation_id)
            .await?
            .expect("the existing (conflicting) row must still exist");
    assert_eq!(
        existing_after_conflict.state,
        mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED,
        "the existing operation must be durably transitioned to manual intervention"
    );
    assert_eq!(
        existing_after_conflict.state_reason_code.as_deref(),
        Some("operation_identity_conflict")
    );
    assert!(
        existing_after_conflict.state_blocker_signature.is_some(),
        "a full typed blocker signature must be persisted, not just the reason code"
    );
    let version_after_first_conflict = existing_after_conflict.state_version;

    // REPAIR 8 restart dedup: a second identical tick (simulating a fresh
    // process observing the same unchanged conflict) must not re-transition
    // the existing row and must report newly_applied = false.
    let outcome2 = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now + ChronoDuration::seconds(30),
    })
    .await?;
    assert_eq!(
        outcome2,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "operation_identity_conflict",
            newly_applied: false,
        },
        "an unchanged identity conflict must survive a process restart without a repeated \
         transition or notification"
    );
    let existing_after_second_tick =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, other_operation_id)
            .await?
            .expect("row must still exist");
    assert_eq!(
        existing_after_second_tick.state_version, version_after_first_conflict,
        "an unchanged identity conflict must never re-transition the existing row"
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
        state_blocker_signature: None,
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
                reason_code: "integrity_halted",
                ..
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
                reason_code: "durable_arm_disarmed",
                ..
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
                reason_code: "operator_managed_run_active",
                ..
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
    let (current, _) = apply_transition(
        pool,
        &current,
        mqk_db::STATE_START_RETRYING,
        None,
        None,
        now,
        None,
        "test setup",
    )
    .await?;
    let (record, _) = apply_transition(
        pool,
        &current,
        mqk_db::STATE_RUNNING,
        None,
        None,
        now,
        Some(run_id),
        "test setup",
    )
    .await?;
    Ok(record)
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
                reason_code: "runtime_run_id_mismatch",
                ..
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
    // REPAIR 9: handle_running now reads durable arm state before scheduling
    // recovery. `sys_arm_state` is a process-wide singleton shared by every
    // test in this binary (e.g. d02 durably persists DISARMED) — explicitly
    // re-persist ARMED here so this test proves the "terminal run, session
    // still open, durably ARMED" recovery path, matching the real
    // production invariant that reaching `running` always implies a prior
    // successful arm was persisted, rather than depending on leftover state
    // from test execution order.
    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None).await?;

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
    // REPAIR 9: see e03's comment — re-persist ARMED so this test proves
    // the halted-integrity override specifically, not incidental leftover
    // DISARMED state from another test sharing the same singleton row.
    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None).await?;

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
                reason_code: "runtime_ended_unsafely",
                ..
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

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn e05_durable_disarm_schedules_no_recovery() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-e05-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let operation = seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;
    // REPAIR 9: a durable DISARMED arm state must block recovery scheduling
    // before any retry timestamp is persisted — never postponed until the
    // next canonical start attempt.
    mqk_db::persist_arm_state_canonical(
        &pool,
        mqk_db::ArmState::Disarmed,
        Some(mqk_db::DisarmReason::OperatorHalt),
    )
    .await?;

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

    let outcome = handle_running(&st, &pool, operation, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "durable_arm_disarmed",
                ..
            }
        ),
        "a durable DISARMED arm state must block recovery scheduling, got {outcome:?}"
    );
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_ne!(row.state, mqk_db::STATE_RECOVERY_RETRYING);
    assert!(
        row.next_retry_utc.is_none(),
        "no retry timestamp may be persisted for a durably disarmed operation"
    );

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
    let (current, _) = apply_transition(
        &pool,
        &current,
        mqk_db::STATE_STOPPING,
        None,
        None,
        now,
        None,
        "test setup",
    )
    .await?;
    let (current, _) = apply_transition(
        &pool,
        &current,
        mqk_db::STATE_STOP_RETRYING,
        None,
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
        stub.postclose_finalize_utc,
        at_postclose_finalize,
    )
    .await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "unresolved_stop_failure_at_postclose_finalize",
                ..
            }
        ),
        "an unresolved stop at postclose_finalize_utc must become manual, got {outcome:?}"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn f05_active_durable_run_without_local_owner_is_never_marked_stopped() -> anyhow::Result<()>
{
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-f05-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let operation = seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;

    // REPAIR 3: the durable run row is left ARMED/RUNNING; no local runtime
    // is ever established for this AppState — the ordinary "daemon crashed
    // mid-session, restarted, but the run row was never terminated" case.
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

    let outcome = handle_session_close(&st, &pool, operation, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "durable_active_run_without_local_owner",
                ..
            }
        ),
        "an active orphaned durable run must never be presented as stopped, got {outcome:?}"
    );
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert!(
        row.stopped_at_utc.is_none(),
        "an active orphaned run must never be recorded as stopped"
    );
    assert_eq!(
        row.stop_attempt_count,
        Some(0),
        "no stop call may be made against an active orphaned run"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn f06_terminal_durable_run_without_local_owner_records_stopped_with_no_stop_call(
) -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-f06-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let operation = seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;

    // `reconcile_durable_run_without_local_owner` now requires a durably
    // clean global reconcile status before ever declaring an orphaned run
    // stopped (b5e6c05e) -- `sys_reconcile_status_state` is a global
    // singleton, so seed a known-clean baseline the same way every sibling
    // test in this repair chain does, rather than relying on whatever a
    // fresh test DB's default happens to be.
    mqk_db::persist_reconcile_status_state(
        &pool,
        &mqk_db::PersistReconcileStatusState {
            status: "ok",
            last_run_at_utc: Some(now),
            snapshot_watermark_ms: None,
            mismatched_positions: 0,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: None,
            updated_at_utc: now,
        },
    )
    .await?;

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
    mqk_db::stop_run(&pool, run_id).await?; // durably terminal

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
    assert_eq!(
        row.stop_attempt_count,
        Some(0),
        "a durably terminal orphaned run must be recorded stopped without any canonical stop call"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn f07_stop_retry_with_mismatched_local_run_makes_zero_stop_calls() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-f07-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let expected_run_id = Uuid::new_v4();
    let mismatched_run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let operation =
        seed_running_operation(&pool, operation_id, &adapter_id, expected_run_id, now).await?;
    let (operation, _) = apply_transition(
        &pool,
        &operation,
        mqk_db::STATE_STOPPING,
        None,
        None,
        now,
        None,
        "test setup",
    )
    .await?;

    // REPAIR 4: a runtime that appeared since routing decided to retry the
    // stop must be re-proven as owned immediately before the stop call —
    // never stopped merely because the operation remains stop_retrying.
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(mismatched_run_id)
        .await?;

    let outcome = retry_stop(&st, &pool, operation, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "mismatched_runtime_at_close",
                ..
            }
        ),
        "a mismatched local run must never be stopped, got {outcome:?}"
    );
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.stop_attempt_count,
        Some(0),
        "no stop call may be made against a mismatched runtime"
    );
    assert_eq!(
        st.locally_owned_run_id().await,
        Some(mismatched_run_id),
        "the mismatched runtime must remain untouched"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, expected_run_id).await;
    cleanup_run(&pool, mismatched_run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// PAPER-SOAK-DAILY-TERMINAL-FINALIZATION-01: proves the E1 outcome-truth
// contract's finalization-eligibility gate (docs/specs/autonomous_daily_
// paper_operations_01e_outcome_truth_contract.md §3.2 condition 3/4) and
// PAPER-SOAK-INBOUND-DRAIN-OWNERSHIP-01's stop_execution_runtime refusal
// compose correctly with zero additional production code: retry_stop's
// existing generic `Err(err) => ...schedule retry...` branch already treats
// the new `runtime.stop.orders_draining` conflict exactly like any other
// stop failure, so `stopped_at_utc` is never set -- and therefore
// classify_autonomous_daily_outcome can never run -- while a broker-
// reachable order for this operation's run remains unresolved. This closes
// the audit's compound concern (a bare ACK could otherwise be finalized as
// `completed_with_activity` before the order's real outcome is known)
// without weakening the E1 contract's own deliberate, source-verified
// decision (§5 tier 2) that reaching the broker is itself real, terminal-
// classification-worthy activity evidence once the underlying order has
// actually resolved.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn d11_stop_retry_defers_finalization_while_order_unresolved_then_stops_once_resolved(
) -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-d11-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let idem = format!("d11-order-{run_id}");

    let operation = seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;
    let (operation, _) = apply_transition(
        &pool,
        &operation,
        mqk_db::STATE_STOPPING,
        None,
        None,
        now,
        None,
        "test setup",
    )
    .await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(run_id).await?;

    // A broker-reachable order that has not yet resolved to a terminal
    // outcome -- exactly the scenario PAPER-SOAK-INBOUND-DRAIN-OWNERSHIP-01
    // must refuse to stop cleanly through.
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &idem,
        serde_json::json!({"symbol": "SPY", "quantity": 1, "order_type": "market", "time_in_force": "day"}),
    )
    .await?;
    sqlx::query("update oms_outbox set status = 'ACKED' where idempotency_key = $1")
        .bind(&idem)
        .execute(&pool)
        .await?;

    // First retry: the order is still unresolved.
    let first = retry_stop(&st, &pool, operation, now).await?;
    assert_eq!(
        first,
        AutonomousDailyCoordinatorTickOutcome::StopAttempted,
        "d11: stop must be attempted-and-deferred, not treated as a clean stop"
    );

    let after_first = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after_first.state,
        mqk_db::STATE_STOP_RETRYING,
        "d11: operation must move to stop_retrying, never stopping-with-stopped_at_utc"
    );
    assert!(
        after_first.stopped_at_utc.is_none(),
        "d11: stopped_at_utc must remain unset while a broker-reachable order is unresolved -- \
         this is the exact precondition (E1 contract §3.2 condition 3) that keeps \
         classify_autonomous_daily_outcome unreachable until the order actually resolves"
    );
    assert_eq!(
        st.locally_owned_run_id().await,
        Some(run_id),
        "d11: local ownership must be preserved so a late broker event for this order is still \
         durably ingested, not silently dropped"
    );

    // The order resolves: a late terminal fill is durably ingested, exactly
    // as it would arrive via the WS transport while ownership is preserved.
    let fill_msg_id = "alpaca:d11-broker-order:fill:2026-08-08T14:05:00.000000Z".to_string();
    mqk_db::inbox_insert_deduped_with_identity(
        &pool,
        run_id,
        &fill_msg_id,
        None,
        &idem,
        "broker-d11-order",
        "fill",
        &serde_json::json!({
            "type":               "fill",
            "broker_message_id":  fill_msg_id,
            "broker_fill_id":     serde_json::Value::Null,
            "internal_order_id":  idem,
            "broker_order_id":    "broker-d11-order",
            "symbol":             "SPY",
            "side":               "Buy",
            "delta_qty":          1_i64,
            "price_micros":       500_000_000_i64,
            "fee_micros":         0_i64
        }),
        0,
        now,
    )
    .await?;

    // Second retry: every broker-reachable order has now resolved.
    let second = retry_stop(&st, &pool, after_first, now).await?;
    assert_eq!(
        second,
        AutonomousDailyCoordinatorTickOutcome::RuntimeStopped,
        "d11: stop must succeed once the order has resolved"
    );
    let after_second = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert!(
        after_second.stopped_at_utc.is_some(),
        "d11: stopped_at_utc must now be set -- finalization is eligible"
    );
    assert_eq!(
        st.locally_owned_run_id().await,
        None,
        "d11: local ownership must be cleared once the drained stop completes"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
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

// ---------------------------------------------------------------------------
// H — Recovery retry sequencing (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn h01_recovery_retry_due_calls_start_without_illegal_start_retrying_edge(
) -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-h01-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now);
    seed_stub_row(&pool, &stub).await?;
    let current = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .unwrap();
    // Drive the row to recovery_retrying via the exact legal edges
    // handle_running itself uses (awaiting_open -> start_retrying -> running
    // -> recovery_retrying), proving this test exercises a durably legal
    // prior state, not a fabricated in-memory-only one.
    let (current, _) = apply_transition(
        &pool,
        &current,
        mqk_db::STATE_START_RETRYING,
        None,
        None,
        now,
        None,
        "test setup",
    )
    .await?;
    let (current, _) = apply_transition(
        &pool,
        &current,
        mqk_db::STATE_RUNNING,
        None,
        None,
        now,
        Some(Uuid::new_v4()),
        "test setup",
    )
    .await?;
    let (current, _) = apply_transition(
        &pool,
        &current,
        mqk_db::STATE_RECOVERY_RETRYING,
        Some("runtime_ended_without_halt"),
        None,
        now,
        None,
        "test setup",
    )
    .await?;
    assert_eq!(current.state, mqk_db::STATE_RECOVERY_RETRYING);

    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None).await?;
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    // REPAIR 1: before this repair, `attempt_canonical_start` durably
    // attempted the illegal `recovery_retrying -> start_retrying` edge here
    // and the whole tick errored out (the DB-level CAS+legality check
    // rejects that edge). It must now succeed and never persist
    // start_retrying for a recovery attempt.
    let outcome =
        attempt_canonical_start(&st, &pool, current, now, mqk_db::STATE_RECOVERY_RETRYING).await?;
    assert!(
        !matches!(outcome, AutonomousDailyCoordinatorTickOutcome::RetryNotDue),
        "the retry must be due at `now`, got {outcome:?}"
    );

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_ne!(
        row.state,
        mqk_db::STATE_START_RETRYING,
        "recovery_retrying must never durably transition through start_retrying"
    );
    assert_eq!(
        row.start_attempt_count, 1,
        "the canonical start call must still be attempted and counted exactly once"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// I — Manual notification dedup (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn i01_newly_applied_is_true_once_then_false_for_an_unchanged_blocker() -> anyhow::Result<()>
{
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-i01-{}", unique_suffix());
    let now = weekday_at(13, 30);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now);
    seed_stub_row(&pool, &stub).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let operator_run_id = Uuid::new_v4();
    st.establish_db_backed_active_run_for_test(operator_run_id)
        .await
        .expect("establish operator run");

    // First observation of the operator-managed-run blocker: newly applied.
    let outcome1 =
        attempt_canonical_start(&st, &pool, stub.clone(), now, mqk_db::STATE_AWAITING_OPEN).await?;
    assert_eq!(
        outcome1,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "operator_managed_run_active",
            newly_applied: true,
        },
        "REPAIR 11: the first observation of a new blocker must report newly_applied = true"
    );

    let updated = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");

    // Second tick, exact same unchanged blocker: never re-transitioned, and
    // newly_applied must be false — session_controller.rs's sole authority
    // for suppressing a repeated critical notification.
    let outcome2 = attempt_canonical_start(
        &st,
        &pool,
        updated.clone(),
        now + ChronoDuration::seconds(30),
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await?;
    assert_eq!(
        outcome2,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "operator_managed_run_active",
            newly_applied: false,
        },
        "REPAIR 11: an unchanged blocker must report newly_applied = false"
    );
    let after_second = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after_second.state_version, updated.state_version,
        "an unchanged blocker must never write a second transition"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// J — AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-FAILSAFE-RECOVERY-CLOSURE-01
// REPAIR 3: running identity-conflict degradation
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn j01_running_identity_conflict_becomes_controller_degraded_with_full_signature(
) -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-j01-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(run_id).await?;

    // A resolvable assignment env now computes a real assignment_identity
    // that differs from the seeded row's "stub-assignment" -- an identity
    // conflict against a `running` existing operation.
    set_resolvable_assignment_env("AAPL");

    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "operation_identity_conflict",
            newly_applied: true,
        },
        "REPAIR 3: a running identity conflict must be reported and durably applied"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must still exist");
    assert_eq!(
        after.state,
        mqk_db::STATE_CONTROLLER_DEGRADED,
        "REPAIR 3: running's only legal degraded edge is controller_degraded, never an \
         unpersisted manual outcome"
    );
    assert_eq!(
        after.state_reason_code.as_deref(),
        Some("operation_identity_conflict")
    );
    assert!(
        after.state_blocker_signature.is_some(),
        "a full typed blocker signature must be persisted"
    );
    assert_eq!(
        after.run_id,
        Some(run_id),
        "degrading to controller_degraded must never rewrite the bound run_id"
    );
    assert_eq!(
        st.locally_owned_run_id().await,
        Some(run_id),
        "REPAIR 3: no runtime interaction at all -- the matching runtime is neither stopped \
         nor attached during this degrade"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn j02_repeated_identical_running_conflict_creates_no_second_event() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-j02-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(run_id).await?;
    set_resolvable_assignment_env("AAPL");

    tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await?;
    let after_first = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");

    let outcome2 = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now + ChronoDuration::seconds(30),
    })
    .await?;
    assert_eq!(
        outcome2,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "operation_identity_conflict",
            newly_applied: false,
        },
        "an unchanged running conflict must never re-transition or re-notify"
    );
    let after_second = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after_second.state_version, after_first.state_version,
        "an unchanged conflict must write nothing"
    );

    let events: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_daily_operation_events where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(&pool)
    .await?;
    // 3 seed events (initial, start_retrying, running) + exactly 1 degrade
    // event.
    assert_eq!(
        events.0, 4,
        "a repeated identical conflict must never insert a second degrade event"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn j03_changed_conflicting_assignment_updates_signature_once_while_remaining_degraded(
) -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-j03-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(run_id).await?;
    set_resolvable_assignment_env("AAPL");

    tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await?;
    let after_first = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after_first.state, mqk_db::STATE_CONTROLLER_DEGRADED);
    let signature_after_first = after_first.state_blocker_signature.clone();

    // A different assignment env recomputes a different assignment_identity
    // -- the conflict signature changes, but the operation must remain
    // controller_degraded (never escalate straight to
    // manual_intervention_required merely because the signature changed).
    reset_env();
    set_resolvable_assignment_env("MSFT");

    let outcome2 = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now + ChronoDuration::seconds(30),
    })
    .await?;
    assert_eq!(
        outcome2,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "operation_identity_conflict",
            newly_applied: true,
        },
        "REPAIR 4: a changed blocker signature while already degraded must be newly applied"
    );
    let after_second = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after_second.state,
        mqk_db::STATE_CONTROLLER_DEGRADED,
        "REPAIR 4: a same-state blocker refresh must remain in the same degraded state"
    );
    assert_eq!(
        after_second.state_version,
        after_first.state_version + 1,
        "the refresh must bump state_version by exactly one"
    );
    assert_ne!(
        after_second.state_blocker_signature, signature_after_first,
        "a changed assignment identity must produce a changed signature"
    );

    // Repeating the now-current (MSFT) conflict must be a true no-op.
    let outcome3 = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now + ChronoDuration::seconds(60),
    })
    .await?;
    assert_eq!(
        outcome3,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "operation_identity_conflict",
            newly_applied: false,
        }
    );
    let after_third = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after_third.state_version, after_second.state_version);

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn j04_running_identity_conflict_still_reaches_canonical_stop_at_persisted_close(
) -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-j04-{}", unique_suffix());
    let now = weekday_at(14, 0); // close = now + 6h = 20:00 UTC
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(run_id).await?;
    // The assignment env is never reverted -- the identity conflict recurs
    // on every subsequent tick, exactly the "recurring conflict" scenario
    // REPAIR 3's close-time branch exists to handle.
    set_resolvable_assignment_env("AAPL");

    tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await?;
    let degraded = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(degraded.state, mqk_db::STATE_CONTROLLER_DEGRADED);

    let at_close = now + ChronoDuration::hours(6) + ChronoDuration::minutes(1);
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: at_close,
    })
    .await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::RuntimeStopped,
        "REPAIR 3: an identity conflict that recurs every tick must never strand the matching \
         runtime -- persisted close still triggers canonical stop reconciliation, got {outcome:?}"
    );
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "the matching runtime must actually be stopped"
    );
    let stopped = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert!(stopped.stopped_at_utc.is_some());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// K — REPAIR 4: same-state blocker refresh (manual_intervention_required)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn k01_changed_manual_blocker_records_exactly_one_self_refresh_event() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-k01-{}", unique_suffix());
    let now = weekday_at(13, 30);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now);
    seed_stub_row(&pool, &stub).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let operator_run_id = Uuid::new_v4();
    st.establish_db_backed_active_run_for_test(operator_run_id)
        .await?;

    // First blocker: an operator-managed run active, via the real start
    // path -- reaches manual_intervention_required with reason
    // "operator_managed_run_active".
    let outcome1 =
        attempt_canonical_start(&st, &pool, stub.clone(), now, mqk_db::STATE_AWAITING_OPEN).await?;
    assert_eq!(
        outcome1,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "operator_managed_run_active",
            newly_applied: true,
        }
    );
    let after_first = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after_first.state,
        mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED
    );

    // A second, different blocker while already manual_intervention_required
    // -- a same-day identity conflict recomputed against this same slot.
    // Before REPAIR 4 this exact self-loop (manual -> manual with a changed
    // reason) was an illegal CAS transition; it must now durably refresh in
    // place.
    set_resolvable_assignment_env("AAPL");
    let outcome2 = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now + ChronoDuration::seconds(30),
    })
    .await?;
    assert_eq!(
        outcome2,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "operation_identity_conflict",
            newly_applied: true,
        },
        "REPAIR 4: a changed blocker while already manual_intervention_required must refresh, \
         not silently drop or bail"
    );
    let after_second = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after_second.state,
        mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED,
        "a same-state refresh must remain in manual_intervention_required"
    );
    assert_eq!(after_second.state_version, after_first.state_version + 1);
    assert_eq!(
        after_second.state_reason_code.as_deref(),
        Some("operation_identity_conflict")
    );

    let events: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_daily_operation_events where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(&pool)
    .await?;
    // 1 initial event + 1 (awaiting_open -> manual) + 1 self-refresh event.
    assert_eq!(
        events.0, 3,
        "the self-refresh must insert exactly one append-only event"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, operator_run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn k02_repeating_changed_manual_blocker_produces_no_further_event() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-k02-{}", unique_suffix());
    let now = weekday_at(13, 30);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now);
    seed_stub_row(&pool, &stub).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let operator_run_id = Uuid::new_v4();
    st.establish_db_backed_active_run_for_test(operator_run_id)
        .await?;
    attempt_canonical_start(&st, &pool, stub.clone(), now, mqk_db::STATE_AWAITING_OPEN).await?;

    set_resolvable_assignment_env("AAPL");
    tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now + ChronoDuration::seconds(30),
    })
    .await?;
    let after_refresh = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");

    let outcome3 = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now + ChronoDuration::seconds(60),
    })
    .await?;
    assert_eq!(
        outcome3,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "operation_identity_conflict",
            newly_applied: false,
        }
    );
    let after_repeat = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after_repeat.state_version, after_refresh.state_version);

    let events: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_daily_operation_events where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        events.0, 3,
        "repeating an unchanged refreshed blocker must insert no further event"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, operator_run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// L — REPAIR 1/2: existing-operation fallback under current resolution
// failure
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn l01_assignment_resolution_failure_degrades_running_operation_and_stops_at_close(
) -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-l01-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(run_id).await?;
    // No assignment env configured -- build_multi_symbol_runtime_config_from_env
    // fails this tick, but a relevant (running) existing operation exists.

    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "assignment_resolution_unavailable",
            newly_applied: true,
        },
        "REPAIR 2: a current assignment-resolution failure with a relevant running operation \
         must degrade it, not report a bare IdentityBlocked outcome"
    );
    let degraded = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must still exist");
    assert_eq!(degraded.state, mqk_db::STATE_CONTROLLER_DEGRADED);
    assert_eq!(
        degraded.start_attempt_count, 0,
        "REPAIR 2: must never attempt a new start without freshly proven configuration"
    );
    assert_eq!(
        st.locally_owned_run_id().await,
        Some(run_id),
        "the matching runtime must remain untouched before close"
    );

    let count: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_daily_operations where deployment_mode = 'PAPER' \
         and adapter_id = $1",
    )
    .bind(&adapter_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        count.0, 1,
        "REPAIR 2: must never create a second operation for this slot family"
    );

    let at_close = now + ChronoDuration::hours(6) + ChronoDuration::minutes(1);
    let outcome2 = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: at_close,
    })
    .await?;
    assert_eq!(
        outcome2,
        AutonomousDailyCoordinatorTickOutcome::RuntimeStopped,
        "REPAIR 2: the matching runtime must still be stopped at persisted close even though \
         current assignment resolution never recovered, got {outcome2:?}"
    );
    assert!(st.locally_owned_run_id().await.is_none());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn l02_no_relevant_existing_operation_with_invalid_config_creates_nothing(
) -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-l02-{}", unique_suffix());
    let now = weekday_at(12, 0);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::IdentityBlocked {
            reason_code: "assignment_missing",
        },
        "REPAIR 2: with no relevant existing operation, the ordinary blocked outcome must be \
         reported unchanged"
    );

    let row = fetch_slot_row(&pool, &adapter_id).await?;
    assert!(
        row.is_none(),
        "REPAIR 1/2: no operation may be created when none is relevant and config is invalid"
    );
    assert!(st.locally_owned_run_id().await.is_none());

    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn l03_invalid_fixed_window_override_does_not_strand_existing_running_operation(
) -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-l03-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(run_id).await?;
    // Only one of the two required override env vars is set -- an invalid
    // configuration, never treated as absent (mirrors a03).
    std::env::set_var(SESSION_START_ENV, "14:30");

    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "calendar_resolution_unavailable",
            newly_applied: true,
        }
    );
    let degraded = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must still exist");
    assert_eq!(degraded.state, mqk_db::STATE_CONTROLLER_DEGRADED);

    let at_close = now + ChronoDuration::hours(6) + ChronoDuration::minutes(1);
    let outcome2 = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: at_close,
    })
    .await?;
    std::env::remove_var(SESSION_START_ENV);
    assert_eq!(
        outcome2,
        AutonomousDailyCoordinatorTickOutcome::RuntimeStopped,
        "an invalid fixed-window override must never strand an existing running operation at \
         its own persisted close, got {outcome2:?}"
    );
    assert!(st.locally_owned_run_id().await.is_none());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// M — REPAIR 5: atomic running-transition error containment
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn m01_store_error_with_unconfirmed_reread_stops_local_runtime() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-m01-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now);
    seed_stub_row(&pool, &stub).await?;
    let seeded = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let (in_start_retrying, _) = apply_transition(
        &pool,
        &seeded,
        mqk_db::STATE_START_RETRYING,
        None,
        None,
        now,
        None,
        "test setup",
    )
    .await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let run_id = Uuid::new_v4();
    // Simulates: the canonical start call already succeeded and this run is
    // locally owned, but the DB was never actually updated to `running`
    // (the store call is about to report an error).
    st.establish_db_backed_active_run_for_test(run_id).await?;

    let outcome = handle_running_transition_store_error(
        &st,
        &pool,
        &in_start_retrying,
        run_id,
        mqk_db::STATE_START_RETRYING,
        anyhow::anyhow!("simulated store error"),
        now,
    )
    .await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "durable_transition_unconfirmed_after_start",
            newly_applied: true,
        },
        "REPAIR 5: a store error whose re-read does not prove commit must fail closed"
    );
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "REPAIR 5: the run must be best-effort stopped, never left active and unmanaged"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after.state, mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED);
    assert_ne!(
        after.run_id,
        Some(run_id),
        "the unconfirmed run must never be silently adopted onto the operation"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn m02_store_error_with_confirmed_reread_is_accepted_idempotently() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-m02-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now);
    seed_stub_row(&pool, &stub).await?;
    let seeded = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let (in_start_retrying, _) = apply_transition(
        &pool,
        &seeded,
        mqk_db::STATE_START_RETRYING,
        None,
        None,
        now,
        None,
        "test setup",
    )
    .await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let run_id = Uuid::new_v4();
    st.establish_db_backed_active_run_for_test(run_id).await?;

    // The running transition actually commits for real -- simulating a
    // transaction that committed server-side despite the client observing
    // an error on the same call.
    let to_running_args = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: in_start_retrying.operation_id,
        expected_state: in_start_retrying.state.clone(),
        expected_state_version: in_start_retrying.state_version,
        run_id,
        started_at_utc: now,
        occurred_at_utc: now,
        bounded_detail: "test: real commit before simulated client-side error".to_string(),
    };
    let apply_outcome =
        mqk_db::transition_autonomous_daily_operation_to_running(&pool, &to_running_args).await?;
    assert!(matches!(
        apply_outcome,
        mqk_db::AutonomousDailyTransitionOutcome::Applied(_)
    ));

    let outcome = handle_running_transition_store_error(
        &st,
        &pool,
        &in_start_retrying,
        run_id,
        mqk_db::STATE_START_RETRYING,
        anyhow::anyhow!("simulated store error despite real commit"),
        now,
    )
    .await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::Started { run_id },
        "REPAIR 5: a re-read that proves the exact commit must be accepted as durable truth"
    );
    assert_eq!(
        st.locally_owned_run_id().await,
        Some(run_id),
        "a confirmed commit must never trigger a best-effort stop"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after.state, mqk_db::STATE_RUNNING);
    assert_eq!(after.run_id, Some(run_id));

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// N — REPAIR 6: preflight assignment loss
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n01_preflight_blocked_assignment_loss_transitions_durably_to_manual() -> anyhow::Result<()>
{
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-n01-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now);
    seed_stub_row(&pool, &stub).await?;
    let seeded = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let (in_preflight_blocked, _) = apply_transition(
        &pool,
        &seeded,
        mqk_db::STATE_PREFLIGHT_BLOCKED,
        Some("latest_completed_bar_pending"),
        None,
        now,
        None,
        "test setup",
    )
    .await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    // reset_env() above leaves no assignment configured.
    let plan = plan_from_stub(&stub);
    let outcome = dispatch_by_state(&st, &pool, in_preflight_blocked.clone(), &plan, now).await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "assignment_missing",
            newly_applied: true,
        },
        "REPAIR 6: assignment loss from preflight_blocked must durably apply the typed manual \
         blocker, not return an unpersisted PreflightBlocked outcome"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after.state, mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED);
    assert_eq!(
        after.state_reason_code.as_deref(),
        Some("assignment_missing")
    );
    assert!(after.state_blocker_signature.is_some());
    assert_eq!(
        after.state_version,
        in_preflight_blocked.state_version + 1,
        "exactly one transition must be recorded"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// O — REPAIR 7: coordinator tick error operator-truth projection
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn o01_coordinator_tick_error_projects_bounded_start_refused_truth() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-o01-{}", unique_suffix());
    let now = weekday_at(14, 0);

    // Two operations for the exact same (deployment_mode, adapter_id) slot
    // family, both `running` (and therefore both "relevant"), on different
    // market dates -- REPAIR 1's fail-closed multi-row guard is the only
    // reachable, deterministic way (short of DB fault injection) to make
    // `tick_autonomous_daily_coordinator` itself return `Err` from a
    // legitimate setup.
    let market_date_a = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let market_date_b = NaiveDate::from_ymd_opt(2026, 7, 21).unwrap();
    let run_id_a = Uuid::new_v4();
    let run_id_b = Uuid::new_v4();
    let op_a = seed_running_operation(&pool, Uuid::new_v4(), &adapter_id, run_id_a, now).await?;
    let create_args_b = mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id: Uuid::new_v4(),
        market_date: market_date_b,
        deployment_mode: "PAPER".to_string(),
        adapter_id: adapter_id.clone(),
        session_plan_identity: "stub-plan-b".to_string(),
        assignment_identity: "stub-assignment-b".to_string(),
        runtime_binding_identity: "stub-binding-b".to_string(),
        calendar_source: "nyse_weekdays_heuristic".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: "nyse_weekdays_heuristic".to_string(),
        effective_operation_open_utc: now,
        effective_operation_close_utc: now + ChronoDuration::hours(6),
        exchange_session_open_utc: now,
        exchange_session_close_utc: now + ChronoDuration::hours(6),
        exchange_is_early_close: false,
        previous_trading_date: market_date_a,
        preopen_start_utc: now - ChronoDuration::minutes(30),
        postclose_finalize_utc: now + ChronoDuration::hours(6) + ChronoDuration::minutes(15),
        initial_state: mqk_db::STATE_AWAITING_OPEN.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: now,
        bounded_detail: "stub seed b".to_string(),
        stop_attempt_count: 0,
    };
    let op_b =
        match mqk_db::create_or_recover_autonomous_daily_operation(&pool, &create_args_b).await? {
            mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => record,
            other => panic!("expected Created, got {other:?}"),
        };
    let (op_b, _) = apply_transition(
        &pool,
        &op_b,
        mqk_db::STATE_START_RETRYING,
        None,
        None,
        now,
        None,
        "test setup",
    )
    .await?;
    let (op_b, _) = apply_transition(
        &pool,
        &op_b,
        mqk_db::STATE_RUNNING,
        None,
        None,
        now,
        Some(run_id_b),
        "test setup",
    )
    .await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    // No assignment env configured -- forces a resolution failure this
    // tick, which is what drives the coordinator into
    // `fetch_relevant_open_autonomous_daily_operation`.
    run_durable_session_controller_tick(&st, now).await;

    assert_eq!(
        st.autonomous_session_truth().await,
        AutonomousSessionTruth::StartRefused {
            detail: "reason_code=coordinator_tick_failed".to_string(),
        },
        "REPAIR 7: an unexpected tick error must project bounded degraded operator truth, not \
         only be logged"
    );

    cleanup_operation(&pool, op_a.operation_id).await;
    cleanup_operation(&pool, op_b.operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// P — AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-NONTRADING-RECOVERY-AND-
// RUNNING-CONFIRMATION-01
// REPAIR 1/3: nontrading-day existing-operation reconciliation
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn p01_weekend_with_existing_running_operation_reaches_canonical_close() -> anyhow::Result<()>
{
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-p01-{}", unique_suffix());
    // Seeded on a weekday; effective_operation_close_utc = seed_now + 6h.
    let seed_now = weekday_at(13, 30);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    seed_running_operation(&pool, operation_id, &adapter_id, run_id, seed_now).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(run_id).await?;

    // 2026-07-25 is the Saturday following the 2026-07-20 Monday seed date --
    // well after the operation's own persisted close.
    let saturday = Utc.with_ymd_and_hms(2026, 7, 25, 15, 0, 0).unwrap();
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: saturday,
    })
    .await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::RuntimeStopped,
        "REPAIR 1/3: a Friday-seeded operation still running when observed on a weekend must \
         reach canonical close reconciliation and stop the matching runtime, got {outcome:?}"
    );
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "the matching runtime must actually be stopped"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert!(after.stopped_at_utc.is_some());

    let count: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_daily_operations where adapter_id = $1",
    )
    .bind(&adapter_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        count.0, 1,
        "REPAIR 1: a nontrading-day lookup must create no new daily operation"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn p02_weekend_with_db_and_no_existing_operation_creates_nothing() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-p02-{}", unique_suffix());
    let st = paper_state_with_db(pool.clone(), &adapter_id);

    let saturday = Utc.with_ymd_and_hms(2026, 7, 25, 15, 0, 0).unwrap();
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: saturday,
    })
    .await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::NotApplicable { .. }
        ),
        "a normal weekend with no unresolved operation must remain quiet, got {outcome:?}"
    );

    let count: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_daily_operations where adapter_id = $1",
    )
    .bind(&adapter_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        count.0, 0,
        "REPAIR 1/2: no operation may be created merely by looking up a nontrading day"
    );
    assert!(st.locally_owned_run_id().await.is_none());

    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn p03_holiday_stopping_operation_continues_restart_safe_stop_reconciliation(
) -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-p03-{}", unique_suffix());
    // Friday 2026-07-03 is the observed Independence Day market holiday
    // (see a02) -- seeded and observed on the same holiday date.
    let seed_now = Utc.with_ymd_and_hms(2026, 7, 3, 13, 0, 0).unwrap();
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let running =
        seed_running_operation(&pool, operation_id, &adapter_id, run_id, seed_now).await?;
    let (stopping, _) = apply_transition(
        &pool,
        &running,
        mqk_db::STATE_STOPPING,
        None,
        None,
        seed_now,
        None,
        "test setup",
    )
    .await?;
    assert_eq!(stopping.run_id, Some(run_id));

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(run_id).await?;

    let later_same_holiday = seed_now + ChronoDuration::minutes(5);
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: later_same_holiday,
    })
    .await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::RuntimeStopped,
        "REPAIR 3: an existing stopping operation observed on a nontrading day must continue \
         restart-safe stop reconciliation, got {outcome:?}"
    );
    assert!(st.locally_owned_run_id().await.is_none());
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert!(after.stopped_at_utc.is_some());

    let count: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_daily_operations where adapter_id = $1",
    )
    .bind(&adapter_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        count.0, 1,
        "REPAIR 1: a nontrading-day lookup must create no new daily operation"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Q — REPAIR 5: legal degraded target after an uncertain running transition
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn q01_store_error_reread_showing_unrelated_running_degrades_legally() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-q01-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let operation_id = Uuid::new_v4();
    let stub = stub_operation_awaiting_open(operation_id, &adapter_id, now);
    seed_stub_row(&pool, &stub).await?;
    let seeded = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let (in_start_retrying, _) = apply_transition(
        &pool,
        &seeded,
        mqk_db::STATE_START_RETRYING,
        None,
        None,
        now,
        None,
        "test setup",
    )
    .await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let committed_run_id = Uuid::new_v4();
    st.establish_db_backed_active_run_for_test(committed_run_id)
        .await?;

    // The running transition genuinely commits (to `committed_run_id`) --
    // simulating a re-read that proves the operation is durably `running`,
    // but never proves the exact commit this call's own `run_id` parameter
    // expected.
    let to_running_args = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: in_start_retrying.operation_id,
        expected_state: in_start_retrying.state.clone(),
        expected_state_version: in_start_retrying.state_version,
        run_id: committed_run_id,
        started_at_utc: now,
        occurred_at_utc: now,
        bounded_detail: "test: unrelated running commit".to_string(),
    };
    let apply_outcome =
        mqk_db::transition_autonomous_daily_operation_to_running(&pool, &to_running_args).await?;
    assert!(matches!(
        apply_outcome,
        mqk_db::AutonomousDailyTransitionOutcome::Applied(_)
    ));

    let attempted_run_id = Uuid::new_v4();
    let outcome = handle_running_transition_store_error(
        &st,
        &pool,
        &in_start_retrying,
        attempted_run_id,
        mqk_db::STATE_START_RETRYING,
        anyhow::anyhow!("simulated store error"),
        now,
    )
    .await?;
    // Before REPAIR 5, a re-read row genuinely `running` (whose only legal
    // manual-adjacent edge is `controller_degraded`) would have attempted
    // the illegal `running -> manual_intervention_required` edge, causing an
    // uncaught `anyhow::bail!` inside `apply_transition`.
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "durable_transition_unconfirmed_after_start",
            newly_applied: true,
        },
        "REPAIR 5: must degrade legally, never bail on an illegal edge, got {outcome:?}"
    );
    assert!(
        st.locally_owned_run_id().await.is_none(),
        "REPAIR 5: the locally-owned run must still be best-effort stopped"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after.state,
        mqk_db::STATE_CONTROLLER_DEGRADED,
        "REPAIR 5: running's only legal degraded edge is controller_degraded"
    );
    assert!(
        after.state_blocker_signature.is_some(),
        "REPAIR 6: a full blocker signature must be persisted"
    );
    assert_eq!(
        after.run_id,
        Some(committed_run_id),
        "degrading to controller_degraded must never rewrite the durably bound run_id"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, committed_run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn q02_store_error_reread_showing_degraded_state_refreshes_in_place() -> anyhow::Result<()> {
    reset_env();
    let pool = test_pool().await?;
    let adapter_id = format!("coord-q02-{}", unique_suffix());
    let now = weekday_at(14, 0);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let running = seed_running_operation(&pool, operation_id, &adapter_id, run_id, now).await?;
    let (degraded, _) = apply_transition(
        &pool,
        &running,
        mqk_db::STATE_CONTROLLER_DEGRADED,
        Some("prior_reason"),
        None,
        now,
        None,
        "test setup",
    )
    .await?;
    assert_eq!(degraded.run_id, Some(run_id));

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.establish_db_backed_active_run_for_test(run_id).await?;

    let outcome = handle_running_transition_store_error(
        &st,
        &pool,
        &degraded,
        run_id,
        mqk_db::STATE_RECOVERY_RETRYING,
        anyhow::anyhow!("simulated store error"),
        now,
    )
    .await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "durable_transition_unconfirmed_after_start",
            newly_applied: true,
        }
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after.state,
        mqk_db::STATE_CONTROLLER_DEGRADED,
        "REPAIR 5: an already blocker-refresh-eligible state must remain in the same state -- a \
         same-state refresh, never an escalation"
    );
    assert_eq!(after.state_version, degraded.state_version + 1);
    assert_eq!(
        after.state_reason_code.as_deref(),
        Some("durable_transition_unconfirmed_after_start")
    );
    assert_eq!(after.run_id, Some(run_id));

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}
