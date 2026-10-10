//! MULTI-STRATEGY-CONFLICT-POLICY-01 Phase B — runtime batching/apply layer.
//!
//! The bridge between one tick's already-derived
//! [`crate::runtime_opportunity_allocation::PendingDecisionWithBarFacts`]
//! batch and the pure `mqk_portfolio::conflict_policy` model.
//!
//! `apply_conflict_policy` is the single call site Phase B wires into
//! `state/loop_runner.rs`, immediately before
//! `runtime_opportunity_allocation::gather_and_apply` (Bundle 5). It never
//! moves Bundle 5, never moves cap #6, and never touches canonical
//! submission — it only narrows (or, in `off`/`shadow` mode, passes through
//! unchanged) the vector Bundle 5 receives.
//!
//! `apply_conflict_policy` itself is I/O-free — every fact it needs (current
//! positions, the resolved mode, run/cycle identity inputs) is supplied by
//! the caller. [`gather_and_resolve`] is the thin, I/O-performing glue
//! `loop_runner.rs` actually calls once per tick; it resolves the effective
//! mode (with the live-lock) and delegates to the pure function above.
//!
//! # AUTHORITY-AND-EVIDENCE-REPAIR-01 (Defects 2 and 3)
//!
//! [`candidate_inputs`] looks up each decision's current position through
//! one canonical symbol index (`mqk_portfolio::canonical_symbol`) so a
//! decision symbol's casing can never cause a held position to read as
//! flat. [`compute_conflict_cycle_id`] now binds cycle identity to every
//! field capable of changing the resolver's output or its truthful
//! evidence — including the configured and effective mode, each
//! candidate's own `timeframe_secs`, full bar provenance, and order
//! semantics — so the same candidates evaluated once in `shadow` and again
//! in `paper_enforced` (or under any other economically-distinct input)
//! never collide on the same `plan_id`.

use mqk_schemas::QtyMicros;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use uuid::Uuid;

use crate::runtime_opportunity_allocation::PendingDecisionWithBarFacts;
use crate::runtime_strategy_conflict_mode::ConflictPolicyMode;
use crate::state::AppState;
use mqk_portfolio::{
    canonical_symbol, resolve_conflict_cycle, ConflictCandidateInput, ConflictCycleContext,
    ConflictCycleResult,
};

/// Length-prefixed (netstring-style) encoding of one field: `"{len}:{s}"`.
/// Concatenating `lp(...)` for a fixed, known-arity, known-order sequence of
/// fields is injective — a field's own content (including a literal colon,
/// comma, or pipe) can never be misread as a delimiter, because each field's
/// exact byte length is recorded immediately before it. This replaces the
/// prior unescaped colon/comma-joined seed (FINAL-IDENTITY-AND-READ-
/// AUTHORITY-REPAIR-01 Defect 3): that format could not distinguish, say, a
/// symbol containing a literal colon from a delimiter boundary. Never
/// decoded — this is a one-way hash preimage, so injectivity (not
/// parseability) is the only requirement.
fn lp(s: &str) -> String {
    format!("{}:{}", s.len(), s)
}

/// One candidate's canonical economic-identity seed fragment for
/// [`compute_conflict_cycle_id`]. Every field capable of changing the pure
/// resolver's output, or the truthful evidence recorded for it, is
/// represented here explicitly (never `Debug`-derived, so the format is
/// exact and stable under field reordering), each length-prefixed via
/// [`lp`] so the concatenation is unambiguous.
fn candidate_identity_seed(c: &ConflictCandidateInput) -> String {
    let symbol = canonical_symbol(&c.symbol);
    let strategy_id = c.strategy_id.trim().to_string();
    let side = c.side.trim().to_ascii_lowercase();
    let order_type = c.order_type.trim().to_ascii_lowercase();
    let tif = c.time_in_force.trim().to_ascii_lowercase();
    let limit_price = c
        .limit_price
        .map(|p| p.to_string())
        .unwrap_or_else(|| "none".to_string());
    // Explicit bar-fact presence: distinct from any individual field being
    // absent, so "missing entirely" and "present but mismatched" can never
    // hash to the same identity by coincidence.
    let bar_present = c.bar_symbol.is_some()
        && c.bar_strategy_id.is_some()
        && c.bar_timeframe.is_some()
        && c.bar_end_ts.is_some()
        && c.close_micros.is_some();
    let bar_symbol = c
        .bar_symbol
        .as_deref()
        .map(canonical_symbol)
        .unwrap_or_else(|| "none".to_string());
    let bar_strategy_id = c
        .bar_strategy_id
        .as_deref()
        .map(str::trim)
        .unwrap_or("none")
        .to_string();
    let bar_timeframe = c
        .bar_timeframe
        .as_deref()
        .map(str::trim)
        .unwrap_or("none")
        .to_string();
    let bar_end_ts = c
        .bar_end_ts
        .map(|t| t.to_string())
        .unwrap_or_else(|| "none".to_string());
    let close_micros = c
        .close_micros
        .map(|v| v.to_string())
        .unwrap_or_else(|| "none".to_string());
    // FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01 Defect 1: this candidate's
    // own `timeframe_secs` is the sole timeframe fact in identity — there is
    // no caller-supplied global timeframe field on `ConflictCandidateInput`
    // at all, so a mixed-timeframe batch's identity can never depend on
    // which assignment happened to dispatch first.
    let fields = [
        symbol,
        strategy_id,
        side,
        c.qty.to_string(),
        c.current_qty.to_string(),
        c.timeframe_secs.to_string(),
        order_type,
        tif,
        limit_price,
        bar_present.to_string(),
        bar_symbol,
        bar_strategy_id,
        bar_timeframe,
        bar_end_ts,
        close_micros,
    ];
    fields.iter().map(|f| lp(f)).collect()
}

/// Deterministic per-cycle economic identity. UUIDv5 of `run_id` +
/// `market_date` + the conflict policy schema version + the configured
/// (requested) mode + the effective mode + the sorted set of every
/// candidate's full economic-identity seed ([`candidate_identity_seed`])
/// this cycle. Every top-level field and every candidate seed is
/// length-prefixed ([`lp`]) so the concatenation is unambiguous.
///
/// This is the single shared canonical cycle-identity implementation
/// (FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01 Defect 3): runtime plan
/// creation ([`apply_conflict_policy`]) and read-side evidence validation
/// (`conflict_evidence_validation::validate_plan_with_candidates`, which
/// reconstructs the exact candidate batch from durable evidence and
/// recomputes this same function) both call it — there is no second,
/// divergent identity implementation anywhere in the codebase.
///
/// Defect 1 repair: no `timeframe` parameter exists here at all. The prior
/// signature took a caller-supplied global timeframe string (derived in
/// `loop_runner.rs` from `multi_symbol_assignments.first()`), so reordering
/// the assignment list could change the cycle id while the exact economic
/// candidate set was unchanged. Every candidate already carries its own
/// authoritative `timeframe_secs` in [`candidate_identity_seed`] — that is
/// now the only timeframe fact identity ever depends on.
///
/// AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 3 repair (carried forward): the
/// previous seed covered only `(symbol, strategy_id, side, qty,
/// current_qty, bar_end_ts)` and omitted mode entirely, so the exact same
/// candidates evaluated once in `shadow` and again in `paper_enforced`
/// collided on the same `plan_id` — the DB then silently treated the
/// enforced write as `AlreadyExists` and preserved stale shadow-mode
/// evidence. Every field capable of changing the resolver's output or its
/// truthful evidence (mode, per-candidate `timeframe_secs`, full bar
/// provenance including close, and order semantics) is now included.
/// Sorting makes the id independent of dispatch order; omitting the
/// loop-tick wall clock and `decision_id` (which embeds wall-clock material
/// — see `decision::bar_result_to_decisions`) makes reprocessing the exact
/// same economic cycle on a later tick produce the exact same id.
pub fn compute_conflict_cycle_id(
    run_id: Uuid,
    market_date: &str,
    configured_mode: ConflictPolicyMode,
    effective_mode: ConflictPolicyMode,
    candidates: &[ConflictCandidateInput],
) -> String {
    let mut seeds: Vec<String> = candidates.iter().map(candidate_identity_seed).collect();
    seeds.sort();
    let candidates_blob: String = seeds.iter().map(|s| lp(s)).collect();
    let top_fields = [
        run_id.to_string(),
        market_date.to_string(),
        mqk_portfolio::CONFLICT_POLICY_SCHEMA_VERSION.to_string(),
        configured_mode.as_str().to_string(),
        effective_mode.as_str().to_string(),
    ];
    let mut seed = "mqk.strategy-conflict-policy-cycle.v3|".to_string();
    for f in &top_fields {
        seed.push_str(&lp(f));
    }
    seed.push_str(&candidates_blob);
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, seed.as_bytes()).to_string()
}

pub struct ConflictPolicyContext {
    /// As configured (requested) before the live-lock, e.g. an operator
    /// asking for `paper_enforced` outside the paper+Alpaca lane. Evidence
    /// and cycle-identity only — dispatch always follows [`Self::mode`]
    /// (the already live-lock-resolved effective mode).
    pub configured_mode: ConflictPolicyMode,
    /// Already live-lock-resolved (see `runtime_strategy_conflict_mode::effective_mode`).
    pub mode: ConflictPolicyMode,
    pub run_id: Uuid,
    pub market_date: String,
    /// Evidence-only (Phase C's `created_at_utc`) — never part of cycle
    /// identity.
    pub now_micros: i64,
}

pub struct ConflictPolicyOutcome {
    /// The decisions to actually feed into Bundle 5's
    /// `runtime_opportunity_allocation::gather_and_apply` — exact original
    /// input decisions and bar facts, never rebuilt.
    pub decisions: Vec<PendingDecisionWithBarFacts>,
    /// `None` when `mode == Off`. `Some` (even an all-refused one)
    /// otherwise — this is the operator-visible/durable-evidence-worthy
    /// plan.
    pub plan: Option<ConflictCycleResult>,
    /// Symbols whose decisions were withheld because more than one strategy
    /// proposed for them while this mode does not enforce arbitration (see
    /// [`refuse_unarbitrated_competition`]). Empty for `paper_enforced` and
    /// whenever no symbol had competing proposers.
    pub unarbitrated_refusals: Vec<UnarbitratedRefusal>,
}

/// One symbol withheld by [`refuse_unarbitrated_competition`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnarbitratedRefusal {
    pub symbol: String,
    /// Distinct proposing strategy ids, sorted.
    pub strategy_ids: Vec<String>,
}

/// Bundle 6 is the only same-symbol arbiter. In `off`/`shadow` it passes
/// every proposal through, so two strategies proposing for one symbol in the
/// same tick would both reach submission, each sized against the same
/// pre-trade position. No arbitration policy is invented here: such symbols
/// are withheld entirely (fail closed) and reported. Symbols with a single
/// proposing strategy are untouched.
pub(crate) fn refuse_unarbitrated_competition(
    decisions: Vec<PendingDecisionWithBarFacts>,
) -> (Vec<PendingDecisionWithBarFacts>, Vec<UnarbitratedRefusal>) {
    let mut proposers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for p in &decisions {
        proposers
            .entry(canonical_symbol(&p.decision.symbol))
            .or_default()
            .insert(p.decision.strategy_id.trim().to_string());
    }
    let refusals: Vec<UnarbitratedRefusal> = proposers
        .iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(symbol, ids)| UnarbitratedRefusal {
            symbol: symbol.clone(),
            strategy_ids: ids.iter().cloned().collect(),
        })
        .collect();
    if refusals.is_empty() {
        return (decisions, refusals);
    }
    let refused: BTreeSet<&str> = refusals.iter().map(|r| r.symbol.as_str()).collect();
    let kept = decisions
        .into_iter()
        .filter(|p| !refused.contains(canonical_symbol(&p.decision.symbol).as_str()))
        .collect();
    (kept, refusals)
}

fn candidate_inputs(
    decisions: &[PendingDecisionWithBarFacts],
    current_positions: &BTreeMap<String, QtyMicros>,
) -> Vec<ConflictCandidateInput> {
    // AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 2: one canonical symbol index
    // for the current-position lookup, built once, so a decision symbol's
    // casing (e.g. "aapl") can never read a held position (keyed "AAPL")
    // as flat. Mirrors the exact same `canonical_symbol` used for
    // grouping, bar-symbol comparison, and evidence inside
    // `mqk_portfolio::conflict_policy` — one normalization, every call
    // site.
    let canonical_positions: BTreeMap<String, QtyMicros> = current_positions
        .iter()
        .map(|(sym, qty)| (canonical_symbol(sym), *qty))
        .collect();

    decisions
        .iter()
        .enumerate()
        .map(|(ordinal, p)| {
            let current_qty = canonical_positions
                .get(&canonical_symbol(&p.decision.symbol))
                .copied()
                .unwrap_or(QtyMicros::ZERO);
            let (bar_symbol, bar_strategy_id, bar_timeframe, bar_end_ts, close_micros) =
                match &p.bar_facts {
                    Some(f) => (
                        Some(f.symbol.clone()),
                        Some(f.strategy_id.clone()),
                        Some(f.timeframe.clone()),
                        Some(f.bar_end_ts),
                        Some(f.close_micros),
                    ),
                    None => (None, None, None, None, None),
                };
            ConflictCandidateInput {
                ordinal,
                symbol: p.decision.symbol.clone(),
                strategy_id: p.decision.strategy_id.clone(),
                timeframe_secs: p.decision.timeframe_secs,
                side: p.decision.side.clone(),
                qty: p.decision.qty,
                current_qty,
                order_type: p.decision.order_type.clone(),
                time_in_force: p.decision.time_in_force.clone(),
                limit_price: p.decision.limit_price,
                bar_symbol,
                bar_strategy_id,
                bar_timeframe,
                bar_end_ts,
                close_micros,
            }
        })
        .collect()
}

/// Apply Bundle 6 conflict resolution to one tick's already-derived,
/// bar-fact-bound decisions.
///
/// `off`: exact pass-through, no plan built, no candidate construction.
/// `shadow`: builds and returns the plan for evidence, but every original
/// input decision passes through unchanged and in original order —
/// identical to Bundle 5's own `Off`/`Shadow` split behavior on the
/// downstream side.
/// `paper_enforced`: replaces each same-symbol group with zero or one exact
/// original decision, output in the plan's deterministic (symbol-ascending)
/// order — never more decisions per symbol than the input contained.
pub fn apply_conflict_policy(
    ctx: &ConflictPolicyContext,
    decisions: Vec<PendingDecisionWithBarFacts>,
    current_positions: &BTreeMap<String, QtyMicros>,
) -> ConflictPolicyOutcome {
    if ctx.mode == ConflictPolicyMode::Off {
        return ConflictPolicyOutcome {
            decisions,
            plan: None,
            unarbitrated_refusals: Vec::new(),
        };
    }

    let candidates = candidate_inputs(&decisions, current_positions);
    let cycle_id = compute_conflict_cycle_id(
        ctx.run_id,
        &ctx.market_date,
        ctx.configured_mode,
        ctx.mode,
        &candidates,
    );
    let cycle_context = ConflictCycleContext {
        cycle_id,
        run_id: ctx.run_id.to_string(),
        market_date: ctx.market_date.clone(),
        policy_schema_version: mqk_portfolio::CONFLICT_POLICY_SCHEMA_VERSION.to_string(),
    };
    let plan = resolve_conflict_cycle(cycle_context, &candidates);

    match ctx.mode {
        ConflictPolicyMode::Off => unreachable!("handled above"),
        ConflictPolicyMode::Shadow => ConflictPolicyOutcome {
            decisions,
            plan: Some(plan),
            unarbitrated_refusals: Vec::new(),
        },
        ConflictPolicyMode::PaperEnforced => {
            let mut by_ordinal: Vec<Option<PendingDecisionWithBarFacts>> =
                decisions.into_iter().map(Some).collect();
            let mut resolved = Vec::new();
            for sym in &plan.symbol_results {
                if let Some(ord) = sym.selected_ordinal {
                    if let Some(slot) = by_ordinal.get_mut(ord) {
                        if let Some(d) = slot.take() {
                            resolved.push(d);
                        }
                    }
                }
            }
            ConflictPolicyOutcome {
                decisions: resolved,
                plan: Some(plan),
                unarbitrated_refusals: Vec::new(),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Phase C: durable evidence persistence (best-effort, never blocks the tick)
// ---------------------------------------------------------------------------

/// `pub(crate)` so `conflict_evidence_validation.rs`'s read-side
/// recomputation compares against this exact same mapping — never a second,
/// possibly-divergent copy.
pub(crate) fn disposition_str(d: mqk_portfolio::ConflictDisposition) -> &'static str {
    use mqk_portfolio::ConflictDisposition;
    match d {
        ConflictDisposition::Passthrough => "passthrough",
        ConflictDisposition::Selected => "selected",
        ConflictDisposition::NotSelected => "not_selected",
        ConflictDisposition::RefusedInvalid => "refused_invalid",
        ConflictDisposition::RefusedConflict => "refused_conflict",
    }
}

fn plan_to_new_db_plan(
    plan: &ConflictCycleResult,
    configured_mode: ConflictPolicyMode,
    effective_mode: ConflictPolicyMode,
    run_id: Uuid,
    created_at_utc: chrono::DateTime<chrono::Utc>,
) -> Option<mqk_db::NewRuntimeStrategyConflictPlan> {
    let plan_id = plan.context.cycle_id.parse::<Uuid>().ok()?;

    let mut candidates = Vec::new();
    let mut selected_count = 0i32;
    let mut refused_count = 0i32;
    for sym in &plan.symbol_results {
        for c in &sym.candidates {
            if c.selected {
                selected_count += 1;
            }
            if matches!(
                c.disposition,
                mqk_portfolio::ConflictDisposition::RefusedInvalid
                    | mqk_portfolio::ConflictDisposition::RefusedConflict
            ) {
                refused_count += 1;
            }
            let bar_present = c.bar_symbol.is_some()
                && c.bar_strategy_id.is_some()
                && c.bar_timeframe.is_some()
                && c.bar_end_ts.is_some()
                && c.close_micros.is_some();
            candidates.push(mqk_db::NewRuntimeStrategyConflictCandidate {
                ordinal: c.ordinal as i32,
                symbol: sym.symbol.clone(),
                strategy_id: c.strategy_id.clone(),
                timeframe_secs: c.timeframe_secs,
                side: c.side.trim().to_ascii_lowercase(),
                qty: c.qty,
                current_qty: c.current_qty,
                order_type: c.order_type.trim().to_ascii_lowercase(),
                time_in_force: c.time_in_force.trim().to_ascii_lowercase(),
                limit_price: c.limit_price,
                proposed_target_qty: c.proposed_target_qty,
                bar_present,
                bar_symbol: c.bar_symbol.clone(),
                bar_strategy_id: c.bar_strategy_id.clone(),
                bar_timeframe: c.bar_timeframe.clone(),
                bar_end_ts: c.bar_end_ts,
                close_micros: c.close_micros,
                selected: c.selected,
                disposition: disposition_str(c.disposition).to_string(),
                reason_code: c.reason_code.clone(),
            });
        }
    }
    let candidate_count = candidates.len() as i32;

    Some(mqk_db::NewRuntimeStrategyConflictPlan {
        plan_id,
        cycle_id: plan_id,
        run_id,
        mode: effective_mode.as_str().to_string(),
        configured_mode: configured_mode.as_str().to_string(),
        market_date: plan.context.market_date.clone(),
        policy_schema_version: plan.context.policy_schema_version.clone(),
        symbol_group_count: plan.symbol_results.len() as i32,
        candidate_count,
        selected_count,
        refused_count,
        truth_state: plan.truth_state.clone(),
        blockers: plan.blockers.clone(),
        created_at_utc,
        candidates,
    })
}

/// Persist `plan` (when present) to durable evidence. Best-effort: a
/// persistence failure is logged and otherwise ignored — this is evidence,
/// not authoritative order/portfolio truth, and must never block or fail
/// the tick that produced it, and must never alter which decision the pure
/// policy already selected. Never called for `mode == Off` (no plan exists
/// in that case).
async fn persist_plan_if_present(
    state_arc: &Arc<AppState>,
    plan: &Option<ConflictCycleResult>,
    configured_mode: ConflictPolicyMode,
    effective_mode: ConflictPolicyMode,
    run_id: Uuid,
    now_micros: i64,
) {
    let Some(plan) = plan else { return };
    let Some(db) = state_arc.db.as_ref() else {
        return;
    };
    // now_micros is loop-tick evidence context, not identity; created_at_utc
    // is recorded as the current wall clock at persistence time.
    let _ = now_micros;
    let created_at_utc = chrono::Utc::now();
    let Some(new_plan) = plan_to_new_db_plan(
        plan,
        configured_mode,
        effective_mode,
        run_id,
        created_at_utc,
    ) else {
        tracing::warn!(
            cycle_id = %plan.context.cycle_id,
            "runtime_strategy_conflict_plan_persist_skipped: cycle_id is not a valid UUID"
        );
        return;
    };
    match mqk_db::insert_runtime_strategy_conflict_plan(db, new_plan).await {
        Ok(mqk_db::InsertRuntimeStrategyConflictPlanOutcome::Inserted)
        | Ok(mqk_db::InsertRuntimeStrategyConflictPlanOutcome::AlreadyExists) => {}
        Ok(mqk_db::InsertRuntimeStrategyConflictPlanOutcome::PayloadCollision { detail }) => {
            // AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 4: same plan_id, but
            // the stored payload diverges from this replay's payload. This
            // must never be treated as idempotent -- log loudly and leave
            // the original row untouched; the tick itself is never
            // blocked (evidence, not authoritative truth).
            tracing::error!(
                cycle_id = %plan.context.cycle_id,
                detail = %detail,
                "runtime_strategy_conflict_plan_persist_payload_collision: same plan_id, \
                 divergent payload -- not idempotent, original row preserved"
            );
        }
        Err(err) => {
            tracing::warn!(
                cycle_id = %plan.context.cycle_id,
                error = %err,
                "runtime_strategy_conflict_plan_persist_failed"
            );
        }
    }
}

/// The live-lock-resolved conflict-policy mode this process dispatches
/// under — the one resolution both the per-tick call site and the
/// start-time arbitration check use.
pub(crate) fn effective_conflict_mode_for_state(
    state: &AppState,
) -> crate::runtime_strategy_conflict_mode::EffectiveConflictPolicyMode {
    let resolution = crate::runtime_strategy_conflict_mode::resolve_conflict_policy_mode_from_env();
    let broker_kind = crate::state::BrokerKind::parse(state.adapter_id());
    crate::runtime_strategy_conflict_mode::effective_mode(
        &resolution,
        state.deployment_mode(),
        broker_kind,
    )
}

/// The single per-tick call site `loop_runner.rs` uses. Resolves the
/// effective mode (env + live-lock) and delegates to the pure
/// [`apply_conflict_policy`] above.
///
/// When the effective mode is `Off`, no plan or candidate is built and
/// decisions pass through, except that symbols with more than one proposing
/// strategy are withheld ([`refuse_unarbitrated_competition`]); the same
/// withholding applies after `Shadow` evidence is recorded.
///
/// Defect 1 repair: no `timeframe` parameter exists here at all — Bundle 6
/// identity is now derived solely from canonical cycle facts and each
/// candidate's own timeframe/bar facts (see [`compute_conflict_cycle_id`]).
/// Bundle 5's `runtime_opportunity_allocation::gather_and_apply` still
/// receives its own accepted `dispatch_timeframe` context directly from
/// `loop_runner.rs` — this narrow call-site split is the only change to
/// Bundle 5's timeframe handling.
pub async fn gather_and_resolve(
    state_arc: &Arc<AppState>,
    run_id: Uuid,
    now_micros: i64,
    market_date: String,
    decisions: Vec<PendingDecisionWithBarFacts>,
    current_positions: &BTreeMap<String, QtyMicros>,
) -> ConflictPolicyOutcome {
    let eff = effective_conflict_mode_for_state(state_arc);

    if eff.effective_mode == ConflictPolicyMode::Off {
        let (decisions, unarbitrated_refusals) = refuse_unarbitrated_competition(decisions);
        return ConflictPolicyOutcome {
            decisions,
            plan: None,
            unarbitrated_refusals,
        };
    }

    let ctx = ConflictPolicyContext {
        configured_mode: eff.configured_mode,
        mode: eff.effective_mode,
        run_id,
        market_date,
        now_micros,
    };
    let mut outcome = apply_conflict_policy(&ctx, decisions, current_positions);
    persist_plan_if_present(
        state_arc,
        &outcome.plan,
        eff.configured_mode,
        eff.effective_mode,
        run_id,
        now_micros,
    )
    .await;
    if eff.effective_mode == ConflictPolicyMode::Shadow {
        let (decisions, unarbitrated_refusals) =
            refuse_unarbitrated_competition(std::mem::take(&mut outcome.decisions));
        outcome.decisions = decisions;
        outcome.unarbitrated_refusals = unarbitrated_refusals;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whole-unit test quantity (`1` == one share == `QTY_MICROS_SCALE` raw).
    fn q(units: i64) -> mqk_schemas::QtyMicros {
        mqk_schemas::QtyMicros::from_whole_units(units).unwrap()
    }
    use crate::decision::InternalStrategyDecision;
    use crate::state::EvaluatedBarFacts;

    fn run_id() -> Uuid {
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test-run")
    }

    const TIMEFRAME: &str = "5m";

    fn decision(symbol: &str, strategy_id: &str, side: &str, qty: i64) -> InternalStrategyDecision {
        InternalStrategyDecision {
            decision_id: format!("{side}-{symbol}-{strategy_id}"),
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs: 300,
            strategy_semantic_fingerprint: String::new(),
            side: side.to_string(),
            qty: q(qty),
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
        }
    }

    fn facts(symbol: &str, strategy_id: &str, bar_end_ts: i64) -> EvaluatedBarFacts {
        EvaluatedBarFacts {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe: TIMEFRAME.to_string(),
            bar_end_ts,
            close_micros: 100_000_000,
        }
    }

    fn bound_buy(
        symbol: &str,
        strategy_id: &str,
        qty: i64,
        bar_end_ts: i64,
    ) -> PendingDecisionWithBarFacts {
        PendingDecisionWithBarFacts {
            decision: decision(symbol, strategy_id, "buy", qty),
            bar_facts: Some(facts(symbol, strategy_id, bar_end_ts)),
            dynamic_selection_provenance: None,
        }
    }

    /// A sell with real bar facts attached, mirroring production
    /// (`loop_runner.rs` clones the same `bar_facts` onto every decision
    /// derived from one bar result, buy or sell).
    fn bound_sell(
        symbol: &str,
        strategy_id: &str,
        qty: i64,
        bar_end_ts: i64,
    ) -> PendingDecisionWithBarFacts {
        PendingDecisionWithBarFacts {
            decision: decision(symbol, strategy_id, "sell", qty),
            bar_facts: Some(facts(symbol, strategy_id, bar_end_ts)),
            dynamic_selection_provenance: None,
        }
    }

    /// A sell with no bar facts at all -- structurally invalid after the
    /// Defect 2 repair.
    fn unbound_sell(symbol: &str, strategy_id: &str, qty: i64) -> PendingDecisionWithBarFacts {
        PendingDecisionWithBarFacts {
            decision: decision(symbol, strategy_id, "sell", qty),
            bar_facts: None,
            dynamic_selection_provenance: None,
        }
    }

    fn ctx(mode: ConflictPolicyMode) -> ConflictPolicyContext {
        ConflictPolicyContext {
            configured_mode: mode,
            mode,
            run_id: run_id(),
            market_date: "2026-07-26".to_string(),
            now_micros: 1_000_000,
        }
    }

    #[test]
    fn off_mode_returns_exact_original_vector_in_exact_original_order() {
        let decisions = vec![
            bound_buy("AAPL", "s1", 10, 1_000),
            unbound_sell("MSFT", "s2", 5),
        ];
        let out = apply_conflict_policy(
            &ctx(ConflictPolicyMode::Off),
            decisions.clone(),
            &BTreeMap::new(),
        );
        assert!(out.plan.is_none());
        assert_eq!(out.decisions.len(), 2);
        assert_eq!(out.decisions[0].decision.symbol, "AAPL");
        assert_eq!(out.decisions[1].decision.symbol, "MSFT");
    }

    #[test]
    fn shadow_mode_returns_exact_original_vector_in_exact_original_order() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(0));
        let decisions = vec![
            bound_buy("AAPL", "s1", 10, 1_000),
            bound_buy("AAPL", "s2", 20, 2_000), // conflicting increase target
        ];
        let out = apply_conflict_policy(&ctx(ConflictPolicyMode::Shadow), decisions, &current);
        assert!(out.plan.is_some());
        assert_eq!(out.decisions.len(), 2, "shadow must not narrow the batch");
    }

    #[test]
    fn paper_enforced_emits_at_most_one_decision_per_symbol() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(0));
        let decisions = vec![
            bound_buy("AAPL", "s1", 10, 1_000),
            bound_buy("AAPL", "s2", 10, 2_000), // equal target -> consensus
        ];
        let out =
            apply_conflict_policy(&ctx(ConflictPolicyMode::PaperEnforced), decisions, &current);
        let aapl_count = out
            .decisions
            .iter()
            .filter(|d| d.decision.symbol == "AAPL")
            .count();
        assert_eq!(aapl_count, 1);
    }

    #[test]
    fn paper_enforced_never_resurrects_a_refused_increase() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(0));
        let decisions = vec![
            bound_buy("AAPL", "s1", 10, 1_000),
            bound_buy("AAPL", "s2", 20, 2_000), // differing targets -> refused
        ];
        let out =
            apply_conflict_policy(&ctx(ConflictPolicyMode::PaperEnforced), decisions, &current);
        assert!(
            !out.decisions.iter().any(|d| d.decision.symbol == "AAPL"),
            "conflicting increase targets must refuse the whole symbol, not pick one"
        );
    }

    #[test]
    fn paper_enforced_preserves_a_selected_reduction_downstream() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(20));
        let decisions = vec![
            bound_buy("AAPL", "s1", 10, 1_000),
            bound_sell("AAPL", "s2", 5, 2_000),
        ];
        let out =
            apply_conflict_policy(&ctx(ConflictPolicyMode::PaperEnforced), decisions, &current);
        assert_eq!(out.decisions.len(), 1);
        assert_eq!(out.decisions[0].decision.side, "sell");
    }

    #[test]
    fn unrelated_symbol_unaffected_by_a_refused_conflict() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(0));
        current.insert("MSFT".to_string(), q(0));
        let decisions = vec![
            bound_buy("AAPL", "s1", 10, 1_000),
            bound_buy("AAPL", "s2", 20, 2_000), // refused
            bound_buy("MSFT", "s1", 5, 3_000),  // unaffected
        ];
        let out =
            apply_conflict_policy(&ctx(ConflictPolicyMode::PaperEnforced), decisions, &current);
        assert_eq!(out.decisions.len(), 1);
        assert_eq!(out.decisions[0].decision.symbol, "MSFT");
    }

    #[test]
    fn unbound_sell_is_refused_and_never_reaches_downstream() {
        // AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 2: a sell with no bar
        // facts is now structurally invalid, end to end.
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(20));
        let decisions = vec![unbound_sell("AAPL", "s1", 5)];
        let out =
            apply_conflict_policy(&ctx(ConflictPolicyMode::PaperEnforced), decisions, &current);
        assert!(out.decisions.is_empty());
    }

    #[test]
    fn lowercase_decision_symbol_still_reads_the_held_position() {
        // AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 2: "aapl" must read the
        // exact same current quantity as "AAPL" -- never zero/flat.
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(20));
        let decisions = vec![bound_sell("aapl", "s1", 5, 1_000)];
        let out =
            apply_conflict_policy(&ctx(ConflictPolicyMode::PaperEnforced), decisions, &current);
        assert_eq!(
            out.decisions.len(),
            1,
            "lowercase symbol must still resolve current_qty=20 and pass"
        );
    }

    #[test]
    fn cycle_id_is_deterministic_and_order_independent() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(0));
        current.insert("MSFT".to_string(), q(0));
        let forward = vec![
            bound_buy("AAPL", "s1", 10, 1_000),
            bound_buy("MSFT", "s1", 5, 2_000),
        ];
        let reversed = vec![
            bound_buy("MSFT", "s1", 5, 2_000),
            bound_buy("AAPL", "s1", 10, 1_000),
        ];
        let out1 = apply_conflict_policy(&ctx(ConflictPolicyMode::Shadow), forward, &current);
        let out2 = apply_conflict_policy(&ctx(ConflictPolicyMode::Shadow), reversed, &current);
        assert_eq!(
            out1.plan.unwrap().context.cycle_id,
            out2.plan.unwrap().context.cycle_id
        );
    }

    #[test]
    fn same_economic_cycle_replayed_on_a_later_tick_yields_the_same_cycle_id() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(0));
        let mut ctx1 = ctx(ConflictPolicyMode::Shadow);
        ctx1.now_micros = 1;
        let mut ctx2 = ctx(ConflictPolicyMode::Shadow);
        ctx2.now_micros = 999_999;
        let d1 = vec![bound_buy("AAPL", "s1", 10, 1_000)];
        let d2 = vec![bound_buy("AAPL", "s1", 10, 1_000)];
        let out1 = apply_conflict_policy(&ctx1, d1, &current);
        let out2 = apply_conflict_policy(&ctx2, d2, &current);
        assert_eq!(
            out1.plan.unwrap().context.cycle_id,
            out2.plan.unwrap().context.cycle_id
        );
    }

    #[test]
    fn different_strategy_changes_cycle_id() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(0));
        let d1 = vec![bound_buy("AAPL", "s1", 10, 1_000)];
        let d2 = vec![bound_buy("AAPL", "s2", 10, 1_000)];
        let out1 = apply_conflict_policy(&ctx(ConflictPolicyMode::Shadow), d1, &current);
        let out2 = apply_conflict_policy(&ctx(ConflictPolicyMode::Shadow), d2, &current);
        assert_ne!(
            out1.plan.unwrap().context.cycle_id,
            out2.plan.unwrap().context.cycle_id
        );
    }

    #[test]
    fn different_bar_changes_cycle_id() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(0));
        let d1 = vec![bound_buy("AAPL", "s1", 10, 1_000)];
        let d2 = vec![bound_buy("AAPL", "s1", 10, 2_000)];
        let out1 = apply_conflict_policy(&ctx(ConflictPolicyMode::Shadow), d1, &current);
        let out2 = apply_conflict_policy(&ctx(ConflictPolicyMode::Shadow), d2, &current);
        assert_ne!(
            out1.plan.unwrap().context.cycle_id,
            out2.plan.unwrap().context.cycle_id
        );
    }

    #[test]
    fn different_target_changes_cycle_id() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(0));
        let d1 = vec![bound_buy("AAPL", "s1", 10, 1_000)];
        let d2 = vec![bound_buy("AAPL", "s1", 11, 1_000)];
        let out1 = apply_conflict_policy(&ctx(ConflictPolicyMode::Shadow), d1, &current);
        let out2 = apply_conflict_policy(&ctx(ConflictPolicyMode::Shadow), d2, &current);
        assert_ne!(
            out1.plan.unwrap().context.cycle_id,
            out2.plan.unwrap().context.cycle_id
        );
    }

    #[test]
    fn different_current_position_changes_cycle_id() {
        let mut current1 = BTreeMap::new();
        current1.insert("AAPL".to_string(), q(0));
        let mut current2 = BTreeMap::new();
        current2.insert("AAPL".to_string(), q(5));
        let d1 = vec![bound_buy("AAPL", "s1", 10, 1_000)];
        let d2 = vec![bound_buy("AAPL", "s1", 10, 1_000)];
        let out1 = apply_conflict_policy(&ctx(ConflictPolicyMode::Shadow), d1, &current1);
        let out2 = apply_conflict_policy(&ctx(ConflictPolicyMode::Shadow), d2, &current2);
        assert_ne!(
            out1.plan.unwrap().context.cycle_id,
            out2.plan.unwrap().context.cycle_id
        );
    }

    #[test]
    fn different_symbol_set_changes_cycle_id() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(0));
        current.insert("MSFT".to_string(), q(0));
        let d1 = vec![bound_buy("AAPL", "s1", 10, 1_000)];
        let d2 = vec![bound_buy("MSFT", "s1", 10, 1_000)];
        let out1 = apply_conflict_policy(&ctx(ConflictPolicyMode::Shadow), d1, &current);
        let out2 = apply_conflict_policy(&ctx(ConflictPolicyMode::Shadow), d2, &current);
        assert_ne!(
            out1.plan.unwrap().context.cycle_id,
            out2.plan.unwrap().context.cycle_id
        );
    }

    #[test]
    fn no_time_or_random_api_in_identity_path_evidence_timestamp_irrelevant() {
        // now_micros differs but cycle_id must match -- proven again here
        // directly against compute_conflict_cycle_id (not just apply_*).
        let candidates = vec![ConflictCandidateInput {
            ordinal: 0,
            symbol: "AAPL".to_string(),
            strategy_id: "s1".to_string(),
            timeframe_secs: 300,
            side: "buy".to_string(),
            qty: q(10),
            current_qty: q(0),
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
            bar_symbol: Some("AAPL".to_string()),
            bar_strategy_id: Some("s1".to_string()),
            bar_timeframe: Some(TIMEFRAME.to_string()),
            bar_end_ts: Some(1_000),
            close_micros: Some(100_000_000),
        }];
        let id1 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        let id2 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        assert_eq!(id1, id2);
    }

    // ── AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 3: cycle identity must
    // depend on mode, per-candidate timeframe_secs, bar timeframe, close,
    // presence, and order semantics ────────────────────────────────────

    fn one_candidate() -> Vec<ConflictCandidateInput> {
        vec![ConflictCandidateInput {
            ordinal: 0,
            symbol: "AAPL".to_string(),
            strategy_id: "s1".to_string(),
            timeframe_secs: 300,
            side: "buy".to_string(),
            qty: q(10),
            current_qty: q(0),
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
            bar_symbol: Some("AAPL".to_string()),
            bar_strategy_id: Some("s1".to_string()),
            bar_timeframe: Some("5m".to_string()),
            bar_end_ts: Some(1_000),
            close_micros: Some(100_000_000),
        }]
    }

    #[test]
    fn shadow_versus_paper_enforced_changes_plan_id() {
        let candidates = one_candidate();
        let shadow = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        let enforced = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::PaperEnforced,
            ConflictPolicyMode::PaperEnforced,
            &candidates,
        );
        assert_ne!(shadow, enforced);
    }

    #[test]
    fn configured_versus_effective_mode_divergence_changes_plan_id() {
        let candidates = one_candidate();
        let live_locked = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::PaperEnforced, // configured
            ConflictPolicyMode::Off,           // live-locked down to Off
            &candidates,
        );
        let honest_off = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Off,
            ConflictPolicyMode::Off,
            &candidates,
        );
        assert_ne!(live_locked, honest_off);
    }

    #[test]
    fn changed_timeframe_secs_changes_plan_id() {
        let mut candidates = one_candidate();
        let id1 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        candidates[0].timeframe_secs = 900;
        let id2 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        assert_ne!(id1, id2);
    }

    #[test]
    fn changed_bar_timeframe_changes_plan_id() {
        let mut candidates = one_candidate();
        let id1 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        candidates[0].bar_timeframe = Some("1h".to_string());
        let id2 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        assert_ne!(id1, id2);
    }

    #[test]
    fn changed_close_changes_plan_id() {
        let mut candidates = one_candidate();
        let id1 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        candidates[0].close_micros = Some(200_000_000);
        let id2 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        assert_ne!(id1, id2);
    }

    #[test]
    fn missing_versus_present_bar_facts_changes_plan_id() {
        let mut candidates = one_candidate();
        let present = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        candidates[0].bar_symbol = None;
        candidates[0].bar_strategy_id = None;
        candidates[0].bar_timeframe = None;
        candidates[0].bar_end_ts = None;
        candidates[0].close_micros = None;
        let missing = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        assert_ne!(present, missing);
    }

    #[test]
    fn changed_order_semantics_changes_plan_id() {
        let mut candidates = one_candidate();
        let market = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        candidates[0].order_type = "limit".to_string();
        candidates[0].limit_price = Some(50_000_000);
        let limit = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        assert_ne!(market, limit);
    }

    #[test]
    fn candidate_input_order_remains_irrelevant_to_plan_id() {
        let mut c0 = one_candidate();
        c0.push(ConflictCandidateInput {
            ordinal: 1,
            symbol: "MSFT".to_string(),
            strategy_id: "s1".to_string(),
            timeframe_secs: 300,
            side: "buy".to_string(),
            qty: q(5),
            current_qty: q(0),
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
            bar_symbol: Some("MSFT".to_string()),
            bar_strategy_id: Some("s1".to_string()),
            bar_timeframe: Some("5m".to_string()),
            bar_end_ts: Some(2_000),
            close_micros: Some(50_000_000),
        });
        let mut c1 = c0.clone();
        c1.reverse();
        let id0 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &c0,
        );
        let id1 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &c1,
        );
        assert_eq!(id0, id1);
    }

    // ── FINAL-IDENTITY-AND-READ-AUTHORITY-REPAIR-01 Defect 1: no global
    // first-assignment timeframe in identity; mixed-timeframe reordering
    // (different assignment order, different source ordinals) must never
    // change the cycle id for the same exact economic candidate set ──────

    fn mixed_timeframe_candidate(
        ordinal: usize,
        symbol: &str,
        timeframe_secs: i64,
        bar_timeframe: &str,
    ) -> ConflictCandidateInput {
        ConflictCandidateInput {
            ordinal,
            symbol: symbol.to_string(),
            strategy_id: "s1".to_string(),
            timeframe_secs,
            side: "buy".to_string(),
            qty: q(10),
            current_qty: q(0),
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
            bar_symbol: Some(symbol.to_string()),
            bar_strategy_id: Some("s1".to_string()),
            bar_timeframe: Some(bar_timeframe.to_string()),
            bar_end_ts: Some(1_000),
            close_micros: Some(100_000_000),
        }
    }

    #[test]
    fn mixed_timeframe_batch_reordered_with_different_ordinals_yields_same_cycle_id() {
        // Simulates two symbols dispatched on different native timeframes
        // (e.g. AAPL on 5m, MSFT on 1h) assembled in one order (AAPL first,
        // ordinal 0/1) and then in the reverse assignment order (MSFT
        // first, ordinal 0/1) -- exactly the scenario where the old
        // `multi_symbol_assignments.first()`-derived global timeframe could
        // change depending on which assignment happened to dispatch first,
        // even though the economic candidate set is identical.
        let first_order = vec![
            mixed_timeframe_candidate(0, "AAPL", 300, "5m"),
            mixed_timeframe_candidate(1, "MSFT", 3_600, "1h"),
        ];
        // Same two candidates, reassembled in the opposite assignment
        // order -- each now carries a different `ordinal` than it did
        // above (MSFT is ordinal 0 here, AAPL is ordinal 1).
        let reversed_order = vec![
            mixed_timeframe_candidate(0, "MSFT", 3_600, "1h"),
            mixed_timeframe_candidate(1, "AAPL", 300, "5m"),
        ];
        let id_first = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &first_order,
        );
        let id_reversed = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &reversed_order,
        );
        assert_eq!(
            id_first, id_reversed,
            "mixed-timeframe reordering with different source ordinals must never change cycle id"
        );
    }

    // -----------------------------------------------------------------
    // MULTI-STRATEGY-RUNTIME-DISPATCH-01 (R1B), proofs #3/#4/#5: genuine
    // same-symbol competing proposals from the two real intraday_scalper
    // variants reach this exact resolver and are resolved deterministically,
    // regardless of input order. Uses real strategy identities specifically
    // (rather than the synthetic "s1"/"s2" the tests above already use for
    // the same underlying mechanism) to tie this coverage explicitly to the
    // R1B same-symbol multi-strategy feature.
    // -----------------------------------------------------------------

    /// R1B #3/#4: both `intraday_scalper` and `intraday_short_scalper`
    /// original proposals for AAPL reach Bundle 6 as real candidates; the
    /// deterministic, authorized survivor is the risk-reducing sell.
    #[test]
    fn r1b_both_real_strategy_proposals_reach_bundle6_and_resolve_deterministically() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(10));
        let decisions = vec![
            bound_buy("AAPL", "intraday_scalper", 5, 1_000),
            bound_sell("AAPL", "intraday_short_scalper", 3, 2_000),
        ];
        let out =
            apply_conflict_policy(&ctx(ConflictPolicyMode::PaperEnforced), decisions, &current);
        assert_eq!(
            out.decisions.len(),
            1,
            "exactly one authorized survivor for AAPL"
        );
        assert_eq!(out.decisions[0].decision.side, "sell");
        assert_eq!(
            out.decisions[0].decision.strategy_id,
            "intraday_short_scalper"
        );
        assert!(
            out.plan.is_some(),
            "paper_enforced must produce durable conflict-resolution evidence"
        );
    }

    /// R1B #5: reversing the two real strategies' input order produces the
    /// identical authorized economic result.
    #[test]
    fn r1b_reversed_real_strategy_input_order_yields_same_result() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(10));
        let forward = vec![
            bound_buy("AAPL", "intraday_scalper", 5, 1_000),
            bound_sell("AAPL", "intraday_short_scalper", 3, 2_000),
        ];
        let mut reversed = forward.clone();
        reversed.reverse();

        let out_forward =
            apply_conflict_policy(&ctx(ConflictPolicyMode::PaperEnforced), forward, &current);
        let out_reversed =
            apply_conflict_policy(&ctx(ConflictPolicyMode::PaperEnforced), reversed, &current);

        assert_eq!(out_forward.decisions.len(), out_reversed.decisions.len());
        assert_eq!(
            out_forward.decisions[0].decision.strategy_id,
            out_reversed.decisions[0].decision.strategy_id,
            "input order must never change which strategy's decision survives"
        );
        assert_eq!(
            out_forward.decisions[0].decision.side,
            out_reversed.decisions[0].decision.side
        );
        assert_eq!(
            out_forward.decisions[0].decision.qty,
            out_reversed.decisions[0].decision.qty
        );
    }

    // -----------------------------------------------------------------
    // CUTOVER-1D-A3-4: exact fractional quantity end to end
    // -----------------------------------------------------------------

    #[test]
    fn a3_4_fractional_conflict_candidate_and_identity() {
        let qm = mqk_schemas::QtyMicros::new;
        let mk = |raw: i64| {
            let mut p = bound_buy("BTC/USD", "s1", 1, 1_000);
            p.decision.qty = qm(raw);
            p
        };
        let mut current = BTreeMap::new();
        current.insert("BTC/USD".to_string(), qm(250));

        let out = apply_conflict_policy(&ctx(ConflictPolicyMode::Shadow), vec![mk(100)], &current);
        let plan = out.plan.expect("plan");
        let row = &plan.symbol_results[0].candidates[0];
        assert_eq!(row.qty, qm(100));
        assert_eq!(row.current_qty, qm(250));
        assert_eq!(row.proposed_target_qty, Some(qm(350)));
        assert_eq!(plan.symbol_results[0].selected_ordinal, Some(0));

        // DB projection carries the exact QtyMicros (no whole-unit narrowing).
        let db_plan = plan_to_new_db_plan(
            &plan,
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            run_id(),
            chrono::Utc::now(),
        )
        .expect("db plan");
        assert_eq!(db_plan.candidates[0].qty, qm(100));
        assert_eq!(db_plan.candidates[0].current_qty, qm(250));
        assert_eq!(db_plan.candidates[0].proposed_target_qty, Some(qm(350)));

        // Cycle identity distinguishes fractional quantities...
        let id = |raw: i64| {
            let inputs = candidate_inputs(&[mk(raw)], &current);
            compute_conflict_cycle_id(
                run_id(),
                "2026-07-26",
                ConflictPolicyMode::Shadow,
                ConflictPolicyMode::Shadow,
                &inputs,
            )
        };
        assert_eq!(id(100), id(100), "deterministic");
        assert_ne!(
            id(100),
            id(101),
            "divergent fractional qty changes identity"
        );
    }

    #[test]
    fn a3_4_whole_equity_conflict_identity_seed_is_byte_identical_to_the_i64_form() {
        // The seed renders quantities via `Display`; a whole QtyMicros must
        // render exactly like the historical i64 (`10`, `-5`, `0`).
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), q(3));
        let inputs = candidate_inputs(&[bound_buy("AAPL", "s1", 10, 1_000)], &current);
        let seed = candidate_identity_seed(&inputs[0]);
        assert!(
            seed.contains(&lp("10")) && seed.contains(&lp("3")),
            "{seed}"
        );
        assert_eq!(inputs[0].qty.to_string(), "10");
        assert_eq!(q(-5).to_string(), "-5");
        assert_eq!(q(0).to_string(), "0");
    }
}
