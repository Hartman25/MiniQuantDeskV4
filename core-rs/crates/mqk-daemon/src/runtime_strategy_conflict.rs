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

use std::collections::BTreeMap;
use std::sync::Arc;

use uuid::Uuid;

use crate::runtime_opportunity_allocation::PendingDecisionWithBarFacts;
use crate::runtime_strategy_conflict_mode::ConflictPolicyMode;
use crate::state::AppState;
use mqk_portfolio::{
    canonical_symbol, resolve_conflict_cycle, ConflictCandidateInput, ConflictCycleContext,
    ConflictCycleResult,
};

/// One candidate's canonical economic-identity seed fragment for
/// [`compute_conflict_cycle_id`]. Every field capable of changing the pure
/// resolver's output, or the truthful evidence recorded for it, is
/// represented here explicitly (never `Debug`-derived, so the format is
/// exact and stable under field reordering).
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
    format!(
        "{symbol}:{strategy_id}:{side}:{qty}:{current_qty}:{timeframe_secs}:{order_type}:{tif}:\
         {limit_price}:{bar_present}:{bar_symbol}:{bar_strategy_id}:{bar_timeframe}:{bar_end_ts}:\
         {close_micros}",
        qty = c.qty,
        current_qty = c.current_qty,
        timeframe_secs = c.timeframe_secs,
    )
}

/// Deterministic per-cycle economic identity. UUIDv5 of `run_id` +
/// `market_date` + `timeframe` + the conflict policy schema version + the
/// configured (requested) mode + the effective mode + the sorted set of
/// every candidate's full economic-identity seed
/// ([`candidate_identity_seed`]) this cycle.
///
/// AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 3 repair: the previous seed
/// covered only `(symbol, strategy_id, side, qty, current_qty, bar_end_ts)`
/// and omitted mode entirely, so the exact same candidates evaluated once
/// in `shadow` and again in `paper_enforced` collided on the same
/// `plan_id` — the DB then silently treated the enforced write as
/// `AlreadyExists` and preserved stale shadow-mode evidence. Every field
/// capable of changing the resolver's output or its truthful evidence
/// (mode, per-candidate `timeframe_secs`, full bar provenance including
/// close, and order semantics) is now included. Sorting makes the id
/// independent of dispatch order; omitting the loop-tick wall clock and
/// `decision_id` (which embeds wall-clock material — see
/// `decision::bar_result_to_decisions`) makes reprocessing the exact same
/// economic cycle on a later tick produce the exact same id.
pub fn compute_conflict_cycle_id(
    run_id: Uuid,
    market_date: &str,
    timeframe: &str,
    configured_mode: ConflictPolicyMode,
    effective_mode: ConflictPolicyMode,
    candidates: &[ConflictCandidateInput],
) -> String {
    let mut seeds: Vec<String> = candidates.iter().map(candidate_identity_seed).collect();
    seeds.sort();
    let candidates_str = seeds.join(",");
    let seed = format!(
        "mqk.strategy-conflict-policy-cycle.v2|{run_id}|{market_date}|{timeframe}|{schema}|\
         {configured}|{effective}|{candidates_str}",
        schema = mqk_portfolio::CONFLICT_POLICY_SCHEMA_VERSION,
        configured = configured_mode.as_str(),
        effective = effective_mode.as_str(),
    );
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
    pub timeframe: String,
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
}

fn candidate_inputs(
    decisions: &[PendingDecisionWithBarFacts],
    current_positions: &BTreeMap<String, i64>,
) -> Vec<ConflictCandidateInput> {
    // AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 2: one canonical symbol index
    // for the current-position lookup, built once, so a decision symbol's
    // casing (e.g. "aapl") can never read a held position (keyed "AAPL")
    // as flat. Mirrors the exact same `canonical_symbol` used for
    // grouping, bar-symbol comparison, and evidence inside
    // `mqk_portfolio::conflict_policy` — one normalization, every call
    // site.
    let canonical_positions: BTreeMap<String, i64> = current_positions
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
                .unwrap_or(0);
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
    current_positions: &BTreeMap<String, i64>,
) -> ConflictPolicyOutcome {
    if ctx.mode == ConflictPolicyMode::Off {
        return ConflictPolicyOutcome {
            decisions,
            plan: None,
        };
    }

    let candidates = candidate_inputs(&decisions, current_positions);
    let cycle_id = compute_conflict_cycle_id(
        ctx.run_id,
        &ctx.market_date,
        &ctx.timeframe,
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
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Phase C: durable evidence persistence (best-effort, never blocks the tick)
// ---------------------------------------------------------------------------

fn disposition_str(d: mqk_portfolio::ConflictDisposition) -> &'static str {
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

/// The single per-tick call site `loop_runner.rs` uses. Resolves the
/// effective mode (env + live-lock) and delegates to the pure
/// [`apply_conflict_policy`] above.
///
/// When the effective mode is `Off`, returns `decisions` untouched and
/// performs no candidate construction — the default configuration has zero
/// additional runtime cost.
pub async fn gather_and_resolve(
    state_arc: &Arc<AppState>,
    run_id: Uuid,
    now_micros: i64,
    market_date: String,
    timeframe: String,
    decisions: Vec<PendingDecisionWithBarFacts>,
    current_positions: &BTreeMap<String, i64>,
) -> ConflictPolicyOutcome {
    let resolution = crate::runtime_strategy_conflict_mode::resolve_conflict_policy_mode_from_env();
    let broker_kind = crate::state::BrokerKind::parse(state_arc.adapter_id());
    let eff = crate::runtime_strategy_conflict_mode::effective_mode(
        &resolution,
        state_arc.deployment_mode(),
        broker_kind,
    );

    if eff.effective_mode == ConflictPolicyMode::Off {
        return ConflictPolicyOutcome {
            decisions,
            plan: None,
        };
    }

    let ctx = ConflictPolicyContext {
        configured_mode: eff.configured_mode,
        mode: eff.effective_mode,
        run_id,
        market_date,
        timeframe,
        now_micros,
    };
    let outcome = apply_conflict_policy(&ctx, decisions, current_positions);
    persist_plan_if_present(
        state_arc,
        &outcome.plan,
        eff.configured_mode,
        eff.effective_mode,
        run_id,
        now_micros,
    )
    .await;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
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
            side: side.to_string(),
            qty,
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
        }
    }

    /// A sell with no bar facts at all -- structurally invalid after the
    /// Defect 2 repair.
    fn unbound_sell(symbol: &str, strategy_id: &str, qty: i64) -> PendingDecisionWithBarFacts {
        PendingDecisionWithBarFacts {
            decision: decision(symbol, strategy_id, "sell", qty),
            bar_facts: None,
        }
    }

    fn ctx(mode: ConflictPolicyMode) -> ConflictPolicyContext {
        ConflictPolicyContext {
            configured_mode: mode,
            mode,
            run_id: run_id(),
            market_date: "2026-07-26".to_string(),
            timeframe: TIMEFRAME.to_string(),
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
        current.insert("AAPL".to_string(), 0i64);
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
        current.insert("AAPL".to_string(), 0i64);
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
        current.insert("AAPL".to_string(), 0i64);
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
        current.insert("AAPL".to_string(), 20i64);
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
        current.insert("AAPL".to_string(), 0i64);
        current.insert("MSFT".to_string(), 0i64);
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
        current.insert("AAPL".to_string(), 20i64);
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
        current.insert("AAPL".to_string(), 20i64);
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
        current.insert("AAPL".to_string(), 0i64);
        current.insert("MSFT".to_string(), 0i64);
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
        current.insert("AAPL".to_string(), 0i64);
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
        current.insert("AAPL".to_string(), 0i64);
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
        current.insert("AAPL".to_string(), 0i64);
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
        current.insert("AAPL".to_string(), 0i64);
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
        current1.insert("AAPL".to_string(), 0i64);
        let mut current2 = BTreeMap::new();
        current2.insert("AAPL".to_string(), 5i64);
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
        current.insert("AAPL".to_string(), 0i64);
        current.insert("MSFT".to_string(), 0i64);
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
            qty: 10,
            current_qty: 0,
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
            TIMEFRAME,
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        let id2 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            TIMEFRAME,
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
            qty: 10,
            current_qty: 0,
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
            TIMEFRAME,
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        let enforced = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            TIMEFRAME,
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
            TIMEFRAME,
            ConflictPolicyMode::PaperEnforced, // configured
            ConflictPolicyMode::Off,           // live-locked down to Off
            &candidates,
        );
        let honest_off = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            TIMEFRAME,
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
            TIMEFRAME,
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        candidates[0].timeframe_secs = 900;
        let id2 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            TIMEFRAME,
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
            TIMEFRAME,
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        candidates[0].bar_timeframe = Some("1h".to_string());
        let id2 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            TIMEFRAME,
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
            TIMEFRAME,
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        candidates[0].close_micros = Some(200_000_000);
        let id2 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            TIMEFRAME,
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
            TIMEFRAME,
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
            TIMEFRAME,
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
            TIMEFRAME,
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &candidates,
        );
        candidates[0].order_type = "limit".to_string();
        candidates[0].limit_price = Some(50_000_000);
        let limit = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            TIMEFRAME,
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
            qty: 5,
            current_qty: 0,
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
            TIMEFRAME,
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &c0,
        );
        let id1 = compute_conflict_cycle_id(
            run_id(),
            "2026-07-26",
            TIMEFRAME,
            ConflictPolicyMode::Shadow,
            ConflictPolicyMode::Shadow,
            &c1,
        );
        assert_eq!(id0, id1);
    }
}
