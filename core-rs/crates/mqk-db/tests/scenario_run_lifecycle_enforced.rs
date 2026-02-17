use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn run_lifecycle_enforced_and_live_exclusive() -> anyhow::Result<()> {
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

    // NOTE: if your local DB has a sqlx migration checksum mismatch,
    // mqk_db::migrate() will fail. Fix by pointing MQK_DATABASE_URL at a fresh DB
    // or resetting your local dev DB (do NOT "edit applied migrations" in real use).
    mqk_db::migrate(&pool).await?;

    let run1 = Uuid::new_v4();
    let run2 = Uuid::new_v4();
    let run3 = Uuid::new_v4();

    // Insert run1 LIVE MAIN (status defaults to CREATED)
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: run1,
            engine_id: "MAIN".to_string(),
            mode: "LIVE".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG1".to_string(),
            config_json: json!({"x": 1}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    // CREATED -> ARMED -> RUNNING
    mqk_db::arm_run(&pool, run1).await?;
    mqk_db::begin_run(&pool, run1).await?;
    mqk_db::heartbeat_run(&pool, run1).await?;

    // Insert run2 LIVE MAIN (allowed; not active yet)
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: run2,
            engine_id: "MAIN".to_string(),
            mode: "LIVE".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG2".to_string(),
            config_json: json!({"x": 2}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    // Try to arm run2 while run1 is RUNNING => MUST FAIL (unique active LIVE per engine)
    let err = mqk_db::arm_run(&pool, run2).await.unwrap_err();
    let msg = format!("{err}");
    let msg_lc = msg.to_lowercase();
    assert!(
        msg.contains("uq_live_engine_active_run")
            || msg_lc.contains("duplicate")
            || msg_lc.contains("unique")
            || msg.contains("23505"),
        "expected unique active LIVE constraint; got: {msg}"
    );

    // Stop run1, then arming run2 should succeed
    mqk_db::stop_run(&pool, run1).await?;

    // PROVE stop worked (otherwise the unique constraint will still block run2)
    let r1 = mqk_db::fetch_run(&pool, run1).await?;
    assert_eq!(
        r1.status.as_str(),
        "STOPPED",
        "stop_run did not transition run1; status={}",
        r1.status.as_str()
    );

    // Now run2 can become active
    mqk_db::arm_run(&pool, run2).await?;
    mqk_db::begin_run(&pool, run2).await?;

    // Insert run3 and verify binding guard works
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: run3,
            engine_id: "EXP".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG3".to_string(),
            config_json: json!({"x": 3}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    let bind_err = mqk_db::assert_run_binding(&pool, run3, "EXP", "PAPER", "WRONG")
        .await
        .unwrap_err();
    assert!(format!("{bind_err}").contains("config_hash"));

    // Cleanup: don't leave an active LIVE run in the DB between local test runs.
    mqk_db::stop_run(&pool, run2).await?;

    Ok(())
}
