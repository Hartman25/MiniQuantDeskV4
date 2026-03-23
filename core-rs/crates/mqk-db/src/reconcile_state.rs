use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

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

// ---------------------------------------------------------------------------
// Broker event cursor — Patch A2
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
