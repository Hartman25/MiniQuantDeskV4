use anyhow::{Context, Result};
use sqlx::{postgres::PgPoolOptions, PgPool};

pub const ENV_DB_URL: &str = "MQK_DATABASE_URL";

/// Connect to Postgres using MQK_DATABASE_URL.
pub async fn connect_from_env() -> Result<PgPool> {
    let url = std::env::var(ENV_DB_URL)
        .with_context(|| format!("missing env var {ENV_DB_URL}"))?;

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .context("failed to connect to Postgres")?;

    Ok(pool)
}

/// Run embedded SQLx migrations.
pub async fn migrate(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .context("db migrate failed")?;
    Ok(())
}

/// Simple status query (connectivity + schema presence).
pub async fn status(pool: &PgPool) -> Result<DbStatus> {
    // Connectivity (Postgres returns `int4` for `select 1`)
    let (one,): (i32,) = sqlx::query_as("select 1")
        .fetch_one(pool)
        .await
        .context("status connectivity query failed")?;
    let ok = one == 1;

    // Check one core table exists
    let (exists,): (bool,) = sqlx::query_as(
        r#"
        select exists (
            select 1
            from information_schema.tables
            where table_schema='public' and table_name='runs'
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .context("status table-exists query failed")?;

    Ok(DbStatus { ok, has_runs_table: exists })
}

#[derive(Debug, Clone)]
pub struct DbStatus {
    pub ok: bool,
    pub has_runs_table: bool,
}
