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

// ---------------------------------------------------------------------------
// AUTON-NO-SIGNAL-OBS-01: Durable strategy signal-evaluation journal
// (strategy_signal_evaluations)
// ---------------------------------------------------------------------------

/// One row from `strategy_signal_evaluations`.
#[derive(Debug, Clone)]
pub struct StrategySignalEvaluationRecord {
    /// Deterministic UUIDv5; caller-derived, never `Uuid::new_v4()`.
    pub evaluation_id: Uuid,
    pub ts_utc: DateTime<Utc>,
    /// `None` when no run was active at evaluation time (observability only —
    /// not part of the outbox/inbox/run lifecycle chain).
    pub run_id: Option<Uuid>,
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe: String,
    /// `"db_loaded"`, `"no_bars_available"`, or `"stale_bars"`.
    pub bar_context_source: String,
    /// Number of completed `md_bars` rows loaded for this attempt. `0` when
    /// `bar_context_source = "no_bars_available"`.
    pub bars_loaded: i64,
    /// `None` only when no completed bars exist at all.
    pub latest_bar_ts_utc: Option<DateTime<Utc>>,
    /// `false` means the strategy's targets summed to zero (or `on_bar` never
    /// ran because a pre-dispatch gate refused first) — informational, not an
    /// error.
    pub signal_generated: bool,
    /// Signed sum of strategy target quantities. `None` when `on_bar` never ran.
    pub signal_qty: Option<i64>,
    /// `"buy"` / `"sell"` derived from the sign of `signal_qty`; `None` when
    /// `signal_qty` is `None` or zero.
    pub signal_side: Option<String>,
    pub reason_code: String,
    pub reason: String,
    /// `"pre_dispatch_gate"` or `"strategy_evaluated"`.
    pub decision_stage: String,
    pub source: String,
}

/// Arguments for inserting a new strategy signal-evaluation row.
#[derive(Debug, Clone)]
pub struct InsertStrategySignalEvaluationArgs {
    pub evaluation_id: Uuid,
    pub ts_utc: DateTime<Utc>,
    pub run_id: Option<Uuid>,
    pub strategy_id: String,
    pub symbol: String,
    pub timeframe: String,
    pub bar_context_source: String,
    pub bars_loaded: i64,
    pub latest_bar_ts_utc: Option<DateTime<Utc>>,
    pub signal_generated: bool,
    pub signal_qty: Option<i64>,
    pub signal_side: Option<String>,
    pub reason_code: String,
    pub reason: String,
    pub decision_stage: String,
    pub source: String,
}

/// Persist a single strategy signal-evaluation row.
///
/// Idempotent via `ON CONFLICT (evaluation_id) DO NOTHING` — a duplicate
/// write attempt for the same logical tick (same deterministic
/// `evaluation_id`) can never produce a second row. Never writes to, reads
/// from, or otherwise touches `oms_outbox`/`oms_inbox`/`runs`.
pub async fn insert_strategy_signal_evaluation(
    pool: &PgPool,
    args: &InsertStrategySignalEvaluationArgs,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into strategy_signal_evaluations (
            evaluation_id, ts_utc, run_id, strategy_id, symbol, timeframe,
            bar_context_source, bars_loaded, latest_bar_ts_utc,
            signal_generated, signal_qty, signal_side,
            reason_code, reason, decision_stage, source
        )
        values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)
        on conflict (evaluation_id) do nothing
        "#,
    )
    .bind(args.evaluation_id)
    .bind(args.ts_utc)
    .bind(args.run_id)
    .bind(&args.strategy_id)
    .bind(&args.symbol)
    .bind(&args.timeframe)
    .bind(&args.bar_context_source)
    .bind(args.bars_loaded)
    .bind(args.latest_bar_ts_utc)
    .bind(args.signal_generated)
    .bind(args.signal_qty)
    .bind(&args.signal_side)
    .bind(&args.reason_code)
    .bind(&args.reason)
    .bind(&args.decision_stage)
    .bind(&args.source)
    .execute(pool)
    .await
    .context("insert_strategy_signal_evaluation failed")?;
    Ok(())
}

/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4-EVALUATION-LINEAGE-AND-AUTONOMOUS-
/// PREOPEN-CLOSURE-01 REPAIR 2: fetch one exact strategy signal-evaluation
/// row by its deterministic `evaluation_id`. `Ok(None)` is authoritative "no
/// such evaluation row exists" — never a synthesized row. Read-only; used by
/// the completed-bar dispatch-claim path to durably confirm that a canonical
/// strategy evaluation actually occurred for this claim's exact identity
/// before the claim may be reported completed.
pub async fn fetch_strategy_signal_evaluation(
    pool: &PgPool,
    evaluation_id: Uuid,
) -> Result<Option<StrategySignalEvaluationRecord>> {
    let row = sqlx::query(
        r#"
        select evaluation_id, ts_utc, run_id, strategy_id, symbol, timeframe,
               bar_context_source, bars_loaded, latest_bar_ts_utc,
               signal_generated, signal_qty, signal_side,
               reason_code, reason, decision_stage, source
        from strategy_signal_evaluations
        where evaluation_id = $1
        "#,
    )
    .bind(evaluation_id)
    .fetch_optional(pool)
    .await
    .context("fetch_strategy_signal_evaluation failed")?;

    row.map(|r| {
        Ok(StrategySignalEvaluationRecord {
            evaluation_id: r.try_get("evaluation_id")?,
            ts_utc: r.try_get("ts_utc")?,
            run_id: r.try_get("run_id")?,
            strategy_id: r.try_get("strategy_id")?,
            symbol: r.try_get("symbol")?,
            timeframe: r.try_get("timeframe")?,
            bar_context_source: r.try_get("bar_context_source")?,
            bars_loaded: r.try_get("bars_loaded")?,
            latest_bar_ts_utc: r.try_get("latest_bar_ts_utc")?,
            signal_generated: r.try_get("signal_generated")?,
            signal_qty: r.try_get("signal_qty")?,
            signal_side: r.try_get("signal_side")?,
            reason_code: r.try_get("reason_code")?,
            reason: r.try_get("reason")?,
            decision_stage: r.try_get("decision_stage")?,
            source: r.try_get("source")?,
        })
    })
    .transpose()
    .map_err(|e: sqlx::Error| anyhow::Error::from(e))
}

/// Fetch the most recent `limit` strategy signal-evaluation rows across all
/// runs and symbols, newest first.
///
/// An empty `Vec` is authoritative: it means no evaluation has been recorded
/// yet, not that the journal is unavailable.
pub async fn fetch_recent_strategy_signal_evaluations(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<StrategySignalEvaluationRecord>> {
    let rows = sqlx::query(
        r#"
        select evaluation_id, ts_utc, run_id, strategy_id, symbol, timeframe,
               bar_context_source, bars_loaded, latest_bar_ts_utc,
               signal_generated, signal_qty, signal_side,
               reason_code, reason, decision_stage, source
        from strategy_signal_evaluations
        order by ts_utc desc
        limit $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("fetch_recent_strategy_signal_evaluations failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(StrategySignalEvaluationRecord {
            evaluation_id: r.try_get("evaluation_id")?,
            ts_utc: r.try_get("ts_utc")?,
            run_id: r.try_get("run_id")?,
            strategy_id: r.try_get("strategy_id")?,
            symbol: r.try_get("symbol")?,
            timeframe: r.try_get("timeframe")?,
            bar_context_source: r.try_get("bar_context_source")?,
            bars_loaded: r.try_get("bars_loaded")?,
            latest_bar_ts_utc: r.try_get("latest_bar_ts_utc")?,
            signal_generated: r.try_get("signal_generated")?,
            signal_qty: r.try_get("signal_qty")?,
            signal_side: r.try_get("signal_side")?,
            reason_code: r.try_get("reason_code")?,
            reason: r.try_get("reason")?,
            decision_stage: r.try_get("decision_stage")?,
            source: r.try_get("source")?,
        });
    }
    Ok(out)
}

/// Fetch the most recent `limit` strategy signal-evaluation rows for a
/// single durable run, newest first.
///
/// PAPER-ORDER-LIFECYCLE-VIS-01B: run-scoped sibling of
/// `fetch_recent_strategy_signal_evaluations`, used to reconstruct one
/// run's lifecycle after a daemon restart. An empty `Vec` is authoritative:
/// it means no evaluation was recorded for this run, not that the journal
/// is unavailable. Read-only — never writes.
pub async fn fetch_strategy_signal_evaluations_for_run(
    pool: &PgPool,
    run_id: Uuid,
    limit: i64,
) -> Result<Vec<StrategySignalEvaluationRecord>> {
    let rows = sqlx::query(
        r#"
        select evaluation_id, ts_utc, run_id, strategy_id, symbol, timeframe,
               bar_context_source, bars_loaded, latest_bar_ts_utc,
               signal_generated, signal_qty, signal_side,
               reason_code, reason, decision_stage, source
        from strategy_signal_evaluations
        where run_id = $1
        order by ts_utc desc
        limit $2
        "#,
    )
    .bind(run_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("fetch_strategy_signal_evaluations_for_run failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(StrategySignalEvaluationRecord {
            evaluation_id: r.try_get("evaluation_id")?,
            ts_utc: r.try_get("ts_utc")?,
            run_id: r.try_get("run_id")?,
            strategy_id: r.try_get("strategy_id")?,
            symbol: r.try_get("symbol")?,
            timeframe: r.try_get("timeframe")?,
            bar_context_source: r.try_get("bar_context_source")?,
            bars_loaded: r.try_get("bars_loaded")?,
            latest_bar_ts_utc: r.try_get("latest_bar_ts_utc")?,
            signal_generated: r.try_get("signal_generated")?,
            signal_qty: r.try_get("signal_qty")?,
            signal_side: r.try_get("signal_side")?,
            reason_code: r.try_get("reason_code")?,
            reason: r.try_get("reason")?,
            decision_stage: r.try_get("decision_stage")?,
            source: r.try_get("source")?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// AUTON-NO-TRADE-OFFHOURS-01B: Durable autonomous no-trade diagnostic
// snapshot (autonomous_no_trade_diagnostics)
// ---------------------------------------------------------------------------

/// One row from `autonomous_no_trade_diagnostics`.
#[derive(Debug, Clone)]
pub struct AutonomousNoTradeDiagnosticRecord {
    /// Deterministic, minute-bucketed UUIDv5; caller-derived, never
    /// `Uuid::new_v4()`.
    pub diagnostic_id: Uuid,
    pub observed_at_utc: DateTime<Utc>,
    /// `None` when no run was active at observation time (the common
    /// off-hours case) — observability only, not part of the
    /// outbox/inbox/run lifecycle chain.
    pub run_id: Option<Uuid>,
    pub mode: String,
    /// `"in_window"` or `"outside_window"`.
    pub session_window_state: String,
    pub runtime_start_allowed: bool,
    pub arm_state: String,
    pub overall_ready: bool,
    pub reason_code: String,
    pub reason: String,
    pub stage: String,
    /// Always `false` for every row this patch writes.
    pub paper_order_attempted: bool,
    /// Always `false` for every row this patch writes.
    pub live_order_attempted: bool,
    pub source: String,
}

/// Arguments for inserting a new autonomous no-trade diagnostic row.
#[derive(Debug, Clone)]
pub struct InsertAutonomousNoTradeDiagnosticArgs {
    pub diagnostic_id: Uuid,
    pub observed_at_utc: DateTime<Utc>,
    pub run_id: Option<Uuid>,
    pub mode: String,
    pub session_window_state: String,
    pub runtime_start_allowed: bool,
    pub arm_state: String,
    pub overall_ready: bool,
    pub reason_code: String,
    pub reason: String,
    pub stage: String,
    pub paper_order_attempted: bool,
    pub live_order_attempted: bool,
    pub source: String,
}

/// Persist a single autonomous no-trade diagnostic row.
///
/// Idempotent via `ON CONFLICT (diagnostic_id) DO NOTHING` — a duplicate
/// write attempt for the same logical (reason_code, stage, observing minute)
/// can never produce a second row, bounding row growth under frequent
/// readiness polling. Never writes to, reads from, or otherwise touches
/// `oms_outbox`/`oms_inbox`/`runs`.
pub async fn insert_autonomous_no_trade_diagnostic(
    pool: &PgPool,
    args: &InsertAutonomousNoTradeDiagnosticArgs,
) -> Result<()> {
    sqlx::query(
        r#"
        insert into autonomous_no_trade_diagnostics (
            diagnostic_id, observed_at_utc, run_id, mode,
            session_window_state, runtime_start_allowed, arm_state,
            overall_ready, reason_code, reason, stage,
            paper_order_attempted, live_order_attempted, source
        )
        values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
        on conflict (diagnostic_id) do nothing
        "#,
    )
    .bind(args.diagnostic_id)
    .bind(args.observed_at_utc)
    .bind(args.run_id)
    .bind(&args.mode)
    .bind(&args.session_window_state)
    .bind(args.runtime_start_allowed)
    .bind(&args.arm_state)
    .bind(args.overall_ready)
    .bind(&args.reason_code)
    .bind(&args.reason)
    .bind(&args.stage)
    .bind(args.paper_order_attempted)
    .bind(args.live_order_attempted)
    .bind(&args.source)
    .execute(pool)
    .await
    .context("insert_autonomous_no_trade_diagnostic failed")?;
    Ok(())
}

/// Fetch the most recent `limit` autonomous no-trade diagnostic rows, newest
/// first.
///
/// An empty `Vec` is authoritative: it means no diagnostic has been recorded
/// yet, not that the journal is unavailable.
pub async fn fetch_recent_autonomous_no_trade_diagnostics(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<AutonomousNoTradeDiagnosticRecord>> {
    let rows = sqlx::query(
        r#"
        select diagnostic_id, observed_at_utc, run_id, mode,
               session_window_state, runtime_start_allowed, arm_state,
               overall_ready, reason_code, reason, stage,
               paper_order_attempted, live_order_attempted, source
        from autonomous_no_trade_diagnostics
        order by observed_at_utc desc
        limit $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("fetch_recent_autonomous_no_trade_diagnostics failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(AutonomousNoTradeDiagnosticRecord {
            diagnostic_id: r.try_get("diagnostic_id")?,
            observed_at_utc: r.try_get("observed_at_utc")?,
            run_id: r.try_get("run_id")?,
            mode: r.try_get("mode")?,
            session_window_state: r.try_get("session_window_state")?,
            runtime_start_allowed: r.try_get("runtime_start_allowed")?,
            arm_state: r.try_get("arm_state")?,
            overall_ready: r.try_get("overall_ready")?,
            reason_code: r.try_get("reason_code")?,
            reason: r.try_get("reason")?,
            stage: r.try_get("stage")?,
            paper_order_attempted: r.try_get("paper_order_attempted")?,
            live_order_attempted: r.try_get("live_order_attempted")?,
            source: r.try_get("source")?,
        });
    }
    Ok(out)
}

/// Fetch the most recent `limit` autonomous no-trade diagnostic rows for a
/// single durable run, newest first.
///
/// PAPER-ORDER-LIFECYCLE-VIS-01B: run-scoped sibling of
/// `fetch_recent_autonomous_no_trade_diagnostics`, used to reconstruct one
/// run's lifecycle after a daemon restart. `run_id` is nullable in the
/// source table (a diagnostic can fire off-hours with no active run), so
/// rows with `run_id IS NULL` never match this filter — by design, since
/// they cannot belong to any specific run's lifecycle. An empty `Vec` is
/// authoritative. Read-only — never writes.
pub async fn fetch_autonomous_no_trade_diagnostics_for_run(
    pool: &PgPool,
    run_id: Uuid,
    limit: i64,
) -> Result<Vec<AutonomousNoTradeDiagnosticRecord>> {
    let rows = sqlx::query(
        r#"
        select diagnostic_id, observed_at_utc, run_id, mode,
               session_window_state, runtime_start_allowed, arm_state,
               overall_ready, reason_code, reason, stage,
               paper_order_attempted, live_order_attempted, source
        from autonomous_no_trade_diagnostics
        where run_id = $1
        order by observed_at_utc desc
        limit $2
        "#,
    )
    .bind(run_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("fetch_autonomous_no_trade_diagnostics_for_run failed")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(AutonomousNoTradeDiagnosticRecord {
            diagnostic_id: r.try_get("diagnostic_id")?,
            observed_at_utc: r.try_get("observed_at_utc")?,
            run_id: r.try_get("run_id")?,
            mode: r.try_get("mode")?,
            session_window_state: r.try_get("session_window_state")?,
            runtime_start_allowed: r.try_get("runtime_start_allowed")?,
            arm_state: r.try_get("arm_state")?,
            overall_ready: r.try_get("overall_ready")?,
            reason_code: r.try_get("reason_code")?,
            reason: r.try_get("reason")?,
            stage: r.try_get("stage")?,
            paper_order_attempted: r.try_get("paper_order_attempted")?,
            live_order_attempted: r.try_get("live_order_attempted")?,
            source: r.try_get("source")?,
        });
    }
    Ok(out)
}
