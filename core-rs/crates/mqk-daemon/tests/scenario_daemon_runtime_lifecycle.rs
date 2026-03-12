//! DB-backed daemon lifecycle wiring tests for RT-01R.
//!
//! These tests are ignored by default because they require MQK_DATABASE_URL.
//! They prove that the daemon's run control routes are wired to a real owned
//! execution loop instead of placeholder in-memory state mutations.

use std::sync::Arc;
use std::time::Duration;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;
use uuid::Uuid;

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
    sqlx::query("DELETE FROM runs WHERE engine_id = 'mqk-daemon' AND mode = 'PAPER'")
        .execute(&pool)
        .await
        .expect("cleanup daemon runs");

    pool
}

fn make_router(st: Arc<state::AppState>) -> axum::Router {
    routes::build_router(st)
}

async fn arm(st: &Arc<state::AppState>) {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(make_router(Arc::clone(st)), req).await;
    assert_eq!(status, StatusCode::OK, "arm failed: {}", parse_json(body));
}

async fn start(st: &Arc<state::AppState>) -> serde_json::Value {
    let req = Request::builder()
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
    let req = Request::builder()
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

async fn stop(st: &Arc<state::AppState>) -> serde_json::Value {
    let req = Request::builder()
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
    let req = Request::builder()
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
    Arc::new(state::AppState::new_with_db(lifecycle_pool().await))
}

#[tokio::test]
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
async fn duplicate_start_is_rejected() {
    let st = daemon_state().await;
    arm(&st).await;
    let _ = start(&st).await;

    let req = Request::builder()
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

#[tokio::test]
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

#[tokio::test]
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

#[tokio::test]
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

    let st = Arc::new(state::AppState::new_with_db(pool));
    let current = status(&st).await;
    assert_eq!(current["state"], "unknown");
    assert_eq!(
        current["active_run_id"].as_str().unwrap_or(""),
        run_id.to_string()
    );
}

#[tokio::test]
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

    let pool = st.db.as_ref().expect("db configured");
    let run = mqk_db::fetch_run(pool, run_id).await.expect("fetch run");
    assert!(matches!(run.status, mqk_db::RunStatus::Halted));

    st.stop_for_shutdown().await;
}
