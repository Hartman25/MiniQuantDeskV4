//! mqk-portfolio: allocation cycle
//!
//! RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase D — pure allocation-cycle model.
//!
//! Composes one same-cycle set of new/increasing-buy candidates through
//! [`crate::allocator::Allocator::allocate_long_only`] and produces a fully
//! explained, deterministic result: per-candidate clamped target quantities
//! plus a bounded, closed disposition/reason-code vocabulary.
//!
//! Pure: no IO, no DB, no clock, no env, no randomness, no new dependency —
//! `cycle_id` and every other identity/timestamp field is supplied by the
//! caller (mqk-daemon), which owns the uuid/clock plumbing this
//! zero-dependency crate intentionally does not carry.
//!
//! # Scope (frozen by docs/specs/runtime_opportunity_allocation_01a)
//! - Callers must pre-filter to only new/increasing-buy candidates
//!   (strategy_target_qty > current_qty). Reducing/exit/flatten decisions
//!   never enter this model — that separation happens upstream, in the
//!   daemon's per-cycle dispatch loop (Phase F), before this function is
//!   called.
//! - `final_target_qty` is always in `[current_qty, strategy_target_qty]` —
//!   allocation can only narrow a buy down toward "no trade", never widen
//!   it past the strategy's own target, and never flip it into a sell.

use std::collections::BTreeMap;

use crate::allocator::{AllocationConstraints, Allocator, Candidate, RejectionReason};

// ---------------------------------------------------------------------------
// Reason codes — bounded, closed vocabulary
// ---------------------------------------------------------------------------

pub const REASON_ALLOCATOR_FULL_TARGET_GRANTED: &str = "allocator_full_target_granted";
pub const REASON_ALLOCATOR_CLAMPED_TO_AVAILABLE_CAPITAL: &str =
    "allocator_clamped_to_available_capital";
pub const REASON_ALLOCATOR_ZERO_CAPITAL_AVAILABLE: &str = "allocator_zero_capital_available";
pub const REASON_ALLOCATOR_MAX_POSITIONS_REACHED: &str = "allocator_max_positions_reached";
pub const REASON_ALLOCATOR_SCALED_TO_ZERO: &str = "allocator_scaled_to_zero";
pub const REASON_ALLOCATOR_NET_WEIGHT_CAP_EXCEEDED: &str = "allocator_net_weight_cap_exceeded";
pub const REASON_NONPOSITIVE_OR_MISSING_PRICE: &str = "nonpositive_or_missing_price";
pub const REASON_INVALID_SCORE: &str = "invalid_score";
pub const REASON_NOT_AN_INCREASE: &str = "not_an_increase";
pub const REASON_FAIL_CLOSED_NONPOSITIVE_EQUITY: &str = "fail_closed_nonpositive_equity";
pub const REASON_FAIL_CLOSED_DUPLICATE_SYMBOL: &str = "fail_closed_duplicate_symbol_in_cycle";
pub const REASON_FAIL_CLOSED_ALLOCATOR_ERROR: &str = "fail_closed_allocator_error";
pub const REASON_ALLOCATOR_OUTPUT_MISSING_CANDIDATE: &str = "allocator_output_missing_candidate";

pub const TRUTH_STATE_COMPUTED: &str = "computed";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Immutable facts about one allocation cycle, supplied by the caller.
#[derive(Clone, Debug, PartialEq)]
pub struct AllocationCycleContext {
    /// Deterministic per-cycle identity (caller-minted, e.g. UUIDv5 of
    /// `run_id` + shared tick timestamp + sorted dispatched-symbol set).
    pub cycle_id: String,
    pub run_id: String,
    /// `"YYYY-MM-DD"`.
    pub market_date: String,
    pub timeframe: String,
    /// Identity/hash of the `runtime-opportunity-set-v1` artifact this
    /// cycle's scores were sourced from.
    pub opportunity_artifact_id: String,
    /// Identity of the durable `PaperPortfolioSnapshot` row this cycle's
    /// `equity_micros` was sourced from (must be `truth_state == "active"`
    /// — enforced by the caller before construction; this module trusts the
    /// value it is given and only guards `equity_micros > 0`).
    pub source_snapshot_id: String,
    pub equity_micros: i64,
}

/// One new/increasing-buy candidate under consideration this cycle.
///
/// Callers must only construct this for candidates already classified as a
/// strict increase (`strategy_target_qty > current_qty`); anything else
/// (hold, reduce, exit, flatten) must never reach this type.
#[derive(Clone, Debug, PartialEq)]
pub struct AllocationCandidateInput {
    pub symbol: String,
    pub strategy_id: String,
    /// Canonical, positive score sourced from the opportunity artifact.
    pub score: f64,
    /// Latest completed-bar close price, in micros.
    pub evaluation_price_micros: i64,
    pub current_qty: i64,
    pub strategy_target_qty: i64,
}

/// Closed disposition vocabulary for one candidate's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocationDisposition {
    /// `final_target_qty == strategy_target_qty` — full strategy intent granted.
    Allowed,
    /// `current_qty < final_target_qty < strategy_target_qty` — narrowed.
    ClampedDown,
    /// `final_target_qty == current_qty` — no capital assigned; no trade.
    RefusedNoCapital,
    /// Excluded before reaching the allocator (bad price/score/context) or
    /// the whole cycle failed closed.
    RefusedFailClosed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AllocationCandidateResult {
    pub symbol: String,
    pub strategy_id: String,
    pub input_score: f64,
    pub target_weight: f64,
    pub current_qty: i64,
    pub strategy_target_qty: i64,
    /// Whole-share quantity implied by `target_weight * equity_micros /
    /// evaluation_price_micros`, floored (conservative — never rounds up).
    pub allocation_target_qty: i64,
    /// `allocation_target_qty` clamped to `[current_qty, strategy_target_qty]`.
    pub final_target_qty: i64,
    pub disposition: AllocationDisposition,
    pub reason_code: String,
    pub evaluation_price_micros: i64,
}

impl AllocationCandidateResult {
    /// Net quantity this candidate would buy, if submitted as-is.
    pub fn buy_delta(&self) -> i64 {
        self.final_target_qty - self.current_qty
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AllocationCycleResult {
    pub context: AllocationCycleContext,
    /// Sorted by symbol ascending — deterministic regardless of the input
    /// candidate slice's order.
    pub candidates: Vec<AllocationCandidateResult>,
    pub gross_weight: f64,
    pub net_weight: f64,
    /// `"computed"` when the allocator ran (individual candidates may still
    /// be `RefusedFailClosed` for their own data reasons); `"fail_closed_*"`
    /// when a cycle-level precondition failed before the allocator ran.
    pub truth_state: String,
    pub blockers: Vec<String>,
}

// ---------------------------------------------------------------------------
// Compute
// ---------------------------------------------------------------------------

fn fail_closed_all(
    context: AllocationCycleContext,
    candidates: &[AllocationCandidateInput],
    truth_state: &str,
    blocker: String,
) -> AllocationCycleResult {
    let results = candidates
        .iter()
        .map(|c| AllocationCandidateResult {
            symbol: c.symbol.clone(),
            strategy_id: c.strategy_id.clone(),
            input_score: c.score,
            target_weight: 0.0,
            current_qty: c.current_qty,
            strategy_target_qty: c.strategy_target_qty,
            allocation_target_qty: c.current_qty,
            final_target_qty: c.current_qty,
            disposition: AllocationDisposition::RefusedFailClosed,
            reason_code: truth_state.to_string(),
            evaluation_price_micros: c.evaluation_price_micros,
        })
        .collect();
    AllocationCycleResult {
        context,
        candidates: results,
        gross_weight: 0.0,
        net_weight: 0.0,
        truth_state: truth_state.to_string(),
        blockers: vec![blocker],
    }
}

fn rejection_reason_code(reason: &RejectionReason) -> &'static str {
    match reason {
        RejectionReason::MaxPositionsReached => REASON_ALLOCATOR_MAX_POSITIONS_REACHED,
        RejectionReason::ScaledToZero => REASON_ALLOCATOR_SCALED_TO_ZERO,
        RejectionReason::NetWeightCapExceeded => REASON_ALLOCATOR_NET_WEIGHT_CAP_EXCEEDED,
    }
}

/// Compute one allocation cycle: run every eligible new/increasing-buy
/// candidate through the long-only allocator seam and clamp each strategy
/// target to what the allocator actually grants.
///
/// `runtime_ceiling` is the hard cap on concurrent positions (e.g. the
/// watchlist-v2 `MULTI_SYMBOL_HARD_CEILING`).
///
/// Deterministic: identical inputs (any candidate order) always produce a
/// bit-identical [`AllocationCycleResult`] (candidates sorted by symbol in
/// the output; the allocator itself is already input-order-independent).
pub fn compute_allocation_cycle(
    context: AllocationCycleContext,
    candidates: &[AllocationCandidateInput],
    runtime_ceiling: usize,
) -> AllocationCycleResult {
    if context.equity_micros <= 0 {
        return fail_closed_all(
            context.clone(),
            candidates,
            REASON_FAIL_CLOSED_NONPOSITIVE_EQUITY,
            format!(
                "cycle equity_micros={} is not positive; refusing all new/increasing buys",
                context.equity_micros
            ),
        );
    }

    // Fail closed the whole cycle on a duplicate symbol (Q10) — never pick
    // one arbitrarily.
    {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for c in candidates {
            if !seen.insert(c.symbol.as_str()) {
                return fail_closed_all(
                    context.clone(),
                    candidates,
                    REASON_FAIL_CLOSED_DUPLICATE_SYMBOL,
                    format!(
                        "symbol '{}' appears more than once in one cycle's candidate set",
                        c.symbol
                    ),
                );
            }
        }
    }

    // Per-candidate data-quality gate: only genuinely-eligible candidates
    // reach the allocator. Ineligible ones are recorded as
    // RefusedFailClosed with a specific reason, but do not fail the cycle.
    let mut results: BTreeMap<String, AllocationCandidateResult> = BTreeMap::new();
    let mut eligible: Vec<&AllocationCandidateInput> = Vec::new();

    for c in candidates {
        let ineligible_reason = if c.evaluation_price_micros <= 0 {
            Some(REASON_NONPOSITIVE_OR_MISSING_PRICE)
        } else if !c.score.is_finite() || c.score <= 0.0 {
            Some(REASON_INVALID_SCORE)
        } else if c.strategy_target_qty <= c.current_qty {
            Some(REASON_NOT_AN_INCREASE)
        } else {
            None
        };

        if let Some(reason) = ineligible_reason {
            results.insert(
                c.symbol.clone(),
                AllocationCandidateResult {
                    symbol: c.symbol.clone(),
                    strategy_id: c.strategy_id.clone(),
                    input_score: c.score,
                    target_weight: 0.0,
                    current_qty: c.current_qty,
                    strategy_target_qty: c.strategy_target_qty,
                    allocation_target_qty: c.current_qty,
                    final_target_qty: c.current_qty,
                    disposition: AllocationDisposition::RefusedFailClosed,
                    reason_code: reason.to_string(),
                    evaluation_price_micros: c.evaluation_price_micros,
                },
            );
        } else {
            eligible.push(c);
        }
    }

    if eligible.is_empty() {
        return AllocationCycleResult {
            context,
            candidates: results.into_values().collect(),
            gross_weight: 0.0,
            net_weight: 0.0,
            truth_state: TRUTH_STATE_COMPUTED.to_string(),
            blockers: vec![],
        };
    }

    let alloc_candidates: Vec<Candidate> = eligible
        .iter()
        .map(|c| Candidate::new(c.symbol.clone(), c.score))
        .collect();

    let allocator = Allocator::new(AllocationConstraints::long_only_standard());
    let decision = match allocator.allocate_long_only(
        context.equity_micros,
        &alloc_candidates,
        runtime_ceiling,
    ) {
        Ok(d) => d,
        Err(err) => {
            return fail_closed_all(
                context.clone(),
                candidates,
                REASON_FAIL_CLOSED_ALLOCATOR_ERROR,
                format!("allocator refused this cycle's candidate set: {err}"),
            );
        }
    };

    for c in &eligible {
        if let Some(&weight) = decision.weights.get(&c.symbol) {
            let notional_cap = weight * context.equity_micros as f64;
            let allocation_target_qty =
                (notional_cap / c.evaluation_price_micros as f64).floor() as i64;
            let allocation_target_qty = allocation_target_qty.max(0);
            let final_target_qty =
                allocation_target_qty.clamp(c.current_qty, c.strategy_target_qty);
            let (disposition, reason_code) = if final_target_qty == c.strategy_target_qty {
                (
                    AllocationDisposition::Allowed,
                    REASON_ALLOCATOR_FULL_TARGET_GRANTED,
                )
            } else if final_target_qty > c.current_qty {
                (
                    AllocationDisposition::ClampedDown,
                    REASON_ALLOCATOR_CLAMPED_TO_AVAILABLE_CAPITAL,
                )
            } else {
                (
                    AllocationDisposition::RefusedNoCapital,
                    REASON_ALLOCATOR_ZERO_CAPITAL_AVAILABLE,
                )
            };
            results.insert(
                c.symbol.clone(),
                AllocationCandidateResult {
                    symbol: c.symbol.clone(),
                    strategy_id: c.strategy_id.clone(),
                    input_score: c.score,
                    target_weight: weight,
                    current_qty: c.current_qty,
                    strategy_target_qty: c.strategy_target_qty,
                    allocation_target_qty,
                    final_target_qty,
                    disposition,
                    reason_code: reason_code.to_string(),
                    evaluation_price_micros: c.evaluation_price_micros,
                },
            );
        } else if let Some(rejected) = decision.rejected.iter().find(|r| r.symbol == c.symbol) {
            results.insert(
                c.symbol.clone(),
                AllocationCandidateResult {
                    symbol: c.symbol.clone(),
                    strategy_id: c.strategy_id.clone(),
                    input_score: c.score,
                    target_weight: 0.0,
                    current_qty: c.current_qty,
                    strategy_target_qty: c.strategy_target_qty,
                    allocation_target_qty: c.current_qty,
                    final_target_qty: c.current_qty,
                    disposition: AllocationDisposition::RefusedNoCapital,
                    reason_code: rejection_reason_code(&rejected.reason).to_string(),
                    evaluation_price_micros: c.evaluation_price_micros,
                },
            );
        } else {
            // Defensive: the allocator's output must account for every
            // input candidate either as a weight or a rejection. If it
            // somehow does not, fail closed for this symbol rather than
            // silently drop it or assume a default.
            results.insert(
                c.symbol.clone(),
                AllocationCandidateResult {
                    symbol: c.symbol.clone(),
                    strategy_id: c.strategy_id.clone(),
                    input_score: c.score,
                    target_weight: 0.0,
                    current_qty: c.current_qty,
                    strategy_target_qty: c.strategy_target_qty,
                    allocation_target_qty: c.current_qty,
                    final_target_qty: c.current_qty,
                    disposition: AllocationDisposition::RefusedFailClosed,
                    reason_code: REASON_ALLOCATOR_OUTPUT_MISSING_CANDIDATE.to_string(),
                    evaluation_price_micros: c.evaluation_price_micros,
                },
            );
        }
    }

    AllocationCycleResult {
        context,
        candidates: results.into_values().collect(),
        gross_weight: decision.gross_weight,
        net_weight: decision.net_weight,
        truth_state: TRUTH_STATE_COMPUTED.to_string(),
        blockers: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAV: i64 = 100_000 * 1_000_000; // $100,000 in micros

    fn ctx() -> AllocationCycleContext {
        AllocationCycleContext {
            cycle_id: "cycle-1".to_string(),
            run_id: "run-1".to_string(),
            market_date: "2026-07-26".to_string(),
            timeframe: "5m".to_string(),
            opportunity_artifact_id: "artifact-1".to_string(),
            source_snapshot_id: "snapshot-1".to_string(),
            equity_micros: NAV,
        }
    }

    fn buy(
        symbol: &str,
        score: f64,
        price_micros: i64,
        current: i64,
        target: i64,
    ) -> AllocationCandidateInput {
        AllocationCandidateInput {
            symbol: symbol.to_string(),
            strategy_id: "intraday_scalper".to_string(),
            score,
            evaluation_price_micros: price_micros,
            current_qty: current,
            strategy_target_qty: target,
        }
    }

    fn result_for<'a>(
        res: &'a AllocationCycleResult,
        symbol: &str,
    ) -> &'a AllocationCandidateResult {
        res.candidates
            .iter()
            .find(|c| c.symbol == symbol)
            .unwrap_or_else(|| panic!("no result for {symbol}"))
    }

    #[test]
    fn nonpositive_equity_fails_closed_all() {
        let mut c = ctx();
        c.equity_micros = 0;
        let res = compute_allocation_cycle(c, &[buy("AAPL", 0.5, 100_000_000, 0, 10)], 5);
        assert_eq!(res.truth_state, REASON_FAIL_CLOSED_NONPOSITIVE_EQUITY);
        assert_eq!(result_for(&res, "AAPL").final_target_qty, 0);
        assert_eq!(
            result_for(&res, "AAPL").disposition,
            AllocationDisposition::RefusedFailClosed
        );
    }

    #[test]
    fn duplicate_symbol_in_cycle_fails_closed_all() {
        let candidates = vec![
            buy("AAPL", 0.5, 100_000_000, 0, 10),
            buy("AAPL", 0.9, 100_000_000, 0, 20),
        ];
        let res = compute_allocation_cycle(ctx(), &candidates, 5);
        assert_eq!(res.truth_state, REASON_FAIL_CLOSED_DUPLICATE_SYMBOL);
    }

    #[test]
    fn nonpositive_price_fails_closed_for_that_symbol_only() {
        let candidates = vec![
            buy("AAPL", 0.5, 0, 0, 10),
            buy("MSFT", 0.5, 100_000_000, 0, 10),
        ];
        let res = compute_allocation_cycle(ctx(), &candidates, 5);
        assert_eq!(res.truth_state, TRUTH_STATE_COMPUTED);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.disposition, AllocationDisposition::RefusedFailClosed);
        assert_eq!(aapl.reason_code, REASON_NONPOSITIVE_OR_MISSING_PRICE);
        assert_eq!(aapl.final_target_qty, 0);
        // MSFT is unaffected by AAPL's bad price.
        let msft = result_for(&res, "MSFT");
        assert_ne!(msft.disposition, AllocationDisposition::RefusedFailClosed);
    }

    #[test]
    fn invalid_score_fails_closed_for_that_symbol_only() {
        let candidates = vec![
            buy("AAPL", f64::NAN, 100_000_000, 0, 10),
            buy("MSFT", 0.5, 100_000_000, 0, 10),
        ];
        let res = compute_allocation_cycle(ctx(), &candidates, 5);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.reason_code, REASON_INVALID_SCORE);
        assert_eq!(aapl.final_target_qty, 0);
    }

    #[test]
    fn hold_cannot_become_buy() {
        // strategy_target_qty == current_qty -> not an increase -> excluded.
        let candidates = vec![buy("AAPL", 0.9, 100_000_000, 10, 10)];
        let res = compute_allocation_cycle(ctx(), &candidates, 5);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.reason_code, REASON_NOT_AN_INCREASE);
        assert_eq!(aapl.final_target_qty, 10);
    }

    #[test]
    fn reducing_target_does_not_enter_competition() {
        // strategy_target_qty < current_qty -> not an increase -> excluded.
        let candidates = vec![buy("AAPL", 0.9, 100_000_000, 20, 5)];
        let res = compute_allocation_cycle(ctx(), &candidates, 5);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.reason_code, REASON_NOT_AN_INCREASE);
        assert_eq!(aapl.final_target_qty, 20);
    }

    #[test]
    fn allocation_never_exceeds_strategy_target() {
        // Single candidate, huge score, cheap price -> allocator would fund
        // far more shares than the strategy itself asked for.
        let candidates = vec![buy("AAPL", 1.0, 1_000_000, 0, 5)];
        let res = compute_allocation_cycle(ctx(), &candidates, 5);
        let aapl = result_for(&res, "AAPL");
        assert!(aapl.final_target_qty <= aapl.strategy_target_qty);
        assert_eq!(aapl.disposition, AllocationDisposition::Allowed);
        assert_eq!(aapl.final_target_qty, 5);
    }

    #[test]
    fn whole_share_rounding_is_conservative_floor() {
        // long_only_standard caps a single position at 20% of equity.
        // 20% of $100,000 = $20,000; price = $3 -> exactly 6666.67 shares,
        // must floor to 6666, never round up to 6667.
        let candidates = vec![buy("AAPL", 1.0, 3_000_000, 0, 100_000)];
        let res = compute_allocation_cycle(ctx(), &candidates, 5);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.allocation_target_qty, 6666);
    }

    #[test]
    fn max_concurrent_positions_enforced_by_runtime_ceiling() {
        let candidates = vec![
            buy("AAA", 5.0, 100_000_000, 0, 10),
            buy("BBB", 4.0, 100_000_000, 0, 10),
            buy("CCC", 3.0, 100_000_000, 0, 10),
        ];
        let res = compute_allocation_cycle(ctx(), &candidates, 2);
        assert_eq!(res.truth_state, TRUTH_STATE_COMPUTED);
        let allowed_or_clamped = res
            .candidates
            .iter()
            .filter(|c| {
                matches!(
                    c.disposition,
                    AllocationDisposition::Allowed | AllocationDisposition::ClampedDown
                )
            })
            .count();
        assert!(
            allowed_or_clamped <= 2,
            "ceiling=2 must bound funded positions"
        );
        let ccc = result_for(&res, "CCC");
        assert_eq!(ccc.reason_code, REASON_ALLOCATOR_MAX_POSITIONS_REACHED);
    }

    #[test]
    fn allocator_error_fails_closed_whole_cycle() {
        // runtime_ceiling=0 is a caller programming error -> allocator errors.
        let candidates = vec![buy("AAPL", 0.5, 100_000_000, 0, 10)];
        let res = compute_allocation_cycle(ctx(), &candidates, 0);
        assert_eq!(res.truth_state, REASON_FAIL_CLOSED_ALLOCATOR_ERROR);
        assert_eq!(result_for(&res, "AAPL").final_target_qty, 0);
    }

    #[test]
    fn same_inputs_produce_identical_cycle_result() {
        let candidates = vec![
            buy("AAA", 3.0, 100_000_000, 0, 10),
            buy("BBB", 2.0, 100_000_000, 0, 10),
        ];
        let r1 = compute_allocation_cycle(ctx(), &candidates, 5);
        let r2 = compute_allocation_cycle(ctx(), &candidates, 5);
        assert_eq!(r1, r2);
    }

    #[test]
    fn different_candidate_input_order_produces_identical_plan() {
        let forward = vec![
            buy("AAA", 3.0, 100_000_000, 0, 10),
            buy("BBB", 2.0, 100_000_000, 0, 10),
            buy("CCC", 1.0, 100_000_000, 0, 10),
        ];
        let reversed = vec![
            buy("CCC", 1.0, 100_000_000, 0, 10),
            buy("BBB", 2.0, 100_000_000, 0, 10),
            buy("AAA", 3.0, 100_000_000, 0, 10),
        ];
        let r1 = compute_allocation_cycle(ctx(), &forward, 5);
        let r2 = compute_allocation_cycle(ctx(), &reversed, 5);
        assert_eq!(r1, r2);
    }

    #[test]
    fn no_capital_assigned_solely_by_input_order_when_ceiling_binds() {
        // Ceiling=1: whichever symbol has the highest score wins, regardless
        // of where it appears in the input slice.
        let forward = vec![
            buy("LOWSCORE", 1.0, 100_000_000, 0, 10),
            buy("HIGHSCORE", 9.0, 100_000_000, 0, 10),
        ];
        let reversed = vec![
            buy("HIGHSCORE", 9.0, 100_000_000, 0, 10),
            buy("LOWSCORE", 1.0, 100_000_000, 0, 10),
        ];
        let r1 = compute_allocation_cycle(ctx(), &forward, 1);
        let r2 = compute_allocation_cycle(ctx(), &reversed, 1);
        assert_eq!(r1, r2);
        assert_eq!(
            result_for(&r1, "HIGHSCORE").disposition,
            AllocationDisposition::Allowed
        );
        assert_eq!(
            result_for(&r1, "LOWSCORE").disposition,
            AllocationDisposition::RefusedNoCapital
        );
    }

    #[test]
    fn buy_delta_reflects_final_target_minus_current() {
        let candidates = vec![buy("AAPL", 1.0, 1_000_000, 3, 5)];
        let res = compute_allocation_cycle(ctx(), &candidates, 5);
        let aapl = result_for(&res, "AAPL");
        assert_eq!(aapl.final_target_qty, 5);
        assert_eq!(aapl.buy_delta(), 2);
    }

    #[test]
    fn empty_candidate_set_is_computed_with_no_candidates() {
        let res = compute_allocation_cycle(ctx(), &[], 5);
        assert_eq!(res.truth_state, TRUTH_STATE_COMPUTED);
        assert!(res.candidates.is_empty());
    }
}
