//! M1-COMPLETED-BAR-PIPELINE-REPAIR-01 (PATCH C): GET
//! /api/v1/autonomous/completed-bar-task-status.
//!
//! Proves:
//!
//! | Test | What it proves                                                      |
//! |------|----------------------------------------------------------------------|
//! | CBS01 | Never-spawned task -> 200, liveness="not_started", generation=0     |
//! | CBS02 | Reflects process-local truth after a forced generation/liveness set |
//! | CBS03 | A GET never claims the single-task spawn slot (read-only)           |
//!
//! All tests are pure in-process. No `MQK_DATABASE_URL` required.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use mqk_daemon::state::autonomous_completed_bar_driver::AutonomousCompletedBarDriverTaskLiveness;
use mqk_daemon::{
    routes::build_router,
    state::{AppState, BrokerKind},
};
use tower::ServiceExt;

fn daemon_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca))
}

async fn get_completed_bar_task_status(router: axum::Router) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .uri("/api/v1/autonomous/completed-bar-task-status")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, j)
}

#[tokio::test]
async fn cbs01_never_spawned_returns_200_not_started() {
    let st = daemon_state();
    let router = build_router(st);
    let (status, body) = get_completed_bar_task_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["truth_state"], "active");
    assert_eq!(body["liveness"], "not_started");
    assert_eq!(body["generation"], 0);
    assert!(body["mode"].is_null());
    assert!(body["operation_id"].is_null());
    assert!(body["last_tick_utc"].is_null());
    assert!(body["last_outcome_code"].is_null());
    assert_eq!(body["consecutive_error_count"], 0);
    assert_eq!(body["restart_count"], 0);
    assert!(body["last_error_code"].is_null());
    assert!(body["now_utc"].is_string());
}

#[tokio::test]
async fn cbs02_reflects_forced_generation_and_liveness() {
    let st = daemon_state();
    st.set_completed_bar_task_generation_for_test(
        7,
        AutonomousCompletedBarDriverTaskLiveness::Failed,
    )
    .await;

    let router = build_router(st);
    let (status, body) = get_completed_bar_task_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["generation"], 7);
    assert_eq!(body["liveness"], "failed");
}

#[tokio::test]
async fn cbs03_get_never_claims_the_spawn_slot() {
    let st = daemon_state();
    let router = build_router(Arc::clone(&st));
    let (status, _body) = get_completed_bar_task_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        !st.completed_bar_task_claimed_for_test(),
        "a read-only status GET must never claim the single-task spawn slot"
    );
    assert!(
        !st.completed_bar_task_has_supervisor_handle_for_test().await,
        "a read-only status GET must never install a supervisor handle"
    );
}
