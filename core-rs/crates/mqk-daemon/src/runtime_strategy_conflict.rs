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

use std::collections::BTreeMap;
use std::sync::Arc;

use uuid::Uuid;

use crate::runtime_opportunity_allocation::PendingDecisionWithBarFacts;
use crate::runtime_strategy_conflict_mode::ConflictPolicyMode;
use crate::state::AppState;
use mqk_portfolio::{
    resolve_conflict_cycle, ConflictCandidateInput, ConflictCycleContext, ConflictCycleResult,
};

/// Deterministic per-cycle economic identity (mirrors
/// `runtime_opportunity_allocation::compute_cycle_id`'s Phase B repair
/// exactly): UUIDv5 of `run_id` + `market_date` + `timeframe` + the conflict
/// policy schema version + the sorted
/// `(symbol, strategy_id, side, qty, current_qty, bar_end_ts)` tuple set of
/// every candidate this cycle. Sorting makes the id independent of dispatch
/// order; omitting the loop-tick wall clock and `decision_id` (which embeds
/// wall-clock material — see `decision::bar_result_to_decisions`) makes
/// reprocessing the exact same economic cycle on a later tick produce the
/// exact same id. `current_qty`/`qty` together fully determine each
/// candidate's proposed target deterministically, so a change to either is
/// already captured without needing the (derived) proposed target itself in
/// the seed.
pub fn compute_conflict_cycle_id(
    run_id: Uuid,
    market_date: &str,
    timeframe: &str,
    candidates: &[ConflictCandidateInput],
) -> String {
    let mut tuples: Vec<(String, String, String, i64, i64, i64)> = candidates
        .iter()
        .map(|c| {
            (
                c.symbol.trim().to_string(),
                c.strategy_id.trim().to_string(),
                c.side.trim().to_ascii_lowercase(),
                c.qty,
                c.current_qty,
                c.bar_end_ts.unwrap_or(i64::MIN),
            )
        })
        .collect();
    tuples.sort();
    let candidates_str = tuples
        .iter()
        .map(|(sym, sid, side, qty, cur, bar)| format!("{sym}:{sid}:{side}:{qty}:{cur}:{bar}"))
        .collect::<Vec<_>>()
        .join(",");
    let seed = format!(
        "mqk.strategy-conflict-policy-cycle.v1|{run_id}|{market_date}|{timeframe}|{}|{candidates_str}",
        mqk_portfolio::CONFLICT_POLICY_SCHEMA_VERSION
    );
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, seed.as_bytes()).to_string()
}

pub struct ConflictPolicyContext {
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
    timeframe: &str,
    current_positions: &BTreeMap<String, i64>,
) -> Vec<ConflictCandidateInput> {
    decisions
        .iter()
        .enumerate()
        .map(|(ordinal, p)| {
            let current_qty = current_positions
                .get(&p.decision.symbol)
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
                timeframe: timeframe.to_string(),
                timeframe_secs: p.decision.timeframe_secs,
                side: p.decision.side.clone(),
                qty: p.decision.qty,
                current_qty,
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

    let candidates = candidate_inputs(&decisions, &ctx.timeframe, current_positions);
    let cycle_id =
        compute_conflict_cycle_id(ctx.run_id, &ctx.market_date, &ctx.timeframe, &candidates);
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
        mode: eff.effective_mode,
        run_id,
        market_date,
        timeframe,
        now_micros,
    };
    apply_conflict_policy(&ctx, decisions, current_positions)
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

    fn unbound_sell(symbol: &str, strategy_id: &str, qty: i64) -> PendingDecisionWithBarFacts {
        PendingDecisionWithBarFacts {
            decision: decision(symbol, strategy_id, "sell", qty),
            bar_facts: None,
        }
    }

    fn ctx(mode: ConflictPolicyMode) -> ConflictPolicyContext {
        ConflictPolicyContext {
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
            unbound_sell("AAPL", "s2", 5),
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
            timeframe: TIMEFRAME.to_string(),
            timeframe_secs: 300,
            side: "buy".to_string(),
            qty: 10,
            current_qty: 0,
            bar_symbol: Some("AAPL".to_string()),
            bar_strategy_id: Some("s1".to_string()),
            bar_timeframe: Some(TIMEFRAME.to_string()),
            bar_end_ts: Some(1_000),
            close_micros: Some(100_000_000),
        }];
        let id1 = compute_conflict_cycle_id(run_id(), "2026-07-26", TIMEFRAME, &candidates);
        let id2 = compute_conflict_cycle_id(run_id(), "2026-07-26", TIMEFRAME, &candidates);
        assert_eq!(id1, id2);
    }
}
