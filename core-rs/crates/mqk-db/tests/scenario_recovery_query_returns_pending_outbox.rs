use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn recovery_query_returns_pending_outbox_for_run() -> anyhow::Result<()> {
    // Skip if no DB configured (local + CI friendly).
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("SKIP: MQK_DATABASE_URL not set");
            return Ok(());
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;

    mqk_db::migrate(&pool).await?;

    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "MAIN".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: json!({"x": 1}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    let k1 = format!("{run_id}_client_order_001");
    let k2 = format!("{run_id}_client_order_002");

    mqk_db::outbox_enqueue(&pool, run_id, &k1, json!({"sym":"SPY"})).await?;
    mqk_db::outbox_enqueue(&pool, run_id, &k2, json!({"sym":"QQQ"})).await?;

    // Claim k1 first (PENDING → CLAIMED), then mark SENT (CLAIMED → SENT).
    // Uses the L3 two-step dispatch protocol.
    let claimed = mqk_db::outbox_claim_batch(&pool, 1, "test-dispatcher").await?;
    assert_eq!(claimed.len(), 1, "must claim exactly one row");
    mqk_db::outbox_mark_sent(&pool, &k1).await?;

    let pending = mqk_db::outbox_list_unacked_for_run(&pool, run_id).await?;
    assert_eq!(pending.len(), 2, "expected 2 unacked outbox rows");
    assert!(pending.iter().any(|r| r.idempotency_key == k1));
    assert!(pending.iter().any(|r| r.idempotency_key == k2));

    Ok(())
}
