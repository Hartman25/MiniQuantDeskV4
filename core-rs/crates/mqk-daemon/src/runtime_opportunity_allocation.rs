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
//! per-symbol loop (caps #2/#4, per-symbol target-state recording, Discord
//! alerts, dry-run diagnostics) is untouched.
//!
//! One exception: cap #6 (`max_new_orders_per_tick`) counts *accepted*
//! submissions, which can only happen at submission time — so it necessarily
//! moves from "skip the rest of this symbol's dispatch before deriving
//! decisions" (its old position, mid-derivation) to "skip the rest of this
//! tick's decisions before submitting" (its new position, after allocation
//! narrowing, in the same dispatch order). This does not change the cap's
//! effect (same running accepted-count, same order, same
//! `"max_new_orders_per_tick_reached"` reason) — it only changes *when* in
//! the pipeline the check runs, which Bundle 5's "collect the whole cycle
//! before submitting" requirement makes unavoidable. See Phase F docs
//! (docs/specs/runtime_opportunity_allocation_01c_shadow_runtime.md).
//!
//! `apply_runtime_opportunity_allocation` itself is I/O-free — current
//! positions, the loaded opportunity artifact, the active durable snapshot,
//! and the exact evaluated-bar facts behind each buy candidate are all
//! supplied by the caller. [`gather_and_apply`] is the thin, I/O-performing
//! glue `loop_runner.rs` actually calls once per tick; it resolves the
//! effective mode (with the live-lock), loads the watchlist/opportunity
//! artifacts, resolves and authority-validates the active durable snapshot,
//! and delegates to the pure function above.
//!
//! # RUNTIME-OPPORTUNITY-ALLOCATION-01-READINESS-AND-AUTHORITY-REPAIR-01
//!
//! This module was repaired after independent source review found three
//! authority defects in the original Phase F/G wiring, all closed here:
//!
//! - **Phase A (exact strategy bar authority)**: the original code re-fetched
//!   "the latest completed bar" from the DB *after* strategy evaluation to
//!   price each buy candidate — a race: a newer bar could land between
//!   evaluation and allocation, silently pricing a decision off a bar the
//!   strategy never saw. [`PendingDecisionWithBarFacts`] now carries the
//!   *exact* [`crate::state::EvaluatedBarFacts`] (symbol, strategy_id,
//!   timeframe, bar end-timestamp, close) each decision was derived from,
//!   captured once at dispatch time (`state.rs`'s
//!   `dispatch_native_strategy_for_symbol_with_loaded_bars_and_facts`) and
//!   carried forward — never re-queried here.
//! - **Phase B (economic cycle identity)**: [`compute_cycle_id`] no longer
//!   includes the loop-tick wall clock (`now_micros`). It is now a pure
//!   function of `run_id`, `market_date`, `timeframe`, the opportunity
//!   artifact id, and the sorted `(symbol, strategy_id, bar_end_ts)` tuple
//!   set of the candidates with proven bar facts — reprocessing the exact
//!   same completed-bar economic cycle on a later tick now yields the exact
//!   same `cycle_id`/`plan_id`, so the durable evidence insert
//!   (`ON CONFLICT (plan_id) DO NOTHING`) is genuinely idempotent per
//!   economic cycle, not per wall-clock tick.
//! - **Phase C (shared snapshot authority)**: `resolve_active_snapshot` no
//!   longer duplicates a private staleness constant or hand-rolls shallow
//!   truth_state/currency/age checks. It reuses the exact accepted Bundle 4
//!   read-side authority seam — `routes::portfolio_provenance`'s
//!   `validate_run_scoped_snapshot_authority` and the new
//!   `validate_snapshot_freshness`, sharing one canonical
//!   `DURABLE_SNAPSHOT_STALE_SECS` — the same identity/mode/source/currency/
//!   truth_state/position-provenance contract every durable portfolio read
//!   route enforces, with no local mirror and no fallback to an older
//!   snapshot on failure.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;

use crate::decision::InternalStrategyDecision;
use crate::runtime_opportunity_artifact::LoadedRuntimeOpportunitySet;
use crate::runtime_opportunity_mode::RuntimeOpportunityAllocationMode;
use crate::state::{AppState, EvaluatedBarFacts};
use mqk_portfolio::{
    compute_allocation_cycle, AllocationCandidateInput, AllocationCandidateResult,
    AllocationCycleContext, AllocationCycleResult, AllocationDisposition,
};

/// The one durable-snapshot fact this module needs — already validated by
/// the caller (`truth_state == "active"`, Paper+Alpaca+USD, real run_id, not
/// stale/future-timestamped; Phase C reuses the shared Bundle 4 authority
/// seam). This module only re-checks `equity_micros > 0` (via the pure cycle
/// model it delegates to).
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveSnapshotFacts {
    pub snapshot_id: String,
    pub equity_micros: i64,
}

/// Pairs one already-derived [`InternalStrategyDecision`] with the exact
/// evaluated-bar facts it was derived from (Phase A). `bar_facts` is `None`
/// when no real completed-bar window backed the decision (the daemon's
/// single-stub fallback dispatch path — see
/// `AppState::dispatch_native_strategy_for_symbol_with_bar_and_facts`); a
/// buy decision with `None` (or mismatched) `bar_facts` is refused
/// individually rather than priced from a re-fetched or substitute bar.
/// Sell/reduce decisions never consult `bar_facts` at all — no price is
/// needed to reduce or exit a position.
#[derive(Debug, Clone)]
pub struct PendingDecisionWithBarFacts {
    pub decision: InternalStrategyDecision,
    pub bar_facts: Option<EvaluatedBarFacts>,
}

/// Deterministic per-cycle economic identity (Phase B): UUIDv5 of `run_id` +
/// `market_date` + `timeframe` + the opportunity artifact id + the sorted
/// `(symbol, strategy_id, bar_end_ts)` tuple set of the candidates with
/// proven bar facts this cycle. Sorting makes the id independent of
/// dispatch/artifact iteration order; omitting the loop-tick wall clock
/// makes reprocessing the exact same completed-bar economic cycle on a later
/// tick produce the exact same id.
pub fn compute_cycle_id(
    run_id: Uuid,
    market_date: &str,
    timeframe: &str,
    opportunity_artifact_id: &str,
    candidate_tuples: &[(String, String, i64)],
) -> String {
    let mut sorted = candidate_tuples.to_vec();
    sorted.sort();
    let candidates_str = sorted
        .iter()
        .map(|(symbol, strategy_id, bar_end_ts)| format!("{symbol}:{strategy_id}:{bar_end_ts}"))
        .collect::<Vec<_>>()
        .join(",");
    let seed = format!(
        "mqk.runtime-opportunity-allocation-cycle.v2|{run_id}|{market_date}|{timeframe}|{opportunity_artifact_id}|{candidates_str}"
    );
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, seed.as_bytes()).to_string()
}

pub struct RuntimeOpportunityAllocationContext {
    /// Already live-lock-resolved (see `runtime_opportunity_mode::effective_mode`).
    pub mode: RuntimeOpportunityAllocationMode,
    pub run_id: Uuid,
    pub market_date: String,
    pub timeframe: String,
    /// Evidence-only (Phase G's `created_at_utc` / `rebuild_decision_with_qty`'s
    /// decision-id salt) — never part of cycle identity (Phase B).
    pub now_micros: i64,
    pub runtime_ceiling: usize,
    /// `None` when the artifact is not configured, missing, invalid, or stale.
    pub opportunity_set: Option<LoadedRuntimeOpportunitySet>,
    /// `None` when no valid (Phase C shared-authority-validated) durable
    /// snapshot exists.
    pub active_snapshot: Option<ActiveSnapshotFacts>,
}

pub const REASON_NO_OPPORTUNITY_AUTHORITY: &str = "fail_closed_no_opportunity_authority";
pub const REASON_NO_DURABLE_SNAPSHOT: &str = "fail_closed_no_durable_snapshot";
pub const REASON_NO_SCORE_FOR_SYMBOL: &str = "fail_closed_no_opportunity_score_for_symbol";
/// Phase A: a buy candidate's `bar_facts` was `None`, or did not match the
/// decision's own symbol/strategy_id/timeframe — refused individually,
/// never priced from a substitute bar.
pub const REASON_MISSING_OR_MISMATCHED_BAR_FACTS: &str =
    "fail_closed_missing_or_mismatched_bar_facts";
/// Phase B: two or more buy candidates this cycle resolved to the exact same
/// `(symbol, strategy_id, bar_end_ts)` tuple — an anomaly (each symbol
/// should be dispatched at most once per tick); the whole cycle fails
/// closed rather than picking one arbitrarily, mirroring the pure cycle
/// model's own duplicate-symbol guard.
pub const REASON_DUPLICATE_CANDIDATE_TUPLE: &str = "fail_closed_duplicate_candidate_tuple";

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

fn refused_candidate_result(
    d: &InternalStrategyDecision,
    current: i64,
    reason: &str,
    evaluation_price_micros: i64,
) -> AllocationCandidateResult {
    AllocationCandidateResult {
        symbol: d.symbol.clone(),
        strategy_id: d.strategy_id.clone(),
        input_score: 0.0,
        target_weight: 0.0,
        current_qty: current,
        strategy_target_qty: current + d.qty,
        allocation_target_qty: current,
        final_target_qty: current,
        disposition: AllocationDisposition::RefusedFailClosed,
        reason_code: reason.to_string(),
        evaluation_price_micros,
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

/// Apply Bundle 5 opportunity allocation to one tick's already-derived,
/// bar-fact-bound decisions.
///
/// `current_positions` is keyed by symbol. Each buy candidate's price comes
/// exclusively from its own `bar_facts` (Phase A) — this function never
/// receives, fetches, or otherwise obtains a price from any other source.
pub fn apply_runtime_opportunity_allocation(
    ctx: &RuntimeOpportunityAllocationContext,
    decisions: Vec<PendingDecisionWithBarFacts>,
    current_positions: &BTreeMap<String, i64>,
) -> RuntimeOpportunityAllocationOutcome {
    if ctx.mode == RuntimeOpportunityAllocationMode::Off {
        return RuntimeOpportunityAllocationOutcome {
            decisions: decisions.into_iter().map(|p| p.decision).collect(),
            plan: None,
        };
    }

    let (buy_pending, other_pending): (Vec<_>, Vec<_>) = decisions
        .into_iter()
        .partition(|p| p.decision.side.eq_ignore_ascii_case("buy"));
    let mut other_decisions: Vec<InternalStrategyDecision> =
        other_pending.into_iter().map(|p| p.decision).collect();

    if buy_pending.is_empty() {
        return RuntimeOpportunityAllocationOutcome {
            decisions: other_decisions,
            plan: None,
        };
    }

    let buy_decisions: Vec<InternalStrategyDecision> =
        buy_pending.iter().map(|p| p.decision.clone()).collect();

    // Phase A: bind each buy decision to the exact evaluated-bar facts
    // carried alongside it. `bar_facts` must be present AND match this
    // exact decision's symbol/strategy_id, and its timeframe must match the
    // cycle's own timeframe -- anything else is refused individually below,
    // never substituted with a re-fetched or different bar's price.
    let mut close_micros_by_symbol: BTreeMap<String, i64> = BTreeMap::new();
    let mut bar_end_ts_by_symbol: BTreeMap<String, i64> = BTreeMap::new();
    for p in &buy_pending {
        if let Some(facts) = &p.bar_facts {
            if facts.symbol == p.decision.symbol
                && facts.strategy_id == p.decision.strategy_id
                && facts.timeframe == ctx.timeframe
            {
                close_micros_by_symbol.insert(p.decision.symbol.clone(), facts.close_micros);
                bar_end_ts_by_symbol.insert(p.decision.symbol.clone(), facts.bar_end_ts);
            }
        }
    }

    // Phase B: economic cycle identity from the sorted (symbol, strategy_id,
    // exact bar) tuple set of only the candidates with proven bar facts.
    let candidate_tuples: Vec<(String, String, i64)> = buy_decisions
        .iter()
        .filter(|d| bar_end_ts_by_symbol.contains_key(&d.symbol))
        .map(|d| {
            (
                d.symbol.clone(),
                d.strategy_id.clone(),
                bar_end_ts_by_symbol[&d.symbol],
            )
        })
        .collect();
    let has_duplicate_tuple = {
        let mut seen: HashSet<(String, String, i64)> = HashSet::new();
        candidate_tuples.iter().any(|t| !seen.insert(t.clone()))
    };

    let opportunity_artifact_id = ctx
        .opportunity_set
        .as_ref()
        .map(|a| a.artifact_id.clone())
        .unwrap_or_default();
    let source_snapshot_id = ctx
        .active_snapshot
        .as_ref()
        .map(|s| s.snapshot_id.clone())
        .unwrap_or_default();
    let equity_micros = ctx
        .active_snapshot
        .as_ref()
        .map(|s| s.equity_micros)
        .unwrap_or(0);

    let cycle_id = compute_cycle_id(
        ctx.run_id,
        &ctx.market_date,
        &ctx.timeframe,
        &opportunity_artifact_id,
        &candidate_tuples,
    );
    let context = AllocationCycleContext {
        cycle_id,
        run_id: ctx.run_id.to_string(),
        market_date: ctx.market_date.clone(),
        timeframe: ctx.timeframe.clone(),
        opportunity_artifact_id,
        source_snapshot_id,
        equity_micros,
    };

    if has_duplicate_tuple {
        let plan = fail_closed_plan(
            context,
            &buy_decisions,
            current_positions,
            REASON_DUPLICATE_CANDIDATE_TUPLE,
        );
        // A duplicate candidate identity makes the whole cycle's economic
        // meaning ambiguous -- refuse every buy this cycle; sells untouched.
        return RuntimeOpportunityAllocationOutcome {
            decisions: other_decisions,
            plan: Some(plan),
        };
    }

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
    let mut refused_individually: Vec<AllocationCandidateResult> = Vec::new();
    for d in &buy_decisions {
        let current = current_positions.get(&d.symbol).copied().unwrap_or(0);
        let Some(close_micros) = close_micros_by_symbol.get(&d.symbol).copied() else {
            // Phase A fail-closed rule: missing/mismatched bar facts refuse
            // only this candidate -- sibling candidates and every sell/reduce
            // decision are unaffected.
            refused_individually.push(refused_candidate_result(
                d,
                current,
                REASON_MISSING_OR_MISMATCHED_BAR_FACTS,
                0,
            ));
            continue;
        };
        match opportunity_set
            .candidates
            .iter()
            .find(|c| c.symbol == d.symbol)
        {
            Some(oc) => candidates.push(AllocationCandidateInput {
                symbol: d.symbol.clone(),
                strategy_id: d.strategy_id.clone(),
                score: oc.score,
                evaluation_price_micros: close_micros,
                current_qty: current,
                strategy_target_qty: current + d.qty,
            }),
            None => refused_individually.push(refused_candidate_result(
                d,
                current,
                REASON_NO_SCORE_FOR_SYMBOL,
                close_micros,
            )),
        }
    }

    let mut plan = compute_allocation_cycle(context, &candidates, ctx.runtime_ceiling);
    plan.candidates.extend(refused_individually);
    // Preserve the documented "sorted by symbol ascending" invariant after
    // extending with individually-refused candidates.
    plan.candidates.sort_by(|a, b| a.symbol.cmp(&b.symbol));

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
                // delta == 0 -> no capital assigned, or refused individually
                // (missing bar facts / no score); decision is dropped (no
                // submission, no trade this cycle).
            }
        }
    }

    RuntimeOpportunityAllocationOutcome {
        decisions: other_decisions,
        plan: Some(plan),
    }
}

// ---------------------------------------------------------------------------
// Phase G: durable evidence persistence (best-effort, never blocks the tick)
// ---------------------------------------------------------------------------

fn disposition_str(d: AllocationDisposition) -> &'static str {
    match d {
        AllocationDisposition::Allowed => "allowed",
        AllocationDisposition::ClampedDown => "clamped_down",
        AllocationDisposition::RefusedNoCapital => "refused_no_capital",
        AllocationDisposition::RefusedFailClosed => "refused_fail_closed",
    }
}

fn plan_to_new_db_plan(
    plan: &AllocationCycleResult,
    mode: RuntimeOpportunityAllocationMode,
) -> Option<mqk_db::NewRuntimeOpportunityAllocationPlan> {
    let plan_id = plan.context.cycle_id.parse::<Uuid>().ok()?;
    let run_id = plan.context.run_id.parse::<Uuid>().ok()?;
    let source_snapshot_id = plan.context.source_snapshot_id.parse::<Uuid>().ok();

    let allowed_count = plan
        .candidates
        .iter()
        .filter(|c| {
            matches!(
                c.disposition,
                AllocationDisposition::Allowed | AllocationDisposition::ClampedDown
            )
        })
        .count() as i32;

    let candidates = plan
        .candidates
        .iter()
        .enumerate()
        .map(|(i, c)| mqk_db::NewRuntimeOpportunityAllocationCandidate {
            ordinal: i as i32,
            symbol: c.symbol.clone(),
            strategy_id: c.strategy_id.clone(),
            input_score_micros: mqk_db::scale_to_micros(c.input_score),
            target_weight_micros: mqk_db::scale_to_micros(c.target_weight),
            current_qty: c.current_qty,
            strategy_target_qty: c.strategy_target_qty,
            allocation_target_qty: c.allocation_target_qty,
            final_target_qty: c.final_target_qty,
            disposition: disposition_str(c.disposition).to_string(),
            reason_code: c.reason_code.clone(),
            evaluation_price_micros: c.evaluation_price_micros,
        })
        .collect();

    Some(mqk_db::NewRuntimeOpportunityAllocationPlan {
        plan_id,
        cycle_id: plan_id,
        run_id,
        mode: mode.as_str().to_string(),
        opportunity_artifact_id: plan.context.opportunity_artifact_id.clone(),
        source_snapshot_id,
        equity_micros: plan.context.equity_micros,
        candidate_count: plan.candidates.len() as i32,
        allowed_count,
        gross_weight_micros: mqk_db::scale_to_micros(plan.gross_weight),
        net_weight_micros: mqk_db::scale_to_micros(plan.net_weight),
        truth_state: plan.truth_state.clone(),
        blockers: plan.blockers.clone(),
        created_at_utc: Utc::now(),
        candidates,
    })
}

/// Persist `plan` (when present) to durable evidence. Best-effort: a
/// persistence failure is logged and otherwise ignored — this is evidence,
/// not authoritative order/portfolio truth, and must never block or fail
/// the tick that produced it. Never called for `mode == Off` (no plan
/// exists in that case).
async fn persist_plan_if_present(
    state_arc: &Arc<AppState>,
    plan: &Option<AllocationCycleResult>,
    mode: RuntimeOpportunityAllocationMode,
) {
    let Some(plan) = plan else { return };
    let Some(db) = state_arc.db.as_ref() else {
        return;
    };
    let Some(new_plan) = plan_to_new_db_plan(plan, mode) else {
        tracing::warn!(
            cycle_id = %plan.context.cycle_id,
            "runtime_opportunity_allocation_plan_persist_skipped: cycle_id/run_id not a valid UUID"
        );
        return;
    };
    match mqk_db::insert_runtime_opportunity_allocation_plan(db, new_plan).await {
        Ok(_) => {}
        Err(err) => {
            tracing::warn!(
                cycle_id = %plan.context.cycle_id,
                error = %err,
                "runtime_opportunity_allocation_plan_persist_failed"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// I/O glue — the only impure code in this module
// ---------------------------------------------------------------------------

fn load_watchlist_and_raw_json() -> Option<(
    crate::watchlist_intake::LoadedWatchlistArtifact,
    serde_json::Value,
)> {
    let outcome = crate::watchlist_intake::evaluate_watchlist_intake_from_env();
    if !outcome.approved_for_autonomous_paper() {
        return None;
    }
    let artifact = outcome.artifact()?.clone();
    let raw_path = std::env::var(crate::watchlist_intake::ENV_PAPER_WATCHLIST_PATH).ok()?;
    let contents = std::fs::read_to_string(raw_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&contents).ok()?;
    Some((artifact, json))
}

/// RUNTIME-OPPORTUNITY-ALLOCATION-01 authority repair (Phase C): resolve the
/// active durable Paper+Alpaca portfolio snapshot for `run_id`, reusing the
/// exact accepted Bundle 4 read-side authority seam
/// (`routes::portfolio_provenance`) instead of a local, shallower
/// truth_state/currency/age check. An invalid or malformed latest snapshot
/// is refused outright — never falls back to an older snapshot.
async fn resolve_active_snapshot(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    now: chrono::DateTime<Utc>,
) -> Option<ActiveSnapshotFacts> {
    let snap = mqk_db::fetch_latest_paper_portfolio_snapshot_for_run(
        pool,
        "paper",
        mqk_db::PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA,
        run_id,
    )
    .await
    .ok()??;

    crate::routes::portfolio_provenance::validate_run_scoped_snapshot_authority(&snap, run_id)
        .ok()?;
    crate::routes::portfolio_provenance::validate_snapshot_freshness(
        snap.snapshot.captured_at_utc,
        now,
        crate::routes::portfolio_provenance::DURABLE_SNAPSHOT_STALE_SECS,
    )
    .ok()?;

    Some(ActiveSnapshotFacts {
        snapshot_id: snap.snapshot.snapshot_id.to_string(),
        equity_micros: snap.snapshot.equity_micros,
    })
}

/// The single per-tick call site `loop_runner.rs` uses. Resolves the
/// effective mode (env + live-lock), gathers every I/O-sourced fact the pure
/// [`apply_runtime_opportunity_allocation`] needs, and delegates to it.
///
/// `decisions` must already carry each buy candidate's exact evaluated-bar
/// facts (Phase A) — this function performs no price lookup of its own.
///
/// When the effective mode is `Off`, returns `decisions` untouched and skips
/// every I/O call below — the default configuration has zero additional
/// runtime cost.
pub async fn gather_and_apply(
    state_arc: &Arc<AppState>,
    run_id: Uuid,
    now_micros: i64,
    market_date: String,
    timeframe: String,
    decisions: Vec<PendingDecisionWithBarFacts>,
    current_positions: &BTreeMap<String, i64>,
) -> RuntimeOpportunityAllocationOutcome {
    let resolution =
        crate::runtime_opportunity_mode::resolve_runtime_opportunity_allocation_mode_from_env();
    let broker_kind = crate::state::BrokerKind::parse(state_arc.adapter_id());
    let eff = crate::runtime_opportunity_mode::effective_mode(
        &resolution,
        state_arc.deployment_mode(),
        broker_kind,
    );

    if eff.effective_mode == RuntimeOpportunityAllocationMode::Off {
        return RuntimeOpportunityAllocationOutcome {
            decisions: decisions.into_iter().map(|p| p.decision).collect(),
            plan: None,
        };
    }

    let runtime_ceiling = crate::watchlist_intake::MULTI_SYMBOL_HARD_CEILING as usize;

    let Some(db) = state_arc.db.as_ref() else {
        let ctx = RuntimeOpportunityAllocationContext {
            mode: eff.effective_mode,
            run_id,
            market_date,
            timeframe,
            now_micros,
            runtime_ceiling,
            opportunity_set: None,
            active_snapshot: None,
        };
        return apply_runtime_opportunity_allocation(&ctx, decisions, current_positions);
    };

    let now_utc = Utc::now();
    let watchlist_bundle = load_watchlist_and_raw_json();
    let opportunity_set: Option<LoadedRuntimeOpportunitySet> = match &watchlist_bundle {
        Some((wl, wl_json)) => {
            let outcome =
                crate::runtime_opportunity_artifact::evaluate_runtime_opportunity_intake_from_env(
                    wl,
                    wl_json,
                    now_utc,
                    &market_date,
                );
            outcome.artifact().cloned()
        }
        None => None,
    };
    let active_snapshot = resolve_active_snapshot(db, run_id, now_utc).await;

    let ctx = RuntimeOpportunityAllocationContext {
        mode: eff.effective_mode,
        run_id,
        market_date,
        timeframe,
        now_micros,
        runtime_ceiling,
        opportunity_set,
        active_snapshot,
    };
    let outcome = apply_runtime_opportunity_allocation(&ctx, decisions, current_positions);
    persist_plan_if_present(state_arc, &outcome.plan, eff.effective_mode).await;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_opportunity_artifact::LoadedRuntimeOpportunityCandidate;

    fn run_id() -> Uuid {
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"test-run")
    }

    const TIMEFRAME: &str = "5m";

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

    fn facts(
        symbol: &str,
        strategy_id: &str,
        bar_end_ts: i64,
        close_micros: i64,
    ) -> EvaluatedBarFacts {
        EvaluatedBarFacts {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe: TIMEFRAME.to_string(),
            bar_end_ts,
            close_micros,
        }
    }

    /// Convenience: pair a decision with matching bar facts at the given
    /// price (bar_end_ts fixed at 1_000 unless the test needs otherwise).
    fn bound_buy(
        symbol: &str,
        strategy_id: &str,
        qty: i64,
        bar_end_ts: i64,
        close_micros: i64,
    ) -> PendingDecisionWithBarFacts {
        PendingDecisionWithBarFacts {
            decision: buy(symbol, strategy_id, qty),
            bar_facts: Some(facts(symbol, strategy_id, bar_end_ts, close_micros)),
        }
    }

    fn unbound(decision: InternalStrategyDecision) -> PendingDecisionWithBarFacts {
        PendingDecisionWithBarFacts {
            decision,
            bar_facts: None,
        }
    }

    fn opportunity_set(candidates: Vec<(&str, &str, f64)>) -> LoadedRuntimeOpportunitySet {
        LoadedRuntimeOpportunitySet {
            artifact_id: "artifact-1".to_string(),
            market_date: "2026-07-26".to_string(),
            timeframe: TIMEFRAME.to_string(),
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
            timeframe: TIMEFRAME.to_string(),
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
            unbound(buy("AAPL", "intraday_scalper", 10)),
            unbound(sell("MSFT", "intraday_scalper", 5)),
        ];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::Off),
            decisions,
            &BTreeMap::new(),
        );
        assert!(out.plan.is_none());
        assert_eq!(out.decisions.len(), 2);
        assert_eq!(out.decisions[0].qty, 10);
    }

    #[test]
    fn no_buy_decisions_this_tick_produces_no_plan() {
        let decisions = vec![unbound(sell("MSFT", "intraday_scalper", 5))];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::Shadow),
            decisions,
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
            let decisions = vec![
                bound_buy("AAPL", "intraday_scalper", 10, 1_000, 100_000_000),
                unbound(sell("TLT", "intraday_scalper", 3)),
            ];
            let out = apply_runtime_opportunity_allocation(&base_ctx(mode), decisions, &current);
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
        let decisions = vec![bound_buy(
            "AAPL",
            "intraday_scalper",
            10,
            1_000,
            100_000_000,
        )];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::Shadow),
            decisions,
            &current,
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
        // Expensive price -> allocator's 20% single-position cap on $100k
        // equity ($20,000) funds far fewer shares than the strategy's
        // target of 10,000.
        let decisions = vec![bound_buy(
            "AAPL",
            "intraday_scalper",
            10_000,
            1_000,
            10_000_000_000, // $10,000/share
        )];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::PaperEnforced),
            decisions,
            &current,
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
        let decisions = vec![
            bound_buy("AAPL", "intraday_scalper", 10, 1_000, 100_000_000),
            bound_buy("MSFT", "intraday_scalper", 10, 1_000, 100_000_000),
        ];
        let out = apply_runtime_opportunity_allocation(&ctx, decisions, &current);
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
            bound_buy("AAPL", "intraday_scalper", 10, 1_000, 100_000_000),
            unbound(sell("TLT", "intraday_scalper", 3)),
        ];
        let out = apply_runtime_opportunity_allocation(&ctx, decisions, &BTreeMap::new());
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
        let decisions = vec![bound_buy(
            "AAPL",
            "intraday_scalper",
            10,
            1_000,
            100_000_000,
        )];
        let out = apply_runtime_opportunity_allocation(&ctx, decisions, &BTreeMap::new());
        assert!(out.decisions.is_empty());
        assert_eq!(out.plan.unwrap().truth_state, REASON_NO_DURABLE_SNAPSHOT);
    }

    #[test]
    fn symbol_not_in_opportunity_set_is_refused_not_fabricated() {
        let mut current = BTreeMap::new();
        current.insert("ZZZZ".to_string(), 0i64);
        let decisions = vec![bound_buy(
            "ZZZZ",
            "intraday_scalper",
            10,
            1_000,
            100_000_000,
        )];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::PaperEnforced),
            decisions,
            &current,
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
        let decisions = vec![
            bound_buy("AAPL", "intraday_scalper", 10, 1_000, 100_000_000),
            bound_buy("MSFT", "intraday_scalper", 10, 1_000, 100_000_000),
        ];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::Shadow),
            decisions,
            &current,
        );
        let plan = out.plan.unwrap();
        assert_eq!(plan.candidates.len(), 2);
    }

    // ── Phase A: exact strategy bar authority ─────────────────────────────

    #[test]
    fn buy_uses_exact_close_carried_from_bar_facts_not_a_different_price() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        // A deliberately distinctive price so we can prove it (and only it)
        // reached the allocator.
        let decisions = vec![bound_buy(
            "AAPL",
            "intraday_scalper",
            10,
            1_000,
            77_000_000, // $77.00 -- must appear verbatim in the plan
        )];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::Shadow),
            decisions,
            &current,
        );
        let plan = out.plan.unwrap();
        let aapl = plan.candidates.iter().find(|c| c.symbol == "AAPL").unwrap();
        assert_eq!(aapl.evaluation_price_micros, 77_000_000);
    }

    #[test]
    fn missing_bar_facts_refuses_only_that_buy_not_siblings() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        current.insert("MSFT".to_string(), 0i64);
        let decisions = vec![
            unbound(buy("AAPL", "intraday_scalper", 10)), // no bar facts at all
            bound_buy("MSFT", "intraday_scalper", 10, 1_000, 100_000_000),
        ];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::PaperEnforced),
            decisions,
            &current,
        );
        let plan = out.plan.unwrap();
        let aapl = plan.candidates.iter().find(|c| c.symbol == "AAPL").unwrap();
        assert_eq!(aapl.reason_code, REASON_MISSING_OR_MISMATCHED_BAR_FACTS);
        assert_eq!(aapl.disposition, AllocationDisposition::RefusedFailClosed);
        assert!(!out.decisions.iter().any(|d| d.symbol == "AAPL"));
        // MSFT (well-formed facts) is unaffected.
        assert!(out.decisions.iter().any(|d| d.symbol == "MSFT"));
    }

    #[test]
    fn mismatched_symbol_in_bar_facts_is_refused() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        let mut pending = bound_buy("AAPL", "intraday_scalper", 10, 1_000, 100_000_000);
        // Corrupt the bound facts to name a different symbol than the
        // decision they're paired with.
        pending.bar_facts.as_mut().unwrap().symbol = "MSFT".to_string();
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::PaperEnforced),
            vec![pending],
            &current,
        );
        assert!(out.decisions.is_empty());
        let plan = out.plan.unwrap();
        let aapl = plan.candidates.iter().find(|c| c.symbol == "AAPL").unwrap();
        assert_eq!(aapl.reason_code, REASON_MISSING_OR_MISMATCHED_BAR_FACTS);
    }

    #[test]
    fn mismatched_strategy_id_in_bar_facts_is_refused() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        let mut pending = bound_buy("AAPL", "intraday_scalper", 10, 1_000, 100_000_000);
        pending.bar_facts.as_mut().unwrap().strategy_id = "other_strategy".to_string();
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::PaperEnforced),
            vec![pending],
            &current,
        );
        assert!(out.decisions.is_empty());
    }

    #[test]
    fn mismatched_timeframe_in_bar_facts_is_refused() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        let mut pending = bound_buy("AAPL", "intraday_scalper", 10, 1_000, 100_000_000);
        pending.bar_facts.as_mut().unwrap().timeframe = "1h".to_string();
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::PaperEnforced),
            vec![pending],
            &current,
        );
        assert!(out.decisions.is_empty());
    }

    #[test]
    fn sell_reduce_unaffected_by_missing_bar_facts() {
        let mut current = BTreeMap::new();
        current.insert("TLT".to_string(), 10i64);
        let decisions = vec![unbound(sell("TLT", "intraday_scalper", 3))];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::PaperEnforced),
            decisions,
            &current,
        );
        assert_eq!(out.decisions.len(), 1);
        assert_eq!(out.decisions[0].side, "sell");
    }

    // ── Phase B: economic cycle identity ───────────────────────────────────

    #[test]
    fn cycle_id_is_deterministic_and_order_independent() {
        let id1 = compute_cycle_id(
            run_id(),
            "2026-07-26",
            TIMEFRAME,
            "artifact-1",
            &[
                ("AAPL".to_string(), "intraday_scalper".to_string(), 1_000),
                ("MSFT".to_string(), "intraday_scalper".to_string(), 2_000),
            ],
        );
        let id2 = compute_cycle_id(
            run_id(),
            "2026-07-26",
            TIMEFRAME,
            "artifact-1",
            &[
                ("MSFT".to_string(), "intraday_scalper".to_string(), 2_000),
                ("AAPL".to_string(), "intraday_scalper".to_string(), 1_000),
            ],
        );
        assert_eq!(id1, id2);
    }

    #[test]
    fn same_economic_cycle_replayed_on_a_later_tick_yields_the_same_cycle_id() {
        // Reprocessing the exact same run/artifact/bar candidate set on a
        // later loop tick (different now_micros) must yield the identical
        // cycle_id/plan_id -- proves identity is bound to the economic
        // cycle, not the wall clock.
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        let mut ctx1 = base_ctx(RuntimeOpportunityAllocationMode::Shadow);
        ctx1.now_micros = 1_000_000;
        let mut ctx2 = base_ctx(RuntimeOpportunityAllocationMode::Shadow);
        ctx2.now_micros = 99_999_999;
        let decisions1 = vec![bound_buy(
            "AAPL",
            "intraday_scalper",
            10,
            1_000,
            100_000_000,
        )];
        let decisions2 = vec![bound_buy(
            "AAPL",
            "intraday_scalper",
            10,
            1_000,
            100_000_000,
        )];
        let out1 = apply_runtime_opportunity_allocation(&ctx1, decisions1, &current);
        let out2 = apply_runtime_opportunity_allocation(&ctx2, decisions2, &current);
        assert_eq!(
            out1.plan.unwrap().context.cycle_id,
            out2.plan.unwrap().context.cycle_id
        );
    }

    #[test]
    fn different_bar_changes_the_cycle_id() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        let decisions1 = vec![bound_buy(
            "AAPL",
            "intraday_scalper",
            10,
            1_000,
            100_000_000,
        )];
        let decisions2 = vec![bound_buy(
            "AAPL",
            "intraday_scalper",
            10,
            2_000, // different bar_end_ts
            100_000_000,
        )];
        let out1 = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::Shadow),
            decisions1,
            &current,
        );
        let out2 = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::Shadow),
            decisions2,
            &current,
        );
        assert_ne!(
            out1.plan.unwrap().context.cycle_id,
            out2.plan.unwrap().context.cycle_id
        );
    }

    #[test]
    fn different_artifact_changes_the_cycle_id() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        let ctx1 = base_ctx(RuntimeOpportunityAllocationMode::Shadow);
        let mut ctx2 = base_ctx(RuntimeOpportunityAllocationMode::Shadow);
        ctx2.opportunity_set.as_mut().unwrap().artifact_id = "artifact-2".to_string();
        let decisions1 = vec![bound_buy(
            "AAPL",
            "intraday_scalper",
            10,
            1_000,
            100_000_000,
        )];
        let decisions2 = vec![bound_buy(
            "AAPL",
            "intraday_scalper",
            10,
            1_000,
            100_000_000,
        )];
        let out1 = apply_runtime_opportunity_allocation(&ctx1, decisions1, &current);
        let out2 = apply_runtime_opportunity_allocation(&ctx2, decisions2, &current);
        assert_ne!(
            out1.plan.unwrap().context.cycle_id,
            out2.plan.unwrap().context.cycle_id
        );
    }

    #[test]
    fn different_strategy_assignment_changes_the_cycle_id() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        let mut ctx = base_ctx(RuntimeOpportunityAllocationMode::Shadow);
        ctx.opportunity_set = Some(opportunity_set(vec![
            ("AAPL", "strategy_a", 0.9),
            ("AAPL", "strategy_b", 0.9),
        ]));
        let decisions1 = vec![bound_buy("AAPL", "strategy_a", 10, 1_000, 100_000_000)];
        let decisions2 = vec![bound_buy("AAPL", "strategy_b", 10, 1_000, 100_000_000)];
        let out1 = apply_runtime_opportunity_allocation(&ctx, decisions1, &current);
        let out2 = apply_runtime_opportunity_allocation(&ctx, decisions2, &current);
        assert_ne!(
            out1.plan.unwrap().context.cycle_id,
            out2.plan.unwrap().context.cycle_id
        );
    }

    #[test]
    fn duplicate_candidate_tuple_fails_the_whole_cycle_closed() {
        // Two distinct PendingDecisionWithBarFacts entries that both resolve
        // to the exact same (symbol, strategy_id, bar_end_ts) tuple -- an
        // anomaly the cycle must refuse wholesale rather than pick one.
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        let decisions = vec![
            bound_buy("AAPL", "intraday_scalper", 10, 1_000, 100_000_000),
            bound_buy("AAPL", "intraday_scalper", 5, 1_000, 100_000_000),
        ];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::PaperEnforced),
            decisions,
            &current,
        );
        assert!(out.decisions.is_empty());
        assert_eq!(
            out.plan.unwrap().truth_state,
            REASON_DUPLICATE_CANDIDATE_TUPLE
        );
    }

    #[test]
    fn durable_insert_is_idempotent_for_the_same_economic_cycle() {
        // Same run/artifact/bar candidate set, submitted "twice" (simulating
        // reprocessing on a later tick) -> identical plan_id (== cycle_id),
        // so `insert_runtime_opportunity_allocation_plan`'s
        // `ON CONFLICT (plan_id) DO NOTHING` makes the second insert a
        // genuine no-op for the same economic cycle.
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        let mut ctx_tick1 = base_ctx(RuntimeOpportunityAllocationMode::Shadow);
        ctx_tick1.now_micros = 1;
        let mut ctx_tick2 = base_ctx(RuntimeOpportunityAllocationMode::Shadow);
        ctx_tick2.now_micros = 2;
        let d1 = vec![bound_buy(
            "AAPL",
            "intraday_scalper",
            10,
            1_000,
            100_000_000,
        )];
        let d2 = vec![bound_buy(
            "AAPL",
            "intraday_scalper",
            10,
            1_000,
            100_000_000,
        )];
        let plan1 = apply_runtime_opportunity_allocation(&ctx_tick1, d1, &current)
            .plan
            .unwrap();
        let plan2 = apply_runtime_opportunity_allocation(&ctx_tick2, d2, &current)
            .plan
            .unwrap();
        let db_plan1 =
            plan_to_new_db_plan(&plan1, RuntimeOpportunityAllocationMode::Shadow).unwrap();
        let db_plan2 =
            plan_to_new_db_plan(&plan2, RuntimeOpportunityAllocationMode::Shadow).unwrap();
        assert_eq!(db_plan1.plan_id, db_plan2.plan_id);
    }

    // ── Phase G: plan -> durable-row conversion ───────────────────────────────

    #[test]
    fn disposition_str_covers_every_variant() {
        assert_eq!(disposition_str(AllocationDisposition::Allowed), "allowed");
        assert_eq!(
            disposition_str(AllocationDisposition::ClampedDown),
            "clamped_down"
        );
        assert_eq!(
            disposition_str(AllocationDisposition::RefusedNoCapital),
            "refused_no_capital"
        );
        assert_eq!(
            disposition_str(AllocationDisposition::RefusedFailClosed),
            "refused_fail_closed"
        );
    }

    #[test]
    fn plan_to_new_db_plan_converts_valid_plan() {
        let mut current = BTreeMap::new();
        current.insert("AAPL".to_string(), 0i64);
        let decisions = vec![bound_buy(
            "AAPL",
            "intraday_scalper",
            10,
            1_000,
            100_000_000,
        )];
        let out = apply_runtime_opportunity_allocation(
            &base_ctx(RuntimeOpportunityAllocationMode::Shadow),
            decisions,
            &current,
        );
        let plan = out.plan.unwrap();
        let db_plan = plan_to_new_db_plan(&plan, RuntimeOpportunityAllocationMode::Shadow).unwrap();
        assert_eq!(db_plan.mode, "shadow");
        assert_eq!(db_plan.plan_id, db_plan.cycle_id);
        assert_eq!(db_plan.run_id, run_id());
        assert_eq!(db_plan.candidates.len(), 1);
        assert_eq!(db_plan.candidates[0].symbol, "AAPL");
        assert_eq!(db_plan.candidates[0].input_score_micros, 900_000);
    }

    #[test]
    fn plan_to_new_db_plan_none_when_cycle_id_not_a_uuid() {
        let context = AllocationCycleContext {
            cycle_id: "not-a-uuid".to_string(),
            run_id: run_id().to_string(),
            market_date: "2026-07-26".to_string(),
            timeframe: TIMEFRAME.to_string(),
            opportunity_artifact_id: "artifact-1".to_string(),
            source_snapshot_id: "snap-1".to_string(),
            equity_micros: 1,
        };
        let plan = AllocationCycleResult {
            context,
            candidates: vec![],
            gross_weight: 0.0,
            net_weight: 0.0,
            truth_state: "computed".to_string(),
            blockers: vec![],
        };
        assert!(plan_to_new_db_plan(&plan, RuntimeOpportunityAllocationMode::Shadow).is_none());
    }
}
