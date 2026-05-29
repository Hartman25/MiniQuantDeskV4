//! Intraday scalper strategy engine.
//!
//! # Target-position semantics (STRATEGY-SIZING-AND-EXIT-AUDIT-01 / STRATEGY-EXIT-RULES-01)
//!
//! `signal_from_recent` returns a *direction* value in `{-1, 0, +1}`.
//! `on_bar` maps this direction to an absolute target portfolio state:
//!
//! ```text
//! direction = +1  →  target = +target_qty   (enter/hold long N shares)
//! direction =  0  →  target = 0             (go flat; close any long)
//! direction = -1  →  target = 0             (go flat; STRATEGY-EXIT-RULES-01)
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
//! Larger values increase position size but do NOT bypass any risk or capital
//! gate (those remain independent).

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
    /// Absolute target share count.  Set once at construction from env or caller.
    target_qty: i64,
}

impl IntradayScalperStrategy {
    /// Construct using `MQK_STRATEGY_TARGET_QTY` from the environment.
    pub fn new(symbol: impl Into<String>) -> Self {
        Self::with_target_qty(symbol, target_qty_from_env())
    }

    /// Construct with an explicit target qty (for tests or callers that supply
    /// the value directly).
    pub fn with_target_qty(symbol: impl Into<String>, target_qty: i64) -> Self {
        debug_assert!(target_qty > 0, "target_qty must be positive");
        Self {
            symbol: symbol.into(),
            target_qty: target_qty.max(1), // clamp: never 0 or negative at runtime
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
        let target = direction.max(0) * self.target_qty;
        StrategyOutput {
            targets: vec![TargetPosition {
                symbol: self.symbol.clone(),
                qty: target,
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

    /// SS-01: direction=+1 × target_qty=1 → target=1 (default behavior preserved).
    #[test]
    fn ss01_bullish_default_target_qty_is_1() {
        let base = 200_000_000i64;
        let bars = vec![
            bar(base, true),
            bar(base, true),
            bar(base, true),
            bar(base, true),
            // last bar close is +25 bps (≥ MICRO_MOVE_BPS=20)
            bar(base + base / 400, true),
        ];
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1);
        let out = s.on_bar(&ctx_with_bars(bars));
        assert_eq!(out.targets.len(), 1);
        assert_eq!(
            out.targets[0].qty, 1,
            "SS-01: bullish signal, target_qty=1 → target=1"
        );
    }

    /// SS-02: direction=+1 × target_qty=5 → target=5 (configurable sizing).
    #[test]
    fn ss02_bullish_target_qty_5() {
        let base = 200_000_000i64;
        let bars = vec![
            bar(base, true),
            bar(base, true),
            bar(base, true),
            bar(base, true),
            bar(base + base / 400, true),
        ];
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 5);
        let out = s.on_bar(&ctx_with_bars(bars));
        assert_eq!(
            out.targets[0].qty, 5,
            "SS-02: bullish signal, target_qty=5 → target=5"
        );
    }

    /// SS-03: direction=0 → target=0 regardless of target_qty.
    #[test]
    fn ss03_hold_signal_target_is_0() {
        let base = 200_000_000i64;
        // All bars same price → 0 bps displacement → hold
        let bars = vec![
            bar(base, true),
            bar(base, true),
            bar(base, true),
            bar(base, true),
            bar(base, true),
        ];
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 5);
        let out = s.on_bar(&ctx_with_bars(bars));
        assert_eq!(
            out.targets[0].qty, 0,
            "SS-03: hold signal → target=0 regardless of target_qty"
        );
    }

    /// SS-04 (STRATEGY-EXIT-RULES-01): bearish signal → target=0 (go flat), not -N.
    ///
    /// Before STRATEGY-EXIT-RULES-01: direction=-1 × target_qty=1 → target=-1 (net-short
    /// intent, silently blocked by B5 guard → no sell ever produced).
    /// After  STRATEGY-EXIT-RULES-01: direction=-1 → clamped to 0 → target=0 (go flat).
    /// With current=1, bar_result_to_decisions computes delta=-1 → sell 1 (B5 passes).
    #[test]
    fn ss04_bearish_signal_emits_flat_target_zero() {
        let base = 200_000_000i64;
        let bars = vec![
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            // last bar is -25 bps (≤ -MICRO_MOVE_BPS=20 → direction=-1 → clamped to 0)
            bar(base, true),
        ];
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1);
        let out = s.on_bar(&ctx_with_bars(bars));
        assert_eq!(
            out.targets[0].qty, 0,
            "SS-04: bearish signal → direction.max(0)=0 → target=0 (go flat, not net-short)"
        );
    }

    // ER-01: bearish signal with long position → sell to flat (B5 passes).
    //
    // Combines the strategy output (target=0) with bar_result_to_decisions
    // delta computation to prove the exit path is now reachable.
    #[test]
    fn er01_bearish_with_long_produces_sell_to_flat() {
        use crate::StrategyBarResult;
        use crate::{IntentMode, StrategyIntents, StrategyOutput};
        use std::collections::BTreeMap;

        let base = 200_000_000i64;
        let bearish_bars = vec![
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base, true), // -25 bps → direction=-1 → target=0
        ];
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1);
        let strategy_out = s.on_bar(&ctx_with_bars(bearish_bars));
        assert_eq!(
            strategy_out.targets[0].qty, 0,
            "ER-01 precondition: bearish signal → target=0"
        );

        // Simulate bar_result_to_decisions with current_position=1.
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

        // bar_result_to_decisions is in mqk-daemon; replicate delta logic inline.
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

    // ER-02: bearish signal from flat → no order (already at target=0).
    #[test]
    fn er02_bearish_from_flat_produces_no_order() {
        let base = 200_000_000i64;
        let bearish_bars = vec![
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base + base / 400, true),
            bar(base, true),
        ];
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1);
        let out = s.on_bar(&ctx_with_bars(bearish_bars));
        let target = out.targets[0].qty; // 0
        let current = 0i64; // flat
        let delta = target - current; // 0
        assert_eq!(
            delta, 0,
            "ER-02: bearish from flat → target=0, current=0 → delta=0 → no order"
        );
    }

    // ER-03: bullish behavior is unchanged (target=+target_qty from flat).
    #[test]
    fn er03_bullish_behavior_unchanged() {
        let base = 200_000_000i64;
        let bullish_bars = vec![
            bar(base, true),
            bar(base, true),
            bar(base, true),
            bar(base, true),
            bar(base + base / 400, true), // +25 bps → direction=+1
        ];
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 3);
        let out = s.on_bar(&ctx_with_bars(bullish_bars));
        assert_eq!(
            out.targets[0].qty, 3,
            "ER-03: bullish signal with target_qty=3 → target=3 (unchanged by STRATEGY-EXIT-RULES-01)"
        );
    }

    /// SS-05: absent or invalid MQK_STRATEGY_TARGET_QTY → target_qty_from_env returns 1.
    ///
    /// Proves conservative default from env: absent/non-positive value → 1.
    /// Passing ≤0 directly to with_target_qty fires a debug_assert (not a valid
    /// caller contract); this test validates the env-path fail-safe only.
    #[test]
    fn ss05_env_absent_or_invalid_defaults_to_1() {
        std::env::remove_var(TARGET_QTY_ENV);
        assert_eq!(
            target_qty_from_env(),
            1,
            "SS-05: absent MQK_STRATEGY_TARGET_QTY → default 1"
        );
        // A non-positive string value also returns the default.
        std::env::set_var(TARGET_QTY_ENV, "0");
        assert_eq!(
            target_qty_from_env(),
            1,
            "SS-05: MQK_STRATEGY_TARGET_QTY=0 → default 1 (filtered out)"
        );
        std::env::remove_var(TARGET_QTY_ENV);
    }
}
