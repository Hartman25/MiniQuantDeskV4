use super::daily_math::{above_sma, gain_loss_sums};
use super::window::{advance_state, complete_positive_tail};
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, RestartRecovery, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta,
    StrategyOutput, StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "trend_filtered_rsi5_reversion_v1";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// Trend: close strictly above the SMA of the latest 200 closes including the decision bar.
const TREND_BARS: usize = 200;
/// Cutler RSI over this many one-session close differences ending at the decision bar.
const RSI_DIFFS: usize = 5;
/// RSI thresholds in percent: entry strictly below, exit strictly above.
const ENTRY_BELOW: i128 = 30;
const EXIT_ABOVE: i128 = 70;
/// `TREND_BARS` bars end at the decision bar; the RSI needs `RSI_DIFFS + 1` of them.
const REQUIRED_BARS: usize = TREND_BARS;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat trend-filtered RSI(5) reversion: enter long while flat when the close is above its 200-day average and the Cutler RSI(5) is below 30, exit when the RSI is above 70 or the trend filter fails. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: REQUIRED_BARS,
    })
    // A LONG lasts until the RSI or trend exit, with no bound on its duration, so a restart
    // cannot prove the state from a finite history window.
    .with_restart_recovery(RestartRecovery::NotRecoverable)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Position {
    Flat,
    Long,
}

/// Stateful FLAT/LONG strategy; the instance owns its state and a fresh instance replays the
/// earlier bars of its first call's window (see `window::advance_state`).
#[derive(Clone, Debug)]
pub struct TrendFilteredRsi5ReversionV1Strategy {
    symbol: String,
    state: Position,
    initialized: bool,
}

impl TrendFilteredRsi5ReversionV1Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            state: Position::Flat,
            initialized: false,
        }
    }

    /// One step over exactly `REQUIRED_BARS` bars ending at the decision bar `t`. With `G`/`L`
    /// the sums of gains/losses of the five differences (the Cutler means share the divisor, so
    /// `RSI = 100*G/(G+L)`, and `RSI = 50` when `G + L == 0`):
    ///   RSI < 30  <=>  `100*G < 30*(G+L)`      RSI > 70  <=>  `100*G > 70*(G+L)`
    /// Both are false when `G + L == 0`, so a flat tape neither enters nor exits on RSI.
    /// `close == SMA200` is NOT trend.
    fn step(state: Position, win: &[BarStub]) -> Position {
        let Some(win) = complete_positive_tail(win, REQUIRED_BARS) else {
            return Position::Flat;
        };
        let trend = above_sma(win, TREND_BARS);
        let (g, l) = gain_loss_sums(win, RSI_DIFFS);
        let rsi_below_entry = 100 * g < ENTRY_BELOW * (g + l);
        let rsi_above_exit = 100 * g > EXIT_ABOVE * (g + l);
        match state {
            Position::Flat if trend && rsi_below_entry => Position::Long,
            Position::Flat => Position::Flat,
            Position::Long if rsi_above_exit || !trend => Position::Flat,
            Position::Long => Position::Long,
        }
    }

    fn advance(&mut self, bars: &[BarStub]) {
        advance_state(
            &mut self.state,
            &mut self.initialized,
            bars,
            REQUIRED_BARS,
            Position::Flat,
            Self::step,
        );
    }
}

impl Strategy for TrendFilteredRsi5ReversionV1Strategy {
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
            .push_i64(RSI_DIFFS as i64)
            .push_i64(ENTRY_BELOW as i64)
            .push_i64(EXIT_ABOVE as i64)
            .push_i64(REQUIRED_BARS as i64)
            .push_str("trend:close_strictly_above_sma_including_decision_bar")
            .push_str("rsi:cutler_simple_means_of_one_session_diffs_50_when_no_movement")
            .push_str("entry_flat:trend_and_rsi_strictly_below_entry")
            .push_str("exit_long:rsi_strictly_above_exit_or_trend_false")
            .push_str("direction:long_flat")
            .push_str("malformed_window:fail_closed_flat")
            .push_str("incomplete_latest:hold_state")
            .push_str("state_recovery:first_call_window_replay_v1")
            .finish()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        self.advance(&ctx.recent.bars);
        let qty = i64::from(self.state == Position::Long);
        StrategyOutput {
            // Fixed one-share Equity signal (0/+1): never short.
            targets: vec![TargetPosition::new(
                self.symbol.clone(),
                QtyMicros::from_whole_units(qty).unwrap_or(QtyMicros::ZERO),
            )],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RecentBarsWindow;

    const BASE: i64 = 100_000_000;

    fn bar(close: i64) -> BarStub {
        BarStub::new(0, true, close, 1)
    }

    fn ctx(b: Vec<BarStub>) -> StrategyContext {
        let len = b.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, b))
    }

    fn call(s: &mut TrendFilteredRsi5ReversionV1Strategy, b: &[BarStub]) -> i64 {
        s.on_bar(&ctx(b.to_vec())).targets[0]
            .qty
            .to_whole_units_checked()
            .unwrap()
    }

    fn run(closes: &[i64]) -> Vec<i64> {
        let bars: Vec<BarStub> = closes.iter().map(|&c| bar(c)).collect();
        let mut s = TrendFilteredRsi5ReversionV1Strategy::new("SPY");
        (1..=bars.len()).map(|i| call(&mut s, &bars[..i])).collect()
    }

    /// 200 bars ending in the diffs `+gain, -loss, 0, 0, 0` from `close[t-5] = BASE`, over a
    /// low history so the close is strictly above the SMA200 (trend true).
    fn rsi_window(gain: i64, loss: i64) -> Vec<BarStub> {
        let mut v: Vec<BarStub> = (0..194).map(|_| bar(BASE / 2)).collect();
        v.push(bar(BASE));
        let c1 = BASE + gain;
        let c2 = c1 - loss;
        v.extend([c1, c2, c2, c2, c2].map(bar));
        v
    }

    fn sig(b: &[BarStub]) -> i64 {
        let mut s = TrendFilteredRsi5ReversionV1Strategy::new("SPY");
        call(&mut s, b)
    }

    #[test]
    fn required_history_is_200_in_both_authorities() {
        let s = TrendFilteredRsi5ReversionV1Strategy::new("SPY");
        assert_eq!(s.required_history_bars(), 200);
        assert_eq!(
            meta().data_requirements.unwrap().minimum_completed_bars,
            200
        );
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
        assert_eq!(meta().restart_recovery, RestartRecovery::NotRecoverable);
    }

    #[test]
    fn rsi_threshold_boundaries_are_strict_at_30_and_70() {
        // gain G, loss L: RSI = 100G/(G+L). G=3,L=7 -> exactly 30 (no entry); L=7.000001 -> <30.
        let unit = 1_000_000;
        assert_eq!(
            sig(&rsi_window(3 * unit, 7 * unit)),
            0,
            "RSI == 30 is no entry"
        );
        assert_eq!(
            sig(&rsi_window(3 * unit, 7 * unit + 1)),
            1,
            "RSI just below 30 enters"
        );
        assert_eq!(
            sig(&rsi_window(3 * unit + 1, 7 * unit)),
            0,
            "RSI just above 30"
        );
    }

    #[test]
    fn rsi_exit_boundary_equality_stays_long_and_above_70_exits() {
        let unit = 1_000_000;
        // Enter via a deep drop, then evaluate a second window whose RSI is exactly/above 70.
        let long_state = |win: Vec<BarStub>| {
            let mut s = TrendFilteredRsi5ReversionV1Strategy::new("SPY");
            s.state = Position::Long;
            s.initialized = true;
            call(&mut s, &win)
        };
        assert_eq!(
            long_state(rsi_window(7 * unit, 3 * unit)),
            1,
            "RSI == 70 stays long"
        );
        assert_eq!(
            long_state(rsi_window(7 * unit + 1, 3 * unit)),
            0,
            "RSI > 70 exits"
        );
        assert_eq!(long_state(rsi_window(7 * unit - 1, 3 * unit)), 1);
    }

    #[test]
    fn a_tape_with_no_movement_has_rsi_50_and_neither_enters_nor_exits() {
        let flat: Vec<BarStub> = (0..200).map(|_| bar(BASE)).collect();
        assert_eq!(
            sig(&flat),
            0,
            "flat tape: RSI 50 and close == SMA is not trend"
        );
        let mut s = TrendFilteredRsi5ReversionV1Strategy::new("SPY");
        s.state = Position::Long;
        s.initialized = true;
        let mut up: Vec<BarStub> = (0..194).map(|_| bar(BASE / 2)).collect();
        up.extend([BASE; 6].map(bar));
        assert_eq!(
            call(&mut s, &up),
            1,
            "RSI 50 holds; trend true (close above SMA)"
        );
    }

    /// Five falling diffs (RSI 0) ending at a close whose distance from the exact SMA200 is
    /// `last_delta` micros: with `last_delta == 0` the 200-bar sum is exactly `200 * close`.
    fn sma_equal_window(last_delta: i64) -> Vec<BarStub> {
        let u = 194_000;
        let h = BASE - 15_000;
        let mut v: Vec<BarStub> = (0..194).map(|_| bar(h)).collect();
        v.push(bar(BASE + 5 * u));
        v.extend([4, 3, 2, 1, 0].map(|k| bar(BASE + k * u)));
        v[199].close_micros += last_delta;
        v
    }

    #[test]
    fn trend_equality_is_not_trend_and_failing_trend_exits_a_long() {
        for (delta, enters) in [(-1, 0), (0, 0), (1, 1)] {
            assert_eq!(
                sig(&sma_equal_window(delta)),
                enters,
                "close - SMA200 sign {delta}"
            );
        }
        for (delta, holds) in [(-1, 0), (0, 0), (1, 1)] {
            let mut s = TrendFilteredRsi5ReversionV1Strategy::new("SPY");
            s.state = Position::Long;
            s.initialized = true;
            assert_eq!(
                call(&mut s, &sma_equal_window(delta)),
                holds,
                "LONG, delta {delta}"
            );
        }
    }

    #[test]
    fn short_window_and_malformed_bars_fail_closed_to_flat_even_from_long() {
        let unit = 1_000_000;
        let good = rsi_window(3 * unit, 7 * unit + 1);
        assert_eq!(sig(&good), 1);
        assert_eq!(sig(&good[1..]), 0, "199 bars");
        let mut s = TrendFilteredRsi5ReversionV1Strategy::new("SPY");
        s.state = Position::Long;
        s.initialized = true;
        let mut bad = good.clone();
        bad[50].close_micros = 0;
        assert_eq!(
            call(&mut s, &bad),
            0,
            "malformed window -> FLAT even from LONG"
        );
        let mut s = TrendFilteredRsi5ReversionV1Strategy::new("SPY");
        let mut inc = good.clone();
        inc[10].is_complete = false;
        assert_eq!(call(&mut s, &inc), 0);
    }

    #[test]
    fn incomplete_latest_bar_holds_the_state() {
        let unit = 1_000_000;
        let good = rsi_window(3 * unit, 7 * unit + 1);
        let mut s = TrendFilteredRsi5ReversionV1Strategy::new("SPY");
        assert_eq!(call(&mut s, &good), 1);
        let mut inc = good.clone();
        let last = inc.len() - 1;
        inc[last] = BarStub::new(0, false, BASE * 3, 1);
        assert_eq!(
            call(&mut s, &inc),
            1,
            "an incomplete exit candidate does not exit"
        );
        let mut s = TrendFilteredRsi5ReversionV1Strategy::new("SPY");
        let mut inc = good;
        let last = inc.len() - 1;
        inc[last].is_complete = false;
        assert_eq!(
            call(&mut s, &inc),
            0,
            "an incomplete entry candidate does not enter"
        );
    }

    #[test]
    fn restarted_instance_replay_equals_the_continuous_state_and_future_bars_do_not_leak() {
        // A rising line with an 11-bar dip-and-recover cycle: entries on the dips, RSI exits
        // on the recoveries.
        let closes: Vec<i64> = (0..500)
            .map(|i: i64| {
                let dip = match i % 11 {
                    6 => 3,
                    7 => 6,
                    8 => 9,
                    9 => 10,
                    10 => 4,
                    _ => 0,
                };
                BASE + i * 100_000 - dip * 1_000_000
            })
            .collect();
        let continuous = run(&closes);
        assert!(
            continuous.contains(&1) && continuous.contains(&0),
            "fixture must exercise both"
        );
        let bars: Vec<BarStub> = closes.iter().map(|&c| bar(c)).collect();
        for t in [250, 300, 333, 400, 499] {
            let mut fresh = TrendFilteredRsi5ReversionV1Strategy::new("SPY");
            assert_eq!(call(&mut fresh, &bars[..=t]), continuous[t], "bar {t}");
        }
        let mut extended = closes[..300].to_vec();
        extended.extend([1, 900_000_000_000, 5]);
        assert_eq!(&run(&extended)[..300], &continuous[..300]);
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_hex() {
        let a = TrendFilteredRsi5ReversionV1Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            a,
            TrendFilteredRsi5ReversionV1Strategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            a,
            TrendFilteredRsi5ReversionV1Strategy::new("QQQ").semantic_fingerprint()
        );
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn fingerprint_changes_when_any_semantic_field_changes() {
        let fp = |version: &str, nums: [i64; 6], tokens: [&str; 8]| {
            let mut b = SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, version);
            b.push_str("SPY").push_i64(nums[0]);
            for n in &nums[1..] {
                b.push_i64(*n);
            }
            // nums = [timeframe, trend, rsi_diffs, entry, exit, required]
            for t in tokens {
                b.push_str(t);
            }
            b.finish()
        };
        let nums = [TIMEFRAME_SECS, 200, 5, 30, 70, 200];
        let tokens = [
            "trend:close_strictly_above_sma_including_decision_bar",
            "rsi:cutler_simple_means_of_one_session_diffs_50_when_no_movement",
            "entry_flat:trend_and_rsi_strictly_below_entry",
            "exit_long:rsi_strictly_above_exit_or_trend_false",
            "direction:long_flat",
            "malformed_window:fail_closed_flat",
            "incomplete_latest:hold_state",
            "state_recovery:first_call_window_replay_v1",
        ];
        let live = TrendFilteredRsi5ReversionV1Strategy::new("SPY").semantic_fingerprint();
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
