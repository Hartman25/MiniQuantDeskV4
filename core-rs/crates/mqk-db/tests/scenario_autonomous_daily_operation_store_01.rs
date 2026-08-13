//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-DURABLE-COORDINATION: mqk-db store
//! proof for `sys_autonomous_daily_operations` /
//! `sys_autonomous_daily_operation_events`.
//!
//! Proves: schema/constraints, race-safe create-or-recover (including
//! same-day identity-conflict handling), CAS transitions (including atomic
//! rollback on a forced event-insert failure), and bounded read-only
//! functions.
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-db --test scenario_autonomous_daily_operation_store_01 \
//!   -- --include-ignored --test-threads=1 --nocapture

use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use mqk_db::{
    create_or_recover_autonomous_daily_operation, fetch_autonomous_daily_operation_by_id,
    fetch_autonomous_daily_operation_for_slot, list_autonomous_daily_operation_events,
    list_recent_autonomous_daily_operations, transition_autonomous_daily_operation,
    AutonomousDailyOperationRecord, AutonomousDailyTransitionOutcome,
    CreateAutonomousDailyOperationArgs, CreateOrRecoverAutonomousDailyOperationOutcome,
    TransitionAutonomousDailyOperationArgs, ENV_DB_URL, STATE_AWAITING_PREOPEN,
    STATE_CALENDAR_UNAVAILABLE, STATE_COMPLETED, STATE_MANUAL_INTERVENTION_REQUIRED,
    STATE_PREFLIGHT_BLOCKED, STATE_PREPARING_DATA, STATE_RUNNING, STATE_START_RETRYING,
    STATE_STOPPING,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn test_pool() -> anyhow::Result<sqlx::PgPool> {
    if std::env::var(ENV_DB_URL).is_err() {
        anyhow::bail!("SKIP: requires MQK_DATABASE_URL");
    }
    mqk_db::testkit_db_pool().await
}

fn unique_suffix() -> String {
    Uuid::new_v4().to_string().replace('-', "")[..10].to_string()
}

fn test_operation_id(seed: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, seed.as_bytes())
}

fn session_bounds(
    market_date: NaiveDate,
) -> (DateTime<Utc>, DateTime<Utc>, DateTime<Utc>, DateTime<Utc>) {
    let open = Utc
        .with_ymd_and_hms(
            market_date.year(),
            market_date.month(),
            market_date.day(),
            13,
            30,
            0,
        )
        .unwrap();
    let close = open + ChronoDuration::hours(6) + ChronoDuration::minutes(30);
    let preopen = open - ChronoDuration::minutes(30);
    let postclose = close + ChronoDuration::minutes(15);
    (open, close, preopen, postclose)
}

#[allow(clippy::too_many_arguments)]
fn make_create_args(
    market_date: NaiveDate,
    deployment_mode: &str,
    adapter_id: &str,
    session_plan_identity: &str,
    assignment_identity: &str,
    runtime_binding_identity: &str,
    occurred_at_utc: DateTime<Utc>,
) -> CreateAutonomousDailyOperationArgs {
    let (open, close, preopen, postclose) = session_bounds(market_date);
    let previous_trading_date = market_date - ChronoDuration::days(3);
    let operation_id = test_operation_id(&format!(
        "{market_date}|{deployment_mode}|{adapter_id}|{session_plan_identity}|{assignment_identity}|{runtime_binding_identity}"
    ));
    CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date,
        deployment_mode: deployment_mode.to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity: session_plan_identity.to_string(),
        assignment_identity: assignment_identity.to_string(),
        runtime_binding_identity: runtime_binding_identity.to_string(),
        calendar_source: "nyse_weekdays_heuristic".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: "nyse_weekdays_heuristic".to_string(),
        effective_operation_open_utc: open,
        effective_operation_close_utc: close,
        exchange_session_open_utc: open,
        exchange_session_close_utc: close,
        exchange_is_early_close: false,
        previous_trading_date,
        preopen_start_utc: preopen,
        postclose_finalize_utc: postclose,
        initial_state: STATE_AWAITING_PREOPEN.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc,
        bounded_detail: "scenario test creation".to_string(),
        stop_attempt_count: 0,
    }
}

/// Like [`make_create_args`], but with an effective operation window that
/// differs from the exchange boundaries (simulating a valid fixed-window
/// override) — proves the store persists the two fact sets distinctly.
#[allow(clippy::too_many_arguments)]
fn make_create_args_with_override_shaped_boundaries(
    market_date: NaiveDate,
    deployment_mode: &str,
    adapter_id: &str,
    session_plan_identity: &str,
    assignment_identity: &str,
    runtime_binding_identity: &str,
    occurred_at_utc: DateTime<Utc>,
) -> CreateAutonomousDailyOperationArgs {
    let mut args = make_create_args(
        market_date,
        deployment_mode,
        adapter_id,
        session_plan_identity,
        assignment_identity,
        runtime_binding_identity,
        occurred_at_utc,
    );
    // Narrow the effective operation window relative to the exchange
    // session, matching what a valid fixed-window override would produce.
    args.effective_operation_open_utc = args.exchange_session_open_utc + ChronoDuration::hours(1);
    args.effective_operation_close_utc = args.exchange_session_close_utc - ChronoDuration::hours(1);
    args
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

fn unwrap_created(
    outcome: CreateOrRecoverAutonomousDailyOperationOutcome,
) -> AutonomousDailyOperationRecord {
    match outcome {
        CreateOrRecoverAutonomousDailyOperationOutcome::Created(r) => r,
        other => panic!("expected Created, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Schema / constraint tests
// ---------------------------------------------------------------------------

/// Both tables exist with their required primary keys / unique constraints.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn schema_tables_and_constraints_exist() -> anyhow::Result<()> {
    let pool = test_pool().await?;

    let ops_exists: bool = sqlx::query_scalar(
        "select exists (select 1 from information_schema.tables where table_name = 'sys_autonomous_daily_operations')",
    )
    .fetch_one(&pool)
    .await?;
    assert!(ops_exists, "sys_autonomous_daily_operations must exist");

    let events_exists: bool = sqlx::query_scalar(
        "select exists (select 1 from information_schema.tables where table_name = 'sys_autonomous_daily_operation_events')",
    )
    .fetch_one(&pool)
    .await?;
    assert!(
        events_exists,
        "sys_autonomous_daily_operation_events must exist"
    );

    let slot_unique_exists: bool = sqlx::query_scalar(
        "select exists (select 1 from pg_constraint where conname = 'sys_autonomous_daily_operations_daily_slot_unique')",
    )
    .fetch_one(&pool)
    .await?;
    assert!(
        slot_unique_exists,
        "daily-slot unique constraint must exist"
    );

    Ok(())
}

/// Migration 0049 is registered in the manifest exactly once, immediately
/// after 0048, and the new exchange-truth columns/constraints it adds exist.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn migration_0049_registered_and_exchange_columns_exist() -> anyhow::Result<()> {
    let manifest_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("migrations")
        .join("manifest.json");
    let manifest_raw = std::fs::read_to_string(&manifest_path).expect("read manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest_raw).expect("parse manifest.json");
    let ids: Vec<String> = manifest["migrations"]
        .as_array()
        .expect("migrations array")
        .iter()
        .map(|m| m["id"].as_str().expect("id").to_string())
        .collect();
    let idx_0048 = ids
        .iter()
        .position(|id| id == "0048")
        .expect("0048 must be registered");
    let idx_0049 = ids
        .iter()
        .position(|id| id == "0049")
        .expect("0049 must be registered");
    assert_eq!(idx_0049, idx_0048 + 1, "0049 must immediately follow 0048");
    assert_eq!(
        ids.iter().filter(|id| id.as_str() == "0049").count(),
        1,
        "0049 must be registered exactly once"
    );

    let pool = test_pool().await?;
    for column in [
        "exchange_session_open_utc",
        "exchange_session_close_utc",
        "exchange_is_early_close",
        "previous_trading_date",
    ] {
        let exists: bool = sqlx::query_scalar(
            "select exists (select 1 from information_schema.columns \
             where table_name = 'sys_autonomous_daily_operations' and column_name = $1)",
        )
        .bind(column)
        .fetch_one(&pool)
        .await?;
        assert!(exists, "column {column} must exist");
    }

    let coherency_check_exists: bool = sqlx::query_scalar(
        "select exists (select 1 from pg_constraint where conname = 'sys_autonomous_daily_operations_exchange_fields_together')",
    )
    .fetch_one(&pool)
    .await?;
    assert!(
        coherency_check_exists,
        "exchange-fields-together coherency constraint must exist"
    );

    Ok(())
}

/// Explicit zero for every bookkeeping counter inserts successfully at
/// creation (the correction forbids a DB-level default supplying zero, not
/// the value zero itself).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn explicit_zero_counters_insert_successfully() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 2).unwrap(); // Monday
    let adapter = format!("adapter_zero_{}", unique_suffix());
    let now = Utc::now();
    let args = make_create_args(
        market_date,
        "paper",
        &adapter,
        "splan-zero",
        "asgn-zero",
        "bind-zero",
        now,
    );

    let outcome = create_or_recover_autonomous_daily_operation(&pool, &args).await?;
    let record = unwrap_created(outcome);
    assert_eq!(record.start_attempt_count, 0);
    assert_eq!(record.provider_poll_attempt_count, 0);
    assert_eq!(record.provider_poll_success_count, 0);
    assert_eq!(record.provider_poll_failure_count, 0);
    assert_eq!(record.bars_observed, 0);
    assert_eq!(record.bars_dispatched, 0);
    assert_eq!(record.state_version, 1);

    cleanup_operation(&pool, record.operation_id).await;
    Ok(())
}

/// A negative counter fails the DB `CHECK` constraint (proven directly at
/// the SQL layer, bypassing the Rust store's own validation).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn negative_counters_fail() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 3).unwrap();
    let (open, close, preopen, postclose) = session_bounds(market_date);
    let operation_id = test_operation_id("negative-counter-test");

    let result = sqlx::query(
        r#"
        insert into sys_autonomous_daily_operations (
            operation_id, market_date, deployment_mode, adapter_id,
            session_plan_identity, assignment_identity, runtime_binding_identity,
            calendar_source, calendar_coverage_state, schedule_source,
            session_open_utc, session_close_utc, preopen_start_utc, postclose_finalize_utc,
            state, state_reason_code, state_version,
            run_id, start_attempt_count, last_start_attempt_utc, next_retry_utc,
            data_refresh_state, last_provider_poll_utc,
            provider_poll_attempt_count, provider_poll_success_count, provider_poll_failure_count,
            last_completed_bar_ts, last_dispatched_bar_ts, bars_observed, bars_dispatched,
            started_at_utc, stopped_at_utc, finalized_at_utc,
            outcome, no_trade_reason, last_error,
            created_at_utc, updated_at_utc
        ) values (
            $1,$2,'paper','neg_test',
            'splan','asgn','bind',
            'nyse_weekdays_heuristic','active','nyse_weekdays_heuristic',
            $3,$4,$5,$6,
            'awaiting_preopen',null,1,
            null,-1,null,null,
            'not_started',null,
            0,0,0,
            null,null,0,0,
            null,null,null,
            null,null,null,
            $7,$7
        )
        "#,
    )
    .bind(operation_id)
    .bind(market_date)
    .bind(open)
    .bind(close)
    .bind(preopen)
    .bind(postclose)
    .bind(Utc::now())
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "negative start_attempt_count must violate a CHECK constraint"
    );
    Ok(())
}

/// `session_open_utc >= session_close_utc` fails the ordering `CHECK`.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn invalid_session_boundaries_fail() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 4).unwrap();
    let operation_id = test_operation_id("invalid-boundary-test");
    let now = Utc::now();
    let bad_close = now - ChronoDuration::hours(1); // close before open

    let result = sqlx::query(
        r#"
        insert into sys_autonomous_daily_operations (
            operation_id, market_date, deployment_mode, adapter_id,
            session_plan_identity, assignment_identity, runtime_binding_identity,
            calendar_source, calendar_coverage_state, schedule_source,
            session_open_utc, session_close_utc, preopen_start_utc, postclose_finalize_utc,
            state, state_reason_code, state_version,
            run_id, start_attempt_count, last_start_attempt_utc, next_retry_utc,
            data_refresh_state, last_provider_poll_utc,
            provider_poll_attempt_count, provider_poll_success_count, provider_poll_failure_count,
            last_completed_bar_ts, last_dispatched_bar_ts, bars_observed, bars_dispatched,
            started_at_utc, stopped_at_utc, finalized_at_utc,
            outcome, no_trade_reason, last_error,
            created_at_utc, updated_at_utc
        ) values (
            $1,$2,'paper','bounds_test',
            'splan','asgn','bind',
            'nyse_weekdays_heuristic','active','nyse_weekdays_heuristic',
            $3,$4,$3,$4,
            'awaiting_preopen',null,1,
            null,0,null,null,
            'not_started',null,
            0,0,0,
            null,null,0,0,
            null,null,null,
            null,null,null,
            $3,$3
        )
        "#,
    )
    .bind(operation_id)
    .bind(market_date)
    .bind(now)
    .bind(bad_close)
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "session_close_utc before session_open_utc must violate a CHECK constraint"
    );
    Ok(())
}

/// An unknown `state` value fails the DB `CHECK` constraint.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn unknown_state_fails() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 5).unwrap();
    let (open, close, preopen, postclose) = session_bounds(market_date);
    let operation_id = test_operation_id("unknown-state-test");

    let result = sqlx::query(
        r#"
        insert into sys_autonomous_daily_operations (
            operation_id, market_date, deployment_mode, adapter_id,
            session_plan_identity, assignment_identity, runtime_binding_identity,
            calendar_source, calendar_coverage_state, schedule_source,
            session_open_utc, session_close_utc, preopen_start_utc, postclose_finalize_utc,
            state, state_reason_code, state_version,
            run_id, start_attempt_count, last_start_attempt_utc, next_retry_utc,
            data_refresh_state, last_provider_poll_utc,
            provider_poll_attempt_count, provider_poll_success_count, provider_poll_failure_count,
            last_completed_bar_ts, last_dispatched_bar_ts, bars_observed, bars_dispatched,
            started_at_utc, stopped_at_utc, finalized_at_utc,
            outcome, no_trade_reason, last_error,
            created_at_utc, updated_at_utc
        ) values (
            $1,$2,'paper','unknown_state_test',
            'splan','asgn','bind',
            'nyse_weekdays_heuristic','active','nyse_weekdays_heuristic',
            $3,$4,$5,$6,
            'not_a_real_state',null,1,
            null,0,null,null,
            'not_started',null,
            0,0,0,
            null,null,0,0,
            null,null,null,
            null,null,null,
            $7,$7
        )
        "#,
    )
    .bind(operation_id)
    .bind(market_date)
    .bind(open)
    .bind(close)
    .bind(preopen)
    .bind(postclose)
    .bind(Utc::now())
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "unknown state must violate the known-states CHECK constraint"
    );
    Ok(())
}

/// An overlong `bounded_detail` on the events table fails its length CHECK.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn overlong_bounded_detail_fails() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 6).unwrap();
    let adapter = format!("adapter_overlong_{}", unique_suffix());
    let now = Utc::now();
    let args = make_create_args(
        market_date,
        "paper",
        &adapter,
        "splan-ol",
        "asgn-ol",
        "bind-ol",
        now,
    );
    let outcome = create_or_recover_autonomous_daily_operation(&pool, &args).await?;
    let record = unwrap_created(outcome);

    let overlong = "x".repeat(5000);
    let result = sqlx::query(
        "insert into sys_autonomous_daily_operation_events \
         (operation_id, transition_seq, from_state, to_state, reason_code, occurred_at_utc, run_id, bounded_detail) \
         values ($1,2,'awaiting_preopen','preparing_data',null,$2,null,$3)",
    )
    .bind(record.operation_id)
    .bind(now)
    .bind(&overlong)
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "overlong bounded_detail must violate its length CHECK constraint"
    );
    cleanup_operation(&pool, record.operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Create / recover
// ---------------------------------------------------------------------------

/// A brand-new operation creates exactly one current-state row and one
/// initial `none -> initial_state` event.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn new_operation_creates_one_row_and_one_initial_event() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 9).unwrap(); // Monday
    let adapter = format!("adapter_new_{}", unique_suffix());
    let now = Utc::now();
    let args = make_create_args(
        market_date,
        "paper",
        &adapter,
        "splan-new",
        "asgn-new",
        "bind-new",
        now,
    );

    let outcome = create_or_recover_autonomous_daily_operation(&pool, &args).await?;
    let record = unwrap_created(outcome);
    assert_eq!(record.state, STATE_AWAITING_PREOPEN);
    assert_eq!(record.state_version, 1);

    let events = list_autonomous_daily_operation_events(&pool, record.operation_id, 20).await?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].transition_seq, 1);
    assert_eq!(events[0].from_state, "none");
    assert_eq!(events[0].to_state, STATE_AWAITING_PREOPEN);

    // Every new create through the current store API supplies all four
    // exchange-truth fields as non-null.
    assert!(record.exchange_session_open_utc.is_some());
    assert!(record.exchange_session_close_utc.is_some());
    assert!(record.exchange_is_early_close.is_some());
    assert!(record.previous_trading_date.is_some());

    cleanup_operation(&pool, record.operation_id).await;
    Ok(())
}

/// Override-shaped data (exchange boundaries wider than the effective
/// operation window) stores and round-trips the two fact sets distinctly —
/// neither is fabricated from the other.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn override_shaped_boundaries_store_exchange_and_effective_distinctly() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 9).unwrap(); // Monday
    let adapter = format!("adapter_boundaries_{}", unique_suffix());
    let now = Utc::now();
    let args = make_create_args_with_override_shaped_boundaries(
        market_date,
        "paper",
        &adapter,
        "splan-bounds",
        "asgn-bounds",
        "bind-bounds",
        now,
    );

    let record = unwrap_created(create_or_recover_autonomous_daily_operation(&pool, &args).await?);
    assert_ne!(
        record.exchange_session_open_utc,
        Some(record.effective_operation_open_utc)
    );
    assert_ne!(
        record.exchange_session_close_utc,
        Some(record.effective_operation_close_utc)
    );
    assert_eq!(
        record.exchange_session_open_utc,
        Some(args.exchange_session_open_utc)
    );
    assert_eq!(
        record.exchange_session_close_utc,
        Some(args.exchange_session_close_utc)
    );
    assert_eq!(
        record.effective_operation_open_utc,
        args.effective_operation_open_utc
    );
    assert_eq!(
        record.effective_operation_close_utc,
        args.effective_operation_close_utc
    );

    let by_id = fetch_autonomous_daily_operation_by_id(&pool, record.operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        by_id.exchange_session_open_utc,
        Some(args.exchange_session_open_utc)
    );
    assert_eq!(
        by_id.effective_operation_open_utc,
        args.effective_operation_open_utc
    );

    cleanup_operation(&pool, record.operation_id).await;
    Ok(())
}

/// A legacy row (created by directly inserting a row that leaves every new
/// exchange-truth column null, simulating a row from before migration 0049)
/// round-trips those fields as `None` — never fabricated from the effective
/// operation boundaries.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn legacy_row_with_null_exchange_fields_is_not_fabricated() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 9).unwrap();
    let (open, close, preopen, postclose) = session_bounds(market_date);
    let operation_id = test_operation_id("legacy-null-exchange-test");
    let now = Utc::now();

    sqlx::query(
        r#"
        insert into sys_autonomous_daily_operations (
            operation_id, market_date, deployment_mode, adapter_id,
            session_plan_identity, assignment_identity, runtime_binding_identity,
            calendar_source, calendar_coverage_state, schedule_source,
            session_open_utc, session_close_utc, preopen_start_utc, postclose_finalize_utc,
            state, state_reason_code, state_version,
            run_id, start_attempt_count, last_start_attempt_utc, next_retry_utc,
            data_refresh_state, last_provider_poll_utc,
            provider_poll_attempt_count, provider_poll_success_count, provider_poll_failure_count,
            last_completed_bar_ts, last_dispatched_bar_ts, bars_observed, bars_dispatched,
            started_at_utc, stopped_at_utc, finalized_at_utc,
            outcome, no_trade_reason, last_error,
            created_at_utc, updated_at_utc
        ) values (
            $1,$2,'paper','legacy_null_test',
            'splan-legacy','asgn-legacy','bind-legacy',
            'nyse_weekdays_heuristic','active','nyse_weekdays_heuristic',
            $3,$4,$5,$6,
            'awaiting_preopen',null,1,
            null,0,null,null,
            'not_started',null,
            0,0,0,
            null,null,0,0,
            null,null,null,
            null,null,null,
            $7,$7
        )
        "#,
    )
    .bind(operation_id)
    .bind(market_date)
    .bind(open)
    .bind(close)
    .bind(preopen)
    .bind(postclose)
    .bind(now)
    .execute(&pool)
    .await?;

    let record = fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("legacy row must exist");
    assert!(record.exchange_session_open_utc.is_none());
    assert!(record.exchange_session_close_utc.is_none());
    assert!(record.exchange_is_early_close.is_none());
    assert!(record.previous_trading_date.is_none());
    // The effective-operation boundaries remain present and unaffected.
    assert_eq!(record.effective_operation_open_utc, open);
    assert_eq!(record.effective_operation_close_utc, close);

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

/// A repeated, identical create call returns `Recovered`, mutates nothing,
/// and creates no second initial event.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn repeated_identical_create_returns_recovered_with_no_second_event() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 10).unwrap();
    let adapter = format!("adapter_recover_{}", unique_suffix());
    let now = Utc::now();
    let args = make_create_args(
        market_date,
        "paper",
        &adapter,
        "splan-rec",
        "asgn-rec",
        "bind-rec",
        now,
    );

    let first = unwrap_created(create_or_recover_autonomous_daily_operation(&pool, &args).await?);

    let second_args = make_create_args(
        market_date,
        "paper",
        &adapter,
        "splan-rec",
        "asgn-rec",
        "bind-rec",
        now + ChronoDuration::seconds(5),
    );
    let second = create_or_recover_autonomous_daily_operation(&pool, &second_args).await?;
    let recovered = match second {
        CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(r) => r,
        other => panic!("expected Recovered, got {other:?}"),
    };
    assert_eq!(recovered.operation_id, first.operation_id);
    // Compare against a fresh DB fetch, not the in-memory `first` value:
    // Postgres `timestamptz` truncates to microsecond precision, while
    // `first.created_at_utc` (returned directly from the `Created` outcome)
    // still carries `Utc::now()`'s full nanosecond precision — an expected
    // storage-precision difference, not evidence of a rewrite.
    let first_from_db = fetch_autonomous_daily_operation_by_id(&pool, first.operation_id)
        .await?
        .expect("original row must exist");
    assert_eq!(
        recovered.created_at_utc, first_from_db.created_at_utc,
        "recovery must not rewrite created_at_utc"
    );

    let events = list_autonomous_daily_operation_events(&pool, first.operation_id, 20).await?;
    assert_eq!(
        events.len(),
        1,
        "recovery must not insert a second initial event"
    );

    cleanup_operation(&pool, first.operation_id).await;
    Ok(())
}

/// Two concurrent identical create calls for the same slot produce exactly
/// one `Created`, one `Recovered`, one row, and one initial event.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn concurrent_identical_create_produces_one_row_one_event() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 11).unwrap();
    let adapter = format!("adapter_concurrent_{}", unique_suffix());
    let now = Utc::now();
    let args_a = make_create_args(
        market_date,
        "paper",
        &adapter,
        "splan-conc",
        "asgn-conc",
        "bind-conc",
        now,
    );
    let args_b = args_a.clone();

    let pool_a = pool.clone();
    let pool_b = pool.clone();
    let (res_a, res_b) = tokio::join!(
        tokio::spawn(async move {
            create_or_recover_autonomous_daily_operation(&pool_a, &args_a).await
        }),
        tokio::spawn(async move {
            create_or_recover_autonomous_daily_operation(&pool_b, &args_b).await
        }),
    );
    let outcome_a = res_a.unwrap()?;
    let outcome_b = res_b.unwrap()?;

    let created_count = [&outcome_a, &outcome_b]
        .iter()
        .filter(|o| {
            matches!(
                o,
                CreateOrRecoverAutonomousDailyOperationOutcome::Created(_)
            )
        })
        .count();
    let recovered_count = [&outcome_a, &outcome_b]
        .iter()
        .filter(|o| {
            matches!(
                o,
                CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(_)
            )
        })
        .count();
    assert_eq!(
        created_count, 1,
        "exactly one concurrent call must observe Created"
    );
    assert_eq!(
        recovered_count, 1,
        "exactly one concurrent call must observe Recovered"
    );

    let operation_id = match &outcome_a {
        CreateOrRecoverAutonomousDailyOperationOutcome::Created(r) => r.operation_id,
        CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(r) => r.operation_id,
        _ => panic!("unexpected outcome"),
    };
    let events = list_autonomous_daily_operation_events(&pool, operation_id, 20).await?;
    assert_eq!(
        events.len(),
        1,
        "concurrent create must produce exactly one initial event"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

/// A different `market_date` creates a distinct row, not a shared slot.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn different_market_date_creates_different_row() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter = format!("adapter_diffdate_{}", unique_suffix());
    let now = Utc::now();
    let day1 = NaiveDate::from_ymd_opt(2099, 3, 16).unwrap();
    let day2 = NaiveDate::from_ymd_opt(2099, 3, 17).unwrap();

    let args1 = make_create_args(
        day1, "paper", &adapter, "splan-d1", "asgn-d1", "bind-d1", now,
    );
    let args2 = make_create_args(
        day2, "paper", &adapter, "splan-d2", "asgn-d2", "bind-d2", now,
    );

    let r1 = unwrap_created(create_or_recover_autonomous_daily_operation(&pool, &args1).await?);
    let r2 = unwrap_created(create_or_recover_autonomous_daily_operation(&pool, &args2).await?);
    assert_ne!(r1.operation_id, r2.operation_id);

    cleanup_operation(&pool, r1.operation_id).await;
    cleanup_operation(&pool, r2.operation_id).await;
    Ok(())
}

/// PAPER-SOAK-FRIDAY-RECOVERY-LAUNCHER-HARDENING-AND-MONITOR-01, Phase 5.
///
/// A prior day's operation left TERMINAL in `manual_intervention_required`
/// (Thursday 2026-08-13's real record: `reason=interior_gap`) must never
/// block, gate, or leak into the next day's operation. The daily slot key
/// `(market_date, deployment_mode, adapter_id)` plus a `market_date`-seeded
/// deterministic `operation_id` should make day 2's create-or-recover
/// entirely independent of day 1's terminal state. `different_market_date_
/// creates_different_row` above already proves cross-date independence in
/// general, but never against a day 1 row actually left in
/// `manual_intervention_required` -- this closes that specific gap.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn manual_intervention_required_prior_day_does_not_block_next_day() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter = format!("adapter_mireq_{}", unique_suffix());
    let now = Utc::now();
    let day1 = NaiveDate::from_ymd_opt(2099, 3, 19).unwrap();
    let day2 = NaiveDate::from_ymd_opt(2099, 3, 20).unwrap();

    // Day 1: create, then walk the real interior_gap path -- preparing_data
    // (data readiness gate runs here) -> manual_intervention_required -- and
    // leave it there, terminal for the day, exactly like Thursday's real
    // record.
    let args1 = make_create_args(
        day1, "paper", &adapter, "splan-d1", "asgn-d1", "bind-d1", now,
    );
    let day1_op = unwrap_created(create_or_recover_autonomous_daily_operation(&pool, &args1).await?);

    let to_preparing = TransitionAutonomousDailyOperationArgs {
        operation_id: day1_op.operation_id,
        expected_state: STATE_AWAITING_PREOPEN.to_string(),
        expected_state_version: 1,
        new_state: STATE_PREPARING_DATA.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: now,
        run_id: None,
        bounded_detail: "data readiness gate running".to_string(),
    };
    transition_autonomous_daily_operation(&pool, &to_preparing).await?;

    let to_manual = TransitionAutonomousDailyOperationArgs {
        operation_id: day1_op.operation_id,
        expected_state: STATE_PREPARING_DATA.to_string(),
        expected_state_version: 2,
        new_state: STATE_MANUAL_INTERVENTION_REQUIRED.to_string(),
        reason_code: Some("interior_gap".to_string()),
        blocker_signature: Some("interior_gap".to_string()),
        occurred_at_utc: now,
        run_id: None,
        bounded_detail: "interior_gap: AAPL/5m required-universe readiness blocked".to_string(),
    };
    let manual_outcome = transition_autonomous_daily_operation(&pool, &to_manual).await?;
    let day1_final = match manual_outcome {
        AutonomousDailyTransitionOutcome::Applied(r) => r,
        other => panic!("expected Applied, got {other:?}"),
    };
    assert_eq!(day1_final.state, STATE_MANUAL_INTERVENTION_REQUIRED);

    // Day 2: an entirely fresh create-or-recover call for the *same*
    // deployment_mode/adapter_id, a different market_date. Must succeed and
    // must not be seeded from, blocked by, or in any way reflect day 1's
    // terminal manual_intervention_required row.
    let args2 = make_create_args(
        day2, "paper", &adapter, "splan-d2", "asgn-d2", "bind-d2", now,
    );
    let day2_op = unwrap_created(create_or_recover_autonomous_daily_operation(&pool, &args2).await?);

    assert_ne!(
        day2_op.operation_id, day1_final.operation_id,
        "day 2 must get its own distinct operation_id, not day 1's"
    );
    assert_eq!(
        day2_op.state, STATE_AWAITING_PREOPEN,
        "day 2 must start fresh at awaiting_preopen, never inheriting day 1's manual_intervention_required"
    );
    assert_eq!(day2_op.market_date, day2);

    // Fetching day 1's slot directly still honestly reports its own terminal
    // state -- day 1's history is preserved, not rewritten or deleted by
    // day 2's creation.
    let day1_refetched = fetch_autonomous_daily_operation_for_slot(&pool, day1, "paper", &adapter)
        .await?
        .expect("day 1 row must still exist");
    assert_eq!(day1_refetched.state, STATE_MANUAL_INTERVENTION_REQUIRED);
    assert_eq!(day1_refetched.operation_id, day1_final.operation_id);

    cleanup_operation(&pool, day1_final.operation_id).await;
    cleanup_operation(&pool, day2_op.operation_id).await;
    Ok(())
}

/// A different `deployment_mode`/`adapter_id` uses a different daily slot,
/// never conflicting with another slot on the same `market_date`.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn different_deployment_or_adapter_uses_different_slot() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 18).unwrap();
    let now = Utc::now();
    let adapter_x = format!("adapter_x_{}", unique_suffix());
    let adapter_y = format!("adapter_y_{}", unique_suffix());

    let args1 = make_create_args(
        market_date,
        "paper",
        &adapter_x,
        "splan-x",
        "asgn-x",
        "bind-x",
        now,
    );
    let args2 = make_create_args(
        market_date,
        "paper",
        &adapter_y,
        "splan-y",
        "asgn-y",
        "bind-y",
        now,
    );

    let r1 = unwrap_created(create_or_recover_autonomous_daily_operation(&pool, &args1).await?);
    let r2 = unwrap_created(create_or_recover_autonomous_daily_operation(&pool, &args2).await?);
    assert_ne!(r1.operation_id, r2.operation_id);

    cleanup_operation(&pool, r1.operation_id).await;
    cleanup_operation(&pool, r2.operation_id).await;
    Ok(())
}

/// Same slot, changed `session_plan_identity` -> `IdentityConflict`; no
/// immutable field on the existing row is mutated and no event is inserted.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn changed_session_plan_identity_returns_identity_conflict() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 19).unwrap();
    let adapter = format!("adapter_splanconf_{}", unique_suffix());
    let now = Utc::now();

    let args1 = make_create_args(
        market_date,
        "paper",
        &adapter,
        "splan-A",
        "asgn-same",
        "bind-same",
        now,
    );
    let original =
        unwrap_created(create_or_recover_autonomous_daily_operation(&pool, &args1).await?);

    let args2 = make_create_args(
        market_date,
        "paper",
        &adapter,
        "splan-B",
        "asgn-same",
        "bind-same",
        now,
    );
    let outcome = create_or_recover_autonomous_daily_operation(&pool, &args2).await?;
    match outcome {
        CreateOrRecoverAutonomousDailyOperationOutcome::IdentityConflict {
            existing_operation_id,
            expected_operation_id,
            differing_fields,
        } => {
            assert_eq!(existing_operation_id, original.operation_id);
            assert_ne!(expected_operation_id, original.operation_id);
            assert!(differing_fields.contains(&"session_plan_identity".to_string()));
        }
        other => panic!("expected IdentityConflict, got {other:?}"),
    }

    let after = fetch_autonomous_daily_operation_by_id(&pool, original.operation_id)
        .await?
        .expect("original row must still exist unmutated");
    assert_eq!(
        after.session_plan_identity, "splan-A",
        "existing identity field must not be rewritten"
    );

    let events = list_autonomous_daily_operation_events(&pool, original.operation_id, 20).await?;
    assert_eq!(
        events.len(),
        1,
        "identity conflict must not insert an event"
    );

    cleanup_operation(&pool, original.operation_id).await;
    Ok(())
}

/// Same slot, changed `assignment_identity` -> `IdentityConflict`.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn changed_assignment_identity_returns_identity_conflict() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 20).unwrap();
    let adapter = format!("adapter_asgnconf_{}", unique_suffix());
    let now = Utc::now();

    let args1 = make_create_args(
        market_date,
        "paper",
        &adapter,
        "splan-same",
        "asgn-A",
        "bind-same",
        now,
    );
    let original =
        unwrap_created(create_or_recover_autonomous_daily_operation(&pool, &args1).await?);

    let args2 = make_create_args(
        market_date,
        "paper",
        &adapter,
        "splan-same",
        "asgn-B",
        "bind-same",
        now,
    );
    let outcome = create_or_recover_autonomous_daily_operation(&pool, &args2).await?;
    match outcome {
        CreateOrRecoverAutonomousDailyOperationOutcome::IdentityConflict {
            differing_fields,
            ..
        } => {
            assert!(differing_fields.contains(&"assignment_identity".to_string()));
        }
        other => panic!("expected IdentityConflict, got {other:?}"),
    }

    cleanup_operation(&pool, original.operation_id).await;
    Ok(())
}

/// Same slot, changed `runtime_binding_identity` -> `IdentityConflict`.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn changed_runtime_binding_identity_returns_identity_conflict() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 23).unwrap(); // Monday
    let adapter = format!("adapter_bindconf_{}", unique_suffix());
    let now = Utc::now();

    let args1 = make_create_args(
        market_date,
        "paper",
        &adapter,
        "splan-same",
        "asgn-same",
        "bind-A",
        now,
    );
    let original =
        unwrap_created(create_or_recover_autonomous_daily_operation(&pool, &args1).await?);

    let args2 = make_create_args(
        market_date,
        "paper",
        &adapter,
        "splan-same",
        "asgn-same",
        "bind-B",
        now,
    );
    let outcome = create_or_recover_autonomous_daily_operation(&pool, &args2).await?;
    match outcome {
        CreateOrRecoverAutonomousDailyOperationOutcome::IdentityConflict {
            differing_fields,
            ..
        } => {
            assert!(differing_fields.contains(&"runtime_binding_identity".to_string()));
        }
        other => panic!("expected IdentityConflict, got {other:?}"),
    }

    cleanup_operation(&pool, original.operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// CAS transitions
// ---------------------------------------------------------------------------

async fn create_test_operation(
    pool: &sqlx::PgPool,
    market_date: NaiveDate,
    adapter: &str,
) -> AutonomousDailyOperationRecord {
    let now = Utc::now();
    let args = make_create_args(
        market_date,
        "paper",
        adapter,
        "splan-t",
        "asgn-t",
        "bind-t",
        now,
    );
    unwrap_created(
        create_or_recover_autonomous_daily_operation(pool, &args)
            .await
            .expect("create must succeed"),
    )
}

/// A legal transition updates the current-state row and inserts a matching
/// event atomically, keeping `state_version == transition_seq`.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn legal_transition_updates_row_and_inserts_matching_event() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 24).unwrap();
    let adapter = format!("adapter_legal_{}", unique_suffix());
    let op = create_test_operation(&pool, market_date, &adapter).await;

    let args = TransitionAutonomousDailyOperationArgs {
        operation_id: op.operation_id,
        expected_state: STATE_AWAITING_PREOPEN.to_string(),
        expected_state_version: 1,
        new_state: STATE_PREPARING_DATA.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: Utc::now(),
        run_id: None,
        bounded_detail: "legal transition".to_string(),
    };
    let outcome = transition_autonomous_daily_operation(&pool, &args).await?;
    let record = match outcome {
        AutonomousDailyTransitionOutcome::Applied(r) => r,
        other => panic!("expected Applied, got {other:?}"),
    };
    assert_eq!(record.state, STATE_PREPARING_DATA);
    assert_eq!(record.state_version, 2);

    let events = list_autonomous_daily_operation_events(&pool, op.operation_id, 20).await?;
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].transition_seq, record.state_version);
    assert_eq!(events[1].from_state, STATE_AWAITING_PREOPEN);
    assert_eq!(events[1].to_state, STATE_PREPARING_DATA);

    cleanup_operation(&pool, op.operation_id).await;
    Ok(())
}

/// An illegal transition writes nothing at all.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn illegal_transition_writes_nothing() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 25).unwrap();
    let adapter = format!("adapter_illegal_{}", unique_suffix());
    let op = create_test_operation(&pool, market_date, &adapter).await;

    // awaiting_preopen -> running is not a legal edge.
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id: op.operation_id,
        expected_state: STATE_AWAITING_PREOPEN.to_string(),
        expected_state_version: 1,
        new_state: STATE_RUNNING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: Utc::now(),
        run_id: None,
        bounded_detail: "illegal transition attempt".to_string(),
    };
    let outcome = transition_autonomous_daily_operation(&pool, &args).await?;
    assert!(matches!(
        outcome,
        AutonomousDailyTransitionOutcome::IllegalTransition
    ));

    let after = fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await?
        .unwrap();
    assert_eq!(after.state, STATE_AWAITING_PREOPEN);
    assert_eq!(after.state_version, 1);
    let events = list_autonomous_daily_operation_events(&pool, op.operation_id, 20).await?;
    assert_eq!(events.len(), 1);

    cleanup_operation(&pool, op.operation_id).await;
    Ok(())
}

/// A stale `expected_state_version` writes nothing.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn stale_version_writes_nothing() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 26).unwrap();
    let adapter = format!("adapter_stale_{}", unique_suffix());
    let op = create_test_operation(&pool, market_date, &adapter).await;

    // Advance once to version 2.
    let first = TransitionAutonomousDailyOperationArgs {
        operation_id: op.operation_id,
        expected_state: STATE_AWAITING_PREOPEN.to_string(),
        expected_state_version: 1,
        new_state: STATE_PREPARING_DATA.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: Utc::now(),
        run_id: None,
        bounded_detail: "advance".to_string(),
    };
    transition_autonomous_daily_operation(&pool, &first).await?;

    // Retry against the now-stale version 1, requesting a *different* legal
    // edge (awaiting_preopen -> calendar_unavailable) than the one already
    // applied — this must not be misclassified as an exact replay (that
    // scenario is covered separately by `exact_retry_returns_already_applied`).
    let stale = TransitionAutonomousDailyOperationArgs {
        operation_id: op.operation_id,
        expected_state: STATE_AWAITING_PREOPEN.to_string(),
        expected_state_version: 1,
        new_state: STATE_CALENDAR_UNAVAILABLE.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: Utc::now(),
        run_id: None,
        bounded_detail: "stale retry".to_string(),
    };
    let outcome = transition_autonomous_daily_operation(&pool, &stale).await?;
    match outcome {
        AutonomousDailyTransitionOutcome::StaleState {
            actual_state,
            actual_state_version,
        } => {
            assert_eq!(actual_state, STATE_PREPARING_DATA);
            assert_eq!(actual_state_version, 2);
        }
        other => panic!("expected StaleState, got {other:?}"),
    }

    let events = list_autonomous_daily_operation_events(&pool, op.operation_id, 20).await?;
    assert_eq!(events.len(), 2, "stale retry must not insert a third event");

    cleanup_operation(&pool, op.operation_id).await;
    Ok(())
}

/// A wrong `expected_state` (version matches, state does not) writes
/// nothing and reports the actual current state.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn wrong_expected_state_writes_nothing() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 27).unwrap();
    let adapter = format!("adapter_wrongstate_{}", unique_suffix());
    let op = create_test_operation(&pool, market_date, &adapter).await;

    let args = TransitionAutonomousDailyOperationArgs {
        operation_id: op.operation_id,
        expected_state: STATE_PREPARING_DATA.to_string(), // actual is awaiting_preopen
        expected_state_version: 1,
        new_state: STATE_PREFLIGHT_BLOCKED.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: Utc::now(),
        run_id: None,
        bounded_detail: "wrong expected state".to_string(),
    };
    let outcome = transition_autonomous_daily_operation(&pool, &args).await?;
    match outcome {
        AutonomousDailyTransitionOutcome::StaleState {
            actual_state,
            actual_state_version,
        } => {
            assert_eq!(actual_state, STATE_AWAITING_PREOPEN);
            assert_eq!(actual_state_version, 1);
        }
        other => panic!("expected StaleState, got {other:?}"),
    }

    let events = list_autonomous_daily_operation_events(&pool, op.operation_id, 20).await?;
    assert_eq!(events.len(), 1);

    cleanup_operation(&pool, op.operation_id).await;
    Ok(())
}

/// An exact retry of an already-applied transition returns `AlreadyApplied`,
/// never a double-increment or a duplicate event.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn exact_retry_returns_already_applied() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 30).unwrap(); // Monday
    let adapter = format!("adapter_replay_{}", unique_suffix());
    let op = create_test_operation(&pool, market_date, &adapter).await;

    let args = TransitionAutonomousDailyOperationArgs {
        operation_id: op.operation_id,
        expected_state: STATE_AWAITING_PREOPEN.to_string(),
        expected_state_version: 1,
        new_state: STATE_PREPARING_DATA.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: Utc::now(),
        run_id: None,
        bounded_detail: "first attempt".to_string(),
    };
    let first = transition_autonomous_daily_operation(&pool, &args).await?;
    assert!(matches!(
        first,
        AutonomousDailyTransitionOutcome::Applied(_)
    ));

    // Retry the identical request (same expected_state/expected_state_version).
    let retry = transition_autonomous_daily_operation(&pool, &args).await?;
    match retry {
        AutonomousDailyTransitionOutcome::AlreadyApplied(record) => {
            assert_eq!(record.state, STATE_PREPARING_DATA);
            assert_eq!(record.state_version, 2);
        }
        other => panic!("expected AlreadyApplied, got {other:?}"),
    }

    let events = list_autonomous_daily_operation_events(&pool, op.operation_id, 20).await?;
    assert_eq!(
        events.len(),
        2,
        "an exact replay must not insert a duplicate event"
    );

    cleanup_operation(&pool, op.operation_id).await;
    Ok(())
}

/// Two concurrent transitions expecting the same `(state, state_version)`
/// permit only one `Applied`; the other observes `StaleState`.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn concurrent_same_version_transitions_permit_only_one_applied() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 3, 31).unwrap();
    let adapter = format!("adapter_concxn_{}", unique_suffix());
    let op = create_test_operation(&pool, market_date, &adapter).await;

    let args_a = TransitionAutonomousDailyOperationArgs {
        operation_id: op.operation_id,
        expected_state: STATE_AWAITING_PREOPEN.to_string(),
        expected_state_version: 1,
        new_state: STATE_PREPARING_DATA.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: Utc::now(),
        run_id: None,
        bounded_detail: "racer A".to_string(),
    };
    let args_b = TransitionAutonomousDailyOperationArgs {
        bounded_detail: "racer B".to_string(),
        ..args_a.clone()
    };

    let pool_a = pool.clone();
    let pool_b = pool.clone();
    let (res_a, res_b) = tokio::join!(
        tokio::spawn(async move { transition_autonomous_daily_operation(&pool_a, &args_a).await }),
        tokio::spawn(async move { transition_autonomous_daily_operation(&pool_b, &args_b).await }),
    );
    let outcome_a = res_a.unwrap()?;
    let outcome_b = res_b.unwrap()?;

    let applied_count = [&outcome_a, &outcome_b]
        .iter()
        .filter(|o| matches!(o, AutonomousDailyTransitionOutcome::Applied(_)))
        .count();
    assert_eq!(
        applied_count, 1,
        "exactly one concurrent transition must be Applied"
    );

    let stale_or_already = [&outcome_a, &outcome_b]
        .iter()
        .filter(|o| {
            matches!(
                o,
                AutonomousDailyTransitionOutcome::StaleState { .. }
                    | AutonomousDailyTransitionOutcome::AlreadyApplied(_)
            )
        })
        .count();
    assert_eq!(stale_or_already, 1);

    let events = list_autonomous_daily_operation_events(&pool, op.operation_id, 20).await?;
    assert_eq!(
        events.len(),
        2,
        "only one transition event may be recorded for version 2"
    );

    cleanup_operation(&pool, op.operation_id).await;
    Ok(())
}

/// A forced event-insert failure (a pre-existing conflicting event row at
/// the destination `transition_seq`) rolls back the current-state UPDATE —
/// proven against a real `PRIMARY KEY` violation, not a mock.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn forced_event_insert_failure_rolls_back_state_update() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 4, 1).unwrap();
    let adapter = format!("adapter_rollback_{}", unique_suffix());
    let op = create_test_operation(&pool, market_date, &adapter).await;

    // Pre-insert a conflicting event row at the transition_seq (2) this
    // transition would produce, forcing the function's own event INSERT to
    // violate the events table's PRIMARY KEY.
    sqlx::query(
        "insert into sys_autonomous_daily_operation_events \
         (operation_id, transition_seq, from_state, to_state, reason_code, occurred_at_utc, run_id, bounded_detail) \
         values ($1,2,'awaiting_preopen','preparing_data',null,$2,null,'pre-existing collision')",
    )
    .bind(op.operation_id)
    .bind(Utc::now())
    .execute(&pool)
    .await?;

    let args = TransitionAutonomousDailyOperationArgs {
        operation_id: op.operation_id,
        expected_state: STATE_AWAITING_PREOPEN.to_string(),
        expected_state_version: 1,
        new_state: STATE_PREPARING_DATA.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: Utc::now(),
        run_id: None,
        bounded_detail: "should roll back".to_string(),
    };
    let result = transition_autonomous_daily_operation(&pool, &args).await;
    assert!(
        result.is_err(),
        "the forced PRIMARY KEY violation must surface as Err"
    );

    let after = fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await?
        .unwrap();
    assert_eq!(
        after.state, STATE_AWAITING_PREOPEN,
        "current-state UPDATE must have rolled back"
    );
    assert_eq!(
        after.state_version, 1,
        "state_version must not have advanced"
    );

    cleanup_operation(&pool, op.operation_id).await;
    Ok(())
}

/// A completed-* state can never transition back to `running` — proven via
/// a real end-to-end pipeline to `completed`, then an attempted illegal
/// transition out of it.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn completed_state_cannot_return_to_running() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2099, 4, 2).unwrap();
    let adapter = format!("adapter_terminal_{}", unique_suffix());
    let op = create_test_operation(&pool, market_date, &adapter).await;

    let steps: [(&str, i64, &str); 5] = [
        (STATE_AWAITING_PREOPEN, 1, STATE_PREPARING_DATA),
        (STATE_PREPARING_DATA, 2, STATE_AWAITING_OPEN_LOCAL),
        (STATE_AWAITING_OPEN_LOCAL, 3, STATE_START_RETRYING),
        (STATE_START_RETRYING, 4, STATE_RUNNING),
        (STATE_RUNNING, 5, STATE_STOPPING),
    ];
    for (expected_state, expected_version, new_state) in steps {
        let args = TransitionAutonomousDailyOperationArgs {
            operation_id: op.operation_id,
            expected_state: expected_state.to_string(),
            expected_state_version: expected_version,
            new_state: new_state.to_string(),
            reason_code: None,
            blocker_signature: None,
            occurred_at_utc: Utc::now(),
            run_id: None,
            bounded_detail: format!("pipeline step to {new_state}"),
        };
        let outcome = transition_autonomous_daily_operation(&pool, &args).await?;
        assert!(
            matches!(outcome, AutonomousDailyTransitionOutcome::Applied(_)),
            "step to {new_state} must apply"
        );
    }

    let final_step = TransitionAutonomousDailyOperationArgs {
        operation_id: op.operation_id,
        expected_state: STATE_STOPPING.to_string(),
        expected_state_version: 6,
        new_state: STATE_COMPLETED.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: Utc::now(),
        run_id: None,
        bounded_detail: "finalize".to_string(),
    };
    let completed_outcome = transition_autonomous_daily_operation(&pool, &final_step).await?;
    assert!(matches!(
        completed_outcome,
        AutonomousDailyTransitionOutcome::Applied(_)
    ));

    let illegal_resurrect = TransitionAutonomousDailyOperationArgs {
        operation_id: op.operation_id,
        expected_state: STATE_COMPLETED.to_string(),
        expected_state_version: 7,
        new_state: STATE_RUNNING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: Utc::now(),
        run_id: None,
        bounded_detail: "illegal resurrection attempt".to_string(),
    };
    let outcome = transition_autonomous_daily_operation(&pool, &illegal_resurrect).await?;
    assert!(matches!(
        outcome,
        AutonomousDailyTransitionOutcome::IllegalTransition
    ));

    let after = fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await?
        .unwrap();
    assert_eq!(after.state, STATE_COMPLETED);
    assert_eq!(after.state_version, 7);

    cleanup_operation(&pool, op.operation_id).await;
    Ok(())
}

// A local alias to avoid importing STATE_AWAITING_OPEN under a name that
// shadows nothing else in this file.
const STATE_AWAITING_OPEN_LOCAL: &str = "awaiting_open";

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

/// Fetch/list functions perform no writes, order deterministically, keep
/// nullable values nullable, and keep explicit zero counters zero.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn reads_are_pure_deterministic_and_preserve_null_vs_zero() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter = format!("adapter_reads_{}", unique_suffix());
    let now = Utc::now();
    let day_a = NaiveDate::from_ymd_opt(2099, 4, 6).unwrap(); // Monday
    let day_b = NaiveDate::from_ymd_opt(2099, 4, 7).unwrap();

    let args_a = make_create_args(
        day_a, "paper", &adapter, "splan-ra", "asgn-ra", "bind-ra", now,
    );
    let args_b = make_create_args(
        day_b,
        "paper",
        &adapter,
        "splan-rb",
        "asgn-rb",
        "bind-rb",
        now + ChronoDuration::seconds(1),
    );
    let rec_a = unwrap_created(create_or_recover_autonomous_daily_operation(&pool, &args_a).await?);
    let rec_b = unwrap_created(create_or_recover_autonomous_daily_operation(&pool, &args_b).await?);

    // Nullable vs. explicit zero.
    assert!(rec_a.state_reason_code.is_none());
    assert!(rec_a.run_id.is_none());
    assert!(rec_a.last_completed_bar_ts.is_none());
    assert_eq!(rec_a.bars_observed, 0);
    assert_eq!(rec_a.bars_dispatched, 0);

    // fetch_by_id / fetch_for_slot are read-only and preserve both boundary
    // sets (effective operation and exchange truth) distinctly.
    let by_id = fetch_autonomous_daily_operation_by_id(&pool, rec_a.operation_id)
        .await?
        .unwrap();
    assert_eq!(by_id.operation_id, rec_a.operation_id);
    assert_eq!(
        by_id.effective_operation_open_utc,
        args_a.effective_operation_open_utc
    );
    assert_eq!(
        by_id.exchange_session_open_utc,
        Some(args_a.exchange_session_open_utc)
    );
    assert_eq!(
        by_id.exchange_is_early_close,
        Some(args_a.exchange_is_early_close)
    );
    assert_eq!(
        by_id.previous_trading_date,
        Some(args_a.previous_trading_date)
    );
    let by_slot = fetch_autonomous_daily_operation_for_slot(&pool, day_a, "paper", &adapter)
        .await?
        .unwrap();
    assert_eq!(by_slot.operation_id, rec_a.operation_id);
    assert_eq!(
        by_slot.exchange_session_close_utc,
        Some(args_a.exchange_session_close_utc)
    );

    // Deterministic ordering: market_date desc first.
    let recent = list_recent_autonomous_daily_operations(&pool, 50).await?;
    let idx_a = recent
        .iter()
        .position(|r| r.operation_id == rec_a.operation_id);
    let idx_b = recent
        .iter()
        .position(|r| r.operation_id == rec_b.operation_id);
    if let (Some(ia), Some(ib)) = (idx_a, idx_b) {
        assert!(ib < ia, "day_b (later market_date) must sort before day_a");
    }
    if let Some(ia) = idx_a {
        // list_recent_* preserves both boundary sets too.
        assert_eq!(
            recent[ia].exchange_session_open_utc,
            Some(args_a.exchange_session_open_utc)
        );
        assert_eq!(
            recent[ia].effective_operation_open_utc,
            args_a.effective_operation_open_utc
        );
    }

    // Event ordering ascending by transition_seq.
    let trans_args = TransitionAutonomousDailyOperationArgs {
        operation_id: rec_a.operation_id,
        expected_state: STATE_AWAITING_PREOPEN.to_string(),
        expected_state_version: 1,
        new_state: STATE_PREPARING_DATA.to_string(),
        reason_code: Some("test_reason".to_string()),
        blocker_signature: None,
        occurred_at_utc: Utc::now(),
        run_id: None,
        bounded_detail: "second event".to_string(),
    };
    transition_autonomous_daily_operation(&pool, &trans_args).await?;
    let events = list_autonomous_daily_operation_events(&pool, rec_a.operation_id, 20).await?;
    assert_eq!(events.len(), 2);
    assert!(events[0].transition_seq < events[1].transition_seq);

    // Bounded limit: a non-positive limit still returns at least one row
    // (clamped to a minimum of 1, never zero rows due to `limit 0`).
    let bounded = list_autonomous_daily_operation_events(&pool, rec_a.operation_id, 0).await?;
    assert!(!bounded.is_empty());
    // A very large limit does not error (clamped to a maximum internally).
    let large = list_recent_autonomous_daily_operations(&pool, 999_999).await?;
    assert!(!large.is_empty());

    cleanup_operation(&pool, rec_a.operation_id).await;
    cleanup_operation(&pool, rec_b.operation_id).await;
    Ok(())
}
