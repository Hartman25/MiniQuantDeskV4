// core-rs/crates/mqk-db/src/inbox.rs
//
// OMS inbox: broker fill reception, deduplication, and apply journalling.
// Extracted from orders.rs (MT-03 DB layer modularization).
//
// This module owns only the oms_inbox table operations.
// The oms_outbox table and broker_order_map remain in orders.rs.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// DB-OUTBOX-SCHEMA-VERSION-01: message_json envelope schema identity
// ---------------------------------------------------------------------------

/// Current `message_json` envelope schema version. Every row inserted via
/// [`inbox_insert_deduped`] / [`inbox_insert_deduped_with_identity`] carries
/// this value under the `schema_version` key. Bump only alongside a
/// documented, backward-compatible change to the envelope shape the readers
/// (chiefly `serde_json::from_value::<mqk_execution::BrokerEvent>`) understand.
pub const MESSAGE_JSON_SCHEMA_VERSION: i64 = 1;

/// Validate-and-stamp `message_json` with the current schema version before
/// it is durably written. This is the single writer-side primitive for
/// `message_json` schema identity -- every INSERT path must route through
/// it.
///
/// - Missing `schema_version`: current version is inserted.
/// - `schema_version == MESSAGE_JSON_SCHEMA_VERSION`: preserved unchanged.
/// - Any other value under the key (a different integer, zero, negative,
///   wrong type) or a non-object envelope: the write is refused -- a
///   caller-supplied explicit non-current version is never silently
///   overwritten, and a non-object payload never reaches disk without an
///   explicit schema identity.
fn stamp_message_json_schema_version(message_json: &Value) -> Result<Value> {
    let Value::Object(map) = message_json else {
        return Err(anyhow!(
            "message_json must be a JSON object to receive an explicit schema_version, refusing write"
        ));
    };
    let mut map = map.clone();
    match map.get("schema_version") {
        None => {
            map.insert(
                "schema_version".to_string(),
                Value::from(MESSAGE_JSON_SCHEMA_VERSION),
            );
        }
        Some(v) if v.as_i64() == Some(MESSAGE_JSON_SCHEMA_VERSION) => {}
        Some(v) => {
            return Err(anyhow!(
                "message_json schema_version {v:?} is not writable (current={MESSAGE_JSON_SCHEMA_VERSION}); refusing to silently overwrite an explicit non-current value"
            ));
        }
    }
    Ok(Value::Object(map))
}

/// Validate a `message_json` envelope's `schema_version` before it is
/// trusted by a reader (e.g. before `serde_json::from_value::<BrokerEvent>`).
///
/// Same fail-closed contract as
/// [`crate::orders::validate_order_json_schema_version`]: missing is
/// accepted (proven historical compatibility), the current version is
/// accepted, a greater integer is refused as an unsupported future version,
/// and anything else present under the key is refused as malformed.
pub fn validate_message_json_schema_version(message_json: &Value) -> Result<()> {
    let Some(field) = message_json.get("schema_version") else {
        return Ok(());
    };
    match field.as_i64() {
        Some(v) if v == MESSAGE_JSON_SCHEMA_VERSION => Ok(()),
        Some(v) if v > MESSAGE_JSON_SCHEMA_VERSION => Err(anyhow!(
            "message_json schema_version {v} is newer than this build supports (current={MESSAGE_JSON_SCHEMA_VERSION})"
        )),
        _ => Err(anyhow!(
            "message_json schema_version {field:?} is malformed or unrecognized (current={MESSAGE_JSON_SCHEMA_VERSION})"
        )),
    }
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
    /// Broker event kind stored at ingest time: "ack", "fill", "partial_fill",
    /// "cancel_ack", "cancel_reject", "replace_ack", "replace_reject", "reject".
    /// Mirrors the `event_kind` column added in migration 0021.
    pub event_kind: String,
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
/// Dedupe rule for every `event_kind`, including `"partial_fill"`, is
/// transport-only and explicit:
/// - conflict key: `(run_id, broker_message_id)`
/// - `broker_fill_id` is optional economic identity metadata and does NOT
///   participate in inbox insertion dedupe.
///
/// # PAPER-SOAK-PARTIAL-FILL-DEDUP-02: why `partial_fill` has no special case here
///
/// PAPER-SOAK-PARTIAL-FILL-DEDUP-01 previously special-cased `partial_fill`
/// here with an economic-match heuristic (same order + same delta_qty + same
/// price_micros + broker timestamps within a fixed window). That heuristic
/// was rejected on independent review: two *legitimate* partial fills for
/// the same order can genuinely share quantity, price, and near-simultaneous
/// timestamps (e.g. two 10-share fills at the same price a second apart),
/// and a timing heuristic cannot tell that case apart from the same physical
/// fill re-delivered over the WS and REST inbound lanes with two different
/// transport identities — collapsing the former is exactly the failure mode
/// PATCH-01 was meant to prevent for the latter.
///
/// This insertion layer therefore always inserts every distinct
/// `(run_id, broker_message_id)` row for `partial_fill`, exactly like every
/// other event kind — full raw evidence from both lanes is preserved, never
/// silently collapsed. Economic dedup for the case PATCH-01 was chasing (the
/// same physical partial fill delivered on both lanes) now happens at the
/// OMS *apply* layer instead: `mqk_execution::oms::state_machine::OmsOrder::
/// apply_with_watermark` recognizes a cross-lane duplicate by comparing the
/// broker-authoritative cumulative-filled-quantity-after-this-event
/// (`BrokerEvent::PartialFill::cum_qty_after`) against the order's current
/// `filled_qty` — when present, an exact identity, never a heuristic, and
/// one that can never collapse two genuinely distinct fills (cumulative
/// filled quantity strictly increases with every real execution, so two
/// distinct fills always carry two distinct watermark values regardless of
/// how close in time or identical in size they are).
///
/// PAPER-SOAK-PARTIAL-FILL-DEDUP-04: `cum_qty_after` is populated the same
/// structural way on both lanes — read directly off a broker-native field
/// carried atomically in the same message/record as the fill itself, never
/// derived or reconstructed from a separately fetched snapshot. On the WS
/// lane it is Alpaca's own live `order.filled_qty` from that specific push.
/// On the REST lane (`mqk-broker-alpaca::fetch_events`) it is Alpaca's own
/// `cum_qty` field on that specific account-activity record. A REST
/// PARTIAL_FILL activity whose `cum_qty` is missing or unparseable fails the
/// whole polling page closed (`BrokerError::Transient`) rather than falling
/// back to an ambiguous `None` — `event_id`-only dedup cannot safely
/// disambiguate a cross-lane duplicate, so an unprovable REST partial is
/// never allowed to reach this insert path at all. `None` from the WS lane
/// (adapters that never supply this field) still falls back to the
/// pre-existing `event_id`-only dedup, unweakened.
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
    inbox_insert_transport_only_deduped(
        pool,
        run_id,
        broker_message_id,
        broker_fill_id,
        internal_order_id,
        broker_order_id,
        event_kind,
        event_json,
        event_ts_ms,
        received_at,
    )
    .await
}

/// Transport-identity-only insert: conflict key is `(run_id, broker_message_id)`
/// (plus whichever other unique indexes exist on `oms_inbox`, e.g.
/// `uq_inbox_run_order_single_fill` for terminal fills). Used directly for
/// every `event_kind`, including `"partial_fill"` (see
/// PAPER-SOAK-PARTIAL-FILL-DEDUP-02 module-level rationale on
/// [`inbox_insert_deduped_with_identity`] for why `partial_fill` no longer
/// gets special-cased here).
#[allow(clippy::too_many_arguments)]
async fn inbox_insert_transport_only_deduped(
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
    let event_json = stamp_message_json_schema_version(event_json)?;
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
                        | Some("uq_inbox_run_order_single_fill")
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
pub async fn inbox_load_unapplied_for_run(
    pool: impl sqlx::PgExecutor<'_>,
    run_id: Uuid,
) -> Result<Vec<InboxRow>> {
    let rows = sqlx::query(
        r#"
        select inbox_id, run_id, broker_message_id, broker_fill_id,
               broker_sequence_id, broker_timestamp, message_json,
               received_at_utc, applied_at_utc, event_kind
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
            event_kind: row.try_get("event_kind")?,
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

/// A stale broker-order-map entry belonging to a HALTED run.
///
/// Returned by [`inbox_find_stale_broker_map_for_halted_runs`] for use by the
/// BROKER-FILL-REPLAY-REPAIR-01 repair planner.  All fields are read-only;
/// no DB writes occur in that query.
#[derive(Debug, Clone)]
pub struct StaleBrokerMapEntry {
    /// Internal order ID (= `oms_outbox.idempotency_key`).
    pub internal_order_id: String,
    /// Exchange-assigned broker order ID.
    pub broker_order_id: String,
    /// Run that owns this order.
    pub run_id: Uuid,
    /// Current `oms_outbox.status` (typically `"SENT"` for an orphaned fill).
    pub outbox_status: String,
    /// `runs.status` for the owning run (always `"HALTED"` from this query).
    pub run_status: String,
    /// When the run was halted.
    pub halted_at_utc: Option<DateTime<Utc>>,
    /// When the broker_order_map entry was registered.
    pub broker_map_registered_at_utc: DateTime<Utc>,
}

/// Find all `broker_order_map` entries whose owning run is HALTED.
///
/// These are candidate stale entries where a broker fill may have occurred but
/// was never applied to the portfolio (the run halted before Phase 3 could
/// drain the inbox, or the inbox row was deleted before being applied).
///
/// Read-only: does not modify any state.  Safe to call at any time.
pub async fn inbox_find_stale_broker_map_for_halted_runs(
    pool: &PgPool,
) -> Result<Vec<StaleBrokerMapEntry>> {
    let rows = sqlx::query(
        r#"
        select bom.internal_id,
               bom.broker_id,
               bom.registered_at_utc,
               o.run_id,
               o.status  as outbox_status,
               r.status  as run_status,
               r.halted_at_utc
          from broker_order_map bom
          join oms_outbox o on bom.internal_id = o.idempotency_key
          join runs r on o.run_id = r.run_id
         where r.status = 'HALTED'
         order by r.halted_at_utc desc, bom.internal_id
        "#,
    )
    .fetch_all(pool)
    .await
    .context("inbox_find_stale_broker_map_for_halted_runs failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(StaleBrokerMapEntry {
            internal_order_id: row.try_get("internal_id")?,
            broker_order_id: row.try_get("broker_id")?,
            broker_map_registered_at_utc: row.try_get("registered_at_utc")?,
            run_id: row.try_get("run_id")?,
            outbox_status: row.try_get("outbox_status")?,
            run_status: row.try_get("run_status")?,
            halted_at_utc: row.try_get("halted_at_utc")?,
        });
    }
    Ok(out)
}

/// Enriched inbox row for operator repair actions.
///
/// Unlike `InboxRow`, this struct exposes `internal_order_id` and
/// `broker_order_id` which are required to identify which specific order
/// a fill row belongs to when performing targeted repair operations.
#[derive(Debug, Clone)]
pub struct InboxFillDetail {
    pub inbox_id: i64,
    pub run_id: Uuid,
    pub broker_message_id: String,
    pub internal_order_id: String,
    pub broker_order_id: String,
    pub event_kind: String,
    pub message_json: Value,
    pub received_at_utc: DateTime<Utc>,
    pub applied_at_utc: Option<DateTime<Utc>>,
}

/// Load unapplied fill or partial_fill inbox rows for a specific order within a run.
///
/// Returns only rows where `applied_at_utc IS NULL` and `event_kind` is
/// `'fill'` or `'partial_fill'` and `internal_order_id` matches.
///
/// Used by BROKER-FILL-REPLAY-APPLY-01 to locate the specific fill evidence
/// for an operator repair action.  Read-only; callers must not mark rows
/// applied without completing a successful portfolio-apply step.
pub async fn inbox_load_unapplied_fill_for_order(
    pool: &PgPool,
    run_id: Uuid,
    internal_order_id: &str,
) -> Result<Vec<InboxFillDetail>> {
    let rows = sqlx::query(
        r#"
        select inbox_id, run_id, broker_message_id, internal_order_id,
               broker_order_id, event_kind, message_json,
               received_at_utc, applied_at_utc
          from oms_inbox
         where run_id = $1
           and internal_order_id = $2
           and event_kind in ('fill', 'partial_fill')
           and applied_at_utc is null
         order by inbox_id asc
        "#,
    )
    .bind(run_id)
    .bind(internal_order_id)
    .fetch_all(pool)
    .await
    .context("inbox_load_unapplied_fill_for_order failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(InboxFillDetail {
            inbox_id: row.try_get("inbox_id")?,
            run_id: row.try_get("run_id")?,
            broker_message_id: row.try_get("broker_message_id")?,
            internal_order_id: row.try_get("internal_order_id")?,
            broker_order_id: row.try_get("broker_order_id")?,
            event_kind: row.try_get("event_kind")?,
            message_json: row.try_get("message_json")?,
            received_at_utc: row.try_get("received_at_utc")?,
            applied_at_utc: row.try_get("applied_at_utc")?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// BRK-GAP-REST-RECOVERY-01 — Run-scoped broker map load
// ---------------------------------------------------------------------------

/// One row from `broker_order_map` scoped to a specific run.
///
/// Used by the WS-gap fill recovery service to match REST account activities
/// to orders that belong to the target run without touching other runs.
#[derive(Debug, Clone)]
pub struct RunBrokerMapEntry {
    /// OMS internal order ID (`oms_outbox.idempotency_key`).
    pub internal_order_id: String,
    /// Alpaca-assigned broker order UUID.
    pub broker_order_id: String,
}

/// Load all `broker_order_map` entries whose owning outbox row belongs to `run_id`.
///
/// Returns one entry per submitted order for the run.  Read-only — no state is
/// mutated.  Used by the WS-gap fill recovery service to scope REST activity
/// matching to a single run without touching sibling runs.
pub async fn broker_map_load_for_run(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Vec<RunBrokerMapEntry>> {
    let rows = sqlx::query(
        r#"
        select bom.internal_id, bom.broker_id
          from broker_order_map bom
          join oms_outbox o on bom.internal_id = o.idempotency_key
         where o.run_id = $1
         order by bom.registered_at_utc asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("broker_map_load_for_run failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(RunBrokerMapEntry {
            internal_order_id: row.try_get("internal_id")?,
            broker_order_id: row.try_get("broker_id")?,
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
               received_at_utc, applied_at_utc, event_kind
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
            event_kind: row.try_get("event_kind")?,
        });
    }
    Ok(out)
}

/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-STRICT-OUTCOME-CLASSIFIER-AND-
/// FINALIZATION-CAS: load every `oms_inbox` row for one run, any
/// `event_kind`, any `applied_at_utc` status, unbounded, ordered by
/// `inbox_id asc`. Read-only. Distinct from
/// [`inbox_load_unapplied_for_run`]/[`inbox_load_all_applied_for_run`] (each
/// scoped to one apply-status half): the finalization classifier's
/// fill/ack/reject activity evidence must see every durable broker event
/// regardless of whether it has been applied to the portfolio yet.
pub async fn inbox_load_all_for_run(pool: &PgPool, run_id: Uuid) -> Result<Vec<InboxRow>> {
    let rows = sqlx::query(
        r#"
        select inbox_id, run_id, broker_message_id, broker_fill_id,
               broker_sequence_id, broker_timestamp, message_json,
               received_at_utc, applied_at_utc, event_kind
          from oms_inbox
         where run_id = $1
         order by inbox_id asc
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("inbox_load_all_for_run failed")?;

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
            event_kind: row.try_get("event_kind")?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// DB-OUTBOX-SCHEMA-VERSION-01: pure unit tests (no DB required)
// ---------------------------------------------------------------------------
#[cfg(test)]
mod message_json_schema_version_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sv01_stamp_writes_current_version_when_missing() {
        let stamped = stamp_message_json_schema_version(&json!({"type": "ack"})).unwrap();
        assert_eq!(
            stamped.get("schema_version").and_then(Value::as_i64),
            Some(MESSAGE_JSON_SCHEMA_VERSION)
        );
        assert_eq!(stamped.get("type").and_then(Value::as_str), Some("ack"));
    }

    #[test]
    fn sv02_stamp_refuses_a_non_object_value() {
        assert!(stamp_message_json_schema_version(&json!("not-an-object")).is_err());
        assert!(stamp_message_json_schema_version(&json!([1, 2, 3])).is_err());
    }

    #[test]
    fn sv10_stamp_preserves_an_explicit_current_version() {
        let stamped = stamp_message_json_schema_version(
            &json!({"type": "ack", "schema_version": MESSAGE_JSON_SCHEMA_VERSION}),
        )
        .unwrap();
        assert_eq!(
            stamped.get("schema_version").and_then(Value::as_i64),
            Some(MESSAGE_JSON_SCHEMA_VERSION)
        );
    }

    #[test]
    fn sv11_stamp_refuses_to_silently_overwrite_a_future_version() {
        let err = stamp_message_json_schema_version(
            &json!({"type": "ack", "schema_version": MESSAGE_JSON_SCHEMA_VERSION + 1}),
        )
        .unwrap_err();
        assert!(err.to_string().contains("not writable"));
    }

    #[test]
    fn sv12_stamp_refuses_malformed_schema_version() {
        assert!(
            stamp_message_json_schema_version(&json!({"type": "ack", "schema_version": "one"}))
                .is_err()
        );
    }

    #[test]
    fn sv13_stamp_refuses_zero_and_negative_schema_version() {
        assert!(
            stamp_message_json_schema_version(&json!({"type": "ack", "schema_version": 0}))
                .is_err()
        );
        assert!(
            stamp_message_json_schema_version(&json!({"type": "ack", "schema_version": -1}))
                .is_err()
        );
    }

    #[test]
    fn sv03_missing_schema_version_is_accepted_as_historical() {
        let legacy = json!({"type": "ack"});
        assert!(validate_message_json_schema_version(&legacy).is_ok());
    }

    #[test]
    fn sv04_current_version_is_accepted() {
        let current = json!({"type": "ack", "schema_version": MESSAGE_JSON_SCHEMA_VERSION});
        assert!(validate_message_json_schema_version(&current).is_ok());
    }

    #[test]
    fn sv05_future_version_is_refused() {
        let future = json!({"type": "ack", "schema_version": MESSAGE_JSON_SCHEMA_VERSION + 1});
        let err = validate_message_json_schema_version(&future).unwrap_err();
        assert!(err.to_string().contains("newer than this build supports"));
    }

    #[test]
    fn sv06_non_integer_schema_version_is_refused_as_malformed() {
        let malformed = json!({"type": "ack", "schema_version": "one"});
        let err = validate_message_json_schema_version(&malformed).unwrap_err();
        assert!(err.to_string().contains("malformed"));
    }
}
