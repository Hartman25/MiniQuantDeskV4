use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// CC-01A: Authoritative strategy registry (sys_strategy_registry)
// ---------------------------------------------------------------------------

/// One row from `sys_strategy_registry`.
#[derive(Debug, Clone)]
pub struct StrategyRegistryRecord {
    /// Canonical strategy identity — the natural primary key.
    pub strategy_id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Whether this strategy is currently enabled.
    pub enabled: bool,
    /// Operator-assigned category (e.g. `"external_signal"`, `"bar_driven"`).
    /// Empty string when unclassified.
    pub kind: String,
    /// UTC timestamp when first registered (TimeSource-injected; not updated on upsert).
    pub registered_at_utc: DateTime<Utc>,
    /// UTC timestamp of the most recent upsert (TimeSource-injected).
    pub updated_at_utc: DateTime<Utc>,
    /// Optional operator note; empty string when not provided.
    pub note: String,
}

/// Arguments for upserting a strategy registry entry.
#[derive(Debug, Clone)]
pub struct UpsertStrategyRegistryArgs {
    /// Canonical strategy identity.  Must not be empty.
    pub strategy_id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Whether the strategy is operationally enabled.
    pub enabled: bool,
    /// Operator-assigned category string.
    pub kind: String,
    /// Provided by the caller from a `TimeSource`.
    /// Written as `registered_at_utc` on first insert; ignored on conflict
    /// (the original registration timestamp is preserved).
    pub registered_at_utc: DateTime<Utc>,
    /// Provided by the caller from a `TimeSource`.
    /// Always written/updated on every upsert.
    pub updated_at_utc: DateTime<Utc>,
    /// Optional operator note; pass empty string when not applicable.
    pub note: String,
}

/// Upsert a strategy registry entry.
///
/// On first insert: all fields are written as supplied.
/// On conflict (same `strategy_id`): `display_name`, `enabled`, `kind`,
/// `updated_at_utc`, and `note` are updated; `registered_at_utc` is preserved
/// from the original insert.
///
/// Returns `Err` if `strategy_id` is empty (validation occurs before any DB
/// contact).
pub async fn upsert_strategy_registry_entry(
    pool: &PgPool,
    args: &UpsertStrategyRegistryArgs,
) -> Result<()> {
    if args.strategy_id.trim().is_empty() {
        anyhow::bail!("upsert_strategy_registry_entry: strategy_id must not be empty");
    }
    sqlx::query(
        r#"
        insert into sys_strategy_registry
            (strategy_id, display_name, enabled, kind, registered_at_utc, updated_at_utc, note)
        values ($1, $2, $3, $4, $5, $6, $7)
        on conflict (strategy_id) do update set
            display_name      = excluded.display_name,
            enabled           = excluded.enabled,
            kind              = excluded.kind,
            updated_at_utc    = excluded.updated_at_utc,
            note              = excluded.note
        "#,
    )
    .bind(&args.strategy_id)
    .bind(&args.display_name)
    .bind(args.enabled)
    .bind(&args.kind)
    .bind(args.registered_at_utc)
    .bind(args.updated_at_utc)
    .bind(&args.note)
    .execute(pool)
    .await
    .context("upsert_strategy_registry_entry failed")?;
    Ok(())
}

/// Fetch all strategy registry entries ordered by `strategy_id`.
///
/// An empty `Vec` is authoritative: it means no strategies have been
/// registered, not that the registry is unavailable.  Callers must not
/// synthesize fake strategy rows when this returns empty.
pub async fn fetch_strategy_registry(pool: &PgPool) -> Result<Vec<StrategyRegistryRecord>> {
    let rows = sqlx::query(
        r#"
        select strategy_id, display_name, enabled, kind,
               registered_at_utc, updated_at_utc, note
        from sys_strategy_registry
        order by strategy_id
        "#,
    )
    .fetch_all(pool)
    .await
    .context("fetch_strategy_registry failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(StrategyRegistryRecord {
            strategy_id: r.try_get("strategy_id")?,
            display_name: r.try_get("display_name")?,
            enabled: r.try_get("enabled")?,
            kind: r.try_get("kind")?,
            registered_at_utc: r.try_get("registered_at_utc")?,
            updated_at_utc: r.try_get("updated_at_utc")?,
            note: r.try_get("note")?,
        });
    }
    Ok(out)
}

/// Fetch a single strategy registry entry by `strategy_id`.
///
/// Returns `Ok(None)` if no entry exists for that ID.  Callers must not
/// treat `None` as "all good / empty means none active" — it means the
/// identity is not registered.
pub async fn fetch_strategy_registry_entry(
    pool: &PgPool,
    strategy_id: &str,
) -> Result<Option<StrategyRegistryRecord>> {
    let row = sqlx::query(
        r#"
        select strategy_id, display_name, enabled, kind,
               registered_at_utc, updated_at_utc, note
        from sys_strategy_registry
        where strategy_id = $1
        "#,
    )
    .bind(strategy_id)
    .fetch_optional(pool)
    .await
    .context("fetch_strategy_registry_entry failed")?;

    match row {
        None => Ok(None),
        Some(r) => Ok(Some(StrategyRegistryRecord {
            strategy_id: r.try_get("strategy_id")?,
            display_name: r.try_get("display_name")?,
            enabled: r.try_get("enabled")?,
            kind: r.try_get("kind")?,
            registered_at_utc: r.try_get("registered_at_utc")?,
            updated_at_utc: r.try_get("updated_at_utc")?,
            note: r.try_get("note")?,
        })),
    }
}

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

/// Fetch the current active suppression for a specific strategy, if any.
///
/// Returns `Ok(Some(record))` when an active suppression exists for the
/// strategy, `Ok(None)` when no active suppression exists (either none was
/// ever created or it has been cleared).
///
/// Used by the internal decision seam (Gate 4) to check per-strategy
/// suppression state without loading all suppressions.  The query targets
/// the `(strategy_id, state)` index for efficiency.
pub async fn fetch_active_suppression_for_strategy(
    pool: &PgPool,
    strategy_id: &str,
) -> Result<Option<StrategySuppressionRecord>> {
    let row = sqlx::query(
        r#"
        select suppression_id, strategy_id, state, trigger_domain, trigger_reason,
               started_at_utc, cleared_at_utc, note
        from sys_strategy_suppressions
        where strategy_id = $1
          and state = 'active'
        order by started_at_utc desc
        limit 1
        "#,
    )
    .bind(strategy_id)
    .fetch_optional(pool)
    .await
    .context("fetch_active_suppression_for_strategy failed")?;

    row.map(|r| {
        Ok(StrategySuppressionRecord {
            suppression_id: r.try_get("suppression_id")?,
            strategy_id: r.try_get("strategy_id")?,
            state: r.try_get("state")?,
            trigger_domain: r.try_get("trigger_domain")?,
            trigger_reason: r.try_get("trigger_reason")?,
            started_at_utc: r.try_get("started_at_utc")?,
            cleared_at_utc: r.try_get("cleared_at_utc")?,
            note: r.try_get("note")?,
        })
    })
    .transpose()
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
