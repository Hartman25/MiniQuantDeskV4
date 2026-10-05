use super::daily_math::close;
use super::window::{advance_state, complete_positive_tail, restore_long_flat};
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, HeldPositionSeed, RestartRecovery, Strategy, StrategyContext,
    StrategyDataRequirements, StrategyMeta, StrategyOutput, StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "close_channel_100_50_trend_v1";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// Entry: close strictly above the highest of the prior `ENTRY_LOOKBACK` closes.
const ENTRY_LOOKBACK: usize = 100;
/// Exit: close strictly below the lowest of the prior `EXIT_LOOKBACK` closes.
const EXIT_LOOKBACK: usize = 50;
/// The prior closes plus the decision bar.
const REQUIRED_BARS: usize = ENTRY_LOOKBACK + 1;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat close channel: enter long while flat when the close exceeds the prior 100 closes' high, exit when it falls below the prior 50 closes' low. Never short.",
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
pub struct CloseChannel10050TrendV1Strategy {
    symbol: String,
    state: Position,
    initialized: bool,
}

impl CloseChannel10050TrendV1Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            state: Position::Flat,
            initialized: false,
        }
    }

    /// One step over exactly `REQUIRED_BARS` bars ending at the decision bar `t`. Both channels
    /// are taken over closes BEFORE `t`; equality with either boundary neither enters nor exits.
    fn step(state: Position, win: &[BarStub]) -> Position {
        let Some(win) = complete_positive_tail(win, REQUIRED_BARS) else {
            return Position::Flat;
        };
        let t = win.len() - 1;
        let c = close(&win[t]);
        match state {
            Position::Flat => {
                let high = win[t - ENTRY_LOOKBACK..t].iter().map(close).max();
                if high.is_some_and(|h| c > h) {
                    Position::Long
                } else {
                    Position::Flat
                }
            }
            Position::Long => {
                let low = win[t - EXIT_LOOKBACK..t].iter().map(close).min();
                if low.is_some_and(|l| c < l) {
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

impl Strategy for CloseChannel10050TrendV1Strategy {
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
            .push_i64(ENTRY_LOOKBACK as i64)
            .push_i64(EXIT_LOOKBACK as i64)
            .push_i64(REQUIRED_BARS as i64)
            .push_str("entry_flat:close_strictly_above_max_prior_closes_current_bar_excluded")
            .push_str("exit_long:close_strictly_below_min_prior_closes_current_bar_excluded")
            .push_str("direction:long_flat")
            .push_str("malformed_window:fail_closed_flat")
            .push_str("incomplete_latest:hold_state")
            .push_str("state_recovery:durable_held_position_seed_v1")
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

    const BASE: i64 = 100_000_000;

    fn bar(close: i64) -> BarStub {
        BarStub::new(0, true, close, 1)
    }

    fn ctx(b: Vec<BarStub>) -> StrategyContext {
        let len = b.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, b))
    }

    fn call(s: &mut CloseChannel10050TrendV1Strategy, b: &[BarStub]) -> i64 {
        s.on_bar(&ctx(b.to_vec())).targets[0]
            .qty
            .to_whole_units_checked()
            .unwrap()
    }

    fn run(closes: &[i64]) -> Vec<i64> {
        let bars: Vec<BarStub> = closes.iter().map(|&c| bar(c)).collect();
        let mut s = CloseChannel10050TrendV1Strategy::new("SPY");
        (1..=bars.len()).map(|i| call(&mut s, &bars[..i])).collect()
    }

    /// 101 bars: `prior(i)` for `i in 0..100` (i = 0 is `t - 100`), then `c` at `t`.
    fn win(prior: impl Fn(usize) -> i64, c: i64) -> Vec<BarStub> {
        let mut v: Vec<BarStub> = (0..100).map(|i| bar(prior(i))).collect();
        v.push(bar(c));
        v
    }

    fn fresh(b: &[BarStub]) -> i64 {
        call(&mut CloseChannel10050TrendV1Strategy::new("SPY"), b)
    }

    fn long(b: &[BarStub]) -> i64 {
        let mut s = CloseChannel10050TrendV1Strategy::new("SPY");
        s.state = Position::Long;
        s.initialized = true;
        call(&mut s, b)
    }

    #[test]
    fn required_history_is_101_in_both_authorities() {
        let s = CloseChannel10050TrendV1Strategy::new("SPY");
        assert_eq!(s.required_history_bars(), 101);
        assert_eq!(
            meta().data_requirements.unwrap().minimum_completed_bars,
            101
        );
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
        assert_eq!(
            meta().restart_recovery,
            RestartRecovery::DurableStateRequired
        );
    }

    #[test]
    fn entry_needs_a_strictly_higher_close_than_every_prior_100_and_excludes_the_current_bar() {
        let prior = |i: usize| if i == 0 { BASE + 10 } else { BASE };
        assert_eq!(
            fresh(&win(prior, BASE + 10)),
            0,
            "equal to the oldest prior high"
        );
        assert_eq!(fresh(&win(prior, BASE + 11)), 1);
        assert_eq!(fresh(&win(prior, BASE + 9)), 0);
        // The 100th prior bar (t - 100) is inside the channel; bars before it are not read.
        let mut older = vec![bar(10 * BASE)];
        older.extend(win(prior, BASE + 11));
        assert_eq!(
            fresh(&older),
            1,
            "a bar before t-100 does not raise the channel"
        );
    }

    #[test]
    fn exit_needs_a_strictly_lower_close_than_every_prior_50_and_stays_long_on_equality() {
        // Only the latest 50 prior closes matter: a lower close at t-51 is outside the exit window.
        let outside = |i: usize| if i == 49 { BASE - 10 } else { BASE };
        assert_eq!(
            long(&win(outside, BASE - 5)),
            0,
            "t-51 is not in the exit channel"
        );
        let inside = |i: usize| if i == 50 { BASE - 10 } else { BASE };
        assert_eq!(
            long(&win(inside, BASE - 5)),
            1,
            "t-50 lowers the exit channel"
        );
        assert_eq!(
            long(&win(inside, BASE - 10)),
            1,
            "equal to the low stays long"
        );
        assert_eq!(long(&win(inside, BASE - 11)), 0, "strictly below exits");
    }

    #[test]
    fn short_window_and_malformed_bars_fail_closed_to_flat_even_from_long() {
        let good = win(|_| BASE, BASE + 1);
        assert_eq!(fresh(&good), 1);
        assert_eq!(fresh(&good[1..]), 0, "100 bars");
        for mutate in [
            |b: &mut BarStub| b.close_micros = 0,
            |b: &mut BarStub| b.is_complete = false,
        ] {
            let mut bad = good.clone();
            mutate(&mut bad[30]);
            assert_eq!(fresh(&bad), 0);
            assert_eq!(long(&bad), 0, "malformed window -> FLAT even from LONG");
        }
    }

    #[test]
    fn incomplete_latest_bar_holds_the_state() {
        let good = win(|_| BASE, BASE + 1);
        let mut s = CloseChannel10050TrendV1Strategy::new("SPY");
        assert_eq!(call(&mut s, &good), 1);
        let mut inc = good.clone();
        let last = inc.len() - 1;
        inc[last] = BarStub::new(0, false, 1, 1);
        assert_eq!(
            call(&mut s, &inc),
            1,
            "an incomplete exit candidate does not exit"
        );
        let mut inc = good;
        let last = inc.len() - 1;
        inc[last].is_complete = false;
        assert_eq!(
            fresh(&inc),
            0,
            "an incomplete entry candidate does not enter"
        );
    }

    fn triangle_tape() -> Vec<i64> {
        (0..700)
            .map(|i: i64| {
                let phase = i % 240;
                BASE + if phase < 120 { phase } else { 240 - phase } * 100_000
            })
            .collect()
    }

    #[test]
    fn restarted_instance_replay_equals_the_continuous_state_and_future_bars_do_not_leak() {
        // Triangle wave: breakouts on the way up, channel exits on the way down.
        let closes = triangle_tape();
        let continuous = run(&closes);
        assert!(continuous.contains(&1) && continuous.contains(&0));
        let bars: Vec<BarStub> = closes.iter().map(|&c| bar(c)).collect();
        for t in [150, 230, 300, 399, 480, 699] {
            let mut fresh_s = CloseChannel10050TrendV1Strategy::new("SPY");
            assert_eq!(call(&mut fresh_s, &bars[..=t]), continuous[t], "bar {t}");
        }
        let mut extended = closes[..300].to_vec();
        extended.extend([1, 900_000_000_000, 5]);
        assert_eq!(&run(&extended)[..300], &continuous[..300]);
    }

    #[test]
    fn durable_restart_at_every_boundary_equals_the_continuous_stream_and_kills_reset_to_flat() {
        let bars = restart_proof::stamped(triangle_tape().into_iter().map(bar).collect());
        let cont = restart_proof::continuous(CloseChannel10050TrendV1Strategy::new("SPY"), &bars);
        assert!(
            cont.contains(&1) && cont.contains(&0),
            "fixture must exercise both"
        );
        for cap in [101, usize::MAX] {
            let bad = restart_proof::diverging_restarts(
                || CloseChannel10050TrendV1Strategy::new("SPY"),
                Some("SPY"),
                &cont,
                &bars,
                1,
                cap,
            );
            assert!(bad.is_empty(), "cap={cap}: {bad:?}");
        }
        let reset_flat = || {
            let mut s = CloseChannel10050TrendV1Strategy::new("SPY");
            s.initialized = true;
            s
        };
        let flat = restart_proof::diverging_restarts(reset_flat, None, &cont, &bars, 1, usize::MAX);
        assert!(
            (1..bars.len()).any(|r| cont[r - 1] == 1 && flat.contains(&r)),
            "reset-to-flat must diverge while a position is held"
        );
        // A record for another symbol restores nothing.
        let mut other = CloseChannel10050TrendV1Strategy::new("SPY");
        other.restore_held_positions(&[HeldPositionSeed {
            symbol: "QQQ".into(),
            entry_bar_end_ts: bars[0].end_ts,
        }]);
        assert_eq!(other.state, Position::Flat);
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_changes_with_every_field() {
        let live = CloseChannel10050TrendV1Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(live.len(), 64);
        assert_eq!(
            live,
            CloseChannel10050TrendV1Strategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            live,
            CloseChannel10050TrendV1Strategy::new("QQQ").semantic_fingerprint()
        );
        let fp = |version: &str, nums: [i64; 4], tokens: [&str; 6]| {
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
        // [timeframe, entry lookback, exit lookback, required]
        let nums = [TIMEFRAME_SECS, 100, 50, 101];
        let tokens = [
            "entry_flat:close_strictly_above_max_prior_closes_current_bar_excluded",
            "exit_long:close_strictly_below_min_prior_closes_current_bar_excluded",
            "direction:long_flat",
            "malformed_window:fail_closed_flat",
            "incomplete_latest:hold_state",
            "state_recovery:durable_held_position_seed_v1",
        ];
        assert_eq!(live, fp(VERSION, nums, tokens), "recipe mirrors the engine");
        let mut stale = tokens;
        stale[tokens.len() - 1] = "state_recovery:first_call_window_replay_v1";
        assert_ne!(
            live,
            fp(VERSION, nums, stale),
            "stale window-replay recovery recipe"
        );
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
