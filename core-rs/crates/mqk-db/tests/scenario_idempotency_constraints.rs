//! Patch D3 — DB-level uniqueness enforcement for broker_order_map.broker_id.
//!
//! Requires a live PostgreSQL instance reachable via MQK_DATABASE_URL.
//! All tests skip automatically when that variable is absent (CI without a DB).

use sqlx::PgPool;

fn is_unique_violation(err: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = err {
        db_err.code().as_deref() == Some("23505")
    } else {
        false
    }
}

/// A second mapping to the same broker_id must be rejected with SQLSTATE 23505.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn broker_order_map_rejects_duplicate_broker_id() {
    let db_url = match std::env::var("MQK_DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored");
        }
    };

    let pool = PgPool::connect(&db_url).await.expect("connect");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrate");

    // Wrap in a transaction so test rows are never committed to the shared DB.
    let mut tx = pool.begin().await.expect("begin tx");

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
    let db_url = match std::env::var("MQK_DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored");
        }
    };

    let pool = PgPool::connect(&db_url).await.expect("connect");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrate");

    let mut tx = pool.begin().await.expect("begin tx");

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
