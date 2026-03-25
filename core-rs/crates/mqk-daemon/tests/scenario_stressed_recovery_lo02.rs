//! LO-02: Stressed Recovery Proof Matrix — in-process and DB-backed proof slices.
//!
//! Proves the recovery behaviors named in
//! `docs/runbooks/stressed_recovery_proof_matrix.md` (SR-01 through SR-12).
//!
//! # Test plan
//!
//! SR-01 through SR-08 are in-process, unconditional, always runnable in CI.
//!
//! SR-09 through SR-12 are DB-backed and require MQK_DATABASE_URL.  They are
//! marked `#[ignore]` and must be opted into explicitly.  Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_stressed_recovery_lo02 -- \
//!   --include-ignored --test-threads=1
//!
//! # DB-backed recovery cases (SR-09..SR-12)
//!
//! | id     | scenario                                          | expected outcome             |
//! |--------|---------------------------------------------------|------------------------------|
//! | SR-09  | clean STOP → fresh restart                        | status = "idle" (safe)       |
//! | SR-10  | orphaned RUNNING run → fresh restart → start      | 409 + fault_class identified |
//! | SR-11  | active suppression → fresh AppState (same DB)     | decision seam returns "suppressed" |
//! | SR-12  | prior outbox row → fresh AppState + same key      | Ok(false), no double-enqueue |

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_daemon::{
    decision::{submit_internal_strategy_decision, InternalStrategyDecision},
    routes, state,
    suppression::{suppress_strategy, SuppressStrategyArgs},
};
use tower::ServiceExt;
use uuid::Uuid;

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

fn dev_router() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

// ---------------------------------------------------------------------------
// SR-01 — Fresh boot is disarmed and idle
//
// Proves: fresh daemon starts fail-closed (disarmed, idle, no active run).
// Operator must explicitly arm before any run can start.
// Matrix ref: stressed_recovery_proof_matrix.md SR-01
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo02_sr01_fresh_boot_is_disarmed_and_idle() {
    let router = dev_router();
    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK);
    let json = parse_json(body);

    assert_eq!(
        json["state"], "idle",
        "SR-01: fresh boot must be idle, not running"
    );
    assert_eq!(
        json["integrity_armed"], false,
        "SR-01: fresh boot must be disarmed (fail-closed)"
    );
    assert!(
        json["active_run_id"].is_null(),
        "SR-01: fresh boot must have no active run; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// SR-02 — Poisoned in-memory cache cannot survive a cold start
//
// Proves: even if the in-process status struct is set to "running" via an
// in-memory write, GET /v1/status returns "idle" because the daemon does
// not honour placeholder running state without DB authority.
// Matrix ref: stressed_recovery_proof_matrix.md SR-02
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo02_sr02_placeholder_running_cannot_survive_cold_start() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Inject poisoned in-memory state: pretend the daemon is "running".
    {
        let mut status = st.status.write().await;
        status.state = "running".to_string();
        status.active_run_id = Some(uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"lo02-sr02-poisoned-state",
        ));
        status.notes = Some("poisoned in-memory state for SR-02 test".to_string());
    }

    let router = routes::build_router(Arc::clone(&st));
    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status_code, body) = call(router, req).await;
    assert_eq!(status_code, StatusCode::OK);
    let json = parse_json(body);

    assert_eq!(
        json["state"], "idle",
        "SR-02: poisoned in-memory running state must not survive — got: {json}"
    );
    assert!(
        json["active_run_id"].is_null(),
        "SR-02: poisoned active_run_id must not be reported — got: {json}"
    );
}

// ---------------------------------------------------------------------------
// SR-03 — Missing operator token fails closed on operator routes
//
// Proves: with MissingTokenFailClosed auth mode, operator routes return 503
// with gate=operator_auth_config; the daemon does not permit privileged actions.
// Read-only health check still works.
// Matrix ref: stressed_recovery_proof_matrix.md SR-03
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo02_sr03_missing_operator_token_fails_closed() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::MissingTokenFailClosed,
    ));
    let router = routes::build_router(st);

    // Operator route: arm — must be refused.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, arm_body) = call(router.clone(), arm_req).await;
    assert_eq!(
        arm_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "SR-03: arm must be refused when operator token is missing"
    );
    let arm_json = parse_json(arm_body);
    assert_eq!(
        arm_json["gate"], "operator_auth_config",
        "SR-03: refusal gate must be operator_auth_config; got: {arm_json}"
    );

    // Operator route: run/start — must also be refused.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (start_status, start_body) = call(router.clone(), start_req).await;
    assert_eq!(
        start_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "SR-03: start must be refused when operator token is missing"
    );
    let start_json = parse_json(start_body);
    assert_eq!(
        start_json["gate"], "operator_auth_config",
        "SR-03: start refusal gate must be operator_auth_config; got: {start_json}"
    );

    // Read-only route: health — must remain available.
    let health_req = Request::builder()
        .method("GET")
        .uri("/v1/health")
        .body(axum::body::Body::empty())
        .unwrap();
    let (health_status, health_body) = call(router, health_req).await;
    assert_eq!(
        health_status,
        StatusCode::OK,
        "SR-03: health must still be reachable when operator token is missing"
    );
    assert_eq!(
        parse_json(health_body)["ok"],
        true,
        "SR-03: health must return ok=true"
    );
}

// ---------------------------------------------------------------------------
// SR-04 — Mode-change guidance is non-empty and actionable
//
// Proves: POST action change-system-mode returns 409 + a ModeChangeGuidanceResponse
// with non-empty preconditions and operator_next_steps — not a crash, not a
// silent 400, not an empty body.
// Also proves: GET /api/v1/ops/mode-change-guidance returns 200 with the same
// structure.
// Matrix ref: stressed_recovery_proof_matrix.md SR-04
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo02_sr04_mode_change_guidance_is_non_empty_and_actionable() {
    let router = dev_router();

    // POST action — must return 409 with guidance body.
    let action_req = Request::builder()
        .method("POST")
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"action_key":"change-system-mode"}"#,
        ))
        .unwrap();
    let (action_status, action_body) = call(router.clone(), action_req).await;
    assert_eq!(
        action_status,
        StatusCode::CONFLICT,
        "SR-04: change-system-mode must return 409, not crash"
    );
    let action_json = parse_json(action_body);
    assert_eq!(
        action_json["transition_permitted"], false,
        "SR-04: transition_permitted must be false; got: {action_json}"
    );
    let preconditions = action_json["preconditions"]
        .as_array()
        .expect("SR-04: preconditions must be an array");
    assert!(
        !preconditions.is_empty(),
        "SR-04: preconditions must be non-empty; got: {action_json}"
    );
    let steps = action_json["operator_next_steps"]
        .as_array()
        .expect("SR-04: operator_next_steps must be an array");
    assert!(
        !steps.is_empty(),
        "SR-04: operator_next_steps must be non-empty; got: {action_json}"
    );
    assert!(
        action_json["canonical_route"].as_str().is_some(),
        "SR-04: canonical_route must be present; got: {action_json}"
    );

    // GET guidance — must return 200 with the same structure.
    let guidance_req = Request::builder()
        .method("GET")
        .uri("/api/v1/ops/mode-change-guidance")
        .body(axum::body::Body::empty())
        .unwrap();
    let (guidance_status, guidance_body) = call(router, guidance_req).await;
    assert_eq!(
        guidance_status,
        StatusCode::OK,
        "SR-04: GET mode-change-guidance must return 200"
    );
    let guidance_json = parse_json(guidance_body);
    assert_eq!(
        guidance_json["transition_permitted"], false,
        "SR-04: GET guidance transition_permitted must be false; got: {guidance_json}"
    );
    assert!(
        guidance_json["preconditions"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "SR-04: GET guidance preconditions must be non-empty; got: {guidance_json}"
    );
    assert_eq!(
        action_json["canonical_route"], guidance_json["canonical_route"],
        "SR-04: POST and GET must agree on canonical_route"
    );
}

// ---------------------------------------------------------------------------
// SR-05 — Run/start without DB returns explicit error, not crash
//
// Proves: POST /v1/run/start after arm, with no DB pool configured, returns 503
// with a clear error message explaining the DB requirement.
// Matrix ref: stressed_recovery_proof_matrix.md SR-05
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo02_sr05_start_without_db_returns_explicit_error() {
    // BRK-00R-04: paper+alpaca is now blocked by the WS continuity gate before
    // the DB gate.  Use live-shadow+alpaca (no WS continuity gate) to prove the
    // DB-backed runtime requirement survives after arm.
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));

    // Arm first.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "SR-05: arm should succeed");

    // Now try to start — must return 503 with a clear message.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (start_status, start_body) = call(routes::build_router(Arc::clone(&st)), start_req).await;
    assert_eq!(
        start_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "SR-05: run/start without DB must return 503"
    );
    let json = parse_json(start_body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "SR-05: error must explain the DB requirement; got: {json}"
    );
    assert_eq!(
        json["fault_class"], "runtime.start_refused.service_unavailable",
        "SR-05: fault_class must be explicit; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// SR-06 — Halt without DB returns explicit error, not crash
//
// Proves: POST /v1/run/halt with no DB pool configured returns 503 with a
// clear error message.  Halt requires DB authority to persist the halt record.
// Matrix ref: stressed_recovery_proof_matrix.md SR-06
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo02_sr06_halt_without_db_returns_explicit_error() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(st);

    let halt_req = Request::builder()
        .method("POST")
        .uri("/v1/run/halt")
        .body(axum::body::Body::empty())
        .unwrap();
    let (halt_status, halt_body) = call(router, halt_req).await;
    assert_eq!(
        halt_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "SR-06: halt without DB must return 503"
    );
    let json = parse_json(halt_body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "SR-06: error must explain the DB requirement; got: {json}"
    );
    assert_eq!(
        json["fault_class"], "runtime.start_refused.service_unavailable",
        "SR-06: fault_class must be explicit; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// SR-07 — Stop on idle is idempotent
//
// Proves: POST /v1/run/stop when already idle returns 200 with state=idle —
// no error, no invented state, no crash.
// Matrix ref: stressed_recovery_proof_matrix.md SR-07
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo02_sr07_stop_on_idle_is_idempotent() {
    let router = dev_router();
    let stop_req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        .body(axum::body::Body::empty())
        .unwrap();
    let (stop_status, stop_body) = call(router, stop_req).await;
    assert_eq!(
        stop_status,
        StatusCode::OK,
        "SR-07: stop on idle must return 200"
    );
    let json = parse_json(stop_body);
    assert_eq!(
        json["state"], "idle",
        "SR-07: stop on idle must return idle state; got: {json}"
    );
    assert!(
        json["active_run_id"].is_null(),
        "SR-07: stop on idle must not invent a run_id; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// SR-08 — Arm/disarm cycle is stable after stress
//
// Proves: arm → disarm → arm produces consistent state transitions, and
// disarm on an already-disarmed state returns armed=false cleanly.
// Matrix ref: stressed_recovery_proof_matrix.md SR-08
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo02_sr08_arm_disarm_cycle_is_stable() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // 1. Disarm on boot state (already disarmed) — idempotent.
    let disarm1_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/disarm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (d1_status, d1_body) = call(routes::build_router(Arc::clone(&st)), disarm1_req).await;
    assert_eq!(
        d1_status,
        StatusCode::OK,
        "SR-08: disarm on boot must be 200"
    );
    assert_eq!(
        parse_json(d1_body)["armed"],
        false,
        "SR-08: disarm on boot must return armed=false"
    );

    // 2. Arm.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, arm_body) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "SR-08: arm must be 200");
    assert_eq!(
        parse_json(arm_body)["armed"],
        true,
        "SR-08: arm must return armed=true"
    );

    // Verify via status.
    let status_req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, status_body) = call(routes::build_router(Arc::clone(&st)), status_req).await;
    assert_eq!(
        parse_json(status_body)["integrity_armed"],
        true,
        "SR-08: status must reflect armed=true after arm"
    );

    // 3. Disarm.
    let disarm2_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/disarm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (d2_status, d2_body) = call(routes::build_router(Arc::clone(&st)), disarm2_req).await;
    assert_eq!(
        d2_status,
        StatusCode::OK,
        "SR-08: second disarm must be 200"
    );
    assert_eq!(
        parse_json(d2_body)["armed"],
        false,
        "SR-08: second disarm must return armed=false"
    );

    // 4. Arm again — verifies the cycle is repeatable.
    let arm2_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm2_status, arm2_body) = call(routes::build_router(Arc::clone(&st)), arm2_req).await;
    assert_eq!(arm2_status, StatusCode::OK, "SR-08: second arm must be 200");
    assert_eq!(
        parse_json(arm2_body)["armed"],
        true,
        "SR-08: second arm must return armed=true"
    );
}

// ===========================================================================
// Phase 2: DB-backed stressed recovery cases (SR-09 through SR-12)
//
// Require MQK_DATABASE_URL; marked #[ignore].
// Run with: --include-ignored --test-threads=1
// ===========================================================================

// ---------------------------------------------------------------------------
// DB pool helper (SR-09..SR-12)
// ---------------------------------------------------------------------------

async fn lo02_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_stressed_recovery_lo02 \
             -- --include-ignored --test-threads=1"
        )
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to test DB");
    mqk_db::migrate(&pool).await.expect("run migrations");
    // Clean daemon runs (cascades to oms_outbox rows).
    sqlx::query("DELETE FROM runs WHERE engine_id = 'mqk-daemon'")
        .execute(&pool)
        .await
        .expect("cleanup daemon runs");
    // Clean persisted arm state so each test starts from a known baseline.
    sqlx::query("DELETE FROM sys_arm_state WHERE sentinel_id = 1")
        .execute(&pool)
        .await
        .expect("cleanup sys_arm_state");
    pool
}

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

async fn seed_registry_lo02(pool: &sqlx::PgPool, strategy_id: &str) {
    let ts = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: format!("LO-02A test {strategy_id}"),
            enabled: true,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("seed_registry_lo02 failed");
}

// ---------------------------------------------------------------------------
// SR-09 — Clean stop + restart → status = "idle"
//
// Proves: a prior session that stopped cleanly (STOPPED run in DB) produces
// status = "idle" on the next fresh AppState boot — not "unknown", not
// "running".  The operator can safely arm and start a new session without
// any manual recovery.
//
// The STOPPED status is closed evidence that the prior session exited safely.
// Matrix ref: stressed_recovery_proof_matrix.md SR-09
// ---------------------------------------------------------------------------

/// SR-09: Clean stop + restart reports status = "idle".
///
/// After a prior session's run is stopped durably, a fresh daemon AppState
/// (same DB, no local loop) must report status = "idle" with no active_run_id.
/// This is the canonical safe-restart path — no operator recovery required.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn lo02_sr09_clean_stop_restart_reports_idle() {
    let pool = lo02_pool().await;
    let run_id = Uuid::new_v4();

    // Simulate a completed prior session: insert → arm → begin → stop.
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "daemon-runtime-paper-ready-v1".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await
    .expect("insert run");
    mqk_db::arm_run(&pool, run_id).await.expect("arm run");
    mqk_db::begin_run(&pool, run_id).await.expect("begin run");
    mqk_db::stop_run(&pool, run_id).await.expect("stop run");

    // Fresh AppState: simulates daemon restart on same DB.
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status_code, body) = call(routes::build_router(Arc::clone(&st)), req).await;
    assert_eq!(status_code, StatusCode::OK);
    let json = parse_json(body);

    assert_eq!(
        json["state"], "idle",
        "SR-09: STOPPED run + fresh AppState must report 'idle', not 'unknown' or 'running'; \
         got: {json}"
    );
    assert!(
        json["active_run_id"].is_null(),
        "SR-09: idle status must have null active_run_id; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// SR-10 — Orphaned RUNNING run blocks start on restart
//
// Proves: when a prior session left a RUNNING run in DB without stopping it
// (e.g., crash/kill -9), a fresh daemon's start route returns 409 with the
// canonical fault_class identifying the orphaned run conflict.
//
// This enforces the fail-closed restart contract: an unresolved active run
// must not be silently overridden by a new start.  The operator must clear
// the orphaned state before a new session can begin.
//
// Matrix ref: stressed_recovery_proof_matrix.md SR-10
// ---------------------------------------------------------------------------

/// SR-10: Orphaned RUNNING run in DB blocks start with the canonical fault class.
///
/// After simulating a crash (run left in RUNNING state, no local loop), a fresh
/// AppState must refuse start with 409 and
/// `fault_class = "runtime.truth_mismatch.durable_active_without_local_owner"`.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn lo02_sr10_orphaned_running_run_blocks_start_on_restart() {
    let pool = lo02_pool().await;
    let run_id = Uuid::new_v4();

    // Simulate a crashed prior session: insert → arm → begin (NO stop).
    // Mode must match the AppState's deployment mode below (LIVE-SHADOW).
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "LIVE-SHADOW".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "daemon-runtime-live-shadow-ready-v1".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await
    .expect("insert run");
    mqk_db::arm_run(&pool, run_id).await.expect("arm run");
    mqk_db::begin_run(&pool, run_id).await.expect("begin run");
    // No stop — run is orphaned in RUNNING state.

    // Fresh AppState with LiveShadow+Alpaca: start_allowed=true, no WS/reconcile gate.
    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool,
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    // Arm in-memory so the integrity gate passes.
    {
        let mut integrity = st.integrity.write().await;
        integrity.disarmed = false;
        integrity.halted = false;
    }

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (start_status, start_body) = call(routes::build_router(Arc::clone(&st)), req).await;

    assert_eq!(
        start_status,
        StatusCode::CONFLICT,
        "SR-10: orphaned RUNNING run must block start with 409; got: {}",
        parse_json(start_body.clone())
    );
    let json = parse_json(start_body);
    assert_eq!(
        json["fault_class"], "runtime.truth_mismatch.durable_active_without_local_owner",
        "SR-10: fault_class must identify the orphaned-run conflict precisely; got: {json}"
    );
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("durable active run exists without local ownership"),
        "SR-10: error message must describe the orphaned run; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// SR-11 — Active suppression survives restart and blocks decision seam
//
// Proves: a strategy suppression written in a prior session (durable DB row)
// is enforced by the decision seam in a fresh AppState on the same DB.
//
// This closes the gap between CC-02A (suppression persists in DB) and the
// decision seam enforcement across a simulated process restart.  The write
// is durable; the enforcement is not session-local.
//
// Matrix ref: stressed_recovery_proof_matrix.md SR-11
// ---------------------------------------------------------------------------

/// SR-11: Active suppression from prior session blocks decision seam in fresh AppState.
///
/// Session 1 suppresses a strategy (durable write).
/// Session 2 (fresh AppState, same DB) submits a decision for that strategy.
/// Gate 4 must return disposition = "suppressed" — durable suppression truth
/// is enforced even after a simulated process restart.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn lo02_sr11_active_suppression_survives_restart_and_blocks_decision_seam() {
    let pool = lo02_pool().await;
    let sid = unique_id("sr11");
    seed_registry_lo02(&pool, &sid).await;

    // Session 1: suppress the strategy.
    let st1 = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let suppress_out = suppress_strategy(
        &st1,
        SuppressStrategyArgs {
            suppression_id: Uuid::new_v4(),
            strategy_id: sid.clone(),
            trigger_domain: "risk".to_string(),
            trigger_reason: "SR-11 restart suppression proof".to_string(),
            started_at_utc: Utc::now(),
            note: String::new(),
        },
    )
    .await;
    assert!(
        suppress_out.suppressed,
        "SR-11: suppress_strategy must succeed before restart simulation; \
         disposition={}, blockers={:?}",
        suppress_out.disposition, suppress_out.blockers
    );

    // Session 2: fresh AppState (same DB) — simulates daemon restart.
    let st2 = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Submit a decision for the suppressed strategy.
    // Gates 0-3 pass; Gate 4 (suppression_check) must return "suppressed".
    let out = submit_internal_strategy_decision(
        &st2,
        InternalStrategyDecision {
            decision_id: unique_id("d"),
            strategy_id: sid.clone(),
            symbol: "AAPL".to_string(),
            side: "buy".to_string(),
            qty: 1,
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
        },
    )
    .await;

    assert_eq!(
        out.disposition, "suppressed",
        "SR-11: durable suppression from prior session must block the decision seam in fresh \
         AppState; disposition={}, blockers={:?}",
        out.disposition, out.blockers
    );
    assert!(
        !out.accepted,
        "SR-11: decision must not be accepted when strategy is suppressed; got: {out:?}"
    );
    assert!(
        out.blockers.iter().any(|b| b.contains("suppressed")),
        "SR-11: blocker message must mention suppression; got: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// SR-12 — Decision outbox idempotency is durable across simulated restart
//
// Proves: the durable outbox's global unique constraint on idempotency_key
// (ON CONFLICT DO NOTHING) prevents a second AppState instance (simulating
// a process restart) from inserting a second row for the same decision_id —
// even if the second call uses a different run_id.
//
// This is the durable foundation of Gate 7 in submit_internal_strategy_decision:
// if a decision was enqueued in a prior session, a fresh session cannot
// double-enqueue it regardless of which run is currently active.
//
// Matrix ref: stressed_recovery_proof_matrix.md SR-12
// ---------------------------------------------------------------------------

/// SR-12: Durable outbox idempotency holds across distinct AppState instances.
///
/// First enqueue under run_id_1 → Ok(true).
/// Second enqueue of same decision_key under run_id_2 → Ok(false), no new row.
/// Row count confirms exactly one outbox entry for the decision key.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run with --include-ignored"]
async fn lo02_sr12_decision_idempotency_across_restart() {
    let pool = lo02_pool().await;

    // Two distinct run_ids simulate two separate process lifetimes.
    let run_id_1 = Uuid::new_v4();
    let run_id_2 = Uuid::new_v4();
    let decision_key = unique_id("sr12-decision");

    // Insert both runs so the oms_outbox FK constraint is satisfied.
    for &run_id in &[run_id_1, run_id_2] {
        mqk_db::insert_run(
            &pool,
            &mqk_db::NewRun {
                run_id,
                engine_id: "mqk-daemon".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: Utc::now(),
                git_hash: "TEST".to_string(),
                config_hash: "daemon-runtime-paper-ready-v1".to_string(),
                config_json: serde_json::json!({}),
                host_fingerprint: "TESTHOST".to_string(),
            },
        )
        .await
        .expect("insert run");
    }

    let order_json = serde_json::json!({
        "symbol": "AAPL",
        "side": "buy",
        "qty": 1,
        "order_type": "market",
        "time_in_force": "day",
        "strategy_id": "sr12-probe",
        "signal_source": "internal_strategy_decision",
    });

    // Session 1: first enqueue — new row, must return Ok(true).
    let first = mqk_db::outbox_enqueue(&pool, run_id_1, &decision_key, order_json.clone())
        .await
        .expect("first outbox_enqueue must not error");
    assert!(
        first,
        "SR-12: first outbox_enqueue must return Ok(true) (new row inserted)"
    );

    // Session 2: fresh AppState (different run_id), same decision_key.
    // The global unique constraint must refuse the duplicate regardless of run_id.
    let second = mqk_db::outbox_enqueue(&pool, run_id_2, &decision_key, order_json.clone())
        .await
        .expect("second outbox_enqueue must not error");
    assert!(
        !second,
        "SR-12: second outbox_enqueue with same decision_key under different run_id \
         must return Ok(false) — global idempotency key is run-independent"
    );

    // Confirm exactly one row exists for this decision key.
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM oms_outbox WHERE idempotency_key = $1")
            .bind(&decision_key)
            .fetch_one(&pool)
            .await
            .expect("count outbox rows");
    assert_eq!(
        count, 1,
        "SR-12: exactly one outbox row must exist for decision_key; got {count}"
    );
}
