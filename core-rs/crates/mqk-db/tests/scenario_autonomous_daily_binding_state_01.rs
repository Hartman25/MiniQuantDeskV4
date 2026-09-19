//! MULTI-STRATEGY-RUNTIME-DISPATCH-01 / autonomous completed-bar driver
//! multi-binding repair (R2A, `V4-STAGE-B-M2-REPAIR-02`): mqk-db store proof
//! for `sys_autonomous_daily_binding_state` (migration 0071).
//!
//! Proves: schema/constraints (closed status/reason vocabulary, reason
//! required iff blocked), restart persistence (a fresh read after the
//! "process" that wrote a row goes away still sees the same durable state),
//! and cross-binding isolation (writing/blocking one binding never mutates
//! another binding's row, even within the same operation).
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-db --test scenario_autonomous_daily_binding_state_01 \
//!   -- --include-ignored --test-threads=1 --nocapture

use chrono::{DateTime, Datelike, Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use mqk_db::{
    create_or_recover_autonomous_daily_operation, fetch_autonomous_daily_binding_states,
    mark_autonomous_daily_binding_active, mark_autonomous_daily_binding_locally_blocked,
    BindingLocalBlockReason, CreateAutonomousDailyOperationArgs,
    CreateOrRecoverAutonomousDailyOperationOutcome, ENV_DB_URL, STATE_AWAITING_PREOPEN,
};
use uuid::Uuid;

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

async fn seed_operation(pool: &sqlx::PgPool, seed: &str) -> anyhow::Result<Uuid> {
    let market_date = NaiveDate::from_ymd_opt(2026, 9, 18).unwrap();
    let (open, close, preopen, postclose) = session_bounds(market_date);
    let operation_id = test_operation_id(seed);
    let args = CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date,
        deployment_mode: "paper".to_string(),
        adapter_id: "alpaca".to_string(),
        session_plan_identity: format!("r2a-session-{seed}"),
        assignment_identity: format!("r2a-assignment-{seed}"),
        runtime_binding_identity: format!("r2a-binding-{seed}"),
        calendar_source: "nyse_weekdays_heuristic".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: "nyse_weekdays_heuristic".to_string(),
        effective_operation_open_utc: open,
        effective_operation_close_utc: close,
        exchange_session_open_utc: open,
        exchange_session_close_utc: close,
        exchange_is_early_close: false,
        previous_trading_date: market_date - ChronoDuration::days(3),
        preopen_start_utc: preopen,
        postclose_finalize_utc: postclose,
        initial_state: STATE_AWAITING_PREOPEN.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: Utc::now(),
        bounded_detail: "r2a scenario test creation".to_string(),
        stop_attempt_count: 0,
    };
    let outcome = create_or_recover_autonomous_daily_operation(pool, &args).await?;
    match outcome {
        CreateOrRecoverAutonomousDailyOperationOutcome::Created(r) => Ok(r.operation_id),
        CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(r) => Ok(r.operation_id),
        other => anyhow::bail!("unexpected create-or-recover outcome: {other:?}"),
    }
}

async fn cleanup(pool: &sqlx::PgPool, operation_id: Uuid) {
    let _ = sqlx::query("delete from sys_autonomous_daily_binding_state where operation_id = $1")
        .bind(operation_id)
        .execute(pool)
        .await;
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

// ---------------------------------------------------------------------------
// Schema / constraint tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn schema_table_and_constraints_exist() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let exists: bool = sqlx::query_scalar(
        "select exists (select 1 from information_schema.tables where table_name = 'sys_autonomous_daily_binding_state')",
    )
    .fetch_one(&pool)
    .await?;
    assert!(exists, "sys_autonomous_daily_binding_state must exist");
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn unrecognized_status_is_rejected_by_the_db_constraint() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "r2a-bad-status").await?;

    let result = sqlx::query(
        "insert into sys_autonomous_daily_binding_state \
         (operation_id, symbol, strategy_id, timeframe, status, reason_code, last_error, updated_at_utc) \
         values ($1,'AAPL','intraday_scalper','5m','not_a_real_status',null,null,now())",
    )
    .bind(operation_id)
    .execute(&pool)
    .await;

    cleanup(&pool, operation_id).await;
    assert!(
        result.is_err(),
        "an unrecognized status must be rejected by the DB check constraint"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn blocked_without_reason_is_rejected() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "r2a-blocked-no-reason").await?;

    let result = sqlx::query(
        "insert into sys_autonomous_daily_binding_state \
         (operation_id, symbol, strategy_id, timeframe, status, reason_code, last_error, updated_at_utc) \
         values ($1,'AAPL','intraday_scalper','5m','locally_blocked',null,null,now())",
    )
    .bind(operation_id)
    .execute(&pool)
    .await;

    cleanup(&pool, operation_id).await;
    assert!(
        result.is_err(),
        "locally_blocked with no reason_code must be rejected"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn active_with_reason_is_rejected() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "r2a-active-with-reason").await?;

    let result = sqlx::query(
        "insert into sys_autonomous_daily_binding_state \
         (operation_id, symbol, strategy_id, timeframe, status, reason_code, last_error, updated_at_utc) \
         values ($1,'AAPL','intraday_scalper','5m','active','binding_readiness_blocked',null,now())",
    )
    .bind(operation_id)
    .execute(&pool)
    .await;

    cleanup(&pool, operation_id).await;
    assert!(
        result.is_err(),
        "active with a stale reason_code must be rejected -- a recovered binding \
         must never carry a leftover block reason"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Restart persistence
// ---------------------------------------------------------------------------

/// A binding marked `locally_blocked`, then read back through a brand-new
/// pool connection (simulating a daemon restart's fresh read), reports the
/// identical durable state.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn locally_blocked_state_survives_a_fresh_read() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "r2a-restart-persistence").await?;
    let now = Utc::now();

    mark_autonomous_daily_binding_locally_blocked(
        &pool,
        operation_id,
        "AAPL",
        "intraday_scalper",
        "5m",
        BindingLocalBlockReason::MarketDataMissingOrStale,
        Some("no bar observed for 3 consecutive ticks"),
        now,
    )
    .await?;

    // A fresh pool, mirroring a restarted process reconnecting.
    let fresh_pool = test_pool().await?;
    let states = fetch_autonomous_daily_binding_states(&fresh_pool, operation_id).await?;

    cleanup(&pool, operation_id).await;

    assert_eq!(states.len(), 1);
    let row = &states[0];
    assert_eq!(row.symbol, "AAPL");
    assert_eq!(row.strategy_id, "intraday_scalper");
    assert_eq!(row.status, "locally_blocked");
    assert_eq!(
        row.reason_code.as_deref(),
        Some("binding_market_data_missing_or_stale")
    );
    assert!(row.last_error.is_some());
    Ok(())
}

/// Recovery: a binding previously `locally_blocked` can be marked `active`
/// again, clearing its reason — proven durable across a fresh read too.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn recovery_to_active_clears_the_reason_durably() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "r2a-recovery").await?;
    let t1 = Utc::now();
    let t2 = t1 + ChronoDuration::seconds(60);

    mark_autonomous_daily_binding_locally_blocked(
        &pool,
        operation_id,
        "AAPL",
        "intraday_scalper",
        "5m",
        BindingLocalBlockReason::NoNewBarWaiting,
        None,
        t1,
    )
    .await?;
    mark_autonomous_daily_binding_active(&pool, operation_id, "AAPL", "intraday_scalper", "5m", t2)
        .await?;

    let states = fetch_autonomous_daily_binding_states(&pool, operation_id).await?;
    cleanup(&pool, operation_id).await;

    assert_eq!(states.len(), 1);
    assert_eq!(states[0].status, "active");
    assert_eq!(states[0].reason_code, None);
    Ok(())
}

// ---------------------------------------------------------------------------
// Cross-binding isolation
// ---------------------------------------------------------------------------

/// R2 requirement #3/#8: blocking one binding (AAPL) must never mutate a
/// sibling binding's row (MSFT), even within the same operation, even for
/// the same strategy_id.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn blocking_one_binding_never_mutates_a_sibling_binding() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "r2a-cross-binding-isolation").await?;
    let now = Utc::now();

    mark_autonomous_daily_binding_active(&pool, operation_id, "AAPL", "intraday_scalper", "5m", now)
        .await?;
    mark_autonomous_daily_binding_active(&pool, operation_id, "MSFT", "intraday_scalper", "5m", now)
        .await?;

    mark_autonomous_daily_binding_locally_blocked(
        &pool,
        operation_id,
        "AAPL",
        "intraday_scalper",
        "5m",
        BindingLocalBlockReason::UnsupportedOrInvalidSymbolTimeframe,
        Some("AAPL-specific failure"),
        now + ChronoDuration::seconds(1),
    )
    .await?;

    let states = fetch_autonomous_daily_binding_states(&pool, operation_id).await?;
    cleanup(&pool, operation_id).await;

    assert_eq!(states.len(), 2);
    let aapl = states
        .iter()
        .find(|s| s.symbol == "AAPL")
        .expect("AAPL row present");
    let msft = states
        .iter()
        .find(|s| s.symbol == "MSFT")
        .expect("MSFT row present");
    assert_eq!(aapl.status, "locally_blocked");
    assert_eq!(msft.status, "active", "MSFT must be unaffected by AAPL's block");
    assert_eq!(msft.reason_code, None);
    Ok(())
}

/// Two strategies on the *same* symbol are independently trackable rows —
/// blocking one strategy's binding for a symbol must not affect another
/// strategy's binding for the same symbol.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn same_symbol_different_strategy_bindings_are_independently_tracked() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    let operation_id = seed_operation(&pool, "r2a-same-symbol-diff-strategy").await?;
    let now = Utc::now();

    mark_autonomous_daily_binding_active(&pool, operation_id, "AAPL", "intraday_scalper", "5m", now)
        .await?;
    mark_autonomous_daily_binding_locally_blocked(
        &pool,
        operation_id,
        "AAPL",
        "intraday_short_scalper",
        "5m",
        BindingLocalBlockReason::ReadinessBlocked,
        None,
        now,
    )
    .await?;

    let states = fetch_autonomous_daily_binding_states(&pool, operation_id).await?;
    cleanup(&pool, operation_id).await;

    assert_eq!(states.len(), 2);
    let scalper = states
        .iter()
        .find(|s| s.strategy_id == "intraday_scalper")
        .unwrap();
    let short = states
        .iter()
        .find(|s| s.strategy_id == "intraday_short_scalper")
        .unwrap();
    assert_eq!(scalper.status, "active");
    assert_eq!(short.status, "locally_blocked");
    Ok(())
}
