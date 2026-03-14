use anyhow::{Context, Result};
use serde::Deserialize;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor, PgPool};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct MigrationManifest {
    migrations: Vec<MigrationEntry>,
}

#[derive(Debug, Deserialize)]
struct MigrationEntry {
    id: String,
    status: String,
}

fn required_db_url() -> String {
    match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => {
            panic!(
                "DB tests require MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_migrate_idempotent_on_clean_db -- --ignored"
            );
        }
    }
}

fn expected_applied_versions() -> Result<Vec<i64>> {
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("migrations")
        .join("manifest.json");
    let manifest_raw = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let manifest: MigrationManifest = serde_json::from_str(&manifest_raw)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;

    manifest
        .migrations
        .into_iter()
        .filter(|entry| entry.status == "applied")
        .map(|entry| {
            entry
                .id
                .parse::<i64>()
                .with_context(|| format!("manifest migration id must parse as i64: {}", entry.id))
        })
        .collect()
}

async fn reset_schema(pool: &PgPool, schema: &str) -> Result<()> {
    let drop_sql = format!("drop schema if exists {schema} cascade");
    let create_sql = format!("create schema {schema}");

    pool.execute(drop_sql.as_str())
        .await
        .with_context(|| format!("failed to drop schema {schema}"))?;
    pool.execute(create_sql.as_str())
        .await
        .with_context(|| format!("failed to create schema {schema}"))?;
    Ok(())
}

async fn connect_with_search_path(db_url: &str, schema: &'static str) -> Result<PgPool> {
    let set_search_path = format!("set search_path to {schema}");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(move |conn, _meta| {
            let set_search_path = set_search_path.clone();
            Box::pin(async move {
                sqlx::query(&set_search_path).execute(conn).await?;
                Ok(())
            })
        })
        .connect(db_url)
        .await
        .with_context(|| format!("failed to connect with search_path={schema}"))?;
    Ok(pool)
}

async fn applied_versions(pool: &PgPool) -> Result<Vec<i64>> {
    let rows = sqlx::query_scalar::<_, i64>(
        "select version from _sqlx_migrations where success = true order by version asc",
    )
    .fetch_all(pool)
    .await
    .context("failed to read _sqlx_migrations versions")?;
    Ok(rows)
}

async fn assert_table_exists(pool: &PgPool, table_name: &str) -> Result<()> {
    let exists = sqlx::query_scalar::<_, bool>(
        r#"
        select exists (
            select 1
            from information_schema.tables
            where table_schema = current_schema()
              and table_name = $1
        )
        "#,
    )
    .bind(table_name)
    .fetch_one(pool)
    .await
    .with_context(|| format!("failed to query table presence for {table_name}"))?;

    assert!(exists, "authoritative bootstrap must create table {table_name}");
    Ok(())
}

/// MIG-03: authoritative migration bootstrap and replay proof.
///
/// DB-backed test, skipped if MQK_DATABASE_URL is not set.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test cargo test -p mqk-db --test scenario_migrate_idempotent_on_clean_db -- --ignored"]
async fn migrate_idempotent_on_clean_db() -> Result<()> {
    const PROOF_SCHEMA: &str = "mig03_authoritative_bootstrap_proof";

    let db_url = required_db_url();
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await
        .context("failed to connect admin pool for migration proof")?;
    reset_schema(&admin_pool, PROOF_SCHEMA).await?;

    let pool = connect_with_search_path(&db_url, PROOF_SCHEMA).await?;
    let expected_versions = expected_applied_versions()?;

    mqk_db::migrate(&pool).await?;

    let versions_after_bootstrap = applied_versions(&pool).await?;
    assert_eq!(
        versions_after_bootstrap, expected_versions,
        "bootstrap must apply the authoritative migration chain exactly once"
    );

    for table_name in [
        "runtime_leader_lease",
        "runtime_control_state",
        "runtime_restart_requests",
    ] {
        assert_table_exists(&pool, table_name).await?;
    }

    mqk_db::migrate(&pool).await?;

    let versions_after_replay = applied_versions(&pool).await?;
    assert_eq!(
        versions_after_replay, expected_versions,
        "replaying the authoritative migration chain must be idempotent"
    );

    Ok(())
}
