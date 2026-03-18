use anyhow::Result;

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn backtest_schema_tables_exist_after_migrate() -> Result<()> {
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

    let required_tables = [
        "runs",
        "audit_events",
        "oms_outbox",
        "oms_inbox",
        "md_bars",
        "run_events",
        "corporate_events",
        "symbol_gics",
        "md_quality_reports",
        "sys_arm_state",
        "broker_order_map",
        "sys_reconcile_checkpoint",
        "runtime_leader_lease",
        "runtime_control_state",
        "runtime_restart_requests",
        "broker_event_cursor",
        "sys_risk_block_state",
        "sys_reconcile_status_state",
        "sys_risk_denial_events",
    ];

    for table in required_tables {
        let exists: bool = sqlx::query_scalar(
            r#"
            select exists (
                select 1
                from information_schema.tables
                where table_schema = 'public'
                  and table_name = $1
            )
            "#,
        )
        .bind(table)
        .fetch_one(&pool)
        .await?;

        assert!(exists, "expected table to exist after migrate: {table}");
    }

    Ok(())
}
