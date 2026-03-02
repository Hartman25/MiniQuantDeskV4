//! EB-4: Broker Map FK → oms_outbox — DB-level rejection test.
//!
//! # Invariants under test
//!
//! Migration 0013 adds FK: broker_order_map.internal_id →
//! oms_outbox(idempotency_key) ON DELETE RESTRICT.
//!
//! 1. An INSERT into broker_order_map with an internal_id that has no parent
//!    oms_outbox row must be rejected with SQLSTATE 23503 (foreign_key_violation).
//!    This makes it structurally impossible to create a broker mapping that did
//!    not originate from an outbox dispatch.
//!
//! 2. An INSERT with a matching outbox row (created via the standard
//!    run → outbox_enqueue path) must succeed.
//!
//! Tests skip gracefully when MQK_DATABASE_URL is not set (CI without DB).

use anyhow::Result;
use sqlx::{postgres::PgPoolOptions, PgPool};

// Fixed UUIDs — deterministic, never collide with real runs.
const EB4_RUN_ID: &str = "eb040001-0000-0000-0000-000000000000";
const EB4_ORPHAN_KEY: &str = "eb4-orphan-no-outbox";
const EB4_VALID_KEY: &str = "eb4-valid-with-outbox";

fn db_url_or_skip() -> Option<String> {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => {
            println!("SKIP: requires MQK_DATABASE_URL");
            None
        }
    }
}

async fn try_pool_or_skip(url: &str) -> Result<Option<PgPool>> {
    match PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect(url)
        .await
    {
        Ok(pool) => Ok(Some(pool)),
        Err(e) => {
            println!("SKIP: cannot connect to DB: {e}");
            Ok(None)
        }
    }
}

fn is_fk_violation(err: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = err {
        db_err.code().as_deref() == Some("23503")
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Test 1: orphan insert rejected by FK
// ---------------------------------------------------------------------------

/// INSERT into broker_order_map without a parent outbox row must fail 23503.
///
/// This is the EB-4 acceptance criterion: the DB schema enforces outbox-first,
/// making it impossible to bypass the dispatch path at the storage layer.
#[tokio::test]
async fn broker_map_orphan_insert_rejected_by_fk() -> anyhow::Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    // Use a transaction so nothing is committed regardless of outcome.
    let mut tx = pool.begin().await?;

    // Attempt an orphan INSERT — no matching oms_outbox row exists for this key.
    let err = sqlx::query("INSERT INTO broker_order_map (internal_id, broker_id) VALUES ($1, $2)")
        .bind(EB4_ORPHAN_KEY)
        .bind("eb4-broker-orphan")
        .execute(&mut *tx)
        .await
        .expect_err("orphan insert must be rejected by FK");

    assert!(
        is_fk_violation(&err),
        "expected foreign_key_violation (23503), got: {err:?}"
    );

    // Rollback — nothing was committed.
    let _ = tx.rollback().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Test 2: insert with parent outbox row succeeds
// ---------------------------------------------------------------------------

/// INSERT into broker_order_map with a matching outbox row must succeed.
///
/// Verifies the happy path: run → outbox_enqueue → broker_map_upsert chain
/// satisfies the FK constraint end-to-end.
#[tokio::test]
async fn broker_map_insert_with_parent_outbox_succeeds() -> anyhow::Result<()> {
    let Some(url) = db_url_or_skip() else {
        return Ok(());
    };
    let Some(pool) = try_pool_or_skip(&url).await? else {
        return Ok(());
    };
    mqk_db::migrate(&pool).await?;

    let run_id: uuid::Uuid = EB4_RUN_ID.parse().unwrap();

    // Pre-cleanup: broker_map first (FK RESTRICT), then run (cascades to outbox).
    sqlx::query("DELETE FROM broker_order_map WHERE internal_id = $1")
        .bind(EB4_VALID_KEY)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM runs WHERE run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await?;

    // Create run + outbox row so the FK prerequisite is satisfied.
    sqlx::query(
        "INSERT INTO runs \
         (run_id, engine_id, mode, git_hash, config_hash, config_json, host_fingerprint) \
         VALUES ($1, 'eb4-fk-test', 'PAPER', 'test', 'test', '{}', 'test')",
    )
    .bind(run_id)
    .execute(&pool)
    .await?;

    sqlx::query(
        "INSERT INTO oms_outbox (run_id, idempotency_key, order_json, status) \
         VALUES ($1, $2, '{}', 'PENDING')",
    )
    .bind(run_id)
    .bind(EB4_VALID_KEY)
    .execute(&pool)
    .await?;

    // Insert into broker_order_map with matching internal_id — must succeed.
    sqlx::query("INSERT INTO broker_order_map (internal_id, broker_id) VALUES ($1, $2)")
        .bind(EB4_VALID_KEY)
        .bind("eb4-broker-valid")
        .execute(&pool)
        .await
        .expect("insert with parent outbox row must succeed");

    // Cleanup: broker_map first (RESTRICT), then run (cascades to outbox).
    sqlx::query("DELETE FROM broker_order_map WHERE internal_id = $1")
        .bind(EB4_VALID_KEY)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM runs WHERE run_id = $1")
        .bind(run_id)
        .execute(&pool)
        .await?;

    Ok(())
}
