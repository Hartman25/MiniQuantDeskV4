use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn arm_preflight_requires_clean_reconcile_when_configured() -> Result<()> {
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
    let engine_id = format!("TEST_ENGINE_{}", Uuid::new_v4());

    // Config requires clean reconcile + valid LIVE risk/kill-switch settings
    let cfg = json!({
        "arming": { "require_clean_reconcile": true },
        "risk": { "daily_loss_limit": 0.02, "max_drawdown": 0.18, "flatten_on_critical": true },
        "runtime": { "deadman_file": "deadman.txt" },
        "data": { "stale_policy": "DISARM" }
    });

    mqk_db::insert_run(
        &pool,
        &mqk_db::NewRun {
            run_id,
            engine_id,
            mode: "LIVE".to_string(),
            started_at_utc: Utc::now(),
            git_hash: "TEST".to_string(),
            config_hash: "TESTCFG".to_string(),
            config_json: cfg,
            host_fingerprint: "TEST|unit".to_string(),
        },
    )
    .await?;

    // No reconcile audit event yet => should fail
    let err = mqk_db::arm_preflight(&pool, run_id).await.unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("requires clean reconcile"),
        "expected reconcile gate failure; got: {msg}"
    );

    // Add CLEAN reconcile audit event
    mqk_db::insert_audit_event(
        &pool,
        &mqk_db::NewAuditEvent {
            event_id: Uuid::new_v4(),
            run_id,
            ts_utc: Utc::now(),
            topic: "reconcile".to_string(),
            event_type: "CLEAN".to_string(),
            payload: json!({"note":"ok"}),
            hash_prev: None,
            hash_self: Some("h1".to_string()),
        },
    )
    .await?;

    // Now should pass and arm
    mqk_db::arm_preflight(&pool, run_id).await?;
    let r = mqk_db::fetch_run(&pool, run_id).await?;
    assert_eq!(r.status.as_str(), "ARMED");

    // cleanup
    mqk_db::halt_run(&pool, run_id).await?;
    Ok(())
}
