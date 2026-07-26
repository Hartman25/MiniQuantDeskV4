//! RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase F — runtime batching/apply layer.
//!
//! The bridge between one tick's already-derived
//! [`crate::decision::InternalStrategyDecision`]s (one per symbol, produced
//! by the existing `bar_result_to_decisions`) and the pure
//! `mqk_portfolio` allocator/cycle model.
//!
//! `apply_runtime_opportunity_allocation` is the single call site Phase F
//! wires into `state/loop_runner.rs`, replacing "submit each symbol's
//! decisions as soon as they're derived" with "collect every symbol's
//! decisions for this tick, then submit" — everything else in the existing
//! per-symbol loop (caps #2/#4/#6, per-symbol target-state recording,
//! Discord alerts, dry-run diagnostics) is untouched.
//!
//! I/O-free itself: prices, current positions, the loaded opportunity
//! artifact, and the active durable snapshot are all supplied by the caller.

use std::collections::BTreeMap;

use uuid::Uuid;

use crate::decision::InternalStrategyDecision;
use crate::runtime_opportunity_artifact::LoadedRuntimeOpportunitySet;
use crate::runtime_opportunity_mode::RuntimeOpportunityAllocationMode;
use mqk_portfolio::{
    compute_allocation_cycle, AllocationCandidateInput, AllocationCycleContext,
    AllocationCycleResult, AllocationDisposition,
};

/// The one durable-snapshot fact this module needs — already validated by
/// the caller (`truth_state == "active"`, Paper+Alpaca+USD, real run_id;
/// Phase A Q4). This module only re-checks `equity_micros > 0` (via the
/// pure cycle model it delegates to).
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveSnapshotFacts {
    pub snapshot_id: String,
    pub equity_micros: i64,
}

/// Deterministic per-cycle identity: UUIDv5 of `run_id` + the shared tick
/// timestamp + the sorted set of symbols with a new/increasing-buy decision
/// this tick. Sorting the symbol set makes the id (and therefore the whole
/// downstream plan) independent of dispatch/artifact iteration order.
pub fn compute_cycle_id(run_id: Uuid, now_micros: i64, symbols: &[String]) -> String {
    let mut sorted = symbols.to_vec();
    sorted.sort();
    let seed = format!(
        "mqk.runtime-opportunity-allocation-cycle.v1|{run_id}|{now_micros}|{}",
        sorted.join(",")
    );
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, seed.as_bytes()).to_string()
}

pub struct RuntimeOpportunityAllocationContext {
    /// Already live-lock-resolved (see `runtime_opportunity_mode::effective_mode`).
    pub mode: RuntimeOpportunityAllocationMode,
    pub run_id: Uuid,
    pub market_date: String,
    pub timeframe: String,
    pub now_micros: i64,
    pub runtime_ceiling: usize,
    /// `None` when the artifact is not configured, missing, invalid, or stale.
    pub opportunity_set: Option<LoadedRuntimeOpportunitySet>,
    /// `None` when no valid (`truth_state == "active"`) durable snapshot exists.
    pub active_snapshot: Option<ActiveSnapshotFacts>,
}

pub const REASON_NO_OPPORTUNITY_AUTHORITY: &str = "fail_closed_no_opportunity_authority";
pub const REASON_NO_DURABLE_SNAPSHOT: &str = "fail_closed_no_durable_snapshot";
pub const REASON_NO_SCORE_FOR_SYMBOL: &str = "fail_closed_no_opportunity_score_for_symbol";

pub struct RuntimeOpportunityAllocationOutcome {
    /// The decisions to actually submit through the unchanged
    /// `submit_internal_strategy_decision` seam — sells always pass through;
    /// buys pass through unchanged (shadow), are clamped/dropped (paper
    /// enforced), or are entirely refused (missing authority).
    pub decisions: Vec<InternalStrategyDecision>,
    /// `None` when `mode == Off` or there were no buy-side decisions this
    /// tick to consider. `Some` (even an all-refused one) otherwise — this
    /// is the operator-visible/durable-evidence-worthy plan.
    pub plan: Option<AllocationCycleResult>,
}

fn rebuild_decision_with_qty(
    original: &InternalStrategyDecision,
    new_qty: i64,
    run_id: Uuid,
    now_micros: i64,
) -> InternalStrategyDecision {
    let decision_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!(
            "{run_id}:{}:{}:{}:{new_qty}:{now_micros}",
            original.strategy_id, original.symbol, original.side
        )
        .as_bytes(),
    )
    .to_string();
    InternalStrategyDecision {
        decision_id,
        strategy_id: original.strategy_id.clone(),
        symbol: original.symbol.clone(),
        timeframe_secs: original.timeframe_secs,
        side: original.side.clone(),
        qty: new_qty,
        order_type: original.order_type.clone(),
        time_in_force: original.time_in_force.clone(),
        limit_price: original.limit_price,
    }
}

fn fail_closed_plan(
    context: AllocationCycleContext,
    buy_decisions: &[InternalStrategyDecision],
    current_positions: &BTreeMap<String, i64>,
    reason: &str,
) -> AllocationCycleResult {
    // Reuse the pure cycle model's own fail-closed shape by feeding it
    // zero-price candidates only when we have no real authority to
    // evaluate against; every candidate is recorded as refused with a
    // context-specific reason rather than the cycle model's generic ones.
    let candidates: Vec<AllocationCandidateInput> = buy_decisions
        .iter()
        .map(|d| {
            let current = current_positions.get(&d.symbol).copied().unwrap_or(0);
            AllocationCandidateInput {
                symbol: d.symbol.clone(),
                strategy_id: d.strategy_id.clone(),
                score: 0.0,
                evaluation_price_micros: 0,
                current_qty: current,
                strategy_target_qty: current + d.qty,
            }
        })
        .collect();
    let mut result = compute_allocation_cycle(context, &candidates, 1);
    result.truth_state = reason.to_string();
    for c in result.candidates.iter_mut() {
        c.reason_code = reason.to_string();
        c.disposition = AllocationDisposition::RefusedFailClosed;
    }
    result.blockers = vec![reason.to_string()];
    result
}

/// Apply Bundle 5 opportunity allocation to one tick's already-derived
/// decisions.
///
/// `current_positions` and `price_by_symbol` are keyed by symbol; a missing
/// entry in `price_by_symbol` is treated as "no price available" (fails
/// closed for that candidate only, via the pure cycle model).
pub fn apply_runtime_opportunity_allocation(
    ctx: &RuntimeOpportunityAllocationContext,
    decisions: Vec<InternalStrategyDecision>,
    current_positions: &BTreeMap<String, i64>,
    price_by_symbol: &BTreeMap<String, i64>,
) -> RuntimeOpportunityAllocationOutcome {
    if ctx.mode == RuntimeOpportunityAllocationMode::Off {
        return RuntimeOpportunityAllocationOutcome {
            decisions,
            plan: None,
        };
    }

    let (buy_decisions, mut other_decisions): (Vec<_>, Vec<_>) = decisions
        .into_iter()
        .partition(|d| d.side.eq_ignore_ascii_case("buy"));

    if buy_decisions.is_empty() {
        return RuntimeOpportunityAllocationOutcome {
            decisions: other_decisions,
            plan: None,
        };
    }

    let symbols: Vec<String> = buy_decisions.iter().map(|d| d.symbol.clone()).collect();
    let cycle_id = compute_cycle_id(ctx.run_id, ctx.now_micros, &symbols);

    let context = AllocationCycleContext {
        cycle_id,
        run_id: ctx.run_id.to_string(),
        market_date: ctx.market_date.clone(),
        timeframe: ctx.timeframe.clone(),
        opportunity_artifact_id: ctx
            .opportunity_set
            .as_ref()
            .map(|a| a.artifact_id.clone())
            .unwrap_or_default(),
        source_snapshot_id: ctx
            .active_snapshot
            .as_ref()
            .map(|s| s.snapshot_id.clone())
            .unwrap_or_default(),
        equity_micros: ctx
            .active_snapshot
            .as_ref()
            .map(|s| s.equity_micros)
            .unwrap_or(0),
    };

    let (Some(opportunity_set), Some(active_snapshot)) =
        (&ctx.opportunity_set, &ctx.active_snapshot)
    else {
        let reason = if ctx.opportunity_set.is_none() {
            REASON_NO_OPPORTUNITY_AUTHORITY
        } else {
            REASON_NO_DURABLE_SNAPSHOT
        };
        let plan = fail_closed_plan(context, &buy_decisions, current_positions, reason);
        // Missing authority refuses every buy this cycle; sells are untouched.
        return RuntimeOpportunityAllocationOutcome {
            decisions: other_decisions,
            plan: Some(plan),
        };
    };
    let _ = active_snapshot; // used via `context.equity_micros` above

    let mut candidates: Vec<AllocationCandidateInput> = Vec::new();
    let mut uncovered: Vec<InternalStrategyDecision> = Vec::new();
    for d in &buy_decisions {
        let current = current_positions.get(&d.symbol).copied().unwrap_or(0);
        match opportunity_set
            .candidates
            .iter()
            .find(|c| c.symbol == d.symbol)
        {
            Some(oc) => candidates.push(AllocationCandidateInput {
                symbol: d.symbol.clone(),
                strategy_id: d.strategy_id.clone(),
                score: oc.score,
                evaluation_price_micros: price_by_symbol.get(&d.symbol).copied().unwrap_or(0),
                current_qty: current,
                strategy_target_qty: current + d.qty,
            }),
            None => uncovered.push(d.clone()),
        }
    }

    let mut plan = compute_allocation_cycle(context, &candidates, ctx.runtime_ceiling);
    for d in &uncovered {
        let current = current_positions.get(&d.symbol).copied().unwrap_or(0);
        plan.candidates
            .push(mqk_portfolio::AllocationCandidateResult {
                symbol: d.symbol.clone(),
                strategy_id: d.strategy_id.clone(),
                input_score: 0.0,
                target_weight: 0.0,
                current_qty: current,
                strategy_target_qty: current + d.qty,
                allocation_target_qty: current,
                final_target_qty: current,
                disposition: AllocationDisposition::RefusedFailClosed,
                reason_code: REASON_NO_SCORE_FOR_SYMBOL.to_string(),
                evaluation_price_micros: price_by_symbol.get(&d.symbol).copied().unwrap_or(0),
            });
    }

    match ctx.mode {
        RuntimeOpportunityAllocationMode::Off => unreachable!("handled above"),
        RuntimeOpportunityAllocationMode::Shadow => {
            // Zero allocator-driven outbox changes: original buy decisions
            // pass through exactly as the strategy computed them.
            other_decisions.extend(buy_decisions);
        }
        RuntimeOpportunityAllocationMode::PaperEnforced => {
            for d in buy_decisions {
                let Some(result) = plan.candidates.iter().find(|c| c.symbol == d.symbol) else {
                    continue; // defensive: every buy decision has a plan entry by construction
                };
                let delta = result.buy_delta();
                if delta > 0 {
                    other_decisions.push(rebuild_decision_with_qty(
                        &d,
                        delta,
                        ctx.run_id,
                        ctx.now_micros,
                    ));
                }
                // delta == 0 -> no capital assigned; decision is dropped
                // (no submission, no trade this cycle).
            }
        }
    }

    RuntimeOpportunityAllocationOutcome {
        decisions: other_decisions,
        plan: Some(plan),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_opportunity_artifact::LoadedRuntimeOpportunityCandidate;

    fn run_id() -> Uuid {
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test-run")
    }

    fn buy(symbol: &str, strategy_id: &str, qty: i64) -> InternalStrategyDecision {
        InternalStrategyDecision {
            decision_id: format!("buy-{symbol}"),
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs: 300,
            side: "buy".to_string(),
            qty,
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
        }
    }

    fn sell(symbol: &str, strategy_id: &str, qty: i64) -> InternalStrategyDecision {
        InternalStrategyDecision {
            decision_id: format!("sell-{symbol}"),
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs: 300,
            side: "sell".to_string(),
            qty,
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
        }
    }

    fn opportunity_set(candidates: Vec<(&str, &str, f64)>) -> LoadedRuntimeOpportunitySet {
        LoadedRuntimeOpportunitySet {
            artifact_id: "artifact-1".to_string(),
            market_date: "2026-07-26".to_string(),
            timeframe: "5m".to_string(),
            source_watchlist_hash: "hash-1".to_string(),
            candidates: candidates
                .into_iter()
                .map(|(sym, strat, score)| LoadedRuntimeOpportunityCandidate {
                    symbol: sym.to_string(),
                    strategy_id: strat.to_string(),
                    score,
                    candidate_artifact_id: format!("cand-{sym}"),
                })
                .collect(),
        }
    }

    fn base_ctx(mode: RuntimeOpportunityAllocationMode) -> RuntimeOpportunityAllocationContext {
        RuntimeOpportunityAllocationContext {
            mode,
            run_id: run_id(),
            market_date: "2026-07-26".to_string(),
            timeframe: "5m".to_string(),
            now_micros: 1_000_000,
            runtime_ceiling: 5,
            opportunity_set: Some(opportunity_set(vec![
                ("AAPL", "intraday_scalper", 0.9),
                ("MSFT", "intraday_scalper", 0.1),
            ])),
            active_snapshot: Some(ActiveSnapshotFacts {
                snapshot_id: "snap-1".to_string(),
                equity_micros: 100_000 * 1_000_000,
            }),
        }
    }

    #[test]
    fn off_mode_passes_through_unchanged_and_no_plan() {
        let decisions = vec![
            buy("AAPL", "intraday_scalper", 10),
            sell("MSFT", "intraday_scalper", 5),
        ];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::Off),
            decisions.clone(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(out.plan.is_none());
        assert_eq!(out.decisions.len(), 2);
        assert_eq!(out.decisions[0].qty, 10);
    }

    #[test]
    fn no_buy_decisions_this_tick_produces_no_plan() {
        let decisions = vec![sell("MSFT", "intraday_scalper", 5)];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::Shadow),
            decisions,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(out.plan.is_none());
        assert_eq!(out.decisions.len(), 1);
    }

    #[test]
    fn sell_decisions_always_pass_through_in_shadow_and_enforced() {
        for mode in [
            RuntimeOpportunityAllocationMode::Shadow,
            RuntimeOpportunityAllocationMode::PaperEnforced,
        ] {
            let mut current = BTreeMap::new();
            current.insert("AAPL".to_string(), 0i64);
            let mut prices = BTreeMap::new();
            prices.insert("AAPL".to_string(), 100_000_000i64);
            let decisions = vec![
                buy("AAPL", "intraday_scalper", 10),
                sell("TLT", "intraday_scalper", 3),
            ];
            let out =
                apply_runtime_opportunity_allocation(&base_ctx(mode), decisions, &current, &prices);
            assert!(
                out.decisions
                    .iter()
                    .any(|d| d.symbol == "TLT" && d.side == "sell" && d.qty == 3),
                "sell must pass through unchanged in {mode:?}"
            );
        }
    }

    #[test]
    fn shadow_mode_produces_plan_but_leaves_buy_qty_unchanged() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        let mut prices = BTreeMap::new();
        prices.insert("AAPL".to_string(), 100_000_000i64);
        let decisions = vec![buy("AAPL", "intraday_scalper", 10)];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::Shadow),
            decisions,
            &current,
            &prices,
        );
        assert!(out.plan.is_some());
        assert_eq!(out.decisions.len(), 1);
        assert_eq!(
            out.decisions[0].qty, 10,
            "shadow must not alter submitted qty"
        );
    }

    #[test]
    fn paper_enforced_clamps_buy_to_allocator_output() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        let mut prices = BTreeMap::new();
        // Expensive price -> allocator's 20% single-position cap on $100k
        // equity ($20,000) funds far fewer shares than the strategy's
        // target of 10,000.
        prices.insert("AAPL".to_string(), 10_000_000_000i64); // $10,000/share
        let decisions = vec![buy("AAPL", "intraday_scalper", 10_000)];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::PaperEnforced),
            decisions,
            &current,
            &prices,
        );
        assert_eq!(out.decisions.len(), 1);
        assert!(
            out.decisions[0].qty < 10_000,
            "expected clamp, got qty={}",
            out.decisions[0].qty
        );
        assert!(out.decisions[0].qty > 0);
    }

    #[test]
    fn paper_enforced_drops_decision_when_no_capital_available() {
        // MSFT has a much lower score than AAPL and a tight ceiling=1 means
        // MSFT gets zero capital.
        let mut ctx = base_ctx(RuntimeOpportunityAllocationMode::PaperEnforced);
        ctx.runtime_ceiling = 1;
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        current.insert("MSFT".to_string(), 0i64);
        let mut prices = BTreeMap::new();
        prices.insert("AAPL".to_string(), 100_000_000i64);
        prices.insert("MSFT".to_string(), 100_000_000i64);
        let decisions = vec![
            buy("AAPL", "intraday_scalper", 10),
            buy("MSFT", "intraday_scalper", 10),
        ];
        let out = apply_runtime_opportunity_allocation(&ctx, decisions, &current, &prices);
        assert!(out.decisions.iter().any(|d| d.symbol == "AAPL"));
        assert!(
            !out.decisions.iter().any(|d| d.symbol == "MSFT"),
            "MSFT should be dropped, not submitted with qty=0"
        );
    }

    #[test]
    fn missing_opportunity_artifact_refuses_all_buys_but_not_sells() {
        let mut ctx = base_ctx(RuntimeOpportunityAllocationMode::PaperEnforced);
        ctx.opportunity_set = None;
        let decisions = vec![
            buy("AAPL", "intraday_scalper", 10),
            sell("TLT", "intraday_scalper", 3),
        ];
        let out = apply_runtime_opportunity_allocation(
            &ctx,
            decisions,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(out.decisions.iter().any(|d| d.symbol == "TLT"));
        assert!(!out.decisions.iter().any(|d| d.symbol == "AAPL"));
        assert_eq!(
            out.plan.unwrap().truth_state,
            REASON_NO_OPPORTUNITY_AUTHORITY
        );
    }

    #[test]
    fn missing_durable_snapshot_refuses_all_buys() {
        let mut ctx = base_ctx(RuntimeOpportunityAllocationMode::PaperEnforced);
        ctx.active_snapshot = None;
        let decisions = vec![buy("AAPL", "intraday_scalper", 10)];
        let out = apply_runtime_opportunity_allocation(
            &ctx,
            decisions,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(out.decisions.is_empty());
        assert_eq!(out.plan.unwrap().truth_state, REASON_NO_DURABLE_SNAPSHOT);
    }

    #[test]
    fn symbol_not_in_opportunity_set_is_refused_not_fabricated() {
        let mut current = BTreeMap::new();
        current.insert("ZZZZ".to_string(), 0i64);
        let mut prices = BTreeMap::new();
        prices.insert("ZZZZ".to_string(), 100_000_000i64);
        let decisions = vec![buy("ZZZZ", "intraday_scalper", 10)];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::PaperEnforced),
            decisions,
            &current,
            &prices,
        );
        assert!(out.decisions.is_empty());
        let plan = out.plan.unwrap();
        let zzzz = plan.candidates.iter().find(|c| c.symbol == "ZZZZ").unwrap();
        assert_eq!(zzzz.reason_code, REASON_NO_SCORE_FOR_SYMBOL);
    }

    #[test]
    fn one_allocator_call_per_tick_not_per_symbol() {
        // Two buy candidates in one call; the resulting plan must carry
        // both under one cycle_id (proving a single compute_allocation_cycle
        // call handled the whole batch, not one call per symbol).
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        current.insert("MSFT".to_string(), 0i64);
        let mut prices = BTreeMap::new();
        prices.insert("AAPL".to_string(), 100_000_000i64);
        prices.insert("MSFT".to_string(), 100_000_000i64);
        let decisions = vec![
            buy("AAPL", "intraday_scalper", 10),
            buy("MSFT", "intraday_scalper", 10),
        ];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::Shadow),
            decisions,
            &current,
            &prices,
        );
        let plan = out.plan.unwrap();
        assert_eq!(plan.candidates.len(), 2);
    }

    #[test]
    fn cycle_id_is_deterministic_and_order_independent() {
        let id1 = compute_cycle_id(run_id(), 42, &["AAPL".to_string(), "MSFT".to_string()]);
        let id2 = compute_cycle_id(run_id(), 42, &["MSFT".to_string(), "AAPL".to_string()]);
        assert_eq!(id1, id2);
    }
}
