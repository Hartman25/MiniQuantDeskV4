//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-COMPLETED-BAR-DATA-DRIVER: mqk-db
//! store proof for the Phase C provider-poll evidence recorders and the
//! durable bar-identity-keyed dispatch claim table
//! (`sys_autonomous_daily_bar_dispatches`, migration `0050`).
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-db --test scenario_autonomous_daily_operation_data_evidence_01 \
//!   -- --include-ignored --test-threads=1 --nocapture

use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use mqk_db::{
    claim_autonomous_daily_bar_dispatch, complete_autonomous_daily_bar_dispatch,
    create_or_recover_autonomous_daily_operation, fail_autonomous_daily_bar_dispatch,
    fetch_autonomous_daily_bar_dispatch, fetch_autonomous_daily_operation_by_id,
    record_completed_bar_observed, record_provider_poll_failed, record_provider_poll_started,
    record_provider_poll_succeeded, BarDispatchClaimOutcome, CreateAutonomousDailyOperationArgs,
    CreateOrRecoverAutonomousDailyOperationOutcome, RecordCompletedBarObservedOutcome,
    RecordProviderPollOutcome, DISPATCH_STATUS_COMPLETED, DISPATCH_STATUS_UNCERTAIN, ENV_DB_URL,
    STATE_AWAITING_PREOPEN,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers (mirror scenario_autonomous_daily_operation_store_01.rs)
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

async fn create_test_operation(
    pool: &sqlx::PgPool,
    adapter_id: &str,
    occurred_at_utc: DateTime<Utc>,
) -> mqk_db::AutonomousDailyOperationRecord {
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let (open, close, preopen, postclose) = session_bounds(market_date);
    let previous_trading_date = market_date - ChronoDuration::days(3);
    let operation_id = test_operation_id(&format!("evidence-test|{adapter_id}"));
    let args = CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date,
        deployment_mode: "paper".to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity: format!("evidence-test-plan|{adapter_id}"),
        assignment_identity: "evidence-test-assignment".to_string(),
        runtime_binding_identity: "evidence-test-binding".to_string(),
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
        bounded_detail: "evidence test creation".to_string(),
    };
    match create_or_recover_autonomous_daily_operation(pool, &args)
        .await
        .expect("create operation")
    {
        CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => record,
        CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(record) => record,
        CreateOrRecoverAutonomousDailyOperationOutcome::IdentityConflict { .. } => {
            panic!("unexpected identity conflict in evidence test fixture")
        }
    }
}

// ---------------------------------------------------------------------------
// Points 1-4: provider-poll counters increment atomically; timestamps are
// caller-supplied.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn provider_poll_counters_increment_atomically_and_timestamps_are_caller_supplied(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("evidence-poll-{}", unique_suffix());
    let t0 = Utc.with_ymd_and_hms(2026, 7, 20, 13, 0, 0).unwrap();
    let operation = create_test_operation(&pool, &adapter_id, t0).await;

    let t1 = t0 + ChronoDuration::minutes(1);
    let started =
        record_provider_poll_started(&pool, operation.operation_id, t1, "polling").await?;
    match started {
        RecordProviderPollOutcome::Recorded {
            provider_poll_attempt_count,
            provider_poll_success_count,
            provider_poll_failure_count,
        } => {
            assert_eq!(provider_poll_attempt_count, 1);
            assert_eq!(provider_poll_success_count, 0);
            assert_eq!(provider_poll_failure_count, 0);
        }
        RecordProviderPollOutcome::NotFound => panic!("operation must exist"),
    }

    let t2 = t1 + ChronoDuration::seconds(5);
    let succeeded =
        record_provider_poll_succeeded(&pool, operation.operation_id, t2, "bar_received").await?;
    match succeeded {
        RecordProviderPollOutcome::Recorded {
            provider_poll_attempt_count,
            provider_poll_success_count,
            provider_poll_failure_count,
        } => {
            assert_eq!(
                provider_poll_attempt_count, 1,
                "success recorder must not also bump attempt count"
            );
            assert_eq!(provider_poll_success_count, 1);
            assert_eq!(provider_poll_failure_count, 0);
        }
        RecordProviderPollOutcome::NotFound => panic!("operation must exist"),
    }

    let t3 = t2 + ChronoDuration::seconds(5);
    let failed = record_provider_poll_failed(
        &pool,
        operation.operation_id,
        t3,
        "provider_rejected",
        "boom",
    )
    .await?;
    match failed {
        RecordProviderPollOutcome::Recorded {
            provider_poll_attempt_count,
            provider_poll_success_count,
            provider_poll_failure_count,
        } => {
            assert_eq!(provider_poll_attempt_count, 1);
            assert_eq!(provider_poll_success_count, 1);
            assert_eq!(provider_poll_failure_count, 1);
        }
        RecordProviderPollOutcome::NotFound => panic!("operation must exist"),
    }

    let refetched = fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await?
        .expect("row exists");
    assert_eq!(refetched.provider_poll_attempt_count, 1);
    assert_eq!(refetched.provider_poll_success_count, 1);
    assert_eq!(refetched.provider_poll_failure_count, 1);
    // Point 4: last_provider_poll_utc reflects the caller-supplied instant of
    // the LAST recorder call (t3), never a server-generated `now()`.
    assert_eq!(refetched.last_provider_poll_utc, Some(t3));
    assert_eq!(refetched.last_error.as_deref(), Some("boom"));
    assert_eq!(refetched.data_refresh_state, "provider_rejected");

    Ok(())
}

// ---------------------------------------------------------------------------
// Points 5-7, 13-14: bar-observation idempotency, monotonic advance, and
// null-vs-zero distinctness.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn bar_observation_idempotent_monotonic_and_null_vs_zero_distinct() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("evidence-obs-{}", unique_suffix());
    let t0 = Utc.with_ymd_and_hms(2026, 7, 20, 13, 0, 0).unwrap();
    let operation = create_test_operation(&pool, &adapter_id, t0).await;

    // Point 13/14: before any observation, bars_observed is explicit zero
    // (never null — it is a not-null column) and last_completed_bar_ts is
    // genuinely null (never fabricated as zero).
    assert_eq!(operation.bars_observed, 0);
    assert_eq!(operation.last_completed_bar_ts, None);

    let ts1 = 1_800_000_000_i64;
    let r1 = record_completed_bar_observed(&pool, operation.operation_id, ts1, t0).await?;
    assert!(matches!(
        r1,
        RecordCompletedBarObservedOutcome::Recorded { bars_observed: 1 }
    ));

    // Point 5: replaying the exact same bar is idempotent — no double count.
    let r1_replay = record_completed_bar_observed(&pool, operation.operation_id, ts1, t0).await?;
    assert!(matches!(
        r1_replay,
        RecordCompletedBarObservedOutcome::AlreadyObserved { bars_observed: 1 }
    ));

    // Point 6: a genuinely newer bar advances.
    let ts2 = ts1 + 300;
    let r2 = record_completed_bar_observed(&pool, operation.operation_id, ts2, t0).await?;
    assert!(matches!(
        r2,
        RecordCompletedBarObservedOutcome::Recorded { bars_observed: 2 }
    ));

    // Point 7: an older bar is refused as a no-op — evidence never rewinds.
    let r_old = record_completed_bar_observed(&pool, operation.operation_id, ts1, t0).await?;
    match r_old {
        RecordCompletedBarObservedOutcome::StaleBarIgnored {
            current_last_completed_bar_ts,
        } => {
            assert_eq!(current_last_completed_bar_ts, Some(ts2));
        }
        other => panic!("expected StaleBarIgnored, got {other:?}"),
    }

    let refetched = fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await?
        .expect("row exists");
    assert_eq!(
        refetched.bars_observed, 2,
        "older-bar no-op must not have touched the counter"
    );
    assert_eq!(refetched.last_completed_bar_ts, Some(ts2));

    Ok(())
}

// ---------------------------------------------------------------------------
// Points 8-9: dispatch claim uniqueness and completed-claim immutability.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn dispatch_claim_unique_by_identity_and_completed_cannot_be_reclaimed() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    let adapter_id = format!("evidence-claim-{}", unique_suffix());
    let t0 = Utc.with_ymd_and_hms(2026, 7, 20, 13, 0, 0).unwrap();
    let operation = create_test_operation(&pool, &adapter_id, t0).await;
    let bar_end_ts = 1_800_000_300_i64;
    // Production order: a bar is always observed before it is claimed for
    // dispatch (`sys_autonomous_daily_operations_bars_dispatched_le_observed`
    // enforces this at the DB level too).
    record_completed_bar_observed(&pool, operation.operation_id, bar_end_ts, t0).await?;

    // Point 8: unique by (operation_id, local_symbol, timeframe, bar_end_ts)
    // — a different timeframe for the identical operation/symbol/end_ts is a
    // distinct, independently-claimable identity.
    let claim_5m = claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
        t0,
    )
    .await?;
    assert!(matches!(claim_5m, BarDispatchClaimOutcome::Claimed));
    let claim_1m = claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "1m",
        bar_end_ts,
        t0,
    )
    .await?;
    assert!(
        matches!(claim_1m, BarDispatchClaimOutcome::Claimed),
        "a distinct timeframe must be independently claimable"
    );

    // Point 9: complete the 5m claim, then prove it cannot be claimed again.
    let completed = complete_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
        t0 + ChronoDuration::seconds(1),
        None,
    )
    .await?;
    assert!(completed);
    let reclaim = claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
        t0 + ChronoDuration::seconds(2),
    )
    .await?;
    match reclaim {
        BarDispatchClaimOutcome::AlreadyCompleted { .. } => {}
        other => panic!("expected AlreadyCompleted, got {other:?}"),
    }
    let row = fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
    )
    .await?
    .expect("row exists");
    assert_eq!(row.status, DISPATCH_STATUS_COMPLETED);

    Ok(())
}

// ---------------------------------------------------------------------------
// Point 10: an unresolved claim is visible after a brand-new DB connection
// (durability, not just in-process caching).
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn unresolved_claim_visible_after_new_db_connection() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("evidence-restart-{}", unique_suffix());
    let t0 = Utc.with_ymd_and_hms(2026, 7, 20, 13, 0, 0).unwrap();
    let operation = create_test_operation(&pool, &adapter_id, t0).await;
    let bar_end_ts = 1_800_000_600_i64;

    let claim1 = claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
        t0,
    )
    .await?;
    assert!(matches!(claim1, BarDispatchClaimOutcome::Claimed));

    // A brand-new pool/connection — proves durable persistence, not
    // in-process state.
    let fresh_pool = mqk_db::testkit_db_pool().await?;
    let row = fetch_autonomous_daily_bar_dispatch(
        &fresh_pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
    )
    .await?
    .expect("claim row must be durably visible");
    assert_eq!(row.status, mqk_db::DISPATCH_STATUS_CLAIMED);

    // A second claim attempt (over the fresh connection) reclassifies to
    // uncertain — the restart-recovery signature.
    let claim2 = claim_autonomous_daily_bar_dispatch(
        &fresh_pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
        t0 + ChronoDuration::minutes(1),
    )
    .await?;
    assert!(
        matches!(claim2, BarDispatchClaimOutcome::Unresolved { status } if status == DISPATCH_STATUS_UNCERTAIN)
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Point 11: dispatch completion and operation counters agree.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn dispatch_completion_and_operation_counters_agree() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("evidence-agree-{}", unique_suffix());
    let t0 = Utc.with_ymd_and_hms(2026, 7, 20, 13, 0, 0).unwrap();
    let operation = create_test_operation(&pool, &adapter_id, t0).await;
    assert_eq!(operation.bars_dispatched, 0);
    assert_eq!(operation.last_dispatched_bar_ts, None);

    let bar_end_ts = 1_800_000_900_i64;
    record_completed_bar_observed(&pool, operation.operation_id, bar_end_ts, t0).await?;
    let claim = claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
        t0,
    )
    .await?;
    assert!(matches!(claim, BarDispatchClaimOutcome::Claimed));

    let evaluation_id = Uuid::new_v4();
    let completed = complete_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
        t0 + ChronoDuration::seconds(1),
        Some(evaluation_id),
    )
    .await?;
    assert!(completed);

    let refetched = fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await?
        .expect("row exists");
    assert_eq!(refetched.bars_dispatched, 1);
    assert_eq!(refetched.last_dispatched_bar_ts, Some(bar_end_ts));

    let claim_row = fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
    )
    .await?
    .expect("row exists");
    assert_eq!(claim_row.status, DISPATCH_STATUS_COMPLETED);
    assert_eq!(claim_row.evaluation_id, Some(evaluation_id));

    // Idempotent replay: completing again is a safe no-op, never a second
    // increment.
    let completed_again = complete_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
        t0 + ChronoDuration::seconds(2),
        Some(evaluation_id),
    )
    .await?;
    assert!(!completed_again);
    let refetched2 = fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await?
        .expect("row exists");
    assert_eq!(
        refetched2.bars_dispatched, 1,
        "replayed completion must not double-increment"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn fail_dispatch_marks_failed_and_never_completes() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("evidence-fail-{}", unique_suffix());
    let t0 = Utc.with_ymd_and_hms(2026, 7, 20, 13, 0, 0).unwrap();
    let operation = create_test_operation(&pool, &adapter_id, t0).await;
    let bar_end_ts = 1_800_001_200_i64;

    let claim = claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
        t0,
    )
    .await?;
    assert!(matches!(claim, BarDispatchClaimOutcome::Claimed));

    let failed = fail_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
        "dispatch returned no result",
    )
    .await?;
    assert!(failed);

    let row = fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
    )
    .await?
    .expect("row exists");
    assert_eq!(row.status, mqk_db::DISPATCH_STATUS_FAILED);
    assert_ne!(row.status, DISPATCH_STATUS_COMPLETED);
    assert_eq!(row.completed_at_utc, None);

    let refetched = fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await?
        .expect("row exists");
    assert_eq!(
        refetched.bars_dispatched, 0,
        "a failed dispatch must never advance bars_dispatched"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Point 12: concurrent identical claims produce exactly one winner.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn concurrent_identical_claims_produce_one_winner() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("evidence-race-{}", unique_suffix());
    let t0 = Utc.with_ymd_and_hms(2026, 7, 20, 13, 0, 0).unwrap();
    let operation = create_test_operation(&pool, &adapter_id, t0).await;
    let bar_end_ts = 1_800_001_500_i64;

    let mut handles = Vec::new();
    for _ in 0..8 {
        let pool = pool.clone();
        let operation_id = operation.operation_id;
        handles.push(tokio::spawn(async move {
            claim_autonomous_daily_bar_dispatch(&pool, operation_id, "ZZEVID", "5m", bar_end_ts, t0)
                .await
        }));
    }
    let mut claimed_count = 0;
    let mut unresolved_count = 0;
    for handle in handles {
        match handle.await.expect("join")? {
            BarDispatchClaimOutcome::Claimed => claimed_count += 1,
            BarDispatchClaimOutcome::Unresolved { .. } => unresolved_count += 1,
            BarDispatchClaimOutcome::AlreadyCompleted { .. } => {
                panic!("no completion happened yet in this test")
            }
        }
    }
    assert_eq!(
        claimed_count, 1,
        "exactly one concurrent claim attempt must win"
    );
    assert_eq!(
        unresolved_count, 7,
        "every other concurrent attempt must observe Unresolved, never a second Claimed"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Point 15: read functions perform no writes.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn read_functions_perform_no_writes() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("evidence-readonly-{}", unique_suffix());
    let t0 = Utc.with_ymd_and_hms(2026, 7, 20, 13, 0, 0).unwrap();
    let operation = create_test_operation(&pool, &adapter_id, t0).await;
    let bar_end_ts = 1_800_001_800_i64;
    claim_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
        t0,
    )
    .await?;

    let before_op = fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await?
        .expect("exists");
    let before_claim = fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
    )
    .await?
    .expect("exists");

    // Call every read function repeatedly; nothing should change.
    for _ in 0..5 {
        let _ = fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id).await?;
        let _ = fetch_autonomous_daily_bar_dispatch(
            &pool,
            operation.operation_id,
            "ZZEVID",
            "5m",
            bar_end_ts,
        )
        .await?;
    }

    let after_op = fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
        .await?
        .expect("exists");
    let after_claim = fetch_autonomous_daily_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZEVID",
        "5m",
        bar_end_ts,
    )
    .await?
    .expect("exists");

    assert_eq!(
        before_op.updated_at_utc, after_op.updated_at_utc,
        "reads must never advance updated_at_utc"
    );
    assert_eq!(
        before_op.state_version, after_op.state_version,
        "reads must never bump state_version"
    );
    assert_eq!(
        before_claim, after_claim,
        "reads must never mutate the claim row"
    );

    Ok(())
}
