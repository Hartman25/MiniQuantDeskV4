//! CC-04: OMS overview canonical surface proof.
//!
//! Proves that `GET /api/v1/oms/overview` returns a single composed view of
//! current trading state from mounted in-memory truth, with explicit
//! truth_state semantics for each lane.
//!
//! All tests are in-process and always runnable in CI without environment
//! variables.

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

fn overview_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/oms/overview")
        .body(axum::body::Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// CC04-01 — Overview returns 200 with required fields
//
// Proves: the route exists, returns 200, and all structural fields are present.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cc04_01_overview_returns_200_with_required_fields() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, overview_req()).await;
    assert_eq!(status, StatusCode::OK, "CC04-01: must return 200");

    let json = parse_json(body);

    // canonical_route must be present and correct.
    assert_eq!(
        json["canonical_route"],
        "/api/v1/oms/overview",
        "CC04-01: canonical_route must be /api/v1/oms/overview"
    );

    // Runtime lane fields must be present.
    assert!(
        json["runtime_status"].is_string(),
        "CC04-01: runtime_status must be a string"
    );
    assert!(
        json["integrity_armed"].is_boolean(),
        "CC04-01: integrity_armed must be a bool"
    );
    assert!(
        json["kill_switch_active"].is_boolean(),
        "CC04-01: kill_switch_active must be a bool"
    );
    assert!(
        json["daemon_mode"].is_string(),
        "CC04-01: daemon_mode must be a string"
    );
    assert!(
        json["fault_signal_count"].is_number(),
        "CC04-01: fault_signal_count must be a number"
    );

    // Account lane.
    assert!(
        json["account_snapshot_state"].is_string(),
        "CC04-01: account_snapshot_state must be a string"
    );

    // Portfolio lane.
    assert!(
        json["portfolio_snapshot_state"].is_string(),
        "CC04-01: portfolio_snapshot_state must be a string"
    );
    assert!(
        json["position_count"].is_number(),
        "CC04-01: position_count must be a number"
    );
    assert!(
        json["open_order_count"].is_number(),
        "CC04-01: open_order_count must be a number"
    );
    assert!(
        json["fill_count"].is_number(),
        "CC04-01: fill_count must be a number"
    );

    // Execution lane.
    assert!(
        json["execution_has_snapshot"].is_boolean(),
        "CC04-01: execution_has_snapshot must be a bool"
    );
    assert!(
        json["execution_active_orders"].is_number(),
        "CC04-01: execution_active_orders must be a number"
    );
    assert!(
        json["execution_pending_orders"].is_number(),
        "CC04-01: execution_pending_orders must be a number"
    );

    // Reconcile lane.
    assert!(
        json["reconcile_status"].is_string(),
        "CC04-01: reconcile_status must be a string"
    );
    assert!(
        json["reconcile_total_mismatches"].is_number(),
        "CC04-01: reconcile_total_mismatches must be a number"
    );
}

// ---------------------------------------------------------------------------
// CC04-02 — Fresh daemon shows fail-closed lane states
//
// Proves: a freshly constructed daemon with no broker snapshot, no execution
// snapshot, and no run active reports the correct fail-closed states for each
// lane — not fake zeros that look like healthy empty truth.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cc04_02_fresh_daemon_shows_fail_closed_lanes() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, overview_req()).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);

    // Runtime: fresh daemon is idle and disarmed.
    assert_eq!(
        json["runtime_status"], "idle",
        "CC04-02: fresh daemon must be idle; got: {json}"
    );
    assert_eq!(
        json["integrity_armed"], false,
        "CC04-02: fresh daemon must be disarmed; got: {json}"
    );
    assert_eq!(
        json["kill_switch_active"], false,
        "CC04-02: fresh daemon must not have kill_switch active; got: {json}"
    );

    // Account + portfolio: no broker snapshot loaded → explicit no_snapshot.
    assert_eq!(
        json["account_snapshot_state"], "no_snapshot",
        "CC04-02: account lane must show no_snapshot before broker snapshot loaded; got: {json}"
    );
    assert_eq!(
        json["portfolio_snapshot_state"], "no_snapshot",
        "CC04-02: portfolio lane must show no_snapshot before broker snapshot loaded; got: {json}"
    );
    // Counts are zero when no snapshot, but the truth_state makes the absence explicit.
    assert_eq!(
        json["position_count"], 0,
        "CC04-02: position_count must be 0 with no_snapshot; got: {json}"
    );
    assert_eq!(
        json["open_order_count"], 0,
        "CC04-02: open_order_count must be 0 with no_snapshot; got: {json}"
    );
    assert_eq!(
        json["fill_count"], 0,
        "CC04-02: fill_count must be 0 with no_snapshot; got: {json}"
    );

    // Equity and cash are absent (not fabricated zeros) when no broker snapshot.
    assert!(
        json["account_equity"].is_null(),
        "CC04-02: account_equity must be null with no_snapshot; got: {json}"
    );
    assert!(
        json["account_cash"].is_null(),
        "CC04-02: account_cash must be null with no_snapshot; got: {json}"
    );
    assert!(
        json["portfolio_snapshot_at_utc"].is_null(),
        "CC04-02: portfolio_snapshot_at_utc must be null with no_snapshot; got: {json}"
    );

    // Execution: no snapshot because execution loop has never started.
    assert_eq!(
        json["execution_has_snapshot"], false,
        "CC04-02: execution_has_snapshot must be false before execution loop starts; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// CC04-03 — Overview is accessible without operator auth (read-only route)
//
// Proves: the overview route is on the public (unauthenticated) sub-router.
// An operator can call it without a Bearer token.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cc04_03_overview_accessible_without_auth() {
    // MissingTokenFailClosed means operator routes return 503.
    // The overview route must remain accessible as it is read-only.
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::MissingTokenFailClosed,
    ));
    let router = routes::build_router(st);

    let (status, body) = call(router, overview_req()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "CC04-03: overview must be accessible without auth (read-only route); got body: {}",
        String::from_utf8_lossy(&body)
    );

    let json = parse_json(body);
    assert_eq!(
        json["canonical_route"],
        "/api/v1/oms/overview",
        "CC04-03: canonical_route must be correct even without auth"
    );
}

// ---------------------------------------------------------------------------
// CC04-04 — Reconcile lane reflects unknown state before first reconcile tick
//
// Proves: before the reconcile loop has run, the reconcile_status is "unknown"
// (not "ok" or fabricated "clean").  The overview surface does not invent
// a clean reconcile state.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cc04_04_reconcile_lane_shows_unknown_before_first_tick() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, overview_req()).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);

    // Fresh daemon has never run reconcile — should be "unknown" not "ok".
    assert_eq!(
        json["reconcile_status"], "unknown",
        "CC04-04: reconcile_status must be unknown before first reconcile tick; got: {json}"
    );
    assert_eq!(
        json["reconcile_total_mismatches"], 0,
        "CC04-04: reconcile_total_mismatches must be 0 when unknown; got: {json}"
    );
    assert!(
        json["reconcile_last_run_at"].is_null(),
        "CC04-04: reconcile_last_run_at must be null before first tick; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// CC04-05 — daemon_mode is correctly surfaced
//
// Proves: the overview route correctly reflects the daemon's deployment mode,
// not a hardcoded placeholder.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cc04_05_daemon_mode_reflected_correctly() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, body) = call(router, overview_req()).await;
    assert_eq!(status, StatusCode::OK);

    let json = parse_json(body);

    // Default daemon mode is "paper".
    assert_eq!(
        json["daemon_mode"], "paper",
        "CC04-05: daemon_mode must be 'paper' for default AppState; got: {json}"
    );
}
