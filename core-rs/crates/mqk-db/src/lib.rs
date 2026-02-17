use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;
use sqlx::Row;

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
    let (one,): (i32,) = sqlx::query_as::<_, (i32,)>("select 1")
        .fetch_one(pool)
        .await
        .context("status connectivity query failed")?;
    let ok = one == 1;

    let (exists,): (bool,) = sqlx::query_as::<_, (bool,)>(
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

/// Insert a new run row. (Status defaults to CREATED in schema/migration)
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

// ----------------------
// PATCH 14: Lifecycle API
// ----------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    Created,
    Armed,
    Running,
    Stopped,
    Halted,
}

impl RunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Created => "CREATED",
            RunStatus::Armed => "ARMED",
            RunStatus::Running => "RUNNING",
            RunStatus::Stopped => "STOPPED",
            RunStatus::Halted => "HALTED",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "CREATED" => Ok(RunStatus::Created),
            "ARMED" => Ok(RunStatus::Armed),
            "RUNNING" => Ok(RunStatus::Running),
            "STOPPED" => Ok(RunStatus::Stopped),
            "HALTED" => Ok(RunStatus::Halted),
            other => Err(anyhow!("invalid run status: {}", other)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunRow {
    pub run_id: Uuid,
    pub engine_id: String,
    pub mode: String,
    pub started_at_utc: DateTime<Utc>,
    pub git_hash: String,
    pub config_hash: String,
    pub config_json: Value,
    pub host_fingerprint: String,
    pub status: RunStatus,
    pub armed_at_utc: Option<DateTime<Utc>>,
    pub running_at_utc: Option<DateTime<Utc>>,
    pub stopped_at_utc: Option<DateTime<Utc>>,
    pub halted_at_utc: Option<DateTime<Utc>>,
    pub last_heartbeat_utc: Option<DateTime<Utc>>,
}

pub async fn fetch_run(pool: &PgPool, run_id: Uuid) -> Result<RunRow> {
    let row = sqlx::query(
        r#"
        select
          run_id,
          engine_id,
          mode,
          started_at_utc,
          git_hash,
          config_hash,
          config_json,
          host_fingerprint,
          status,
          armed_at_utc,
          running_at_utc,
          stopped_at_utc,
          halted_at_utc,
          last_heartbeat_utc
        from runs
        where run_id = $1
        "#,
    )
    .bind(run_id)
    .fetch_one(pool)
    .await
    .context("fetch_run failed")?;

    Ok(RunRow {
        run_id: row.try_get("run_id")?,
        engine_id: row.try_get("engine_id")?,
        mode: row.try_get("mode")?,
        started_at_utc: row.try_get("started_at_utc")?,
        git_hash: row.try_get("git_hash")?,
        config_hash: row.try_get("config_hash")?,
        config_json: row.try_get("config_json")?,
        host_fingerprint: row.try_get("host_fingerprint")?,
        status: RunStatus::parse(&row.try_get::<String, _>("status")?)?,
        armed_at_utc: row.try_get("armed_at_utc")?,
        running_at_utc: row.try_get("running_at_utc")?,
        stopped_at_utc: row.try_get("stopped_at_utc")?,
        halted_at_utc: row.try_get("halted_at_utc")?,
        last_heartbeat_utc: row.try_get("last_heartbeat_utc")?,
    })
}

/// Enforce run binding to {engine_id, mode, config_hash}.
pub async fn assert_run_binding(
    pool: &PgPool,
    run_id: Uuid,
    engine_id: &str,
    mode: &str,
    config_hash: &str,
) -> Result<()> {
    let r = fetch_run(pool, run_id).await?;
    if r.engine_id != engine_id {
        return Err(anyhow!(
            "run binding mismatch: engine_id expected={} actual={}",
            engine_id,
            r.engine_id
        ));
    }
    if r.mode != mode {
        return Err(anyhow!(
            "run binding mismatch: mode expected={} actual={}",
            mode,
            r.mode
        ));
    }
    if r.config_hash != config_hash {
        return Err(anyhow!(
            "run binding mismatch: config_hash expected={} actual={}",
            config_hash,
            r.config_hash
        ));
    }
    Ok(())
}

/// Transition: CREATED/STOPPED -> ARMED
pub async fn arm_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    // IMPORTANT: preserve DB error details (e.g., unique index violations)
    let res: std::result::Result<Option<(Uuid,)>, sqlx::Error> = sqlx::query_as(
        r#"
        update runs
        set
          status = 'ARMED',
          armed_at_utc = coalesce(armed_at_utc, now()),
          stopped_at_utc = null
        where run_id = $1
          and status in ('CREATED','STOPPED')
        returning run_id
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await;

    let maybe = match res {
        Ok(v) => v,
        Err(e) => {
            // Include the sqlx error string so callers/tests can detect unique violations.
            return Err(anyhow!("arm_run failed: {}", e));
        }
    };

    match maybe {
        Some(_) => Ok(()),
        None => {
            let r = fetch_run(pool, run_id).await?;
            Err(anyhow!(
                "arm_run refused: current_status={}",
                r.status.as_str()
            ))
        }
    }
}


/// Transition: ARMED -> RUNNING
pub async fn begin_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let maybe: Option<(Uuid,)> = sqlx::query_as(
        r#"
        update runs
        set
          status = 'RUNNING',
          running_at_utc = coalesce(running_at_utc, now())
        where run_id = $1
          and status = 'ARMED'
        returning run_id
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await
    .context("begin_run update failed")?;

    match maybe {
        Some(_) => Ok(()),
        None => {
            let r = fetch_run(pool, run_id).await?;
            Err(anyhow!("begin_run refused: current_status={}", r.status.as_str()))
        }
    }
}

/// Transition: ARMED/RUNNING -> STOPPED
pub async fn stop_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let maybe: Option<(Uuid,)> = sqlx::query_as(
        r#"
        update runs
        set
          status = 'STOPPED',
          stopped_at_utc = coalesce(stopped_at_utc, now())
        where run_id = $1
          and status in ('ARMED','RUNNING')
        returning run_id
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await
    .context("stop_run update failed")?;

    match maybe {
        Some(_) => Ok(()),
        None => {
            let r = fetch_run(pool, run_id).await?;
            Err(anyhow!("stop_run refused: current_status={}", r.status.as_str()))
        }
    }
}

/// Transition: ANY -> HALTED (terminal)
pub async fn halt_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query(
        r#"
        update runs
        set
          status = 'HALTED',
          halted_at_utc = coalesce(halted_at_utc, now())
        where run_id = $1
        "#,
    )
    .bind(run_id)
    .execute(pool)
    .await
    .context("halt_run update failed")?;

    Ok(())
}

/// Heartbeat: RUNNING only.
pub async fn heartbeat_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let maybe: Option<(Uuid,)> = sqlx::query_as(
        r#"
        update runs
        set last_heartbeat_utc = now()
        where run_id = $1
          and status = 'RUNNING'
        returning run_id
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await
    .context("heartbeat_run update failed")?;

    match maybe {
        Some(_) => Ok(()),
        None => {
            let r = fetch_run(pool, run_id).await?;
            Err(anyhow!(
                "heartbeat refused: current_status={}",
                r.status.as_str()
            ))
        }
    }
}
