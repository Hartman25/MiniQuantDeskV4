/// PATCH A: Backtest/replay schema additions must exist after migrations.
///
/// DB-backed test, skipped if MQK_DATABASE_URL is not set.

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored"]
async fn backtest_schema_tables_exist_after_migrate() -> anyhow::Result<()> {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!("DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db -- --include-ignored");
        }
    };

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;

    mqk_db::migrate(&pool).await?;

    for table in ["md_bars", "run_events", "corporate_events", "symbol_gics"] {
        let (exists,): (bool,) = sqlx::query_as(
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

        assert!(exists, "expected table '{table}' to exist after migrate()");
    }

    Ok(())
}
