use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// CC-02: Durable strategy suppression persistence (sys_strategy_suppressions)
// ---------------------------------------------------------------------------

/// One row from `sys_strategy_suppressions`.
#[derive(Debug, Clone)]
pub struct StrategySuppressionRecord {
    /// UUID primary key, provided by the caller (no synthetic generation here).
    pub suppression_id: Uuid,
    /// Authoritative strategy identity.
    pub strategy_id: String,
    /// `"active"` or `"cleared"`.
    pub state: String,
    /// Category of the trigger (e.g. `"operator"`, `"risk"`, `"integrity"`).
    pub trigger_domain: String,
    /// Human-readable one-line reason.
    pub trigger_reason: String,
    /// UTC timestamp when the suppression was created (TimeSource-injected by caller).
    pub started_at_utc: DateTime<Utc>,
    /// UTC timestamp when cleared; `None` while still active.
    pub cleared_at_utc: Option<DateTime<Utc>>,
    /// Optional operator note; empty string when not provided.
    pub note: String,
}

/// Arguments for inserting a new strategy suppression.
#[derive(Debug, Clone)]
pub struct InsertStrategySuppressionArgs {
    /// Caller-provided UUID; must be deterministic or at least caller-owned.
    pub suppression_id: Uuid,
    pub strategy_id: String,
    pub trigger_domain: String,
    pub trigger_reason: String,
    /// Provided by the caller from a `TimeSource`; not derived from `now()` here.
    pub started_at_utc: DateTime<Utc>,
    pub note: String,
}

/// Insert a new active strategy suppression.
///
/// Uses `ON CONFLICT (suppression_id) DO NOTHING` — idempotent.
/// Repeated inserts for the same `suppression_id` are silent no-ops.
pub async fn insert_strategy_suppression(
    pool: &PgPool,
    args: &InsertStrategySuppressionArgs,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into sys_strategy_suppressions
            (suppression_id, strategy_id, state, trigger_domain, trigger_reason, started_at_utc, note)
        values ($1, $2, 'active', $3, $4, $5, $6)
        on conflict (suppression_id) do nothing
        "#,
    )
    .bind(args.suppression_id)
    .bind(&args.strategy_id)
    .bind(&args.trigger_domain)
    .bind(&args.trigger_reason)
    .bind(args.started_at_utc)
    .bind(&args.note)
    .execute(pool)
    .await
    .context("insert_strategy_suppression failed")?;
    Ok(())
}

/// Clear an active suppression by ID.
///
/// Sets `state = 'cleared'` and `cleared_at_utc = cleared_at_utc` only if
/// the row is currently `'active'`.  Returns `true` if a row was updated,
/// `false` if no active row matched (already cleared or not found).
pub async fn clear_strategy_suppression(
    pool: &PgPool,
    suppression_id: Uuid,
    cleared_at_utc: DateTime<Utc>,
) -> Result<bool> {
    let result = sqlx::query(
        r#"
        update sys_strategy_suppressions
        set state = 'cleared', cleared_at_utc = $2
        where suppression_id = $1 and state = 'active'
        "#,
    )
    .bind(suppression_id)
    .bind(cleared_at_utc)
    .execute(pool)
    .await
    .context("clear_strategy_suppression failed")?;
    Ok(result.rows_affected() > 0)
}

/// Fetch all strategy suppressions ordered newest-first.
///
/// Returns active and cleared suppressions.  The route layer can filter by
/// state if needed; the full set is returned here for operator visibility.
/// Returns an empty `Vec` if no suppressions have been recorded.
pub async fn fetch_strategy_suppressions(pool: &PgPool) -> Result<Vec<StrategySuppressionRecord>> {
    let rows = sqlx::query(
        r#"
        select suppression_id, strategy_id, state, trigger_domain, trigger_reason,
               started_at_utc, cleared_at_utc, note
        from sys_strategy_suppressions
        order by started_at_utc desc
        "#,
    )
    .fetch_all(pool)
    .await
    .context("fetch_strategy_suppressions failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(StrategySuppressionRecord {
            suppression_id: r.try_get("suppression_id")?,
            strategy_id: r.try_get("strategy_id")?,
            state: r.try_get("state")?,
            trigger_domain: r.try_get("trigger_domain")?,
            trigger_reason: r.try_get("trigger_reason")?,
            started_at_utc: r.try_get("started_at_utc")?,
            cleared_at_utc: r.try_get("cleared_at_utc")?,
            note: r.try_get("note")?,
        });
    }
    Ok(out)
}
