//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 7C Part 1 — durable
//! dynamic-selection plan evidence store.
//!
//! Persists `mqk_portfolio::dynamic_selection::DynamicSelectionPlan` as
//! evidence only: never portfolio, fill, order, promotion, or P&L truth.
//!
//! Written only when the effective mode is `shadow` or `paper_enforced` --
//! the default `off` mode never calls anything in this module.
//! `approved_for_live` is constrained `false` at the DB level (see
//! migration 0059) in addition to always being constructed `false` here.
//!
//! Idempotent by construction: `plan_id` is the caller-minted deterministic
//! UUIDv5 derived from `canonical_plan_identity_material`. When `plan_id`
//! already exists, [`insert_dynamic_selection_plan`] fetches the stored
//! header/symbols/candidates and compares them canonically (independent of
//! insertion order) against the incoming payload. An exact match is the
//! intended idempotent no-op (`AlreadyExists`); any divergence is a
//! [`InsertDynamicSelectionPlanOutcome::PayloadCollision`] -- never silently
//! accepted as a replay, never overwrites the original row.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Single shared candidate-read bound authority, mirroring the conflict-
/// evidence store's `RUNTIME_STRATEGY_CONFLICT_CANDIDATE_READ_BOUND`. A
/// legitimate plan is bounded by
/// `mqk_portfolio::dynamic_selection::MAX_CANDIDATE_PAIRS` (25 today); this
/// bound is generous headroom while still refusing an unbounded read for a
/// pathological/corrupted row.
pub const DYNAMIC_SELECTION_CANDIDATE_READ_BOUND: usize = 256;

#[derive(Debug, Clone)]
pub struct NewDynamicSelectionPlanSymbol {
    pub symbol: String,
    pub selected_strategy_id: Option<String>,
    /// `"selected"`, `"not_selected"`, or `"refused"`.
    pub disposition: String,
    pub reason_code: String,
    pub exact_reason_code: String,
}

#[derive(Debug, Clone)]
pub struct NewDynamicSelectionPlanCandidate {
    pub ordinal: i32,
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe_secs: i64,
    pub canonical_score_decimal: Option<String>,
    pub canonical_score_micros: Option<i64>,
    pub scanner_rank: Option<i32>,
    pub watchlist_assigned: bool,
    pub promotion_query_ok: bool,
    pub promotion_state: Option<String>,
    pub promotion_effective: bool,
    pub promotion_expired: bool,
    pub evidence_resolved: bool,
    pub review_state_is_paper_candidate: bool,
    pub evidence_review_state: Option<String>,
    pub durable_legacy_fingerprint: Option<String>,
    pub recomputed_legacy_fingerprint: Option<String>,
    pub legacy_fingerprint_matches: bool,
    pub durable_exact_fingerprint_v2: Option<String>,
    pub recomputed_exact_fingerprint_v2: Option<String>,
    pub exact_fingerprint_v2_matches: bool,
    pub registry_enabled: bool,
    pub plugin_instantiable: bool,
    pub timeframe_matches: bool,
    pub data_ready: bool,
    pub evidence_review_id: Option<String>,
    pub evidence_scanner_scan_id: Option<String>,
    pub evidence_artifact_path: Option<String>,
    pub evidence_git_hash: Option<String>,
    pub promotion_transition_id: Option<String>,
    pub promotion_effective_at: Option<String>,
    pub promotion_expires_at: Option<String>,
    pub evidence_transition_id: Option<String>,
    pub exact_reason_code: String,
    pub selected: bool,
    /// `"selected"`, `"not_selected"`, or `"refused"`.
    pub disposition: String,
    pub reason_code: String,
}

#[derive(Debug, Clone)]
pub struct NewDynamicSelectionPlan {
    pub plan_id: Uuid,
    pub run_id: Uuid,
    pub library_schema_version: String,
    pub context_schema_version: String,
    /// `"off"`, `"shadow"`, or `"paper_enforced"` -- the requested mode
    /// before the live-lock. `"off"` must never be persisted (the daemon
    /// never calls this module for `Off`).
    pub configured_mode: String,
    /// The already live-lock-resolved effective mode. `"off"` must never be
    /// persisted here either.
    pub effective_mode: String,
    pub live_lock_applied: bool,
    /// Always `false` -- constrained at the DB level too (migration 0059).
    pub approved_for_live: bool,
    pub source_kind: String,
    pub source_identity: String,
    pub market_date: String,
    /// `"shadow_allowed"`, `"shadow_invalid"`, `"paper_enforced_allowed"`,
    /// or `"paper_enforced_refused"` -- the exact
    /// `DynamicSelectionStartGateDisposition` this plan was resolved under.
    /// Distinct from `effective_mode`: a refused start still carries
    /// `effective_mode = "paper_enforced"`, so this column is what
    /// distinguishes an allowed header from a refused one for the same
    /// mode. `"off"` is never a valid value here -- Off never writes a row.
    pub disposition: String,
    pub truth_state: String,
    pub blockers: Vec<String>,
    pub symbol_count: i32,
    pub candidate_count: i32,
    pub selected_count: i32,
    pub refused_count: i32,
    pub writer_version: String,
    pub created_at_utc: DateTime<Utc>,
    pub symbols: Vec<NewDynamicSelectionPlanSymbol>,
    pub candidates: Vec<NewDynamicSelectionPlanCandidate>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DynamicSelectionPlanRecord {
    pub plan_id: Uuid,
    pub run_id: Uuid,
    pub library_schema_version: String,
    pub context_schema_version: String,
    pub configured_mode: String,
    pub effective_mode: String,
    pub live_lock_applied: bool,
    pub approved_for_live: bool,
    pub source_kind: String,
    pub source_identity: String,
    pub market_date: String,
    /// `"shadow_allowed"`, `"shadow_invalid"`, `"paper_enforced_allowed"`,
    /// or `"paper_enforced_refused"` -- the exact
    /// `DynamicSelectionStartGateDisposition` this plan was resolved under.
    /// Distinct from `effective_mode`: a refused start still carries
    /// `effective_mode = "paper_enforced"`, so this column is what
    /// distinguishes an allowed header from a refused one for the same
    /// mode. `"off"` is never a valid value here -- Off never writes a row.
    pub disposition: String,
    pub truth_state: String,
    pub blockers: Vec<String>,
    pub symbol_count: i32,
    pub candidate_count: i32,
    pub selected_count: i32,
    pub refused_count: i32,
    pub writer_version: String,
    pub created_at_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DynamicSelectionPlanSymbolRecord {
    pub plan_id: Uuid,
    pub symbol: String,
    pub selected_strategy_id: Option<String>,
    pub disposition: String,
    pub reason_code: String,
    pub exact_reason_code: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DynamicSelectionPlanCandidateRecord {
    pub plan_id: Uuid,
    pub ordinal: i32,
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe_secs: i64,
    pub canonical_score_decimal: Option<String>,
    pub canonical_score_micros: Option<i64>,
    pub scanner_rank: Option<i32>,
    pub watchlist_assigned: bool,
    pub promotion_query_ok: bool,
    pub promotion_state: Option<String>,
    pub promotion_effective: bool,
    pub promotion_expired: bool,
    pub evidence_resolved: bool,
    pub review_state_is_paper_candidate: bool,
    pub evidence_review_state: Option<String>,
    pub durable_legacy_fingerprint: Option<String>,
    pub recomputed_legacy_fingerprint: Option<String>,
    pub legacy_fingerprint_matches: bool,
    pub durable_exact_fingerprint_v2: Option<String>,
    pub recomputed_exact_fingerprint_v2: Option<String>,
    pub exact_fingerprint_v2_matches: bool,
    pub registry_enabled: bool,
    pub plugin_instantiable: bool,
    pub timeframe_matches: bool,
    pub data_ready: bool,
    pub evidence_review_id: Option<String>,
    pub evidence_scanner_scan_id: Option<String>,
    pub evidence_artifact_path: Option<String>,
    pub evidence_git_hash: Option<String>,
    pub promotion_transition_id: Option<String>,
    pub promotion_effective_at: Option<String>,
    pub promotion_expires_at: Option<String>,
    pub evidence_transition_id: Option<String>,
    pub exact_reason_code: String,
    pub selected: bool,
    pub disposition: String,
    pub reason_code: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InsertDynamicSelectionPlanOutcome {
    Inserted,
    /// `plan_id` already existed and the stored payload is canonically
    /// identical to this replay -- idempotent no-op (re-run of the same
    /// logical plan, or a crash/restart replay).
    AlreadyExists,
    /// `plan_id` already existed but the stored payload diverges from this
    /// replay's payload. Never silently accepted as idempotent and never
    /// overwrites the original row -- fail-closed collision outcome for the
    /// caller to refuse start on.
    PayloadCollision { detail: String },
}

/// Canonical, order-independent snapshot of the plan header's comparable
/// fields (excludes `created_at_utc`, which is evidence-only wall-clock
/// capture time, not part of the plan's economic identity).
#[derive(Debug, Clone, PartialEq)]
struct PlanSnapshot {
    run_id: Uuid,
    library_schema_version: String,
    context_schema_version: String,
    configured_mode: String,
    effective_mode: String,
    live_lock_applied: bool,
    approved_for_live: bool,
    source_kind: String,
    source_identity: String,
    market_date: String,
    disposition: String,
    truth_state: String,
    blockers: Vec<String>,
    symbol_count: i32,
    candidate_count: i32,
    selected_count: i32,
    refused_count: i32,
    writer_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SymbolSnapshot {
    symbol: String,
    selected_strategy_id: Option<String>,
    disposition: String,
    reason_code: String,
    exact_reason_code: String,
}

/// Canonical, order-independent snapshot of one candidate row. `ordinal` is
/// deliberately excluded -- it identifies which input position a candidate
/// occupied in one particular write, never part of its economic identity
/// (mirrors the conflict-evidence store's `CandidateSnapshot`).
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CandidateSnapshot {
    symbol: String,
    strategy_id: String,
    timeframe_secs: i64,
    canonical_score_decimal: Option<String>,
    canonical_score_micros: Option<i64>,
    scanner_rank: Option<i32>,
    watchlist_assigned: bool,
    promotion_query_ok: bool,
    promotion_state: Option<String>,
    promotion_effective: bool,
    promotion_expired: bool,
    evidence_resolved: bool,
    review_state_is_paper_candidate: bool,
    evidence_review_state: Option<String>,
    durable_legacy_fingerprint: Option<String>,
    recomputed_legacy_fingerprint: Option<String>,
    legacy_fingerprint_matches: bool,
    durable_exact_fingerprint_v2: Option<String>,
    recomputed_exact_fingerprint_v2: Option<String>,
    exact_fingerprint_v2_matches: bool,
    registry_enabled: bool,
    plugin_instantiable: bool,
    timeframe_matches: bool,
    data_ready: bool,
    evidence_review_id: Option<String>,
    evidence_scanner_scan_id: Option<String>,
    evidence_artifact_path: Option<String>,
    evidence_git_hash: Option<String>,
    promotion_transition_id: Option<String>,
    promotion_effective_at: Option<String>,
    promotion_expires_at: Option<String>,
    evidence_transition_id: Option<String>,
    exact_reason_code: String,
    selected: bool,
    disposition: String,
    reason_code: String,
}

fn new_plan_snapshot(plan: &NewDynamicSelectionPlan) -> PlanSnapshot {
    PlanSnapshot {
        run_id: plan.run_id,
        library_schema_version: plan.library_schema_version.clone(),
        context_schema_version: plan.context_schema_version.clone(),
        configured_mode: plan.configured_mode.clone(),
        effective_mode: plan.effective_mode.clone(),
        live_lock_applied: plan.live_lock_applied,
        approved_for_live: plan.approved_for_live,
        source_kind: plan.source_kind.clone(),
        source_identity: plan.source_identity.clone(),
        market_date: plan.market_date.clone(),
        disposition: plan.disposition.clone(),
        truth_state: plan.truth_state.clone(),
        blockers: {
            let mut b = plan.blockers.clone();
            b.sort();
            b
        },
        symbol_count: plan.symbol_count,
        candidate_count: plan.candidate_count,
        selected_count: plan.selected_count,
        refused_count: plan.refused_count,
        writer_version: plan.writer_version.clone(),
    }
}

fn stored_plan_snapshot(rec: &DynamicSelectionPlanRecord) -> PlanSnapshot {
    PlanSnapshot {
        run_id: rec.run_id,
        library_schema_version: rec.library_schema_version.clone(),
        context_schema_version: rec.context_schema_version.clone(),
        configured_mode: rec.configured_mode.clone(),
        effective_mode: rec.effective_mode.clone(),
        live_lock_applied: rec.live_lock_applied,
        approved_for_live: rec.approved_for_live,
        source_kind: rec.source_kind.clone(),
        source_identity: rec.source_identity.clone(),
        market_date: rec.market_date.clone(),
        disposition: rec.disposition.clone(),
        truth_state: rec.truth_state.clone(),
        blockers: {
            let mut b = rec.blockers.clone();
            b.sort();
            b
        },
        symbol_count: rec.symbol_count,
        candidate_count: rec.candidate_count,
        selected_count: rec.selected_count,
        refused_count: rec.refused_count,
        writer_version: rec.writer_version.clone(),
    }
}

fn new_symbol_snapshots(symbols: &[NewDynamicSelectionPlanSymbol]) -> Vec<SymbolSnapshot> {
    let mut snapshots: Vec<SymbolSnapshot> = symbols
        .iter()
        .map(|s| SymbolSnapshot {
            symbol: s.symbol.clone(),
            selected_strategy_id: s.selected_strategy_id.clone(),
            disposition: s.disposition.clone(),
            reason_code: s.reason_code.clone(),
            exact_reason_code: s.exact_reason_code.clone(),
        })
        .collect();
    snapshots.sort();
    snapshots
}

fn stored_symbol_snapshots(symbols: &[DynamicSelectionPlanSymbolRecord]) -> Vec<SymbolSnapshot> {
    let mut snapshots: Vec<SymbolSnapshot> = symbols
        .iter()
        .map(|s| SymbolSnapshot {
            symbol: s.symbol.clone(),
            selected_strategy_id: s.selected_strategy_id.clone(),
            disposition: s.disposition.clone(),
            reason_code: s.reason_code.clone(),
            exact_reason_code: s.exact_reason_code.clone(),
        })
        .collect();
    snapshots.sort();
    snapshots
}

fn new_candidate_snapshots(
    candidates: &[NewDynamicSelectionPlanCandidate],
) -> Vec<CandidateSnapshot> {
    let mut snapshots: Vec<CandidateSnapshot> = candidates
        .iter()
        .map(|c| CandidateSnapshot {
            symbol: c.symbol.clone(),
            strategy_id: c.strategy_id.clone(),
            timeframe_secs: c.timeframe_secs,
            canonical_score_decimal: c.canonical_score_decimal.clone(),
            canonical_score_micros: c.canonical_score_micros,
            scanner_rank: c.scanner_rank,
            watchlist_assigned: c.watchlist_assigned,
            promotion_query_ok: c.promotion_query_ok,
            promotion_state: c.promotion_state.clone(),
            promotion_effective: c.promotion_effective,
            promotion_expired: c.promotion_expired,
            evidence_resolved: c.evidence_resolved,
            review_state_is_paper_candidate: c.review_state_is_paper_candidate,
            evidence_review_state: c.evidence_review_state.clone(),
            durable_legacy_fingerprint: c.durable_legacy_fingerprint.clone(),
            recomputed_legacy_fingerprint: c.recomputed_legacy_fingerprint.clone(),
            legacy_fingerprint_matches: c.legacy_fingerprint_matches,
            durable_exact_fingerprint_v2: c.durable_exact_fingerprint_v2.clone(),
            recomputed_exact_fingerprint_v2: c.recomputed_exact_fingerprint_v2.clone(),
            exact_fingerprint_v2_matches: c.exact_fingerprint_v2_matches,
            registry_enabled: c.registry_enabled,
            plugin_instantiable: c.plugin_instantiable,
            timeframe_matches: c.timeframe_matches,
            data_ready: c.data_ready,
            evidence_review_id: c.evidence_review_id.clone(),
            evidence_scanner_scan_id: c.evidence_scanner_scan_id.clone(),
            evidence_artifact_path: c.evidence_artifact_path.clone(),
            evidence_git_hash: c.evidence_git_hash.clone(),
            promotion_transition_id: c.promotion_transition_id.clone(),
            promotion_effective_at: c.promotion_effective_at.clone(),
            promotion_expires_at: c.promotion_expires_at.clone(),
            evidence_transition_id: c.evidence_transition_id.clone(),
            exact_reason_code: c.exact_reason_code.clone(),
            selected: c.selected,
            disposition: c.disposition.clone(),
            reason_code: c.reason_code.clone(),
        })
        .collect();
    snapshots.sort();
    snapshots
}

fn stored_candidate_snapshots(
    candidates: &[DynamicSelectionPlanCandidateRecord],
) -> Vec<CandidateSnapshot> {
    let mut snapshots: Vec<CandidateSnapshot> = candidates
        .iter()
        .map(|c| CandidateSnapshot {
            symbol: c.symbol.clone(),
            strategy_id: c.strategy_id.clone(),
            timeframe_secs: c.timeframe_secs,
            canonical_score_decimal: c.canonical_score_decimal.clone(),
            canonical_score_micros: c.canonical_score_micros,
            scanner_rank: c.scanner_rank,
            watchlist_assigned: c.watchlist_assigned,
            promotion_query_ok: c.promotion_query_ok,
            promotion_state: c.promotion_state.clone(),
            promotion_effective: c.promotion_effective,
            promotion_expired: c.promotion_expired,
            evidence_resolved: c.evidence_resolved,
            review_state_is_paper_candidate: c.review_state_is_paper_candidate,
            evidence_review_state: c.evidence_review_state.clone(),
            durable_legacy_fingerprint: c.durable_legacy_fingerprint.clone(),
            recomputed_legacy_fingerprint: c.recomputed_legacy_fingerprint.clone(),
            legacy_fingerprint_matches: c.legacy_fingerprint_matches,
            durable_exact_fingerprint_v2: c.durable_exact_fingerprint_v2.clone(),
            recomputed_exact_fingerprint_v2: c.recomputed_exact_fingerprint_v2.clone(),
            exact_fingerprint_v2_matches: c.exact_fingerprint_v2_matches,
            registry_enabled: c.registry_enabled,
            plugin_instantiable: c.plugin_instantiable,
            timeframe_matches: c.timeframe_matches,
            data_ready: c.data_ready,
            evidence_review_id: c.evidence_review_id.clone(),
            evidence_scanner_scan_id: c.evidence_scanner_scan_id.clone(),
            evidence_artifact_path: c.evidence_artifact_path.clone(),
            evidence_git_hash: c.evidence_git_hash.clone(),
            promotion_transition_id: c.promotion_transition_id.clone(),
            promotion_effective_at: c.promotion_effective_at.clone(),
            promotion_expires_at: c.promotion_expires_at.clone(),
            evidence_transition_id: c.evidence_transition_id.clone(),
            exact_reason_code: c.exact_reason_code.clone(),
            selected: c.selected,
            disposition: c.disposition.clone(),
            reason_code: c.reason_code.clone(),
        })
        .collect();
    snapshots.sort();
    snapshots
}

/// Persist one dynamic-selection plan and its symbol/candidate rows
/// atomically. Only ever called for `effective_mode in {"shadow",
/// "paper_enforced"}` -- the default `off` mode must never reach this
/// function.
pub async fn insert_dynamic_selection_plan(
    pool: &PgPool,
    plan: NewDynamicSelectionPlan,
) -> Result<InsertDynamicSelectionPlanOutcome> {
    let mut tx = pool
        .begin()
        .await
        .context("insert_dynamic_selection_plan: begin failed")?;

    let existing_plan_row = sqlx::query(
        "select plan_id, run_id, library_schema_version, context_schema_version, \
         configured_mode, effective_mode, live_lock_applied, approved_for_live, \
         source_kind, source_identity, market_date, disposition, truth_state, blockers, symbol_count, \
         candidate_count, selected_count, refused_count, writer_version, created_at_utc \
         from sys_dynamic_selection_plans where plan_id = $1",
    )
    .bind(plan.plan_id)
    .fetch_optional(&mut *tx)
    .await
    .context("insert_dynamic_selection_plan: existence check failed")?;

    if let Some(row) = existing_plan_row {
        let existing_record = plan_row_to_record(&row);

        let existing_symbol_rows = sqlx::query(
            "select plan_id, symbol, selected_strategy_id, disposition, reason_code, \
             exact_reason_code from sys_dynamic_selection_plan_symbols where plan_id = $1",
        )
        .bind(plan.plan_id)
        .fetch_all(&mut *tx)
        .await
        .context("insert_dynamic_selection_plan: existing symbols query failed")?;
        let existing_symbols: Vec<DynamicSelectionPlanSymbolRecord> =
            existing_symbol_rows.iter().map(symbol_row_to_record).collect();

        let existing_candidate_rows = sqlx::query(&candidate_select_sql(
            "where plan_id = $1",
        ))
        .bind(plan.plan_id)
        .fetch_all(&mut *tx)
        .await
        .context("insert_dynamic_selection_plan: existing candidates query failed")?;
        let existing_candidates: Vec<DynamicSelectionPlanCandidateRecord> =
            existing_candidate_rows
                .iter()
                .map(candidate_row_to_record)
                .collect();

        tx.rollback()
            .await
            .context("insert_dynamic_selection_plan: rollback (read-only path) failed")?;

        let plans_match = stored_plan_snapshot(&existing_record) == new_plan_snapshot(&plan);
        let symbols_match =
            stored_symbol_snapshots(&existing_symbols) == new_symbol_snapshots(&plan.symbols);
        let candidates_match = stored_candidate_snapshots(&existing_candidates)
            == new_candidate_snapshots(&plan.candidates);

        if plans_match && symbols_match && candidates_match {
            return Ok(InsertDynamicSelectionPlanOutcome::AlreadyExists);
        }
        return Ok(InsertDynamicSelectionPlanOutcome::PayloadCollision {
            detail: format!(
                "plan_id {} already exists with a divergent payload (plan fields match: {}, \
                 symbol fields match: {}, candidate fields match: {}); never treated as \
                 idempotent replay",
                plan.plan_id, plans_match, symbols_match, candidates_match
            ),
        });
    }

    sqlx::query(
        r#"
        insert into sys_dynamic_selection_plans
            (plan_id, run_id, library_schema_version, context_schema_version,
             configured_mode, effective_mode, live_lock_applied, approved_for_live,
             source_kind, source_identity, market_date, disposition, truth_state, blockers,
             symbol_count, candidate_count, selected_count, refused_count, writer_version,
             created_at_utc)
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                $18, $19, $20)
        "#,
    )
    .bind(plan.plan_id)
    .bind(plan.run_id)
    .bind(&plan.library_schema_version)
    .bind(&plan.context_schema_version)
    .bind(&plan.configured_mode)
    .bind(&plan.effective_mode)
    .bind(plan.live_lock_applied)
    .bind(plan.approved_for_live)
    .bind(&plan.source_kind)
    .bind(&plan.source_identity)
    .bind(&plan.market_date)
    .bind(&plan.disposition)
    .bind(&plan.truth_state)
    .bind(&plan.blockers)
    .bind(plan.symbol_count)
    .bind(plan.candidate_count)
    .bind(plan.selected_count)
    .bind(plan.refused_count)
    .bind(&plan.writer_version)
    .bind(plan.created_at_utc)
    .execute(&mut *tx)
    .await
    .context("insert_dynamic_selection_plan: insert plan row failed")?;

    for s in &plan.symbols {
        sqlx::query(
            r#"
            insert into sys_dynamic_selection_plan_symbols
                (plan_id, symbol, selected_strategy_id, disposition, reason_code,
                 exact_reason_code)
            values ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(plan.plan_id)
        .bind(&s.symbol)
        .bind(&s.selected_strategy_id)
        .bind(&s.disposition)
        .bind(&s.reason_code)
        .bind(&s.exact_reason_code)
        .execute(&mut *tx)
        .await
        .context("insert_dynamic_selection_plan: insert symbol row failed")?;
    }

    for c in &plan.candidates {
        sqlx::query(
            r#"
            insert into sys_dynamic_selection_plan_candidates
                (plan_id, ordinal, symbol, strategy_id, timeframe_secs,
                 canonical_score_decimal, canonical_score_micros, scanner_rank,
                 watchlist_assigned, promotion_query_ok, promotion_state,
                 promotion_effective, promotion_expired, evidence_resolved,
                 review_state_is_paper_candidate, evidence_review_state,
                 durable_legacy_fingerprint, recomputed_legacy_fingerprint,
                 legacy_fingerprint_matches, durable_exact_fingerprint_v2,
                 recomputed_exact_fingerprint_v2, exact_fingerprint_v2_matches,
                 registry_enabled, plugin_instantiable, timeframe_matches, data_ready,
                 evidence_review_id, evidence_scanner_scan_id, evidence_artifact_path,
                 evidence_git_hash, promotion_transition_id, promotion_effective_at,
                 promotion_expires_at, evidence_transition_id, exact_reason_code, selected,
                 disposition, reason_code)
            values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                    $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30,
                    $31, $32, $33, $34, $35, $36, $37, $38)
            "#,
        )
        .bind(plan.plan_id)
        .bind(c.ordinal)
        .bind(&c.symbol)
        .bind(&c.strategy_id)
        .bind(c.timeframe_secs)
        .bind(&c.canonical_score_decimal)
        .bind(c.canonical_score_micros)
        .bind(c.scanner_rank)
        .bind(c.watchlist_assigned)
        .bind(c.promotion_query_ok)
        .bind(&c.promotion_state)
        .bind(c.promotion_effective)
        .bind(c.promotion_expired)
        .bind(c.evidence_resolved)
        .bind(c.review_state_is_paper_candidate)
        .bind(&c.evidence_review_state)
        .bind(&c.durable_legacy_fingerprint)
        .bind(&c.recomputed_legacy_fingerprint)
        .bind(c.legacy_fingerprint_matches)
        .bind(&c.durable_exact_fingerprint_v2)
        .bind(&c.recomputed_exact_fingerprint_v2)
        .bind(c.exact_fingerprint_v2_matches)
        .bind(c.registry_enabled)
        .bind(c.plugin_instantiable)
        .bind(c.timeframe_matches)
        .bind(c.data_ready)
        .bind(&c.evidence_review_id)
        .bind(&c.evidence_scanner_scan_id)
        .bind(&c.evidence_artifact_path)
        .bind(&c.evidence_git_hash)
        .bind(&c.promotion_transition_id)
        .bind(&c.promotion_effective_at)
        .bind(&c.promotion_expires_at)
        .bind(&c.evidence_transition_id)
        .bind(&c.exact_reason_code)
        .bind(c.selected)
        .bind(&c.disposition)
        .bind(&c.reason_code)
        .execute(&mut *tx)
        .await
        .context("insert_dynamic_selection_plan: insert candidate row failed")?;
    }

    tx.commit()
        .await
        .context("insert_dynamic_selection_plan: commit failed")?;

    Ok(InsertDynamicSelectionPlanOutcome::Inserted)
}

const CANDIDATE_COLUMNS: &str = "plan_id, ordinal, symbol, strategy_id, timeframe_secs, \
     canonical_score_decimal, canonical_score_micros, scanner_rank, watchlist_assigned, \
     promotion_query_ok, promotion_state, promotion_effective, promotion_expired, \
     evidence_resolved, review_state_is_paper_candidate, evidence_review_state, \
     durable_legacy_fingerprint, recomputed_legacy_fingerprint, legacy_fingerprint_matches, \
     durable_exact_fingerprint_v2, recomputed_exact_fingerprint_v2, \
     exact_fingerprint_v2_matches, registry_enabled, plugin_instantiable, timeframe_matches, \
     data_ready, evidence_review_id, evidence_scanner_scan_id, evidence_artifact_path, \
     evidence_git_hash, promotion_transition_id, promotion_effective_at, \
     promotion_expires_at, evidence_transition_id, exact_reason_code, selected, disposition, \
     reason_code";

fn candidate_select_sql(clause: &str) -> String {
    format!("select {CANDIDATE_COLUMNS} from sys_dynamic_selection_plan_candidates {clause}")
}

fn plan_row_to_record(row: &sqlx::postgres::PgRow) -> DynamicSelectionPlanRecord {
    use sqlx::Row;
    DynamicSelectionPlanRecord {
        plan_id: row.get("plan_id"),
        run_id: row.get("run_id"),
        library_schema_version: row.get("library_schema_version"),
        context_schema_version: row.get("context_schema_version"),
        configured_mode: row.get("configured_mode"),
        effective_mode: row.get("effective_mode"),
        live_lock_applied: row.get("live_lock_applied"),
        approved_for_live: row.get("approved_for_live"),
        source_kind: row.get("source_kind"),
        source_identity: row.get("source_identity"),
        market_date: row.get("market_date"),
        disposition: row.get("disposition"),
        truth_state: row.get("truth_state"),
        blockers: row.get("blockers"),
        symbol_count: row.get("symbol_count"),
        candidate_count: row.get("candidate_count"),
        selected_count: row.get("selected_count"),
        refused_count: row.get("refused_count"),
        writer_version: row.get("writer_version"),
        created_at_utc: row.get("created_at_utc"),
    }
}

fn symbol_row_to_record(row: &sqlx::postgres::PgRow) -> DynamicSelectionPlanSymbolRecord {
    use sqlx::Row;
    DynamicSelectionPlanSymbolRecord {
        plan_id: row.get("plan_id"),
        symbol: row.get("symbol"),
        selected_strategy_id: row.get("selected_strategy_id"),
        disposition: row.get("disposition"),
        reason_code: row.get("reason_code"),
        exact_reason_code: row.get("exact_reason_code"),
    }
}

fn candidate_row_to_record(row: &sqlx::postgres::PgRow) -> DynamicSelectionPlanCandidateRecord {
    use sqlx::Row;
    DynamicSelectionPlanCandidateRecord {
        plan_id: row.get("plan_id"),
        ordinal: row.get("ordinal"),
        symbol: row.get("symbol"),
        strategy_id: row.get("strategy_id"),
        timeframe_secs: row.get("timeframe_secs"),
        canonical_score_decimal: row.get("canonical_score_decimal"),
        canonical_score_micros: row.get("canonical_score_micros"),
        scanner_rank: row.get("scanner_rank"),
        watchlist_assigned: row.get("watchlist_assigned"),
        promotion_query_ok: row.get("promotion_query_ok"),
        promotion_state: row.get("promotion_state"),
        promotion_effective: row.get("promotion_effective"),
        promotion_expired: row.get("promotion_expired"),
        evidence_resolved: row.get("evidence_resolved"),
        review_state_is_paper_candidate: row.get("review_state_is_paper_candidate"),
        evidence_review_state: row.get("evidence_review_state"),
        durable_legacy_fingerprint: row.get("durable_legacy_fingerprint"),
        recomputed_legacy_fingerprint: row.get("recomputed_legacy_fingerprint"),
        legacy_fingerprint_matches: row.get("legacy_fingerprint_matches"),
        durable_exact_fingerprint_v2: row.get("durable_exact_fingerprint_v2"),
        recomputed_exact_fingerprint_v2: row.get("recomputed_exact_fingerprint_v2"),
        exact_fingerprint_v2_matches: row.get("exact_fingerprint_v2_matches"),
        registry_enabled: row.get("registry_enabled"),
        plugin_instantiable: row.get("plugin_instantiable"),
        timeframe_matches: row.get("timeframe_matches"),
        data_ready: row.get("data_ready"),
        evidence_review_id: row.get("evidence_review_id"),
        evidence_scanner_scan_id: row.get("evidence_scanner_scan_id"),
        evidence_artifact_path: row.get("evidence_artifact_path"),
        evidence_git_hash: row.get("evidence_git_hash"),
        promotion_transition_id: row.get("promotion_transition_id"),
        promotion_effective_at: row.get("promotion_effective_at"),
        promotion_expires_at: row.get("promotion_expires_at"),
        evidence_transition_id: row.get("evidence_transition_id"),
        exact_reason_code: row.get("exact_reason_code"),
        selected: row.get("selected"),
        disposition: row.get("disposition"),
        reason_code: row.get("reason_code"),
    }
}

/// Fetch one plan, its symbol rows, and its candidate rows (ordered by
/// `ordinal`) by `plan_id`. Unbounded -- reserved for trusted internal DB
/// replay/collision comparison. API/GUI/validator evidence projection must
/// use [`fetch_dynamic_selection_plan_for_read`] instead.
pub async fn fetch_dynamic_selection_plan(
    pool: &PgPool,
    plan_id: Uuid,
) -> Result<
    Option<(
        DynamicSelectionPlanRecord,
        Vec<DynamicSelectionPlanSymbolRecord>,
        Vec<DynamicSelectionPlanCandidateRecord>,
    )>,
> {
    let Some(plan_row) = sqlx::query(
        "select plan_id, run_id, library_schema_version, context_schema_version, \
         configured_mode, effective_mode, live_lock_applied, approved_for_live, \
         source_kind, source_identity, market_date, disposition, truth_state, blockers, symbol_count, \
         candidate_count, selected_count, refused_count, writer_version, created_at_utc \
         from sys_dynamic_selection_plans where plan_id = $1",
    )
    .bind(plan_id)
    .fetch_optional(pool)
    .await
    .context("fetch_dynamic_selection_plan: plan query failed")?
    else {
        return Ok(None);
    };

    let symbol_rows = sqlx::query(
        "select plan_id, symbol, selected_strategy_id, disposition, reason_code, \
         exact_reason_code from sys_dynamic_selection_plan_symbols where plan_id = $1 \
         order by symbol",
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await
    .context("fetch_dynamic_selection_plan: symbols query failed")?;
    let symbols = symbol_rows.iter().map(symbol_row_to_record).collect();

    let candidate_rows = sqlx::query(&candidate_select_sql("where plan_id = $1 order by ordinal"))
        .bind(plan_id)
        .fetch_all(pool)
        .await
        .context("fetch_dynamic_selection_plan: candidates query failed")?;
    let candidates = candidate_rows.iter().map(candidate_row_to_record).collect();

    Ok(Some((plan_row_to_record(&plan_row), symbols, candidates)))
}

/// Outcome of the bounded read-side fetch seam. Explicitly distinguishes a
/// complete in-bound plan, a plan proven to exceed the candidate bound (no
/// candidate data projected), and a plan that does not exist -- no caller
/// can infer completeness from a silently truncated candidate vector.
#[derive(Debug, Clone, PartialEq)]
pub enum BoundedDynamicSelectionPlanFetch {
    Complete(
        DynamicSelectionPlanRecord,
        Vec<DynamicSelectionPlanSymbolRecord>,
        Vec<DynamicSelectionPlanCandidateRecord>,
    ),
    CandidateLimitExceeded {
        plan: DynamicSelectionPlanRecord,
        observed_at_least: usize,
    },
    NotFound,
}

/// Bounded read-side fetch for status/list/detail evidence projection.
/// Fetches at most `bound + 1` candidate rows via SQL `LIMIT` -- enough to
/// prove the durable plan exceeds `bound`, never more.
pub async fn fetch_dynamic_selection_plan_for_read(
    pool: &PgPool,
    plan_id: Uuid,
    bound: usize,
) -> Result<BoundedDynamicSelectionPlanFetch> {
    let Some(plan_row) = sqlx::query(
        "select plan_id, run_id, library_schema_version, context_schema_version, \
         configured_mode, effective_mode, live_lock_applied, approved_for_live, \
         source_kind, source_identity, market_date, disposition, truth_state, blockers, symbol_count, \
         candidate_count, selected_count, refused_count, writer_version, created_at_utc \
         from sys_dynamic_selection_plans where plan_id = $1",
    )
    .bind(plan_id)
    .fetch_optional(pool)
    .await
    .context("fetch_dynamic_selection_plan_for_read: plan query failed")?
    else {
        return Ok(BoundedDynamicSelectionPlanFetch::NotFound);
    };
    let plan_record = plan_row_to_record(&plan_row);

    let fetch_limit: i64 = i64::try_from(bound.saturating_add(1))
        .context("fetch_dynamic_selection_plan_for_read: bound does not fit in i64")?;

    let candidate_rows = sqlx::query(&candidate_select_sql(
        "where plan_id = $1 order by ordinal limit $2",
    ))
    .bind(plan_id)
    .bind(fetch_limit)
    .fetch_all(pool)
    .await
    .context("fetch_dynamic_selection_plan_for_read: candidates query failed")?;

    if candidate_rows.len() > bound {
        return Ok(BoundedDynamicSelectionPlanFetch::CandidateLimitExceeded {
            plan: plan_record,
            observed_at_least: candidate_rows.len(),
        });
    }

    let symbol_rows = sqlx::query(
        "select plan_id, symbol, selected_strategy_id, disposition, reason_code, \
         exact_reason_code from sys_dynamic_selection_plan_symbols where plan_id = $1 \
         order by symbol",
    )
    .bind(plan_id)
    .fetch_all(pool)
    .await
    .context("fetch_dynamic_selection_plan_for_read: symbols query failed")?;
    let symbols = symbol_rows.iter().map(symbol_row_to_record).collect();
    let candidates = candidate_rows.iter().map(candidate_row_to_record).collect();
    Ok(BoundedDynamicSelectionPlanFetch::Complete(
        plan_record,
        symbols,
        candidates,
    ))
}

/// Fetch up to `limit` most recent plans for `run_id`, newest first, tied
/// deterministically on `plan_id DESC`.
pub async fn fetch_recent_dynamic_selection_plans(
    pool: &PgPool,
    run_id: Uuid,
    limit: i64,
) -> Result<Vec<DynamicSelectionPlanRecord>> {
    let rows = sqlx::query(
        "select plan_id, run_id, library_schema_version, context_schema_version, \
         configured_mode, effective_mode, live_lock_applied, approved_for_live, \
         source_kind, source_identity, market_date, disposition, truth_state, blockers, symbol_count, \
         candidate_count, selected_count, refused_count, writer_version, created_at_utc \
         from sys_dynamic_selection_plans \
         where run_id = $1 order by created_at_utc desc, plan_id desc limit $2",
    )
    .bind(run_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("fetch_recent_dynamic_selection_plans: query failed")?;

    Ok(rows.iter().map(plan_row_to_record).collect())
}
