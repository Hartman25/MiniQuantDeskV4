use chrono::Utc;
use uuid::Uuid;

/// Ensures the run lifecycle state machine is enforced AND
/// that LIVE exclusivity (only one active LIVE run per engine) is enforced.
///
/// DB-backed test. Skips if MQK_DATABASE_URL is not set.
#[tokio::test]
async fn run_lifecycle_enforced_and_live_exclusive() -> anyhow::Result<()> {
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

<<<<<<< HEAD
    // Use a unique engine_id so the test doesn't collide with any existing runs in the same DB.
    // We still test exclusivity by creating two LIVE runs with the SAME engine_id inside this test.
    let engine_id = format!("TEST_ENGINE_{}", Uuid::new_v4());

    // --- Lifecycle enforcement ---
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: engine_id.clone(),
=======
    // IMPORTANT: Use a unique engine_id per test run so we never collide with
    // leftover rows in a developer DB.
    let engine = format!("MAIN_{}", Uuid::new_v4().simple());

    let run1 = Uuid::new_v4();
    let run2 = Uuid::new_v4();
    let run3 = Uuid::new_v4();

    // Insert run1 LIVE <engine> (status defaults to CREATED)
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: run1,
            engine_id: engine.clone(),
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

    // Insert run2 LIVE <engine> (allowed; not active yet)
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: run2,
            engine_id: engine.clone(),
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
>>>>>>> origin/claude/strange-burnell
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG_TEST".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    // begin without arm should fail
    let err = mqk_db::begin_run(&pool, run_id).await.unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("begin_run invalid state"),
        "expected begin_run invalid state; got: {msg}"
    );

    // heartbeat without running should fail
    let err = mqk_db::heartbeat_run(&pool, run_id).await.unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("heartbeat_run invalid state"),
        "expected heartbeat_run invalid state; got: {msg}"
    );

    // stop without armed/running should fail
    let err = mqk_db::stop_run(&pool, run_id).await.unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("stop_run invalid state"),
        "expected stop_run invalid state; got: {msg}"
    );

    // arm -> begin -> heartbeat -> stop should succeed
    mqk_db::arm_run(&pool, run_id).await?;
    mqk_db::begin_run(&pool, run_id).await?;
    mqk_db::heartbeat_run(&pool, run_id).await?;
    mqk_db::stop_run(&pool, run_id).await?;

    // --- LIVE exclusivity enforcement ---
    // Create two LIVE runs with SAME engine_id. Only one can be ARMED/RUNNING.
    let live1 = Uuid::new_v4();
    let live2 = Uuid::new_v4();

    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: live1,
            engine_id: engine_id.clone(),
            mode: "LIVE".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG_TEST".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id: live2,
            engine_id: engine_id.clone(),
            mode: "LIVE".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG_TEST".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    // Arm first LIVE run -> OK
    mqk_db::arm_run(&pool, live1).await?;

    // Arm second LIVE run -> must fail with unique active LIVE constraint
    let err = mqk_db::arm_run(&pool, live2).await.unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("unique active LIVE constraint"),
        "expected unique active LIVE constraint; got: {msg}"
    );

    // Cleanup: halt both live runs (sticky safe state).
    mqk_db::halt_run(&pool, live1).await?;
    mqk_db::halt_run(&pool, live2).await?;

    Ok(())
}
