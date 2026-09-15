//! M1-STALE-MANUAL-OP-FINALIZATION-01: proof tests for
//! `POST /api/v1/autonomous/daily-operation/finalize-stale-manual`.
//!
//! DB-backed; skip without `MQK_DATABASE_URL`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_m1_stale_manual_op_finalization_01 \
//!   -- --test-threads=1 --nocapture
//!
//! Reproduces the exact production incident class diagnosed against the live
//! Paper DB on 2026-09-15: a `manual_intervention_required` operation that
//! durably bound a run (`running -> controller_degraded ->
//! manual_intervention_required`, reason `durable_active_run_without_local_
//! owner`) whose run was later independently halted (deadman) and cleared to
//! `STOPPED` via the existing `clear-halted-run` operator action --
//! `clear_halted_run_and_reset_stale_claims` never touches
//! `sys_autonomous_daily_operations`, so `stopped_at_utc` stays NULL forever
//! and `fetch_relevant_open_autonomous_daily_operation`'s REPAIR-2 clause
//! (`run_id is not null and stopped_at_utc is null` stays relevant
//! regardless of state or window) blocks every later day's operation with an
//! ambiguity refusal.
//!
//! No real provider, broker, or network call is made anywhere in this file.
//! Every fixture reaches its seeded state only through real production
//! primitives (`mqk_db::create_or_recover_autonomous_daily_operation`,
//! `mqk_db::transition_autonomous_daily_operation`, `mqk_db::insert_run`,
//! `mqk_db::arm_run`/`begin_run`/`halt_run`/`clear_halted_run`,
//! `mqk_db::outbox_enqueue`, `mqk_db::persist_reconcile_status_state`) --
//! never a raw `UPDATE` standing in for one of those.

use std::sync::Arc;

use axum::http::{Method, Request, StatusCode};
use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use http_body_util::BodyExt;
use mqk_daemon::state::{
    self, AppState, AutonomousDailyScheduleSource, AutonomousDailySessionPlan,
};
use mqk_db::{
    AutonomousDailyTransitionOutcome, CreateAutonomousDailyOperationArgs, NewRun,
    PersistReconcileStatusState, RunStatus, TransitionAutonomousDailyOperationArgs,
    STATE_AWAITING_OPEN, STATE_CONTROLLER_DEGRADED, STATE_MANUAL_INTERVENTION_REQUIRED,
    STATE_RUNNING, STATE_START_RETRYING,
};
use tower::ServiceExt;
use uuid::Uuid;

const ROUTE: &str = "/api/v1/autonomous/daily-operation/finalize-stale-manual";

// ---------------------------------------------------------------------------
// Helpers
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

fn paper_state_with_db(db: sqlx::PgPool, adapter_id: &str) -> Arc<AppState> {
    let mut st = AppState::new_for_test_with_db_mode_and_broker(
        db,
        state::DeploymentMode::Paper,
        state::BrokerKind::Paper,
    );
    st.set_adapter_id_for_test(adapter_id);
    Arc::new(st)
}

/// A fixed PAST weekday -- 2026-08-10 is a Monday. Every fixture in this file
/// that must be "prior day" uses this; a same-day fixture uses `today()`
/// relative to whatever clock override the test installs.
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
/// event log shows (verified read-only against the live Paper DB):
/// none -> awaiting_open -> start_retrying -> running(run_id) ->
/// controller_degraded -> manual_intervention_required
/// (`durable_active_run_without_local_owner`). Returns the final row.
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
        "awaiting_open",
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

/// Seeds a `runs` row that reaches durable `STOPPED` via the exact real
/// production chain (`CREATED -> ARMED -> RUNNING -> HALTED -> STOPPED`,
/// mirroring `clear-halted-run`'s own accepted contract).
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
    let _ = sqlx::query("delete from sys_autonomous_daily_bar_dispatches where operation_id = $1")
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

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json = serde_json::from_slice(&body).expect("response body is not valid JSON");
    (status, json)
}

fn finalize_req(operation_id: Uuid) -> Request<axum::body::Body> {
    let body = serde_json::json!({ "operation_id": operation_id.to_string() });
    Request::builder()
        .method(Method::POST)
        .uri(ROUTE)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

fn finalize_req_with_date(
    operation_id: Uuid,
    expected_market_date: &str,
) -> Request<axum::body::Body> {
    let body = serde_json::json!({
        "operation_id": operation_id.to_string(),
        "expected_market_date": expected_market_date,
    });
    Request::builder()
        .method(Method::POST)
        .uri(ROUTE)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
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
// P1 (LOAD-BEARING): exact production ambiguity class -- RED before repair,
// GREEN after.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn p1_stale_manual_op_ambiguity_red_then_green() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-p1-{}", unique_suffix());
    let stale_date = fixed_past_day();
    let today_date = stale_date + chrono::Duration::days(4); // still a weekday window for this fixture's own plan math
    let stale_plan = fixed_plan(stale_date);
    let today_plan = fixed_plan(today_date);
    let stale_run_id = Uuid::new_v4();
    let stale_operation_id = Uuid::new_v4();
    let today_operation_id = Uuid::new_v4();

    let stale_seed_now = stale_plan.effective_operation_open_utc + chrono::Duration::minutes(30);
    seed_orphaned_manual_intervention_operation(
        &pool,
        &stale_plan,
        stale_operation_id,
        &adapter_id,
        stale_run_id,
        stale_seed_now,
    )
    .await?;
    seed_stopped_run(
        &pool,
        stale_run_id,
        stale_seed_now + chrono::Duration::hours(4),
    )
    .await?;
    seed_clean_reconcile(&pool, stale_seed_now + chrono::Duration::hours(5)).await?;

    // TODAY's fresh operation: a plain non-terminal row (matches production
    // shape: freshly created, `awaiting_preopen`).
    let today_now = today_plan.effective_operation_open_utc + chrono::Duration::minutes(1);
    seed_operation_row(
        &pool,
        &today_plan,
        today_operation_id,
        &adapter_id,
        today_now,
        "awaiting_preopen",
    )
    .await?;

    // --- RED: before repair, exactly two equally-authoritative rows. ---
    let ambiguity = mqk_db::fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "PAPER",
        &adapter_id,
        today_now,
    )
    .await;
    let err = ambiguity.expect_err("RED: must fail closed with ambiguity before repair");
    assert!(
        err.to_string().contains("equally authoritative"),
        "RED: unexpected error shape: {err}"
    );

    // --- Apply the repair: call the new route on the exact stale operation. ---
    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_daily_data_readiness_clock_override_for_test(Some(today_now))
        .await;
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(stale_operation_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "repair call must succeed: {json:?}");
    let truth_state = json["truth_state"].as_str().unwrap_or_default();
    assert!(
        truth_state == "finalized" || truth_state == "stopped_not_finalized",
        "unexpected truth_state: {json:?}"
    );

    // Fixed safety fields must never move.
    assert_eq!(json["runtime_started"], false);
    assert_eq!(json["arm_modified"], false);
    assert_eq!(json["halt_changed"], false);
    assert_eq!(json["reconcile_changed"], false);
    assert_eq!(json["orders_submitted"], 0);

    let stale_row = fetch_row(&pool, stale_operation_id).await;
    assert!(
        stale_row.stopped_at_utc.is_some(),
        "stale row must have durable stop truth after repair"
    );
    assert_ne!(
        stale_row.state, STATE_MANUAL_INTERVENTION_REQUIRED,
        "stale row must have left manual_intervention_required"
    );
    // A18: this route must never be usable as a retry/start seam -- it can
    // only ever move the operation toward stopping/terminal states, never
    // back onto an on-ramp state.
    assert!(
        !matches!(
            stale_row.state.as_str(),
            "preparing_data" | "awaiting_open" | "start_retrying" | "running"
        ),
        "route must never re-enter an on-ramp state: {}",
        stale_row.state
    );

    // Original reason/provenance must remain auditable in the event log.
    let events: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "select from_state, to_state, reason_code from sys_autonomous_daily_operation_events \
         where operation_id = $1 order by transition_seq",
    )
    .bind(stale_operation_id)
    .fetch_all(&pool)
    .await?;
    assert!(
        events
            .iter()
            .any(|(_, _, reason)| reason.as_deref()
                == Some("durable_active_run_without_local_owner")),
        "original blocker reason must remain in the auditable event log: {events:?}"
    );

    // --- GREEN: after repair, only today's operation is relevant. ---
    let resolved = mqk_db::fetch_relevant_open_autonomous_daily_operation(
        &pool,
        "PAPER",
        &adapter_id,
        today_now,
    )
    .await?
    .expect("GREEN: today's operation must resolve unambiguously");
    assert_eq!(
        resolved.operation_id, today_operation_id,
        "GREEN: the ONLY relevant operation must be today's"
    );

    cleanup_operation(&pool, stale_operation_id).await;
    cleanup_operation(&pool, today_operation_id).await;
    cleanup_run(&pool, stale_run_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Negative controls
// ---------------------------------------------------------------------------

/// A1: a current-calendar-day operation must be refused, even with a bound,
/// safely-terminal run.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n1_current_day_operation_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-n1-{}", unique_suffix());
    // Use "today" per the daemon's own default clock (Utc::now()) so the
    // operation's market_date really is >= now_utc.date_naive() without any
    // clock override on the route side.
    let today = Utc::now().date_naive();
    let plan = fixed_plan(today);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    seed_orphaned_manual_intervention_operation(
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
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{json:?}");
    assert_eq!(json["truth_state"], "not_prior_day");
    let row = fetch_row(&pool, operation_id).await;
    assert_eq!(row.state, STATE_MANUAL_INTERVENTION_REQUIRED);
    assert!(row.stopped_at_utc.is_none());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

/// A2: a non-PAPER deployment_mode operation must be refused.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n2_non_paper_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-n2-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    let create_args = CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date,
        deployment_mode: "LIVE-SHADOW".to_string(),
        adapter_id: adapter_id.clone(),
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
        initial_state: STATE_AWAITING_OPEN.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: now,
        bounded_detail: "test fixture seed".to_string(),
        stop_attempt_count: 0,
    };
    mqk_db::create_or_recover_autonomous_daily_operation(&pool, &create_args).await?;
    let start_retrying = real_transition(
        &pool,
        operation_id,
        STATE_AWAITING_OPEN,
        1,
        STATE_START_RETRYING,
        None,
        None,
        now,
        "test",
    )
    .await;
    real_transition(
        &pool,
        operation_id,
        STATE_START_RETRYING,
        start_retrying.state_version,
        STATE_MANUAL_INTERVENTION_REQUIRED,
        Some("durable_active_run_without_local_owner"),
        Some(run_id),
        now,
        "test",
    )
    .await;
    seed_stopped_run(&pool, run_id, now + chrono::Duration::hours(1)).await?;
    seed_clean_reconcile(&pool, now + chrono::Duration::hours(1)).await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_daily_data_readiness_clock_override_for_test(Some(
        plan.effective_operation_close_utc
            + chrono::Duration::days(1)
            + chrono::Duration::minutes(5),
    ))
    .await;
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{json:?}");
    assert_eq!(json["truth_state"], "not_authorized");

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

/// A3: a nonexistent operation_id must be refused (404, zero mutation).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n3_nonexistent_operation_is_not_found() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-n3-{}", unique_suffix());
    let st = paper_state_with_db(pool, &adapter_id);
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(Uuid::new_v4()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json:?}");
    assert_eq!(json["truth_state"], "not_found");
    Ok(())
}

/// A4/A16: expected_market_date mismatch must be refused.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n4_market_date_mismatch_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-n4-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    seed_orphaned_manual_intervention_operation(
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
    st.set_daily_data_readiness_clock_override_for_test(Some(
        plan.effective_operation_close_utc
            + chrono::Duration::days(1)
            + chrono::Duration::minutes(5),
    ))
    .await;
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req_with_date(operation_id, "1999-01-01"),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{json:?}");
    assert_eq!(json["truth_state"], "identity_mismatch");
    let row = fetch_row(&pool, operation_id).await;
    assert!(row.stopped_at_utc.is_none());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

/// A5: any state other than manual_intervention_required must be refused
/// (here: running).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n5_wrong_state_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-n5-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        now,
        STATE_AWAITING_OPEN,
    )
    .await?;
    let start_retrying = real_transition(
        &pool,
        operation_id,
        STATE_AWAITING_OPEN,
        1,
        STATE_START_RETRYING,
        None,
        None,
        now,
        "test",
    )
    .await;
    let running = real_transition(
        &pool,
        operation_id,
        STATE_START_RETRYING,
        start_retrying.state_version,
        STATE_RUNNING,
        None,
        Some(run_id),
        now,
        "test",
    )
    .await;
    assert_eq!(running.state, STATE_RUNNING);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_daily_data_readiness_clock_override_for_test(Some(
        plan.effective_operation_close_utc
            + chrono::Duration::days(1)
            + chrono::Duration::minutes(5),
    ))
    .await;
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{json:?}");
    assert_eq!(json["truth_state"], "not_manual_intervention_required");
    let row = fetch_row(&pool, operation_id).await;
    assert_eq!(row.state, STATE_RUNNING, "state must be unchanged");

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

/// A6: manual_intervention_required with no run_id (a pre-start blocker
/// shape) must be refused -- the retry route is the correct seam instead.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n6_no_run_id_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-n6-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        now,
        "awaiting_preopen",
    )
    .await?;
    real_transition(
        &pool,
        operation_id,
        "awaiting_preopen",
        1,
        STATE_MANUAL_INTERVENTION_REQUIRED,
        Some("interior_gap"),
        None,
        now,
        "test: prestart blocker, never bound a run",
    )
    .await;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_daily_data_readiness_clock_override_for_test(Some(
        plan.effective_operation_close_utc
            + chrono::Duration::days(1)
            + chrono::Duration::minutes(5),
    ))
    .await;
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{json:?}");
    assert_eq!(json["truth_state"], "no_run_id");

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

/// A7: linked run row missing must be refused.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n7_run_missing_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-n7-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4(); // never inserted into `runs`
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    seed_orphaned_manual_intervention_operation(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        run_id,
        now,
    )
    .await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_daily_data_readiness_clock_override_for_test(Some(
        plan.effective_operation_close_utc
            + chrono::Duration::days(1)
            + chrono::Duration::minutes(5),
    ))
    .await;
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{json:?}");
    assert_eq!(json["truth_state"], "run_missing");
    let row = fetch_row(&pool, operation_id).await;
    assert!(row.stopped_at_utc.is_none());

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

/// A8: linked run still RUNNING must be refused.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n8_run_running_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-n8-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    seed_orphaned_manual_intervention_operation(
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
    mqk_db::begin_run(&pool, run_id).await?; // left RUNNING, never halted/stopped

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_daily_data_readiness_clock_override_for_test(Some(
        plan.effective_operation_close_utc
            + chrono::Duration::days(1)
            + chrono::Duration::minutes(5),
    ))
    .await;
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{json:?}");
    assert_eq!(json["truth_state"], "run_not_stopped");
    let row = fetch_row(&pool, operation_id).await;
    assert!(row.stopped_at_utc.is_none());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

/// A9: linked run HALTED (not cleared) must be refused -- a halt stays
/// visible; never silently released by this route.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n9_run_halted_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-n9-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    seed_orphaned_manual_intervention_operation(
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
    mqk_db::begin_run(&pool, run_id).await?;
    mqk_db::halt_run(&pool, run_id, now).await?; // HALTED, never cleared

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_daily_data_readiness_clock_override_for_test(Some(
        plan.effective_operation_close_utc
            + chrono::Duration::days(1)
            + chrono::Duration::minutes(5),
    ))
    .await;
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{json:?}");
    assert_eq!(json["truth_state"], "run_not_stopped");
    let row = fetch_row(&pool, operation_id).await;
    assert!(row.stopped_at_utc.is_none());
    let run_row = mqk_db::fetch_run(&pool, run_id).await?;
    assert!(
        matches!(run_row.status, RunStatus::Halted),
        "the halt itself must never be silently cleared by this route"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

/// A10: unacked outbox row for the stale run must be refused.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n10_unacked_outbox_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-n10-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    seed_orphaned_manual_intervention_operation(
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
        &format!("m1-n10-order-{}", unique_suffix()),
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
    st.set_daily_data_readiness_clock_override_for_test(Some(
        plan.effective_operation_close_utc
            + chrono::Duration::days(1)
            + chrono::Duration::minutes(5),
    ))
    .await;
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{json:?}");
    assert_eq!(json["truth_state"], "unresolved_outbox_at_run_reconcile");
    let row = fetch_row(&pool, operation_id).await;
    assert!(row.stopped_at_utc.is_none());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

/// A11: unapplied inbox row for the stale run must be refused.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n11_unapplied_inbox_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-n11-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    seed_orphaned_manual_intervention_operation(
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
    mqk_db::inbox_insert_deduped(
        &pool,
        run_id,
        &format!("m1-n11-msg-{}", unique_suffix()),
        serde_json::json!({
            "type": "fill",
            "broker_message_id": "m1-n11-msg",
            "internal_order_id": "m1-n11-order",
            "broker_order_id": null,
            "symbol": "AAPL",
            "side": "Buy",
            "delta_qty": 1_i64,
            "price_micros": 100_000_000_i64,
            "fee_micros": 0_i64
        }),
    )
    .await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_daily_data_readiness_clock_override_for_test(Some(
        plan.effective_operation_close_utc
            + chrono::Duration::days(1)
            + chrono::Duration::minutes(5),
    ))
    .await;
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{json:?}");
    assert_eq!(json["truth_state"], "unresolved_inbox_at_run_reconcile");
    let row = fetch_row(&pool, operation_id).await;
    assert!(row.stopped_at_utc.is_none());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

/// A12: dirty global reconcile status must be refused.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n12_dirty_reconcile_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-n12-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    seed_orphaned_manual_intervention_operation(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        run_id,
        now,
    )
    .await?;
    seed_stopped_run(&pool, run_id, now + chrono::Duration::hours(1)).await?;
    mqk_db::persist_reconcile_status_state(
        &pool,
        &PersistReconcileStatusState {
            status: "dirty",
            last_run_at_utc: Some(now),
            snapshot_watermark_ms: None,
            mismatched_positions: 1,
            mismatched_orders: 0,
            mismatched_fills: 0,
            unmatched_broker_events: 0,
            note: Some("test: dirty reconcile fixture"),
            updated_at_utc: now,
        },
    )
    .await?;

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_daily_data_readiness_clock_override_for_test(Some(
        plan.effective_operation_close_utc
            + chrono::Duration::days(1)
            + chrono::Duration::minutes(5),
    ))
    .await;
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{json:?}");
    assert_eq!(json["truth_state"], "reconcile_dirty");
    let row = fetch_row(&pool, operation_id).await;
    assert!(row.stopped_at_utc.is_none());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

/// A13: this daemon process still locally owning the linked run must be
/// refused, zero mutation -- mirrors `clear-halted-run`'s own local-
/// quiescence gate (H08).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n13_local_execution_loop_active_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-n13-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    seed_orphaned_manual_intervention_operation(
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
    st.set_daily_data_readiness_clock_override_for_test(Some(
        plan.effective_operation_close_utc
            + chrono::Duration::days(1)
            + chrono::Duration::minutes(5),
    ))
    .await;
    st.inject_running_loop_for_test(run_id).await;
    assert_eq!(
        st.locally_owned_run_id().await,
        Some(run_id),
        "fixture precondition"
    );

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "{json:?}");
    assert_eq!(json["truth_state"], "local_execution_loop_active");
    let row = fetch_row(&pool, operation_id).await;
    assert!(row.stopped_at_utc.is_none());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

/// A17: wrong adapter_id must be refused.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n17_wrong_adapter_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let seed_adapter = format!("m1-n17-seed-{}", unique_suffix());
    let daemon_adapter = format!("m1-n17-daemon-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    seed_orphaned_manual_intervention_operation(
        &pool,
        &plan,
        operation_id,
        &seed_adapter,
        run_id,
        now,
    )
    .await?;
    seed_stopped_run(&pool, run_id, now + chrono::Duration::hours(1)).await?;
    seed_clean_reconcile(&pool, now + chrono::Duration::hours(1)).await?;

    // Daemon configured for a DIFFERENT adapter than the seeded operation.
    let st = paper_state_with_db(pool.clone(), &daemon_adapter);
    st.set_daily_data_readiness_clock_override_for_test(Some(
        plan.effective_operation_close_utc
            + chrono::Duration::days(1)
            + chrono::Duration::minutes(5),
    ))
    .await;
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{json:?}");
    assert_eq!(json["truth_state"], "wrong_adapter");
    let row = fetch_row(&pool, operation_id).await;
    assert!(row.stopped_at_utc.is_none());

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}

/// N14 (CAS/replay-safety): a repeat call after a prior successful
/// terminalization must not re-mutate or fabricate a second outcome -- the
/// operation has already left `manual_intervention_required`, so the second
/// call is refused by the ordinary state check, exactly like any other
/// concurrent-loser CAS outcome in this codebase.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n14_replay_after_success_is_refused_not_remutated() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("m1-n14-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let run_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let now = plan.effective_operation_open_utc + chrono::Duration::minutes(30);

    seed_orphaned_manual_intervention_operation(
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
    st.set_daily_data_readiness_clock_override_for_test(Some(
        plan.effective_operation_close_utc
            + chrono::Duration::days(1)
            + chrono::Duration::minutes(5),
    ))
    .await;

    let (status1, json1) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;
    assert_eq!(status1, StatusCode::OK, "{json1:?}");
    let row_after_first = fetch_row(&pool, operation_id).await;
    let state_version_after_first = row_after_first.state_version;

    let (status2, json2) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;
    assert_eq!(
        status2,
        StatusCode::CONFLICT,
        "replay must be refused, not re-applied: {json2:?}"
    );
    assert_eq!(json2["truth_state"], "not_manual_intervention_required");

    let row_after_second = fetch_row(&pool, operation_id).await;
    assert_eq!(
        row_after_second.state_version, state_version_after_first,
        "replay must not mutate state_version a second time"
    );

    cleanup_operation(&pool, operation_id).await;
    cleanup_run(&pool, run_id).await;
    Ok(())
}
