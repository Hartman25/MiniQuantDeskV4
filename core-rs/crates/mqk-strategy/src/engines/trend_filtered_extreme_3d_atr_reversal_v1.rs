use super::daily_math::{above_sma, close, prior_true_range_sum};
use super::window::{advance_state, complete_valid_ohlc_tail, hold_phase_from_anchor};
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, HeldPositionSeed, RestartRecovery, Strategy, StrategyContext,
    StrategyDataRequirements, StrategyMeta, StrategyOutput, StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "trend_filtered_extreme_3d_atr_reversal_v1";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// Trend: close strictly above the SMA of the latest 200 closes including the decision bar.
const TREND_BARS: usize = 200;
/// The drop is measured from `close[t - DROP_SESSIONS]` to `close[t]`.
const DROP_SESSIONS: usize = 3;
/// ATR is the simple mean of the true ranges of the `ATR_BARS` sessions immediately BEFORE `t`.
const ATR_BARS: usize = 20;
/// Event iff `drop_fraction > 1.5 * atr_fraction`, as `3 / 2`.
const EVENT_MULT_NUM: i128 = 3;
const EVENT_MULT_DEN: i128 = 2;
/// Long outputs per event, the event bar's own output included (the position is held for this
/// many sessions after the signal bar).
const HOLD_OUTPUTS: u8 = 5;
const REQUIRED_BARS: usize = TREND_BARS;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat trend-filtered 3-day extreme-drop reversal: when the close is above its 200-day average and the 3-session drop exceeds 1.5 times the prior 20-session ATR fraction, hold long for exactly 5 sessions with no extension. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: REQUIRED_BARS,
    })
    // The hold counter is phase-dependent over an unbounded event history: the durable entry
    // anchor seeds it (`restore_held_positions`); a window replay cannot.
    .with_restart_recovery(RestartRecovery::DurableStateRequired)
}

/// Stateful hold machine; the instance owns `held` (Long outputs already emitted in the active
/// cycle, 0 = idle) and a fresh instance replays the earlier bars of its first call's window.
#[derive(Clone, Debug)]
pub struct TrendFilteredExtreme3dAtrReversalV1Strategy {
    symbol: String,
    held: u8,
    initialized: bool,
    pending_anchor: Option<i64>,
}

impl TrendFilteredExtreme3dAtrReversalV1Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            held: 0,
            initialized: false,
            pending_anchor: None,
        }
    }

    /// With `S` the sum of the 20 prior true ranges (`ATR = S/20`):
    ///   `(c3 - c) / c3  >  3/2 * (S/20) / c1`   <=>   `(c3 - c) * 40 * c1  >  3 * S * c3`
    /// (`c3 = close[t-3]`, `c1 = close[t-1]`, all positive). The event bar's own range is not in
    /// the ATR.
    fn event(win: &[BarStub]) -> bool {
        let t = win.len() - 1;
        let (c, c3, c1) = (
            close(&win[t]),
            close(&win[t - DROP_SESSIONS]),
            close(&win[t - 1]),
        );
        let s = prior_true_range_sum(win, ATR_BARS);
        above_sma(win, TREND_BARS)
            && (c3 - c) * ATR_BARS as i128 * EVENT_MULT_DEN * c1 > EVENT_MULT_NUM * s * c3
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
        if let Some(anchor) = self.pending_anchor.take() {
            self.held =
                hold_phase_from_anchor(bars, anchor, REQUIRED_BARS, HOLD_OUTPUTS, Self::step);
            return;
        }
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

impl Strategy for TrendFilteredExtreme3dAtrReversalV1Strategy {
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
            .push_i64(TREND_BARS as i64)
            .push_i64(DROP_SESSIONS as i64)
            .push_i64(ATR_BARS as i64)
            .push_i64(EVENT_MULT_NUM as i64)
            .push_i64(EVENT_MULT_DEN as i64)
            .push_i64(i64::from(HOLD_OUTPUTS))
            .push_i64(REQUIRED_BARS as i64)
            .push_str("trend:close_strictly_above_sma_including_decision_bar")
            .push_str("atr:simple_mean_true_range_of_20_sessions_before_decision_bar")
            .push_str("event:drop_fraction_strictly_above_multiple_of_atr_fraction")
            .push_str("hold:exact_sessions_after_signal_bar_no_extension_no_resize")
            .push_str("direction:long_flat")
            .push_str("malformed_window:fail_closed_flat")
            .push_str("incomplete_latest:hold_state")
            .push_str("state_recovery:first_call_window_replay_v1")
            .finish()
    }

    fn restore_held_positions(&mut self, held: &[HeldPositionSeed]) {
        self.initialized = true;
        self.held = 0;
        self.pending_anchor = held
            .iter()
            .find(|h| h.symbol == self.symbol)
            .map(|h| h.entry_bar_end_ts);
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
    use super::super::window::restart_proof;
    use super::*;
    use crate::RecentBarsWindow;

    const M: i64 = 1_000_000;

    fn ohlc(prev: i64, c: i64, pad: i64) -> BarStub {
        let (h, l) = (prev.max(c) + pad, prev.min(c) - pad);
        BarStub::with_ohlcv(0, true, c, h, l, c, 1)
    }

    /// 0..=99 at `early`, a linear ramp to 100M over 100..=149, then 150..=196 flat at 100M with
    /// a true range of exactly 2M, then `tail` as `(close, pad)` bars from index 197. Bars 197
    /// and 198 (pad 0) also have a true range of exactly 2M, so the 20 prior true ranges of a
    /// decision bar at 199 sum to 40M (ATR 2M) regardless of the decision bar's own range.
    fn tape(early: i64, tail: &[(i64, i64)]) -> Vec<BarStub> {
        let mut v: Vec<BarStub> = (0..=99).map(|_| ohlc(early, early, 0)).collect();
        for i in 100..=149 {
            let c = early + (100 * M - early) * (i - 99) / 50;
            let prev = v.last().unwrap().close_micros;
            v.push(ohlc(prev, c, 0));
        }
        for _ in 150..=196 {
            v.push(ohlc(100 * M, 100 * M, M));
        }
        for &(c, pad) in tail {
            let prev = v.last().unwrap().close_micros;
            v.push(ohlc(prev, c, pad));
        }
        v
    }

    fn window(early: i64, last_close: i64) -> Vec<BarStub> {
        tape(early, &[(98 * M, 0), (100 * M, 0), (last_close, 5 * M)])
    }

    fn ctx(b: Vec<BarStub>) -> StrategyContext {
        let len = b.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, b))
    }

    fn call(s: &mut TrendFilteredExtreme3dAtrReversalV1Strategy, b: &[BarStub]) -> i64 {
        s.on_bar(&ctx(b.to_vec())).targets[0]
            .qty
            .to_whole_units_checked()
            .unwrap()
    }

    fn sig(b: &[BarStub]) -> i64 {
        call(
            &mut TrendFilteredExtreme3dAtrReversalV1Strategy::new("SPY"),
            b,
        )
    }

    fn run(bars: &[BarStub]) -> Vec<i64> {
        let mut s = TrendFilteredExtreme3dAtrReversalV1Strategy::new("SPY");
        (1..=bars.len()).map(|i| call(&mut s, &bars[..i])).collect()
    }

    #[test]
    fn required_history_is_200_in_both_authorities() {
        let s = TrendFilteredExtreme3dAtrReversalV1Strategy::new("SPY");
        assert_eq!(s.required_history_bars(), 200);
        assert_eq!(
            meta().data_requirements.unwrap().minimum_completed_bars,
            200
        );
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
        assert_eq!(
            meta().restart_recovery,
            RestartRecovery::DurableStateRequired
        );
    }

    #[test]
    fn event_boundary_is_strict_and_atr_excludes_the_event_bar() {
        // close[t-3] = close[t-1] = 100M and ATR = 2M: the event needs a drop > 1.5 * 2M = 3M.
        // The (wide) event bar's own range would raise the ATR if it were wrongly included.
        assert_eq!(
            sig(&window(60 * M, 97 * M)),
            0,
            "drop == 1.5 * ATR is not an event"
        );
        assert_eq!(
            sig(&window(60 * M, 97 * M - 1)),
            1,
            "just past the boundary"
        );
        assert_eq!(sig(&window(60 * M, 97 * M + 1)), 0);
        assert_eq!(sig(&window(60 * M, 100 * M)), 0, "no drop");
        assert_eq!(sig(&window(60 * M, 110 * M)), 0, "a rise is never an event");
    }

    #[test]
    fn trend_must_hold_at_the_decision_bar() {
        assert_eq!(sig(&window(60 * M, 90 * M)), 1);
        assert_eq!(
            sig(&window(300 * M, 90 * M)),
            0,
            "close below SMA200: no entry"
        );
    }

    #[test]
    fn hold_is_exactly_five_outputs_with_no_extension_and_a_fresh_event_after_the_hold() {
        // Event at 199; further qualifying events at 201 and 202 land inside the hold.
        let tail = [
            (98 * M, 0),
            (100 * M, 0),
            (96 * M + M / 2, 5 * M),
            (96 * M + M / 2, M),
            (90 * M, M),
        ];
        let mut full = tape(60 * M, &tail);
        full.extend((0..13).map(|_| ohlc(90 * M, 90 * M, M)));
        let fresh_event = |t: usize| sig(&full[t + 1 - 200..=t]) == 1;
        assert!(
            fresh_event(199) && fresh_event(201),
            "in-hold events exist in this fixture"
        );
        let out = run(&full);
        assert_eq!(out.len(), 215);
        assert!(out[..199].iter().all(|&q| q == 0));
        assert_eq!(&out[199..=203], &[1, 1, 1, 1, 1], "five Long outputs");
        assert!(
            out[204..].iter().all(|&q| q == 0),
            "no extension, then flat: {out:?}"
        );
        // The idle bar right after the hold evaluates a fresh event (a new cycle, not an
        // extension): an event at 204 makes a sixth consecutive Long output.
        let mut again = full[..204].to_vec();
        again.push(ohlc(90 * M, 84 * M, M));
        assert!(sig(&again[again.len() - 200..]) == 1);
        assert_eq!(*run(&again).last().unwrap(), 1);
    }

    #[test]
    fn malformed_and_short_windows_fail_closed_to_flat_even_mid_hold() {
        let good = window(60 * M, 90 * M);
        assert_eq!(sig(&good), 1);
        assert_eq!(sig(&good[1..]), 0, "199 bars");
        for mutate in [
            |b: &mut BarStub| b.close_micros = 0,
            |b: &mut BarStub| b.open_micros = 0,
            |b: &mut BarStub| b.low_micros = 0,
            |b: &mut BarStub| b.low_micros = b.high_micros + 1,
            |b: &mut BarStub| b.is_complete = false,
        ] {
            let mut bad = good.clone();
            mutate(&mut bad[100]);
            assert_eq!(sig(&bad), 0, "malformed bar in the window");
            let mut s = TrendFilteredExtreme3dAtrReversalV1Strategy::new("SPY");
            s.held = 3;
            s.initialized = true;
            assert_eq!(call(&mut s, &bad), 0, "malformed window ends a hold");
        }
    }

    #[test]
    fn incomplete_latest_bar_holds_the_counter() {
        let good = window(60 * M, 90 * M);
        let mut s = TrendFilteredExtreme3dAtrReversalV1Strategy::new("SPY");
        assert_eq!(call(&mut s, &good), 1);
        let mut inc = good.clone();
        let last = inc.len() - 1;
        inc[last].is_complete = false;
        assert_eq!(call(&mut s, &inc), 1);
        assert_eq!(s.held, 1, "the counter did not advance");
        assert_eq!(sig(&inc), 0, "an incomplete candidate event does not enter");
    }

    fn hold_fixture(chain: bool) -> Vec<BarStub> {
        let tail = [
            (98 * M, 0),
            (100 * M, 0),
            (96 * M + M / 2, 5 * M),
            (96 * M + M / 2, M),
            (90 * M, M),
        ];
        let mut full = tape(60 * M, &tail);
        full.extend((0..4).map(|_| ohlc(90 * M, 90 * M, M)));
        if chain {
            full.push(ohlc(90 * M, 84 * M, M));
            full.extend((0..12).map(|_| ohlc(84 * M, 84 * M, M)));
        } else {
            full.extend((0..9).map(|_| ohlc(90 * M, 90 * M, M)));
        }
        restart_proof::stamped(full)
    }

    fn fresh() -> TrendFilteredExtreme3dAtrReversalV1Strategy {
        TrendFilteredExtreme3dAtrReversalV1Strategy::new("SPY")
    }

    #[test]
    fn restart_at_every_boundary_equals_the_continuous_stream() {
        for chain in [false, true] {
            let full = hold_fixture(chain);
            let cont = restart_proof::continuous(fresh(), &full);
            let ones: Vec<usize> = (0..cont.len()).filter(|&i| cont[i] == 1).collect();
            assert_eq!(ones[0], 199);
            assert_eq!(ones.len(), if chain { 10 } else { 5 }, "{cont:?}");
            for cap in [usize::MAX, 260] {
                let bad =
                    restart_proof::diverging_restarts(fresh, Some("SPY"), &cont, &full, 1, cap);
                assert!(bad.is_empty(), "chain={chain} cap={cap}: {bad:?}");
            }
        }
        // The minimum daemon window (exactly 200 bars) is exact while no cycle boundary lies
        // before the window.
        let full = hold_fixture(false);
        let cont = restart_proof::continuous(fresh(), &full);
        let bad = restart_proof::diverging_restarts(fresh, Some("SPY"), &cont, &full, 199, 200);
        assert!(bad.is_empty(), "{bad:?}");
    }

    #[test]
    fn restart_mutations_reset_to_flat_and_reset_hold_counter_are_killed() {
        let full = hold_fixture(false);
        let cont = restart_proof::continuous(fresh(), &full);
        let reset_flat = || {
            let mut s = fresh();
            s.initialized = true;
            s
        };
        let reset_counter = || {
            let mut s = fresh();
            s.initialized = true;
            s.held = 1;
            s
        };
        let flat =
            restart_proof::diverging_restarts(reset_flat, None, &cont, &full, 200, usize::MAX);
        assert!(
            (200..=203).all(|r| flat.contains(&r)),
            "reset-to-flat must diverge at every mid-hold boundary: {flat:?}"
        );
        let counter =
            restart_proof::diverging_restarts(reset_counter, None, &cont, &full, 200, usize::MAX);
        assert!(
            (201..=203).all(|r| counter.contains(&r)),
            "reset-counter must diverge at every mid-hold boundary: {counter:?}"
        );
    }

    #[test]
    fn restore_fails_closed_to_flat_when_the_anchor_is_unprovable() {
        let full = hold_fixture(false);
        let seed = |ts: i64| {
            vec![HeldPositionSeed {
                symbol: "SPY".into(),
                entry_bar_end_ts: ts,
            }]
        };
        let decide_at = |seeds: &[HeldPositionSeed], window: &[BarStub]| {
            let mut s = fresh();
            s.restore_held_positions(seeds);
            restart_proof::decide(&mut s, window)
        };
        let w = &full[..=201];
        assert_eq!(decide_at(&seed(full[199].end_ts), w), 1, "provable anchor");
        assert_eq!(decide_at(&seed(12_345), w), 0, "anchor absent");
        assert_eq!(
            decide_at(&seed(full[200].end_ts), w),
            0,
            "anchor bar that starts no cycle"
        );
        assert_eq!(
            decide_at(&seed(full[199].end_ts), &w[10..]),
            0,
            "short window"
        );
        let other = vec![HeldPositionSeed {
            symbol: "QQQ".into(),
            entry_bar_end_ts: full[199].end_ts,
        }];
        assert_eq!(decide_at(&seed(full[199].end_ts), &full[..=200]), 1);
        assert_eq!(
            decide_at(&other, &full[..=200]),
            0,
            "another symbol's record"
        );
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_hex() {
        let a = TrendFilteredExtreme3dAtrReversalV1Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            a,
            TrendFilteredExtreme3dAtrReversalV1Strategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            a,
            TrendFilteredExtreme3dAtrReversalV1Strategy::new("QQQ").semantic_fingerprint()
        );
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn fingerprint_changes_when_any_semantic_field_changes() {
        let fp = |version: &str, nums: [i64; 8], tokens: [&str; 8]| {
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
        // [timeframe, trend, drop sessions, atr bars, mult num, mult den, hold, required]
        let nums = [TIMEFRAME_SECS, 200, 3, 20, 3, 2, 5, 200];
        let tokens = [
            "trend:close_strictly_above_sma_including_decision_bar",
            "atr:simple_mean_true_range_of_20_sessions_before_decision_bar",
            "event:drop_fraction_strictly_above_multiple_of_atr_fraction",
            "hold:exact_sessions_after_signal_bar_no_extension_no_resize",
            "direction:long_flat",
            "malformed_window:fail_closed_flat",
            "incomplete_latest:hold_state",
            "state_recovery:first_call_window_replay_v1",
        ];
        let live = TrendFilteredExtreme3dAtrReversalV1Strategy::new("SPY").semantic_fingerprint();
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
