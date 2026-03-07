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
        "outbox_orders",
        "inbox_fills",
        "broker_order_map",
        "arm_state",
        "reconcile_reports",
        "md_bars",
        "md_ingest_runs",
        "md_quality_reports",
        "backtest_runs",
        "backtest_fills",
        "backtest_equity_curve",
        "promotion_runs",
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
