use anyhow::Result;
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn forged_audit_event_cannot_satisfy_arming() -> Result<()> {
    if std::env::var(mqk_db::ENV_DB_URL).is_err() {
        eprintln!(
            "SKIP: requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"
        );
        return Ok(());
    }

    let pool = match mqk_db::testkit_db_pool().await {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("SKIP: unable to connect using MQK_DATABASE_URL: {e}");
            return Ok(());
        }
    };

    mqk_db::migrate(&pool).await?;

    let run_id = Uuid::new_v4();

    let cfg = json!({
        "arming": { "require_clean_reconcile": true },
        "risk": {
            "daily_loss_limit": 1000.0,
            "max_drawdown": 1000.0,
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

    // Insert a forged/non-authoritative event payload directly; this must not satisfy arming.
    sqlx::query(
        r#"
        insert into audit_log (run_id, ts_utc, event_type, payload_json)
        values ($1, now(), $2, $3)
        "#,
    )
    .bind(run_id)
    .bind("reconcile_clean")
    .bind(json!({
        "forged": true,
        "source": "test_direct_insert"
    }))
    .execute(&pool)
    .await?;

    let err = mqk_db::arm_preflight(&pool, run_id).await.unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("reconcile")
            || msg.contains("audit")
            || msg.contains("arming")
            || msg.contains("preflight"),
        "expected forged audit evidence to be rejected, got: {msg}"
    );

    Ok(())
}
