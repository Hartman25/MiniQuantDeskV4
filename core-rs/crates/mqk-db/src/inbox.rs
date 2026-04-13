// core-rs/crates/mqk-db/src/inbox.rs
//
// OMS inbox: broker fill reception, deduplication, and apply journalling.
// Extracted from orders.rs (MT-03 DB layer modularization).
//
// This module owns only the oms_inbox table operations.
// The oms_outbox table and broker_order_map remain in orders.rs.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

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

/// Insert a broker message/fill into oms_inbox with dedupe on (run_id, broker_message_id).
///
/// Idempotent behavior:
/// - If (run_id, broker_message_id) already exists, returns Ok(false) and does NOT create a
///   second row.
/// - If inserted, returns Ok(true).
///
/// RT-3: dedupe is scoped to the run — the same broker_message_id can appear in different
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
/// RT-3: `run_id` is now required — dedupe is scoped to (run_id, broker_message_id).
///
/// `applied_at` is caller-supplied — no SQL `now()` in this function (FC-8
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

/// Minimal row for the broker ACK causality lane.
///
/// Contains only the fields required by the causality route:
/// - `inbox_id` — durable ingest position (for display only)
/// - `broker_message_id` — used as `linked_id` in the causality node
/// - `received_at_utc` — the durable ACK timestamp surfaced as `timestamp`
///
/// This struct is intentionally smaller than `InboxRow` to avoid selecting
/// columns (e.g. `message_json`, `applied_at_utc`) that are irrelevant here.
#[derive(Debug, Clone)]
pub struct InboxAckRow {
    pub inbox_id: i64,
    pub broker_message_id: String,
    pub received_at_utc: chrono::DateTime<Utc>,
}

/// Fetch `oms_inbox` rows where `event_kind = 'ack'` for a specific order,
/// ordered by `inbox_id asc` (durable ingest order).
///
/// Used by the causality route (EXEC-CAUSE-01C) to surface the durable broker
/// ACK moment.  Returns an empty vec when no ACK rows exist — never errors on
/// absence.
///
/// Scoped to `(run_id, internal_order_id)` so the result is always
/// run-specific and order-specific.
pub async fn inbox_fetch_ack_rows_for_order(
    pool: &PgPool,
    run_id: Uuid,
    internal_order_id: &str,
) -> Result<Vec<InboxAckRow>> {
    let rows = sqlx::query(
        r#"
        select inbox_id, broker_message_id, received_at_utc
          from oms_inbox
         where run_id = $1
           and internal_order_id = $2
           and event_kind = 'ack'
         order by inbox_id asc
        "#,
    )
    .bind(run_id)
    .bind(internal_order_id)
    .fetch_all(pool)
    .await
    .context("inbox_fetch_ack_rows_for_order failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(InboxAckRow {
            inbox_id: row.try_get("inbox_id")?,
            broker_message_id: row.try_get("broker_message_id")?,
            received_at_utc: row.try_get("received_at_utc")?,
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
