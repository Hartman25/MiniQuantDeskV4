//! MULTI-STRATEGY-RUNTIME-DISPATCH-01 same-symbol multi-strategy repair
//! (V4-BULK-CODE-COMPLETION-STAGE-B-M2-01): store proof for the
//! strategy_id-scoped durable bar-identity-keyed dispatch claim table
//! (`sys_autonomous_daily_binding_bar_dispatches`, migration `0075`).
//!
//! Mirrors `scenario_autonomous_daily_operation_data_evidence_01.rs`'s own
//! proof shape for migration 0050's claim/complete/fail functions, applied
//! to the strategy-scoped table this session added. The one new invariant
//! 0050 cannot express and 0075 exists for: two different strategies bound
//! to the exact same `(symbol, timeframe)` must be independently claimable,
//! never sharing one claim identity.
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-db --test scenario_autonomous_daily_binding_bar_dispatch_01 \
//!   -- --include-ignored --test-threads=1 --nocapture

use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use mqk_db::{
    claim_autonomous_daily_binding_bar_dispatch, complete_autonomous_daily_binding_bar_dispatch,
    create_or_recover_autonomous_daily_operation, fail_autonomous_daily_binding_bar_dispatch,
    fetch_autonomous_daily_binding_bar_dispatch, BarDispatchClaimOutcome,
    CreateAutonomousDailyOperationArgs, CreateOrRecoverAutonomousDailyOperationOutcome,
    DISPATCH_STATUS_CLAIMED, DISPATCH_STATUS_COMPLETED, DISPATCH_STATUS_FAILED,
    DISPATCH_STATUS_UNCERTAIN, ENV_DB_URL, STATE_AWAITING_PREOPEN,
};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers (mirror scenario_autonomous_daily_operation_data_evidence_01.rs;
// each integration test file is its own crate, so these are duplicated
// rather than shared)
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
    let operation_id = test_operation_id(&format!("binding-bar-dispatch-test|{adapter_id}"));
    let args = CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date,
        deployment_mode: "paper".to_string(),
        adapter_id: adapter_id.to_string(),
        execution_domain: mqk_db::EXECUTION_DOMAIN_EQUITY_NYSE.to_string(),
        session_plan_identity: format!("binding-bar-dispatch-test-plan|{adapter_id}"),
        assignment_identity: "binding-bar-dispatch-test-assignment".to_string(),
        runtime_binding_identity: "binding-bar-dispatch-test-binding".to_string(),
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
        data_refresh_state: "idle".to_string(),
        occurred_at_utc,
        bounded_detail: "binding-bar-dispatch-test-fixture".to_string(),
        stop_attempt_count: 0,
    };
    match create_or_recover_autonomous_daily_operation(pool, &args)
        .await
        .expect("create_or_recover_autonomous_daily_operation must succeed")
    {
        CreateOrRecoverAutonomousDailyOperationOutcome::Created(record)
        | CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(record) => record,
        CreateOrRecoverAutonomousDailyOperationOutcome::IdentityConflict { .. } => {
            panic!("unexpected identity conflict for a fresh test fixture operation_id")
        }
    }
}

// ---------------------------------------------------------------------------
// Requirement #3/core-0075-invariant: two different strategies bound to the
// exact same (symbol, timeframe, bar_end_ts) are independently claimable —
// the one thing migration 0050's identity cannot express.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn same_symbol_two_strategies_claim_independently_never_collide() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("binding-bar-{}", unique_suffix());
    let t0 = Utc.with_ymd_and_hms(2026, 7, 20, 13, 0, 0).unwrap();
    let operation = create_test_operation(&pool, &adapter_id, t0).await;
    let bar_end_ts = 1_800_000_300_i64;

    let claim_a = claim_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZBIND",
        "strategy_A",
        "5m",
        bar_end_ts,
        t0,
    )
    .await?;
    assert!(
        matches!(claim_a, BarDispatchClaimOutcome::Claimed),
        "strategy_A's claim on ZZBIND/5m/{bar_end_ts} must succeed"
    );

    // Requirement #4 proof at the durable-claim layer: strategy_B binding
    // the exact same symbol/timeframe/bar_end_ts must claim independently —
    // never blocked, reclassified, or merged with strategy_A's row.
    let claim_b = claim_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZBIND",
        "strategy_B",
        "5m",
        bar_end_ts,
        t0,
    )
    .await?;
    assert!(
        matches!(claim_b, BarDispatchClaimOutcome::Claimed),
        "strategy_B must independently claim the identical (symbol, timeframe, bar_end_ts) \
         that strategy_A already claimed — this is exactly what migration 0050 cannot express \
         and 0075 exists for"
    );

    let row_a = fetch_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZBIND",
        "strategy_A",
        "5m",
        bar_end_ts,
    )
    .await?
    .expect("strategy_A's row exists");
    let row_b = fetch_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZBIND",
        "strategy_B",
        "5m",
        bar_end_ts,
    )
    .await?
    .expect("strategy_B's row exists");
    assert_eq!(row_a.status, DISPATCH_STATUS_CLAIMED);
    assert_eq!(row_b.status, DISPATCH_STATUS_CLAIMED);
    assert_eq!(row_a.strategy_id, "strategy_A");
    assert_eq!(row_b.strategy_id, "strategy_B");

    // Completing strategy_A's claim must never affect strategy_B's row.
    let completed_a = complete_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZBIND",
        "strategy_A",
        "5m",
        bar_end_ts,
        t0 + ChronoDuration::seconds(1),
        None,
    )
    .await?;
    assert!(completed_a);
    let row_b_after = fetch_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZBIND",
        "strategy_B",
        "5m",
        bar_end_ts,
    )
    .await?
    .expect("strategy_B's row still exists");
    assert_eq!(
        row_b_after.status, DISPATCH_STATUS_CLAIMED,
        "strategy_A completing must never mutate strategy_B's independent claim"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Requirement #3: replay of the same (symbol, strategy_id, timeframe,
// bar_end_ts) after completion never creates a second economic
// evaluation/dispatch — AlreadyCompleted, not a fresh Claimed.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn replay_after_completion_is_already_completed_never_a_second_dispatch() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    let adapter_id = format!("binding-bar-replay-{}", unique_suffix());
    let t0 = Utc.with_ymd_and_hms(2026, 7, 20, 13, 0, 0).unwrap();
    let operation = create_test_operation(&pool, &adapter_id, t0).await;
    let bar_end_ts = 1_800_000_600_i64;

    let claim = claim_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZREPLAY",
        "strategy_A",
        "5m",
        bar_end_ts,
        t0,
    )
    .await?;
    assert!(matches!(claim, BarDispatchClaimOutcome::Claimed));

    let evaluation_id = Uuid::new_v4();
    let completed = complete_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZREPLAY",
        "strategy_A",
        "5m",
        bar_end_ts,
        t0 + ChronoDuration::seconds(1),
        Some(evaluation_id),
    )
    .await?;
    assert!(completed);

    // Replay (e.g. the driver's own retry after a restart): must observe
    // AlreadyCompleted with the SAME evaluation_id, never re-claim.
    let replay = claim_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZREPLAY",
        "strategy_A",
        "5m",
        bar_end_ts,
        t0 + ChronoDuration::seconds(2),
    )
    .await?;
    match replay {
        BarDispatchClaimOutcome::AlreadyCompleted {
            evaluation_id: replayed_id,
        } => {
            assert_eq!(
                replayed_id,
                Some(evaluation_id),
                "replay must surface the original evaluation id, never a new one"
            );
        }
        other => panic!("expected AlreadyCompleted on replay, got {other:?}"),
    }

    let row = fetch_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZREPLAY",
        "strategy_A",
        "5m",
        bar_end_ts,
    )
    .await?
    .expect("row exists");
    assert_eq!(row.status, DISPATCH_STATUS_COMPLETED);

    Ok(())
}

// ---------------------------------------------------------------------------
// Requirement #5: an unresolved/failed durable claim remains fail-closed —
// a second claim attempt on a still-`claimed` row reclassifies to
// `uncertain` (never silently redispatched), and an explicit failure marks
// `failed` and stays that way.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn unresolved_and_failed_claims_remain_fail_closed() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("binding-bar-failclosed-{}", unique_suffix());
    let t0 = Utc.with_ymd_and_hms(2026, 7, 20, 13, 0, 0).unwrap();
    let operation = create_test_operation(&pool, &adapter_id, t0).await;
    let bar_end_ts = 1_800_000_900_i64;

    let claim1 = claim_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZFAIL",
        "strategy_A",
        "5m",
        bar_end_ts,
        t0,
    )
    .await?;
    assert!(matches!(claim1, BarDispatchClaimOutcome::Claimed));

    // A second claim attempt against the still-`claimed` row (e.g. a crash
    // between claim and confirmation) reclassifies to `uncertain` — the
    // restart-recovery signature, never a silent redispatch.
    let claim2 = claim_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZFAIL",
        "strategy_A",
        "5m",
        bar_end_ts,
        t0 + ChronoDuration::minutes(1),
    )
    .await?;
    assert!(
        matches!(claim2, BarDispatchClaimOutcome::Unresolved { status } if status == DISPATCH_STATUS_UNCERTAIN)
    );

    // A fresh operation/bar identity, explicitly failed (the bounded
    // deposit-and-confirm retry window's own timeout path).
    let bar_end_ts_2 = bar_end_ts + 300;
    let claim3 = claim_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZFAIL",
        "strategy_A",
        "5m",
        bar_end_ts_2,
        t0,
    )
    .await?;
    assert!(matches!(claim3, BarDispatchClaimOutcome::Claimed));
    let failed = fail_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZFAIL",
        "strategy_A",
        "5m",
        bar_end_ts_2,
        "host-pool dispatch confirmation not observed within the bounded retry window",
    )
    .await?;
    assert!(failed);
    let row = fetch_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZFAIL",
        "strategy_A",
        "5m",
        bar_end_ts_2,
    )
    .await?
    .expect("row exists");
    assert_eq!(row.status, DISPATCH_STATUS_FAILED);

    // A further claim attempt against the `failed` row must remain
    // Unresolved — never auto-redispatched.
    let claim4 = claim_autonomous_daily_binding_bar_dispatch(
        &pool,
        operation.operation_id,
        "ZZFAIL",
        "strategy_A",
        "5m",
        bar_end_ts_2,
        t0 + ChronoDuration::minutes(2),
    )
    .await?;
    assert!(
        matches!(claim4, BarDispatchClaimOutcome::Unresolved { status } if status == DISPATCH_STATUS_FAILED)
    );

    Ok(())
}
