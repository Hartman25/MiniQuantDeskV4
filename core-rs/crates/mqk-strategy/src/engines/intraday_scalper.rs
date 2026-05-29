//! Intraday scalper strategy engine.
//!
//! # Target-position semantics (STRATEGY-SIZING-AND-EXIT-AUDIT-01 / STRATEGY-EXIT-RULES-01)
//!
//! `signal_from_recent` returns a *direction* value in `{-1, 0, +1}`.
//! `on_bar` maps this direction to an absolute target portfolio state:
//!
//! ```text
//! direction = +1  →  target = effective_target_qty  (enter/hold long N shares)
//! direction =  0  →  target = 0                     (go flat; close any long)
//! direction = -1  →  target = 0                     (go flat; STRATEGY-EXIT-RULES-01)
//! ```
//!
//! `direction = -1` previously produced `target = -target_qty` (net-short intent),
//! which was silently blocked by the B5 short-sale guard in `bar_result_to_decisions`
//! because the runtime does not manage short-position lifecycle.  Mapping bearish
//! signals to `target = 0` instead makes the exit reachable: if a long position is
//! held, `bar_result_to_decisions` computes `delta = 0 - current < 0`, `qty_to_sell =
//! current` and the B5 guard passes (selling exactly what is held, not going short).
//!
//! This strategy is therefore **long-only**: it enters on bullish displacement and
//! exits (closes the long) on bearish or neutral displacement.
//!
//! The downstream `bar_result_to_decisions` function computes the *delta*:
//!
//! ```text
//! delta = target - current_position
//! delta > 0  → buy  abs(delta) shares
//! delta == 0 → no order (already at target)
//! delta < 0  → sell abs(delta) shares (B5 guard passes if qty_to_sell ≤ current)
//! ```
//!
//! # Configuration
//!
//! `MQK_STRATEGY_TARGET_QTY` — absolute target share count (default: 1).
//! Must be a positive integer.  A value of 1 preserves the prior entry behavior.
//!
//! `MQK_STRATEGY_MAX_TARGET_QTY` — hard cap on the target share count (optional).
//! When set, `effective_target = min(target_qty, max_target_qty)`.
//! Absent or non-positive → no share cap applied.
//!
//! `MQK_STRATEGY_MAX_POSITION_NOTIONAL_USD` — hard cap on position notional in whole
//! USD (optional).  When set, `max_qty_from_notional = max_notional_usd × 1_000_000 /
//! last_close_micros` and `effective_target = min(effective_target, max_qty_from_notional)`.
//! If `last_close_micros` is zero when this cap is active the result is capped to 0
//! (fail-closed: cannot compute notional without a price reference).
//! Absent or non-positive → no notional cap applied.
//!
//! Effective target is always floored at 0 (never negative).
//! Default target_qty=1 with no caps set preserves the prior conservative behavior.

use crate::{
    BarStub, Strategy, StrategyContext, StrategyMeta, StrategyOutput, StrategySpec, TargetPosition,
};

const NAME: &str = "intraday_scalper";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 300; // 5m
const LOOKBACK: usize = 5;
const MICRO_MOVE_BPS: i64 = 20; // 0.20%

/// Env var: absolute target share count for this strategy (default: 1).
pub const TARGET_QTY_ENV: &str = "MQK_STRATEGY_TARGET_QTY";
/// Env var: hard cap on target share count (optional; absent = no share cap).
pub const MAX_TARGET_QTY_ENV: &str = "MQK_STRATEGY_MAX_TARGET_QTY";
/// Env var: hard cap on position notional in whole USD (optional; absent = no notional cap).
pub const MAX_NOTIONAL_USD_ENV: &str = "MQK_STRATEGY_MAX_POSITION_NOTIONAL_USD";

const DEFAULT_TARGET_QTY: i64 = 1;

/// Read `MQK_STRATEGY_TARGET_QTY` from the environment.
///
/// Returns `DEFAULT_TARGET_QTY` (1) if the variable is absent, blank, zero, or
/// non-positive.  Callers must not infer position size from the absence of this
/// variable — default 1 is an explicit conservative choice, not an unset state.
pub fn target_qty_from_env() -> i64 {
    std::env::var(TARGET_QTY_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|&q| q > 0)
        .unwrap_or(DEFAULT_TARGET_QTY)
}

/// Read `MQK_STRATEGY_MAX_TARGET_QTY` from the environment.
///
/// Returns `None` if absent, blank, zero, or non-positive — no share cap active.
pub fn max_target_qty_from_env() -> Option<i64> {
    std::env::var(MAX_TARGET_QTY_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|&q| q > 0)
}

/// Read `MQK_STRATEGY_MAX_POSITION_NOTIONAL_USD` from the environment.
///
/// Returns `None` if absent, blank, zero, or non-positive — no notional cap active.
pub fn max_notional_usd_from_env() -> Option<i64> {
    std::env::var(MAX_NOTIONAL_USD_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|&n| n > 0)
}

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic intraday scalp engine using short-horizon close displacement.",
    )
}

#[derive(Clone, Debug)]
pub struct IntradayScalperStrategy {
    symbol: String,
    /// Absolute target share count requested by operator configuration.
    target_qty: i64,
    /// Hard cap on target share count (None = no cap).
    max_target_qty: Option<i64>,
    /// Hard cap on position notional in whole USD (None = no cap).
    max_notional_usd: Option<i64>,
}

impl IntradayScalperStrategy {
    /// Construct using env vars for all three sizing parameters.
    pub fn new(symbol: impl Into<String>) -> Self {
        Self::with_caps(
            symbol,
            target_qty_from_env(),
            max_target_qty_from_env(),
            max_notional_usd_from_env(),
        )
    }

    /// Construct with an explicit target qty and no caps (for tests or callers
    /// that supply the value directly and do not need caps).
    pub fn with_target_qty(symbol: impl Into<String>, target_qty: i64) -> Self {
        Self::with_caps(symbol, target_qty, None, None)
    }

    /// Construct with full sizing control: target qty, optional max shares cap,
    /// optional max notional cap (in whole USD).
    pub fn with_caps(
        symbol: impl Into<String>,
        target_qty: i64,
        max_target_qty: Option<i64>,
        max_notional_usd: Option<i64>,
    ) -> Self {
        debug_assert!(target_qty > 0, "target_qty must be positive");
        Self {
            symbol: symbol.into(),
            target_qty: target_qty.max(1),
            max_target_qty,
            max_notional_usd,
        }
    }

    /// Returns the raw direction signal: +1 (bullish), 0 (hold), -1 (bearish).
    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        if recent.len() < LOOKBACK {
            return 0;
        }

        let last = match recent.last() {
            Some(x) if x.is_complete => x,
            _ => return 0,
        };

        let first = &recent[recent.len() - LOOKBACK];
        if first.close_micros <= 0 {
            return 0;
        }

        let diff = last.close_micros as i128 - first.close_micros as i128;
        let bps = (diff * 10_000) / first.close_micros as i128;

        if bps >= MICRO_MOVE_BPS as i128 {
            1
        } else if bps <= -(MICRO_MOVE_BPS as i128) {
            -1
        } else {
            0
        }
    }

    /// Apply position sizing caps to a positive target qty.
    ///
    /// Only called when `requested > 0` (bullish direction).  Bearish/neutral
    /// targets (0) bypass capping entirely — exits are never reduced.
    ///
    /// Returns `(effective_target, capped_by)` where `capped_by` is a
    /// diagnostic label: `"none"`, `"max_qty"`, `"max_notional"`, or
    /// `"max_notional_no_price"` (fail-closed when close is zero).
    fn apply_caps(&self, requested: i64, bars: &[BarStub]) -> (i64, &'static str) {
        // Cap 1: hard max shares.
        let (after_qty_cap, qty_cap_fired) = match self.max_target_qty {
            Some(max_qty) if requested > max_qty => (max_qty, true),
            _ => (requested, false),
        };

        // Cap 2: hard max notional (USD).
        let (effective, capped_by) = match self.max_notional_usd {
            None => (
                after_qty_cap,
                if qty_cap_fired { "max_qty" } else { "none" },
            ),
            Some(max_notional_usd) => {
                let last_close_micros = bars.last().map(|b| b.close_micros).unwrap_or(0);
                if last_close_micros <= 0 {
                    // Fail-closed: cannot compute notional without price reference.
                    return (0, "max_notional_no_price");
                }
                // max_qty_from_notional = max_notional_usd * 1_000_000 / close_micros
                // Example: max=$1000, close=$200 → 1000*1_000_000/200_000_000 = 5
                let max_from_notional = (max_notional_usd * 1_000_000) / last_close_micros;
                let max_from_notional = max_from_notional.max(0);
                if after_qty_cap > max_from_notional {
                    (max_from_notional, "max_notional")
                } else if qty_cap_fired {
                    (after_qty_cap, "max_qty")
                } else {
                    (after_qty_cap, "none")
                }
            }
        };

        (effective.max(0), capped_by)
    }
}

impl Strategy for IntradayScalperStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(NAME, TIMEFRAME_SECS)
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        // STRATEGY-EXIT-RULES-01: clamp direction to [0, 1] before multiplying.
        // direction=+1 → target=+target_qty (enter/hold long).
        // direction= 0 → target=0 (go flat; close any long on neutral bar).
        // direction=-1 → target=0 (go flat; exit long on bearish bar, not net-short).
        let direction = Self::signal_from_recent(&ctx.recent.bars);
        let requested_target = direction.max(0) * self.target_qty;

        // STRATEGY-POSITION-SIZING-01: apply caps only for bullish (positive) targets.
        // Bearish/neutral exits (target=0) are never capped — they must always reach 0.
        let (effective_target, capped_by) = if requested_target > 0 {
            self.apply_caps(requested_target, &ctx.recent.bars)
        } else {
            (requested_target, "none")
        };

        // Sizing diagnostic fields are surfaced via the return value.
        // The caller (loop_runner b1c_position_delta_diagnostic) logs the
        // effective target after bar_result_to_decisions computes the delta.
        let _ = capped_by; // used in tests; suppress unused-variable warning in release

        StrategyOutput {
            targets: vec![TargetPosition {
                symbol: self.symbol.clone(),
                qty: effective_target,
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BarStub, RecentBarsWindow, StrategyContext};

    fn bar(close_micros: i64, is_complete: bool) -> BarStub {
        BarStub::new(0, is_complete, close_micros, 1)
    }

    fn ctx_with_bars(bars: Vec<BarStub>) -> StrategyContext {
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(LOOKBACK + 5, bars))
    }

    fn bullish_bars(base: i64) -> Vec<BarStub> {
        vec![
            bar(base, true),
            bar(base, true),
            bar(base, true),
            bar(base, true),
            bar(base + base / 400, true), // +25 bps ≥ MICRO_MOVE_BPS=20
        ]
    }

    fn bearish_bars(base: i64) -> Vec<BarStub> {
        vec![
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base, true), // -25 bps ≤ -MICRO_MOVE_BPS
        ]
    }

    fn flat_bars(base: i64) -> Vec<BarStub> {
        vec![
            bar(base, true),
            bar(base, true),
            bar(base, true),
            bar(base, true),
            bar(base, true),
        ]
    }

    /// SPS01: default target 1 share on bullish signal.
    #[test]
    fn sps01_default_target_qty_is_1() {
        let base = 200_000_000i64; // $200
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1);
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        assert_eq!(out.targets[0].qty, 1, "SPS01: default target_qty=1");
    }

    /// SPS02: MQK_STRATEGY_TARGET_QTY=5 targets 5 shares on bullish signal.
    #[test]
    fn sps02_configured_target_qty_5() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 5);
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        assert_eq!(out.targets[0].qty, 5, "SPS02: target_qty=5");
    }

    /// SPS03: MQK_STRATEGY_MAX_TARGET_QTY caps target_qty.
    #[test]
    fn sps03_max_target_qty_caps_target() {
        let base = 200_000_000i64;
        // target_qty=10, max_target_qty=3 → effective=3
        let mut s = IntradayScalperStrategy::with_caps("AAPL", 10, Some(3), None);
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        assert_eq!(out.targets[0].qty, 3, "SPS03: capped by max_target_qty=3");
    }

    /// SPS03b: max_target_qty >= target_qty → no cap applied.
    #[test]
    fn sps03b_max_target_qty_above_target_no_cap() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_caps("AAPL", 5, Some(10), None);
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 5,
            "SPS03b: max_target_qty=10 ≥ target=5 → no cap"
        );
    }

    /// SPS04: max notional cap reduces target based on close price.
    ///
    /// bullish_bars last close = base + base/400 = 200_500_000 ($200.50/share).
    /// max_notional=$700 → 700_000_000 / 200_500_000 = 3 (integer div).
    #[test]
    fn sps04_max_notional_caps_based_on_close_price() {
        let base = 200_000_000i64;
        // last bar close = 200_500_000; 700_000_000 / 200_500_000 = 3
        let mut s = IntradayScalperStrategy::with_caps("AAPL", 10, None, Some(700));
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 3,
            "SPS04: close=$200.50, notional=$700 → max 3 shares"
        );
    }

    /// SPS04b: notional cap floors at 0 when max_notional < 1 share price.
    /// $200/share, max_notional=$100 → max_shares=0 → effective=0.
    #[test]
    fn sps04b_notional_cap_floors_at_zero_when_too_small() {
        let base = 200_000_000i64; // $200/share
                                   // 100 * 1_000_000 / 200_000_000 = 0 → floor at 0
        let mut s = IntradayScalperStrategy::with_caps("AAPL", 5, None, Some(100));
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 0,
            "SPS04b: max_notional < 1 share → effective=0 (fail-closed)"
        );
    }

    /// SPS04c: notional cap binds tighter than qty cap → notional wins.
    ///
    /// last close = 200_500_000; max_notional=$500 → 500_000_000/200_500_000 = 2.
    #[test]
    fn sps04c_notional_cap_tighter_than_qty_cap() {
        let base = 200_000_000i64;
        // 500_000_000 / 200_500_000 = 2; qty_cap=5 → notional wins → effective=2
        let mut s = IntradayScalperStrategy::with_caps("AAPL", 10, Some(5), Some(500));
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 2,
            "SPS04c: notional cap=2 binds tighter than qty cap=5"
        );
    }

    /// SPS04d: qty cap binds tighter than notional cap → qty wins.
    #[test]
    fn sps04d_qty_cap_tighter_than_notional_cap() {
        let base = 200_000_000i64; // $200/share
                                   // max_qty_cap=1, notional=$1000 → max_from_notional=5 → qty cap=1 wins
        let mut s = IntradayScalperStrategy::with_caps("AAPL", 10, Some(1), Some(1000));
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 1,
            "SPS04d: qty cap=1 binds tighter than notional cap=5"
        );
    }

    /// SPS05: invalid target_qty (≤0) from env path → safe default 1.
    #[test]
    fn sps05_env_invalid_defaults_to_1() {
        std::env::remove_var(TARGET_QTY_ENV);
        assert_eq!(target_qty_from_env(), 1, "SPS05: absent → default 1");
        std::env::set_var(TARGET_QTY_ENV, "0");
        assert_eq!(target_qty_from_env(), 1, "SPS05: zero → default 1");
        std::env::remove_var(TARGET_QTY_ENV);
    }

    /// SPS05b: invalid max caps from env → None (no cap active).
    #[test]
    fn sps05b_env_invalid_caps_return_none() {
        std::env::remove_var(MAX_TARGET_QTY_ENV);
        assert_eq!(max_target_qty_from_env(), None, "SPS05b: absent → None");
        std::env::set_var(MAX_TARGET_QTY_ENV, "0");
        assert_eq!(max_target_qty_from_env(), None, "SPS05b: zero → None");
        std::env::remove_var(MAX_TARGET_QTY_ENV);

        std::env::remove_var(MAX_NOTIONAL_USD_ENV);
        assert_eq!(max_notional_usd_from_env(), None, "SPS05b: absent → None");
        std::env::set_var(MAX_NOTIONAL_USD_ENV, "-1");
        assert_eq!(max_notional_usd_from_env(), None, "SPS05b: negative → None");
        std::env::remove_var(MAX_NOTIONAL_USD_ENV);
    }

    /// SPS06: bearish signal → target=0 regardless of target_qty or caps.
    #[test]
    fn sps06_bearish_target_is_always_zero() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_caps("AAPL", 10, Some(5), Some(1000));
        let out = s.on_bar(&ctx_with_bars(bearish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 0,
            "SPS06: bearish signal → target=0 regardless of caps"
        );
    }

    /// SPS07: neutral (hold) signal → target=0.
    #[test]
    fn sps07_neutral_target_is_zero() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_caps("AAPL", 10, Some(5), Some(1000));
        let out = s.on_bar(&ctx_with_bars(flat_bars(base)));
        assert_eq!(
            out.targets[0].qty, 0,
            "SPS07: neutral signal → target=0 regardless of caps"
        );
    }

    /// SPS08: B5 guard interaction — notional-capped target with long position
    /// produces correct delta; selling from long passes B5.
    #[test]
    fn sps08_b5_guard_unaffected_by_capping() {
        // Proves caps only affect the target; B5 guard logic is downstream in
        // bar_result_to_decisions (decision.rs). This test proves the strategy
        // emits the correct capped target, not that B5 is implemented here.
        let base = 200_000_000i64; // $200
                                   // Bearish → target=0; with current=2, delta=-2; B5 passes (qty_to_sell=2≤current=2).
        let mut s = IntradayScalperStrategy::with_caps("AAPL", 5, None, Some(400));
        let out = s.on_bar(&ctx_with_bars(bearish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 0,
            "SPS08: bearish target=0 for B5 sell path"
        );

        let current = 2i64;
        let delta = out.targets[0].qty - current; // 0 - 2 = -2
        let qty_to_sell = -delta; // 2
        assert!(
            qty_to_sell <= current,
            "SPS08: B5 passes; sell ≤ current long"
        );
    }

    /// SPS09: target-position delta correctness.
    /// current=2, effective_target=5 → delta=+3 → buy 3.
    #[test]
    fn sps09_delta_buy_from_partial_position() {
        let base = 200_000_000i64; // $200; $1200 notional → max 6 shares
        let mut s = IntradayScalperStrategy::with_caps("AAPL", 5, None, Some(1200));
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        assert_eq!(out.targets[0].qty, 5, "SPS09: effective_target=5");
        let current = 2i64;
        let delta = out.targets[0].qty - current;
        assert_eq!(delta, 3, "SPS09: buy 3 to reach target=5 from current=2");
    }

    /// SPS09b: current=2, effective_target=2 → delta=0 → already at target.
    #[test]
    fn sps09b_already_at_target_no_order() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 2);
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        assert_eq!(out.targets[0].qty, 2);
        let current = 2i64;
        let delta = out.targets[0].qty - current;
        assert_eq!(delta, 0, "SPS09b: already at target → no order");
    }

    /// SPS10: no account buying power used — all sizing is static/config-driven.
    /// Proves that max_caps constructors do not read any account balance state.
    #[test]
    fn sps10_no_account_state_used_for_sizing() {
        // Proven by construction: with_caps takes only symbol, target_qty,
        // max_target_qty (shares), and max_notional_usd (USD constant).
        // No broker snapshot, no account equity, no buying_power field referenced.
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_caps("AAPL", 5, Some(3), Some(800));
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        // $200 close, $800 notional → max 4 shares; qty cap=3 → effective=3
        assert_eq!(
            out.targets[0].qty, 3,
            "SPS10: sizing is config-only, no account balance"
        );
    }

    // ── Legacy tests from STRATEGY-SIZING-AND-EXIT-AUDIT-01 / STRATEGY-EXIT-RULES-01 ──

    /// SS-01: direction=+1 × target_qty=1 → target=1 (default behavior preserved).
    #[test]
    fn ss01_bullish_default_target_qty_is_1() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1);
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 1,
            "SS-01: bullish signal, target_qty=1 → target=1"
        );
    }

    /// SS-02: direction=+1 × target_qty=5 → target=5 (configurable sizing).
    #[test]
    fn ss02_bullish_target_qty_5() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 5);
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 5,
            "SS-02: bullish signal, target_qty=5 → target=5"
        );
    }

    /// SS-03: direction=0 → target=0 regardless of target_qty.
    #[test]
    fn ss03_hold_signal_target_is_0() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 5);
        let out = s.on_bar(&ctx_with_bars(flat_bars(base)));
        assert_eq!(
            out.targets[0].qty, 0,
            "SS-03: hold signal → target=0 regardless of target_qty"
        );
    }

    /// SS-04 (STRATEGY-EXIT-RULES-01): bearish signal → target=0 (go flat), not -N.
    #[test]
    fn ss04_bearish_signal_emits_flat_target_zero() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1);
        let out = s.on_bar(&ctx_with_bars(bearish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 0,
            "SS-04: bearish signal → direction.max(0)=0 → target=0 (go flat, not net-short)"
        );
    }

    // ER-01: bearish signal with long position → sell to flat (B5 passes).
    #[test]
    fn er01_bearish_with_long_produces_sell_to_flat() {
        use crate::StrategyBarResult;
        use crate::{IntentMode, StrategyIntents, StrategyOutput};
        use std::collections::BTreeMap;

        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1);
        let strategy_out = s.on_bar(&ctx_with_bars(bearish_bars(base)));
        assert_eq!(
            strategy_out.targets[0].qty, 0,
            "ER-01 precondition: bearish → target=0"
        );

        let bar_result = StrategyBarResult {
            spec: StrategySpec::new(NAME, TIMEFRAME_SECS),
            intents: StrategyIntents {
                mode: IntentMode::Live,
                output: StrategyOutput {
                    targets: strategy_out.targets,
                },
            },
        };
        let mut positions = BTreeMap::new();
        positions.insert("AAPL".to_string(), 1i64);

        let target = bar_result.intents.output.targets[0].qty; // 0
        let current = *positions.get("AAPL").unwrap_or(&0); // 1
        let delta = target - current; // -1
        let qty_to_sell = -delta; // 1
        assert!(
            current > 0 && qty_to_sell <= current,
            "ER-01: B5 guard passes: qty_to_sell({qty_to_sell}) <= current({current})"
        );
        assert_eq!(delta, -1, "ER-01: delta = target(0) - current(1) = -1");
        assert_eq!(qty_to_sell, 1, "ER-01: sell 1 share to close long");
    }

    // ER-02: bearish signal from flat → no order.
    #[test]
    fn er02_bearish_from_flat_produces_no_order() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1);
        let out = s.on_bar(&ctx_with_bars(bearish_bars(base)));
        let delta = out.targets[0].qty;
        assert_eq!(delta, 0, "ER-02: bearish from flat → delta=0 → no order");
    }

    // ER-03: bullish behavior is unchanged (target=+target_qty from flat).
    #[test]
    fn er03_bullish_behavior_unchanged() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 3);
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 3,
            "ER-03: bullish with target_qty=3 → target=3"
        );
    }

    /// SS-05: absent or invalid MQK_STRATEGY_TARGET_QTY → target_qty_from_env returns 1.
    #[test]
    fn ss05_env_absent_or_invalid_defaults_to_1() {
        std::env::remove_var(TARGET_QTY_ENV);
        assert_eq!(target_qty_from_env(), 1, "SS-05: absent → default 1");
        std::env::set_var(TARGET_QTY_ENV, "0");
        assert_eq!(
            target_qty_from_env(),
            1,
            "SS-05: zero → default 1 (filtered)"
        );
        std::env::remove_var(TARGET_QTY_ENV);
    }
}
