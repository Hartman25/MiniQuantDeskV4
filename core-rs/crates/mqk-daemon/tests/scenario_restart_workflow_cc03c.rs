//! CC-03C: Controlled restart workflow wired to mounted truth — proof tests.
//!
//! Proves that GET /api/v1/ops/mode-change-guidance surfaces a mounted,
//! operator-visible `restart_workflow` field that:
//!   - is fail-closed when no DB is available (truth_state = "backend_unavailable")
//!   - honestly surfaces absence when no pending intent exists (truth_state = "no_pending")
//!   - surfaces a durable pending intent when one exists (truth_state = "active")
//!   - does not surface completed intents as pending
//!   - is coherent with CC-03A transition verdicts
//!
//! # Proof matrix
//!
//! | Test   | What it proves                                                              |
//! |--------|-----------------------------------------------------------------------------|
//! | RW-01  | No DB → GET guidance restart_workflow.truth_state = "backend_unavailable"  |
//! | RW-02  | DB, no pending intent → load_pending returns None → truth_state no_pending  |
//! | RW-03  | DB, pending intent present → load_pending returns Some → truth_state active |
//! | RW-04  | Completed intent → load_pending returns None → truth_state no_pending       |
//! | RW-05  | All 16 CC-03A verdict strings are in the DB CHECK constraint set            |
//!
//! RW-01 and RW-05 run unconditionally (no DB required).
//! RW-02 through RW-04 require MQK_DATABASE_URL and are marked #[ignore].
//! Run DB tests with:
//!
//! ```text
//! MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//! cargo test -p mqk-daemon --test scenario_restart_workflow_cc03c \
//!   -- --include-ignored --test-threads 1
//! ```

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use chrono::{TimeZone, Utc};
use mqk_daemon::{
    mode_transition::evaluate_mode_transition,
    routes::build_router,
    state::{AppState, BrokerKind, DeploymentMode, OperatorAuthMode},
};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_restart_workflow_cc03c \
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

fn fixed_ts(offset_secs: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + offset_secs, 0)
        .single()
        .expect("valid fixed timestamp")
}

/// The engine id used by load_pending_restart_intent — must match DAEMON_ENGINE_ID.
const ENGINE_ID: &str = "mqk-daemon";

/// Delete all sys_restart_intent rows for the daemon engine id.
/// Called at the start of each DB-backed test for isolation.
async fn cleanup_daemon_intents(pool: &sqlx::PgPool) {
    sqlx::query("delete from sys_restart_intent where engine_id = $1")
        .bind(ENGINE_ID)
        .execute(pool)
        .await
        .expect("cleanup sys_restart_intent");
}

/// Call GET /api/v1/ops/mode-change-guidance and return parsed body.
async fn get_guidance(st: Arc<AppState>) -> serde_json::Value {
    let router = build_router(st);
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/ops/mode-change-guidance")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "mode-change-guidance must return 200"
    );
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

// ---------------------------------------------------------------------------
// RW-01 (no DB): fail-closed — backend_unavailable without a DB pool
// ---------------------------------------------------------------------------

/// CC-03C / RW-01: Without a DB pool configured, GET /api/v1/ops/mode-change-guidance
/// must surface `restart_workflow.truth_state = "backend_unavailable"` and
/// `pending_intent = null`.
///
/// This proves the mounted seam is fail-closed: absence of a DB pool is
/// represented honestly rather than synthesising a "no pending intent" claim
/// that the daemon cannot actually support.
#[tokio::test]
async fn rw_01_no_db_restart_workflow_is_backend_unavailable() {
    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ));

    let j = get_guidance(st).await;

    let rw = &j["restart_workflow"];
    assert_eq!(
        rw["truth_state"], "backend_unavailable",
        "RW-01: no DB must produce truth_state=backend_unavailable; body: {j}"
    );
    assert!(
        rw["pending_intent"].is_null(),
        "RW-01: pending_intent must be null when backend_unavailable; body: {j}"
    );

    // Prove the field is always present — the operator surface must never
    // omit restart_workflow, regardless of DB availability.
    assert!(
        !rw.is_null(),
        "RW-01: restart_workflow must always be present in mode-change-guidance; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// RW-02 (DB): no_pending — honest absence when no pending intent exists
// ---------------------------------------------------------------------------

/// CC-03C / RW-02: When a DB pool is configured but no pending restart intent
/// exists for the daemon engine, `load_pending_restart_intent` returns `None`
/// and the mounted seam surfaces `truth_state = "no_pending"` with `pending_intent = null`.
///
/// This proves honest absence: the daemon does not synthesise a pending intent.
#[tokio::test]
#[ignore]
async fn rw_02_db_no_pending_intent_surfaces_no_pending() {
    let pool = make_db_pool().await;
    cleanup_daemon_intents(&pool).await;

    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Paper,
    ));

    // Call load_pending_restart_intent directly — this is the production seam
    // that build_mode_change_guidance delegates to.
    let intent = st.load_pending_restart_intent().await;
    assert!(
        intent.is_none(),
        "RW-02: no pending intent → load_pending_restart_intent must return None; got: {intent:?}"
    );

    // Verify the HTTP surface reflects this.
    let j = get_guidance(st).await;
    let rw = &j["restart_workflow"];
    assert_eq!(
        rw["truth_state"], "no_pending",
        "RW-02: DB with no pending intent must produce truth_state=no_pending; body: {j}"
    );
    assert!(
        rw["pending_intent"].is_null(),
        "RW-02: pending_intent must be null when no_pending; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// RW-03 (DB): active — pending intent is mounted with correct fields
// ---------------------------------------------------------------------------

/// CC-03C / RW-03: When a pending restart intent exists in `sys_restart_intent`
/// for the daemon engine, the mounted seam must surface `truth_state = "active"`
/// and `pending_intent` must reflect the durable record faithfully.
///
/// This proves the mounted surface is a real read of CC-03B durable truth, not
/// a synthetic or inferred state.
#[tokio::test]
#[ignore]
async fn rw_03_pending_intent_surfaced_in_restart_workflow() {
    let pool = make_db_pool().await;
    cleanup_daemon_intents(&pool).await;

    let intent_id = Uuid::new_v4();
    let ts = fixed_ts(0);

    mqk_db::insert_restart_intent(
        &pool,
        &mqk_db::NewRestartIntent {
            intent_id,
            engine_id: ENGINE_ID.to_string(),
            from_mode: "paper".to_string(),
            to_mode: "live-shadow".to_string(),
            transition_verdict: "admissible_with_restart".to_string(),
            initiated_by: "operator".to_string(),
            initiated_at_utc: ts,
            note: "rw03 test note".to_string(),
        },
    )
    .await
    .expect("insert restart intent");

    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Paper,
    ));

    let j = get_guidance(st).await;

    let rw = &j["restart_workflow"];
    assert_eq!(
        rw["truth_state"], "active",
        "RW-03: pending intent must produce truth_state=active; body: {j}"
    );

    let pi = &rw["pending_intent"];
    assert!(
        !pi.is_null(),
        "RW-03: pending_intent must not be null when active; body: {j}"
    );
    assert_eq!(
        pi["intent_id"],
        intent_id.to_string(),
        "RW-03: intent_id must match inserted record; body: {j}"
    );
    assert_eq!(pi["from_mode"], "paper", "RW-03: from_mode; body: {j}");
    assert_eq!(pi["to_mode"], "live-shadow", "RW-03: to_mode; body: {j}");
    assert_eq!(
        pi["transition_verdict"], "admissible_with_restart",
        "RW-03: transition_verdict; body: {j}"
    );
    assert_eq!(
        pi["initiated_by"], "operator",
        "RW-03: initiated_by; body: {j}"
    );
    assert_eq!(pi["note"], "rw03 test note", "RW-03: note; body: {j}");
}

// ---------------------------------------------------------------------------
// RW-04 (DB): completed intent is NOT surfaced as pending
// ---------------------------------------------------------------------------

/// CC-03C / RW-04: After an intent is transitioned to "completed",
/// `load_pending_restart_intent` returns `None` and the mounted seam surfaces
/// `truth_state = "no_pending"`.
///
/// This proves the mounted surface does not confuse historical completed
/// records with current workflow state.
#[tokio::test]
#[ignore]
async fn rw_04_completed_intent_not_surfaced_as_pending() {
    let pool = make_db_pool().await;
    cleanup_daemon_intents(&pool).await;

    let intent_id = Uuid::new_v4();
    let ts = fixed_ts(100);

    mqk_db::insert_restart_intent(
        &pool,
        &mqk_db::NewRestartIntent {
            intent_id,
            engine_id: ENGINE_ID.to_string(),
            from_mode: "paper".to_string(),
            to_mode: "live-shadow".to_string(),
            transition_verdict: "admissible_with_restart".to_string(),
            initiated_by: "operator".to_string(),
            initiated_at_utc: ts,
            note: "".to_string(),
        },
    )
    .await
    .expect("insert restart intent");

    let updated =
        mqk_db::update_restart_intent_status(&pool, intent_id, "completed", fixed_ts(200))
            .await
            .expect("update status");
    assert!(updated, "RW-04: update must affect a row");

    let st = Arc::new(AppState::new_for_test_with_db_mode_and_broker(
        pool,
        DeploymentMode::Paper,
        BrokerKind::Paper,
    ));

    let j = get_guidance(st).await;

    let rw = &j["restart_workflow"];
    assert_eq!(
        rw["truth_state"], "no_pending",
        "RW-04: completed intent must not appear as pending; body: {j}"
    );
    assert!(
        rw["pending_intent"].is_null(),
        "RW-04: pending_intent must be null after completion; body: {j}"
    );
}

// ---------------------------------------------------------------------------
// RW-05 (in-process): CC-03A coherence — all verdict strings are DB-legal
// ---------------------------------------------------------------------------

/// CC-03C / RW-05: All 16 (from, to) DeploymentMode combinations produce a
/// verdict string from `evaluate_mode_transition` that is in the set allowed
/// by the `sys_restart_intent.transition_verdict` DB CHECK constraint.
///
/// This is a pure in-process coherence proof requiring no DB.  It proves that
/// a restart intent created by any mode transition will always have a
/// DB-legal verdict string — the CC-03A seam and CC-03B schema are coherent.
#[test]
fn rw_05_all_cc03a_verdicts_are_db_legal_for_sys_restart_intent() {
    // The four strings allowed by the DB CHECK constraint in 0031_restart_intent.sql.
    let valid_db_verdicts = [
        "same_mode",
        "admissible_with_restart",
        "refused",
        "fail_closed",
    ];

    let modes = [
        DeploymentMode::Paper,
        DeploymentMode::LiveShadow,
        DeploymentMode::LiveCapital,
        DeploymentMode::Backtest,
    ];

    for from in modes {
        for to in modes {
            let verdict = evaluate_mode_transition(from, to);
            let v = verdict.as_str();
            assert!(
                valid_db_verdicts.contains(&v),
                "RW-05: evaluate_mode_transition({from:?},{to:?}) = '{v}' \
                 is not in the DB CHECK constraint set {valid_db_verdicts:?}"
            );
        }
    }

    // Spot-check: Paper→LiveShadow is admissible_with_restart (the primary
    // use-case for a durable restart intent).
    let paper_to_shadow =
        evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::LiveShadow);
    assert_eq!(
        paper_to_shadow.as_str(),
        "admissible_with_restart",
        "RW-05: Paper→LiveShadow must be admissible_with_restart"
    );
}
