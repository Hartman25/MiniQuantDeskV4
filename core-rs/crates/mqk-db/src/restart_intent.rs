//! CC-03B: Durable restart intent / restart provenance.
//!
//! Provides the DB read/write layer for `sys_restart_intent`.  Every durable
//! record carries a `transition_verdict` that must be derived from the
//! canonical CC-03A mode-transition seam — callers must not invent verdict
//! strings that differ from `ModeTransitionVerdict::as_str()`.
//!
//! # Lifecycle
//!
//! ```text
//! insert_restart_intent  →  status = 'pending'
//!                           ↓                ↓               ↓
//!                      completed        cancelled       superseded
//! ```
//!
//! # Provenance
//!
//! `initiated_by` distinguishes who initiated the intent:
//! - `"operator"` — direct operator action through the control plane
//! - `"system"`   — daemon's own decision (e.g., automatic recovery)
//! - `"recovery"` — explicit restart-recovery flow

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Arguments for inserting a new restart intent.
///
/// `from_mode` and `to_mode` must be `DeploymentMode::as_api_label()` values.
/// `transition_verdict` must be `ModeTransitionVerdict::as_str()` — one of
/// `"same_mode"`, `"admissible_with_restart"`, `"refused"`, `"fail_closed"`.
/// `initiated_by` must be one of `"operator"`, `"system"`, `"recovery"`.
/// `initiated_at_utc` is caller-injected; never derived from `Utc::now()` here.
#[derive(Debug, Clone)]
pub struct NewRestartIntent {
    pub intent_id: Uuid,
    pub engine_id: String,
    /// Current deployment mode label at time of intent (e.g. `"paper"`).
    pub from_mode: String,
    /// Intended target deployment mode label (e.g. `"live-shadow"`).
    pub to_mode: String,
    /// CC-03A canonical transition verdict string.
    pub transition_verdict: String,
    /// Who/what initiated this intent: `"operator"`, `"system"`, or `"recovery"`.
    pub initiated_by: String,
    /// Timestamp when the intent was initiated.  Caller-injected; no DB default.
    pub initiated_at_utc: DateTime<Utc>,
    /// Optional operator note or provenance reference.
    pub note: String,
}

/// A row read back from `sys_restart_intent`.
#[derive(Debug, Clone)]
pub struct RestartIntentRow {
    pub intent_id: Uuid,
    pub engine_id: String,
    pub from_mode: String,
    pub to_mode: String,
    pub transition_verdict: String,
    pub initiated_by: String,
    pub initiated_at_utc: DateTime<Utc>,
    pub status: String,
    pub completed_at_utc: Option<DateTime<Utc>>,
    pub note: String,
}

// ---------------------------------------------------------------------------
// Row mapping
// ---------------------------------------------------------------------------

fn row_from_pg(row: sqlx::postgres::PgRow) -> Result<RestartIntentRow> {
    Ok(RestartIntentRow {
        intent_id: row.try_get("intent_id")?,
        engine_id: row.try_get("engine_id")?,
        from_mode: row.try_get("from_mode")?,
        to_mode: row.try_get("to_mode")?,
        transition_verdict: row.try_get("transition_verdict")?,
        initiated_by: row.try_get("initiated_by")?,
        initiated_at_utc: row.try_get("initiated_at_utc")?,
        status: row.try_get("status")?,
        completed_at_utc: row.try_get("completed_at_utc")?,
        note: row.try_get("note")?,
    })
}

// ---------------------------------------------------------------------------
// Write path
// ---------------------------------------------------------------------------

/// Insert a new restart intent with `status = 'pending'`.
///
/// `intent_id` must be unique; callers are responsible for generating it.
/// The `transition_verdict` is stored as-is — callers must derive it from
/// `evaluate_mode_transition(from, to).as_str()` for coherence with CC-03A.
pub async fn insert_restart_intent(pool: &PgPool, args: &NewRestartIntent) -> Result<()> {
    sqlx::query(
        r#"
        insert into sys_restart_intent (
            intent_id, engine_id, from_mode, to_mode, transition_verdict,
            initiated_by, initiated_at_utc, status, note
        ) values (
            $1, $2, $3, $4, $5, $6, $7, 'pending', $8
        )
        "#,
    )
    .bind(args.intent_id)
    .bind(&args.engine_id)
    .bind(&args.from_mode)
    .bind(&args.to_mode)
    .bind(&args.transition_verdict)
    .bind(&args.initiated_by)
    .bind(args.initiated_at_utc)
    .bind(&args.note)
    .execute(pool)
    .await
    .context("insert_restart_intent failed")?;
    Ok(())
}

/// Transition a pending intent to a terminal status.
///
/// Only rows with `status = 'pending'` are updated.  Returns `true` when a
/// row was updated, `false` when the intent was not found or was already in a
/// terminal state (idempotent).
///
/// `status` must be one of: `"completed"`, `"cancelled"`, `"superseded"`.
/// `completed_at_utc` is caller-injected — never derived from `Utc::now()` here.
pub async fn update_restart_intent_status(
    pool: &PgPool,
    intent_id: Uuid,
    status: &str,
    completed_at_utc: DateTime<Utc>,
) -> Result<bool> {
    let res = sqlx::query(
        r#"
        update sys_restart_intent
        set status = $2,
            completed_at_utc = $3
        where intent_id = $1
          and status = 'pending'
        "#,
    )
    .bind(intent_id)
    .bind(status)
    .bind(completed_at_utc)
    .execute(pool)
    .await
    .context("update_restart_intent_status failed")?;
    Ok(res.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// Read path
// ---------------------------------------------------------------------------

/// Fetch the most recent restart intent for `engine_id` regardless of status.
///
/// Returns `Ok(None)` when no intent exists — this is an honest "no record"
/// state, not a failure.  Returns `Err` only on a genuine DB fault.
pub async fn fetch_latest_restart_intent_for_engine(
    pool: &PgPool,
    engine_id: &str,
) -> Result<Option<RestartIntentRow>> {
    let row = sqlx::query(
        r#"
        select intent_id, engine_id, from_mode, to_mode, transition_verdict,
               initiated_by, initiated_at_utc, status, completed_at_utc, note
        from sys_restart_intent
        where engine_id = $1
        order by initiated_at_utc desc, intent_id desc
        limit 1
        "#,
    )
    .bind(engine_id)
    .fetch_optional(pool)
    .await
    .context("fetch_latest_restart_intent_for_engine failed")?;

    row.map(row_from_pg).transpose()
}

/// Fetch the most recent **pending** restart intent for `engine_id`.
///
/// Returns `Ok(None)` when no pending intent exists — honest absence.
/// Returns `Err` only on a genuine DB fault.
pub async fn fetch_pending_restart_intent_for_engine(
    pool: &PgPool,
    engine_id: &str,
) -> Result<Option<RestartIntentRow>> {
    let row = sqlx::query(
        r#"
        select intent_id, engine_id, from_mode, to_mode, transition_verdict,
               initiated_by, initiated_at_utc, status, completed_at_utc, note
        from sys_restart_intent
        where engine_id = $1
          and status = 'pending'
        order by initiated_at_utc desc, intent_id desc
        limit 1
        "#,
    )
    .bind(engine_id)
    .fetch_optional(pool)
    .await
    .context("fetch_pending_restart_intent_for_engine failed")?;

    row.map(row_from_pg).transpose()
}
