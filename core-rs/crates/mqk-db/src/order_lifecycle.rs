//! EXEC-02: Order lifecycle event persistence.
//!
//! `oms_order_lifecycle_events` records cancel and replace lifecycle events
//! per order per run. Populated best-effort from Phase 3b of
//! `ExecutionOrchestrator::tick()` for the four non-fill broker ack/reject
//! event kinds: `cancel_ack`, `replace_ack`, `cancel_reject`, `replace_reject`.
//!
//! Fill events are NOT recorded here — those live in `fill_quality_telemetry`.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Row to insert into `oms_order_lifecycle_events`.
///
/// `event_id` must equal `broker_message_id` so that repeated best-effort
/// writes from the orchestrator are idempotent via `ON CONFLICT DO NOTHING`.
#[derive(Debug, Clone)]
pub struct NewOrderLifecycleEvent {
    /// Equals `broker_message_id` — deduplication identity.
    pub event_id: String,
    pub run_id: Uuid,
    pub internal_order_id: String,
    /// `"cancel_ack"` | `"replace_ack"` | `"cancel_reject"` | `"replace_reject"`
    pub operation: String,
    /// Broker-assigned order ID; `None` for paper adapters.
    pub broker_order_id: Option<String>,
    /// Post-replace total qty (`replace_ack` only); `None` for all others.
    pub new_total_qty: Option<i64>,
    pub recorded_at_utc: DateTime<Utc>,
}

/// A lifecycle event row returned by the read path.
#[derive(Debug, Clone)]
pub struct OrderLifecycleEventRow {
    pub event_id: String,
    pub run_id: Uuid,
    pub internal_order_id: String,
    pub operation: String,
    pub broker_order_id: Option<String>,
    pub new_total_qty: Option<i64>,
    pub recorded_at_utc: DateTime<Utc>,
}

/// Persist a single order lifecycle event.
///
/// Idempotent via `ON CONFLICT (event_id) DO NOTHING` — repeated
/// best-effort writes from the orchestrator cannot produce duplicate rows.
pub async fn insert_order_lifecycle_event(
    pool: &PgPool,
    ev: &NewOrderLifecycleEvent,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into oms_order_lifecycle_events
            (event_id, run_id, internal_order_id, operation,
             broker_order_id, new_total_qty, recorded_at_utc)
        values ($1, $2, $3, $4, $5, $6, $7)
        on conflict (event_id) do nothing
        "#,
    )
    .bind(&ev.event_id)
    .bind(ev.run_id)
    .bind(&ev.internal_order_id)
    .bind(&ev.operation)
    .bind(&ev.broker_order_id)
    .bind(ev.new_total_qty)
    .bind(ev.recorded_at_utc)
    .execute(pool)
    .await
    .context("insert_order_lifecycle_event failed")?;
    Ok(())
}

/// Load all lifecycle events for a run, oldest-first.
///
/// Returns at most 200 rows.  Returns an empty `Vec` when no events have
/// been recorded — authoritative empty (not absence of source).
pub async fn fetch_order_lifecycle_events_for_run(
    pool: &PgPool,
    run_id: Uuid,
) -> Result<Vec<OrderLifecycleEventRow>> {
    let rows = sqlx::query(
        r#"
        select event_id, run_id, internal_order_id, operation,
               broker_order_id, new_total_qty, recorded_at_utc
        from oms_order_lifecycle_events
        where run_id = $1
        order by recorded_at_utc asc
        limit 200
        "#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await
    .context("fetch_order_lifecycle_events_for_run failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(OrderLifecycleEventRow {
            event_id: r.get("event_id"),
            run_id: r.try_get("run_id")?,
            internal_order_id: r.get("internal_order_id"),
            operation: r.get("operation"),
            broker_order_id: r.get("broker_order_id"),
            new_total_qty: r.get("new_total_qty"),
            recorded_at_utc: r.try_get("recorded_at_utc")?,
        });
    }
    Ok(out)
}
