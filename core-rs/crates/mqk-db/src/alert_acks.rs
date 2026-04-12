//! OPS-02: durable operator alert-ack persistence.
//!
//! `sys_alert_acks` stores one row per acknowledged alert class slug.
//! The source of truth for *active* alerts remains the in-memory fault-signal
//! computation; this table only tracks whether an operator has acknowledged
//! a given alert class.
//!
//! Ack is advisory and idempotent (upsert pattern).  A subsequent ack on the
//! same `alert_id` updates the timestamp and `acked_by`.

use anyhow::Result;
use sqlx::{PgPool, Row};

/// A persisted alert acknowledgment record from `sys_alert_acks`.
#[derive(Debug, Clone)]
pub struct AlertAckRow {
    pub alert_id: String,
    pub acked_at_utc: chrono::DateTime<chrono::Utc>,
    pub acked_by: String,
}

/// Upsert an alert acknowledgment.
///
/// If `alert_id` already exists the row is updated with the new
/// `acked_at_utc` and `acked_by`.  Returns the written row.
pub async fn upsert_alert_ack(
    db: &PgPool,
    alert_id: &str,
    acked_at_utc: chrono::DateTime<chrono::Utc>,
    acked_by: &str,
) -> Result<AlertAckRow> {
    let row = sqlx::query(
        r#"
        INSERT INTO sys_alert_acks (alert_id, acked_at_utc, acked_by)
        VALUES ($1, $2, $3)
        ON CONFLICT (alert_id) DO UPDATE
          SET acked_at_utc = EXCLUDED.acked_at_utc,
              acked_by     = EXCLUDED.acked_by
        RETURNING alert_id, acked_at_utc, acked_by
        "#,
    )
    .bind(alert_id)
    .bind(acked_at_utc)
    .bind(acked_by)
    .fetch_one(db)
    .await?;

    Ok(AlertAckRow {
        alert_id: row.get("alert_id"),
        acked_at_utc: row.get("acked_at_utc"),
        acked_by: row.get("acked_by"),
    })
}

/// Load all persisted ack records.
///
/// Returns an empty `Vec` when the table is empty — authoritative empty
/// (no acks recorded, not absence of source).
pub async fn load_alert_acks(db: &PgPool) -> Result<Vec<AlertAckRow>> {
    let rows = sqlx::query("SELECT alert_id, acked_at_utc, acked_by FROM sys_alert_acks")
        .fetch_all(db)
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| AlertAckRow {
            alert_id: r.get("alert_id"),
            acked_at_utc: r.get("acked_at_utc"),
            acked_by: r.get("acked_by"),
        })
        .collect())
}
