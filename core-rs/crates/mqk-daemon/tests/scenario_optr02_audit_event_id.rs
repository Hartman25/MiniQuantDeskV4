//! OPTR-02 — Control-action response contract: audit_event_id surfacing.
//!
//! Proves that `start-system`, `stop-system`, and `kill-switch` in
//! `POST /api/v1/ops/action` surface `audit.audit_event_id` honestly:
//! null when no audit event was written (no run anchor or no DB), not as a
//! hardcoded lie when an event was actually persisted.
//!
//! ## What is being proven
//!
//! Before OPTR-02 all three actions hardcoded `audit_event_id: None` in the
//! response body even when `write_operator_audit_event` had written a durable
//! row.  The fix changes `None` to `audit_uuid.map(|id| id.to_string())`.
//!
//! These tests prove the null-honest path (no DB or no run anchor → null is
//! correct, not a lie) and that fail-closed behaviour is unchanged.
//!
//! ## Proof matrix
//!
//! | Test  | Claim                                                                        |
//! |-------|------------------------------------------------------------------------------|
//! | P-01  | start-system without DB → 503 (fail-closed; 200 path unreachable)           |
//! | P-02  | stop-system without DB → 200, audit.audit_event_id=null (honest: no write)  |
//! | P-03  | kill-switch without DB → 503 (fail-closed per CB-06; 200 path unreachable)  |
//!
//! All tests are pure in-process (no DB required).
//!
//! ## Explicit open
//!
//! The positive case — audit_event_id non-null when a durable write actually
//! occurred — requires a daemon with an active execution loop (live run anchor).
//! That path is not exercised here because it requires full lifecycle setup
//! beyond the scope of a patch-local proof.  The code change is inspectable:
//! `audit_uuid.map(|id| id.to_string())` propagates a non-None UUID directly.

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

fn ops_action_req(action_key: &str) -> Request<axum::body::Body> {
    let body = serde_json::json!({ "action_key": action_key }).to_string();
    Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap()
}

/// LiveShadow+Alpaca, no DB.  Deployment gate passes; integrity gate is the
/// next blocker for start.  kill-switch requires DB so it hits the DB gate.
fn no_db_state() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ))
}

// ---------------------------------------------------------------------------
// P-01: start-system without DB → 503 (fail-closed; 200 path unreachable)
// ---------------------------------------------------------------------------

/// OPTR-02 / P-01: `start-system` without DB returns 503 (db_pool gate).
///
/// Proves:
/// - Fail-closed behaviour is unchanged by the OPTR-02 fix
/// - The 200 path — where audit_event_id is now surfaced — is not reachable
///   without a DB, so no regression is introduced on this path
#[tokio::test]
async fn optr02_p1_start_system_no_db_returns_503() {
    let st = no_db_state();

    // Arm so the integrity gate passes and start reaches the DB gate.
    let (arm_status, _) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("arm-execution"),
    )
    .await;
    assert_eq!(arm_status, StatusCode::OK, "P-01: arm must succeed first");

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("start-system"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "P-01: start-system without DB must return 503 (fail-closed); body: {json}"
    );
    assert!(
        json["error"].is_string(),
        "P-01: 503 response must carry RuntimeErrorResponse error field; body: {json}"
    );
    assert!(
        json["fault_class"].is_string(),
        "P-01: 503 response must carry fault_class field; body: {json}"
    );
}

// ---------------------------------------------------------------------------
// P-02: stop-system without DB → 200, audit.audit_event_id=null (honest null)
// ---------------------------------------------------------------------------

/// OPTR-02 / P-02: `stop-system` without DB returns 200 with
/// `audit.audit_event_id=null`.
///
/// Proves:
/// - null is honest: no DB means no write, so null is the correct value
/// - `audit.durable_db_write` is false (no DB, no durable write)
/// - Response shape is `OperatorActionResponse` with an `audit` sub-object
///   containing `audit_event_id` (field is present even when null)
/// - `requested_action` echoes the submitted key
///
/// stop-system can succeed without a DB when there is no active local run
/// (stop_execution_runtime returns current_status_snapshot directly in that case).
#[tokio::test]
async fn optr02_p2_stop_system_no_db_audit_event_id_is_honest_null() {
    let st = no_db_state();

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("stop-system"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::OK,
        "P-02: stop-system with no active run must return 200; body: {json}"
    );
    assert_eq!(
        json["requested_action"].as_str().unwrap_or(""),
        "stop-system",
        "P-02: requested_action must echo stop-system; body: {json}"
    );
    assert_eq!(
        json["accepted"].as_bool().unwrap_or(false),
        true,
        "P-02: accepted must be true; body: {json}"
    );
    assert!(
        json["audit"].is_object(),
        "P-02: response must have audit sub-object; body: {json}"
    );
    // audit_event_id must be null: no DB → no write → honest null
    assert!(
        json["audit"]["audit_event_id"].is_null(),
        "P-02: audit_event_id must be null when no DB (honest null, not a hardcoded lie); \
         body: {json}"
    );
    // durable_db_write must be false: no DB present
    assert_eq!(
        json["audit"]["durable_db_write"].as_bool().unwrap_or(true),
        false,
        "P-02: durable_db_write must be false when no DB; body: {json}"
    );
}

// ---------------------------------------------------------------------------
// P-03: kill-switch without DB → 503 (fail-closed per CB-06)
// ---------------------------------------------------------------------------

/// OPTR-02 / P-03: `kill-switch` without DB returns 503 (fail-closed).
///
/// Proves:
/// - CB-06 fail-closed semantics are unchanged by the OPTR-02 fix
/// - The 200 path — where audit_event_id is now surfaced — is unreachable
///   without a DB (halt_execution_runtime calls db_pool() which fails closed)
#[tokio::test]
async fn optr02_p3_kill_switch_no_db_returns_503() {
    let st = no_db_state();

    // Arm so the integrity gate passes and kill-switch reaches the DB gate.
    let (arm_status, _) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("arm-execution"),
    )
    .await;
    assert_eq!(arm_status, StatusCode::OK, "P-03: arm must succeed first");

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        ops_action_req("kill-switch"),
    )
    .await;
    let json = parse_json(body);

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "P-03: kill-switch without DB must return 503 (fail-closed per CB-06); body: {json}"
    );
    assert!(
        json["error"].is_string(),
        "P-03: 503 response must carry RuntimeErrorResponse error field; body: {json}"
    );
    assert!(
        json["fault_class"].is_string(),
        "P-03: 503 response must carry fault_class field; body: {json}"
    );
}
