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
//!
//! # FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01 Defect 2
//!
//! The candidate comparison above was still keyed by `ordinal` (a
//! `BTreeMap<i32, CandidateSnapshot>`), and `ordinal` is assigned by
//! `enumerate()` over Bundle 6's incoming batch — so the identical economic
//! candidate set replayed in a different input order received different
//! ordinals and was incorrectly classified as `PayloadCollision`.
//! [`new_candidate_snapshots`] / [`stored_candidate_snapshots`] now exclude
//! `ordinal` from the comparable payload and return a canonically *sorted*
//! `Vec<CandidateSnapshot>` (preserving multiplicity — a genuine duplicate
//! candidate is still distinguishable from a single one) instead of a map
//! keyed by ordinal.
//!
//! # FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01 Defect 6
//!
//! [`fetch_recent_runtime_strategy_conflict_plans`] now orders by
//! `created_at_utc DESC, plan_id DESC` — a tie on `created_at_utc` (two
//! plans persisted within the same wall-clock tick) previously had no
//! deterministic tie-break, so "latest" could return either row
//! nondeterministically across identical reads.
//!
//! # UTF8-AND-BOUNDED-READ-CLOSURE Defect 2
//!
//! [`fetch_runtime_strategy_conflict_plan`] always fetched every candidate
//! row for a plan unconditionally — the daemon's read-side validator only
//! rejected a plan with more than its own candidate bound *after* that
//! unbounded fetch completed, so a pathological/corrupted plan claiming
//! thousands of candidates could still force an unbounded DB read, an
//! unbounded allocation, and an unbounded response body before fail-closed
//! validation ever ran.
//!
//! [`fetch_runtime_strategy_conflict_plan_for_read`] is a separate, bounded
//! read-side fetch seam for status/list/detail evidence projection: it
//! fetches at most [`RUNTIME_STRATEGY_CONFLICT_CANDIDATE_READ_BOUND`] + 1
//! candidate rows via SQL `LIMIT`, which is just enough to prove the durable
//! plan exceeds the bound without ever fetching more. It never projects
//! partial candidate data for an over-limit plan.
//!
//! [`fetch_runtime_strategy_conflict_plan`] (unbounded) remains for trusted
//! internal DB replay/collision comparison (idempotent-replay payload
//! comparison requires the exact, complete stored candidate set) — API
//! routes must use the bounded seam, never this one, for evidence
//! projection.
//!
//! [`RUNTIME_STRATEGY_CONFLICT_CANDIDATE_READ_BOUND`] is the single shared
//! bound authority: the daemon's read-side evidence validator imports this
//! same constant (see `mqk-daemon::conflict_evidence_validation`) instead of
//! declaring its own, so the SQL fetch bound and the validator's bound can
//! never silently drift apart.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use mqk_schemas::QtyMicros;
use sqlx::PgPool;
use uuid::Uuid;

use crate::runtime_qty_evidence::{
    decode_optional_quantity, decode_required_quantity, RUNTIME_QTY_EVIDENCE_SCHEMA_MICROS_V1,
};

/// UTF8-AND-BOUNDED-READ-CLOSURE: the single shared candidate-read bound
/// authority. Consumed by both [`fetch_runtime_strategy_conflict_plan_for_read`]
/// (the SQL `LIMIT`) and the daemon's read-side evidence validator (see
/// `mqk-daemon::conflict_evidence_validation::MAX_CANDIDATES_PER_PLAN`,
/// which is defined in terms of this same constant) so the two bounds can
/// never silently drift apart. Normal plans carry a handful of candidates
/// (bounded by `watchlist_intake::MULTI_SYMBOL_HARD_CEILING`, currently 5,
/// times at most a few competing strategies per symbol); this bound is
/// generous headroom while still refusing an unbounded read for a
/// pathological/corrupted row.
pub const RUNTIME_STRATEGY_CONFLICT_CANDIDATE_READ_BOUND: usize = 64;

#[derive(Debug, Clone)]
pub struct NewRuntimeStrategyConflictCandidate {
    pub ordinal: i32,
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe_secs: i64,
    /// `"buy"` or `"sell"`.
    pub side: String,
    pub qty: QtyMicros,
    pub current_qty: QtyMicros,
    /// Order semantics -- part of the cycle's economic identity
    /// (`compute_conflict_cycle_id`), persisted here as durable evidence.
    pub order_type: String,
    pub time_in_force: String,
    pub limit_price: Option<i64>,
    pub proposed_target_qty: Option<QtyMicros>,
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
    pub qty: QtyMicros,
    pub current_qty: QtyMicros,
    /// `None` on a 0056-era row.
    pub order_type: Option<String>,
    pub time_in_force: Option<String>,
    pub limit_price: Option<i64>,
    pub proposed_target_qty: Option<QtyMicros>,
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
    PayloadCollision {
        detail: String,
    },
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

/// Canonical, order-independent snapshot of one candidate row.
///
/// FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01 Defect 2: this snapshot
/// deliberately excludes `ordinal`. `ordinal` is assigned by `enumerate()`
/// over Bundle 6's incoming batch (see
/// `mqk-daemon::runtime_strategy_conflict::candidate_inputs`) purely to
/// identify *which* input decision a selection refers to for that one
/// resolution call — it is never part of a candidate's economic identity.
/// The exact same economic candidate set replayed in a different input
/// order is assigned different ordinals, but must still compare as the
/// identical set: see [`new_candidate_snapshots`] / [`stored_candidate_snapshots`],
/// which sort snapshots into a canonical order (preserving multiplicity)
/// instead of keying a map by `ordinal`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CandidateSnapshot {
    symbol: String,
    strategy_id: String,
    timeframe_secs: i64,
    side: String,
    qty: QtyMicros,
    current_qty: QtyMicros,
    order_type: Option<String>,
    time_in_force: Option<String>,
    limit_price: Option<i64>,
    proposed_target_qty: Option<QtyMicros>,
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

/// Canonical order: sorted ascending, `ordinal` excluded entirely from the
/// comparable payload. Two candidate vectors describing the identical
/// economic multiset (same values, same multiplicity) always sort to the
/// same `Vec<CandidateSnapshot>` regardless of caller insertion order or
/// which ordinal each was assigned this replay.
fn new_candidate_snapshots(
    candidates: &[NewRuntimeStrategyConflictCandidate],
) -> Vec<CandidateSnapshot> {
    let mut snapshots: Vec<CandidateSnapshot> = candidates
        .iter()
        .map(|c| CandidateSnapshot {
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
        })
        .collect();
    snapshots.sort();
    snapshots
}

fn stored_candidate_snapshots(
    candidates: &[RuntimeStrategyConflictCandidateRecord],
) -> Vec<CandidateSnapshot> {
    let mut snapshots: Vec<CandidateSnapshot> = candidates
        .iter()
        .map(|c| CandidateSnapshot {
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
        })
        .collect();
    snapshots.sort();
    snapshots
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
            "select plan_id, ordinal, symbol, strategy_id, timeframe_secs, side, \
             quantity_schema_version, qty, qty_micros, current_qty, current_qty_micros, \
             order_type, time_in_force, limit_price, proposed_target_qty, \
             proposed_target_qty_micros, bar_present, bar_symbol, bar_strategy_id, \
             bar_timeframe, bar_end_ts, close_micros, selected, disposition, reason_code \
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
                .collect::<Result<Vec<_>>>()
                .context(
                    "insert_runtime_strategy_conflict_plan: decode existing candidates failed",
                )?;

        tx.rollback()
            .await
            .context("insert_runtime_strategy_conflict_plan: rollback (read-only path) failed")?;

        let plans_match = stored_plan_snapshot(&existing_record) == new_plan_snapshot(&plan);
        let candidates_match = stored_candidate_snapshots(&existing_candidates)
            == new_candidate_snapshots(&plan.candidates);

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
                (plan_id, ordinal, symbol, strategy_id, timeframe_secs, side,
                 quantity_schema_version, qty_micros, current_qty_micros,
                 order_type, time_in_force, limit_price, proposed_target_qty_micros,
                 bar_present, bar_symbol, bar_strategy_id, bar_timeframe, bar_end_ts,
                 close_micros, selected, disposition, reason_code)
            values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                    $17, $18, $19, $20, $21, $22)
            "#,
        )
        .bind(plan.plan_id)
        .bind(c.ordinal)
        .bind(&c.symbol)
        .bind(&c.strategy_id)
        .bind(c.timeframe_secs)
        .bind(&c.side)
        .bind(RUNTIME_QTY_EVIDENCE_SCHEMA_MICROS_V1)
        .bind(c.qty.raw())
        .bind(c.current_qty.raw())
        .bind(&c.order_type)
        .bind(&c.time_in_force)
        .bind(c.limit_price)
        .bind(c.proposed_target_qty.map(QtyMicros::raw))
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

fn candidate_row_to_record(
    row: &sqlx::postgres::PgRow,
) -> Result<RuntimeStrategyConflictCandidateRecord> {
    use sqlx::Row;

    let quantity_schema_version: Option<String> = row
        .try_get("quantity_schema_version")
        .context("CUTOVER-1D-A2: read conflict quantity_schema_version")?;

    Ok(RuntimeStrategyConflictCandidateRecord {
        plan_id: row.get("plan_id"),
        ordinal: row.get("ordinal"),
        symbol: row.get("symbol"),
        strategy_id: row.get("strategy_id"),
        timeframe_secs: row.get("timeframe_secs"),
        side: row.get("side"),
        qty: decode_required_quantity(
            row,
            quantity_schema_version.as_deref(),
            "qty",
            "qty_micros",
        )?,
        current_qty: decode_required_quantity(
            row,
            quantity_schema_version.as_deref(),
            "current_qty",
            "current_qty_micros",
        )?,
        order_type: row.get("order_type"),
        time_in_force: row.get("time_in_force"),
        limit_price: row.get("limit_price"),
        proposed_target_qty: decode_optional_quantity(
            row,
            quantity_schema_version.as_deref(),
            "proposed_target_qty",
            "proposed_target_qty_micros",
        )?,
        bar_present: row.get("bar_present"),
        bar_symbol: row.get("bar_symbol"),
        bar_strategy_id: row.get("bar_strategy_id"),
        bar_timeframe: row.get("bar_timeframe"),
        bar_end_ts: row.get("bar_end_ts"),
        close_micros: row.get("close_micros"),
        selected: row.get("selected"),
        disposition: row.get("disposition"),
        reason_code: row.get("reason_code"),
    })
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
        "select plan_id, ordinal, symbol, strategy_id, timeframe_secs, side, \
         quantity_schema_version, qty, qty_micros, current_qty, current_qty_micros, \
         order_type, time_in_force, limit_price, proposed_target_qty, proposed_target_qty_micros, \
         bar_present, bar_symbol, bar_strategy_id, bar_timeframe, bar_end_ts, close_micros, \
         selected, disposition, reason_code \
         from sys_runtime_strategy_conflict_candidates where plan_id = $1 order by ordinal",
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await
    .context("fetch_runtime_strategy_conflict_plan: candidates query failed")?;

    let candidates = candidate_rows
        .iter()
        .map(candidate_row_to_record)
        .collect::<Result<Vec<_>>>()
        .context("fetch_runtime_strategy_conflict_plan: decode candidates failed")?;
    Ok(Some((plan_row_to_record(&plan_row), candidates)))
}

/// UTF8-AND-BOUNDED-READ-CLOSURE Defect 2: outcome of the bounded read-side
/// fetch seam. Explicitly distinguishes a complete in-bound plan, a plan
/// proven to exceed the bound (with no candidate data projected), and a
/// plan that does not exist -- no caller can infer completeness from a
/// silently truncated candidate vector.
#[derive(Debug, Clone, PartialEq)]
pub enum BoundedConflictPlanFetch {
    /// The plan exists and its candidate count is within the bound -- every
    /// candidate row is included, exactly as fetched.
    Complete(
        RuntimeStrategyConflictPlanRecord,
        Vec<RuntimeStrategyConflictCandidateRecord>,
    ),
    /// The plan exists but its candidate count exceeds `bound`. No
    /// candidate data is projected. `observed_at_least` is the number of
    /// rows actually returned by the bounded `LIMIT bound + 1` query
    /// (always exactly `bound + 1`) -- a proven lower bound on the true
    /// count, never the true count itself.
    CandidateLimitExceeded {
        plan: RuntimeStrategyConflictPlanRecord,
        observed_at_least: usize,
    },
    /// No plan exists for this `plan_id`.
    NotFound,
}

/// UTF8-AND-BOUNDED-READ-CLOSURE Defect 2: bounded read-side fetch for
/// status/list/detail evidence projection. Fetches at most `bound + 1`
/// candidate rows via SQL `LIMIT` -- enough to prove the durable plan
/// exceeds `bound`, never more. Distinct from
/// [`fetch_runtime_strategy_conflict_plan`] (unbounded), which remains
/// reserved for trusted internal DB replay/collision comparison where
/// complete-payload comparison is required. API routes must call this
/// function, never the unbounded one, for evidence projection.
pub async fn fetch_runtime_strategy_conflict_plan_for_read(
    pool: &PgPool,
    plan_id: Uuid,
    bound: usize,
) -> Result<BoundedConflictPlanFetch> {
    let Some(plan_row) = sqlx::query(
        "select plan_id, cycle_id, run_id, mode, configured_mode, market_date, \
         policy_schema_version, symbol_group_count, candidate_count, selected_count, \
         refused_count, truth_state, blockers, created_at_utc \
         from sys_runtime_strategy_conflict_plans where plan_id = $1",
    )
    .bind(plan_id)
    .fetch_optional(pool)
    .await
    .context("fetch_runtime_strategy_conflict_plan_for_read: plan query failed")?
    else {
        return Ok(BoundedConflictPlanFetch::NotFound);
    };
    let plan_record = plan_row_to_record(&plan_row);

    let fetch_limit: i64 = i64::try_from(bound.saturating_add(1))
        .context("fetch_runtime_strategy_conflict_plan_for_read: bound does not fit in i64")?;

    let candidate_rows = sqlx::query(
        "select plan_id, ordinal, symbol, strategy_id, timeframe_secs, side, \
         quantity_schema_version, qty, qty_micros, current_qty, current_qty_micros, \
         order_type, time_in_force, limit_price, proposed_target_qty, proposed_target_qty_micros, \
         bar_present, bar_symbol, bar_strategy_id, bar_timeframe, bar_end_ts, close_micros, \
         selected, disposition, reason_code \
         from sys_runtime_strategy_conflict_candidates where plan_id = $1 order by ordinal \
         limit $2",
    )
    .bind(plan_id)
    .bind(fetch_limit)
    .fetch_all(pool)
    .await
    .context("fetch_runtime_strategy_conflict_plan_for_read: candidates query failed")?;

    if candidate_rows.len() > bound {
        return Ok(BoundedConflictPlanFetch::CandidateLimitExceeded {
            plan: plan_record,
            observed_at_least: candidate_rows.len(),
        });
    }

    let candidates = candidate_rows
        .iter()
        .map(candidate_row_to_record)
        .collect::<Result<Vec<_>>>()
        .context("fetch_runtime_strategy_conflict_plan_for_read: decode candidates failed")?;
    Ok(BoundedConflictPlanFetch::Complete(plan_record, candidates))
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
         where run_id = $1 order by created_at_utc desc, plan_id desc limit $2",
    )
    .bind(run_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("fetch_recent_runtime_strategy_conflict_plans: query failed")?;

    Ok(rows.iter().map(plan_row_to_record).collect())
}

#[cfg(test)]
mod cutover_1d_a3_tests {
    use super::*;
    use chrono::TimeZone;
    use sqlx::Row;

    const FIXTURE_RUN: &[u8] = b"cutover-1d-a3-conflict-run-v1";
    const FIXTURE_PLAN: &[u8] = b"cutover-1d-a3-conflict-plan-v1";

    async fn cleanup_fixture(pool: &PgPool, run_id: Uuid) {
        let _ = sqlx::query(
            "delete from sys_runtime_strategy_conflict_candidates where plan_id in \
             (select plan_id from sys_runtime_strategy_conflict_plans where run_id = $1)",
        )
        .bind(run_id)
        .execute(pool)
        .await;
        let _ = sqlx::query("delete from sys_runtime_strategy_conflict_plans where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("delete from runs where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
    }

    fn candidate(
        qty_raw: i64,
        current_raw: i64,
        proposed_raw: i64,
    ) -> NewRuntimeStrategyConflictCandidate {
        NewRuntimeStrategyConflictCandidate {
            ordinal: 0,
            symbol: "BTC/USD".to_string(),
            strategy_id: "fixture".to_string(),
            timeframe_secs: 300,
            side: "buy".to_string(),
            qty: QtyMicros::new(qty_raw),
            current_qty: QtyMicros::new(current_raw),
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
            proposed_target_qty: Some(QtyMicros::new(proposed_raw)),
            bar_present: true,
            bar_symbol: Some("BTC/USD".to_string()),
            bar_strategy_id: Some("fixture".to_string()),
            bar_timeframe: Some("5m".to_string()),
            bar_end_ts: Some(1_000),
            close_micros: Some(100_000_000),
            selected: true,
            disposition: "selected".to_string(),
            reason_code: "fixture".to_string(),
        }
    }

    fn plan(
        run_id: Uuid,
        plan_id: Uuid,
        c: NewRuntimeStrategyConflictCandidate,
    ) -> NewRuntimeStrategyConflictPlan {
        NewRuntimeStrategyConflictPlan {
            plan_id,
            cycle_id: plan_id,
            run_id,
            mode: "shadow".to_string(),
            configured_mode: "shadow".to_string(),
            market_date: "2099-09-22".to_string(),
            policy_schema_version: "multi-strategy-conflict-policy-v1".to_string(),
            symbol_group_count: 1,
            candidate_count: 1,
            selected_count: 1,
            refused_count: 0,
            truth_state: "computed".to_string(),
            blockers: vec![],
            created_at_utc: Utc.with_ymd_and_hms(2099, 9, 22, 12, 1, 0).unwrap(),
            candidates: vec![c],
        }
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; CUTOVER-1D-A3 durable QtyMicros proof"]
    async fn cutover_1d_a3_conflict_evidence_is_natively_qty_micros() {
        if std::env::var(crate::ENV_DB_URL).is_err() {
            eprintln!("skipped: requires MQK_DATABASE_URL");
            return;
        }
        let pool = crate::testkit_db_pool()
            .await
            .expect("CUTOVER-1D-A3 conflict test DB");
        let run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, FIXTURE_RUN);
        let plan_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, FIXTURE_PLAN);
        cleanup_fixture(&pool, run_id).await;
        crate::insert_run(
            &pool,
            &crate::NewRun {
                run_id,
                engine_id: "cutover-1d-a3-conflict".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: Utc.with_ymd_and_hms(2099, 9, 22, 12, 0, 0).unwrap(),
                git_hash: "cutover-1d-a3".to_string(),
                config_hash: "cutover-1d-a3".to_string(),
                config_json: serde_json::json!({}),
                host_fingerprint: "cutover-1d-a3".to_string(),
            },
        )
        .await
        .expect("fixture run insert");

        // 0.0001 / 0.00025 / 0.00035: raw micros are bound verbatim, never
        // whole * 1e6.
        assert_eq!(
            insert_runtime_strategy_conflict_plan(
                &pool,
                plan(run_id, plan_id, candidate(100, 250, 350))
            )
            .await
            .expect("insert fractional QtyMicros-v1 conflict evidence"),
            InsertRuntimeStrategyConflictPlanOutcome::Inserted
        );
        let row = sqlx::query(
            "select quantity_schema_version, qty, current_qty, proposed_target_qty, \
             qty_micros, current_qty_micros, proposed_target_qty_micros \
             from sys_runtime_strategy_conflict_candidates where plan_id = $1",
        )
        .bind(plan_id)
        .fetch_one(&pool)
        .await
        .expect("raw conflict evidence row");
        assert_eq!(
            row.get::<String, _>("quantity_schema_version"),
            RUNTIME_QTY_EVIDENCE_SCHEMA_MICROS_V1
        );
        assert_eq!(row.get::<Option<i64>, _>("qty"), None);
        assert_eq!(row.get::<Option<i64>, _>("current_qty"), None);
        assert_eq!(row.get::<Option<i64>, _>("proposed_target_qty"), None);
        assert_eq!(row.get::<Option<i64>, _>("qty_micros"), Some(100));
        assert_eq!(row.get::<Option<i64>, _>("current_qty_micros"), Some(250));
        assert_eq!(
            row.get::<Option<i64>, _>("proposed_target_qty_micros"),
            Some(350)
        );

        // Exact fractional read.
        let (_, records) = fetch_runtime_strategy_conflict_plan(&pool, plan_id)
            .await
            .expect("fetch")
            .expect("plan exists");
        assert_eq!(records[0].qty, QtyMicros::new(100));
        assert_eq!(records[0].current_qty, QtyMicros::new(250));
        assert_eq!(records[0].proposed_target_qty, Some(QtyMicros::new(350)));

        // Idempotent replay of the identical fractional payload.
        assert_eq!(
            insert_runtime_strategy_conflict_plan(
                &pool,
                plan(run_id, plan_id, candidate(100, 250, 350))
            )
            .await
            .expect("replay"),
            InsertRuntimeStrategyConflictPlanOutcome::AlreadyExists
        );
        // Each quantity field diverging by exactly ONE micro, in isolation,
        // collides (a comparison that truncated or rounded any of them would
        // call these an idempotent replay).
        for (label, divergent) in [
            ("qty", candidate(101, 250, 350)),
            ("current_qty", candidate(100, 251, 350)),
            ("proposed_target_qty", candidate(100, 250, 351)),
        ] {
            let outcome =
                insert_runtime_strategy_conflict_plan(&pool, plan(run_id, plan_id, divergent))
                    .await
                    .expect("divergent replay");
            assert!(
                matches!(
                    outcome,
                    InsertRuntimeStrategyConflictPlanOutcome::PayloadCollision { .. }
                ),
                "{label}: {outcome:?}"
            );
        }

        // Historical NULL-schema row decodes through the checked whole-unit
        // conversion (2 whole units == 2_000_000 micros), never reinterpreted.
        sqlx::query(
            "insert into sys_runtime_strategy_conflict_candidates \
             (plan_id, ordinal, symbol, strategy_id, timeframe_secs, side, qty, current_qty, \
              proposed_target_qty, selected, disposition, reason_code) \
             values ($1, 1, 'AAPL', 'fixture', 300, 'buy', 2, 1, 3, false, 'not_selected', 'fixture')",
        )
        .bind(plan_id)
        .execute(&pool)
        .await
        .expect("insert historical whole-unit candidate row");
        let (_, records) = fetch_runtime_strategy_conflict_plan(&pool, plan_id)
            .await
            .expect("fetch mixed-epoch plan")
            .expect("plan exists");
        let hist = records.iter().find(|r| r.ordinal == 1).unwrap();
        assert_eq!(hist.qty, QtyMicros::new(2_000_000));
        assert_eq!(hist.current_qty, QtyMicros::new(1_000_000));
        assert_eq!(hist.proposed_target_qty, Some(QtyMicros::new(3_000_000)));

        cleanup_fixture(&pool, run_id).await;
    }

    #[tokio::test]
    #[ignore = "requires MQK_DATABASE_URL; CUTOVER-1D-A3 decode refusal proof"]
    async fn cutover_1d_a3_mixed_or_unknown_quantity_encoding_refuses() {
        if std::env::var(crate::ENV_DB_URL).is_err() {
            eprintln!("skipped: requires MQK_DATABASE_URL");
            return;
        }
        let pool = crate::testkit_db_pool().await.expect("test DB");
        // Rows synthesized from literals: the table CHECK forbids these
        // shapes at rest, so the decoder is proven directly.
        let decode = |schema: Option<&'static str>, legacy: Option<i64>, micros: Option<i64>| {
            let pool = pool.clone();
            async move {
                let row = sqlx::query("select $1::bigint as qty, $2::bigint as qty_micros")
                    .bind(legacy)
                    .bind(micros)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                decode_required_quantity(&row, schema, "qty", "qty_micros")
            }
        };
        // Well-formed shapes.
        assert_eq!(
            decode(None, Some(2), None).await.unwrap(),
            QtyMicros::new(2_000_000)
        );
        assert_eq!(
            decode(Some("qty_micros_v1"), None, Some(100))
                .await
                .unwrap(),
            QtyMicros::new(100)
        );
        // Mixed / missing authority / unknown encoding refuse.
        assert!(decode(None, Some(2), Some(2_000_000)).await.is_err());
        assert!(decode(None, None, None).await.is_err());
        assert!(decode(Some("qty_micros_v1"), Some(2), Some(2_000_000))
            .await
            .is_err());
        assert!(decode(Some("qty_micros_v1"), None, None).await.is_err());
        assert!(decode(Some("qty_micros_v2"), None, Some(1)).await.is_err());
        // Historical value that cannot be scaled refuses.
        assert!(decode(None, Some(i64::MAX), None).await.is_err());
    }
}
