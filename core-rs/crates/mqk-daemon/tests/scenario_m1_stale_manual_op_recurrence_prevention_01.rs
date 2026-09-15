//! M1-STALE-MANUAL-OP-RECURRENCE-PREVENTION-01: proof that the ordinary
//! coordinator tick (`dispatch_by_state`), not just the operator-triggered
//! `finalize-stale-manual` route, now reaches durable stop truth for a
//! `manual_intervention_required` operation whose bound run has since been
//! independently proven safely terminal.
//!
//! DB-backed; skip without `MQK_DATABASE_URL`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_m1_stale_manual_op_recurrence_prevention_01 \
//!   -- --test-threads=1 --nocapture --ignored
//!
//! Root cause this closes (M1-STALE-MANUAL-OP-FINALIZATION-01's manifest,
//! Phase E): `STATE_MANUAL_INTERVENTION_REQUIRED`'s ordinary tick arm in
//! `autonomous_daily_coordinator::dispatch_by_state` only ever attempted the
//! narrow "data-repairable preflight" automatic recovery -- unlike its
//! sibling `STATE_CONTROLLER_DEGRADED`, it never called
//! `reconcile_durable_run_without_local_owner` to check whether a bound run
//! had since become durably terminal. Any operation that reaches
//! `manual_intervention_required` with a bound `run_id` for a non-data-
//! repairable reason (e.g. `durable_active_run_without_local_owner`, an
//! orphaned run later halted by the deadman safety mechanism and cleared to
//! STOPPED via the `clear-halted-run` operator action -- which never touches
//! `sys_autonomous_daily_operations`) stayed relevant forever per
//! `fetch_relevant_open_autonomous_daily_operation`'s REPAIR-2 clause,
//! deterministically reproducing the exact 2026-09-11 incident shape on
//! every future day, requiring the same manual operator repair indefinitely.
//!
//! No real provider, broker, or network call is made anywhere in this file.

use std::sync::Arc;

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use mqk_daemon::state::autonomous_daily_coordinator::{
    dispatch_by_state, AutonomousDailyCoordinatorTickOutcome,
};
use mqk_daemon::state::{
    self, AppState, AutonomousDailyScheduleSource, AutonomousDailySessionPlan,
};
use mqk_db::{
    AutonomousDailyTransitionOutcome, CreateAutonomousDailyOperationArgs, NewRun,
    PersistReconcileStatusState, TransitionAutonomousDailyOperationArgs, STATE_AWAITING_OPEN,
    STATE_CONTROLLER_DEGRADED, STATE_MANUAL_INTERVENTION_REQUIRED, STATE_RUNNING,
    STATE_START_RETRYING,
};
use uuid::Uuid;

async fn test_pool() -> anyhow::Result<sqlx::PgPool> {
    if std::env::var("MQK_DATABASE_URL").is_err() {
        anyhow::bail!("SKIP: requires MQK_DATABASE_URL");
    }
    mqk_db::testkit_db_pool().await
}

fn unique_suffix() -> String {
    Uuid::new_v4().to_string().replace('-', "")[..10].to_string()
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

fn fixed_past_day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 8, 10).expect("valid date")
}

fn fixed_plan(market_date: NaiveDate) -> AutonomousDailySessionPlan {
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
    let close = open + chrono::Duration::hours(6) + chrono::Duration::minutes(30);
    AutonomousDailySessionPlan {
        market_date: market_date.format("%Y-%m-%d").to_string(),
        previous_trading_date: (market_date - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string(),
        exchange_session_open_utc: open,
        exchange_session_close_utc: close,
        exchange_is_early_close: false,
        effective_operation_open_utc: open - chrono::Duration::minutes(30),
        effective_operation_close_utc: close + chrono::Duration::minutes(15),
        calendar_source: "nyse_weekdays_heuristic".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: AutonomousDailyScheduleSource::FixedWindowOverride,
        preopen_start_utc: open - chrono::Duration::hours(11),
        postclose_finalize_utc: close + chrono::Duration::minutes(15),
        session_plan_identity: format!("test-plan-{market_date}"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn seed_operation_row(
    pool: &sqlx::PgPool,
    plan: &AutonomousDailySessionPlan,
    operation_id: Uuid,
    adapter_id: &str,
    now_utc: DateTime<Utc>,
    initial_state: &str,
) -> anyhow::Result<()> {
    let create_args = CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date: NaiveDate::parse_from_str(&plan.market_date, "%Y-%m-%d")?,
        deployment_mode: "PAPER".to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity: plan.session_plan_identity.clone(),
        assignment_identity: "test-assignment".to_string(),
        runtime_binding_identity: "test-runtime-binding".to_string(),
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
        initial_state: initial_state.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: now_utc,
        bounded_detail: "test fixture seed".to_string(),
        stop_attempt_count: 0,
    };
    mqk_db::create_or_recover_autonomous_daily_operation(pool, &create_args).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn real_transition(
    pool: &sqlx::PgPool,
    operation_id: Uuid,
    expected_state: &str,
    expected_state_version: i64,
    new_state: &str,
    reason_code: Option<&str>,
    run_id: Option<Uuid>,
    now_utc: DateTime<Utc>,
    detail: &str,
) -> mqk_db::AutonomousDailyOperationRecord {
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id,
        expected_state: expected_state.to_string(),
        expected_state_version,
        new_state: new_state.to_string(),
        reason_code: reason_code.map(|s| s.to_string()),
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id,
        bounded_detail: detail.to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation(pool, &args)
        .await
        .expect("transition query must not fail")
    {
        AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    }
}

/// Reproduces the exact real chain the 2026-09-11 production incident's own
/// event log shows: none -> awaiting_open -> start_retrying -> running(run_id)
/// -> controller_degraded -> manual_intervention_required
/// (`durable_active_run_without_local_owner`).
async fn seed_orphaned_manual_intervention_operation(
    pool: &sqlx::PgPool,
    plan: &AutonomousDailySessionPlan,
    operation_id: Uuid,
    adapter_id: &str,
    run_id: Uuid,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<mqk_db::AutonomousDailyOperationRecord> {
    seed_operation_row(
        pool,
        plan,
        operation_id,
        adapter_id,
        now_utc,
        STATE_AWAITING_OPEN,
    )
    .await?;
    let start_retrying = real_transition(
        pool,
        operation_id,
        STATE_AWAITING_OPEN,
        1,
        STATE_START_RETRYING,
        None,
        None,
        now_utc,
        "test: enter start_retrying",
    )
    .await;
    let running = real_transition(
        pool,
        operation_id,
        STATE_START_RETRYING,
        start_retrying.state_version,
        STATE_RUNNING,
        None,
        Some(run_id),
        now_utc,
        "test: canonical start succeeded",
    )
    .await;
    let degraded = real_transition(
        pool,
        operation_id,
        STATE_RUNNING,
        running.state_version,
        STATE_CONTROLLER_DEGRADED,
        Some("interior_gap"),
        Some(run_id),
        now_utc,
        "test: reason_code=interior_gap",
    )
    .await;
    let manual = real_transition(
        pool,
        operation_id,
        STATE_CONTROLLER_DEGRADED,
        degraded.state_version,
        STATE_MANUAL_INTERVENTION_REQUIRED,
        Some("durable_active_run_without_local_owner"),
        Some(run_id),
        now_utc,
        "test: the durable run row is still armed/running but no local runtime owns it",
    )
    .await;
    assert_eq!(manual.state, STATE_MANUAL_INTERVENTION_REQUIRED);
    assert_eq!(manual.run_id, Some(run_id));
    assert!(manual.stopped_at_utc.is_none(), "fixture precondition");
    Ok(manual)
}

async fn seed_stopped_run(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    mqk_db::insert_run(
        pool,
        &NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(pool, run_id).await?;
    mqk_db::begin_run(pool, run_id).await?;
    mqk_db::halt_run(pool, run_id, now).await?;
    mqk_db::clear_halted_run(pool, run_id).await?;
    Ok(())
}

async fn seed_clean_reconcile(pool: &sqlx::PgPool, now: DateTime<Utc>) -> anyhow::Result<()> {
    mqk_db::persist_reconcile_status_state(
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
    .await?;
    Ok(())
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
    let _ = sqlx::query(
        "delete from broker_order_map where internal_id in \
         (select idempotency_key from oms_outbox where run_id = $1)",
    )
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

async fn fetch_row(
    pool: &sqlx::PgPool,
    operation_id: Uuid,
) -> mqk_db::AutonomousDailyOperationRecord {
    mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id)
        .await
        .expect("fetch must not fail")
        .expect("row must exist")
}

// ---------------------------------------------------------------------------
// GREEN: an ordinary coordinator tick, with no operator action at all, must
// now reach durable stop truth once the bound run is independently proven
// safely terminal.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn green_ordinary_tick_reconciles_terminal_run_without_operator_action() -> anyhow::Result<()>
{
    let pool = test_pool().await?;
    let adapter_id = format!("m1-rec-green-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    let manual = seed_orphaned_manual_intervention_operation(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        run_id,
        now,
    )
    .await?;
    seed_stopped_run(&pool, run_id, now + chrono::Duration::hours(1)).await?;
    seed_clean_reconcile(&pool, now + chrono::Duration::hours(1)).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let tick_now = now + chrono::Duration::hours(2);

    // No route call, no CAS/version supplied by a human -- exactly the
    // ordinary per-tick call shape `session_controller.rs` uses in
    // production.
    let outcome = dispatch_by_state(&st, &pool, manual, &plan, tick_now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::RuntimeStopped
        ),
        "GREEN: an ordinary tick must reach RuntimeStopped once the bound run is proven \
         terminal; got {outcome:?}"
    );

    let row = fetch_row(&pool, operation_id).await;
    assert!(
        row.stopped_at_utc.is_some(),
        "GREEN: durable stop truth must be recorded"
    );
    assert_ne!(
        row.state, STATE_MANUAL_INTERVENTION_REQUIRED,
        "GREEN: operation must have left manual_intervention_required"
    );

    // Recurrence proof proper: once a LATER day's `now_utc` falls outside
    // this stale operation's own persisted preopen..postclose window (the
    // "is this still today's own operation" clause no longer applies), it
    // must no longer be relevant -- so it can never again block a later
    // day's fresh operation with an ambiguity refusal.
    let later_day_now = plan.postclose_finalize_utc + chrono::Duration::days(1);
    let relevant = mqk_db::fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "PAPER",
        &adapter_id,
        later_day_now,
    )
    .await?;
    assert!(
        relevant.is_none(),
        "GREEN: the reconciled row must no longer be relevant on a later day: {relevant:?}"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Negative control: a genuinely active (RUNNING) linked run must NOT be
// auto-closed by an ordinary tick.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn negative_control_active_run_is_not_auto_closed() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-rec-neg1-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    let manual = seed_orphaned_manual_intervention_operation(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        run_id,
        now,
    )
    .await?;
    mqk_db::insert_run(
        &pool,
        &NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?; // left RUNNING -- genuinely active

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let tick_now = now + chrono::Duration::hours(2);

    let outcome = dispatch_by_state(&st, &pool, manual, &plan, tick_now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired { .. }
        ),
        "negative control: an active RUNNING run must never be auto-closed; got {outcome:?}"
    );

    let row = fetch_row(&pool, operation_id).await;
    assert_eq!(row.state, STATE_MANUAL_INTERVENTION_REQUIRED);
    assert!(row.stopped_at_utc.is_none());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Negative control: economically-unsettled evidence (unacked outbox) must
// NOT be auto-closed even once the run itself is terminal.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn negative_control_unresolved_outbox_is_not_auto_closed() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-rec-neg2-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    let manual = seed_orphaned_manual_intervention_operation(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        run_id,
        now,
    )
    .await?;
    seed_stopped_run(&pool, run_id, now + chrono::Duration::hours(1)).await?;
    seed_clean_reconcile(&pool, now + chrono::Duration::hours(1)).await?;
    mqk_db::outbox_enqueue(
        &pool,
        run_id,
        &format!("m1-rec-neg2-order-{}", unique_suffix()),
        serde_json::json!({
            "symbol": "AAPL",
            "side": "Buy",
            "qty": 1,
            "order_type": "Market",
            "time_in_force": "Day"
        }),
    )
    .await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let tick_now = now + chrono::Duration::hours(2);

    let outcome = dispatch_by_state(&st, &pool, manual, &plan, tick_now).await?;
    assert!(
        matches!(
            outcome,
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired { .. }
        ),
        "negative control: unresolved outbox evidence must never be auto-closed; got {outcome:?}"
    );

    let row = fetch_row(&pool, operation_id).await;
    assert_eq!(row.state, STATE_MANUAL_INTERVENTION_REQUIRED);
    assert!(row.stopped_at_utc.is_none());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}
