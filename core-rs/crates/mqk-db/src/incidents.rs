//! OPS-01: durable incident persistence.
//!
//! `sys_incidents` stores one row per operator-declared incident.  Incidents
//! are distinct from alert acknowledgments (OPS-02): an ack records that an
//! operator noticed a fault; an incident records a deliberate decision to
//! track a fault condition as a named, lifecycle-bearing event.
//!
//! # Lifecycle
//!
//! `status`: `"open"` → `"resolved"`.  The route layer validates on write;
//! no DB constraint enforces the transition.
//!
//! # Incident ID
//!
//! Callers derive `incident_id` as a UUIDv5 over the incident namespace +
//! `title:opened_at_utc` so that re-submits at identical wall time are
//! idempotent (INSERT ... ON CONFLICT DO NOTHING).

use anyhow::Result;
use sqlx::{PgPool, Row};

/// A persisted incident record from `sys_incidents`.
#[derive(Debug, Clone)]
pub struct IncidentDbRow {
    pub incident_id: String,
    pub opened_at_utc: chrono::DateTime<chrono::Utc>,
    pub title: String,
    pub severity: String,
    pub status: String,
    pub linked_alert_id: Option<String>,
    pub opened_by: String,
}

/// Arguments for inserting a new incident.
pub struct InsertIncidentArgs<'a> {
    pub incident_id: &'a str,
    pub opened_at_utc: chrono::DateTime<chrono::Utc>,
    pub title: &'a str,
    pub severity: &'a str,
    pub linked_alert_id: Option<&'a str>,
    pub opened_by: &'a str,
}

/// Insert a new incident.
///
/// Uses `ON CONFLICT (incident_id) DO NOTHING` so that an identical
/// UUIDv5-derived ID submitted twice leaves the row unchanged.  Returns
/// `None` when the row already existed (idempotent no-op).
pub async fn insert_incident(
    db: &PgPool,
    args: InsertIncidentArgs<'_>,
) -> Result<Option<IncidentDbRow>> {
    let row = sqlx::query(
        r#"
        INSERT INTO sys_incidents
            (incident_id, opened_at_utc, title, severity, status, linked_alert_id, opened_by)
        VALUES ($1, $2, $3, $4, 'open', $5, $6)
        ON CONFLICT (incident_id) DO NOTHING
        RETURNING incident_id, opened_at_utc, title, severity, status, linked_alert_id, opened_by
        "#,
    )
    .bind(args.incident_id)
    .bind(args.opened_at_utc)
    .bind(args.title)
    .bind(args.severity)
    .bind(args.linked_alert_id)
    .bind(args.opened_by)
    .fetch_optional(db)
    .await?;

    Ok(row.map(|r| IncidentDbRow {
        incident_id: r.get("incident_id"),
        opened_at_utc: r.get("opened_at_utc"),
        title: r.get("title"),
        severity: r.get("severity"),
        status: r.get("status"),
        linked_alert_id: r.get("linked_alert_id"),
        opened_by: r.get("opened_by"),
    }))
}

/// Resolve an existing incident by setting `status = 'resolved'`.
///
/// Uses `UPDATE … RETURNING` so the caller receives the post-update row.
/// Returns `None` when no row with the given `incident_id` exists (→ 404).
/// Idempotent: if the incident is already `"resolved"` the UPDATE still
/// succeeds and the updated row is returned unchanged.
pub async fn resolve_incident(
    db: &PgPool,
    incident_id: &str,
) -> Result<Option<IncidentDbRow>> {
    let row = sqlx::query(
        r#"
        UPDATE sys_incidents
        SET status = 'resolved'
        WHERE incident_id = $1
        RETURNING incident_id, opened_at_utc, title, severity, status, linked_alert_id, opened_by
        "#,
    )
    .bind(incident_id)
    .fetch_optional(db)
    .await?;

    Ok(row.map(|r| IncidentDbRow {
        incident_id: r.get("incident_id"),
        opened_at_utc: r.get("opened_at_utc"),
        title: r.get("title"),
        severity: r.get("severity"),
        status: r.get("status"),
        linked_alert_id: r.get("linked_alert_id"),
        opened_by: r.get("opened_by"),
    }))
}

/// Load all incidents ordered newest-first.
///
/// Returns an authoritative empty `Vec` when no incidents exist.
pub async fn list_incidents(db: &PgPool) -> Result<Vec<IncidentDbRow>> {
    let rows = sqlx::query(
        r#"
        SELECT incident_id, opened_at_utc, title, severity, status, linked_alert_id, opened_by
        FROM sys_incidents
        ORDER BY opened_at_utc DESC, incident_id DESC
        "#,
    )
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| IncidentDbRow {
            incident_id: r.get("incident_id"),
            opened_at_utc: r.get("opened_at_utc"),
            title: r.get("title"),
            severity: r.get("severity"),
            status: r.get("status"),
            linked_alert_id: r.get("linked_alert_id"),
            opened_by: r.get("opened_by"),
        })
        .collect())
}
