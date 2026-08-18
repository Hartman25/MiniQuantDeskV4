//! AUTONOMOUS-DAILY-STALE-EVIDENCE-DEGRADED-FINALIZATION-01: proof tests for
//! `POST /api/v1/autonomous/daily-operation/finalize-stale`.
//!
//! DB-backed; skip without `MQK_DATABASE_URL`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_autonomous_daily_stale_evidence_degraded_finalization_01 \
//!   -- --test-threads=1 --nocapture
//!
//! No real provider, broker, or network call is made anywhere in this file.
//! Every fixture reaches its seeded state only through the real production
//! CAS primitive (`mqk_db::transition_autonomous_daily_operation`) or the
//! real creation primitive (`mqk_db::create_or_recover_autonomous_daily_operation`)
//! — never a raw `UPDATE`.
//!
//! This route exists to unstick a PRIOR-DAY operation stuck in
//! `evidence_degraded` (with `stopped_at_utc` already set) that blocks every
//! later day's ordinary coordinator tick via
//! `fetch_relevant_open_autonomous_daily_operation`'s ambiguity guard. The
//! negative controls below are the load-bearing proof: this route must
//! refuse an operation that is not exactly in that narrow state, in any of
//! the independent ways it could fail to qualify.

use std::sync::Arc;

use axum::http::{Method, Request, StatusCode};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use http_body_util::BodyExt;
use mqk_daemon::state::{self, AppState, AutonomousDailyScheduleSource, AutonomousDailySessionPlan};
use mqk_db::{
    AutonomousDailyTransitionOutcome, CreateAutonomousDailyOperationArgs,
    TransitionAutonomousDailyOperationArgs, STATE_EVIDENCE_DEGRADED, STATE_RUNNING,
};
use uuid::Uuid;

const ROUTE: &str = "/api/v1/autonomous/daily-operation/finalize-stale";

// ---------------------------------------------------------------------------
// Helpers (duplicated per-file by this repo's own test-binary convention —
// see scenario_autonomous_daily_operator_retry_01.rs for the same pattern)
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

/// A fixed PAST weekday, deliberately far behind any test's real wall clock
/// — 2026-08-10 is a Monday. Every fixture in this file represents an
/// operation whose own session window has already closed; none of them need
/// real market-data rows (this route never reads md_bars).
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
        previous_trading_date: (market_date - chrono::Duration::days(1)).format("%Y-%m-%d").to_string(),
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

use chrono::Datelike;

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
        // (types now match CreateAutonomousDailyOperationArgs exactly: plain
        // DateTime<Utc>/bool, not Option — see mqk-db::autonomous_daily_operation)
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

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, serde_json::Value) {
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

use tower::ServiceExt;

fn finalize_req(operation_id: Uuid) -> Request<axum::body::Body> {
    let body = serde_json::json!({ "operation_id": operation_id.to_string() });
    Request::builder()
        .method(Method::POST)
        .uri(ROUTE)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

async fn fetch_row(pool: &sqlx::PgPool, operation_id: Uuid) -> mqk_db::AutonomousDailyOperationRecord {
    mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation_id)
        .await
        .expect("fetch must not fail")
        .expect("row must exist")
}

// ---------------------------------------------------------------------------
// Negative control #1: a genuinely active/in-progress (running) operation
// must never be finalized by this route.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n1_running_operation_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("stale-fin-n1-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let now = plan.effective_operation_close_utc + chrono::Duration::minutes(5);
    let operation_id = Uuid::new_v4();

    // Real legal chain (mqk_db::is_legal_operation_transition): the daemon
    // itself only ever creates a fresh row at `awaiting_open`/
    // `awaiting_preopen`/`preparing_data`/`calendar_unavailable` -- there is
    // no direct `-> running` initial state. `awaiting_open -> start_retrying
    // -> running` is the real path, mirroring the same chain
    // `dispatch_by_state` drives in production.
    seed_operation_row(&pool, &plan, operation_id, &adapter_id, now, "awaiting_open").await?;
    let start_retrying = real_transition(
        &pool,
        operation_id,
        "awaiting_open",
        1,
        "start_retrying",
        None,
        None,
        now,
        "test: enter start_retrying",
    )
    .await;
    let run_id = Uuid::new_v4();
    let running = real_transition(
        &pool,
        operation_id,
        "start_retrying",
        start_retrying.state_version,
        STATE_RUNNING,
        None,
        Some(run_id),
        now,
        "test: enter running with a real run_id",
    )
    .await;
    assert_eq!(running.state, STATE_RUNNING);

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "running operation must be refused: {json:?}");
    assert_eq!(json["truth_state"], "not_evidence_degraded");

    let row = fetch_row(&pool, operation_id).await;
    assert_eq!(row.state, STATE_RUNNING, "state must be completely unchanged");

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Negative control #2: evidence_degraded but stopped_at_utc still NULL
// (the "mid-run degraded" shape, never finalization-eligible) must refuse.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n2_evidence_degraded_without_stopped_at_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("stale-fin-n2-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let now = plan.effective_operation_close_utc + chrono::Duration::minutes(5);
    let operation_id = Uuid::new_v4();

    // Real legal chain for the "mid-run degrade" shape: `running ->
    // evidence_degraded` is directly legal (mqk_db::is_legal_operation_
    // transition) and never passes through `stopping` -- this is exactly
    // why `stopped_at_utc` stays NULL for this shape (see
    // autonomous_daily_coordinator.rs's own doc comment on that gate). This
    // also happens to carry a real run_id, so it is doubly ineligible; the
    // assertion below still isolates the stopped_at_utc gate specifically
    // by checking the exact refusal reason this route's ordering produces.
    seed_operation_row(&pool, &plan, operation_id, &adapter_id, now, "awaiting_open").await?;
    let start_retrying = real_transition(
        &pool, operation_id, "awaiting_open", 1, "start_retrying", None, None, now,
        "test: enter start_retrying",
    )
    .await;
    let run_id = Uuid::new_v4();
    let running = real_transition(
        &pool,
        operation_id,
        "start_retrying",
        start_retrying.state_version,
        STATE_RUNNING,
        None,
        Some(run_id),
        now,
        "test: enter running",
    )
    .await;
    let degraded = real_transition(
        &pool,
        operation_id,
        STATE_RUNNING,
        running.state_version,
        STATE_EVIDENCE_DEGRADED,
        Some("unknown_incomplete_bar_coverage"),
        Some(run_id),
        now,
        "test: degrade directly from running -- never sets stopped_at_utc",
    )
    .await;
    assert_eq!(degraded.state, STATE_EVIDENCE_DEGRADED);
    assert!(degraded.stopped_at_utc.is_none(), "fixture precondition");

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "no stopped_at_utc must be refused: {json:?}");
    assert_eq!(json["truth_state"], "not_stopped");

    let row = fetch_row(&pool, operation_id).await;
    assert_eq!(row.state, STATE_EVIDENCE_DEGRADED, "state must be completely unchanged");
    assert!(row.stopped_at_utc.is_none());

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Negative control #3: session window not yet closed -- this route may
// never touch the current/live trading day's operation, even if every other
// field would otherwise qualify.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn n3_session_not_yet_closed_is_refused() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("stale-fin-n3-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    // `now` is BEFORE this operation's own session close -- the one field
    // this route checks directly against the operation's own record.
    let now_mid_session = plan.effective_operation_open_utc + chrono::Duration::hours(1);
    let operation_id = Uuid::new_v4();

    seed_operation_row(
        &pool,
        &plan,
        operation_id,
        &adapter_id,
        now_mid_session,
        "awaiting_open",
    )
    .await?;
    // Real legal chain: awaiting_open -> stopping (sets stopped_at_utc) ->
    // evidence_degraded. Using an artificially early `occurred_at_utc` here
    // only to construct the fixture shape (a stopped+degraded row whose
    // stored `effective_operation_close_utc` is still in the route's
    // future) -- production would never naturally stop an operation before
    // its own session closes; this route's own session-window gate is what
    // must catch that shape regardless of how it arose.
    let _ = real_transition(
        &pool,
        operation_id,
        "awaiting_open",
        1,
        "stopping",
        None,
        None,
        now_mid_session,
        "test: stop mid-window",
    )
    .await;
    // `stopped_at_utc` is stamped by a dedicated, separate CAS call in
    // production (`mqk_db::record_stopped_at`), never by the generic
    // `transition_autonomous_daily_operation` UPDATE itself.
    mqk_db::record_stopped_at(&pool, operation_id, now_mid_session)
        .await
        .expect("record_stopped_at must succeed");
    let stopping = fetch_row(&pool, operation_id).await;
    assert!(stopping.stopped_at_utc.is_some(), "fixture precondition: record_stopped_at sets it");
    let degraded = real_transition(
        &pool,
        operation_id,
        "stopping",
        stopping.state_version,
        STATE_EVIDENCE_DEGRADED,
        Some("unknown_incomplete_bar_coverage"),
        None,
        now_mid_session,
        "test: degrade after stopping mid-window",
    )
    .await;
    assert_eq!(degraded.state, STATE_EVIDENCE_DEGRADED);
    assert!(degraded.stopped_at_utc.is_some(), "fixture precondition");

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    // Pin the route's own clock to the same mid-session instant used to seed
    // the fixture -- the route must see the window as still open.
    st.set_daily_data_readiness_clock_override_for_test(Some(now_mid_session))
        .await;

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT, "mid-session operation must be refused: {json:?}");
    assert_eq!(json["truth_state"], "session_not_closed");

    let row = fetch_row(&pool, operation_id).await;
    assert_ne!(row.state, "completed_no_trade");
    assert_ne!(row.state, "completed_with_activity");

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Positive scenario: the exact narrow stale shape this route exists for --
// evidence_degraded, stopped_at_utc set, zero economic activity, session
// window closed -- must NOT be refused by any of this route's own gates and
// must reach the real finalization codepath.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn p1_stale_no_run_evidence_degraded_is_not_refused_by_this_routes_own_gates() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let adapter_id = format!("stale-fin-p1-{}", unique_suffix());
    let market_date = fixed_past_day();
    let plan = fixed_plan(market_date);
    let stopped_now = plan.effective_operation_close_utc - chrono::Duration::minutes(30);
    let now_after_close = plan.effective_operation_close_utc + chrono::Duration::minutes(5);
    let operation_id = Uuid::new_v4();

    // Real legal chain, exactly matching the actual production incident
    // this route exists to recover from (verified against the live
    // 2026-08-17 operation's own event log: none -> awaiting_open ->
    // stopping -> evidence_degraded, never touching running/start_retrying
    // -- the coordinator observed the window already closed before it ever
    // got a chance to start).
    seed_operation_row(&pool, &plan, operation_id, &adapter_id, stopped_now, "awaiting_open").await?;
    let stopping = real_transition(
        &pool,
        operation_id,
        "awaiting_open",
        1,
        "stopping",
        None,
        None,
        stopped_now,
        "test: session closed before any runtime ever started",
    )
    .await;
    let _ = stopping;
    mqk_db::record_stopped_at(&pool, operation_id, stopped_now)
        .await
        .expect("record_stopped_at must succeed");
    let stopping = fetch_row(&pool, operation_id).await;
    let degraded = real_transition(
        &pool,
        operation_id,
        "stopping",
        stopping.state_version,
        STATE_EVIDENCE_DEGRADED,
        Some("unknown_incomplete_bar_coverage"),
        None,
        stopped_now,
        "test: reproduce the exact stale shape -- no run ever started, session closed later",
    )
    .await;
    assert_eq!(degraded.state, STATE_EVIDENCE_DEGRADED);
    assert!(degraded.run_id.is_none(), "fixture precondition: no run ever started");
    assert!(degraded.stopped_at_utc.is_some(), "fixture precondition: stopped_at_utc set");

    let st = paper_state_with_db(pool.clone(), &adapter_id);
    st.set_daily_data_readiness_clock_override_for_test(Some(now_after_close))
        .await;

    let (status, json) = call(
        mqk_daemon::routes::build_router(Arc::clone(&st)),
        finalize_req(operation_id),
    )
    .await;

    // This route's own gates (state, stopped_at_utc, session-closed,
    // zero-activity) must all pass -- whatever the deeper E2B classifier
    // inside handle_outcome_finalization then decides is out of this
    // route's scope to assert on, but it must never be refused by one of
    // THIS route's own named refusal codes.
    let truth_state = json["truth_state"].as_str().unwrap_or_default();
    assert_ne!(truth_state, "not_evidence_degraded", "{json:?}");
    assert_ne!(truth_state, "not_stopped", "{json:?}");
    assert_ne!(truth_state, "session_not_closed", "{json:?}");
    assert_ne!(truth_state, "not_recoverable", "{json:?}");
    assert_eq!(status, StatusCode::OK, "{json:?}");

    // Every safety field this route can never touch must stay at its fixed,
    // permanently-false/zero value regardless of the finalization outcome.
    assert_eq!(json["runtime_started"], false);
    assert_eq!(json["arm_modified"], false);
    assert_eq!(json["halt_changed"], false);
    assert_eq!(json["reconcile_changed"], false);
    assert_eq!(json["orders_submitted"], 0);

    cleanup_operation(&pool, operation_id).await;
    Ok(())
}
