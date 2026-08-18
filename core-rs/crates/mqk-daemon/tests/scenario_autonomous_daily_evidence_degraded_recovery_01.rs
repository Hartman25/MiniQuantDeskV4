//! PAPER-SOAK-4DAY-20260818-01 EVIDENCE-DEGRADED-RECOVERY-01: proof tests for
//! the same-session `evidence_degraded -> running` recovery attempt
//! (`autonomous_daily_coordinator::attempt_evidence_degraded_recovery`).
//!
//! DB-backed; skip without `MQK_DATABASE_URL`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_autonomous_daily_evidence_degraded_recovery_01 \
//!   -- --test-threads=1 --nocapture --ignored
//!
//! No real provider, broker, or network call is made anywhere in this file.
//! Every `runs` row fixture is driven through the real lifecycle primitives
//! (`mqk_db::insert_run` / `arm_run` / `begin_run` / `stop_run` / `halt_run`)
//! -- never a raw `UPDATE`. Every operation fixture reaches `evidence_
//! degraded` only through the real production CAS
//! (`mqk_db::transition_autonomous_daily_operation` /
//! `mqk_db::record_stopped_at`).
//!
//! Incident this closes: `evidence_degraded -> running` is a legal edge in
//! the durable state graph, but before this repair no production caller
//! ever requested it -- an operation that reached the post-stop
//! `unknown_incomplete_bar_coverage` shape of `evidence_degraded` was
//! structurally stuck there for the rest of the session even once the
//! transient condition genuinely cleared.

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
// scenario_autonomous_daily_controller_degraded_recovery_01.rs for the same
// pattern)
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
/// raw `UPDATE`.
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

async fn reset_reconcile_status_clean(pool: &sqlx::PgPool, now_utc: DateTime<Utc>) -> anyhow::Result<()> {
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

/// Full fixture setup shared by every test below: a resolvable env, a
/// zero-activity `evidence_degraded` / `unknown_incomplete_bar_coverage`
/// operation bound to `run_id`, itself still `ARMED` (not yet driven to
/// `STOPPED`) -- the caller then drives `run_id`'s `runs` row into whatever
/// shape the test needs before calling `dispatch_by_state`.
async fn build_fixture(
    pool: &sqlx::PgPool,
    adapter_id: &str,
) -> anyhow::Result<(AutonomousDailySessionPlan, Uuid, Uuid, DateTime<Utc>)> {
    reset_env();
    set_resolvable_assignment_env("AAPL");
    let now = weekday_at(15, 0); // squarely mid-session, well before postclose_finalize_utc

    // `sys_reconcile_status_state` is a global singleton (no run_id/
    // operation_id key) -- reset it to a known-clean baseline before every
    // test so an earlier test's dirty-reconcile fixture can never leak into
    // a later one regardless of execution order.
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
// 1. clean, terminal, in-window -> first tick schedules (does not
//    immediately start)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t1_clean_stopped_run_first_tick_schedules_not_immediate_start() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ev-deg-t1-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    let before = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert!(before.next_retry_utc.is_none(), "fixture precondition");
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = dispatch_by_state(&st, &pool, before, &plan, now).await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::RecoveryScheduled,
        "the first eligible tick must schedule a bounded backoff, never an immediate start"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after.state,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        "scheduling must self-loop within evidence_degraded, never transition state"
    );
    assert!(
        after.next_retry_utc.is_some(),
        "a bounded recovery retry time must now be durably scheduled"
    );
    assert_eq!(
        after.run_id,
        Some(run_id),
        "the original run_id must be unchanged until a real start actually succeeds"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 2. once the scheduled retry is due, the real canonical start sequence is
//    genuinely attempted (not merely rescheduled again) and durably counted
//    -- the same proof strength already accepted for the structurally
//    identical `recovery_retrying` repair's own due-retry test
//    (`h01_recovery_retry_due_calls_start_without_illegal_start_retrying_edge`).
//    A full successful start into `running` additionally requires a real
//    provider/broker/registry fixture (see `scenario_autonomous_paper_day_
//    lifecycle_auton12.rs`'s AL-03) outside this file's scope; the CAS-
//    level proof that `evidence_degraded -> running` is accepted once a
//    start genuinely succeeds is established directly by reading
//    `transition_autonomous_daily_operation_to_running`, which validates
//    only `is_legal_operation_transition(expected_state, running)` --
//    already true for `evidence_degraded` -- and never inspects `from_state`
//    beyond that.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t2_due_retry_genuinely_attempts_start_never_reschedules_again() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ev-deg-t2-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None).await?;
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    let before = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let outcome = dispatch_by_state(&st, &pool, before, &plan, now).await?;
    assert_eq!(outcome, AutonomousDailyCoordinatorTickOutcome::RecoveryScheduled);

    let scheduled = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let scheduled_retry_utc = scheduled.next_retry_utc.expect("must be scheduled");
    let due_now = scheduled_retry_utc + Duration::seconds(1);

    let outcome2 = dispatch_by_state(&st, &pool, scheduled, &plan, due_now).await?;
    assert!(
        !matches!(
            outcome2,
            AutonomousDailyCoordinatorTickOutcome::RecoveryScheduled
        ),
        "once due, the tick must genuinely attempt the canonical start sequence, never merely \
         reschedule the same backoff again; got {outcome2:?}"
    );
    assert!(
        !matches!(outcome2, AutonomousDailyCoordinatorTickOutcome::RetryNotDue),
        "the retry is due at `due_now`; got {outcome2:?}"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after.start_attempt_count, 1,
        "the canonical start call must be attempted and durably counted exactly once"
    );
    assert_ne!(
        after.state,
        mqk_db::STATE_START_RETRYING,
        "evidence_degraded must never durably transition through start_retrying -- that \
         intermediate hop is reserved for the awaiting_open entry point"
    );
    assert_eq!(
        after.run_id,
        Some(run_id),
        "a failed start attempt must never silently replace the durable run_id"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 3. wrong reason code -> falls through unchanged to existing finalization
//    (self-loop), never attempts recovery
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t3_non_coverage_reason_never_recovers_falls_through_to_finalization() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    let adapter_id = format!("ev-deg-t3-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    // Overwrite the fixture's reason code to a non-eligible, genuinely
    // unsafe-shaped one via the same self-refresh CAS production uses --
    // never a raw UPDATE.
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let refresh_args = mqk_db::RefreshAutonomousDailyOperationBlockerArgs {
        operation_id,
        expected_state: row.state.clone(),
        expected_state_version: row.state_version,
        reason_code: "unknown_order_evidence_conflict".to_string(),
        blocker_signature: None,
        occurred_at_utc: now,
        bounded_detail: "test: simulate a non-coverage evidence-blocked reason".to_string(),
    };
    mqk_db::refresh_autonomous_daily_operation_blocker(&pool, &refresh_args).await?;

    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        operation.state_reason_code.as_deref(),
        Some("unknown_order_evidence_conflict")
    );
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = dispatch_by_state(&st, &pool, operation, &plan, now).await?;
    assert!(
        !matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::RecoveryScheduled
                | AutonomousDailyCoordinatorTickOutcome::Recovered { .. }
        ),
        "a non-`unknown_incomplete_bar_coverage` reason must never be recovery-eligible; \
         got {outcome:?}"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert!(
        after.next_retry_utc.is_none(),
        "an ineligible reason must never durably schedule a recovery retry"
    );
    assert_eq!(after.state, mqk_db::STATE_EVIDENCE_DEGRADED);

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 4. session window already closed -> never recovers. `dispatch_by_state`'s
//    own top-level session-close override intercepts before the
//    `evidence_degraded` arm is even reached (pre-existing, unmodified
//    behavior: a clean, terminal run past close legally advances toward
//    `stopping` via the same `reconcile_durable_run_without_local_owner`
//    seam `controller_degraded` uses) -- the invariant this test actually
//    proves is narrower and unconditional: no fresh run is ever created and
//    no recovery retry is ever scheduled once the window has closed,
//    regardless of which code path is the one that observes that.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t4_session_window_closed_never_recovers() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ev-deg-t4-{}", unique_suffix());
    let (plan, operation_id, run_id, _now) = build_fixture(&pool, &adapter_id).await?;

    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let after_close = operation.postclose_finalize_utc + Duration::seconds(1);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = dispatch_by_state(&st, &pool, operation, &plan, after_close).await?;
    assert!(
        !matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::RecoveryScheduled
                | AutonomousDailyCoordinatorTickOutcome::Recovered { .. }
        ),
        "a closed session window must never be recovery-eligible; got {outcome:?}"
    );

    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert!(
        after.next_retry_utc.is_none(),
        "no recovery retry may ever be durably scheduled once the session window is closed"
    );
    assert_eq!(
        after.run_id,
        Some(run_id),
        "no fresh run may ever be created once the session window is closed -- the original \
         terminal run_id must be unchanged"
    );
    assert_ne!(
        after.state,
        mqk_db::STATE_RUNNING,
        "a closed session window must never legally reach running"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 5. run still ARMED/RUNNING (never actually stopped) -> fail closed,
//    never recovers
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t5_run_not_actually_terminal_fails_closed() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ev-deg-t5-{}", unique_suffix());
    // Build the fixture manually (not via `build_fixture`, which always
    // drives the run to STOPPED) so the run is left genuinely RUNNING while
    // the operation row is independently forced into the post-stop
    // evidence_degraded shape -- an adversarial, contradictory fixture
    // proving the recovery path re-proves run termination itself and never
    // trusts the operation row's own `stopped_at_utc` alone.
    reset_env();
    set_resolvable_assignment_env("AAPL");
    let now = weekday_at(15, 0);
    reset_reconcile_status_clean(&pool, now).await?;
    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None).await?;
    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_identity_for_env(&adapter_id, "AAPL", now);
    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
        now,
    )
    .await?;
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(&pool, &new_run(run_id, now)).await?;
    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?; // left RUNNING -- never stopped
    seed_evidence_degraded_operation(&pool, operation_id, run_id, now).await?;

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
        "a run that is not actually terminal must fail closed regardless of the operation \
         row's own stopped_at_utc; got {outcome:?}"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(after.state, mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED);

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 6. run durably HALTED (sticky) -> never recovers, even though it is not
//    "active" in the Armed/Running sense
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t6_halted_run_never_recovers() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ev-deg-t6-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    // The fixture already drove this run to STOPPED; halt it afterward to
    // prove the strict `== Stopped` check, not merely "not Armed/Running".
    mqk_db::halt_run(&pool, run_id, now).await?;
    let run_row = mqk_db::fetch_run(&pool, run_id).await?;
    assert_eq!(run_row.status.as_str(), "HALTED", "fixture precondition");

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
        "a durably HALTED run must never be treated as safe to recover merely because it is \
         not Armed/Running; got {outcome:?}"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 7. an unacked outbox row on the terminal run -> fail closed
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t7_unresolved_outbox_fails_closed() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ev-deg-t7-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

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
                reason_code: "evidence_degraded_recovery_unresolved_outbox",
                ..
            }
        ),
        "an unresolved outbox row must fail closed rather than allow a fresh start while an \
         order may still be in flight; got {outcome:?}"
    );
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert!(after.next_retry_utc.is_none());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 8. an unapplied inbox row on the terminal run -> fail closed
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t8_unresolved_inbox_fails_closed() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ev-deg-t8-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    let inserted = mqk_db::inbox_insert_deduped(
        &pool,
        run_id,
        &format!("test-broker-msg-{}", unique_suffix()),
        serde_json::json!({"event_kind": "fill"}),
    )
    .await?;
    assert!(inserted, "fixture precondition: inbox row must be newly inserted");

    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = dispatch_by_state(&st, &pool, operation, &plan, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "evidence_degraded_recovery_unresolved_inbox",
                ..
            }
        ),
        "an unapplied inbox row must fail closed rather than allow a fresh start while broker \
         evidence is not fully applied; got {outcome:?}"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 9. dirty global reconcile status -> fail closed
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t9_dirty_reconcile_fails_closed() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ev-deg-t9-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

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
                reason_code: "evidence_degraded_recovery_reconcile_dirty",
                ..
            }
        ),
        "a dirty global reconcile status must fail closed; got {outcome:?}"
    );

    reset_reconcile_status_clean(&pool, now).await?;
    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 10. durably DISARMED arm state -> fail closed, never retried
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t10_durably_disarmed_fails_closed() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ev-deg-t10-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;

    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Disarmed, None).await?;

    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = dispatch_by_state(&st, &pool, operation, &plan, now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "durable_arm_disarmed",
                ..
            }
        ),
        "a durably DISARMED arm state must fail closed and is never automatically retried; \
         got {outcome:?}"
    );

    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None).await?;
    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 11. no run was ever bound (run_id None) -- clean reconcile is sufficient,
//     eligible for recovery
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t11_no_run_ever_bound_is_still_eligible() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ev-deg-t11-{}", unique_suffix());
    reset_env();
    set_resolvable_assignment_env("AAPL");
    let now = weekday_at(15, 0);
    reset_reconcile_status_clean(&pool, now).await?;
    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None).await?;
    let (plan, assignment_identity, runtime_binding_identity, operation_id) =
        resolve_identity_for_env(&adapter_id, "AAPL", now);
    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
        now,
    )
    .await?;
    // -> stopping -> evidence_degraded, with no run_id ever bound.
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: row.state.clone(),
        expected_state_version: row.state_version,
        new_state: mqk_db::STATE_STOPPING.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: now,
        run_id: None,
        bounded_detail: "test setup: -> stopping, no run ever bound".to_string(),
    };
    let _row = match mqk_db::transition_autonomous_daily_operation(&pool, &args).await? {
        AutonomousDailyTransitionOutcome::Applied(r) => r,
        other => panic!("expected Applied, got {other:?}"),
    };
    mqk_db::record_stopped_at(&pool, operation_id, now).await?;
    let row = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: row.state.clone(),
        expected_state_version: row.state_version,
        new_state: mqk_db::STATE_EVIDENCE_DEGRADED.to_string(),
        reason_code: Some(REASON_INCOMPLETE_BAR_COVERAGE.to_string()),
        blocker_signature: None,
        occurred_at_utc: now,
        run_id: None,
        bounded_detail: "test setup: -> evidence_degraded, no run ever bound".to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation(&pool, &args).await? {
        AutonomousDailyTransitionOutcome::Applied(_) => {}
        other => panic!("expected Applied, got {other:?}"),
    }

    let operation = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert!(operation.run_id.is_none(), "fixture precondition");
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let outcome = dispatch_by_state(&st, &pool, operation, &plan, now).await?;
    assert_eq!(
        outcome,
        AutonomousDailyCoordinatorTickOutcome::RecoveryScheduled,
        "a row that never bound a run has nothing to prove terminal and must still be \
         eligible, gated only on reason code / session window / clean reconcile; \
         got {outcome:?}"
    );

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// 12. repeated ticks before the retry is due -> idempotent, no duplicate
//     scheduling, no premature start
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn t12_repeated_ticks_before_due_are_idempotent() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("ev-deg-t12-{}", unique_suffix());
    let (plan, operation_id, run_id, now) = build_fixture(&pool, &adapter_id).await?;
    let st = paper_state_with_db(pool.clone(), &adapter_id);

    let before = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let outcome1 = dispatch_by_state(&st, &pool, before, &plan, now).await?;
    assert_eq!(outcome1, AutonomousDailyCoordinatorTickOutcome::RecoveryScheduled);
    let after_first = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    let first_retry_utc = after_first.next_retry_utc.expect("must be scheduled");

    // A second tick, still before the retry is due: must not start, and
    // must not reschedule to a different time.
    let outcome2 = dispatch_by_state(&st, &pool, after_first.clone(), &plan, now).await?;
    assert!(
        !matches!(
            outcome2,
            AutonomousDailyCoordinatorTickOutcome::Recovered { .. }
        ),
        "must never start before the scheduled retry is due; got {outcome2:?}"
    );
    let after_second = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation_id)
        .await?
        .expect("row must exist");
    assert_eq!(
        after_second.next_retry_utc,
        Some(first_retry_utc),
        "the scheduled retry time must not be silently rewritten by a not-yet-due tick"
    );
    assert_eq!(after_second.state, mqk_db::STATE_EVIDENCE_DEGRADED);

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}
