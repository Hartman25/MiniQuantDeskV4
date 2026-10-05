use super::daily_math::close;
use super::window::{advance_state, complete_valid_ohlc_tail, restore_long_flat};
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, HeldPositionSeed, RestartRecovery, Strategy, StrategyContext,
    StrategyDataRequirements, StrategyMeta, StrategyOutput, StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "volatility_contraction_breakout_v1";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// Short and long high-low ranges over the sessions BEFORE the decision bar.
const SHORT_RANGE_BARS: usize = 10;
const LONG_RANGE_BARS: usize = 60;
/// Contraction iff `range_short * 4 < range_long` (short range strictly under 25% of the long).
const CONTRACTION_DIVISOR: i128 = 4;
/// Breakout level: the highest close of the `BREAKOUT_BARS` sessions before the decision bar.
const BREAKOUT_BARS: usize = 20;
/// Exit level: the lowest close of the `EXIT_BARS` sessions before the decision bar.
const EXIT_BARS: usize = 10;
const REQUIRED_BARS: usize = LONG_RANGE_BARS + 1;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat volatility-contraction breakout: enter long while flat when the prior 10-session high-low range is under 25% of the prior 60-session range and the close is above the prior 20 closes, exit when the close falls below the prior 10 closes. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: REQUIRED_BARS,
    })
    // A LONG lasts until its exit rule with no bound on its duration: the durable held-position
    // record seeds Long/Flat exactly (`restore_held_positions`); a window replay cannot.
    .with_restart_recovery(RestartRecovery::DurableStateRequired)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Position {
    Flat,
    Long,
}

#[derive(Clone, Debug)]
pub struct VolatilityContractionBreakoutV1Strategy {
    symbol: String,
    state: Position,
    initialized: bool,
}

fn high_low_range(bars: &[BarStub]) -> i128 {
    let hi = bars.iter().map(|b| i128::from(b.high_micros)).max();
    let lo = bars.iter().map(|b| i128::from(b.low_micros)).min();
    hi.zip(lo).map_or(0, |(h, l)| h - l)
}

impl VolatilityContractionBreakoutV1Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            state: Position::Flat,
            initialized: false,
        }
    }

    /// One step over exactly `REQUIRED_BARS` bars ending at the decision bar `t`; every range and
    /// level reads only bars before `t`.
    fn step(state: Position, win: &[BarStub]) -> Position {
        let Some(win) = complete_valid_ohlc_tail(win, REQUIRED_BARS) else {
            return Position::Flat;
        };
        let t = win.len() - 1;
        let c = close(&win[t]);
        match state {
            Position::Flat => {
                let short = high_low_range(&win[t - SHORT_RANGE_BARS..t]);
                let long = high_low_range(&win[t - LONG_RANGE_BARS..t]);
                let contraction = long > 0 && short * CONTRACTION_DIVISOR < long;
                let breakout = win[t - BREAKOUT_BARS..t]
                    .iter()
                    .map(close)
                    .max()
                    .is_some_and(|level| c > level);
                if contraction && breakout {
                    Position::Long
                } else {
                    Position::Flat
                }
            }
            Position::Long => {
                let broke_down = win[t - EXIT_BARS..t]
                    .iter()
                    .map(close)
                    .min()
                    .is_some_and(|floor| c < floor);
                if broke_down {
                    Position::Flat
                } else {
                    Position::Long
                }
            }
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

impl Strategy for VolatilityContractionBreakoutV1Strategy {
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
            .push_i64(SHORT_RANGE_BARS as i64)
            .push_i64(LONG_RANGE_BARS as i64)
            .push_i64(CONTRACTION_DIVISOR as i64)
            .push_i64(BREAKOUT_BARS as i64)
            .push_i64(EXIT_BARS as i64)
            .push_i64(REQUIRED_BARS as i64)
            .push_str("range:max_high_minus_min_low_of_prior_sessions_current_bar_excluded")
            .push_str(
                "contraction:long_range_positive_and_short_range_times_divisor_strictly_below_long",
            )
            .push_str("entry_flat:contraction_and_close_strictly_above_prior_closes_max")
            .push_str("exit_long:close_strictly_below_prior_closes_min")
            .push_str("direction:long_flat")
            .push_str("malformed_window:fail_closed_flat")
            .push_str("incomplete_latest:hold_state")
            .push_str("state_recovery:first_call_window_replay_v1")
            .finish()
    }

    fn restore_held_positions(&mut self, held: &[HeldPositionSeed]) {
        restore_long_flat(
            &mut self.state,
            &mut self.initialized,
            &self.symbol,
            held,
            Position::Flat,
            Position::Long,
        );
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        self.advance(&ctx.recent.bars);
        StrategyOutput {
            // Fixed one-share Equity signal (0/+1): never short.
            targets: vec![TargetPosition::new(
                self.symbol.clone(),
                QtyMicros::from_whole_units(i64::from(self.state == Position::Long))
                    .unwrap_or(QtyMicros::ZERO),
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

    fn ohlc(open: i64, high: i64, low: i64, close: i64) -> BarStub {
        BarStub::with_ohlcv(0, true, open, high, low, close, 1)
    }

    fn ctx(b: Vec<BarStub>) -> StrategyContext {
        let len = b.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, b))
    }

    fn call(s: &mut VolatilityContractionBreakoutV1Strategy, b: &[BarStub]) -> i64 {
        s.on_bar(&ctx(b.to_vec())).targets[0]
            .qty
            .to_whole_units_checked()
            .unwrap()
    }

    fn fresh(b: &[BarStub]) -> i64 {
        call(&mut VolatilityContractionBreakoutV1Strategy::new("SPY"), b)
    }

    fn long(b: &[BarStub]) -> i64 {
        let mut s = VolatilityContractionBreakoutV1Strategy::new("SPY");
        s.state = Position::Long;
        s.initialized = true;
        call(&mut s, b)
    }

    /// 61 bars. Prior 60 (indices 0..=59): closes 100M, lows 100M; bars 0..=49 have high
    /// `100M + wide`, bars 50..=59 high `100M + narrow` (so range_short = `narrow` and
    /// range_long = `max(wide, narrow)`). The decision bar closes at `c` with an arbitrary,
    /// very wide own range that no statistic may read.
    fn win(wide: i64, narrow: i64, c: i64) -> Vec<BarStub> {
        let mut v: Vec<BarStub> = (0..50)
            .map(|_| ohlc(100 * M, 100 * M + wide, 100 * M, 100 * M))
            .collect();
        v.extend((0..10).map(|_| ohlc(100 * M, 100 * M + narrow, 100 * M, 100 * M)));
        v.push(ohlc(c, 1_000 * M, M, c));
        v
    }

    #[test]
    fn required_history_is_61_in_both_authorities() {
        let s = VolatilityContractionBreakoutV1Strategy::new("SPY");
        assert_eq!(s.required_history_bars(), 61);
        assert_eq!(meta().data_requirements.unwrap().minimum_completed_bars, 61);
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
        assert_eq!(
            meta().restart_recovery,
            RestartRecovery::DurableStateRequired
        );
    }

    #[test]
    fn contraction_is_strictly_under_a_quarter_and_excludes_the_decision_bar() {
        let up = 101 * M;
        assert_eq!(fresh(&win(40 * M, 10 * M, up)), 0, "range10*4 == range60");
        assert_eq!(fresh(&win(40 * M + 1, 10 * M, up)), 1, "just under 25%");
        assert_eq!(fresh(&win(40 * M, 10 * M - 1, up)), 1);
        assert_eq!(fresh(&win(40 * M, 10 * M + 1, up)), 0);
        // The decision bar's own (huge) range would end the contraction if it were read.
        assert_eq!(fresh(&win(100 * M, M, up)), 1);
    }

    #[test]
    fn a_zero_long_range_is_never_a_contraction() {
        assert_eq!(fresh(&win(0, 0, 101 * M)), 0);
    }

    #[test]
    fn breakout_needs_a_close_strictly_above_the_prior_twenty_closes_only() {
        let mut w = win(40 * M, M, 100 * M);
        assert_eq!(fresh(&w), 0, "equal to the prior 20 closes");
        w = win(40 * M, M, 100 * M + 1);
        assert_eq!(fresh(&w), 1);
        // The close 20 sessions back (index 40) is read; 21 back (index 39) is not.
        let mut older = win(40 * M, M, 120 * M);
        older[39].close_micros = 500 * M;
        assert_eq!(
            fresh(&older),
            1,
            "close[t-21] is outside the breakout window"
        );
        let mut inside = win(40 * M, M, 120 * M);
        inside[40].close_micros = 500 * M;
        assert_eq!(fresh(&inside), 0, "close[t-20] is inside it");
    }

    #[test]
    fn exit_is_strictly_below_the_prior_ten_closes_and_contraction_is_not_rechecked() {
        let mut w = win(40 * M, M, 100 * M);
        assert_eq!(long(&w), 1, "equal to the prior 10 closes holds");
        w = win(40 * M, M, 100 * M - 1);
        assert_eq!(long(&w), 0);
        // Wide recent range (no contraction) does not exit a LONG.
        assert_eq!(long(&win(40 * M, 40 * M, 100 * M)), 1);
        // close[t-10] (index 50) is read; close[t-11] (index 49) is not.
        let mut inside = win(40 * M, M, 95 * M);
        inside[50].close_micros = 90 * M;
        assert_eq!(long(&inside), 1, "95M is not below the 90M floor");
        let mut outside = win(40 * M, M, 99 * M);
        outside[49].close_micros = 98 * M;
        assert_eq!(
            long(&outside),
            0,
            "99M < 100M; the older 98M is out of the window"
        );
    }

    #[test]
    fn short_window_and_malformed_bars_fail_closed_to_flat_even_from_long() {
        let good = win(40 * M, M, 120 * M);
        assert_eq!(fresh(&good), 1);
        assert_eq!(fresh(&good[1..]), 0, "60 bars");
        for mutate in [
            |b: &mut BarStub| b.close_micros = 0,
            |b: &mut BarStub| b.open_micros = 0,
            |b: &mut BarStub| b.low_micros = 0,
            |b: &mut BarStub| b.low_micros = b.high_micros + 1,
            |b: &mut BarStub| b.is_complete = false,
        ] {
            let mut bad = good.clone();
            mutate(&mut bad[10]);
            assert_eq!(fresh(&bad), 0);
            assert_eq!(long(&bad), 0, "malformed window -> FLAT even from LONG");
        }
    }

    #[test]
    fn incomplete_latest_bar_holds_the_state() {
        let good = win(40 * M, M, 120 * M);
        let mut s = VolatilityContractionBreakoutV1Strategy::new("SPY");
        assert_eq!(call(&mut s, &good), 1);
        let mut inc = win(40 * M, M, 10 * M);
        inc.last_mut().unwrap().is_complete = false;
        assert_eq!(
            call(&mut s, &inc),
            1,
            "an incomplete exit candidate does not exit"
        );
        let mut inc = good;
        inc.last_mut().unwrap().is_complete = false;
        assert_eq!(
            fresh(&inc),
            0,
            "an incomplete entry candidate does not enter"
        );
    }

    fn cycle_tape() -> Vec<BarStub> {
        // Period 120: 0..40 wide, 40..80 narrow at 100M, 80..100 breakout at 106M, 100..120 94M.
        (0..1_200)
            .map(|i| {
                let (close, half) = match i % 120 {
                    0..=39 => (100 * M, 20 * M),
                    40..=79 => (100 * M, M / 2),
                    80..=99 => (106 * M, M / 2),
                    _ => (94 * M, M / 2),
                };
                ohlc(close, close + half, close - half, close)
            })
            .collect()
    }

    #[test]
    fn restarted_instance_replay_equals_the_continuous_state_and_future_bars_do_not_leak() {
        let bars = cycle_tape();
        let mut s = VolatilityContractionBreakoutV1Strategy::new("SPY");
        let continuous: Vec<i64> = (1..=bars.len()).map(|i| call(&mut s, &bars[..i])).collect();
        assert!(continuous.contains(&1) && continuous.contains(&0));
        for t in [80, 100, 119, 200, 399, 700, 1_199] {
            assert_eq!(fresh(&bars[..=t]), continuous[t], "bar {t}");
        }
        let mut extended = bars[..300].to_vec();
        extended.push(ohlc(1, 900 * M, 1, 1));
        let mut s = VolatilityContractionBreakoutV1Strategy::new("SPY");
        let run: Vec<i64> = (1..=300).map(|i| call(&mut s, &extended[..i])).collect();
        assert_eq!(run, continuous[..300]);
    }

    #[test]
    fn durable_restart_at_every_boundary_equals_the_continuous_stream_and_kills_reset_to_flat() {
        let bars = restart_proof::stamped(cycle_tape());
        let cont =
            restart_proof::continuous(VolatilityContractionBreakoutV1Strategy::new("SPY"), &bars);
        assert!(
            cont.contains(&1) && cont.contains(&0),
            "fixture must exercise both"
        );
        for cap in [61, usize::MAX] {
            let bad = restart_proof::diverging_restarts(
                || VolatilityContractionBreakoutV1Strategy::new("SPY"),
                Some("SPY"),
                &cont,
                &bars,
                1,
                cap,
            );
            assert!(bad.is_empty(), "cap={cap}: {bad:?}");
        }
        let reset_flat = || {
            let mut s = VolatilityContractionBreakoutV1Strategy::new("SPY");
            s.initialized = true;
            s
        };
        let flat = restart_proof::diverging_restarts(reset_flat, None, &cont, &bars, 1, usize::MAX);
        assert!(
            (1..bars.len()).any(|r| cont[r - 1] == 1 && flat.contains(&r)),
            "reset-to-flat must diverge while a position is held"
        );
        // A record for another symbol restores nothing.
        let mut other = VolatilityContractionBreakoutV1Strategy::new("SPY");
        other.restore_held_positions(&[HeldPositionSeed {
            symbol: "QQQ".into(),
            entry_bar_end_ts: bars[0].end_ts,
        }]);
        assert_eq!(other.state, Position::Flat);
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_changes_with_every_field() {
        let live = VolatilityContractionBreakoutV1Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(live.len(), 64);
        assert_eq!(
            live,
            VolatilityContractionBreakoutV1Strategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            live,
            VolatilityContractionBreakoutV1Strategy::new("QQQ").semantic_fingerprint()
        );
        let fp = |version: &str, nums: [i64; 7], tokens: [&str; 8]| {
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
        // [timeframe, short range, long range, divisor, breakout, exit, required]
        let nums = [TIMEFRAME_SECS, 10, 60, 4, 20, 10, 61];
        let tokens = [
            "range:max_high_minus_min_low_of_prior_sessions_current_bar_excluded",
            "contraction:long_range_positive_and_short_range_times_divisor_strictly_below_long",
            "entry_flat:contraction_and_close_strictly_above_prior_closes_max",
            "exit_long:close_strictly_below_prior_closes_min",
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
