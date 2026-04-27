// core-rs/crates/mqk-db/src/flow.rs
//
// FLOW-03: Execution flow read model.
//
// Assembles a unified, time-ordered sequence of flow events from three
// existing durable tables:
//
//   oms_outbox                  → outbox lifecycle stages
//   oms_order_lifecycle_events  → cancel / replace ack / reject events
//   fill_quality_telemetry      → fill events
//
// No new migration is required. All source data is already durable.
//
// The read model is append-only from the caller's perspective: it reflects
// whatever timestamps are present in the source tables at the time of the
// query. It does not synthesize, infer, or fabricate events.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

/// Query filter for execution flow rows.
///
/// All fields are optional. When `run_id` and `internal_order_id` are both
/// absent the query returns all rows (bounded by `limit`), which is only
/// useful in testing. In production the route handler always provides at
/// least one filter.
pub struct FlowQuery {
    /// Restrict to a single durable run. `None` = no run filter.
    pub run_id: Option<Uuid>,
    /// Restrict to a single order. Maps to `idempotency_key` in `oms_outbox`
    /// and to `internal_order_id` in the other two tables. `None` = no order filter.
    pub internal_order_id: Option<String>,
    /// Maximum rows to return after merging all sources. Clamped by the
    /// caller; must be ≥ 1.
    pub limit: usize,
}

// ---------------------------------------------------------------------------
// Output row
// ---------------------------------------------------------------------------

/// One row in the assembled execution flow read model.
#[derive(Debug, Clone)]
pub struct ExecutionFlowRow {
    /// Stable, deterministic identifier for this row (no UUIDv4).
    /// Format varies by source: `"outbox:enqueued:{key}"`, `"lifecycle:{id}"`,
    /// `"fill:{id}"`.
    pub row_id: String,
    /// Authoritative timestamp for this event (from the source table column).
    pub ts_utc: DateTime<Utc>,
    /// Stage label. See module doc for the complete vocabulary.
    pub stage: String,
    /// `"info"` | `"warn"` | `"error"`.
    pub severity: String,
    pub run_id: Uuid,
    /// The internal order ID. For outbox rows this equals `idempotency_key`.
    pub internal_order_id: Option<String>,
    pub broker_order_id: Option<String>,
    pub symbol: Option<String>,
    /// Short operator-readable description of this event.
    pub message: String,
    /// Which durable table this row was derived from.
    pub source_table: String,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Assemble execution flow rows from existing durable tables.
///
/// Runs three independent queries (one per source table), merges the results
/// in memory, sorts by `ts_utc` ascending, and applies `query.limit`.
///
/// Idempotent: running the same query twice returns the same result.
/// Read-only: no writes, no state changes.
pub async fn fetch_execution_flow(
    pool: &PgPool,
    query: FlowQuery,
) -> Result<Vec<ExecutionFlowRow>> {
    let mut rows: Vec<ExecutionFlowRow> = Vec::new();

    let per_source_limit = query.limit.max(1) as i64;

    // --- Source 1: oms_outbox ---
    let outbox_rows = fetch_outbox_flow_rows(
        pool,
        query.run_id,
        query.internal_order_id.as_deref(),
        per_source_limit,
    )
    .await
    .context("fetch_outbox_flow_rows failed")?;
    rows.extend(outbox_rows);

    // --- Source 2: oms_order_lifecycle_events ---
    let lifecycle_rows = fetch_lifecycle_flow_rows(
        pool,
        query.run_id,
        query.internal_order_id.as_deref(),
        per_source_limit,
    )
    .await
    .context("fetch_lifecycle_flow_rows failed")?;
    rows.extend(lifecycle_rows);

    // --- Source 3: fill_quality_telemetry ---
    let fill_rows = fetch_fill_flow_rows(
        pool,
        query.run_id,
        query.internal_order_id.as_deref(),
        per_source_limit,
    )
    .await
    .context("fetch_fill_flow_rows failed")?;
    rows.extend(fill_rows);

    // Merge: sort ascending by timestamp, then apply limit.
    rows.sort_by_key(|r| r.ts_utc);
    rows.truncate(query.limit);

    Ok(rows)
}

// ---------------------------------------------------------------------------
// Source 1: oms_outbox
// ---------------------------------------------------------------------------

async fn fetch_outbox_flow_rows(
    pool: &PgPool,
    run_id: Option<Uuid>,
    order_id: Option<&str>,
    limit: i64,
) -> Result<Vec<ExecutionFlowRow>> {
    // Each outbox row can produce up to 4 flow events (one per non-null
    // lifecycle timestamp). We fetch at most `limit` outbox rows, then
    // expand them; the outer merge applies the hard cap.
    let db_rows = sqlx::query(
        r#"
        select idempotency_key, run_id, order_json, status,
               created_at_utc, claimed_at_utc, dispatching_at_utc, sent_at_utc
        from oms_outbox
        where ($1::uuid is null or run_id = $1)
          and ($2::text is null or idempotency_key = $2)
        order by created_at_utc asc
        limit $3
        "#,
    )
    .bind(run_id)
    .bind(order_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("outbox flow query failed")?;

    let mut out = Vec::new();
    for r in db_rows {
        let key: String = r.try_get("idempotency_key")?;
        let rid: Uuid = r.try_get("run_id")?;
        let status: String = r.try_get("status")?;
        let order_json: serde_json::Value = r.try_get("order_json")?;
        let created: DateTime<Utc> = r.try_get("created_at_utc")?;
        let claimed: Option<DateTime<Utc>> = r.try_get("claimed_at_utc")?;
        let dispatching: Option<DateTime<Utc>> = r.try_get("dispatching_at_utc")?;
        let sent: Option<DateTime<Utc>> = r.try_get("sent_at_utc")?;

        let symbol = order_json
            .get("symbol")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let is_terminal = matches!(status.as_str(), "FAILED" | "AMBIGUOUS");

        // Event 1: outbox_enqueued (always present)
        let enqueue_msg = if is_terminal {
            format!(
                "Outbox enqueued (current status: {}). Symbol: {}.",
                status,
                symbol.as_deref().unwrap_or("—")
            )
        } else {
            format!(
                "Outbox enqueued. Symbol: {}.",
                symbol.as_deref().unwrap_or("—")
            )
        };
        let enqueue_severity = if is_terminal { "warn" } else { "info" };
        out.push(ExecutionFlowRow {
            row_id: format!("outbox:enqueued:{key}"),
            ts_utc: created,
            stage: "outbox_enqueued".to_string(),
            severity: enqueue_severity.to_string(),
            run_id: rid,
            internal_order_id: Some(key.clone()),
            broker_order_id: None,
            symbol: symbol.clone(),
            message: enqueue_msg,
            source_table: "oms_outbox".to_string(),
        });

        // Event 2: outbox_claimed (when claimed_at_utc is set)
        if let Some(ts) = claimed {
            out.push(ExecutionFlowRow {
                row_id: format!("outbox:claimed:{key}"),
                ts_utc: ts,
                stage: "outbox_claimed".to_string(),
                severity: "info".to_string(),
                run_id: rid,
                internal_order_id: Some(key.clone()),
                broker_order_id: None,
                symbol: symbol.clone(),
                message: "Outbox row claimed by dispatcher.".to_string(),
                source_table: "oms_outbox".to_string(),
            });
        }

        // Event 3: outbox_dispatching (when dispatching_at_utc is set)
        if let Some(ts) = dispatching {
            out.push(ExecutionFlowRow {
                row_id: format!("outbox:dispatching:{key}"),
                ts_utc: ts,
                stage: "outbox_dispatching".to_string(),
                severity: "info".to_string(),
                run_id: rid,
                internal_order_id: Some(key.clone()),
                broker_order_id: None,
                symbol: symbol.clone(),
                message: "Dispatcher preparing broker submit.".to_string(),
                source_table: "oms_outbox".to_string(),
            });
        }

        // Event 4: broker_sent (when sent_at_utc is set)
        if let Some(ts) = sent {
            out.push(ExecutionFlowRow {
                row_id: format!("outbox:sent:{key}"),
                ts_utc: ts,
                stage: "broker_sent".to_string(),
                severity: "info".to_string(),
                run_id: rid,
                internal_order_id: Some(key.clone()),
                broker_order_id: None,
                symbol: symbol.clone(),
                message: "Order submitted to broker.".to_string(),
                source_table: "oms_outbox".to_string(),
            });
        }
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Source 2: oms_order_lifecycle_events
// ---------------------------------------------------------------------------

async fn fetch_lifecycle_flow_rows(
    pool: &PgPool,
    run_id: Option<Uuid>,
    order_id: Option<&str>,
    limit: i64,
) -> Result<Vec<ExecutionFlowRow>> {
    let db_rows = sqlx::query(
        r#"
        select event_id, run_id, internal_order_id, operation,
               broker_order_id, new_total_qty, recorded_at_utc
        from oms_order_lifecycle_events
        where ($1::uuid is null or run_id = $1)
          and ($2::text is null or internal_order_id = $2)
        order by recorded_at_utc asc
        limit $3
        "#,
    )
    .bind(run_id)
    .bind(order_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("lifecycle flow query failed")?;

    let mut out = Vec::new();
    for r in db_rows {
        let event_id: String = r.try_get("event_id")?;
        let rid: Uuid = r.try_get("run_id")?;
        let internal_order_id: String = r.try_get("internal_order_id")?;
        let operation: String = r.try_get("operation")?;
        let broker_order_id: Option<String> = r.try_get("broker_order_id")?;
        let new_total_qty: Option<i64> = r.try_get("new_total_qty")?;
        let recorded_at: DateTime<Utc> = r.try_get("recorded_at_utc")?;

        let (stage, severity, message) = lifecycle_event_attrs(&operation, new_total_qty);

        out.push(ExecutionFlowRow {
            row_id: format!("lifecycle:{event_id}"),
            ts_utc: recorded_at,
            stage,
            severity,
            run_id: rid,
            internal_order_id: Some(internal_order_id),
            broker_order_id,
            symbol: None,
            message,
            source_table: "oms_order_lifecycle_events".to_string(),
        });
    }

    Ok(out)
}

fn lifecycle_event_attrs(operation: &str, new_total_qty: Option<i64>) -> (String, String, String) {
    match operation {
        "cancel_ack" => (
            "broker_cancel_ack".to_string(),
            "info".to_string(),
            "Cancel acknowledged by broker.".to_string(),
        ),
        "replace_ack" => {
            let msg = match new_total_qty {
                Some(qty) => format!("Replace acknowledged. New total qty: {qty}."),
                None => "Replace acknowledged.".to_string(),
            };
            ("broker_replace_ack".to_string(), "info".to_string(), msg)
        }
        "cancel_reject" => (
            "broker_cancel_reject".to_string(),
            "warn".to_string(),
            "Cancel rejected by broker.".to_string(),
        ),
        "replace_reject" => (
            "broker_replace_reject".to_string(),
            "warn".to_string(),
            "Replace rejected by broker.".to_string(),
        ),
        other => (
            format!("broker_lifecycle_{other}"),
            "info".to_string(),
            format!("Lifecycle event: {other}."),
        ),
    }
}

// ---------------------------------------------------------------------------
// Source 3: fill_quality_telemetry
// ---------------------------------------------------------------------------

async fn fetch_fill_flow_rows(
    pool: &PgPool,
    run_id: Option<Uuid>,
    order_id: Option<&str>,
    limit: i64,
) -> Result<Vec<ExecutionFlowRow>> {
    let db_rows = sqlx::query(
        r#"
        select telemetry_id, run_id, internal_order_id, broker_order_id,
               symbol, fill_kind, fill_received_at_utc
        from fill_quality_telemetry
        where ($1::uuid is null or run_id = $1)
          and ($2::text is null or internal_order_id = $2)
        order by fill_received_at_utc asc
        limit $3
        "#,
    )
    .bind(run_id)
    .bind(order_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("fill flow query failed")?;

    let mut out = Vec::new();
    for r in db_rows {
        let telemetry_id: Uuid = r.try_get("telemetry_id")?;
        let rid: Uuid = r.try_get("run_id")?;
        let internal_order_id: String = r.try_get("internal_order_id")?;
        let broker_order_id: Option<String> = r.try_get("broker_order_id")?;
        let symbol: String = r.try_get("symbol")?;
        let fill_kind: String = r.try_get("fill_kind")?;
        let received_at: DateTime<Utc> = r.try_get("fill_received_at_utc")?;

        let (stage, message) = fill_event_attrs(&fill_kind, &symbol);

        out.push(ExecutionFlowRow {
            row_id: format!("fill:{telemetry_id}"),
            ts_utc: received_at,
            stage,
            severity: "info".to_string(),
            run_id: rid,
            internal_order_id: Some(internal_order_id),
            broker_order_id,
            symbol: Some(symbol),
            message,
            source_table: "fill_quality_telemetry".to_string(),
        });
    }

    Ok(out)
}

fn fill_event_attrs(fill_kind: &str, symbol: &str) -> (String, String) {
    match fill_kind {
        "partial_fill" => (
            "broker_partial_fill".to_string(),
            format!("Partial fill received for {symbol}."),
        ),
        "final_fill" => (
            "broker_final_fill".to_string(),
            format!("Final fill received for {symbol}."),
        ),
        other => (
            format!("broker_fill_{other}"),
            format!("Fill event for {symbol}: {other}."),
        ),
    }
}

// ---------------------------------------------------------------------------
// Pure unit tests (no DB required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_event_attrs_cancel_ack() {
        let (stage, sev, msg) = lifecycle_event_attrs("cancel_ack", None);
        assert_eq!(stage, "broker_cancel_ack");
        assert_eq!(sev, "info");
        assert!(msg.contains("Cancel acknowledged"));
    }

    #[test]
    fn lifecycle_event_attrs_replace_ack_with_qty() {
        let (stage, sev, msg) = lifecycle_event_attrs("replace_ack", Some(50));
        assert_eq!(stage, "broker_replace_ack");
        assert_eq!(sev, "info");
        assert!(msg.contains("50"));
    }

    #[test]
    fn lifecycle_event_attrs_cancel_reject_is_warn() {
        let (stage, sev, _msg) = lifecycle_event_attrs("cancel_reject", None);
        assert_eq!(stage, "broker_cancel_reject");
        assert_eq!(sev, "warn");
    }

    #[test]
    fn lifecycle_event_attrs_replace_reject_is_warn() {
        let (stage, sev, _msg) = lifecycle_event_attrs("replace_reject", None);
        assert_eq!(stage, "broker_replace_reject");
        assert_eq!(sev, "warn");
    }

    #[test]
    fn fill_event_attrs_partial_fill() {
        let (stage, msg) = fill_event_attrs("partial_fill", "NVDA");
        assert_eq!(stage, "broker_partial_fill");
        assert!(msg.contains("NVDA"));
        assert!(msg.contains("Partial"));
    }

    #[test]
    fn fill_event_attrs_final_fill() {
        let (stage, _msg) = fill_event_attrs("final_fill", "TSLA");
        assert_eq!(stage, "broker_final_fill");
    }
}
