//! AUTONOMOUS-DAILY-STOPPING-EVIDENCE-DEGRADED-OSCILLATION-01: proof tests
//! that a closed-session `evidence_degraded` operation (post-stop
//! `unknown_incomplete_bar_coverage` shape, `stopped_at_utc` set) never
//! re-enters `stopping` on an ordinary coordinator tick.
//!
//! DB-backed; skip without `MQK_DATABASE_URL`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_autonomous_daily_stopping_evidence_degraded_oscillation_01 \
//!   -- --test-threads=1 --nocapture --ignored
//!
//! No real provider, broker, or network call is made anywhere in this file.
//!
//! Incident this closes: `dispatch_by_state`'s D2.17 close-priority guard
//! never excluded `evidence_degraded`, so every post-close tick on an
//! already-durably-stopped `evidence_degraded` operation was routed to
//! `handle_session_close` -> `reconcile_durable_run_without_local_owner`,
//! which unconditionally re-requested `stopping`; the very next tick's
//! `handle_stopping` -> `handle_outcome_finalization` then immediately
//! reclassified back to `evidence_degraded` -- an unbounded ~30s oscillation
//! observed live on 2026-08-18 (operation `6aaa0349-e49c-5e2b-aa41-
//! 0439ec59b1a7`, state_version climbing past 330 with zero economic
//! activity).

use std::sync::Arc;

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
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

const REASON_INCOMPLETE_BAR_COVERAGE: &str = "unknown_incomplete_bar_coverage";

// ---------------------------------------------------------------------------
// Helpers (duplicated per this repo's own test-binary convention -- see
// scenario_autonomous_daily_evidence_degraded_recovery_01.rs for the same
// pattern; each `tests/*.rs` file is its own compiled binary)
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

/// A weekday at a fixed wall-clock time (2026-07-20 is a Monday).
fn weekday_at(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 20, hour, minute, 0).unwrap()
}

fn dormant_runtime_binding_identity() -> String {
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

/// Drive a freshly-seeded (`awaiting_open`) operation to the post-stop
/// `evidence_degraded` / `unknown_incomplete_bar_coverage` shape bound to
/// `run_id`, via the same legal edges production uses (`awaiting_open ->
/// start_retrying -> running -> stopping -> evidence_degraded`), with
/// `stopped_at_utc` genuinely recorded via `record_stopped_at` -- never a
/// raw `UPDATE`. Identical to the accepted fixture in
/// scenario_autonomous_daily_evidence_degraded_recovery_01.rs.
async fn seed_evidence_degraded_operation(
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
        new_state: mqk_db::STATE_STOPPING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: Some(run_id),
        bounded_detail: "test setup: -> stopping".to_string(),
    };
    let _row = match mqk_db::transition_autonomous_daily_operation(pool, &args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => r,
        other => panic!("expected Applied, got {other:?}"),
    };
    mqk_db::record_stopped_at(pool, operation_id, now_utc).await?;
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id)
        .await?
        .expect("row must exist after record_stopped_at");
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: row.state.clone(),
        expected_state_version: row.state_version,
        new_state: mqk_db::STATE_EVIDENCE_DEGRADED.to_string(),
        reason_code: Some(REASON_INCOMPLETE_BAR_COVERAGE.to_string()),
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: Some(run_id),
        bounded_detail: "test setup: -> evidence_degraded (simulated finalization-evidence-gap)"
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
    let _ = sqlx::query("delete from oms_inbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from oms_outbox where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(pool)
        .await;
}

async fn reset_reconcile_status_clean(
    pool: &sqlx::PgPool,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<()> {
    mqk_db::persist_reconcile_status_state(
        pool,
        &mqk_db::PersistReconcileStatusState {
            status: "ok",
            last_run_at_utc: Some(now_utc),
            snapshot_watermark_ms: None,
            mismatched_positions: 0,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: None,
            updated_at_utc: now_utc,
        },
    )
    .await?;
    Ok(())
}

async fn outbox_row_count_for_run(pool: &sqlx::PgPool, run_id: Uuid) -> anyhow::Result<i64> {
    let (count,): (i64,) = sqlx::query_as("select count(*) from oms_outbox where run_id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await?;
    Ok(count)
}

async fn event_row_count(pool: &sqlx::PgPool, operation_id: Uuid) -> anyhow::Result<i64> {
    let (count,): (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_daily_operation_events where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

/// Full fixture: a resolvable env, a genuinely-STOPPED run, and a durable
/// `evidence_degraded` / `unknown_incomplete_bar_coverage` operation bound to
/// it (`stopped_at_utc` set) -- the exact post-stop shape that oscillated in
/// production on 2026-08-18. Global reconcile status is reset clean and the
/// run carries zero outbox/inbox rows, matching Tuesday's real zero-orders
/// evidence.
async fn build_fixture(
    pool: &sqlx::PgPool,
    adapter_id: &str,
) -> anyhow::Result<(AutonomousDailySessionPlan, Uuid, Uuid, DateTime<Utc>)> {
    reset_env();
    set_resolvable_assignment_env("AAPL");
    let now = weekday_at(15, 0); // squarely mid-session, well before postclose_finalize_utc

    reset_reconcile_status_clean(pool, now).await?;
    mqk_db::persist_arm_state_canonical(pool, mqk_db::ArmState::Armed, None).await?;

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
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    mqk_db::stop_run(pool, run_id).await?; // genuinely STOPPED, zero orders ever created
    seed_evidence_degraded_operation(pool, operation_id, run_id, now).await?;
    Ok((plan, operation_id, run_id, now))
}

// ---------------------------------------------------------------------------
// 1. THE CORE PROOF: repeated post-close, post-postclose-finalize ticks on
//    an already-durably-stopped evidence_degraded operation never re-enter
//    `stopping`, never create a new run, never write an order, and converge
//    (state_version stabilizes) instead of oscillating forever.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t1_closed_session_evidence_degraded_never_reenters_stopping() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ev-deg-osc-t1-{}", unique_suffix());
    let (plan, operation_id, run_id, _seed_now) = build_fixture(&pool, &adapter_id).await?;

    let seeded = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(seeded.state, mqk_db::STATE_EVIDENCE_DEGRADED, "fixture precondition");
    assert!(seeded.stopped_at_utc.is_some(), "fixture precondition");
    // Past both the effective close and the postclose_finalize_utc window,
    // matching the accepted t4_session_window_closed_never_recovers timing
    // from scenario_autonomous_daily_evidence_degraded_recovery_01.rs.
    let tick0 = seeded.postclose_finalize_utc + Duration::seconds(1);
    assert!(
        tick0 >= plan.effective_operation_close_utc,
        "fixture precondition: tick0 must be past effective close"
    );

    let st = paper_state_with_db(pool.clone(), &adapter_id);

    let mut last_state_version: Option<i64> = None;
    for tick_index in 0..5i64 {
        let now = tick0 + Duration::seconds(30 * tick_index);
        let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
            .await?
            .expect("row must exist before tick");
        let outcome = dispatch_by_state(&st, &pool, operation, &plan, now).await?;

        let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
            .await?
            .expect("row must exist after tick");

        assert_ne!(
            after.state,
            mqk_db::STATE_STOPPING,
            "tick {tick_index}: a closed-session evidence_degraded operation must never be \
             pushed back into stopping (the observed production oscillation); outcome was \
             {outcome:?}"
        );
        assert_ne!(
            after.state,
            mqk_db::STATE_STOP_RETRYING,
            "tick {tick_index}: must never reach stop_retrying either"
        );
        assert_ne!(
            after.state,
            mqk_db::STATE_RUNNING,
            "tick {tick_index}: a closed session window must never legally reach running"
        );
        assert_eq!(
            after.run_id,
            Some(run_id),
            "tick {tick_index}: no fresh run may ever be created on a closed-session tick -- \
             the original terminal run_id must be unchanged"
        );

        if let Some(prev) = last_state_version {
            assert!(
                after.state_version >= prev,
                "tick {tick_index}: state_version must never move backward"
            );
            if tick_index >= 2 {
                assert_eq!(
                    after.state_version, prev,
                    "tick {tick_index}: by the third tick the operation must have converged -- \
                     state_version must stop changing (this is the exact invariant the observed \
                     oscillation violated, climbing past 330 in production)"
                );
            }
        }
        last_state_version = Some(after.state_version);
    }

    let final_events = event_row_count(&pool, operation_id).await?;
    // At most one reclassification write beyond the four seed transitions
    // (awaiting_open->start_retrying->running->stopping->evidence_degraded)
    // is tolerated for the coordinator's own first-tick reclassification;
    // an oscillating loop would instead have produced 2 new events per tick
    // (10 across 5 ticks).
    assert!(
        final_events <= 6,
        "event log must stay bounded, not grow ~2x per tick: {final_events} events after 5 ticks"
    );

    let outbox_count = outbox_row_count_for_run(&pool, run_id).await?;
    assert_eq!(
        outbox_count, 0,
        "no order/economic action may ever be produced by a closed-session evidence_degraded tick"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 2. Same fixture, but the run still has an unacked outbox row: recovery
//    logic must still fail closed (this proves my fix did not weaken the
//    existing outbox/reconcile safety checks -- it only changed which path
//    reaches evidence_degraded's own arm, not what that arm requires).
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t2_closed_session_still_fail_closed_on_unacked_outbox() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ev-deg-osc-t2-{}", unique_suffix());
    let (plan, operation_id, run_id, seed_now) = build_fixture(&pool, &adapter_id).await?;

    // Leave one unacked outbox row on the (already STOPPED) run -- an order
    // that may still be in flight to the broker.
    sqlx::query(
        "insert into oms_outbox (run_id, idempotency_key, order_json, status, created_at_utc) \
         values ($1, $2, '{}'::jsonb, 'SENT', $3)",
    )
    .bind(run_id)
    .bind(format!("test-unacked-{}", unique_suffix()))
    .bind(seed_now)
    .execute(&pool)
    .await?;

    let seeded = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let tick0 = seeded.postclose_finalize_utc + Duration::seconds(1);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = dispatch_by_state(&st, &pool, seeded, &plan, tick0).await?;
    assert!(
        !matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::RecoveryScheduled
                | AutonomousDailyCoordinatorTickOutcome::Recovered { .. }
        ),
        "an unacked outbox row must never allow recovery; got {outcome:?}"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_ne!(
        after.state,
        mqk_db::STATE_STOPPING,
        "must not be pushed into stopping either"
    );
    assert_ne!(after.state, mqk_db::STATE_RUNNING);

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}
