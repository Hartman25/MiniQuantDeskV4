//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-
//! PROJECTION: proof tests for the read-only daily-operation API
//! (`GET /api/v1/autonomous/daily-operation[s]`) and the additive
//! `daily_operation` summary block on readiness/paper-status/preflight.
//!
//! Group A (router-level, no DB required): route registration, HTTP method
//! shape, malformed `market_date` -> 400, no-DB `backend_unavailable`.
//!
//! Group B (DB-backed, `#[ignore]`, real port-5434 test DB): truth-state
//! vocabulary, terminal/nonterminal projection, full-lineage counts,
//! history ordering/limit clamping, summary-block fail-soft behavior,
//! read-only proof.
//!
//! Run the DB-backed group with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_autonomous_daily_operation_api_01 \
//!   -- --include-ignored --test-threads=1 --nocapture
//!
//! No coordinator invocation, no classifier/finalizer call from any route
//! handler, no GUI, no real provider/broker/Discord/network call anywhere
//! in this file.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use mqk_daemon::{
    routes::build_router,
    state::{AppState, BrokerKind, DeploymentMode},
};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

async fn get(router: axum::Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, j)
}

async fn post_status(router: axum::Router, uri: &str) -> StatusCode {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    router.oneshot(req).await.unwrap().status()
}

fn no_db_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_mode(DeploymentMode::Paper))
}

// ---------------------------------------------------------------------------
// Group A -- router-level, no DB required
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a01_single_route_registered_get_returns_200() {
    let router = build_router(no_db_state());
    let (status, body) = get(router, "/api/v1/autonomous/daily-operation").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["canonical_route"].as_str().unwrap(),
        "/api/v1/autonomous/daily-operation"
    );
}

#[tokio::test]
async fn a02_history_route_registered_get_returns_200() {
    let router = build_router(no_db_state());
    let (status, body) = get(router, "/api/v1/autonomous/daily-operations").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["canonical_route"].as_str().unwrap(),
        "/api/v1/autonomous/daily-operations"
    );
}

#[tokio::test]
async fn a03_invalid_market_date_returns_400() {
    let router = build_router(no_db_state());
    let (status, body) = get(
        router,
        "/api/v1/autonomous/daily-operation?market_date=not-a-date",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["truth_state"].as_str().unwrap(), "invalid_request");
    assert!(body["operation"].is_null());
}

#[tokio::test]
async fn a03b_malformed_month_day_returns_400() {
    let router = build_router(no_db_state());
    let (status, _body) = get(
        router,
        "/api/v1/autonomous/daily-operation?market_date=2026-13-40",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a04_post_to_single_route_is_not_registered() {
    let router = build_router(no_db_state());
    let status = post_status(router, "/api/v1/autonomous/daily-operation").await;
    assert_ne!(
        status,
        StatusCode::OK,
        "POST must not be accepted on the read-only single route"
    );
    assert!(
        status == StatusCode::METHOD_NOT_ALLOWED || status == StatusCode::NOT_FOUND,
        "expected 405/404 for POST, got {status}"
    );
}

#[tokio::test]
async fn a05_post_to_history_route_is_not_registered() {
    let router = build_router(no_db_state());
    let status = post_status(router, "/api/v1/autonomous/daily-operations").await;
    assert!(
        status == StatusCode::METHOD_NOT_ALLOWED || status == StatusCode::NOT_FOUND,
        "expected 405/404 for POST, got {status}"
    );
}

#[tokio::test]
async fn a06_single_route_no_db_pool_is_backend_unavailable() {
    let router = build_router(no_db_state());
    let (status, body) = get(router, "/api/v1/autonomous/daily-operation").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"].as_str().unwrap(), "backend_unavailable");
    assert!(body["operation"].is_null());
}

#[tokio::test]
async fn a07_history_route_no_db_pool_is_backend_unavailable() {
    let router = build_router(no_db_state());
    let (status, body) = get(router, "/api/v1/autonomous/daily-operations").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"].as_str().unwrap(), "backend_unavailable");
    assert!(body["rows"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a08_readiness_response_contains_additive_summary_block_no_db() {
    let router = build_router(no_db_state());
    let (status, body) = get(router, "/api/v1/autonomous/readiness").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("daily_operation").is_some());
    assert_eq!(
        body["daily_operation"]["truth_state"].as_str().unwrap(),
        "backend_unavailable"
    );
    // Parent gate fields remain present and untouched by the summary.
    assert!(body.get("overall_ready").is_some());
}

#[tokio::test]
async fn a09_paper_status_response_contains_additive_summary_block_no_db() {
    let router = build_router(no_db_state());
    let (status, body) = get(router, "/api/v1/autonomous/paper-status").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("daily_operation").is_some());
    assert!(body.get("readiness_classification").is_some());
}

#[tokio::test]
async fn a10_preflight_response_contains_additive_summary_block_no_db() {
    let router = build_router(no_db_state());
    let (status, body) = get(router, "/api/v1/system/preflight").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("daily_operation").is_some());
    assert!(body.get("deployment_start_allowed").is_some());
}

// ---------------------------------------------------------------------------
// Group B -- DB-backed proofs
// ---------------------------------------------------------------------------

async fn maybe_db(label: &str) -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            eprintln!("{label}: skipped DB-backed proof because MQK_DATABASE_URL is not set");
            return None;
        }
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect MQK_DATABASE_URL");
    mqk_db::migrate(&pool).await.expect("run migrations");
    sqlx::query("delete from oms_inbox where run_id in (select run_id from runs where engine_id like 'e4api%')")
        .execute(&pool).await.expect("clean inbox");
    sqlx::query("delete from oms_outbox where run_id in (select run_id from runs where engine_id like 'e4api%')")
        .execute(&pool).await.expect("clean outbox");
    sqlx::query("delete from strategy_signal_evaluations where run_id in (select run_id from runs where engine_id like 'e4api%')")
        .execute(&pool).await.expect("clean evaluations");
    sqlx::query("delete from runs where engine_id like 'e4api%'")
        .execute(&pool).await.expect("clean runs");
    sqlx::query("delete from sys_autonomous_daily_bar_dispatches where operation_id in (select operation_id from sys_autonomous_daily_operations where adapter_id like 'e4api-%')")
        .execute(&pool).await.expect("clean claims");
    sqlx::query("delete from sys_autonomous_session_events where id in (select 'autonomous_daily_coverage_bound:' || operation_id::text from sys_autonomous_daily_operations where adapter_id like 'e4api-%')")
        .execute(&pool).await.expect("clean coverage events");
    sqlx::query("delete from sys_autonomous_daily_operation_events where operation_id in (select operation_id from sys_autonomous_daily_operations where adapter_id like 'e4api-%')")
        .execute(&pool).await.expect("clean operation events");
    sqlx::query("delete from sys_autonomous_daily_operations where adapter_id like 'e4api-%'")
        .execute(&pool).await.expect("clean operations");
    Some(pool)
}

fn broken_pool() -> sqlx::PgPool {
    // Lazily-connecting pool against an unreachable address: pool
    // construction succeeds, but every query fails at execution time --
    // the exact "DB pool present, query itself fails" shape (E4.13).
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/nonexistent_e4_test_db")
        .expect("connect_lazy never eagerly connects")
}

/// Resolve "today's" canonical market date using the exact same production
/// resolver the route uses internally -- lets tests create a fixture for
/// whatever date the route's own default-market-date path will look up,
/// without mocking the wall clock. `None` when no market date can be
/// established at all right now (caller should skip).
fn resolve_todays_market_date_str() -> Option<String> {
    let timing = mqk_daemon::state::AutonomousDailyPlanTiming::production_default();
    match mqk_daemon::state::resolve_autonomous_daily_session_plan_from_env(Utc::now(), &timing) {
        mqk_daemon::state::AutonomousDailySessionPlanResolution::Applicable(plan) => {
            Some(plan.market_date)
        }
        mqk_daemon::state::AutonomousDailySessionPlanResolution::NotApplicable {
            market_date,
            ..
        } => Some(market_date),
        mqk_daemon::state::AutonomousDailySessionPlanResolution::Blocked { market_date, .. } => {
            market_date
        }
    }
}

fn unique_adapter_id(label: &str) -> String {
    format!("e4api-{}-{}", label, &Uuid::new_v4().to_string()[..8])
}

fn app_state(pool: sqlx::PgPool, adapter_id: &str) -> Arc<AppState> {
    let mut st = AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    );
    st.set_adapter_id_for_test(adapter_id);
    Arc::new(st)
}

fn monday_at(hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 20, hour, minute, second)
        .unwrap()
}

async fn create_operation(
    pool: &sqlx::PgPool,
    adapter_id: &str,
    market_date: NaiveDate,
    initial_state: &str,
    now_utc: DateTime<Utc>,
) -> mqk_db::AutonomousDailyOperationRecord {
    let args = mqk_db::CreateAutonomousDailyOperationArgs {
        operation_id: Uuid::new_v4(),
        market_date,
        deployment_mode: "PAPER".to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity: "sp".to_string(),
        assignment_identity: "ai".to_string(),
        runtime_binding_identity: "rbi".to_string(),
        calendar_source: "test".to_string(),
        calendar_coverage_state: "active".to_string(),
        schedule_source: "fixed_window_override".to_string(),
        effective_operation_open_utc: now_utc - chrono::Duration::hours(1),
        effective_operation_close_utc: now_utc,
        exchange_session_open_utc: now_utc - chrono::Duration::hours(1),
        exchange_session_close_utc: now_utc,
        exchange_is_early_close: false,
        previous_trading_date: market_date - chrono::Duration::days(3),
        preopen_start_utc: now_utc - chrono::Duration::hours(2),
        postclose_finalize_utc: now_utc + chrono::Duration::minutes(15),
        initial_state: initial_state.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: now_utc - chrono::Duration::hours(2),
        bounded_detail: "e4 api test fixture".to_string(),
        stop_attempt_count: 0,
    };
    match mqk_db::create_or_recover_autonomous_daily_operation(pool, &args)
        .await
        .expect("create ok")
    {
        mqk_db::CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => record,
        other => panic!("expected Created, got {other:?}"),
    }
}

/// Create an operation and drive it directly to `stopping` via the one
/// legal `awaiting_open -> stopping` edge -- `stopping` is not itself a
/// legal *initial* state.
async fn stopping_operation(
    pool: &sqlx::PgPool,
    adapter_id: &str,
    market_date: NaiveDate,
    now_utc: DateTime<Utc>,
) -> mqk_db::AutonomousDailyOperationRecord {
    let op = create_operation(
        pool,
        adapter_id,
        market_date,
        mqk_db::STATE_AWAITING_OPEN,
        now_utc,
    )
    .await;
    transition(pool, &op, mqk_db::STATE_STOPPING, None, now_utc).await
}

async fn transition(
    pool: &sqlx::PgPool,
    operation: &mqk_db::AutonomousDailyOperationRecord,
    new_state: &str,
    reason_code: Option<&str>,
    now_utc: DateTime<Utc>,
) -> mqk_db::AutonomousDailyOperationRecord {
    let args = mqk_db::TransitionAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        new_state: new_state.to_string(),
        reason_code: reason_code.map(|s| s.to_string()),
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: None,
        bounded_detail: "e4 api test transition".to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation(pool, &args)
        .await
        .expect("transition ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    }
}

async fn transition_to_running(
    pool: &sqlx::PgPool,
    operation: &mqk_db::AutonomousDailyOperationRecord,
    run_id: Uuid,
    now_utc: DateTime<Utc>,
) -> mqk_db::AutonomousDailyOperationRecord {
    let args = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        run_id,
        started_at_utc: now_utc,
        occurred_at_utc: now_utc,
        bounded_detail: "e4 api test running".to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation_to_running(pool, &args)
        .await
        .expect("transition to running ok")
    {
        mqk_db::AutonomousDailyTransitionOutcome::Applied(record) => record,
        other => panic!("expected Applied, got {other:?}"),
    }
}

async fn insert_run(pool: &sqlx::PgPool, run_id: Uuid, engine_id: &str, now_utc: DateTime<Utc>) {
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: engine_id.to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now_utc,
            git_hash: "e4test".to_string(),
            config_hash: "e4test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "e4test".to_string(),
        },
    )
    .await
    .expect("insert run ok");
}

async fn insert_evaluation(pool: &sqlx::PgPool, run_id: Uuid, now_utc: DateTime<Utc>) {
    mqk_db::insert_strategy_signal_evaluation(
        pool,
        &mqk_db::InsertStrategySignalEvaluationArgs {
            evaluation_id: Uuid::new_v4(),
            ts_utc: now_utc,
            run_id: Some(run_id),
            strategy_id: "intraday_scalper".to_string(),
            symbol: "ZZE4".to_string(),
            timeframe: "5m".to_string(),
            bar_context_source: "db_loaded".to_string(),
            bars_loaded: 1,
            latest_bar_ts_utc: Some(now_utc),
            signal_generated: false,
            signal_qty: Some(0),
            signal_side: None,
            reason_code: "flat".to_string(),
            reason: "flat".to_string(),
            decision_stage: "strategy_evaluated".to_string(),
            source: "test".to_string(),
        },
    )
    .await
    .expect("insert evaluation ok");
}

async fn insert_outbox_row(pool: &sqlx::PgPool, run_id: Uuid, idem_key: &str) {
    mqk_db::outbox_enqueue(pool, run_id, idem_key, serde_json::json!({"symbol":"ZZE4"}))
        .await
        .expect("outbox enqueue ok");
}

async fn insert_inbox_fill(pool: &sqlx::PgPool, run_id: Uuid, msg_id: &str) {
    mqk_db::inbox_insert_deduped(
        pool,
        run_id,
        msg_id,
        serde_json::json!({"event_kind": "fill", "order_id": msg_id}),
    )
    .await
    .expect("inbox insert ok");
}

async fn insert_inbox_ack(pool: &sqlx::PgPool, run_id: Uuid, msg_id: &str) {
    mqk_db::inbox_insert_deduped(
        pool,
        run_id,
        msg_id,
        serde_json::json!({"event_kind": "ack", "order_id": msg_id}),
    )
    .await
    .expect("inbox insert ok");
}

async fn count_operation_events(pool: &sqlx::PgPool, operation_id: Uuid) -> i64 {
    let row: (i64,) = sqlx::query_as(
        "select count(*) from sys_autonomous_daily_operation_events where operation_id = $1",
    )
    .bind(operation_id)
    .fetch_one(pool)
    .await
    .expect("count events ok");
    row.0
}

async fn count_all(pool: &sqlx::PgPool, table: &str) -> i64 {
    let sql = format!("select count(*) from {table}");
    let row: (i64,) = sqlx::query_as(&sql).fetch_one(pool).await.expect("count ok");
    row.0
}

// --- b01/b02: exact-slot fetch, explicit market_date ---

#[tokio::test]
#[ignore]
async fn b01_explicit_market_date_no_matching_row_is_not_found() {
    let Some(pool) = maybe_db("b01").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b01");
    let st = app_state(pool, &adapter_id);
    let router = build_router(st);
    let (status, body) = get(
        router,
        "/api/v1/autonomous/daily-operation?market_date=2026-07-20",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"].as_str().unwrap(), "not_found");
    assert!(body["operation"].is_null());
}

#[tokio::test]
#[ignore]
async fn b02_explicit_market_date_fetches_exact_slot() {
    let Some(pool) = maybe_db("b02").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b02");
    let now = monday_at(20, 0, 0);
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let op = create_operation(
        &pool,
        &adapter_id,
        market_date,
        mqk_db::STATE_AWAITING_OPEN,
        now,
    )
    .await;

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(
        router,
        "/api/v1/autonomous/daily-operation?market_date=2026-07-20",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"].as_str().unwrap(), "active");
    let row = &body["operation"];
    assert_eq!(row["operation_id"].as_str().unwrap(), op.operation_id.to_string());
    assert_eq!(row["deployment_mode"].as_str().unwrap(), "PAPER");
    assert_eq!(row["adapter_id"].as_str().unwrap(), adapter_id);
    assert_eq!(row["state"].as_str().unwrap(), mqk_db::STATE_AWAITING_OPEN);
    // Never started -> authoritative zero counts, evidence pending.
    assert_eq!(row["strategy_evaluation_count"].as_i64().unwrap(), 0);
    assert_eq!(row["order_activity_count"].as_i64().unwrap(), 0);
    assert_eq!(row["fill_count"].as_i64().unwrap(), 0);
    assert_eq!(row["evidence_state"].as_str().unwrap(), "pending");
    assert_eq!(row["finalization_status"].as_str().unwrap(), "not_yet_eligible");
    assert!(row["outcome_class"].is_null());
    assert!(row["finalized_at_utc"].is_null());
}

#[tokio::test]
#[ignore]
async fn b03_default_market_date_uses_canonical_market_date_logic() {
    let Some(pool) = maybe_db("b03").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b03");

    // Use the exact same production resolver the route uses internally to
    // compute "today's" canonical market date -- proves the route's default
    // path agrees with canonical calendar logic without mocking the clock.
    let Some(market_date_str) = resolve_todays_market_date_str() else {
        eprintln!("b03: skipped -- canonical market date could not be resolved today");
        return;
    };
    let market_date = NaiveDate::parse_from_str(&market_date_str, "%Y-%m-%d").unwrap();

    let op = create_operation(
        &pool,
        &adapter_id,
        market_date,
        mqk_db::STATE_AWAITING_OPEN,
        Utc::now(),
    )
    .await;

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(router, "/api/v1/autonomous/daily-operation").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"].as_str().unwrap(), "active");
    assert_eq!(
        body["operation"]["operation_id"].as_str().unwrap(),
        op.operation_id.to_string()
    );
    assert_eq!(body["operation"]["market_date"].as_str().unwrap(), market_date_str);
}

#[tokio::test]
#[ignore]
async fn b04_no_db_pool_is_backend_unavailable() {
    // Structural: covered non-DB in Group A (a06/a07); repeated here under
    // the DB-backed group naming purely for the required-test-matrix index.
    let router = build_router(no_db_state());
    let (status, body) = get(router, "/api/v1/autonomous/daily-operation").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"].as_str().unwrap(), "backend_unavailable");
}

#[tokio::test]
#[ignore]
async fn b05_closed_failing_db_returns_query_failed() {
    let adapter_id = unique_adapter_id("b05");
    let st = app_state(broken_pool(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(router, "/api/v1/autonomous/daily-operation").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"].as_str().unwrap(), "query_failed");
    assert!(body["operation"].is_null());
}

#[tokio::test]
#[ignore]
async fn b05b_history_closed_failing_db_returns_query_failed() {
    let adapter_id = unique_adapter_id("b05b");
    let st = app_state(broken_pool(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(router, "/api/v1/autonomous/daily-operations").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"].as_str().unwrap(), "query_failed");
    assert!(body["rows"].as_array().unwrap().is_empty());
}

// --- terminal / nonterminal projection ---

#[tokio::test]
#[ignore]
async fn b06_terminal_no_trade_row_projects_no_trade() {
    let Some(pool) = maybe_db("b06").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b06");
    let now = monday_at(20, 0, 0);
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let op = stopping_operation(&pool, &adapter_id, market_date, now).await;
    let finalize_args = mqk_db::FinalizeAutonomousDailyOperationArgs {
        operation_id: op.operation_id,
        expected_state: op.state.clone(),
        expected_state_version: op.state_version,
        target_state: mqk_db::STATE_COMPLETED_NO_TRADE.to_string(),
        outcome: mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL.to_string(),
        finalized_at_utc: now,
        bounded_detail: "e4 test finalize no-trade".to_string(),
    };
    mqk_db::finalize_autonomous_daily_operation(&pool, &finalize_args)
        .await
        .expect("finalize ok");

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(
        router,
        &format!("/api/v1/autonomous/daily-operation?market_date={market_date}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let row = &body["operation"];
    assert_eq!(row["state"].as_str().unwrap(), mqk_db::STATE_COMPLETED_NO_TRADE);
    assert_eq!(row["outcome_class"].as_str().unwrap(), "no_trade");
    assert_eq!(
        row["outcome_reason_code"].as_str().unwrap(),
        mqk_db::OUTCOME_NO_TRADE_STRATEGY_EVALUATED_NO_SIGNAL
    );
    assert!(!row["finalized_at_utc"].is_null());
    assert_eq!(row["finalization_status"].as_str().unwrap(), "finalized");
    assert_eq!(row["evidence_state"].as_str().unwrap(), "complete");
    assert!(row["evidence_blockers"].as_array().unwrap().is_empty());
}

#[tokio::test]
#[ignore]
async fn b07_terminal_activity_row_projects_with_activity() {
    let Some(pool) = maybe_db("b07").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b07");
    let now = monday_at(20, 0, 0);
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let op = stopping_operation(&pool, &adapter_id, market_date, now).await;
    let finalize_args = mqk_db::FinalizeAutonomousDailyOperationArgs {
        operation_id: op.operation_id,
        expected_state: op.state.clone(),
        expected_state_version: op.state_version,
        target_state: mqk_db::STATE_COMPLETED_WITH_ACTIVITY.to_string(),
        outcome: mqk_db::OUTCOME_ACTIVITY_FILL_CONFIRMED.to_string(),
        finalized_at_utc: now,
        bounded_detail: "e4 test finalize activity".to_string(),
    };
    mqk_db::finalize_autonomous_daily_operation(&pool, &finalize_args)
        .await
        .expect("finalize ok");

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(
        router,
        &format!("/api/v1/autonomous/daily-operation?market_date={market_date}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let row = &body["operation"];
    assert_eq!(row["outcome_class"].as_str().unwrap(), "with_activity");
    assert_eq!(
        row["outcome_reason_code"].as_str().unwrap(),
        mqk_db::OUTCOME_ACTIVITY_FILL_CONFIRMED
    );
    assert_eq!(row["evidence_state"].as_str().unwrap(), "complete");
}

#[tokio::test]
#[ignore]
async fn b08_generic_completed_row_projects_completed() {
    let Some(pool) = maybe_db("b08").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b08");
    let now = monday_at(20, 0, 0);
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let op = stopping_operation(&pool, &adapter_id, market_date, now).await;
    let op = transition(&pool, &op, mqk_db::STATE_COMPLETED, None, now).await;
    assert!(op.outcome.is_none(), "generic completed carries no outcome");

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(
        router,
        &format!("/api/v1/autonomous/daily-operation?market_date={market_date}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let row = &body["operation"];
    assert_eq!(row["state"].as_str().unwrap(), mqk_db::STATE_COMPLETED);
    assert_eq!(row["outcome_class"].as_str().unwrap(), "completed");
    assert!(row["outcome_reason_code"].is_null(), "usually null for generic completed");
    assert_eq!(row["finalization_status"].as_str().unwrap(), "finalized");
}

#[tokio::test]
#[ignore]
async fn b09_nonterminal_row_exposes_null_outcome_and_finalized_fields() {
    let Some(pool) = maybe_db("b09").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b09");
    let now = monday_at(20, 0, 0);
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    create_operation(&pool, &adapter_id, market_date, mqk_db::STATE_AWAITING_OPEN, now).await;

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(
        router,
        &format!("/api/v1/autonomous/daily-operation?market_date={market_date}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let row = &body["operation"];
    assert!(row["outcome_class"].is_null());
    assert!(row["outcome_reason_code"].is_null());
    assert!(row["finalized_at_utc"].is_null());
    assert_eq!(row["finalization_status"].as_str().unwrap(), "not_yet_eligible");
}

#[tokio::test]
#[ignore]
async fn b10_stopped_stopping_row_exposes_awaiting_finalization() {
    let Some(pool) = maybe_db("b10").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b10");
    let now = monday_at(20, 0, 0);
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let op = stopping_operation(&pool, &adapter_id, market_date, now).await;
    mqk_db::record_autonomous_runtime_stopped(&pool, op.operation_id, now)
        .await
        .expect("record stopped ok");

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(
        router,
        &format!("/api/v1/autonomous/daily-operation?market_date={market_date}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let row = &body["operation"];
    assert_eq!(row["finalization_status"].as_str().unwrap(), "awaiting_finalization");
    assert!(row["outcome_class"].is_null());
}

#[tokio::test]
#[ignore]
async fn b11_post_stop_evidence_degraded_exposes_blocked_insufficient_evidence() {
    let Some(pool) = maybe_db("b11").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b11");
    let now = monday_at(20, 0, 0);
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let op = stopping_operation(&pool, &adapter_id, market_date, now).await;
    mqk_db::record_autonomous_runtime_stopped(&pool, op.operation_id, now)
        .await
        .expect("record stopped ok");
    let op = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await
        .unwrap()
        .unwrap();
    let op = transition(
        &pool,
        &op,
        mqk_db::STATE_EVIDENCE_DEGRADED,
        Some("unknown_run_lineage_unavailable"),
        now,
    )
    .await;
    assert!(op.stopped_at_utc.is_some());

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(
        router,
        &format!("/api/v1/autonomous/daily-operation?market_date={market_date}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let row = &body["operation"];
    assert_eq!(
        row["finalization_status"].as_str().unwrap(),
        "blocked_insufficient_evidence"
    );
    assert_eq!(row["evidence_state"].as_str().unwrap(), "degraded");
    let blockers = row["evidence_blockers"].as_array().unwrap();
    assert!(blockers
        .iter()
        .any(|v| v.as_str() == Some("unknown_run_lineage_unavailable")));
    assert!(row["outcome_class"].is_null());
}

// --- full-lineage counts ---

#[tokio::test]
#[ignore]
async fn b12_earlier_replacement_run_evidence_contributes_to_counts() {
    let Some(pool) = maybe_db("b12").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b12");
    let now = monday_at(14, 0, 0);
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();

    // First run: reaches running, produces one evaluation, one outbox row,
    // and one fill, then recovers (run replaced).
    let op = create_operation(
        &pool,
        &adapter_id,
        market_date,
        mqk_db::STATE_AWAITING_OPEN,
        now,
    )
    .await;
    let op = transition(&pool, &op, mqk_db::STATE_START_RETRYING, None, now).await;
    let run_1 = Uuid::new_v4();
    insert_run(&pool, run_1, "e4api-b12-run1", now).await;
    let op = transition_to_running(&pool, &op, run_1, now).await;
    insert_evaluation(&pool, run_1, now).await;
    insert_outbox_row(&pool, run_1, &format!("b12-order-{run_1}")).await;
    insert_inbox_fill(&pool, run_1, &format!("b12-fill-{run_1}")).await;
    let op = transition(&pool, &op, mqk_db::STATE_RECOVERY_RETRYING, None, now).await;

    // Second (replacement) run: one more evaluation only.
    let run_2 = Uuid::new_v4();
    insert_run(&pool, run_2, "e4api-b12-run2", now).await;
    let op = transition_to_running(&pool, &op, run_2, now).await;
    insert_evaluation(&pool, run_2, now).await;
    let op = transition(&pool, &op, mqk_db::STATE_STOPPING, None, now).await;
    mqk_db::record_autonomous_runtime_stopped(&pool, op.operation_id, now)
        .await
        .expect("record stopped ok");

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(
        router,
        &format!("/api/v1/autonomous/daily-operation?market_date={market_date}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let row = &body["operation"];
    // The operation's mutable run_id column is now run_2 only, but full
    // lineage evidence from run_1 must still be counted.
    assert_eq!(row["run_id"].as_str().unwrap(), run_2.to_string());
    assert_eq!(row["strategy_evaluation_count"].as_i64().unwrap(), 2);
    assert_eq!(row["order_activity_count"].as_i64().unwrap(), 1);
    assert_eq!(row["fill_count"].as_i64().unwrap(), 1);
}

#[tokio::test]
#[ignore]
async fn b12b_ack_inbox_event_contributes_to_order_activity_count_not_fill_count() {
    let Some(pool) = maybe_db("b12b").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b12b");
    let now = monday_at(14, 0, 0);
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();

    let op = create_operation(
        &pool,
        &adapter_id,
        market_date,
        mqk_db::STATE_AWAITING_OPEN,
        now,
    )
    .await;
    let op = transition(&pool, &op, mqk_db::STATE_START_RETRYING, None, now).await;
    let run_1 = Uuid::new_v4();
    insert_run(&pool, run_1, "e4api-b12b-run1", now).await;
    let op = transition_to_running(&pool, &op, run_1, now).await;
    insert_inbox_ack(&pool, run_1, &format!("b12b-ack-{run_1}")).await;
    let op = transition(&pool, &op, mqk_db::STATE_STOPPING, None, now).await;
    mqk_db::record_autonomous_runtime_stopped(&pool, op.operation_id, now)
        .await
        .expect("record stopped ok");

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(
        router,
        &format!("/api/v1/autonomous/daily-operation?market_date={market_date}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let row = &body["operation"];
    assert_eq!(row["order_activity_count"].as_i64().unwrap(), 1);
    assert_eq!(row["fill_count"].as_i64().unwrap(), 0);
}

#[tokio::test]
#[ignore]
async fn b13_invalid_run_lineage_yields_null_counts_never_false_zero() {
    let Some(pool) = maybe_db("b13").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b13");
    let now = monday_at(14, 0, 0);
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();

    let op = create_operation(
        &pool,
        &adapter_id,
        market_date,
        mqk_db::STATE_AWAITING_OPEN,
        now,
    )
    .await;
    let op = transition(&pool, &op, mqk_db::STATE_START_RETRYING, None, now).await;
    let run_1 = Uuid::new_v4();
    insert_run(&pool, run_1, "e4api-b13-run1", now).await;
    let op = transition_to_running(&pool, &op, run_1, now).await;

    // Corrupt the lineage: current run_id column no longer matches any
    // to_state='running' transition's run_id -- CurrentRunMismatch.
    sqlx::query("update sys_autonomous_daily_operations set run_id = $1 where operation_id = $2")
        .bind(Uuid::new_v4())
        .bind(op.operation_id)
        .execute(&pool)
        .await
        .expect("corrupt run_id ok");

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(
        router,
        &format!("/api/v1/autonomous/daily-operation?market_date={market_date}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let row = &body["operation"];
    assert!(row["strategy_evaluation_count"].is_null());
    assert!(row["order_activity_count"].is_null());
    assert!(row["fill_count"].is_null());
    assert_eq!(row["evidence_state"].as_str().unwrap(), "unavailable");
    let blockers = row["evidence_blockers"].as_array().unwrap();
    assert!(blockers
        .iter()
        .any(|v| v.as_str() == Some("unknown_run_lineage_unavailable")));
}

// --- history route ---

#[tokio::test]
#[ignore]
async fn b14_history_ordering_matches_db_helper() {
    let Some(pool) = maybe_db("b14").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b14");
    let now = monday_at(14, 0, 0);
    // Deliberately far in the future so these three rows are guaranteed to
    // sort ahead of any pre-existing data in this shared test database
    // (ordering is `market_date desc` first) -- makes the ordering
    // assertion robust regardless of accumulated prior test runs.
    let d1 = NaiveDate::from_ymd_opt(2099, 7, 18).unwrap();
    let d2 = NaiveDate::from_ymd_opt(2099, 7, 19).unwrap();
    let d3 = NaiveDate::from_ymd_opt(2099, 7, 20).unwrap();
    create_operation(&pool, &adapter_id, d1, mqk_db::STATE_AWAITING_OPEN, now).await;
    create_operation(&pool, &adapter_id, d2, mqk_db::STATE_AWAITING_OPEN, now).await;
    create_operation(&pool, &adapter_id, d3, mqk_db::STATE_AWAITING_OPEN, now).await;

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(router, "/api/v1/autonomous/daily-operations?limit=10").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"].as_str().unwrap(), "active");
    let rows = body["rows"].as_array().unwrap();
    let this_adapter: Vec<&Value> = rows
        .iter()
        .filter(|r| r["adapter_id"].as_str() == Some(adapter_id.as_str()))
        .collect();
    assert_eq!(this_adapter.len(), 3);
    assert_eq!(this_adapter[0]["market_date"].as_str().unwrap(), d3.to_string());
    assert_eq!(this_adapter[1]["market_date"].as_str().unwrap(), d2.to_string());
    assert_eq!(this_adapter[2]["market_date"].as_str().unwrap(), d1.to_string());
}

#[tokio::test]
#[ignore]
async fn b15_default_history_limit_is_20() {
    let router = build_router(no_db_state());
    let (status, body) = get(router, "/api/v1/autonomous/daily-operations").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["requested_limit"].as_i64().unwrap(), 20);
    assert_eq!(body["effective_limit"].as_i64().unwrap(), 20);
}

#[tokio::test]
#[ignore]
async fn b16_limit_zero_clamps_to_one() {
    let router = build_router(no_db_state());
    let (status, body) = get(router, "/api/v1/autonomous/daily-operations?limit=0").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["requested_limit"].as_i64().unwrap(), 0);
    assert_eq!(body["effective_limit"].as_i64().unwrap(), 1);
}

#[tokio::test]
#[ignore]
async fn b17_limit_500_clamps_to_100() {
    let router = build_router(no_db_state());
    let (status, body) = get(router, "/api/v1/autonomous/daily-operations?limit=500").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["requested_limit"].as_i64().unwrap(), 500);
    assert_eq!(body["effective_limit"].as_i64().unwrap(), 100);
}

#[tokio::test]
#[ignore]
async fn b18_authoritative_empty_history_is_active_not_unavailable() {
    let Some(pool) = maybe_db("b18").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b18-empty");
    let st = app_state(pool, &adapter_id);
    let router = build_router(st);
    let (status, body) = get(router, "/api/v1/autonomous/daily-operations?limit=5").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"].as_str().unwrap(), "active");
    // No rows exist for this never-used adapter_id, but the surface is
    // authoritatively active, not "unavailable".
    assert!(body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["adapter_id"].as_str() != Some(adapter_id.as_str())));
}

// --- summary blocks (DB-backed active path) ---

#[tokio::test]
#[ignore]
async fn b19_readiness_summary_block_reflects_active_operation_and_leaves_gates_unchanged() {
    let Some(pool) = maybe_db("b19").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b19");
    // The additive summary block resolves "today's" slot via Utc::now()
    // internally (the route handler owns the clock read) -- the fixture
    // must therefore target the same canonical market date the route will
    // look up, not an arbitrary fixed date.
    let Some(market_date_str) = resolve_todays_market_date_str() else {
        eprintln!("b19: skipped -- canonical market date could not be resolved today");
        return;
    };
    let market_date = NaiveDate::parse_from_str(&market_date_str, "%Y-%m-%d").unwrap();
    let now = Utc::now();
    let op = stopping_operation(&pool, &adapter_id, market_date, now).await;

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (status, body) = get(router, "/api/v1/autonomous/readiness").await;
    assert_eq!(status, StatusCode::OK);
    // `app_state` is Paper+Alpaca (canonical path), so readiness's own gate
    // evaluation proceeds normally -- the additive summary block must never
    // change that outcome. `overall_ready`/`blockers` are computed from live
    // gate state, independent of the daily-operation row this test created.
    assert_eq!(body["truth_state"].as_str().unwrap(), "active");
    assert!(body.get("overall_ready").is_some());
    assert!(body.get("blockers").is_some());
    assert_eq!(
        body["daily_operation"]["operation_id"].as_str().unwrap(),
        op.operation_id.to_string()
    );
    assert_eq!(
        body["daily_operation"]["state"].as_str().unwrap(),
        mqk_db::STATE_STOPPING
    );
}

#[tokio::test]
#[ignore]
async fn b20_summary_db_failure_is_fail_soft_and_does_not_change_parent_status() {
    // Deliberately non-Alpaca broker: `is_paper_alpaca` is false, so
    // paper-status's own `not_applicable` branch touches nothing else in
    // the broken pool -- this isolates the additive summary block as the
    // *only* daily-operation DB read on this path, proving specifically
    // that block's own fail-soft behavior rather than incidental robustness
    // of unrelated pre-existing DB-touching code elsewhere in the route.
    let mut inner = AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Paper,
    );
    inner.db = Some(broken_pool());
    let st = Arc::new(inner);
    let router = build_router(st);
    let (status, body) = get(router, "/api/v1/autonomous/paper-status").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a broken daily-operation DB read must never turn paper-status into 500"
    );
    assert_eq!(body["truth_state"].as_str().unwrap(), "not_applicable");
    assert_eq!(
        body["daily_operation"]["truth_state"].as_str().unwrap(),
        "query_failed"
    );
    assert!(body.get("readiness_classification").is_some());
}

// --- read-only proof ---

#[tokio::test]
#[ignore]
async fn b21_calling_single_route_creates_zero_operation_events_and_no_state_change() {
    let Some(pool) = maybe_db("b21").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b21");
    let now = monday_at(20, 0, 0);
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let op = create_operation(&pool, &adapter_id, market_date, mqk_db::STATE_AWAITING_OPEN, now).await;

    let before_events = count_operation_events(&pool, op.operation_id).await;
    let before = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await
        .unwrap()
        .unwrap();

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (status, _body) = get(
        router,
        &format!("/api/v1/autonomous/daily-operation?market_date={market_date}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let after_events = count_operation_events(&pool, op.operation_id).await;
    let after = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, op.operation_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(before_events, after_events, "zero operation event delta");
    assert_eq!(before.state, after.state, "zero state delta");
    assert_eq!(
        before.state_version, after.state_version,
        "zero state_version delta"
    );
    assert_eq!(before.run_id, after.run_id, "zero run delta");
}

#[tokio::test]
#[ignore]
async fn b22_calling_both_routes_creates_zero_run_outbox_inbox_claim_evaluation_rows() {
    let Some(pool) = maybe_db("b22").await else {
        return;
    };
    let adapter_id = unique_adapter_id("b22");
    let now = monday_at(20, 0, 0);
    let market_date = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    create_operation(&pool, &adapter_id, market_date, mqk_db::STATE_AWAITING_OPEN, now).await;

    let before_runs = count_all(&pool, "runs").await;
    let before_outbox = count_all(&pool, "oms_outbox").await;
    let before_inbox = count_all(&pool, "oms_inbox").await;
    let before_claims = count_all(&pool, "sys_autonomous_daily_bar_dispatches").await;
    let before_evals = count_all(&pool, "strategy_signal_evaluations").await;

    let st = app_state(pool.clone(), &adapter_id);
    let router = build_router(st);
    let (s1, _) = get(
        router.clone(),
        &format!("/api/v1/autonomous/daily-operation?market_date={market_date}"),
    )
    .await;
    let (s2, _) = get(router, "/api/v1/autonomous/daily-operations?limit=10").await;
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(s2, StatusCode::OK);

    assert_eq!(before_runs, count_all(&pool, "runs").await);
    assert_eq!(before_outbox, count_all(&pool, "oms_outbox").await);
    assert_eq!(before_inbox, count_all(&pool, "oms_inbox").await);
    assert_eq!(
        before_claims,
        count_all(&pool, "sys_autonomous_daily_bar_dispatches").await
    );
    assert_eq!(
        before_evals,
        count_all(&pool, "strategy_signal_evaluations").await
    );
}

#[tokio::test]
async fn b23_no_classifier_or_finalizer_symbol_referenced_by_route_source() {
    // Structural, non-DB: the E4 route module's own source text must never
    // reference the classifier/finalizer or any mutating DB helper. This is
    // a fast regression trip-wire mirroring E3's own `ci_24` pattern; the
    // guard script performs the authoritative semantic check.
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/routes/autonomous_daily_operations.rs"
    ))
    .expect("read route source");
    // Check call syntax ("symbol(") rather than bare substring, since the
    // module's own doc comments name every one of these forbidden helpers
    // to explain what is deliberately never invoked.
    let forbidden = [
        "classify_and_finalize_autonomous_daily_operation(",
        "finalize_autonomous_daily_operation(",
        "create_or_recover_autonomous_daily_operation(",
        "transition_autonomous_daily_operation(",
        "transition_autonomous_daily_operation_to_running(",
        "persist_autonomous_daily_finalization_blocker(",
        "persist_autonomous_session_event(",
        "write_and_confirm_coverage_authority(",
        "start_execution_runtime(",
        "stop_execution_runtime(",
    ];
    for symbol in forbidden {
        assert!(
            !source.contains(symbol),
            "route source must never call {symbol}"
        );
    }
}

#[tokio::test]
#[ignore]
async fn b24_no_provider_broker_discord_calls_structural() {
    // The route module never constructs a provider/broker client or a
    // Discord notifier -- structurally proven by grepping its own imports,
    // mirroring b23's approach. DB-gated only because it shares the ignore
    // group with the rest of this proof family; it performs no DB I/O.
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/routes/autonomous_daily_operations.rs"
    ))
    .expect("read route source");
    for symbol in ["DiscordNotifier", "AlpacaBrokerAdapter", "HistoricalProvider"] {
        assert!(
            !source.contains(symbol),
            "route source must never reference {symbol}"
        );
    }
}
