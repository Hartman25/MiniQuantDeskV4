//! BRK-09R: Reconcile truth start gate for broker-backed paper path.
//!
//! Proves that `start_execution_runtime` for paper+alpaca now blocks when
//! the persisted reconcile status carries evidence of prior drift.
//!
//! # Gate ordering (post-BRK-09R)
//!
//! ```text
//! POST /v1/run/start
//!   Gate 1: deployment_mode       (paper+paper → 403)
//!   Gate 2: integrity_armed       (disarmed → 403)
//!   Gate 3: alpaca_ws_continuity  (ColdStartUnproven|GapDetected → 403; Live → pass)
//!   Gate 4: reconcile_truth       (dirty|stale → 403; ok|unknown → pass)  ← BRK-09R
//!   Gate 5: db                    (no DB → 503)
//! ```
//!
//! # What each test proves
//!
//! | Test | Reconcile status | WS continuity | Expected outcome                    |
//! |------|-----------------|---------------|-------------------------------------|
//! | R01  | dirty           | Live          | 403 gate=reconcile_truth            |
//! | R02  | stale           | Live          | 403 gate=reconcile_truth            |
//! | R03  | unknown (default) | Live        | 503 DB gate (reconcile gate passes) |
//! | R04  | ok              | Live          | 503 DB gate (reconcile gate passes) |
//! | R05  | dirty           | ColdStartUnproven | 403 gate=alpaca_ws_continuity  |
//!
//! R05 proves gate ordering: WS continuity fires before reconcile truth when
//! both conditions are failing simultaneously.
//!
//! All tests are pure in-process (no DB required).  Reconcile state is set
//! via the public `publish_reconcile_snapshot` seam.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{
    routes,
    state::{AppState, BrokerKind, DeploymentMode, ReconcileStatusSnapshot},
};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, serde_json::Value) {
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

/// Paper+alpaca state, integrity armed, WS continuity Live.
/// This is the "all prior gates pass" baseline — the reconcile gate is
/// the only variable in tests R01-R04.
async fn ready_state() -> Arc<AppState> {
    let st = Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));
    // Arm integrity gate.
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(status, StatusCode::OK, "ready_state: arm must succeed");

    // Establish Live WS continuity (simulates WS transport connect→auth→subscribe).
    st.update_ws_continuity(mqk_daemon::state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:ord-1:new:2026-01-01T00:00:00Z".to_string(),
        last_event_at: "2026-01-01T00:00:00Z".to_string(),
    })
    .await;

    st
}

fn dirty_reconcile(note: &str) -> ReconcileStatusSnapshot {
    ReconcileStatusSnapshot {
        status: "dirty".to_string(),
        last_run_at: Some("2026-01-01T00:00:00Z".to_string()),
        snapshot_watermark_ms: Some(1_000_000),
        mismatched_positions: 1,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some(note.to_string()),
    }
}

fn stale_reconcile(note: &str) -> ReconcileStatusSnapshot {
    ReconcileStatusSnapshot {
        status: "stale".to_string(),
        last_run_at: Some("2026-01-01T00:00:00Z".to_string()),
        snapshot_watermark_ms: Some(500_000),
        mismatched_positions: 0,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some(note.to_string()),
    }
}

fn ok_reconcile() -> ReconcileStatusSnapshot {
    ReconcileStatusSnapshot {
        status: "ok".to_string(),
        last_run_at: Some("2026-01-01T00:01:00Z".to_string()),
        snapshot_watermark_ms: Some(2_000_000),
        mismatched_positions: 0,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: None,
    }
}

fn try_start_req() -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/run/start")
        .body(axum::body::Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// R01 — dirty reconcile blocks paper+alpaca start
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk09r_r01_dirty_reconcile_blocks_paper_alpaca_start() {
    let st = ready_state().await;

    // Set reconcile truth to "dirty" (evidence of prior broker/local drift).
    st.publish_reconcile_snapshot(dirty_reconcile("brk09r-r01: simulated prior drift")).await;

    let (status, json) = call(routes::build_router(Arc::clone(&st)), try_start_req()).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "R01: dirty reconcile must block start (403); got: {status}"
    );
    assert_eq!(
        json["gate"], "reconcile_truth",
        "R01: gate must be reconcile_truth for dirty reconcile; got: {json}"
    );
    assert_eq!(
        json["fault_class"], "runtime.start_refused.reconcile_dirty",
        "R01: fault_class must identify reconcile-dirty refusal; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// R02 — stale reconcile blocks paper+alpaca start
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk09r_r02_stale_reconcile_blocks_paper_alpaca_start() {
    let st = ready_state().await;

    // Set reconcile truth to "stale" (stale broker snapshot — not proven clean).
    st.publish_reconcile_snapshot(stale_reconcile("brk09r-r02: stale broker snapshot")).await;

    let (status, json) = call(routes::build_router(Arc::clone(&st)), try_start_req()).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "R02: stale reconcile must block start (403); got: {status}"
    );
    assert_eq!(
        json["gate"], "reconcile_truth",
        "R02: gate must be reconcile_truth for stale reconcile; got: {json}"
    );
    assert_eq!(
        json["fault_class"], "runtime.start_refused.reconcile_dirty",
        "R02: fault_class must identify reconcile-dirty refusal for stale too; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// R03 — unknown reconcile (boot default) does NOT block start
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk09r_r03_unknown_reconcile_does_not_block_start() {
    let st = ready_state().await;
    // reconcile_status is initial_reconcile_status() = "unknown" — no prior evidence.
    // No publish_reconcile_snapshot call: let the default state stand.

    let (status, json) = call(routes::build_router(Arc::clone(&st)), try_start_req()).await;

    // "unknown" passes the reconcile gate; next blocker is DB not configured (503).
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "R03: unknown reconcile must pass reconcile gate and reach DB gate (503); got: {status}"
    );
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "R03: blocker must be DB gate, not reconcile gate; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// R04 — ok reconcile does NOT block start
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brk09r_r04_ok_reconcile_does_not_block_start() {
    let st = ready_state().await;

    // Set reconcile truth to "ok" (clean prior session).
    st.publish_reconcile_snapshot(ok_reconcile()).await;

    let (status, json) = call(routes::build_router(Arc::clone(&st)), try_start_req()).await;

    // "ok" passes the reconcile gate; next blocker is DB not configured (503).
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "R04: ok reconcile must pass reconcile gate and reach DB gate (503); got: {status}"
    );
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("runtime DB is not configured"),
        "R04: blocker must be DB gate, not reconcile gate; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// R05 — gate ordering: WS continuity fires before reconcile truth
// ---------------------------------------------------------------------------
//
// When BOTH WS continuity is unproven AND reconcile is dirty, the WS continuity
// gate (Gate 3) fires first and names itself as the blocker.  Reconcile (Gate 4)
// is only reached when WS is Live.
//
// This proves the operator is told about WS issues before reconcile issues:
// the operator must fix WS first, THEN fix reconcile.

#[tokio::test]
async fn brk09r_r05_ws_continuity_gate_fires_before_reconcile_gate() {
    let st = Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));
    // Arm integrity (Gate 2 must pass).
    let arm_req = Request::builder()
        .method("POST")
        .uri("/v1/integrity/arm")
        .body(axum::body::Body::empty())
        .unwrap();
    let (arm_status, _) = call(routes::build_router(Arc::clone(&st)), arm_req).await;
    assert_eq!(arm_status, StatusCode::OK, "R05: arm must succeed");

    // Leave WS continuity as ColdStartUnproven (Gate 3 fires).
    // Set reconcile to dirty (Gate 4 would fire if Gate 3 did not fire first).
    st.publish_reconcile_snapshot(dirty_reconcile("brk09r-r05: dirty AND ws unproven")).await;

    let (status, json) = call(routes::build_router(Arc::clone(&st)), try_start_req()).await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "R05: start must be blocked 403 when both WS unproven and reconcile dirty; got: {status}"
    );
    assert_eq!(
        json["gate"], "alpaca_ws_continuity",
        "R05: gate must be alpaca_ws_continuity (not reconcile_truth) — WS gate fires first; got: {json}"
    );

    // Now fix WS to Live; reconcile gate should now fire.
    st.update_ws_continuity(mqk_daemon::state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:ord-1:new:2026-01-01T00:00:00Z".to_string(),
        last_event_at: "2026-01-01T00:00:00Z".to_string(),
    })
    .await;

    let (status2, json2) = call(routes::build_router(Arc::clone(&st)), try_start_req()).await;

    assert_eq!(
        status2,
        StatusCode::FORBIDDEN,
        "R05: after fixing WS, dirty reconcile must now block (403); got: {status2}"
    );
    assert_eq!(
        json2["gate"], "reconcile_truth",
        "R05: after fixing WS, gate must be reconcile_truth; got: {json2}"
    );
}
