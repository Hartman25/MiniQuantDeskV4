//! MULTI-STRATEGY-CONFLICT-POLICY-01 Phase C — durable conflict-resolution
//! evidence store.
//!
//! Persists `mqk_portfolio::conflict_policy::ConflictCycleResult` as
//! evidence only: never portfolio, fill, order, promotion, or P&L truth.
//!
//! Written only when the runtime mode is `shadow` or `paper_enforced` — the
//! default `off` mode never calls anything in this module. Idempotent by
//! construction: `plan_id` is the caller-minted deterministic `cycle_id`.
//!
//! # AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 4
//!
//! A `plan_id` match no longer implies idempotent replay by itself. When
//! `plan_id` already exists, [`insert_runtime_strategy_conflict_plan`] now
//! fetches the stored plan and candidates and compares them canonically
//! (independent of candidate insertion order, sensitive to every evidence
//! field) against the incoming payload. An exact match is the intended
//! idempotent no-op (`AlreadyExists`); any divergence is a
//! [`InsertRuntimeStrategyConflictPlanOutcome::PayloadCollision`] — never
//! silently accepted as a replay.

use std::collections::BTreeMap;

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
    /// Order semantics -- part of the cycle's economic identity
    /// (`compute_conflict_cycle_id`), persisted here as durable evidence.
    pub order_type: String,
    pub time_in_force: String,
    pub limit_price: Option<i64>,
    pub proposed_target_qty: Option<i64>,
    /// `true` only when every bar-identity field below is present. An
    /// explicit tri-state field distinct from any individual bar column
    /// being null, so "bar facts entirely absent" and "bar facts present"
    /// are never conflated.
    pub bar_present: bool,
    pub bar_symbol: Option<String>,
    pub bar_strategy_id: Option<String>,
    pub bar_timeframe: Option<String>,
    pub bar_end_ts: Option<i64>,
    pub close_micros: Option<i64>,
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
    /// This is the already live-lock-resolved *effective* mode.
    pub mode: String,
    /// The mode as configured/requested before the live-lock. Distinct
    /// from `mode` (Defect 3/5) so evidence can distinguish "operator
    /// asked for X" from "X is what actually ran."
    pub configured_mode: String,
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
    /// `None` on a 0056-era row written before this column existed --
    /// legacy/incomplete evidence, never fabricated.
    pub configured_mode: Option<String>,
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
    /// `None` on a 0056-era row.
    pub order_type: Option<String>,
    pub time_in_force: Option<String>,
    pub limit_price: Option<i64>,
    pub proposed_target_qty: Option<i64>,
    /// `None` on a 0056-era row -- legacy/unknown, distinct from `Some(false)`.
    pub bar_present: Option<bool>,
    pub bar_symbol: Option<String>,
    pub bar_strategy_id: Option<String>,
    pub bar_timeframe: Option<String>,
    pub bar_end_ts: Option<i64>,
    pub close_micros: Option<i64>,
    pub selected: bool,
    pub disposition: String,
    pub reason_code: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InsertRuntimeStrategyConflictPlanOutcome {
    Inserted,
    /// `plan_id` already existed and the stored payload is canonically
    /// identical to this replay -- idempotent no-op (re-run of the same
    /// logical cycle, or a crash/restart replay).
    AlreadyExists,
    /// `plan_id` already existed but the stored payload diverges from this
    /// replay's payload (AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 4). Never
    /// silently accepted as idempotent and never overwrites the original
    /// row -- fail-closed collision outcome for the caller to log/alert on.
    PayloadCollision { detail: String },
}

/// Canonical, order-independent snapshot of one plan's comparable fields
/// (excludes `created_at_utc`, which is evidence-only wall-clock capture
/// time, not part of the plan's economic identity or truth).
#[derive(Debug, Clone, PartialEq)]
struct PlanSnapshot {
    cycle_id: Uuid,
    run_id: Uuid,
    mode: String,
    configured_mode: Option<String>,
    market_date: String,
    policy_schema_version: String,
    symbol_group_count: i32,
    candidate_count: i32,
    selected_count: i32,
    refused_count: i32,
    truth_state: String,
    blockers: Vec<String>,
}

/// Canonical, order-independent snapshot of one candidate row keyed by
/// `ordinal` (unique within a plan) so two candidate vectors can be
/// compared regardless of caller insertion order while remaining sensitive
/// to every evidence field.
#[derive(Debug, Clone, PartialEq)]
struct CandidateSnapshot {
    symbol: String,
    strategy_id: String,
    timeframe_secs: i64,
    side: String,
    qty: i64,
    current_qty: i64,
    order_type: Option<String>,
    time_in_force: Option<String>,
    limit_price: Option<i64>,
    proposed_target_qty: Option<i64>,
    bar_present: Option<bool>,
    bar_symbol: Option<String>,
    bar_strategy_id: Option<String>,
    bar_timeframe: Option<String>,
    bar_end_ts: Option<i64>,
    close_micros: Option<i64>,
    selected: bool,
    disposition: String,
    reason_code: String,
}

fn new_plan_snapshot(plan: &NewRuntimeStrategyConflictPlan) -> PlanSnapshot {
    PlanSnapshot {
        cycle_id: plan.cycle_id,
        run_id: plan.run_id,
        mode: plan.mode.clone(),
        configured_mode: Some(plan.configured_mode.clone()),
        market_date: plan.market_date.clone(),
        policy_schema_version: plan.policy_schema_version.clone(),
        symbol_group_count: plan.symbol_group_count,
        candidate_count: plan.candidate_count,
        selected_count: plan.selected_count,
        refused_count: plan.refused_count,
        truth_state: plan.truth_state.clone(),
        blockers: {
            let mut b = plan.blockers.clone();
            b.sort();
            b
        },
    }
}

fn stored_plan_snapshot(rec: &RuntimeStrategyConflictPlanRecord) -> PlanSnapshot {
    PlanSnapshot {
        cycle_id: rec.cycle_id,
        run_id: rec.run_id,
        mode: rec.mode.clone(),
        configured_mode: rec.configured_mode.clone(),
        market_date: rec.market_date.clone(),
        policy_schema_version: rec.policy_schema_version.clone(),
        symbol_group_count: rec.symbol_group_count,
        candidate_count: rec.candidate_count,
        selected_count: rec.selected_count,
        refused_count: rec.refused_count,
        truth_state: rec.truth_state.clone(),
        blockers: {
            let mut b = rec.blockers.clone();
            b.sort();
            b
        },
    }
}

fn new_candidate_snapshots(
    candidates: &[NewRuntimeStrategyConflictCandidate],
) -> BTreeMap<i32, CandidateSnapshot> {
    candidates
        .iter()
        .map(|c| {
            (
                c.ordinal,
                CandidateSnapshot {
                    symbol: c.symbol.clone(),
                    strategy_id: c.strategy_id.clone(),
                    timeframe_secs: c.timeframe_secs,
                    side: c.side.clone(),
                    qty: c.qty,
                    current_qty: c.current_qty,
                    order_type: Some(c.order_type.clone()),
                    time_in_force: Some(c.time_in_force.clone()),
                    limit_price: c.limit_price,
                    proposed_target_qty: c.proposed_target_qty,
                    bar_present: Some(c.bar_present),
                    bar_symbol: c.bar_symbol.clone(),
                    bar_strategy_id: c.bar_strategy_id.clone(),
                    bar_timeframe: c.bar_timeframe.clone(),
                    bar_end_ts: c.bar_end_ts,
                    close_micros: c.close_micros,
                    selected: c.selected,
                    disposition: c.disposition.clone(),
                    reason_code: c.reason_code.clone(),
                },
            )
        })
        .collect()
}

fn stored_candidate_snapshots(
    candidates: &[RuntimeStrategyConflictCandidateRecord],
) -> BTreeMap<i32, CandidateSnapshot> {
    candidates
        .iter()
        .map(|c| {
            (
                c.ordinal,
                CandidateSnapshot {
                    symbol: c.symbol.clone(),
                    strategy_id: c.strategy_id.clone(),
                    timeframe_secs: c.timeframe_secs,
                    side: c.side.clone(),
                    qty: c.qty,
                    current_qty: c.current_qty,
                    order_type: c.order_type.clone(),
                    time_in_force: c.time_in_force.clone(),
                    limit_price: c.limit_price,
                    proposed_target_qty: c.proposed_target_qty,
                    bar_present: c.bar_present,
                    bar_symbol: c.bar_symbol.clone(),
                    bar_strategy_id: c.bar_strategy_id.clone(),
                    bar_timeframe: c.bar_timeframe.clone(),
                    bar_end_ts: c.bar_end_ts,
                    close_micros: c.close_micros,
                    selected: c.selected,
                    disposition: c.disposition.clone(),
                    reason_code: c.reason_code.clone(),
                },
            )
        })
        .collect()
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

    let existing_plan_row = sqlx::query(
        "select plan_id, cycle_id, run_id, mode, configured_mode, market_date, \
         policy_schema_version, symbol_group_count, candidate_count, selected_count, \
         refused_count, truth_state, blockers, created_at_utc \
         from sys_runtime_strategy_conflict_plans where plan_id = $1",
    )
    .bind(plan.plan_id)
    .fetch_optional(&mut *tx)
    .await
    .context("insert_runtime_strategy_conflict_plan: existence check failed")?;

    if let Some(row) = existing_plan_row {
        let existing_record = plan_row_to_record(&row);
        let existing_candidate_rows = sqlx::query(
            "select plan_id, ordinal, symbol, strategy_id, timeframe_secs, side, qty, \
             current_qty, order_type, time_in_force, limit_price, proposed_target_qty, \
             bar_present, bar_symbol, bar_strategy_id, bar_timeframe, bar_end_ts, \
             close_micros, selected, disposition, reason_code \
             from sys_runtime_strategy_conflict_candidates where plan_id = $1",
        )
        .bind(plan.plan_id)
        .fetch_all(&mut *tx)
        .await
        .context("insert_runtime_strategy_conflict_plan: existing candidates query failed")?;
        let existing_candidates: Vec<RuntimeStrategyConflictCandidateRecord> =
            existing_candidate_rows
                .iter()
                .map(candidate_row_to_record)
                .collect();

        tx.rollback()
            .await
            .context("insert_runtime_strategy_conflict_plan: rollback (read-only path) failed")?;

        let plans_match = stored_plan_snapshot(&existing_record) == new_plan_snapshot(&plan);
        let candidates_match =
            stored_candidate_snapshots(&existing_candidates) == new_candidate_snapshots(&plan.candidates);

        if plans_match && candidates_match {
            return Ok(InsertRuntimeStrategyConflictPlanOutcome::AlreadyExists);
        }
        return Ok(InsertRuntimeStrategyConflictPlanOutcome::PayloadCollision {
            detail: format!(
                "plan_id {} already exists with a divergent payload (plan fields match: {}, \
                 candidate fields match: {}); never treated as idempotent replay",
                plan.plan_id, plans_match, candidates_match
            ),
        });
    }

    sqlx::query(
        r#"
        insert into sys_runtime_strategy_conflict_plans
            (plan_id, cycle_id, run_id, mode, configured_mode, market_date,
             policy_schema_version, symbol_group_count, candidate_count, selected_count,
             refused_count, truth_state, blockers, created_at_utc)
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        "#,
    )
    .bind(plan.plan_id)
    .bind(plan.cycle_id)
    .bind(plan.run_id)
    .bind(&plan.mode)
    .bind(&plan.configured_mode)
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
                 current_qty, order_type, time_in_force, limit_price, proposed_target_qty,
                 bar_present, bar_symbol, bar_strategy_id, bar_timeframe, bar_end_ts,
                 close_micros, selected, disposition, reason_code)
            values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                    $17, $18, $19, $20, $21)
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
        .bind(&c.order_type)
        .bind(&c.time_in_force)
        .bind(c.limit_price)
        .bind(c.proposed_target_qty)
        .bind(c.bar_present)
        .bind(&c.bar_symbol)
        .bind(&c.bar_strategy_id)
        .bind(&c.bar_timeframe)
        .bind(c.bar_end_ts)
        .bind(c.close_micros)
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
        configured_mode: row.get("configured_mode"),
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
        order_type: row.get("order_type"),
        time_in_force: row.get("time_in_force"),
        limit_price: row.get("limit_price"),
        proposed_target_qty: row.get("proposed_target_qty"),
        bar_present: row.get("bar_present"),
        bar_symbol: row.get("bar_symbol"),
        bar_strategy_id: row.get("bar_strategy_id"),
        bar_timeframe: row.get("bar_timeframe"),
        bar_end_ts: row.get("bar_end_ts"),
        close_micros: row.get("close_micros"),
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
        "select plan_id, cycle_id, run_id, mode, configured_mode, market_date, \
         policy_schema_version, symbol_group_count, candidate_count, selected_count, \
         refused_count, truth_state, blockers, created_at_utc \
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
         order_type, time_in_force, limit_price, proposed_target_qty, bar_present, bar_symbol, \
         bar_strategy_id, bar_timeframe, bar_end_ts, close_micros, selected, disposition, \
         reason_code \
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
        "select plan_id, cycle_id, run_id, mode, configured_mode, market_date, \
         policy_schema_version, symbol_group_count, candidate_count, selected_count, \
         refused_count, truth_state, blockers, created_at_utc \
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
