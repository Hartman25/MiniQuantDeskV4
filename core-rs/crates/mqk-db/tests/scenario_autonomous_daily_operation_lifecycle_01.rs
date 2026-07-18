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
    STATE_MANUAL_INTERVENTION_REQUIRED, STATE_PREPARING_DATA, STATE_RUNNING, STATE_START_RETRYING,
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
