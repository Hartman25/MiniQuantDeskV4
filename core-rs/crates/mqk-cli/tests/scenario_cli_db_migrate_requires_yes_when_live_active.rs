use chrono::Utc;
use predicates::prelude::*;
use uuid::Uuid;

/// PATCH 17: `mqk db migrate` must refuse when there is a LIVE run in ARMED/RUNNING unless --yes.
///
/// DB-backed test, skipped if MQK_DATABASE_URL is not set.
#[allow(deprecated)]
#[tokio::test]
async fn cli_db_migrate_requires_yes_when_live_active() -> anyhow::Result<()> {
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
    mqk_db::migrate(&pool).await?;

    // Create a LIVE run, then arm it to make it "active".
    // IMPORTANT: unique engine_id avoids collisions with other tests / local runs.
    let run_id = Uuid::new_v4();
    let engine_id = format!("TEST_ENGINE_{}", Uuid::new_v4());

    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id,
            mode: "LIVE".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG_TEST".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;
    mqk_db::arm_run(&pool, run_id).await?;

    // Run CLI from core-rs/ so relative assumptions match.
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .canonicalize()?;
    let core_rs_dir = repo_root.join("core-rs");

    // Without --yes => must fail with refusal message.
    let mut cmd = assert_cmd::Command::cargo_bin("mqk-cli")?;
    cmd.current_dir(&core_rs_dir)
        .env(mqk_db::ENV_DB_URL, &url)
        .args(["db", "migrate"]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("REFUSING MIGRATE"));

    // With --yes => should succeed.
    let mut cmd2 = assert_cmd::Command::cargo_bin("mqk-cli")?;
    cmd2.current_dir(&core_rs_dir)
        .env(mqk_db::ENV_DB_URL, &url)
        .args(["db", "migrate", "--yes"]);
    cmd2.assert().success();

    // Cleanup: halt the run so we don't leave an active LIVE run behind.
    mqk_db::halt_run(&pool, run_id).await?;

    Ok(())
}
