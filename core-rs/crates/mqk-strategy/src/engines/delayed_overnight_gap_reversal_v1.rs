use super::daily_math::{close, prior_true_range_sum};
use super::window::{advance_state, complete_valid_ohlc_tail};
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, RestartRecovery, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta,
    StrategyOutput, StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "delayed_overnight_gap_reversal_v1";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// ATR is the simple mean of the true ranges of the `ATR_BARS` sessions immediately BEFORE `t`.
const ATR_BARS: usize = 20;
/// Event iff `gap_fraction > 1.5 * atr_fraction`, as `3 / 2`.
const EVENT_MULT_NUM: i128 = 3;
const EVENT_MULT_DEN: i128 = 2;
/// Long outputs per event, the event bar's own output included: the signal is emitted when the
/// event bar completes and the position is held for this many sessions after it.
const HOLD_OUTPUTS: u8 = 3;
/// The oldest true range reads the close one session before it.
const REQUIRED_BARS: usize = ATR_BARS + 2;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat delayed overnight-gap reversal: when a session opens below the prior close by more than 1.5 times the prior 20-session ATR fraction, signal after that session completes and hold long for exactly 3 sessions with no extension. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: REQUIRED_BARS,
    })
    // The 3-session hold counter is phase-dependent over an unbounded event history (an in-hold
    // event is ignored), so a restart cannot prove it from a finite window.
    .with_restart_recovery(RestartRecovery::NotRecoverable)
}

/// Stateful hold machine; the instance owns `held` (Long outputs already emitted in the active
/// cycle, 0 = idle) and a fresh instance replays the earlier bars of its first call's window.
#[derive(Clone, Debug)]
pub struct DelayedOvernightGapReversalV1Strategy {
    symbol: String,
    held: u8,
    initialized: bool,
}

impl DelayedOvernightGapReversalV1Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            held: 0,
            initialized: false,
        }
    }

    /// With `S` the sum of the 20 prior true ranges (`ATR = S/20`) and `c1 = close[t-1]`:
    ///   `(c1 - open[t]) / c1  >  3/2 * (S/20) / c1`   <=>   `(c1 - open[t]) * 40  >  3 * S`
    /// (`c1` cancels). The event bar's own range is not in the ATR and its close is not read.
    fn event(win: &[BarStub]) -> bool {
        let t = win.len() - 1;
        let gap = close(&win[t - 1]) - i128::from(win[t].open_micros);
        let s = prior_true_range_sum(win, ATR_BARS);
        gap * ATR_BARS as i128 * EVENT_MULT_DEN > EVENT_MULT_NUM * s
    }

    /// One step over exactly `REQUIRED_BARS` bars ending at `t`. Bars 1..HOLD_OUTPUTS of an active
    /// cycle only count the hold (an in-hold event neither extends nor resizes it); the bar after
    /// the last Long output is an ordinary idle bar, so an event there starts a fresh cycle.
    fn step(held: u8, win: &[BarStub]) -> u8 {
        let Some(win) = complete_valid_ohlc_tail(win, REQUIRED_BARS) else {
            return 0;
        };
        if (1..HOLD_OUTPUTS).contains(&held) {
            held + 1
        } else {
            u8::from(Self::event(win))
        }
    }

    fn advance(&mut self, bars: &[BarStub]) {
        advance_state(
            &mut self.held,
            &mut self.initialized,
            bars,
            REQUIRED_BARS,
            0,
            Self::step,
        );
    }
}

impl Strategy for DelayedOvernightGapReversalV1Strategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(NAME, TIMEFRAME_SECS)
    }

    fn required_history_bars(&self) -> usize {
        REQUIRED_BARS
    }

    fn semantic_fingerprint(&self) -> String {
        SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, VERSION)
            .push_str(&self.symbol)
            .push_i64(TIMEFRAME_SECS)
            .push_i64(ATR_BARS as i64)
            .push_i64(EVENT_MULT_NUM as i64)
            .push_i64(EVENT_MULT_DEN as i64)
            .push_i64(i64::from(HOLD_OUTPUTS))
            .push_i64(REQUIRED_BARS as i64)
            .push_str("gap:prior_close_minus_decision_bar_open")
            .push_str("atr:simple_mean_true_range_of_20_sessions_before_decision_bar")
            .push_str("event:gap_fraction_strictly_above_multiple_of_atr_fraction")
            .push_str("signal:after_event_bar_completes_no_same_bar_fill")
            .push_str("hold:exact_sessions_after_signal_bar_no_extension_no_resize")
            .push_str("direction:long_flat")
            .push_str("malformed_window:fail_closed_flat")
            .push_str("incomplete_latest:hold_state")
            .push_str("state_recovery:first_call_window_replay_v1")
            .finish()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        self.advance(&ctx.recent.bars);
        StrategyOutput {
            // Fixed one-share Equity signal (0/+1): never short.
            targets: vec![TargetPosition::new(
                self.symbol.clone(),
                QtyMicros::from_whole_units(i64::from(self.held > 0)).unwrap_or(QtyMicros::ZERO),
            )],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RecentBarsWindow;

    const M: i64 = 1_000_000;

    fn ohlc(open: i64, high: i64, low: i64, close: i64) -> BarStub {
        BarStub::with_ohlcv(0, true, open, high, low, close, 1)
    }

    /// A quiet bar: closes and opens at 100M with a true range of exactly 2M.
    fn quiet() -> BarStub {
        ohlc(100 * M, 101 * M, 99 * M, 100 * M)
    }

    /// 21 quiet bars (indices 0..=20: ATR over 1..=20 is exactly 2M) then the decision bar.
    fn window(open: i64, tail: BarStub) -> Vec<BarStub> {
        let mut v: Vec<BarStub> = (0..21).map(|_| quiet()).collect();
        let mut t = tail;
        t.open_micros = open;
        v.push(t);
        v
    }

    /// An event bar: opens at `open`, closes back at 100M, with a wide own range.
    fn event_bar(open: i64) -> BarStub {
        ohlc(open, 500 * M, M, 100 * M)
    }

    fn ctx(b: Vec<BarStub>) -> StrategyContext {
        let len = b.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, b))
    }

    fn call(s: &mut DelayedOvernightGapReversalV1Strategy, b: &[BarStub]) -> i64 {
        s.on_bar(&ctx(b.to_vec())).targets[0]
            .qty
            .to_whole_units_checked()
            .unwrap()
    }

    fn sig(b: &[BarStub]) -> i64 {
        call(&mut DelayedOvernightGapReversalV1Strategy::new("SPY"), b)
    }

    fn run(bars: &[BarStub]) -> Vec<i64> {
        let mut s = DelayedOvernightGapReversalV1Strategy::new("SPY");
        (1..=bars.len()).map(|i| call(&mut s, &bars[..i])).collect()
    }

    #[test]
    fn required_history_is_22_in_both_authorities() {
        let s = DelayedOvernightGapReversalV1Strategy::new("SPY");
        assert_eq!(s.required_history_bars(), 22);
        assert_eq!(meta().data_requirements.unwrap().minimum_completed_bars, 22);
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
        assert_eq!(meta().restart_recovery, RestartRecovery::NotRecoverable);
    }

    #[test]
    fn event_boundary_is_strict_on_the_open_gap_and_atr_excludes_the_event_bar() {
        // close[t-1] = 100M and ATR = 2M: the event needs open < 100M - 1.5 * 2M = 97M. The event
        // bar's own wide range and its close are not read.
        assert_eq!(sig(&window(97 * M, event_bar(0))), 0, "gap == 1.5 * ATR");
        assert_eq!(
            sig(&window(97 * M - 1, event_bar(0))),
            1,
            "just past the boundary"
        );
        assert_eq!(sig(&window(97 * M + 1, event_bar(0))), 0);
        assert_eq!(sig(&window(100 * M, event_bar(0))), 0, "no gap");
        assert_eq!(
            sig(&window(110 * M, event_bar(0))),
            0,
            "a gap up is never an event"
        );
        // The gap is measured to the OPEN, not the close.
        let mut closes_low = event_bar(0);
        closes_low.close_micros = 50 * M;
        assert_eq!(sig(&window(97 * M, closes_low)), 0);
        let mut closes_high = event_bar(0);
        closes_high.close_micros = 150 * M;
        assert_eq!(sig(&window(97 * M - 1, closes_high)), 1);
    }

    fn hold_tape(extra_event_at: Option<usize>) -> Vec<BarStub> {
        // Event at 21 (open 90M) and an in-hold event at 22 (open 90M vs a 100M prior close).
        let mut v: Vec<BarStub> = (0..21).map(|_| quiet()).collect();
        v.push(ohlc(90 * M, 101 * M, 85 * M, 100 * M));
        v.push(ohlc(90 * M, 101 * M, 85 * M, 100 * M));
        for i in 23..32 {
            v.push(if Some(i) == extra_event_at {
                ohlc(90 * M, 101 * M, 85 * M, 100 * M)
            } else {
                quiet()
            });
        }
        v
    }

    #[test]
    fn hold_is_exactly_three_outputs_with_no_extension_and_a_fresh_event_after_the_hold() {
        let full = hold_tape(None);
        let out = run(&full);
        assert!(out[..21].iter().all(|&q| q == 0));
        assert_eq!(
            &out[21..=23],
            &[1, 1, 1],
            "three Long outputs, the in-hold event ignored"
        );
        assert!(out[24..].iter().all(|&q| q == 0), "no extension: {out:?}");
        // The idle bar right after the hold evaluates a fresh event: a new cycle, not an extension.
        let again = run(&hold_tape(Some(24)));
        assert_eq!(&again[21..=26], &[1, 1, 1, 1, 1, 1]);
        assert!(again[27..].iter().all(|&q| q == 0));
    }

    #[test]
    fn the_signal_is_never_emitted_before_the_event_bar_completes() {
        let mut inc = window(90 * M, event_bar(0));
        inc.last_mut().unwrap().is_complete = false;
        assert_eq!(sig(&inc), 0, "an incomplete event bar is no signal");
        let mut s = DelayedOvernightGapReversalV1Strategy::new("SPY");
        assert_eq!(call(&mut s, &window(90 * M, event_bar(0))), 1);
        assert_eq!(
            call(&mut s, &inc),
            1,
            "the counter holds on an incomplete bar"
        );
        assert_eq!(s.held, 1);
    }

    #[test]
    fn malformed_and_short_windows_fail_closed_to_flat_even_mid_hold() {
        let good = window(90 * M, event_bar(0));
        assert_eq!(sig(&good), 1);
        assert_eq!(sig(&good[1..]), 0, "21 bars");
        for mutate in [
            |b: &mut BarStub| b.close_micros = 0,
            |b: &mut BarStub| b.open_micros = 0,
            |b: &mut BarStub| b.low_micros = 0,
            |b: &mut BarStub| b.low_micros = b.high_micros + 1,
            |b: &mut BarStub| b.is_complete = false,
        ] {
            let mut bad = good.clone();
            mutate(&mut bad[5]);
            assert_eq!(sig(&bad), 0, "malformed bar in the window");
            let mut s = DelayedOvernightGapReversalV1Strategy::new("SPY");
            s.held = 2;
            s.initialized = true;
            assert_eq!(call(&mut s, &bad), 0, "malformed window ends a hold");
        }
    }

    #[test]
    fn a_restart_with_only_the_minimum_window_cannot_recover_the_hold() {
        let full = hold_tape(None);
        let continuous = run(&full);
        assert_eq!(continuous[22], 1, "mid-hold");
        let t = 22;
        assert_eq!(
            sig(&full[t + 1 - 22..=t]),
            1,
            "this window happens to hold its own event"
        );
        let t = 23;
        assert_eq!(continuous[t], 1);
        assert_eq!(
            sig(&full[t + 1 - 22..=t]),
            0,
            "capped-window restart sees no event"
        );
        let mut fresh = DelayedOvernightGapReversalV1Strategy::new("SPY");
        assert_eq!(
            call(&mut fresh, &full[..=t]),
            1,
            "an uncapped window replays the hold"
        );
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_changes_with_every_field() {
        let live = DelayedOvernightGapReversalV1Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(live.len(), 64);
        assert_eq!(
            live,
            DelayedOvernightGapReversalV1Strategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            live,
            DelayedOvernightGapReversalV1Strategy::new("QQQ").semantic_fingerprint()
        );
        let fp = |version: &str, nums: [i64; 6], tokens: [&str; 9]| {
            let mut b = SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, version);
            b.push_str("SPY");
            for n in nums {
                b.push_i64(n);
            }
            for t in tokens {
                b.push_str(t);
            }
            b.finish()
        };
        // [timeframe, atr bars, mult num, mult den, hold, required]
        let nums = [TIMEFRAME_SECS, 20, 3, 2, 3, 22];
        let tokens = [
            "gap:prior_close_minus_decision_bar_open",
            "atr:simple_mean_true_range_of_20_sessions_before_decision_bar",
            "event:gap_fraction_strictly_above_multiple_of_atr_fraction",
            "signal:after_event_bar_completes_no_same_bar_fill",
            "hold:exact_sessions_after_signal_bar_no_extension_no_resize",
            "direction:long_flat",
            "malformed_window:fail_closed_flat",
            "incomplete_latest:hold_state",
            "state_recovery:first_call_window_replay_v1",
        ];
        assert_eq!(live, fp(VERSION, nums, tokens), "recipe mirrors the engine");
        assert_ne!(live, fp("0.1.1", nums, tokens));
        for i in 0..nums.len() {
            let mut m = nums;
            m[i] += 1;
            assert_ne!(live, fp(VERSION, m, tokens), "number {i}");
        }
        for i in 0..tokens.len() {
            let mut m = tokens;
            m[i] = "mutated";
            assert_ne!(live, fp(VERSION, nums, m), "token {i}");
        }
    }
}
