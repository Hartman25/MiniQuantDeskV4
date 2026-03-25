//! CC-01B-F1: Daemon route proof for /api/v1/strategy/summary against
//! authoritative active-fleet registry truth.
//!
//! Proves that after CC-01B the summary route:
//! - returns fail-closed "no_db" when the DB is unavailable
//! - returns authoritative "registry" truth (from postgres.sys_strategy_registry)
//!   when the DB is available, even when the registry is empty
//! - correctly surfaces enabled = true / false per registered strategy
//! - reflects the real daemon arm state in the `armed` field
//! - never returns the old placeholder "not_wired" truth_state
//!
//! No-DB tests run unconditionally.
//! DB-backed tests require MQK_DATABASE_URL and are marked #[ignore].
//! Run DB tests with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_strategy_summary_registry -- --include-ignored

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{routes, state};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared helpers — mirror the pattern in scenario_daemon_routes.rs
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

fn parse_json(b: bytes::Bytes) -> serde_json::Value {
    serde_json::from_slice(&b).expect("body is not valid JSON")
}

fn summary_request() -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/strategy/summary")
        .body(axum::body::Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// DB helper (for #[ignore] tests)
// ---------------------------------------------------------------------------

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_strategy_summary_registry -- --include-ignored"
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

/// Upsert a test registry entry with a caller-supplied enabled flag.
async fn seed_registry(pool: &sqlx::PgPool, strategy_id: &str, display_name: &str, enabled: bool) {
    let ts = chrono::Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: display_name.to_string(),
            enabled,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("seed_registry: upsert failed");
}

/// Generate a unique strategy_id for test isolation.
fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

/// Find a row in the summary JSON by strategy_id.
fn find_row<'a>(rows: &'a serde_json::Value, strategy_id: &str) -> Option<&'a serde_json::Value> {
    rows.as_array()?
        .iter()
        .find(|r| r["strategy_id"] == strategy_id)
}

// ---------------------------------------------------------------------------
// Test 1 (no DB): DB unavailable → fail-closed "no_db"
// ---------------------------------------------------------------------------

/// CC-01B-F1 / proof 1: DB unavailable → summary is fail-closed, not authoritative.
///
/// When no DB is present the route must return truth_state == "no_db" and
/// an empty rows array.  It must NOT imply "no active strategies" as an
/// authoritative statement — that would be a false claim.
#[tokio::test]
async fn summary_no_db_returns_fail_closed_no_db() {
    // AppState::new() has no DB pool.
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(st);

    let (status, body) = call(router, summary_request()).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["truth_state"], "no_db",
        "DB unavailable must return truth_state 'no_db', not 'not_wired' or 'active'"
    );
    assert_eq!(
        json["backend"], "postgres.sys_strategy_registry",
        "backend must identify the intended source even when unavailable"
    );
    assert_eq!(
        json["rows"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(usize::MAX),
        0,
        "rows must be empty when DB is unavailable"
    );
    // Must never claim the old placeholder state.
    assert_ne!(
        json["truth_state"], "not_wired",
        "no_db response must not regress to old 'not_wired' placeholder"
    );
    assert_ne!(
        json["truth_state"], "active",
        "no_db response must not claim 'active' truth"
    );
}

// ---------------------------------------------------------------------------
// Test 2 (DB): DB available + empty registry → authoritative empty
// ---------------------------------------------------------------------------

/// CC-01B-F1 / proof 2: DB available + empty registry → authoritative "registry" truth.
///
/// When the DB is available the route must use it as the source
/// (truth_state == "registry") regardless of whether the registry is empty.
/// An empty result from the DB is authoritative — it means no strategies are
/// registered, not that the source is unavailable.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn summary_with_db_uses_registry_truth_state() {
    let pool = make_db_pool().await;

    // Clear the registry so we start from a known empty baseline.
    sqlx::query("delete from sys_strategy_registry")
        .execute(&pool)
        .await
        .expect("truncate registry for test");

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(st);

    let (status, body) = call(router, summary_request()).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["truth_state"], "registry",
        "DB present must return truth_state 'registry'"
    );
    assert_eq!(
        json["backend"], "postgres.sys_strategy_registry",
        "backend must be postgres.sys_strategy_registry"
    );
    // Empty rows is authoritative when DB is available — it means no strategies
    // are registered, not that the registry is unavailable.
    assert_eq!(
        json["rows"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(usize::MAX),
        0,
        "empty registry must produce empty rows (authoritative empty, not error)"
    );
    // Must not regress to old placeholder states.
    assert_ne!(json["truth_state"], "not_wired");
    assert_ne!(json["truth_state"], "no_db");
}

// ---------------------------------------------------------------------------
// Test 3 (DB): enabled and disabled strategies both appear in summary rows
// ---------------------------------------------------------------------------

/// CC-01B-F1 / proof 3: registered+enabled and registered+disabled strategies
/// both appear in the summary rows with correct `enabled` values.
///
/// Disabled strategies must not be silently omitted — the summary reflects
/// the full registry so callers can distinguish active from inactive.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn summary_reflects_enabled_and_disabled_registry_entries() {
    let pool = make_db_pool().await;

    let id_enabled = unique_id("cc01bf1_on");
    let id_disabled = unique_id("cc01bf1_off");

    seed_registry(&pool, &id_enabled, "Enabled Strategy", true).await;
    seed_registry(&pool, &id_disabled, "Disabled Strategy", false).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(st);

    let (status, body) = call(router, summary_request()).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["truth_state"], "registry");

    let rows = &json["rows"];

    // Both strategies must appear regardless of enabled state.
    let enabled_row = find_row(rows, &id_enabled)
        .unwrap_or_else(|| panic!("enabled strategy '{id_enabled}' must appear in summary rows"));
    let disabled_row = find_row(rows, &id_disabled)
        .unwrap_or_else(|| panic!("disabled strategy '{id_disabled}' must appear in summary rows"));

    // enabled flag must reflect registry truth, not a hardcoded value.
    assert_eq!(
        enabled_row["enabled"], true,
        "registered+enabled strategy must have enabled: true"
    );
    assert_eq!(
        disabled_row["enabled"], false,
        "registered+disabled strategy must have enabled: false, not silently omitted"
    );
}

// ---------------------------------------------------------------------------
// Test 4 (DB): armed field reflects daemon integrity arm state
// ---------------------------------------------------------------------------

/// CC-01B-F1 / proof 4: `armed` in each summary row reflects the daemon's
/// current integrity arm state — not a per-strategy synthetic value.
///
/// A fresh AppState defaults to disarmed (execution blocked), so every row
/// must carry armed: false.  The field is global, not per-strategy.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn summary_armed_reflects_daemon_integrity_arm_state() {
    let pool = make_db_pool().await;

    let id = unique_id("cc01bf1_arm");
    seed_registry(&pool, &id, "Arm Test Strategy", true).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    // Default AppState: execution is blocked (disarmed) — no arm/run lifecycle
    // has been triggered.
    let router = routes::build_router(st);

    let (status, body) = call(router, summary_request()).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["truth_state"], "registry");

    let rows = &json["rows"];
    let row = find_row(rows, &id)
        .unwrap_or_else(|| panic!("strategy '{id}' must appear in summary rows"));

    // Fresh AppState is disarmed — armed must be false.
    // This proves the field reflects real integrity state, not synthetic truth.
    assert_eq!(
        row["armed"], false,
        "fresh AppState is disarmed; armed must be false, not a synthetic 'true'"
    );
    // enabled must also be correct.
    assert_eq!(row["enabled"], true);
}

// ---------------------------------------------------------------------------
// Test 5 (no DB): truth_state is never "not_wired" — no regression to placeholder
// ---------------------------------------------------------------------------

/// CC-01B-F1 / proof 5: the old "not_wired" / env-var placeholder path is gone.
///
/// Regardless of whether MQK_STRATEGY_IDS is set, the summary route must
/// never return truth_state == "not_wired".  The in-memory fleet snapshot
/// (which depended on that env var) is no longer the source of truth.
#[tokio::test]
async fn summary_never_returns_not_wired_truth_state() {
    // Test with MQK_STRATEGY_IDS explicitly absent (simulates unconfigured env).
    // No DB — so we get fail-closed "no_db", not the old "not_wired".
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(st);

    let (status, body) = call(router, summary_request()).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_ne!(
        json["truth_state"], "not_wired",
        "truth_state must never be 'not_wired' — that placeholder has been replaced \
         by 'no_db' (fail-closed) or 'registry' (authoritative)"
    );
    // The only valid states from the route are "no_db" and "registry".
    let ts = json["truth_state"].as_str().unwrap_or("");
    assert!(
        ts == "no_db" || ts == "registry",
        "truth_state must be 'no_db' or 'registry'; got: {ts}"
    );
}

// ---------------------------------------------------------------------------
// CC-01C: Row-level registry-derived field truth
// ---------------------------------------------------------------------------

/// CC-01C / proof 1: summary row fields are populated from authoritative registry truth.
///
/// display_name, kind, registered_at, and note must be sourced directly from
/// sys_strategy_registry — not invented, not synthetic, not empty defaults.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn summary_row_fields_sourced_from_registry() -> anyhow::Result<()> {
    let pool = make_db_pool().await;
    let ts = chrono::Utc::now();
    let id = unique_id("cc01c_fields");

    mqk_db::upsert_strategy_registry_entry(
        &pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: id.clone(),
            display_name: "My Momentum Strategy".to_string(),
            enabled: true,
            kind: "external_signal".to_string(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: "operator-supplied note".to_string(),
        },
    )
    .await?;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(st);

    let (status, body) = call(router, summary_request()).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["truth_state"], "registry");

    let row = find_row(&json["rows"], &id)
        .unwrap_or_else(|| panic!("strategy '{id}' must appear in summary rows"));

    // CC-01C: all non-null fields sourced from registry, not invented.
    assert_eq!(
        row["display_name"], "My Momentum Strategy",
        "display_name must match registry value"
    );
    assert_eq!(
        row["kind"], "external_signal",
        "kind must match registry value"
    );
    assert_eq!(row["enabled"], true, "enabled must match registry value");
    assert_eq!(
        row["note"], "operator-supplied note",
        "note must match registry value"
    );
    // registered_at must be a non-empty RFC3339 string.
    let reg_at = row["registered_at"].as_str().unwrap_or("");
    assert!(
        !reg_at.is_empty(),
        "registered_at must be a non-empty RFC3339 string"
    );
    assert!(
        reg_at.contains('T'),
        "registered_at must look like an RFC3339 timestamp; got: {reg_at}"
    );

    // Operational metrics remain honest null — no source exists yet.
    assert!(
        row["health_status"].is_null(),
        "health_status must be honest null"
    );
    assert!(
        row["universe_size"].is_null(),
        "universe_size must be honest null"
    );
    assert!(
        row["pending_intents"].is_null(),
        "pending_intents must be honest null"
    );
    assert!(
        row["open_positions"].is_null(),
        "open_positions must be honest null"
    );
    assert!(row["today_pnl"].is_null(), "today_pnl must be honest null");
    assert!(
        row["drawdown_pct"].is_null(),
        "drawdown_pct must be honest null"
    );
    assert!(row["regime"].is_null(), "regime must be honest null");
    assert!(
        row["throttle_state"].is_null(),
        "throttle_state must be honest null"
    );
    assert!(
        row["last_decision_time"].is_null(),
        "last_decision_time must be honest null"
    );

    Ok(())
}

/// CC-01C / proof 2: enabled and disabled strategies both surface correct row fields.
///
/// display_name and kind are present for both — the fields are not gated on enabled state.
/// enabled flag correctly distinguishes them.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn summary_row_fields_present_for_both_enabled_and_disabled() -> anyhow::Result<()> {
    let pool = make_db_pool().await;
    let ts = chrono::Utc::now();
    let id_on = unique_id("cc01c_on");
    let id_off = unique_id("cc01c_off");

    seed_registry(&pool, &id_on, "Active Strategy", true).await;
    mqk_db::upsert_strategy_registry_entry(
        &pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: id_off.clone(),
            display_name: "Inactive Strategy".to_string(),
            enabled: false,
            kind: "bar_driven".to_string(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await?;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(st);

    let (status, body) = call(router, summary_request()).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["truth_state"], "registry");

    let row_on = find_row(&json["rows"], &id_on)
        .unwrap_or_else(|| panic!("enabled strategy '{id_on}' must appear in rows"));
    let row_off = find_row(&json["rows"], &id_off)
        .unwrap_or_else(|| panic!("disabled strategy '{id_off}' must appear in rows"));

    // Both rows must carry display_name from registry, not empty/synthetic.
    assert_eq!(row_on["display_name"], "Active Strategy");
    assert_eq!(row_off["display_name"], "Inactive Strategy");

    // kind sourced correctly for both.
    assert_eq!(row_off["kind"], "bar_driven");

    // enabled flag distinguishes them.
    assert_eq!(row_on["enabled"], true);
    assert_eq!(row_off["enabled"], false);

    // note is empty string (not null) when none was set — honest empty, not synthetic.
    assert_eq!(row_off["note"], "");

    Ok(())
}

/// CC-01C / proof 3: no row is synthesized for unregistered strategies.
///
/// The summary only contains rows for strategies in sys_strategy_registry.
/// Strategies that were never registered must not appear — even as empty shells.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn summary_no_row_synthesized_for_unregistered_strategy() -> anyhow::Result<()> {
    let pool = make_db_pool().await;
    let ghost_id = unique_id("cc01c_ghost");

    // Do NOT insert ghost_id into the registry.

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(st);

    let (status, body) = call(router, summary_request()).await;
    let json = parse_json(body);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["truth_state"], "registry");

    assert!(
        find_row(&json["rows"], &ghost_id).is_none(),
        "unregistered strategy '{ghost_id}' must not appear in summary rows"
    );

    Ok(())
}

/// CC-01C / proof 4: kind is empty string (not null) for unclassified strategies.
///
/// sys_strategy_registry.kind defaults to ''.  The API must surface this as an
/// empty string, distinguishing "unclassified" from "null/unavailable".
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn summary_row_kind_empty_string_for_unclassified() -> anyhow::Result<()> {
    let pool = make_db_pool().await;
    let id = unique_id("cc01c_unclass");

    // Register with empty kind (unclassified).
    seed_registry(&pool, &id, "Unclassified Strategy", true).await;

    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let router = routes::build_router(st);

    let (status, body) = call(router, summary_request()).await;
    let json = parse_json(body);

    let row = find_row(&json["rows"], &id)
        .unwrap_or_else(|| panic!("strategy '{id}' must appear in summary rows"));

    // kind must be empty string (DB default), not null or synthetic.
    assert_eq!(
        row["kind"], "",
        "unclassified strategy must have kind='' (empty string), not null or a fabricated value"
    );
    assert!(
        !row["kind"].is_null(),
        "kind must never be null — empty string signals unclassified"
    );

    Ok(())
}
