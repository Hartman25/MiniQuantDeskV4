// core-rs/crates/mqk-db/src/lib.rs
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::Row;
use sqlx::{
    postgres::{PgPoolOptions, PgRow},
    PgPool,
};
use uuid::Uuid;

pub const ENV_DB_URL: &str = "MQK_DATABASE_URL";

pub mod runtime_lease;

// ---------------------------------------------------------------------------
// TimeSource â€” injectable clock abstraction (FC-5)
// ---------------------------------------------------------------------------

/// Abstraction over a UTC clock, injected wherever enforcement or
/// state-transition logic needs a timestamp.
///
/// This crate must remain deterministic: it must not read the wall clock.
/// Production code should provide a `TimeSource` implementation at the
/// runtime/daemon layer and inject it into db calls.
pub trait TimeSource: Send + Sync {
    fn now_utc(&self) -> DateTime<Utc>;
}

/// Deterministic `TimeSource` for tests and scenario replay.
#[derive(Clone, Copy, Debug)]
pub struct FixedClock {
    now: DateTime<Utc>,
}

impl FixedClock {
    pub fn new(now: DateTime<Utc>) -> Self {
        Self { now }
    }
}

impl TimeSource for FixedClock {
    fn now_utc(&self) -> DateTime<Utc> {
        self.now
    }
}

// -----------------------------
// Backtest Market Data (Patch A/B/C)
// -----------------------------
// PATCH B/C: expose md module + re-export ingest/report types at crate root
pub mod md;

pub use md::{
    fetch_md_bars, latest_stored_bar_end_ts, CoverageTotals, FetchMdBarsArgs, IngestCsvArgs,
    IngestProviderBarsArgs, IngestResult, MdBarRow, MdQualityReport, ProviderBar,
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

/// Count runs that started in the last 24 hours for the given engine and mode.
///
/// Uses the DB clock (`now() - interval '24 hours'`) to avoid pulling wall-clock
/// time into library code.  Returns 0 when the `runs` table does not yet exist
/// (pre-migration); callers must treat that as "table missing, count untrustworthy"
/// if needed — the route layer maps this to `None` when `db` is absent entirely.
pub async fn count_runs_in_last_24h(pool: &PgPool, engine_id: &str, mode: &str) -> Result<i64> {
    let st = status(pool).await?;
    if !st.has_runs_table {
        return Ok(0);
    }

    let (n,): (i64,) = sqlx::query_as::<_, (i64,)>(
        r#"
        select count(*)::bigint
        from runs
        where engine_id = $1
          and mode = $2
          and started_at_utc > now() - interval '24 hours' -- allow: ops-metadata — 24h restart count for operator display, not an enforcement or capital-decision path
        "#,
    )
    .bind(engine_id)
    .bind(mode)
    .fetch_one(pool)
    .await
    .context("count_runs_in_last_24h failed")?;

    Ok(n)
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

fn run_row_from_row(row: PgRow) -> Result<RunRow> {
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

    run_row_from_row(row)
}

pub async fn fetch_latest_run_for_engine(
    pool: &PgPool,
    engine_id: &str,
    mode: &str,
) -> Result<Option<RunRow>> {
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
        where engine_id = $1
          and mode = $2
        order by started_at_utc desc, run_id desc
        limit 1
        "#,
    )
    .bind(engine_id)
    .bind(mode)
    .fetch_optional(pool)
    .await
    .context("fetch_latest_run_for_engine failed")?;

    row.map(run_row_from_row).transpose()
}

pub async fn fetch_active_run_for_engine(
    pool: &PgPool,
    engine_id: &str,
    mode: &str,
) -> Result<Option<RunRow>> {
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
        where engine_id = $1
          and mode = $2
          and status in ('ARMED', 'RUNNING')
        order by started_at_utc desc, run_id desc
        limit 1
        "#,
    )
    .bind(engine_id)
    .bind(mode)
    .fetch_optional(pool)
    .await
    .context("fetch_active_run_for_engine failed")?;

    row.map(run_row_from_row).transpose()
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
    //    A forged audit event is insufficient â€” only a genuine reconcile checkpoint passes.
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
                     no sys_reconcile_checkpoint row found for run â€” \
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
            armed_at_utc = now() -- allow: ops-metadata
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
            running_at_utc = now() -- allow: ops-metadata
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
            stopped_at_utc = now() -- allow: ops-metadata
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
///
/// `halted_at` is injected by the caller (FC-9: no now() in enforcement path).
pub async fn halt_run(pool: &PgPool, run_id: Uuid, halted_at: DateTime<Utc>) -> Result<()> {
    sqlx::query(
        r#"
        update runs
        set status = 'HALTED',
            halted_at_utc = $2
        where run_id = $1
        "#,
    )
    .bind(run_id)
    .bind(halted_at)
    .execute(pool)
    .await
    .context("halt_run update failed")?;

    Ok(())
}

/// Heartbeat: RUNNING only updates last_heartbeat_utc.
///
/// `heartbeat_at` is injected by the caller (FC-9: no now() in enforcement path).
pub async fn heartbeat_run(pool: &PgPool, run_id: Uuid, heartbeat_at: DateTime<Utc>) -> Result<()> {
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
        set last_heartbeat_utc = $2
        where run_id = $1
        "#,
    )
    .bind(run_id)
    .bind(heartbeat_at)
    .execute(pool)
    .await
    .context("heartbeat_run update failed")?;

    Ok(())
}

/// Deadman: compute whether a RUNNING run's heartbeat is stale.
/// - If run is not RUNNING => false
/// - If last_heartbeat_utc is NULL => true (RUNNING with no heartbeat is unsafe)
///
/// `now` is injected by the caller (D1-3: no wall-clock reads in enforcement path).
pub async fn deadman_expired(
    pool: &PgPool,
    run_id: Uuid,
    ttl_seconds: i64,
    now: DateTime<Utc>,
) -> Result<bool> {
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

    let age = now.signed_duration_since(last).num_seconds();

    Ok(age > ttl_seconds)
}

/// Deadman enforcement: if RUNNING and expired, HALT the run (sticky) and return true.
/// Otherwise return false.
///
/// `now` is injected by the caller (D1-3: no wall-clock reads in enforcement path).
pub async fn enforce_deadman_or_halt(
    pool: &PgPool,
    run_id: Uuid,
    ttl_seconds: i64,
    now: DateTime<Utc>,
) -> Result<bool> {
    let expired = deadman_expired(pool, run_id, ttl_seconds, now).await?;
    if !expired {
        return Ok(false);
    }

    // Only halt if still RUNNING at time of enforcement (avoid halting stopped/armed).
    let r = fetch_run(pool, run_id).await?;
    if r.status.as_str() == "RUNNING" {
        halt_run(pool, run_id, now).await?;
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
    pub status: String, // PENDING | CLAIMED | DISPATCHING | SENT | ACKED | FAILED
    pub created_at_utc: DateTime<Utc>,
    pub sent_at_utc: Option<DateTime<Utc>>,
    pub claimed_at_utc: Option<DateTime<Utc>>,
    pub claimed_by: Option<String>,
    /// RT-5: timestamp written before gateway.submit(); null until DISPATCHING.
    pub dispatching_at_utc: Option<DateTime<Utc>>,
    /// RT-5: dispatcher identity written before gateway.submit(); null until DISPATCHING.
    pub dispatch_attempt_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbiguousOutboxRow {
    pub idempotency_key: String,
    pub status: String, // AMBIGUOUS | DISPATCHING | SENT (without broker map)
    pub broker_order_id: Option<String>,
}

/// Load restart-ambiguous outbox rows for a run.
///
/// Policy (A4):
/// - `AMBIGUOUS` is always quarantined: `BrokerError::AmbiguousSubmit` was
///   returned, meaning the broker may or may not have accepted the order.
///   These rows can only exit quarantine via `outbox_reset_ambiguous_to_pending`
///   (explicit operator/reconcile-proof release).
/// - `DISPATCHING` is always ambiguous on restart: broker submit may have
///   been attempted, but the process died before closure.
/// - `SENT` is ambiguous only when the broker-order map is still missing.
///   A normal healthy `SENT` row with a broker map entry must NOT be
///   quarantined every tick, otherwise the system would halt during
///   ordinary pre-ACK operation.
///
/// This helper therefore returns only rows that are unsafe to continue past
/// restart without operator intervention.
pub async fn outbox_load_restart_ambiguous_for_run(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Vec<AmbiguousOutboxRow>> {
    let rows = sqlx::query(
        r#"
        select
            o.idempotency_key,
            o.status,
            m.broker_id as broker_order_id
        from oms_outbox o
        left join broker_order_map m
          on m.internal_id = o.idempotency_key
        where o.run_id = $1
          and (
                o.status = 'AMBIGUOUS'
                or o.status = 'DISPATCHING'
                or (
                    o.status = 'SENT'
                    and m.broker_id is null
                )
          )
        order by o.outbox_id asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("outbox_load_restart_ambiguous_for_run failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(AmbiguousOutboxRow {
            idempotency_key: row.try_get("idempotency_key")?,
            status: row.try_get("status")?,
            broker_order_id: row.try_get("broker_order_id")?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// OutboxClaimToken (FC-2)
// ---------------------------------------------------------------------------

/// Unforgeable proof that an outbox row has been claimed via
/// [`outbox_claim_batch`].
///
/// # Forgeability
///
/// The `_priv` field is `pub(crate)`, preventing struct-literal construction
/// outside this crate. The only `pub(crate)` constructor (`OutboxClaimToken::new`)
/// is called exclusively inside `outbox_claim_batch`, which atomically performs
/// `FOR UPDATE SKIP LOCKED` â€” the DB lock IS the proof.
///
/// External code may name this type (needed to implement `BrokerAdapter` and
/// call `BrokerGateway::submit`) but cannot construct it. In production, the
/// only way to obtain a token is through `outbox_claim_batch`. In tests,
/// [`OutboxClaimToken::for_test`] is available as an explicit escape hatch.
///
/// ```text
/// âœ…  let claimed = outbox_claim_batch(&pool, â€¦).await?;   // production path
///     let token = &claimed[0].token;
/// âœ…  OutboxClaimToken::for_test(id, key)                  // tests only
/// âŒ  OutboxClaimToken { _priv: (), â€¦ }                    // ERROR: private field
/// ```
#[allow(clippy::manual_non_exhaustive)]
#[derive(Debug, Clone)]
pub struct OutboxClaimToken {
    /// The DB row ID of the claimed outbox entry.
    pub outbox_id: i64,
    /// The idempotency key (`client_order_id`) of the claimed outbox entry.
    pub idempotency_key: String,
    /// Prevents struct-literal construction outside this crate (FC-2).
    pub(crate) _priv: (),
}

impl OutboxClaimToken {
    /// Construct a claim token from a successfully claimed outbox row.
    ///
    /// `pub(crate)` â€” only callable inside `mqk-db`. Callers outside this
    /// crate must obtain tokens via [`outbox_claim_batch`].
    ///
    /// # Compile-time gate
    ///
    /// Compiled only when at least one of the following is active:
    /// - `test` â€” for the `for_test` escape hatch used in unit tests
    /// - `feature = "runtime-claim"` â€” for `outbox_claim_batch` (production path)
    /// - `feature = "testkit"` â€” for integration test infrastructure
    ///
    /// In a plain `cargo build` / `cargo clippy` without any of these, this
    /// function is not present and cannot be called â€” enforcing the RT-1 gate.
    #[cfg(any(test, feature = "runtime-claim", feature = "testkit"))]
    pub(crate) fn new(outbox_id: i64, idempotency_key: impl Into<String>) -> Self {
        Self {
            outbox_id,
            idempotency_key: idempotency_key.into(),
            _priv: (),
        }
    }

    /// Test-only escape hatch. Do NOT call from production code.
    ///
    /// # Compile-time gate
    ///
    /// This function is compiled only when:
    /// - `#[cfg(test)]` is active (i.e., the **owning crate** is being tested
    ///   via `cargo test -p mqk-db`), OR
    /// - the `testkit` Cargo feature is explicitly enabled.
    ///
    /// The `testkit` feature MUST NOT be listed in any production crate's
    /// `[dependencies]` â€” only in `[dev-dependencies]` of test/testkit crates.
    ///
    /// In production, tokens are returned exclusively by [`outbox_claim_batch`],
    /// coupling each token to a real DB-level `FOR UPDATE SKIP LOCKED` row
    /// lock. This function bypasses that guarantee and exists solely for unit
    /// and integration test setup.
    #[doc(hidden)]
    #[cfg(any(test, feature = "testkit"))]
    pub fn for_test(outbox_id: i64, idempotency_key: impl Into<String>) -> Self {
        Self::new(outbox_id, idempotency_key)
    }
}

/// Return type of [`outbox_claim_batch`].
///
/// Bundles the claimed [`OutboxRow`] with its [`OutboxClaimToken`], ensuring
/// the token is always paired with the row that generated it.
///
/// # Availability
///
/// Gated behind `feature = "runtime-claim"` (production) or `feature = "testkit"`
/// (tests). See RT-1.
// RT-1: single-dispatcher boundary. Only mqk-runtime (runtime-claim feature) and
// test infrastructure (testkit feature) may use this type. Daemon and CLI must
// not depend on mqk-db with either feature active.
#[cfg(any(feature = "runtime-claim", feature = "testkit"))]
#[derive(Debug, Clone)]
pub struct ClaimedOutboxRow {
    /// The claimed outbox row (status = `CLAIMED`).
    pub row: OutboxRow,
    /// Unforgeable proof of the DB claim. Pass to `BrokerGateway::submit`.
    pub token: OutboxClaimToken,
}

#[derive(Debug, Clone)]
pub struct InboxRow {
    pub inbox_id: i64,
    pub run_id: Uuid,
    pub broker_message_id: String,
    pub broker_fill_id: Option<String>,
    pub broker_sequence_id: Option<String>,
    pub broker_timestamp: Option<String>,
    pub message_json: Value,
    pub received_at_utc: DateTime<Utc>,
    /// NULL until inbox_mark_applied() is called after a successful portfolio
    /// apply.  Rows with applied_at_utc IS NULL are returned by
    /// inbox_load_unapplied_for_run() for crash-recovery replay (Patch D2).
    pub applied_at_utc: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct BrokerEventIdentity {
    pub broker_message_id: String,
    pub broker_fill_id: Option<String>,
    pub broker_sequence_id: Option<String>,
    pub broker_timestamp: Option<String>,
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
/// Returns [`ClaimedOutboxRow`]s, each containing the claimed [`OutboxRow`] **and**
/// an [`OutboxClaimToken`] constructed from the DB row â€” coupling the token to the
/// actual lock (FC-2). Returns an empty `Vec` if no `PENDING` rows are available.
///
/// The caller MUST:
/// - call `outbox_mark_dispatching` immediately before `gateway.submit()`, THEN
/// - call `outbox_mark_sent` after a successful submit (DISPATCHING â†’ SENT), OR
/// - call `outbox_mark_failed` on submit failure (row quarantined as FAILED).
///
/// `outbox_release_claim` (CLAIMED â†’ PENDING) is only valid while the row is
/// still CLAIMED â€” i.e. before `outbox_mark_dispatching` is called.
///
/// # Availability â€” RT-1 single-dispatcher gate
///
/// This function is only compiled when `feature = "runtime-claim"` (enabled
/// exclusively by `mqk-runtime`) or `feature = "testkit"` (test infrastructure)
/// is active. Daemon and CLI crates must NOT enable either feature; any attempt
/// to call this function from those crates produces `error[E0425]` at compile time.
// RT-1: gate enforced here. Do not remove without updating the prover.
#[cfg(any(feature = "runtime-claim", feature = "testkit"))]
async fn outbox_claim_batch_inner(
    pool: &PgPool,
    run_id: Option<Uuid>,
    batch_size: i64,
    dispatcher_id: &str,
    claimed_at: DateTime<Utc>,
) -> Result<Vec<ClaimedOutboxRow>> {
    let rows = if let Some(run_id) = run_id {
        sqlx::query(
            r#"
            with to_claim as (
                select outbox_id
                from oms_outbox
                where run_id = $2
                  and status = 'PENDING'
                order by outbox_id asc
                limit $1
                for update skip locked
            )
            update oms_outbox
               set status         = 'CLAIMED',
                   claimed_at_utc = $4,
                   claimed_by     = $3
             where outbox_id in (select outbox_id from to_claim)
            returning outbox_id, run_id, idempotency_key, order_json, status,
                      created_at_utc, sent_at_utc, claimed_at_utc, claimed_by,
                      dispatching_at_utc, dispatch_attempt_id
            "#,
        )
        .bind(batch_size)
        .bind(run_id)
        .bind(dispatcher_id)
        .bind(claimed_at)
        .fetch_all(pool)
        .await
        .context("outbox_claim_batch_for_run failed")?
    } else {
        sqlx::query(
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
                   claimed_at_utc = $3,
                   claimed_by     = $2
             where outbox_id in (select outbox_id from to_claim)
            returning outbox_id, run_id, idempotency_key, order_json, status,
                      created_at_utc, sent_at_utc, claimed_at_utc, claimed_by,
                      dispatching_at_utc, dispatch_attempt_id
            "#,
        )
        .bind(batch_size)
        .bind(dispatcher_id)
        .bind(claimed_at)
        .fetch_all(pool)
        .await
        .context("outbox_claim_batch failed")?
    };

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let outbox_row = OutboxRow {
            outbox_id: row.try_get("outbox_id")?,
            run_id: row.try_get("run_id")?,
            idempotency_key: row.try_get("idempotency_key")?,
            order_json: row.try_get("order_json")?,
            status: row.try_get("status")?,
            created_at_utc: row.try_get("created_at_utc")?,
            sent_at_utc: row.try_get("sent_at_utc")?,
            claimed_at_utc: row.try_get("claimed_at_utc")?,
            claimed_by: row.try_get("claimed_by")?,
            dispatching_at_utc: row.try_get("dispatching_at_utc")?,
            dispatch_attempt_id: row.try_get("dispatch_attempt_id")?,
        };
        let token = OutboxClaimToken::new(outbox_row.outbox_id, &outbox_row.idempotency_key);
        out.push(ClaimedOutboxRow {
            row: outbox_row,
            token,
        });
    }
    Ok(out)
}

#[cfg(any(feature = "runtime-claim", feature = "testkit"))]
pub async fn outbox_claim_batch(
    pool: &PgPool,
    batch_size: i64,
    dispatcher_id: &str,
    claimed_at: DateTime<Utc>,
) -> Result<Vec<ClaimedOutboxRow>> {
    outbox_claim_batch_inner(pool, None, batch_size, dispatcher_id, claimed_at).await
}

#[cfg(any(feature = "runtime-claim", feature = "testkit"))]
pub async fn outbox_claim_batch_for_run(
    pool: &PgPool,
    run_id: Uuid,
    batch_size: i64,
    dispatcher_id: &str,
    claimed_at: DateTime<Utc>,
) -> Result<Vec<ClaimedOutboxRow>> {
    outbox_claim_batch_inner(pool, Some(run_id), batch_size, dispatcher_id, claimed_at).await
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

/// RT-5: Advance a CLAIMED outbox row to DISPATCHING immediately before calling
/// `gateway.submit()`.
///
/// Writing DISPATCHING before the broker call closes the W4 crash window:
/// `outbox_reset_stale_claims` only resets `CLAIMED` rows â€” a crash between
/// `outbox_mark_dispatching` and `outbox_mark_sent` leaves the row in
/// `DISPATCHING`, preventing silent requeue and double-submit on restart.
///
/// `dispatching_at` is caller-supplied (no SQL `now()` â€” FC-7 policy).
/// `dispatch_attempt_id` identifies which dispatcher instance was in-flight;
/// used for crash-recovery audit.
///
/// Returns `true` if the row transitioned `CLAIMED â†’ DISPATCHING`; `false` if
/// not found or not in `CLAIMED` state.
pub async fn outbox_mark_dispatching(
    pool: &PgPool,
    idempotency_key: &str,
    dispatch_attempt_id: &str,
    dispatching_at: DateTime<Utc>,
) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status              = 'DISPATCHING',
               dispatching_at_utc  = $3,
               dispatch_attempt_id = $2
         where idempotency_key = $1
           and status = 'CLAIMED'
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .bind(dispatch_attempt_id)
    .bind(dispatching_at)
    .fetch_optional(pool)
    .await
    .context("outbox_mark_dispatching failed")?;

    Ok(row.is_some())
}

/// Reset stale CLAIMED rows back to PENDING â€” the crash-recovery reaper (FC-6).
///
/// Called on orchestrator startup (and optionally on a periodic sweep) to
/// recover rows left in CLAIMED state by a crashed or stuck dispatcher.
///
/// A row is considered stale when its `claimed_at_utc` is strictly earlier
/// than `stale_threshold`.  The threshold is caller-supplied â€” no wall-clock
/// inside this function (FC-5 policy).  In production, pass
/// `time_source.now_utc() - stale_duration`; in tests, pass an explicit
/// timestamp.
///
/// Returns the number of rows reset.  Only `CLAIMED` rows are affected.
/// Terminal states (`SENT`, `ACKED`, `FAILED`) and `PENDING` rows are never
/// modified.
pub async fn outbox_reset_stale_claims(
    pool: &PgPool,
    stale_threshold: DateTime<Utc>,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        update oms_outbox
           set status         = 'PENDING',
               claimed_at_utc = null,
               claimed_by     = null
         where status         = 'CLAIMED'
           and claimed_at_utc < $1
        "#,
    )
    .bind(stale_threshold)
    .execute(pool)
    .await
    .context("outbox_reset_stale_claims failed")?;

    Ok(result.rows_affected())
}

/// Fetch a single outbox row by idempotency_key.
pub async fn outbox_fetch_by_idempotency_key(
    pool: &PgPool,
    idempotency_key: &str,
) -> Result<Option<OutboxRow>> {
    let row = sqlx::query(
        r#"
        select outbox_id, run_id, idempotency_key, order_json, status,
               created_at_utc, sent_at_utc, claimed_at_utc, claimed_by,
               dispatching_at_utc, dispatch_attempt_id
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
        dispatching_at_utc: row.try_get("dispatching_at_utc")?,
        dispatch_attempt_id: row.try_get("dispatch_attempt_id")?,
    }))
}

/// Atomically persist `internal_id â†’ broker_id` and transition the outbox row
/// to `SENT`.
///
/// This closes the Patch 3A durability gap:
/// the system must not durably acknowledge dispatch (`SENT`) without also
/// durably persisting the broker order ID mapping needed for restart recovery.
///
/// Transaction semantics:
/// - upsert `(internal_id, broker_id)` into `broker_order_map`
/// - transition `oms_outbox` row to `SENT`
/// - commit only if both steps succeed
///
/// Returns `true` if the outbox row transitioned to `SENT`; `false` if not
/// found or not in an acceptable pre-SENT state. If the outbox transition does
/// not occur, the transaction is not committed, so the broker map upsert is
/// rolled back as well.
///
/// Accepts both `CLAIMED` and `DISPATCHING`:
/// - Production path (RT-5): `DISPATCHING â†’ SENT`
/// - Legacy test path: `CLAIMED â†’ SENT`
pub async fn outbox_mark_sent_with_broker_map(
    pool: &PgPool,
    internal_id: &str,
    broker_id: &str,
    sent_at: DateTime<Utc>,
) -> Result<bool> {
    let mut tx = pool
        .begin()
        .await
        .context("outbox_mark_sent_with_broker_map begin failed")?;

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
    .execute(&mut *tx)
    .await
    .context("outbox_mark_sent_with_broker_map broker_map_upsert failed")?;

    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status      = 'SENT',
               sent_at_utc = coalesce(sent_at_utc, $2)
         where idempotency_key = $1
           and status in ('CLAIMED', 'DISPATCHING')
        returning outbox_id
        "#,
    )
    .bind(internal_id)
    .bind(sent_at)
    .fetch_optional(&mut *tx)
    .await
    .context("outbox_mark_sent_with_broker_map outbox_mark_sent failed")?;

    let Some((_outbox_id,)) = row else {
        return Ok(false);
    };

    tx.commit()
        .await
        .context("outbox_mark_sent_with_broker_map commit failed")?;

    Ok(true)
}

/// Mark an outbox row as ACKED.
/// Returns true if transitioned, false if not found.
pub async fn outbox_mark_acked(pool: &PgPool, idempotency_key: &str) -> Result<bool> {
    // Only SENT → ACKED is a valid transition.  Any other predecessor is an
    // explicit protocol violation and must return Err, not a silent Ok(false).
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status = 'ACKED'
         where idempotency_key = $1
           and status = 'SENT'
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_mark_acked failed")?;

    if row.is_some() {
        return Ok(true);
    }

    // Row was not updated.  Distinguish "already ACKED" (idempotent ok) from
    // "wrong predecessor state" (protocol violation → Err).
    let existing: Option<(String,)> =
        sqlx::query_as("SELECT status FROM oms_outbox WHERE idempotency_key = $1")
            .bind(idempotency_key)
            .fetch_optional(pool)
            .await
            .context("outbox_mark_acked status check failed")?;

    match existing {
        Some((status,)) if status == "ACKED" => Ok(false), // already acked; idempotent
        Some((status,)) => Err(anyhow!(
            "outbox_mark_acked: invalid transition from {status} to ACKED \
             (only SENT → ACKED is valid)"
        )),
        None => Ok(false), // row not found; caller can treat as no-op
    }
}

/// Mark a CLAIMED or DISPATCHING outbox row as FAILED.
///
/// Returns true if a row transitioned to FAILED; false otherwise.
/// Accepts both `CLAIMED` and `DISPATCHING` â€” use `outbox_claim_batch` first.
/// After RT-5, the production submit-failure path calls this with a DISPATCHING row.
pub async fn outbox_mark_failed(pool: &PgPool, idempotency_key: &str) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status = 'FAILED'
         where idempotency_key = $1
           and status in ('CLAIMED', 'DISPATCHING')
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_mark_failed failed")?;

    Ok(row.is_some())
}

/// Reset a `DISPATCHING` row back to `PENDING` for safe retry.
///
/// Used by the orchestrator when the broker adapter returns a retryable error
/// (`Transport` or `RateLimit`) â€” i.e., the request provably never reached the
/// broker.  Clears the claim fields so `outbox_claim_batch` can re-claim the
/// row on the next tick.
///
/// Returns `true` if the row was reset; `false` if not found or not
/// `DISPATCHING`.
pub async fn outbox_reset_dispatching_to_pending(
    pool: &PgPool,
    idempotency_key: &str,
) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status                 = 'PENDING',
               claimed_by             = null,
               claimed_at_utc         = null,
               dispatching_at_utc     = null,
               dispatch_attempt_id    = null
         where idempotency_key = $1
           and status = 'DISPATCHING'
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_reset_dispatching_to_pending failed")?;

    Ok(row.is_some())
}

/// A4: Transition a DISPATCHING outbox row to AMBIGUOUS explicit quarantine.
///
/// Called when `BrokerError::AmbiguousSubmit` is returned by the broker
/// adapter: the submit reached the broker transport layer but the outcome
/// is definitively unknown (timeout after send, partial ACK, connection drop
/// between send and receive).
///
/// Unlike `DISPATCHING` (which is also written for rows that crashed mid-
/// dispatch), `AMBIGUOUS` explicitly encodes "broker confirmed: outcome
/// unknown". It is structurally prevented from re-entering normal dispatch:
/// - `outbox_claim_batch` only claims `PENDING` rows â€” `AMBIGUOUS` is skipped.
/// - `outbox_load_restart_ambiguous_for_run` always returns `AMBIGUOUS` rows.
/// - The only exit is `outbox_reset_ambiguous_to_pending`.
///
/// Returns `true` if the row transitioned `DISPATCHING â†’ AMBIGUOUS`; `false`
/// if not found or not in `DISPATCHING` state.
pub async fn outbox_mark_ambiguous(pool: &PgPool, idempotency_key: &str) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status = 'AMBIGUOUS'
         where idempotency_key = $1
           and status = 'DISPATCHING'
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_mark_ambiguous failed")?;

    Ok(row.is_some())
}

/// A4: Release an AMBIGUOUS outbox row back to PENDING.
///
/// This is the ONLY safe path to re-enable dispatch for an order that was
/// quarantined by `outbox_mark_ambiguous`. It MUST only be called after:
/// - reconcile proof confirms the order was NOT accepted by the broker, OR
/// - an operator has verified the broker state and confirmed no live order
///   for this `idempotency_key` exists at the broker.
///
/// Clears all claim/dispatch metadata so `outbox_claim_batch` can re-claim
/// the row on the next tick after the run is re-armed.
///
/// Returns `true` if the row was released; `false` if not found or not in
/// `AMBIGUOUS` state (safe: calling this on a non-AMBIGUOUS row is a no-op).
pub async fn outbox_reset_ambiguous_to_pending(
    pool: &PgPool,
    idempotency_key: &str,
) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        update oms_outbox
           set status              = 'PENDING',
               claimed_by          = null,
               claimed_at_utc      = null,
               dispatching_at_utc  = null,
               dispatch_attempt_id = null
         where idempotency_key = $1
           and status = 'AMBIGUOUS'
        returning outbox_id
        "#,
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .context("outbox_reset_ambiguous_to_pending failed")?;

    Ok(row.is_some())
}

/// Recovery query: list outbox rows that are not terminal (not ACKED).
///
/// Includes PENDING, CLAIMED, DISPATCHING, SENT, FAILED, and AMBIGUOUS rows â€”
/// all statuses that indicate the order has not yet been confirmed by the broker.
///
/// NOTE: This does NOT talk to broker yet.
/// It provides the minimal deterministic input required for a future reconcile step.
pub async fn outbox_list_unacked_for_run(pool: &PgPool, run_id: Uuid) -> Result<Vec<OutboxRow>> {
    let rows = sqlx::query(
        r#"
        select outbox_id, run_id, idempotency_key, order_json, status,
               created_at_utc, sent_at_utc, claimed_at_utc, claimed_by,
               dispatching_at_utc, dispatch_attempt_id
        from oms_outbox
        where run_id = $1
          and status in ('PENDING','CLAIMED','DISPATCHING','SENT','FAILED','AMBIGUOUS')
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
            dispatching_at_utc: row.try_get("dispatching_at_utc")?,
            dispatch_attempt_id: row.try_get("dispatch_attempt_id")?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Arm state persistence â€” Patch L7
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
        values (1, $1, $2, now()) -- allow: ops-metadata
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
/// Returns `None` if no state has ever been persisted (fresh system â€” caller
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmState {
    Armed,
    Disarmed,
}

impl ArmState {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Armed => "ARMED",
            Self::Disarmed => "DISARMED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisarmReason {
    OperatorDisarm,
    OperatorHalt,
    ReconcileDrift,
    DeadmanExpired,
    DeadmanSupervisorFailure,
    DeadmanHeartbeatPersistFailed,
}

impl DisarmReason {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::OperatorDisarm => "OperatorDisarm",
            Self::OperatorHalt => "OperatorHalt",
            Self::ReconcileDrift => "ReconcileDrift",
            Self::DeadmanExpired => "DeadmanExpired",
            Self::DeadmanSupervisorFailure => "DeadmanSupervisorFailure",
            Self::DeadmanHeartbeatPersistFailed => "DeadmanHeartbeatPersistFailed",
        }
    }
}

pub async fn persist_arm_state_canonical(
    pool: &PgPool,
    state: ArmState,
    reason: Option<DisarmReason>,
) -> Result<()> {
    let reason_str = match state {
        ArmState::Armed => None,
        ArmState::Disarmed => reason.map(DisarmReason::as_db_str),
    };
    persist_arm_state(pool, state.as_db_str(), reason_str).await
}

#[derive(Debug, Clone)]
pub struct RiskBlockState {
    pub blocked: bool,
    pub reason: Option<String>,
    pub updated_at_utc: DateTime<Utc>,
}

/// Persist the current risk-block posture to `sys_risk_block_state` (singleton row).
pub async fn persist_risk_block_state(
    pool: &PgPool,
    blocked: bool,
    reason: Option<&str>,
    updated_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into sys_risk_block_state (sentinel_id, blocked, reason, updated_at_utc)
        values (1, $1, $2, $3)
        on conflict (sentinel_id) do update
            set blocked        = excluded.blocked,
                reason         = excluded.reason,
                updated_at_utc = excluded.updated_at_utc
        "#,
    )
    .bind(blocked)
    .bind(reason)
    .bind(updated_at)
    .execute(pool)
    .await
    .context("persist_risk_block_state failed")?;
    Ok(())
}

/// Load the last persisted risk-block posture.
pub async fn load_risk_block_state(pool: &PgPool) -> Result<Option<RiskBlockState>> {
    let row = sqlx::query(
        r#"
        select blocked, reason, updated_at_utc
        from sys_risk_block_state
        where sentinel_id = 1
        "#,
    )
    .fetch_optional(pool)
    .await
    .context("load_risk_block_state failed")?;

    let Some(row) = row else { return Ok(None) };
    Ok(Some(RiskBlockState {
        blocked: row.try_get("blocked")?,
        reason: row.try_get("reason")?,
        updated_at_utc: row.try_get("updated_at_utc")?,
    }))
}

// ---------------------------------------------------------------------------
// RD-01: Durable risk denial event history (sys_risk_denial_events)
// ---------------------------------------------------------------------------

/// One row from `sys_risk_denial_events`.
#[derive(Debug, Clone)]
pub struct RiskDenialEventRow {
    /// Deterministic display ID: `"{denied_at_utc_micros}:{rule_code}"`.
    /// Unique for all practical purposes within a deployment.
    pub id: String,
    /// UTC timestamp when the denial was captured.
    pub denied_at_utc: DateTime<Utc>,
    /// Machine-readable rule code, e.g. `"POSITION_LIMIT_EXCEEDED"`.
    pub rule: String,
    /// Human-readable one-line summary from `RiskReason::as_summary()`.
    pub message: String,
    /// Symbol from the denied order.
    pub symbol: Option<String>,
    /// Requested order quantity, if populated by the risk rule.
    pub requested_qty: Option<i64>,
    /// Configured limit that was breached, if populated by the risk rule.
    pub limit_qty: Option<i64>,
    /// Always `"critical"` for risk gate denials.
    pub severity: String,
}

/// Persist a single risk denial event to `sys_risk_denial_events`.
///
/// Uses `ON CONFLICT (id) DO NOTHING` — idempotent.  Repeated inserts for
/// the same deterministic `id` are silent no-ops, so best-effort writes from
/// the orchestrator can never double-count a denial.
pub async fn persist_risk_denial_event(pool: &PgPool, row: &RiskDenialEventRow) -> Result<()> {
    sqlx::query(
        r#"
        insert into sys_risk_denial_events
            (id, denied_at_utc, rule, message, symbol, requested_qty, limit_qty, severity)
        values ($1, $2, $3, $4, $5, $6, $7, $8)
        on conflict (id) do nothing
        "#,
    )
    .bind(&row.id)
    .bind(row.denied_at_utc)
    .bind(&row.rule)
    .bind(&row.message)
    .bind(&row.symbol)
    .bind(row.requested_qty)
    .bind(row.limit_qty)
    .bind(&row.severity)
    .execute(pool)
    .await
    .context("persist_risk_denial_event failed")?;
    Ok(())
}

/// Load the most recent `limit` risk denial events ordered newest-first.
///
/// Used by `GET /api/v1/risk/denials` to surface restart-safe denial history.
/// Returns an empty `Vec` if no denials have been recorded yet.
pub async fn load_recent_risk_denial_events(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<RiskDenialEventRow>> {
    let rows = sqlx::query(
        r#"
        select id, denied_at_utc, rule, message, symbol, requested_qty, limit_qty, severity
        from sys_risk_denial_events
        order by denied_at_utc desc
        limit $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("load_recent_risk_denial_events failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(RiskDenialEventRow {
            id: r.try_get("id")?,
            denied_at_utc: r.try_get("denied_at_utc")?,
            rule: r.try_get("rule")?,
            message: r.try_get("message")?,
            symbol: r.try_get("symbol")?,
            requested_qty: r.try_get("requested_qty")?,
            limit_qty: r.try_get("limit_qty")?,
            severity: r.try_get("severity")?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct ReconcileStatusState {
    pub status: String,
    pub last_run_at_utc: Option<DateTime<Utc>>,
    pub snapshot_watermark_ms: Option<i64>,
    pub mismatched_positions: i32,
    pub mismatched_orders: i32,
    pub mismatched_fills: i32,
    pub unmatched_broker_events: i32,
    pub note: Option<String>,
    pub updated_at_utc: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PersistReconcileStatusState<'a> {
    pub status: &'a str,
    pub last_run_at_utc: Option<DateTime<Utc>>,
    pub snapshot_watermark_ms: Option<i64>,
    pub mismatched_positions: i32,
    pub mismatched_orders: i32,
    pub mismatched_fills: i32,
    pub unmatched_broker_events: i32,
    pub note: Option<&'a str>,
    pub updated_at_utc: DateTime<Utc>,
}

/// Persist the current reconcile status posture to `sys_reconcile_status_state`.
pub async fn persist_reconcile_status_state(
    pool: &PgPool,
    state: &PersistReconcileStatusState<'_>,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into sys_reconcile_status_state (
            sentinel_id,
            status,
            last_run_at_utc,
            snapshot_watermark_ms,
            mismatched_positions,
            mismatched_orders,
            mismatched_fills,
            unmatched_broker_events,
            note,
            updated_at_utc
        )
        values (1, $1, $2, $3, $4, $5, $6, $7, $8, $9)
        on conflict (sentinel_id) do update
            set status = excluded.status,
                last_run_at_utc = excluded.last_run_at_utc,
                snapshot_watermark_ms = excluded.snapshot_watermark_ms,
                mismatched_positions = excluded.mismatched_positions,
                mismatched_orders = excluded.mismatched_orders,
                mismatched_fills = excluded.mismatched_fills,
                unmatched_broker_events = excluded.unmatched_broker_events,
                note = excluded.note,
                updated_at_utc = excluded.updated_at_utc
        "#,
    )
    .bind(state.status)
    .bind(state.last_run_at_utc)
    .bind(state.snapshot_watermark_ms)
    .bind(state.mismatched_positions)
    .bind(state.mismatched_orders)
    .bind(state.mismatched_fills)
    .bind(state.unmatched_broker_events)
    .bind(state.note)
    .bind(state.updated_at_utc)
    .execute(pool)
    .await
    .context("persist_reconcile_status_state failed")?;
    Ok(())
}

/// Load the last persisted reconcile status posture.
pub async fn load_reconcile_status_state(pool: &PgPool) -> Result<Option<ReconcileStatusState>> {
    let row = sqlx::query(
        r#"
        select status,
               last_run_at_utc,
               snapshot_watermark_ms,
               mismatched_positions,
               mismatched_orders,
               mismatched_fills,
               unmatched_broker_events,
               note,
               updated_at_utc
        from sys_reconcile_status_state
        where sentinel_id = 1
        "#,
    )
    .fetch_optional(pool)
    .await
    .context("load_reconcile_status_state failed")?;

    let Some(row) = row else { return Ok(None) };
    Ok(Some(ReconcileStatusState {
        status: row.try_get("status")?,
        last_run_at_utc: row.try_get("last_run_at_utc")?,
        snapshot_watermark_ms: row.try_get("snapshot_watermark_ms")?,
        mismatched_positions: row.try_get("mismatched_positions")?,
        mismatched_orders: row.try_get("mismatched_orders")?,
        mismatched_fills: row.try_get("mismatched_fills")?,
        unmatched_broker_events: row.try_get("unmatched_broker_events")?,
        note: row.try_get("note")?,
        updated_at_utc: row.try_get("updated_at_utc")?,
    }))
}

/// Insert a broker message/fill into oms_inbox with dedupe on (run_id, broker_message_id).
///
/// Idempotent behavior:
/// - If (run_id, broker_message_id) already exists, returns Ok(false) and does NOT create a
///   second row.
/// - If inserted, returns Ok(true).
///
/// RT-3: dedupe is scoped to the run â€” the same broker_message_id can appear in different
/// runs without collision (broker IDs are only unique within a session).
///
/// Patch D2 caller contract:
/// ```text
/// let inserted = inbox_insert_deduped(pool, run_id, msg_id, json).await?;
/// if inserted {
///     apply_fill_to_portfolio(json);                   // idempotent apply
///     inbox_mark_applied(pool, run_id, msg_id).await?; // journal completion
/// }
/// ```
/// On crash between insert and mark_applied: the row surfaces in
/// `inbox_load_unapplied_for_run` for recovery replay.
pub async fn inbox_insert_deduped(
    pool: &PgPool,
    run_id: Uuid,
    broker_message_id: &str,
    message_json: serde_json::Value,
) -> Result<bool> {
    // Legacy compatibility shim:
    // older callers only provide (run_id, broker_message_id, message_json).
    // Derive the richer identity fields best-effort from the payload, then
    // delegate to the canonical insert path.

    let broker_fill_id = message_json.get("broker_fill_id").and_then(|v| v.as_str());

    let internal_order_id = message_json
        .get("internal_order_id")
        .or_else(|| message_json.get("order_id"))
        .or_else(|| message_json.get("client_order_id"))
        .and_then(|v| v.as_str())
        .unwrap_or(broker_message_id);

    let broker_order_id = message_json
        .get("broker_order_id")
        .or_else(|| message_json.get("order_id"))
        .or_else(|| message_json.get("client_order_id"))
        .and_then(|v| v.as_str())
        .unwrap_or(internal_order_id);

    let event_kind = message_json
        .get("event_kind")
        .or_else(|| message_json.get("kind"))
        .or_else(|| message_json.get("event_type"))
        .or_else(|| message_json.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("UNKNOWN");

    let event_ts_ms = message_json
        .get("event_ts_ms")
        .or_else(|| message_json.get("ts_ms"))
        .or_else(|| message_json.get("timestamp_ms"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let received_at = message_json
        .get("received_at_utc")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|| DateTime::<Utc>::from_timestamp_millis(event_ts_ms)) // allow: ops-metadata — parsing stored event millis, not a wall-clock read
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);

    inbox_insert_deduped_with_identity(
        pool,
        run_id,
        broker_message_id,
        broker_fill_id,
        internal_order_id,
        broker_order_id,
        event_kind,
        &message_json,
        event_ts_ms,
        received_at,
    )
    .await
}

/// Insert a broker message/fill into oms_inbox with explicit identity fields.
///
/// Dedupe rule is transport-only and explicit:
/// - conflict key: `(run_id, broker_message_id)`
/// - `broker_fill_id` is optional economic identity metadata and does NOT
///   participate in inbox insertion dedupe.
#[allow(clippy::too_many_arguments)]
pub async fn inbox_insert_deduped_with_identity(
    pool: &PgPool,
    run_id: Uuid,
    broker_message_id: &str,
    broker_fill_id: Option<&str>,
    internal_order_id: &str,
    broker_order_id: &str,
    event_kind: &str,
    event_json: &serde_json::Value,
    event_ts_ms: i64,
    received_at: DateTime<Utc>,
) -> Result<bool> {
    let insert_result = sqlx::query(
        r#"
        insert into oms_inbox (
            run_id,
            broker_message_id,
            broker_fill_id,
            internal_order_id,
            broker_order_id,
            event_kind,
            message_json,
            event_ts_ms,
            received_at_utc,
            applied_at_utc
        )
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9, null)
        "#,
    )
    .bind(run_id)
    .bind(broker_message_id)
    .bind(broker_fill_id)
    .bind(internal_order_id)
    .bind(broker_order_id)
    .bind(event_kind)
    .bind(event_json)
    .bind(event_ts_ms)
    .bind(received_at)
    .execute(pool)
    .await;

    match insert_result {
        Ok(done) => Ok(done.rows_affected() == 1),

        Err(sqlx::Error::Database(db_err))
            if db_err.code().as_deref() == Some("23505")
                && matches!(
                    db_err.constraint(),
                    Some("uq_inbox_run_broker_message_id")
                        | Some("uq_inbox_run_message")
                        | Some("uq_inbox_run_broker_fill_id")
                ) =>
        {
            Ok(false)
        }

        Err(e) => Err(e).context("inbox_insert_deduped_with_identity failed"),
    }
}
/// Stamp `applied_at_utc` on an inbox row after its fill has been
/// successfully applied to in-process portfolio state.
///
/// Part of the Patch D2 crash-recovery contract:
/// - Call this immediately after the portfolio apply completes.
/// - Rows where `applied_at_utc IS NULL` appear in
///   `inbox_load_unapplied_for_run` and must be replayed at startup.
///
/// RT-3: `run_id` is now required â€” dedupe is scoped to (run_id, broker_message_id).
///
/// `applied_at` is caller-supplied â€” no SQL `now()` in this function (FC-8
/// policy: wall-clock excluded from the fill-apply path).  In production,
/// pass `time_source.now_utc()`; in tests, pass an explicit timestamp.
///
/// Idempotent: silently succeeds if (run_id, broker_message_id) is not present
/// or has already been stamped.
pub async fn inbox_mark_applied(
    pool: &PgPool,
    run_id: Uuid,
    broker_message_id: &str,
    applied_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        r#"
        update oms_inbox
           set applied_at_utc = $3
         where run_id = $1
           and broker_message_id = $2
           and applied_at_utc is null
        "#,
    )
    .bind(run_id)
    .bind(broker_message_id)
    .bind(applied_at)
    .execute(pool)
    .await
    .context("inbox_mark_applied failed")?;
    Ok(())
}

/// Load inbox rows for a run that were received but not yet applied
/// (`applied_at_utc IS NULL`).
///
/// Call this at startup/recovery to identify fills whose apply step did not
/// complete before a crash. Replay these events in canonical durable ingest
/// order (`inbox_id ASC`), independent of `broker_message_id`; each apply must
/// be idempotent so re-applying a partially-applied fill is safe. After
/// successfully applying each row, call `inbox_mark_applied`.
///
/// Uses the partial index `idx_inbox_run_unapplied` for efficiency.
pub async fn inbox_load_unapplied_for_run(pool: &PgPool, run_id: Uuid) -> Result<Vec<InboxRow>> {
    let rows = sqlx::query(
        r#"
        select inbox_id, run_id, broker_message_id, broker_fill_id,
               broker_sequence_id, broker_timestamp, message_json,
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
            broker_fill_id: row.try_get("broker_fill_id")?,
            broker_sequence_id: row.try_get("broker_sequence_id")?,
            broker_timestamp: row.try_get("broker_timestamp")?,
            message_json: row.try_get("message_json")?,
            received_at_utc: row.try_get("received_at_utc")?,
            applied_at_utc: row.try_get("applied_at_utc")?,
        });
    }
    Ok(out)
}

/// Load outbox rows with status SENT or ACKED (submitted to broker), ordered
/// by outbox_id asc.  Used at cold-start to reconstruct the in-flight OMS
/// order map without querying the broker.
pub async fn outbox_load_submitted_for_run(pool: &PgPool, run_id: Uuid) -> Result<Vec<OutboxRow>> {
    let rows = sqlx::query(
        r#"
        select outbox_id, run_id, idempotency_key, order_json, status,
               created_at_utc, sent_at_utc, claimed_at_utc, claimed_by,
               dispatching_at_utc, dispatch_attempt_id
          from oms_outbox
         where run_id = $1
           and status in ('SENT', 'ACKED')
         order by outbox_id asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("outbox_load_submitted_for_run failed")?;

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
            dispatching_at_utc: row.try_get("dispatching_at_utc")?,
            dispatch_attempt_id: row.try_get("dispatch_attempt_id")?,
        });
    }
    Ok(out)
}

/// Load all applied inbox rows (`applied_at_utc IS NOT NULL`), ordered by
/// inbox_id asc.  Used at cold-start to replay fills into the portfolio and
/// advance OMS order state.  Disjoint from the unapplied set processed by
/// Phase 3, so no double-apply risk.
pub async fn inbox_load_all_applied_for_run(pool: &PgPool, run_id: Uuid) -> Result<Vec<InboxRow>> {
    let rows = sqlx::query(
        r#"
        select inbox_id, run_id, broker_message_id, broker_fill_id,
               broker_sequence_id, broker_timestamp, message_json,
               received_at_utc, applied_at_utc
          from oms_inbox
         where run_id = $1
           and applied_at_utc is not null
         order by inbox_id asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("inbox_load_all_applied_for_run failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(InboxRow {
            inbox_id: row.try_get("inbox_id")?,
            run_id: row.try_get("run_id")?,
            broker_message_id: row.try_get("broker_message_id")?,
            broker_fill_id: row.try_get("broker_fill_id")?,
            broker_sequence_id: row.try_get("broker_sequence_id")?,
            broker_timestamp: row.try_get("broker_timestamp")?,
            message_json: row.try_get("message_json")?,
            received_at_utc: row.try_get("received_at_utc")?,
            applied_at_utc: row.try_get("applied_at_utc")?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Broker order ID map persistence â€” Patch A4
// ---------------------------------------------------------------------------

/// Persist (or update) an `internal_id â†’ broker_id` mapping after a successful
/// broker submit.
///
/// Uses `ON CONFLICT â€¦ DO UPDATE` so idempotent retries (e.g. after a crash
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

/// Remove an `internal_id â†’ broker_id` mapping when an order reaches a terminal
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

/// Load all live `internal_id â†’ broker_id` pairs from DB.
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
// Reconcile checkpoint â€” Patch B1
// ---------------------------------------------------------------------------

/// A persisted reconcile checkpoint written by the reconcile engine.
///
/// `arm_preflight` checks this table (not `audit_events`) for reconcile
/// cleanliness.  A CLEAN verdict here requires the reconcile engine to have
/// called `reconcile_checkpoint_write` â€” inserting a fake audit event is
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
/// `now` is injected by the caller (D1-4: schema DEFAULT now() removed from
/// created_at_utc; caller binds the timestamp explicitly).
pub async fn reconcile_checkpoint_write(
    pool: &PgPool,
    run_id: Uuid,
    verdict: &str,
    snapshot_watermark_ms: i64,
    result_hash: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into sys_reconcile_checkpoint
            (run_id, verdict, snapshot_watermark_ms, result_hash, created_at_utc)
        values ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(run_id)
    .bind(verdict)
    .bind(snapshot_watermark_ms)
    .bind(result_hash)
    .bind(now)
    .execute(pool)
    .await
    .context("reconcile_checkpoint_write failed")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Broker event cursor â€” Patch A2
// ---------------------------------------------------------------------------

/// Load the persisted broker event cursor for the given adapter.
///
/// Returns `None` if no cursor has been persisted yet (fresh system or first
/// run for this adapter).  The orchestrator treats `None` as "start from the
/// beginning", which is always safe because `oms_inbox` deduplicates events by
/// `(run_id, broker_message_id)`.
pub async fn load_broker_cursor(pool: &PgPool, adapter_id: &str) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        r#"
        select cursor_value
        from broker_event_cursor
        where adapter_id = $1
        "#,
    )
    .bind(adapter_id)
    .fetch_optional(pool)
    .await
    .context("load_broker_cursor failed")?;
    Ok(row.map(|(v,)| v))
}

/// Advance (upsert) the broker event cursor for the given adapter.
///
/// Called by the orchestrator ONLY after all events in the current batch have
/// been persisted to `oms_inbox`.  Ordering contract:
///   1. `inbox_insert_deduped` succeeds for every event in the batch.
///   2. `advance_broker_cursor` persists the new cursor.
///
/// If the process crashes between steps 1 and 2 the cursor is NOT advanced.
/// On restart the orchestrator re-fetches from the old cursor and `oms_inbox`
/// dedup prevents double-apply.
///
/// `updated_at` is caller-supplied (no SQL `now()`; D1 policy).
pub async fn advance_broker_cursor(
    pool: &PgPool,
    adapter_id: &str,
    cursor_value: &str,
    updated_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into broker_event_cursor (adapter_id, cursor_value, updated_at)
        values ($1, $2, $3)
        on conflict (adapter_id) do update
            set cursor_value = excluded.cursor_value,
                updated_at   = excluded.updated_at
        "#,
    )
    .bind(adapter_id)
    .bind(cursor_value)
    .bind(updated_at)
    .execute(pool)
    .await
    .context("advance_broker_cursor failed")?;
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
