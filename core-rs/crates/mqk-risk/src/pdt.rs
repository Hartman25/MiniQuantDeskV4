//! PDT (Pattern Day Trader) enforcement policy and helpers.
//!
//! # Regulatory Background
//! FINRA Rule 4210 defines a *pattern day trader* as any customer who executes
//! four or more *day trades* within five business days, **provided** the number
//! of day trades is more than six percent of total trades in that five-day
//! period.  A flagged PDT account must maintain a minimum equity of $25,000;
//! otherwise the broker restricts the account to closing-only orders.
//!
//! # Design
//! This module provides **explicit, deterministic** PDT helpers that are
//! *separate* from the generic risk limits in `engine.rs`.  The risk engine's
//! `PdtContext { pdt_ok: bool }` is produced by calling [`evaluate_pdt`] and
//! reading [`PdtDecision::trading_allowed`], keeping policy logic here and the
//! risk engine lean.
//!
//! All arithmetic uses `u32` day-trade counts and fixed-point micros (`i64`)
//! so there is no floating-point in the hot path.
//!
//! # Units
//! - Equity amounts are in **micros** (1 USD = 1_000_000 micros).
//! - Day IDs are caller-supplied opaque `u32` values (e.g. `YYYYMMDD`).
//! - The rolling window is `window_days` calendar-day IDs wide.

use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// FINRA threshold: four or more day trades in five business days triggers PDT.
pub const PDT_DAY_TRADE_THRESHOLD: u32 = 4;

/// FINRA minimum equity to trade when flagged as a PDT account (in micros).
/// $25,000 × 1_000_000.
pub const PDT_MIN_EQUITY_MICROS: i64 = 25_000 * 1_000_000;

/// Default rolling window width in trading days.
pub const PDT_DEFAULT_WINDOW_DAYS: u32 = 5;

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

/// PDT enforcement policy configuration.
///
/// Separate from [`crate::RiskConfig`] so that PDT concerns do not bleed into
/// the generic limit engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdtPolicy {
    /// Enable PDT enforcement.  When `false`, [`evaluate_pdt`] always returns
    /// `trading_allowed = true` (used in paper / backtest modes).
    pub enabled: bool,

    /// Number of consecutive trading-day IDs to include in the rolling window.
    /// FINRA uses 5; this is configurable for testing.
    pub window_days: u32,

    /// Max day trades permitted in the window before the account is restricted.
    /// FINRA threshold is 4 (i.e., ≥ 4 triggers PDT).  We default to 3 so
    /// that reaching 3 is still `ok` but adding a 4th trade blocks.
    ///
    /// Set to `PDT_DAY_TRADE_THRESHOLD - 1` (= 3) to match FINRA exactly.
    pub max_day_trades_in_window: u32,

    /// Minimum equity the account must hold when flagged PDT.
    /// Set to `PDT_MIN_EQUITY_MICROS` for FINRA compliance.
    pub min_equity_micros: i64,
}

impl PdtPolicy {
    /// FINRA-compliant defaults.
    pub fn finra_defaults() -> Self {
        Self {
            enabled: true,
            window_days: PDT_DEFAULT_WINDOW_DAYS,
            // ≥ 4 triggers PDT, so 3 is the maximum allowed without restriction.
            max_day_trades_in_window: PDT_DAY_TRADE_THRESHOLD - 1,
            min_equity_micros: PDT_MIN_EQUITY_MICROS,
        }
    }

    /// Permissive policy for paper / backtest mode (enforcement off).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            window_days: PDT_DEFAULT_WINDOW_DAYS,
            max_day_trades_in_window: u32::MAX,
            min_equity_micros: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Mutable PDT state maintained by the runtime across ticks.
///
/// Keyed by day ID so the rolling window can be computed deterministically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdtState {
    /// Day-trade count per trading day.  Key = day_id (e.g. `20260216`).
    /// Old entries outside the rolling window are pruned on each [`tick_pdt`]
    /// call so memory usage is bounded.
    pub day_trade_counts: BTreeMap<u32, u32>,

    /// Whether the account is currently flagged as a PDT account.
    /// Set to `true` once the day-trade count in the rolling window exceeds
    /// `policy.max_day_trades_in_window`.  Cleared only by an explicit call
    /// to [`clear_pdt_flag`] (e.g. after equity is restored above the minimum).
    pub flagged_pdt: bool,
}

impl PdtState {
    /// Create a fresh state with no trading history.
    pub fn new() -> Self {
        Self {
            day_trade_counts: BTreeMap::new(),
            flagged_pdt: false,
        }
    }
}

impl Default for PdtState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Input / Output
// ---------------------------------------------------------------------------

/// Context for a single PDT evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdtInput {
    /// Current trading day ID (opaque; must be monotonically non-decreasing).
    pub day_id: u32,

    /// Current account equity in micros.
    pub equity_micros: i64,

    /// `true` if the trade the caller is about to submit would count as a
    /// *day trade* under FINRA rules (same-session open + close of a position).
    /// The caller is responsible for tracking this; the PDT engine records it.
    pub is_day_trade: bool,
}

/// Outcome of a PDT evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdtDecision {
    /// Whether the proposed trade is allowed under PDT rules.
    /// Feed this into [`crate::PdtContext::pdt_ok`].
    pub trading_allowed: bool,

    /// Human-readable reason for the decision.
    pub reason: PdtReason,

    /// Rolling window day-trade count at the time of evaluation.
    pub window_day_trade_count: u32,
}

/// Reason codes for PDT decisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdtReason {
    /// PDT enforcement is disabled.
    EnforcementDisabled,
    /// Trade allowed; count is within the permitted window.
    AllowedWithinLimit,
    /// Trade allowed because it is not a day trade.
    AllowedNotDayTrade,
    /// Trade blocked: adding this day trade would exceed the window limit.
    BlockedWouldExceedLimit,
    /// Trade blocked: account is already flagged PDT and equity is below minimum.
    BlockedFlaggedBelowMinEquity,
    /// Trade blocked: account is already flagged PDT (equity above minimum, but
    /// the account requires broker-level clearance to resume).
    BlockedFlaggedPdt,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the rolling window day-trade count for the strict calendar range
/// `[current_day_id - (window_days - 1), current_day_id]`.
///
/// Day IDs are `YYYYMMDD` integers.  Integer subtraction is used for the
/// floor, which is correct for any same-month span; cross-month spans in
/// tests should use realistic date values to stay accurate.
fn rolling_count(counts: &BTreeMap<u32, u32>, current_day_id: u32, window_days: u32) -> u32 {
    if window_days == 0 {
        return 0;
    }
    // Floor = first day_id still inside the window.
    let floor = current_day_id.saturating_sub(window_days - 1);
    counts.range(floor..=current_day_id).map(|(_, &v)| v).sum()
}

/// Prune entries older than the rolling window from `state.day_trade_counts`.
///
/// Removes all keys with `day_id < current_day_id - (window_days - 1)`.
pub fn prune_old_days(state: &mut PdtState, current_day_id: u32, window_days: u32) {
    if window_days == 0 {
        state.day_trade_counts.clear();
        return;
    }
    let floor = current_day_id.saturating_sub(window_days - 1);
    state.day_trade_counts.retain(|&k, _| k >= floor);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Tick the PDT state for the current evaluation (pruning + peak tracking).
///
/// Call this once per evaluation, before [`evaluate_pdt`], so that the
/// rolling window is always clean.
pub fn tick_pdt(policy: &PdtPolicy, state: &mut PdtState, input: &PdtInput) {
    prune_old_days(state, input.day_id, policy.window_days);
}

/// Evaluate whether the proposed trade is allowed under PDT rules.
///
/// This is **pure** and does **not** mutate `state` — call [`record_day_trade`]
/// separately if the trade proceeds, to keep the decision and side-effect apart.
pub fn evaluate_pdt(policy: &PdtPolicy, state: &PdtState, input: &PdtInput) -> PdtDecision {
    // 1) Enforcement disabled → always allow.
    if !policy.enabled {
        return PdtDecision {
            trading_allowed: true,
            reason: PdtReason::EnforcementDisabled,
            window_day_trade_count: rolling_count(
                &state.day_trade_counts,
                input.day_id,
                policy.window_days,
            ),
        };
    }

    let window_count = rolling_count(&state.day_trade_counts, input.day_id, policy.window_days);

    // 2) Already flagged PDT.
    if state.flagged_pdt {
        if input.equity_micros < policy.min_equity_micros {
            return PdtDecision {
                trading_allowed: false,
                reason: PdtReason::BlockedFlaggedBelowMinEquity,
                window_day_trade_count: window_count,
            };
        }
        // Flagged but equity is sufficient — still blocked pending clearance.
        return PdtDecision {
            trading_allowed: false,
            reason: PdtReason::BlockedFlaggedPdt,
            window_day_trade_count: window_count,
        };
    }

    // 3) Not a day trade → allow unconditionally.
    if !input.is_day_trade {
        return PdtDecision {
            trading_allowed: true,
            reason: PdtReason::AllowedNotDayTrade,
            window_day_trade_count: window_count,
        };
    }

    // 4) Day trade: would this push count over the limit?
    let projected = window_count.saturating_add(1);
    if projected > policy.max_day_trades_in_window {
        return PdtDecision {
            trading_allowed: false,
            reason: PdtReason::BlockedWouldExceedLimit,
            window_day_trade_count: window_count,
        };
    }

    // 5) Allowed.
    PdtDecision {
        trading_allowed: true,
        reason: PdtReason::AllowedWithinLimit,
        window_day_trade_count: window_count,
    }
}

/// Record that a day trade occurred on `day_id`.
///
/// Call this **after** the trade is confirmed to have executed (not on
/// intention), so counts are accurate.  If `is_day_trade` is false this is a
/// no-op.
///
/// Also sets `state.flagged_pdt = true` if the updated rolling count exceeds
/// `policy.max_day_trades_in_window`.
pub fn record_day_trade(policy: &PdtPolicy, state: &mut PdtState, day_id: u32) {
    *state.day_trade_counts.entry(day_id).or_insert(0) += 1;

    let window_count = rolling_count(&state.day_trade_counts, day_id, policy.window_days);
    if window_count > policy.max_day_trades_in_window {
        state.flagged_pdt = true;
    }
}

/// Explicitly clear the PDT flag (e.g. after the broker confirms equity
/// has been restored above the minimum and the account is cleared to trade).
///
/// This does NOT reset the day-trade counts — those persist across the window.
pub fn clear_pdt_flag(state: &mut PdtState) {
    state.flagged_pdt = false;
}

/// Convert a [`PdtDecision`] into the [`crate::PdtContext`] the risk engine
/// expects.  This is the bridge between the explicit PDT module and the generic
/// risk engine.
pub fn to_pdt_context(decision: &PdtDecision) -> crate::PdtContext {
    crate::PdtContext {
        pdt_ok: decision.trading_allowed,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> PdtPolicy {
        PdtPolicy::finra_defaults()
    }

    fn state() -> PdtState {
        PdtState::new()
    }

    fn input(day_id: u32, equity_micros: i64, is_day_trade: bool) -> PdtInput {
        PdtInput {
            day_id,
            equity_micros,
            is_day_trade,
        }
    }

    const DAY1: u32 = 20260210;
    const DAY2: u32 = 20260211;
    const DAY3: u32 = 20260212;
    const DAY4: u32 = 20260213;
    const DAY5: u32 = 20260214;
    const DAY6: u32 = 20260217; // outside 5-day window from DAY1

    const EQUITY_OK: i64 = 30_000 * 1_000_000; // above $25k
    const EQUITY_LOW: i64 = 10_000 * 1_000_000; // below $25k

    // --- Disabled policy ---

    #[test]
    fn disabled_policy_always_allows() {
        let p = PdtPolicy::disabled();
        let s = state();
        let d = evaluate_pdt(&p, &s, &input(DAY1, EQUITY_LOW, true));
        assert!(d.trading_allowed);
        assert_eq!(d.reason, PdtReason::EnforcementDisabled);
    }

    // --- Non-day trades always allowed ---

    #[test]
    fn non_day_trade_always_allowed() {
        let p = policy();
        let s = state();
        let d = evaluate_pdt(&p, &s, &input(DAY1, EQUITY_OK, false));
        assert!(d.trading_allowed);
        assert_eq!(d.reason, PdtReason::AllowedNotDayTrade);
    }

    // --- Within limit ---

    #[test]
    fn first_day_trade_allowed() {
        let p = policy();
        let mut s = state();
        tick_pdt(&p, &mut s, &input(DAY1, EQUITY_OK, true));
        let d = evaluate_pdt(&p, &s, &input(DAY1, EQUITY_OK, true));
        assert!(d.trading_allowed);
        assert_eq!(d.reason, PdtReason::AllowedWithinLimit);
        assert_eq!(d.window_day_trade_count, 0); // nothing recorded yet
    }

    #[test]
    fn three_day_trades_in_window_allowed() {
        let p = policy(); // max = 3
        let mut s = state();

        // Record 3 day trades across 3 days.
        record_day_trade(&p, &mut s, DAY1);
        record_day_trade(&p, &mut s, DAY2);
        record_day_trade(&p, &mut s, DAY3);

        tick_pdt(&p, &mut s, &input(DAY3, EQUITY_OK, true));
        let d = evaluate_pdt(&p, &s, &input(DAY3, EQUITY_OK, true));
        // 3 recorded; adding 1 more would be 4 > 3 → blocked.
        assert!(!d.trading_allowed);
        assert_eq!(d.reason, PdtReason::BlockedWouldExceedLimit);
        assert_eq!(d.window_day_trade_count, 3);
    }

    // --- Exceeding the limit flags PDT ---

    #[test]
    fn fourth_day_trade_blocks_and_flags_pdt() {
        let p = policy(); // max_day_trades_in_window = 3
        let mut s = state();

        record_day_trade(&p, &mut s, DAY1);
        record_day_trade(&p, &mut s, DAY2);
        record_day_trade(&p, &mut s, DAY3);

        // 4th day trade recorded → flagged.
        record_day_trade(&p, &mut s, DAY4);
        assert!(s.flagged_pdt);
    }

    #[test]
    fn flagged_pdt_above_min_equity_blocked() {
        let p = policy();
        let mut s = state();
        s.flagged_pdt = true;

        let d = evaluate_pdt(&p, &s, &input(DAY1, EQUITY_OK, true));
        assert!(!d.trading_allowed);
        assert_eq!(d.reason, PdtReason::BlockedFlaggedPdt);
    }

    #[test]
    fn flagged_pdt_below_min_equity_blocked_different_reason() {
        let p = policy();
        let mut s = state();
        s.flagged_pdt = true;

        let d = evaluate_pdt(&p, &s, &input(DAY1, EQUITY_LOW, true));
        assert!(!d.trading_allowed);
        assert_eq!(d.reason, PdtReason::BlockedFlaggedBelowMinEquity);
    }

    #[test]
    fn clear_pdt_flag_allows_trading_again() {
        let p = policy();
        let mut s = state();
        s.flagged_pdt = true;

        clear_pdt_flag(&mut s);
        assert!(!s.flagged_pdt);

        // After clearing, within-limit day trades are allowed.
        let d = evaluate_pdt(&p, &s, &input(DAY1, EQUITY_OK, true));
        assert!(d.trading_allowed);
    }

    // --- Rolling window expiry ---

    #[test]
    fn old_day_trades_roll_out_of_window() {
        let p = policy(); // window = 5 days
        let mut s = state();

        // Record 3 trades on DAY1.
        record_day_trade(&p, &mut s, DAY1);
        record_day_trade(&p, &mut s, DAY1);
        record_day_trade(&p, &mut s, DAY1);

        // DAY6 is 6 days after DAY1 → DAY1 is outside the 5-day window.
        tick_pdt(&p, &mut s, &input(DAY6, EQUITY_OK, true));
        let d = evaluate_pdt(&p, &s, &input(DAY6, EQUITY_OK, true));

        // Window should not include DAY1's 3 trades; count = 0 → allowed.
        assert!(d.trading_allowed);
        assert_eq!(d.window_day_trade_count, 0);
    }

    #[test]
    fn trades_on_day5_still_in_window() {
        let p = policy(); // window = 5 days
        let mut s = state();

        record_day_trade(&p, &mut s, DAY1);
        record_day_trade(&p, &mut s, DAY5);

        // Evaluating on DAY5: both DAY1 and DAY5 are within the 5 most-recent days.
        tick_pdt(&p, &mut s, &input(DAY5, EQUITY_OK, true));
        let d = evaluate_pdt(&p, &s, &input(DAY5, EQUITY_OK, true));

        assert_eq!(d.window_day_trade_count, 2);
    }

    // --- to_pdt_context bridge ---

    #[test]
    fn to_pdt_context_allowed() {
        let decision = PdtDecision {
            trading_allowed: true,
            reason: PdtReason::AllowedWithinLimit,
            window_day_trade_count: 1,
        };
        let ctx = to_pdt_context(&decision);
        assert!(ctx.pdt_ok);
    }

    #[test]
    fn to_pdt_context_blocked() {
        let decision = PdtDecision {
            trading_allowed: false,
            reason: PdtReason::BlockedWouldExceedLimit,
            window_day_trade_count: 3,
        };
        let ctx = to_pdt_context(&decision);
        assert!(!ctx.pdt_ok);
    }

    // --- Prune helper ---

    #[test]
    fn prune_old_days_removes_stale_entries() {
        let mut s = state();
        s.day_trade_counts.insert(DAY1, 2);
        s.day_trade_counts.insert(DAY2, 1);
        s.day_trade_counts.insert(DAY5, 3);

        // Window = 2 most recent days from DAY5 → keeps DAY4/DAY5 range.
        // DAY1 and DAY2 are older than the 2 most-recent entries.
        prune_old_days(&mut s, DAY5, 2);

        assert!(!s.day_trade_counts.contains_key(&DAY1));
        assert!(!s.day_trade_counts.contains_key(&DAY2));
        assert!(s.day_trade_counts.contains_key(&DAY5));
    }

    // --- record_day_trade idempotence / accumulation ---

    #[test]
    fn multiple_day_trades_same_day_accumulate() {
        let p = policy();
        let mut s = state();

        record_day_trade(&p, &mut s, DAY1);
        record_day_trade(&p, &mut s, DAY1);

        assert_eq!(s.day_trade_counts[&DAY1], 2);
    }
}
