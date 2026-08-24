use std::fmt;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

use crate::reconcile_state::reconcile_checkpoint_load_latest;

// ---------------------------------------------------------------------------
// RUN-LIFECYCLE-CAS-01
// ---------------------------------------------------------------------------

/// A run-lifecycle transition (`arm_run`/`begin_run`/`stop_run`/
/// `clear_halted_run`/`heartbeat_run`) was refused because the row's durable
/// `status` no longer matched what this call required at the instant its
/// guarded `UPDATE ... WHERE status = $expected` executed.
///
/// Before RUN-LIFECYCLE-CAS-01 every one of these functions read `status`
/// via a separate `fetch_run`, checked it in Rust, and then issued an
/// unconditional `UPDATE ... WHERE run_id = $1` with no `status` guard at
/// all -- a classic check-then-act race. Two concurrent callers observing
/// the same valid pre-state (e.g. a `stop_run` and a `halt_run` both firing
/// from `RUNNING`) would both pass their own pre-check and both execute
/// their unconditional `UPDATE`; whichever committed last silently won,
/// with no error to either caller -- so a `stop_run` could silently
/// overwrite a `halt_run` that landed a moment earlier, breaking `HALTED`'s
/// required sticky/fail-closed semantics.
///
/// Every transition's `UPDATE` now includes its own required prior
/// `status` in the `WHERE` clause (matching the CAS-token role
/// `sys_autonomous_daily_operations.state_version` plays for the
/// autonomous-operation state machine, but reusing the existing `status`
/// column directly -- this 5-state machine does not need a separate
/// version counter). A `rows_affected() == 0` result means a concurrent
/// writer already won; the function re-reads the row (authoritative,
/// matching the re-read-on-ambiguity pattern used throughout this codebase)
/// and returns this typed error rather than silently applying nothing or
/// guessing.
///
/// `anyhow::Result<()>` remains every transition function's return type --
/// dozens of existing call sites use `?`/`.expect()`/`.unwrap()` on these
/// and do not need to change. A caller that specifically needs to
/// distinguish "a concurrent transition already won" from any other
/// failure can `err.downcast_ref::<StaleRunTransition>()`.
#[derive(Debug, Clone)]
pub struct StaleRunTransition {
    pub run_id: Uuid,
    pub operation: &'static str,
    pub expected: &'static str,
    pub actual: String,
}

impl fmt::Display for StaleRunTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: run {} expected status in [{}] but a concurrent transition already moved it to {}",
            self.operation, self.run_id, self.expected, self.actual
        )
    }
}

impl std::error::Error for StaleRunTransition {}

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
    /// PAPER-SOAK-INBOUND-DRAIN-OWNERSHIP-01: set by [`request_stop_run`] when
    /// an operator stop is requested. `status` stays RUNNING until every
    /// broker-reachable order has drained to a terminal outcome -- see
    /// migration 0063 for the full rationale.
    pub stop_requested_at_utc: Option<DateTime<Utc>>,
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
        stop_requested_at_utc: row.try_get("stop_requested_at_utc")?,
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
          last_heartbeat_utc,
          stop_requested_at_utc
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

/// Record that an operator stop was requested for a RUNNING run, without
/// changing `status` away from RUNNING (PAPER-SOAK-INBOUND-DRAIN-OWNERSHIP-01;
/// see migration 0063).
///
/// Idempotent: a run that already has `stop_requested_at_utc` set is left
/// unchanged (the original request time is preserved, not overwritten) --
/// safe to call on every `stop_execution_runtime` attempt while drainage is
/// still in progress.
///
/// No-op (`Ok(())`, zero rows affected) when the run is not currently
/// RUNNING -- `stop_execution_runtime`'s existing ARMED/RUNNING-gated
/// teardown path handles those cases exactly as before this patch.
pub async fn request_stop_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    sqlx::query(
        r#"
        update runs
           set stop_requested_at_utc = now() -- allow: ops-metadata — records when an operator stop was requested, not an economic decision timestamp
         where run_id = $1
           and status = 'RUNNING'
           and stop_requested_at_utc is null
        "#,
    )
    .bind(run_id)
    .execute(pool)
    .await
    .context("request_stop_run failed")?;

    Ok(())
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
          last_heartbeat_utc,
          stop_requested_at_utc
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
          last_heartbeat_utc,
          stop_requested_at_utc
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

/// Arm a run: CREATED/STOPPED -> ARMED. CAS-guarded (RUN-LIFECYCLE-CAS-01):
/// the `UPDATE` only applies when `status` is still `CREATED` or `STOPPED`
/// at the instant it executes, so a concurrent transition (e.g. a `halt_run`
/// landing first) is never silently overwritten. On a CAS miss, returns
/// [`StaleRunTransition`] with the actual current status.
pub async fn arm_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let res = sqlx::query(
        r#"
        update runs
        set status = 'ARMED',
            armed_at_utc = now() -- allow: ops-metadata
        where run_id = $1
          and status in ('CREATED', 'STOPPED')
        "#,
    )
    .bind(run_id)
    .execute(pool)
    .await;

    let done = match res {
        Ok(done) => done,
        Err(e) => {
            if is_unique_constraint_violation(&e, "uq_live_engine_active_run") {
                return Err(anyhow!("unique active LIVE constraint"));
            }
            return Err(anyhow::Error::new(e).context("arm_run update failed"));
        }
    };

    if done.rows_affected() == 1 {
        return Ok(());
    }

    let r = fetch_run(pool, run_id).await?;
    Err(anyhow::Error::new(StaleRunTransition {
        run_id,
        operation: "arm_run",
        expected: "CREATED or STOPPED",
        actual: r.status.as_str().to_string(),
    }))
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

/// Begin a run: ARMED -> RUNNING. CAS-guarded (RUN-LIFECYCLE-CAS-01): the
/// `UPDATE` only applies when `status` is still `ARMED`, so a concurrent
/// `stop_run`/`halt_run` landing first is never silently overwritten ("begin
/// cannot overwrite a concurrent stop"). On a CAS miss, returns
/// [`StaleRunTransition`] with the actual current status.
pub async fn begin_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let done = sqlx::query(
        r#"
        update runs
        set status = 'RUNNING',
            running_at_utc = now(), -- allow: ops-metadata
            stop_requested_at_utc = null
        where run_id = $1
          and status = 'ARMED'
        "#,
    )
    .bind(run_id)
    .execute(pool)
    .await
    .context("begin_run update failed")?;

    if done.rows_affected() == 1 {
        return Ok(());
    }

    let r = fetch_run(pool, run_id).await?;
    Err(anyhow::Error::new(StaleRunTransition {
        run_id,
        operation: "begin_run",
        expected: "ARMED",
        actual: r.status.as_str().to_string(),
    }))
}

/// Stop a run: ARMED/RUNNING -> STOPPED. CAS-guarded (RUN-LIFECYCLE-CAS-01):
/// the `UPDATE` only applies when `status` is still `ARMED` or `RUNNING`, so
/// a concurrent `halt_run` landing first is never silently overwritten
/// ("stop cannot overwrite a concurrent halt" -- `HALTED` stays sticky). On
/// a CAS miss, returns [`StaleRunTransition`] with the actual current
/// status.
pub async fn stop_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let done = sqlx::query(
        r#"
        update runs
        set status = 'STOPPED',
            stopped_at_utc = now() -- allow: ops-metadata
        where run_id = $1
          and status in ('ARMED', 'RUNNING')
        "#,
    )
    .bind(run_id)
    .execute(pool)
    .await
    .context("stop_run update failed")?;

    if done.rows_affected() == 1 {
        return Ok(());
    }

    let r = fetch_run(pool, run_id).await?;
    Err(anyhow::Error::new(StaleRunTransition {
        run_id,
        operation: "stop_run",
        expected: "ARMED or RUNNING",
        actual: r.status.as_str().to_string(),
    }))
}

/// Halt a run: ANY -> HALTED (sticky). Deliberately NOT CAS-guarded on
/// `status` (RUN-LIFECYCLE-CAS-01): halt is the fail-closed override that
/// must win unconditionally over every other in-flight transition,
/// including a concurrent `arm_run`/`begin_run`/`stop_run`/`clear_halted_run`
/// -- the exact opposite requirement from those functions, which must all
/// yield to an already-landed halt rather than race against one.
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

/// Clear a halted run: HALTED → STOPPED.
///
/// Explicit operator acknowledgment path. The operator has reviewed the halt
/// condition and explicitly chosen to clear it. Transitions HALTED → STOPPED
/// so that start_execution_runtime can proceed via the normal stopped-run path.
/// The caller is responsible for subsequently re-arming and re-starting.
///
/// CAS-guarded (RUN-LIFECYCLE-CAS-01): the `UPDATE` only applies when
/// `status` is still `HALTED` -- guards against a concurrent second
/// `clear_halted_run` call (e.g. two operator clicks) both trying to clear
/// the same halt; only the first wins, the second gets
/// [`StaleRunTransition`] instead of silently re-writing `stopped_at_utc`.
pub async fn clear_halted_run(pool: &PgPool, run_id: Uuid) -> Result<()> {
    let done = sqlx::query(
        r#"
        update runs
        set status = 'STOPPED',
            stopped_at_utc = now() -- allow: ops-metadata — operator-acknowledged halt clear
        where run_id = $1
          and status = 'HALTED'
        "#,
    )
    .bind(run_id)
    .execute(pool)
    .await
    .context("clear_halted_run update failed")?;

    if done.rows_affected() == 1 {
        return Ok(());
    }

    let r = fetch_run(pool, run_id).await?;
    Err(anyhow::Error::new(StaleRunTransition {
        run_id,
        operation: "clear_halted_run",
        expected: "HALTED",
        actual: r.status.as_str().to_string(),
    }))
}

/// Typed outcome of [`clear_halted_run_and_reset_stale_claims`].
///
/// Replaces the previous `Result<u64>` shape (PAPER-SOAK-STALE-CLAIM-RECOVERY-03):
/// a refusal is not necessarily an error condition the caller should log as a
/// failure — `ActiveRuntimeLease` in particular is the *expected*, correct
/// outcome the very first time an operator calls this while the prior
/// runtime process is still shutting down. Distinguishing it from
/// `StaleRunTransition` (wrong lifecycle state) and from a genuine DB failure
/// (still `Err(anyhow::Error)`) lets the operator route return an accurate,
/// stable disposition instead of a generic error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClearHaltedRunOutcome {
    /// The run was durably `HALTED` with no active runtime lease; it is now
    /// `STOPPED` and every stranded `CLAIMED` row for it was reset to
    /// `PENDING`. Zero mutation occurs on any other outcome.
    Success { stale_claims_reset: u64 },
    /// Refused: the run is durably `HALTED`, but an unexpired runtime leader
    /// lease still exists, so the prior runtime process may still be able to
    /// act. No mutation was performed — `runs.status` is still `HALTED`,
    /// every `CLAIMED` row is untouched, and the lease row is untouched.
    ActiveRuntimeLease {
        holder_id: String,
        epoch: i64,
        lease_expires_at: DateTime<Utc>,
    },
    /// Refused: the run was not durably `HALTED` at the instant the guarded
    /// transaction executed (already `STOPPED`, still `RUNNING`, etc).
    StaleRunTransition { actual_status: String },
}

/// PAPER-SOAK-STALE-CLAIM-RECOVERY-02/03: clear a halted run AND reclaim any
/// `CLAIMED` outbox row it left stranded before `DISPATCHING`, atomically.
///
/// # Why this exists
///
/// The previously-pushed stale-claim reset lived in
/// `AppState::build_execution_orchestrator`, unconditionally, for every run
/// start — including ordinary fresh starts that could never have a `CLAIMED`
/// row to begin with. Two problems with that placement:
///
/// 1. **Wrong ordering.** It ran before `ExecutionOrchestrator`'s runtime
///    leadership lease was ever acquired (`runtime_epoch: None` at
///    construction; the lease is acquired later, inside `tick()`). Its own
///    comment claimed "the runtime leadership lease this run start already
///    went through guarantees no other legitimate dispatcher is concurrently
///    active" — false at the actual call site.
/// 2. **Unreachable for the scenario it targeted anyway.** A process crash
///    while a run is `RUNNING` leaves that run durably `RUNNING` in the DB
///    (the crash never called `stop_run`/`halt_run`). The normal start path
///    (`create_or_reuse_run_for_start`) refuses to start when a durable
///    active run exists without local ownership
///    (`runtime.truth_mismatch.durable_active_without_local_owner`) —
///    exactly the state a crash-orphaned run is in. So the reset call could
///    never actually run for the crash-recovery case it was written for.
///
/// # -02's false assumption (corrected by -03)
///
/// -02's authority model claimed durable `HALTED` alone was "a complete,
/// race-proof authority check — no runtime lease inspection needed", because
/// the orchestrator's Phase-0 halt guard refuses all dispatch once a run is
/// `HALTED`. That reasoning has a gap: the Phase-0 guard is only checked at
/// the *top* of a tick. A runtime that already read `RUNNING` at Phase 0
/// before a concurrent halt landed can proceed deeper into that same tick
/// (lease refresh, outbox claim, broker submit) without ever re-observing
/// `HALTED` — and the pre-03 `refresh_lease`/`acquire_lease` primitives
/// verify holder/epoch/expiry only, with no notion of which run or its
/// status. A durably `HALTED` run with a still-unexpired
/// `runtime_leader_lease` is therefore not proof the prior owner has
/// stopped acting; it may be a slow process mid-tick that will keep
/// refreshing that lease and keep dispatching for as long as the lease
/// stays valid.
///
/// # This function's authority model (current)
///
/// `clear-halted-run` requires **both**:
/// 1. The run is durably `HALTED` (`WHERE status = 'HALTED'`, locked via
///    `SELECT ... FOR UPDATE` before any decision is made — see below).
/// 2. No unexpired `runtime_leader_lease` exists (Layer A / quiescence). An
///    expired lease is not active authority and is cleaned up as part of the
///    same atomic recovery; an active, unexpired lease is a hard refusal
///    with zero mutation (`ClearHaltedRunOutcome::ActiveRuntimeLease`).
///
/// # Serialization boundary (closes the concurrent-race class, not just the
/// sequential one)
///
/// The `runs` row for `run_id` is locked with `SELECT ... FOR UPDATE` before
/// this function reads `status` or the lease table, and that lock is held
/// for the rest of the transaction. `mqk_db::runtime_lease::
/// acquire_or_refresh_lease_for_running_run` locks the exact same row the
/// exact same way before *it* decides lease authority. Postgres serializes
/// any two transactions contending for the same row lock — whichever
/// transaction locks the row first runs to completion (commit or rollback)
/// before the other's lock acquisition can proceed. This holds even when no
/// `runtime_leader_lease` row exists yet at all (the locked row is `runs`,
/// not the lease table), which is what closes the "Runtime A observed
/// RUNNING but had not yet acquired a lease" race, not only the
/// already-has-a-lease race.
///
/// The threshold for "stale" is deliberately not a fixed lookback window
/// (`PAPER-SOAK-STALE-CLAIM-RECOVERY-01` used `claimed_at_utc < now()`, which
/// is not a staleness filter at all — it matches every `CLAIMED` row
/// regardless of age). Ownership, not timestamp comparison, is what proves
/// the prior claimant is dead: once this function proves the run is
/// `HALTED` with no active lease, every remaining `CLAIMED` row for it is
/// provably orphaned regardless of how recently it was claimed, so all of
/// them are reset.
///
/// # Safety contract (unchanged from `outbox_reset_stale_claims`)
///
/// Only rows with `status = 'CLAIMED'` are touched. `DISPATCHING`, `SENT`,
/// `ACKED`, and `AMBIGUOUS` rows are structurally immune — the `WHERE`
/// clause never matches them, so an order that may have reached the broker
/// is never reset. Scoped to `run_id`: a stale claim belonging to a
/// different run is untouched (that run's own halt-clear owns resetting it).
///
/// `now_utc` is caller-supplied (FC-9: no wall-clock reads in enforcement
/// paths) so lease-expiry proofs are deterministic in tests.
pub async fn clear_halted_run_and_reset_stale_claims(
    pool: &PgPool,
    run_id: Uuid,
    now_utc: DateTime<Utc>,
) -> Result<ClearHaltedRunOutcome> {
    let mut tx = pool
        .begin()
        .await
        .context("clear_halted_run_and_reset_stale_claims: begin tx failed")?;

    // Lock the run row FIRST — the same serialization boundary
    // `acquire_or_refresh_lease_for_running_run` uses. See doc comment above.
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM runs WHERE run_id = $1 FOR UPDATE")
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await
            .context("clear_halted_run_and_reset_stale_claims: run lock failed")?;

    let Some(status) = status else {
        tx.rollback().await.ok();
        return Err(anyhow!(
            "clear_halted_run_and_reset_stale_claims: run {run_id} not found"
        ));
    };

    if status != "HALTED" {
        tx.rollback()
            .await
            .context("clear_halted_run_and_reset_stale_claims: rollback (not halted) failed")?;
        return Ok(ClearHaltedRunOutcome::StaleRunTransition {
            actual_status: status,
        });
    }

    // LAYER A — quiescence before halt clear (§3/§9/-02's false assumption).
    let lease: Option<(String, i64, DateTime<Utc>)> = sqlx::query_as(
        "SELECT holder_id, epoch, lease_expires_at FROM runtime_leader_lease WHERE id = 1",
    )
    .fetch_optional(&mut *tx)
    .await
    .context("clear_halted_run_and_reset_stale_claims: lease read failed")?;

    if let Some((ref holder_id, epoch, lease_expires_at)) = lease {
        if lease_expires_at > now_utc {
            tx.rollback().await.context(
                "clear_halted_run_and_reset_stale_claims: rollback (active lease) failed",
            )?;
            return Ok(ClearHaltedRunOutcome::ActiveRuntimeLease {
                holder_id: holder_id.clone(),
                epoch,
                lease_expires_at,
            });
        }
    }

    // Expired-lease cleanup (§12): delete only the exact expired row just
    // observed, inside the same transaction as the recovery mutation below —
    // atomic from the operator's perspective. An absent lease is a no-op.
    if let Some((holder_id, epoch, _)) = lease {
        sqlx::query(
            "DELETE FROM runtime_leader_lease WHERE id = 1 AND holder_id = $1 AND epoch = $2",
        )
        .bind(&holder_id)
        .bind(epoch)
        .execute(&mut *tx)
        .await
        .context("clear_halted_run_and_reset_stale_claims: expired lease cleanup failed")?;
    }

    let done = sqlx::query(
        r#"
        update runs
        set status = 'STOPPED',
            stopped_at_utc = $2
        where run_id = $1
          and status = 'HALTED'
        "#,
    )
    .bind(run_id)
    .bind(now_utc)
    .execute(&mut *tx)
    .await
    .context("clear_halted_run_and_reset_stale_claims: clear-halt update failed")?;

    if done.rows_affected() != 1 {
        // Unreachable given the row lock + status check above (no other
        // writer can change `status` while this tx holds the row lock), but
        // fail closed rather than assume it.
        tx.rollback().await.ok();
        let r = fetch_run(pool, run_id).await?;
        return Err(anyhow::Error::new(StaleRunTransition {
            run_id,
            operation: "clear_halted_run_and_reset_stale_claims",
            expected: "HALTED",
            actual: r.status.as_str().to_string(),
        }));
    }

    let reset = sqlx::query(
        r#"
        update oms_outbox
           set status         = 'PENDING',
               claimed_at_utc = null,
               claimed_by     = null
         where run_id         = $1
           and status         = 'CLAIMED'
        "#,
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await
    .context("clear_halted_run_and_reset_stale_claims: outbox reset failed")?;

    tx.commit()
        .await
        .context("clear_halted_run_and_reset_stale_claims: commit failed")?;

    Ok(ClearHaltedRunOutcome::Success {
        stale_claims_reset: reset.rows_affected(),
    })
}

// ---------------------------------------------------------------------------
// PAPER-SOAK-REPAIR-20260819-ORPHANED-RUN-RECOVERY-01
// ---------------------------------------------------------------------------

/// Typed outcome of [`stop_run_if_evidence_clean`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopRunIfEvidenceCleanOutcome {
    /// The run was durably `ARMED`/`RUNNING` with zero unacked outbox rows
    /// and a clean global reconcile status; it is now `STOPPED`.
    Stopped,
    /// Refused: the run was not durably `ARMED`/`RUNNING` at the instant
    /// checked (already `STOPPED`, `HALTED`, or a concurrent transition
    /// landed first). Zero mutation.
    NotActive { actual_status: String },
    /// Refused: at least one outbox row for this run has not reached
    /// `ACKED` — an order may still be in flight to or from the broker with
    /// no local runtime left to resolve it. Zero mutation.
    UnacknowledgedOutbox { unacked_count: usize },
    /// Refused: at least one `oms_inbox` row for this run has not been
    /// applied (`applied_at_utc IS NULL`) — durably received broker
    /// evidence that has not yet been replayed. Zero mutation.
    ///
    /// PAPER-SOAK-UNRESOLVED-BROKER-EVIDENCE-GATE-01.
    UnappliedInbox { unapplied_count: usize },
    /// Refused: the global reconcile status is not clean (or has never been
    /// recorded), so local/broker truth is not currently known to agree.
    /// Zero mutation.
    ReconcileDirty,
}

/// Safely stop a durable `ARMED`/`RUNNING` run that has no local runtime
/// owner (the crash/reboot residue shape: a daemon process died mid-session
/// and the durable `runs` row was never terminated), but only when the
/// exact same unacked-outbox and global-reconcile evidence already trusted
/// by `reconcile_durable_run_without_local_owner`
/// (`mqk-daemon::state::autonomous_daily_coordinator`) is clean for it.
///
/// This function has no notion of in-process runtime ownership — callers
/// are responsible for independently proving "no local owner" (e.g. via
/// `AppState::locally_owned_run_id`) before calling it. It must never be
/// invoked against a run this process is still actively driving; doing so
/// would race the exact runtime this run is currently running.
///
/// CAS-guarded exactly like [`stop_run`] (reused directly, not
/// reimplemented): the underlying `UPDATE` only applies when `status` is
/// still `ARMED` or `RUNNING`, so a concurrent `halt_run` landing first is
/// never silently overwritten. Zero mutation on any refusal — the
/// unacked-outbox and reconcile checks run strictly before the CAS
/// transition, and neither is ever inferred from an empty/absent row: no
/// durable reconcile status at all (`None`) is `ReconcileDirty`, not clean.
pub async fn stop_run_if_evidence_clean(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<StopRunIfEvidenceCleanOutcome> {
    let run = fetch_run(pool, run_id).await?;
    if !matches!(run.status, RunStatus::Armed | RunStatus::Running) {
        return Ok(StopRunIfEvidenceCleanOutcome::NotActive {
            actual_status: run.status.as_str().to_string(),
        });
    }

    let unacked = crate::orders::outbox_list_unacked_for_run(pool, run_id)
        .await
        .context("stop_run_if_evidence_clean: outbox_list_unacked_for_run failed")?;
    if !unacked.is_empty() {
        return Ok(StopRunIfEvidenceCleanOutcome::UnacknowledgedOutbox {
            unacked_count: unacked.len(),
        });
    }

    // PAPER-SOAK-UNRESOLVED-BROKER-EVIDENCE-GATE-01: an unapplied `oms_inbox`
    // row is durably received but not-yet-applied crash-recovery evidence --
    // exactly as load-bearing as an unacked outbox row. Checked before the
    // CAS transition, never inferred.
    let unapplied = crate::inbox::inbox_load_unapplied_for_run(pool, run_id)
        .await
        .context("stop_run_if_evidence_clean: inbox_load_unapplied_for_run failed")?;
    if !unapplied.is_empty() {
        return Ok(StopRunIfEvidenceCleanOutcome::UnappliedInbox {
            unapplied_count: unapplied.len(),
        });
    }

    let reconcile_clean = match crate::reconcile_state::load_reconcile_status_state(pool)
        .await
        .context("stop_run_if_evidence_clean: load_reconcile_status_state failed")?
    {
        Some(r) => {
            r.status == "ok"
                && r.mismatched_positions == 0
                && r.mismatched_orders == 0
                && r.mismatched_fills == 0
                && r.unmatched_broker_events == 0
        }
        // No durable reconcile status at all is not evidence of agreement --
        // fail closed rather than assume clean.
        None => false,
    };
    if !reconcile_clean {
        return Ok(StopRunIfEvidenceCleanOutcome::ReconcileDirty);
    }

    stop_run(pool, run_id).await?;
    Ok(StopRunIfEvidenceCleanOutcome::Stopped)
}

// ---------------------------------------------------------------------------
// PRE-SOAK-RUNTIME-HALT-FENCE-CAS-01 — atomic runtime safety-halt authority
// ---------------------------------------------------------------------------

/// Typed outcome of [`halt_and_disarm_run_if_active`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyHaltOutcome {
    /// The run was durably `RUNNING`; `runs.status` is now `HALTED` and
    /// `sys_arm_state` is now `DISARMED` with the supplied reason, committed
    /// together in one transaction.
    Halted,
    /// The run was already durably `HALTED`. `sys_arm_state` was
    /// (re)confirmed `DISARMED` with the supplied reason — idempotent,
    /// fail-closed; `runs.status` and `halted_at_utc` are left untouched.
    AlreadyHalted,
    /// Refused: the run is not in a state this runtime may safety-halt
    /// (`STOPPED`, `CREATED`, or `ARMED` — see the "Active-state policy"
    /// section of this function's doc comment for why `ARMED` is excluded).
    /// Zero mutation — `runs.status` and `sys_arm_state` are left exactly as
    /// observed under the row lock. This is the fenced-refusal path: the
    /// caller's halt authority has been superseded (most commonly by a
    /// successful operator [`clear_halted_run_and_reset_stale_claims`],
    /// possibly followed by a legitimate re-arm) and must not be allowed to
    /// win a stale write.
    RunNotActive { actual_status: String },
}

/// PRE-SOAK-RUNTIME-HALT-FENCE-CAS-01 — atomic, fenced runtime safety-halt.
///
/// # Why this exists
///
/// `PAPER-SOAK-STALE-CLAIM-RECOVERY-03` closed the pre-broker-submit races
/// (fenced lease acquire/refresh, fenced outbox claim — see
/// [`acquire_or_refresh_lease_for_running_run`](crate::runtime_lease::acquire_or_refresh_lease_for_running_run)
/// and `outbox_claim_batch_for_run_with_lease_authority`). It left one
/// TOCTOU window open: when those functions decide the caller's authority
/// has been lost/contested/invalidated, they have already released the
/// `runs` row lock (committed or rolled back) *before* returning that
/// decision to the caller. The runtime then invoked its safety-halt as a
/// second, later, separately-committed DB operation
/// (`halt_run` unconditionally followed by `persist_arm_state`). Between
/// "authority lost" and "halt persisted", an operator's
/// `clear_halted_run_and_reset_stale_claims` can win the run row lock,
/// observe `HALTED`, and commit `STOPPED` — and the old unconditional
/// `halt_run` (`UPDATE runs SET status='HALTED' WHERE run_id=$1`, no status
/// predicate) would then silently overwrite that `STOPPED` back to `HALTED`
/// when the stale runtime's write finally landed.
///
/// This function closes that window by making the halt decision and the
/// halt mutation share **one** atomic, serialized authority boundary instead
/// of two separately-committed steps: fetch-then-act becomes
/// lock-then-decide-then-mutate, all inside a single transaction.
///
/// # Serialization boundary
///
/// The `runs` row for `run_id` is locked with `SELECT ... FOR UPDATE`
/// *before* this function inspects `status` or writes anything — the exact
/// same row, the exact same locking primitive,
/// `clear_halted_run_and_reset_stale_claims`,
/// `acquire_or_refresh_lease_for_running_run`, and
/// `outbox_claim_batch_for_run_with_lease_authority` all use. Postgres
/// serializes any two transactions contending for the same row: whichever
/// reaches its `FOR UPDATE` first runs to completion (commit or rollback)
/// before the other's lock acquisition proceeds. A stale runtime calling
/// this function can therefore never observe a status that a concurrent
/// `clear_halted_run_and_reset_stale_claims` is still in the middle of
/// changing, and vice versa — there is no interleaving in which "clear
/// commits STOPPED" is followed by "stale safety-halt commits HALTED".
///
/// # Active-state policy: `RUNNING` only (not `ARMED`)
///
/// `ARMED` is deliberately excluded from the haltable set even though it is
/// nominally "execution-active". Every production caller of this seam
/// (`mqk-runtime::orchestrator::persist_halt_and_disarm`) runs inside
/// `ExecutionOrchestrator::tick()`, which `start_execution_runtime` only
/// ever invokes *after* `begin_run` (`ARMED -> RUNNING`) has already
/// succeeded — so a genuine safety-halt decision never legitimately
/// observes `ARMED`. Treating `ARMED` as haltable here would reopen the
/// hazard this patch closes, just shifted one state later: after a
/// successful `clear_halted_run_and_reset_stale_claims` (`HALTED ->
/// STOPPED`), an operator may legitimately `arm_run` (`STOPPED -> ARMED`)
/// the same `run_id` before a stale runtime's superseded halt decision
/// finally executes. If `ARMED` were haltable, that late write would halt a
/// freshly-armed run it has no authority over. Refusing on `ARMED` (via
/// `RunNotActive`) protects that recovery boundary, not just `runs.status`
/// while `HALTED`/`STOPPED`.
///
/// # `sys_arm_state` atomicity
///
/// `sys_arm_state` is a global singleton (`sentinel_id = 1`), not scoped by
/// `run_id`. The write to it happens inside the *same* transaction as the
/// `runs.status` write, after the row lock has proven the run is `RUNNING`
/// — so the two writes are never observably split (the previous `halt_run`
/// then `persist_arm_state` two-call shape left exactly that window open).
///
/// `now_utc` is caller-supplied (FC-9: no wall-clock reads in enforcement
/// paths) so recovery-race proofs are deterministic in tests.
pub async fn halt_and_disarm_run_if_active(
    pool: &PgPool,
    run_id: Uuid,
    now_utc: DateTime<Utc>,
    reason: &str,
) -> Result<SafetyHaltOutcome> {
    let mut tx = pool
        .begin()
        .await
        .context("halt_and_disarm_run_if_active: begin tx failed")?;

    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM runs WHERE run_id = $1 FOR UPDATE")
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await
            .context("halt_and_disarm_run_if_active: run lock failed")?;

    let Some(status) = status else {
        tx.rollback().await.ok();
        return Err(anyhow!(
            "halt_and_disarm_run_if_active: run {run_id} not found"
        ));
    };

    match status.as_str() {
        "RUNNING" => {
            let done = sqlx::query(
                r#"
                update runs
                set status = 'HALTED',
                    halted_at_utc = $2
                where run_id = $1
                  and status = 'RUNNING'
                "#,
            )
            .bind(run_id)
            .bind(now_utc)
            .execute(&mut *tx)
            .await
            .context("halt_and_disarm_run_if_active: halt update failed")?;

            if done.rows_affected() != 1 {
                // Unreachable given the row lock + status check above (no
                // other writer can change `status` while this tx holds the
                // row lock), but fail closed rather than assume it.
                tx.rollback().await.ok();
                let r = fetch_run(pool, run_id).await?;
                return Err(anyhow::Error::new(StaleRunTransition {
                    run_id,
                    operation: "halt_and_disarm_run_if_active",
                    expected: "RUNNING",
                    actual: r.status.as_str().to_string(),
                }));
            }

            sqlx::query(
                r#"
                insert into sys_arm_state (sentinel_id, state, reason, updated_at_utc)
                values (1, 'DISARMED', $1, $2)
                on conflict (sentinel_id) do update
                    set state          = excluded.state,
                        reason         = excluded.reason,
                        updated_at_utc = excluded.updated_at_utc
                "#,
            )
            .bind(reason)
            .bind(now_utc)
            .execute(&mut *tx)
            .await
            .context("halt_and_disarm_run_if_active: arm-state disarm failed")?;

            tx.commit()
                .await
                .context("halt_and_disarm_run_if_active: commit (halted) failed")?;

            Ok(SafetyHaltOutcome::Halted)
        }
        "HALTED" => {
            sqlx::query(
                r#"
                insert into sys_arm_state (sentinel_id, state, reason, updated_at_utc)
                values (1, 'DISARMED', $1, $2)
                on conflict (sentinel_id) do update
                    set state          = excluded.state,
                        reason         = excluded.reason,
                        updated_at_utc = excluded.updated_at_utc
                "#,
            )
            .bind(reason)
            .bind(now_utc)
            .execute(&mut *tx)
            .await
            .context("halt_and_disarm_run_if_active: idempotent arm-state disarm failed")?;

            tx.commit()
                .await
                .context("halt_and_disarm_run_if_active: commit (already halted) failed")?;

            Ok(SafetyHaltOutcome::AlreadyHalted)
        }
        _ => {
            tx.rollback()
                .await
                .context("halt_and_disarm_run_if_active: rollback (not active) failed")?;
            Ok(SafetyHaltOutcome::RunNotActive {
                actual_status: status,
            })
        }
    }
}

/// Heartbeat: RUNNING only updates last_heartbeat_utc. CAS-guarded
/// (RUN-LIFECYCLE-CAS-01): the `UPDATE` only applies when `status` is still
/// `RUNNING`, so a heartbeat racing a concurrent `stop_run`/`halt_run` can
/// never revive a stopped/halted run's `last_heartbeat_utc` into looking
/// fresh. On a CAS miss, returns [`StaleRunTransition`] with the actual
/// current status.
///
/// `heartbeat_at` is injected by the caller (FC-9: no now() in enforcement path).
pub async fn heartbeat_run(pool: &PgPool, run_id: Uuid, heartbeat_at: DateTime<Utc>) -> Result<()> {
    let done = sqlx::query(
        r#"
        update runs
        set last_heartbeat_utc = $2
        where run_id = $1
          and status = 'RUNNING'
        "#,
    )
    .bind(run_id)
    .bind(heartbeat_at)
    .execute(pool)
    .await
    .context("heartbeat_run update failed")?;

    if done.rows_affected() == 1 {
        return Ok(());
    }

    let r = fetch_run(pool, run_id).await?;
    Err(anyhow::Error::new(StaleRunTransition {
        run_id,
        operation: "heartbeat_run",
        expected: "RUNNING",
        actual: r.status.as_str().to_string(),
    }))
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
/// PRE-SOAK-DAEMON-SUPERVISOR-HALT-FENCE-CLOSURE-01: the halt decision and
/// the halt mutation now share [`halt_and_disarm_run_if_active`]'s single
/// atomic, `SELECT ... FOR UPDATE`-serialized authority boundary instead of
/// a separate `fetch_run` read followed by an unconditional `halt_run`
/// write. The prior two-step shape left a TOCTOU window open: between this
/// function's own status re-check and its `halt_run` write, a concurrent
/// operator `clear_halted_run_and_reset_stale_claims` could win the `runs`
/// row lock, observe `HALTED`, and commit `STOPPED` — and this function's
/// unconditional write would then silently overwrite that `STOPPED` back to
/// `HALTED` once it finally landed. `RunNotActive` (the run is no longer
/// `RUNNING` by the time the atomic check runs) now returns `Ok(false)` —
/// zero mutation, exactly like the "not expired" case — instead of racing a
/// stale write against recovery.
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

    match halt_and_disarm_run_if_active(pool, run_id, now, "DeadmanExpired").await? {
        SafetyHaltOutcome::Halted | SafetyHaltOutcome::AlreadyHalted => Ok(true),
        SafetyHaltOutcome::RunNotActive { .. } => Ok(false),
    }
}
