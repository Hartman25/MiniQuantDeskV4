//! B2A: Strategy activation registry gate proof.
//!
//! Proves that the DB `sys_strategy_registry` is the final authority on
//! whether a configured native strategy is allowed to activate.  The plugin
//! registry (B1A) is necessary but not sufficient: an Active bootstrap must
//! also be present and enabled in the durable registry before the run starts.
//!
//! # Activation authority model
//!
//! ```text
//! Gate A (B1A): plugin registry presence    — pre-DB, in-process
//! Gate B (B2A): DB registry enabled flag    — after db_pool(), before run rows
//! ```
//!
//! Both gates must pass before any run row is created.  Gate A fires first.
//!
//! # Tests
//!
//! | ID   | Fleet                    | Registry row       | Expected                        |
//! |------|--------------------------|--------------------|---------------------------------|
//! | N01  | absent                   | —                  | 503 DB gate (Dormant → pass)    |
//! | N02  | `swing_momentum` + DB    | enabled=true       | 200 start success               |
//! | N03  | `swing_momentum` + DB    | enabled=false      | 403 strategy_registry gate      |
//! | N04  | `swing_momentum` + DB    | absent             | 403 strategy_registry gate      |
//! | N05  | `unknown_strategy`       | —                  | 403 native_strategy_bootstrap   |
//!
//! N01 and N05 are pure in-process (no DB).
//! N02–N04 are DB-backed; skip gracefully without `MQK_DATABASE_URL`.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::Utc;
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

/// Create a LiveShadow+Alpaca state and arm it.
/// LiveShadow bypasses the WS continuity gate (BRK-00R-04).
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
            .expect("B2A DB test: failed to connect to MQK_DATABASE_URL"),
    )
}

async fn clean_db_state(pool: &sqlx::PgPool, strategy_id: &str) {
    sqlx::query("DELETE FROM runtime_leader_lease WHERE id = 1")
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM runtime_control_state WHERE id = 1")
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM sys_strategy_registry WHERE strategy_id = $1")
        .bind(strategy_id)
        .execute(pool)
        .await
        .ok();
}

// ---------------------------------------------------------------------------
// N01 — No fleet (Dormant) skips the registry gate entirely
//
// The registry gate is conditioned on an Active bootstrap.  When no fleet is
// configured the bootstrap is Dormant, the gate is skipped, and the next
// refusal is the DB pool gate (503).  Pure in-process; no DB required.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b2a_n01_no_fleet_dormant_skips_registry_gate() {
    let st = armed_live_shadow_state().await;
    st.set_strategy_fleet_for_test(None).await;

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("POST")
            .uri("/v1/run/start")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "N01: Dormant bootstrap must skip the registry gate and reach the DB gate (503); \
         got: {status} body: {}",
        String::from_utf8_lossy(&body)
    );
    let json = parse_json(body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "N01: 503 must be the DB gate, not another gate; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// N05 — Unknown plugin → 403 native_strategy_bootstrap (pre-registry gate)
//
// Gate A (B1A) fires before Gate B (B2A).  A strategy not in the plugin
// registry never reaches the DB registry check.  Pure in-process; no DB.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn b2a_n05_unknown_plugin_refused_at_bootstrap_gate() {
    let st = armed_live_shadow_state().await;
    st.set_strategy_fleet_for_test(Some(vec![fleet_entry("not_a_real_strategy")]))
        .await;

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("POST")
            .uri("/v1/run/start")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "N05: unknown plugin must return 403 at bootstrap gate; got: {status}"
    );
    let json = parse_json(body);
    assert_eq!(
        json["gate"], "native_strategy_bootstrap",
        "N05: gate must be native_strategy_bootstrap (B1A), not strategy_registry (B2A); got: {json}"
    );
}

// ---------------------------------------------------------------------------
// N02 — DB: swing_momentum + registry enabled=true → start succeeds
//
// Both Gate A (plugin present) and Gate B (registry enabled) pass.
// Start must succeed (200 OK).
// Skips gracefully without MQK_DATABASE_URL.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b2a_n02_registry_enabled_allows_activation() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("N02: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    mqk_db::migrate(&pool).await.expect("migrate");
    clean_db_state(&pool, "swing_momentum").await;

    // Insert enabled registry row.
    let now = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        &pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: "swing_momentum".to_string(),
            display_name: "Swing Momentum".to_string(),
            enabled: true,
            kind: "native".to_string(),
            registered_at_utc: now,
            updated_at_utc: now,
            note: String::new(),
        },
    )
    .await
    .expect("N02: upsert_strategy_registry_entry must succeed");

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
    assert_eq!(arm_status, StatusCode::OK, "N02: arm must succeed");

    st.set_strategy_fleet_for_test(Some(vec![fleet_entry("swing_momentum")]))
        .await;

    let (start_status, start_body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("POST")
            .uri("/v1/run/start")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(
        start_status,
        StatusCode::OK,
        "N02: swing_momentum with enabled registry row must start successfully; \
         got: {start_status} body: {}",
        String::from_utf8_lossy(&start_body)
    );

    // Cleanup.
    let _ = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("POST")
            .uri("/v1/run/stop")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    clean_db_state(&pool, "swing_momentum").await;
}

// ---------------------------------------------------------------------------
// N03 — DB: swing_momentum + registry enabled=false → 403 strategy_registry
//
// Gate A passes (plugin exists), Gate B fires (registry row present, disabled).
// Start must be refused with 403 and gate=strategy_registry.
// Skips gracefully without MQK_DATABASE_URL.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b2a_n03_registry_disabled_refused_at_registry_gate() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("N03: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    mqk_db::migrate(&pool).await.expect("migrate");
    clean_db_state(&pool, "swing_momentum").await;

    // Insert disabled registry row.
    let now = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        &pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: "swing_momentum".to_string(),
            display_name: "Swing Momentum".to_string(),
            enabled: false,
            kind: "native".to_string(),
            registered_at_utc: now,
            updated_at_utc: now,
            note: String::new(),
        },
    )
    .await
    .expect("N03: upsert_strategy_registry_entry must succeed");

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
    assert_eq!(arm_status, StatusCode::OK, "N03: arm must succeed");

    st.set_strategy_fleet_for_test(Some(vec![fleet_entry("swing_momentum")]))
        .await;

    let (start_status, start_body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("POST")
            .uri("/v1/run/start")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(
        start_status,
        StatusCode::FORBIDDEN,
        "N03: disabled registry row must refuse start with 403; \
         got: {start_status} body: {}",
        String::from_utf8_lossy(&start_body)
    );
    let json = parse_json(start_body);
    assert_eq!(
        json["gate"], "strategy_registry",
        "N03: gate must be strategy_registry (B2A); got: {json}"
    );
    assert_eq!(
        json["fault_class"], "runtime.start_refused.strategy_registry_disabled",
        "N03: fault_class must identify disabled registry; got: {json}"
    );
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("swing_momentum"),
        "N03: error must name the strategy; got: {json}"
    );

    clean_db_state(&pool, "swing_momentum").await;
}

// ---------------------------------------------------------------------------
// N04 — DB: swing_momentum + no registry row → 403 strategy_registry
//
// Gate A passes (plugin exists), Gate B fires (registry row absent).
// Start must be refused with 403 and gate=strategy_registry.
// Skips gracefully without MQK_DATABASE_URL.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn b2a_n04_registry_absent_refused_at_registry_gate() {
    let Some(pool) = db_pool_or_skip().await else {
        eprintln!("N04: skipped (MQK_DATABASE_URL not set)");
        return;
    };

    mqk_db::migrate(&pool).await.expect("migrate");
    // Ensure no registry row exists for swing_momentum.
    clean_db_state(&pool, "swing_momentum").await;

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
    assert_eq!(arm_status, StatusCode::OK, "N04: arm must succeed");

    st.set_strategy_fleet_for_test(Some(vec![fleet_entry("swing_momentum")]))
        .await;

    let (start_status, start_body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .method("POST")
            .uri("/v1/run/start")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;

    assert_eq!(
        start_status,
        StatusCode::FORBIDDEN,
        "N04: absent registry row must refuse start with 403; \
         got: {start_status} body: {}",
        String::from_utf8_lossy(&start_body)
    );
    let json = parse_json(start_body);
    assert_eq!(
        json["gate"], "strategy_registry",
        "N04: gate must be strategy_registry (B2A); got: {json}"
    );
    assert_eq!(
        json["fault_class"], "runtime.start_refused.strategy_registry_missing",
        "N04: fault_class must identify missing registry row; got: {json}"
    );
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("swing_momentum"),
        "N04: error must name the strategy; got: {json}"
    );
}
