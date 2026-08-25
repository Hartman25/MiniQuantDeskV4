//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-DURABLE-SESSION-COORDINATOR: mqk-db
//! store proof for the D2 start/running/stop attempt evidence recorders
//! (`record_start_attempt`, `record_running_started`, `record_retry_timing`,
//! `clear_retry_timing`, `record_stop_attempt`, `record_stopped_at`) and for
//! migration `0051_autonomous_daily_stop_retry_evidence.sql`.
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-db --test scenario_autonomous_daily_operation_lifecycle_01 \
//!   -- --include-ignored --test-threads=1 --nocapture

use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use mqk_db::{
    clear_retry_timing, create_or_recover_autonomous_daily_operation,
    fetch_autonomous_daily_operation_event_at_sequence,
    fetch_relevant_open_autonomous_daily_operation, list_autonomous_daily_operation_events,
    record_retry_timing, record_running_started, record_start_attempt, record_stop_attempt,
    record_stopped_at, refresh_autonomous_daily_operation_blocker,
    transition_autonomous_daily_operation, AutonomousDailyOperationRecord,
    AutonomousDailyTransitionOutcome, CreateAutonomousDailyOperationArgs,
    CreateOrRecoverAutonomousDailyOperationOutcome, RecordRetryTimingOutcome,
    RecordRunningStartedOutcome, RecordStartAttemptOutcome, RecordStopAttemptOutcome,
    RecordStoppedAtOutcome, RefreshAutonomousDailyOperationBlockerArgs,
    RefreshAutonomousDailyOperationBlockerOutcome, TransitionAutonomousDailyOperationArgs,
    ENV_DB_URL, STATE_AWAITING_OPEN, STATE_AWAITING_PREOPEN, STATE_CONTROLLER_DEGRADED,
    STATE_MANUAL_INTERVENTION_REQUIRED, STATE_PREPARING_DATA, STATE_RECOVERY_RETRYING,
    STATE_RUNNING, STATE_START_RETRYING, STATE_STOPPING, STATE_STOP_RETRYING,
};
use mqk_db::{
    arm_run, begin_run, insert_run, persist_reconcile_status_state, stop_run, NewRun,
    PersistReconcileStatusState,
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

fn unique_suffix() -> String {
    Uuid::new_v4().to_string().replace('-', "")[..10].to_string()
}

async fn cleanup_run(pool: &sqlx::PgPool, run_id: Uuid) {
    let _ = sqlx::query("delete from oms_outbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
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

/// Create one fresh operation row for `seed`, returning its `operation_id`.
async fn seed_operation(pool: &sqlx::PgPool, seed: &str) -> Uuid {
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let (open, close, preopen, postclose) = session_bounds(market_date);
    let previous_trading_date = market_date - ChronoDuration::days(3);
    let operation_id = test_operation_id(seed);
    let args = CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date,
        deployment_mode: "paper".to_string(),
        adapter_id: format!("lifecycle-test-{seed}"),
        session_plan_identity: format!("lifecycle-plan|{seed}"),
        assignment_identity: "lifecycle-assignment".to_string(),
        runtime_binding_identity: "lifecycle-binding".to_string(),
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
        occurred_at_utc: preopen,
        bounded_detail: "lifecycle test creation".to_string(),
        stop_attempt_count: 0,
    };
    match create_or_recover_autonomous_daily_operation(pool, &args)
        .await
        .expect("create operation")
    {
        CreateOrRecoverAutonomousDailyOperationOutcome::Created(r) => r.operation_id,
        CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(r) => r.operation_id,
        other => panic!("expected Created or Recovered, got {other:?}"),
    }
}

/// Create one fresh operation row for `seed` on an arbitrary `market_date`
/// (all sharing the same `adapter_id` suffix per `seed`), returning its
/// `operation_id`. Used only by the REPAIR 1 relevant-lookup tests, which
/// need multiple distinct daily slots for the same adapter.
async fn seed_operation_for_date(pool: &sqlx::PgPool, seed: &str, market_date: NaiveDate) -> Uuid {
    let (open, close, preopen, postclose) = session_bounds(market_date);
    let previous_trading_date = market_date - ChronoDuration::days(3);
    let operation_id = test_operation_id(&format!("{seed}|{market_date}"));
    let args = CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date,
        deployment_mode: "paper".to_string(),
        adapter_id: format!("lifecycle-test-{seed}"),
        session_plan_identity: format!("lifecycle-plan|{seed}|{market_date}"),
        assignment_identity: "lifecycle-assignment".to_string(),
        runtime_binding_identity: "lifecycle-binding".to_string(),
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
        occurred_at_utc: preopen,
        bounded_detail: "lifecycle test creation".to_string(),
        stop_attempt_count: 0,
    };
    match create_or_recover_autonomous_daily_operation(pool, &args)
        .await
        .expect("create operation")
    {
        CreateOrRecoverAutonomousDailyOperationOutcome::Created(r) => r.operation_id,
        CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(r) => r.operation_id,
        other => panic!("expected Created or Recovered, got {other:?}"),
    }
}

/// Advance a freshly seeded (`awaiting_preopen`) operation through the real
/// legal CAS chain to `running`, binding `run_id`. Returns the resulting
/// record (correct `state_version`).
async fn advance_to_running(
    pool: &sqlx::PgPool,
    operation_id: Uuid,
    run_id: Uuid,
    ts: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyOperationRecord> {
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id)
        .await?
        .expect("row must exist");
    let row = advance_one(pool, &row, STATE_PREPARING_DATA, ts).await?;
    let row = advance_one(pool, &row, STATE_AWAITING_OPEN, ts).await?;
    let row = advance_one(pool, &row, STATE_START_RETRYING, ts).await?;
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id: row.operation_id,
        expected_state: row.state.clone(),
        expected_state_version: row.state_version,
        new_state: STATE_RUNNING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: ts,
        run_id: Some(run_id),
        bounded_detail: "test setup: -> running".to_string(),
    };
    match transition_autonomous_daily_operation(pool, &args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => Ok(r),
        other => panic!("expected Applied, got {other:?}"),
    }
}

async fn advance_one(
    pool: &sqlx::PgPool,
    row: &AutonomousDailyOperationRecord,
    new_state: &str,
    ts: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyOperationRecord> {
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id: row.operation_id,
        expected_state: row.state.clone(),
        expected_state_version: row.state_version,
        new_state: new_state.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: ts,
        run_id: None,
        bounded_detail: format!("test setup: -> {new_state}"),
    };
    match transition_autonomous_daily_operation(pool, &args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => Ok(r),
        other => panic!("expected Applied transitioning to {new_state}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Migration / schema proof (D2.24 #1-#4)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn migration_0051_registered_exactly_once_immediately_after_0050() -> anyhow::Result<()> {
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
    let idx_0050 = ids
        .iter()
        .position(|id| id == "0050")
        .expect("0050 must be registered");
    let idx_0051 = ids
        .iter()
        .position(|id| id == "0051")
        .expect("0051 must be registered");
    assert_eq!(idx_0051, idx_0050 + 1, "0051 must immediately follow 0050");
    assert_eq!(
        ids.iter().filter(|id| id.as_str() == "0051").count(),
        1,
        "0051 must be registered exactly once"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn stop_retry_columns_exist_and_are_nullable() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    for (column, nullable) in [
        ("stop_attempt_count", "YES"),
        ("last_stop_attempt_utc", "YES"),
    ] {
        let row: (String,) = sqlx::query_as(
            "select is_nullable from information_schema.columns \
             where table_name = 'sys_autonomous_daily_operations' and column_name = $1",
        )
        .bind(column)
        .fetch_one(&pool)
        .await?;
        assert_eq!(row.0, nullable, "column {column} nullability mismatch");
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn new_row_stores_explicit_stop_attempt_zero() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "explicit-zero").await;

    let row: (Option<i64>,) = sqlx::query_as(
        "select stop_attempt_count from sys_autonomous_daily_operations where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        row.0,
        Some(0),
        "a freshly created row must store an explicit stop_attempt_count of 0, not null"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn legacy_row_predating_0051_round_trips_stop_evidence_as_null() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "legacy-null").await;

    // Simulate a row created before migration 0051: force the new columns
    // back to null, exactly as a genuinely legacy row would round-trip.
    sqlx::query(
        "update sys_autonomous_daily_operations \
         set stop_attempt_count = null, last_stop_attempt_utc = null \
         where operation_id = $1",
    )
    .bind(operation_id)
    .execute(&pool)
    .await?;

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.stop_attempt_count, None,
        "legacy-shaped row must round-trip stop_attempt_count as None, never fabricated as 0"
    );
    assert_eq!(row.last_stop_attempt_utc, None);

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// record_start_attempt (D2.24 #5-#6, #9)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn record_start_attempt_increments_atomically_and_sets_caller_timestamp() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "start-attempt-basic").await;
    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 13, 31, 0).unwrap();
    let next_retry = ts + ChronoDuration::seconds(30);

    match record_start_attempt(&pool, operation_id, ts, Some(next_retry)).await? {
        RecordStartAttemptOutcome::Recorded {
            start_attempt_count,
        } => assert_eq!(start_attempt_count, 1),
        other => panic!("expected Recorded, got {other:?}"),
    }

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.start_attempt_count, 1);
    assert_eq!(row.last_start_attempt_utc, Some(ts));
    assert_eq!(
        row.next_retry_utc,
        Some(next_retry),
        "next_retry_utc must be exactly the caller-supplied value"
    );

    match record_start_attempt(&pool, operation_id, ts + ChronoDuration::seconds(60), None).await? {
        RecordStartAttemptOutcome::Recorded {
            start_attempt_count,
        } => assert_eq!(
            start_attempt_count, 2,
            "a second call must increment by exactly one"
        ),
        other => panic!("expected Recorded, got {other:?}"),
    }

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn record_start_attempt_not_found_for_unknown_operation() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let unknown = Uuid::new_v4();
    match record_start_attempt(&pool, unknown, Utc::now(), None).await? {
        RecordStartAttemptOutcome::NotFound => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn concurrent_start_attempt_increments_are_not_lost() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "start-attempt-concurrent").await;
    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 13, 31, 0).unwrap();

    let mut handles = Vec::new();
    for _ in 0..10 {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            record_start_attempt(&pool, operation_id, ts, None)
                .await
                .expect("record_start_attempt failed")
        }));
    }
    for h in handles {
        h.await.expect("task panicked");
    }

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.start_attempt_count, 10,
        "10 concurrent increments must never be lost to a lost update"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// record_running_started (D2.24 #10-#11)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn record_running_started_sets_timestamp_clears_retry_and_error_idempotently(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "running-started").await;
    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 13, 31, 0).unwrap();

    record_retry_timing(
        &pool,
        operation_id,
        Some(ts + ChronoDuration::seconds(30)),
        Some("transient failure"),
        ts,
    )
    .await?;

    match record_running_started(&pool, operation_id, ts).await? {
        RecordRunningStartedOutcome::Recorded { started_at_utc } => {
            assert_eq!(started_at_utc, ts)
        }
        other => panic!("expected Recorded, got {other:?}"),
    }

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.started_at_utc, Some(ts));
    assert_eq!(row.next_retry_utc, None, "next_retry_utc must be cleared");
    assert_eq!(row.last_error, None, "last_error must be cleared");

    // Idempotent: a second call with a later timestamp must never rewind
    // the already-recorded started_at_utc.
    let later = ts + ChronoDuration::minutes(5);
    record_running_started(&pool, operation_id, later).await?;
    let row2 = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row2.started_at_utc,
        Some(ts),
        "started_at_utc must never rewind once recorded"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// record_retry_timing / clear_retry_timing (D2.24 #9)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn record_and_clear_retry_timing_uses_caller_supplied_values() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "retry-timing").await;
    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 13, 31, 0).unwrap();
    let next_retry = ts + ChronoDuration::seconds(60);

    match record_retry_timing(&pool, operation_id, Some(next_retry), Some("db blip"), ts).await? {
        RecordRetryTimingOutcome::Recorded => {}
        other => panic!("expected Recorded, got {other:?}"),
    }
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.next_retry_utc, Some(next_retry));
    assert_eq!(row.last_error.as_deref(), Some("db blip"));

    clear_retry_timing(&pool, operation_id, ts + ChronoDuration::seconds(1)).await?;
    let row2 = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row2.next_retry_utc, None);
    assert_eq!(row2.last_error, None);

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// record_stop_attempt (D2.24 #7-#8)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn record_stop_attempt_increments_from_explicit_zero() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "stop-attempt-basic").await;
    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 20, 1, 0).unwrap();

    match record_stop_attempt(&pool, operation_id, ts).await? {
        RecordStopAttemptOutcome::Recorded { stop_attempt_count } => {
            assert_eq!(stop_attempt_count, 1)
        }
        other => panic!("expected Recorded, got {other:?}"),
    }
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.stop_attempt_count, Some(1));
    assert_eq!(row.last_stop_attempt_utc, Some(ts));

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn record_stop_attempt_on_legacy_null_row_starts_counting_from_zero() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "stop-attempt-legacy").await;
    sqlx::query(
        "update sys_autonomous_daily_operations set stop_attempt_count = null where operation_id = $1",
    )
    .bind(operation_id)
    .execute(&pool)
    .await?;

    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 20, 1, 0).unwrap();
    match record_stop_attempt(&pool, operation_id, ts).await? {
        RecordStopAttemptOutcome::Recorded {
            stop_attempt_count,
        } => assert_eq!(
            stop_attempt_count, 1,
            "a legacy-null row's first recorded attempt must count from zero, never panic or fabricate prior history"
        ),
        other => panic!("expected Recorded, got {other:?}"),
    }

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn concurrent_stop_attempt_increments_are_not_lost() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "stop-attempt-concurrent").await;
    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 20, 1, 0).unwrap();

    let mut handles = Vec::new();
    for _ in 0..10 {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            record_stop_attempt(&pool, operation_id, ts)
                .await
                .expect("record_stop_attempt failed")
        }));
    }
    for h in handles {
        h.await.expect("task panicked");
    }

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.stop_attempt_count,
        Some(10),
        "10 concurrent increments must never be lost to a lost update"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// record_stopped_at (D2.24 #12)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn record_stopped_at_is_idempotent_and_never_rewinds() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "stopped-at").await;
    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 20, 1, 0).unwrap();

    match record_stopped_at(&pool, operation_id, ts).await? {
        RecordStoppedAtOutcome::Recorded { stopped_at_utc } => assert_eq!(stopped_at_utc, ts),
        other => panic!("expected Recorded, got {other:?}"),
    }

    let later = ts + ChronoDuration::minutes(2);
    match record_stopped_at(&pool, operation_id, later).await? {
        RecordStoppedAtOutcome::Recorded { stopped_at_utc } => assert_eq!(
            stopped_at_utc, ts,
            "a second call must never rewind the already-recorded stopped_at_utc"
        ),
        other => panic!("expected Recorded, got {other:?}"),
    }

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Counter-only updates never touch state_version or the events table
// (D2.24 #14)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn evidence_recorders_never_insert_transition_events_or_bump_state_version(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "no-transition-events").await;
    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 13, 31, 0).unwrap();

    let before = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let events_before = list_autonomous_daily_operation_events(&pool, operation_id, 100).await?;

    record_start_attempt(&pool, operation_id, ts, None).await?;
    record_running_started(&pool, operation_id, ts).await?;
    record_retry_timing(&pool, operation_id, None, None, ts).await?;
    record_stop_attempt(&pool, operation_id, ts).await?;
    record_stopped_at(&pool, operation_id, ts).await?;

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let events_after = list_autonomous_daily_operation_events(&pool, operation_id, 100).await?;

    assert_eq!(
        before.state_version, after.state_version,
        "counter-only evidence recorders must never bump state_version"
    );
    assert_eq!(
        events_before.len(),
        events_after.len(),
        "counter-only evidence recorders must never insert a transition event"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-LIFECYCLE-CLOSURE-REPAIR-01
// Migration 0052 / durable blocker signature
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn migration_0052_registered_exactly_once_immediately_after_0051() -> anyhow::Result<()> {
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
    let idx_0051 = ids
        .iter()
        .position(|id| id == "0051")
        .expect("0051 must be registered");
    let idx_0052 = ids
        .iter()
        .position(|id| id == "0052")
        .expect("0052 must be registered");
    assert_eq!(idx_0052, idx_0051 + 1, "0052 must immediately follow 0051");
    assert_eq!(
        ids.iter().filter(|id| id.as_str() == "0052").count(),
        1,
        "0052 must be registered exactly once"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn state_blocker_signature_column_exists_and_is_nullable() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let row: (String,) = sqlx::query_as(
        "select is_nullable from information_schema.columns \
         where table_name = 'sys_autonomous_daily_operations' and column_name = 'state_blocker_signature'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(row.0, "YES");
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn new_row_stores_explicit_null_blocker_signature() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "blocker-sig-new-null").await;

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.state_blocker_signature, None,
        "a freshly created row must store an explicit null blocker signature"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn legacy_row_round_trips_null_blocker_signature() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "blocker-sig-legacy").await;
    sqlx::query(
        "update sys_autonomous_daily_operations set state_blocker_signature = null \
         where operation_id = $1",
    )
    .bind(operation_id)
    .execute(&pool)
    .await?;

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.state_blocker_signature, None,
        "a legacy-shaped row must round-trip state_blocker_signature as None, never fabricated"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn manual_transition_stores_reason_and_signature_atomically() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "blocker-sig-manual").await;
    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 13, 31, 0).unwrap();

    let args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: STATE_AWAITING_PREOPEN.to_string(),
        expected_state_version: 1,
        new_state: STATE_MANUAL_INTERVENTION_REQUIRED.to_string(),
        reason_code: Some("assignment_missing".to_string()),
        blocker_signature: Some("sha256:deadbeef".to_string()),
        occurred_at_utc: ts,
        run_id: None,
        bounded_detail: "manual blocker with signature".to_string(),
    };
    match transition_autonomous_daily_operation(&pool, &args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => {
            assert_eq!(r.state_reason_code.as_deref(), Some("assignment_missing"));
            assert_eq!(
                r.state_blocker_signature.as_deref(),
                Some("sha256:deadbeef")
            );
        }
        other => panic!("expected Applied, got {other:?}"),
    }

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.state_reason_code.as_deref(), Some("assignment_missing"));
    assert_eq!(
        row.state_blocker_signature.as_deref(),
        Some("sha256:deadbeef")
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn nonblocked_transition_clears_stale_blocker_signature() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "blocker-sig-clear").await;
    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 13, 31, 0).unwrap();

    let blocked_args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: STATE_AWAITING_PREOPEN.to_string(),
        expected_state_version: 1,
        new_state: STATE_MANUAL_INTERVENTION_REQUIRED.to_string(),
        reason_code: Some("assignment_missing".to_string()),
        blocker_signature: Some("sha256:deadbeef".to_string()),
        occurred_at_utc: ts,
        run_id: None,
        bounded_detail: "manual blocker with signature".to_string(),
    };
    let blocked = match transition_autonomous_daily_operation(&pool, &blocked_args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => r,
        other => panic!("expected Applied, got {other:?}"),
    };
    assert!(blocked.state_blocker_signature.is_some());

    // A legal forward edge out of manual_intervention_required (back into
    // awaiting_preopen) that carries no reason/signature must clear the
    // stale blocker signature, never leave it dangling.
    let recovery_args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: STATE_MANUAL_INTERVENTION_REQUIRED.to_string(),
        expected_state_version: blocked.state_version,
        new_state: STATE_AWAITING_PREOPEN.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: ts,
        run_id: None,
        bounded_detail: "condition cleared".to_string(),
    };
    match transition_autonomous_daily_operation(&pool, &recovery_args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => {
            assert_eq!(r.state_reason_code, None);
            assert_eq!(
                r.state_blocker_signature, None,
                "a non-blocked forward transition must clear a stale blocker signature"
            );
        }
        other => panic!("expected Applied, got {other:?}"),
    }

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-LIFECYCLE-CLOSURE-REPAIR-01
// Atomic running transition (transition_autonomous_daily_operation_to_running)
// ---------------------------------------------------------------------------

/// Advance a freshly seeded (`awaiting_preopen`) operation to
/// `start_retrying` via the exact real legal edges
/// (`awaiting_preopen -> preparing_data -> awaiting_open -> start_retrying`),
/// proving the fixture used by the running-transition tests below is a
/// durably legal prior state, not a fabricated one.
async fn advance_operation_to_start_retrying(
    pool: &sqlx::PgPool,
    operation_id: Uuid,
    ts: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyOperationRecord> {
    async fn step(
        pool: &sqlx::PgPool,
        operation_id: Uuid,
        expected_state: &str,
        expected_state_version: i64,
        new_state: &str,
        ts: DateTime<Utc>,
    ) -> anyhow::Result<AutonomousDailyOperationRecord> {
        let args = TransitionAutonomousDailyOperationArgs {
            operation_id,
            expected_state: expected_state.to_string(),
            expected_state_version,
            new_state: new_state.to_string(),
            reason_code: None,
            blocker_signature: None,
            occurred_at_utc: ts,
            run_id: None,
            bounded_detail: "advance to start_retrying".to_string(),
        };
        match transition_autonomous_daily_operation(pool, &args).await? {
            AutonomousDailyTransitionOutcome::Applied(r) => Ok(r),
            other => anyhow::bail!("expected Applied, got {other:?}"),
        }
    }
    let r1 = step(
        pool,
        operation_id,
        STATE_AWAITING_PREOPEN,
        1,
        STATE_PREPARING_DATA,
        ts,
    )
    .await?;
    let r2 = step(
        pool,
        operation_id,
        STATE_PREPARING_DATA,
        r1.state_version,
        STATE_AWAITING_OPEN,
        ts,
    )
    .await?;
    step(
        pool,
        operation_id,
        STATE_AWAITING_OPEN,
        r2.state_version,
        STATE_START_RETRYING,
        ts,
    )
    .await
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn transition_to_running_is_atomic_state_runid_started_and_retry_clear() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "to-running-atomic").await;
    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 13, 31, 0).unwrap();
    let current = advance_operation_to_start_retrying(&pool, operation_id, ts).await?;
    record_retry_timing(
        &pool,
        operation_id,
        Some(ts + ChronoDuration::seconds(30)),
        Some("prior transient failure"),
        ts,
    )
    .await?;

    let run_id = Uuid::new_v4();
    let to_running_args = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id,
        expected_state: current.state.clone(),
        expected_state_version: current.state_version,
        run_id,
        started_at_utc: ts,
        occurred_at_utc: ts,
        bounded_detail: "atomic running proof".to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation_to_running(&pool, &to_running_args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => {
            assert_eq!(r.state, STATE_RUNNING);
            assert_eq!(r.run_id, Some(run_id));
            assert_eq!(r.started_at_utc, Some(ts));
            assert_eq!(r.next_retry_utc, None, "next_retry_utc must be cleared");
            assert_eq!(r.last_error, None, "last_error must be cleared");
            assert_eq!(r.state_reason_code, None);
            assert_eq!(r.state_blocker_signature, None);
            assert_eq!(r.state_version, current.state_version + 1);
        }
        other => panic!("expected Applied, got {other:?}"),
    }

    let events = list_autonomous_daily_operation_events(&pool, operation_id, 100).await?;
    let last = events.last().expect("at least one event must exist");
    assert_eq!(last.to_state, STATE_RUNNING);
    assert_eq!(last.run_id, Some(run_id));
    assert_eq!(
        last.transition_seq,
        current.state_version + 1,
        "exactly one matching event must be inserted at the new state_version"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn transition_to_running_forced_event_insert_failure_rolls_back_everything(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "to-running-rollback").await;
    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 13, 31, 0).unwrap();
    let current = advance_operation_to_start_retrying(&pool, operation_id, ts).await?;

    // Pre-seed a conflicting event row at the exact transition_seq the
    // running transition would use, forcing its INSERT to violate the
    // primary key and the whole transaction to roll back.
    sqlx::query(
        "insert into sys_autonomous_daily_operation_events \
         (operation_id, transition_seq, from_state, to_state, reason_code, occurred_at_utc, \
          run_id, bounded_detail) \
         values ($1, $2, $3, $4, null, $5, null, 'pre-seeded conflict')",
    )
    .bind(operation_id)
    .bind(current.state_version + 1)
    .bind(&current.state)
    .bind(STATE_RUNNING)
    .bind(ts)
    .execute(&pool)
    .await?;

    let run_id = Uuid::new_v4();
    let to_running_args = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id,
        expected_state: current.state.clone(),
        expected_state_version: current.state_version,
        run_id,
        started_at_utc: ts,
        occurred_at_utc: ts,
        bounded_detail: "forced rollback proof".to_string(),
    };
    let result =
        mqk_db::transition_autonomous_daily_operation_to_running(&pool, &to_running_args).await;
    assert!(
        result.is_err(),
        "the conflicting event insert must fail the whole transaction"
    );

    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        row.state, current.state,
        "state must roll back to its pre-attempt value"
    );
    assert_eq!(
        row.state_version, current.state_version,
        "state_version must not be incremented on a rolled-back transaction"
    );
    assert_eq!(
        row.run_id, None,
        "run_id must never be adopted on a rolled-back transaction"
    );
    assert_eq!(
        row.started_at_utc, None,
        "started_at_utc must never be set on a rolled-back transaction"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn concurrent_transitions_to_running_produce_exactly_one_applied() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "to-running-concurrent").await;
    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 13, 31, 0).unwrap();
    let current = advance_operation_to_start_retrying(&pool, operation_id, ts).await?;

    let mut handles = Vec::new();
    for _ in 0..5 {
        let pool = pool.clone();
        let expected_state = current.state.clone();
        let expected_state_version = current.state_version;
        handles.push(tokio::spawn(async move {
            let args = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
                operation_id,
                expected_state,
                expected_state_version,
                run_id: Uuid::new_v4(),
                started_at_utc: ts,
                occurred_at_utc: ts,
                bounded_detail: "race".to_string(),
            };
            mqk_db::transition_autonomous_daily_operation_to_running(&pool, &args).await
        }));
    }
    let mut applied_count = 0;
    for h in handles {
        if let Ok(Ok(AutonomousDailyTransitionOutcome::Applied(_))) = h.await {
            applied_count += 1;
        }
    }
    assert_eq!(
        applied_count, 1,
        "exactly one concurrent running transition must be applied"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-LIFECYCLE-CLOSURE-REPAIR-01
// record_autonomous_runtime_stopped
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn record_autonomous_runtime_stopped_clears_retry_state_atomically_and_is_idempotent(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "runtime-stopped").await;
    let ts = Utc.with_ymd_and_hms(2026, 7, 20, 20, 1, 0).unwrap();
    record_retry_timing(
        &pool,
        operation_id,
        Some(ts + ChronoDuration::seconds(30)),
        Some("stop attempt failed"),
        ts,
    )
    .await?;

    match mqk_db::record_autonomous_runtime_stopped(&pool, operation_id, ts).await? {
        mqk_db::RecordAutonomousRuntimeStoppedOutcome::Recorded { stopped_at_utc } => {
            assert_eq!(stopped_at_utc, ts)
        }
        other => panic!("expected Recorded, got {other:?}"),
    }
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.stopped_at_utc, Some(ts));
    assert_eq!(row.next_retry_utc, None, "next_retry_utc must be cleared");
    assert_eq!(row.last_error, None, "last_error must be cleared");

    // Idempotent: a second call with a later timestamp must never rewind
    // the already-recorded stopped_at_utc.
    let later = ts + ChronoDuration::minutes(2);
    match mqk_db::record_autonomous_runtime_stopped(&pool, operation_id, later).await? {
        mqk_db::RecordAutonomousRuntimeStoppedOutcome::Recorded { stopped_at_utc } => {
            assert_eq!(
                stopped_at_utc, ts,
                "a second call must never rewind the already-recorded stopped_at_utc"
            )
        }
        other => panic!("expected Recorded, got {other:?}"),
    }

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-FAILSAFE-RECOVERY-CLOSURE-01
// REPAIR 1: fetch_relevant_open_autonomous_daily_operation
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_selects_the_authoritative_current_operation() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let operation_id = seed_operation_for_date(&pool, "relopen-a", market_date).await;
    let run_id = Uuid::new_v4();
    let ts = session_bounds(market_date).0;
    let running = advance_to_running(&pool, operation_id, run_id, ts).await?;
    assert_eq!(running.state, STATE_RUNNING);

    // `running` is in the active-lifecycle-state set, so any `now_utc` --
    // even far outside the operation's own window -- must still find it.
    let far_future = ts + ChronoDuration::days(30);
    let found = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-a",
        far_future,
    )
    .await?
    .expect("the running operation must be found");
    assert_eq!(found.operation_id, operation_id);

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_ignores_terminal_historical_rows() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let operation_id = seed_operation_for_date(&pool, "relopen-terminal", market_date).await;
    let ts = session_bounds(market_date).0;
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let row = advance_one(&pool, &row, STATE_STOPPING, ts).await?;
    let stopped = advance_one(&pool, &row, mqk_db::STATE_COMPLETED_NO_TRADE, ts).await?;
    assert_eq!(stopped.state, mqk_db::STATE_COMPLETED_NO_TRADE);

    // Even `now_utc` squarely inside the operation's own persisted window
    // must not resurrect a terminal row.
    let inside_window = ts + ChronoDuration::hours(1);
    let found = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-terminal",
        inside_window,
    )
    .await?;
    assert!(found.is_none(), "a terminal row must never be relevant");

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stale_manual_row_cannot_shadow_running_current_row(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();

    // A stale historical row: manual_intervention_required (not an active-
    // lifecycle state) from a prior day, whose own window does not include
    // `now_utc` used below.
    let stale_id = seed_operation_for_date(&pool, "relopen-stale", yesterday).await;
    let stale_ts = session_bounds(yesterday).0;
    let stale_row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, stale_id)
        .await?
        .expect("row must exist");
    advance_one(
        &pool,
        &stale_row,
        STATE_MANUAL_INTERVENTION_REQUIRED,
        stale_ts,
    )
    .await?;

    // Today's current row: running.
    let current_id = seed_operation_for_date(&pool, "relopen-stale", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let found = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-stale",
        current_ts,
    )
    .await?
    .expect("the running current row must be found");
    assert_eq!(
        found.operation_id, current_id,
        "a stale historical manual row must never shadow the running current row"
    );

    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-STALE-EVIDENCE-DEGRADED-AMBIGUITY-SCOPING-01: a prior-
// market-date `evidence_degraded` row that never obtained runtime/execution
// authority must not durably shadow a later date's real operation -- but an
// `evidence_degraded` row carrying genuine run/execution evidence, or one
// from the *current* market date, must still block exactly as
// conservatively as `manual_intervention_required` already does.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stale_no_run_evidence_degraded_row_cannot_shadow_running_current_row(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();

    // A stale historical row reaching evidence_degraded via the real
    // production path for "session closed before any runtime ever started"
    // (awaiting_preopen -> stopping -> evidence_degraded), with zero run
    // evidence: no run_id, no started_at_utc, bars_dispatched = 0.
    let stale_id = seed_operation_for_date(&pool, "relopen-ed-stale", yesterday).await;
    let stale_ts = session_bounds(yesterday).0;
    let stale_row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, stale_id)
        .await?
        .expect("row must exist");
    let stale_row = advance_one(&pool, &stale_row, STATE_STOPPING, stale_ts).await?;
    record_stopped_at(&pool, stale_id, stale_ts).await?;
    let stale_row = advance_one(
        &pool,
        &stale_row,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        stale_ts,
    )
    .await?;
    assert_eq!(stale_row.state, mqk_db::STATE_EVIDENCE_DEGRADED);
    assert!(stale_row.run_id.is_none(), "fixture precondition: no run ever started");

    // Today's current row: running.
    let current_id = seed_operation_for_date(&pool, "relopen-ed-stale", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let found = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-ed-stale",
        current_ts,
    )
    .await?
    .expect("the running current row must be found");
    assert_eq!(
        found.operation_id, current_id,
        "a stale no-run evidence_degraded row must never shadow the running current row"
    );

    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_evidence_degraded_row_with_run_evidence_still_blocks(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();

    // A prior-day row that DID obtain a real run (the "mid-run degrade"
    // shape: running -> evidence_degraded directly, never touching
    // stopping) -- genuine unresolved execution evidence must still fail
    // closed regardless of date, exactly like an unstopped bound run
    // already does via the `run_id is not null and stopped_at_utc is null`
    // clause.
    let stale_id = seed_operation_for_date(&pool, "relopen-ed-active", yesterday).await;
    let stale_ts = session_bounds(yesterday).0;
    let stale_run_id = Uuid::new_v4();
    let running = advance_to_running(&pool, stale_id, stale_run_id, stale_ts).await?;
    let degraded = advance_one(
        &pool,
        &running,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        stale_ts,
    )
    .await?;
    assert_eq!(degraded.state, mqk_db::STATE_EVIDENCE_DEGRADED);
    assert!(degraded.run_id.is_some(), "fixture precondition: a real run was bound");

    let current_id = seed_operation_for_date(&pool, "relopen-ed-active", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let result = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-ed-active",
        current_ts,
    )
    .await;
    assert!(
        result.is_err(),
        "an evidence_degraded row with real run evidence must still fail closed as ambiguous, \
         even from a prior date; got {result:?}"
    );

    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_same_day_no_run_evidence_degraded_still_found() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();

    // A same-day evidence_degraded row with zero run evidence must still be
    // found via the date-window clause -- this route's narrowing must never
    // silently ignore evidence_degraded outright, only date-scope it the
    // same way manual_intervention_required is already scoped.
    let operation_id = seed_operation_for_date(&pool, "relopen-ed-sameday", today).await;
    let ts = session_bounds(today).0;
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let row = advance_one(&pool, &row, STATE_STOPPING, ts).await?;
    record_stopped_at(&pool, operation_id, ts).await?;
    let degraded = advance_one(&pool, &row, mqk_db::STATE_EVIDENCE_DEGRADED, ts).await?;
    assert!(degraded.run_id.is_none(), "fixture precondition: no run ever started");

    let inside_window = ts + ChronoDuration::hours(1);
    let found = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-ed-sameday",
        inside_window,
    )
    .await?
    .expect("a same-day evidence_degraded row must still be found while inside its own window");
    assert_eq!(found.operation_id, operation_id);

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-EVIDENCE-DEGRADED-LIFECYCLE-AUTHORITY-SEPARATION-01:
// a bound run_id alone is not proof of unresolved economic authority --
// only a genuinely terminal run with zero unresolved outbox/reconcile
// evidence may be released from unconditional relevance. These three
// tests exercise exactly the proof-based checks added to this query
// (a real `runs` row, driven through insert_run/arm_run/begin_run/
// stop_run -- never a raw UPDATE).
// ---------------------------------------------------------------------------

async fn reset_reconcile_status_clean(pool: &sqlx::PgPool, now: DateTime<Utc>) -> anyhow::Result<()> {
    persist_reconcile_status_state(
        pool,
        &PersistReconcileStatusState {
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
    .await
}

fn new_run_fixture(run_id: Uuid, now: DateTime<Utc>) -> NewRun {
    NewRun {
        run_id,
        engine_id: "mqk-daemon".to_string(),
        mode: "PAPER".to_string(),
        started_at_utc: now,
        git_hash: "TEST".to_string(),
        config_hash: "TEST".to_string(),
        config_json: serde_json::json!({}),
        host_fingerprint: "test-host".to_string(),
    }
}

/// Seed an evidence_degraded operation, from yesterday, bound to a real
/// `runs` row -- reaching evidence_degraded via the real "mid-run degrade"
/// path (`running -> evidence_degraded` directly) and then, separately,
/// recording `stopped_at_utc` via `record_stopped_at` (mirroring
/// `reconcile_durable_run_without_local_owner`'s own terminal-branch call)
/// -- so the fixture matches exactly what AUTONOMOUS-DAILY-CONTROLLER-
/// DEGRADED-RECOVERY-01 produces in production for a `controller_degraded`
/// operation whose run turned out to be safely stopped.
async fn seed_stopped_evidence_degraded_with_run(
    pool: &sqlx::PgPool,
    seed: &str,
    market_date: NaiveDate,
    run_id: Uuid,
) -> anyhow::Result<Uuid> {
    let (open, _close, _preopen, _postclose) = session_bounds(market_date);
    let operation_id = seed_operation_for_date(pool, seed, market_date).await;
    let running = advance_to_running(pool, operation_id, run_id, open).await?;
    let degraded_args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: running.state.clone(),
        expected_state_version: running.state_version,
        new_state: mqk_db::STATE_EVIDENCE_DEGRADED.to_string(),
        reason_code: Some("unknown_incomplete_bar_coverage".to_string()),
        blocker_signature: None,
        occurred_at_utc: open,
        run_id: None,
        bounded_detail: "test setup: -> evidence_degraded (unknown_incomplete_bar_coverage)"
            .to_string(),
    };
    let degraded = match transition_autonomous_daily_operation(pool, &degraded_args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => r,
        other => panic!("expected Applied, got {other:?}"),
    };
    assert_eq!(degraded.state, mqk_db::STATE_EVIDENCE_DEGRADED);
    record_stopped_at(pool, operation_id, open).await?;
    Ok(operation_id)
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stopped_run_with_unacked_outbox_still_blocks() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    reset_reconcile_status_clean(&pool, Utc::now()).await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();

    let (open, _, _, _) = session_bounds(yesterday);
    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?; // genuinely STOPPED

    let stale_id =
        seed_stopped_evidence_degraded_with_run(&pool, "relopen-ed-outbox", yesterday, run_id)
            .await?;

    // A SENT-but-not-yet-ACKED order still associated with the now-stopped
    // run -- an order may still be in flight; this must never be silently
    // released.
    sqlx::query(
        "insert into oms_outbox (run_id, idempotency_key, order_json, status, created_at_utc, \
         sent_at_utc) values ($1, $2, '{}'::jsonb, 'SENT', $3, $3)",
    )
    .bind(run_id)
    .bind(format!("test-unresolved-{}", unique_suffix()))
    .bind(open)
    .execute(&pool)
    .await?;

    let current_id = seed_operation_for_date(&pool, "relopen-ed-outbox", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let result = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-ed-outbox",
        current_ts,
    )
    .await;
    assert!(
        result.is_err(),
        "a stopped run with an unacked outbox row must still fail closed as ambiguous; \
         got {result:?}"
    );

    let _ = sqlx::query("delete from oms_outbox where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await;
    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stopped_run_with_dirty_reconcile_still_blocks() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();

    let (open, _, _, _) = session_bounds(yesterday);
    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?;

    let stale_id =
        seed_stopped_evidence_degraded_with_run(&pool, "relopen-ed-dirty", yesterday, run_id)
            .await?;

    persist_reconcile_status_state(
        &pool,
        &PersistReconcileStatusState {
            status: "dirty",
            last_run_at_utc: Some(open),
            snapshot_watermark_ms: None,
            mismatched_positions: 1,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: Some("test: simulated position disagreement"),
            updated_at_utc: open,
        },
    )
    .await?;

    let current_id = seed_operation_for_date(&pool, "relopen-ed-dirty", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let result = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-ed-dirty",
        current_ts,
    )
    .await;
    assert!(
        result.is_err(),
        "a stopped run with a dirty global reconcile status must still fail closed as \
         ambiguous; got {result:?}"
    );

    reset_reconcile_status_clean(&pool, current_ts).await?;
    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// PAPER-SOAK-UNRESOLVED-BROKER-EVIDENCE-GATE-01: an unapplied `oms_inbox`
// row (durably received broker evidence not yet applied) and a nonzero
// `unmatched_broker_events` reconcile counter must both block release
// exactly like an unacked outbox row / dirty reconcile status already do.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stopped_run_with_unapplied_inbox_still_blocks() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    reset_reconcile_status_clean(&pool, Utc::now()).await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();

    let (open, _, _, _) = session_bounds(yesterday);
    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?; // genuinely STOPPED

    let stale_id =
        seed_stopped_evidence_degraded_with_run(&pool, "relopen-ed-inbox", yesterday, run_id)
            .await?;

    // Broker evidence durably received but not yet applied -- exactly the
    // crash-recovery-replay shape that must never be silently released.
    mqk_db::inbox_insert_deduped(
        &pool,
        run_id,
        &format!("test-unapplied-{}", unique_suffix()),
        serde_json::json!({"event_kind": "fill"}),
    )
    .await?;

    let current_id = seed_operation_for_date(&pool, "relopen-ed-inbox", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let result = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-ed-inbox",
        current_ts,
    )
    .await;
    assert!(
        result.is_err(),
        "a stopped run with an unapplied inbox row must still fail closed as ambiguous; \
         got {result:?}"
    );

    let _ = sqlx::query("delete from oms_inbox where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await;
    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stopped_run_with_unmatched_broker_events_still_blocks(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();

    let (open, _, _, _) = session_bounds(yesterday);
    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?;

    let stale_id = seed_stopped_evidence_degraded_with_run(
        &pool,
        "relopen-ed-unmatched",
        yesterday,
        run_id,
    )
    .await?;

    // status="ok" and every mismatch counter is zero, but
    // unmatched_broker_events is nonzero -- must still be treated as dirty.
    persist_reconcile_status_state(
        &pool,
        &PersistReconcileStatusState {
            status: "ok",
            last_run_at_utc: Some(open),
            snapshot_watermark_ms: None,
            mismatched_positions: 0,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 1,
            note: Some("test: simulated unmatched broker event"),
            updated_at_utc: open,
        },
    )
    .await?;

    let current_id = seed_operation_for_date(&pool, "relopen-ed-unmatched", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let result = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-ed-unmatched",
        current_ts,
    )
    .await;
    assert!(
        result.is_err(),
        "a stopped run with unmatched_broker_events != 0 must still fail closed as ambiguous; \
         got {result:?}"
    );

    reset_reconcile_status_clean(&pool, current_ts).await?;
    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stopped_run_zero_activity_clean_reconcile_is_released(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    reset_reconcile_status_clean(&pool, Utc::now()).await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();

    let (open, _, _, _) = session_bounds(yesterday);
    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?; // genuinely STOPPED, zero orders ever created

    let stale_id =
        seed_stopped_evidence_degraded_with_run(&pool, "relopen-ed-released", yesterday, run_id)
            .await?;

    let current_id = seed_operation_for_date(&pool, "relopen-ed-released", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let found = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-ed-released",
        current_ts,
    )
    .await?
    .expect("the running current row must be found");
    assert_eq!(
        found.operation_id, current_id,
        "a proven-terminal, zero-activity, clean-reconcile evidence_degraded row must be \
         released and never shadow the current running operation"
    );

    // The stale row's own evidence classification must remain completely
    // untouched -- this repair separates lifecycle authority from evidence
    // truth, it never rewrites history.
    let stale_row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, stale_id)
        .await?
        .expect("row must exist");
    assert_eq!(stale_row.state, mqk_db::STATE_EVIDENCE_DEGRADED);
    assert_eq!(
        stale_row.state_reason_code.as_deref(),
        Some("unknown_incomplete_bar_coverage")
    );

    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_multiple_active_rows_fail_closed() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let date_a = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let date_b = NaiveDate::from_ymd_opt(2026, 7, 21).unwrap();

    let id_a = seed_operation_for_date(&pool, "relopen-multi", date_a).await;
    let id_b = seed_operation_for_date(&pool, "relopen-multi", date_b).await;
    let ts_a = session_bounds(date_a).0;
    let ts_b = session_bounds(date_b).0;
    advance_to_running(&pool, id_a, Uuid::new_v4(), ts_a).await?;
    advance_to_running(&pool, id_b, Uuid::new_v4(), ts_b).await?;

    let result = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-multi",
        ts_a,
    )
    .await;
    assert!(
        result.is_err(),
        "multiple equally authoritative active rows must fail closed, never guess"
    );

    cleanup_operation(&pool, id_a).await;
    cleanup_operation(&pool, id_b).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// PAPER-SOAK-STALE-STOPPING-RELEASE-01: `stopping` had no release path at
// all -- unlike `evidence_degraded`, which AUTONOMOUS-DAILY-EVIDENCE-
// DEGRADED-LIFECYCLE-AUTHORITY-SEPARATION-01 already gave a proof-based
// release clause. A run whose canonical stop path genuinely completed
// (`stopped_at_utc` set via `record_autonomous_runtime_stopped`, the bound
// `runs` row STOPPED, zero unacked outbox, clean global reconcile) but
// whose CAS state transition never advanced past `stopping` -- exactly the
// shape `stopping -> evidence_degraded -> stopping` reconciliation leaves
// behind when its last-ever transition lands back on `stopping` -- stayed
// unconditionally relevant forever, permanently shadowing every later
// date's real operation from `fetch_relevant_open_autonomous_daily_operation`
// and, in production, silently starving the autonomous completed-bar driver
// of a resolvable "current operation" on every subsequent day.
// ---------------------------------------------------------------------------

/// Seed a `stopping` operation, from `market_date`, bound to a real `runs`
/// row that is genuinely `STOPPED`, with `stopped_at_utc` recorded via the
/// same canonical recorder production uses on the
/// `reconcile_durable_run_without_local_owner` terminal branch
/// (`record_autonomous_runtime_stopped`).
async fn seed_stopped_stopping_with_run(
    pool: &sqlx::PgPool,
    seed: &str,
    market_date: NaiveDate,
    run_id: Uuid,
) -> anyhow::Result<Uuid> {
    let (open, _close, _preopen, _postclose) = session_bounds(market_date);
    let operation_id = seed_operation_for_date(pool, seed, market_date).await;
    let running = advance_to_running(pool, operation_id, run_id, open).await?;
    let stopping = advance_one(pool, &running, STATE_STOPPING, open).await?;
    assert_eq!(stopping.state, STATE_STOPPING);
    mqk_db::record_autonomous_runtime_stopped(pool, operation_id, open).await?;
    Ok(operation_id)
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stopped_run_zero_activity_clean_reconcile_stopping_is_released(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    reset_reconcile_status_clean(&pool, Utc::now()).await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();

    let (open, _, _, _) = session_bounds(yesterday);
    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?; // genuinely STOPPED, zero orders ever created

    let stale_id =
        seed_stopped_stopping_with_run(&pool, "relopen-stopping-released", yesterday, run_id)
            .await?;

    let current_id = seed_operation_for_date(&pool, "relopen-stopping-released", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    // RED before the repair: this call returns `Err` ("2 equally
    // authoritative active operations found"), reproducing the real
    // Tuesday-stopping-blocks-Wednesday production defect. GREEN after the
    // repair: the stale stopping row is released and only the current
    // running operation is found.
    let found = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-stopping-released",
        current_ts,
    )
    .await?
    .expect(
        "a proven-terminal, zero-activity, clean-reconcile stopping row must be released and \
         never shadow the current running operation",
    );
    assert_eq!(
        found.operation_id, current_id,
        "the stale stopping row must not shadow the current running operation"
    );

    // The stale row's own state must remain completely untouched -- this
    // repair separates lifecycle authority from evidence truth, it never
    // rewrites history.
    let stale_row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, stale_id)
        .await?
        .expect("row must exist");
    assert_eq!(stale_row.state, STATE_STOPPING);

    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stopping_row_with_unacked_outbox_still_blocks() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    reset_reconcile_status_clean(&pool, Utc::now()).await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();

    let (open, _, _, _) = session_bounds(yesterday);
    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?;

    let stale_id =
        seed_stopped_stopping_with_run(&pool, "relopen-stopping-outbox", yesterday, run_id)
            .await?;

    // A SENT-but-not-yet-ACKED order still associated with the now-stopped
    // run -- an order may still be in flight; this must never be silently
    // released, exactly like the existing evidence_degraded proof.
    sqlx::query(
        "insert into oms_outbox (run_id, idempotency_key, order_json, status, created_at_utc, \
         sent_at_utc) values ($1, $2, '{}'::jsonb, 'SENT', $3, $3)",
    )
    .bind(run_id)
    .bind(format!("test-unresolved-{}", unique_suffix()))
    .bind(open)
    .execute(&pool)
    .await?;

    let current_id = seed_operation_for_date(&pool, "relopen-stopping-outbox", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let result = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-stopping-outbox",
        current_ts,
    )
    .await;
    assert!(
        result.is_err(),
        "a stopping row whose run has an unacked outbox row must still fail closed as \
         ambiguous; got {result:?}"
    );

    let _ = sqlx::query("delete from oms_outbox where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await;
    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stopping_row_with_dirty_reconcile_still_blocks() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();

    let (open, _, _, _) = session_bounds(yesterday);
    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?;

    let stale_id =
        seed_stopped_stopping_with_run(&pool, "relopen-stopping-dirty", yesterday, run_id).await?;

    persist_reconcile_status_state(
        &pool,
        &PersistReconcileStatusState {
            status: "dirty",
            last_run_at_utc: Some(open),
            snapshot_watermark_ms: None,
            mismatched_positions: 1,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: Some("test: simulated position disagreement"),
            updated_at_utc: open,
        },
    )
    .await?;

    let current_id = seed_operation_for_date(&pool, "relopen-stopping-dirty", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let result = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-stopping-dirty",
        current_ts,
    )
    .await;
    assert!(
        result.is_err(),
        "a stopping row whose run stopped with a dirty global reconcile status must still fail \
         closed as ambiguous; got {result:?}"
    );

    reset_reconcile_status_clean(&pool, current_ts).await?;
    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// PAPER-SOAK-UNRESOLVED-BROKER-EVIDENCE-GATE-01: `stopping` shares the exact
// evidence-gated release clause -- an unapplied inbox row must block it too.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stopping_row_with_unapplied_inbox_still_blocks() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    reset_reconcile_status_clean(&pool, Utc::now()).await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();

    let (open, _, _, _) = session_bounds(yesterday);
    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?;

    let stale_id =
        seed_stopped_stopping_with_run(&pool, "relopen-stopping-inbox", yesterday, run_id).await?;

    mqk_db::inbox_insert_deduped(
        &pool,
        run_id,
        &format!("test-unapplied-{}", unique_suffix()),
        serde_json::json!({"event_kind": "fill"}),
    )
    .await?;

    let current_id = seed_operation_for_date(&pool, "relopen-stopping-inbox", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let result = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-stopping-inbox",
        current_ts,
    )
    .await;
    assert!(
        result.is_err(),
        "a stopping row whose run stopped with an unapplied inbox row must still fail closed \
         as ambiguous; got {result:?}"
    );

    let _ = sqlx::query("delete from oms_inbox where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await;
    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_same_day_stopping_with_stopped_run_still_found() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    reset_reconcile_status_clean(&pool, Utc::now()).await?;
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();
    let (open, _, _, _) = session_bounds(today);

    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?;

    let operation_id =
        seed_stopped_stopping_with_run(&pool, "relopen-stopping-sameday", today, run_id).await?;

    // Same-day stopping row, even with fully proven-safe release evidence,
    // must still be found while `now_utc` falls inside its own persisted
    // window -- release only ever applies once the operation's own market
    // date has passed, exactly mirroring evidence_degraded's existing
    // same-day protection. This is the direct proof that no
    // active/running/stopping operation still owning today's runtime/OMS
    // truth is ever silently ignored by this repair.
    let inside_window = open + ChronoDuration::hours(1);
    let found = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-stopping-sameday",
        inside_window,
    )
    .await?
    .expect("a same-day stopping row must still be found while inside its own window");
    assert_eq!(found.operation_id, operation_id);

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// PAPER-SOAK-STALE-STOP-STATE-RELEASE-REPAIR-01: `stop_retrying` had the
// identical unconditional-relevance gap `stopping` had before
// PAPER-SOAK-STALE-STOPPING-RELEASE-01 closed it. A durable
// `state = stop_retrying, stopped_at_utc != null` row is a legal shape --
// a stop-retry attempt may successfully stop the bound runtime and durably
// record `stopped_at_utc` via `record_autonomous_runtime_stopped` while the
// CAS state transition never advances past `stop_retrying` (process/session
// end before the next finalization tick). Left unconditionally relevant,
// that stale row can collide with a newer valid operation and reproduce the
// exact same "2 equally authoritative active operations found" permanent
// fail-closed wedge that `stopping` reproduced.
// ---------------------------------------------------------------------------

/// Seed a `stop_retrying` operation, from `market_date`, bound to a real
/// `runs` row, reached via the real legal CAS chain
/// `running -> stopping -> stop_retrying`, with `stopped_at_utc` recorded
/// via the same canonical recorder production uses
/// (`record_autonomous_runtime_stopped`).
async fn seed_stopped_stop_retrying_with_run(
    pool: &sqlx::PgPool,
    seed: &str,
    market_date: NaiveDate,
    run_id: Uuid,
) -> anyhow::Result<Uuid> {
    let (open, _close, _preopen, _postclose) = session_bounds(market_date);
    let operation_id = seed_operation_for_date(pool, seed, market_date).await;
    let running = advance_to_running(pool, operation_id, run_id, open).await?;
    let stopping = advance_one(pool, &running, STATE_STOPPING, open).await?;
    assert_eq!(stopping.state, STATE_STOPPING);
    let stop_retrying = advance_one(pool, &stopping, STATE_STOP_RETRYING, open).await?;
    assert_eq!(stop_retrying.state, STATE_STOP_RETRYING);
    mqk_db::record_autonomous_runtime_stopped(pool, operation_id, open).await?;
    Ok(operation_id)
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stopped_run_zero_activity_clean_reconcile_stop_retrying_is_released(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    reset_reconcile_status_clean(&pool, Utc::now()).await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();

    let (open, _, _, _) = session_bounds(yesterday);
    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?; // genuinely STOPPED, zero orders ever created

    let stale_id = seed_stopped_stop_retrying_with_run(
        &pool,
        "relopen-stop-retrying-released",
        yesterday,
        run_id,
    )
    .await?;

    let current_id = seed_operation_for_date(&pool, "relopen-stop-retrying-released", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    // RED before the repair: this call returns `Err` ("2 equally
    // authoritative active operations found"), reproducing the
    // stop_retrying analogue of the stopping production defect. GREEN
    // after the repair: the stale stop_retrying row is released and only
    // the current running operation is found.
    let found = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-stop-retrying-released",
        current_ts,
    )
    .await?
    .expect(
        "a proven-terminal, zero-activity, clean-reconcile stop_retrying row must be released \
         and never shadow the current running operation",
    );
    assert_eq!(
        found.operation_id, current_id,
        "the stale stop_retrying row must not shadow the current running operation"
    );

    // The stale row's own state must remain completely untouched -- this
    // repair separates lifecycle authority from evidence truth, it never
    // rewrites history.
    let stale_row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, stale_id)
        .await?
        .expect("row must exist");
    assert_eq!(stale_row.state, STATE_STOP_RETRYING);

    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stop_retrying_row_with_unacked_outbox_still_blocks(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    reset_reconcile_status_clean(&pool, Utc::now()).await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();

    let (open, _, _, _) = session_bounds(yesterday);
    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?;

    let stale_id = seed_stopped_stop_retrying_with_run(
        &pool,
        "relopen-stop-retrying-outbox",
        yesterday,
        run_id,
    )
    .await?;

    // A SENT-but-not-yet-ACKED order still associated with the now-stopped
    // run -- an order may still be in flight; this must never be silently
    // released, exactly like the existing stopping/evidence_degraded proof.
    sqlx::query(
        "insert into oms_outbox (run_id, idempotency_key, order_json, status, created_at_utc, \
         sent_at_utc) values ($1, $2, '{}'::jsonb, 'SENT', $3, $3)",
    )
    .bind(run_id)
    .bind(format!("test-unresolved-{}", unique_suffix()))
    .bind(open)
    .execute(&pool)
    .await?;

    let current_id = seed_operation_for_date(&pool, "relopen-stop-retrying-outbox", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let result = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-stop-retrying-outbox",
        current_ts,
    )
    .await;
    assert!(
        result.is_err(),
        "a stop_retrying row whose run has an unacked outbox row must still fail closed as \
         ambiguous; got {result:?}"
    );

    let _ = sqlx::query("delete from oms_outbox where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await;
    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stop_retrying_row_with_dirty_reconcile_still_blocks(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();

    let (open, _, _, _) = session_bounds(yesterday);
    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?;

    let stale_id =
        seed_stopped_stop_retrying_with_run(&pool, "relopen-stop-retrying-dirty", yesterday, run_id)
            .await?;

    persist_reconcile_status_state(
        &pool,
        &PersistReconcileStatusState {
            status: "dirty",
            last_run_at_utc: Some(open),
            snapshot_watermark_ms: None,
            mismatched_positions: 1,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: Some("test: simulated position disagreement"),
            updated_at_utc: open,
        },
    )
    .await?;

    let current_id = seed_operation_for_date(&pool, "relopen-stop-retrying-dirty", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let result = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-stop-retrying-dirty",
        current_ts,
    )
    .await;
    assert!(
        result.is_err(),
        "a stop_retrying row whose run stopped with a dirty global reconcile status must still \
         fail closed as ambiguous; got {result:?}"
    );

    reset_reconcile_status_clean(&pool, current_ts).await?;
    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// PAPER-SOAK-UNRESOLVED-BROKER-EVIDENCE-GATE-01: `stop_retrying` shares the
// exact evidence-gated release clause -- an unapplied inbox row must block
// it too.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stop_retrying_row_with_unapplied_inbox_still_blocks(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    reset_reconcile_status_clean(&pool, Utc::now()).await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();

    let (open, _, _, _) = session_bounds(yesterday);
    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?;

    let stale_id = seed_stopped_stop_retrying_with_run(
        &pool,
        "relopen-stop-retrying-inbox",
        yesterday,
        run_id,
    )
    .await?;

    mqk_db::inbox_insert_deduped(
        &pool,
        run_id,
        &format!("test-unapplied-{}", unique_suffix()),
        serde_json::json!({"event_kind": "fill"}),
    )
    .await?;

    let current_id = seed_operation_for_date(&pool, "relopen-stop-retrying-inbox", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let result = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-stop-retrying-inbox",
        current_ts,
    )
    .await;
    assert!(
        result.is_err(),
        "a stop_retrying row whose run stopped with an unapplied inbox row must still fail \
         closed as ambiguous; got {result:?}"
    );

    let _ = sqlx::query("delete from oms_inbox where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await;
    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_same_day_stop_retrying_with_stopped_run_still_found(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    reset_reconcile_status_clean(&pool, Utc::now()).await?;
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();
    let (open, _, _, _) = session_bounds(today);

    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?;

    let operation_id = seed_stopped_stop_retrying_with_run(
        &pool,
        "relopen-stop-retrying-sameday",
        today,
        run_id,
    )
    .await?;

    // Same-day stop_retrying row, even with fully proven-safe release
    // evidence, must still be found while `now_utc` falls inside its own
    // persisted window -- release only ever applies once the operation's
    // own market date has passed, exactly mirroring stopping's/
    // evidence_degraded's existing same-day protection.
    let inside_window = open + ChronoDuration::hours(1);
    let found = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-stop-retrying-sameday",
        inside_window,
    )
    .await?
    .expect("a same-day stop_retrying row must still be found while inside its own window");
    assert_eq!(found.operation_id, operation_id);

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stop_retrying_row_with_stopped_at_utc_null_still_relevant(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    reset_reconcile_status_clean(&pool, Utc::now()).await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();
    let (open, _, _, _) = session_bounds(yesterday);

    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?;
    stop_run(&pool, run_id).await?; // run is genuinely STOPPED ...

    // ... but the operation row never gets `stopped_at_utc` recorded --
    // deliberately skip `record_autonomous_runtime_stopped`. Proves the
    // release predicate's first, mandatory conjunct (`stopped_at_utc is not
    // null`) alone gates release, independent of the bound run's own state.
    let stale_id = seed_operation_for_date(&pool, "relopen-stop-retrying-null", yesterday).await;
    let running = advance_to_running(&pool, stale_id, run_id, open).await?;
    let stopping = advance_one(&pool, &running, STATE_STOPPING, open).await?;
    let stop_retrying = advance_one(&pool, &stopping, STATE_STOP_RETRYING, open).await?;
    assert_eq!(stop_retrying.state, STATE_STOP_RETRYING);

    let current_id = seed_operation_for_date(&pool, "relopen-stop-retrying-null", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let result = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-stop-retrying-null",
        current_ts,
    )
    .await;
    assert!(
        result.is_err(),
        "a stop_retrying row with stopped_at_utc still null must remain unconditionally \
         relevant and fail closed against the current running operation; got {result:?}"
    );

    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_stop_retrying_row_with_non_stopped_run_still_relevant(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    reset_reconcile_status_clean(&pool, Utc::now()).await?;
    let yesterday = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let run_id = Uuid::new_v4();
    let (open, _, _, _) = session_bounds(yesterday);

    insert_run(&pool, &new_run_fixture(run_id, open)).await?;
    arm_run(&pool, run_id).await?;
    begin_run(&pool, run_id).await?; // run stays RUNNING -- never durably STOPPED

    let stale_id =
        seed_stopped_stop_retrying_with_run(&pool, "relopen-stop-retrying-nonstopped", yesterday, run_id)
            .await?;

    let current_id =
        seed_operation_for_date(&pool, "relopen-stop-retrying-nonstopped", today).await;
    let current_run_id = Uuid::new_v4();
    let current_ts = session_bounds(today).0;
    advance_to_running(&pool, current_id, current_run_id, current_ts).await?;

    let result = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-stop-retrying-nonstopped",
        current_ts,
    )
    .await;
    assert!(
        result.is_err(),
        "a stop_retrying row whose bound run is not durably STOPPED must remain relevant and \
         fail closed against the current running operation; got {result:?}"
    );

    cleanup_operation(&pool, stale_id).await;
    cleanup_operation(&pool, current_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// REPAIR 4: refresh_autonomous_daily_operation_blocker
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn blocker_self_refresh_stores_same_state_with_new_reason_and_one_event() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "refresh-a").await;
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let ts = session_bounds(market_date).0;
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let blocked_args = TransitionAutonomousDailyOperationArgs {
        operation_id: row.operation_id,
        expected_state: row.state.clone(),
        expected_state_version: row.state_version,
        new_state: STATE_MANUAL_INTERVENTION_REQUIRED.to_string(),
        reason_code: Some("reason_a".to_string()),
        blocker_signature: Some("sha256:aaaa".to_string()),
        occurred_at_utc: ts,
        run_id: None,
        bounded_detail: "test setup: -> manual (reason_a)".to_string(),
    };
    let blocked = match transition_autonomous_daily_operation(&pool, &blocked_args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => r,
        other => panic!("expected Applied, got {other:?}"),
    };
    let events_before = list_autonomous_daily_operation_events(&pool, operation_id, 100).await?;

    let refresh_args = RefreshAutonomousDailyOperationBlockerArgs {
        operation_id,
        expected_state: STATE_MANUAL_INTERVENTION_REQUIRED.to_string(),
        expected_state_version: blocked.state_version,
        reason_code: "reason_b".to_string(),
        blocker_signature: Some("sha256:bbbb".to_string()),
        occurred_at_utc: ts + ChronoDuration::seconds(30),
        bounded_detail: "test: self-refresh reason_a -> reason_b".to_string(),
    };
    let refreshed = match refresh_autonomous_daily_operation_blocker(&pool, &refresh_args).await? {
        RefreshAutonomousDailyOperationBlockerOutcome::Applied(r) => r,
        other => panic!("expected Applied, got {other:?}"),
    };
    assert_eq!(
        refreshed.state, STATE_MANUAL_INTERVENTION_REQUIRED,
        "the refresh must remain in the same state"
    );
    assert_eq!(refreshed.state_reason_code.as_deref(), Some("reason_b"));
    assert_eq!(
        refreshed.state_blocker_signature.as_deref(),
        Some("sha256:bbbb")
    );
    assert_eq!(refreshed.state_version, blocked.state_version + 1);

    let events_after = list_autonomous_daily_operation_events(&pool, operation_id, 100).await?;
    assert_eq!(
        events_after.len(),
        events_before.len() + 1,
        "the refresh must insert exactly one append-only event"
    );
    let last_event = events_after.last().expect("at least one event");
    assert_eq!(last_event.from_state, STATE_MANUAL_INTERVENTION_REQUIRED);
    assert_eq!(last_event.to_state, STATE_MANUAL_INTERVENTION_REQUIRED);
    assert_eq!(last_event.reason_code.as_deref(), Some("reason_b"));

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn blocker_self_refresh_identical_values_are_idempotent() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "refresh-b").await;
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let ts = session_bounds(market_date).0;
    let _row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    // `controller_degraded` has no legal edge directly from
    // `awaiting_preopen` in the pure transition graph -- seed via `running`
    // first, then use the CAS graph's real `running -> controller_degraded`
    // edge.
    let running = advance_to_running(&pool, operation_id, Uuid::new_v4(), ts).await?;
    let degrade_args = TransitionAutonomousDailyOperationArgs {
        operation_id: running.operation_id,
        expected_state: running.state.clone(),
        expected_state_version: running.state_version,
        new_state: STATE_CONTROLLER_DEGRADED.to_string(),
        reason_code: Some("reason_a".to_string()),
        blocker_signature: Some("sha256:aaaa".to_string()),
        occurred_at_utc: ts,
        run_id: None,
        bounded_detail: "test setup: running -> controller_degraded".to_string(),
    };
    let degraded = match transition_autonomous_daily_operation(&pool, &degrade_args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => r,
        other => panic!("expected Applied, got {other:?}"),
    };

    let refresh_args = RefreshAutonomousDailyOperationBlockerArgs {
        operation_id,
        expected_state: STATE_CONTROLLER_DEGRADED.to_string(),
        expected_state_version: degraded.state_version,
        reason_code: "reason_b".to_string(),
        blocker_signature: Some("sha256:bbbb".to_string()),
        occurred_at_utc: ts + ChronoDuration::seconds(30),
        bounded_detail: "test: self-refresh".to_string(),
    };
    let refreshed = match refresh_autonomous_daily_operation_blocker(&pool, &refresh_args).await? {
        RefreshAutonomousDailyOperationBlockerOutcome::Applied(r) => r,
        other => panic!("expected Applied, got {other:?}"),
    };
    let events_after_first =
        list_autonomous_daily_operation_events(&pool, operation_id, 100).await?;

    // Repeating the exact same call (same stale expected_state_version,
    // same target values) must be idempotent: no further write, no further
    // event.
    let repeat = refresh_autonomous_daily_operation_blocker(&pool, &refresh_args).await?;
    match repeat {
        RefreshAutonomousDailyOperationBlockerOutcome::AlreadyApplied(r) => {
            assert_eq!(r.state_version, refreshed.state_version);
        }
        other => panic!("expected AlreadyApplied, got {other:?}"),
    }
    let events_after_repeat =
        list_autonomous_daily_operation_events(&pool, operation_id, 100).await?;
    assert_eq!(
        events_after_repeat.len(),
        events_after_first.len(),
        "an idempotent replay must insert no further event"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn blocker_self_refresh_arbitrary_state_self_loops_remain_illegal() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "refresh-illegal").await;
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(row.state, STATE_AWAITING_PREOPEN);

    // `awaiting_preopen` is not blocker-refresh-eligible -- an arbitrary
    // same-state loop must remain illegal, never silently accepted.
    let refresh_args = RefreshAutonomousDailyOperationBlockerArgs {
        operation_id,
        expected_state: STATE_AWAITING_PREOPEN.to_string(),
        expected_state_version: row.state_version,
        reason_code: "anything".to_string(),
        blocker_signature: None,
        occurred_at_utc: row.created_at_utc,
        bounded_detail: "test: illegal self-loop attempt".to_string(),
    };
    let outcome = refresh_autonomous_daily_operation_blocker(&pool, &refresh_args).await?;
    assert!(
        matches!(
            outcome,
            RefreshAutonomousDailyOperationBlockerOutcome::IllegalTarget
        ),
        "expected IllegalTarget, got {outcome:?}"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after.state_version, row.state_version,
        "an illegal target must never write anything"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-NONTRADING-RECOVERY-AND-RUNNING-
// CONFIRMATION-01
// REPAIR 2: fetch_relevant_open_autonomous_daily_operation's bound-but-
// unstopped-run relevance clause
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_bound_run_without_stop_evidence_is_relevant_outside_window(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let operation_id = seed_operation_for_date(&pool, "relopen-bound-unstopped", market_date).await;
    let run_id = Uuid::new_v4();
    let ts = session_bounds(market_date).0;
    let running = advance_to_running(&pool, operation_id, run_id, ts).await?;

    // recovery_retrying -> manual_intervention_required carries the bound
    // run_id along (never cleared) into a state that is neither in the
    // active-lifecycle set nor a terminal state -- exactly the gap REPAIR 2
    // closes.
    let recovering = advance_one(&pool, &running, STATE_RECOVERY_RETRYING, ts).await?;
    let degraded = advance_one(&pool, &recovering, STATE_MANUAL_INTERVENTION_REQUIRED, ts).await?;
    assert_eq!(degraded.run_id, Some(run_id));
    assert!(degraded.stopped_at_utc.is_none());

    let far_outside_window = ts + ChronoDuration::days(30);
    let found = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-bound-unstopped",
        far_outside_window,
    )
    .await?
    .expect(
        "REPAIR 2: a bound run without durable stop evidence must remain relevant regardless of \
         current state or window",
    );
    assert_eq!(found.operation_id, operation_id);

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_historical_manual_row_without_bound_run_is_ignored_outside_window(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let operation_id = seed_operation_for_date(&pool, "relopen-manual-norun", market_date).await;
    let ts = session_bounds(market_date).0;
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let manual = advance_one(&pool, &row, STATE_MANUAL_INTERVENTION_REQUIRED, ts).await?;
    assert_eq!(manual.run_id, None);

    let far_outside_window = ts + ChronoDuration::days(30);
    let found = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-manual-norun",
        far_outside_window,
    )
    .await?;
    assert!(
        found.is_none(),
        "an old manual row that never bound a run must not gain relevance merely because it is \
         recent"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn relevant_open_lookup_terminal_row_with_bound_run_and_no_stop_evidence_remains_excluded(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let operation_id = seed_operation_for_date(&pool, "relopen-terminal-bound", market_date).await;
    let run_id = Uuid::new_v4();
    let ts = session_bounds(market_date).0;
    let running = advance_to_running(&pool, operation_id, run_id, ts).await?;
    let stopping = advance_one(&pool, &running, STATE_STOPPING, ts).await?;
    let completed = advance_one(&pool, &stopping, mqk_db::STATE_COMPLETED_NO_TRADE, ts).await?;
    // Deliberately never calling `record_autonomous_runtime_stopped`: the
    // run_id remains bound and `stopped_at_utc` remains null even in this
    // terminal state, to prove the top-level `state not in (completed*)`
    // exclusion still wins over REPAIR 2's new bound-run clause.
    assert_eq!(completed.run_id, Some(run_id));
    assert!(completed.stopped_at_utc.is_none());

    let far_outside_window = ts + ChronoDuration::days(30);
    let found = fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "paper",
        "lifecycle-test-relopen-terminal-bound",
        far_outside_window,
    )
    .await?;
    assert!(
        found.is_none(),
        "a terminal row must remain excluded even when it still carries a bound run_id with no \
         stop evidence"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-NONTRADING-RECOVERY-AND-RUNNING-
// CONFIRMATION-01
// REPAIR 4: fetch_autonomous_daily_operation_event_at_sequence
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn event_at_sequence_returns_matching_event_and_rejects_wrong_sequence() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "event-seq-exact").await;
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let after = advance_one(&pool, &row, STATE_PREPARING_DATA, row.created_at_utc).await?;

    let found = fetch_autonomous_daily_operation_event_at_sequence(
        &pool,
        operation_id,
        after.state_version,
    )
    .await?
    .expect("the exact transition event must be found");
    assert_eq!(found.from_state, STATE_AWAITING_PREOPEN);
    assert_eq!(found.to_state, STATE_PREPARING_DATA);
    assert_eq!(found.transition_seq, after.state_version);

    let wrong_sequence = fetch_autonomous_daily_operation_event_at_sequence(
        &pool,
        operation_id,
        after.state_version + 1,
    )
    .await?;
    assert!(
        wrong_sequence.is_none(),
        "a wrong transition_seq must return None, never the nearest event"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn event_at_sequence_correct_after_more_than_100_earlier_events() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "event-seq-100plus").await;
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let ts = row.created_at_utc;
    let manual = advance_one(&pool, &row, STATE_MANUAL_INTERVENTION_REQUIRED, ts).await?;
    let first_manual_seq = manual.state_version;

    // Generate more than 100 additional append-only self-refresh events --
    // `list_autonomous_daily_operation_events`'s bounded `[1, 100]` cap would
    // never see the ones seeded below without raising `limit` far past its
    // documented default.
    let mut current = manual;
    let mut last_seq = first_manual_seq;
    for i in 0..110 {
        let refresh_args = RefreshAutonomousDailyOperationBlockerArgs {
            operation_id,
            expected_state: current.state.clone(),
            expected_state_version: current.state_version,
            reason_code: format!("reason-{i}"),
            blocker_signature: None,
            occurred_at_utc: ts,
            bounded_detail: "test: bulk self-refresh".to_string(),
        };
        current = match refresh_autonomous_daily_operation_blocker(&pool, &refresh_args).await? {
            RefreshAutonomousDailyOperationBlockerOutcome::Applied(r) => r,
            other => panic!("expected Applied, got {other:?}"),
        };
        last_seq = current.state_version;
    }
    assert!(
        last_seq - first_manual_seq >= 100,
        "test setup must generate at least 100 events after the first manual transition"
    );

    // The exact-sequence lookup must still find both the very first manual
    // transition and the final self-refresh event, despite well over 100
    // events existing on this operation.
    let first_event =
        fetch_autonomous_daily_operation_event_at_sequence(&pool, operation_id, first_manual_seq)
            .await?
            .expect("the first manual transition event must still be found");
    assert_eq!(first_event.to_state, STATE_MANUAL_INTERVENTION_REQUIRED);

    let last_event =
        fetch_autonomous_daily_operation_event_at_sequence(&pool, operation_id, last_seq)
            .await?
            .expect("the final self-refresh event must still be found");
    assert_eq!(last_event.from_state, STATE_MANUAL_INTERVENTION_REQUIRED);
    assert_eq!(last_event.to_state, STATE_MANUAL_INTERVENTION_REQUIRED);

    let events_via_bounded_list =
        list_autonomous_daily_operation_events(&pool, operation_id, 100).await?;
    assert!(
        (events_via_bounded_list.len() as i64) < last_seq,
        "the bounded list this exact lookup replaces must not itself see every event"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}
