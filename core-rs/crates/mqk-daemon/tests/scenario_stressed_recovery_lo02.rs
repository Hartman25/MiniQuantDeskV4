//! LO-02: Stressed Recovery Proof Matrix — in-process proof slice.
//!
//! Proves the in-process (no DB needed) recovery behaviors named in
//! `docs/runbooks/stressed_recovery_proof_matrix.md` (SR-01 through SR-08).
//!
//! DB-backed cases (SR-09 through SR-12) are covered by the `#[ignore]`-gated
//! tests in `scenario_daemon_runtime_lifecycle.rs` and are referenced in the
//! matrix doc.
//!
//! These tests are always runnable in CI without any environment variables.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
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
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
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
