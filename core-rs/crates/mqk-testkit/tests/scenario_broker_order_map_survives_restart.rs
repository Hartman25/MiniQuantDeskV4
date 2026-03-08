//! Scenario: Broker order-ID mapping survives simulated crash — Patch A4 / 3C-1
//!
//! # Invariants under test
//!
//! 1. `broker_map_upsert` persists an `internal_id → broker_id` pair to DB
//!    immediately after a successful broker submit.
//! 2. After a simulated restart (pool drop + reconnect), `broker_map_load`
//!    recovers all live mappings — cancel/replace can still locate the right
//!    broker order ID.
//! 3. The recovered pairs can repopulate an in-memory `BrokerOrderMap` exactly
//!    as the daemon startup path would, and subsequent `broker_id()` lookups
//!    succeed.
//! 4. `broker_map_remove` deletes a terminal-state entry; a subsequent
//!    `broker_map_load` does not return the deleted entry.
//! 5. `broker_map_upsert` is idempotent — re-registering the same `internal_id`
//!    does not produce duplicate rows and overwrites with the latest `broker_id`.
//! 6. Patch 3C-1: restart recovery must preserve a NON-IDENTITY mapping.
//!    The recovered broker ID must remain distinct from the internal ID, proving
//!    post-restart cancel/replace lookup would target the broker order ID rather
//!    than silently falling back to the internal/client order ID.
//!
//! # EB-4 FK prerequisite
//!
//! Migration 0013 adds a FK: broker_order_map.internal_id →
//! oms_outbox(idempotency_key) ON DELETE RESTRICT. Tests must now create a
//! run + outbox entry for each idempotency key before calling
//! broker_map_upsert.
//!
//! Cleanup order:
//!   broker_map_remove → delete run
//!
//! `runs.run_id` deletion cascades to `oms_outbox` via `ON DELETE CASCADE`.
//!
//! # PROOF LANE
//!
//! This is a load-bearing institutional proof test. It MUST fail hard if
//! MQK_DATABASE_URL is absent or the DB is unreachable. Silent skip is not
//! acceptable — a skipped proof test is an unproven invariant.

use chrono::Utc;
use mqk_execution::BrokerOrderMap;
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

// Fixed UUIDs for the test runs — deterministic, never collide with real runs.
const RESTART_RUN_ID: &str = "a4a40001-0000-0000-0000-000000000000";
const IDEM_RUN_ID: &str = "a4a40002-0000-0000-0000-000000000000";

// ---------------------------------------------------------------------------
// PROOF LANE harness helpers — fail hard on absent or unreachable DB.
// ---------------------------------------------------------------------------

/// Panics with a clear message if MQK_DATABASE_URL is not set.
fn require_db_url() -> String {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "PROOF: MQK_DATABASE_URL is not set. \
             This is a load-bearing proof test and cannot be skipped. \
             Set MQK_DATABASE_URL to a live Postgres instance and re-run."
        ),
    }
}

/// Panics if the DB is unreachable.
async fn require_pool(url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(url)
        .await
        .unwrap_or_else(|e| panic!("PROOF: cannot connect to DB: {e}"))
}

// ---------------------------------------------------------------------------
// 1 + 2 + 3 + 4 + 6: full crash-restart roundtrip with NON-IDENTITY proof
// ---------------------------------------------------------------------------

#[tokio::test]
async fn broker_order_map_survives_simulated_restart() -> anyhow::Result<()> {
    let url = require_db_url();

    // --- Pre-crash session ---------------------------------------------------

    let pool_before = require_pool(&url).await;
    mqk_db::migrate(&pool_before).await?;

    let run_id: Uuid = RESTART_RUN_ID.parse().unwrap();

    // Isolate test rows so parallel runs do not interfere.
    // Order: broker_order_map first (FK RESTRICT), then run (cascades to outbox).
    sqlx::query("delete from broker_order_map where internal_id like 'a4-restart-%'")
        .execute(&pool_before)
        .await?;
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool_before)
        .await?;

    // EB-4: create run + outbox entries so broker_map_upsert satisfies the FK.
    mqk_db::insert_run(
        &pool_before,
        &mqk_db::NewRun {
            run_id,
            engine_id: "eb4-test".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "eb4-test".to_string(),
            config_hash: "eb4-test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "eb4-test".to_string(),
        },
    )
    .await?;
    mqk_db::outbox_enqueue(
        &pool_before,
        run_id,
        "a4-restart-ord-1",
        serde_json::json!({}),
    )
    .await?;
    mqk_db::outbox_enqueue(
        &pool_before,
        run_id,
        "a4-restart-ord-2",
        serde_json::json!({}),
    )
    .await?;

    // Simulate two orders confirmed by the broker (post-submit registration).
    //
    // IMPORTANT (Patch 3C-1):
    // broker IDs MUST be distinct from internal IDs. If they are equal, restart
    // tests can pass while still hiding a real live-trading bug.
    mqk_db::broker_map_upsert(&pool_before, "a4-restart-ord-1", "broker-XYZ-001").await?;
    mqk_db::broker_map_upsert(&pool_before, "a4-restart-ord-2", "broker-XYZ-002").await?;

    // Simulate process crash: drop the connection pool.
    drop(pool_before);

    // --- Post-restart session ------------------------------------------------

    let pool_after = require_pool(&url).await;
    mqk_db::migrate(&pool_after).await?;

    // Recovery: load all persisted mappings (daemon startup path).
    let loaded = mqk_db::broker_map_load(&pool_after).await?;
    let test_pairs: Vec<_> = loaded
        .into_iter()
        .filter(|(id, _)| id.starts_with("a4-restart-"))
        .collect();

    assert_eq!(
        test_pairs.len(),
        2,
        "both mappings must survive the simulated restart"
    );

    // Repopulate the in-memory BrokerOrderMap exactly as daemon startup would.
    let mut order_map = BrokerOrderMap::new();
    for (internal_id, broker_id) in &test_pairs {
        order_map.register(internal_id.clone(), broker_id.clone());
    }

    // After restart, cancel/replace can locate the correct broker order ID.
    assert_eq!(
        order_map.broker_id("a4-restart-ord-1"),
        Some("broker-XYZ-001"),
        "cancel/replace must locate ord-1 broker ID after restart"
    );
    assert_eq!(
        order_map.broker_id("a4-restart-ord-2"),
        Some("broker-XYZ-002"),
        "cancel/replace must locate ord-2 broker ID after restart"
    );

    // Patch 3C-1: prove the mapping stays NON-IDENTITY after restart.
    //
    // This closes the weak proof where a paper/identity broker could make tests
    // pass even if live cancel/replace would accidentally target the internal ID.
    assert_ne!(
        "a4-restart-ord-1", "broker-XYZ-001",
        "test fixture must use non-identity broker IDs"
    );
    assert_ne!(
        "a4-restart-ord-2", "broker-XYZ-002",
        "test fixture must use non-identity broker IDs"
    );

    let recovered_1 = order_map.broker_id("a4-restart-ord-1");
    let recovered_2 = order_map.broker_id("a4-restart-ord-2");

    assert_ne!(
        recovered_1,
        Some("a4-restart-ord-1"),
        "restart recovery must not fall back to internal_id for ord-1"
    );
    assert_ne!(
        recovered_2,
        Some("a4-restart-ord-2"),
        "restart recovery must not fall back to internal_id for ord-2"
    );

    // Terminal-state cleanup: orders filled → remove from map.
    mqk_db::broker_map_remove(&pool_after, "a4-restart-ord-1").await?;
    mqk_db::broker_map_remove(&pool_after, "a4-restart-ord-2").await?;

    // Subsequent load must not return the deleted entries.
    let after_remove = mqk_db::broker_map_load(&pool_after).await?;
    let still_present = after_remove
        .iter()
        .any(|(id, _)| id.starts_with("a4-restart-"));
    assert!(
        !still_present,
        "removed entries must not appear in broker_map_load after terminal state"
    );

    // EB-4 cleanup: broker_map rows are gone; now delete run (cascades to outbox).
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool_after)
        .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// 5: upsert is idempotent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn broker_map_upsert_is_idempotent() -> anyhow::Result<()> {
    let url = require_db_url();
    let pool = require_pool(&url).await;
    mqk_db::migrate(&pool).await?;

    let run_id: Uuid = IDEM_RUN_ID.parse().unwrap();

    // Pre-test cleanup: broker_map first (FK RESTRICT), then run (cascades to outbox).
    sqlx::query("delete from broker_order_map where internal_id = 'a4-idem-ord'")
        .execute(&pool)
        .await?;
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await?;

    // EB-4: create run + outbox entry so broker_map_upsert satisfies the FK.
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "eb4-test".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "eb4-test".to_string(),
            config_hash: "eb4-test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "eb4-test".to_string(),
        },
    )
    .await?;
    mqk_db::outbox_enqueue(&pool, run_id, "a4-idem-ord", serde_json::json!({})).await?;

    // First registration after submit.
    mqk_db::broker_map_upsert(&pool, "a4-idem-ord", "broker-first").await?;

    // Idempotent retry with the same broker_id must not error or duplicate.
    mqk_db::broker_map_upsert(&pool, "a4-idem-ord", "broker-first").await?;

    let loaded = mqk_db::broker_map_load(&pool).await?;
    let count = loaded.iter().filter(|(id, _)| id == "a4-idem-ord").count();
    assert_eq!(count, 1, "upsert must not create a duplicate row");

    // Broker re-assigns a new ID (rare but possible on re-submit) — must overwrite.
    mqk_db::broker_map_upsert(&pool, "a4-idem-ord", "broker-reassigned").await?;
    let loaded2 = mqk_db::broker_map_load(&pool).await?;
    let broker_id = loaded2
        .iter()
        .find(|(id, _)| id == "a4-idem-ord")
        .map(|(_, bid)| bid.as_str());
    assert_eq!(
        broker_id,
        Some("broker-reassigned"),
        "upsert must overwrite with the latest broker_id"
    );

    // Terminal-state cleanup.
    mqk_db::broker_map_remove(&pool, "a4-idem-ord").await?;

    // EB-4 cleanup: broker_map row gone; delete run (cascades to outbox).
    sqlx::query("delete from runs where run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await?;

    Ok(())
}
