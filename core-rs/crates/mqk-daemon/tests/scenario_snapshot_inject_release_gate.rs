//! S7-3: Disable Snapshot Inject in Release Builds
//!
//! Proves that the dev-only snapshot inject/clear endpoint is gated at two
//! independent layers so it cannot be enabled accidentally in production:
//!
//! 1. **Compile-time layer** — `cfg(not(debug_assertions))` makes
//!    `snapshot_inject_allowed_with_env` return `false` unconditionally in
//!    release builds, regardless of the env var.
//!
//! 2. **Runtime layer** — in debug builds, the env var must also be `"1"`
//!    or `"true"`.  An absent or falsy value keeps the gate closed.
//!
//! Five gate properties tested:
//!
//! 1. **Gate is closed when env var is absent** — `snapshot_inject_allowed_with_env(None)`
//!    returns `false`; the env var is required even in debug builds.
//!
//! 2. **Gate is closed when env var is `"0"`** — an explicit opt-out value
//!    (`"0"`, `"false"`, etc.) keeps the gate closed.
//!
//! 3. **Gate respects `cfg(debug_assertions)` for env-var `"1"`** —
//!    `snapshot_inject_allowed_with_env(Some("1"))` equals `cfg!(debug_assertions)`.
//!    In debug builds this is `true`; in release builds this is `false`.
//!    The single assertion is correct in both build profiles.
//!
//! 4. **HTTP snapshot POST returns 403 when env var absent** — the gate is
//!    wired to the real HTTP handler; a missing env var produces a `403`
//!    with `gate = "dev_snapshot_inject"`.
//!
//! 5. **HTTP snapshot DELETE returns 403 when env var absent** — the same
//!    gate covers the clear endpoint.

use mqk_daemon::dev_gate::snapshot_inject_allowed_with_env;
use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt; // oneshot

// ---------------------------------------------------------------------------
// Helpers (shared with other scenario files — duplicated to keep tests self-contained)
// ---------------------------------------------------------------------------

fn make_router() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_token(None));
    routes::build_router(st)
}

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
// Gate property 1: closed when env var is absent
// ---------------------------------------------------------------------------

/// GATE 1 of 5.
///
/// `snapshot_inject_allowed_with_env(None)` must return `false`.
/// The env var is required even in debug builds — there is no implicit
/// default-open behaviour.
#[test]
fn gate_is_closed_when_env_var_is_absent() {
    assert!(
        !snapshot_inject_allowed_with_env(None),
        "snapshot inject must be disabled when env var is absent"
    );
}

// ---------------------------------------------------------------------------
// Gate property 2: closed when env var is "0"
// ---------------------------------------------------------------------------

/// GATE 2 of 5.
///
/// Explicit falsy values (`"0"`, `"false"`, `"no"`, empty string) must keep
/// the gate closed.  Only `"1"` or `"true"` open it (in debug builds).
#[test]
fn gate_is_closed_when_env_var_is_zero_or_false() {
    for falsy in &["0", "false", "False", "FALSE", "no", ""] {
        assert!(
            !snapshot_inject_allowed_with_env(Some(falsy)),
            "snapshot inject must be disabled for env var = {:?}",
            falsy
        );
    }
}

// ---------------------------------------------------------------------------
// Gate property 3: respects cfg(debug_assertions)
// ---------------------------------------------------------------------------

/// GATE 3 of 5.
///
/// `snapshot_inject_allowed_with_env(Some("1"))` must equal
/// `cfg!(debug_assertions)`.
///
/// - Debug builds (`cargo test`): `cfg!(debug_assertions)` is `true` →
///   the function returns `true`.
/// - Release builds (`cargo test --release`): `cfg!(debug_assertions)` is
///   `false` → the compile-time gate fires and the function returns `false`,
///   regardless of the env var value.
///
/// This single assertion is correct in **both** build profiles and proves
/// the compile-time gate is active.
#[test]
fn gate_with_env_one_equals_debug_assertions_flag() {
    let result = snapshot_inject_allowed_with_env(Some("1"));
    let expected = cfg!(debug_assertions);
    assert_eq!(
        result, expected,
        "snapshot_inject_allowed_with_env(Some(\"1\")) must equal \
         cfg!(debug_assertions) = {}; this assertion verifies the \
         compile-time gate is active in release builds",
        expected
    );
}

// ---------------------------------------------------------------------------
// Gate property 4: HTTP snapshot POST returns 403 when env var absent
// ---------------------------------------------------------------------------

/// GATE 4 of 5.
///
/// The HTTP gate is wired to `snapshot_inject_allowed()`.  A `POST` to
/// `/v1/trading/snapshot` without `MQK_DEV_ALLOW_SNAPSHOT_INJECT` set must
/// return `403 Forbidden` with `gate = "dev_snapshot_inject"`.
///
/// (Tests run without the env var set; even if it were set in CI the
/// compile-time gate ensures release builds remain closed.)
#[tokio::test]
async fn snapshot_post_returns_403_when_env_var_absent() {
    let router = make_router();

    // Build a minimal valid BrokerSnapshot JSON body.
    let body = serde_json::json!({
        "captured_at_utc": "2000-01-01T00:00:00Z",
        "account": { "equity": "100", "cash": "50", "currency": "USD" },
        "positions": [],
        "orders": [],
        "fills": []
    });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/trading/snapshot")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();

    let (status, body_bytes) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "snapshot POST must return 403 when env var absent"
    );

    let json = parse_json(body_bytes);
    assert_eq!(
        json["gate"], "dev_snapshot_inject",
        "403 body must name gate = \"dev_snapshot_inject\""
    );
}

// ---------------------------------------------------------------------------
// Gate property 5: HTTP snapshot DELETE returns 403 when env var absent
// ---------------------------------------------------------------------------

/// GATE 5 of 5.
///
/// The same gate covers the clear endpoint.  A `DELETE` to
/// `/v1/trading/snapshot` without the env var must return `403 Forbidden`
/// with `gate = "dev_snapshot_inject"`.
#[tokio::test]
async fn snapshot_delete_returns_403_when_env_var_absent() {
    let router = make_router();

    let req = Request::builder()
        .method("DELETE")
        .uri("/v1/trading/snapshot")
        .body(axum::body::Body::empty())
        .unwrap();

    let (status, body_bytes) = call(router, req).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "snapshot DELETE must return 403 when env var absent"
    );

    let json = parse_json(body_bytes);
    assert_eq!(
        json["gate"], "dev_snapshot_inject",
        "403 body must name gate = \"dev_snapshot_inject\""
    );
}
