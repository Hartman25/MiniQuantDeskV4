use std::sync::Arc;
use std::time::Duration;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;
use uuid::Uuid;

const TEST_OPERATOR_TOKEN: &str = "test-operator-token";

fn authed(builder: axum::http::request::Builder) -> axum::http::request::Builder {
    builder.header("Authorization", format!("Bearer {TEST_OPERATOR_TOKEN}"))
}

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    (status, body)
}

fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

fn body_json(value: serde_json::Value) -> axum::body::Body {
    axum::body::Body::from(serde_json::to_vec(&value).expect("json encode"))
}

fn make_router() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

fn valid_order_request() -> serde_json::Value {
    serde_json::json!({
        "client_request_id": "manual-order-001",
        "symbol": "AAPL",
        "side": "buy",
        "qty": 10,
    })
}

fn cancel_request(cancel_request_id: &str) -> serde_json::Value {
    serde_json::json!({
        "cancel_request_id": cancel_request_id,
    })
}

fn blockers_contain(json: &serde_json::Value, needle: &str) -> bool {
    json["blockers"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .any(|v| v.as_str().unwrap_or("").contains(needle))
        })
        .unwrap_or(false)
}

#[tokio::test]
async fn manual_order_submit_route_requires_operator_auth_when_token_mode_is_enabled() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::TokenRequired(TEST_OPERATOR_TOKEN.to_string()),
    ));

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/execution/orders")
        .header("content-type", "application/json")
        .body(body_json(valid_order_request()))
        .unwrap();

    let (status, body) = call(routes::build_router(st), req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let json = parse_json(body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("Bearer token"),
        "expected operator auth refusal, got: {json}"
    );
}

#[tokio::test]
async fn manual_order_submit_without_db_fails_closed() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/execution/orders")
        .header("content-type", "application/json")
        .body(body_json(valid_order_request()))
        .unwrap();

    let (status, body) = call(make_router(), req).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let json = parse_json(body);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "unavailable");
    assert_eq!(json["client_request_id"], "manual-order-001");
    assert!(
        blockers_contain(&json, "durable execution DB truth is unavailable"),
        "expected DB-unavailable blocker, got: {json}"
    );
}

#[tokio::test]
async fn manual_order_submit_rejects_market_order_with_limit_price() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/execution/orders")
        .header("content-type", "application/json")
        .body(body_json(serde_json::json!({
            "client_request_id": "manual-order-market-limit",
            "symbol": "AAPL",
            "side": "buy",
            "qty": 10,
            "order_type": "market",
            "limit_price": 123450000,
        })))
        .unwrap();

    let (status, body) = call(make_router(), req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let json = parse_json(body);
    assert_eq!(json["disposition"], "rejected");
    assert!(
        blockers_contain(&json, "market order must not carry limit_price"),
        "expected market/limit semantic rejection, got: {json}"
    );
}

#[tokio::test]
async fn manual_order_submit_rejects_limit_order_without_limit_price() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/execution/orders")
        .header("content-type", "application/json")
        .body(body_json(serde_json::json!({
            "client_request_id": "manual-order-limit-missing",
            "symbol": "AAPL",
            "side": "buy",
            "qty": 10,
            "order_type": "limit",
        })))
        .unwrap();

    let (status, body) = call(make_router(), req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let json = parse_json(body);
    assert_eq!(json["disposition"], "rejected");
    assert!(
        blockers_contain(&json, "limit order must carry limit_price"),
        "expected limit/price semantic rejection, got: {json}"
    );
}

#[tokio::test]
async fn manual_order_submit_rejects_blank_symbol_bad_qty_and_bad_side() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/execution/orders")
        .header("content-type", "application/json")
        .body(body_json(serde_json::json!({
            "client_request_id": "manual-order-bad-fields",
            "symbol": "   ",
            "side": "hold",
            "qty": 0,
        })))
        .unwrap();

    let (status, body) = call(make_router(), req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let json = parse_json(body);
    let blockers = json["blockers"].as_array().expect("blockers array");
    assert!(blockers.iter().any(|v| v
        .as_str()
        .unwrap_or("")
        .contains("symbol must not be blank")));
    assert!(blockers
        .iter()
        .any(|v| v.as_str().unwrap_or("").contains("side must be one of")));
    assert!(blockers
        .iter()
        .any(|v| v.as_str().unwrap_or("").contains("qty must be positive")));
}

async fn lifecycle_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon scenario_daemon_order_submit -- --include-ignored"
        )
    });

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect");

    mqk_db::migrate(&pool).await.expect("migrate");
    sqlx::query("DELETE FROM broker_order_map")
        .execute(&pool)
        .await
        .expect("cleanup broker_order_map");
    sqlx::query("DELETE FROM oms_inbox")
        .execute(&pool)
        .await
        .expect("cleanup oms_inbox");
    sqlx::query("DELETE FROM oms_outbox")
        .execute(&pool)
        .await
        .expect("cleanup oms_outbox");
    sqlx::query("DELETE FROM audit_events")
        .execute(&pool)
        .await
        .expect("cleanup audit_events");
    sqlx::query("DELETE FROM runtime_leader_lease WHERE id = 1")
        .execute(&pool)
        .await
        .expect("cleanup runtime_leader_lease");
    sqlx::query("DELETE FROM runtime_control_state WHERE id = 1")
        .execute(&pool)
        .await
        .expect("cleanup runtime_control_state");
    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("cleanup sys_arm_state");
    sqlx::query("DELETE FROM sys_reconcile_status_state")
        .execute(&pool)
        .await
        .expect("cleanup sys_reconcile_status_state");
    sqlx::query("DELETE FROM runs WHERE engine_id = 'mqk-daemon' AND mode = 'PAPER'")
        .execute(&pool)
        .await
        .expect("cleanup daemon runs");

    pool
}

fn db_router(st: Arc<state::AppState>) -> axum::Router {
    routes::build_router(st)
}

async fn daemon_state() -> Arc<state::AppState> {
    let state = Arc::new(state::AppState::new_with_db_and_operator_auth(
        lifecycle_pool().await,
        state::OperatorAuthMode::TokenRequired(TEST_OPERATOR_TOKEN.to_string()),
    ));
    {
        let mut broker = state.broker_snapshot.write().await;
        *broker = Some(mqk_schemas::BrokerSnapshot {
            captured_at_utc: chrono::Utc::now(),
            account: mqk_schemas::BrokerAccount {
                equity: "100000".to_string(),
                cash: "100000".to_string(),
                currency: "USD".to_string(),
            },
            orders: vec![],
            fills: vec![],
            positions: vec![],
        });
    }
    {
        let mut execution = state.execution_snapshot.write().await;
        *execution = Some(mqk_runtime::observability::ExecutionSnapshot {
            run_id: None,
            active_orders: vec![],
            pending_outbox: vec![],
            recent_inbox_events: vec![],
            portfolio: mqk_runtime::observability::PortfolioSnapshot {
                cash_micros: 0,
                realized_pnl_micros: 0,
                positions: vec![],
            },
            system_block_state: None,
            recent_risk_denials: vec![],
            snapshot_at_utc: chrono::Utc::now(),
        });
    }
    state
}

async fn arm(st: &Arc<state::AppState>) {
    let req = authed(Request::builder())
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(db_router(Arc::clone(st)), req).await;
    assert_eq!(status, StatusCode::OK, "arm failed: {}", parse_json(body));
}

async fn start(st: &Arc<state::AppState>) -> serde_json::Value {
    let req = authed(Request::builder())
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(db_router(Arc::clone(st)), req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "start failed: {}",
        parse_json(body.clone())
    );
    parse_json(body)
}

async fn post_manual_order(
    st: &Arc<state::AppState>,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = authed(Request::builder())
        .method("POST")
        .uri("/api/v1/execution/orders")
        .header("content-type", "application/json")
        .body(body_json(body))
        .unwrap();
    let (status, body) = call(db_router(Arc::clone(st)), req).await;
    (status, parse_json(body))
}

async fn post_manual_cancel(
    st: &Arc<state::AppState>,
    order_id: &str,
    cancel_request_id: &str,
) -> (StatusCode, serde_json::Value) {
    let req = authed(Request::builder())
        .method("POST")
        .uri(format!("/api/v1/execution/orders/{order_id}/cancel"))
        .header("content-type", "application/json")
        .body(body_json(cancel_request(cancel_request_id)))
        .unwrap();
    let (status, body) = call(db_router(Arc::clone(st)), req).await;
    (status, parse_json(body))
}

async fn seed_active_run(st: &Arc<state::AppState>, inject_local_owner: bool) -> Uuid {
    let pool = st.db.as_ref().expect("db configured");
    let run_id = Uuid::new_v4();
    let now = chrono::Utc::now();

    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "test-git-hash".to_string(),
            config_hash: "test-config-hash".to_string(),
            config_json: serde_json::json!({"source": "scenario_daemon_order_submit"}),
            host_fingerprint: "test-host".to_string(),
        },
    )
    .await
    .expect("insert run");
    mqk_db::arm_run(pool, run_id).await.expect("arm run");
    mqk_db::begin_run(pool, run_id).await.expect("begin run");
    mqk_db::heartbeat_run(pool, run_id, now)
        .await
        .expect("heartbeat run");

    if inject_local_owner {
        st.inject_running_loop_for_test(run_id).await;
    }

    {
        let mut execution = st.execution_snapshot.write().await;
        let snapshot = execution.as_mut().expect("execution snapshot seeded");
        snapshot.run_id = Some(run_id);
        snapshot.snapshot_at_utc = now;
    }

    run_id
}

async fn seed_cancelable_order(
    st: &Arc<state::AppState>,
    run_id: Uuid,
    order_id: &str,
    status: &str,
) {
    let pool = st.db.as_ref().expect("db configured");
    let broker_order_id = format!("broker-{order_id}");
    let now = chrono::Utc::now();

    mqk_db::outbox_enqueue(
        pool,
        run_id,
        order_id,
        serde_json::json!({
            "request_type": "submit",
            "symbol": "AAPL",
            "side": "buy",
            "qty": 10,
            "order_type": "market",
            "time_in_force": "day",
            "limit_price": null,
        }),
    )
    .await
    .expect("seed submit outbox row");
    mqk_db::broker_map_upsert(pool, order_id, &broker_order_id)
        .await
        .expect("seed broker map");
    sqlx::query(
        "UPDATE oms_outbox SET status = 'SENT', sent_at_utc = $2 WHERE idempotency_key = $1",
    )
    .bind(order_id)
    .bind(now)
    .execute(pool)
    .await
    .expect("seed sent outbox row");

    {
        let mut execution = st.execution_snapshot.write().await;
        let snapshot = execution.as_mut().expect("execution snapshot seeded");
        snapshot.run_id = Some(run_id);
        snapshot.snapshot_at_utc = now;
        snapshot
            .active_orders
            .retain(|row| row.order_id != order_id);
        snapshot
            .active_orders
            .push(mqk_runtime::observability::OrderSnapshot {
                order_id: order_id.to_string(),
                broker_order_id: Some(broker_order_id),
                symbol: "AAPL".to_string(),
                total_qty: 10,
                filled_qty: if status == "PartiallyFilled" { 3 } else { 0 },
                status: status.to_string(),
            });
    }
}

#[tokio::test]
async fn manual_order_cancel_route_requires_operator_auth_when_token_mode_is_enabled() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::TokenRequired(TEST_OPERATOR_TOKEN.to_string()),
    ));

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/execution/orders/live-order-001/cancel")
        .header("content-type", "application/json")
        .body(body_json(cancel_request("cancel-auth-check-001")))
        .unwrap();

    let (status, body) = call(routes::build_router(st), req).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let json = parse_json(body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("Bearer token"),
        "expected operator auth refusal, got: {json}"
    );
}

#[tokio::test]
async fn manual_order_cancel_without_db_fails_closed() {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/execution/orders/live-order-001/cancel")
        .header("content-type", "application/json")
        .body(body_json(cancel_request("cancel-no-db-001")))
        .unwrap();

    let (status, body) = call(make_router(), req).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let json = parse_json(body);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "unavailable");
    assert_eq!(json["order_id"], "live-order-001");
    assert!(
        blockers_contain(&json, "durable execution DB truth is unavailable"),
        "expected DB-unavailable blocker, got: {json}"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_cancel_with_db_but_no_active_run_fails_closed() {
    let st = daemon_state().await;
    arm(&st).await;

    let (status, json) =
        post_manual_cancel(&st, "live-order-001", "cancel-no-active-run-001").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "unavailable");
    assert!(
        blockers_contain(&json, "no active durable run"),
        "expected no-active-run blocker, got: {json}"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_cancel_fails_closed_when_durable_arm_state_load_fails() {
    let st = daemon_state().await;
    arm(&st).await;

    let pool = st.db.as_ref().expect("db configured").clone();
    pool.close().await;

    let (status, json) =
        post_manual_cancel(&st, "live-order-001", "cancel-arm-load-fail-001").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "unavailable");
    assert!(
        blockers_contain(&json, "durable arm-state truth could not be loaded"),
        "expected durable arm-state load failure blocker, got: {json}"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_cancel_refuses_when_durable_arm_state_is_disarmed() {
    let st = daemon_state().await;
    arm(&st).await;
    let _run_id = seed_active_run(&st, true).await;

    let pool = st.db.as_ref().expect("db configured");
    mqk_db::persist_arm_state(pool, "DISARMED", Some("IntegrityViolation"))
        .await
        .expect("persist durable disarmed state");

    let (status, json) = post_manual_cancel(&st, "live-order-001", "cancel-disarmed-001").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "rejected");
    assert!(
        blockers_contain(&json, "durable arm state is disarmed"),
        "expected durable disarmed blocker, got: {json}"
    );

    st.stop_for_shutdown().await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_cancel_refuses_when_durable_arm_state_is_halted() {
    let st = daemon_state().await;
    arm(&st).await;
    let _run_id = seed_active_run(&st, true).await;

    let pool = st.db.as_ref().expect("db configured");
    mqk_db::persist_arm_state(pool, "DISARMED", Some("OperatorHalt"))
        .await
        .expect("persist durable halted state");

    let (status, json) = post_manual_cancel(&st, "live-order-001", "cancel-halted-001").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "rejected");
    assert!(
        blockers_contain(&json, "durable arm state is halted"),
        "expected durable halted blocker, got: {json}"
    );

    st.stop_for_shutdown().await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_cancel_refuses_when_runtime_is_not_accepting_actions() {
    let st = daemon_state().await;
    arm(&st).await;
    let _run_id = seed_active_run(&st, false).await;

    let (status, json) =
        post_manual_cancel(&st, "live-order-001", "cancel-runtime-not-running-001").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "unavailable");
    assert!(
        blockers_contain(&json, "is not accepting operator cancel actions"),
        "expected runtime-not-accepting blocker, got: {json}"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_cancel_refuses_unknown_or_untargetable_order_id_honestly() {
    let st = daemon_state().await;
    arm(&st).await;
    let _run_id = seed_active_run(&st, true).await;

    let (status, json) =
        post_manual_cancel(&st, "unknown-order-001", "cancel-unknown-target-001").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "rejected");
    assert!(
        blockers_contain(&json, "unknown or not durably targetable"),
        "expected untargetable-order blocker, got: {json}"
    );

    st.stop_for_shutdown().await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_cancel_enqueues_one_pending_outbox_row_for_known_target() {
    let st = daemon_state().await;
    arm(&st).await;
    let run_id = seed_active_run(&st, true).await;
    seed_cancelable_order(&st, run_id, "live-order-001", "Open").await;

    let (status, json) = post_manual_cancel(&st, "live-order-001", "cancel-enqueue-001").await;
    assert_eq!(status, StatusCode::OK, "cancel failed: {json}");
    assert_eq!(json["accepted"], true);
    assert_eq!(json["disposition"], "enqueued");
    assert_eq!(json["order_id"], "live-order-001");
    assert_eq!(json["active_run_id"], run_id.to_string());

    let cancel_request_id = "cancel-enqueue-001";
    let pool = st.db.as_ref().expect("db configured");
    let row = mqk_db::outbox_fetch_by_idempotency_key(pool, cancel_request_id)
        .await
        .expect("fetch cancel outbox row")
        .expect("cancel outbox row present");
    assert_eq!(row.run_id, run_id);
    assert_eq!(row.status, "PENDING");
    assert_eq!(row.order_json["request_type"], "cancel");
    assert_eq!(row.order_json["target_order_id"], "live-order-001");
    assert_eq!(row.order_json["cancel_request_id"], "cancel-enqueue-001");

    st.stop_for_shutdown().await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_cancel_duplicate_request_is_truthful_noop() {
    let st = daemon_state().await;
    arm(&st).await;
    let run_id = seed_active_run(&st, true).await;
    seed_cancelable_order(&st, run_id, "live-order-001", "Open").await;

    let (first_status, first_json) =
        post_manual_cancel(&st, "live-order-001", "cancel-duplicate-001").await;
    assert_eq!(
        first_status,
        StatusCode::OK,
        "first cancel failed: {first_json}"
    );
    assert_eq!(first_json["disposition"], "enqueued");

    let (second_status, second_json) =
        post_manual_cancel(&st, "live-order-001", "cancel-duplicate-001").await;
    assert_eq!(
        second_status,
        StatusCode::OK,
        "duplicate cancel failed: {second_json}"
    );
    assert_eq!(second_json["accepted"], false);
    assert_eq!(second_json["disposition"], "duplicate");

    let cancel_request_id = "cancel-duplicate-001";
    let pool = st.db.as_ref().expect("db configured");
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM oms_outbox WHERE idempotency_key = $1")
            .bind(cancel_request_id)
            .fetch_one(pool)
            .await
            .expect("count cancel outbox rows");
    assert_eq!(count, 1, "duplicate cancel must not create a second row");

    st.stop_for_shutdown().await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_cancel_same_request_id_for_different_order_is_explicit_collision_refusal() {
    let st = daemon_state().await;
    arm(&st).await;
    let run_id = seed_active_run(&st, true).await;
    seed_cancelable_order(&st, run_id, "live-order-001", "Open").await;
    seed_cancelable_order(&st, run_id, "live-order-002", "Open").await;

    let (first_status, first_json) =
        post_manual_cancel(&st, "live-order-001", "cancel-collision-001").await;
    assert_eq!(
        first_status,
        StatusCode::OK,
        "first cancel failed: {first_json}"
    );
    assert_eq!(first_json["accepted"], true);
    assert_eq!(first_json["disposition"], "enqueued");

    let (second_status, second_json) =
        post_manual_cancel(&st, "live-order-002", "cancel-collision-001").await;
    assert_eq!(
        second_status,
        StatusCode::CONFLICT,
        "cross-order cancel-request collision must be refused honestly: {second_json}"
    );
    assert_eq!(second_json["accepted"], false);
    assert_eq!(second_json["disposition"], "rejected");
    assert_ne!(second_json["disposition"], "duplicate");
    assert!(
        blockers_contain(
            &second_json,
            "already bound to different order_id 'live-order-001'"
        ),
        "expected explicit cross-order collision blocker, got: {second_json}"
    );
    assert!(
        blockers_contain(&second_json, "order_id 'live-order-002'"),
        "expected blocker to mention refused current order target, got: {second_json}"
    );

    let pool = st.db.as_ref().expect("db configured");
    let row = mqk_db::outbox_fetch_by_idempotency_key(pool, "cancel-collision-001")
        .await
        .expect("fetch collision row")
        .expect("collision row present");
    assert_eq!(row.run_id, run_id);
    assert_eq!(row.order_json["request_type"], "cancel");
    assert_eq!(row.order_json["target_order_id"], "live-order-001");

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM oms_outbox WHERE idempotency_key = $1")
            .bind("cancel-collision-001")
            .fetch_one(pool)
            .await
            .expect("count collision rows");
    assert_eq!(
        count, 1,
        "cross-order collision must not create a second row"
    );

    st.stop_for_shutdown().await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_cancel_second_distinct_request_is_allowed_after_order_returns_to_live_state()
{
    let st = daemon_state().await;
    arm(&st).await;
    let run_id = seed_active_run(&st, true).await;
    seed_cancelable_order(&st, run_id, "live-order-001", "Open").await;

    let (first_status, first_json) =
        post_manual_cancel(&st, "live-order-001", "cancel-attempt-001").await;
    assert_eq!(
        first_status,
        StatusCode::OK,
        "first cancel failed: {first_json}"
    );
    assert_eq!(first_json["accepted"], true);
    assert_eq!(first_json["disposition"], "enqueued");

    {
        let mut execution = st.execution_snapshot.write().await;
        let snapshot = execution.as_mut().expect("execution snapshot seeded");
        let order = snapshot
            .active_orders
            .iter_mut()
            .find(|row| row.order_id == "live-order-001")
            .expect("seeded live order present");
        order.status = "Open".to_string();
    }

    let (second_status, second_json) =
        post_manual_cancel(&st, "live-order-001", "cancel-attempt-002").await;
    assert_eq!(
        second_status,
        StatusCode::OK,
        "second cancel after restored live state failed: {second_json}"
    );
    assert_eq!(second_json["accepted"], true);
    assert_eq!(second_json["disposition"], "enqueued");

    let pool = st.db.as_ref().expect("db configured");
    let first_row = mqk_db::outbox_fetch_by_idempotency_key(pool, "cancel-attempt-001")
        .await
        .expect("fetch first cancel row")
        .expect("first cancel row present");
    let second_row = mqk_db::outbox_fetch_by_idempotency_key(pool, "cancel-attempt-002")
        .await
        .expect("fetch second cancel row")
        .expect("second cancel row present");

    assert_eq!(first_row.run_id, run_id);
    assert_eq!(first_row.order_json["request_type"], "cancel");
    assert_eq!(first_row.order_json["target_order_id"], "live-order-001");
    assert_eq!(
        first_row.order_json["cancel_request_id"],
        "cancel-attempt-001"
    );
    assert_eq!(second_row.run_id, run_id);
    assert_eq!(second_row.order_json["request_type"], "cancel");
    assert_eq!(second_row.order_json["target_order_id"], "live-order-001");
    assert_eq!(
        second_row.order_json["cancel_request_id"],
        "cancel-attempt-002"
    );

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM oms_outbox WHERE idempotency_key IN ($1, $2)",
    )
    .bind("cancel-attempt-001")
    .bind("cancel-attempt-002")
    .fetch_one(pool)
    .await
    .expect("count distinct cancel attempts");
    assert_eq!(
        count, 2,
        "distinct cancel request IDs must create distinct rows"
    );

    st.stop_for_shutdown().await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_cancel_reject_restored_live_state_is_not_blocked_by_first_request_id() {
    let st = daemon_state().await;
    arm(&st).await;
    let run_id = seed_active_run(&st, true).await;
    seed_cancelable_order(&st, run_id, "live-order-001", "Open").await;

    let (first_status, first_json) =
        post_manual_cancel(&st, "live-order-001", "cancel-reject-attempt-001").await;
    assert_eq!(
        first_status,
        StatusCode::OK,
        "first cancel failed: {first_json}"
    );
    assert_eq!(first_json["accepted"], true);
    assert_eq!(first_json["disposition"], "enqueued");

    {
        let mut execution = st.execution_snapshot.write().await;
        let snapshot = execution.as_mut().expect("execution snapshot seeded");
        let order = snapshot
            .active_orders
            .iter_mut()
            .find(|row| row.order_id == "live-order-001")
            .expect("seeded live order present");
        order.status = "CancelPending".to_string();
        order.status = "Open".to_string();
    }

    let (second_status, second_json) =
        post_manual_cancel(&st, "live-order-001", "cancel-reject-attempt-002").await;
    assert_eq!(
        second_status,
        StatusCode::OK,
        "cancel after restored live state failed: {second_json}"
    );
    assert_eq!(second_json["accepted"], true);
    assert_eq!(second_json["disposition"], "enqueued");

    let pool = st.db.as_ref().expect("db configured");
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM oms_outbox WHERE idempotency_key IN ($1, $2)",
    )
    .bind("cancel-reject-attempt-001")
    .bind("cancel-reject-attempt-002")
    .fetch_one(pool)
    .await
    .expect("count cancel-reject recovery rows");
    assert_eq!(
        count, 2,
        "cancel-reject recovery must not be blocked by the first cancel request ID"
    );

    st.stop_for_shutdown().await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_submit_with_db_but_no_active_run_fails_closed() {
    let st = daemon_state().await;
    arm(&st).await;

    let (status, json) = post_manual_order(&st, valid_order_request()).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "unavailable");
    assert!(
        blockers_contain(&json, "no active durable run"),
        "expected no-active-run blocker, got: {json}"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_submit_fails_closed_when_durable_arm_state_load_fails() {
    let st = daemon_state().await;
    arm(&st).await;

    {
        let integrity = st.integrity.read().await;
        assert!(
            !integrity.is_execution_blocked(),
            "expected local in-memory integrity to remain armed before forcing DB failure"
        );
    }

    let pool = st.db.as_ref().expect("db configured").clone();
    pool.close().await;

    let (status, json) = post_manual_order(&st, valid_order_request()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "unavailable");
    assert!(
        blockers_contain(&json, "durable arm-state truth could not be loaded"),
        "expected durable arm-state load failure blocker, got: {json}"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_submit_refuses_when_durable_arm_state_is_disarmed_even_if_local_state_is_armed(
) {
    let st = daemon_state().await;
    arm(&st).await;
    start(&st).await;

    {
        let integrity = st.integrity.read().await;
        assert!(
            !integrity.is_execution_blocked(),
            "expected local in-memory integrity to remain armed before durable disarm override"
        );
    }

    let pool = st.db.as_ref().expect("db configured");
    mqk_db::persist_arm_state(pool, "DISARMED", Some("IntegrityViolation"))
        .await
        .expect("persist durable disarmed state");

    let (status, json) = post_manual_order(&st, valid_order_request()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "rejected");
    assert!(
        blockers_contain(&json, "durable arm state is disarmed"),
        "expected durable disarmed blocker, got: {json}"
    );

    st.stop_for_shutdown().await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_submit_refuses_when_durable_arm_state_is_halted_even_if_local_state_is_armed()
{
    let st = daemon_state().await;
    arm(&st).await;
    start(&st).await;

    {
        let integrity = st.integrity.read().await;
        assert!(
            !integrity.is_execution_blocked(),
            "expected local in-memory integrity to remain armed before durable halt override"
        );
    }

    let pool = st.db.as_ref().expect("db configured");
    mqk_db::persist_arm_state(pool, "DISARMED", Some("OperatorHalt"))
        .await
        .expect("persist durable halted state");

    let (status, json) = post_manual_order(&st, valid_order_request()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json["accepted"], false);
    assert_eq!(json["disposition"], "rejected");
    assert!(
        blockers_contain(&json, "durable arm state is halted"),
        "expected durable halted blocker, got: {json}"
    );

    st.stop_for_shutdown().await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_submit_enqueues_one_pending_outbox_row_for_active_run() {
    let st = daemon_state().await;
    arm(&st).await;
    let started = start(&st).await;
    let run_id = Uuid::parse_str(started["active_run_id"].as_str().expect("run_id string"))
        .expect("valid run uuid");

    let (status, json) = post_manual_order(&st, valid_order_request()).await;
    assert_eq!(status, StatusCode::OK, "submit failed: {json}");
    assert_eq!(json["accepted"], true);
    assert_eq!(json["disposition"], "enqueued");
    assert_eq!(json["active_run_id"], run_id.to_string());

    let pool = st.db.as_ref().expect("db configured");
    let row = mqk_db::outbox_fetch_by_idempotency_key(pool, "manual-order-001")
        .await
        .expect("fetch outbox row")
        .expect("outbox row present");
    assert_eq!(row.run_id, run_id);
    assert_eq!(row.status, "PENDING");
    assert_eq!(row.order_json["symbol"], "AAPL");
    assert_eq!(row.order_json["side"], "buy");
    assert_eq!(row.order_json["qty"], 10);
    assert_eq!(row.order_json["order_type"], "market");
    assert_eq!(row.order_json["time_in_force"], "day");
    assert!(row.order_json["limit_price"].is_null());

    st.stop_for_shutdown().await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_submit_duplicate_client_request_id_is_noop() {
    let st = daemon_state().await;
    arm(&st).await;
    start(&st).await;

    let (first_status, first_json) = post_manual_order(&st, valid_order_request()).await;
    assert_eq!(
        first_status,
        StatusCode::OK,
        "first submit failed: {first_json}"
    );
    assert_eq!(first_json["disposition"], "enqueued");

    let (second_status, second_json) = post_manual_order(&st, valid_order_request()).await;
    assert_eq!(
        second_status,
        StatusCode::OK,
        "duplicate submit failed: {second_json}"
    );
    assert_eq!(second_json["accepted"], false);
    assert_eq!(second_json["disposition"], "duplicate");

    let pool = st.db.as_ref().expect("db configured");
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM oms_outbox WHERE idempotency_key = $1")
            .bind("manual-order-001")
            .fetch_one(pool)
            .await
            .expect("count outbox rows");
    assert_eq!(
        count, 1,
        "duplicate client_request_id must not create a second row"
    );

    st.stop_for_shutdown().await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn manual_order_submit_accepts_limit_order_with_explicit_defaults_aligned_to_runtime() {
    let st = daemon_state().await;
    arm(&st).await;
    start(&st).await;

    let (status, json) = post_manual_order(
        &st,
        serde_json::json!({
            "client_request_id": "manual-order-limit-001",
            "symbol": "MSFT",
            "side": "sell",
            "qty": "25",
            "order_type": "limit",
            "time_in_force": "gtc",
            "limit_price": "123450000",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "limit submit failed: {json}");
    assert_eq!(json["disposition"], "enqueued");

    let pool = st.db.as_ref().expect("db configured");
    let row = mqk_db::outbox_fetch_by_idempotency_key(pool, "manual-order-limit-001")
        .await
        .expect("fetch limit row")
        .expect("limit row present");
    assert_eq!(row.order_json["symbol"], "MSFT");
    assert_eq!(row.order_json["side"], "sell");
    assert_eq!(row.order_json["qty"], 25);
    assert_eq!(row.order_json["order_type"], "limit");
    assert_eq!(row.order_json["time_in_force"], "gtc");
    assert_eq!(row.order_json["limit_price"], 123450000);

    st.stop_for_shutdown().await;
    tokio::time::sleep(Duration::from_millis(25)).await;
}
