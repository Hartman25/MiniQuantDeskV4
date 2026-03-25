//! CC-02A: Daemon-level proof tests for the strategy suppression write seam.
//!
//! Proves that `suppress_strategy` and `clear_suppression`:
//! - field validation rejects blank strategy_id / trigger_domain / trigger_reason
//! - no DB → disposition "unavailable"
//! - unregistered strategy → disposition "rejected"
//! - registered strategy (enabled or disabled) → disposition "suppressed"
//! - written suppression is visible through the canonical read seam
//!   (fetch_strategy_suppressions / GET /api/v1/strategy/suppressions)
//! - clearing an active suppression → disposition "cleared"
//! - clearing an already-cleared suppression → disposition "not_active"
//! - no DB on clear → disposition "unavailable"
//!
//! No-DB tests run unconditionally.
//! DB-backed tests require MQK_DATABASE_URL and are marked #[ignore].
//! Run DB tests with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_suppress_strategy -- --include-ignored

use std::sync::Arc;

use axum::http::Request;
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_daemon::{
    routes, state,
    suppression::{clear_suppression, suppress_strategy, SuppressStrategyArgs},
};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

fn make_args(
    suppression_id: Uuid,
    strategy_id: &str,
    trigger_domain: &str,
    trigger_reason: &str,
) -> SuppressStrategyArgs {
    SuppressStrategyArgs {
        suppression_id,
        strategy_id: strategy_id.to_string(),
        trigger_domain: trigger_domain.to_string(),
        trigger_reason: trigger_reason.to_string(),
        started_at_utc: Utc::now(),
        note: String::new(),
    }
}

// ---------------------------------------------------------------------------
// DB helper (for #[ignore] tests)
// ---------------------------------------------------------------------------

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_suppress_strategy -- --include-ignored"
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

async fn seed_registry(pool: &sqlx::PgPool, strategy_id: &str, enabled: bool) {
    let ts = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: format!("Test {strategy_id}"),
            enabled,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("seed_registry upsert failed");
}

async fn suppressions_response(st: Arc<state::AppState>) -> serde_json::Value {
    let router = routes::build_router(st);
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/suppressions")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("collect failed")
        .to_bytes();
    serde_json::from_slice(&body).expect("body is not valid JSON")
}

// ---------------------------------------------------------------------------
// Gate 0: field validation (no DB required)
// ---------------------------------------------------------------------------

/// CC-02A / Gate 0: blank strategy_id → rejected.
#[tokio::test]
async fn suppress_blank_strategy_id_rejected() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let mut args = make_args(Uuid::new_v4(), "", "operator", "test");
    args.strategy_id = String::new();
    let out = suppress_strategy(&st, args).await;

    assert!(!out.suppressed);
    assert_eq!(out.disposition, "rejected");
    assert!(
        out.blockers.iter().any(|b| b.contains("strategy_id")),
        "blockers must mention strategy_id; got: {:?}",
        out.blockers
    );
}

/// CC-02A / Gate 0: blank trigger_domain → rejected.
#[tokio::test]
async fn suppress_blank_trigger_domain_rejected() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let mut args = make_args(Uuid::new_v4(), "strat-x", "", "test");
    args.trigger_domain = "  ".to_string(); // whitespace-only
    let out = suppress_strategy(&st, args).await;

    assert!(!out.suppressed);
    assert_eq!(out.disposition, "rejected");
    assert!(
        out.blockers.iter().any(|b| b.contains("trigger_domain")),
        "blockers must mention trigger_domain; got: {:?}",
        out.blockers
    );
}

/// CC-02A / Gate 0: blank trigger_reason → rejected.
#[tokio::test]
async fn suppress_blank_trigger_reason_rejected() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let mut args = make_args(Uuid::new_v4(), "strat-x", "operator", "");
    args.trigger_reason = String::new();
    let out = suppress_strategy(&st, args).await;

    assert!(!out.suppressed);
    assert_eq!(out.disposition, "rejected");
    assert!(
        out.blockers.iter().any(|b| b.contains("trigger_reason")),
        "blockers must mention trigger_reason; got: {:?}",
        out.blockers
    );
}

// ---------------------------------------------------------------------------
// Gate 1: DB present (no DB required)
// ---------------------------------------------------------------------------

/// CC-02A / Gate 1: no DB → suppress returns unavailable.
#[tokio::test]
async fn suppress_no_db_returns_unavailable() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let args = make_args(Uuid::new_v4(), "strat-a", "operator", "test");
    let out = suppress_strategy(&st, args).await;

    assert!(!out.suppressed);
    assert_eq!(
        out.disposition, "unavailable",
        "no DB must return unavailable; got: {:?}",
        out.disposition
    );
}

/// CC-02A / Gate 1: no DB → clear returns unavailable.
#[tokio::test]
async fn clear_no_db_returns_unavailable() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let out = clear_suppression(&st, Uuid::new_v4(), Utc::now()).await;

    assert!(!out.cleared);
    assert_eq!(
        out.disposition, "unavailable",
        "no DB must return unavailable for clear; got: {:?}",
        out.disposition
    );
}

// ---------------------------------------------------------------------------
// Gate 2: registry check (DB required)
// ---------------------------------------------------------------------------

/// CC-02A / Gate 2: unregistered strategy → rejected.
///
/// A strategy_id that has no row in sys_strategy_registry must be refused
/// with "rejected", not "unavailable".  This is a deliberate identity failure,
/// not a transient system error.
#[tokio::test]
#[ignore]
async fn suppress_unregistered_strategy_rejected() {
    let pool = make_db_pool().await;
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let sid = unique_id("unreg");
    let args = make_args(Uuid::new_v4(), &sid, "operator", "should be refused");
    let out = suppress_strategy(&st, args).await;

    assert!(!out.suppressed);
    assert_eq!(
        out.disposition, "rejected",
        "unregistered strategy must be rejected; got: {:?}",
        out.disposition
    );
    assert!(
        out.blockers.iter().any(|b| b.contains("not registered")),
        "blocker must mention 'not registered'; got: {:?}",
        out.blockers
    );
}

/// CC-02A / Gate 2: registered + enabled strategy → accepted.
///
/// The primary happy path: a strategy registered and enabled in the registry
/// must produce disposition "suppressed" and be visible via the read seam.
#[tokio::test]
#[ignore]
async fn suppress_registered_enabled_strategy_accepted() {
    let pool = make_db_pool().await;
    let sid = unique_id("enabled");
    seed_registry(&pool, &sid, true).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let sup_id = Uuid::new_v4();
    let args = make_args(sup_id, &sid, "operator", "enabled strategy suppression");
    let out = suppress_strategy(&st, args).await;

    assert!(
        out.suppressed,
        "registered+enabled strategy must be suppressible; disposition={:?}, blockers={:?}",
        out.disposition, out.blockers
    );
    assert_eq!(out.disposition, "suppressed");
    assert_eq!(out.suppression_id, sup_id);
    assert!(out.blockers.is_empty());

    // Verify via direct DB read.
    let rows = mqk_db::fetch_strategy_suppressions(&pool)
        .await
        .expect("fetch failed");
    let row = rows
        .iter()
        .find(|r| r.suppression_id == sup_id)
        .expect("inserted suppression must appear in fetch");
    assert_eq!(row.state, "active");
    assert_eq!(row.strategy_id, sid);
}

/// CC-02A / Gate 2: registered + DISABLED strategy → accepted.
///
/// Disabled strategies are still registered identities.  Suppression is about
/// halting activity, not registration state.  A disabled strategy can be
/// suppressed so that if it is re-enabled later, suppression is already in
/// place.
#[tokio::test]
#[ignore]
async fn suppress_registered_disabled_strategy_accepted() {
    let pool = make_db_pool().await;
    let sid = unique_id("disabled");
    seed_registry(&pool, &sid, false).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let sup_id = Uuid::new_v4();
    let args = make_args(sup_id, &sid, "operator", "disabled strategy suppression");
    let out = suppress_strategy(&st, args).await;

    assert!(
        out.suppressed,
        "registered+disabled strategy must be suppressible; disposition={:?}, blockers={:?}",
        out.disposition, out.blockers
    );
    assert_eq!(out.disposition, "suppressed");
}

/// CC-02A / idempotency: submitting same suppression_id twice → both suppressed.
///
/// The underlying insert is ON CONFLICT DO NOTHING; re-submitting the same
/// suppression_id is silent and both calls return "suppressed".  The row
/// count remains one.
#[tokio::test]
#[ignore]
async fn suppress_same_id_twice_is_idempotent() {
    let pool = make_db_pool().await;
    let sid = unique_id("idem");
    seed_registry(&pool, &sid, true).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let sup_id = Uuid::new_v4();
    let first = suppress_strategy(&st, make_args(sup_id, &sid, "operator", "first")).await;
    assert_eq!(first.disposition, "suppressed");

    let second = suppress_strategy(&st, make_args(sup_id, &sid, "operator", "second")).await;
    assert_eq!(
        second.disposition, "suppressed",
        "idempotent re-submit must return suppressed, not an error"
    );

    let rows = mqk_db::fetch_strategy_suppressions(&pool)
        .await
        .expect("fetch failed");
    let count = rows.iter().filter(|r| r.suppression_id == sup_id).count();
    assert_eq!(
        count, 1,
        "exactly one suppression row must exist after two submits"
    );
}

// ---------------------------------------------------------------------------
// Read seam proof (DB required)
// ---------------------------------------------------------------------------

/// CC-02A / read seam: written suppression is visible via GET /api/v1/strategy/suppressions.
///
/// This is the canonical proof that the write seam and the mounted read route
/// see the same durable truth.
#[tokio::test]
#[ignore]
async fn suppressed_strategy_visible_through_route_read_seam() {
    let pool = make_db_pool().await;
    let sid = unique_id("read");
    seed_registry(&pool, &sid, true).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let sup_id = Uuid::new_v4();
    let out = suppress_strategy(&st, make_args(sup_id, &sid, "risk", "visible via route")).await;
    assert_eq!(
        out.disposition, "suppressed",
        "suppression must succeed before route check"
    );

    let json = suppressions_response(Arc::clone(&st)).await;
    assert_eq!(
        json["truth_state"], "active",
        "route must return truth_state='active' when DB is present"
    );

    let rows = json["rows"].as_array().expect("rows must be array");
    let row = rows
        .iter()
        .find(|r| r["suppression_id"].as_str() == Some(&sup_id.to_string()))
        .expect("inserted suppression must appear in route response");

    assert_eq!(row["strategy_id"], sid.as_str());
    assert_eq!(row["state"], "active");
    assert_eq!(row["trigger_domain"], "risk");
    assert_eq!(row["trigger_reason"], "visible via route");
    assert!(
        row["cleared_at"].is_null(),
        "active suppression must have null cleared_at in route response"
    );
}

// ---------------------------------------------------------------------------
// Clear path (DB required)
// ---------------------------------------------------------------------------

/// CC-02A / clear: clearing an active suppression → "cleared".
///
/// After `clear_suppression` returns "cleared", the row must have
/// state = 'cleared' and a non-null cleared_at_utc, and must remain
/// visible in the canonical read seam (audit trail).
#[tokio::test]
#[ignore]
async fn clear_active_suppression_transitions_to_cleared() {
    let pool = make_db_pool().await;
    let sid = unique_id("clr");
    seed_registry(&pool, &sid, true).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let sup_id = Uuid::new_v4();
    suppress_strategy(&st, make_args(sup_id, &sid, "operator", "will be cleared")).await;

    let cleared_at = Utc::now();
    let out = clear_suppression(&st, sup_id, cleared_at).await;

    assert!(
        out.cleared,
        "clearing active suppression must return cleared=true; disposition={:?}",
        out.disposition
    );
    assert_eq!(out.disposition, "cleared");
    assert_eq!(out.suppression_id, sup_id);
    assert!(out.blockers.is_empty());

    // Verify via DB.
    let rows = mqk_db::fetch_strategy_suppressions(&pool)
        .await
        .expect("fetch failed");
    let row = rows
        .iter()
        .find(|r| r.suppression_id == sup_id)
        .expect("cleared suppression must still appear in fetch");
    assert_eq!(row.state, "cleared");
    assert!(row.cleared_at_utc.is_some(), "cleared_at_utc must be set");
}

/// CC-02A / clear: clearing already-cleared suppression → "not_active".
///
/// A second clear on the same suppression_id must return "not_active" —
/// no error, just honest accounting that no active row was found.
#[tokio::test]
#[ignore]
async fn clear_already_cleared_suppression_returns_not_active() {
    let pool = make_db_pool().await;
    let sid = unique_id("dbl");
    seed_registry(&pool, &sid, true).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let sup_id = Uuid::new_v4();
    suppress_strategy(&st, make_args(sup_id, &sid, "operator", "double clear")).await;
    let first_clear = clear_suppression(&st, sup_id, Utc::now()).await;
    assert_eq!(first_clear.disposition, "cleared");

    let second_clear = clear_suppression(&st, sup_id, Utc::now()).await;
    assert!(!second_clear.cleared);
    assert_eq!(
        second_clear.disposition, "not_active",
        "second clear must return not_active, not an error"
    );
}

/// CC-02A / clear: clearing a never-inserted suppression_id → "not_active".
#[tokio::test]
#[ignore]
async fn clear_nonexistent_suppression_returns_not_active() {
    let pool = make_db_pool().await;
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let phantom = Uuid::new_v4();
    let out = clear_suppression(&st, phantom, Utc::now()).await;

    assert!(!out.cleared);
    assert_eq!(
        out.disposition, "not_active",
        "clearing unknown suppression_id must return not_active"
    );
}

/// CC-02A / route integration: cleared suppression appears as 'cleared' in route response.
///
/// After clearing, the row remains in the read seam with state = 'cleared'
/// so the operator audit trail is complete.
#[tokio::test]
#[ignore]
async fn cleared_suppression_visible_as_cleared_in_route() {
    let pool = make_db_pool().await;
    let sid = unique_id("route_clr");
    seed_registry(&pool, &sid, true).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let sup_id = Uuid::new_v4();
    suppress_strategy(&st, make_args(sup_id, &sid, "operator", "route clear test")).await;
    clear_suppression(&st, sup_id, Utc::now()).await;

    let json = suppressions_response(Arc::clone(&st)).await;
    let rows = json["rows"].as_array().expect("rows must be array");
    let row = rows
        .iter()
        .find(|r| r["suppression_id"].as_str() == Some(&sup_id.to_string()))
        .expect("cleared suppression must appear in route response");

    assert_eq!(
        row["state"], "cleared",
        "state must be 'cleared' in route after clear"
    );
    assert!(
        !row["cleared_at"].is_null(),
        "cleared_at must be non-null after clear"
    );
}
