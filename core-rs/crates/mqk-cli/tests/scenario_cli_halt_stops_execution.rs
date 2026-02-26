use chrono::Utc;
use uuid::Uuid;

/// PATCH 16: `mqk run halt` must transition a run to HALTED in the DB.
///
/// This test is DB-backed and is skipped if MQK_DATABASE_URL is not set.
#[allow(deprecated)]
#[tokio::test]
async fn cli_halt_transitions_run_to_halted() -> anyhow::Result<()> {
    // Skip if no DB configured (local + CI friendly).
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("SKIP: MQK_DATABASE_URL not set");
            return Ok(());
        }
    };

    let pool = match sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("SKIP: cannot connect to DB: {e}");
            return Ok(());
        }
    };
    if let Err(e) = mqk_db::migrate(&pool).await {
        eprintln!("SKIP: cannot migrate DB: {e}");
        return Ok(());
    }

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .canonicalize()?;
    let core_rs_dir = repo_root.join("core-rs");

    // Create a PAPER run directly (no need to invoke `run start` here).
    let run_id = Uuid::new_v4();
    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "MAIN".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG_TEST".to_string(),
            config_json: serde_json::json!({"arming": {"require_manual_confirmation": false}}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    // Halt via CLI.
    let mut cmd = assert_cmd::Command::cargo_bin("mqk-cli")?;
    cmd.current_dir(&core_rs_dir)
        .env(mqk_db::ENV_DB_URL, &url)
        .args([
            "run",
            "halt",
            "--run-id",
            &run_id.to_string(),
            "--reason",
            "unit_test",
        ]);
    cmd.assert().success();

    // Verify DB row transitioned.
    let r = mqk_db::fetch_run(&pool, run_id).await?;
    assert_eq!(r.status.as_str(), "HALTED");

    Ok(())
}
