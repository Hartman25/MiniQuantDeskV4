//! OPS-CONTROL-01 / OPS-CONTROL-02 — persisted control-plane workflow and
//! expanded action catalog proof tests.
//!
//! ## OPS-CONTROL-01 proof matrix
//!
//! | Test   | What it proves                                                              |
//! |--------|-----------------------------------------------------------------------------|
//! | OC-01  | request-mode-change with missing target_mode → 400 missing_target_mode     |
//! | OC-02  | request-mode-change with invalid target_mode → 400 invalid_target_mode     |
//! | OC-03  | request-mode-change same-mode (paper→paper) → 200 no_op, no DB write       |
//! | OC-04  | request-mode-change refused (→backtest) → 409 blocked_refused              |
//! | OC-05  | request-mode-change fail_closed (→live-capital) → 409 blocked_fail_closed  |
//! | OC-06  | request-mode-change admissible (→live-shadow) no DB → 503                  |
//! | OC-07  | cancel-mode-transition no DB → 503                                         |
//! | OC-08  | cancel-mode-transition no pending intent (no DB) → 503                     |
//! | OC-09  | change-system-mode legacy path still returns 409 (compat guard)            |
//! | OC-10  | request-mode-change admissible → 200 pending_restart, intent snapshot      |
//!          |   in response (DB-backed, #[ignore])                                        |
//! | OC-11  | cancel-mode-transition cancels pending intent → 200 intent_cancelled       |
//!          |   (DB-backed, #[ignore])                                                    |
//!
//! ## OPS-CONTROL-02 proof matrix
//!
//! | Test   | What it proves                                                              |
//! |--------|-----------------------------------------------------------------------------|
//! | OC-20  | catalog has exactly 7 entries                                               |
//! | OC-21  | catalog contains request-mode-change with required fields                   |
//! | OC-22  | catalog contains cancel-mode-transition with required fields                |
//! | OC-23  | request-mode-change is enabled when not halted (no-DB state)                |
//! | OC-24  | cancel-mode-transition is disabled when no DB (backend_unavailable)         |
//! | OC-25  | change-system-mode does not appear in catalog                               |
//!
//! OC-01..OC-09, OC-20..OC-25 are pure in-process (no DB required).
//! OC-10..OC-11 require MQK_DATABASE_URL and are #[ignore].

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use mqk_daemon::{
    routes::build_router,
    state::{AppState, OperatorAuthMode},
};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_ops_control_oc01_oc02 \
             -- --include-ignored --test-threads 1"
        )
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to test DB");
    mqk_db::migrate(&pool).await.expect("run migrations");
    pool
}

const ENGINE_ID: &str = "mqk-daemon";

async fn cleanup_daemon_intents(pool: &sqlx::PgPool) {
    sqlx::query("delete from sys_restart_intent where engine_id = $1")
        .bind(ENGINE_ID)
        .execute(pool)
        .await
        .expect("cleanup sys_restart_intent");
}

/// POST /api/v1/ops/action with a JSON body; return (status, parsed body).
async fn post_action(
    router: axum::Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

/// GET /api/v1/ops/catalog; return parsed body.
async fn get_catalog(router: axum::Router) -> serde_json::Value {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/ops/catalog")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "catalog must return 200");
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn no_db_router() -> axum::Router {
    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ));
    build_router(st)
}

// ---------------------------------------------------------------------------
// OPS-CONTROL-01 tests (pure in-process)
// ---------------------------------------------------------------------------

/// OC-01: request-mode-change with missing target_mode → 400, disposition = "missing_target_mode".
#[tokio::test]
async fn oc_01_request_mode_change_missing_target_mode_returns_400() {
    let (status, j) = post_action(
        no_db_router(),
        serde_json::json!({ "action_key": "request-mode-change" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "OC-01: missing target_mode must return 400; body: {j}"
    );
    assert_eq!(
        j["accepted"], false,
        "OC-01: accepted must be false; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("missing_target_mode"),
        "OC-01: disposition must be missing_target_mode; body: {j}"
    );
    assert!(
        j["blockers"].as_array().is_some_and(|b| !b.is_empty()),
        "OC-01: blockers must be non-empty; body: {j}"
    );
}

/// OC-02: request-mode-change with invalid target_mode → 400, disposition = "invalid_target_mode".
#[tokio::test]
async fn oc_02_request_mode_change_invalid_target_mode_returns_400() {
    let (status, j) = post_action(
        no_db_router(),
        serde_json::json!({
            "action_key": "request-mode-change",
            "target_mode": "not-a-real-mode"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "OC-02: invalid target_mode must return 400; body: {j}"
    );
    assert_eq!(
        j["accepted"], false,
        "OC-02: accepted must be false; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("invalid_target_mode"),
        "OC-02: disposition must be invalid_target_mode; body: {j}"
    );
}

/// OC-03: request-mode-change same-mode (paper→paper) → 200, no_op, no DB write.
///
/// Proves that same-mode transitions are accepted honestly as a no-op rather
/// than rejected or silently ignored.
#[tokio::test]
async fn oc_03_request_mode_change_same_mode_returns_200_no_op() {
    let (status, j) = post_action(
        no_db_router(),
        // Default test state is Paper mode.
        serde_json::json!({
            "action_key": "request-mode-change",
            "target_mode": "paper"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "OC-03: same-mode transition must return 200; body: {j}"
    );
    assert_eq!(
        j["accepted"], true,
        "OC-03: same-mode transition must be accepted; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("no_op"),
        "OC-03: same-mode disposition must be no_op; body: {j}"
    );
    assert!(
        j["pending_restart_intent"].is_null(),
        "OC-03: pending_restart_intent must be null for no_op; body: {j}"
    );
    assert_eq!(
        j["audit"]["durable_db_write"], false,
        "OC-03: no DB write for same-mode; body: {j}"
    );
}

/// OC-04: request-mode-change → backtest is refused → 409, disposition = "blocked_refused".
#[tokio::test]
async fn oc_04_request_mode_change_refused_returns_409() {
    let (status, j) = post_action(
        no_db_router(),
        serde_json::json!({
            "action_key": "request-mode-change",
            "target_mode": "backtest"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "OC-04: refused transition must return 409; body: {j}"
    );
    assert_eq!(
        j["accepted"], false,
        "OC-04: refused transition must not be accepted; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("blocked_refused"),
        "OC-04: disposition must be blocked_refused; body: {j}"
    );
    assert!(
        j["blockers"].as_array().is_some_and(|b| !b.is_empty()),
        "OC-04: blockers must explain the refusal; body: {j}"
    );
    assert!(
        j["pending_restart_intent"].is_null(),
        "OC-04: refused transition must not write an intent; body: {j}"
    );
}

/// OC-05: request-mode-change → live-capital is fail_closed → 409,
/// disposition = "blocked_fail_closed".
#[tokio::test]
async fn oc_05_request_mode_change_fail_closed_returns_409() {
    let (status, j) = post_action(
        no_db_router(),
        serde_json::json!({
            "action_key": "request-mode-change",
            "target_mode": "live-capital"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "OC-05: fail_closed transition must return 409; body: {j}"
    );
    assert_eq!(
        j["accepted"], false,
        "OC-05: fail_closed transition must not be accepted; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("blocked_fail_closed"),
        "OC-05: disposition must be blocked_fail_closed; body: {j}"
    );
    assert!(
        j["pending_restart_intent"].is_null(),
        "OC-05: fail_closed transition must not write an intent; body: {j}"
    );
}

/// OC-06: request-mode-change admissible (paper→live-shadow) without a DB
/// → 503 SERVICE_UNAVAILABLE (fail-closed: cannot persist intent without DB).
#[tokio::test]
async fn oc_06_request_mode_change_admissible_no_db_returns_503() {
    let (status, j) = post_action(
        no_db_router(),
        serde_json::json!({
            "action_key": "request-mode-change",
            "target_mode": "live-shadow"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "OC-06: admissible transition without DB must return 503; body: {j}"
    );
    // The body uses RuntimeErrorResponse shape (not OperatorActionResponse).
    assert!(
        j["error"].as_str().is_some_and(|e| !e.is_empty()),
        "OC-06: 503 must include an error message; body: {j}"
    );
}

/// OC-07: cancel-mode-transition without a DB → 503 (fail-closed).
#[tokio::test]
async fn oc_07_cancel_mode_transition_no_db_returns_503() {
    let (status, j) = post_action(
        no_db_router(),
        serde_json::json!({ "action_key": "cancel-mode-transition" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "OC-07: cancel without DB must return 503; body: {j}"
    );
    assert!(
        j["error"].as_str().is_some_and(|e| !e.is_empty()),
        "OC-07: 503 must include an error message; body: {j}"
    );
}

/// OC-09: change-system-mode legacy path still returns 409 + ModeChangeGuidanceResponse
/// (compat guard — proves OPS-CONTROL-01 did not break the existing MT-09 behavior).
#[tokio::test]
async fn oc_09_change_system_mode_legacy_still_returns_409() {
    let (status, j) = post_action(
        no_db_router(),
        serde_json::json!({ "action_key": "change-system-mode" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "OC-09: change-system-mode legacy path must still return 409; body: {j}"
    );
    assert_eq!(
        j["transition_permitted"], false,
        "OC-09: transition_permitted must be false; body: {j}"
    );
    assert!(
        j["canonical_route"]
            .as_str()
            .is_some_and(|r| r.contains("mode-change-guidance")),
        "OC-09: canonical_route must reference mode-change-guidance; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// OPS-CONTROL-02 catalog tests (pure in-process)
// ---------------------------------------------------------------------------

/// OC-20: catalog has exactly 7 entries after OPS-CONTROL-02 expansion.
#[tokio::test]
async fn oc_20_catalog_has_exactly_7_entries() {
    let j = get_catalog(no_db_router()).await;
    let actions = j["actions"].as_array().expect("actions must be an array");
    assert_eq!(
        actions.len(),
        7,
        "OC-20: catalog must have exactly 7 entries; got: {:?}",
        actions
            .iter()
            .filter_map(|a| a["action_key"].as_str())
            .collect::<Vec<_>>()
    );
}

/// OC-21: catalog contains request-mode-change with all required fields.
#[tokio::test]
async fn oc_21_catalog_contains_request_mode_change_with_required_fields() {
    let j = get_catalog(no_db_router()).await;
    let actions = j["actions"].as_array().expect("actions must be an array");
    let entry = actions
        .iter()
        .find(|a| a["action_key"].as_str() == Some("request-mode-change"))
        .expect("OC-21: request-mode-change must be in catalog");

    assert!(
        entry["label"].is_string(),
        "OC-21: request-mode-change must have label"
    );
    assert!(
        entry["level"].is_number(),
        "OC-21: request-mode-change must have level"
    );
    assert!(
        entry["description"].is_string(),
        "OC-21: request-mode-change must have description"
    );
    assert!(
        entry["requires_reason"].is_boolean(),
        "OC-21: request-mode-change must have requires_reason"
    );
    assert!(
        entry["confirm_text"].is_string(),
        "OC-21: request-mode-change must have confirm_text"
    );
    assert!(
        entry["enabled"].is_boolean(),
        "OC-21: request-mode-change must have enabled"
    );
}

/// OC-22: catalog contains cancel-mode-transition with all required fields.
#[tokio::test]
async fn oc_22_catalog_contains_cancel_mode_transition_with_required_fields() {
    let j = get_catalog(no_db_router()).await;
    let actions = j["actions"].as_array().expect("actions must be an array");
    let entry = actions
        .iter()
        .find(|a| a["action_key"].as_str() == Some("cancel-mode-transition"))
        .expect("OC-22: cancel-mode-transition must be in catalog");

    assert!(
        entry["label"].is_string(),
        "OC-22: cancel-mode-transition must have label"
    );
    assert!(
        entry["level"].is_number(),
        "OC-22: cancel-mode-transition must have level"
    );
    assert!(
        entry["description"].is_string(),
        "OC-22: cancel-mode-transition must have description"
    );
    assert!(
        entry["enabled"].is_boolean(),
        "OC-22: cancel-mode-transition must have enabled"
    );
}

/// OC-23: request-mode-change is enabled when not halted (no-DB test state).
#[tokio::test]
async fn oc_23_request_mode_change_enabled_when_not_halted() {
    let j = get_catalog(no_db_router()).await;
    let actions = j["actions"].as_array().expect("actions must be an array");
    let entry = actions
        .iter()
        .find(|a| a["action_key"].as_str() == Some("request-mode-change"))
        .expect("request-mode-change must be in catalog");

    assert_eq!(
        entry["enabled"], true,
        "OC-23: request-mode-change must be enabled when not halted; entry: {entry}"
    );
}

/// OC-24: cancel-mode-transition is disabled when no DB (no pending intent possible).
///
/// Also proves disabled_reason is present.
#[tokio::test]
async fn oc_24_cancel_mode_transition_disabled_when_no_db() {
    let j = get_catalog(no_db_router()).await;
    let actions = j["actions"].as_array().expect("actions must be an array");
    let entry = actions
        .iter()
        .find(|a| a["action_key"].as_str() == Some("cancel-mode-transition"))
        .expect("cancel-mode-transition must be in catalog");

    assert_eq!(
        entry["enabled"], false,
        "OC-24: cancel-mode-transition must be disabled when no DB; entry: {entry}"
    );
    assert!(
        entry["disabled_reason"].is_string(),
        "OC-24: cancel-mode-transition must have disabled_reason when no DB; entry: {entry}"
    );
}

/// OC-25: change-system-mode does not appear in catalog.
#[tokio::test]
async fn oc_25_change_system_mode_absent_from_catalog() {
    let j = get_catalog(no_db_router()).await;
    let actions = j["actions"].as_array().expect("actions must be an array");
    let keys: Vec<&str> = actions
        .iter()
        .filter_map(|a| a["action_key"].as_str())
        .collect();

    assert!(
        !keys.contains(&"change-system-mode"),
        "OC-25: change-system-mode must not appear in catalog; keys: {keys:?}"
    );
}

// ---------------------------------------------------------------------------
// DB-backed tests (require MQK_DATABASE_URL, #[ignore] in CI)
// ---------------------------------------------------------------------------

/// OC-10: request-mode-change admissible (paper→live-shadow) with DB →
/// 200 pending_restart, durable intent persisted, snapshot in response.
#[tokio::test]
#[ignore]
async fn oc_10_request_mode_change_admissible_persists_intent_in_db() {
    let pool = make_db_pool().await;
    cleanup_daemon_intents(&pool).await;

    let st = Arc::new(
        mqk_daemon::state::AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            mqk_daemon::state::DeploymentMode::Paper,
            mqk_daemon::state::BrokerKind::Paper,
        ),
    );
    let router = build_router(st);

    let (status, j) = post_action(
        router,
        serde_json::json!({
            "action_key": "request-mode-change",
            "target_mode": "live-shadow",
            "reason": "oc10 test intent"
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "OC-10: admissible transition with DB must return 200; body: {j}"
    );
    assert_eq!(
        j["accepted"], true,
        "OC-10: accepted must be true; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("pending_restart"),
        "OC-10: disposition must be pending_restart; body: {j}"
    );

    // Snapshot must be present and coherent.
    let pi = &j["pending_restart_intent"];
    assert!(
        !pi.is_null(),
        "OC-10: pending_restart_intent must not be null; body: {j}"
    );
    assert_eq!(
        pi["from_mode"].as_str(),
        Some("paper"),
        "OC-10: from_mode must be paper; body: {j}"
    );
    assert_eq!(
        pi["to_mode"].as_str(),
        Some("live-shadow"),
        "OC-10: to_mode must be live-shadow; body: {j}"
    );
    assert_eq!(
        pi["transition_verdict"].as_str(),
        Some("admissible_with_restart"),
        "OC-10: transition_verdict must be admissible_with_restart; body: {j}"
    );
    assert_eq!(
        pi["initiated_by"].as_str(),
        Some("operator"),
        "OC-10: initiated_by must be operator; body: {j}"
    );
    assert_eq!(
        pi["note"].as_str(),
        Some("oc10 test intent"),
        "OC-10: note must reflect provided reason; body: {j}"
    );

    // The audit field must confirm DB write.
    assert_eq!(
        j["audit"]["durable_db_write"], true,
        "OC-10: durable_db_write must be true; body: {j}"
    );
    assert!(
        j["audit"]["durable_targets"]
            .as_array()
            .is_some_and(|t| t.iter().any(|v| v.as_str() == Some("sys_restart_intent"))),
        "OC-10: durable_targets must include sys_restart_intent; body: {j}"
    );

    // Verify the intent is actually in the DB.
    let intent_id_str = pi["intent_id"]
        .as_str()
        .expect("intent_id must be a string");
    let pending = mqk_db::fetch_pending_restart_intent_for_engine(&pool, ENGINE_ID)
        .await
        .expect("fetch_pending must not error")
        .expect("OC-10: pending intent must exist in DB after request-mode-change");

    assert_eq!(
        pending.intent_id.to_string(),
        intent_id_str,
        "OC-10: DB intent_id must match response snapshot"
    );
    assert_eq!(pending.from_mode, "paper");
    assert_eq!(pending.to_mode, "live-shadow");
    assert_eq!(pending.transition_verdict, "admissible_with_restart");
    assert_eq!(pending.initiated_by, "operator");
    assert_eq!(pending.status, "pending");
}

/// OC-11: cancel-mode-transition with an existing pending intent → 200
/// intent_cancelled, intent removed from pending view.
#[tokio::test]
#[ignore]
async fn oc_11_cancel_mode_transition_cancels_pending_intent() {
    use chrono::{TimeZone, Utc};

    let pool = make_db_pool().await;
    cleanup_daemon_intents(&pool).await;

    // Seed a pending intent directly.
    let intent_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"oc11-test-intent");
    mqk_db::insert_restart_intent(
        &pool,
        &mqk_db::NewRestartIntent {
            intent_id,
            engine_id: ENGINE_ID.to_string(),
            from_mode: "paper".to_string(),
            to_mode: "live-shadow".to_string(),
            transition_verdict: "admissible_with_restart".to_string(),
            initiated_by: "operator".to_string(),
            initiated_at_utc: Utc.timestamp_opt(1_700_000_000, 0).single().unwrap(),
            note: "oc11 test".to_string(),
        },
    )
    .await
    .expect("seed intent");

    let st = Arc::new(
        mqk_daemon::state::AppState::new_for_test_with_db_mode_and_broker(
            pool.clone(),
            mqk_daemon::state::DeploymentMode::Paper,
            mqk_daemon::state::BrokerKind::Paper,
        ),
    );
    let router = build_router(st);

    let (status, j) = post_action(
        router,
        serde_json::json!({ "action_key": "cancel-mode-transition" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "OC-11: cancel-mode-transition must return 200; body: {j}"
    );
    assert_eq!(
        j["accepted"], true,
        "OC-11: accepted must be true; body: {j}"
    );
    assert_eq!(
        j["disposition"].as_str(),
        Some("intent_cancelled"),
        "OC-11: disposition must be intent_cancelled; body: {j}"
    );
    assert_eq!(
        j["audit"]["durable_db_write"], true,
        "OC-11: durable_db_write must be true; body: {j}"
    );

    // Verify the intent is no longer pending in the DB.
    let still_pending = mqk_db::fetch_pending_restart_intent_for_engine(&pool, ENGINE_ID)
        .await
        .expect("fetch_pending must not error");
    assert!(
        still_pending.is_none(),
        "OC-11: pending intent must be gone after cancel-mode-transition; got: {still_pending:?}"
    );
}
