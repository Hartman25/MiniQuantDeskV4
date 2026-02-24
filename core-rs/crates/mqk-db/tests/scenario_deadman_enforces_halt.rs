use chrono::{Duration, Utc};
use uuid::Uuid;

/// PATCH 18: deadman enforcement must halt a RUNNING run when heartbeat is stale.
///
/// DB-backed test. Skips if MQK_DATABASE_URL is not set.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn deadman_enforce_halts_running_when_heartbeat_stale() -> anyhow::Result<()> {
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
    let engine_id = format!("TEST_ENGINE_{}", Uuid::new_v4());

    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id,
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG_TEST".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;

    // Set an old heartbeat manually (stale).
    let old = Utc::now() - Duration::seconds(3600);
    sqlx::query("update runs set last_heartbeat_utc = $1 where run_id = $2")
        .bind(old)
        .bind(run_id)
        .execute(&pool)
        .await?;

    // TTL 10 seconds => should expire.
    let halted = mqk_db::enforce_deadman_or_halt(&pool, run_id, 10).await?;
    assert!(halted, "expected deadman to halt the run");

    let r = mqk_db::fetch_run(&pool, run_id).await?;
    assert_eq!(r.status.as_str(), "HALTED");

    Ok(())
}
