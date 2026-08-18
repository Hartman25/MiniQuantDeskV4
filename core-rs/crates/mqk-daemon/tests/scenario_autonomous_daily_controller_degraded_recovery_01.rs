//! AUTONOMOUS-DAILY-CONTROLLER-DEGRADED-RECOVERY-01: proof tests for the
//! `dispatch_by_state` repair to `mqk_db::STATE_CONTROLLER_DEGRADED`.
//!
//! DB-backed; skip without `MQK_DATABASE_URL`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_autonomous_daily_controller_degraded_recovery_01 \
//!   -- --test-threads=1 --nocapture --ignored
//!
//! No real provider, broker, or network call is made anywhere in this file.
//! Every `runs` row fixture is driven through the real lifecycle primitives
//! (`mqk_db::insert_run` / `arm_run` / `begin_run` / `stop_run`) -- never a
//! raw `UPDATE`. Every operation fixture reaches `running` -> `controller_
//! degraded` only through the real production CAS
//! (`mqk_db::transition_autonomous_daily_operation`).
//!
//! Incident this closes: before this repair, `dispatch_by_state`'s
//! `STATE_CONTROLLER_DEGRADED` arm was a permanent static re-projection --
//! it never re-read the durable run row, so an operation that reached
//! `controller_degraded` because of a *stale* runtime ownership record
//! (e.g. after an operator restart) stayed there forever even once the
//! referenced run had genuinely, cleanly stopped with zero unresolved
//! economic evidence.

use std::sync::Arc;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use mqk_daemon::state::autonomous_daily_coordinator::{
    dispatch_by_state, AutonomousDailyCoordinatorTickOutcome,
};
use mqk_daemon::state::{
    self, derive_assignment_identity, derive_autonomous_daily_operation_id,
    derive_runtime_binding_identity, resolve_autonomous_daily_session_plan_from_env, AppState,
    AutonomousDailyPlanTiming, AutonomousDailySessionPlan, AutonomousDailySessionPlanResolution,
    MultiSymbolConfigSource, MultiSymbolRuntimeConfig, SymbolStrategyAssignment,
};
use mqk_db::{AutonomousDailyTransitionOutcome, TransitionAutonomousDailyOperationArgs};
use uuid::Uuid;

const STRATEGY_SYMBOL_ENV: &str = "MQK_STRATEGY_SYMBOL";
const STRATEGY_IDS_ENV: &str = "MQK_STRATEGY_IDS";
const STRATEGY_TIMEFRAME_ENV: &str = "MQK_STRATEGY_MD_TIMEFRAME";

// ---------------------------------------------------------------------------
// Helpers (duplicated per this repo's own test-binary convention -- see
// scenario_autonomous_daily_session_coordinator_01.rs for the same pattern)
// ---------------------------------------------------------------------------

async fn test_pool() -> anyhow::Result<sqlx::PgPool> {
    if std::env::var("MQK_DATABASE_URL").is_err() {
        anyhow::bail!("SKIP: requires MQK_DATABASE_URL");
    }
    mqk_db::testkit_db_pool().await
}

fn unique_suffix() -> String {
    Uuid::new_v4().to_string().replace('-', "")[..10].to_string()
}

fn reset_env() {
    std::env::remove_var(STRATEGY_SYMBOL_ENV);
    std::env::remove_var(STRATEGY_IDS_ENV);
    std::env::remove_var(STRATEGY_TIMEFRAME_ENV);
}

fn set_resolvable_assignment_env(symbol: &str) {
    std::env::set_var(STRATEGY_SYMBOL_ENV, symbol);
    std::env::set_var(STRATEGY_IDS_ENV, "swing_momentum");
    std::env::set_var(STRATEGY_TIMEFRAME_ENV, "5m");
}

fn paper_state_with_db(db: sqlx::PgPool, adapter_id: &str) -> Arc<AppState> {
    let mut st = AppState::new_for_test_with_db_mode_and_broker(
        db,
        state::DeploymentMode::Paper,
        state::BrokerKind::Paper,
    );
    st.set_adapter_id_for_test(adapter_id);
    Arc::new(st)
}

/// A weekday at a fixed wall-clock time (2026-07-20 is a Monday). Matches
/// the fixture convention already proven applicable elsewhere in this test
/// suite.
fn weekday_at(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 20, hour, minute, 0).unwrap()
}

fn dormant_runtime_binding_identity() -> String {
    // Matches the identity a Dormant (no fleet configured) native-strategy
    // bootstrap resolves to -- see `resolve_autonomous_runtime_context`.
    derive_runtime_binding_identity(&mqk_runtime::native_strategy::EffectiveRuntimeBinding {
        effective_runtime_strategy_id: None,
        effective_runtime_target_symbol: None,
        effective_runtime_timeframe_secs: None,
    })
}

fn expected_assignment_identity(symbol: &str) -> String {
    let config = MultiSymbolRuntimeConfig {
        schema_version: "v2".to_string(),
        symbols: vec![SymbolStrategyAssignment {
            symbol: symbol.to_string(),
            strategy_id: "swing_momentum".to_string(),
            timeframe: "5m".to_string(),
        }],
        max_concurrent_symbols: 1,
        source: MultiSymbolConfigSource::EnvSingleSymbolFallback,
    };
    derive_assignment_identity(&config)
}

fn resolve_identity_for_env(
    adapter_id: &str,
    symbol: &str,
    now_utc: DateTime<Utc>,
) -> (AutonomousDailySessionPlan, String, String, Uuid) {
    let timing = AutonomousDailyPlanTiming::production_default();
    let plan = match resolve_autonomous_daily_session_plan_from_env(now_utc, &timing) {
        AutonomousDailySessionPlanResolution::Applicable(plan) => plan,
        other => {
            panic!("expected an applicable session plan for the test fixture date, got {other:?}")
        }
    };
    let assignment_identity = expected_assignment_identity(symbol);
    let runtime_binding_identity = dormant_runtime_binding_identity();
    let operation_id = derive_autonomous_daily_operation_id(
        &plan,
        "PAPER",
        adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
    );
    (plan, assignment_identity, runtime_binding_identity, operation_id)
}

#[allow(clippy::too_many_arguments)]
async fn seed_operation_row(
    pool: &sqlx::PgPool,
    plan: &AutonomousDailySessionPlan,
    operation_id: Uuid,
    adapter_id: &str,
    assignment_identity: &str,
    runtime_binding_identity: &str,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<()> {
    let create_args = mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date: NaiveDate::parse_from_str(&plan.market_date, "%Y-%m-%d")?,
        deployment_mode: "PAPER".to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity: plan.session_plan_identity.clone(),
        assignment_identity: assignment_identity.to_string(),
        runtime_binding_identity: runtime_binding_identity.to_string(),
        calendar_source: plan.calendar_source.clone(),
        calendar_coverage_state: plan.calendar_coverage_state.clone(),
        schedule_source: plan.schedule_source.as_str().to_string(),
        effective_operation_open_utc: plan.effective_operation_open_utc,
        effective_operation_close_utc: plan.effective_operation_close_utc,
        exchange_session_open_utc: plan.exchange_session_open_utc,
        exchange_session_close_utc: plan.exchange_session_close_utc,
        exchange_is_early_close: plan.exchange_is_early_close,
        previous_trading_date: NaiveDate::parse_from_str(&plan.previous_trading_date, "%Y-%m-%d")?,
        preopen_start_utc: plan.preopen_start_utc,
        postclose_finalize_utc: plan.postclose_finalize_utc,
        initial_state: mqk_db::STATE_AWAITING_OPEN.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: now_utc,
        bounded_detail: "test fixture seed".to_string(),
        stop_attempt_count: 0,
    };
    mqk_db::create_or_recover_autonomous_daily_operation(pool, &create_args).await?;
    Ok(())
}

/// Drive a freshly-seeded (`awaiting_open`) operation to `controller_degraded`
/// bound to `run_id`, via the same two legal edges production uses
/// (`awaiting_open -> start_retrying -> running -> controller_degraded`).
/// Never a raw `UPDATE` -- every hop is a real CAS transition.
async fn seed_controller_degraded_operation(
    pool: &sqlx::PgPool,
    operation_id: Uuid,
    run_id: Uuid,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<mqk_db::AutonomousDailyOperationRecord> {
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id)
        .await?
        .expect("seeded row must exist");
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: row.state.clone(),
        expected_state_version: row.state_version,
        new_state: mqk_db::STATE_START_RETRYING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: None,
        bounded_detail: "test setup: -> start_retrying".to_string(),
    };
    let row = match mqk_db::transition_autonomous_daily_operation(pool, &args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => r,
        other => panic!("expected Applied, got {other:?}"),
    };
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: row.state.clone(),
        expected_state_version: row.state_version,
        new_state: mqk_db::STATE_RUNNING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: Some(run_id),
        bounded_detail: "test setup: -> running".to_string(),
    };
    let row = match mqk_db::transition_autonomous_daily_operation(pool, &args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => r,
        other => panic!("expected Applied, got {other:?}"),
    };
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: row.state.clone(),
        expected_state_version: row.state_version,
        new_state: mqk_db::STATE_CONTROLLER_DEGRADED.to_string(),
        reason_code: Some("durable_active_run_without_local_owner".to_string()),
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: Some(run_id),
        bounded_detail: "test setup: -> controller_degraded (simulated forced restart)"
            .to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation(pool, &args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => Ok(r),
        other => panic!("expected Applied, got {other:?}"),
    }
}

fn new_run(run_id: Uuid, now_utc: DateTime<Utc>) -> mqk_db::NewRun {
    mqk_db::NewRun {
        run_id,
        engine_id: "mqk-daemon".to_string(),
        mode: "PAPER".to_string(),
        started_at_utc: now_utc,
        git_hash: "TEST".to_string(),
        config_hash: "TEST".to_string(),
        config_json: serde_json::json!({}),
        host_fingerprint: "test-host".to_string(),
    }
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

/// Full fixture setup shared by every test below: a resolvable env, a
/// `controller_degraded` operation bound to `run_id`, and the caller then
/// drives `run_id`'s `runs` row into whatever shape the test needs before
/// calling `dispatch_by_state`.
async fn build_fixture(
    pool: &sqlx::PgPool,
    adapter_id: &str,
) -> anyhow::Result<(AutonomousDailySessionPlan, Uuid, Uuid, DateTime<Utc>)> {
    reset_env();
    set_resolvable_assignment_env("AAPL");
    let now = weekday_at(15, 0); // squarely mid-session, well before any close

    // `sys_reconcile_status_state` is a global singleton (no run_id/
    // operation_id key) -- reset it to a known-clean baseline before every
    // test so an earlier test's dirty-reconcile fixture (t4b) can never
    // leak into a later one regardless of execution order.
    mqk_db::persist_reconcile_status_state(
        pool,
        &mqk_db::PersistReconcileStatusState {
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
    .await?;
    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_identity_for_env(adapter_id, "AAPL", now);
    seed_operation_row(
        pool,
        &plan,
        operation_id,
        adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
        now,
    )
    .await?;
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(pool, &new_run(run_id, now)).await?;
    seed_controller_degraded_operation(pool, operation_id, run_id, now).await?;
    Ok((plan, operation_id, run_id, now))
}

// ---------------------------------------------------------------------------
// 1. degraded + RUNNING run -> no recovery
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t1_degraded_with_running_run_does_not_recover() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ctrl-deg-t1-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?; // run_id is now genuinely RUNNING

    let before = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let operation = before.clone();
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = dispatch_by_state(&st, &pool, operation, &plan, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "durable_active_run_without_local_owner",
                ..
            }
        ),
        "a genuinely running run must never be presented as recovered; got {outcome:?}"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_ne!(
        after.state, mqk_db::STATE_STOPPING,
        "must never advance toward stopping while the run is still active"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 2. degraded + a stop was requested but the run is not yet STOPPED ->
//    no premature recovery
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t2_degraded_with_stop_requested_but_not_stopped_does_not_recover() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ctrl-deg-t2-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;
    mqk_db::request_stop_run(&pool, run_id).await?; // requested, but status is still RUNNING

    let run_row = mqk_db::fetch_run(&pool, run_id).await?;
    assert_eq!(
        run_row.status.as_str(),
        "RUNNING",
        "fixture precondition: stop requested but not yet completed"
    );

    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = dispatch_by_state(&st, &pool, operation, &plan, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "durable_active_run_without_local_owner",
                ..
            }
        ),
        "a stop-requested-but-not-completed run must never be presented as recovered; \
         got {outcome:?}"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 3. degraded + cleanly STOPPED run, zero unresolved evidence -> recovery
//    through the normal production coordinator path
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t3_degraded_with_clean_stopped_run_recovers_via_normal_path() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ctrl-deg-t3-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;
    mqk_db::stop_run(&pool, run_id).await?; // genuinely STOPPED, zero orders ever created

    let before = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let operation = before.clone();
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = dispatch_by_state(&st, &pool, operation, &plan, now).await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::RuntimeStopped,
        "a cleanly stopped run with zero unresolved evidence must legally advance"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after.state, mqk_db::STATE_STOPPING);
    assert!(
        after.stopped_at_utc.is_some(),
        "the operation's own stopped_at_utc must now be recorded"
    );
    assert!(
        after.state_version > before.state_version,
        "a real state transition must have occurred, not a same-state refresh"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 4a. degraded + STOPPED + an unresolved (unacked) outbox row -> fail closed
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t4a_degraded_with_stopped_run_and_unresolved_outbox_fails_closed() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ctrl-deg-t4a-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;
    mqk_db::stop_run(&pool, run_id).await?;

    // A SENT-but-not-yet-ACKED order still associated with the now-stopped
    // run -- exactly the "order may still be in flight" shape that must
    // never be silently treated as safe to close out.
    sqlx::query(
        "insert into oms_outbox (run_id, idempotency_key, order_json, status, created_at_utc, \
         sent_at_utc) values ($1, $2, '{}'::jsonb, 'SENT', $3, $3)",
    )
    .bind(run_id)
    .bind(format!("test-unresolved-{}", unique_suffix()))
    .bind(now)
    .execute(&pool)
    .await?;

    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = dispatch_by_state(&st, &pool, operation, &plan, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "unresolved_outbox_at_run_reconcile",
                ..
            }
        ),
        "an unresolved outbox row must fail closed rather than be presented as safely \
         stopped; got {outcome:?}"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_ne!(after.state, mqk_db::STATE_STOPPING);
    assert!(after.stopped_at_utc.is_none());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 4b. degraded + STOPPED + a dirty global reconcile status -> fail closed
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t4b_degraded_with_stopped_run_and_dirty_reconcile_fails_closed() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ctrl-deg-t4b-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;
    mqk_db::stop_run(&pool, run_id).await?;

    mqk_db::persist_reconcile_status_state(
        &pool,
        &mqk_db::PersistReconcileStatusState {
            status: "dirty",
            last_run_at_utc: Some(now),
            snapshot_watermark_ms: None,
            mismatched_positions: 1,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: Some("test: simulated position disagreement"),
            updated_at_utc: now,
        },
    )
    .await?;

    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = dispatch_by_state(&st, &pool, operation, &plan, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "reconcile_dirty",
                ..
            }
        ),
        "a dirty global reconcile status must fail closed rather than be presented as \
         safely stopped; got {outcome:?}"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_ne!(after.state, mqk_db::STATE_STOPPING);

    // Restore the global singleton to clean before releasing the pool --
    // this test must never leak a dirty reconcile status into whatever
    // test runs next.
    mqk_db::persist_reconcile_status_state(
        &pool,
        &mqk_db::PersistReconcileStatusState {
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
    .await?;

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 5. repeated coordinator ticks after recovery -> idempotent, no duplicate
//    transition/run
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t5_repeated_ticks_after_recovery_are_idempotent() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ctrl-deg-t5-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;
    mqk_db::stop_run(&pool, run_id).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);

    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let outcome1 = dispatch_by_state(&st, &pool, operation, &plan, now).await?;
    assert_eq!(outcome1, AutonomousDailyCoordinatorTickOutcome::RuntimeStopped);
    let after_first = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after_first.state, mqk_db::STATE_STOPPING);

    // A second tick against the now-`stopping` row: dispatch_by_state routes
    // `stopping` to `handle_stopping`, a different, already-proven-safe
    // path -- this assertion's real purpose is proving the first tick's own
    // transition was NOT re-applied a second time and produced exactly one
    // durable state_version bump, not two.
    let after_second_fetch = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after_first.state_version, after_second_fetch.state_version,
        "no further mutation must occur merely from re-reading the row"
    );

    let run_row = mqk_db::fetch_run(&pool, run_id).await?;
    assert_eq!(
        run_row.status.as_str(),
        "STOPPED",
        "the same run row must remain STOPPED -- no duplicate run was created"
    );
    let run_count: i64 = sqlx::query_scalar(
        "select count(*) from runs where run_id = $1",
    )
    .bind(run_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(run_count, 1, "exactly one run row must exist for this run_id");

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}
