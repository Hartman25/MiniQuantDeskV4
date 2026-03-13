//! Scenario: Outbox ACK transition guard — DB-04
//!
//! Invariant under test:
//! - ACK is only legal from SENT.
//! - ACK from any non-SENT predecessor must fail explicitly.

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

async fn make_pool(url: &str) -> anyhow::Result<sqlx::PgPool> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(url)
        .await?;
    mqk_db::migrate(&pool).await?;
    Ok(pool)
}

async fn cleanup_outbox(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::query("delete from broker_order_map")
        .execute(pool)
        .await?;
    sqlx::query("delete from oms_outbox").execute(pool).await?;
    Ok(())
}

async fn make_run(pool: &sqlx::PgPool) -> anyhow::Result<Uuid> {
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "MAIN".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "DB04-TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;
    Ok(run_id)
}

fn require_db_url() -> String {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-db -- --include-ignored"
        ),
    }
}

#[tokio::test]
#[ignore]
async fn ack_from_sent_succeeds() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    cleanup_outbox(&pool).await?;

    let run_id = make_run(&pool).await?;
    let key = format!("{run_id}_ack_valid");

    mqk_db::outbox_enqueue(&pool, run_id, &key, json!({"symbol":"SPY","qty":1})).await?;

    sqlx::query("update oms_outbox set status = 'SENT' where idempotency_key = $1")
        .bind(&key)
        .execute(&pool)
        .await?;

    let acked = mqk_db::outbox_mark_acked(&pool, &key).await?;
    assert!(acked);

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &key)
        .await?
        .expect("outbox row must exist");
    assert_eq!(row.status, "ACKED");

    Ok(())
}

#[tokio::test]
#[ignore]
async fn ack_from_non_sent_fails_explicitly() -> anyhow::Result<()> {
    let pool = make_pool(&require_db_url()).await?;
    cleanup_outbox(&pool).await?;

    let run_id = make_run(&pool).await?;
    let key = format!("{run_id}_ack_invalid");

    mqk_db::outbox_enqueue(&pool, run_id, &key, json!({"symbol":"QQQ","qty":2})).await?;

    let err = mqk_db::outbox_mark_acked(&pool, &key)
        .await
        .expect_err("ACK from PENDING must fail explicitly");
    assert!(
        err.to_string().contains("invalid transition"),
        "unexpected error: {err:#}"
    );

    let row = mqk_db::outbox_fetch_by_idempotency_key(&pool, &key)
        .await?
        .expect("outbox row must exist");
    assert_eq!(row.status, "PENDING");

    Ok(())
}
