use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn inbox_broker_message_id_dedupes_inserts() -> anyhow::Result<()> {
    // Skip if no DB configured (local + CI friendly).
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored");
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

    let msg_id = format!("BROKER_MSG_{}_{}", run_id, Uuid::new_v4());

    let inserted_1 =
        mqk_db::inbox_insert_deduped(&pool, run_id, &msg_id, json!({"fill":"one"})).await?;
    assert!(inserted_1, "expected first inbox insert to create row");

    let inserted_2 =
        mqk_db::inbox_insert_deduped(&pool, run_id, &msg_id, json!({"fill":"one"})).await?;
    assert!(
        !inserted_2,
        "expected second inbox insert to be deduped (no second row created)"
    );

    Ok(())
}
