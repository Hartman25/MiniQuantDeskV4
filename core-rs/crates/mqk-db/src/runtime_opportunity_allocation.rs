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

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Fixed-point scale for scores/weights persisted here (matches the
/// codebase's existing "micros" convention).
pub const RUNTIME_OPPORTUNITY_SCALE: f64 = 1_000_000.0;

pub fn scale_to_micros(value: f64) -> i64 {
    (value * RUNTIME_OPPORTUNITY_SCALE).round() as i64
}

/// CUTOVER-1D-A2: durable quantity evidence encoding written by all new
/// Bundle 5 candidate rows. Historical rows remain `NULL` and use the
/// pre-0076 whole-unit columns.
const RUNTIME_QTY_EVIDENCE_SCHEMA_MICROS_V1: &str = "qty_micros_v1";
const RUNTIME_QTY_MICROS_SCALE: i64 = 1_000_000;

fn whole_qty_to_micros(field: &str, value: i64) -> Result<i64> {
    value.checked_mul(RUNTIME_QTY_MICROS_SCALE).ok_or_else(|| {
        anyhow!(
            "CUTOVER-1D-A2: {} whole-unit quantity {} overflows QtyMicros",
            field,
            value
        )
    })
}

fn micros_to_whole_qty(field: &str, value: i64) -> Result<i64> {
    if value % RUNTIME_QTY_MICROS_SCALE != 0 {
        return Err(anyhow!(
            "CUTOVER-1D-A2: {} contains fractional QtyMicros value {}; the current public record seam is still whole-unit and must fail closed",
            field,
            value
        ));
    }
    Ok(value / RUNTIME_QTY_MICROS_SCALE)
}

fn decode_required_quantity(
    row: &sqlx::postgres::PgRow,
    quantity_schema_version: Option<&str>,
    legacy_field: &str,
    micros_field: &str,
) -> Result<i64> {
    use sqlx::Row;

    let legacy: Option<i64> = row
        .try_get(legacy_field)
        .with_context(|| format!("CUTOVER-1D-A2: read legacy quantity field {legacy_field}"))?;
    let micros: Option<i64> = row
        .try_get(micros_field)
        .with_context(|| format!("CUTOVER-1D-A2: read QtyMicros field {micros_field}"))?;

    match quantity_schema_version {
        None => match (legacy, micros) {
            (Some(value), None) => Ok(value),
            _ => Err(anyhow!(
                "CUTOVER-1D-A2: historical quantity row has mixed/missing authority for {}/{}",
                legacy_field,
                micros_field
            )),
        },
        Some(RUNTIME_QTY_EVIDENCE_SCHEMA_MICROS_V1) => match (legacy, micros) {
            (None, Some(value)) => micros_to_whole_qty(micros_field, value),
            _ => Err(anyhow!(
                "CUTOVER-1D-A2: qty_micros_v1 row has mixed/missing authority for {}/{}",
                legacy_field,
                micros_field
            )),
        },
        Some(other) => Err(anyhow!(
            "CUTOVER-1D-A2: unsupported quantity_schema_version '{other}'"
        )),
    }
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
        let current_qty_micros = whole_qty_to_micros("current_qty", c.current_qty)?;
        let strategy_target_qty_micros =
            whole_qty_to_micros("strategy_target_qty", c.strategy_target_qty)?;
        let allocation_target_qty_micros =
            whole_qty_to_micros("allocation_target_qty", c.allocation_target_qty)?;
        let final_target_qty_micros =
            whole_qty_to_micros("final_target_qty", c.final_target_qty)?;

        sqlx::query(
            r#"
            insert into sys_runtime_opportunity_allocation_candidates
                (plan_id, ordinal, symbol, strategy_id, input_score_micros,
                 target_weight_micros, quantity_schema_version,
                 current_qty_micros, strategy_target_qty_micros,
                 allocation_target_qty_micros, final_target_qty_micros,
                 disposition, reason_code, evaluation_price_micros)
            values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            "#,
        )
        .bind(plan.plan_id)
        .bind(c.ordinal)
        .bind(&c.symbol)
        .bind(&c.strategy_id)
        .bind(c.input_score_micros)
        .bind(c.target_weight_micros)
        .bind(RUNTIME_QTY_EVIDENCE_SCHEMA_MICROS_V1)
        .bind(current_qty_micros)
        .bind(strategy_target_qty_micros)
        .bind(allocation_target_qty_micros)
        .bind(final_target_qty_micros)
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
) -> Result<RuntimeOpportunityAllocationCandidateRecord> {
    use sqlx::Row;

    let quantity_schema_version: Option<String> = row
        .try_get("quantity_schema_version")
        .context("CUTOVER-1D-A2: read opportunity quantity_schema_version")?;

    Ok(RuntimeOpportunityAllocationCandidateRecord {
        plan_id: row.get("plan_id"),
        ordinal: row.get("ordinal"),
        symbol: row.get("symbol"),
        strategy_id: row.get("strategy_id"),
        input_score_micros: row.get("input_score_micros"),
        target_weight_micros: row.get("target_weight_micros"),
        current_qty: decode_required_quantity(
            row,
            quantity_schema_version.as_deref(),
            "current_qty",
            "current_qty_micros",
        )?,
        strategy_target_qty: decode_required_quantity(
            row,
            quantity_schema_version.as_deref(),
            "strategy_target_qty",
            "strategy_target_qty_micros",
        )?,
        allocation_target_qty: decode_required_quantity(
            row,
            quantity_schema_version.as_deref(),
            "allocation_target_qty",
            "allocation_target_qty_micros",
        )?,
        final_target_qty: decode_required_quantity(
            row,
            quantity_schema_version.as_deref(),
            "final_target_qty",
            "final_target_qty_micros",
        )?,
        disposition: row.get("disposition"),
        reason_code: row.get("reason_code"),
        evaluation_price_micros: row.get("evaluation_price_micros"),
    })
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
         quantity_schema_version, current_qty, current_qty_micros, \
         strategy_target_qty, strategy_target_qty_micros, \
         allocation_target_qty, allocation_target_qty_micros, \
         final_target_qty, final_target_qty_micros, disposition, reason_code, \
         evaluation_price_micros \
         from sys_runtime_opportunity_allocation_candidates where plan_id = $1 order by ordinal",
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await
    .context("fetch_runtime_opportunity_allocation_plan: candidates query failed")?;

    let candidates = candidate_rows
        .iter()
        .map(candidate_row_to_record)
        .collect::<Result<Vec<_>>>()
        .context("fetch_runtime_opportunity_allocation_plan: decode candidates failed")?;
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


#[cfg(test)]
mod cutover_1d_a2_tests {
    use super::*;
    use chrono::TimeZone;
    use sqlx::Row;

    async fn cleanup_fixture(pool: &PgPool, run_id: Uuid) {
        let _ = sqlx::query(
            "delete from sys_runtime_opportunity_allocation_candidates where plan_id in \
             (select plan_id from sys_runtime_opportunity_allocation_plans where run_id = $1)",
        )
        .bind(run_id)
        .execute(pool)
        .await;
        let _ =
            sqlx::query("delete from sys_runtime_opportunity_allocation_plans where run_id = $1")
                .bind(run_id)
                .execute(pool)
                .await;
        let _ = sqlx::query("delete from runs where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
    }

    #[test]
    fn cutover_1d_a2_opportunity_checked_scaling_and_fractional_refusal() {
        assert_eq!(whole_qty_to_micros("qty", 2).unwrap(), 2_000_000);
        assert_eq!(whole_qty_to_micros("qty", -3).unwrap(), -3_000_000);
        assert!(whole_qty_to_micros("qty", i64::MAX).is_err());
        assert_eq!(
            micros_to_whole_qty("qty_micros", 4_000_000).unwrap(),
            4
        );
        let err = micros_to_whole_qty("qty_micros", 500_000).unwrap_err();
        assert!(
            err.to_string().contains("fractional QtyMicros"),
            "{err}"
        );
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; CUTOVER-1D-A2 durable writer/read proof"]
    async fn cutover_1d_a2_opportunity_writer_uses_qty_micros_v1_and_fractional_read_fails_closed()
    {
        if std::env::var(crate::ENV_DB_URL).is_err() {
            eprintln!("skipped: requires MQK_DATABASE_URL");
            return;
        }
        let pool = crate::testkit_db_pool()
            .await
            .expect("CUTOVER-1D-A2 opportunity test DB");

        let run_id = Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            b"cutover-1d-a2-opportunity-writer-v1",
        );
        let plan_id = Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            b"cutover-1d-a2-opportunity-plan-v1",
        );

        cleanup_fixture(&pool, run_id).await;

        crate::insert_run(
            &pool,
            &crate::NewRun {
                run_id,
                engine_id: "cutover-1d-a2-opportunity".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: Utc.with_ymd_and_hms(2099, 9, 22, 12, 0, 0).unwrap(),
                git_hash: "cutover-1d-a2".to_string(),
                config_hash: "cutover-1d-a2".to_string(),
                config_json: serde_json::json!({}),
                host_fingerprint: "cutover-1d-a2".to_string(),
            },
        )
        .await
        .expect("fixture run insert");

        let plan = NewRuntimeOpportunityAllocationPlan {
            plan_id,
            cycle_id: plan_id,
            run_id,
            mode: "shadow".to_string(),
            opportunity_artifact_id: "cutover-1d-a2".to_string(),
            source_snapshot_id: None,
            equity_micros: 100_000_000_000,
            candidate_count: 1,
            allowed_count: 1,
            gross_weight_micros: 100_000,
            net_weight_micros: 100_000,
            truth_state: "computed".to_string(),
            blockers: vec![],
            created_at_utc: Utc.with_ymd_and_hms(2099, 9, 22, 12, 1, 0).unwrap(),
            candidates: vec![NewRuntimeOpportunityAllocationCandidate {
                ordinal: 0,
                symbol: "AAPL".to_string(),
                strategy_id: "fixture".to_string(),
                input_score_micros: 900_000,
                target_weight_micros: 100_000,
                current_qty: 1,
                strategy_target_qty: 2,
                allocation_target_qty: 2,
                final_target_qty: 2,
                disposition: "allowed".to_string(),
                reason_code: "fixture".to_string(),
                evaluation_price_micros: 100_000_000,
            }],
        };

        assert_eq!(
            insert_runtime_opportunity_allocation_plan(&pool, plan)
                .await
                .expect("insert QtyMicros-v1 opportunity evidence"),
            InsertRuntimeOpportunityAllocationPlanOutcome::Inserted
        );

        let row = sqlx::query(
            "select quantity_schema_version, current_qty, strategy_target_qty, \
             allocation_target_qty, final_target_qty, current_qty_micros, \
             strategy_target_qty_micros, allocation_target_qty_micros, \
             final_target_qty_micros \
             from sys_runtime_opportunity_allocation_candidates where plan_id = $1",
        )
        .bind(plan_id)
        .fetch_one(&pool)
        .await
        .expect("raw opportunity evidence row");

        assert_eq!(
            row.get::<String, _>("quantity_schema_version"),
            RUNTIME_QTY_EVIDENCE_SCHEMA_MICROS_V1
        );
        assert_eq!(row.get::<Option<i64>, _>("current_qty"), None);
        assert_eq!(row.get::<Option<i64>, _>("strategy_target_qty"), None);
        assert_eq!(row.get::<Option<i64>, _>("allocation_target_qty"), None);
        assert_eq!(row.get::<Option<i64>, _>("final_target_qty"), None);
        assert_eq!(
            row.get::<Option<i64>, _>("current_qty_micros"),
            Some(1_000_000)
        );
        assert_eq!(
            row.get::<Option<i64>, _>("strategy_target_qty_micros"),
            Some(2_000_000)
        );
        assert_eq!(
            row.get::<Option<i64>, _>("allocation_target_qty_micros"),
            Some(2_000_000)
        );
        assert_eq!(
            row.get::<Option<i64>, _>("final_target_qty_micros"),
            Some(2_000_000)
        );

        let (_, records) = fetch_runtime_opportunity_allocation_plan(&pool, plan_id)
            .await
            .expect("fetch QtyMicros-v1 opportunity evidence")
            .expect("plan exists");
        assert_eq!(records[0].current_qty, 1);
        assert_eq!(records[0].strategy_target_qty, 2);
        assert_eq!(records[0].allocation_target_qty, 2);
        assert_eq!(records[0].final_target_qty, 2);

        sqlx::query(
            "update sys_runtime_opportunity_allocation_candidates \
             set current_qty_micros = 500000 where plan_id = $1",
        )
        .bind(plan_id)
        .execute(&pool)
        .await
        .expect("inject valid fractional durable QtyMicros evidence");

        let err = fetch_runtime_opportunity_allocation_plan(&pool, plan_id)
            .await
            .expect_err("whole-unit record seam must reject fractional durable evidence");
        assert!(
            format!("{err:#}").contains("fractional QtyMicros"),
            "{err:#}"
        );

        cleanup_fixture(&pool, run_id).await;
    }
}
