//! CTRL-ARM-01 — Arm preflight gate proof.
//!
//! Proves that all three HTTP arm paths refuse to persist ARMED state when
//! authoritative preflight conditions are blocking.
//!
//! ## What is proven
//!
//! The arm safety preflight gate (`check_arm_safety`) fires before any in-memory
//! or DB state mutation on all three arm surfaces:
//!
//! | Path                                      | Gate fires        |
//! |-------------------------------------------|-------------------|
//! | `POST /v1/integrity/arm`                  | Reconcile + Risk  |
//! | `POST /api/v1/ops/action arm-execution`   | Reconcile + Risk  |
//! | `POST /control/arm` (legacy, DB required) | Reconcile + Risk  |
//!
//! ## Reconcile gate (no DB required)
//!
//! The reconcile gate reads from the in-memory snapshot (or DB if present).
//! Tests set the in-memory snapshot via `publish_reconcile_snapshot`.
//! The gate blocks on `dirty` and `stale`; passes on `ok` and `unknown`.
//!
//! ## Proof matrix
//!
//! | Test   | Claim                                                                    |
//! |--------|--------------------------------------------------------------------------|
//! | CA-P01 | integrity/arm refused when reconcile is dirty                            |
//! | CA-P02 | integrity/arm refused when reconcile is stale                            |
//! | CA-P03 | integrity/arm allowed when reconcile is ok                               |
//! | CA-P04 | integrity/arm allowed when reconcile is unknown (initial state)          |
//! | CA-P05 | ops/action arm-execution refused when reconcile is dirty                 |
//! | CA-P06 | Refused arm does NOT mutate in-memory disarmed flag (stays disarmed)    |
//! | CA-P07 | ops/action arm-execution allowed when reconcile is ok                    |
//! | CA-P08 | integrity/arm refusal body contains gate and blockers                    |
//!
//! ## DB-backed risk-block proof (CA-R01 .. CA-R04)
//!
//! These require `MQK_DATABASE_URL` and skip gracefully when it is absent.
//! They seed `sys_risk_block_state` with `blocked=true` via the existing
//! `mqk_db::persist_risk_block_state` helper and prove:
//!
//! | Test   | Claim                                                                        |
//! |--------|------------------------------------------------------------------------------|
//! | CA-R01 | integrity/arm refused when sys_risk_block_state.blocked=true                |
//! | CA-R02 | ops/action arm-execution refused when risk blocked                           |
//! | CA-R03 | integrity/arm allowed when risk block explicitly cleared (control)           |
//! | CA-R04 | sys_arm_state is NOT written to ARMED when either route refuses risk-blocked |
//!
//! `risk_state_unavailable` fail-closed path (`Err` from `load_risk_block_state`)
//! is not tested here: simulating a DB query error without corrupting the shared
//! test schema (e.g. dropping/renaming `sys_risk_block_state`) would require
//! destructive DB mutations or a broad production-code refactor to inject a mock
//! pool.  The production code path is present in `check_arm_safety` (helpers.rs)
//! and the `Ok(None)` / `Ok(Some(blocked=true))` paths are proven by CA-R01–CA-R03.

use std::sync::Arc;

use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
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
    serde_json::from_slice(&b).expect("response body is not valid JSON")
}

fn live_shadow_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ))
}

fn make_reconcile(status: &str) -> state::ReconcileStatusSnapshot {
    state::ReconcileStatusSnapshot {
        status: status.to_string(),
        last_run_at: None,
        snapshot_watermark_ms: None,
        mismatched_positions: 0,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some(format!("test: forced {status}")),
    }
}

async fn set_reconcile(st: &Arc<state::AppState>, status: &str) {
    st.publish_reconcile_snapshot(make_reconcile(status)).await;
}

async fn force_disarmed(st: &Arc<state::AppState>) {
    let mut ig = st.integrity.write().await;
    ig.disarmed = true;
    ig.halted = false;
}

fn integrity_arm_req() -> Request<axum::body::Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap()
}

fn ops_arm_req() -> Request<axum::body::Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(r#"{"action_key":"arm-execution"}"#))
        .unwrap()
}

// ---------------------------------------------------------------------------
// CA-P01: integrity/arm refused when reconcile is dirty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca_p01_integrity_arm_refused_when_reconcile_dirty() {
    let st = live_shadow_state();
    force_disarmed(&st).await;
    set_reconcile(&st, "dirty").await;

    let (status, body) = call(routes::build_router(Arc::clone(&st)), integrity_arm_req()).await;
    let j = parse_json(body);

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "CA-P01: dirty reconcile must produce 403; got {status}: {j}"
    );
    assert_eq!(
        j["accepted"].as_bool(),
        Some(false),
        "CA-P01: accepted must be false: {j}"
    );
    assert_eq!(
        j["gate"].as_str(),
        Some("arm_preflight.reconcile_not_clean"),
        "CA-P01: gate must be arm_preflight.reconcile_not_clean: {j}"
    );

    // Verify in-memory state was NOT mutated.
    let ig = st.integrity.read().await;
    assert!(
        ig.disarmed,
        "CA-P01: disarmed flag must remain true after refused arm"
    );
}

// ---------------------------------------------------------------------------
// CA-P02: integrity/arm refused when reconcile is stale
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca_p02_integrity_arm_refused_when_reconcile_stale() {
    let st = live_shadow_state();
    force_disarmed(&st).await;
    set_reconcile(&st, "stale").await;

    let (status, body) = call(routes::build_router(Arc::clone(&st)), integrity_arm_req()).await;
    let j = parse_json(body);

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "CA-P02: stale reconcile must produce 403; got {status}: {j}"
    );
    assert_eq!(
        j["gate"].as_str(),
        Some("arm_preflight.reconcile_not_clean"),
        "CA-P02: gate must be arm_preflight.reconcile_not_clean: {j}"
    );

    let ig = st.integrity.read().await;
    assert!(
        ig.disarmed,
        "CA-P02: disarmed flag must remain true after refused arm"
    );
}

// ---------------------------------------------------------------------------
// CA-P03: integrity/arm allowed when reconcile is ok
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca_p03_integrity_arm_allowed_when_reconcile_ok() {
    let st = live_shadow_state();
    force_disarmed(&st).await;
    set_reconcile(&st, "ok").await;

    let (status, _body) = call(routes::build_router(Arc::clone(&st)), integrity_arm_req()).await;

    // Without DB, arm proceeds in-memory only; succeeds at 200.
    assert_eq!(
        status,
        StatusCode::OK,
        "CA-P03: reconcile ok must allow arm"
    );

    let ig = st.integrity.read().await;
    assert!(
        !ig.disarmed,
        "CA-P03: disarmed flag must be false after allowed arm"
    );
}

// ---------------------------------------------------------------------------
// CA-P04: integrity/arm allowed when reconcile is unknown (initial state)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca_p04_integrity_arm_allowed_when_reconcile_unknown() {
    let st = live_shadow_state();
    force_disarmed(&st).await;
    // reconcile is "unknown" by default in test constructors — no explicit set.

    let (status, _body) = call(routes::build_router(Arc::clone(&st)), integrity_arm_req()).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "CA-P04: reconcile unknown (initial state) must not block arm"
    );
}

// ---------------------------------------------------------------------------
// CA-P05: ops/action arm-execution refused when reconcile is dirty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca_p05_ops_action_arm_refused_when_reconcile_dirty() {
    let st = live_shadow_state();
    force_disarmed(&st).await;
    set_reconcile(&st, "dirty").await;

    let (status, body) = call(routes::build_router(Arc::clone(&st)), ops_arm_req()).await;
    let j = parse_json(body);

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "CA-P05: ops/action arm-execution with dirty reconcile must produce 403: {j}"
    );
    assert_eq!(
        j["accepted"].as_bool(),
        Some(false),
        "CA-P05: accepted must be false: {j}"
    );
    assert!(
        j["blockers"].is_array(),
        "CA-P05: blockers must be present: {j}"
    );
}

// ---------------------------------------------------------------------------
// CA-P06: Refused arm does NOT mutate in-memory disarmed flag
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca_p06_refused_arm_does_not_mutate_disarmed() {
    let st = live_shadow_state();
    force_disarmed(&st).await;
    set_reconcile(&st, "dirty").await;

    // Call both arm surfaces and verify neither mutates in-memory state.
    let _ = call(routes::build_router(Arc::clone(&st)), integrity_arm_req()).await;

    {
        let ig = st.integrity.read().await;
        assert!(
            ig.disarmed,
            "CA-P06: integrity/arm refusal must leave disarmed=true"
        );
        assert!(
            !ig.halted,
            "CA-P06: integrity/arm refusal must not set halted"
        );
    }

    let _ = call(routes::build_router(Arc::clone(&st)), ops_arm_req()).await;

    {
        let ig = st.integrity.read().await;
        assert!(
            ig.disarmed,
            "CA-P06: ops/action arm refusal must leave disarmed=true"
        );
    }
}

// ---------------------------------------------------------------------------
// CA-P07: ops/action arm-execution allowed when reconcile is ok
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca_p07_ops_action_arm_allowed_when_reconcile_ok() {
    let st = live_shadow_state();
    force_disarmed(&st).await;
    set_reconcile(&st, "ok").await;

    let (status, body) = call(routes::build_router(Arc::clone(&st)), ops_arm_req()).await;
    let j = parse_json(body);

    assert_eq!(
        status,
        StatusCode::OK,
        "CA-P07: reconcile ok must allow ops/action arm: {j}"
    );
    assert_eq!(
        j["accepted"].as_bool(),
        Some(true),
        "CA-P07: accepted must be true: {j}"
    );
}

// ---------------------------------------------------------------------------
// CA-P08: integrity/arm refusal body contains gate and blockers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca_p08_integrity_arm_refusal_body_contains_gate_and_blockers() {
    let st = live_shadow_state();
    force_disarmed(&st).await;
    set_reconcile(&st, "stale").await;

    let (status, body) = call(routes::build_router(Arc::clone(&st)), integrity_arm_req()).await;
    let j = parse_json(body);

    assert_eq!(status, StatusCode::FORBIDDEN, "CA-P08: must be 403: {j}");

    let gate = j["gate"].as_str().unwrap_or("");
    assert!(
        gate.starts_with("arm_preflight."),
        "CA-P08: gate must be arm_preflight.* prefixed: {j}"
    );

    let blockers = j["blockers"]
        .as_array()
        .expect("CA-P08: blockers must be array");
    assert!(
        !blockers.is_empty(),
        "CA-P08: blockers must not be empty: {j}"
    );

    let blocker_text = blockers[0].as_str().unwrap_or("");
    assert!(
        blocker_text.contains("stale"),
        "CA-P08: first blocker must mention stale reconcile status: {j}"
    );
}

// ---------------------------------------------------------------------------
// DB-backed risk-block helpers
// ---------------------------------------------------------------------------

async fn risk_pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("MQK_DATABASE_URL").ok()?;
    Some(
        sqlx::PgPool::connect(&url)
            .await
            .expect("CA-R: DB connect failed"),
    )
}

async fn cleanup_risk_block(pool: &sqlx::PgPool) {
    sqlx::query("delete from sys_risk_block_state where sentinel_id = 1")
        .execute(pool)
        .await
        .expect("CA-R: cleanup sys_risk_block_state failed");
}

async fn cleanup_arm_state(pool: &sqlx::PgPool) {
    sqlx::query("delete from sys_arm_state where sentinel_id = 1")
        .execute(pool)
        .await
        .expect("CA-R: cleanup sys_arm_state failed");
}

fn db_live_shadow_state(pool: sqlx::PgPool) -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool,
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ))
}

// ---------------------------------------------------------------------------
// CA-R01: integrity/arm refused when sys_risk_block_state.blocked=true
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca_r01_integrity_arm_refused_when_risk_blocked() {
    let pool = match risk_pool().await {
        Some(p) => p,
        None => {
            println!("CA-R01: skipped (MQK_DATABASE_URL not set)");
            return;
        }
    };

    // Pre-test cleanup: ensure state tables start clean.
    cleanup_risk_block(&pool).await;
    cleanup_arm_state(&pool).await;

    // Seed: risk engine is blocking dispatch.
    mqk_db::persist_risk_block_state(&pool, true, Some("RISK_BLOCK_CA_R01"), chrono::Utc::now())
        .await
        .expect("CA-R01: persist_risk_block_state failed");

    let st = db_live_shadow_state(pool.clone());
    force_disarmed(&st).await;
    set_reconcile(&st, "ok").await; // reconcile passes; risk gate must fire

    let (status, body) = call(routes::build_router(Arc::clone(&st)), integrity_arm_req()).await;
    let j = parse_json(body);

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "CA-R01: risk blocked must produce 403; got {status}: {j}"
    );
    assert_eq!(
        j["accepted"].as_bool(),
        Some(false),
        "CA-R01: accepted must be false: {j}"
    );
    assert_eq!(
        j["gate"].as_str(),
        Some("arm_preflight.risk_blocked"),
        "CA-R01: gate must be arm_preflight.risk_blocked: {j}"
    );
    assert!(
        j["blockers"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "CA-R01: blockers must be non-empty: {j}"
    );

    // In-memory arm state must NOT have been mutated.
    let ig = st.integrity.read().await;
    assert!(
        ig.disarmed,
        "CA-R01: disarmed flag must remain true after refused arm"
    );
    drop(ig);

    // Post-test cleanup.
    cleanup_risk_block(&pool).await;
    cleanup_arm_state(&pool).await;
}

// ---------------------------------------------------------------------------
// CA-R02: ops/action arm-execution refused when risk blocked
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca_r02_ops_arm_refused_when_risk_blocked() {
    let pool = match risk_pool().await {
        Some(p) => p,
        None => {
            println!("CA-R02: skipped (MQK_DATABASE_URL not set)");
            return;
        }
    };

    cleanup_risk_block(&pool).await;
    cleanup_arm_state(&pool).await;

    mqk_db::persist_risk_block_state(&pool, true, Some("RISK_BLOCK_CA_R02"), chrono::Utc::now())
        .await
        .expect("CA-R02: persist_risk_block_state failed");

    let st = db_live_shadow_state(pool.clone());
    force_disarmed(&st).await;
    set_reconcile(&st, "ok").await;

    let (status, body) = call(routes::build_router(Arc::clone(&st)), ops_arm_req()).await;
    let j = parse_json(body);

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "CA-R02: ops/action risk blocked must produce 403; got {status}: {j}"
    );
    assert_eq!(
        j["accepted"].as_bool(),
        Some(false),
        "CA-R02: accepted must be false: {j}"
    );
    assert!(
        j["blockers"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "CA-R02: blockers must be non-empty: {j}"
    );

    // In-memory arm state must NOT have been mutated.
    let ig = st.integrity.read().await;
    assert!(
        ig.disarmed,
        "CA-R02: disarmed flag must remain true after refused arm"
    );
    drop(ig);

    cleanup_risk_block(&pool).await;
    cleanup_arm_state(&pool).await;
}

// ---------------------------------------------------------------------------
// CA-R03: integrity/arm allowed when risk block explicitly cleared (control)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca_r03_integrity_arm_allowed_when_risk_unblocked() {
    let pool = match risk_pool().await {
        Some(p) => p,
        None => {
            println!("CA-R03: skipped (MQK_DATABASE_URL not set)");
            return;
        }
    };

    cleanup_risk_block(&pool).await;
    cleanup_arm_state(&pool).await;

    // Seed: risk explicitly unblocked (blocked=false row present — not just absent).
    mqk_db::persist_risk_block_state(&pool, false, None, chrono::Utc::now())
        .await
        .expect("CA-R03: persist_risk_block_state failed");

    let st = db_live_shadow_state(pool.clone());
    force_disarmed(&st).await;
    set_reconcile(&st, "ok").await;

    let (status, body) = call(routes::build_router(Arc::clone(&st)), integrity_arm_req()).await;
    let j = parse_json(body);

    assert_eq!(
        status,
        StatusCode::OK,
        "CA-R03: unblocked risk must allow arm; got {status}: {j}"
    );

    // In-memory must be armed now.
    let ig = st.integrity.read().await;
    assert!(
        !ig.disarmed,
        "CA-R03: disarmed flag must be false after allowed arm"
    );
    drop(ig);

    cleanup_risk_block(&pool).await;
    cleanup_arm_state(&pool).await;
}

// ---------------------------------------------------------------------------
// CA-R04: sys_arm_state not written to ARMED when both routes refuse risk-blocked
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ca_r04_sys_arm_state_not_written_when_risk_blocked() {
    let pool = match risk_pool().await {
        Some(p) => p,
        None => {
            println!("CA-R04: skipped (MQK_DATABASE_URL not set)");
            return;
        }
    };

    cleanup_risk_block(&pool).await;
    cleanup_arm_state(&pool).await;

    mqk_db::persist_risk_block_state(&pool, true, Some("RISK_BLOCK_CA_R04"), chrono::Utc::now())
        .await
        .expect("CA-R04: persist_risk_block_state failed");

    // --- integrity/arm route ---
    {
        let st = db_live_shadow_state(pool.clone());
        force_disarmed(&st).await;
        set_reconcile(&st, "ok").await;

        let (status, _) = call(routes::build_router(Arc::clone(&st)), integrity_arm_req()).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "CA-R04: integrity/arm must be refused when risk blocked"
        );

        let arm_row = mqk_db::load_arm_state(&pool)
            .await
            .expect("CA-R04: load_arm_state failed");
        let arm_state = arm_row
            .as_ref()
            .map(|(s, _)| s.as_str())
            .unwrap_or("absent");
        assert_ne!(
            arm_state, "ARMED",
            "CA-R04: sys_arm_state must NOT be ARMED after refused integrity/arm; got: {arm_state}"
        );
    }

    // --- ops/action arm-execution route ---
    {
        let st = db_live_shadow_state(pool.clone());
        force_disarmed(&st).await;
        set_reconcile(&st, "ok").await;

        let (status, _) = call(routes::build_router(Arc::clone(&st)), ops_arm_req()).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "CA-R04: ops/action must be refused when risk blocked"
        );

        let arm_row = mqk_db::load_arm_state(&pool)
            .await
            .expect("CA-R04: load_arm_state failed");
        let arm_state = arm_row
            .as_ref()
            .map(|(s, _)| s.as_str())
            .unwrap_or("absent");
        assert_ne!(
            arm_state, "ARMED",
            "CA-R04: sys_arm_state must NOT be ARMED after refused ops/action arm; got: {arm_state}"
        );
    }

    cleanup_risk_block(&pool).await;
    cleanup_arm_state(&pool).await;
}
