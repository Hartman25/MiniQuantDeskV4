//! AUTON-PAPER-OPS-04 — clear-halted-run operator path proof tests.
//!
//! Proves that the `clear-halted-run` ops/action correctly:
//! - Refuses without a DB (503 fail-closed)
//! - Reports required catalog fields (enabled=false when no halted run / no DB)
//! - Refuses when no run exists (409 no_run_found)
//! - Refuses when latest run is not HALTED (409 run_not_halted)
//! - Clears a HALTED run to STOPPED (200 halted_run_cleared, durable write)
//! - After clearing, start_execution_runtime is no longer blocked by halted lifecycle
//!
//! ## Test matrix
//!
//! | Test  | What it proves                                                             |
//! |-------|----------------------------------------------------------------------------|
//! | H01   | clear-halted-run without DB → 503 fail-closed                             |
//! | H02   | catalog contains clear-halted-run with required fields (no DB)             |
//! | H03   | clear-halted-run with no run in DB → 409 no_run_found (DB-backed)         |
//! | H04   | clear-halted-run when latest run is STOPPED → 409 run_not_halted (DB)     |
//! | H05   | clear-halted-run on a HALTED run → 200 halted_run_cleared, durable (DB)  |
//! | H06   | after clear, start is no longer blocked by halted_lifecycle gate (DB)     |
//!
//! H01-H02 are pure in-process (no DB required).
//! H03-H06 require MQK_DATABASE_URL and are #[ignore].

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use mqk_daemon::{
    routes::build_router,
    state::{AppState, BrokerKind, DeploymentMode},
};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn no_db_router() -> axum::Router {
    let st = Arc::new(AppState::new_for_test_with_mode(DeploymentMode::Paper));
    build_router(st)
}

async fn post_action(
    router: axum::Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    // ExplicitDevNoToken mode: Authorization header is not checked.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, j)
}

async fn get_catalog(router: axum::Router) -> serde_json::Value {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/ops/catalog")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

// ---------------------------------------------------------------------------
// DB helpers
// ---------------------------------------------------------------------------

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_clear_halted_run_auton04 \
             -- --include-ignored --test-threads 1"
        )
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to test DB");
    mqk_db::migrate(&pool).await.expect("run migrations");
    pool
}

async fn cleanup_test_runs(pool: &sqlx::PgPool) {
    // Delete only test-specific runs keyed by the deterministic UUIDs below.
    for seed in &[
        b"mqk-daemon.auton04.h03".as_ref(),
        b"mqk-daemon.auton04.h04".as_ref(),
        b"mqk-daemon.auton04.h05".as_ref(),
        b"mqk-daemon.auton04.h06".as_ref(),
    ] {
        let run_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, seed);
        sqlx::query("delete from runs where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await
            .ok();
    }
}

// ---------------------------------------------------------------------------
// H01: clear-halted-run without DB → 503
// ---------------------------------------------------------------------------

/// H01: clear-halted-run without DB → 503 SERVICE_UNAVAILABLE (fail-closed).
#[tokio::test]
async fn h01_clear_halted_run_no_db_returns_503() {
    let (status, j) = post_action(
        no_db_router(),
        serde_json::json!({ "action_key": "clear-halted-run" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "H01: clear-halted-run without DB must return 503; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// H02: catalog entry present with required fields (pure, no DB)
// ---------------------------------------------------------------------------

/// H02: ops/catalog contains clear-halted-run with all required fields.
///
/// Also proves the entry is disabled (enabled=false) when no DB is available.
#[tokio::test]
async fn h02_catalog_contains_clear_halted_run_with_required_fields() {
    let j = get_catalog(no_db_router()).await;
    let actions = j["actions"].as_array().expect("actions must be an array");

    let entry = actions
        .iter()
        .find(|a| a["action_key"].as_str() == Some("clear-halted-run"))
        .expect("H02: clear-halted-run must be in catalog");

    assert!(
        entry["label"].is_string(),
        "H02: clear-halted-run must have label"
    );
    assert!(
        entry["level"].is_number(),
        "H02: clear-halted-run must have level"
    );
    assert!(
        entry["description"].is_string(),
        "H02: clear-halted-run must have description"
    );
    assert!(
        entry["requires_reason"].is_boolean(),
        "H02: clear-halted-run must have requires_reason"
    );
    assert!(
        entry["confirm_text"].is_string(),
        "H02: clear-halted-run must have confirm_text"
    );
    assert!(
        entry["enabled"].is_boolean(),
        "H02: clear-halted-run must have enabled"
    );
    // No DB → no halted run detectable → disabled.
    assert_eq!(
        entry["enabled"], false,
        "H02: clear-halted-run must be disabled when no DB; entry: {entry}"
    );
    assert!(
        entry["disabled_reason"].is_string(),
        "H02: clear-halted-run must have disabled_reason when no DB"
    );
}

// ---------------------------------------------------------------------------
// DB-backed tests (require MQK_DATABASE_URL, #[ignore] in CI)
// ---------------------------------------------------------------------------

/// H03: clear-halted-run with no run in DB → 409 no_run_found.
#[tokio::test]
#[ignore]
async fn h03_clear_halted_run_no_run_returns_409() {
    let pool = make_db_pool().await;
    cleanup_test_runs(&pool).await;

    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Paper,
    ));
    let router = build_router(st);

    let (status, j) = post_action(
        router,
        serde_json::json!({ "action_key": "clear-halted-run" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "H03: clear-halted-run with no run must return 409; body: {j}"
    );
    assert_eq!(j["accepted"], false, "H03: must not be accepted; body: {j}");
    assert_eq!(
        j["disposition"].as_str(),
        Some("no_run_found"),
        "H03: disposition must be no_run_found; body: {j}"
    );
}

/// H04: clear-halted-run when latest run is STOPPED → 409 run_not_halted.
#[tokio::test]
#[ignore]
async fn h04_clear_halted_run_stopped_run_returns_409() {
    let pool = make_db_pool().await;
    cleanup_test_runs(&pool).await;

    let run_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"mqk-daemon.auton04.h04");
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: chrono::Utc::now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await
    .expect("insert_run");
    // CREATED → ARMED → RUNNING → STOPPED
    mqk_db::arm_run(&pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(&pool, run_id).await.expect("begin_run");
    mqk_db::stop_run(&pool, run_id).await.expect("stop_run");

    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Paper,
    ));
    let router = build_router(st);

    let (status, j) = post_action(
        router,
        serde_json::json!({ "action_key": "clear-halted-run" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "H04: clear-halted-run on STOPPED run must return 409; body: {j}"
    );
    assert_eq!(j["accepted"], false, "H04: must not be accepted; body: {j}");
    assert_eq!(
        j["disposition"].as_str(),
        Some("run_not_halted"),
        "H04: disposition must be run_not_halted; body: {j}"
    );
    assert!(
        j["blockers"].as_array().is_some_and(|b| !b.is_empty()),
        "H04: blockers must explain the refusal; body: {j}"
    );
}

/// H05: clear-halted-run on a HALTED run → 200 halted_run_cleared, durable write.
#[tokio::test]
#[ignore]
async fn h05_clear_halted_run_on_halted_run_returns_200() {
    let pool = make_db_pool().await;
    cleanup_test_runs(&pool).await;

    let run_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"mqk-daemon.auton04.h05");
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: chrono::Utc::now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::halt_run(&pool, run_id, chrono::Utc::now())
        .await
        .expect("halt_run");

    // Confirm initial state is HALTED.
    let before = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run");
    assert!(
        matches!(before.status, mqk_db::RunStatus::Halted),
        "H05 precondition: run must be HALTED; status: {}",
        before.status.as_str()
    );

    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Paper,
    ));
    let router = build_router(st);

    let (status, j) = post_action(
        router,
        serde_json::json!({ "action_key": "clear-halted-run" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "H05: clear-halted-run on HALTED run must return 200; body: {j}"
    );
    assert_eq!(j["accepted"], true, "H05: must be accepted; body: {j}");
    assert_eq!(
        j["disposition"].as_str(),
        Some("halted_run_cleared"),
        "H05: disposition must be halted_run_cleared; body: {j}"
    );
    assert_eq!(
        j["audit"]["durable_db_write"], true,
        "H05: must report a durable DB write; body: {j}"
    );
    assert!(
        j["warnings"].as_array().is_some_and(|w| !w.is_empty()),
        "H05: warnings must instruct operator to re-arm and re-start; body: {j}"
    );

    // Durable verification: run must now be STOPPED.
    let after = mqk_db::fetch_run(&pool, run_id).await.expect("fetch_run after");
    assert!(
        matches!(after.status, mqk_db::RunStatus::Stopped),
        "H05: run must be STOPPED after clear; status: {}",
        after.status.as_str()
    );
}

/// H06: after clear_halted_run, start_execution_runtime is no longer blocked
/// by the `runtime.start_refused.halted_lifecycle` gate.
///
/// The start may still fail at another gate (e.g. integrity disarmed), but the
/// specific halted_lifecycle fault_class must not appear after clearing.
#[tokio::test]
#[ignore]
async fn h06_after_clear_halted_lifecycle_gate_is_unblocked() {
    let pool = make_db_pool().await;
    cleanup_test_runs(&pool).await;

    let run_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"mqk-daemon.auton04.h06");
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: chrono::Utc::now(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test-node".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::halt_run(&pool, run_id, chrono::Utc::now())
        .await
        .expect("halt_run");

    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        DeploymentMode::Paper,
        BrokerKind::Paper,
    ));

    // Before clear: start must be blocked by halted_lifecycle.
    {
        let err = st
            .start_execution_runtime()
            .await
            .expect_err("H06: start must fail before clear");
        assert_eq!(
            err.fault_class(),
            "runtime.start_refused.halted_lifecycle",
            "H06: before clear the fault_class must be halted_lifecycle; got: {}",
            err
        );
    }

    // Clear via DB directly (same fn the route calls).
    mqk_db::clear_halted_run(&pool, run_id)
        .await
        .expect("clear_halted_run");

    // After clear: start must NOT fail at halted_lifecycle.
    match st.start_execution_runtime().await {
        Ok(_) => {} // acceptable
        Err(err) => {
            assert_ne!(
                err.fault_class(),
                "runtime.start_refused.halted_lifecycle",
                "H06: after clear, halted_lifecycle gate must not fire; got: {}",
                err
            );
        }
    }
}
