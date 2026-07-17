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
    list_autonomous_daily_operation_events, record_retry_timing, record_running_started,
    record_start_attempt, record_stop_attempt, record_stopped_at,
    CreateAutonomousDailyOperationArgs, CreateOrRecoverAutonomousDailyOperationOutcome,
    RecordRetryTimingOutcome, RecordRunningStartedOutcome, RecordStartAttemptOutcome,
    RecordStopAttemptOutcome, RecordStoppedAtOutcome, ENV_DB_URL, STATE_AWAITING_PREOPEN,
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
