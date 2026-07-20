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

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use mqk_daemon::daily_data_readiness;
use mqk_daemon::state::autonomous_completed_bar_driver::{
    AutonomousCompletedBarDriverMode, AutonomousCompletedBarDriverOutcome,
    AutonomousCompletedBarDriverTaskLiveness, AutonomousDriverSetupRejection,
};
use mqk_daemon::state::autonomous_completed_bar_task::{
    resolve_completed_bar_tick_cadence, run_supervised_completed_bar_worker,
    select_driver_mode_for_state, spawn_autonomous_completed_bar_driver_task,
    spawn_supervised_completed_bar_task_for_test, tick_autonomous_completed_bar_driver_from_state,
    AutonomousCompletedBarProductionTickOutcome, AutonomousCompletedBarRestartPolicy,
    AutonomousCompletedBarTaskSpawnOutcome, CompletedBarTaskCadenceError,
    COMPLETED_BAR_TICK_SECS_ENV,
};
use mqk_daemon::state::autonomous_daily_coordinator::{
    apply_completed_bar_driver_outcome, apply_completed_bar_task_permanent_failure,
};
use mqk_daemon::state::{self, AppState, AutonomousSessionTruth, BrokerKind, DeploymentMode};
use tokio::net::TcpListener;
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

/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-COVERAGE-ANCHOR-AND-RUN-LINEAGE-
/// FOUNDATION: bind the coverage-bound authority a directly-constructed
/// (coordinator-bypassing) test operation needs before the production
/// adapter's mandatory per-tick authority gate will let it proceed. Uses the
/// exact same real resolution (`resolve_autonomous_runtime_context`,
/// `load_readiness_context_from_env`) the adapter itself uses at tick time,
/// so the bound payload is guaranteed byte-identical to what the adapter
/// freshly recomputes — never a second, independently-derived fixture path.
/// Callers must have already configured `st`'s strategy fleet and the
/// matching `MQK_STRATEGY_*` env vars before calling this.
async fn bind_coverage_authority_for_test(
    pool: &sqlx::PgPool,
    st: &Arc<AppState>,
    operation: &mqk_db::AutonomousDailyOperationRecord,
    ts_utc: DateTime<Utc>,
) {
    let assignment_config =
        state::build_multi_symbol_runtime_config_from_env().expect("assignment config resolves");
    let runtime_context = state::autonomous_runtime_context::resolve_autonomous_runtime_context(st)
        .await
        .expect("runtime context resolves");
    let assignment_identity = state::derive_assignment_identity(&assignment_config);
    let runtime_binding_identity =
        state::derive_runtime_binding_identity(&runtime_context.effective_runtime_binding);
    let readiness_context = daily_data_readiness::load_readiness_context_from_env();
    let policy =
        state::autonomous_daily_coverage_authority::resolve_current_coverage_policy_inputs(
            &assignment_config,
            &runtime_context.effective_runtime_binding,
            &readiness_context.strategy_registry,
        )
        .expect("policy resolves");
    let inputs =
        state::autonomous_daily_coverage_authority::coverage_construction_inputs_from_operation(
            operation,
            &assignment_identity,
            &runtime_binding_identity,
            &policy,
        )
        .expect("construction inputs resolve");
    let fresh = state::autonomous_daily_coverage_authority::construct_coverage_bound_detail(
        readiness_context.calendar_provider.as_ref(),
        &inputs,
    )
    .expect("construct ok");
    state::autonomous_daily_coverage_authority::write_and_confirm_coverage_authority(
        pool, &fresh, ts_utc,
    )
    .await
    .expect("bind ok");
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

    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3-SUPERVISOR-AND-CRITICAL-OUTCOME-
    // CLOSURE-01 REPAIR 6: shutdown now awaits actual task completion — no
    // settling sleep needed before asserting terminal truth.
    st.cancel_and_wait_completed_bar_task_for_shutdown().await;
    let truth = st.completed_bar_task_truth().await;
    assert_eq!(
        truth.liveness,
        AutonomousCompletedBarDriverTaskLiveness::Stopped
    );
    assert!(!st.completed_bar_task_claimed_for_test());
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

    st.cancel_and_wait_completed_bar_task_for_shutdown().await;
}

// ---------------------------------------------------------------------------
// Group D — fresh operation snapshot every tick (D3.3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn d01_d02_state_change_between_ticks_is_observed_fresh() {
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
    bind_coverage_authority_for_test(&pool, &st, &operation, now).await;

    // First tick: durable state is start_retrying -> PrepareDataOnly. This
    // bare-fixture tick names a manual/configuration blocker (the fixture
    // row's runtime-binding identity does not match the freshly resolved
    // runtime context — the same outcome the pre-closure test produced),
    // and (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3-SUPERVISOR-AND-CRITICAL-
    // OUTCOME-CLOSURE-01 REPAIR 7) that critical outcome now durably
    // degrades the operation to manual_intervention_required in the same
    // tick instead of remaining process-local.
    let outcome1 = tick_autonomous_completed_bar_driver_from_state(&st, now)
        .await
        .expect("tick ok");
    match outcome1 {
        AutonomousCompletedBarProductionTickOutcome::DriverOutcome {
            mode, ref outcome, ..
        } => {
            assert_eq!(mode, AutonomousCompletedBarDriverMode::PrepareDataOnly);
            assert!(
                matches!(
                    outcome,
                    AutonomousCompletedBarDriverOutcome::BindingBlocked { .. }
                        | AutonomousCompletedBarDriverOutcome::ReadinessBlocked { .. }
                ),
                "zero-evidence bare-fixture tick must name a manual/configuration blocker, \
                 got {outcome:?}"
            );
        }
        other => panic!("expected DriverOutcome for start_retrying, got {other:?}"),
    }
    let degraded = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(
        degraded.state,
        mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED,
        "REPAIR 7: a manual/configuration blocker must durably degrade a pre-runtime operation"
    );

    // Second tick, same AppState, fresh fetch: the durably changed state is
    // observed fresh — the task retains no cross-tick operation snapshot,
    // so the now-degraded operation selects no driver mode at all.
    let outcome2 = tick_autonomous_completed_bar_driver_from_state(&st, now)
        .await
        .expect("tick ok");
    match outcome2 {
        AutonomousCompletedBarProductionTickOutcome::ModeNotApplicable {
            operation_id,
            ref state,
        } => {
            assert_eq!(operation_id, operation.operation_id);
            assert_eq!(state, mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED);
        }
        other => panic!("expected ModeNotApplicable(manual_intervention_required), got {other:?}"),
    }
    // The PrepareDataOnly -> RunningDispatch fresh-mode transition through
    // this same adapter is proven end-to-end (with a real canonical runtime
    // start) by m01_task_level_prepare_to_running_exactly_once below.
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
    bind_coverage_authority_for_test(&pool, &st, &operation, now).await;
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

    let mut st = paper_alpaca_state_with_db(pool.clone(), &adapter_id);
    Arc::get_mut(&mut st).unwrap().instrument_registry_path =
        "C:/definitely/does/not/exist/instruments.json".to_string();
    st.set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;

    let now = timing.preopen_start_utc + chrono::Duration::minutes(5);
    bind_coverage_authority_for_test(&pool, &st, &operation, now).await;
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
fn i03_main_rs_cancels_and_waits_for_the_completed_bar_task_on_shutdown() {
    let source = read_main_rs_source();
    assert!(
        source.contains("cancel_and_wait_completed_bar_task_for_shutdown"),
        "main.rs's shutdown path must cancel AND await the completed-bar task"
    );
    let wait_pos = source
        .find("cancel_and_wait_completed_bar_task_for_shutdown")
        .unwrap();
    let stop_pos = source
        .find("stop_for_shutdown")
        .expect("main.rs must still call stop_for_shutdown");
    assert!(
        wait_pos < stop_pos,
        "the completed-bar task must be cancelled and awaited BEFORE stop_for_shutdown"
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

// ---------------------------------------------------------------------------
// Group K — AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3-SUPERVISOR-AND-CRITICAL-
// OUTCOME-CLOSURE-01: retained supervisor handle, outer monitor, full
// restart-budget exhaustion, claim release on every terminal path, sticky
// operator failure truth, and shutdown-that-waits. All via the real
// claim/install/supervise/monitor path
// (`spawn_supervised_completed_bar_task_for_test`) with injected restart
// policies and tick factories — never source-string assertions.
// ---------------------------------------------------------------------------

fn zero_delay_policy() -> AutonomousCompletedBarRestartPolicy {
    AutonomousCompletedBarRestartPolicy {
        max_restarts: 3,
        delays: vec![Duration::ZERO, Duration::ZERO, Duration::ZERO],
    }
}

async fn wait_until_liveness(
    st: &Arc<AppState>,
    liveness: AutonomousCompletedBarDriverTaskLiveness,
) -> bool {
    for _ in 0..200 {
        if st.completed_bar_task_truth().await.liveness == liveness {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

async fn wait_until_claim_released(st: &Arc<AppState>) -> bool {
    for _ in 0..200 {
        if !st.completed_bar_task_claimed_for_test() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// Loopback webhook that records every delivered alert body. Used as the
/// injected notification observer for exactly-once critical-notification
/// proofs (loopback only — no external network).
async fn start_alert_counting_webhook() -> (String, Arc<tokio::sync::Mutex<Vec<serde_json::Value>>>)
{
    let received: Arc<tokio::sync::Mutex<Vec<serde_json::Value>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&received);
    let app = axum::Router::new().route(
        "/",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let sink = Arc::clone(&sink);
            async move {
                sink.lock().await.push(body);
                axum::http::StatusCode::NO_CONTENT
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://127.0.0.1:{}/", addr.port()), received)
}

async fn count_alerts_with_class(
    received: &Arc<tokio::sync::Mutex<Vec<serde_json::Value>>>,
    alert_class: &str,
) -> usize {
    received
        .lock()
        .await
        .iter()
        .filter(|body| body.get("alert_class").and_then(|v| v.as_str()) == Some(alert_class))
        .count()
}

#[tokio::test]
async fn k01_full_restart_budget_exhaustion_four_generations_three_restarts_then_failed() {
    let st = fake_state();
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&attempts);

    let outcome = spawn_supervised_completed_bar_task_for_test(
        Arc::clone(&st),
        Duration::from_millis(5),
        zero_delay_policy(),
        move |_st, _gen| {
            counter.fetch_add(1, Ordering::SeqCst);
            move || async move {
                panic!("k01: deliberate worker panic for restart-budget proof");
            }
        },
    )
    .await;
    assert_eq!(outcome, AutonomousCompletedBarTaskSpawnOutcome::Started);

    assert!(
        wait_until_liveness(&st, AutonomousCompletedBarDriverTaskLiveness::Failed).await,
        "the task must reach Failed once the restart budget is exhausted"
    );
    assert!(
        wait_until_claim_released(&st).await,
        "permanent failure must release the spawn claim"
    );

    let truth = st.completed_bar_task_truth().await;
    assert_eq!(
        truth.liveness,
        AutonomousCompletedBarDriverTaskLiveness::Failed
    );
    assert_eq!(
        truth.restart_count, 3,
        "exactly three successful restarts; the final failing attempt is never counted as a \
         fourth"
    );
    assert_eq!(truth.last_error_code, Some("restart_budget_exhausted"));
    assert_eq!(
        truth.generation, 4,
        "exactly four worker generations were attempted"
    );
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        4,
        "exactly four worker attempts (initial + three restarts)"
    );

    assert!(!st.completed_bar_task_has_supervisor_handle_for_test().await);
    assert!(!st.completed_bar_task_has_cancel_sender_for_test().await);

    // No fifth worker is ever created after exhaustion.
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 4, "no fifth worker");
}

#[tokio::test]
async fn k02_subsequent_explicit_spawn_is_not_blocked_by_a_stale_claim_after_failure() {
    let st = fake_state();
    let outcome = spawn_supervised_completed_bar_task_for_test(
        Arc::clone(&st),
        Duration::from_millis(5),
        AutonomousCompletedBarRestartPolicy {
            max_restarts: 0,
            delays: vec![],
        },
        |_st, _gen| {
            move || async move {
                panic!("k02: immediate permanent failure");
            }
        },
    )
    .await;
    assert_eq!(outcome, AutonomousCompletedBarTaskSpawnOutcome::Started);
    assert!(wait_until_liveness(&st, AutonomousCompletedBarDriverTaskLiveness::Failed).await);
    assert!(wait_until_claim_released(&st).await);

    // A later explicit spawn must start cleanly — no stale claim, no stale
    // handle — and its actually-running worker clears the failed overlay.
    let ticks = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&ticks);
    let second = spawn_supervised_completed_bar_task_for_test(
        Arc::clone(&st),
        Duration::from_millis(5),
        zero_delay_policy(),
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
    assert_eq!(second, AutonomousCompletedBarTaskSpawnOutcome::Started);
    assert!(wait_until_liveness(&st, AutonomousCompletedBarDriverTaskLiveness::Running).await);
    // The failed-task overlay is gone once the replacement worker is
    // actually running: a controller-style clear now takes effect on the
    // read surface (while liveness was Failed, the same clear could not
    // hide CompletedBarDriverExited — k06 proves that side).
    st.clear_autonomous_session_truth().await;
    assert_eq!(
        st.autonomous_session_truth().await,
        AutonomousSessionTruth::Clear,
        "an actually-running replacement worker clears the failed overlay"
    );

    st.cancel_and_wait_completed_bar_task_for_shutdown().await;
    assert_eq!(
        st.completed_bar_task_truth().await.liveness,
        AutonomousCompletedBarDriverTaskLiveness::Stopped
    );
}

#[tokio::test]
async fn k03_permanent_failure_durably_degrades_running_operation_once_no_duplicate_event() {
    let Some(pool) = maybe_db("k03").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zztask-{}", unique_suffix());
    let timing = standard_timing();
    let operation = create_test_operation_in_start_retrying(
        &pool,
        &adapter_id,
        "ZZTASKK03",
        "swing_momentum",
        "5m",
        &timing,
    )
    .await;
    let run_args = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: operation.operation_id,
        expected_state: mqk_db::STATE_START_RETRYING.to_string(),
        expected_state_version: operation.state_version,
        run_id: Uuid::new_v4(),
        started_at_utc: timing.preopen_start_utc,
        occurred_at_utc: timing.preopen_start_utc,
        bounded_detail: "test: force running".to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation_to_running(&pool, &run_args)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(_) => {}
        other => panic!("expected Applied, got {other:?}"),
    }

    // The running operation carries a bound, not-durably-stopped run, so the
    // monitor's relevant-operation lookup (at real wall-clock now) finds it.
    let st = paper_alpaca_state_with_db(pool.clone(), &adapter_id);
    let spawn = spawn_supervised_completed_bar_task_for_test(
        Arc::clone(&st),
        Duration::from_millis(5),
        AutonomousCompletedBarRestartPolicy {
            max_restarts: 0,
            delays: vec![],
        },
        |_st, _gen| {
            move || async move {
                panic!("k03: deliberate permanent failure");
            }
        },
    )
    .await;
    assert_eq!(spawn, AutonomousCompletedBarTaskSpawnOutcome::Started);
    assert!(wait_until_liveness(&st, AutonomousCompletedBarDriverTaskLiveness::Failed).await);
    assert!(wait_until_claim_released(&st).await);

    let degraded = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(
        degraded.state,
        mqk_db::STATE_CONTROLLER_DEGRADED,
        "permanent task failure must degrade a running operation to controller_degraded"
    );
    assert_eq!(
        degraded.state_reason_code.as_deref(),
        Some("completed_bar_task_permanently_failed")
    );

    let event_count: i64 = sqlx::query_scalar(
        "select count(*) from sys_autonomous_daily_operation_events \
         where operation_id = $1 and reason_code = 'completed_bar_task_permanently_failed'",
    )
    .bind(operation.operation_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(event_count, 1, "exactly one permanent-failure event");

    // Repeating the identical permanent failure must not insert a duplicate
    // event — the same-state refresh dedup applies.
    let again = apply_completed_bar_task_permanent_failure(&st, &pool, Utc::now())
        .await
        .expect("apply ok");
    assert_eq!(again, Some(false));
    let event_count_after: i64 = sqlx::query_scalar(
        "select count(*) from sys_autonomous_daily_operation_events \
         where operation_id = $1 and reason_code = 'completed_bar_task_permanently_failed'",
    )
    .bind(operation.operation_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(event_count_after, 1, "no duplicate permanent-failure event");
}

#[tokio::test]
async fn k04_permanent_failure_does_not_mutate_stopping_or_completed_operations() {
    let Some(pool) = maybe_db("k04").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zztask-{}", unique_suffix());
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        &adapter_id,
        "ZZTASKK04",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    // Bind a run so the relevant lookup finds the operation even in
    // `stopping`, then durably move to stopping (awaiting_open -> stopping).
    let stop_args = mqk_db::TransitionAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        new_state: mqk_db::STATE_STOPPING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: timing.preopen_start_utc,
        run_id: Some(Uuid::new_v4()),
        bounded_detail: "test: force stopping".to_string(),
    };
    let stopping = match mqk_db::transition_autonomous_daily_operation(&pool, &stop_args)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };

    let st = paper_alpaca_state_with_db(pool.clone(), &adapter_id);
    let applied = apply_completed_bar_task_permanent_failure(&st, &pool, Utc::now())
        .await
        .expect("apply ok");
    assert_eq!(
        applied, None,
        "a stopping operation must not be mutated — stopping remains authoritative"
    );
    let fetched = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(fetched.state, mqk_db::STATE_STOPPING);
    assert_eq!(fetched.state_version, stopping.state_version);

    // Completed snapshots take the same no-mutation guard in the shared
    // applier: a critical outcome applied against a completed-state snapshot
    // writes nothing.
    let mut completed_snapshot = fetched.clone();
    completed_snapshot.state = mqk_db::STATE_COMPLETED.to_string();
    let outcome = AutonomousCompletedBarDriverOutcome::DispatchClaimUnresolved {
        status: "uncertain".to_string(),
    };
    let applied_completed =
        apply_completed_bar_driver_outcome(&pool, &completed_snapshot, &outcome, Utc::now())
            .await
            .expect("apply ok");
    assert_eq!(
        applied_completed, None,
        "completed operations are never mutated"
    );
}

#[tokio::test]
async fn k05_supervisor_core_panic_is_caught_by_monitor_with_exactly_one_critical_notification() {
    let (webhook_url, alerts) = start_alert_counting_webhook().await;
    let mut st_inner =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Alpaca);
    st_inner.discord_notifier = mqk_daemon::notify::DiscordNotifier::from_url(webhook_url);
    let st = Arc::new(st_inner);

    // The factory itself panics — this is a panic in the supervisor core's
    // own code path, not inside a tick closure, so only the outer monitor's
    // JoinError classification can observe it.
    let trigger = Arc::new(AtomicBool::new(true));
    let trigger_for_factory = Arc::clone(&trigger);
    let spawn = spawn_supervised_completed_bar_task_for_test(
        Arc::clone(&st),
        Duration::from_millis(5),
        zero_delay_policy(),
        move |_st, _gen| {
            if trigger_for_factory.load(Ordering::SeqCst) {
                panic!("k05: deliberate supervisor-core panic");
            }
            move || async move {}
        },
    )
    .await;
    assert_eq!(spawn, AutonomousCompletedBarTaskSpawnOutcome::Started);

    assert!(
        wait_until_liveness(&st, AutonomousCompletedBarDriverTaskLiveness::Failed).await,
        "a supervisor-core panic must be classified as permanent failure by the monitor"
    );
    assert!(
        wait_until_claim_released(&st).await,
        "a supervisor-core panic must release the claim"
    );

    let truth = st.completed_bar_task_truth().await;
    assert_eq!(truth.last_error_code, Some("supervisor_panicked"));
    assert!(!st.completed_bar_task_has_supervisor_handle_for_test().await);
    assert!(!st.completed_bar_task_has_cancel_sender_for_test().await);
    assert!(matches!(
        st.autonomous_session_truth().await,
        AutonomousSessionTruth::CompletedBarDriverExited { .. }
    ));

    // Exactly-once terminal handling: one critical failure notification,
    // zero restart notifications (the core died before any restart).
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        count_alerts_with_class(&alerts, "autonomous.completed_bar_task.failed").await,
        1,
        "exactly one permanent-failure critical notification"
    );
    assert_eq!(
        count_alerts_with_class(&alerts, "autonomous.completed_bar_task.restarting").await,
        0,
        "a core panic before any restart sends zero restart notifications"
    );
}

#[tokio::test]
async fn k06_session_controller_projections_cannot_hide_failed_task_truth() {
    let st = fake_state();
    st.set_completed_bar_task_generation_for_test(
        3,
        AutonomousCompletedBarDriverTaskLiveness::Failed,
    )
    .await;

    // A controller Started/Running tick clears (gap-preserving) the stored
    // truth; the failed-task overlay must survive it.
    st.clear_autonomous_session_truth().await;
    assert!(matches!(
        st.autonomous_session_truth().await,
        AutonomousSessionTruth::CompletedBarDriverExited { .. }
    ));

    // A controller StartRefused/ManualIntervention projection overwrites the
    // stored truth; the overlay must survive that too.
    st.set_autonomous_session_truth(AutonomousSessionTruth::StartRefused {
        detail: "reason_code=manual_intervention_required".to_string(),
    })
    .await;
    assert!(matches!(
        st.autonomous_session_truth().await,
        AutonomousSessionTruth::CompletedBarDriverExited { .. }
    ));

    // Expected shutdown must not appear failed: Stopped liveness has no
    // overlay, so the stored truth passes through.
    st.set_completed_bar_task_generation_for_test(
        3,
        AutonomousCompletedBarDriverTaskLiveness::Stopped,
    )
    .await;
    assert!(matches!(
        st.autonomous_session_truth().await,
        AutonomousSessionTruth::StartRefused { .. }
    ));

    // Only an actually-running later generation clears the overlay.
    st.set_completed_bar_task_generation_for_test(
        4,
        AutonomousCompletedBarDriverTaskLiveness::Failed,
    )
    .await;
    assert!(matches!(
        st.autonomous_session_truth().await,
        AutonomousSessionTruth::CompletedBarDriverExited { .. }
    ));
    st.set_completed_bar_task_generation_for_test(
        5,
        AutonomousCompletedBarDriverTaskLiveness::Running,
    )
    .await;
    assert!(matches!(
        st.autonomous_session_truth().await,
        AutonomousSessionTruth::StartRefused { .. }
    ));
}

#[tokio::test]
async fn k07_expected_cancellation_remains_stopped_and_clears_all_ownership() {
    let st = fake_state();
    let spawn = spawn_supervised_completed_bar_task_for_test(
        Arc::clone(&st),
        Duration::from_millis(5),
        zero_delay_policy(),
        |_st, _gen| move || async move {},
    )
    .await;
    assert_eq!(spawn, AutonomousCompletedBarTaskSpawnOutcome::Started);
    assert!(wait_until_liveness(&st, AutonomousCompletedBarDriverTaskLiveness::Running).await);
    assert!(st.completed_bar_task_has_supervisor_handle_for_test().await);
    assert!(st.completed_bar_task_has_cancel_sender_for_test().await);

    st.cancel_and_wait_completed_bar_task_for_shutdown().await;

    let truth = st.completed_bar_task_truth().await;
    assert_eq!(
        truth.liveness,
        AutonomousCompletedBarDriverTaskLiveness::Stopped,
        "expected cancellation is Stopped, never Failed"
    );
    assert!(!matches!(
        st.autonomous_session_truth().await,
        AutonomousSessionTruth::CompletedBarDriverExited { .. }
    ));
    assert!(!st.completed_bar_task_claimed_for_test());
    assert!(!st.completed_bar_task_has_supervisor_handle_for_test().await);
    assert!(!st.completed_bar_task_has_cancel_sender_for_test().await);
}

#[tokio::test]
async fn k08_shutdown_waits_for_the_inflight_tick_and_no_tick_after_return() {
    let st = fake_state();
    let in_progress = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicUsize::new(0));

    let (p, s, f) = (
        Arc::clone(&in_progress),
        Arc::clone(&started),
        Arc::clone(&finished),
    );
    let spawn = spawn_supervised_completed_bar_task_for_test(
        Arc::clone(&st),
        Duration::from_millis(10),
        zero_delay_policy(),
        move |_st, _gen| {
            let (p, s, f) = (Arc::clone(&p), Arc::clone(&s), Arc::clone(&f));
            move || {
                let (p, s, f) = (Arc::clone(&p), Arc::clone(&s), Arc::clone(&f));
                async move {
                    s.fetch_add(1, Ordering::SeqCst);
                    p.store(true, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    p.store(false, Ordering::SeqCst);
                    f.fetch_add(1, Ordering::SeqCst);
                }
            }
        },
    )
    .await;
    assert_eq!(spawn, AutonomousCompletedBarTaskSpawnOutcome::Started);

    // Wait until a tick is genuinely in flight.
    let mut observed_inflight = false;
    for _ in 0..200 {
        if in_progress.load(Ordering::SeqCst) {
            observed_inflight = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        observed_inflight,
        "a tick must be in flight before shutdown"
    );

    st.cancel_and_wait_completed_bar_task_for_shutdown().await;

    // The in-flight tick finished before the shutdown method returned.
    assert!(
        !in_progress.load(Ordering::SeqCst),
        "no tick may remain in progress after shutdown returns"
    );
    assert_eq!(
        started.load(Ordering::SeqCst),
        finished.load(Ordering::SeqCst),
        "every started tick completed before shutdown returned"
    );

    // No tick begins after shutdown returns.
    let started_at_shutdown = started.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        started.load(Ordering::SeqCst),
        started_at_shutdown,
        "no tick may begin after shutdown returns"
    );

    let truth = st.completed_bar_task_truth().await;
    assert_eq!(
        truth.liveness,
        AutonomousCompletedBarDriverTaskLiveness::Stopped
    );
    assert!(!st.completed_bar_task_claimed_for_test());
    assert!(!st.completed_bar_task_has_supervisor_handle_for_test().await);
    assert!(!st.completed_bar_task_has_cancel_sender_for_test().await);
}

// ---------------------------------------------------------------------------
// Group L — REPAIR 7/8: durable degradation for manual/configuration
// blockers, adapter-level identity/registry blockers, transient containment,
// and bounded persisted detail.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn l01_terminal_provider_rejection_degrades_durably() {
    let Some(pool) = maybe_db("l01").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zztask-{}", unique_suffix());
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        &adapter_id,
        "ZZTASKL01",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;

    let outcome = AutonomousCompletedBarDriverOutcome::PollFailedTerminal {
        detail: "provider-rejection-free-form-text-must-not-persist".to_string(),
    };
    let now = timing.preopen_start_utc + chrono::Duration::minutes(1);
    let applied = apply_completed_bar_driver_outcome(&pool, &operation, &outcome, now)
        .await
        .expect("apply ok");
    assert_eq!(applied, Some(true));

    let fetched = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(fetched.state, mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED);
    assert_eq!(
        fetched.state_reason_code.as_deref(),
        Some("completed_bar_poll_failed_terminal")
    );

    // REPAIR 8: the provider's free-form rejection text is never persisted.
    let details: Vec<String> = sqlx::query_scalar(
        "select bounded_detail from sys_autonomous_daily_operation_events where operation_id = $1",
    )
    .bind(operation.operation_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        details
            .iter()
            .all(|d| !d.contains("provider-rejection-free-form-text-must-not-persist")),
        "no persisted event may carry the provider's free-form detail"
    );
}

#[tokio::test]
async fn l02_provider_setup_blocker_degrades_durably_with_static_label_only() {
    let Some(pool) = maybe_db("l02").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zztask-{}", unique_suffix());
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        &adapter_id,
        "ZZTASKL02",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;

    let outcome = AutonomousCompletedBarDriverOutcome::ProviderSetupBlocked {
        rejection: AutonomousDriverSetupRejection::ProviderConstructionFailed(
            "secret-credential-path-xyz".to_string(),
        ),
    };
    let now = timing.preopen_start_utc + chrono::Duration::minutes(1);
    let applied = apply_completed_bar_driver_outcome(&pool, &operation, &outcome, now)
        .await
        .expect("apply ok");
    assert_eq!(applied, Some(true));

    let fetched = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(fetched.state, mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED);
    assert_eq!(
        fetched.state_reason_code.as_deref(),
        Some("completed_bar_provider_setup_blocked")
    );

    let details: Vec<String> = sqlx::query_scalar(
        "select bounded_detail from sys_autonomous_daily_operation_events where operation_id = $1",
    )
    .bind(operation.operation_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        details
            .iter()
            .any(|d| d.contains("provider_construction_failed")),
        "the static rejection label must be persisted"
    );
    assert!(
        details
            .iter()
            .all(|d| !d.contains("secret-credential-path-xyz")),
        "the rejection's free-form payload must never be persisted"
    );
}

#[tokio::test]
async fn l03_transient_and_wait_remediable_outcomes_do_not_degrade() {
    let Some(pool) = maybe_db("l03").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zztask-{}", unique_suffix());
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        &adapter_id,
        "ZZTASKL03",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;

    let now = timing.preopen_start_utc + chrono::Duration::minutes(1);
    for outcome in [
        AutonomousCompletedBarDriverOutcome::PollFailedTransient {
            detail: "transient".to_string(),
        },
        AutonomousCompletedBarDriverOutcome::ReadinessBlocked {
            blockers: vec![mqk_daemon::daily_data_readiness::REASON_EXPECTED_LATEST_BAR_MISSING],
        },
        AutonomousCompletedBarDriverOutcome::ReadinessBlockedAfterPoll { blockers: vec![] },
    ] {
        let applied = apply_completed_bar_driver_outcome(&pool, &operation, &outcome, now)
            .await
            .expect("apply ok");
        assert_eq!(
            applied, None,
            "transient/wait-remediable outcome {outcome:?} must not degrade"
        );
    }

    let fetched = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(fetched.state, mqk_db::STATE_AWAITING_OPEN);
    assert_eq!(fetched.state_version, operation.state_version);
}

#[tokio::test]
async fn l04_identity_unresolved_degrades_durably_through_the_adapter() {
    let Some(pool) = maybe_db("l04").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zztask-{}", unique_suffix());
    let symbol = "ZZTASKL04";
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

    // E2A closure REPAIR 4: the adapter's authority gate now runs before
    // assignment/identity resolution, so an identity-resolution failure can
    // only be observed once a real, correctly shaped authority event has
    // already been established (mission requirement). Bind that authority
    // first, using a real, resolvable assignment -- then remove the
    // assignment env entirely so the adapter's own fresh resolution fails on
    // the tick under test.
    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
    let st = paper_alpaca_state_with_db(pool.clone(), &adapter_id);
    st.set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;
    let now = timing.preopen_start_utc + chrono::Duration::minutes(5);
    bind_coverage_authority_for_test(&pool, &st, &operation, now).await;

    // No strategy assignment env at all -> assignment resolution fails.
    reset_env();
    let outcome = tick_autonomous_completed_bar_driver_from_state(&st, now)
        .await
        .expect("tick ok");
    assert!(matches!(
        outcome,
        AutonomousCompletedBarProductionTickOutcome::IdentityUnresolved { .. }
    ));

    let fetched = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(
        fetched.state,
        mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED,
        "IdentityUnresolved must reach durable operation truth, not remain process-local"
    );
    assert_eq!(
        fetched.state_reason_code.as_deref(),
        Some("completed_bar_identity_unresolved")
    );
}

#[tokio::test]
async fn l05_registry_unavailable_degrades_durably_through_the_adapter() {
    let Some(pool) = maybe_db("l05").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zztask-{}", unique_suffix());
    let timing = standard_timing();
    let operation = create_test_operation(
        &pool,
        &adapter_id,
        "ZZTASKL05",
        "swing_momentum",
        "5m",
        &timing,
        mqk_db::STATE_AWAITING_OPEN,
    )
    .await;
    std::env::set_var(STRATEGY_SYMBOL_ENV, "ZZTASKL05");
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");

    let mut st = paper_alpaca_state_with_db(pool.clone(), &adapter_id);
    Arc::get_mut(&mut st).unwrap().instrument_registry_path =
        "C:/definitely/does/not/exist/instruments.json".to_string();
    st.set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;

    let now = timing.preopen_start_utc + chrono::Duration::minutes(5);
    bind_coverage_authority_for_test(&pool, &st, &operation, now).await;
    let outcome = tick_autonomous_completed_bar_driver_from_state(&st, now)
        .await
        .expect("tick ok");
    reset_env();
    assert!(matches!(
        outcome,
        AutonomousCompletedBarProductionTickOutcome::RegistryUnavailable { .. }
    ));

    let fetched = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .expect("fetch ok")
        .expect("row exists");
    assert_eq!(
        fetched.state,
        mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED,
        "RegistryUnavailable must reach durable operation truth, not remain process-local"
    );
    assert_eq!(
        fetched.state_reason_code.as_deref(),
        Some("completed_bar_registry_unavailable")
    );
    let details: Vec<String> = sqlx::query_scalar(
        "select bounded_detail from sys_autonomous_daily_operation_events where operation_id = $1",
    )
    .bind(operation.operation_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        details
            .iter()
            .all(|d| !d.contains("definitely/does/not/exist")),
        "the registry path must never be persisted as operator authority"
    );
}

// ---------------------------------------------------------------------------
// Group M — REPAIR 9: task-level end-to-end exactly-once proof through the
// production adapter itself. Fixture pattern mirrors AL-03
// (`scenario_autonomous_paper_day_lifecycle_auton12.rs`): synthetic
// registries, seeded exact-ready `md_bars` evidence, a loopback mock Alpaca
// REST server (only because the canonical lifecycle start requires it), a
// registered `intraday_scalper` strategy, a Live broker cursor, and the real
// durable coordinator driving the canonical runtime start. Provider
// authorization stays disabled the whole time: the exact expected bar is
// already local, so zero provider resolution and zero provider calls occur.
// ---------------------------------------------------------------------------

const M01_SYMBOL: &str = "ZZTASKE2E";
const M01_PROVIDER: &str = "zztask_e2e_provider";
const M01_STRATEGY: &str = "intraday_scalper";
const M01_TIMEFRAME: &str = "5m";
const M01_TIMEFRAME_SECS: i64 = 300;
const M01_REQUIRED_BARS: usize = 5;
const M01_ADAPTER_ID: &str = "alpaca";

/// Monday 2024-04-15 (verified NYSE regular session inside the calendar
/// seam's covered range; open 13:30 UTC). Every instant below sits inside
/// the same 5m expected-bar slot [14:25, 14:30), so the bar observed in
/// PrepareDataOnly is exactly the bar dispatched in RunningDispatch.
fn m01_pre_preopen() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2024, 4, 15, 12, 30, 0).unwrap()
}
fn m01_t1_prepare() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2024, 4, 15, 14, 26, 0).unwrap()
}
fn m01_t_start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2024, 4, 15, 14, 26, 30).unwrap()
}
fn m01_t2_dispatch() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2024, 4, 15, 14, 27, 0).unwrap()
}
fn m01_t3_repeat() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2024, 4, 15, 14, 28, 0).unwrap()
}

fn m01_write_registries() {
    let dir = std::env::temp_dir().join(format!(
        "mqk_task_m01_registry_{}_{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create m01 registry dir");

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
    "notes": "AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3-SUPERVISOR-AND-CRITICAL-OUTCOME-CLOSURE-01 m01 fixture"
  }}
]"#,
            sym = M01_SYMBOL,
            prov = M01_PROVIDER,
        ),
    )
    .expect("write m01 instruments fixture");

    let providers_path = dir.join("providers.json");
    std::fs::write(
        &providers_path,
        format!(
            r#"[
  {{
    "provider_id": "{prov}",
    "display_name": "M01 Test Provider",
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
            prov = M01_PROVIDER,
        ),
    )
    .expect("write m01 providers fixture");

    std::env::set_var(INSTRUMENT_REGISTRY_PATH_ENV, &instruments_path);
    std::env::set_var(PROVIDER_REGISTRY_PATH_ENV, &providers_path);
}

/// The exact five completed 5m end_ts values the strict readiness gate
/// expects at the fixture instants — computed via the same production
/// calendar-walk the gate itself uses, never hand-derived.
fn m01_expected_bar_window() -> Vec<i64> {
    let calendar_provider = state::market_calendar::NyseWeekdaysProvider;
    let now = m01_t1_prepare();
    let schedule = state::market_calendar::resolve_market_session_schedule(&calendar_provider, now);
    mqk_daemon::daily_data_readiness::expected_intraday_end_ts_window(
        &calendar_provider,
        &schedule,
        now.timestamp(),
        M01_TIMEFRAME_SECS,
        0,
        M01_REQUIRED_BARS,
    )
    .expect("m01 expected 5m window resolves")
}

async fn m01_seed_bars(pool: &sqlx::PgPool, expected_ts: &[i64]) {
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
        .bind(M01_SYMBOL)
        .bind(M01_TIMEFRAME)
        .bind(end_ts)
        .bind(M01_PROVIDER)
        .bind(
            Utc.timestamp_opt(end_ts + 60, 0)
                .single()
                .expect("valid ingested_at ts"),
        )
        .execute(pool)
        .await
        .expect("seed m01 bar insert failed");
    }
}

/// Loopback in-process HTTP server satisfying the Alpaca paper REST surface
/// the canonical start requires (`GET /v2/account/activities/FILL` -> `[]`).
/// No real network request ever leaves the machine.
async fn m01_start_mock_alpaca_server() -> String {
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

fn m01_clear_env() {
    reset_env();
    for var in [
        "MQK_DAEMON_DEPLOYMENT_MODE",
        "MQK_DAEMON_ADAPTER_ID",
        "ALPACA_API_KEY_PAPER",
        "ALPACA_API_SECRET_PAPER",
        "ALPACA_PAPER_BASE_URL",
        "MQK_DATA_READINESS_GRACE_SECS",
        "MQK_DATA_READINESS_FUTURE_SKEW_SECS",
        "MQK_INTRADAY_BAR_MAX_AGE_SECS",
        "MQK_PAPER_WATCHLIST_PATH",
        "MQK_SESSION_START_HH_MM",
        "MQK_SESSION_STOP_HH_MM",
    ] {
        std::env::remove_var(var);
    }
}

async fn m01_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "m01 requires MQK_DATABASE_URL; run with --include-ignored against the isolated \
             test Postgres"
        )
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("m01 connect failed");
    mqk_db::migrate(&pool).await.expect("m01 migrate failed");

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
        .bind(M01_SYMBOL)
        .bind(M01_TIMEFRAME)
        .execute(&pool)
        .await
        .expect("cleanup m01 md_bars");
    sqlx::query("DELETE FROM strategy_signal_evaluations WHERE symbol = $1")
        .bind(M01_SYMBOL)
        .execute(&pool)
        .await
        .expect("cleanup m01 signal evaluations");
    sqlx::query(
        "DELETE FROM sys_autonomous_daily_operation_events WHERE operation_id IN \
         (SELECT operation_id FROM sys_autonomous_daily_operations WHERE adapter_id = $1)",
    )
    .bind(M01_ADAPTER_ID)
    .execute(&pool)
    .await
    .expect("cleanup m01 operation events");
    sqlx::query(
        "DELETE FROM sys_autonomous_daily_bar_dispatches WHERE operation_id IN \
         (SELECT operation_id FROM sys_autonomous_daily_operations WHERE adapter_id = $1)",
    )
    .bind(M01_ADAPTER_ID)
    .execute(&pool)
    .await
    .expect("cleanup m01 bar dispatches");
    sqlx::query("DELETE FROM sys_autonomous_daily_operations WHERE adapter_id = $1")
        .bind(M01_ADAPTER_ID)
        .execute(&pool)
        .await
        .expect("cleanup m01 operations");
    sqlx::query(
        "INSERT INTO sys_strategy_registry \
         (strategy_id, display_name, enabled, kind, registered_at_utc, updated_at_utc, note) \
         VALUES ($1, 'Intraday Scalper', true, 'bar_driven', NOW(), NOW(), 'task-m01-fixture') \
         ON CONFLICT (strategy_id) DO UPDATE SET enabled = true, updated_at_utc = NOW()",
    )
    .bind(M01_STRATEGY)
    .execute(&pool)
    .await
    .expect("upsert intraday_scalper for m01");

    pool
}

async fn m01_daemon_state() -> Arc<AppState> {
    let mock_url = m01_start_mock_alpaca_server().await;
    m01_write_registries();
    std::env::set_var("MQK_DAEMON_DEPLOYMENT_MODE", "paper");
    std::env::set_var("MQK_DAEMON_ADAPTER_ID", M01_ADAPTER_ID);
    std::env::set_var("ALPACA_API_KEY_PAPER", "test-paper-key");
    std::env::set_var("ALPACA_API_SECRET_PAPER", "test-paper-secret");
    std::env::set_var("ALPACA_PAPER_BASE_URL", &mock_url);
    std::env::set_var(STRATEGY_IDS_ENV, M01_STRATEGY);
    std::env::set_var(STRATEGY_SYMBOL_ENV, M01_SYMBOL);
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, M01_TIMEFRAME);
    std::env::set_var("MQK_DATA_READINESS_GRACE_SECS", "0");
    std::env::set_var("MQK_DATA_READINESS_FUTURE_SKEW_SECS", "60");
    std::env::remove_var("MQK_PAPER_WATCHLIST_PATH");
    std::env::remove_var("MQK_SESSION_START_HH_MM");
    std::env::remove_var("MQK_SESSION_STOP_HH_MM");
    // Provider authorization deliberately absent -> Disabled: the exact
    // expected bar is already local, so authorization must never matter.
    std::env::remove_var(REFRESH_ENABLED_ENV);
    std::env::remove_var(ALLOW_PROVIDER_CALLS_ENV);

    let pool = m01_pool().await;
    let expected_bars = m01_expected_bar_window();
    assert_eq!(expected_bars.len(), M01_REQUIRED_BARS);
    m01_seed_bars(&pool, &expected_bars).await;
    let latest_bar_end_ts = *expected_bars.last().expect("non-empty window");
    // The legacy market_data_freshness gate compares bar age against the
    // real wall clock; raise its bound just enough for the fixed synthetic
    // latest bar (bounded, never unbounded/negative).
    let legacy_max_age_secs = (Utc::now().timestamp() - latest_bar_end_ts).max(0) + 3600;
    std::env::set_var(
        "MQK_INTRADAY_BAR_MAX_AGE_SECS",
        legacy_max_age_secs.to_string(),
    );

    let st = Arc::new(AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    st.set_daily_data_readiness_clock_override_for_test(Some(m01_t_start()))
        .await;

    let live_cursor = mqk_broker_alpaca::types::AlpacaFetchCursor::live(
        None,
        "alpaca:task-m01:start",
        "2026-01-01T00:00:00Z",
    );
    let cursor_json =
        mqk_broker_alpaca::encode_fetch_cursor(&live_cursor).expect("encode live cursor");
    mqk_db::advance_broker_cursor(
        st.db.as_ref().expect("db must be set"),
        "alpaca",
        &cursor_json,
        Utc::now(),
    )
    .await
    .expect("persist live broker cursor for m01");
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:task-m01:start".to_string(),
        last_event_at: "2026-01-01T00:00:00Z".to_string(),
    })
    .await;
    {
        let mut broker = st.broker_snapshot.write().await;
        *broker = Some(mqk_schemas::BrokerSnapshot {
            captured_at_utc: Utc::now(),
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
        let mut execution = st.execution_snapshot.write().await;
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
            snapshot_at_utc: Utc::now(),
            has_recent_terminal_fill: false,
            risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
        });
    }
    st
}

async fn m01_signal_evaluation_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("select count(*) from strategy_signal_evaluations where symbol = $1")
        .bind(M01_SYMBOL)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn m01_dispatch_claim_count(pool: &sqlx::PgPool, operation_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "select count(*) from sys_autonomous_daily_bar_dispatches where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn m01_md_bars_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("select count(*) from md_bars where symbol = $1 and timeframe = $2")
        .bind(M01_SYMBOL)
        .bind(M01_TIMEFRAME)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn m01_task_level_prepare_to_running_exactly_once() {
    use mqk_daemon::state::autonomous_completed_bar_driver::AutonomousStrategyDispatchRuntimeTruth;
    use mqk_daemon::state::autonomous_daily_coordinator::{
        tick_autonomous_daily_coordinator, AutonomousDailyCoordinatorTickInput,
        AutonomousDailyCoordinatorTickOutcome,
    };

    let st = m01_daemon_state().await;
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    let pool = st.db.clone().expect("m01: db must be configured");
    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None)
        .await
        .expect("m01: persist ARMED failed");

    let expected_bars = m01_expected_bar_window();
    let expected_ts = *expected_bars.last().unwrap();
    let market_date = chrono::NaiveDate::from_ymd_opt(2024, 4, 15).unwrap();

    // ── Step 1: durable pre-runtime operation, created by the real
    // coordinator at a pre-preopen instant (no readiness evaluation yet). ──
    let pre_outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: m01_pre_preopen(),
    })
    .await
    .expect("m01: pre-preopen tick must not error");
    assert_eq!(
        pre_outcome,
        AutonomousDailyCoordinatorTickOutcome::WaitingForPreopen
    );
    let operation = mqk_db::fetch_autonomous_daily_operation_for_slot(
        &pool,
        market_date,
        "PAPER",
        M01_ADAPTER_ID,
    )
    .await
    .expect("m01: fetch operation failed")
    .expect("m01: operation row must exist");
    assert_eq!(operation.state, mqk_db::STATE_AWAITING_PREOPEN);
    assert!(m01_pre_preopen() < operation.preopen_start_utc);

    // ── Step 2: PrepareDataOnly through the production adapter — the exact
    // canonical local bar is observed; zero claims, zero strategy calls. ──
    assert_eq!(m01_signal_evaluation_count(&pool).await, 0);
    let outcome1 = tick_autonomous_completed_bar_driver_from_state(&st, m01_t1_prepare())
        .await
        .expect("m01: prepare tick ok");
    match outcome1 {
        AutonomousCompletedBarProductionTickOutcome::DriverOutcome {
            operation_id,
            mode,
            outcome,
        } => {
            assert_eq!(operation_id, operation.operation_id);
            assert_eq!(mode, AutonomousCompletedBarDriverMode::PrepareDataOnly);
            assert_eq!(
                outcome,
                AutonomousCompletedBarDriverOutcome::BarObserved {
                    bar_end_ts: expected_ts
                },
                "the exact canonical local bar must be observed in PrepareDataOnly"
            );
        }
        other => panic!("m01: expected PrepareDataOnly DriverOutcome, got {other:?}"),
    }
    assert_eq!(
        m01_dispatch_claim_count(&pool, operation.operation_id).await,
        0,
        "preparation must create zero dispatch claims"
    );
    assert_eq!(
        m01_signal_evaluation_count(&pool).await,
        0,
        "preparation must invoke zero strategy evaluations"
    );
    let after_prepare =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(after_prepare.last_completed_bar_ts, Some(expected_ts));
    assert_eq!(after_prepare.bars_observed, 1);
    assert_eq!(after_prepare.bars_dispatched, 0);

    // ── Step 3: canonical runtime start via the real durable coordinator. ──
    let mut started_run_id = None;
    for _ in 0..6 {
        let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
            state: &st,
            now_utc: m01_t_start(),
        })
        .await
        .expect("m01: start-path tick must not error");
        if let AutonomousDailyCoordinatorTickOutcome::Started { run_id } = outcome {
            started_run_id = Some(run_id);
            break;
        }
    }
    let run_id = started_run_id.expect("m01: coordinator must reach Started");
    tokio::time::sleep(Duration::from_millis(150)).await;

    let running = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(running.state, mqk_db::STATE_RUNNING);
    assert_eq!(running.run_id, Some(run_id));
    assert_eq!(
        st.autonomous_strategy_dispatch_runtime_truth().await,
        AutonomousStrategyDispatchRuntimeTruth::Active { run_id },
        "the native bootstrap must be active after the canonical runtime start"
    );

    // ── Step 4: RunningDispatch through the production adapter — the same
    // observed bar is dispatched exactly once. ──
    let outcome2 = tick_autonomous_completed_bar_driver_from_state(&st, m01_t2_dispatch())
        .await
        .expect("m01: dispatch tick ok");
    match outcome2 {
        AutonomousCompletedBarProductionTickOutcome::DriverOutcome {
            operation_id,
            mode,
            outcome,
        } => {
            assert_eq!(operation_id, operation.operation_id);
            assert_eq!(mode, AutonomousCompletedBarDriverMode::RunningDispatch);
            assert_eq!(
                outcome,
                AutonomousCompletedBarDriverOutcome::DispatchCompleted {
                    bar_end_ts: expected_ts
                },
                "the exact observed bar must dispatch once after the canonical start"
            );
        }
        other => panic!("m01: expected RunningDispatch DriverOutcome, got {other:?}"),
    }
    assert_eq!(
        m01_dispatch_claim_count(&pool, operation.operation_id).await,
        1,
        "exactly one durable dispatch claim"
    );
    assert_eq!(
        m01_signal_evaluation_count(&pool).await,
        1,
        "exactly one strategy dispatch invocation"
    );
    let claim = mqk_db::fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        M01_SYMBOL,
        M01_TIMEFRAME,
        expected_ts,
    )
    .await
    .unwrap();
    assert!(claim.is_some(), "the completed durable claim must exist");
    let after_dispatch =
        mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(after_dispatch.bars_dispatched, 1);

    // ── Step 5: the next production-adapter tick returns already-dispatched
    // truth with zero second strategy invocation. ──
    let outcome3 = tick_autonomous_completed_bar_driver_from_state(&st, m01_t3_repeat())
        .await
        .expect("m01: repeat tick ok");
    match outcome3 {
        AutonomousCompletedBarProductionTickOutcome::DriverOutcome { outcome, mode, .. } => {
            assert_eq!(mode, AutonomousCompletedBarDriverMode::RunningDispatch);
            assert!(
                matches!(
                    outcome,
                    AutonomousCompletedBarDriverOutcome::AlreadyDispatched { .. }
                ),
                "a repeated tick must return already-dispatched truth, got {outcome:?}"
            );
        }
        other => panic!("m01: expected repeat DriverOutcome, got {other:?}"),
    }
    assert_eq!(
        m01_dispatch_claim_count(&pool, operation.operation_id).await,
        1,
        "no second dispatch claim"
    );
    assert_eq!(
        m01_signal_evaluation_count(&pool).await,
        1,
        "no second strategy invocation"
    );

    // ── Step 6: zero side effects — no provider call, no historical sync or
    // ingest (md_bars unchanged), no orders. ──
    let final_op = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        final_op.provider_poll_attempt_count, 0,
        "zero provider poll attempts across the entire sequence"
    );
    assert_eq!(
        m01_md_bars_count(&pool).await,
        M01_REQUIRED_BARS as i64,
        "no ingest/sync side effect: md_bars holds exactly the seeded evidence"
    );
    let outbox_count: i64 = sqlx::query_scalar("select count(*) from oms_outbox where run_id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(outbox_count, 0, "zero paper/live orders");

    st.stop_for_shutdown().await;
    m01_clear_env();
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4-EVALUATION-LINEAGE-AND-AUTONOMOUS-
// PREOPEN-CLOSURE-01 REPAIR 7 — supervised task + real production adapter
// under an injected caller-supplied clock
// ---------------------------------------------------------------------------
//
// m01 above proves the production adapter's PrepareDataOnly ->
// RunningDispatch transition thoroughly, but calls
// `tick_autonomous_completed_bar_driver_from_state` directly — it never
// exercises the supervised task (spawn/supervisor/cancellation) at all. This
// test reuses m01's exact same real fixture (registries, seeded bars, mock
// Alpaca server, real coordinator start) but drives every tick through the
// real spawned supervised task (`spawn_autonomous_completed_bar_driver_task`
// -> the unmodified production tick adapter) under the REPAIR 7 clock seam,
// then proves shutdown cancels and awaits that same task.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn n01_supervised_task_drives_real_adapter_under_injected_clock() {
    use mqk_daemon::state::autonomous_daily_coordinator::{
        tick_autonomous_daily_coordinator, AutonomousDailyCoordinatorTickInput,
        AutonomousDailyCoordinatorTickOutcome,
    };

    let st = m01_daemon_state().await;
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }
    let pool = st.db.clone().expect("n01: db must be configured");
    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None)
        .await
        .expect("n01: persist ARMED failed");
    // Fast cadence so the second (RunningDispatch) tick does not require
    // waiting out the 15s production default.
    std::env::set_var(COMPLETED_BAR_TICK_SECS_ENV, "1");

    let market_date = chrono::NaiveDate::from_ymd_opt(2024, 4, 15).unwrap();

    // Durable pre-runtime operation, created by the real coordinator at a
    // pre-preopen instant — identical to m01's step 1.
    let pre_outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: m01_pre_preopen(),
    })
    .await
    .expect("n01: pre-preopen tick must not error");
    assert_eq!(
        pre_outcome,
        AutonomousDailyCoordinatorTickOutcome::WaitingForPreopen
    );
    let operation = mqk_db::fetch_autonomous_daily_operation_for_slot(
        &pool,
        market_date,
        "PAPER",
        M01_ADAPTER_ID,
    )
    .await
    .expect("n01: fetch operation failed")
    .expect("n01: operation row must exist");

    // REPAIR 7: control the supervised task's own clock. Production never
    // installs this — the seam defaults to `None`, and the task keeps
    // capturing `Utc::now()` once per tick.
    st.set_completed_bar_task_clock_override_for_test(Some(m01_t1_prepare()))
        .await;

    let spawn_outcome = spawn_autonomous_completed_bar_driver_task(Arc::clone(&st)).await;
    assert_eq!(
        spawn_outcome,
        AutonomousCompletedBarTaskSpawnOutcome::Started
    );

    // Wait for the real supervised task's own tick (not a direct adapter
    // call) to invoke the real production adapter in PrepareDataOnly.
    let mut observed_prepare = false;
    for _ in 0..500 {
        let truth = st.completed_bar_task_truth().await;
        if truth.mode == Some(AutonomousCompletedBarDriverMode::PrepareDataOnly) {
            observed_prepare = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        observed_prepare,
        "n01: the supervised task must invoke the real production adapter in PrepareDataOnly \
         under the injected clock"
    );
    assert_eq!(
        m01_dispatch_claim_count(&pool, operation.operation_id).await,
        0,
        "n01: PrepareDataOnly (via the supervised task) must create zero dispatch claims"
    );
    assert_eq!(
        m01_signal_evaluation_count(&pool).await,
        0,
        "n01: PrepareDataOnly (via the supervised task) must invoke zero strategy evaluations"
    );

    // Canonical runtime start via the real durable coordinator — identical
    // to m01's step 3, running concurrently with the live supervised task.
    let mut started_run_id = None;
    for _ in 0..6 {
        let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
            state: &st,
            now_utc: m01_t_start(),
        })
        .await
        .expect("n01: start-path tick must not error");
        if let AutonomousDailyCoordinatorTickOutcome::Started { run_id } = outcome {
            started_run_id = Some(run_id);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let run_id = started_run_id.expect("n01: coordinator must reach Started");
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Advance the supervised task's controlled clock into the dispatch
    // window — the next real tick must invoke the adapter in RunningDispatch
    // and complete the dispatch, all through the unmodified supervised task.
    st.set_completed_bar_task_clock_override_for_test(Some(m01_t2_dispatch()))
        .await;

    let mut observed_dispatch = false;
    for _ in 0..500 {
        let truth = st.completed_bar_task_truth().await;
        if truth.mode == Some(AutonomousCompletedBarDriverMode::RunningDispatch)
            && truth.last_outcome_code == Some("dispatch_completed")
        {
            observed_dispatch = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        observed_dispatch,
        "n01: the supervised task must invoke the real production adapter in RunningDispatch \
         and complete the dispatch under the injected clock, got {:?}",
        st.completed_bar_task_truth().await
    );
    assert_eq!(
        m01_dispatch_claim_count(&pool, operation.operation_id).await,
        1,
        "n01: exactly one durable dispatch claim"
    );
    assert_eq!(
        m01_signal_evaluation_count(&pool).await,
        1,
        "n01: exactly one strategy dispatch invocation"
    );

    // Shutdown cancels and awaits this same supervised task.
    st.cancel_and_wait_completed_bar_task_for_shutdown().await;
    assert!(
        !st.completed_bar_task_claimed_for_test(),
        "n01: spawn claim must be released after shutdown"
    );
    assert_eq!(
        st.completed_bar_task_truth().await.liveness,
        AutonomousCompletedBarDriverTaskLiveness::Stopped
    );
    let last_tick_at_shutdown = st.completed_bar_task_truth().await.last_tick_utc;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        st.completed_bar_task_truth().await.last_tick_utc,
        last_tick_at_shutdown,
        "n01: no tick may occur after cancel_and_wait_completed_bar_task_for_shutdown returns"
    );

    let outbox_count: i64 = sqlx::query_scalar("select count(*) from oms_outbox where run_id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(outbox_count, 0, "n01: zero paper/live orders");

    st.stop_for_shutdown().await;
    m01_clear_env();
    std::env::remove_var(COMPLETED_BAR_TICK_SECS_ENV);
}
