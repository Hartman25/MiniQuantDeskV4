//! LO-03C: Live-shadow restart + recovery proof.
//!
//! Proves that live-shadow restart and recovery behavior is explicit and
//! fail-closed at every operator-visible surface.  The central claims:
//!
//! - There is no in-daemon restart path (C1).  Operator restarts are OS-level.
//! - The mode-change-guidance surface is the canonical restart workflow entry
//!   point and explicitly refuses hot switching (C2).
//! - The restart workflow truth is fail-closed (backend_unavailable) when DB
//!   is unavailable — never optimistically "no_pending" or "active" (C3).
//! - The restart_truth field is present but contains honest null/false values
//!   without DB — no fabricated durable run state (C4).
//! - The operator_next_steps are explicit and require disarm as the first
//!   gate — no vague or implicit restart guidance (C5).
//! - The live-shadow → live-capital transition verdict is fail_closed — no
//!   permissive promotion path from live-shadow exists (C6).
//!
//! Does NOT reopen:
//! - LO-03A (start/stop/halt gate chain) — closed in scenario_live_shadow_operator_lo03ab.rs
//! - LO-03B (live routing enable/disable) — closed in scenario_live_shadow_operator_lo03ab.rs
//! - LO-03G (arm/disarm audit durability) — closed in scenario_audit_ops_lo03g.rs
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
// LO-03C: Live-shadow restart + recovery proof
// ===========================================================================

// ---------------------------------------------------------------------------
// LO-03C-C1 — /control/restart is not mounted for live-shadow
//
// The `/control/restart` handler is intentionally dead code
// (`#[allow(dead_code)]`) and is NOT wired into `control::router()`.
// POST /control/restart must therefore return 404.
//
// This is the structural proof that no in-daemon restart path exists for
// live-shadow.  The operator must restart the daemon process via OS-level
// commands (SIGTERM / service stop), not via an API call.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03c_c1_restart_route_not_mounted_for_live_shadow() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
    ));
    let router = routes::build_router(st);

    let req = Request::builder()
        .method("POST")
        .uri("/control/restart")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(router, req).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "C1: POST /control/restart must return 404 (route not mounted); \
         no in-daemon restart path exists — operator must use OS-level restart"
    );
}

// ---------------------------------------------------------------------------
// LO-03C-C2 — mode-change-guidance explicitly refuses hot switching
//
// GET /api/v1/ops/mode-change-guidance is the canonical restart workflow
// surface.  For live-shadow+alpaca, transition_permitted must be false and
// the current_mode must correctly report "live-shadow".
//
// This proves hot switching is explicitly refused at the only operator-visible
// transition surface — no permissive path through any other route exists.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03c_c2_mode_change_guidance_refuses_hot_switching_for_live_shadow() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
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
        "C2: mode-change-guidance must return 200; got: {status}"
    );

    let j = json(body);
    assert_eq!(
        j["current_mode"], "live-shadow",
        "C2: current_mode must be live-shadow; got: {j}"
    );
    assert_eq!(
        j["transition_permitted"],
        serde_json::Value::Bool(false),
        "C2: transition_permitted must be false — hot switching is not supported; got: {j}"
    );
    assert!(
        j["transition_refused_reason"].as_str().unwrap_or("").len() > 10,
        "C2: transition_refused_reason must be a non-trivial explanation; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03C-C3 — restart_workflow truth is "backend_unavailable" without DB
//
// The restart workflow truth surface (`restart_workflow.truth_state`) must be
// "backend_unavailable" when no DB pool is configured.  This is the
// fail-closed contract: the daemon cannot know whether a pending restart intent
// exists without DB, so it must not optimistically report "no_pending" or
// fabricate "active".
//
// "backend_unavailable" is the only honest answer when DB is absent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03c_c3_restart_workflow_is_backend_unavailable_without_db() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
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
        "C3: mode-change-guidance must return 200"
    );

    let j = json(body);
    let rw = &j["restart_workflow"];
    assert_eq!(
        rw["truth_state"], "backend_unavailable",
        "C3: restart_workflow.truth_state must be 'backend_unavailable' without DB; \
         'no_pending' or 'active' without DB would be fabricated truth; got: {j}"
    );
    assert_eq!(
        rw["pending_intent"],
        serde_json::Value::Null,
        "C3: restart_workflow.pending_intent must be null when backend_unavailable; got: {j}"
    );
    // Explicit non-optimism: these two states must not be returned without DB.
    assert_ne!(
        rw["truth_state"], "active",
        "C3: restart_workflow must not claim 'active' without DB — fabricated; got: {j}"
    );
    assert_ne!(
        rw["truth_state"], "no_pending",
        "C3: restart_workflow must not claim 'no_pending' without DB — optimistic; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03C-C4 — restart_truth surface is present with honest null fields
//             (no fabricated durable run state)
//
// `restart_truth` is computed from `restart_truth_snapshot()` which succeeds
// even without DB (returns Ok with all-null fields when no durable state is
// reachable).  The response field must therefore be present (not null) but
// contain only honest null/false values.
//
// This proves the daemon does not fabricate a durable run claim without DB
// and does not falsely assert local ownership when no run is active.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03c_c4_restart_truth_is_present_with_honest_null_fields_without_db() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
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
        "C4: mode-change-guidance must return 200"
    );

    let j = json(body);
    // restart_truth must be present (not null) — the snapshot succeeds without DB.
    assert!(
        !j["restart_truth"].is_null(),
        "C4: restart_truth must be present (not null) — snapshot succeeds without DB; got: {j}"
    );

    let rt = &j["restart_truth"];

    // Without DB: no durable run is reachable, so durable_active_run_id must be null.
    assert_eq!(
        rt["durable_active_run_id"],
        serde_json::Value::Null,
        "C4: durable_active_run_id must be null without DB — cannot fabricate durable state; got: {j}"
    );
    // Without a live execution loop: no local run, so local_owned_run_id must be null.
    assert_eq!(
        rt["local_owned_run_id"],
        serde_json::Value::Null,
        "C4: local_owned_run_id must be null when no execution loop is running; got: {j}"
    );
    // With both nulls: the ownership conflict flag must be false.
    assert_eq!(
        rt["durable_active_without_local_ownership"],
        serde_json::Value::Bool(false),
        "C4: durable_active_without_local_ownership must be false when both run ids are null; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03C-C5 — operator_next_steps is explicit and requires disarm first
//
// The restart workflow is only safe if the daemon is disarmed before the
// process is stopped.  The mode-change-guidance response must surface
// operator_next_steps that are non-empty and place disarm as the first
// explicit action.
//
// This proves the mounted restart guidance is not vague ("just restart") but
// names the disarm gate explicitly — an operator cannot claim they were not
// told.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03c_c5_operator_next_steps_explicit_and_requires_disarm_first() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
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
        "C5: mode-change-guidance must return 200"
    );

    let j = json(body);
    let steps = j["operator_next_steps"]
        .as_array()
        .expect("C5: operator_next_steps must be a JSON array");

    assert!(
        !steps.is_empty(),
        "C5: operator_next_steps must not be empty — restart path must be explicit; got: {j}"
    );

    let first = steps[0].as_str().unwrap_or("");
    assert!(
        first.to_lowercase().contains("disarm"),
        "C5: first operator_next_step must require disarm — \
         restarting without disarming is unsafe; first_step='{first}'; got: {j}"
    );
}

// ---------------------------------------------------------------------------
// LO-03C-C6 — live-shadow → live-capital transition verdict is fail_closed
//
// From live-shadow, upgrading to live-capital must be fail_closed (not
// admissible_with_restart, not same_mode, not refused).
//
// fail_closed means the path is architecturally intended but requires a proof
// chain (TV-01D / live_trust_complete=true) that is not yet closed.  This
// proves no permissive upgrade from live-shadow to live-capital exists —
// the operator cannot satisfy preconditions to proceed today.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lo03c_c6_live_shadow_to_live_capital_verdict_is_fail_closed() {
    let st = Arc::new(state::AppState::new_for_test_with_mode_and_broker(
        state::DeploymentMode::LiveShadow,
        state::BrokerKind::Alpaca,
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
        "C6: mode-change-guidance must return 200"
    );

    let j = json(body);
    let verdicts = j["transition_verdicts"]
        .as_array()
        .expect("C6: transition_verdicts must be a JSON array");

    let lc = verdicts
        .iter()
        .find(|v| v["target_mode"].as_str() == Some("live-capital"))
        .expect("C6: transition_verdicts must include a live-capital entry");

    assert_eq!(
        lc["verdict"], "fail_closed",
        "C6: live-shadow → live-capital must be fail_closed — \
         permissive upgrade would allow live capital execution without the TV-01D proof chain; \
         got: {j}"
    );
    // Explicit non-admissibility: this must not be promotable with any precondition list.
    assert_ne!(
        lc["verdict"], "admissible_with_restart",
        "C6: live-shadow → live-capital must NOT be admissible_with_restart — \
         proof chain (TV-01D) must close first; got: {j}"
    );
}
