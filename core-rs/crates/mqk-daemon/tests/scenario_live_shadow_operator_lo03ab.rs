//! LO-03A / LO-03B: Live-shadow operator control and routing proof.
//!
//! LO-03A: Proves live-shadow start/stop/halt operator controls are gated,
//!         explicit, and not permissive.
//!
//! LO-03B: Proves live routing enable/disable is explicit and not silently
//!         inferred from live-shadow mode designation or runtime state.
//!
//! All tests are pure in-process (no DB, no env vars).
//! All tests are always runnable in CI without MQK_DATABASE_URL.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::util::ServiceExt;

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

fn json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

// ===========================================================================
// LO-03A: Live-shadow start/stop/halt operator proof
// ===========================================================================

// ---------------------------------------------------------------------------
// LO-03A-S1 — live-shadow + paper is blocked at deployment gate
//
// live-shadow requires a real broker (Alpaca) for honest external truth.
// live-shadow + paper has no external source and is fail-closed at
// deployment_mode_readiness before the integrity gate fires.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03a_s01_live_shadow_paper_blocked_at_deployment_gate() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Paper,
    ));
    let router = routes::build_router(st);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "S1: live-shadow+paper must be 403 — deployment gate must block before integrity gate"
    );
    let j = json(body);
    assert_eq!(
        j["gate"], "deployment_mode",
        "S1: gate must be deployment_mode, not integrity_armed; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03A-S2 — live-shadow + alpaca disarmed: integrity gate fires explicitly
//
// When the daemon is disarmed (boot state), live-shadow+alpaca start must be
// blocked at the integrity gate.  This proves the integrity gate is real and
// not silently bypassed for live-shadow mode.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03a_s02_live_shadow_alpaca_disarmed_blocked_at_integrity_gate() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(st);

    // Do NOT arm — daemon boots disarmed.
    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "S2: live-shadow+alpaca disarmed must be 403 — integrity gate must not be bypassed for live-shadow"
    );
    let j = json(body);
    assert_eq!(
        j["gate"], "integrity_armed",
        "S2: gate must be integrity_armed; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03A-S3 — live-shadow + alpaca armed: full pre-DB gate chain proven
//
// After arm, live-shadow+alpaca must pass: deployment gate, integrity gate,
// and the DB gate (503 — no DB configured).  Critically, it must NOT be
// blocked by a WS continuity gate (unlike paper+alpaca or live-capital).
//
// Proof: if a WS continuity gate existed for live-shadow, the gate field
// would be "alpaca_ws_continuity" (403), not a DB 503.  We get 503, proving
// the gate chain is: deployment→integrity→[no WS gate]→DB.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03a_s03_live_shadow_alpaca_armed_reaches_db_gate_no_ws_continuity_gate() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(Arc::clone(&st));

    // Arm the integrity gate.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(router.clone(), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "S3: arm must succeed");

    // Start must now reach and fire the DB gate (not WS continuity).
    let start_req = Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap();
    let (start_status, start_body) = call(routes::build_router(Arc::clone(&st)), start_req).await;
    assert_eq!(
        start_status,
        StatusCode::SERVICE_UNAVAILABLE,
        "S3: live-shadow+alpaca armed must reach DB gate (503), not WS continuity gate (403)"
    );
    let j = json(start_body);
    assert!(
        j["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "S3: 503 must state DB is not configured (deployment+integrity+no-WS gates all passed); got: {j}"
    );
    // Explicit non-presence of WS continuity gate: if we were at a WS gate the
    // response would be 403 with gate=alpaca_ws_continuity.  We're at 503 which
    // proves the WS gate is absent for live-shadow mode.
    assert_ne!(
        j["gate"], "alpaca_ws_continuity",
        "S3: live-shadow must NOT have a WS continuity gate; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03A-S4 — live-shadow halt without DB: durable halt is required
//
// Halt sets integrity.halted=true and then calls db_pool() unconditionally.
// Without DB, halt must return 503.  This proves halt is a durable operator
// action — it cannot succeed without DB backing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03a_s04_live_shadow_halt_requires_db() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(st);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/halt")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "S4: live-shadow halt without DB must be 503 — halt must be durable"
    );
    let j = json(body);
    assert!(
        j["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "S4: halt error must state DB is not configured; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03A-S5 — live-shadow stop when idle: graceful no-op (idempotent)
//
// Stop when there is no active run and no DB is a controlled no-op: it
// returns the current status snapshot rather than an error.  This proves
// stop is safe to issue in idle state — it will not error, it will not
// silently start, and it will not return a fabricated "stopped" state.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03a_s05_live_shadow_stop_when_idle_is_graceful_noop() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(st);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/run/stop")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "S5: live-shadow stop when idle must return 200 (graceful no-op)"
    );
    let j = json(body);
    // Must reflect current state (idle), not a fabricated "stopped".
    assert_eq!(
        j["state"], "idle",
        "S5: stop when idle must return state=idle, not fabricated stopped; got: {j}"
    );
    assert_eq!(
        j["active_run_id"],
        serde_json::Value::Null,
        "S5: stop when idle must have no active_run_id; got: {j}"
    );
}

// ===========================================================================
// LO-03B: Live routing enable/disable guarded proof
// ===========================================================================

// ---------------------------------------------------------------------------
// LO-03B-R1 — live-shadow idle: live_routing_enabled is false, not null
//
// The system/status surface must return live_routing_enabled=false (not null)
// for a live-shadow daemon at idle.  "null" would imply "unknown" or "not
// applicable" — an honest idle live-shadow daemon is not routing live orders
// and must say so explicitly.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03b_r01_live_shadow_idle_live_routing_is_false_not_null() {
    let st = Arc::new(state::AppState::new_for_test_with_mode(
        state::DeploymentMode::LiveShadow,
    ));
    let router = routes::build_router(st);

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK, "R1: system/status must return 200");

    let j = json(body);
    assert_eq!(
        j["live_routing_enabled"],
        serde_json::Value::Bool(false),
        "R1: live_routing_enabled must be false (not null, not true) for idle live-shadow daemon; got: {j}"
    );
    assert_ne!(
        j["live_routing_enabled"],
        serde_json::Value::Null,
        "R1: live_routing_enabled must not be null — unknown routing state is not acceptable; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03B-R2 — live-shadow mode designation does not imply live routing
//
// daemon_mode = "live-shadow" AND live_routing_enabled = false must coexist
// simultaneously.  This proves routing truth is not inferred from mode
// designation — they are distinct orthogonal signals.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03b_r02_live_shadow_mode_does_not_imply_live_routing_enabled() {
    let st = Arc::new(state::AppState::new_for_test_with_mode(
        state::DeploymentMode::LiveShadow,
    ));
    let router = routes::build_router(st);

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, body) = call(router, req).await;
    assert_eq!(status, StatusCode::OK, "R2: system/status must return 200");

    let j = json(body);
    assert_eq!(
        j["daemon_mode"], "live-shadow",
        "R2: daemon_mode must be live-shadow; got: {j}"
    );
    assert_eq!(
        j["live_routing_enabled"],
        serde_json::Value::Bool(false),
        "R2: live_routing_enabled must be false even though daemon_mode is live-shadow — mode != routing; got: {j}"
    );
    // Prove they are distinct by asserting neither implies the other.
    assert_ne!(
        j["daemon_mode"].as_str().unwrap_or(""),
        "live",
        "R2: live-shadow must not surface as 'live' mode; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03B-R3 — live-shadow DB mode string cannot satisfy live routing condition
//
// The live routing logic checks run.mode against "LIVE" and "LIVE-CAPITAL"
// (exact eq_ignore_ascii_case match).  live-shadow stores "LIVE-SHADOW" in
// the DB.  "LIVE-SHADOW" must not match either condition.
//
// This is the structural non-enablement proof: even if a live-shadow run
// were "running" with DB, the routing check would return false.
// ---------------------------------------------------------------------------

#[test]
fn lo03b_r03_live_shadow_db_mode_string_does_not_satisfy_live_routing_condition() {
    let mode_str = state::DeploymentMode::LiveShadow.as_db_mode();

    // The live routing function in environment_and_live_routing_truth uses:
    //   run.mode.eq_ignore_ascii_case("LIVE") || run.mode.eq_ignore_ascii_case("LIVE-CAPITAL")
    // Neither must match for live-shadow.
    let matches_live = mode_str.eq_ignore_ascii_case("LIVE");
    let matches_live_capital = mode_str.eq_ignore_ascii_case("LIVE-CAPITAL");

    assert!(
        !matches_live,
        "R3: live-shadow db mode '{}' must NOT match 'LIVE' — live routing must not be silently enabled",
        mode_str
    );
    assert!(
        !matches_live_capital,
        "R3: live-shadow db mode '{}' must NOT match 'LIVE-CAPITAL' — live routing must not be silently enabled",
        mode_str
    );

    let would_enable_routing = matches_live || matches_live_capital;
    assert!(
        !would_enable_routing,
        "R3: live routing condition must be false for live-shadow db mode '{}'; \
         live routing requires exact 'LIVE' or 'LIVE-CAPITAL' mode string",
        mode_str
    );

    // Confirm the actual db mode string so the proof is legible.
    assert_eq!(
        mode_str, "LIVE-SHADOW",
        "R3: live-shadow db mode must be 'LIVE-SHADOW' — explicit not 'LIVE'"
    );
}
