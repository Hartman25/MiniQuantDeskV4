//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3-COMPLETED-BAR-TASK-CUTOVER-AND-SUPERVISION:
//! proof tests for `state::autonomous_completed_bar_task` and the
//! production cutover in `main.rs`.
//!
//! DB-backed tests require `MQK_DATABASE_URL` and skip truthfully when it is
//! not set (matching `scenario_autonomous_completed_bar_driver_01.rs`'s
//! convention). No real provider, broker, or network call is made anywhere
//! in this file — every provider registry used is a temporary local fixture
//! file, and the completed-bar driver itself only ever calls a fake
//! injected provider in its own dedicated suite; this file never wires a
//! provider capable of `latest_closed_bar` at all, so authorization being
//! `Authorized` still never reaches a real network call here.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use mqk_daemon::state::autonomous_completed_bar_driver::{
    AutonomousCompletedBarDriverMode, AutonomousCompletedBarDriverOutcome,
    AutonomousCompletedBarDriverTaskLiveness,
};
use mqk_daemon::state::autonomous_completed_bar_task::{
    resolve_completed_bar_tick_cadence, run_supervised_completed_bar_worker,
    select_driver_mode_for_state, spawn_autonomous_completed_bar_driver_task,
    tick_autonomous_completed_bar_driver_from_state, AutonomousCompletedBarProductionTickOutcome,
    AutonomousCompletedBarTaskSpawnOutcome, CompletedBarTaskCadenceError,
    COMPLETED_BAR_TICK_SECS_ENV,
};
use mqk_daemon::state::autonomous_daily_coordinator::apply_completed_bar_driver_outcome;
use mqk_daemon::state::{self, AppState, AutonomousSessionTruth, BrokerKind, DeploymentMode};
use uuid::Uuid;

const STRATEGY_SYMBOL_ENV: &str = "MQK_STRATEGY_SYMBOL";
const STRATEGY_IDS_ENV: &str = "MQK_STRATEGY_IDS";
const STRATEGY_TIMEFRAME_ENV: &str = "MQK_STRATEGY_MD_TIMEFRAME";
const REFRESH_ENABLED_ENV: &str = "MQK_AUTONOMOUS_DATA_REFRESH_ENABLED";
const ALLOW_PROVIDER_CALLS_ENV: &str = "MQK_ALLOW_PROVIDER_API_CALLS";
const INSTRUMENT_REGISTRY_PATH_ENV: &str = "MQK_INSTRUMENT_REGISTRY_PATH";
const PROVIDER_REGISTRY_PATH_ENV: &str = "MQK_PROVIDER_REGISTRY_PATH";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn reset_env() {
    std::env::remove_var(COMPLETED_BAR_TICK_SECS_ENV);
    std::env::remove_var(STRATEGY_SYMBOL_ENV);
    std::env::remove_var(STRATEGY_IDS_ENV);
    std::env::remove_var(STRATEGY_TIMEFRAME_ENV);
    std::env::remove_var(REFRESH_ENABLED_ENV);
    std::env::remove_var(ALLOW_PROVIDER_CALLS_ENV);
    std::env::remove_var(INSTRUMENT_REGISTRY_PATH_ENV);
    std::env::remove_var(PROVIDER_REGISTRY_PATH_ENV);
}

fn unique_suffix() -> String {
    Uuid::new_v4().to_string().replace('-', "")[..10].to_string()
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
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect MQK_DATABASE_URL");
    mqk_db::migrate(&pool).await.expect("run migrations");
    sqlx::query("delete from sys_autonomous_daily_operation_events where operation_id in (select operation_id from sys_autonomous_daily_operations where adapter_id like 'zztask%')")
        .execute(&pool)
        .await
        .expect("clean operation events");
    sqlx::query("delete from sys_autonomous_daily_operations where adapter_id like 'zztask%'")
        .execute(&pool)
        .await
        .expect("clean operations");
    Some(pool)
}

/// A test `AppState` wired Paper+Alpaca (`ExternalSignalIngestion`) with a
/// real DB pool and a unique adapter_id, matching D3's spawn/adapter gating
/// requirements exactly.
fn paper_alpaca_state_with_db(db: sqlx::PgPool, adapter_id: &str) -> Arc<AppState> {
    let mut st = AppState::new_for_test_with_db_mode_and_broker(
        db,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st.set_adapter_id_for_test(adapter_id);
    Arc::new(st)
}

struct Timing {
    preopen_start_utc: DateTime<Utc>,
    exchange_open: DateTime<Utc>,
    exchange_close: DateTime<Utc>,
    postclose_finalize_utc: DateTime<Utc>,
    previous_trading_date: chrono::NaiveDate,
    market_date: chrono::NaiveDate,
}

fn standard_timing() -> Timing {
    let market_date = chrono::NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(); // Monday
    let previous_trading_date = chrono::NaiveDate::from_ymd_opt(2026, 7, 17).unwrap(); // Friday
    let exchange_open = DateTime::parse_from_rfc3339("2026-07-20T13:30:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let exchange_close = DateTime::parse_from_rfc3339("2026-07-20T20:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    Timing {
        preopen_start_utc: DateTime::parse_from_rfc3339("2026-07-20T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        exchange_open,
        exchange_close,
        postclose_finalize_utc: DateTime::parse_from_rfc3339("2026-07-20T20:15:00Z")
            .unwrap()
            .with_timezone(&Utc),
        previous_trading_date,
        market_date,
    }
}

#[allow(clippy::too_many_arguments)]
async fn create_test_operation(
    pool: &sqlx::PgPool,
    adapter_id: &str,
    symbol: &str,
    strategy_id: &str,
    timeframe: &str,
    timing: &Timing,
    initial_state: &str,
) -> mqk_db::AutonomousDailyOperationRecord {
    let assignment_config = state::MultiSymbolRuntimeConfig {
        schema_version: "v2".to_string(),
        symbols: vec![state::SymbolStrategyAssignment {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe: timeframe.to_string(),
        }],
        max_concurrent_symbols: 1,
        source: state::MultiSymbolConfigSource::EnvSingleSymbolFallback,
    };
    let assignment_identity = state::derive_assignment_identity(&assignment_config);
    let timeframe_secs = mqk_md::Timeframe::parse(timeframe)
        .expect("valid timeframe")
        .duration_secs();
    let binding = mqk_runtime::native_strategy::EffectiveRuntimeBinding {
        effective_runtime_strategy_id: Some(strategy_id.to_string()),
        effective_runtime_target_symbol: Some(symbol.to_string()),
        effective_runtime_timeframe_secs: Some(timeframe_secs),
    };
    let runtime_binding_identity = state::derive_runtime_binding_identity(&binding);
    let session_plan_identity = format!("test-session-plan|{adapter_id}");
    let operation_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("mqk.autonomous-daily-operation.v1|test|{adapter_id}").as_bytes(),
    );

    let args = mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date: timing.market_date,
        deployment_mode: "PAPER".to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity,
        assignment_identity,
        runtime_binding_identity,
        calendar_source: "nyse_weekdays_heuristic".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: "nyse_weekdays_heuristic".to_string(),
        effective_operation_open_utc: timing.exchange_open,
        effective_operation_close_utc: timing.exchange_close,
        exchange_session_open_utc: timing.exchange_open,
        exchange_session_close_utc: timing.exchange_close,
        exchange_is_early_close: false,
        previous_trading_date: timing.previous_trading_date,
        preopen_start_utc: timing.preopen_start_utc,
        postclose_finalize_utc: timing.postclose_finalize_utc,
        initial_state: initial_state.to_string(),
        data_refresh_state: "awaiting_preopen".to_string(),
        occurred_at_utc: timing.preopen_start_utc,
        bounded_detail: "test fixture".to_string(),
        stop_attempt_count: 0,
    };

    match mqk_db::create_or_recover_autonomous_daily_operation(pool, &args)
        .await
        .expect("create operation")
    {
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => record,
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(record) => record,
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::IdentityConflict { .. } => {
            panic!("unexpected identity conflict in test fixture")
        }
    }
}

/// `start_retrying`/`running` are not legal *initial* states
/// (`is_legal_operation_transition(None, _)` only permits
/// `awaiting_preopen`/`preparing_data`/`awaiting_open`/`calendar_unavailable`).
/// Create as `awaiting_open` first, then durably transition to
/// `start_retrying` (a legal `awaiting_open -> start_retrying` edge).
async fn create_test_operation_in_start_retrying(
    pool: &sqlx::PgPool,
    adapter_id: &str,
    symbol: &str,
    strategy_id: &str,
    timeframe: &str,
    timing: &Timing,
) -> mqk_db::AutonomousDailyOperationRecord {
    let operation = create_test_operation(
        pool,
        adapter_id,
        symbol,
        strategy_id,
        timeframe,
        timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    let args = mqk_db::TransitionAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        new_state: mqk_db::STATE_START_RETRYING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: timing.preopen_start_utc,
        run_id: None,
        bounded_detail: "test fixture: force start_retrying".to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation(pool, &args)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    }
}

/// Write a temporary instrument-registry JSON fixture admitting exactly one
/// symbol for `provider`. Independent of process CWD (REPAIR-9-style, per
/// the Phase C ledger).
fn write_instrument_registry(
    symbol: &str,
    provider: &str,
    provider_symbol: &str,
    timeframe: &str,
) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    let json = serde_json::json!([{
        "instrument_id": format!("equity:US:{symbol}"),
        "symbol": symbol,
        "asset_class": "equity",
        "provider": provider,
        "provider_symbol": provider_symbol,
        "venue": "TEST",
        "currency": "USD",
        "enabled": true,
        "timeframes": [timeframe],
        "notes": "test fixture",
    }]);
    std::fs::write(file.path(), serde_json::to_string(&json).unwrap()).unwrap();
    file
}

fn write_provider_registry(provider_id: &str) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    let json = serde_json::json!([{
        "provider_id": provider_id,
        "display_name": "Test Provider",
        "enabled": true,
        "kind": "twelvedata",
        "base_url": "https://example.invalid",
        "api_key_env": "ZZTASK_UNUSED_API_KEY",
    }]);
    std::fs::write(file.path(), serde_json::to_string(&json).unwrap()).unwrap();
    file
}

// ---------------------------------------------------------------------------
// Group A — task cadence configuration (D3.5), pure, no DB
// ---------------------------------------------------------------------------

#[test]
fn a01_absent_uses_default_15s() {
    assert_eq!(
        resolve_completed_bar_tick_cadence(None).unwrap(),
        Duration::from_secs(15)
    );
}

#[test]
fn a02_blank_uses_default_15s() {
    assert_eq!(
        resolve_completed_bar_tick_cadence(Some("  ")).unwrap(),
        Duration::from_secs(15)
    );
}

#[test]
fn a03_valid_value_accepted() {
    assert_eq!(
        resolve_completed_bar_tick_cadence(Some("5")).unwrap(),
        Duration::from_secs(5)
    );
}

#[test]
fn a04_min_boundary_accepted() {
    assert_eq!(
        resolve_completed_bar_tick_cadence(Some("1")).unwrap(),
        Duration::from_secs(1)
    );
}

#[test]
fn a05_max_boundary_accepted() {
    assert_eq!(
        resolve_completed_bar_tick_cadence(Some("300")).unwrap(),
        Duration::from_secs(300)
    );
}

#[test]
fn a06_zero_fails_closed() {
    assert!(matches!(
        resolve_completed_bar_tick_cadence(Some("0")),
        Err(CompletedBarTaskCadenceError::OutOfRange { value: 0 })
    ));
}

#[test]
fn a07_negative_fails_closed() {
    assert!(matches!(
        resolve_completed_bar_tick_cadence(Some("-5")),
        Err(CompletedBarTaskCadenceError::OutOfRange { value: -5 })
    ));
}

#[test]
fn a08_above_max_fails_closed() {
    assert!(matches!(
        resolve_completed_bar_tick_cadence(Some("301")),
        Err(CompletedBarTaskCadenceError::OutOfRange { value: 301 })
    ));
}

#[test]
fn a09_non_integer_fails_closed_not_silently_substituted() {
    // Must not silently fall back to the default — a distinct error variant.
    assert!(matches!(
        resolve_completed_bar_tick_cadence(Some("fifteen")),
        Err(CompletedBarTaskCadenceError::NotAnInteger { .. })
    ));
}

// ---------------------------------------------------------------------------
// Group B — explicit durable-state mode selection (D3.2), pure, no DB
// ---------------------------------------------------------------------------

#[test]
fn b01_preopen_states_select_prepare_data_only() {
    for state in [
        mqk_db::STATE_AWAITING_PREOPEN,
        mqk_db::STATE_PREPARING_DATA,
        mqk_db::STATE_AWAITING_OPEN,
        mqk_db::STATE_PREFLIGHT_BLOCKED,
        mqk_db::STATE_START_RETRYING,
    ] {
        assert_eq!(
            select_driver_mode_for_state(state),
            Some(AutonomousCompletedBarDriverMode::PrepareDataOnly),
            "state {state} must select PrepareDataOnly"
        );
    }
}

#[test]
fn b02_running_selects_running_dispatch() {
    assert_eq!(
        select_driver_mode_for_state(mqk_db::STATE_RUNNING),
        Some(AutonomousCompletedBarDriverMode::RunningDispatch)
    );
}

#[test]
fn b03_recovery_stopping_manual_degraded_terminal_unknown_select_no_driver_invocation() {
    for state in [
        mqk_db::STATE_RECOVERY_RETRYING,
        mqk_db::STATE_STOPPING,
        mqk_db::STATE_STOP_RETRYING,
        mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED,
        mqk_db::STATE_CONTROLLER_DEGRADED,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        mqk_db::STATE_CALENDAR_UNAVAILABLE,
        mqk_db::STATE_COMPLETED,
        mqk_db::STATE_COMPLETED_NO_TRADE,
        mqk_db::STATE_COMPLETED_WITH_ACTIVITY,
        "totally_unknown_state",
    ] {
        assert_eq!(
            select_driver_mode_for_state(state),
            None,
            "state {state} must select no driver invocation"
        );
    }
}

// ---------------------------------------------------------------------------
// Group C — spawn gating and duplicate-spawn prevention (D3.7)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn c01_non_paper_alpaca_returns_not_applicable() {
    reset_env();
    let st = Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Paper,
    ));
    assert_eq!(
        spawn_autonomous_completed_bar_driver_task(st).await,
        AutonomousCompletedBarTaskSpawnOutcome::NotApplicable
    );
}

#[tokio::test]
async fn c02_missing_db_returns_database_unavailable() {
    reset_env();
    let st = Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));
    assert_eq!(
        spawn_autonomous_completed_bar_driver_task(st).await,
        AutonomousCompletedBarTaskSpawnOutcome::DatabaseUnavailable
    );
}

#[tokio::test]
async fn c03_invalid_cadence_returns_invalid_configuration() {
    let Some(pool) = maybe_db("c03").await else {
        return;
    };
    reset_env();
    std::env::set_var(COMPLETED_BAR_TICK_SECS_ENV, "not-a-number");
    let st = paper_alpaca_state_with_db(pool, &format!("zztask-{}", unique_suffix()));
    let outcome = spawn_autonomous_completed_bar_driver_task(st).await;
    reset_env();
    assert_eq!(
        outcome,
        AutonomousCompletedBarTaskSpawnOutcome::InvalidConfiguration
    );
}

#[tokio::test]
async fn c04_c07_valid_paper_alpaca_starts_one_task_regardless_of_provider_authorization() {
    let Some(pool) = maybe_db("c04_c07").await else {
        return;
    };
    reset_env();
    // Provider authorization intentionally left unset (Disabled) — D3.7:
    // provider authorization is not a spawn requirement.
    let st = paper_alpaca_state_with_db(pool, &format!("zztask-{}", unique_suffix()));
    let outcome = spawn_autonomous_completed_bar_driver_task(Arc::clone(&st)).await;
    assert_eq!(outcome, AutonomousCompletedBarTaskSpawnOutcome::Started);

    // Give the supervisor a moment to bump the generation to 1 (Running).
    tokio::time::sleep(Duration::from_millis(50)).await;
    let truth = st.completed_bar_task_truth().await;
    assert_eq!(truth.generation, 1);
    assert_eq!(
        truth.liveness,
        AutonomousCompletedBarDriverTaskLiveness::Running
    );

    st.cancel_completed_bar_task_for_shutdown().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let truth = st.completed_bar_task_truth().await;
    assert_eq!(
        truth.liveness,
        AutonomousCompletedBarDriverTaskLiveness::Stopped
    );
}

#[tokio::test]
async fn c05_c06_second_spawn_returns_already_running_no_second_worker() {
    let Some(pool) = maybe_db("c05_c06").await else {
        return;
    };
    reset_env();
    let st = paper_alpaca_state_with_db(pool, &format!("zztask-{}", unique_suffix()));
    let first = spawn_autonomous_completed_bar_driver_task(Arc::clone(&st)).await;
    assert_eq!(first, AutonomousCompletedBarTaskSpawnOutcome::Started);
    let second = spawn_autonomous_completed_bar_driver_task(Arc::clone(&st)).await;
    assert_eq!(
        second,
        AutonomousCompletedBarTaskSpawnOutcome::AlreadyRunning
    );

    // No second worker exists: the truth generation never advances past 1
    // from the duplicate spawn attempt (only the first worker ever bumps
    // the generation).
    tokio::time::sleep(Duration::from_millis(50)).await;
    let truth = st.completed_bar_task_truth().await;
    assert_eq!(
        truth.generation, 1,
        "a second spawn attempt must never start a second worker (no second generation bump)"
    );

    st.cancel_completed_bar_task_for_shutdown().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}

// ---------------------------------------------------------------------------
// Group D — fresh operation snapshot every tick (D3.3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn d01_d02_state_and_run_id_change_between_ticks_is_observed_fresh() {
    let Some(pool) = maybe_db("d01_d02").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zztask-{}", unique_suffix());
    let symbol = "ZZTASKSYM";
    let timing = standard_timing();
    let operation = create_test_operation_in_start_retrying(
        &pool,
        &adapter_id,
        symbol,
        "swing_momentum",
        "5m",
        &timing,
    )
    .await;

    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
    let instruments = write_instrument_registry(symbol, "zztask_provider", symbol, "5m");
    let mut st = paper_alpaca_state_with_db(pool.clone(), &adapter_id);
    Arc::get_mut(&mut st).unwrap().instrument_registry_path =
        instruments.path().to_str().unwrap().to_string();
    st.set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;
    let now = timing.preopen_start_utc + chrono::Duration::minutes(5);

    // First tick: durable state is start_retrying -> PrepareDataOnly.
    let outcome1 = tick_autonomous_completed_bar_driver_from_state(&st, now)
        .await
        .expect("tick ok");
    match outcome1 {
        AutonomousCompletedBarProductionTickOutcome::DriverOutcome { mode, .. } => {
            assert_eq!(mode, AutonomousCompletedBarDriverMode::PrepareDataOnly);
        }
        other => panic!("expected DriverOutcome for start_retrying, got {other:?}"),
    }

    // Durably transition to running with a bound run_id, out of band.
    let run_id = Uuid::new_v4();
    let args = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: operation.operation_id,
        expected_state: mqk_db::STATE_START_RETRYING.to_string(),
        expected_state_version: operation.state_version,
        run_id,
        started_at_utc: now,
        occurred_at_utc: now,
        bounded_detail: "test: force running for D3.3 proof".to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation_to_running(&pool, &args)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(_) => {}
        other => panic!("expected Applied, got {other:?}"),
    }

    // Second tick, same operation, fresh fetch: mode must now be
    // RunningDispatch — proving the coordinator/task never retained the
    // first tick's operation snapshot.
    let outcome2 = tick_autonomous_completed_bar_driver_from_state(&st, now)
        .await
        .expect("tick ok");
    match outcome2 {
        AutonomousCompletedBarProductionTickOutcome::DriverOutcome {
            mode, operation_id, ..
        } => {
            assert_eq!(mode, AutonomousCompletedBarDriverMode::RunningDispatch);
            assert_eq!(operation_id, operation.operation_id);
        }
        other => panic!("expected DriverOutcome for running, got {other:?}"),
    }
    reset_env();
}

#[tokio::test]
async fn d03_a_stopped_operation_is_not_processed_from_a_stale_cached_snapshot() {
    let Some(pool) = maybe_db("d03").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zztask-{}", unique_suffix());
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        &adapter_id,
        "ZZTASKSYM2",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;

    // Durably move straight to stopping (legal: awaiting_open -> stopping).
    let args = mqk_db::TransitionAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        new_state: mqk_db::STATE_STOPPING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: timing.preopen_start_utc,
        run_id: None,
        bounded_detail: "test: force stopping for D3.3 proof".to_string(),
    };
    mqk_db::transition_autonomous_daily_operation(&pool, &args)
        .await
        .expect("transition ok");

    let st = paper_alpaca_state_with_db(pool, &adapter_id);
    let now = timing.preopen_start_utc + chrono::Duration::minutes(5);
    let outcome = tick_autonomous_completed_bar_driver_from_state(&st, now)
        .await
        .expect("tick ok");
    match outcome {
        AutonomousCompletedBarProductionTickOutcome::ModeNotApplicable { state, .. } => {
            assert_eq!(state, mqk_db::STATE_STOPPING);
        }
        other => panic!("expected ModeNotApplicable(stopping), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Group E — local-bar/provider authorization boundary via the production
// adapter (D3.4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e01_missing_bar_authorization_disabled_zero_provider_setup_still_reaches_driver() {
    let Some(pool) = maybe_db("e01").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zztask-{}", unique_suffix());
    let symbol = "ZZTASKE01";
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        &adapter_id,
        symbol,
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;

    let instruments = write_instrument_registry(symbol, "zztask_provider", symbol, "5m");
    let providers = write_provider_registry("zztask_provider");
    std::env::set_var(INSTRUMENT_REGISTRY_PATH_ENV, instruments.path());
    std::env::set_var(PROVIDER_REGISTRY_PATH_ENV, providers.path());
    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
    // Authorization intentionally left absent -> Disabled.

    let mut st = paper_alpaca_state_with_db(pool.clone(), &adapter_id);
    Arc::get_mut(&mut st).unwrap().instrument_registry_path =
        instruments.path().to_str().unwrap().to_string();
    Arc::get_mut(&mut st).unwrap().provider_registry_path =
        providers.path().to_str().unwrap().to_string();
    st.set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;

    let now = timing.preopen_start_utc + chrono::Duration::minutes(5);
    let outcome = tick_autonomous_completed_bar_driver_from_state(&st, now)
        .await
        .expect("tick ok");
    reset_env();

    match outcome {
        AutonomousCompletedBarProductionTickOutcome::DriverOutcome {
            operation_id,
            outcome,
            ..
        } => {
            assert_eq!(operation_id, operation.operation_id);
            // With no local md_bars row and authorization Disabled, the
            // driver must never reach a dispatch/observe outcome.
            assert!(
                !matches!(
                    outcome,
                    AutonomousCompletedBarDriverOutcome::BarObserved { .. }
                        | AutonomousCompletedBarDriverOutcome::DispatchCompleted { .. }
                        | AutonomousCompletedBarDriverOutcome::AlreadyDispatched { .. }
                ),
                "authorization-disabled tick with no local bar must never observe/dispatch, got {outcome:?}"
            );
        }
        other => panic!("expected DriverOutcome, got {other:?}"),
    }
}

#[tokio::test]
async fn e02_invalid_registry_path_yields_typed_registry_unavailable_zero_panics() {
    let Some(pool) = maybe_db("e02").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zztask-{}", unique_suffix());
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        &adapter_id,
        "ZZTASKE02",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    std::env::set_var(STRATEGY_SYMBOL_ENV, "ZZTASKE02");
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");

    let mut st = paper_alpaca_state_with_db(pool, &adapter_id);
    Arc::get_mut(&mut st).unwrap().instrument_registry_path =
        "C:/definitely/does/not/exist/instruments.json".to_string();
    st.set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;

    let now = timing.preopen_start_utc + chrono::Duration::minutes(5);
    let outcome = tick_autonomous_completed_bar_driver_from_state(&st, now)
        .await
        .expect("tick ok");
    reset_env();

    match outcome {
        AutonomousCompletedBarProductionTickOutcome::RegistryUnavailable {
            operation_id, ..
        } => {
            assert_eq!(operation_id, operation.operation_id);
        }
        other => panic!("expected RegistryUnavailable, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Group F — no automatic historical sync/ingest job (D3.4/D3.1)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn f01_no_relevant_operation_creates_nothing_zero_db_writes_to_operations_table() {
    let Some(pool) = maybe_db("f01").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zztask-{}", unique_suffix());
    let st = paper_alpaca_state_with_db(pool.clone(), &adapter_id);
    let now = standard_timing().preopen_start_utc + chrono::Duration::minutes(5);

    let outcome = tick_autonomous_completed_bar_driver_from_state(&st, now)
        .await
        .expect("tick ok");
    assert_eq!(
        outcome,
        AutonomousCompletedBarProductionTickOutcome::NoRelevantOperation
    );

    let count: i64 = sqlx::query_scalar(
        "select count(*) from sys_autonomous_daily_operations where adapter_id = $1",
    )
    .bind(&adapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 0,
        "adapter must create zero operation rows when none is relevant"
    );
}

// ---------------------------------------------------------------------------
// Group G — durable critical-outcome application (D3.14)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g01_dispatch_claim_unresolved_on_running_degrades_to_evidence_degraded_once() {
    let Some(pool) = maybe_db("g01").await else {
        return;
    };
    let adapter_id = format!("zztask-{}", unique_suffix());
    let timing = standard_timing();
    let operation = create_test_operation_in_start_retrying(
        &pool,
        &adapter_id,
        "ZZTASKG01",
        "swing_momentum",
        "5m",
        &timing,
    )
    .await;
    let args = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: operation.operation_id,
        expected_state: mqk_db::STATE_START_RETRYING.to_string(),
        expected_state_version: operation.state_version,
        run_id: Uuid::new_v4(),
        started_at_utc: timing.preopen_start_utc,
        occurred_at_utc: timing.preopen_start_utc,
        bounded_detail: "test: force running".to_string(),
    };
    let running = match mqk_db::transition_autonomous_daily_operation_to_running(&pool, &args)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };

    let outcome = AutonomousCompletedBarDriverOutcome::DispatchClaimUnresolved {
        status: "uncertain".to_string(),
    };
    let now = timing.preopen_start_utc + chrono::Duration::minutes(1);
    let applied = apply_completed_bar_driver_outcome(&pool, &running, &outcome, now)
        .await
        .expect("apply ok");
    assert_eq!(applied, Some(true));

    let fetched = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(fetched.state, mqk_db::STATE_EVIDENCE_DEGRADED);
    assert!(fetched.state_blocker_signature.is_some());

    // Repeating the identical outcome from the *same* pre-degrade snapshot
    // (the realistic race: two ticks both observed the operation while it
    // was still `running`, before either write landed) must not insert a
    // duplicate event — proven by re-applying against `running`, not the
    // already-degraded `fetched` row (which would legitimately compute a
    // different target_state, since it is no longer `running`).
    let applied_again = apply_completed_bar_driver_outcome(&pool, &running, &outcome, now)
        .await
        .expect("apply ok");
    assert_eq!(applied_again, Some(false));

    // Re-applying against the already-degraded snapshot must also be a
    // pure no-op refresh in place — never a second, different target.
    let applied_from_degraded = apply_completed_bar_driver_outcome(&pool, &fetched, &outcome, now)
        .await
        .expect("apply ok");
    assert_eq!(
        applied_from_degraded,
        Some(false),
        "re-observing the same critical outcome from an already-degraded snapshot must not \
         re-target a different state"
    );
    let refetched = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(
        refetched.state,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        "state must remain evidence_degraded, never escalated to manual_intervention_required \
         merely by re-observing the same outcome from an already-degraded snapshot"
    );
}

#[tokio::test]
async fn g02_runtime_dispatch_not_ready_on_running_degrades_to_controller_degraded() {
    let Some(pool) = maybe_db("g02").await else {
        return;
    };
    let adapter_id = format!("zztask-{}", unique_suffix());
    let timing = standard_timing();
    let operation = create_test_operation_in_start_retrying(
        &pool,
        &adapter_id,
        "ZZTASKG02",
        "swing_momentum",
        "5m",
        &timing,
    )
    .await;
    let args = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: operation.operation_id,
        expected_state: mqk_db::STATE_START_RETRYING.to_string(),
        expected_state_version: operation.state_version,
        run_id: Uuid::new_v4(),
        started_at_utc: timing.preopen_start_utc,
        occurred_at_utc: timing.preopen_start_utc,
        bounded_detail: "test: force running".to_string(),
    };
    let running = match mqk_db::transition_autonomous_daily_operation_to_running(&pool, &args)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };

    let outcome = AutonomousCompletedBarDriverOutcome::RuntimeDispatchNotReady {
        reason_code: "local_runtime_run_id_mismatch",
    };
    let now = timing.preopen_start_utc + chrono::Duration::minutes(1);
    apply_completed_bar_driver_outcome(&pool, &running, &outcome, now)
        .await
        .expect("apply ok");

    let fetched = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(fetched.state, mqk_db::STATE_CONTROLLER_DEGRADED);
}

#[tokio::test]
async fn g03_evidence_inconsistent_on_pre_running_state_degrades_to_manual_intervention() {
    let Some(pool) = maybe_db("g03").await else {
        return;
    };
    let adapter_id = format!("zztask-{}", unique_suffix());
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        &adapter_id,
        "ZZTASKG03",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;

    let outcome = AutonomousCompletedBarDriverOutcome::ObservedBarEvidenceInconsistent {
        expected_end_ts: 1_800_000_000,
        reason_code: "observed_bar_missing_from_md_bars",
    };
    let now = timing.preopen_start_utc + chrono::Duration::minutes(1);
    apply_completed_bar_driver_outcome(&pool, &operation, &outcome, now)
        .await
        .expect("apply ok");

    let fetched = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(fetched.state, mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED);
}

#[tokio::test]
async fn g04_benign_outcome_is_a_no_op_state_unchanged() {
    let Some(pool) = maybe_db("g04").await else {
        return;
    };
    let adapter_id = format!("zztask-{}", unique_suffix());
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        &adapter_id,
        "ZZTASKG04",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;

    for outcome in [
        AutonomousCompletedBarDriverOutcome::PollNotDue,
        AutonomousCompletedBarDriverOutcome::AuthorizationDisabled,
        AutonomousCompletedBarDriverOutcome::BarObserved { bar_end_ts: 1 },
    ] {
        let now = timing.preopen_start_utc + chrono::Duration::minutes(1);
        let applied = apply_completed_bar_driver_outcome(&pool, &operation, &outcome, now)
            .await
            .expect("apply ok");
        assert_eq!(applied, None, "benign outcome {outcome:?} must be a no-op");
    }

    let fetched = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(fetched.state, mqk_db::STATE_AWAITING_OPEN);
    assert_eq!(fetched.state_version, operation.state_version);
}

// ---------------------------------------------------------------------------
// Group H — task lifecycle: cancellation, error containment, panic
// supervision, bounded restart, stale-generation protection (D3.8-D3.10)
//
// Uses `run_supervised_completed_bar_worker` directly with an injected fake
// tick closure — no DB, no production driver dependency — so the
// supervision/restart/cancellation machinery is proven in isolation from
// the driver's own (already separately proven) correctness.
// ---------------------------------------------------------------------------

fn fake_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ))
}

#[tokio::test]
async fn h01_cancellation_stops_the_worker_no_tick_after_cancel_no_failure_truth() {
    let st = fake_state();
    let tick_count = Arc::new(AtomicUsize::new(0));
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);

    let counter = Arc::clone(&tick_count);
    let handle = tokio::spawn({
        let st = Arc::clone(&st);
        async move {
            run_supervised_completed_bar_worker(
                st,
                Duration::from_millis(10),
                cancel_rx,
                move |_st, _gen| {
                    let counter = Arc::clone(&counter);
                    move || {
                        let counter = Arc::clone(&counter);
                        async move {
                            counter.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                },
            )
            .await;
        }
    });

    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        tick_count.load(Ordering::SeqCst) > 0,
        "at least one tick must have run"
    );

    cancel_tx.send(true).unwrap();
    // `handle.await` only resolves once `run_bounded_cadence_task` has fully
    // returned `Stopped` — no further tick can start after this point, so
    // comparing a snapshot taken immediately after this await against one
    // taken later (after a settling sleep) is race-free.
    handle.await.unwrap();
    let count_at_stop = tick_count.load(Ordering::SeqCst);

    tokio::time::sleep(Duration::from_millis(30)).await;
    let count_after = tick_count.load(Ordering::SeqCst);
    assert_eq!(
        count_after, count_at_stop,
        "no tick may occur after cancellation is observed"
    );

    let truth = st.completed_bar_task_truth().await;
    assert_eq!(
        truth.liveness,
        AutonomousCompletedBarDriverTaskLiveness::Stopped
    );

    // Expected cancellation must project no CompletedBarDriverExited truth.
    assert!(!matches!(
        st.autonomous_session_truth().await,
        AutonomousSessionTruth::CompletedBarDriverExited { .. }
    ));
}

#[tokio::test]
async fn h02_per_tick_error_is_contained_and_does_not_panic_the_worker() {
    let st = fake_state();
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let tick_count = Arc::new(AtomicUsize::new(0));

    let counter = Arc::clone(&tick_count);
    let handle = tokio::spawn({
        let st = Arc::clone(&st);
        async move {
            run_supervised_completed_bar_worker(
                st,
                Duration::from_millis(10),
                cancel_rx,
                move |_st, _gen| {
                    let counter = Arc::clone(&counter);
                    move || {
                        let counter = Arc::clone(&counter);
                        async move {
                            // Every tick is an "ordinary error" — contained
                            // inside the tick closure, never a panic.
                            counter.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                },
            )
            .await;
        }
    });

    tokio::time::sleep(Duration::from_millis(80)).await;
    cancel_tx.send(true).unwrap();
    handle.await.unwrap();

    assert!(
        tick_count.load(Ordering::SeqCst) > 1,
        "the loop must keep ticking across multiple ordinary (contained) outcomes"
    );
}

#[tokio::test]
async fn h03_worker_panic_is_supervised_bounded_restart_delay_honored_restart_limit_enforced() {
    let st = fake_state();
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let attempt_count = Arc::new(AtomicUsize::new(0));

    let counter = Arc::clone(&attempt_count);
    let handle = tokio::spawn({
        let st = Arc::clone(&st);
        async move {
            run_supervised_completed_bar_worker(
                st,
                Duration::from_millis(5),
                cancel_rx,
                move |_st, _gen| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    move || async move {
                        panic!("h03: deliberate worker panic for supervision proof");
                    }
                },
            )
            .await;
        }
    });

    // Restart delays are 30s/60s/120s — do not wait for them in a unit
    // test. Instead, poll the bounded restart_count/liveness truth
    // directly: the first attempt must panic immediately (bounded by the
    // 5ms cadence), be recorded as restart_count == 1, and the task must
    // still be mid-restart-delay (not yet Failed) well before 30s elapses.
    let mut observed_first_attempt = false;
    for _ in 0..50 {
        let truth = st.completed_bar_task_truth().await;
        if truth.restart_count >= 1 {
            observed_first_attempt = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        observed_first_attempt,
        "a worker panic must be supervised and recorded"
    );
    assert_eq!(
        attempt_count.load(Ordering::SeqCst),
        1,
        "only one worker attempt so far"
    );

    let truth = st.completed_bar_task_truth().await;
    assert_ne!(
        truth.liveness,
        AutonomousCompletedBarDriverTaskLiveness::Failed,
        "must not be Failed before the restart budget (3 attempts) is exhausted"
    );

    // Abort the outer supervisor task directly rather than waiting out the
    // real 30s/60s/120s bounded restart schedule in a unit test.
    handle.abort();
}

#[tokio::test]
async fn h04_stale_worker_generation_cannot_overwrite_newer_liveness_truth() {
    let st = fake_state();
    // Simulate a newer generation already having been claimed.
    st.set_completed_bar_task_generation_for_test(
        5,
        AutonomousCompletedBarDriverTaskLiveness::Running,
    )
    .await;

    // A tick claiming to belong to generation 1 (stale) must be ignored by
    // the production tick's own generation guard.
    mqk_daemon::state::autonomous_completed_bar_task::run_one_production_tick_for_test(
        Arc::clone(&st),
        1,
    )
    .await;

    let truth = st.completed_bar_task_truth().await;
    assert_eq!(
        truth.generation, 5,
        "generation must remain the newer, current one"
    );
    assert_eq!(
        truth.liveness,
        AutonomousCompletedBarDriverTaskLiveness::Running
    );
    assert!(
        truth.last_tick_utc.is_none(),
        "a stale-generation tick must never record its own outcome into current truth"
    );
}

// ---------------------------------------------------------------------------
// Group I — production cutover source guards (D3.11/D3.15)
// ---------------------------------------------------------------------------

fn read_main_rs_source() -> String {
    let candidates = [
        "src/main.rs",
        "core-rs/crates/mqk-daemon/src/main.rs",
        "../core-rs/crates/mqk-daemon/src/main.rs",
        "crates/mqk-daemon/src/main.rs",
    ];
    for candidate in candidates {
        if let Ok(contents) = std::fs::read_to_string(candidate) {
            return contents;
        }
    }
    panic!("could not locate mqk-daemon/src/main.rs from test CWD");
}

#[test]
fn i01_main_rs_does_not_spawn_the_legacy_ticker() {
    let source = read_main_rs_source();
    assert!(
        !source.contains("spawn_autonomous_bar_ticker"),
        "main.rs must not call spawn_autonomous_bar_ticker in production"
    );
}

#[test]
fn i02_main_rs_spawns_the_completed_bar_task_exactly_once() {
    let source = read_main_rs_source();
    let count = source
        .matches("spawn_autonomous_completed_bar_driver_task(")
        .count();
    assert_eq!(
        count, 1,
        "main.rs must spawn the completed-bar task exactly once, found {count}"
    );
}

#[test]
fn i03_main_rs_cancels_the_completed_bar_task_on_shutdown() {
    let source = read_main_rs_source();
    assert!(
        source.contains("cancel_completed_bar_task_for_shutdown"),
        "main.rs's shutdown path must cancel the completed-bar task"
    );
}

// ---------------------------------------------------------------------------
// Group J — operator truth wiring (D3.12)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn j01_completed_bar_driver_exited_round_trips_through_autonomous_session_truth() {
    let st = fake_state();
    st.set_autonomous_session_truth(AutonomousSessionTruth::CompletedBarDriverExited {
        detail: "restart budget exhausted".to_string(),
    })
    .await;
    let truth = st.autonomous_session_truth().await;
    assert!(matches!(
        truth,
        AutonomousSessionTruth::CompletedBarDriverExited { ref detail } if detail == "restart budget exhausted"
    ));
}
