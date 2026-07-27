//! MULTI-STRATEGY-CONFLICT-POLICY-01 Phase C — durable conflict-resolution
//! evidence store.
//!
//! Persists `mqk_portfolio::conflict_policy::ConflictCycleResult` as
//! evidence only: never portfolio, fill, order, promotion, or P&L truth.
//!
//! Written only when the runtime mode is `shadow` or `paper_enforced` — the
//! default `off` mode never calls anything in this module. Idempotent by
//! construction: `plan_id` is the caller-minted deterministic `cycle_id`, so
//! `ON CONFLICT (plan_id) DO NOTHING` (via the same existence-check +
//! rollback-on-duplicate pattern used by
//! `runtime_opportunity_allocation::insert_runtime_opportunity_allocation_plan`)
//! makes re-persisting the same logical cycle a no-op rather than a
//! duplicate or an error.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NewRuntimeStrategyConflictCandidate {
    pub ordinal: i32,
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe_secs: i64,
    /// `"buy"` or `"sell"`.
    pub side: String,
    pub qty: i64,
    pub current_qty: i64,
    pub proposed_target_qty: Option<i64>,
    pub bar_end_ts: Option<i64>,
    pub selected: bool,
    /// One of: `passthrough`, `selected`, `not_selected`, `refused_invalid`,
    /// `refused_conflict`.
    pub disposition: String,
    pub reason_code: String,
}

#[derive(Debug, Clone)]
pub struct NewRuntimeStrategyConflictPlan {
    pub plan_id: Uuid,
    pub cycle_id: Uuid,
    pub run_id: Uuid,
    /// `"shadow"` or `"paper_enforced"` — `"off"` must never be persisted.
    pub mode: String,
    pub market_date: String,
    pub policy_schema_version: String,
    pub symbol_group_count: i32,
    pub candidate_count: i32,
    pub selected_count: i32,
    pub refused_count: i32,
    pub truth_state: String,
    pub blockers: Vec<String>,
    pub created_at_utc: DateTime<Utc>,
    pub candidates: Vec<NewRuntimeStrategyConflictCandidate>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeStrategyConflictPlanRecord {
    pub plan_id: Uuid,
    pub cycle_id: Uuid,
    pub run_id: Uuid,
    pub mode: String,
    pub market_date: String,
    pub policy_schema_version: String,
    pub symbol_group_count: i32,
    pub candidate_count: i32,
    pub selected_count: i32,
    pub refused_count: i32,
    pub truth_state: String,
    pub blockers: Vec<String>,
    pub created_at_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeStrategyConflictCandidateRecord {
    pub plan_id: Uuid,
    pub ordinal: i32,
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe_secs: i64,
    pub side: String,
    pub qty: i64,
    pub current_qty: i64,
    pub proposed_target_qty: Option<i64>,
    pub bar_end_ts: Option<i64>,
    pub selected: bool,
    pub disposition: String,
    pub reason_code: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InsertRuntimeStrategyConflictPlanOutcome {
    Inserted,
    /// `plan_id` already existed — idempotent no-op (re-run of the same
    /// logical cycle, or a crash/restart replay).
    AlreadyExists,
}

/// Persist one conflict plan and its candidates atomically. Only ever
/// called for `mode in {"shadow", "paper_enforced"}` — the default `off`
/// mode must never reach this function.
pub async fn insert_runtime_strategy_conflict_plan(
    pool: &PgPool,
    plan: NewRuntimeStrategyConflictPlan,
) -> Result<InsertRuntimeStrategyConflictPlanOutcome> {
    let mut tx = pool
        .begin()
        .await
        .context("insert_runtime_strategy_conflict_plan: begin failed")?;

    let existing: Option<Uuid> = sqlx::query_scalar(
        "select plan_id from sys_runtime_strategy_conflict_plans where plan_id = $1",
    )
    .bind(plan.plan_id)
    .fetch_optional(&mut *tx)
    .await
    .context("insert_runtime_strategy_conflict_plan: existence check failed")?;

    if existing.is_some() {
        tx.rollback()
            .await
            .context("insert_runtime_strategy_conflict_plan: rollback (read-only path) failed")?;
        return Ok(InsertRuntimeStrategyConflictPlanOutcome::AlreadyExists);
    }

    sqlx::query(
        r#"
        insert into sys_runtime_strategy_conflict_plans
            (plan_id, cycle_id, run_id, mode, market_date, policy_schema_version,
             symbol_group_count, candidate_count, selected_count, refused_count,
             truth_state, blockers, created_at_utc)
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        "#,
    )
    .bind(plan.plan_id)
    .bind(plan.cycle_id)
    .bind(plan.run_id)
    .bind(&plan.mode)
    .bind(&plan.market_date)
    .bind(&plan.policy_schema_version)
    .bind(plan.symbol_group_count)
    .bind(plan.candidate_count)
    .bind(plan.selected_count)
    .bind(plan.refused_count)
    .bind(&plan.truth_state)
    .bind(&plan.blockers)
    .bind(plan.created_at_utc)
    .execute(&mut *tx)
    .await
    .context("insert_runtime_strategy_conflict_plan: insert plan row failed")?;

    for c in &plan.candidates {
        sqlx::query(
            r#"
            insert into sys_runtime_strategy_conflict_candidates
                (plan_id, ordinal, symbol, strategy_id, timeframe_secs, side, qty,
                 current_qty, proposed_target_qty, bar_end_ts, selected, disposition,
                 reason_code)
            values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            "#,
        )
        .bind(plan.plan_id)
        .bind(c.ordinal)
        .bind(&c.symbol)
        .bind(&c.strategy_id)
        .bind(c.timeframe_secs)
        .bind(&c.side)
        .bind(c.qty)
        .bind(c.current_qty)
        .bind(c.proposed_target_qty)
        .bind(c.bar_end_ts)
        .bind(c.selected)
        .bind(&c.disposition)
        .bind(&c.reason_code)
        .execute(&mut *tx)
        .await
        .context("insert_runtime_strategy_conflict_plan: insert candidate row failed")?;
    }

    tx.commit()
        .await
        .context("insert_runtime_strategy_conflict_plan: commit failed")?;

    Ok(InsertRuntimeStrategyConflictPlanOutcome::Inserted)
}

fn plan_row_to_record(row: &sqlx::postgres::PgRow) -> RuntimeStrategyConflictPlanRecord {
    use sqlx::Row;
    RuntimeStrategyConflictPlanRecord {
        plan_id: row.get("plan_id"),
        cycle_id: row.get("cycle_id"),
        run_id: row.get("run_id"),
        mode: row.get("mode"),
        market_date: row.get("market_date"),
        policy_schema_version: row.get("policy_schema_version"),
        symbol_group_count: row.get("symbol_group_count"),
        candidate_count: row.get("candidate_count"),
        selected_count: row.get("selected_count"),
        refused_count: row.get("refused_count"),
        truth_state: row.get("truth_state"),
        blockers: row.get("blockers"),
        created_at_utc: row.get("created_at_utc"),
    }
}

fn candidate_row_to_record(row: &sqlx::postgres::PgRow) -> RuntimeStrategyConflictCandidateRecord {
    use sqlx::Row;
    RuntimeStrategyConflictCandidateRecord {
        plan_id: row.get("plan_id"),
        ordinal: row.get("ordinal"),
        symbol: row.get("symbol"),
        strategy_id: row.get("strategy_id"),
        timeframe_secs: row.get("timeframe_secs"),
        side: row.get("side"),
        qty: row.get("qty"),
        current_qty: row.get("current_qty"),
        proposed_target_qty: row.get("proposed_target_qty"),
        bar_end_ts: row.get("bar_end_ts"),
        selected: row.get("selected"),
        disposition: row.get("disposition"),
        reason_code: row.get("reason_code"),
    }
}

/// Fetch one plan and its candidates (ordered by `ordinal`) by `plan_id`.
pub async fn fetch_runtime_strategy_conflict_plan(
    pool: &PgPool,
    plan_id: Uuid,
) -> Result<
    Option<(
        RuntimeStrategyConflictPlanRecord,
        Vec<RuntimeStrategyConflictCandidateRecord>,
    )>,
> {
    let Some(plan_row) = sqlx::query(
        "select plan_id, cycle_id, run_id, mode, market_date, policy_schema_version, \
         symbol_group_count, candidate_count, selected_count, refused_count, truth_state, \
         blockers, created_at_utc \
         from sys_runtime_strategy_conflict_plans where plan_id = $1",
    )
    .bind(plan_id)
    .fetch_optional(pool)
    .await
    .context("fetch_runtime_strategy_conflict_plan: plan query failed")?
    else {
        return Ok(None);
    };

    let candidate_rows = sqlx::query(
        "select plan_id, ordinal, symbol, strategy_id, timeframe_secs, side, qty, current_qty, \
         proposed_target_qty, bar_end_ts, selected, disposition, reason_code \
         from sys_runtime_strategy_conflict_candidates where plan_id = $1 order by ordinal",
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await
    .context("fetch_runtime_strategy_conflict_plan: candidates query failed")?;

    let candidates = candidate_rows.iter().map(candidate_row_to_record).collect();
    Ok(Some((plan_row_to_record(&plan_row), candidates)))
}

/// Fetch up to `limit` most recent plans for `run_id`, newest first.
pub async fn fetch_recent_runtime_strategy_conflict_plans(
    pool: &PgPool,
    run_id: Uuid,
    limit: i64,
) -> Result<Vec<RuntimeStrategyConflictPlanRecord>> {
    let rows = sqlx::query(
        "select plan_id, cycle_id, run_id, mode, market_date, policy_schema_version, \
         symbol_group_count, candidate_count, selected_count, refused_count, truth_state, \
         blockers, created_at_utc \
         from sys_runtime_strategy_conflict_plans \
         where run_id = $1 order by created_at_utc desc limit $2",
    )
    .bind(run_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("fetch_recent_runtime_strategy_conflict_plans: query failed")?;

    Ok(rows.iter().map(plan_row_to_record).collect())
}
