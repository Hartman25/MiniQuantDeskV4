//! RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase G — durable allocation-cycle
//! evidence store.
//!
//! Persists `mqk_portfolio::AllocationCycleResult` (Phase D) as evidence
//! only: never portfolio, fill, order, or P&L truth, and never a second NAV
//! source (equity_micros / source_snapshot_id here are copies of the
//! already-durable `sys_paper_portfolio_snapshots` row a cycle read).
//!
//! Written only when the runtime mode is `shadow` or `paper_enforced` — the
//! default `off` mode never calls anything in this module. Idempotent by
//! construction: `plan_id` is the caller-minted deterministic `cycle_id`, so
//! `ON CONFLICT (plan_id) DO NOTHING` makes re-persisting the same logical
//! cycle a no-op rather than a duplicate or an error.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Fixed-point scale for scores/weights persisted here (matches the
/// codebase's existing "micros" convention).
pub const RUNTIME_OPPORTUNITY_SCALE: f64 = 1_000_000.0;

pub fn scale_to_micros(value: f64) -> i64 {
    (value * RUNTIME_OPPORTUNITY_SCALE).round() as i64
}

#[derive(Debug, Clone)]
pub struct NewRuntimeOpportunityAllocationCandidate {
    pub ordinal: i32,
    pub symbol: String,
    pub strategy_id: String,
    pub input_score_micros: i64,
    pub target_weight_micros: i64,
    pub current_qty: i64,
    pub strategy_target_qty: i64,
    pub allocation_target_qty: i64,
    pub final_target_qty: i64,
    /// One of: `allowed`, `clamped_down`, `refused_no_capital`,
    /// `refused_fail_closed`.
    pub disposition: String,
    pub reason_code: String,
    pub evaluation_price_micros: i64,
}

#[derive(Debug, Clone)]
pub struct NewRuntimeOpportunityAllocationPlan {
    pub plan_id: Uuid,
    pub cycle_id: Uuid,
    pub run_id: Uuid,
    /// `"shadow"` or `"paper_enforced"` — `"off"` must never be persisted.
    pub mode: String,
    pub opportunity_artifact_id: String,
    pub source_snapshot_id: Option<Uuid>,
    pub equity_micros: i64,
    pub candidate_count: i32,
    pub allowed_count: i32,
    pub gross_weight_micros: i64,
    pub net_weight_micros: i64,
    pub truth_state: String,
    pub blockers: Vec<String>,
    pub created_at_utc: DateTime<Utc>,
    pub candidates: Vec<NewRuntimeOpportunityAllocationCandidate>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeOpportunityAllocationPlanRecord {
    pub plan_id: Uuid,
    pub cycle_id: Uuid,
    pub run_id: Uuid,
    pub mode: String,
    pub opportunity_artifact_id: String,
    pub source_snapshot_id: Option<Uuid>,
    pub equity_micros: i64,
    pub candidate_count: i32,
    pub allowed_count: i32,
    pub gross_weight_micros: i64,
    pub net_weight_micros: i64,
    pub truth_state: String,
    pub blockers: Vec<String>,
    pub created_at_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeOpportunityAllocationCandidateRecord {
    pub plan_id: Uuid,
    pub ordinal: i32,
    pub symbol: String,
    pub strategy_id: String,
    pub input_score_micros: i64,
    pub target_weight_micros: i64,
    pub current_qty: i64,
    pub strategy_target_qty: i64,
    pub allocation_target_qty: i64,
    pub final_target_qty: i64,
    pub disposition: String,
    pub reason_code: String,
    pub evaluation_price_micros: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InsertRuntimeOpportunityAllocationPlanOutcome {
    Inserted,
    /// `plan_id` already existed — idempotent no-op (re-run of the same
    /// logical cycle, or a crash/restart replay).
    AlreadyExists,
}

/// Persist one allocation plan and its candidates atomically. Only ever
/// called for `mode in {"shadow", "paper_enforced"}` — the default `off`
/// mode must never reach this function.
pub async fn insert_runtime_opportunity_allocation_plan(
    pool: &PgPool,
    plan: NewRuntimeOpportunityAllocationPlan,
) -> Result<InsertRuntimeOpportunityAllocationPlanOutcome> {
    let mut tx = pool
        .begin()
        .await
        .context("insert_runtime_opportunity_allocation_plan: begin failed")?;

    let existing: Option<Uuid> = sqlx::query_scalar(
        "select plan_id from sys_runtime_opportunity_allocation_plans where plan_id = $1",
    )
    .bind(plan.plan_id)
    .fetch_optional(&mut *tx)
    .await
    .context("insert_runtime_opportunity_allocation_plan: existence check failed")?;

    if existing.is_some() {
        tx.rollback().await.context(
            "insert_runtime_opportunity_allocation_plan: rollback (read-only path) failed",
        )?;
        return Ok(InsertRuntimeOpportunityAllocationPlanOutcome::AlreadyExists);
    }

    sqlx::query(
        r#"
        insert into sys_runtime_opportunity_allocation_plans
            (plan_id, cycle_id, run_id, mode, opportunity_artifact_id,
             source_snapshot_id, equity_micros, candidate_count, allowed_count,
             gross_weight_micros, net_weight_micros, truth_state, blockers, created_at_utc)
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        "#,
    )
    .bind(plan.plan_id)
    .bind(plan.cycle_id)
    .bind(plan.run_id)
    .bind(&plan.mode)
    .bind(&plan.opportunity_artifact_id)
    .bind(plan.source_snapshot_id)
    .bind(plan.equity_micros)
    .bind(plan.candidate_count)
    .bind(plan.allowed_count)
    .bind(plan.gross_weight_micros)
    .bind(plan.net_weight_micros)
    .bind(&plan.truth_state)
    .bind(&plan.blockers)
    .bind(plan.created_at_utc)
    .execute(&mut *tx)
    .await
    .context("insert_runtime_opportunity_allocation_plan: insert plan row failed")?;

    for c in &plan.candidates {
        sqlx::query(
            r#"
            insert into sys_runtime_opportunity_allocation_candidates
                (plan_id, ordinal, symbol, strategy_id, input_score_micros,
                 target_weight_micros, current_qty, strategy_target_qty,
                 allocation_target_qty, final_target_qty, disposition, reason_code,
                 evaluation_price_micros)
            values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            "#,
        )
        .bind(plan.plan_id)
        .bind(c.ordinal)
        .bind(&c.symbol)
        .bind(&c.strategy_id)
        .bind(c.input_score_micros)
        .bind(c.target_weight_micros)
        .bind(c.current_qty)
        .bind(c.strategy_target_qty)
        .bind(c.allocation_target_qty)
        .bind(c.final_target_qty)
        .bind(&c.disposition)
        .bind(&c.reason_code)
        .bind(c.evaluation_price_micros)
        .execute(&mut *tx)
        .await
        .context("insert_runtime_opportunity_allocation_plan: insert candidate row failed")?;
    }

    tx.commit()
        .await
        .context("insert_runtime_opportunity_allocation_plan: commit failed")?;

    Ok(InsertRuntimeOpportunityAllocationPlanOutcome::Inserted)
}

fn plan_row_to_record(row: &sqlx::postgres::PgRow) -> RuntimeOpportunityAllocationPlanRecord {
    use sqlx::Row;
    RuntimeOpportunityAllocationPlanRecord {
        plan_id: row.get("plan_id"),
        cycle_id: row.get("cycle_id"),
        run_id: row.get("run_id"),
        mode: row.get("mode"),
        opportunity_artifact_id: row.get("opportunity_artifact_id"),
        source_snapshot_id: row.get("source_snapshot_id"),
        equity_micros: row.get("equity_micros"),
        candidate_count: row.get("candidate_count"),
        allowed_count: row.get("allowed_count"),
        gross_weight_micros: row.get("gross_weight_micros"),
        net_weight_micros: row.get("net_weight_micros"),
        truth_state: row.get("truth_state"),
        blockers: row.get("blockers"),
        created_at_utc: row.get("created_at_utc"),
    }
}

fn candidate_row_to_record(
    row: &sqlx::postgres::PgRow,
) -> RuntimeOpportunityAllocationCandidateRecord {
    use sqlx::Row;
    RuntimeOpportunityAllocationCandidateRecord {
        plan_id: row.get("plan_id"),
        ordinal: row.get("ordinal"),
        symbol: row.get("symbol"),
        strategy_id: row.get("strategy_id"),
        input_score_micros: row.get("input_score_micros"),
        target_weight_micros: row.get("target_weight_micros"),
        current_qty: row.get("current_qty"),
        strategy_target_qty: row.get("strategy_target_qty"),
        allocation_target_qty: row.get("allocation_target_qty"),
        final_target_qty: row.get("final_target_qty"),
        disposition: row.get("disposition"),
        reason_code: row.get("reason_code"),
        evaluation_price_micros: row.get("evaluation_price_micros"),
    }
}

/// Fetch one plan and its candidates (ordered by `ordinal`) by `plan_id`.
pub async fn fetch_runtime_opportunity_allocation_plan(
    pool: &PgPool,
    plan_id: Uuid,
) -> Result<
    Option<(
        RuntimeOpportunityAllocationPlanRecord,
        Vec<RuntimeOpportunityAllocationCandidateRecord>,
    )>,
> {
    let Some(plan_row) = sqlx::query(
        "select plan_id, cycle_id, run_id, mode, opportunity_artifact_id, source_snapshot_id, \
         equity_micros, candidate_count, allowed_count, gross_weight_micros, net_weight_micros, \
         truth_state, blockers, created_at_utc \
         from sys_runtime_opportunity_allocation_plans where plan_id = $1",
    )
    .bind(plan_id)
    .fetch_optional(pool)
    .await
    .context("fetch_runtime_opportunity_allocation_plan: plan query failed")?
    else {
        return Ok(None);
    };

    let candidate_rows = sqlx::query(
        "select plan_id, ordinal, symbol, strategy_id, input_score_micros, target_weight_micros, \
         current_qty, strategy_target_qty, allocation_target_qty, final_target_qty, disposition, \
         reason_code, evaluation_price_micros \
         from sys_runtime_opportunity_allocation_candidates where plan_id = $1 order by ordinal",
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await
    .context("fetch_runtime_opportunity_allocation_plan: candidates query failed")?;

    let candidates = candidate_rows.iter().map(candidate_row_to_record).collect();
    Ok(Some((plan_row_to_record(&plan_row), candidates)))
}

/// Fetch up to `limit` most recent plans for `run_id`, newest first.
pub async fn fetch_recent_runtime_opportunity_allocation_plans(
    pool: &PgPool,
    run_id: Uuid,
    limit: i64,
) -> Result<Vec<RuntimeOpportunityAllocationPlanRecord>> {
    let rows = sqlx::query(
        "select plan_id, cycle_id, run_id, mode, opportunity_artifact_id, source_snapshot_id, \
         equity_micros, candidate_count, allowed_count, gross_weight_micros, net_weight_micros, \
         truth_state, blockers, created_at_utc \
         from sys_runtime_opportunity_allocation_plans \
         where run_id = $1 order by created_at_utc desc limit $2",
    )
    .bind(run_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("fetch_recent_runtime_opportunity_allocation_plans: query failed")?;

    Ok(rows.iter().map(plan_row_to_record).collect())
}
