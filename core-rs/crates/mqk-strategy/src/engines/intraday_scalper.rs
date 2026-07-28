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
//! When `allow_short_signals = true` (opt-in; default `false`), `direction = -1`
//! produces `target = -effective_target_qty` instead.  Downstream B5 guard and
//! risk gates remain active; this flag only lets the strategy express intent.
//!
//! This strategy is **long-only by default**: it enters on bullish displacement and
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
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};

// ── Decision vocabulary (STRATEGY-DECISION-OBSERVABILITY-01) ─────────────────
pub const DECISION_SIGNAL_LONG: &str = "signal_long";
pub const DECISION_SIGNAL_SHORT: &str = "short_due_to_negative_direction";
pub const DECISION_FLAT_NEGATIVE: &str = "flat_due_to_negative_direction";
pub const DECISION_FLAT_BELOW_THRESHOLD: &str = "flat_below_threshold";
pub const DECISION_INSUFFICIENT_BARS: &str = "insufficient_bars";

/// Read-only diagnostic snapshot produced by `compute_diagnostics`.
///
/// All fields are derived purely from the bar window with no side-effects on
/// the strategy decision path.  `decision` is one of the `DECISION_*` constants.
#[derive(Clone, Debug)]
pub struct IntradayScalperDiagnostics {
    pub lookback_bars: usize,
    pub threshold_bps: i64,
    /// Unix-second timestamp of the latest bar (`recent.last().end_ts`), or `None`
    /// when the bar window is empty.
    pub latest_bar_ts: Option<i64>,
    /// Close price of the latest bar in micros, or `None` when empty.
    pub latest_close_micros: Option<i64>,
    /// Unix-second timestamp of the lookback anchor bar, or `None` when
    /// `recent.len() < LOOKBACK`.
    pub lookback_bar_ts: Option<i64>,
    /// Close price of the lookback anchor bar in micros, or `None` when
    /// `recent.len() < LOOKBACK` or anchor `close_micros <= 0`.
    pub lookback_close_micros: Option<i64>,
    /// Signed displacement in basis points: `(latest - lookback) * 10_000 /
    /// lookback`.  `None` when insufficient bars or anchor price is invalid.
    pub move_bps: Option<i64>,
    /// Absolute value of `move_bps`. `None` in the same cases.
    pub abs_move_bps: Option<i64>,
    /// `threshold_bps - abs_move_bps`.  Positive means move is still below
    /// threshold (gap remaining).  Zero or negative means threshold was met.
    /// `None` when `abs_move_bps` is `None`.
    pub gap_to_threshold_bps: Option<i64>,
    /// Raw direction as returned by `signal_from_recent`: `+1`, `0`, or `-1`.
    pub raw_direction: i64,
    /// One of `DECISION_*` constants.
    pub decision: &'static str,
    /// Human-readable reason for the decision.
    pub reason: &'static str,
}

/// Compute a read-only diagnostic snapshot from a bar window.
///
/// Pure function — does not modify strategy state or affect the decision path.
/// Safe to call on any bar slice; returns `decision = "insufficient_bars"` when
/// the window is too small rather than panicking.
///
/// Equivalent to `compute_diagnostics_with_config(recent, false)`.
pub fn compute_diagnostics(recent: &[BarStub]) -> IntradayScalperDiagnostics {
    compute_diagnostics_with_config(recent, false)
}

/// Compute a read-only diagnostic snapshot from a bar window, with explicit
/// short-signal configuration.
///
/// When `allow_short_signals = true`, bearish displacement returns
/// `decision = DECISION_SIGNAL_SHORT` (`"short_due_to_negative_direction"`)
/// instead of `DECISION_FLAT_NEGATIVE`.  All other logic is identical to
/// [`compute_diagnostics`].
pub fn compute_diagnostics_with_config(
    recent: &[BarStub],
    allow_short_signals: bool,
) -> IntradayScalperDiagnostics {
    let latest = recent.last();
    let latest_bar_ts = latest.map(|b| b.end_ts);
    let latest_close_micros = latest.map(|b| b.close_micros);

    // Insufficient bars: window too small or last bar not complete.
    if recent.len() < LOOKBACK || !latest.map(|b| b.is_complete).unwrap_or(false) {
        return IntradayScalperDiagnostics {
            lookback_bars: LOOKBACK,
            threshold_bps: MICRO_MOVE_BPS,
            latest_bar_ts,
            latest_close_micros,
            lookback_bar_ts: None,
            lookback_close_micros: None,
            move_bps: None,
            abs_move_bps: None,
            gap_to_threshold_bps: None,
            raw_direction: 0,
            decision: DECISION_INSUFFICIENT_BARS,
            reason: "fewer than LOOKBACK complete bars in window",
        };
    }

    let anchor = &recent[recent.len() - LOOKBACK];
    let lookback_bar_ts = Some(anchor.end_ts);

    if anchor.close_micros <= 0 {
        return IntradayScalperDiagnostics {
            lookback_bars: LOOKBACK,
            threshold_bps: MICRO_MOVE_BPS,
            latest_bar_ts,
            latest_close_micros,
            lookback_bar_ts,
            lookback_close_micros: Some(anchor.close_micros),
            move_bps: None,
            abs_move_bps: None,
            gap_to_threshold_bps: None,
            raw_direction: 0,
            decision: DECISION_INSUFFICIENT_BARS,
            reason: "lookback anchor close_micros is zero or negative",
        };
    }

    let lookback_close_micros = Some(anchor.close_micros);
    let last_close = latest.unwrap().close_micros; // safe: checked len >= LOOKBACK above
    let diff = last_close as i128 - anchor.close_micros as i128;
    let bps_i128 = (diff * 10_000) / anchor.close_micros as i128;
    let move_bps = bps_i128.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
    let abs_move_bps = move_bps.unsigned_abs() as i64;
    let gap_to_threshold_bps = MICRO_MOVE_BPS - abs_move_bps;

    let (raw_direction, decision, reason) = if bps_i128 >= MICRO_MOVE_BPS as i128 {
        (1i64, DECISION_SIGNAL_LONG, "move_bps >= threshold_bps")
    } else if bps_i128 <= -(MICRO_MOVE_BPS as i128) {
        if allow_short_signals {
            (
                -1i64,
                DECISION_SIGNAL_SHORT,
                "bearish displacement; short signals enabled",
            )
        } else {
            (
                -1i64,
                DECISION_FLAT_NEGATIVE,
                "bearish displacement; long-only strategy maps to flat",
            )
        }
    } else {
        (
            0i64,
            DECISION_FLAT_BELOW_THRESHOLD,
            "abs move_bps below threshold; insufficient displacement for signal",
        )
    };

    IntradayScalperDiagnostics {
        lookback_bars: LOOKBACK,
        threshold_bps: MICRO_MOVE_BPS,
        latest_bar_ts,
        latest_close_micros: Some(last_close),
        lookback_bar_ts,
        lookback_close_micros,
        move_bps: Some(move_bps),
        abs_move_bps: Some(abs_move_bps),
        gap_to_threshold_bps: Some(gap_to_threshold_bps),
        raw_direction,
        decision,
        reason,
    }
}

pub(crate) const NAME: &str = "intraday_scalper";
/// Strategy ID for the short-only variant.  Distinct from `NAME` so both
/// identities can be registered and selected independently.
pub const SHORT_NAME: &str = "intraday_short_scalper";
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
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: LOOKBACK,
    })
}

/// Return strategy metadata for the short-only variant.
///
/// Use this instead of [`meta`] when registering `"intraday_short_scalper"`
/// in a [`crate::PluginRegistry`].
pub fn meta_short() -> StrategyMeta {
    StrategyMeta::new(
        SHORT_NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Short-only intraday scalp engine. \
         Bearish displacement produces a negative target qty; \
         bullish displacement maps to flat (long direction suppressed).",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: LOOKBACK,
    })
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
    /// When `true`, bearish displacement (direction=-1) produces a negative
    /// target qty instead of 0.  Default: `false` (long-only).
    ///
    /// Downstream B5 guard and risk gates remain active regardless of this flag.
    allow_short_signals: bool,
    /// Strategy identity returned by `spec()`.  Defaults to `NAME`
    /// ("intraday_scalper"); set to `SHORT_NAME` for the short-only variant.
    strategy_name: &'static str,
    /// When `true`, bullish direction is suppressed to flat so this strategy
    /// only expresses bearish (short) intent.  Requires `allow_short_signals`
    /// to be `true` to produce negative targets; a stand-alone `is_short_only`
    /// without `allow_short_signals` has no observable effect.
    is_short_only: bool,
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
            allow_short_signals: false,
            strategy_name: NAME,
            is_short_only: false,
        }
    }

    /// Enable or disable short signal generation for this strategy instance.
    ///
    /// When `enabled = true`, bearish displacement (direction=-1) produces a
    /// negative target qty instead of 0.  Default: `false` (long-only).
    ///
    /// Short signals still pass through the daemon's B5 guard and short-entry
    /// policy gates; this flag does not bypass those downstream controls.
    pub fn short_signals(mut self, enabled: bool) -> Self {
        self.allow_short_signals = enabled;
        self
    }

    /// Construct the short-only variant of this strategy.
    ///
    /// Identity: `SHORT_NAME` (`"intraday_short_scalper"`), distinct from
    /// `"intraday_scalper"`.
    ///
    /// Behaviour:
    /// - bearish displacement → `-effective_target_qty` (short signal)
    /// - bullish displacement → `0` (short-only; long direction suppressed)
    /// - below threshold     → `0` (no signal)
    ///
    /// Reads sizing from env vars (`MQK_STRATEGY_TARGET_QTY` etc.) at
    /// construction time, identical to [`new`].  Downstream B5 guard and
    /// risk gates remain active; this constructor does not bypass them.
    pub fn new_short(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            target_qty: target_qty_from_env(),
            max_target_qty: max_target_qty_from_env(),
            max_notional_usd: max_notional_usd_from_env(),
            allow_short_signals: true,
            strategy_name: SHORT_NAME,
            is_short_only: true,
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
        StrategySpec::new(self.strategy_name, TIMEFRAME_SECS)
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let direction = Self::signal_from_recent(&ctx.recent.bars);

        // SHORT-SIDE-PARALLEL-STRATEGY-DRY-RUN-01: short-only mode suppresses
        // bullish direction (clamp ≤ 0) so this variant only expresses bearish
        // intent.  Default (is_short_only=false): direction is unmodified.
        let effective_direction = if self.is_short_only {
            direction.min(0)
        } else {
            direction
        };

        // STRATEGY-EXIT-RULES-01 (default, allow_short_signals=false): clamp direction
        // to [0, 1].  direction=+1 → +target_qty; direction=0/-1 → 0 (go flat).
        // SHORT-SIDE-STRATEGY-GATE-01 (allow_short_signals=true): direction=-1 produces
        // -target_qty instead of 0.  Downstream B5 and risk gates remain active.
        let requested_target = if self.allow_short_signals {
            effective_direction * self.target_qty
        } else {
            effective_direction.max(0) * self.target_qty
        };

        // STRATEGY-POSITION-SIZING-01: apply caps for all non-zero targets.
        // Neutral exits (0) bypass caps. Short signals apply caps to magnitude then
        // restore the sign — same sizing rules as the long side.
        let (effective_target, capped_by) = if requested_target > 0 {
            self.apply_caps(requested_target, &ctx.recent.bars)
        } else if requested_target < 0 {
            let (capped_mag, cap_label) = self.apply_caps(-requested_target, &ctx.recent.bars);
            (-capped_mag, cap_label)
        } else {
            (0, "none")
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

    // ── STRATEGY-DECISION-OBSERVABILITY-01: compute_diagnostics unit tests ──────

    /// OBS-D01: bullish displacement → decision=signal_long, move_bps >= threshold.
    #[test]
    fn obs_d01_bullish_displacement_is_signal_long() {
        // base=$200, last close = base + base/400 = +25 bps
        let base = 200_000_000i64;
        let last_close = base + base / 400; // 200_500_000 (+25 bps)
        let bars: Vec<BarStub> = vec![
            BarStub::new(1000, true, base, 1),
            BarStub::new(1001, true, base, 1),
            BarStub::new(1002, true, base, 1),
            BarStub::new(1003, true, base, 1),
            BarStub::new(1004, true, last_close, 1),
        ];
        let d = compute_diagnostics(&bars);
        assert_eq!(
            d.decision, DECISION_SIGNAL_LONG,
            "OBS-D01: decision=signal_long"
        );
        assert_eq!(d.raw_direction, 1, "OBS-D01: raw_direction=+1");
        let move_bps = d.move_bps.expect("OBS-D01: move_bps must be Some");
        let abs_move_bps = d.abs_move_bps.expect("OBS-D01: abs_move_bps must be Some");
        assert!(move_bps >= MICRO_MOVE_BPS, "OBS-D01: move_bps >= threshold");
        assert_eq!(
            abs_move_bps, move_bps,
            "OBS-D01: positive displacement, abs==move"
        );
        let gap = d.gap_to_threshold_bps.expect("OBS-D01: gap must be Some");
        assert!(gap <= 0, "OBS-D01: gap <= 0 means threshold was met");
        assert_eq!(
            d.threshold_bps, MICRO_MOVE_BPS,
            "OBS-D01: threshold matches constant"
        );
        assert_eq!(
            d.lookback_bars, LOOKBACK,
            "OBS-D01: lookback_bars matches constant"
        );
    }

    /// OBS-D02: below-threshold move → decision=flat_below_threshold, gap positive.
    #[test]
    fn obs_d02_below_threshold_is_flat_below_threshold() {
        // base=$530.57, last close = $530.64 → ~1 bps (below 20 bps threshold)
        let base = 530_570_000i64; // $530.57 in micros
        let last_close = 530_640_000i64; // $530.64 in micros → ~1.3 bps above
        let bars: Vec<BarStub> = vec![
            BarStub::new(1000, true, base, 1),
            BarStub::new(1001, true, base, 1),
            BarStub::new(1002, true, base, 1),
            BarStub::new(1003, true, base, 1),
            BarStub::new(1004, true, last_close, 1),
        ];
        let d = compute_diagnostics(&bars);
        assert_eq!(
            d.decision, DECISION_FLAT_BELOW_THRESHOLD,
            "OBS-D02: decision=flat_below_threshold"
        );
        assert_eq!(d.raw_direction, 0, "OBS-D02: raw_direction=0");
        let abs_bps = d.abs_move_bps.expect("OBS-D02: abs_move_bps must be Some");
        assert!(
            abs_bps < MICRO_MOVE_BPS,
            "OBS-D02: abs_move_bps below threshold"
        );
        let gap = d.gap_to_threshold_bps.expect("OBS-D02: gap must be Some");
        assert!(gap > 0, "OBS-D02: gap > 0 means below threshold");
    }

    /// OBS-D03: AMD pull-back scenario — 15 bps move stays flat_below_threshold.
    #[test]
    fn obs_d03_amd_pullback_scenario_flat_below_threshold() {
        // AMD: lookback $523.57, latest $530.64 at 15:35; then pulled back to +15 bps
        let lookback_micros = 523_570_000i64; // $523.57
                                              // +15 bps on $523.57 ≈ $524.35
        let latest_micros = 524_354_550i64; // ~$524.35 ≈ +15 bps
        let bars: Vec<BarStub> = vec![
            BarStub::new(1000, true, lookback_micros, 1),
            BarStub::new(1001, true, lookback_micros, 1),
            BarStub::new(1002, true, lookback_micros, 1),
            BarStub::new(1003, true, lookback_micros, 1),
            BarStub::new(1004, true, latest_micros, 1),
        ];
        let d = compute_diagnostics(&bars);
        assert_eq!(
            d.decision, DECISION_FLAT_BELOW_THRESHOLD,
            "OBS-D03: AMD +15bps pullback stays flat"
        );
        let gap = d.gap_to_threshold_bps.expect("OBS-D03: gap must be Some");
        assert!(gap > 0, "OBS-D03: gap_to_threshold_bps > 0");
        assert_eq!(
            d.lookback_close_micros,
            Some(lookback_micros),
            "OBS-D03: lookback_close_micros correct"
        );
        assert_eq!(
            d.latest_close_micros,
            Some(latest_micros),
            "OBS-D03: latest_close_micros correct"
        );
    }

    /// OBS-D04: negative displacement → decision=flat_due_to_negative_direction.
    #[test]
    fn obs_d04_negative_displacement_is_flat_due_to_negative_direction() {
        let base = 200_000_000i64;
        let last_close = base - base / 400; // -25 bps
        let bars: Vec<BarStub> = vec![
            BarStub::new(1000, true, base, 1),
            BarStub::new(1001, true, base, 1),
            BarStub::new(1002, true, base, 1),
            BarStub::new(1003, true, base, 1),
            BarStub::new(1004, true, last_close, 1),
        ];
        let d = compute_diagnostics(&bars);
        assert_eq!(
            d.decision, DECISION_FLAT_NEGATIVE,
            "OBS-D04: decision=flat_due_to_negative_direction"
        );
        assert_eq!(d.raw_direction, -1, "OBS-D04: raw_direction=-1");
        let move_bps = d.move_bps.expect("OBS-D04: move_bps must be Some");
        assert!(move_bps < 0, "OBS-D04: move_bps is negative");
    }

    /// OBS-D05: insufficient bars → no panic, decision=insufficient_bars.
    #[test]
    fn obs_d05_insufficient_bars_no_panic() {
        // Only 3 bars (< LOOKBACK=5)
        let bars: Vec<BarStub> = vec![
            BarStub::new(1000, true, 200_000_000, 1),
            BarStub::new(1001, true, 200_500_000, 1),
            BarStub::new(1002, true, 201_000_000, 1),
        ];
        let d = compute_diagnostics(&bars);
        assert_eq!(
            d.decision, DECISION_INSUFFICIENT_BARS,
            "OBS-D05: insufficient bars → no panic"
        );
        assert!(d.move_bps.is_none(), "OBS-D05: move_bps is None");
        assert!(
            d.gap_to_threshold_bps.is_none(),
            "OBS-D05: gap_to_threshold_bps is None"
        );
    }

    /// OBS-D06: empty bars → no panic, decision=insufficient_bars.
    #[test]
    fn obs_d06_empty_bars_no_panic() {
        let d = compute_diagnostics(&[]);
        assert_eq!(
            d.decision, DECISION_INSUFFICIENT_BARS,
            "OBS-D06: empty → no panic"
        );
        assert!(d.latest_bar_ts.is_none(), "OBS-D06: latest_bar_ts is None");
    }

    /// OBS-D07: incomplete last bar → decision=insufficient_bars.
    #[test]
    fn obs_d07_incomplete_last_bar_is_insufficient() {
        let base = 200_000_000i64;
        let bars: Vec<BarStub> = vec![
            BarStub::new(1000, true, base, 1),
            BarStub::new(1001, true, base, 1),
            BarStub::new(1002, true, base, 1),
            BarStub::new(1003, true, base, 1),
            BarStub::new(1004, false, 0, 0), // incomplete last bar
        ];
        let d = compute_diagnostics(&bars);
        assert_eq!(
            d.decision, DECISION_INSUFFICIENT_BARS,
            "OBS-D07: incomplete last bar → insufficient_bars"
        );
    }

    /// OBS-D08: AMD 135 bps candidate → decision=signal_long.
    #[test]
    fn obs_d08_amd_valid_signal_candidate_is_signal_long() {
        // AMD 15:35 scenario: lookback $523.57, latest $530.64 → +135 bps
        let lookback_micros = 523_570_000i64;
        let latest_micros = 530_640_000i64;
        let bars: Vec<BarStub> = vec![
            BarStub::new(1000, true, lookback_micros, 1),
            BarStub::new(1001, true, lookback_micros, 1),
            BarStub::new(1002, true, lookback_micros, 1),
            BarStub::new(1003, true, lookback_micros, 1),
            BarStub::new(1004, true, latest_micros, 1),
        ];
        let d = compute_diagnostics(&bars);
        assert_eq!(
            d.decision, DECISION_SIGNAL_LONG,
            "OBS-D08: AMD 135bps → signal_long"
        );
        let move_bps = d.move_bps.unwrap();
        // (530_640_000 - 523_570_000) * 10_000 / 523_570_000 ≈ 135
        assert!((130..=140).contains(&move_bps), "OBS-D08: move_bps ~135");
        let gap = d.gap_to_threshold_bps.unwrap();
        assert!(
            gap < 0,
            "OBS-D08: gap < 0 (threshold exceeded by large margin)"
        );
    }

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

    // ── SHORT-SIDE-STRATEGY-GATE-01: short signal gate tests ─────────────────────

    /// SSG01: Default constructor has allow_short_signals=false (long-only by default).
    #[test]
    fn ssg01_default_is_long_only() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1);
        let out = s.on_bar(&ctx_with_bars(bearish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 0,
            "SSG01: default allow_short_signals=false → bearish maps to 0"
        );
    }

    /// SSG02: Bearish displacement with default config returns target qty 0 regardless of sizing.
    #[test]
    fn ssg02_bearish_default_config_returns_zero() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 3);
        let out = s.on_bar(&ctx_with_bars(bearish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 0,
            "SSG02: target_qty=3, bearish, default → target=0 (long-only)"
        );
    }

    /// SSG03: ss04 continuity — existing bearish-maps-to-flat behavior is unaffected.
    #[test]
    fn ssg03_ss04_continuity_bearish_long_only_flat() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1);
        let out = s.on_bar(&ctx_with_bars(bearish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 0,
            "SSG03: ss04 continuity — bearish → target=0 with default (long-only)"
        );
    }

    /// SSG04: Bearish displacement with short signals enabled returns negative target qty.
    #[test]
    fn ssg04_bearish_short_enabled_returns_negative_target() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1).short_signals(true);
        let out = s.on_bar(&ctx_with_bars(bearish_bars(base)));
        assert_eq!(
            out.targets[0].qty, -1,
            "SSG04: bearish + allow_short_signals=true → target=-1"
        );
    }

    /// SSG05: Bullish displacement with short signals enabled still returns positive target qty.
    #[test]
    fn ssg05_bullish_short_enabled_unchanged() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 3).short_signals(true);
        let out = s.on_bar(&ctx_with_bars(bullish_bars(base)));
        assert_eq!(
            out.targets[0].qty, 3,
            "SSG05: bullish + short_signals=true → target=3 (positive; unchanged)"
        );
    }

    /// SSG06: Below-threshold move with short signals enabled still returns zero.
    #[test]
    fn ssg06_below_threshold_short_enabled_returns_zero() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 1).short_signals(true);
        let out = s.on_bar(&ctx_with_bars(flat_bars(base)));
        assert_eq!(
            out.targets[0].qty, 0,
            "SSG06: below-threshold + short_signals=true → target=0 (no signal)"
        );
    }

    /// SSG07: Short target qty magnitude matches target_qty sizing (target_qty=5 → -5).
    #[test]
    fn ssg07_short_target_magnitude_matches_sizing() {
        let base = 200_000_000i64;
        let mut s = IntradayScalperStrategy::with_target_qty("AAPL", 5).short_signals(true);
        let out = s.on_bar(&ctx_with_bars(bearish_bars(base)));
        assert_eq!(
            out.targets[0].qty, -5,
            "SSG07: target_qty=5, bearish + short_signals=true → target=-5"
        );
    }

    /// SSG08: Diagnostics (disabled) — bearish returns flat_due_to_negative_direction.
    #[test]
    fn ssg08_diagnostics_long_only_returns_flat_negative() {
        let base = 200_000_000i64;
        let d = compute_diagnostics_with_config(&bearish_bars(base), false);
        assert_eq!(
            d.decision, DECISION_FLAT_NEGATIVE,
            "SSG08: disabled → flat_due_to_negative_direction"
        );
        assert_eq!(
            d.reason, "bearish displacement; long-only strategy maps to flat",
            "SSG08: reason text matches long-only label"
        );
    }

    /// SSG09: Diagnostics (enabled) — bearish returns short_due_to_negative_direction.
    #[test]
    fn ssg09_diagnostics_short_enabled_returns_signal_short() {
        let base = 200_000_000i64;
        let d = compute_diagnostics_with_config(&bearish_bars(base), true);
        assert_eq!(
            d.decision, DECISION_SIGNAL_SHORT,
            "SSG09: enabled → short_due_to_negative_direction"
        );
        assert_eq!(
            d.reason, "bearish displacement; short signals enabled",
            "SSG09: reason text matches short-enabled label"
        );
    }

    /// SSG10: Caps are applied to short signal magnitude (same rules as long side).
    /// target_qty=10, max_qty_cap=3, bearish + short → target=-3.
    #[test]
    fn ssg10_caps_applied_to_short_signal_magnitude() {
        let base = 200_000_000i64;
        let mut s =
            IntradayScalperStrategy::with_caps("AAPL", 10, Some(3), None).short_signals(true);
        let out = s.on_bar(&ctx_with_bars(bearish_bars(base)));
        assert_eq!(
            out.targets[0].qty, -3,
            "SSG10: target_qty=10 capped at 3, bearish + short_signals → target=-3"
        );
    }

    /// SSG11: B5 logic would block a short-from-flat target emitted by allow_short_signals=true.
    /// Mirrors the classification in decision.rs.  Full daemon integration is proven by
    /// scenario_native_strategy_b5_short_guard::b5_s01_sell_from_flat_is_blocked.
    #[test]
    fn ssg11_b5_logic_blocks_short_from_flat_target() {
        let base = 200_000_000i64;
        let mut strategy = IntradayScalperStrategy::with_target_qty("AAPL", 1).short_signals(true);
        let out = strategy.on_bar(&ctx_with_bars(bearish_bars(base)));

        assert_eq!(
            out.targets[0].qty, -1,
            "SSG11: strategy emits target=-1 (short signal expressed)"
        );

        // B5 guard in decision.rs: delta < 0 AND (current <= 0 OR abs(delta) > current)
        // → ShortOpen intent → filtered (no decision produced).
        let current = 0i64; // flat position
        let delta = out.targets[0].qty - current; // -1
        let is_b5_blocked = delta < 0 && current <= 0;
        assert!(
            is_b5_blocked,
            "SSG11: delta={delta}, current={current} → B5 classifies ShortOpen and blocks"
        );
    }
}
