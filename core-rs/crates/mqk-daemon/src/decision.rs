//! CC-01D: Internal strategy decision-to-intent seam.
//!
//! Provides the narrowest fail-closed path that lets an internally-originated
//! strategy decision be validated against the canonical strategy registry and
//! converted into a durable execution intent candidate via the canonical
//! outbox path.
//!
//! # Gate sequence
//!
//! ```text
//! 0.  field_validation     — decision_id / strategy_id / symbol / side / qty
//! 1.  day_signal_limit     — PT-AUTO-02: per-run intake bound not exceeded (account-wide)
//! 1f. symbol_day_order_cap — MULTI-SYMBOL-DAY-ORDER-CAP-01: optional per-symbol daily order count cap (cap #4)
//! 1e. capital_budget       — B6/TV-04B: per-strategy budget authorized (same gate as external signal path)
//! 1g. per_symbol_notional  — MULTI-SYMBOL-CAPITAL-CAPS-01: optional per-symbol notional cap (cap #3); limit orders only, SizingUnverifiable pass-through for market orders
//! 1h. sector_risk          — ETF-RISK-CLOSURE-01: optional per-sector live gross exposure cap (`MQK_SECTOR_EXPOSURE_LIMITS_BPS`); uses real live weights/marks, fail-closed when enabled and unverifiable, risk-reducing orders always allowed; shared with the external signal path via `capital_policy::sector_risk_gate` (ETF-RISK-EXTERNAL-SIGNAL-GATE-01)
//! 2.  db_present           — no DB → unavailable
//! 3.  registry_check       — strategy must be registered AND enabled
//! 3b. paper_promotion      — STRATEGY-PROMOTION-REGISTRY-01D: exact (strategy_id, symbol, timeframe_secs) identity must be `active_paper`; registered+enabled is necessary but not sufficient; shared with the external signal path via `promotion_gate::evaluate_paper_promotion_gate`
//! 4.  suppression_check    — strategy must not be actively suppressed (per-strategy targeted query)
//! 5.  arm_state            — durable arm state must be ARMED
//! 6.  active_run           — active run must exist and be in "running" state
//! 7.  outbox_enqueue       — durable idempotent write (signal_source = "internal_strategy_decision")
//! ```
//!
//! This is a library function, not an HTTP handler.  Callers receive a
//! structured [`InternalDecisionOutcome`] rather than an HTTP response.
//! The function is intentionally narrow: it does not schedule, allocate, or
//! reason about alpha.

use std::collections::BTreeMap;
use std::sync::Arc;

use uuid::Uuid;

use crate::state::AppState;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// An internally-originated strategy decision submitted for validation and
/// outbox enqueue.
///
/// All string fields must be non-empty and trimmed by the caller.
#[derive(Debug, Clone)]
pub struct InternalStrategyDecision {
    /// Caller-assigned stable identity for this decision (idempotency key).
    ///
    /// Must be non-empty and unique per logical decision.  Resubmitting the
    /// same `decision_id` is safe: Gate 7 is idempotent (ON CONFLICT DO NOTHING).
    pub decision_id: String,
    /// Authoritative strategy identifier.  Must match a registered + enabled
    /// row in `sys_strategy_registry`.
    pub strategy_id: String,
    /// Ticker symbol (e.g. "AAPL").
    pub symbol: String,
    /// Canonical strategy timeframe in seconds (matches
    /// `StrategySpec::timeframe_secs`).  STRATEGY-PROMOTION-REGISTRY-01D:
    /// part of the exact `(strategy_id, symbol, timeframe_secs)` identity
    /// the paper-promotion gate (Gate 3b) checks — must be positive.
    pub timeframe_secs: i64,
    /// Order side: "buy" or "sell" (case-insensitive; normalised internally).
    pub side: String,
    /// Share quantity.  Must be positive.
    pub qty: i64,
    /// Order type: "market" or "limit".
    pub order_type: String,
    /// Time-in-force: "day", "gtc", "ioc", "fok".
    pub time_in_force: String,
    /// Limit price in cents (required when order_type == "limit").
    pub limit_price: Option<i64>,
}

/// Outcome of a single call to [`submit_internal_strategy_decision`].
#[derive(Debug, Clone)]
pub struct InternalDecisionOutcome {
    /// `true` only when Gate 7 returned `Ok(true)` (new outbox row inserted).
    /// `false` for duplicates and all gate failures.
    pub accepted: bool,
    /// Machine-readable disposition:
    ///
    /// | value              | meaning                                              |
    /// |--------------------|------------------------------------------------------|
    /// | `"accepted"`       | passed all gates; new outbox row inserted            |
    /// | `"duplicate"`      | decision_id already in outbox; no new row            |
    /// | `"rejected"`       | field validation failure, registry gate failure, or per-symbol notional cap denial (cap #3, Gate 1g) |
    /// | `"unavailable"`    | transient system state (no DB, arm-state I/O, run)   |
    /// | `"suppressed"`     | strategy is actively suppressed                      |
    /// | `"day_limit_reached"` | PT-AUTO-02 per-run intake bound exceeded          |
    /// | `"symbol_day_limit_reached"` | MULTI-SYMBOL-DAY-ORDER-CAP-01: per-symbol daily order count cap exceeded (cap #4, Gate 1f) |
    /// | `"budget_denied"`  | B6/TV-04B: capital policy present but strategy not budget-authorized |
    /// | `"policy_invalid"` | B6/TV-04B: capital policy configured but structurally invalid        |
    /// | `"sector_config_invalid"` | ETF-RISK-CLOSURE-01: `MQK_SECTOR_EXPOSURE_LIMITS_BPS` is set but malformed (Gate 1h) |
    /// | `"sector_weights_missing"` | ETF-RISK-CLOSURE-01: sector risk is enabled for this symbol's sector but live weights/marks could not be established (Gate 1h) |
    /// | `"sector_nav_unavailable"` | ETF-RISK-CLOSURE-01: sector risk is enabled but portfolio NAV is not positive (Gate 1h) |
    /// | `"sector_limit_exceeded"`  | ETF-RISK-CLOSURE-01: candidate order would exceed a configured per-sector gross exposure cap and is not risk-reducing (Gate 1h) |
    /// | `"promotion_missing"` | STRATEGY-PROMOTION-REGISTRY-01D: no promotion record exists for this exact identity (Gate 3b) |
    /// | `"promotion_shadow_only"` | Gate 3b: current state is `shadow_approved` (research/shadow only, never paper-tradable) |
    /// | `"promotion_not_active"` | Gate 3b: current state is `paper_approved` (evidence accepted, activation still required) |
    /// | `"promotion_demoted"` / `"promotion_retired"` / `"promotion_rejected"` / `"promotion_expired"` | Gate 3b: current state blocks trading |
    pub disposition: String,
    /// Echoed from [`InternalStrategyDecision::decision_id`].
    pub decision_id: String,
    /// Echoed from [`InternalStrategyDecision::strategy_id`].
    pub strategy_id: String,
    /// Active run UUID at time of processing (present from Gate 6 onwards).
    pub active_run_id: Option<Uuid>,
    /// Human-readable explanations for non-accepted outcomes.  Empty on success.
    pub blockers: Vec<String>,
}

// ---------------------------------------------------------------------------
// Implementation helpers
// ---------------------------------------------------------------------------

fn outcome(
    accepted: bool,
    disposition: &str,
    decision_id: &str,
    strategy_id: &str,
    active_run_id: Option<Uuid>,
    blockers: Vec<String>,
) -> InternalDecisionOutcome {
    InternalDecisionOutcome {
        accepted,
        disposition: disposition.to_string(),
        decision_id: decision_id.to_string(),
        strategy_id: strategy_id.to_string(),
        active_run_id,
        blockers,
    }
}

// ---------------------------------------------------------------------------
// Gate 0: field validation
// ---------------------------------------------------------------------------

/// Returns `Err(blockers)` if any required field is invalid.
fn validate_fields(d: &InternalStrategyDecision) -> Result<(), Vec<String>> {
    let mut blockers = Vec::new();

    if d.decision_id.trim().is_empty() {
        blockers.push("decision_id must not be blank".to_string());
    }
    if d.strategy_id.trim().is_empty() {
        blockers.push("strategy_id must not be blank".to_string());
    }
    if d.symbol.trim().is_empty() {
        blockers.push("symbol must not be blank".to_string());
    }
    if d.timeframe_secs <= 0 {
        blockers.push("timeframe_secs must be positive".to_string());
    }

    let side = d.side.trim().to_ascii_lowercase();
    if !matches!(side.as_str(), "buy" | "sell") {
        blockers.push("side must be one of: buy, sell".to_string());
    }

    if d.qty <= 0 {
        blockers.push("qty must be positive".to_string());
    } else if d.qty > i32::MAX as i64 {
        blockers.push("qty is out of range for broker request".to_string());
    }

    let order_type = d.order_type.trim().to_ascii_lowercase();
    if !matches!(order_type.as_str(), "market" | "limit") {
        blockers.push("order_type must be one of: market, limit".to_string());
    }

    let tif = d.time_in_force.trim().to_ascii_lowercase();
    if !matches!(tif.as_str(), "day" | "gtc" | "ioc" | "fok") {
        blockers.push("time_in_force must be one of: day, gtc, ioc, fok".to_string());
    }

    if order_type == "limit" && d.limit_price.is_none() {
        blockers.push("limit_price is required when order_type is 'limit'".to_string());
    }

    if blockers.is_empty() {
        Ok(())
    } else {
        Err(blockers)
    }
}

// ---------------------------------------------------------------------------
// order_json shape for the outbox
// ---------------------------------------------------------------------------

fn build_order_json(d: &InternalStrategyDecision) -> serde_json::Value {
    serde_json::json!({
        "symbol":         d.symbol.trim(),
        "side":           d.side.trim().to_ascii_lowercase(),
        "qty":            d.qty,
        "order_type":     d.order_type.trim().to_ascii_lowercase(),
        "time_in_force":  d.time_in_force.trim().to_ascii_lowercase(),
        "limit_price":    d.limit_price,
        "strategy_id":    d.strategy_id.trim(),
        "signal_source":  "internal_strategy_decision",
    })
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// B1C: StrategyBarResult → InternalStrategyDecision translation
// ---------------------------------------------------------------------------

/// B1C: Translate a `StrategyBarResult` from the execution loop into a list of
/// `InternalStrategyDecision`s ready for submission through
/// [`submit_internal_strategy_decision`].
///
/// # Semantics: target position → order delta
///
/// `TargetPosition.qty` is a **signed target portfolio state**, not an
/// incremental order size.  The order qty is the delta between the target and
/// the current held position:
///
/// ```text
/// delta = target.qty - current_positions[symbol]   (0 if symbol absent = flat)
/// delta > 0  →  buy  abs(delta) shares
/// delta < 0  →  sell abs(delta) shares  (only if holdings cover the sell; see B5 guard)
/// delta == 0 →  skip (already at target; no order)
/// ```
///
/// Callers must pass an authoritative `current_positions` map derived from the
/// most recent execution snapshot.  A symbol absent from the map is treated as
/// flat (qty = 0) — correct for symbols with no open position.
///
/// # Fail-closed rules
///
/// - `result.intents.should_execute()` is `false` (shadow mode) → returns empty.
/// - `result.intents.output.targets` is empty → returns empty (no-op bar).
/// - Delta == 0 for a target → skipped (already at target; no order needed).
/// - **B5 short-sale guard**: `delta < 0` AND `current <= 0` → skipped (no long
///   position to sell against; would open a short, which the native strategy
///   runtime does not support).
/// - **B5 short-sale guard**: `delta < 0` AND `abs(delta) > current` → skipped
///   (sell would exceed existing long holdings, driving the position net-short;
///   not supported by this runtime).
///
/// # B5 rationale
///
/// The native strategy runtime tracks portfolio positions but does not manage
/// short-position lifecycle (margin, borrow, cover semantics).  A sell decision
/// that would result in a net-short position is silently dropped here rather
/// than forwarded to the broker where it would either be rejected (causing
/// visible broker error) or filled (resulting in a short position the runtime
/// cannot safely manage).  Fail-closed: skip the unsupported intent rather than
/// propagate it.
///
/// # Output fields
///
/// | Source                | Decision field      | Value                    |
/// |-----------------------|---------------------|--------------------------|
/// | `target.symbol`       | `symbol`            | as-is                    |
/// | `delta > 0`           | `side`              | `"buy"`                  |
/// | `delta < 0`           | `side`              | `"sell"`                 |
/// | `abs(delta)`          | `qty`               | positive share count     |
/// | —                     | `order_type`        | `"market"`               |
/// | —                     | `time_in_force`     | `"day"`                  |
/// | —                     | `limit_price`       | `None`                   |
///
/// `decision_id` is a UUIDv5 derived from
/// `"mqk.strategy-decision.v3|{run_id}|{strategy_id}|{symbol}|{timeframe_secs}|{target_qty}|{bar_end_ts}"`
/// where `target_qty` is the strategy's raw *signed target position*
/// (`TargetPosition::qty`, before subtracting `current`) and `bar_end_ts` is
/// the exact completed-bar identity (`EvaluatedBarFacts::bar_end_ts`) this
/// decision was evaluated against
/// (STRATEGY-DECISION-IDEMPOTENCY-01/STRATEGY-DECISION-ECONOMIC-IDEMPOTENCY-02).
///
/// This parameter is named `bar_end_ts`, not `now_micros`, deliberately:
/// the 1-second execution loop tick may re-run the same strategy evaluation
/// against the same still-current completed bar many times before a new bar
/// closes (e.g. while an earlier decision for that bar has not yet resolved
/// to a terminal broker outcome). Seeding this identity from wall-clock time
/// (the prior behavior) made every such re-evaluation produce a distinct
/// `decision_id`, defeating the outbox's `ON CONFLICT DO NOTHING` dedup
/// entirely — a real duplicate-live-order path, bounded only by
/// `MAX_AUTONOMOUS_SIGNALS_PER_RUN`, not by design.
///
/// # -02: identity is the target, not the derived delta
///
/// -01 anchored identity on `bar_end_ts` but still included the *derived*
/// order quantity (`delta = target.qty - current`) and `side`. That
/// reintroduces the same class of bug one level down: `current` is read
/// from the live portfolio snapshot, which moves the instant the strategy's
/// own already-working order for this exact bar/target receives a partial
/// fill — so re-evaluating the *same* bar/target after a partial fill
/// computed a *different* `delta`, and therefore a *different*
/// `decision_id`, for what is economically the identical intent. That let a
/// second order reach the outbox for the remaining (post-partial-fill)
/// delta while the original order's remaining quantity was still working —
/// two live orders racing to fill the same target.
///
/// Anchoring on `target_qty` instead (the strategy's target, which by
/// construction does not move when a fill against the current in-flight
/// order updates `current`) means the same logical intent — same run,
/// strategy, symbol, timeframe, completed bar, target — always produces the
/// same `decision_id` regardless of how `current`/`delta` drift while that
/// intent's order is still working. Because `decision_id` is also the
/// outbox `idempotency_key` (Gate 7, `ON CONFLICT (idempotency_key) DO
/// NOTHING`), this makes "at most one durable order per intent" a property
/// the database enforces directly, not something every caller must
/// separately reason about — the *first* delta computed for an intent is
/// the one, and only one, that is ever durably submitted; a later
/// re-evaluation computing a smaller delta (because part of the first order
/// already filled) is rejected as a duplicate before it can add exposure
/// beyond the original target. See `runtime_opportunity_allocation.rs`'s
/// `compute_cycle_id` for the same bar-anchored-identity pattern applied to
/// the allocator's economic-cycle identity.
///
/// This function is pure (no IO, no state mutation) and exported for test
/// isolation.
pub fn bar_result_to_decisions(
    result: &mqk_strategy::StrategyBarResult,
    run_id: Uuid,
    bar_end_ts: i64,
    current_positions: &BTreeMap<String, i64>,
) -> Vec<InternalStrategyDecision> {
    if !result.intents.should_execute() {
        return vec![];
    }
    let strategy_id = result.spec.name.clone();
    result
        .intents
        .output
        .targets
        .iter()
        .filter_map(|t| {
            // Delta-to-target: TargetPosition.qty is a target portfolio state,
            // not an incremental order size.  Symbols absent from the map are
            // treated as flat (current = 0).
            let current = current_positions.get(&t.symbol).copied().unwrap_or(0);
            let delta = t.qty - current;
            if delta == 0 {
                return None; // already at target; no order needed
            }
            // SHORT-SIDE-INTENT-MODEL-01: classify intent explicitly so the
            // control plane distinguishes sell-to-close from short-open.
            // Short-open and sell-beyond-long are blocked fail-closed (B5 backstop).
            // Call evaluate_short_entry_policy directly for policy diagnostics; see
            // scenario_short_side_intent_model_01 for integrated proof tests.
            let intent = crate::capital_policy::classify_order_intent(current, delta);
            let (side, qty) = match intent {
                crate::capital_policy::OrderIntent::LongOpen
                | crate::capital_policy::OrderIntent::BuyToCover
                | crate::capital_policy::OrderIntent::BuyToFlat
                | crate::capital_policy::OrderIntent::BuyBeyondShortToLong => {
                    ("buy".to_string(), delta)
                }
                crate::capital_policy::OrderIntent::SellToClose
                | crate::capital_policy::OrderIntent::SellToFlat => ("sell".to_string(), -delta),
                crate::capital_policy::OrderIntent::ShortOpen
                | crate::capital_policy::OrderIntent::SellBeyondLongToShort
                | crate::capital_policy::OrderIntent::NoOp => return None,
            };
            // STRATEGY-DECISION-ECONOMIC-IDEMPOTENCY-02: identity is anchored
            // on the strategy's raw signed TARGET (`t.qty`), never on the
            // derived `delta`/`side` — see the doc comment above for why.
            let decision_id = Uuid::new_v5(
                &Uuid::NAMESPACE_DNS,
                format!(
                    "mqk.strategy-decision.v3|{run_id}|{strategy_id}|{symbol}|{timeframe_secs}|{target_qty}|{bar_end_ts}",
                    symbol = t.symbol,
                    timeframe_secs = result.spec.timeframe_secs,
                    target_qty = t.qty,
                )
                .as_bytes(),
            )
            .to_string();
            Some(InternalStrategyDecision {
                decision_id,
                strategy_id: strategy_id.clone(),
                symbol: t.symbol.clone(),
                timeframe_secs: result.spec.timeframe_secs,
                side,
                qty,
                order_type: "market".to_string(),
                time_in_force: "day".to_string(),
                limit_price: None,
            })
        })
        .collect()
}

/// STRATEGY-DECISION-IDEMPOTENCY-01: the one production seam that decides
/// whether a bar evaluation's decisions are safe to compute at all, given
/// whatever completed-bar-identity evidence (`EvaluatedBarFacts`) the
/// dispatch pipeline was able to establish for this tick.
///
/// `bar_facts.is_none()` means the legacy stub-context fallback fired
/// (`AppState::dispatch_native_strategy_for_symbol_with_bar_and_facts`'s
/// no-DB-context branch, which always yields `is_complete=false` and
/// therefore empty `targets` in practice) -- there is no durably provable
/// bar identity to anchor `decision_id` on. Rather than fall back to
/// wall-clock time (which would silently reintroduce the exact duplicate-
/// decision-id bug STRATEGY-DECISION-IDEMPOTENCY-01 closes), this refuses
/// to produce any decision at all when facts are missing and the strategy
/// nonetheless produced a nonzero target -- a structural fail-closed
/// backstop, not the expected path.
pub fn decisions_from_bar_facts(
    result: &mqk_strategy::StrategyBarResult,
    run_id: Uuid,
    bar_facts: Option<&crate::state::EvaluatedBarFacts>,
    current_positions: &BTreeMap<String, i64>,
) -> Vec<InternalStrategyDecision> {
    match bar_facts {
        Some(facts) => bar_result_to_decisions(result, run_id, facts.bar_end_ts, current_positions),
        None => {
            if !result.intents.output.targets.is_empty() {
                tracing::error!(
                    run_id = %run_id,
                    "decision_id_bar_anchor_missing: strategy produced target(s) with no \
                     provable completed-bar identity (bar_facts unavailable); refusing to \
                     submit any decision this tick"
                );
            }
            Vec::new()
        }
    }
}

/// Validate an internally-originated strategy decision against the canonical
/// registry and enqueue it to the durable outbox path.
///
/// The gate sequence is strictly ordered (fail-fast).  See module docs for
/// the full sequence and disposition values.
///
/// This function is `async` because Gates 2–7 require DB and state reads.
/// It does NOT hold the lifecycle mutex lock for its entire duration —
/// lifecycle_guard is not acquired here because this is not an HTTP handler
/// and its callers are expected to manage concurrency at a higher level.
pub async fn submit_internal_strategy_decision(
    state: &Arc<AppState>,
    decision: InternalStrategyDecision,
) -> InternalDecisionOutcome {
    let did = decision.decision_id.trim().to_string();
    let sid = decision.strategy_id.trim().to_string();

    // Gate 0: field validation.
    if let Err(blockers) = validate_fields(&decision) {
        return outcome(false, "rejected", &did, &sid, None, blockers);
    }

    // Gate 1: PT-AUTO-02 per-run signal intake bound.
    if state.day_signal_limit_exceeded() {
        return outcome(
            false,
            "day_limit_reached",
            &did,
            &sid,
            None,
            vec![format!(
                "internal decision refused: autonomous day signal limit reached \
                 ({} signals accepted this run); \
                 no further decisions will be accepted until the next run start",
                state.day_signal_count()
            )],
        );
    }

    // Gate 1f: MULTI-SYMBOL-DAY-ORDER-CAP-01 — optional per-symbol daily order
    // count cap (cap #4, design doc §6). Disabled (None) unless
    // MQK_PER_SYMBOL_DAY_ORDER_LIMIT is set; in that case Gate 1 above always
    // passes through unaffected — this is an additive, independent counter.
    if state
        .symbol_day_order_limit_exceeded(&decision.symbol)
        .await
    {
        return outcome(
            false,
            "symbol_day_limit_reached",
            &did,
            &sid,
            None,
            vec![format!(
                "internal decision refused: per-symbol daily order count limit reached for {} \
                 ({} orders accepted this run for this symbol); \
                 no further decisions for this symbol will be accepted until the next run start",
                decision.symbol.trim(),
                state.symbol_day_order_count(&decision.symbol).await
            )],
        );
    }

    // Gate 1e: B6 — TV-04B per-strategy capital budget authorization.
    //
    // Applies the same capital budget gate that the external signal path
    // (POST /api/v1/strategy/signal Gate 1e) enforces.  Without this gate,
    // a strategy can be budget-denied for external signals yet still have its
    // internally-generated bar decisions reach the durable outbox.
    //
    // Placed before Gate 2 (DB) because budget denial is a pure filesystem
    // check — cheaper than DB operations, and budget-denied decisions must
    // never consume DB quota or advance the day signal counter.
    //
    // PolicyNotConfigured → no budget enforcement active; pass through.
    // BudgetAuthorized    → explicit strategy budget authorization; pass.
    // BudgetDenied        → strategy not capital-authorized; fail-closed.
    // PolicyInvalid       → policy configured but structurally invalid; fail-closed.
    {
        use crate::capital_policy::{evaluate_strategy_budget_from_env, StrategyBudgetOutcome};
        let budget = evaluate_strategy_budget_from_env(&sid);
        if !budget.is_signal_safe() {
            let (disposition, blocker) = match &budget {
                StrategyBudgetOutcome::BudgetDenied { reason } => (
                    "budget_denied",
                    format!("internal decision refused: {reason}"),
                ),
                StrategyBudgetOutcome::PolicyInvalid { reason } => (
                    "policy_invalid",
                    format!(
                        "internal decision unavailable: capital allocation policy \
                         is configured but invalid: {reason}"
                    ),
                ),
                _ => (
                    "unavailable",
                    "internal decision unavailable: capital policy evaluation failed".to_string(),
                ),
            };
            // DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: alert on budget denial from
            // the internal (B1C loop) path — same high-value signal as Gate 1e.
            if disposition == "budget_denied" {
                let notifier = state.discord_notifier.clone();
                let env = Some(state.deployment_mode().as_api_label().to_string());
                let blocker_copy = blocker.clone();
                let sid_copy = sid.clone();
                tokio::spawn(async move {
                    notifier
                        .notify_trade_event(&crate::notify::TradeEventPayload {
                            stage: "signal.blocked".to_string(),
                            run_id: None,
                            symbol: None,
                            side: None,
                            qty: None,
                            price_micros: None,
                            order_id: None,
                            detail: Some(format!(
                                "gate=gate_1e_budget path=internal_decision \
                                 strategy={sid_copy} reason={blocker_copy}"
                            )),
                            environment: env,
                            summary: format!(
                                "signal.blocked [budget_denied] internal decision \
                                 strategy={sid_copy} | {blocker_copy}"
                            ),
                            ts_utc: chrono::Utc::now().to_rfc3339(), // allow: ops-metadata notification timestamp
                        })
                        .await;
                });
            }
            return outcome(false, disposition, &did, &sid, None, vec![blocker]);
        }
    }

    // Gate 1g: MULTI-SYMBOL-CAPITAL-CAPS-01 — optional per-symbol notional cap
    // (cap #3, design doc §6 "Cap #3 — per_symbol_max_notional_usd").
    //
    // Disabled (NoSizingConstraint) unless MQK_PER_SYMBOL_MAX_NOTIONAL_USD is
    // set to a positive number.
    //
    // Honest gap: implied notional is only computable for limit orders
    // (qty x limit_price). B1C (bar_result_to_decisions) always sets
    // order_type="market" / limit_price=None, so this gate is
    // SizingUnverifiable (pass-through) for every B1C-originated decision —
    // dormant in practice until limit-order support exists for the internal
    // decision path. Proven via direct construction of a limit-order
    // InternalStrategyDecision in scenario_multi_symbol_capital_caps_01.rs.
    //
    // NoSizingConstraint        -> cap disabled or order within cap; pass.
    // SizingUnverifiable        -> market order; pass (honest, cannot deny
    //                               the unmeasured).
    // SizingDeniedPerSymbolCap  -> over cap; "rejected" with a blocker naming
    //                               the cap (design doc §6 cap #3 test).
    {
        use crate::capital_policy::{
            evaluate_per_symbol_notional_cap_from_env, PositionSizingOutcome,
        };
        let sizing = evaluate_per_symbol_notional_cap_from_env(
            &decision.symbol,
            decision.qty,
            decision.limit_price,
        );
        if let PositionSizingOutcome::SizingDeniedPerSymbolCap {
            symbol,
            implied_notional_usd,
            cap_usd,
        } = &sizing
        {
            let blocker = format!(
                "internal decision refused: symbol '{symbol}' implied notional \
                 ${implied_notional_usd:.2} exceeds per_symbol_max_notional_usd=${cap_usd:.2} \
                 (MQK_PER_SYMBOL_MAX_NOTIONAL_USD)"
            );
            // DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: alert on per-symbol
            // notional cap denial — same high-value signal as Gate 1e/1f.
            let notifier = state.discord_notifier.clone();
            let env = Some(state.deployment_mode().as_api_label().to_string());
            let symbol_owned = symbol.clone();
            let blocker_copy = blocker.clone();
            tokio::spawn(async move {
                notifier
                    .notify_trade_event(&crate::notify::TradeEventPayload {
                        stage: "signal.blocked".to_string(),
                        run_id: None,
                        symbol: Some(symbol_owned.clone()),
                        side: None,
                        qty: None,
                        price_micros: None,
                        order_id: None,
                        detail: Some(format!(
                            "gate=gate_1g_per_symbol_notional_cap path=internal_decision \
                             reason={blocker_copy}"
                        )),
                        environment: env,
                        summary: format!(
                            "signal.blocked [sizing_denied_per_symbol_cap] internal decision \
                             symbol={symbol_owned} | {blocker_copy}"
                        ),
                        ts_utc: chrono::Utc::now().to_rfc3339(), // allow: ops-metadata notification timestamp
                    })
                    .await;
            });
            return outcome(false, "rejected", &did, &sid, None, vec![blocker]);
        }
    }

    // Gate 1h: ETF-RISK-CLOSURE-01 / ETF-RISK-EXTERNAL-SIGNAL-GATE-01 —
    // optional per-sector live gross exposure cap
    // (`MQK_SECTOR_EXPOSURE_LIMITS_BPS`).
    //
    // Default-off: an unset/empty env var disables this gate entirely and
    // the decision path never touches the DB or the instrument registry for
    // this check (mirrors Gate 1g's MQK_PER_SYMBOL_MAX_NOTIONAL_USD shape —
    // same env-var-driven, no-cap-means-no-check pattern).
    //
    // The registry/snapshot/marks glue and the fail-closed rules (missing
    // snapshot/DB/mark/NAV never fabricate a price or treat a gap as zero)
    // live in `capital_policy::sector_risk_gate::evaluate_sector_risk_gate`,
    // shared with the external signal path's gate in `routes/strategy.rs` —
    // one mechanism, two callers, so behavior cannot drift between an
    // internally-generated order and an externally-submitted signal.
    {
        let result = crate::capital_policy::sector_risk_gate::evaluate_sector_risk_gate(
            state,
            decision.symbol.trim(),
            decision.side.trim(),
            decision.qty,
        )
        .await;

        if !result.allowed {
            let prefix = if matches!(
                result.reason_code.as_str(),
                "sector_config_invalid" | "unavailable"
            ) {
                "internal decision unavailable"
            } else {
                "internal decision refused"
            };
            let blocker = format!(
                "{prefix}: {}",
                result
                    .message
                    .clone()
                    .unwrap_or_else(|| format!("sector risk gate denied ({})", result.reason_code))
            );
            let notifier = state.discord_notifier.clone();
            let env = Some(state.deployment_mode().as_api_label().to_string());
            let symbol_owned = decision.symbol.trim().to_string();
            let blocker_copy = blocker.clone();
            let reason_code_copy = result.reason_code.clone();
            tokio::spawn(async move {
                notifier
                    .notify_trade_event(&crate::notify::TradeEventPayload {
                        stage: "signal.blocked".to_string(),
                        run_id: None,
                        symbol: Some(symbol_owned.clone()),
                        side: None,
                        qty: None,
                        price_micros: None,
                        order_id: None,
                        detail: Some(format!(
                            "gate=gate_1h_sector_risk path=internal_decision \
                             reason={blocker_copy}"
                        )),
                        environment: env,
                        summary: format!(
                            "signal.blocked [{reason_code_copy}] internal decision \
                             symbol={symbol_owned} | {blocker_copy}"
                        ),
                        ts_utc: chrono::Utc::now().to_rfc3339(), // allow: ops-metadata notification timestamp
                    })
                    .await;
            });
            return outcome(false, &result.reason_code, &did, &sid, None, vec![blocker]);
        }
    }

    // Gate 2: DB must be present.
    let Some(db) = state.db.as_ref() else {
        return outcome(
            false,
            "unavailable",
            &did,
            &sid,
            None,
            vec!["durable execution DB truth is unavailable on this daemon".to_string()],
        );
    };

    // Gate 3: strategy must be registered and enabled in sys_strategy_registry.
    match mqk_db::fetch_strategy_registry_entry(db, &sid).await {
        Ok(Some(record)) if record.enabled => {
            // Pass — registered and enabled.
        }
        Ok(Some(_record)) => {
            return outcome(
                false,
                "rejected",
                &did,
                &sid,
                None,
                vec![format!(
                    "internal decision refused: strategy '{sid}' is registered but disabled \
                     in the strategy registry"
                )],
            );
        }
        Ok(None) => {
            return outcome(
                false,
                "rejected",
                &did,
                &sid,
                None,
                vec![format!(
                    "internal decision refused: strategy '{sid}' is not registered \
                     in the strategy registry"
                )],
            );
        }
        Err(err) => {
            return outcome(
                false,
                "unavailable",
                &did,
                &sid,
                None,
                vec![format!(
                    "internal decision unavailable: registry lookup failed: {err}"
                )],
            );
        }
    }

    // Gate 3b: STRATEGY-PROMOTION-REGISTRY-01D — strategy must be
    // paper-promoted (active_paper) for this exact
    // (strategy_id, symbol, timeframe_secs) identity.
    //
    // Registered + enabled (Gate 3 above) is necessary but never
    // sufficient for paper trading: a strategy can be registered and
    // enabled yet have no promotion record at all, or a shadow/demoted/
    // retired/rejected/expired one, and must still be refused here.
    // Shared with the external signal path (routes/strategy.rs) via
    // `promotion_gate::evaluate_paper_promotion_gate` — one mechanism, two
    // callers, so promotion enforcement cannot drift between an
    // internally-generated order and an externally-submitted signal.
    {
        let promotion = crate::promotion_gate::evaluate_paper_promotion_gate(
            db,
            crate::promotion_gate::PromotionRunMode::from(state.deployment_mode()),
            &sid,
            decision.symbol.trim(),
            decision.timeframe_secs,
        )
        .await;
        if !promotion.paper_tradable {
            let disposition = match promotion.reason_code {
                mqk_db::PromotionReasonCode::PromotionDbUnavailable
                | mqk_db::PromotionReasonCode::PromotionQueryFailed => "unavailable",
                other => other.code(),
            };
            return outcome(
                false,
                disposition,
                &did,
                &sid,
                None,
                vec![format!("internal decision refused: {}", promotion.blocker)],
            );
        }
    }

    // Gate 4: strategy must not be actively suppressed.
    //
    // Uses a targeted per-strategy query so the decision seam does not load
    // all suppressions for all strategies on every call.  Fail-closed:
    // if the suppression truth is unavailable the decision is refused.
    match mqk_db::fetch_active_suppression_for_strategy(db, &sid).await {
        Ok(Some(sup)) => {
            return outcome(
                false,
                "suppressed",
                &did,
                &sid,
                None,
                vec![format!(
                    "internal decision refused: strategy '{sid}' is suppressed \
                     ({}: {})",
                    sup.trigger_domain, sup.trigger_reason
                )],
            );
        }
        Ok(None) => {
            // No active suppression — pass.
        }
        Err(err) => {
            return outcome(
                false,
                "unavailable",
                &did,
                &sid,
                None,
                vec![format!(
                    "internal decision unavailable: suppression check failed: {err}"
                )],
            );
        }
    }

    // Gate 5: durable arm state must be ARMED.
    let (durable_arm_state, durable_arm_reason) = match mqk_db::load_arm_state(db).await {
        Ok(Some((s, r))) => (s, r),
        Ok(None) => {
            return outcome(
                false,
                "rejected",
                &did,
                &sid,
                None,
                vec![
                    "internal decision refused: durable arm state is not armed; \
                      fresh systems default to disarmed until explicitly armed"
                        .to_string(),
                ],
            );
        }
        Err(err) => {
            return outcome(
                false,
                "unavailable",
                &did,
                &sid,
                None,
                vec![format!(
                    "internal decision unavailable: arm-state truth could not be loaded: {err}"
                )],
            );
        }
    };

    if durable_arm_state != "ARMED" {
        let blocker = match durable_arm_reason.as_deref() {
            Some("OperatorHalt") => {
                "internal decision refused: durable arm state is halted".to_string()
            }
            Some(reason) => {
                format!("internal decision refused: durable arm state is disarmed ({reason})")
            }
            None => "internal decision refused: durable arm state is not armed".to_string(),
        };
        return outcome(false, "rejected", &did, &sid, None, vec![blocker]);
    }

    // Gate 6: active run must exist and be in "running" state.
    let status = match state.current_status_snapshot().await {
        Ok(s) => s,
        Err(err) => {
            return outcome(
                false,
                "unavailable",
                &did,
                &sid,
                None,
                vec![err.to_string()],
            );
        }
    };

    let Some(active_run_id) = status.active_run_id else {
        return outcome(
            false,
            "unavailable",
            &did,
            &sid,
            None,
            vec!["internal decision refused: no active durable run is available".to_string()],
        );
    };

    if status.state != "running" {
        let mut blockers = vec![format!(
            "internal decision refused: runtime state '{}' is not accepting decisions",
            status.state
        )];
        if let Some(note) = status.notes {
            blockers.push(note);
        }
        return outcome(
            false,
            "unavailable",
            &did,
            &sid,
            Some(active_run_id),
            blockers,
        );
    }

    // Gate 7: enqueue to outbox (idempotent).
    let order_json = build_order_json(&decision);
    match mqk_db::outbox_enqueue(db, active_run_id, &did, order_json).await {
        Ok(true) => {
            // PT-AUTO-02: count only new enqueues; duplicates do not consume quota.
            state.increment_day_signal_count();
            // MULTI-SYMBOL-DAY-ORDER-CAP-01: per-symbol counterpart (cap #4),
            // incremented alongside the account-wide counter above.
            state
                .increment_symbol_day_order_count(&decision.symbol)
                .await;
            outcome(true, "accepted", &did, &sid, Some(active_run_id), vec![])
        }
        Ok(false) => outcome(
            false,
            "duplicate",
            &did,
            &sid,
            Some(active_run_id),
            vec![format!(
                "decision_id '{did}' already exists in outbox; no new row was created"
            )],
        ),
        Err(err) => outcome(
            false,
            "unavailable",
            &did,
            &sid,
            Some(active_run_id),
            vec![format!("outbox enqueue failed: {err}")],
        ),
    }
}
