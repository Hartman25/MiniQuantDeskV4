use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

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

/// Insert a new run row.
pub async fn insert_run(pool: &PgPool, run: &NewRun) -> Result<()> {
    sqlx::query(
        r#"
        insert into runs (
          run_id, engine_id, mode, started_at_utc, git_hash, config_hash, config_json, host_fingerprint
        ) values (
          $1, $2, $3, $4, $5, $6, $7, $8
        )
        "#,
    )
    .bind(run.run_id)
    .bind(&run.engine_id)
    .bind(&run.mode)
    .bind(run.started_at_utc)
    .bind(&run.git_hash)
    .bind(&run.config_hash)
    .bind(&run.config_json)
    .bind(&run.host_fingerprint)
    .execute(pool)
    .await
    .context("insert_run failed")?;

    Ok(())
}

#[derive(Debug, Clone)]
pub struct NewRun {
    pub run_id: Uuid,
    pub engine_id: String,
    pub mode: String, // PAPER | LIVE
    pub started_at_utc: DateTime<Utc>,
    pub git_hash: String,
    pub config_hash: String,
    pub config_json: Value,
    pub host_fingerprint: String,
}

/// Insert one audit event row (append-only semantics enforced at app layer).
pub async fn insert_audit_event(pool: &PgPool, ev: &NewAuditEvent) -> Result<()> {
    sqlx::query(
        r#"
        insert into audit_events (
          event_id, run_id, ts_utc, topic, event_type, payload, hash_prev, hash_self
        ) values (
          $1, $2, $3, $4, $5, $6, $7, $8
        )
        "#,
    )
    .bind(ev.event_id)
    .bind(ev.run_id)
    .bind(ev.ts_utc)
    .bind(&ev.topic)
    .bind(&ev.event_type)
    .bind(&ev.payload)
    .bind(&ev.hash_prev)
    .bind(&ev.hash_self)
    .execute(pool)
    .await
    .context("insert_audit_event failed")?;

    Ok(())
}

#[derive(Debug, Clone)]
pub struct NewAuditEvent {
    pub event_id: Uuid,
    pub run_id: Uuid,
    pub ts_utc: DateTime<Utc>,
    pub topic: String,
    pub event_type: String,
    pub payload: Value,
    pub hash_prev: Option<String>,
    pub hash_self: Option<String>,
}
