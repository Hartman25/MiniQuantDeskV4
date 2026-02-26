// core-rs/crates/mqk-db/src/lib.rs
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::Row;
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

pub const ENV_DB_URL: &str = "MQK_DATABASE_URL";

// -----------------------------
// Backtest Market Data (Patch A/B/C)
// -----------------------------
// PATCH B/C: expose md module + re-export ingest/report types at crate root
pub mod md;

pub use md::{
    fetch_md_bars, CoverageTotals, FetchMdBarsArgs, IngestCsvArgs, IngestProviderBarsArgs,
    IngestResult, MdBarRow, MdQualityReport, ProviderBar,
};

pub use md::{ingest_csv_to_md_bars, ingest_provider_bars_to_md_bars};

/// Connect to Postgres using MQK_DATABASE_URL.
pub async fn connect_from_env() -> Result<PgPool> {
    let url = std::env::var(ENV_DB_URL).with_context(|| format!("missing env var {ENV_DB_URL}"))?;

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .context("failed to connect to Postgres")?;

    Ok(pool)
}

/// Test helper used by integration tests:
/// - Connect using MQK_DATABASE_URL
/// - Ensure migrations are applied
pub async fn testkit_db_pool() -> Result<PgPool> {
    let pool = connect_from_env().await?;
    migrate(&pool).await?;
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

    Ok(DbStatus {
        ok,
        has_runs_table: exists,
    })
}

#[derive(Debug, Clone)]
pub struct DbStatus {
    pub ok: bool,
    pub has_runs_table: bool,
}

/// Count LIVE runs that are operationally "active": ARMED or RUNNING.
/// This is used by CLI guardrails to prevent accidental migration of a live/armed DB.
pub async fn count_active_live_runs(pool: &PgPool) -> Result<i64> {
    // If schema doesn't exist yet, treat as 0 (safe) rather than failing.
    let st = status(pool).await?;
    if !st.has_runs_table {
        return Ok(0);
    }

    let (n,): (i64,) = sqlx::query_as::<_, (i64,)>(
        r#"
        select count(*)::bigint
        from runs
        where mode = 'LIVE'
          and status in ('ARMED','RUNNING')
        "#,
    )
    .fetch_one(pool)
    .await
    .context("count_active_live_runs failed")?;

    Ok(n)
}

/// Convenience boolean.
pub async fn has_active_live_runs(pool: &PgPool) -> Result<bool> {
    Ok(count_active_live_runs(pool).await? > 0)
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
          event_id, run_id, ts_utc, topic, event_type, payload, hash_prev,
          hash_self
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

#[derive(Debug, Clone)]
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

/// Verify that a run is bound to (engine_id, mode, config_hash).
pub async fn assert_run_binding(
    pool: &PgPool,
    run_id: Uuid,
    engine_id: &str,
    mode: &str,
    config_hash: &str,
) -> Result<()> {
    let r = fetch_run(pool, run_id).await?;
    if r.engine_id != engine_id {
        return Err(anyhow!("run binding mismatch: engine_id"));
    }
    if r.mode != mode {
        return Err(anyhow!("run binding mismatch: mode"));
    }
    if r.config_hash != config_hash {
        return Err(anyhow!("run binding mismatch: config_hash"));
    }
    Ok(())
}

fn cfg_bool(v: &serde_json::Value, ptr: &str, default: bool) -> bool {
    v.pointer(ptr).and_then(|x| x.as_bool()).unwrap_or(default)
}

fn cfg_f64(v: &serde_json::Value, ptr: &str) -> Option<f64> {
    v.pointer(ptr).and_then(|x| x.as_f64())
}

fn cfg_i64(v: &serde_json::Value, ptr: &str) -> Option<i64> {
    v.pointer(ptr).and_then(|x| x.as_i64())
}

fn cfg_str<'a>(v: &'a serde_json::Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(|x| x.as_str())
}

/// Pre-flight gate before arming a run (Patch 20).
///
/// NOTE:
/// - This does NOT talk to the broker.
/// - This does NOT mutate DB state.
/// - CLI must call `arm_preflight()` first, THEN `arm_run()`.
///
/// IMPORTANT (tests):
/// - Do NOT hard-fail on config_hash here; Patch 20 tests focus on arming safety invariants.
/// - Keep checks focused on arming safety invariants.
///
/// PATCH B1: Reconcile cleanliness is now verified via `sys_reconcile_checkpoint`,
/// NOT by reading `audit_events`. A forged `insert_audit_event` row with
/// `topic='reconcile', event_type='CLEAN'` no longer satisfies this gate.
/// Callers must use `reconcile_checkpoint_write` after a genuine reconcile pass.
pub async fn arm_preflight(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let r = fetch_run(pool, run_id).await?;

    // Clone config_json so we can safely borrow it without holding `r` borrows across awaits.
    let cfg = r.config_json.clone();
    let cfg_ref = &cfg;

    let is_live = r.mode.eq_ignore_ascii_case("LIVE");

    // Arming settings (default strict).
    let require_clean_reconcile = cfg_bool(cfg_ref, "/arming/require_clean_reconcile", true);

    // 1) Require a CLEAN reconcile checkpoint for LIVE (PATCH B1).
    //
    //    Previously this checked audit_events.event_type='CLEAN', which was forgeable
    //    by calling insert_audit_event() with any payload.  Now we check the dedicated
    //    sys_reconcile_checkpoint table, written only by reconcile_checkpoint_write().
    //    A forged audit event is insufficient — only a genuine reconcile checkpoint passes.
    if is_live && require_clean_reconcile {
        let checkpoint = reconcile_checkpoint_load_latest(pool, run_id).await?;
        match checkpoint.as_ref().map(|c| c.verdict.as_str()) {
            Some("CLEAN") => {}
            Some(other) => {
                return Err(anyhow!(
                    "arm_preflight reconcile not clean: latest checkpoint verdict='{}'",
                    other
                ));
            }
            None => {
                return Err(anyhow!(
                    "arm_preflight requires clean reconcile checkpoint: \
                     no sys_reconcile_checkpoint row found for run — \
                     insert_audit_event alone is insufficient (PATCH B1)"
                ));
            }
        }
    }

    // 2) Risk limits must be present and non-zero for LIVE.
    if is_live {
        let daily_loss_limit = cfg_f64(cfg_ref, "/risk/daily_loss_limit").unwrap_or(0.0);
        if daily_loss_limit <= 0.0 {
            // TEST CONTRACT: scenario_arm_preflight_blocks_zero_risk_limits.rs
            // asserts msg contains both "risk" and "zero"
            return Err(anyhow!(
                "risk.daily_loss_limit is zero (must be > 0 for LIVE)"
            ));
        }

        // Optional but if present must be > 0.
        if let Some(mdd) = cfg_f64(cfg_ref, "/risk/max_drawdown") {
            if mdd <= 0.0 {
                return Err(anyhow!(
                    "arm_preflight invalid risk.max_drawdown for LIVE (must be > 0): {mdd}"
                ));
            }
        }
    }

    // 3) Kill-switch / safety policy presence checks (LIVE only, opt-in).
    // Tests for Patch 20 currently focus on reconcile + risk invariants.
    // Only enforce these when explicitly enabled by config.
    if is_live && cfg_bool(cfg_ref, "/arming/require_killswitch_policies", false) {
        let stale_policy = cfg_str(cfg_ref, "/data/stale_policy").unwrap_or("");
        if stale_policy.is_empty() || stale_policy.eq_ignore_ascii_case("IGNORE") {
            return Err(anyhow!(
                "arm_preflight data.stale_policy must be set and not IGNORE for LIVE"
            ));
        }

        let feed_policy = cfg_str(cfg_ref, "/data/feed_disagreement_policy").unwrap_or("");
        if feed_policy.is_empty() || feed_policy.eq_ignore_ascii_case("IGNORE") {
            return Err(anyhow!(
                "arm_preflight data.feed_disagreement_policy must be set and not IGNORE for LIVE"
            ));
        }

        let max_rejects = cfg_i64(cfg_ref, "/risk/reject_storm/max_rejects").unwrap_or(0);
        if max_rejects <= 0 {
            return Err(anyhow!(
                "arm_preflight risk.reject_storm.max_rejects must be > 0 for LIVE"
            ));
        }
    }

    // Patch 20 orchestration: after preflight passes, perform the arm transition.
    arm_run(pool, run_id).await
}

/// Arm a run: CREATED/STOPPED -> ARMED.
pub async fn arm_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let r = fetch_run(pool, run_id).await?;
    match r.status {
        RunStatus::Created | RunStatus::Stopped => {}
        _ => return Err(anyhow!("arm_run invalid state: {}", r.status.as_str())),
    }

    let res = sqlx::query(
        r#"
        update runs
        set status = 'ARMED',
            armed_at_utc = now()
        where run_id = $1
        "#,
    )
    .bind(run_id)
    .execute(pool)
    .await;

    match res {
        Ok(_) => Ok(()),
        Err(e) => {
            if is_unique_constraint_violation(&e, "uq_live_engine_active_run") {
                return Err(anyhow!("unique active LIVE constraint"));
            }
            Err(anyhow::Error::new(e).context("arm_run update failed"))
        }
    }
}

/// Detect a Postgres unique constraint violation by name.
fn is_unique_constraint_violation(err: &sqlx::Error, constraint: &str) -> bool {
    match err {
        sqlx::Error::Database(db_err) => {
            db_err.constraint() == Some(constraint)
                || (db_err.code().as_deref() == Some("23505")
                    && db_err.constraint() == Some(constraint))
        }
        _ => false,
    }
}

/// Begin a run: ARMED -> RUNNING.
pub async fn begin_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let r = fetch_run(pool, run_id).await?;
    match r.status {
        RunStatus::Armed => {}
        _ => return Err(anyhow!("begin_run invalid state: {}", r.status.as_str())),
    }

    sqlx::query(
        r#"
        update runs
        set status = 'RUNNING',
            running_at_utc = now()
        where run_id = $1
        "#,
    )
    .bind(run_id)
    .execute(pool)
    .await
    .context("begin_run update failed")?;

    Ok(())
}

/// Stop a run: ARMED/RUNNING -> STOPPED.
pub async fn stop_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let r = fetch_run(pool, run_id).await?;
    match r.status {
        RunStatus::Armed | RunStatus::Running => {}
        _ => return Err(anyhow!("stop_run invalid state: {}", r.status.as_str())),
    }

    sqlx::query(
        r#"
        update runs
        set status = 'STOPPED',
            stopped_at_utc = now()
        where run_id = $1
        "#,
    )
    .bind(run_id)
    .execute(pool)
    .await
    .context("stop_run update failed")?;

    Ok(())
}

/// Halt a run: ANY -> HALTED (sticky).
pub async fn halt_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query(
        r#"
        update runs
        set status = 'HALTED',
            halted_at_utc = now()
        where run_id = $1
        "#,
    )
    .bind(run_id)
    .execute(pool)
    .await
    .context("halt_run update failed")?;

    Ok(())
}

/// Heartbeat: RUNNING only updates last_heartbeat_utc.
pub async fn heartbeat_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let r = fetch_run(pool, run_id).await?;
    match r.status {
        RunStatus::Running => {}
        _ => {
            return Err(anyhow!(
                "heartbeat_run invalid state: {}",
                r.status.as_str()
            ))
        }
    }

    sqlx::query(
        r#"
        update runs
        set last_heartbeat_utc = now()
        where run_id = $1
        "#,
    )
    .bind(run_id)
    .execute(pool)
    .await
    .context("heartbeat_run update failed")?;

    Ok(())
}

/// Deadman: compute whether a RUNNING run's heartbeat is stale.
/// - If run is not RUNNING => false
/// - If last_heartbeat_utc is NULL => true (RUNNING with no heartbeat is unsafe)
pub async fn deadman_expired(pool: &PgPool, run_id: Uuid, ttl_seconds: i64) -> Result<bool> {
    if ttl_seconds <= 0 {
        return Err(anyhow!("deadman ttl_seconds must be > 0"));
    }

    let r = fetch_run(pool, run_id).await?;
    if r.status.as_str() != "RUNNING" {
        return Ok(false);
    }

    let last = match r.last_heartbeat_utc {
        Some(t) => t,
        None => return Ok(true),
    };

    let age = Utc::now().signed_duration_since(last).num_seconds();

    Ok(age > ttl_seconds)
}

/// Deadman enforcement: if RUNNING and expired, HALT the run (sticky) and return true.
/// Otherwise return false.
pub async fn enforce_deadman_or_halt(
    pool: &PgPool,
    run_id: Uuid,
    ttl_seconds: i64,
) -> Result<bool> {
    let expired = deadman_expired(pool, run_id, ttl_seconds).await?;
    if !expired {
        return Ok(false);
    }

    // Only halt if still RUNNING at time of enforcement (avoid halting stopped/armed).
    let r = fetch_run(pool, run_id).await?;
    if r.status.as_str() == "RUNNING" {
        halt_run(pool, run_id).await?;
        return Ok(true);
    }

    Ok(false)
}

// -----------------------------
// PATCH 19: OMS Outbox / Inbox
// -----------------------------

#[derive(Debug, Clone)]
pub struct OutboxRow {
    pub outbox_id: i64,
    pub run_id: Uuid,
    pub idempotency_key: String,
    pub order_json: Value,
    pub status: String, // PENDING | CLAIMED | SENT | ACKED | FAILED
    pub created_at_utc: DateTime<Utc>,
    pub sent_at_utc: Option<DateTime<Utc>>,
    pub claimed_at_utc: Option<DateTime<Utc>>,
    pub claimed_by: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InboxRow {
    pub inbox_id: i64,
    pub run_id: Uuid,
    pub broker_message_id: String,
    pub message_json: Value,
    pub received_at_utc: DateTime<Utc>,
    /// NULL until inbox_mark_applied() is called after a successful portfolio
    /// apply.  Rows with applied_at_utc IS NULL are returned by
    /// inbox_load_unapplied_for_run() for crash-recovery replay (Patch D2).
    pub applied_at_utc: Option<DateTime<Utc>>,
}

/// Enqueue an order intent into oms_outbox.
///
/// Idempotent behavior:
/// - If idempotency_key already exists, returns Ok(false) and does NOT create a second row.
/// - If inserted, returns Ok(true).
///
/// This matches the allocator-grade requirement: restarts cannot double-submit.
pub async fn outbox_enqueue(
    pool: &PgPool,
    run_id: Uuid,
    idempotency_key: &str,
    order_json: Value,
) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        insert into oms_outbox (run_id, idempotency_key, order_json, status)
        values ($1, $2, $3, 'PENDING')
        on conflict (idempotency_key) do nothing
        returning outbox_id
        "#,
    )
    .bind(run_id)
    .bind(idempotency_key)
    .bind(order_json)
    .fetch_optional(pool)
    .await
    .context("outbox_enqueue failed")?;

    Ok(row.is_some())
}

/// Atomically claim up to `batch_size` PENDING outbox rows for exclusive dispatch.
///
/// Uses `FOR UPDATE SKIP LOCKED` so concurrent dispatchers never claim the same row.
/// Returns the claimed rows (now in status CLAIMED). Returns an empty Vec if no
/// PENDING rows are available.
///
/// The caller MUST either:
/// - call `outbox_mark_sent` after a successful broker submit, OR
/// - call `outbox_release_claim` on failure to revert to PENDING.
pub async fn outbox_claim_batch(
    pool: &PgPool,
    batch_size: i64,
    dispatcher_id: &str,
) -> Result<Vec<OutboxRow>> {
    let rows = sqlx::query(
        r#"
        with to_claim as (
            select outbox_id
            from oms_outbox
            where status = 'PENDING'
            order by outbox_id asc
            limit $1
            for update skip locked
        )
        update oms_outbox
           set status         = 'CLAIMED',
               claimed_at_utc = now(),
               claimed_by     = $2
         where outbox_id in (select outbox_id from to_claim)
        returning outbox_id, run_id, idempotency_key, order_json, status,
                  created_at_utc, sent_at_utc, claimed_at_utc, claimed_by
        "#,
    )
    .bind(batch_size)
    .bind(dispatcher_id)
    .fetch_all(pool)
    .await
    .context("outbox_claim_batch failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(OutboxRow {
            outbox_id: row.try_get("outbox_id")?,
            run_id: row.try_get("run_id")?,
            idempotency_key: row.try_get("idempotency_key")?,
            order_json: row.try_get("order_json")?,
            status: row.try_get("status")?,
            created_at_utc: row.try_get("created_at_utc")?,
            sent_at_utc: row.try_get("sent_at_utc")?,
            claimed_at_utc: row.try_get("claimed_at_utc")?,
            claimed_by: row.try_get("claimed_by")?,
        });
    }
    Ok(out)
}

/// Release a CLAIMED row back to PENDING.
///
/// Called when a dispatcher fails before broker submit and wants to relinquish
/// its claim so another dispatcher (or a future retry) can pick it up.
/// Returns true if the row was CLAIMED and is now PENDING; false otherwise.
pub async fn outbox_release_claim(pool: &PgPool, idempotency_key: &str) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status         = 'PENDING',
               claimed_at_utc = null,
               claimed_by     = null
         where idempotency_key = $1
           and status = 'CLAIMED'
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_release_claim failed")?;

    Ok(row.is_some())
}

/// Fetch a single outbox row by idempotency_key.
pub async fn outbox_fetch_by_idempotency_key(
    pool: &PgPool,
    idempotency_key: &str,
) -> Result<Option<OutboxRow>> {
    let row = sqlx::query(
        r#"
        select outbox_id, run_id, idempotency_key, order_json, status,
               created_at_utc, sent_at_utc, claimed_at_utc, claimed_by
        from oms_outbox
        where idempotency_key = $1
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_fetch_by_idempotency_key failed")?;

    let Some(row) = row else { return Ok(None) };

    Ok(Some(OutboxRow {
        outbox_id: row.try_get("outbox_id")?,
        run_id: row.try_get("run_id")?,
        idempotency_key: row.try_get("idempotency_key")?,
        order_json: row.try_get("order_json")?,
        status: row.try_get("status")?,
        created_at_utc: row.try_get("created_at_utc")?,
        sent_at_utc: row.try_get("sent_at_utc")?,
        claimed_at_utc: row.try_get("claimed_at_utc")?,
        claimed_by: row.try_get("claimed_by")?,
    }))
}

/// Mark a CLAIMED outbox row as SENT (sets sent_at_utc).
///
/// Returns true if a row transitioned CLAIMED → SENT; false if not found or
/// not in CLAIMED state.
///
/// **Patch L3 enforcement:** only rows that have been claimed via
/// `outbox_claim_batch` can be marked SENT. Attempting to mark a PENDING row
/// SENT without first claiming it returns `false`, preventing a rogue
/// dispatcher from bypassing the claim/lock protocol.
pub async fn outbox_mark_sent(pool: &PgPool, idempotency_key: &str) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status      = 'SENT',
               sent_at_utc = coalesce(sent_at_utc, now())
         where idempotency_key = $1
           and status = 'CLAIMED'
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_mark_sent failed")?;

    Ok(row.is_some())
}

/// Mark an outbox row as ACKED.
/// Returns true if transitioned, false if not found.
pub async fn outbox_mark_acked(pool: &PgPool, idempotency_key: &str) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
        set status = 'ACKED'
        where idempotency_key = $1
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_mark_acked failed")?;

    Ok(row.is_some())
}

/// Mark a CLAIMED outbox row as FAILED.
///
/// Returns true if a row transitioned CLAIMED → FAILED; false otherwise.
/// Only CLAIMED rows can be failed — use `outbox_claim_batch` first.
pub async fn outbox_mark_failed(pool: &PgPool, idempotency_key: &str) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status = 'FAILED'
         where idempotency_key = $1
           and status = 'CLAIMED'
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_mark_failed failed")?;

    Ok(row.is_some())
}

/// Recovery query: list outbox rows that are not terminal (not ACKED).
///
/// Includes PENDING, CLAIMED, SENT, and FAILED rows — all statuses that
/// indicate the order has not yet been confirmed by the broker.
///
/// NOTE: This does NOT talk to broker yet.
/// It provides the minimal deterministic input required for a future reconcile step.
pub async fn outbox_list_unacked_for_run(pool: &PgPool, run_id: Uuid) -> Result<Vec<OutboxRow>> {
    let rows = sqlx::query(
        r#"
        select outbox_id, run_id, idempotency_key, order_json, status,
               created_at_utc, sent_at_utc, claimed_at_utc, claimed_by
        from oms_outbox
        where run_id = $1
          and status in ('PENDING','CLAIMED','SENT','FAILED')
        order by outbox_id asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("outbox_list_unacked_for_run failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(OutboxRow {
            outbox_id: row.try_get("outbox_id")?,
            run_id: row.try_get("run_id")?,
            idempotency_key: row.try_get("idempotency_key")?,
            order_json: row.try_get("order_json")?,
            status: row.try_get("status")?,
            created_at_utc: row.try_get("created_at_utc")?,
            sent_at_utc: row.try_get("sent_at_utc")?,
            claimed_at_utc: row.try_get("claimed_at_utc")?,
            claimed_by: row.try_get("claimed_by")?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Arm state persistence — Patch L7
// ---------------------------------------------------------------------------

/// Persist the current arm state to `sys_arm_state` (upsert singleton row).
///
/// `state` must be `"ARMED"` or `"DISARMED"`.
/// `reason` is the `DisarmReason` variant name when `state == "DISARMED"`;
/// pass `None` (or `Some("BootDefault")`) when arming.
pub async fn persist_arm_state(pool: &PgPool, state: &str, reason: Option<&str>) -> Result<()> {
    sqlx::query(
        r#"
        insert into sys_arm_state (sentinel_id, state, reason, updated_at_utc)
        values (1, $1, $2, now())
        on conflict (sentinel_id) do update
            set state          = excluded.state,
                reason         = excluded.reason,
                updated_at_utc = excluded.updated_at_utc
        "#,
    )
    .bind(state)
    .bind(reason)
    .execute(pool)
    .await
    .context("persist_arm_state failed")?;
    Ok(())
}

/// Load the last persisted arm state.
///
/// Returns `None` if no state has ever been persisted (fresh system — caller
/// should treat this as `DISARMED / BootDefault`).
///
/// Returns `Some((state, reason))` where `state` is `"ARMED"` or `"DISARMED"`
/// and `reason` is the `DisarmReason` variant name (or `None` when armed).
pub async fn load_arm_state(pool: &PgPool) -> Result<Option<(String, Option<String>)>> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        r#"
        select state, reason
        from sys_arm_state
        where sentinel_id = 1
        "#,
    )
    .fetch_optional(pool)
    .await
    .context("load_arm_state failed")?;
    Ok(row)
}

/// Insert a broker message/fill into oms_inbox with dedupe on broker_message_id.
///
/// Idempotent behavior:
/// - If broker_message_id already exists, returns Ok(false) and does NOT create a second row.
/// - If inserted, returns Ok(true).
///
/// Patch D2 caller contract:
/// ```text
/// let inserted = inbox_insert_deduped(pool, run_id, msg_id, json).await?;
/// if inserted {
///     apply_fill_to_portfolio(json);          // idempotent apply
///     inbox_mark_applied(pool, msg_id).await?; // journal completion
/// }
/// ```
/// On crash between insert and mark_applied: the row surfaces in
/// `inbox_load_unapplied_for_run` for recovery replay.
pub async fn inbox_insert_deduped(
    pool: &PgPool,
    run_id: Uuid,
    broker_message_id: &str,
    message_json: Value,
) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        insert into oms_inbox (run_id, broker_message_id, message_json)
        values ($1, $2, $3)
        on conflict (broker_message_id) do nothing
        returning inbox_id
        "#,
    )
    .bind(run_id)
    .bind(broker_message_id)
    .bind(message_json)
    .fetch_optional(pool)
    .await
    .context("inbox_insert_deduped failed")?;

    Ok(row.is_some())
}

/// Stamp `applied_at_utc = now()` on an inbox row after its fill has been
/// successfully applied to in-process portfolio state.
///
/// Part of the Patch D2 crash-recovery contract:
/// - Call this immediately after the portfolio apply completes.
/// - Rows where `applied_at_utc IS NULL` appear in
///   `inbox_load_unapplied_for_run` and must be replayed at startup.
///
/// Idempotent: silently succeeds if `broker_message_id` is not present or
/// has already been stamped.
pub async fn inbox_mark_applied(pool: &PgPool, broker_message_id: &str) -> Result<()> {
    sqlx::query(
        r#"
        update oms_inbox
           set applied_at_utc = now()
         where broker_message_id = $1
           and applied_at_utc is null
        "#,
    )
    .bind(broker_message_id)
    .execute(pool)
    .await
    .context("inbox_mark_applied failed")?;
    Ok(())
}

/// Load inbox rows for a run that were received but not yet applied
/// (`applied_at_utc IS NULL`).
///
/// Call this at startup/recovery to identify fills whose apply step did not
/// complete before a crash.  Replay these fills in `inbox_id` ascending order;
/// each apply must be idempotent so re-applying a partially-applied fill is
/// safe.  After successfully applying each row, call `inbox_mark_applied`.
///
/// Uses the partial index `idx_inbox_run_unapplied` for efficiency.
pub async fn inbox_load_unapplied_for_run(pool: &PgPool, run_id: Uuid) -> Result<Vec<InboxRow>> {
    let rows = sqlx::query(
        r#"
        select inbox_id, run_id, broker_message_id, message_json,
               received_at_utc, applied_at_utc
          from oms_inbox
         where run_id = $1
           and applied_at_utc is null
         order by inbox_id asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("inbox_load_unapplied_for_run failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(InboxRow {
            inbox_id: row.try_get("inbox_id")?,
            run_id: row.try_get("run_id")?,
            broker_message_id: row.try_get("broker_message_id")?,
            message_json: row.try_get("message_json")?,
            received_at_utc: row.try_get("received_at_utc")?,
            applied_at_utc: row.try_get("applied_at_utc")?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Broker order ID map persistence — Patch A4
// ---------------------------------------------------------------------------

/// Persist (or update) an `internal_id → broker_id` mapping after a successful
/// broker submit.
///
/// Uses `ON CONFLICT … DO UPDATE` so idempotent retries (e.g. after a crash
/// between submit and `outbox_mark_sent`) safely overwrite rather than fail.
///
/// Call this immediately after a confirmed broker submit, before returning from
/// the dispatch loop.
pub async fn broker_map_upsert(pool: &PgPool, internal_id: &str, broker_id: &str) -> Result<()> {
    sqlx::query(
        r#"
        insert into broker_order_map (internal_id, broker_id)
        values ($1, $2)
        on conflict (internal_id) do update
            set broker_id = excluded.broker_id
        "#,
    )
    .bind(internal_id)
    .bind(broker_id)
    .execute(pool)
    .await
    .context("broker_map_upsert failed")?;
    Ok(())
}

/// Remove an `internal_id → broker_id` mapping when an order reaches a terminal
/// state (filled, cancel-ack, rejected).
///
/// Silently succeeds if `internal_id` is not present (idempotent cleanup).
pub async fn broker_map_remove(pool: &PgPool, internal_id: &str) -> Result<()> {
    sqlx::query(
        r#"
        delete from broker_order_map
        where internal_id = $1
        "#,
    )
    .bind(internal_id)
    .execute(pool)
    .await
    .context("broker_map_remove failed")?;
    Ok(())
}

/// Load all live `internal_id → broker_id` pairs from DB.
///
/// Called at daemon startup to repopulate the in-memory `BrokerOrderMap`
/// (see `mqk-execution/id_map.rs`) so cancel/replace operations can target the
/// correct broker order ID after a crash or planned restart.
///
/// Returns pairs ordered by `registered_at_utc` ascending (insertion order).
pub async fn broker_map_load(pool: &PgPool) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query(
        r#"
        select internal_id, broker_id
        from broker_order_map
        order by registered_at_utc asc
        "#,
    )
    .fetch_all(pool)
    .await
    .context("broker_map_load failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push((
            row.try_get::<String, _>("internal_id")?,
            row.try_get::<String, _>("broker_id")?,
        ));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Reconcile checkpoint — Patch B1
// ---------------------------------------------------------------------------

/// A persisted reconcile checkpoint written by the reconcile engine.
///
/// `arm_preflight` checks this table (not `audit_events`) for reconcile
/// cleanliness.  A CLEAN verdict here requires the reconcile engine to have
/// called `reconcile_checkpoint_write` — inserting a fake audit event is
/// insufficient.
#[derive(Debug, Clone)]
pub struct ReconcileCheckpoint {
    pub checkpoint_id: i64,
    pub run_id: Uuid,
    /// `"CLEAN"` or `"DIRTY"`.
    pub verdict: String,
    /// `SnapshotWatermark::last_accepted_ms()` at reconcile time.
    pub snapshot_watermark_ms: i64,
    /// Caller-computed hash of the reconcile payload (auditability hook).
    pub result_hash: String,
    pub created_at_utc: DateTime<Utc>,
}

/// Write a reconcile checkpoint after a genuine reconcile pass.
///
/// This is the **only** function that satisfies the `arm_preflight` reconcile
/// gate (PATCH B1).  `insert_audit_event` with `event_type='CLEAN'` no longer
/// fulfils arming.
///
/// `verdict` must be `"CLEAN"` or `"DIRTY"`.
/// `snapshot_watermark_ms` should be `SnapshotWatermark::last_accepted_ms()`.
/// `result_hash` is a caller-computed hash (e.g. SHA-256 of the reconcile
/// report JSON) for auditability; it is stored but not cryptographically
/// verified by arming.
pub async fn reconcile_checkpoint_write(
    pool: &PgPool,
    run_id: Uuid,
    verdict: &str,
    snapshot_watermark_ms: i64,
    result_hash: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into sys_reconcile_checkpoint
            (run_id, verdict, snapshot_watermark_ms, result_hash)
        values ($1, $2, $3, $4)
        "#,
    )
    .bind(run_id)
    .bind(verdict)
    .bind(snapshot_watermark_ms)
    .bind(result_hash)
    .execute(pool)
    .await
    .context("reconcile_checkpoint_write failed")?;
    Ok(())
}

/// Load the most recent reconcile checkpoint for a run.
///
/// Returns `None` if the reconcile engine has not yet written any checkpoint
/// for this run (arming should fail in that case).
pub async fn reconcile_checkpoint_load_latest(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Option<ReconcileCheckpoint>> {
    let row = sqlx::query(
        r#"
        select checkpoint_id, run_id, verdict, snapshot_watermark_ms, result_hash, created_at_utc
        from sys_reconcile_checkpoint
        where run_id = $1
        order by created_at_utc desc
        limit 1
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await
    .context("reconcile_checkpoint_load_latest failed")?;

    let Some(row) = row else { return Ok(None) };

    Ok(Some(ReconcileCheckpoint {
        checkpoint_id: row.try_get("checkpoint_id")?,
        run_id: row.try_get("run_id")?,
        verdict: row.try_get("verdict")?,
        snapshot_watermark_ms: row.try_get("snapshot_watermark_ms")?,
        result_hash: row.try_get("result_hash")?,
        created_at_utc: row.try_get("created_at_utc")?,
    }))
}
