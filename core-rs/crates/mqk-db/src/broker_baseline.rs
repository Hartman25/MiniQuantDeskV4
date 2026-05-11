//! Broker position baseline — durable operator-confirmed adoption record.
//!
//! Stores a single-row sentinel snapshot that the reconcile tick uses as
//! local truth when no execution run is active.  Written by the
//! `POST /api/v1/ops/repair/adopt-broker-position-baseline` route.
//!
//! # Sentinel invariant
//!
//! Only one row (sentinel_id = 1) ever exists.  Upsert is idempotent with
//! respect to the sentinel row — subsequent adoptions overwrite the prior one.

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Row returned from `load_broker_position_baseline`.
#[derive(Debug, Clone)]
pub struct BrokerBaselineRow {
    /// Serialized `mqk_schemas::BrokerSnapshot` JSON captured at adoption time.
    pub broker_snapshot_json: serde_json::Value,
    /// When the operator adopted this baseline.
    pub adopted_at_utc: DateTime<Utc>,
    /// Confirmation string supplied by the operator (e.g. `ADOPT_BROKER_POSITION_BASELINE`).
    pub adopted_by: String,
    /// Deterministic audit event ID written alongside the adoption.
    pub audit_event_id: Uuid,
}

/// Upsert the broker position baseline (idempotent — overwrites sentinel_id=1).
pub async fn upsert_broker_position_baseline(
    pool: &PgPool,
    broker_snapshot_json: &serde_json::Value,
    adopted_at_utc: DateTime<Utc>,
    adopted_by: &str,
    audit_event_id: Uuid,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into sys_broker_position_baseline
            (sentinel_id, broker_snapshot_json, adopted_at_utc, adopted_by, audit_event_id)
        values (1, $1, $2, $3, $4)
        on conflict (sentinel_id) do update
            set broker_snapshot_json = excluded.broker_snapshot_json,
                adopted_at_utc       = excluded.adopted_at_utc,
                adopted_by           = excluded.adopted_by,
                audit_event_id       = excluded.audit_event_id
        "#,
    )
    .bind(broker_snapshot_json)
    .bind(adopted_at_utc)
    .bind(adopted_by)
    .bind(audit_event_id)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|e| anyhow::anyhow!("upsert_broker_position_baseline failed: {e}"))
}

/// Load the current broker position baseline, if one has been adopted.
pub async fn load_broker_position_baseline(pool: &PgPool) -> Result<Option<BrokerBaselineRow>> {
    let row: Option<(serde_json::Value, DateTime<Utc>, String, Uuid)> = sqlx::query_as(
        r#"
        select broker_snapshot_json, adopted_at_utc, adopted_by, audit_event_id
        from sys_broker_position_baseline
        where sentinel_id = 1
        "#,
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| anyhow::anyhow!("load_broker_position_baseline failed: {e}"))?;

    Ok(row.map(
        |(broker_snapshot_json, adopted_at_utc, adopted_by, audit_event_id)| BrokerBaselineRow {
            broker_snapshot_json,
            adopted_at_utc,
            adopted_by,
            audit_event_id,
        },
    ))
}

/// Delete the broker position baseline (clear adoption).
pub async fn clear_broker_position_baseline(pool: &PgPool) -> Result<()> {
    sqlx::query("delete from sys_broker_position_baseline where sentinel_id = 1")
        .execute(pool)
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("clear_broker_position_baseline failed: {e}"))
}
