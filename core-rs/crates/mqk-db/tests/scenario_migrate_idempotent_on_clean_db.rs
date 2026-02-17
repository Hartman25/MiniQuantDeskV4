/// PATCH 17: migrating twice on a clean DB must be idempotent.
///
/// DB-backed test, skipped if MQK_DATABASE_URL is not set.
#[tokio::test]
async fn migrate_idempotent_on_clean_db() -> anyhow::Result<()> {
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
    mqk_db::migrate(&pool).await?;

    Ok(())
}
