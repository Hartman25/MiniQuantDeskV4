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
//! 2.  db_present           — no DB → unavailable
//! 3.  registry_check       — strategy must be registered AND enabled
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
/// `"{run_id}:{strategy_id}:{symbol}:{side}:{qty}:{now_micros}"` where `qty`
/// is the absolute delta — idempotent across crash-restart within the same
/// microsecond window.
///
/// This function is pure (no IO, no state mutation) and exported for test
/// isolation.
pub fn bar_result_to_decisions(
    result: &mqk_strategy::StrategyBarResult,
    run_id: Uuid,
    now_micros: i64,
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
            let decision_id = Uuid::new_v5(
                &Uuid::NAMESPACE_DNS,
                format!(
                    "{run_id}:{strategy_id}:{symbol}:{side}:{qty}:{now_micros}",
                    symbol = t.symbol
                )
                .as_bytes(),
            )
            .to_string();
            Some(InternalStrategyDecision {
                decision_id,
                strategy_id: strategy_id.clone(),
                symbol: t.symbol.clone(),
                side,
                qty,
                order_type: "market".to_string(),
                time_in_force: "day".to_string(),
                limit_price: None,
            })
        })
        .collect()
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
