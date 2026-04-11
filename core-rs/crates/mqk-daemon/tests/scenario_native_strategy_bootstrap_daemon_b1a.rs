//! B1A-close: Daemon integration proof — native strategy bootstrap lifecycle.
//!
//! Proves that `build_daemon_plugin_registry()` now contains real built-in
//! strategy engines and that the B1A bootstrap gate behaves correctly through
//! the actual daemon start path.
//!
//! # Gate ordering context (LiveShadow+Alpaca, no WS gate applies)
//!
//! ```text
//! 1. deployment_readiness gate  → pass (LiveShadow+Alpaca is ready)
//! 2. integrity gate             → pass (armed)
//! 3. [WS continuity gate]       → NOT applicable to LiveShadow (BRK-00R-04)
//! 4. [artifact/capital gates]   → pass (no config files present)
//! 5. B1A bootstrap gate         → tested here
//! 6. DB pool gate               → 503 (no DB; proves bootstrap gate passed)
//! ```
//!
//! # Tests
//!
//! | ID   | Fleet                    | Expected             | Proof                          |
//! |------|--------------------------|----------------------|--------------------------------|
//! | L01  | `swing_momentum`         | 503 (DB gate)        | registered built-in → pass     |
//! | L02  | `unknown_strategy`       | 403 bootstrap gate   | unregistered → fail-closed     |
//! | L03  | absent                   | 503 (DB gate)        | Dormant → pass                 |
//! | L04  | `swing_momentum` + DB    | bootstrap=active     | storage proof (DB-backed)      |
//! | L05  | `swing_momentum` + DB    | bootstrap=None       | stop clear proof (DB-backed)   |
//! | L06  | `swing_momentum` + DB    | bootstrap=None       | halt clear proof (DB-backed)   |
//!
//! L01–L03 are pure in-process; no DB or network required.
//! L04–L06 are DB-backed and skip gracefully without `MQK_DATABASE_URL`.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use state::StrategyFleetEntry;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

/// Create a LiveShadow+Alpaca state, arm it, and return it.
/// LiveShadow is used because it bypasses the WS continuity gate (BRK-00R-04),
/// which would otherwise fire for Paper+Alpaca before reaching the bootstrap gate.
async fn armed_live_shadow_state() -> Arc<state::AppState> {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "arm must succeed in test setup");
    st
}

fn fleet_entry(strategy_id: &str) -> StrategyFleetEntry {
    StrategyFleetEntry {
        strategy_id: strategy_id.to_string(),
    }
}

// ---------------------------------------------------------------------------
// L01 — `swing_momentum` (real built-in) passes the bootstrap gate
//
// Before B1A-close: build_daemon_plugin_registry() returned an empty registry,
// so `swing_momentum` in the fleet would produce a 403 (bootstrap gate).
// After B1A-close: the built-in is registered; the bootstrap gate passes and
// the start refusal is 503 (DB gate), not 403.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b1a_l01_swing_momentum_fleet_passes_bootstrap_gate() {
    let st = armed_live_shadow_state().await;

    st.set_strategy_fleet_for_test(Some(vec![fleet_entry("swing_momentum")]))
        .await;

    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;

    // Must NOT be 403 from the bootstrap gate.
    // Must be 503 from the DB gate (no DB configured in this test).
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "L01: swing_momentum must pass the bootstrap gate and reach the DB gate (503); \
         if 403, the built-in is not registered in build_daemon_plugin_registry(); \
         got: {status} body: {}",
        String::from_utf8_lossy(&body)
    );
    let json = parse_json(body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "L01: 503 must be the DB gate, not another gate; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// L02 — Unknown strategy in fleet → 403 native_strategy_bootstrap gate
//
// Fail-closed behavior: fleet names a strategy not in any registry →
// start must be refused before any DB operations are attempted.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b1a_l02_unknown_strategy_fleet_refused_fail_closed() {
    let st = armed_live_shadow_state().await;

    st.set_strategy_fleet_for_test(Some(vec![fleet_entry("not_a_real_strategy")]))
        .await;

    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "L02: unknown strategy must return 403; got: {status}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["gate"], "native_strategy_bootstrap",
        "L02: gate must be native_strategy_bootstrap; got: {json}"
    );
    assert_eq!(
        json["fault_class"], "runtime.start_refused.native_strategy_bootstrap_failed",
        "L02: fault_class must identify bootstrap failure; got: {json}"
    );
    // The error must name the unregistered strategy.
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("not_a_real_strategy"),
        "L02: error must name the missing strategy; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// L03 — No fleet configured → Dormant → passes bootstrap gate
//
// Dormant is not an error: operators who have not configured MQK_STRATEGY_IDS
// must still be able to start the daemon.  The bootstrap gate must not block
// on a Dormant outcome.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b1a_l03_no_fleet_dormant_passes_bootstrap_gate() {
    let st = armed_live_shadow_state().await;

    // No fleet configured (default after new_for_test_with_mode_and_broker).
    // Explicitly clear to be safe.
    st.set_strategy_fleet_for_test(None).await;

    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(routes::build_router(Arc::clone(&st)), start_req).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "L03: no fleet (Dormant) must pass the bootstrap gate and reach the DB gate (503); \
         if 403, Dormant is incorrectly being treated as a failure; got: {status}"
    );
    let json = parse_json(body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "L03: 503 must be the DB gate; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// L04 — DB-backed: start with swing_momentum → bootstrap stored as Active
//
// Skips gracefully when MQK_DATABASE_URL is not set.
// ---------------------------------------------------------------------------

async fn db_pool_or_skip() -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(v) => v,
        Err(_) => return None,
    };
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("B1A DB test: failed to connect to MQK_DATABASE_URL"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b1a_l04_start_with_registered_strategy_stores_active_bootstrap() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("L04: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    mqk_db::migrate(&pool).await.expect("migrate");

    // Clean up any stale state from prior test runs.
    sqlx::query("DELETE FROM runtime_leader_lease WHERE id = 1")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM runtime_control_state WHERE id = 1")
        .execute(&pool)
        .await
        .ok();

    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));

    // Arm.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "L04: arm must succeed");

    // Configure swing_momentum fleet.
    st.set_strategy_fleet_for_test(Some(vec![fleet_entry("swing_momentum")]))
        .await;

    // Bootstrap must be None before start.
    assert!(
        st.native_strategy_bootstrap_truth_state_for_test()
            .await
            .is_none(),
        "L04: bootstrap must be None before start"
    );

    // Start.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (start_status, start_body) = call(routes::build_router(Arc::clone(&st)), start_req).await;
    assert_eq!(
        start_status,
        StatusCode::OK,
        "L04: start must succeed; got: {start_status} body: {}",
        String::from_utf8_lossy(&start_body)
    );

    // Bootstrap must now be stored as Active.
    let truth = st
        .native_strategy_bootstrap_truth_state_for_test()
        .await
        .expect("L04: bootstrap must be Some after successful start");
    assert_eq!(
        truth, "active",
        "L04: bootstrap truth_state must be 'active' after start with registered strategy; got: {truth}"
    );

    // Cleanup: stop the run.
    let stop_req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        .body(axum::body::Body::empty())
        .unwrap();
    let _ = call(routes::build_router(Arc::clone(&st)), stop_req).await;
}

// ---------------------------------------------------------------------------
// L05 — DB-backed: start + stop → bootstrap cleared
//
// Skips gracefully when MQK_DATABASE_URL is not set.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b1a_l05_stop_clears_native_strategy_bootstrap() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("L05: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    mqk_db::migrate(&pool).await.expect("migrate");

    sqlx::query("DELETE FROM runtime_leader_lease WHERE id = 1")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM runtime_control_state WHERE id = 1")
        .execute(&pool)
        .await
        .ok();

    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));

    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "L05: arm must succeed");

    st.set_strategy_fleet_for_test(Some(vec![fleet_entry("swing_momentum")]))
        .await;

    // Start.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (start_status, start_body) = call(routes::build_router(Arc::clone(&st)), start_req).await;
    assert_eq!(
        start_status,
        StatusCode::OK,
        "L05: start must succeed; body: {}",
        String::from_utf8_lossy(&start_body)
    );

    // Bootstrap is Active after start.
    let before_stop = st
        .native_strategy_bootstrap_truth_state_for_test()
        .await
        .expect("L05: bootstrap must be Some after start");
    assert_eq!(
        before_stop, "active",
        "L05: bootstrap must be active after start"
    );

    // Stop.
    let stop_req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        .body(axum::body::Body::empty())
        .unwrap();
    let (stop_status, stop_body) = call(routes::build_router(Arc::clone(&st)), stop_req).await;
    assert_eq!(
        stop_status,
        StatusCode::OK,
        "L05: stop must succeed; body: {}",
        String::from_utf8_lossy(&stop_body)
    );

    // Bootstrap must be cleared after stop.
    let after_stop = st.native_strategy_bootstrap_truth_state_for_test().await;
    assert!(
        after_stop.is_none(),
        "L05: bootstrap must be None after stop; got: {after_stop:?}"
    );
}

// ---------------------------------------------------------------------------
// L06 — DB-backed: start + halt → bootstrap cleared
//
// Proves that halt_execution_runtime() clears native_strategy_bootstrap,
// closing the B1A lifecycle proof for the halt path.
//
// Skips gracefully when MQK_DATABASE_URL is not set.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b1a_l06_halt_clears_native_strategy_bootstrap() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("L06: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    mqk_db::migrate(&pool).await.expect("migrate");

    sqlx::query("DELETE FROM runtime_leader_lease WHERE id = 1")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM runtime_control_state WHERE id = 1")
        .execute(&pool)
        .await
        .ok();

    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool.clone(),
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));

    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "L06: arm must succeed");

    st.set_strategy_fleet_for_test(Some(vec![fleet_entry("swing_momentum")]))
        .await;

    // Start.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (start_status, start_body) = call(routes::build_router(Arc::clone(&st)), start_req).await;
    assert_eq!(
        start_status,
        StatusCode::OK,
        "L06: start must succeed; body: {}",
        String::from_utf8_lossy(&start_body)
    );

    // Bootstrap must be Active after start.
    let before_halt = st
        .native_strategy_bootstrap_truth_state_for_test()
        .await
        .expect("L06: bootstrap must be Some after start");
    assert_eq!(
        before_halt, "active",
        "L06: bootstrap must be active after start"
    );

    // Halt.
    let halt_req = Request::builder()
        .method("POST")
        .uri("/v1/run/halt")
        .body(axum::body::Body::empty())
        .unwrap();
    let (halt_status, halt_body) = call(routes::build_router(Arc::clone(&st)), halt_req).await;
    assert_eq!(
        halt_status,
        StatusCode::OK,
        "L06: halt must succeed; body: {}",
        String::from_utf8_lossy(&halt_body)
    );

    // Bootstrap must be cleared after halt.
    let after_halt = st.native_strategy_bootstrap_truth_state_for_test().await;
    assert!(
        after_halt.is_none(),
        "L06: bootstrap must be None after halt; got: {after_halt:?}"
    );
}
