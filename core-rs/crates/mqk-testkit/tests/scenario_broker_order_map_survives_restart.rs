//! Scenario: Broker order-ID mapping survives simulated crash — Patch A4
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
//!
//! Requires `MQK_DATABASE_URL`. Skips with a diagnostic message if absent.

use mqk_execution::BrokerOrderMap;

// ---------------------------------------------------------------------------
// 1 + 2 + 3 + 4: full crash-restart roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn broker_order_map_survives_simulated_restart() -> anyhow::Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("SKIP: MQK_DATABASE_URL not set");
            return Ok(());
        }
    };

    // --- Pre-crash session ---------------------------------------------------

    let pool_before = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await?;
    mqk_db::migrate(&pool_before).await?;

    // Isolate test rows so parallel runs do not interfere.
    sqlx::query("delete from broker_order_map where internal_id like 'a4-restart-%'")
        .execute(&pool_before)
        .await?;

    // Simulate two orders confirmed by the broker (post-submit registration).
    mqk_db::broker_map_upsert(&pool_before, "a4-restart-ord-1", "broker-XYZ-001").await?;
    mqk_db::broker_map_upsert(&pool_before, "a4-restart-ord-2", "broker-XYZ-002").await?;

    // Simulate process crash: drop the connection pool.
    drop(pool_before);

    // --- Post-restart session ------------------------------------------------

    let pool_after = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await?;
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

    Ok(())
}

// ---------------------------------------------------------------------------
// 5: upsert is idempotent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn broker_map_upsert_is_idempotent() -> anyhow::Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("SKIP: MQK_DATABASE_URL not set");
            return Ok(());
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await?;
    mqk_db::migrate(&pool).await?;

    sqlx::query("delete from broker_order_map where internal_id = 'a4-idem-ord'")
        .execute(&pool)
        .await?;

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

    // Cleanup.
    mqk_db::broker_map_remove(&pool, "a4-idem-ord").await?;

    Ok(())
}
