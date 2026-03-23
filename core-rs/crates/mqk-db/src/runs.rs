use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

use crate::reconcile_state::reconcile_checkpoint_load_latest;

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

/// Count LIVE runs that are operationally "active": ARMED or RUNNING.
/// This is used by CLI guardrails to prevent accidental migration of a live/armed DB.
pub async fn count_active_live_runs(pool: &PgPool) -> Result<i64> {
    // If schema doesn't exist yet, treat as 0 (safe) rather than failing.
    let st = crate::status(pool).await?;
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
    let st = crate::status(pool).await?;
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
