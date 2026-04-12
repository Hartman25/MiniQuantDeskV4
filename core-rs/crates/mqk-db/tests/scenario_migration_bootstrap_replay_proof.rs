use serde::Deserialize;
use std::collections::BTreeSet;
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

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn migration_bootstrap_and_replay_follow_authoritative_manifest() -> anyhow::Result<()> {
    println!("DB URL = {:?}", std::env::var("MQK_DATABASE_URL"));
    let db_url = std::env::var(mqk_db::ENV_DB_URL)
        .expect("DB tests require MQK_DATABASE_URL; run against a disposable postgres db");

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await?;

    // sqlx prepared-statement protocol rejects multi-statement strings; split.
    sqlx::query("DROP SCHEMA IF EXISTS public CASCADE")
        .execute(&pool)
        .await?;
    sqlx::query("CREATE SCHEMA public").execute(&pool).await?;

    mqk_db::migrate(&pool).await?;

    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("migrations")
        .join("manifest.json");

    let manifest_raw = fs::read_to_string(&manifest_path)?;
    let manifest: MigrationManifest = serde_json::from_str(&manifest_raw)?;

    let expected_versions: BTreeSet<i64> = manifest
        .migrations
        .into_iter()
        .filter(|m| m.status == "applied")
        .map(|m| {
            m.id.parse::<i64>().unwrap_or_else(|_| {
                panic!("manifest applied migration id is not numeric: {}", m.id)
            })
        })
        .collect();

    let applied_rows = sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations")
        .fetch_all(&pool)
        .await?;
    let applied_versions: BTreeSet<i64> = applied_rows.into_iter().collect();

    assert_eq!(
        expected_versions, applied_versions,
        "applied SQLx migration versions must exactly match authoritative manifest applied chain"
    );

    mqk_db::migrate(&pool).await?;

    let replay_rows = sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations")
        .fetch_all(&pool)
        .await?;
    let replay_versions: BTreeSet<i64> = replay_rows.into_iter().collect();

    assert_eq!(
        expected_versions, replay_versions,
        "re-running migrate must preserve the same authoritative applied chain"
    );

    Ok(())
}
