//! DB-backed daemon lifecycle wiring tests for RT-01R.
//!
//! These tests are ignored by default because they require MQK_DATABASE_URL.
//! They prove that the daemon's run control routes are wired to a real owned
//! execution loop instead of placeholder in-memory state mutations.

use std::sync::Arc;
use std::time::Duration;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_broker_alpaca::{encode_fetch_cursor, types::AlpacaFetchCursor};
use mqk_daemon::{artifact_intake::ENV_ARTIFACT_PATH, routes, state};
use tokio::net::TcpListener;
use tower::ServiceExt;
use uuid::Uuid;

const TEST_OPERATOR_TOKEN: &str = "test-operator-token";

/// Spawn a minimal in-process HTTP server that satisfies the Alpaca paper REST
/// surface needed by lifecycle tests.  Returns the `http://127.0.0.1:{port}`
/// base URL to set as `ALPACA_PAPER_BASE_URL`.
///
/// Handled routes:
/// - `GET /v2/account/activities` → `[]`  (fetch_events polling, always empty)
async fn start_mock_alpaca_server() -> String {
    let app = axum::Router::new().route(
        "/v2/account/activities",
        axum::routing::get(|| async { axum::Json(serde_json::json!([])) }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{}", addr.port())
}

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

async fn lifecycle_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon runtime_ -- --include-ignored"
        )
    });

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect");

    mqk_db::migrate(&pool).await.expect("migrate");
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
    sqlx::query("DELETE FROM runs WHERE engine_id = 'mqk-daemon'")
        .execute(&pool)
        .await
        .expect("cleanup daemon runs");
    // DMON-06: clear persisted reconcile status so each test starts from
    // "unknown" rather than inheriting a stale "ok" from a prior session.
    sqlx::query("DELETE FROM sys_reconcile_status_state")
        .execute(&pool)
        .await
        .expect("cleanup sys_reconcile_status_state");
    // Clear any persisted broker cursor so daemon_state() seeds it fresh.
    sqlx::query("DELETE FROM broker_event_cursor WHERE adapter_id = 'alpaca'")
        .execute(&pool)
        .await
        .expect("cleanup broker_event_cursor");

    pool
}

fn make_router(st: Arc<state::AppState>) -> axum::Router {
    routes::build_router(st)
}

async fn arm(st: &Arc<state::AppState>) {
    let req = authed(Request::builder())
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(make_router(Arc::clone(st)), req).await;
    assert_eq!(status, StatusCode::OK, "arm failed: {}", parse_json(body));
}

async fn start(st: &Arc<state::AppState>) -> serde_json::Value {
    let req = authed(Request::builder())
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(make_router(Arc::clone(st)), req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "start failed: {}",
        parse_json(body.clone())
    );
    parse_json(body)
}

async fn status(st: &Arc<state::AppState>) -> serde_json::Value {
    let req = authed(Request::builder())
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(make_router(Arc::clone(st)), req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "status failed: {}",
        parse_json(body.clone())
    );
    parse_json(body)
}

async fn control_status(st: &Arc<state::AppState>) -> serde_json::Value {
    let req = authed(Request::builder())
        .method("GET")
        .uri("/control/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(make_router(Arc::clone(st)), req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "control/status failed: {}",
        parse_json(body.clone())
    );
    parse_json(body)
}

async fn control_arm(st: &Arc<state::AppState>) -> (StatusCode, serde_json::Value) {
    let req = authed(Request::builder())
        .method("POST")
        .uri("/control/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(make_router(Arc::clone(st)), req).await;
    (status, parse_json(body))
}

async fn control_disarm(st: &Arc<state::AppState>) -> (StatusCode, serde_json::Value) {
    let req = authed(Request::builder())
        .method("POST")
        .uri("/control/disarm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(make_router(Arc::clone(st)), req).await;
    (status, parse_json(body))
}

async fn stop(st: &Arc<state::AppState>) -> serde_json::Value {
    let req = authed(Request::builder())
        .method("POST")
        .uri("/v1/run/stop")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(make_router(Arc::clone(st)), req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "stop failed: {}",
        parse_json(body.clone())
    );
    parse_json(body)
}

async fn halt(st: &Arc<state::AppState>) -> serde_json::Value {
    let req = authed(Request::builder())
        .method("POST")
        .uri("/v1/run/halt")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(make_router(Arc::clone(st)), req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "halt failed: {}",
        parse_json(body.clone())
    );
    parse_json(body)
}

async fn daemon_state() -> Arc<state::AppState> {
    // PT-TRUTH-01 / ENV-TRUTH-01: DB-backed lifecycle tests that expect a real
    // start must use the honest broker-backed paper path and provide canonical
    // paper credentials. These ignored tests run serially (`--test-threads=1`).
    //
    // A minimal in-process mock HTTP server handles the Alpaca paper REST surface
    // so tests do not require real Alpaca credentials. ALPACA_PAPER_BASE_URL
    // overrides the paper endpoint URL used by build_daemon_broker.
    let mock_url = start_mock_alpaca_server().await;
    #[allow(deprecated)]
    unsafe {
        std::env::set_var("MQK_DAEMON_DEPLOYMENT_MODE", "paper");
        std::env::set_var("MQK_DAEMON_ADAPTER_ID", "alpaca");
        std::env::set_var("ALPACA_API_KEY_PAPER", "test-paper-key");
        std::env::set_var("ALPACA_API_SECRET_PAPER", "test-paper-secret");
        std::env::set_var("ALPACA_PAPER_BASE_URL", &mock_url);
    }

    let state = Arc::new(state::AppState::new_with_db_and_operator_auth(
        lifecycle_pool().await,
        state::OperatorAuthMode::TokenRequired(TEST_OPERATOR_TOKEN.to_string()),
    ));
    // Persist a Live broker cursor to the DB so that build_execution_orchestrator
    // loads Live continuity state from the cursor (not ColdStartUnproven from None).
    // Without this, the cursor load in state.rs line ~1490 overwrites the in-memory
    // Live state set below back to ColdStartUnproven, causing the initial tick to fail.
    let live_cursor = AlpacaFetchCursor::live(None, "alpaca:test:start", "2026-01-01T00:00:00Z");
    let cursor_json = encode_fetch_cursor(&live_cursor).expect("encode live cursor");
    mqk_db::advance_broker_cursor(
        state.db.as_ref().expect("db must be set"),
        "alpaca",
        &cursor_json,
        chrono::Utc::now(),
    )
    .await
    .expect("persist live broker cursor for lifecycle test");
    state
        .update_ws_continuity(state::AlpacaWsContinuityState::Live {
            last_message_id: "alpaca:test:start".to_string(),
            last_event_at: "2026-01-01T00:00:00Z".to_string(),
        })
        .await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn start_spawns_real_execution_loop() {
    let st = daemon_state().await;
    arm(&st).await;

    let started = start(&st).await;
    let run_id = Uuid::parse_str(started["active_run_id"].as_str().expect("run_id string"))
        .expect("valid run uuid");

    tokio::time::sleep(Duration::from_millis(150)).await;

    let pool = st.db.as_ref().expect("db configured");
    let run = mqk_db::fetch_run(pool, run_id).await.expect("fetch run");
    assert!(matches!(run.status, mqk_db::RunStatus::Running));

    st.stop_for_shutdown().await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn control_restart_route_is_not_exposed_even_with_durable_runtime_conflict_truth() {
    let pool = lifecycle_pool().await;
    let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"mqk-daemon-rt02-restart-conflict");
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: chrono::Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "daemon-runtime-paper-v1".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await
    .expect("insert run");
    mqk_db::arm_run(&pool, run_id).await.expect("arm run");
    mqk_db::begin_run(&pool, run_id).await.expect("begin run");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::TokenRequired(TEST_OPERATOR_TOKEN.to_string()),
    ));
    let req = authed(Request::builder())
        .method("POST")
        .uri("/control/restart")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(r#"{"reason":"operator request"}"#))
        .unwrap();
    let (status, _body) = call(make_router(st), req).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn hostile_restart_with_poisoned_local_cache_still_reports_durable_halt_truth() {
    let pool = lifecycle_pool().await;
    let durable_run_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        b"mqk-daemon-rt01r-hostile-restart-durable-run",
    );
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: durable_run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: chrono::Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "daemon-runtime-paper-v1".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await
    .expect("insert durable run");
    mqk_db::arm_run(&pool, durable_run_id)
        .await
        .expect("arm durable run");
    mqk_db::halt_run(&pool, durable_run_id, chrono::Utc::now())
        .await
        .expect("halt durable run");
    mqk_db::persist_arm_state(&pool, "DISARMED", Some("OperatorHalt"))
        .await
        .expect("persist durable operator halt");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::TokenRequired(TEST_OPERATOR_TOKEN.to_string()),
    ));
    {
        let mut status = st.status.write().await;
        status.state = "running".to_string();
        status.active_run_id = Some(Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            b"mqk-daemon-rt01r-hostile-restart-poisoned-run",
        ));
        status.notes = Some("poisoned in-memory runtime cache".to_string());
        status.integrity_armed = true;
    }

    let runtime = status(&st).await;
    assert_eq!(runtime["state"], "halted");
    assert_eq!(
        runtime["active_run_id"],
        serde_json::Value::String(durable_run_id.to_string())
    );

    let control = control_status(&st).await;
    assert_eq!(control["run_state"], "halted");
    assert_eq!(control["run_owned_locally"], false);
    assert_eq!(control["deadman_armed_state"], "DISARMED");
    assert_eq!(control["deadman_reason"], "OperatorHalt");

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(make_router(Arc::clone(&st)), req).await;
    assert_eq!(code, StatusCode::OK);
    let system = parse_json(body);
    assert_eq!(system["runtime_status"], "halted");
    assert_eq!(system["kill_switch_active"], true);
    assert_eq!(system["has_warning"], true);

    st.stop_for_shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn duplicate_start_is_rejected() {
    let st = daemon_state().await;
    arm(&st).await;
    let _ = start(&st).await;

    let req = authed(Request::builder())
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(make_router(Arc::clone(&st)), req).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        parse_json(body)["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime already active"),
        "duplicate start must be rejected"
    );

    st.stop_for_shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn stop_terminates_active_loop() {
    let st = daemon_state().await;
    arm(&st).await;
    let started = start(&st).await;
    let run_id = Uuid::parse_str(started["active_run_id"].as_str().expect("run_id string"))
        .expect("valid run uuid");

    let stopped = stop(&st).await;
    assert_eq!(stopped["state"], "idle");
    assert!(stopped["active_run_id"].is_null());

    let pool = st.db.as_ref().expect("db configured");
    let run = mqk_db::fetch_run(pool, run_id).await.expect("fetch run");
    assert!(matches!(run.status, mqk_db::RunStatus::Stopped));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn halt_disarms_or_halts_active_loop() {
    let st = daemon_state().await;
    arm(&st).await;
    let started = start(&st).await;
    let run_id = Uuid::parse_str(started["active_run_id"].as_str().expect("run_id string"))
        .expect("valid run uuid");

    let halted = halt(&st).await;
    assert_eq!(halted["state"], "halted");
    assert_eq!(halted["integrity_armed"], false);

    let pool = st.db.as_ref().expect("db configured");
    let run = mqk_db::fetch_run(pool, run_id).await.expect("fetch run");
    assert!(matches!(run.status, mqk_db::RunStatus::Halted));

    let arm_state = mqk_db::load_arm_state(pool)
        .await
        .expect("load arm state")
        .expect("arm state persisted");
    assert_eq!(arm_state.0, "DISARMED");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn status_reflects_real_loop_ownership() {
    let st = daemon_state().await;
    arm(&st).await;
    let started = start(&st).await;

    let current = status(&st).await;
    assert_eq!(current["state"], "running");
    assert_eq!(current["active_run_id"], started["active_run_id"]);

    st.stop_for_shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn runtime_loop_heartbeats_deadman_while_running() {
    let st = daemon_state().await;
    arm(&st).await;
    let started = start(&st).await;
    let run_id = Uuid::parse_str(started["active_run_id"].as_str().expect("run_id string"))
        .expect("valid run uuid");

    tokio::time::sleep(Duration::from_millis(1200)).await;
    let pool = st.db.as_ref().expect("db configured");
    let run = mqk_db::fetch_run(pool, run_id).await.expect("fetch run");
    assert!(
        run.last_heartbeat_utc.is_some(),
        "running loop must persist heartbeats"
    );

    st.stop_for_shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn deadman_expiry_halts_and_disarms_runtime() {
    let st = daemon_state().await;
    arm(&st).await;
    let started = start(&st).await;
    let run_id = Uuid::parse_str(started["active_run_id"].as_str().expect("run_id string"))
        .expect("valid run uuid");

    let pool = st.db.as_ref().expect("db configured");
    tokio::time::sleep(Duration::from_millis(800)).await;

    sqlx::query(
        "UPDATE runs SET last_heartbeat_utc = now() - interval '10 second' WHERE run_id = $1",
    )
    .bind(run_id)
    .execute(pool)
    .await
    .expect("force stale heartbeat");

    tokio::time::sleep(Duration::from_millis(1500)).await;

    let run = mqk_db::fetch_run(pool, run_id).await.expect("fetch run");
    assert!(matches!(run.status, mqk_db::RunStatus::Halted));

    let arm_state = mqk_db::load_arm_state(pool)
        .await
        .expect("load arm state")
        .expect("arm state persisted");
    assert_eq!(arm_state.0, "DISARMED");
    assert_eq!(arm_state.1.as_deref(), Some("DeadmanExpired"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn runtime_refuses_to_continue_after_deadman_expiry() {
    let st = daemon_state().await;
    arm(&st).await;
    let started = start(&st).await;
    let run_id = Uuid::parse_str(started["active_run_id"].as_str().expect("run_id string"))
        .expect("valid run uuid");

    let pool = st.db.as_ref().expect("db configured");
    tokio::time::sleep(Duration::from_millis(800)).await;

    sqlx::query(
        "UPDATE runs SET last_heartbeat_utc = now() - interval '10 second' WHERE run_id = $1",
    )
    .bind(run_id)
    .execute(pool)
    .await
    .expect("force stale heartbeat");
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let req = authed(Request::builder())
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status_code, _) = call(make_router(Arc::clone(&st)), req).await;
    assert_eq!(status_code, StatusCode::FORBIDDEN);

    st.stop_for_shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn heartbeat_persistence_failure_fails_closed() {
    let st = daemon_state().await;
    arm(&st).await;
    let started = start(&st).await;
    let run_id = Uuid::parse_str(started["active_run_id"].as_str().expect("run_id string"))
        .expect("valid run uuid");

    let pool = st.db.as_ref().expect("db configured");
    mqk_db::stop_run(pool, run_id)
        .await
        .expect("force terminal state");

    tokio::time::sleep(Duration::from_millis(1500)).await;

    let current = status(&st).await;
    assert_ne!(
        current["state"], "running",
        "runtime must fail closed after heartbeat persistence failure"
    );

    st.stop_for_shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn status_surface_reports_deadman_truth() {
    let st = daemon_state().await;
    arm(&st).await;
    let started = start(&st).await;
    let run_id = Uuid::parse_str(started["active_run_id"].as_str().expect("run_id string"))
        .expect("valid run uuid");

    let req = authed(Request::builder())
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(make_router(Arc::clone(&st)), req).await;
    assert_eq!(code, StatusCode::OK);
    let json = parse_json(body);
    assert_eq!(json["deadman_status"], "healthy");

    let pool = st.db.as_ref().expect("db configured");
    tokio::time::sleep(Duration::from_millis(800)).await;

    sqlx::query(
        "UPDATE runs SET last_heartbeat_utc = now() - interval '10 second' WHERE run_id = $1",
    )
    .bind(run_id)
    .execute(pool)
    .await
    .expect("force stale heartbeat");
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let req2 = authed(Request::builder())
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code2, body2) = call(make_router(Arc::clone(&st)), req2).await;
    assert_eq!(code2, StatusCode::OK);
    let json2 = parse_json(body2);
    assert_eq!(json2["deadman_status"], "expired");

    st.stop_for_shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn clean_shutdown_or_stop_does_not_look_like_deadman_healthy_forever() {
    let st = daemon_state().await;
    arm(&st).await;
    let _ = start(&st).await;

    let stopped = stop(&st).await;
    assert_eq!(stopped["state"], "idle");
    assert_eq!(stopped["deadman_status"], "inactive");
}

#[tokio::test]
async fn cannot_report_running_from_placeholder_state_alone() {
    let st = Arc::new(state::AppState::new());
    {
        let mut status = st.status.write().await;
        status.state = "running".to_string();
        status.active_run_id = Some(Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            b"mqk-daemon-rt01r-placeholder",
        ));
    }

    let current = status(&st).await;
    assert_eq!(current["state"], "idle");
    assert!(current["active_run_id"].is_null());
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn restart_reconstructs_safe_runtime_status() {
    let pool = lifecycle_pool().await;
    let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"mqk-daemon-rt01r-restart-run");
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: chrono::Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "daemon-runtime-paper-v1".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await
    .expect("insert run");
    mqk_db::arm_run(&pool, run_id).await.expect("arm run");
    mqk_db::begin_run(&pool, run_id).await.expect("begin run");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::TokenRequired(TEST_OPERATOR_TOKEN.to_string()),
    ));
    let current = status(&st).await;
    assert_eq!(current["state"], "unknown");
    assert_eq!(
        current["active_run_id"].as_str().unwrap_or(""),
        run_id.to_string()
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn control_status_reflects_real_runtime_truth() {
    let st = daemon_state().await;
    let pool = st.db.as_ref().expect("db configured");

    let now_utc = chrono::Utc::now();

    sqlx::query(
        r#"
        INSERT INTO runtime_control_state (id, desired_armed, updated_at)
        VALUES (1, true, $1)
        ON CONFLICT (id) DO UPDATE
           SET desired_armed = excluded.desired_armed,
               updated_at = excluded.updated_at
        "#,
    )
    .bind(now_utc)
    .execute(pool)
    .await
    .expect("seed runtime_control_state");

    sqlx::query(
        r#"
        INSERT INTO runtime_leader_lease (id, holder_id, epoch, lease_expires_at, updated_at)
        VALUES (1, 'scenario-daemon', 7, $1, $2)
        ON CONFLICT (id) DO UPDATE
           SET holder_id = excluded.holder_id,
               epoch = excluded.epoch,
               lease_expires_at = excluded.lease_expires_at,
               updated_at = excluded.updated_at
        "#,
    )
    .bind(now_utc + chrono::Duration::seconds(30))
    .bind(now_utc)
    .execute(pool)
    .await
    .expect("seed runtime_leader_lease");

    mqk_db::persist_arm_state(pool, "DISARMED", Some("DeadmanHalt"))
        .await
        .expect("persist arm state");

    let body = control_status(&st).await;
    assert_eq!(body["desired_armed"], true);
    assert_eq!(body["leader_holder_id"], "scenario-daemon");
    assert_eq!(body["leader_epoch"], 7);
    assert_eq!(body["deadman_armed_state"], "DISARMED");
    assert_eq!(body["deadman_reason"], "DeadmanHalt");
    assert_eq!(body["reconcile_status"], "unknown");
    assert_eq!(body["run_state"], "idle");
    assert_eq!(body["run_owned_locally"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn status_does_not_overstate_running_on_local_handle_without_durable_active_run() {
    let st = daemon_state().await;
    arm(&st).await;

    let started = start(&st).await;
    let run_id = Uuid::parse_str(started["active_run_id"].as_str().expect("run_id string"))
        .expect("valid run uuid");

    let pool = st.db.as_ref().expect("db configured");
    mqk_db::stop_run(pool, run_id)
        .await
        .expect("stop run durably");

    let status_json = status(&st).await;
    assert_eq!(status_json["state"], "unknown");
    assert_eq!(
        status_json["active_run_id"],
        serde_json::Value::Null,
        "durable stopped run must not be reported as running via local ownership"
    );

    let control = control_status(&st).await;
    assert_eq!(control["run_state"], "unknown");
    assert_eq!(control["run_owned_locally"], false);

    st.stop_for_shutdown().await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn control_disarm_is_durable_or_explicitly_scoped() {
    let st = daemon_state().await;
    let pool = st.db.as_ref().expect("db configured");

    let runs_before_arm: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs")
        .fetch_one(pool)
        .await
        .expect("count runs before arm");

    let (arm_status, arm_body) = control_arm(&st).await;
    assert_eq!(arm_status, StatusCode::OK);
    assert_eq!(arm_body["requested_action"], "control.arm");
    assert_eq!(arm_body["accepted"], true);

    if let Some(arm_audit_event_id) = arm_body["audit"]["audit_event_id"].as_str() {
        let arm_audit_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_id::text = $1 AND topic = 'operator'",
        )
        .bind(arm_audit_event_id)
        .fetch_one(pool)
        .await
        .expect("verify arm audit row");
        assert_eq!(arm_audit_count, 1);
    } else {
        assert!(
            arm_body["audit"]["audit_event_id"].is_null(),
            "arm audit_event_id must be present or null"
        );
    }

    let runs_after_arm: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs")
        .fetch_one(pool)
        .await
        .expect("count runs after arm");
    assert_eq!(
        runs_after_arm, runs_before_arm,
        "control.arm must not synthesize a durable run anchor"
    );

    let armed: bool =
        sqlx::query_scalar("SELECT desired_armed FROM runtime_control_state WHERE id = 1")
            .fetch_one(pool)
            .await
            .expect("read desired_armed after arm");
    assert!(armed);

    let (disarm_status, disarm_body) = control_disarm(&st).await;
    assert_eq!(disarm_status, StatusCode::OK);
    assert_eq!(disarm_body["requested_action"], "control.disarm");
    assert_eq!(disarm_body["accepted"], true);

    if let Some(disarm_audit_event_id) = disarm_body["audit"]["audit_event_id"].as_str() {
        let disarm_audit_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_events WHERE event_id::text = $1 AND topic = 'operator'",
        )
        .bind(disarm_audit_event_id)
        .fetch_one(pool)
        .await
        .expect("verify disarm audit row");
        assert_eq!(disarm_audit_count, 1);
    } else {
        assert!(
            disarm_body["audit"]["audit_event_id"].is_null(),
            "disarm audit_event_id must be present or null"
        );
    }

    let runs_after_disarm: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs")
        .fetch_one(pool)
        .await
        .expect("count runs after disarm");
    assert_eq!(
        runs_after_disarm, runs_after_arm,
        "control.disarm must not synthesize a durable run anchor"
    );

    let disarmed: bool =
        sqlx::query_scalar("SELECT desired_armed FROM runtime_control_state WHERE id = 1")
            .fetch_one(pool)
            .await
            .expect("read desired_armed after disarm");
    assert!(!disarmed);
}

#[tokio::test]
async fn control_restart_route_is_not_exposed() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let req = Request::builder()
        .method("POST")
        .uri("/control/restart")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(r#"{"reason":"operator request"}"#))
        .unwrap();
    let (status, _body) = call(make_router(st), req).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn durable_halted_run_is_reported_as_halted_by_operator_surfaces() {
    let st = daemon_state().await;
    arm(&st).await;

    let started = start(&st).await;
    let run_id = Uuid::parse_str(started["active_run_id"].as_str().expect("run_id string"))
        .expect("valid run uuid");

    let _ = halt(&st).await;

    let status_json = status(&st).await;
    assert_eq!(status_json["state"], "halted");
    assert_eq!(
        status_json["active_run_id"],
        serde_json::Value::String(run_id.to_string())
    );

    let req = authed(Request::builder())
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (code, body) = call(make_router(Arc::clone(&st)), req).await;
    assert_eq!(code, StatusCode::OK);
    let system = parse_json(body);
    assert_eq!(system["runtime_status"], "halted");
    assert_eq!(system["kill_switch_active"], true);
    assert_eq!(system["has_warning"], true);

    let pool = st.db.as_ref().expect("db configured");
    let run = mqk_db::fetch_run(pool, run_id).await.expect("fetch run");
    assert!(matches!(run.status, mqk_db::RunStatus::Halted));

    st.stop_for_shutdown().await;
}

// ---------------------------------------------------------------------------
// IR-01 proof tests — control operator-audit durable-truth closure
// ---------------------------------------------------------------------------
//
// These tests directly prove that no synthetic run row is created when no real
// run exists, and that operator action history remains honest in both states.

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ir01_control_arm_no_run_no_synthetic_run_created() {
    let st = daemon_state().await;

    let (status, json) = control_arm(&st).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "control/arm must return 200: {json}"
    );

    assert!(
        json["audit"]["audit_event_id"].is_null(),
        "audit_event_id must be null when no real run exists; got: {json}"
    );

    let pool = st.db.as_ref().expect("db configured");
    let run_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE engine_id = 'mqk-daemon'")
            .fetch_one(pool)
            .await
            .expect("count runs");
    assert_eq!(
        run_count, 0,
        "no synthetic run row must be created; found {run_count} rows"
    );

    let desired: bool =
        sqlx::query_scalar("SELECT desired_armed FROM runtime_control_state WHERE id = 1")
            .fetch_one(pool)
            .await
            .expect("read desired_armed");
    assert!(desired, "desired_armed must be true after control/arm");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ir01_control_disarm_no_run_no_synthetic_run_created() {
    let st = daemon_state().await;

    let (status, json) = control_disarm(&st).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "control/disarm must return 200: {json}"
    );

    assert!(
        json["audit"]["audit_event_id"].is_null(),
        "audit_event_id must be null when no real run exists; got: {json}"
    );

    let pool = st.db.as_ref().expect("db configured");
    let run_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE engine_id = 'mqk-daemon'")
            .fetch_one(pool)
            .await
            .expect("count runs");
    assert_eq!(
        run_count, 0,
        "no synthetic run row must be created; found {run_count} rows"
    );

    let disarmed: bool =
        sqlx::query_scalar("SELECT NOT desired_armed FROM runtime_control_state WHERE id = 1")
            .fetch_one(pool)
            .await
            .expect("read desired_armed after disarm");
    assert!(disarmed, "desired_armed must be false after control/disarm");
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn ir01_control_arm_with_real_run_writes_audit_event() {
    let st = daemon_state().await;
    let pool = st.db.as_ref().expect("db configured");

    let real_run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"ir01-real-run-for-audit-event-test");
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id: real_run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: chrono::Utc::now(),
            git_hash: "abc123".to_string(),
            config_hash: "test-config-hash".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await
    .expect("insert real run");

    let (status, json) = control_arm(&st).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "control/arm must return 200: {json}"
    );

    let event_id_str = json["audit"]["audit_event_id"]
        .as_str()
        .expect("audit_event_id must be a string when a real run exists");
    let event_id = Uuid::parse_str(event_id_str).expect("audit_event_id must be a valid UUID");

    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audit_events WHERE event_id = $1)")
            .bind(event_id)
            .fetch_one(pool)
            .await
            .expect("check audit_events row");
    assert!(
        exists,
        "audit_events row must exist for event_id {event_id}"
    );

    let run_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE engine_id = 'mqk-daemon'")
            .fetch_one(pool)
            .await
            .expect("count runs");
    assert_eq!(
        run_count, 1,
        "exactly one run row must exist (the real one); found {run_count}"
    );
}

// ---------------------------------------------------------------------------
// TV-01D-F1: start_execution_runtime consumes and surfaces artifact provenance
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored --test-threads=1"]
async fn tv01d_f1_start_execution_runtime_consumes_and_surfaces_artifact_provenance() {
    let artifact_id = "tv01d-f1-e2e-real-start-path-abc999";

    let artifact_dir = std::env::temp_dir().join(format!(
        "mqk_tv01d_f1_artifact_{}_{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&artifact_dir).expect("TV-01D-F1: create artifact dir");

    let manifest = format!(
        r#"{{
  "schema_version": "promoted-v1",
  "artifact_id": "{artifact_id}",
  "artifact_type": "signal_pack",
  "stage": "paper",
  "produced_by": "research-py/promote.py",
  "data_root": "promoted/signal_packs/{artifact_id}"
}}"#
    );
    let manifest_path = artifact_dir.join("promoted_manifest.json");
    std::fs::write(&manifest_path, &manifest).expect("TV-01D-F1: write manifest");

    // gate-v1 canonical schema — must match what evaluate_artifact_deployability reads.
    let deployability_gate = format!(
        r#"{{
  "schema_version": "gate-v1",
  "artifact_id": "{artifact_id}",
  "passed": true,
  "checks": [],
  "overall_reason": "All four deployability checks passed: trade count, sample window, daily turnover, and active day fraction are within bounds.",
  "evaluated_at_utc": "2026-01-01T00:00:00Z"
}}"#
    );
    std::fs::write(
        artifact_dir.join("deployability_gate.json"),
        &deployability_gate,
    )
    .expect("TV-01D-F1: write deployability gate");

    // TV-03C gate fires after TV-02C when MQK_ARTIFACT_PATH is set.
    // parity-v1 canonical schema — must satisfy evaluate_parity_evidence_from_env.
    let parity_evidence = serde_json::json!({
        "schema_version": "parity-v1",
        "artifact_id": artifact_id,
        "gate_passed": true,
        "gate_schema_version": "gate-v1",
        "shadow_evidence": {
            "evidence_available": false,
            "evidence_note": "No shadow evaluation run performed for this artifact"
        },
        "comparison_basis": "paper+alpaca supervised path",
        "live_trust_complete": false,
        "live_trust_gaps": [],
        "produced_at_utc": "2026-03-01T00:00:00Z"
    });
    std::fs::write(
        artifact_dir.join("parity_evidence.json"),
        serde_json::to_vec_pretty(&parity_evidence)
            .expect("TV-01D-F1: serialize parity evidence"),
    )
    .expect("TV-01D-F1: write parity evidence");

    #[allow(deprecated)]
    unsafe {
        std::env::set_var(ENV_ARTIFACT_PATH, manifest_path.to_str().unwrap());
    }

    let st = daemon_state().await;
    arm(&st).await;

    let started = start(&st).await;
    assert!(
        started["active_run_id"].is_string(),
        "TV-01D-F1: start must return an active_run_id; got: {started}"
    );

    let router_after_start = make_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/run-artifact")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router_after_start, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "TV-01D-F1: run-artifact after real start must return 200"
    );
    let j = parse_json(body);

    assert_eq!(
        j["truth_state"], "active",
        "TV-01D-F1: after real start with accepted artifact, truth_state must be \
         'active' — proves start_execution_runtime wrote provenance; body: {j}"
    );
    assert_eq!(
        j["artifact_id"].as_str().unwrap_or(""),
        artifact_id,
        "TV-01D-F1: artifact_id must match the promoted manifest exactly; body: {j}"
    );
    assert_eq!(
        j["artifact_type"].as_str().unwrap_or(""),
        "signal_pack",
        "TV-01D-F1: artifact_type must match; body: {j}"
    );
    assert_eq!(
        j["stage"].as_str().unwrap_or(""),
        "paper",
        "TV-01D-F1: stage must match; body: {j}"
    );
    assert_eq!(
        j["produced_by"].as_str().unwrap_or(""),
        "research-py/promote.py",
        "TV-01D-F1: produced_by must match; body: {j}"
    );

    stop(&st).await;

    let router_after_stop = make_router(Arc::clone(&st));
    let req2 = Request::builder()
        .method("GET")
        .uri("/api/v1/system/run-artifact")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status2, body2) = call(router_after_stop, req2).await;
    assert_eq!(
        status2,
        StatusCode::OK,
        "TV-01D-F1: run-artifact after real stop must return 200"
    );
    let j2 = parse_json(body2);
    assert_eq!(
        j2["truth_state"], "no_run",
        "TV-01D-F1: after real stop, truth_state must be 'no_run' — provenance \
         cleared by stop_execution_runtime, not a test seam; body: {j2}"
    );
    assert!(
        j2["artifact_id"].is_null(),
        "TV-01D-F1: artifact_id must be null after real stop; body: {j2}"
    );

    #[allow(deprecated)]
    unsafe {
        std::env::remove_var(ENV_ARTIFACT_PATH);
    }
    let _ = std::fs::remove_dir_all(&artifact_dir);

    st.stop_for_shutdown().await;
}
