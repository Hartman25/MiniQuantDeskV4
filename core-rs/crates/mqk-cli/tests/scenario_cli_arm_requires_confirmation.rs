use chrono::Utc;
use predicates::prelude::*;
use uuid::Uuid;

/// PATCH 16: `mqk run arm` must enforce manual confirmation for LIVE runs when configured.
///
/// This test is DB-backed and is skipped if MQK_DATABASE_URL is not set.
#[allow(deprecated)]
#[tokio::test]
async fn cli_arm_requires_confirmation_for_live() -> anyhow::Result<()> {
    // Skip if no DB configured (local + CI friendly).
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("SKIP: MQK_DATABASE_URL not set");
            return Ok(());
        }
    };

    // Attempt DB connection. If the caller's env points at an unreachable DB (or has placeholder
    // credentials), skip instead of failing the whole workspace test suite.
    let pool = match sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("SKIP: cannot connect to MQK_DATABASE_URL: {e}");
            return Ok(());
        }
    };
    mqk_db::migrate(&pool).await?;

    // Create a LIVE run with config_json containing arming requirements.
    // IMPORTANT: use a unique engine_id to avoid colliding with other active LIVE runs in the same DB.
    let run_id = Uuid::new_v4();
    let engine_id = format!("TEST_ENGINE_{}", Uuid::new_v4());

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .canonicalize()?;

    let base = repo_root.join("config/defaults/base.yaml");
    let engine = repo_root.join("config/engines/main.yaml");
    let risk = repo_root.join("config/risk_profiles/tier_A_consistent.yaml");

    let base_s = base.to_string_lossy().to_string();
    let engine_s = engine.to_string_lossy().to_string();
    let risk_s = risk.to_string_lossy().to_string();

    let loaded =
        mqk_config::load_layered_yaml(&[base_s.as_str(), engine_s.as_str(), risk_s.as_str()])?;

    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: engine_id.clone(),
            mode: "LIVE".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: loaded.config_hash,
            config_json: loaded.config_json,
            host_fingerprint: "TESTHOST".to_string(),
        },
    )
    .await?;

    mqk_db::insert_audit_event(
        &pool,
        &mqk_db::NewAuditEvent {
            event_id: Uuid::new_v4(),
            run_id,
            ts_utc: Utc::now(),
            topic: "reconcile".to_string(),
            event_type: "CLEAN".to_string(),
            payload: serde_json::json!({"note":"ok"}),
            hash_prev: None,
            hash_self: Some("h_cli".to_string()),
        },
    )
    .await?;

    // Run CLI from core-rs/ so relative paths match the binary's assumptions.
    let core_rs_dir = repo_root.join("core-rs");

    // Arm without --confirm must fail.
    let mut cmd = assert_cmd::Command::cargo_bin("mqk-cli")?;
    cmd.current_dir(&core_rs_dir)
        .env(mqk_db::ENV_DB_URL, &url)
        .args(["run", "arm", "--run-id", &run_id.to_string()]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("manual confirmation required"));

    // Arm with correct confirmation must succeed.
    let mut cmd2 = assert_cmd::Command::cargo_bin("mqk-cli")?;
    cmd2.current_dir(&core_rs_dir)
        .env(mqk_db::ENV_DB_URL, &url)
        .args([
            "run",
            "arm",
            "--run-id",
            &run_id.to_string(),
            "--confirm",
            "ARM LIVE 0000 0.02",
        ]);

    cmd2.assert().success();

    // Cleanup: do not leave an active LIVE run in the DB.
    mqk_db::halt_run(&pool, run_id).await?;
    Ok(())
}
