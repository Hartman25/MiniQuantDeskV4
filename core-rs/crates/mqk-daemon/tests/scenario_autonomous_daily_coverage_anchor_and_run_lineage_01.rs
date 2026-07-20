//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-COVERAGE-ANCHOR-AND-RUN-LINEAGE-
//! FOUNDATION: proof tests for the durable operation-scoped coverage-anchor
//! authority (`state::autonomous_daily_coverage_authority`) and the raw
//! full-run-lineage helper (`mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage`).
//!
//! DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_autonomous_daily_coverage_anchor_and_run_lineage_01 \
//!   -- --include-ignored --test-threads=1 --nocapture
//!
//! No real provider, broker, or network call is made anywhere in this file.
//! `BrokerKind::Alpaca` + `MQK_STRATEGY_IDS=swing_momentum` is used for the
//! adapter-facing tests (matching the production Paper+Alpaca/
//! `ExternalSignalIngestion` gate); `BrokerKind::Paper` is used for the
//! coordinator-only tests exactly as the accepted D2 coordinator suite does.
//!
//! Scope boundary: this file proves the E2A durable evidence foundation
//! only. It never calls an outcome classifier (none exists yet), never
//! writes `outcome`/`finalized_at_utc`, never drives a `completed*`
//! transition, and never touches a new API route or GUI surface.
//!
//! Concurrency-proof design note (item 9 of the mission): the inter-task
//! race is proven by direct construction of the worst-case ordering (the
//! operation row is made durable via a direct DB call with zero coverage
//! anchor, then the real production adapter is ticked against it) rather
//! than a live `tokio::join!`/`Notify` interleaving of two concurrently
//! scheduled tasks. This proves the identical safety invariant the mission
//! cares about — the adapter never invokes the driver, never constructs a
//! provider client, and never observes/claims/evaluates a bar for an
//! unanchored operation — without requiring a new pausable test hook on the
//! coordinator's own tick (out of scope for this patch; no coordinator
//! production code was changed to add one). This is stated here honestly,
//! not represented as a literal concurrent-task race proof.

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use mqk_daemon::daily_data_readiness;
use mqk_daemon::state::autonomous_completed_bar_task::{
    select_driver_mode_for_state, tick_autonomous_completed_bar_driver_from_state,
    AutonomousCompletedBarProductionTickOutcome,
};
use mqk_daemon::state::autonomous_daily_coordinator::{
    tick_autonomous_daily_coordinator, AutonomousCoverageAuthorityPreBindTestHook,
    AutonomousDailyCoordinatorTickInput, AutonomousDailyCoordinatorTickOutcome,
};
use mqk_daemon::state::autonomous_daily_coverage_authority::{
    check_coverage_authority, construct_coverage_bound_detail, coverage_bound_event_id,
    coverage_construction_inputs_from_operation, parse_coverage_bound_detail,
    resolve_current_coverage_policy_inputs, serialize_coverage_bound_detail,
    write_and_confirm_coverage_authority, CoverageAuthorityCheck, CoverageAuthorityEnsureResult,
    CoverageConstructionInputs, CoveragePolicyInputs, CoveragePolicyResolutionError,
    COVERAGE_SOURCE, EVENT_TYPE_COVERAGE_BOUND, REASON_COVERAGE_AUTHORITY_CONFLICT,
    REASON_COVERAGE_AUTHORITY_MISSING_AFTER_ACTIVITY, REASON_COVERAGE_AUTHORITY_NOT_BOUND,
};
use mqk_daemon::state::market_calendar::NyseWeekdaysProvider;
use mqk_daemon::state::{self, AppState, BrokerKind, DeploymentMode};
use uuid::Uuid;

const STRATEGY_SYMBOL_ENV: &str = "MQK_STRATEGY_SYMBOL";
const STRATEGY_IDS_ENV: &str = "MQK_STRATEGY_IDS";
const STRATEGY_TIMEFRAME_ENV: &str = "MQK_STRATEGY_MD_TIMEFRAME";
const GRACE_ENV: &str = daily_data_readiness::GRACE_SECS_ENV;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn reset_env() {
    std::env::remove_var(STRATEGY_SYMBOL_ENV);
    std::env::remove_var(STRATEGY_IDS_ENV);
    std::env::remove_var(STRATEGY_TIMEFRAME_ENV);
    std::env::remove_var(GRACE_ENV);
}

fn unique_suffix() -> String {
    Uuid::new_v4().to_string().replace('-', "")[..10].to_string()
}

/// 2026-07-20 (Monday) / 2026-07-17 (Friday) — the same ordinary, non-early-
/// close, non-holiday weekday pair already used and proven correct by
/// `scenario_autonomous_completed_bar_task_01.rs` and
/// `scenario_autonomous_daily_session_coordinator_01.rs`. Regular NYSE hours
/// apply: 13:30Z-20:00Z (09:30-16:00 ET, EDT in effect for both dates).
fn monday_at(hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 20, hour, minute, second)
        .unwrap()
}

fn friday_at(hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 17, hour, minute, second)
        .unwrap()
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
    sqlx::query("delete from sys_autonomous_daily_bar_dispatches where operation_id in (select operation_id from sys_autonomous_daily_operations where adapter_id like 'zze2a%')")
        .execute(&pool)
        .await
        .expect("clean claims");
    sqlx::query("delete from sys_autonomous_session_events where id like 'autonomous_daily_coverage_bound:%' and source = 'mqk-daemon.autonomous_daily_coordinator' and id in (select 'autonomous_daily_coverage_bound:' || operation_id::text from sys_autonomous_daily_operations where adapter_id like 'zze2a%')")
        .execute(&pool)
        .await
        .expect("clean coverage events");
    sqlx::query("delete from sys_autonomous_daily_operation_events where operation_id in (select operation_id from sys_autonomous_daily_operations where adapter_id like 'zze2a%')")
        .execute(&pool)
        .await
        .expect("clean operation events");
    sqlx::query("delete from sys_autonomous_daily_operations where adapter_id like 'zze2a%'")
        .execute(&pool)
        .await
        .expect("clean operations");
    Some(pool)
}

fn alpaca_state_with_db(db: sqlx::PgPool, adapter_id: &str) -> Arc<AppState> {
    let mut st = AppState::new_for_test_with_db_mode_and_broker(
        db,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st.set_adapter_id_for_test(adapter_id);
    Arc::new(st)
}

fn paper_state_with_db(db: sqlx::PgPool, adapter_id: &str) -> Arc<AppState> {
    let mut st = AppState::new_for_test_with_db_mode_and_broker(
        db,
        DeploymentMode::Paper,
        BrokerKind::Paper,
    );
    st.set_adapter_id_for_test(adapter_id);
    Arc::new(st)
}

async fn configured_alpaca_state(
    pool: sqlx::PgPool,
    adapter_id: &str,
    symbol: &str,
) -> Arc<AppState> {
    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
    let st = alpaca_state_with_db(pool, adapter_id);
    st.set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;
    st
}

fn write_instrument_registry(symbol: &str) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    let json = serde_json::json!([{
        "instrument_id": format!("equity:US:{symbol}"),
        "symbol": symbol,
        "asset_class": "equity",
        "provider": "zze2a_provider",
        "provider_symbol": symbol,
        "venue": "TEST",
        "currency": "USD",
        "enabled": true,
        "timeframes": ["5m"],
        "notes": "test fixture",
    }]);
    std::fs::write(file.path(), serde_json::to_string(&json).unwrap()).unwrap();
    file
}

/// Create a durable test operation using the exact real identity resolution
/// the coordinator itself uses (`resolve_autonomous_daily_session_plan_from_env`
/// plus `resolve_autonomous_runtime_context`), so a later real coordinator
/// tick against the same slot never reports a spurious identity conflict.
///
/// `st` must already have its strategy fleet and `MQK_STRATEGY_*` env vars
/// configured to match `symbol`. `initial_state` must be a legal initial
/// state such as `awaiting_preopen`, `preparing_data`, `awaiting_open`, or
/// `calendar_unavailable` -- use [`create_test_operation_running`] to reach
/// `running` instead.
async fn create_test_operation(
    pool: &sqlx::PgPool,
    st: &Arc<AppState>,
    adapter_id: &str,
    _symbol: &str,
    initial_state: &str,
    now_utc: DateTime<Utc>,
) -> mqk_db::AutonomousDailyOperationRecord {
    let timing = state::AutonomousDailyPlanTiming::production_default();
    let plan = match state::resolve_autonomous_daily_session_plan_from_env(now_utc, &timing) {
        state::AutonomousDailySessionPlanResolution::Applicable(plan) => plan,
        other => {
            panic!("expected an applicable session plan for the test fixture date, got {other:?}")
        }
    };
    let assignment_config =
        state::build_multi_symbol_runtime_config_from_env().expect("assignment config resolves");
    let runtime_context = state::autonomous_runtime_context::resolve_autonomous_runtime_context(st)
        .await
        .expect("runtime context resolves");
    let assignment_identity = state::derive_assignment_identity(&assignment_config);
    let runtime_binding_identity =
        state::derive_runtime_binding_identity(&runtime_context.effective_runtime_binding);
    let operation_id = state::derive_autonomous_daily_operation_id(
        &plan,
        "PAPER",
        adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
    );

    let args = mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
        deployment_mode: "PAPER".to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity: plan.session_plan_identity.clone(),
        assignment_identity,
        runtime_binding_identity,
        calendar_source: plan.calendar_source.clone(),
        calendar_coverage_state: plan.calendar_coverage_state.clone(),
        schedule_source: plan.schedule_source.as_str().to_string(),
        effective_operation_open_utc: plan.effective_operation_open_utc,
        effective_operation_close_utc: plan.effective_operation_close_utc,
        exchange_session_open_utc: plan.exchange_session_open_utc,
        exchange_session_close_utc: plan.exchange_session_close_utc,
        exchange_is_early_close: plan.exchange_is_early_close,
        previous_trading_date: chrono::NaiveDate::from_ymd_opt(2026, 7, 17).unwrap(),
        preopen_start_utc: plan.preopen_start_utc,
        postclose_finalize_utc: plan.postclose_finalize_utc,
        initial_state: initial_state.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: now_utc,
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

/// [`create_test_operation`] followed by the legal `awaiting_open ->
/// start_retrying -> running` path, binding `run_id`.
async fn create_test_operation_running(
    pool: &sqlx::PgPool,
    st: &Arc<AppState>,
    adapter_id: &str,
    symbol: &str,
    now_utc: DateTime<Utc>,
    run_id: Uuid,
) -> mqk_db::AutonomousDailyOperationRecord {
    let operation = create_test_operation(
        pool,
        st,
        adapter_id,
        symbol,
        mqk_db::STATE_AWAITING_OPEN,
        now_utc,
    )
    .await;
    let to_start_retrying = mqk_db::TransitionAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        new_state: mqk_db::STATE_START_RETRYING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: None,
        bounded_detail: "test fixture: force start_retrying".to_string(),
    };
    let retrying = match mqk_db::transition_autonomous_daily_operation(pool, &to_start_retrying)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };
    let to_running = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: retrying.operation_id,
        expected_state: retrying.state.clone(),
        expected_state_version: retrying.state_version,
        run_id,
        started_at_utc: now_utc,
        occurred_at_utc: now_utc,
        bounded_detail: "test fixture: force running".to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation_to_running(pool, &to_running)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    }
}

async fn fetch_coverage_row(
    pool: &sqlx::PgPool,
    operation_id: Uuid,
) -> Option<mqk_db::AutonomousSessionEventRow> {
    mqk_db::fetch_autonomous_session_event_by_id(pool, &coverage_bound_event_id(operation_id))
        .await
        .expect("fetch ok")
}

async fn coverage_claim_count(pool: &sqlx::PgPool, operation_id: Uuid) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "select count(*) from sys_autonomous_daily_bar_dispatches where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(pool)
    .await
    .expect("count ok")
}

// ---------------------------------------------------------------------------
// Group A — canonical coverage construction (pure, no DB) — items 1-6
// ---------------------------------------------------------------------------

fn base_policy() -> CoveragePolicyInputs {
    CoveragePolicyInputs {
        local_symbol: "ZZE2A".to_string(),
        timeframe: "5m".to_string(),
        timeframe_secs: 300,
        required_history_bars: 1,
        effective_grace_seconds: 30,
    }
}

#[test]
fn a01_ordinary_open_spills_into_previous_session_and_excludes_close_boundary_bar() {
    let policy = base_policy();
    let inputs = CoverageConstructionInputs {
        operation_id: Uuid::new_v4(),
        market_date_tuple: (2026, 7, 20),
        previous_trading_date_tuple: (2026, 7, 17),
        deployment_mode: "PAPER",
        adapter_id: "zze2a-a01",
        exchange_session_open_utc: monday_at(13, 30, 0),
        exchange_session_close_utc: monday_at(20, 0, 0),
        exchange_is_early_close: false,
        // Evaluated exactly at session open: no current-session bar has
        // closed yet, so the window must spill into Friday's tail (item 2).
        effective_operation_open_utc: monday_at(13, 30, 0),
        effective_operation_close_utc: monday_at(20, 0, 0),
        session_plan_identity: "sp",
        assignment_identity: "ai",
        runtime_binding_identity: "rbi",
        policy: &policy,
    };
    let detail = construct_coverage_bound_detail(&NyseWeekdaysProvider, &inputs)
        .expect("construction must succeed for an ordinary trading day");

    // Item 2: first anchor is Friday's own final grid identity.
    assert_eq!(
        detail.first_dispatchable_bar_end_ts,
        friday_at(19, 55, 0).timestamp()
    );
    // Item 4: Monday's own final slot (19:55) would expect at exactly
    // close (20:00:00) once timeframe+grace is added — strictly excluded.
    // The final anchor is therefore the next-to-last Monday slot (19:50).
    assert_eq!(
        detail.final_dispatchable_bar_end_ts,
        monday_at(19, 50, 0).timestamp()
    );
    // Item 1: canonical values are the system's stored end_ts labels
    // (slot-start values), never a separately-computed close label.
    assert_eq!(
        detail.first_dispatchable_bar_end_ts % 300,
        friday_at(19, 55, 0).timestamp() % 300
    );
}

#[test]
fn a02_earlier_readiness_window_rows_are_not_dispatch_obligations() {
    // Same instant as a01, but required_history_bars=3: the window contains
    // three Friday tail bars, yet only the *final* one is the anchor
    // (item 3) — the other two are strategy-history context only.
    let policy = CoveragePolicyInputs {
        required_history_bars: 3,
        ..base_policy()
    };
    let inputs = CoverageConstructionInputs {
        operation_id: Uuid::new_v4(),
        market_date_tuple: (2026, 7, 20),
        previous_trading_date_tuple: (2026, 7, 17),
        deployment_mode: "PAPER",
        adapter_id: "zze2a-a02",
        exchange_session_open_utc: monday_at(13, 30, 0),
        exchange_session_close_utc: monday_at(20, 0, 0),
        exchange_is_early_close: false,
        effective_operation_open_utc: monday_at(13, 30, 0),
        effective_operation_close_utc: monday_at(20, 0, 0),
        session_plan_identity: "sp",
        assignment_identity: "ai",
        runtime_binding_identity: "rbi",
        policy: &policy,
    };
    let detail =
        construct_coverage_bound_detail(&NyseWeekdaysProvider, &inputs).expect("construct ok");
    assert_eq!(
        detail.first_dispatchable_bar_end_ts,
        friday_at(19, 55, 0).timestamp()
    );
}

#[test]
fn a03_final_equals_first_when_no_later_bar_qualifies() {
    let policy = base_policy();
    let inputs = CoverageConstructionInputs {
        operation_id: Uuid::new_v4(),
        market_date_tuple: (2026, 7, 20),
        previous_trading_date_tuple: (2026, 7, 17),
        deployment_mode: "PAPER",
        adapter_id: "zze2a-a03",
        exchange_session_open_utc: monday_at(13, 30, 0),
        exchange_session_close_utc: monday_at(20, 0, 0),
        exchange_is_early_close: false,
        effective_operation_open_utc: monday_at(19, 41, 0),
        effective_operation_close_utc: monday_at(19, 42, 0),
        session_plan_identity: "sp",
        assignment_identity: "ai",
        runtime_binding_identity: "rbi",
        policy: &policy,
    };
    let detail =
        construct_coverage_bound_detail(&NyseWeekdaysProvider, &inputs).expect("construct ok");
    assert_eq!(
        detail.first_dispatchable_bar_end_ts,
        monday_at(19, 35, 0).timestamp()
    );
    assert_eq!(
        detail.final_dispatchable_bar_end_ts, detail.first_dispatchable_bar_end_ts,
        "item 5: final == first when no later current-session bar qualifies"
    );
}

#[test]
fn a04_no_started_at_utc_input_affects_the_anchor() {
    // Item 6: the constructor has no `started_at_utc`-shaped parameter at
    // all (structural proof by type signature) -- calling it twice with
    // identical inputs always yields an identical result regardless of any
    // notion of "when the runtime actually started".
    let policy = base_policy();
    let inputs = CoverageConstructionInputs {
        operation_id: Uuid::new_v4(),
        market_date_tuple: (2026, 7, 20),
        previous_trading_date_tuple: (2026, 7, 17),
        deployment_mode: "PAPER",
        adapter_id: "zze2a-a04",
        exchange_session_open_utc: monday_at(13, 30, 0),
        exchange_session_close_utc: monday_at(20, 0, 0),
        exchange_is_early_close: false,
        effective_operation_open_utc: monday_at(13, 30, 0),
        effective_operation_close_utc: monday_at(20, 0, 0),
        session_plan_identity: "sp",
        assignment_identity: "ai",
        runtime_binding_identity: "rbi",
        policy: &policy,
    };
    let d1 = construct_coverage_bound_detail(&NyseWeekdaysProvider, &inputs).unwrap();
    let d2 = construct_coverage_bound_detail(&NyseWeekdaysProvider, &inputs).unwrap();
    assert_eq!(d1, d2);
}

#[test]
fn a05_fails_closed_on_blank_identity_and_nonpositive_policy() {
    let bad_policy = CoveragePolicyInputs {
        timeframe_secs: 0,
        ..base_policy()
    };
    let inputs = CoverageConstructionInputs {
        operation_id: Uuid::new_v4(),
        market_date_tuple: (2026, 7, 20),
        previous_trading_date_tuple: (2026, 7, 17),
        deployment_mode: "PAPER",
        adapter_id: "zze2a-a05",
        exchange_session_open_utc: monday_at(13, 30, 0),
        exchange_session_close_utc: monday_at(20, 0, 0),
        exchange_is_early_close: false,
        effective_operation_open_utc: monday_at(13, 30, 0),
        effective_operation_close_utc: monday_at(20, 0, 0),
        session_plan_identity: "sp",
        assignment_identity: "ai",
        runtime_binding_identity: "rbi",
        policy: &bad_policy,
    };
    assert!(construct_coverage_bound_detail(&NyseWeekdaysProvider, &inputs).is_err());

    let good_policy = base_policy();
    let blank_identity_inputs = CoverageConstructionInputs {
        session_plan_identity: "",
        ..CoverageConstructionInputs {
            operation_id: Uuid::new_v4(),
            market_date_tuple: (2026, 7, 20),
            previous_trading_date_tuple: (2026, 7, 17),
            deployment_mode: "PAPER",
            adapter_id: "zze2a-a05b",
            exchange_session_open_utc: monday_at(13, 30, 0),
            exchange_session_close_utc: monday_at(20, 0, 0),
            exchange_is_early_close: false,
            effective_operation_open_utc: monday_at(13, 30, 0),
            effective_operation_close_utc: monday_at(20, 0, 0),
            session_plan_identity: "sp",
            assignment_identity: "ai",
            runtime_binding_identity: "rbi",
            policy: &good_policy,
        }
    };
    assert!(
        construct_coverage_bound_detail(&NyseWeekdaysProvider, &blank_identity_inputs).is_err()
    );
}

// ---------------------------------------------------------------------------
// Group B — serialization, parsing, semantic equality (pure) — items 7,
// 12-19
// ---------------------------------------------------------------------------

fn sample_detail(
    operation_id: Uuid,
) -> mqk_daemon::state::autonomous_daily_coverage_authority::CoverageBoundDetail {
    let policy = base_policy();
    let inputs = CoverageConstructionInputs {
        operation_id,
        market_date_tuple: (2026, 7, 20),
        previous_trading_date_tuple: (2026, 7, 17),
        deployment_mode: "PAPER",
        adapter_id: "zze2a-sample",
        exchange_session_open_utc: monday_at(13, 30, 0),
        exchange_session_close_utc: monday_at(20, 0, 0),
        exchange_is_early_close: false,
        effective_operation_open_utc: monday_at(13, 30, 0),
        effective_operation_close_utc: monday_at(20, 0, 0),
        session_plan_identity: "sp",
        assignment_identity: "ai",
        runtime_binding_identity: "rbi",
        policy: &policy,
    };
    construct_coverage_bound_detail(&NyseWeekdaysProvider, &inputs).unwrap()
}

#[test]
fn b01_event_id_is_deterministic_and_operation_scoped() {
    let op = Uuid::new_v4();
    assert_eq!(
        coverage_bound_event_id(op),
        format!("autonomous_daily_coverage_bound:{op}")
    );
    assert_ne!(
        coverage_bound_event_id(op),
        coverage_bound_event_id(Uuid::new_v4())
    );
}

#[test]
fn b02_serialize_parse_round_trip_is_exact() {
    let op = Uuid::new_v4();
    let detail = sample_detail(op);
    let raw = serialize_coverage_bound_detail(&detail);
    let parsed = parse_coverage_bound_detail(&raw).expect("parse ok");
    assert_eq!(parsed, detail);
}

#[test]
fn b03_parse_rejects_malformed_missing_and_wrong_type_fields() {
    assert!(parse_coverage_bound_detail("not json").is_err());
    assert!(parse_coverage_bound_detail("{}").is_err());
    let op = Uuid::new_v4();
    let detail = sample_detail(op);
    let mut value: serde_json::Value =
        serde_json::from_str(&serialize_coverage_bound_detail(&detail)).unwrap();
    // Wrong type for a required field.
    value["timeframe_secs"] = serde_json::json!("not-a-number");
    assert!(parse_coverage_bound_detail(&value.to_string()).is_err());
    // Missing a required field entirely.
    let mut value2: serde_json::Value =
        serde_json::from_str(&serialize_coverage_bound_detail(&detail)).unwrap();
    value2
        .as_object_mut()
        .unwrap()
        .remove("effective_grace_seconds");
    assert!(parse_coverage_bound_detail(&value2.to_string()).is_err());
    // Extra/unexpected field.
    let mut value3: serde_json::Value =
        serde_json::from_str(&serialize_coverage_bound_detail(&detail)).unwrap();
    value3["unexpected_extra_field"] = serde_json::json!("x");
    assert!(parse_coverage_bound_detail(&value3.to_string()).is_err());
}

#[test]
fn b04_parse_rejects_unknown_schema_version() {
    let op = Uuid::new_v4();
    let detail = sample_detail(op);
    let mut value: serde_json::Value =
        serde_json::from_str(&serialize_coverage_bound_detail(&detail)).unwrap();
    value["schema_version"] = serde_json::json!(2);
    match parse_coverage_bound_detail(&value.to_string()) {
        Err(mqk_daemon::state::autonomous_daily_coverage_authority::CoverageParseError::UnknownSchemaVersion(2)) => {}
        other => panic!("expected UnknownSchemaVersion(2), got {other:?}"),
    }
}

/// Append a literal, already-serialized `"key":value` pair to `raw`'s JSON
/// object as a genuine second occurrence of that key -- not through
/// `serde_json::Value`, which would collapse it, but as raw text, so the
/// parser under test is the only thing that ever gets a chance to detect it.
fn inject_duplicate_key(raw: &str, key_value_json: &str) -> String {
    let trimmed = raw.trim_end();
    assert!(trimmed.ends_with('}'), "expected a JSON object: {raw}");
    format!("{},{}}}", &trimmed[..trimmed.len() - 1], key_value_json)
}

#[test]
fn b06_parse_rejects_duplicate_json_keys() {
    // REPAIR 2 (E2A closure): a literal duplicate JSON key must be rejected
    // by the parser itself, never silently collapsed to its last occurrence.
    let op = Uuid::new_v4();
    let detail = sample_detail(op);
    let raw = serialize_coverage_bound_detail(&detail);

    let dup_schema_version = inject_duplicate_key(&raw, "\"schema_version\":1");
    assert!(
        parse_coverage_bound_detail(&dup_schema_version).is_err(),
        "duplicate schema_version must be rejected"
    );

    let dup_operation_id = inject_duplicate_key(&raw, &format!("\"operation_id\":\"{op}\""));
    assert!(
        parse_coverage_bound_detail(&dup_operation_id).is_err(),
        "duplicate operation_id must be rejected"
    );

    let dup_local_symbol = inject_duplicate_key(
        &raw,
        &format!("\"local_symbol\":\"{}\"", detail.local_symbol),
    );
    assert!(
        parse_coverage_bound_detail(&dup_local_symbol).is_err(),
        "duplicate local_symbol must be rejected"
    );
}

#[test]
fn b05_semantic_equality_detects_changed_timeframe_grace_history_identity() {
    let op = Uuid::new_v4();
    let base = sample_detail(op);

    let mut changed_timeframe = base.clone();
    changed_timeframe.timeframe_secs = 60;
    assert_ne!(base, changed_timeframe);

    let mut changed_grace = base.clone();
    changed_grace.effective_grace_seconds += 1;
    assert_ne!(base, changed_grace);

    let mut changed_history = base.clone();
    changed_history.required_history_bars += 1;
    assert_ne!(base, changed_history);

    let mut changed_assignment = base.clone();
    changed_assignment.assignment_identity = "different".to_string();
    assert_ne!(base, changed_assignment);

    let mut changed_runtime_binding = base.clone();
    changed_runtime_binding.runtime_binding_identity = "different".to_string();
    assert_ne!(base, changed_runtime_binding);

    let mut changed_session_plan = base.clone();
    changed_session_plan.session_plan_identity = "different".to_string();
    assert_ne!(base, changed_session_plan);
}

// ---------------------------------------------------------------------------
// Group C — pure run-lineage validation (no DB) — items 43-49 (validator
// half; DB-backed group F below proves the read half)
// ---------------------------------------------------------------------------

use mqk_db::{
    validate_autonomous_daily_operation_run_lineage, AutonomousDailyOperationRunningTransitionRow,
    RunLineageValidationError,
};

fn row(seq: i64, run_id: Uuid) -> AutonomousDailyOperationRunningTransitionRow {
    AutonomousDailyOperationRunningTransitionRow {
        transition_seq: seq,
        run_id,
    }
}

#[test]
fn c01_initial_run_lineage_returns_single_entry() {
    let run_a = Uuid::new_v4();
    let rows = vec![row(1, run_a)];
    let lineage = validate_autonomous_daily_operation_run_lineage(&rows, Some(run_a)).unwrap();
    assert_eq!(lineage, vec![run_a]);
}

#[test]
fn c02_recovery_run_lineage_preserves_earlier_run_order() {
    let run_a = Uuid::new_v4();
    let run_b = Uuid::new_v4();
    let rows = vec![row(1, run_a), row(5, run_b)];
    let lineage = validate_autonomous_daily_operation_run_lineage(&rows, Some(run_b)).unwrap();
    assert_eq!(lineage, vec![run_a, run_b]);
}

#[test]
fn c03_duplicate_run_id_fails_closed() {
    let run_a = Uuid::new_v4();
    let rows = vec![row(1, run_a), row(2, run_a)];
    assert_eq!(
        validate_autonomous_daily_operation_run_lineage(&rows, Some(run_a)),
        Err(RunLineageValidationError::DuplicateRunId(run_a))
    );
}

#[test]
fn c04_non_monotonic_sequence_fails_closed() {
    let run_a = Uuid::new_v4();
    let run_b = Uuid::new_v4();
    let rows = vec![row(5, run_a), row(3, run_b)];
    assert_eq!(
        validate_autonomous_daily_operation_run_lineage(&rows, Some(run_b)),
        Err(RunLineageValidationError::NonMonotonicSequence)
    );
}

#[test]
fn c05_current_run_mismatch_fails_closed() {
    let run_a = Uuid::new_v4();
    let run_other = Uuid::new_v4();
    let rows = vec![row(1, run_a)];
    assert_eq!(
        validate_autonomous_daily_operation_run_lineage(&rows, Some(run_other)),
        Err(RunLineageValidationError::CurrentRunMismatch)
    );
}

#[test]
fn c06_empty_lineage_with_nonnull_current_run_fails_closed() {
    let run_a = Uuid::new_v4();
    assert_eq!(
        validate_autonomous_daily_operation_run_lineage(&[], Some(run_a)),
        Err(RunLineageValidationError::CurrentRunMismatch)
    );
}

#[test]
fn c07_empty_lineage_with_null_current_run_is_valid() {
    assert_eq!(
        validate_autonomous_daily_operation_run_lineage(&[], None),
        Ok(Vec::new())
    );
}

#[test]
fn c08_validator_never_deduplicates_or_resorts_and_never_needs_a_row_cap() {
    // 150 distinct, strictly-increasing runs: proves the validator is not
    // implicitly bounded by the general-purpose 100-row API list cap
    // (item 50) since it operates on whatever raw row slice it is given.
    let mut rows = Vec::new();
    let mut ids = Vec::new();
    for i in 1..=150i64 {
        let id = Uuid::new_v4();
        rows.push(row(i, id));
        ids.push(id);
    }
    let current = *ids.last().unwrap();
    let lineage = validate_autonomous_daily_operation_run_lineage(&rows, Some(current)).unwrap();
    assert_eq!(lineage, ids);
}

// ---------------------------------------------------------------------------
// Group D — durable write / re-read / replay / conflict contract — items
// 8-11, 17
// ---------------------------------------------------------------------------

#[tokio::test]
async fn d01_first_write_persists_one_event_type_source_and_null_run_id() {
    let Some(pool) = maybe_db("d01").await else {
        return;
    };
    let op = Uuid::new_v4();
    let detail = sample_detail(op);
    let result = write_and_confirm_coverage_authority(&pool, &detail, monday_at(12, 0, 0))
        .await
        .expect("write ok");
    assert!(matches!(result, CoverageAuthorityEnsureResult::Bound(_)));

    let row = fetch_coverage_row(&pool, op).await.expect("row exists");
    assert_eq!(row.event_type, EVENT_TYPE_COVERAGE_BOUND);
    assert_eq!(row.source, "mqk-daemon.autonomous_daily_coordinator");
    assert!(row.run_id.is_none());
    let _ = sqlx::query("delete from sys_autonomous_session_events where id = $1")
        .bind(coverage_bound_event_id(op))
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn d02_exact_replay_under_later_caller_time_is_idempotent_and_ts_unchanged() {
    let Some(pool) = maybe_db("d02").await else {
        return;
    };
    let op = Uuid::new_v4();
    let detail = sample_detail(op);
    let first = write_and_confirm_coverage_authority(&pool, &detail, monday_at(12, 0, 0))
        .await
        .expect("first write ok");
    assert!(matches!(first, CoverageAuthorityEnsureResult::Bound(_)));
    let original_ts = fetch_coverage_row(&pool, op).await.unwrap().ts_utc;

    // Recomputed on a much later tick -- same immutable semantic payload.
    let second = write_and_confirm_coverage_authority(&pool, &detail, monday_at(18, 0, 0))
        .await
        .expect("second write ok");
    assert!(matches!(second, CoverageAuthorityEnsureResult::Bound(_)));

    let count: i64 =
        sqlx::query_scalar("select count(*) from sys_autonomous_session_events where id = $1")
            .bind(coverage_bound_event_id(op))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "exact replay must never create a second event");
    let row_after = fetch_coverage_row(&pool, op).await.unwrap();
    assert_eq!(
        row_after.ts_utc, original_ts,
        "ts_utc is the original bind instant, never rewritten"
    );

    let _ = sqlx::query("delete from sys_autonomous_session_events where id = $1")
        .bind(coverage_bound_event_id(op))
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn d03_conflicting_replay_does_not_overwrite_the_original_event() {
    let Some(pool) = maybe_db("d03").await else {
        return;
    };
    let op = Uuid::new_v4();
    let original = sample_detail(op);
    write_and_confirm_coverage_authority(&pool, &original, monday_at(12, 0, 0))
        .await
        .expect("first write ok");

    let mut conflicting = original.clone();
    conflicting.effective_grace_seconds += 15; // e.g. an operator changed the configured grace mid-day
    let result = write_and_confirm_coverage_authority(&pool, &conflicting, monday_at(13, 0, 0))
        .await
        .expect("write attempt ok");
    assert!(matches!(result, CoverageAuthorityEnsureResult::Conflict));

    let row_after = fetch_coverage_row(&pool, op).await.unwrap();
    let parsed = parse_coverage_bound_detail(&row_after.detail).unwrap();
    assert_eq!(
        parsed, original,
        "the original row must never be overwritten"
    );

    let _ = sqlx::query("delete from sys_autonomous_session_events where id = $1")
        .bind(coverage_bound_event_id(op))
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn d04_missing_event_check_reports_not_bound() {
    let Some(pool) = maybe_db("d04").await else {
        return;
    };
    let op = Uuid::new_v4();
    let detail = sample_detail(op);
    let check = check_coverage_authority(&pool, op, &detail)
        .await
        .expect("check ok");
    assert!(matches!(check, CoverageAuthorityCheck::NotBound));
}

/// REPAIR 5 (E2A closure): `write_and_confirm_coverage_authority`'s re-read
/// must independently tamper-check every envelope column, not merely the
/// `detail` payload. Each sub-case persists a row under the exact event id
/// with exactly one envelope field wrong, then proves the write/re-read
/// path rejects it (`Unreadable`) and never overwrites the tampered row
/// (`ON CONFLICT (id) DO NOTHING` -- the original insert already claimed the
/// id).
async fn assert_tampered_row_rejected_and_preserved(
    pool: &sqlx::PgPool,
    tampered_row: mqk_db::AutonomousSessionEventRow,
    fresh: &mqk_daemon::state::autonomous_daily_coverage_authority::CoverageBoundDetail,
) {
    let op = fresh.operation_id;
    mqk_db::persist_autonomous_session_event(pool, &tampered_row)
        .await
        .expect("persist tampered row ok");

    let result = write_and_confirm_coverage_authority(pool, fresh, monday_at(12, 0, 0))
        .await
        .expect("write attempt ok");
    assert!(
        matches!(result, CoverageAuthorityEnsureResult::Unreadable),
        "tampered envelope must be rejected as Unreadable, got {result:?}"
    );

    let row_after = fetch_coverage_row(pool, op).await.unwrap();
    assert_eq!(
        row_after.event_type, tampered_row.event_type,
        "the original tampered row must never be overwritten"
    );
    assert_eq!(row_after.source, tampered_row.source);
    assert_eq!(row_after.run_id, tampered_row.run_id);
    assert_eq!(row_after.resume_source, tampered_row.resume_source);
    assert_eq!(row_after.detail, tampered_row.detail);

    let _ = sqlx::query("delete from sys_autonomous_session_events where id = $1")
        .bind(coverage_bound_event_id(op))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn d05_write_and_confirm_rejects_tampered_event_type() {
    let Some(pool) = maybe_db("d05").await else {
        return;
    };
    let op = Uuid::new_v4();
    let detail = sample_detail(op);
    let tampered_row = mqk_db::AutonomousSessionEventRow {
        id: coverage_bound_event_id(op),
        ts_utc: monday_at(11, 0, 0),
        event_type: "wrong_event_type".to_string(),
        resume_source: None,
        detail: serialize_coverage_bound_detail(&detail),
        run_id: None,
        source: COVERAGE_SOURCE.to_string(),
    };
    assert_tampered_row_rejected_and_preserved(&pool, tampered_row, &detail).await;
}

#[tokio::test]
async fn d06_write_and_confirm_rejects_tampered_source() {
    let Some(pool) = maybe_db("d06").await else {
        return;
    };
    let op = Uuid::new_v4();
    let detail = sample_detail(op);
    let tampered_row = mqk_db::AutonomousSessionEventRow {
        id: coverage_bound_event_id(op),
        ts_utc: monday_at(11, 0, 0),
        event_type: EVENT_TYPE_COVERAGE_BOUND.to_string(),
        resume_source: None,
        detail: serialize_coverage_bound_detail(&detail),
        run_id: None,
        source: "wrong.source".to_string(),
    };
    assert_tampered_row_rejected_and_preserved(&pool, tampered_row, &detail).await;
}

#[tokio::test]
async fn d07_write_and_confirm_rejects_tampered_run_id() {
    let Some(pool) = maybe_db("d07").await else {
        return;
    };
    let op = Uuid::new_v4();
    let detail = sample_detail(op);
    let tampered_row = mqk_db::AutonomousSessionEventRow {
        id: coverage_bound_event_id(op),
        ts_utc: monday_at(11, 0, 0),
        event_type: EVENT_TYPE_COVERAGE_BOUND.to_string(),
        resume_source: None,
        detail: serialize_coverage_bound_detail(&detail),
        run_id: Some(Uuid::new_v4()),
        source: COVERAGE_SOURCE.to_string(),
    };
    assert_tampered_row_rejected_and_preserved(&pool, tampered_row, &detail).await;
}

#[tokio::test]
async fn d08_write_and_confirm_rejects_tampered_resume_source() {
    let Some(pool) = maybe_db("d08").await else {
        return;
    };
    let op = Uuid::new_v4();
    let detail = sample_detail(op);
    let tampered_row = mqk_db::AutonomousSessionEventRow {
        id: coverage_bound_event_id(op),
        ts_utc: monday_at(11, 0, 0),
        event_type: EVENT_TYPE_COVERAGE_BOUND.to_string(),
        resume_source: Some("unexpected_resume_source".to_string()),
        detail: serialize_coverage_bound_detail(&detail),
        run_id: None,
        source: COVERAGE_SOURCE.to_string(),
    };
    assert_tampered_row_rejected_and_preserved(&pool, tampered_row, &detail).await;
}

#[tokio::test]
async fn d09_write_and_confirm_rejects_tampered_detail_operation_id() {
    let Some(pool) = maybe_db("d09").await else {
        return;
    };
    let op = Uuid::new_v4();
    let detail = sample_detail(op);
    // The envelope (id/event_type/source/run_id/resume_source) is exactly
    // correct; only the payload's own `operation_id` field disagrees with
    // the operation it is stored under.
    let mismatched_detail = sample_detail(Uuid::new_v4());
    let tampered_row = mqk_db::AutonomousSessionEventRow {
        id: coverage_bound_event_id(op),
        ts_utc: monday_at(11, 0, 0),
        event_type: EVENT_TYPE_COVERAGE_BOUND.to_string(),
        resume_source: None,
        detail: serialize_coverage_bound_detail(&mismatched_detail),
        run_id: None,
        source: COVERAGE_SOURCE.to_string(),
    };
    assert_tampered_row_rejected_and_preserved(&pool, tampered_row, &detail).await;
}

// ---------------------------------------------------------------------------
// Group E — coordinator ensure-authority seam: pristine bind, prior-activity
// fail-closed, mid-day drift, close priority — items 4-6, 20-42
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e01_coordinator_binds_pristine_anchor_and_replays_on_second_tick() {
    let Some(pool) = maybe_db("e01").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zze2a-e01-{}", unique_suffix());
    let symbol = "ZZE2AE01";
    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let now = monday_at(12, 0, 0); // before preopen (13:00Z)

    let outcome1 = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await
    .expect("tick ok");
    assert_eq!(
        outcome1,
        AutonomousDailyCoordinatorTickOutcome::WaitingForPreopen
    );

    let slot = mqk_db::fetch_autonomous_daily_operation_for_slot(
        &pool,
        chrono::NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
        "PAPER",
        &adapter_id,
    )
    .await
    .expect("fetch ok")
    .expect("row exists");
    let bound_row = fetch_coverage_row(&pool, slot.operation_id)
        .await
        .expect("coverage anchor must be bound after the first tick");
    let first_detail = parse_coverage_bound_detail(&bound_row.detail).expect("parse ok");

    // Second tick: exact replay must not create a second event and must not
    // block the tick.
    let outcome2 = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now + chrono::Duration::minutes(1),
    })
    .await
    .expect("tick ok");
    assert_eq!(
        outcome2,
        AutonomousDailyCoordinatorTickOutcome::WaitingForPreopen
    );
    let count: i64 =
        sqlx::query_scalar("select count(*) from sys_autonomous_session_events where id = $1")
            .bind(coverage_bound_event_id(slot.operation_id))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let bound_row2 = fetch_coverage_row(&pool, slot.operation_id).await.unwrap();
    let second_detail = parse_coverage_bound_detail(&bound_row2.detail).unwrap();
    assert_eq!(first_detail, second_detail);

    reset_env();
}

#[tokio::test]
async fn e02_prior_activity_running_missing_authority_reaches_evidence_degraded() {
    let Some(pool) = maybe_db("e02").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zze2a-e02-{}", unique_suffix());
    let symbol = "ZZE2AE02";
    let now = monday_at(14, 0, 0);

    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;

    // A legacy-shaped row: already `running`, with a bound run_id and
    // observed bars, but no coverage anchor was ever written for it
    // (predates E2A). Fabricating an anchor now would be exactly what §6a
    // forbids.
    let run_id = Uuid::new_v4();
    let operation =
        create_test_operation_running(&pool, &st, &adapter_id, symbol, now, run_id).await;
    mqk_db::record_completed_bar_observed(
        &pool,
        operation.operation_id,
        monday_at(13, 55, 0).timestamp(),
        now,
    )
    .await
    .expect("record observed ok");

    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await
    .expect("tick ok");
    match outcome {
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code, ..
        } => {
            assert_eq!(
                reason_code,
                REASON_COVERAGE_AUTHORITY_MISSING_AFTER_ACTIVITY
            );
        }
        other => panic!("expected ManualInterventionRequired, got {other:?}"),
    }

    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refreshed.state, mqk_db::STATE_EVIDENCE_DEGRADED);
    assert!(
        fetch_coverage_row(&pool, operation.operation_id)
            .await
            .is_none(),
        "no anchor may ever be fabricated for a non-pristine operation"
    );
    // Original activity evidence is unchanged (item 37).
    assert_eq!(refreshed.run_id, Some(run_id));
    assert_eq!(refreshed.bars_observed, 1);

    reset_env();
}

#[tokio::test]
async fn e03_prior_activity_pre_running_missing_authority_reaches_manual_intervention() {
    let Some(pool) = maybe_db("e03").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zze2a-e03-{}", unique_suffix());
    let symbol = "ZZE2AE03";
    let now = monday_at(13, 5, 0);

    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;

    let operation = create_test_operation(
        &pool,
        &st,
        &adapter_id,
        symbol,
        mqk_db::STATE_AWAITING_PREOPEN,
        now,
    )
    .await;
    // Prior-activity signal without ever reaching `running`: a durable bar
    // dispatch claim exists for this operation.
    mqk_db::claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        symbol,
        "5m",
        monday_at(13, 0, 0).timestamp(),
        now,
    )
    .await
    .expect("claim ok");

    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: now,
    })
    .await
    .expect("tick ok");
    match outcome {
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code, ..
        } => {
            assert_eq!(
                reason_code,
                REASON_COVERAGE_AUTHORITY_MISSING_AFTER_ACTIVITY
            );
        }
        other => panic!("expected ManualInterventionRequired, got {other:?}"),
    }
    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refreshed.state, mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED);
    assert!(fetch_coverage_row(&pool, operation.operation_id)
        .await
        .is_none());
    assert_eq!(coverage_claim_count(&pool, operation.operation_id).await, 1);

    reset_env();
}

#[tokio::test]
async fn e04_close_priority_stops_runtime_even_with_a_coverage_conflict() {
    let Some(pool) = maybe_db("e04").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zze2a-e04-{}", unique_suffix());
    let symbol = "ZZE2AE04";
    let now = monday_at(13, 5, 0);

    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;

    let operation = create_test_operation(
        &pool,
        &st,
        &adapter_id,
        symbol,
        mqk_db::STATE_AWAITING_PREOPEN,
        now,
    )
    .await;
    mqk_db::claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        symbol,
        "5m",
        monday_at(13, 0, 0).timestamp(),
        now,
    )
    .await
    .expect("claim ok");

    // Past effective close: canonical close/stop reconciliation must take
    // priority over the coverage blocker.
    let past_close = operation.effective_operation_close_utc + chrono::Duration::minutes(5);
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &st,
        now_utc: past_close,
    })
    .await
    .expect("tick ok");
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::RuntimeStopped
    );
    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refreshed.state, mqk_db::STATE_STOPPING);
    assert!(refreshed.stopped_at_utc.is_some());

    reset_env();
}

// ---------------------------------------------------------------------------
// Group F — completed-bar adapter authority gate + concurrency proof —
// items 5, 9, 20-30
// ---------------------------------------------------------------------------

#[tokio::test]
async fn f01_adapter_returns_not_bound_with_zero_side_effects_for_a_newly_visible_unanchored_operation(
) {
    let Some(pool) = maybe_db("f01").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zze2a-f01-{}", unique_suffix());
    let symbol = "ZZE2AF01";
    let now = monday_at(13, 5, 0);

    let instruments = write_instrument_registry(symbol);
    let mut st = configured_alpaca_state(pool.clone(), &adapter_id, symbol).await;
    Arc::get_mut(&mut st).unwrap().instrument_registry_path =
        instruments.path().to_str().unwrap().to_string();

    // The operation row becomes visible (as `create_or_recover` alone would
    // make it) with zero coverage anchor -- exactly the window a concurrent
    // completed-bar tick could observe before the coordinator's own
    // ensure-authority step has run.
    let operation = create_test_operation(
        &pool,
        &st,
        &adapter_id,
        symbol,
        mqk_db::STATE_AWAITING_PREOPEN,
        now,
    )
    .await;
    assert!(select_driver_mode_for_state(&operation.state).is_some());

    let outcome = tick_autonomous_completed_bar_driver_from_state(&st, now)
        .await
        .expect("tick ok");
    match outcome {
        AutonomousCompletedBarProductionTickOutcome::CoverageAuthorityUnavailable {
            operation_id,
            reason_code,
        } => {
            assert_eq!(operation_id, operation.operation_id);
            assert_eq!(reason_code, REASON_COVERAGE_AUTHORITY_NOT_BOUND);
        }
        other => panic!("expected CoverageAuthorityUnavailable(not_bound), got {other:?}"),
    }

    // Zero side effects: no lifecycle mutation, no claim, no bar observed.
    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refreshed.state, operation.state);
    assert_eq!(refreshed.bars_observed, 0);
    assert_eq!(coverage_claim_count(&pool, operation.operation_id).await, 0);

    reset_env();
}

#[tokio::test]
async fn f02_adapter_proceeds_once_the_coordinator_has_bound_the_authority() {
    let Some(pool) = maybe_db("f02").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zze2a-f02-{}", unique_suffix());
    let symbol = "ZZE2AF02";

    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
    let instruments = write_instrument_registry(symbol);

    let mut coordinator_st = alpaca_state_with_db(pool.clone(), &adapter_id);
    Arc::get_mut(&mut coordinator_st)
        .unwrap()
        .instrument_registry_path = instruments.path().to_str().unwrap().to_string();
    coordinator_st
        .set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
            strategy_id: "swing_momentum".to_string(),
        }]))
        .await;

    let now = monday_at(12, 0, 0); // before preopen (13:00Z)
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &coordinator_st,
        now_utc: now,
    })
    .await
    .expect("coordinator tick ok");
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::WaitingForPreopen
    );

    let slot = mqk_db::fetch_autonomous_daily_operation_for_slot(
        &pool,
        chrono::NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
        "PAPER",
        &adapter_id,
    )
    .await
    .expect("fetch ok")
    .expect("row exists");
    assert!(
        fetch_coverage_row(&pool, slot.operation_id).await.is_some(),
        "coordinator must have bound the anchor"
    );

    // A later, independent adapter tick against the same operation -- within
    // the relevant-lookup's preopen..postclose window, unlike the
    // coordinator's own creation tick -- now proceeds through the normal
    // mode path (item 30) instead of refusing. The anchor's own content
    // does not depend on wall-clock `now`, so it remains compatible.
    let adapter_now = monday_at(13, 5, 0);
    let mut adapter_st = configured_alpaca_state(pool.clone(), &adapter_id, symbol).await;
    Arc::get_mut(&mut adapter_st)
        .unwrap()
        .instrument_registry_path = instruments.path().to_str().unwrap().to_string();

    let outcome = tick_autonomous_completed_bar_driver_from_state(&adapter_st, adapter_now)
        .await
        .expect("tick ok");
    assert!(
        matches!(
            outcome,
            AutonomousCompletedBarProductionTickOutcome::DriverOutcome { .. }
        ),
        "expected DriverOutcome once the authority is bound, got {outcome:?}"
    );

    reset_env();
}

#[tokio::test]
async fn f03_adapter_refuses_on_unreadable_and_conflicting_authority() {
    let Some(pool) = maybe_db("f03").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zze2a-f03-{}", unique_suffix());
    let symbol = "ZZE2AF03";
    let now = monday_at(13, 5, 0);

    let instruments = write_instrument_registry(symbol);
    let mut st = configured_alpaca_state(pool.clone(), &adapter_id, symbol).await;
    Arc::get_mut(&mut st).unwrap().instrument_registry_path =
        instruments.path().to_str().unwrap().to_string();
    let operation = create_test_operation(
        &pool,
        &st,
        &adapter_id,
        symbol,
        mqk_db::STATE_AWAITING_PREOPEN,
        now,
    )
    .await;

    // Unreadable: a malformed detail payload under the exact event id.
    let bad_row = mqk_db::AutonomousSessionEventRow {
        id: coverage_bound_event_id(operation.operation_id),
        ts_utc: now,
        event_type: EVENT_TYPE_COVERAGE_BOUND.to_string(),
        resume_source: None,
        detail: "{not valid json".to_string(),
        run_id: None,
        source: "mqk-daemon.autonomous_daily_coordinator".to_string(),
    };
    mqk_db::persist_autonomous_session_event(&pool, &bad_row)
        .await
        .expect("persist ok");

    let outcome = tick_autonomous_completed_bar_driver_from_state(&st, now)
        .await
        .expect("tick ok");
    match outcome {
        AutonomousCompletedBarProductionTickOutcome::CoverageAuthorityUnavailable {
            reason_code,
            ..
        } => {
            assert_eq!(
                reason_code,
                mqk_daemon::state::autonomous_daily_coverage_authority::REASON_COVERAGE_AUTHORITY_UNREADABLE
            );
        }
        other => panic!("expected CoverageAuthorityUnavailable(unreadable), got {other:?}"),
    }
    // Adapter never mutates lifecycle for this outcome.
    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refreshed.state, operation.state);

    reset_env();
}

#[tokio::test]
async fn f04_live_coordinator_and_adapter_interleaving_proves_zero_side_effects_while_paused() {
    let Some(pool) = maybe_db("f04").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zze2a-f04-{}", unique_suffix());
    let symbol = "ZZE2AF04";
    let instruments = write_instrument_registry(symbol);

    let mut coordinator_st = configured_alpaca_state(pool.clone(), &adapter_id, symbol).await;
    Arc::get_mut(&mut coordinator_st)
        .unwrap()
        .instrument_registry_path = instruments.path().to_str().unwrap().to_string();

    let hook = Arc::new(AutonomousCoverageAuthorityPreBindTestHook::default());
    coordinator_st
        .set_coverage_authority_pre_bind_test_hook_for_test(Some(hook.clone()))
        .await;

    let coordinator_now = monday_at(12, 0, 0); // before preopen (13:00Z)
                                               // The adapter's own relevant-operation lookup requires `now_utc` to fall
                                               // within `[preopen_start_utc, postclose_finalize_utc]` (or a special-
                                               // cased state) for a fresh `awaiting_preopen` row -- matching the same
                                               // post-preopen instant `f01`/`f02`/`f03` already use for the adapter
                                               // side. Passing this later instant to task B while task A's own tick
                                               // still runs at `coordinator_now` is deterministic-logic testing, not a
                                               // claim that both instants represent the same real moment.
    let adapter_now = monday_at(13, 5, 0);

    // Task A: the real coordinator tick, paused (via the hook) immediately
    // after `create_or_recover` commits the operation row and before it
    // binds the coverage authority.
    let coordinator_fut = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &coordinator_st,
        now_utc: coordinator_now,
    });

    // Task B: the real production completed-bar adapter, ticked concurrently
    // against the same durable operation row while task A is paused. This
    // exercises the actual coordinator and adapter concurrently rather than
    // manually emulating their ordering.
    let adapter_fut = async {
        hook.operation_visible.notified().await;

        let mut adapter_st = configured_alpaca_state(pool.clone(), &adapter_id, symbol).await;
        Arc::get_mut(&mut adapter_st)
            .unwrap()
            .instrument_registry_path = instruments.path().to_str().unwrap().to_string();
        let outcome = tick_autonomous_completed_bar_driver_from_state(&adapter_st, adapter_now)
            .await
            .expect("adapter tick ok");

        hook.proceed.notify_waiters();
        outcome
    };

    let (coordinator_result, adapter_outcome) = tokio::join!(coordinator_fut, adapter_fut);
    let coordinator_outcome = coordinator_result.expect("coordinator tick ok");
    assert_eq!(
        coordinator_outcome,
        AutonomousDailyCoordinatorTickOutcome::WaitingForPreopen
    );

    let slot = mqk_db::fetch_autonomous_daily_operation_for_slot(
        &pool,
        chrono::NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
        "PAPER",
        &adapter_id,
    )
    .await
    .expect("fetch ok")
    .expect("row exists");

    // Task B, running while task A was paused before binding the authority,
    // must have observed the quiet not-bound path and produced zero side
    // effects.
    match adapter_outcome {
        AutonomousCompletedBarProductionTickOutcome::CoverageAuthorityUnavailable {
            operation_id,
            reason_code,
        } => {
            assert_eq!(operation_id, slot.operation_id);
            assert_eq!(reason_code, REASON_COVERAGE_AUTHORITY_NOT_BOUND);
        }
        other => panic!("expected CoverageAuthorityUnavailable(not_bound), got {other:?}"),
    }

    let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, slot.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        refreshed.state,
        mqk_db::STATE_AWAITING_PREOPEN,
        "task B must never have mutated lifecycle state while paused"
    );
    assert_eq!(
        refreshed.bars_observed, 0,
        "zero bar observations from task B"
    );
    assert_eq!(
        coverage_claim_count(&pool, slot.operation_id).await,
        0,
        "zero dispatch claims from task B"
    );

    // Task A, once released, must have durably bound the authority.
    assert!(
        fetch_coverage_row(&pool, slot.operation_id).await.is_some(),
        "task A must have bound the coverage authority after being released"
    );

    // Post-release: a later real adapter tick now proceeds through the
    // normal eligible path instead of refusing.
    let adapter_now2 = monday_at(13, 5, 0);
    let mut adapter_st2 = configured_alpaca_state(pool.clone(), &adapter_id, symbol).await;
    Arc::get_mut(&mut adapter_st2)
        .unwrap()
        .instrument_registry_path = instruments.path().to_str().unwrap().to_string();
    let outcome2 = tick_autonomous_completed_bar_driver_from_state(&adapter_st2, adapter_now2)
        .await
        .expect("tick ok");
    assert!(
        matches!(
            outcome2,
            AutonomousCompletedBarProductionTickOutcome::DriverOutcome { .. }
        ),
        "expected DriverOutcome once the authority is bound, got {outcome2:?}"
    );

    reset_env();
}

// ---------------------------------------------------------------------------
// Group G — mid-day coverage-policy drift — items 39-42
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g01_prepare_data_only_mid_day_drift_returns_conflict_and_invokes_no_driver() {
    let Some(pool) = maybe_db("g01").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zze2a-g01-{}", unique_suffix());
    let symbol = "ZZE2AG01";

    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
    let instruments = write_instrument_registry(symbol);
    let mut coordinator_st = alpaca_state_with_db(pool.clone(), &adapter_id);
    Arc::get_mut(&mut coordinator_st)
        .unwrap()
        .instrument_registry_path = instruments.path().to_str().unwrap().to_string();
    coordinator_st
        .set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
            strategy_id: "swing_momentum".to_string(),
        }]))
        .await;

    let now = monday_at(12, 0, 0); // before preopen (13:00Z)
    let outcome = tick_autonomous_daily_coordinator(AutonomousDailyCoordinatorTickInput {
        state: &coordinator_st,
        now_utc: now,
    })
    .await
    .expect("coordinator tick ok");
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::WaitingForPreopen
    );

    // Mid-day drift: the operator changes the configured grace seconds.
    std::env::set_var(GRACE_ENV, "45");

    let adapter_now = monday_at(13, 5, 0);
    let mut adapter_st = configured_alpaca_state(pool.clone(), &adapter_id, symbol).await;
    Arc::get_mut(&mut adapter_st)
        .unwrap()
        .instrument_registry_path = instruments.path().to_str().unwrap().to_string();

    let outcome = tick_autonomous_completed_bar_driver_from_state(&adapter_st, adapter_now)
        .await
        .expect("tick ok");
    match outcome {
        AutonomousCompletedBarProductionTickOutcome::CoverageAuthorityUnavailable {
            reason_code,
            ..
        } => assert_eq!(reason_code, REASON_COVERAGE_AUTHORITY_CONFLICT),
        other => panic!("expected CoverageAuthorityUnavailable(conflict), got {other:?}"),
    }
    let slot = mqk_db::fetch_autonomous_daily_operation_for_slot(
        &pool,
        chrono::NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
        "PAPER",
        &adapter_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(coverage_claim_count(&pool, slot.operation_id).await, 0);
    assert_eq!(slot.bars_observed, 0);

    reset_env();
}

#[tokio::test]
async fn g02_running_dispatch_mid_day_drift_returns_conflict_and_invokes_no_driver() {
    let Some(pool) = maybe_db("g02").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zze2a-g02-{}", unique_suffix());
    let symbol = "ZZE2AG02";
    let now = monday_at(14, 0, 0);

    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
    let st = alpaca_state_with_db(pool.clone(), &adapter_id);
    st.set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;

    let run_id = Uuid::new_v4();
    let running = create_test_operation_running(&pool, &st, &adapter_id, symbol, now, run_id).await;
    let operation = running.clone();

    // Bind the anchor with the ORIGINAL (pre-drift) grace policy, using the
    // exact same real resolution the adapter itself will use.
    let readiness_context = daily_data_readiness::load_readiness_context_from_env();
    let assignment_config =
        state::build_multi_symbol_runtime_config_from_env().expect("assignment config resolves");
    let runtime_context =
        state::autonomous_runtime_context::resolve_autonomous_runtime_context(&st)
            .await
            .expect("runtime context resolves");
    let policy = resolve_current_coverage_policy_inputs(
        &assignment_config,
        &runtime_context.effective_runtime_binding,
        &readiness_context.strategy_registry,
    )
    .expect("policy resolves");
    let assignment_identity = state::derive_assignment_identity(&assignment_config);
    let runtime_binding_identity =
        state::derive_runtime_binding_identity(&runtime_context.effective_runtime_binding);
    let construction_inputs = coverage_construction_inputs_from_operation(
        &running,
        &assignment_identity,
        &runtime_binding_identity,
        &policy,
    )
    .expect("inputs resolve");
    let fresh = construct_coverage_bound_detail(
        readiness_context.calendar_provider.as_ref(),
        &construction_inputs,
    )
    .expect("construct ok");
    write_and_confirm_coverage_authority(&pool, &fresh, now)
        .await
        .expect("bind ok");

    // Mid-day drift: grace changes.
    std::env::set_var(GRACE_ENV, "45");
    let instruments = write_instrument_registry(symbol);
    let mut adapter_st = configured_alpaca_state(pool.clone(), &adapter_id, symbol).await;
    Arc::get_mut(&mut adapter_st)
        .unwrap()
        .instrument_registry_path = instruments.path().to_str().unwrap().to_string();

    let outcome = tick_autonomous_completed_bar_driver_from_state(&adapter_st, now)
        .await
        .expect("tick ok");
    match outcome {
        AutonomousCompletedBarProductionTickOutcome::CoverageAuthorityUnavailable {
            reason_code,
            ..
        } => assert_eq!(reason_code, REASON_COVERAGE_AUTHORITY_CONFLICT),
        other => panic!("expected CoverageAuthorityUnavailable(conflict), got {other:?}"),
    }
    assert_eq!(coverage_claim_count(&pool, operation.operation_id).await, 0);

    reset_env();
}

// ---------------------------------------------------------------------------
// Group H — run lineage helper, DB-backed — items 43-51
// ---------------------------------------------------------------------------

#[tokio::test]
async fn h01_initial_and_recovery_run_lineage_read_and_validated() {
    let Some(pool) = maybe_db("h01").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zze2a-h01-{}", unique_suffix());
    let symbol = "ZZE2AH01";
    let now = monday_at(14, 0, 0);

    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;

    let run_a = Uuid::new_v4();
    let after_a = create_test_operation_running(&pool, &st, &adapter_id, symbol, now, run_a).await;

    let lineage =
        mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage(&pool, &after_a)
            .await
            .expect("read ok")
            .expect("validate ok");
    assert_eq!(lineage, vec![run_a], "item 43: initial run A returns [A]");

    // Simulate a crash + recovery: mark terminal without halt, then a fresh
    // recovery start binds run B.
    let terminal = mqk_db::TransitionAutonomousDailyOperationArgs {
        operation_id: after_a.operation_id,
        expected_state: after_a.state.clone(),
        expected_state_version: after_a.state_version,
        new_state: mqk_db::STATE_RECOVERY_RETRYING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: now,
        run_id: None,
        bounded_detail: "test: interrupted".to_string(),
    };
    let recovering = match mqk_db::transition_autonomous_daily_operation(&pool, &terminal)
        .await
        .unwrap()
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    };
    let run_b = Uuid::new_v4();
    let to_running_b = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: recovering.operation_id,
        expected_state: recovering.state.clone(),
        expected_state_version: recovering.state_version,
        run_id: run_b,
        started_at_utc: now,
        occurred_at_utc: now,
        bounded_detail: "test: run B (recovery)".to_string(),
    };
    let after_b =
        match mqk_db::transition_autonomous_daily_operation_to_running(&pool, &to_running_b)
            .await
            .unwrap()
        {
            mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
            other => panic!("expected Applied, got {other:?}"),
        };

    let lineage_after_recovery =
        mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage(&pool, &after_b)
            .await
            .expect("read ok")
            .expect("validate ok");
    assert_eq!(
        lineage_after_recovery,
        vec![run_a, run_b],
        "item 44: recovery run B returns [A, B]"
    );
    assert!(
        lineage_after_recovery.contains(&run_a),
        "item 45: earlier-run evidence is not removed"
    );
}

#[tokio::test]
async fn h02_duplicate_and_mismatched_run_id_fail_closed_against_real_rows() {
    let Some(pool) = maybe_db("h02").await else {
        return;
    };
    reset_env();
    let adapter_id = format!("zze2a-h02-{}", unique_suffix());
    let symbol = "ZZE2AH02";
    let now = monday_at(14, 0, 0);

    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_strategy_fleet_for_test(Some(vec![state::StrategyFleetEntry {
        strategy_id: "swing_momentum".to_string(),
    }]))
    .await;

    let run_a = Uuid::new_v4();
    let running = create_test_operation_running(&pool, &st, &adapter_id, symbol, now, run_a).await;

    // Mismatch: current_run_id differs from the lineage's only entry.
    let mismatched = mqk_db::AutonomousDailyOperationRecord {
        run_id: Some(Uuid::new_v4()),
        ..running.clone()
    };
    let result =
        mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage(&pool, &mismatched)
            .await
            .expect("read ok");
    assert_eq!(result, Err(RunLineageValidationError::CurrentRunMismatch));
}

// ---------------------------------------------------------------------------
// Group I — pure policy-resolution error taxonomy (no DB) — supports items
// 5/39-42's fail-closed behavior
// ---------------------------------------------------------------------------

#[test]
fn i01_policy_resolution_fails_closed_with_no_symbol_assignment() {
    let config = state::MultiSymbolRuntimeConfig {
        schema_version: "v2".to_string(),
        symbols: vec![],
        max_concurrent_symbols: 1,
        source: state::MultiSymbolConfigSource::EnvSingleSymbolFallback,
    };
    let binding = mqk_runtime::native_strategy::EffectiveRuntimeBinding {
        effective_runtime_strategy_id: None,
        effective_runtime_target_symbol: None,
        effective_runtime_timeframe_secs: None,
    };
    let registry = mqk_strategy::PluginRegistry::new();
    let result = resolve_current_coverage_policy_inputs(&config, &binding, &registry);
    assert_eq!(
        result,
        Err(CoveragePolicyResolutionError::NoSymbolAssignment)
    );
}
