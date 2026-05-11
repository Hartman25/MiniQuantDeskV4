//! AUTON-NO-TRADE-01 — Autonomous no-trade diagnosis proof
//!
//! Proves that the autonomous readiness surface accurately reports the
//! arm-state blocker that prevents session starts, distinguishing between:
//!
//! - `arm_pending` (DB=ARMED, in-memory pending, auto-heals on next tick)
//! - `disarmed_db` (DB=DISARMED, operator must re-arm; try_autonomous_arm
//!   will refuse on every session-window tick)
//!
//! Without this fix, a ReconcileDrift disarm surfaced as "arm_pending" with
//! "(DB-ARMED → auto-advances)" — misleading the operator into believing the
//! system would self-heal when it would not.
//!
//! | Test  | Claim                                                                              |
//! |-------|------------------------------------------------------------------------------------|
//! | ND-01 | No DB configured → arm_state = "arm_pending" (no DB = can't assert DISARMED)       |
//! | ND-02 | DB-backed: DISARMED(ReconcileDrift) → arm_state = "disarmed_db", blocker explains   |
//! | ND-03 | DB-backed: ARMED in DB → arm_state = "arm_pending" (auto-heals)                    |
//! | ND-04 | disarmed_db → overall_ready = false, no false-success signalled                     |
//! | ND-05 | disarmed_db blocker does NOT say "DB-ARMED → auto-advances" (no false promise)       |
//! | ND-06 | try_autonomous_arm fails when DB is DISARMED (confirms the refusal the route exposes) |

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use state::{AlpacaWsContinuityState, BrokerKind};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn db_pool_or_skip(label: &str) -> Option<sqlx::PgPool> {
    let url = match std::env::var("MQK_DATABASE_URL") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("{label}: skipping — MQK_DATABASE_URL not set");
            return None;
        }
    };
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("AUTON-NO-TRADE-01: failed to connect to MQK_DATABASE_URL"),
    )
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

fn make_paper_alpaca() -> Arc<state::AppState> {
    Arc::new(state::AppState::new_for_test_with_broker_kind(
        BrokerKind::Alpaca,
    ))
}

// ---------------------------------------------------------------------------
// ND-01 — No DB: disarmed in-memory → arm_pending (backward-compat, no DB means
//          we cannot assert DISARMED; treat as pending).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nd01_no_db_disarmed_memory_is_arm_pending() {
    let st = make_paper_alpaca();
    // Advance WS to Live so WS gate passes.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "nd01-msg".to_string(),
        last_event_at: "2026-05-11T14:00:00Z".to_string(),
    })
    .await;
    // Leave in-memory integrity as disarmed (default for new_for_test).
    // No DB is configured for this AppState.
    assert!(
        st.db.is_none(),
        "ND-01 precondition: test AppState must not have a DB pool"
    );

    let (status, body) = call(
        routes::build_router(st),
        Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(
        v["arm_state"], "arm_pending",
        "ND-01: no-DB disarmed path must remain arm_pending"
    );
    assert_eq!(v["arm_ready"], false, "ND-01: arm_ready must be false");
    assert_eq!(
        v["overall_ready"], false,
        "ND-01: overall_ready must be false"
    );
}

// ---------------------------------------------------------------------------
// ND-02 / ND-04 / ND-05 — DB-backed: DISARMED → arm_state = "disarmed_db",
//         overall_ready = false, blocker explains without false promise.
//
// Skips gracefully when MQK_DATABASE_URL is absent (CI without a live DB).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nd02_db_disarmed_surfaces_disarmed_db_arm_state() {
    let pool = match db_pool_or_skip("nd02").await {
        Some(p) => p,
        None => return,
    };

    // Persist DISARMED(ReconcileDrift) so load_arm_state returns DISARMED.
    mqk_db::persist_arm_state_canonical(
        &pool,
        mqk_db::ArmState::Disarmed,
        Some(mqk_db::DisarmReason::ReconcileDrift),
    )
    .await
    .expect("nd02: persist_arm_state_canonical failed");

    let st_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&std::env::var("MQK_DATABASE_URL").unwrap())
        .await
        .expect("nd02: AppState pool connect failed");
    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        st_pool,
        state::DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));
    // Advance WS to Live.
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "nd02-msg".to_string(),
        last_event_at: "2026-05-11T14:00:00Z".to_string(),
    })
    .await;
    // Leave in-memory integrity disarmed (default).

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    // ND-02: arm_state must be "disarmed_db", not "arm_pending".
    assert_eq!(
        v["arm_state"], "disarmed_db",
        "ND-02: DB=DISARMED must surface arm_state=disarmed_db"
    );
    assert_eq!(v["arm_ready"], false, "ND-02: arm_ready must be false");

    // ND-04: overall_ready must be false — disarmed_db is not false-success.
    assert_eq!(
        v["overall_ready"], false,
        "ND-04: disarmed_db must not produce overall_ready=true"
    );

    // ND-05: blocker must mention DISARMED and the required operator action;
    //        must NOT claim "DB-ARMED → auto-advances".
    let blockers = v["blockers"].as_array().expect("blockers must be array");
    assert!(
        blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("DISARMED")),
        "ND-05: blocker must mention DISARMED: {blockers:?}"
    );
    assert!(
        blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("arm-execution")),
        "ND-05: blocker must mention arm-execution action: {blockers:?}"
    );
    assert!(
        !blockers
            .iter()
            .any(|b| b.as_str().unwrap_or("").contains("DB-ARMED")),
        "ND-05: disarmed_db blocker must NOT claim DB-ARMED auto-advances: {blockers:?}"
    );
}

// ---------------------------------------------------------------------------
// ND-03 — DB-backed: ARMED in DB + disarmed in-memory → arm_state = "arm_pending".
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nd03_db_armed_disarmed_memory_is_arm_pending() {
    let pool = match db_pool_or_skip("nd03").await {
        Some(p) => p,
        None => return,
    };

    // Persist ARMED so load_arm_state returns ARMED.
    mqk_db::persist_arm_state_canonical(&pool, mqk_db::ArmState::Armed, None)
        .await
        .expect("nd03: persist_arm_state_canonical failed");

    let st_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&std::env::var("MQK_DATABASE_URL").unwrap())
        .await
        .expect("nd03: AppState pool connect failed");
    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        st_pool,
        state::DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));
    st.update_ws_continuity(AlpacaWsContinuityState::Live {
        last_message_id: "nd03-msg".to_string(),
        last_event_at: "2026-05-11T14:00:00Z".to_string(),
    })
    .await;
    // Leave in-memory integrity disarmed (default).

    let (status, body) = call(
        routes::build_router(Arc::clone(&st)),
        Request::builder()
            .uri("/api/v1/autonomous/readiness")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v = parse_json(body);

    assert_eq!(
        v["arm_state"], "arm_pending",
        "ND-03: DB=ARMED + in-memory disarmed must surface arm_state=arm_pending (auto-heals)"
    );
    assert_eq!(v["arm_ready"], false);
}

// ---------------------------------------------------------------------------
// ND-06 — try_autonomous_arm fails when DB is DISARMED (confirms the refusal
//          the readiness surface exposes).  DB-backed, skips without DB.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nd06_try_autonomous_arm_fails_when_db_disarmed() {
    let pool = match db_pool_or_skip("nd06").await {
        Some(p) => p,
        None => return,
    };

    mqk_db::persist_arm_state_canonical(
        &pool,
        mqk_db::ArmState::Disarmed,
        Some(mqk_db::DisarmReason::ReconcileDrift),
    )
    .await
    .expect("nd06: persist_arm_state_canonical failed");

    let st_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&std::env::var("MQK_DATABASE_URL").unwrap())
        .await
        .expect("nd06: AppState pool connect failed");
    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        st_pool,
        state::DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));

    let result = st.try_autonomous_arm().await;
    assert!(
        result.is_err(),
        "ND-06: try_autonomous_arm must fail when DB arm state is DISARMED"
    );
    let err_msg = result.unwrap_err();
    assert!(
        err_msg.contains("DISARMED"),
        "ND-06: error must mention DISARMED: {err_msg}"
    );
}
