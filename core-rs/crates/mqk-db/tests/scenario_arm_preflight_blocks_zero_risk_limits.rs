use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn arm_preflight_blocks_live_when_risk_limits_zero() -> Result<()> {
    // Skip if no DB configured (local + CI friendly).
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!("SKIP: MQK_DATABASE_URL not set");
        return Ok(());
    }

    let pool = mqk_db::testkit_db_pool().await?;
    mqk_db::migrate(&pool).await?;

    let run_id = Uuid::new_v4();

    // LIVE run: risk limits invalid (zero)
    let cfg = json!({
        "arming": { "require_clean_reconcile": false },
        "risk": {
            "daily_loss_limit": 0.0,
            "max_drawdown": 0.0,
            "flatten_on_critical": true,
            "reject_storm": { "window_seconds": 60, "max_rejects": 3 }
        }
    });

    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "MAIN".to_string(),
            mode: "LIVE".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "CFG".to_string(),
            config_json: cfg,
            host_fingerprint: "TEST|unit".to_string(),
        },
    )
    .await?;

    let err = mqk_db::arm_preflight(&pool, run_id).await.unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("risk") && msg.contains("zero"),
        "expected risk limits preflight failure, got: {msg}"
    );

    Ok(())
}
