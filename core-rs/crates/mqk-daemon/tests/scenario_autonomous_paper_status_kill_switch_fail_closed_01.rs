//! KILL-SWITCH-AUTONOMOUS-PAPER-STATUS-FAIL-CLOSED-01: proves
//! `GET /api/v1/autonomous/paper-status` can never emit
//! `kill_switch_active=false` when `current_status_snapshot()` itself
//! fails (durable kill-switch truth UNKNOWN, never a confirmed-clear
//! reading).
//!
//! # Relationship to existing coverage
//!
//! `scenario_kill_switch_status_read_error_fail_closed_01.rs`
//! (KILL-SWITCH-FAIL-CLOSED-READ-ERROR-VERIFY-01) proves
//! `current_status_snapshot`/`GET /api/v1/system/status` fail closed when
//! the durable `sys_arm_state` READ itself fails but `current_status_
//! snapshot` still returns `Ok` (with `state` forced to `"halted"`). This
//! file proves the DOWNSTREAM `/api/v1/autonomous/paper-status` route
//! fails closed in the genuinely different case where `current_status_
//! snapshot` returns `Err` outright (e.g. the `runs` lookup itself
//! fails) -- the route's own `Err(_) => ...` branch previously hardcoded
//! `kill_switch_active: false`.
//!
//! # Fault injection technique (non-destructive)
//!
//! Connects a SEPARATE `PgPool` to the exact same `MQK_DATABASE_URL` the
//! sibling KS/KSRE tests already connect with successfully, then
//! restricts `search_path` to a schema that does not exist. Every query
//! `current_status_snapshot` issues (including the unqualified `runs`
//! lookup via `fetch_latest_run_for_engine`) then fails to resolve, so
//! `current_status_snapshot` returns a genuine `Err` -- no shared-schema
//! mutation, no destructive DDL; the fault is entirely connection-local
//! and vanishes when the pool is dropped.
//!
//! Run:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_autonomous_paper_status_kill_switch_fail_closed_01

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use mqk_daemon::routes::build_router;
use mqk_daemon::state::{AppState, BrokerKind, DeploymentMode};
use tower::ServiceExt;

/// A pool whose every connection can resolve nothing in the real schema --
/// `search_path` is restricted to a schema that genuinely does not exist,
/// so `current_status_snapshot`'s `runs` lookup (and every other durable
/// read it might make) fails outright.
async fn fault_injecting_pool_or_skip() -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(v) => v,
        Err(_) => return None,
    };
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("SET search_path TO mqk_apsks01_nonexistent_schema")
                        .execute(&mut *conn)
                        .await
                        .map(|_| ())
                })
            })
            .connect(&url)
            .await
            .expect(
                "APSKS-01: failed to connect to MQK_DATABASE_URL \
                 (same URL the sibling KS/KSRE tests connect with successfully)",
            ),
    )
}

async fn get_paper_status(router: axum::Router) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .uri("/api/v1/autonomous/paper-status")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, j)
}

/// RED proof that the fault-injection pool genuinely makes
/// `current_status_snapshot` fail (not merely a connection error) --
/// proves this test's own fault injection is real before trusting the
/// production-code assertion below.
#[tokio::test]
async fn apsks01a_fault_injection_pool_genuinely_fails_current_status_snapshot() {
    let Some(pool) = fault_injecting_pool_or_skip().await else {
        eprintln!("APSKS-01a: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    let state = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));
    let result = state.current_status_snapshot().await;
    assert!(
        result.is_err(),
        "APSKS-01a: fault-injecting pool must make current_status_snapshot genuinely fail \
         (search_path restricted to a nonexistent schema); got: {result:?}"
    );
}

/// APSKS-01b: the production assertion. When `current_status_snapshot`
/// fails, `GET /api/v1/autonomous/paper-status` must report
/// `truth_state="degraded"` and `kill_switch_active=true` -- never
/// `false`, which would represent unknown durable kill-switch truth as a
/// confirmed-clear (inactive) kill switch.
#[tokio::test]
async fn apsks01b_paper_status_fails_closed_when_status_snapshot_unavailable() {
    let Some(pool) = fault_injecting_pool_or_skip().await else {
        eprintln!("APSKS-01b: skipped (MQK_DATABASE_URL not set)");
        return;
    };
    let state = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));
    let router = build_router(state);
    let (status, body) = get_paper_status(router).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["truth_state"].as_str().unwrap(),
        "degraded",
        "status snapshot unavailable must surface truth_state=degraded"
    );
    assert_eq!(
        body["kill_switch_active"].as_bool().unwrap(),
        true,
        "kill_switch_active must fail closed to true when current_status_snapshot is unavailable \
         -- unknown durable kill-switch truth must never be represented as a known-inactive kill switch"
    );
}
