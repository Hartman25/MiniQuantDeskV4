//! Patch D3 — DB-level uniqueness enforcement for broker_order_map.broker_id.
//!
//! Requires a live PostgreSQL instance reachable via MQK_DATABASE_URL.
//! All tests skip automatically when that variable is absent (CI without a DB).
//!
//! EB-4 note: Migration 0013 adds FK broker_order_map.internal_id →
//! oms_outbox(idempotency_key) ON DELETE RESTRICT. Each test now creates the
//! required run + outbox rows within the same transaction so the FK is
//! satisfied. Everything rolls back together — no persistent state left in the
//! shared DB.

use sqlx::PgPool;
use uuid::Uuid;

// Fixed UUIDs — deterministic, never collide with real runs.
const D3_UNIQ_RUN_ID: &str = "d3000001-0000-0000-0000-000000000000";
const D3_DIST_RUN_ID: &str = "d3000002-0000-0000-0000-000000000000";

fn is_unique_violation(err: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = err {
        db_err.code().as_deref() == Some("23505")
    } else {
        false
    }
}

fn db_url_or_panic() -> String {
    std::env::var("MQK_DATABASE_URL").unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"
        )
    })
}

async fn connect_and_migrate(db_url: &str) -> PgPool {
    let pool = PgPool::connect(db_url).await.expect("connect");
    mqk_db::migrate(&pool).await.expect("migrate");
    pool
}

/// A second mapping to the same broker_id must be rejected with SQLSTATE 23505.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn broker_order_map_rejects_duplicate_broker_id() {
    let db_url = db_url_or_panic();
    let pool = connect_and_migrate(&db_url).await;

    // Wrap in a transaction so test rows are never committed to the shared DB.
    let mut tx = pool.begin().await.expect("begin tx");

    // Parse fixed UUID (bind UUID type, not text).
    let run_id = Uuid::parse_str(D3_UNIQ_RUN_ID).expect("valid UUID constant");

    // EB-4: create a run so outbox rows satisfy the oms_outbox.run_id FK.
    sqlx::query(
        "INSERT INTO runs \
         (run_id, engine_id, mode, git_hash, config_hash, config_json, host_fingerprint) \
         VALUES ($1, 'd3-test', 'PAPER', 'd3', 'd3', '{}', 'd3') \
         ON CONFLICT (run_id) DO NOTHING",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await
    .expect("run insert should succeed");

    // EB-4: create outbox rows for both internal_ids so broker_order_map FK is satisfied.
    for key in ["d3-internal-001", "d3-internal-002"] {
        sqlx::query(
            "INSERT INTO oms_outbox (run_id, idempotency_key, order_json, status) \
             VALUES ($1, $2, '{}', 'PENDING') \
             ON CONFLICT (idempotency_key) DO NOTHING",
        )
        .bind(run_id)
        .bind(key)
        .execute(&mut *tx)
        .await
        .expect("outbox row insert should succeed");
    }

    // First internal_id mapped to broker-001 — must succeed.
    sqlx::query(
        "INSERT INTO broker_order_map (internal_id, broker_id) \
         VALUES ($1, $2)",
    )
    .bind("d3-internal-001")
    .bind("d3-broker-001")
    .execute(&mut *tx)
    .await
    .expect("first insert should succeed");

    // Second internal_id attempting the SAME broker_id — must be rejected.
    let err = sqlx::query(
        "INSERT INTO broker_order_map (internal_id, broker_id) \
         VALUES ($1, $2)",
    )
    .bind("d3-internal-002")
    .bind("d3-broker-001")
    .execute(&mut *tx)
    .await
    .expect_err("duplicate broker_id must be rejected");

    assert!(
        is_unique_violation(&err),
        "expected unique_violation (23505), got: {err:?}"
    );

    // Rollback — leave the DB clean regardless of outcome.
    let _ = tx.rollback().await;
}

/// Two mappings with distinct broker_ids must both succeed.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn broker_order_map_allows_distinct_broker_ids() {
    let db_url = db_url_or_panic();
    let pool = connect_and_migrate(&db_url).await;

    let mut tx = pool.begin().await.expect("begin tx");

    // Parse fixed UUID (bind UUID type, not text).
    let run_id = Uuid::parse_str(D3_DIST_RUN_ID).expect("valid UUID constant");

    // EB-4: create a run and outbox rows for both internal_ids.
    sqlx::query(
        "INSERT INTO runs \
         (run_id, engine_id, mode, git_hash, config_hash, config_json, host_fingerprint) \
         VALUES ($1, 'd3-test', 'PAPER', 'd3', 'd3', '{}', 'd3') \
         ON CONFLICT (run_id) DO NOTHING",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await
    .expect("run insert should succeed");

    for key in ["d3-pos-internal-001", "d3-pos-internal-002"] {
        sqlx::query(
            "INSERT INTO oms_outbox (run_id, idempotency_key, order_json, status) \
             VALUES ($1, $2, '{}', 'PENDING') \
             ON CONFLICT (idempotency_key) DO NOTHING",
        )
        .bind(run_id)
        .bind(key)
        .execute(&mut *tx)
        .await
        .expect("outbox row insert should succeed");
    }

    // First row — distinct broker_id, must succeed.
    sqlx::query(
        "INSERT INTO broker_order_map (internal_id, broker_id) \
         VALUES ($1, $2)",
    )
    .bind("d3-pos-internal-001")
    .bind("d3-pos-broker-001")
    .execute(&mut *tx)
    .await
    .expect("first distinct broker_id should succeed");

    // Second row — different broker_id, must also succeed.
    sqlx::query(
        "INSERT INTO broker_order_map (internal_id, broker_id) \
         VALUES ($1, $2)",
    )
    .bind("d3-pos-internal-002")
    .bind("d3-pos-broker-002")
    .execute(&mut *tx)
    .await
    .expect("second distinct broker_id should succeed");

    let _ = tx.rollback().await;
}
