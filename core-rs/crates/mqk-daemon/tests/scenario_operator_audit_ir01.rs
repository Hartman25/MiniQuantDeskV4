//! IR-01: Control operator-audit durable-truth closure — scenario proof.
//!
//! ## What IR-01 claims
//!
//! No synthetic run row is ever created to anchor an operator-audit event when no
//! real run exists.  `audit_event_id` in the arm/disarm response is null when
//! there is no real run to anchor it to — honest absence, not a fabricated UUID.
//!
//! ## Proof structure
//!
//! ### Pure in-process tests (P-series — no MQK_DATABASE_URL required)
//!
//! These tests pass in CI without a database:
//!
//! | Test | Claim |
//! |------|-------|
//! | P1 | `arm-execution` without DB → 200, `audit_event_id=null`, `durable_db_write=false` |
//! | P2 | `disarm-execution` without DB → 200, `audit_event_id=null`, `durable_db_write=false` |
//! | P3 | `/control/arm` without DB → 503 (primary arm route is fail-closed on DB) |
//!
//! P1 and P2 prove the canonical GUI arm/disarm path (ops/action) surfaces honest
//! null audit truth when no DB is configured.  P3 proves the legacy direct route
//! enforces a hard DB dependency — no arm possible → no run anchor possible.
//!
//! ### DB-backed proofs (no MQK_DATABASE_URL — run with `--include-ignored`)
//!
//! The authoritative IR-01 DB-backed proofs live in `scenario_daemon_runtime_lifecycle`:
//!
//! | Test name | Claim |
//! |-----------|-------|
//! | `ir01_control_arm_no_run_no_synthetic_run_created` | arm with DB but no real run: `audit_event_id=null`, zero `runs` rows |
//! | `ir01_control_disarm_no_run_no_synthetic_run_created` | same for disarm |
//! | `ir01_control_arm_with_real_run_writes_audit_event` | arm with a real run: non-null `audit_event_id`, one `audit_events` row |
//!
//! These three tests prove the run-anchor resolution logic: when no real run exists,
//! `write_control_operator_audit_event` returns `Ok(None)` (no DB write, no synthetic
//! row), and the response contract surfaces `audit_event_id=null`.
//!
//! ## CI command
//!
//! ```sh
//! # Pure tests (no DB required):
//! cargo test -p mqk-daemon --test scenario_operator_audit_ir01
//!
//! # Full IR-01 closure (DB required):
//! MQK_DATABASE_URL=postgres://... \
//!   cargo test -p mqk-daemon --test scenario_daemon_runtime_lifecycle \
//!     ir01 -- --include-ignored
//! ```

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;

fn make_no_db_router() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st)
}

async fn post_json(router: axum::Router, uri: &str, body: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("body collect failed")
        .to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn post_empty(router: axum::Router, uri: &str) -> StatusCode {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    resp.into_body()
        .collect()
        .await
        .expect("body collect failed");
    status
}

/// IR-01-P1: arm-execution without DB returns 200 with null audit_event_id.
///
/// Proves the canonical GUI arm path (POST /api/v1/ops/action with
/// action_key="arm-execution") surfaces honest null audit truth when no DB is
/// configured:
///
///   - accepted = true  (arm succeeded in-memory)
///   - audit.durable_db_write = false  (no DB → no durable write)
///   - audit.audit_event_id = null  (no run anchor → no audit row created)
///
/// This is the IR-01 invariant on the in-memory path: honest absence, not a
/// fabricated UUID standing in for a real audit record.
#[tokio::test]
async fn ir01_p1_arm_execution_without_db_has_null_audit_event_id() {
    let router = make_no_db_router();
    let (status, json) = post_json(
        router,
        "/api/v1/ops/action",
        r#"{"action_key":"arm-execution"}"#,
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "IR-01-P1: arm-execution must return 200 even without DB; got {status}: {json}"
    );

    assert_eq!(
        json["accepted"],
        serde_json::json!(true),
        "IR-01-P1: accepted must be true; got: {json}"
    );

    assert!(
        json["audit"]["audit_event_id"].is_null(),
        "IR-01-P1: audit_event_id must be null when no DB is configured — \
         a non-null value here would indicate a fabricated run anchor; got: {json}"
    );

    assert_eq!(
        json["audit"]["durable_db_write"],
        serde_json::json!(false),
        "IR-01-P1: durable_db_write must be false when no DB is configured; got: {json}"
    );
}

/// IR-01-P2: disarm-execution without DB returns 200 with null audit_event_id.
///
/// Same IR-01 invariant as P1 for the disarm path: honest null audit_event_id
/// when no real run anchor exists, not a fabricated UUID.
#[tokio::test]
async fn ir01_p2_disarm_execution_without_db_has_null_audit_event_id() {
    let router = make_no_db_router();
    let (status, json) = post_json(
        router,
        "/api/v1/ops/action",
        r#"{"action_key":"disarm-execution"}"#,
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "IR-01-P2: disarm-execution must return 200 even without DB; got {status}: {json}"
    );

    assert_eq!(
        json["accepted"],
        serde_json::json!(true),
        "IR-01-P2: accepted must be true; got: {json}"
    );

    assert!(
        json["audit"]["audit_event_id"].is_null(),
        "IR-01-P2: audit_event_id must be null when no DB is configured — \
         a non-null value here would indicate a fabricated run anchor; got: {json}"
    );

    assert_eq!(
        json["audit"]["durable_db_write"],
        serde_json::json!(false),
        "IR-01-P2: durable_db_write must be false when no DB is configured; got: {json}"
    );
}

/// IR-01-P3: POST /control/arm without DB is fail-closed (returns 503).
///
/// Proves the legacy direct arm route enforces a hard DB dependency.  Because
/// this route returns 503 without a DB, there is no code path through which a
/// synthetic run row could be created — the IR-01 "no synthetic run" invariant
/// is structurally enforced at the fail-closed boundary before any DB write
/// could occur.
#[tokio::test]
async fn ir01_p3_control_arm_without_db_is_fail_closed() {
    let router = make_no_db_router();
    let status = post_empty(router, "/control/arm").await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "IR-01-P3: /control/arm without DB must return 503 (fail-closed); \
         any 2xx here would open a path to synthetic run creation"
    );
}
