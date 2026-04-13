//! CTRL-AUTH-01 — Control-plane authority consistency proof.
//!
//! Proves that the control-plane authority model is consistent across all
//! control entry points: both canonical and legacy operator surfaces, kill-switch
//! semantics, auth fail-closed posture, and transition consistency.
//!
//! ## What is being proven
//!
//! The daemon exposes three arm/disarm surfaces:
//!
//! | Path                                        | Requires DB | Writes `desired_armed` |
//! |---------------------------------------------|-------------|------------------------|
//! | `POST /v1/integrity/arm`   (canonical-1)    | No          | No                     |
//! | `POST /api/v1/ops/action {arm-execution}`   | No          | No                     |
//! | `POST /control/arm`        (legacy)         | Yes         | Yes                    |
//!
//! All three paths are on the auth-protected operator router surface.  The
//! legacy `/control/arm` has a stricter DB requirement — a bounded divergence
//! that does not affect execution-safety state (both `sys_arm_state` and the
//! in-memory `integrity` fields are updated via every arm path when a DB is
//! present, and in-memory-only via the canonical paths without DB).
//!
//! The `desired_armed` column in `runtime_control_state` is surfaced only by
//! `/control/status` for operator display; it is NOT read by any execution gate.
//! Execution gating reads `sys_arm_state` (DB) and `IntegrityState::is_execution_blocked()`
//! (in-memory).  The `desired_armed` divergence is therefore display-only and
//! explicitly bounded here.
//!
//! ## Proof matrix
//!
//! | Test  | Claim                                                                       |
//! |-------|-----------------------------------------------------------------------------|
//! | CA-01 | Canonical arm (ops/action arm-execution) → 200 ARMED, accepted=true        |
//! | CA-02 | Legacy /control/arm requires DB; returns 503 without DB (fail-closed)      |
//! | CA-03 | After canonical arm, start advances past integrity gate to DB gate (503)    |
//! | CA-04 | Canonical disarm-execution → 200 DISARMED, accepted=true                   |
//! | CA-05 | After canonical disarm, start blocks at integrity gate (403)                |
//! | CB-01 | Forced halted state blocks /v1/run/start at integrity gate                  |
//! | CB-02 | Forced halted state blocks ops/action start-system at integrity gate        |
//! | CB-03 | Canonical arm (ops/action) clears halted; start advances past integrity gate |
//! | CB-04 | /v1/integrity/arm (canonical form 2) also clears halted state              |
//! | CB-05 | Disarm-only does NOT clear halted; start remains blocked at integrity gate  |
//! | CB-06 | ops/action kill-switch requires DB (fail-closed without DB)                 |
//! | CC-01 | MissingTokenFailClosed blocks /control/arm (legacy path)                    |
//! | CC-02 | MissingTokenFailClosed blocks /api/v1/ops/action (canonical)                |
//! | CC-03 | Wrong token blocks /control/arm                                             |
//! | CC-04 | Wrong token blocks /api/v1/ops/action                                       |
//! | CC-05 | No Authorization header with TokenRequired blocks /control/disarm            |
//! | CC-06 | No Authorization header with TokenRequired blocks /v1/integrity/disarm      |
//! | CD-01 | Canonical arm is idempotent: arm twice → start passes integrity gate        |
//! | CD-02 | Halt → canonical arm cycle produces correct execution-gate state            |
//! | CD-03 | Repeated disarm does not affect halted flag; start blocked after both        |
//! | CE-01 | /control/restart is not registered → 404 (no stale restart path)           |
//! | CE-02 | ops/action unknown action_key → 400 (not silent bypass)                     |
//! | CE-03 | /control/arm returns 503 without DB; execution-safety not affected          |
//!
//! All tests are pure in-process (no DB or network required).

use std::sync::Arc;

use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Shared helpers
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

/// LiveShadow+Alpaca with ExplicitDevNoToken: deployment gate passes,
/// WS continuity gate not applicable, integrity gate is the next blocker.
fn live_shadow_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ))
}

/// Directly set halted+disarmed on the shared integrity state.
async fn force_halted(st: &Arc<state::AppState>) {
    let mut ig = st.integrity.write().await;
    ig.disarmed = true;
    ig.halted = true;
}

/// POST /api/v1/ops/action with a JSON action body.
fn ops_action_req(action_key: &str) -> Request<axum::body::Body> {
    let body = serde_json::json!({ "action_key": action_key }).to_string();
    Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap()
}

/// POST to a URI with an empty body (operator control routes).
fn post_req(uri: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap()
}

/// POST to a URI with a Bearer token.
fn post_req_authed(uri: &str, token: &str) -> Request<axum::body::Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(axum::body::Body::empty())
        .unwrap()
}

/// POST /api/v1/ops/action with a JSON action body and a Bearer token.
fn ops_action_req_authed(action_key: &str, token: &str) -> Request<axum::body::Body> {
    let body = serde_json::json!({ "action_key": action_key }).to_string();
    Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(axum::body::Body::from(body))
        .unwrap()
}

// ---------------------------------------------------------------------------
// A: Canonical control action path is authoritative
// ---------------------------------------------------------------------------

/// CA-01: Canonical arm via ops/action arm-execution succeeds and reports ARMED.
#[tokio::test]
async fn ca01_canonical_arm_execution_reports_armed() {
    let st = live_shadow_state();

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("arm-execution"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::OK,
        "ops/action arm-execution must return 200: {json}"
    );
    assert_eq!(
        json["accepted"], true,
        "arm-execution must be accepted: {json}"
    );
    assert_eq!(
        json["resulting_integrity_state"], "ARMED",
        "arm-execution must report ARMED: {json}"
    );
    assert_eq!(
        json["disposition"], "applied",
        "arm-execution must be applied: {json}"
    );
}

/// CA-02: Legacy /control/arm requires DB; returns 503 without DB.
///
/// This is a bounded divergence: the legacy path writes `runtime_control_state.desired_armed`
/// which requires a DB connection.  The canonical arm paths work without DB.
/// Without DB the legacy arm is fail-closed (503) — it does NOT silently succeed.
#[tokio::test]
async fn ca02_legacy_control_arm_requires_db_fails_closed_without_db() {
    let st = live_shadow_state();

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/control/arm"),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "legacy /control/arm must fail-closed (503) without DB: {}",
        parse_json(body)
    );
}

/// CA-03: After canonical arm, start advances past integrity gate and hits the DB gate.
///
/// Proves: canonical arm-execution produces execution-safety state change
/// (integrity.disarmed=false, integrity.halted=false) that the start gate
/// recognises as armed.  Without DB, start hits the DB requirement (503)
/// instead of the integrity gate (403).
#[tokio::test]
async fn ca03_canonical_arm_advances_start_past_integrity_gate() {
    let st = live_shadow_state();

    // Step 1: arm via canonical path.
    let (arm_status, _) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("arm-execution"),
    )
    .await;
    assert_eq!(arm_status, StatusCode::OK, "arm must succeed first");

    // Step 2: start — must pass integrity gate, fail at DB gate.
    let (start_status, start_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/v1/run/start"),
    )
    .await;
    let json = parse_json(start_body);

    assert_eq!(
        start_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "after canonical arm, start must reach DB gate (503), not integrity gate: {json}"
    );
    let error_str = json["error"].as_str().unwrap_or("");
    assert!(
        error_str.contains("runtime DB is not configured")
            || error_str.contains("DB")
            || json["fault_class"].as_str().unwrap_or("").contains("db")
            || json["gate"].as_str().unwrap_or("") == "db",
        "start blocker after canonical arm must be DB-related, not integrity: {json}"
    );
}

/// CA-04: Canonical disarm-execution returns DISARMED.
#[tokio::test]
async fn ca04_canonical_disarm_execution_reports_disarmed() {
    let st = live_shadow_state();

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("disarm-execution"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::OK,
        "ops/action disarm-execution must return 200: {json}"
    );
    assert_eq!(
        json["accepted"], true,
        "disarm-execution must be accepted: {json}"
    );
    assert_eq!(
        json["resulting_integrity_state"], "DISARMED",
        "disarm-execution must report DISARMED: {json}"
    );
}

/// CA-05: After canonical disarm, start blocks at integrity gate (403).
#[tokio::test]
async fn ca05_canonical_disarm_then_start_blocked_at_integrity_gate() {
    let st = live_shadow_state();

    // Step 1: arm first (so we can meaningfully disarm).
    let (arm_status, _) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("arm-execution"),
    )
    .await;
    assert_eq!(arm_status, StatusCode::OK);

    // Step 2: disarm.
    let (disarm_status, _) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("disarm-execution"),
    )
    .await;
    assert_eq!(disarm_status, StatusCode::OK);

    // Step 3: start must be blocked at integrity gate.
    let (start_status, start_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/v1/run/start"),
    )
    .await;
    let json = parse_json(start_body);

    assert_eq!(
        start_status,
        StatusCode::FORBIDDEN,
        "after canonical disarm, start must block at integrity gate: {json}"
    );
    assert_eq!(
        json["gate"], "integrity_armed",
        "integrity gate must be the named blocker: {json}"
    );
}

// ---------------------------------------------------------------------------
// B: Kill-switch semantics dominate controlled execution paths
// ---------------------------------------------------------------------------

/// CB-01: Forced halted state blocks /v1/run/start at integrity gate.
#[tokio::test]
async fn cb01_forced_halt_blocks_run_start() {
    let st = live_shadow_state();
    force_halted(&st).await;

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/v1/run/start"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "halted state must block /v1/run/start: {json}"
    );
    assert_eq!(
        json["gate"], "integrity_armed",
        "integrity gate must be named blocker: {json}"
    );
}

/// CB-02: Forced halted state blocks ops/action start-system at the same gate.
///
/// Proves that the kill-switch dominates BOTH the direct start route and the
/// ops/action canonical start surface — no divergence between entry points.
#[tokio::test]
async fn cb02_forced_halt_blocks_ops_action_start_system() {
    let st = live_shadow_state();
    force_halted(&st).await;

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("start-system"),
    )
    .await;

    // ops/action start-system calls halt_execution_runtime which in turn calls
    // start_execution_runtime.  The integrity gate fires before any DB access.
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "halted state must block ops/action start-system: {}",
        parse_json(body)
    );
}

/// CB-03: Canonical arm (ops/action arm-execution) clears halted state.
///
/// Proves: once arm is applied, the kill-switch is explicitly re-opened by
/// operator intent and start can proceed past the integrity gate.
#[tokio::test]
async fn cb03_canonical_arm_clears_halted_state() {
    let st = live_shadow_state();
    force_halted(&st).await;

    // Verify halt is active.
    let (blocked_status, _) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/v1/run/start"),
    )
    .await;
    assert_eq!(
        blocked_status,
        StatusCode::FORBIDDEN,
        "start must be blocked before arm"
    );

    // Arm via canonical path.
    let (arm_status, _) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("arm-execution"),
    )
    .await;
    assert_eq!(arm_status, StatusCode::OK, "canonical arm must succeed");

    // Start must now pass the integrity gate (hits DB gate next).
    let (start_status, start_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/v1/run/start"),
    )
    .await;
    let json = parse_json(start_body);

    assert_ne!(
        start_status,
        StatusCode::FORBIDDEN,
        "after canonical arm, start must no longer block at integrity gate: {json}"
    );
    assert_eq!(
        start_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "start must proceed past integrity gate to DB gate: {json}"
    );
}

/// CB-04: /v1/integrity/arm (canonical form 2) also clears halted state.
///
/// Proves that the two canonical arm forms are equivalent in kill-switch semantics.
#[tokio::test]
async fn cb04_integrity_arm_route_clears_halted_state() {
    let st = live_shadow_state();
    force_halted(&st).await;

    // Arm via /v1/integrity/arm (different canonical path, same semantics).
    let (arm_status, arm_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/v1/integrity/arm"),
    )
    .await;
    assert_eq!(
        arm_status,
        StatusCode::OK,
        "integrity arm must succeed: {}",
        parse_json(arm_body)
    );

    // Start must pass integrity gate.
    let (start_status, start_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/v1/run/start"),
    )
    .await;
    let json = parse_json(start_body);

    assert_ne!(
        start_status,
        StatusCode::FORBIDDEN,
        "/v1/integrity/arm must clear halt and allow start to proceed past integrity gate: {json}"
    );
}

/// CB-05: Disarm alone does NOT clear halted flag; start remains blocked.
///
/// Proves: disarm only sets `integrity.disarmed = true`.  When halted=true and
/// disarmed is already true, the execution gate still fires (both flags block).
/// An operator cannot escape a halt by disarming — they must explicitly arm.
#[tokio::test]
async fn cb05_disarm_alone_does_not_clear_halted() {
    let st = live_shadow_state();
    force_halted(&st).await;

    // Disarm (which only writes disarmed=true; halted stays true).
    let (disarm_status, _) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("disarm-execution"),
    )
    .await;
    assert_eq!(disarm_status, StatusCode::OK, "disarm must succeed");

    // Start must still be blocked — halted=true dominates even after disarm.
    let (start_status, start_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/v1/run/start"),
    )
    .await;
    let json = parse_json(start_body);

    assert_eq!(
        start_status,
        StatusCode::FORBIDDEN,
        "halted state must persist after disarm; start must remain blocked: {json}"
    );
    assert_eq!(
        json["gate"], "integrity_armed",
        "integrity gate must still be the named blocker after disarm-only: {json}"
    );
}

/// CB-06: ops/action kill-switch requires DB; fails closed without DB.
///
/// Proves: the kill-switch is not silently accepted without durable state.
/// This matches the existing /v1/run/halt behaviour (both call halt_execution_runtime).
#[tokio::test]
async fn cb06_kill_switch_requires_db_fails_closed() {
    let st = live_shadow_state();

    // Arm first so we can observe the halt-gate, not the integrity gate.
    let (arm_status, _) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("arm-execution"),
    )
    .await;
    assert_eq!(arm_status, StatusCode::OK);

    let (status, _) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("kill-switch"),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "ops/action kill-switch must fail closed (503) without DB"
    );
}

// ---------------------------------------------------------------------------
// C: Auth and authority fail closed consistently
// ---------------------------------------------------------------------------

/// CC-01: MissingTokenFailClosed blocks the legacy /control/arm path.
///
/// Proves: the legacy arm path is on the auth-protected operator surface and
/// cannot be used to bypass the auth model.
#[tokio::test]
async fn cc01_missing_token_blocks_legacy_control_arm() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::MissingTokenFailClosed,
    ));

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/control/arm"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "MissingTokenFailClosed must block /control/arm: {json}"
    );
    assert_eq!(
        json["gate"], "operator_auth_config",
        "auth config gate must be named: {json}"
    );
}

/// CC-02: MissingTokenFailClosed blocks the canonical /api/v1/ops/action path.
#[tokio::test]
async fn cc02_missing_token_blocks_canonical_ops_action() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::MissingTokenFailClosed,
    ));

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("arm-execution"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "MissingTokenFailClosed must block ops/action: {json}"
    );
    assert_eq!(
        json["gate"], "operator_auth_config",
        "auth config gate must be named: {json}"
    );
}

/// CC-03: Wrong Bearer token blocks the legacy /control/arm path.
#[tokio::test]
async fn cc03_wrong_token_blocks_legacy_control_arm() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::TokenRequired("correct-token".to_string()),
    ));

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req_authed("/control/arm", "wrong-token"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "wrong token must be rejected on /control/arm: {json}"
    );
    assert_eq!(
        json["gate"], "operator_token",
        "token gate must be named: {json}"
    );
}

/// CC-04: Wrong Bearer token blocks the canonical /api/v1/ops/action path.
#[tokio::test]
async fn cc04_wrong_token_blocks_canonical_ops_action() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::TokenRequired("correct-token".to_string()),
    ));

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req_authed("arm-execution", "bad-token"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "wrong token must be rejected on ops/action: {json}"
    );
    assert_eq!(
        json["gate"], "operator_token",
        "token gate must be named: {json}"
    );
}

/// CC-05: No Authorization header with TokenRequired blocks /control/disarm.
///
/// Proves: the legacy disarm path is auth-gated consistently with arm.
/// A disarm without a token cannot remove execution safety from outside the
/// authenticated operator surface.
#[tokio::test]
async fn cc05_no_auth_header_blocks_legacy_control_disarm() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::TokenRequired("my-token".to_string()),
    ));

    // No Authorization header — should be rejected.
    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/control/disarm"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "no auth header must be rejected on /control/disarm: {json}"
    );
    assert_eq!(
        json["gate"], "operator_token",
        "token gate must be named: {json}"
    );
}

/// CC-06: No Authorization header with TokenRequired blocks /v1/integrity/disarm.
///
/// Proves: canonical disarm is also auth-gated; cannot be called without a token.
#[tokio::test]
async fn cc06_no_auth_header_blocks_canonical_integrity_disarm() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::TokenRequired("my-token".to_string()),
    ));

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/v1/integrity/disarm"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "no auth header must be rejected on /v1/integrity/disarm: {json}"
    );
    assert_eq!(
        json["gate"], "operator_token",
        "token gate must be named: {json}"
    );
}

// ---------------------------------------------------------------------------
// D: Conflicting control state cannot bypass authority model
// ---------------------------------------------------------------------------

/// CD-01: Canonical arm is idempotent — arming twice does not corrupt state.
///
/// Proves: a second arm call does not flip back to disarmed or produce a
/// different execution-gate outcome.
#[tokio::test]
async fn cd01_canonical_arm_is_idempotent() {
    let st = live_shadow_state();

    // Arm once.
    let (arm1_status, _) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("arm-execution"),
    )
    .await;
    assert_eq!(arm1_status, StatusCode::OK, "first arm must succeed");

    // Arm a second time.
    let (arm2_status, arm2_body) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("arm-execution"),
    )
    .await;
    assert_eq!(arm2_status, StatusCode::OK, "second arm must also succeed");
    let json = parse_json(arm2_body);
    assert_eq!(
        json["resulting_integrity_state"], "ARMED",
        "second arm must still report ARMED: {json}"
    );

    // Start must still pass integrity gate (hits DB gate).
    let (start_status, _) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/v1/run/start"),
    )
    .await;
    assert_eq!(
        start_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "after double-arm, start must remain past integrity gate at DB gate"
    );
}

/// CD-02: Halt → canonical arm cycle produces correct execution-gate state.
///
/// Proves: the authority model is consistent under a forced-halt followed by
/// explicit operator re-arm.  The re-arm is the ONLY valid path out of halt.
#[tokio::test]
async fn cd02_halt_then_canonical_arm_produces_correct_state() {
    let st = live_shadow_state();

    // Force halt.
    force_halted(&st).await;

    // Verify blocked.
    let (blocked_status, blocked_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/v1/run/start"),
    )
    .await;
    assert_eq!(
        blocked_status,
        StatusCode::FORBIDDEN,
        "must be blocked after halt: {}",
        parse_json(blocked_body)
    );

    // Re-arm.
    let (arm_status, _) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("arm-execution"),
    )
    .await;
    assert_eq!(arm_status, StatusCode::OK, "re-arm must succeed");

    // Verify start passes integrity gate.
    let (start_status, start_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/v1/run/start"),
    )
    .await;
    assert_ne!(
        start_status,
        StatusCode::FORBIDDEN,
        "halt→arm cycle must restore start capability past integrity gate: {}",
        parse_json(start_body)
    );
}

/// CD-03: Repeated disarm does not affect the halted flag.
///
/// Proves: disarming twice after a forced halt does not clear halt via any
/// side effect; the execution gate remains fail-closed.
#[tokio::test]
async fn cd03_repeated_disarm_cannot_escape_halted_state() {
    let st = live_shadow_state();
    force_halted(&st).await;

    // Disarm twice.
    for _ in 0..2 {
        let (ds, _) = call(
            routes::build_router(Arc::clone(&st)),
            ops_action_req("disarm-execution"),
        )
        .await;
        assert_eq!(ds, StatusCode::OK, "disarm must succeed");
    }

    // Start must still be blocked by halted state.
    let (start_status, start_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/v1/run/start"),
    )
    .await;
    let json = parse_json(start_body);

    assert_eq!(
        start_status,
        StatusCode::FORBIDDEN,
        "repeated disarm must not escape halted state: {json}"
    );
    assert_eq!(
        json["gate"], "integrity_armed",
        "integrity gate must still be active: {json}"
    );
}

// ---------------------------------------------------------------------------
// E: No legacy fallback for control surface
// ---------------------------------------------------------------------------

/// CE-01: /control/restart is not registered — returns 404.
///
/// Proves: the stale restart route is not reachable.  An operator cannot trigger
/// a legacy restart behind the canonical ops/action surface.
/// (The handler exists in source with `#[allow(dead_code)]` and always returns
/// 503 restart_not_authoritative, but it is intentionally not mounted.)
#[tokio::test]
async fn ce01_control_restart_route_is_not_registered() {
    let st = live_shadow_state();

    let (status, _) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/control/restart"),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "/control/restart must return 404 (route not registered)"
    );
}

/// CE-02: ops/action with an unknown action_key returns 400, not a silent bypass.
///
/// Proves: the canonical control surface explicitly rejects unknown actions.
/// There is no fallthrough path that silently succeeds.
#[tokio::test]
async fn ce02_ops_action_unknown_key_returns_400() {
    let st = live_shadow_state();

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("not-a-real-action"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unknown ops/action action_key must return 400: {json}"
    );
    assert_eq!(
        json["disposition"], "unknown_action",
        "unknown action must be named explicitly: {json}"
    );
    assert_eq!(
        json["accepted"], false,
        "unknown action must not be accepted: {json}"
    );
}

/// CE-03: The legacy /control/arm path failing closed (503) without DB means
/// no stale arm can succeed via the legacy surface in a no-DB daemon.
///
/// Proves the bounded gap is fail-closed, not open: the legacy path never
/// yields a synthetic arm success when the DB requirement is not met.
#[tokio::test]
async fn ce03_legacy_control_arm_fail_closed_is_not_a_bypass() {
    let st = live_shadow_state();

    // Legacy arm fails (no DB).
    let (legacy_arm_status, legacy_arm_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/control/arm"),
    )
    .await;
    assert_eq!(
        legacy_arm_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "legacy arm must fail closed: {}",
        parse_json(legacy_arm_body)
    );

    // Start must still block at integrity gate — legacy arm did NOT arm the system.
    let (start_status, start_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_req("/v1/run/start"),
    )
    .await;
    let json = parse_json(start_body);

    assert_eq!(
        start_status,
        StatusCode::FORBIDDEN,
        "start must block at integrity gate after failed legacy arm: {json}"
    );
    assert_eq!(
        json["gate"], "integrity_armed",
        "integrity gate must be the blocker (legacy arm did not arm): {json}"
    );
}
