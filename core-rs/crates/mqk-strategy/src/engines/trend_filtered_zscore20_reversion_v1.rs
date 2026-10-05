use super::daily_math::{above_sma, close, close_sum};
use super::window::{advance_state, complete_positive_tail, restore_long_flat};
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, HeldPositionSeed, RestartRecovery, Strategy, StrategyContext,
    StrategyDataRequirements, StrategyMeta, StrategyOutput, StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "trend_filtered_zscore20_reversion_v1";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// Trend: close strictly above the SMA of the latest 200 closes including the decision bar.
const TREND_BARS: usize = 200;
/// Mean and population standard deviation over the `Z_BARS` closes BEFORE the decision bar.
const Z_BARS: usize = 20;
/// Entry while flat when `z < -ENTRY_Z` (strict).
const ENTRY_Z: i128 = 2;
const REQUIRED_BARS: usize = TREND_BARS;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat trend-filtered 20-day z-score reversion: enter long while flat when the close is above its 200-day average and its z-score against the prior 20 closes is below -2, exit when the close is at or above the prior 20-close mean or the trend filter fails. Never short.",
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
pub struct TrendFilteredZscore20ReversionV1Strategy {
    symbol: String,
    state: Position,
    initialized: bool,
}

impl TrendFilteredZscore20ReversionV1Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            state: Position::Flat,
            initialized: false,
        }
    }

    /// One step over exactly `REQUIRED_BARS` bars ending at the decision bar `t`. With `n = 20`,
    /// `S1`/`S2` the sum/sum of squares of the prior `n` closes and `V = n*S2 - S1^2 = n^2 * var`
    /// (population variance, exact):
    ///   mean = S1/n,  std = sqrt(V)/n,  z = (n*c - S1) / sqrt(V)
    ///   z < -2   <=>   D = S1 - n*c > 0  and  D^2 > 4*V        (and V > 0: std <= 0 never enters)
    ///   c >= mean  <=>  n*c >= S1
    fn step(state: Position, win: &[BarStub]) -> Position {
        let Some(win) = complete_positive_tail(win, REQUIRED_BARS) else {
            return Position::Flat;
        };
        let t = win.len() - 1;
        let prior = &win[t - Z_BARS..t];
        let n = Z_BARS as i128;
        let c = close(&win[t]);
        let s1 = close_sum(prior);
        let s2: i128 = prior.iter().map(|b| close(b) * close(b)).sum();
        let v = n * s2 - s1 * s1;
        let d = s1 - n * c;
        let trend = above_sma(win, TREND_BARS);
        match state {
            Position::Flat if trend && v > 0 && d > 0 && d * d > ENTRY_Z * ENTRY_Z * v => {
                Position::Long
            }
            Position::Flat => Position::Flat,
            Position::Long if n * c >= s1 || !trend => Position::Flat,
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

impl Strategy for TrendFilteredZscore20ReversionV1Strategy {
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
            .push_i64(Z_BARS as i64)
            .push_i64(ENTRY_Z as i64)
            .push_i64(REQUIRED_BARS as i64)
            .push_str("trend:close_strictly_above_sma_including_decision_bar")
            .push_str("z:mean_and_population_std_of_prior_closes_current_bar_excluded")
            .push_str("entry_flat:trend_and_z_strictly_below_minus_entry_z_std_positive")
            .push_str("exit_long:close_at_or_above_prior_mean_or_trend_false")
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

    const A: i64 = 100_000_000;
    const D: i64 = 1_000_000;

    fn bar(close: i64) -> BarStub {
        BarStub::new(0, true, close, 1)
    }

    fn ctx(b: Vec<BarStub>) -> StrategyContext {
        let len = b.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, b))
    }

    fn call(s: &mut TrendFilteredZscore20ReversionV1Strategy, b: &[BarStub]) -> i64 {
        s.on_bar(&ctx(b.to_vec())).targets[0]
            .qty
            .to_whole_units_checked()
            .unwrap()
    }

    fn run(closes: &[i64]) -> Vec<i64> {
        let bars: Vec<BarStub> = closes.iter().map(|&c| bar(c)).collect();
        let mut s = TrendFilteredZscore20ReversionV1Strategy::new("SPY");
        (1..=bars.len()).map(|i| call(&mut s, &bars[..i])).collect()
    }

    /// 200 bars: 179 of `early`, then 20 prior closes `prior(i)`, then `c`.
    fn win(early: i64, prior: impl Fn(usize) -> i64, c: i64) -> Vec<BarStub> {
        let mut v: Vec<BarStub> = (0..179).map(|_| bar(early)).collect();
        v.extend((0..20).map(|i| bar(prior(i))));
        v.push(bar(c));
        v
    }

    /// Ten closes at `A + D` and ten at `A - D`: mean exactly `A`, population std exactly `D`.
    fn two_point(i: usize) -> i64 {
        if i.is_multiple_of(2) {
            A + D
        } else {
            A - D
        }
    }

    fn fresh(b: &[BarStub]) -> i64 {
        call(&mut TrendFilteredZscore20ReversionV1Strategy::new("SPY"), b)
    }

    fn long(b: &[BarStub]) -> i64 {
        let mut s = TrendFilteredZscore20ReversionV1Strategy::new("SPY");
        s.state = Position::Long;
        s.initialized = true;
        call(&mut s, b)
    }

    #[test]
    fn required_history_is_200_in_both_authorities() {
        let s = TrendFilteredZscore20ReversionV1Strategy::new("SPY");
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
    fn entry_is_strictly_below_minus_two_population_std() {
        let low = 50 * D;
        assert_eq!(
            fresh(&win(low, two_point, A - 2 * D)),
            0,
            "z == -2 is no entry"
        );
        assert_eq!(
            fresh(&win(low, two_point, A - 2 * D - 1)),
            1,
            "just below -2"
        );
        assert_eq!(fresh(&win(low, two_point, A - 2 * D + 1)), 0);
        assert_eq!(
            fresh(&win(low, two_point, A + 5 * D)),
            0,
            "a rise never enters"
        );
    }

    #[test]
    fn the_standard_deviation_is_the_population_one_over_prior_closes_only() {
        // Sample std (n-1) would be larger and reject this entry; the current close is not in
        // the statistics (including it would widen the std and also reject it).
        let low = 50 * D;
        assert_eq!(fresh(&win(low, two_point, A - 2 * D - 1)), 1);
        assert_eq!(fresh(&win(low, two_point, A - 2 * D - D / 100)), 1);
    }

    #[test]
    fn zero_standard_deviation_never_enters_even_on_a_collapse() {
        assert_eq!(fresh(&win(50 * D, |_| A, A - 1)), 0, "std == 0");
        assert_eq!(
            fresh(&win(50 * D, |_| A, A - 30 * D)),
            0,
            "std == 0, deep drop"
        );
        // One tick of movement makes std positive: the same collapse now qualifies.
        assert_eq!(
            fresh(&win(50 * D, |i| if i == 0 { A + 1 } else { A }, A - 30 * D)),
            1
        );
    }

    #[test]
    fn trend_must_hold_for_entry_and_failing_trend_exits_a_long() {
        assert_eq!(
            fresh(&win(300 * D, two_point, A - 3 * D)),
            0,
            "close below SMA200"
        );
        assert_eq!(fresh(&win(50 * D, two_point, A - 3 * D)), 1);
        // LONG below the mean with the trend failing exits; with the trend intact it holds.
        assert_eq!(long(&win(300 * D, two_point, A - 3 * D)), 0);
        assert_eq!(long(&win(50 * D, two_point, A - 3 * D)), 1);
    }

    #[test]
    fn exit_is_at_or_above_the_prior_mean() {
        let low = 50 * D;
        assert_eq!(long(&win(low, two_point, A - 1)), 1, "below the mean holds");
        assert_eq!(long(&win(low, two_point, A)), 0, "equal to the mean exits");
        assert_eq!(long(&win(low, two_point, A + 1)), 0);
    }

    #[test]
    fn short_window_and_malformed_bars_fail_closed_to_flat_even_from_long() {
        let good = win(50 * D, two_point, A - 3 * D);
        assert_eq!(fresh(&good), 1);
        assert_eq!(fresh(&good[1..]), 0, "199 bars");
        for mutate in [
            |b: &mut BarStub| b.close_micros = 0,
            |b: &mut BarStub| b.is_complete = false,
        ] {
            let mut bad = good.clone();
            mutate(&mut bad[60]);
            assert_eq!(fresh(&bad), 0);
            assert_eq!(long(&bad), 0, "malformed window -> FLAT even from LONG");
        }
    }

    #[test]
    fn incomplete_latest_bar_holds_the_state() {
        let good = win(50 * D, two_point, A - 3 * D);
        let mut s = TrendFilteredZscore20ReversionV1Strategy::new("SPY");
        assert_eq!(call(&mut s, &good), 1);
        let mut inc = good.clone();
        let last = inc.len() - 1;
        inc[last] = BarStub::new(0, false, 2 * A, 1);
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

    fn dip_tape() -> Vec<i64> {
        (0..500)
            .map(|i: i64| {
                let noise = (i * 7) % 3 * 10_000;
                let dip = match i % 25 {
                    23 => 5_000_000,
                    24 => 1_200_000,
                    _ => 0,
                };
                A + i * 50_000 + noise - dip
            })
            .collect()
    }

    #[test]
    fn restarted_instance_replay_equals_the_continuous_state_and_future_bars_do_not_leak() {
        // A rising line with periodic sharp dips that revert: entries on the dips (z < -2 of a
        // small-noise prior window), mean exits on the recoveries.
        let closes = dip_tape();
        let continuous = run(&closes);
        assert!(continuous.contains(&1) && continuous.contains(&0));
        let bars: Vec<BarStub> = closes.iter().map(|&c| bar(c)).collect();
        for t in [224, 249, 250, 274, 299, 400, 499] {
            let mut fresh_s = TrendFilteredZscore20ReversionV1Strategy::new("SPY");
            assert_eq!(call(&mut fresh_s, &bars[..=t]), continuous[t], "bar {t}");
        }
        let mut extended = closes[..300].to_vec();
        extended.extend([1, 900_000_000_000, 5]);
        assert_eq!(&run(&extended)[..300], &continuous[..300]);
    }

    #[test]
    fn durable_restart_at_every_boundary_equals_the_continuous_stream_and_kills_reset_to_flat() {
        let bars = restart_proof::stamped(dip_tape().into_iter().map(bar).collect());
        let cont =
            restart_proof::continuous(TrendFilteredZscore20ReversionV1Strategy::new("SPY"), &bars);
        assert!(
            cont.contains(&1) && cont.contains(&0),
            "fixture must exercise both"
        );
        for cap in [200, usize::MAX] {
            let bad = restart_proof::diverging_restarts(
                || TrendFilteredZscore20ReversionV1Strategy::new("SPY"),
                Some("SPY"),
                &cont,
                &bars,
                1,
                cap,
            );
            assert!(bad.is_empty(), "cap={cap}: {bad:?}");
        }
        let reset_flat = || {
            let mut s = TrendFilteredZscore20ReversionV1Strategy::new("SPY");
            s.initialized = true;
            s
        };
        let flat = restart_proof::diverging_restarts(reset_flat, None, &cont, &bars, 1, usize::MAX);
        assert!(
            (1..bars.len()).any(|r| cont[r - 1] == 1 && flat.contains(&r)),
            "reset-to-flat must diverge while a position is held"
        );
        // A record for another symbol restores nothing.
        let mut other = TrendFilteredZscore20ReversionV1Strategy::new("SPY");
        other.restore_held_positions(&[HeldPositionSeed {
            symbol: "QQQ".into(),
            entry_bar_end_ts: bars[0].end_ts,
        }]);
        assert_eq!(other.state, Position::Flat);
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_changes_with_every_field() {
        let live = TrendFilteredZscore20ReversionV1Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(live.len(), 64);
        assert_eq!(
            live,
            TrendFilteredZscore20ReversionV1Strategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            live,
            TrendFilteredZscore20ReversionV1Strategy::new("QQQ").semantic_fingerprint()
        );
        let fp = |version: &str, nums: [i64; 5], tokens: [&str; 8]| {
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
        // [timeframe, trend bars, z bars, entry z, required]
        let nums = [TIMEFRAME_SECS, 200, 20, 2, 200];
        let tokens = [
            "trend:close_strictly_above_sma_including_decision_bar",
            "z:mean_and_population_std_of_prior_closes_current_bar_excluded",
            "entry_flat:trend_and_z_strictly_below_minus_entry_z_std_positive",
            "exit_long:close_at_or_above_prior_mean_or_trend_false",
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
