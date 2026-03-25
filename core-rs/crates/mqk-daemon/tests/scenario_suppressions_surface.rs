//! CC-02C: Mounted suppression surface proof.
//!
//! Proves the four surface-contract cases for GET /api/v1/strategy/suppressions,
//! using the canonical CC-02A write seam (suppress_strategy, clear_suppression)
//! rather than raw DB inserts — so the proof is end-to-end:
//!
//!   write seam → durable DB → mounted route truth surface
//!
//! # Surface contract
//!
//! | truth_state | meaning                                                        |
//! |-------------|----------------------------------------------------------------|
//! | `"no_db"`   | DB unavailable; rows is empty and NOT authoritative            |
//! | `"active"`  | DB present; rows are authoritative (empty = no suppressions)   |
//!
//! # Test plan
//!
//! 1. No DB → `"no_db"`, empty rows, correct backend/canonical_route fields
//!    (unconditional; proves fail-closed without any DB dependency)
//! 2. DB + no suppression written for a unique strategy → `"active"`, strategy
//!    does not appear in rows (proves DB presence yields authoritative truth,
//!    not a collapse to "no_db" or "unavailable")
//! 3. DB + `suppress_strategy()` → `"active"`, row appears with correct
//!    field values and state="active" (proves write seam → surface round-trip)
//! 4. DB + `suppress_strategy()` + `clear_suppression()` → row remains
//!    visible with state="cleared" and non-null cleared_at (proves cleared
//!    lifecycle is surfaced honestly, not hidden)
//!
//! No-DB tests run unconditionally.
//! DB-backed tests require MQK_DATABASE_URL and are marked #[ignore].
//! Run DB tests with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_suppressions_surface -- --include-ignored

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_daemon::{
    routes,
    state,
    suppression::{clear_suppression, suppress_strategy, SuppressStrategyArgs},
};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

async fn get_suppressions(st: Arc<state::AppState>) -> (StatusCode, serde_json::Value) {
    let router = routes::build_router(st);
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/suppressions")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot failed");
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("collect failed")
        .to_bytes();
    let json = serde_json::from_slice(&body).expect("body is not valid JSON");
    (status, json)
}

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_suppressions_surface -- --include-ignored"
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

async fn seed_registry(pool: &sqlx::PgPool, strategy_id: &str) {
    let ts = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: format!("Surface test {strategy_id}"),
            enabled: true,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("seed_registry failed");
}

fn find_row<'a>(
    rows: &'a [serde_json::Value],
    suppression_id: &Uuid,
) -> Option<&'a serde_json::Value> {
    rows.iter()
        .find(|r| r["suppression_id"].as_str() == Some(&suppression_id.to_string()))
}

// ---------------------------------------------------------------------------
// Case 1: No DB → fail-closed "no_db" (unconditional)
// ---------------------------------------------------------------------------

/// CC-02C / surface case 1: no DB → truth_state = "no_db".
///
/// Without a DB pool the mounted route must declare the source unavailable.
/// The response must carry the correct backend and canonical_route identity
/// so the caller can identify what source was attempted and failed.
///
/// "no_db" must NOT be confused with "no suppressions" — it means the truth
/// source is unavailable, not that suppressions are authoritatively absent.
#[tokio::test]
async fn surface_no_db_is_fail_closed() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let (status, json) = get_suppressions(st).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["truth_state"], "no_db",
        "no DB must return truth_state='no_db'; got: {json}"
    );
    assert_eq!(
        json["backend"], "postgres.sys_strategy_suppressions",
        "backend must identify the intended source even when unavailable; got: {json}"
    );
    assert_eq!(
        json["canonical_route"], "/api/v1/strategy/suppressions",
        "canonical_route must be self-declared; got: {json}"
    );
    assert!(
        json["rows"].as_array().is_some_and(|r| r.is_empty()),
        "rows must be empty when no_db (not authoritative empty — source unavailable); got: {json}"
    );
}

// ---------------------------------------------------------------------------
// Case 2: DB present + no suppression for strategy → authoritative "active"
// ---------------------------------------------------------------------------

/// CC-02C / surface case 2: DB present, no suppression written → truth_state = "active".
///
/// When a DB is present and no suppression exists for a given strategy,
/// the route must return truth_state = "active" — not "no_db".  This
/// distinguishes "source unavailable" from "source available, nothing suppressed".
///
/// The unique strategy_id is used to prove that the specific strategy
/// does not appear in the rows array, without relying on the table being
/// globally empty (which would be fragile in a shared test DB).
#[tokio::test]
#[ignore]
async fn surface_db_present_no_suppression_is_authoritative_active() {
    let pool = make_db_pool().await;
    let sid = unique_id("nosup");
    seed_registry(&pool, &sid).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let (status, json) = get_suppressions(st).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["truth_state"], "active",
        "DB present must return truth_state='active', not 'no_db'; got: {json}"
    );
    assert_eq!(
        json["backend"], "postgres.sys_strategy_suppressions",
        "backend must be declared on active path; got: {json}"
    );
    assert_eq!(
        json["canonical_route"], "/api/v1/strategy/suppressions",
        "canonical_route must be declared on active path; got: {json}"
    );

    // The specific strategy must not appear in the rows — it was never suppressed.
    let rows = json["rows"].as_array().expect("rows must be an array");
    let has_our_strategy = rows.iter().any(|r| r["strategy_id"] == sid.as_str());
    assert!(
        !has_our_strategy,
        "a strategy with no suppression must not appear in route rows; sid={sid}"
    );
}

// ---------------------------------------------------------------------------
// Case 3: suppress_strategy (write seam) → active row surfaces truthfully
// ---------------------------------------------------------------------------

/// CC-02C / surface case 3: write seam → surface round-trip for active suppression.
///
/// Calls suppress_strategy (the CC-02A write seam) and then reads the
/// mounted route.  Proves:
/// * truth_state = "active"
/// * the row appears with correct strategy_id, trigger_domain, trigger_reason
/// * state = "active"
/// * cleared_at is null (not yet cleared)
/// * the write seam and the mounted read surface see the same durable truth
#[tokio::test]
#[ignore]
async fn surface_active_suppression_reflects_write_seam() {
    let pool = make_db_pool().await;
    let sid = unique_id("active");
    seed_registry(&pool, &sid).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let sup_id = Uuid::new_v4();
    let write_out = suppress_strategy(
        &st,
        SuppressStrategyArgs {
            suppression_id: sup_id,
            strategy_id: sid.clone(),
            trigger_domain: "operator".to_string(),
            trigger_reason: "CC-02C surface proof".to_string(),
            started_at_utc: Utc::now(),
            note: "cc-02c-active".to_string(),
        },
    )
    .await;

    assert!(
        write_out.suppressed,
        "suppress_strategy must succeed before surface check; disposition={:?}, blockers={:?}",
        write_out.disposition,
        write_out.blockers
    );

    let (status, json) = get_suppressions(Arc::clone(&st)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["truth_state"], "active",
        "truth_state must be 'active' when DB is present with a written suppression; got: {json}"
    );

    let rows = json["rows"].as_array().expect("rows must be array");
    let row = find_row(rows, &sup_id)
        .expect("written suppression must appear in mounted route surface");

    assert_eq!(row["strategy_id"], sid.as_str(), "strategy_id must match canonical identity");
    assert_eq!(row["state"], "active", "state must be 'active' before clearing");
    assert_eq!(row["trigger_domain"], "operator");
    assert_eq!(row["trigger_reason"], "CC-02C surface proof");
    assert!(
        row["cleared_at"].is_null(),
        "cleared_at must be null for an active suppression; got: {row}"
    );
}

// ---------------------------------------------------------------------------
// Case 4: suppress + clear (lifecycle) → cleared row surfaces honestly
// ---------------------------------------------------------------------------

/// CC-02C / surface case 4: full suppression lifecycle surfaces truthfully.
///
/// After suppress_strategy + clear_suppression, the row must remain visible
/// in the mounted surface with state = "cleared" and a non-null cleared_at.
///
/// This proves:
/// * cleared state is surfaced honestly (not hidden after clearing)
/// * cleared_at timestamp is populated and surfaced
/// * truth_state remains "active" (DB available) even after a lifecycle transition
/// * the operator audit trail is complete: suppress → active → cleared
#[tokio::test]
#[ignore]
async fn surface_cleared_suppression_reflects_lifecycle() {
    let pool = make_db_pool().await;
    let sid = unique_id("lifecycle");
    seed_registry(&pool, &sid).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    let sup_id = Uuid::new_v4();

    // Step 1: create the suppression.
    let suppress_out = suppress_strategy(
        &st,
        SuppressStrategyArgs {
            suppression_id: sup_id,
            strategy_id: sid.clone(),
            trigger_domain: "risk".to_string(),
            trigger_reason: "CC-02C lifecycle proof".to_string(),
            started_at_utc: Utc::now(),
            note: String::new(),
        },
    )
    .await;
    assert!(
        suppress_out.suppressed,
        "suppress_strategy must succeed; disposition={:?}",
        suppress_out.disposition
    );

    // Verify active state in surface before clearing.
    let (_, before_json) = get_suppressions(Arc::clone(&st)).await;
    let before_rows = before_json["rows"].as_array().expect("rows must be array");
    let before_row = find_row(before_rows, &sup_id)
        .expect("suppression must be visible before clear");
    assert_eq!(
        before_row["state"], "active",
        "state must be 'active' before clearing; got: {before_row}"
    );
    assert!(
        before_row["cleared_at"].is_null(),
        "cleared_at must be null before clearing; got: {before_row}"
    );

    // Step 2: clear the suppression.
    let clear_out = clear_suppression(&st, sup_id, Utc::now()).await;
    assert!(
        clear_out.cleared,
        "clear_suppression must succeed; disposition={:?}",
        clear_out.disposition
    );

    // Step 3: verify cleared state in surface.
    let (status, after_json) = get_suppressions(Arc::clone(&st)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        after_json["truth_state"], "active",
        "truth_state must remain 'active' after clearing (DB still available); got: {after_json}"
    );

    let after_rows = after_json["rows"].as_array().expect("rows must be array");
    let after_row = find_row(after_rows, &sup_id)
        .expect("cleared suppression must remain visible in surface (audit trail)");

    assert_eq!(
        after_row["state"], "cleared",
        "state must be 'cleared' in surface after clear_suppression; got: {after_row}"
    );
    assert!(
        !after_row["cleared_at"].is_null(),
        "cleared_at must be non-null after clearing; got: {after_row}"
    );
    assert_eq!(
        after_row["strategy_id"], sid.as_str(),
        "strategy_id must remain aligned with canonical identity after lifecycle"
    );
}
