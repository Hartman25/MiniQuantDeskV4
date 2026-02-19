use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn outbox_idempotency_key_dedupes_inserts() -> anyhow::Result<()> {
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

    // Create a run.
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

    let idem = format!("{run_id}_client_order_001");

    // First insert should create the row.
    let created_1 =
        mqk_db::outbox_enqueue(&pool, run_id, &idem, json!({"symbol":"SPY","qty":1})).await?;
    assert!(created_1, "expected first enqueue to create outbox row");

    // Second insert with same key should be deduped (no second row).
    let created_2 =
        mqk_db::outbox_enqueue(&pool, run_id, &idem, json!({"symbol":"SPY","qty":1})).await?;
    assert!(
        !created_2,
        "expected second enqueue to be deduped (no second row created)"
    );

    let rows = mqk_db::outbox_fetch_by_idempotency_key(&pool, &idem).await?;
    assert!(rows.is_some(), "expected outbox row to exist");
    assert_eq!(rows.unwrap().idempotency_key, idem);

    Ok(())
}
