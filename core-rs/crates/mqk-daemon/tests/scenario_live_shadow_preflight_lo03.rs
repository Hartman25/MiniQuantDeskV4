//! LO-03: Live-shadow / live-capital preflight proof.
//!
//! Proves the daemon-side preconditions and gates that must hold before
//! transitioning to live-shadow or live-capital posture, as described in
//! `docs/runbooks/live_shadow_operational_proof.md` (Leg 2).
//!
//! These tests cover the in-process (no DB needed) precondition verification.
//! They complement the artifact-chain proof (research-py tests) and the
//! execution-gate proof (mqk-strategy / mqk-reconcile tests).
//!
//! All tests are always runnable in CI without any environment variables.

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

// ---------------------------------------------------------------------------
// P-01 — Daemon boots disarmed: precondition holds for live-shadow/capital
//
// The operator must explicitly arm before any execution can start.
// This is the first daemon-side precondition for live-shadow posture.
// Doc ref: live_shadow_operational_proof.md Leg 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03_p01_daemon_boots_disarmed_precondition_holds() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(Arc::clone(&st));

    let health_req = Request::builder()
        .method("GET")
        .uri("/v1/health")
        .body(axum::body::Body::empty())
        .unwrap();
    let (health_status, health_body) = call(router.clone(), health_req).await;
    assert_eq!(
        health_status,
        StatusCode::OK,
        "P-01: daemon must be reachable at boot"
    );
    assert_eq!(
        parse_json(health_body)["ok"],
        true,
        "P-01: health must return ok=true"
    );

    let status_req = Request::builder()
        .method("GET")
        .uri("/v1/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (_, status_body) = call(router, status_req).await;
    let json = parse_json(status_body);
    assert_eq!(
        json["integrity_armed"], false,
        "P-01: daemon must boot disarmed — operator must arm explicitly before live-shadow posture; got: {json}"
    );
    assert_eq!(
        json["state"], "idle",
        "P-01: daemon must boot idle; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// P-02 — Run/start is blocked before explicit arm
//
// Proves: the integrity gate blocks execution without an explicit arm.
// No run can start before the operator deliberately arms the daemon.
// Doc ref: live_shadow_operational_proof.md Leg 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03_p02_start_blocked_before_arm() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(st);

    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (start_status, start_body) = call(router, start_req).await;
    assert_eq!(
        start_status,
        StatusCode::FORBIDDEN,
        "P-02: run/start must be 403 before arm — gate blocks execution without explicit arm"
    );
    let json = parse_json(start_body);
    assert_eq!(
        json["gate"], "integrity_armed",
        "P-02: gate must be integrity_armed; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// P-03 — Run/start is blocked without DB authority
//
// Proves: even after arm, execution requires a DB-backed runtime.
// A live or shadow run cannot start without durable DB backing.
// Doc ref: live_shadow_operational_proof.md Leg 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03_p03_start_blocked_without_db_authority() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    // Arm the integrity gate.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(
        arm_status,
        StatusCode::OK,
        "P-03: arm must succeed before DB gate test"
    );

    // Now try to start — DB is not configured, must return 503.
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (start_status, start_body) = call(routes::build_router(Arc::clone(&st)), start_req).await;
    assert_eq!(
        start_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "P-03: run/start after arm but without DB must return 503 — live run requires DB backing"
    );
    let json = parse_json(start_body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "P-03: error must state DB is not configured; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// P-04 — Halt is blocked without DB authority
//
// Proves: halt requires DB authority to persist the halt record durably.
// An undurable halt would leave no audit trail and would not survive restart.
// Doc ref: live_shadow_operational_proof.md Leg 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03_p04_halt_blocked_without_db_authority() {
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
        "P-04: halt without DB must return 503 — halt must be durable"
    );
    let json = parse_json(halt_body);
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "P-04: error must state DB is not configured; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// P-05 — Mode-change is guided, not silent
//
// Proves: a mode-change attempt returns explicit operator guidance with
// named preconditions and steps.  This is the path an operator would hit
// during a supervised paper → live transition.
// Doc ref: live_shadow_operational_proof.md Leg 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03_p05_mode_change_guidance_explicit_and_actionable() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(st);

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/ops/mode-change-guidance")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "P-05: mode-change guidance must return 200"
    );

    let json = parse_json(body);

    // Transition must be refused (hot switching not supported).
    assert_eq!(
        json["transition_permitted"], false,
        "P-05: transition_permitted must be false (no hot switching); got: {json}"
    );

    // Refused but not a dead end: preconditions must be listed.
    let preconditions = json["preconditions"]
        .as_array()
        .expect("P-05: preconditions must be an array");
    assert!(
        !preconditions.is_empty(),
        "P-05: preconditions must be non-empty (guidance must be actionable); got: {json}"
    );

    // Steps must be listed and numbered.
    let steps = json["operator_next_steps"]
        .as_array()
        .expect("P-05: operator_next_steps must be an array");
    assert!(
        steps.len() >= 5,
        "P-05: at least 5 operator steps must be present; got: {} steps in {json}",
        steps.len()
    );

    // Canonical route must be present so the operator knows where to look.
    let canonical_route = json["canonical_route"].as_str().unwrap_or("");
    assert!(
        canonical_route.contains("mode-change-guidance"),
        "P-05: canonical_route must reference mode-change-guidance; got: {canonical_route}"
    );

    // Refused reason must be present and non-empty.
    let refused_reason = json["transition_refused_reason"].as_str().unwrap_or("");
    assert!(
        !refused_reason.is_empty(),
        "P-05: transition_refused_reason must be non-empty; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// P-06 — Operator token gate is fail-closed (live/capital safety gate)
//
// Proves: with missing operator token, ALL operator routes are refused.
// No execution can be started without an explicit operator token in production mode.
// This is the outermost safety gate for live-capital posture.
// Doc ref: live_shadow_operational_proof.md Leg 2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03_p06_operator_token_gate_is_fail_closed() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::MissingTokenFailClosed,
    ));
    let router = routes::build_router(st);

    // Each of these operator routes must be refused with the same gate.
    let operator_routes: &[(&str, &str)] = &[
        ("POST", "/v1/integrity/arm"),
        ("POST", "/v1/run/start"),
        ("POST", "/v1/run/halt"),
        ("POST", "/v1/integrity/disarm"),
    ];

    for (method, uri) in operator_routes {
        let req = Request::builder()
            .method(*method)
            .uri(*uri)
            .body(axum::body::Body::empty())
            .unwrap();
        let (status, body) = call(router.clone(), req).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "P-06: {method} {uri} must be 503 with missing token"
        );
        let json = parse_json(body);
        assert_eq!(
            json["gate"], "operator_auth_config",
            "P-06: {method} {uri} gate must be operator_auth_config; got: {json}"
        );
    }

    // Read-only routes must still be available.
    let health_req = Request::builder()
        .method("GET")
        .uri("/v1/health")
        .body(axum::body::Body::empty())
        .unwrap();
    let (health_status, _) = call(router, health_req).await;
    assert_eq!(
        health_status,
        StatusCode::OK,
        "P-06: health must remain available even when operator token is missing"
    );
}
