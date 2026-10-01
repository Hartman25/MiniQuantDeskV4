use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "pullback_mean_reversion_20_2";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// Completed closes in the trailing window, including the latest bar.
const LOOKBACK: usize = 20;
/// Entry distance below the mean, in population standard deviations. Exactly 2,
/// so the entry condition squares to an integer comparison.
const SIGMA_MULTIPLIER: i128 = 2;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat pullback mean reversion: enter long when the completed close is at least 2 population standard deviations below its trailing 20-day mean, hold until the close reaches the mean. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: LOOKBACK,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Position {
    Flat,
    Long,
}

/// Stateful FLAT/LONG strategy. The instance owns its state and the same
/// instance sees sequential bars within a run. A fresh instance starts FLAT and,
/// on its first `on_bar` call, derives its state by replaying the same machine
/// over the earlier bars of that call's window (a no-op when the window holds a
/// single bar, as in every Backtest, emitter, scanner and robustness run).
#[derive(Clone, Debug)]
pub struct PullbackMeanReversion202Strategy {
    symbol: String,
    state: Position,
    initialized: bool,
}

impl PullbackMeanReversion202Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            state: Position::Flat,
            initialized: false,
        }
    }

    /// Sum and sum of squares (integer micros, `i128`) of a 20-bar window, or
    /// `None` if any bar is incomplete or has a non-positive close (malformed).
    fn window_sums(win: &[BarStub]) -> Option<(i128, i128)> {
        let mut s: i128 = 0;
        let mut q: i128 = 0;
        for b in win {
            if !b.is_complete || b.close_micros <= 0 {
                return None;
            }
            let c = b.close_micros as i128;
            s += c;
            q += c * c;
        }
        Some((s, q))
    }

    /// One step of the frozen state machine over an exactly-20-bar window whose
    /// last bar is complete. With `S` the sum and `Q` the sum of squares, the
    /// population variance is `(20*Q - S^2) / 400`, so:
    ///   entry  (FLAT): `c <= mean - 2*std`  <=>  `S - 20c >= 0` and
    ///                  `(S - 20c)^2 >= 4*(20*Q - S^2)` with `20*Q - S^2 > 0`
    ///   exit   (LONG): `c >= mean`          <=>  `20c >= S`
    /// Boundary equality is an entry and an exit respectively; a malformed
    /// window fails closed to FLAT.
    fn step(state: Position, win: &[BarStub]) -> Position {
        let Some((s, q)) = Self::window_sums(win) else {
            return Position::Flat;
        };
        let c = win[win.len() - 1].close_micros as i128;
        let n = LOOKBACK as i128;
        match state {
            Position::Flat => {
                let below = s - n * c;
                let spread = n * q - s * s;
                if spread > 0
                    && below >= 0
                    && below * below >= SIGMA_MULTIPLIER * SIGMA_MULTIPLIER * spread
                {
                    Position::Long
                } else {
                    Position::Flat
                }
            }
            Position::Long => {
                if n * c >= s {
                    Position::Flat
                } else {
                    Position::Long
                }
            }
        }
    }

    fn advance(&mut self, bars: &[BarStub]) {
        if !self.initialized {
            self.initialized = true;
            if bars.len() > 1 {
                // Replay every earlier completed bar once, from FLAT.
                for t in 0..bars.len() - 1 {
                    if t + 1 >= LOOKBACK && bars[t].is_complete {
                        self.state = Self::step(self.state, &bars[t + 1 - LOOKBACK..=t]);
                    }
                }
            }
        }
        if bars.len() < LOOKBACK {
            self.state = Position::Flat;
            return;
        }
        match bars.last() {
            Some(last) if last.is_complete => {
                self.state = Self::step(self.state, &bars[bars.len() - LOOKBACK..]);
            }
            // Incomplete latest bar: no new actionable state; hold the state.
            _ => {}
        }
    }
}

impl Strategy for PullbackMeanReversion202Strategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(NAME, TIMEFRAME_SECS)
    }

    fn required_history_bars(&self) -> usize {
        LOOKBACK
    }

    fn semantic_fingerprint(&self) -> String {
        SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, VERSION)
            .push_str(&self.symbol)
            .push_i64(TIMEFRAME_SECS)
            .push_i64(LOOKBACK as i64)
            .push_i64(SIGMA_MULTIPLIER as i64)
            .push_str("population_variance_divisor_n")
            .push_str("entry_flat:close<=mean-sigma*std")
            .push_str("exit_long:close>=mean")
            .push_str("direction:long_flat")
            .push_str("malformed_window:fail_closed_flat")
            .push_str("incomplete_latest:hold_state")
            .push_str("state_recovery:first_call_window_replay_v1")
            .finish()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        self.advance(&ctx.recent.bars);
        let qty = match self.state {
            Position::Long => 1,
            Position::Flat => 0,
        };
        StrategyOutput {
            // Fixed one-share Equity signal (0/+1): never short, never sized
            // for, or registered against, a non-Equity asset class.
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
    use crate::{BarStub, RecentBarsWindow, StrategyContext};

    const H: i64 = 100_000_000;
    const L: i64 = 90_000_000;

    fn bar(close: i64, complete: bool) -> BarStub {
        BarStub::new(0, complete, close, 1)
    }

    fn bars(closes: &[i64]) -> Vec<BarStub> {
        closes.iter().map(|&c| bar(c, true)).collect()
    }

    fn ctx(b: Vec<BarStub>) -> StrategyContext {
        let len = b.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, b))
    }

    fn qty(out: &StrategyOutput) -> i64 {
        out.targets[0].qty.to_whole_units_checked().unwrap()
    }

    /// Window of 16 closes at H, 3 at L and the latest `c`: with c == L the
    /// close sits EXACTLY 2 population std below the mean (mean 98M, std 4M).
    fn entry_window(c: i64) -> Vec<i64> {
        let mut v = vec![H; 16];
        v.extend([L; 3]);
        v.push(c);
        v
    }

    fn call(s: &mut PullbackMeanReversion202Strategy, closes: &[i64]) -> i64 {
        qty(&s.on_bar(&ctx(bars(closes))))
    }

    /// Bar-by-bar run over `closes`, one growing prefix window per call.
    fn run(closes: &[i64]) -> Vec<i64> {
        let mut s = PullbackMeanReversion202Strategy::new("SPY");
        (1..=closes.len())
            .map(|i| call(&mut s, &closes[..i]))
            .collect()
    }

    #[test]
    fn fewer_than_20_bars_is_flat() {
        let out = run(&[H, L, L, L, L, L, L, L, L, L, L, L, L, L, L, L, L, L, L]);
        assert!(out.iter().all(|&q| q == 0));
    }

    #[test]
    fn flat_at_the_exact_entry_threshold_enters_long() {
        let mut s = PullbackMeanReversion202Strategy::new("SPY");
        assert_eq!(call(&mut s, &entry_window(L)), 1, "equality is an entry");
    }

    #[test]
    fn flat_just_above_the_entry_threshold_stays_flat() {
        let mut s = PullbackMeanReversion202Strategy::new("SPY");
        assert_eq!(call(&mut s, &entry_window(L + 1)), 0);
    }

    #[test]
    fn flat_below_the_entry_threshold_enters_long() {
        let mut s = PullbackMeanReversion202Strategy::new("SPY");
        assert_eq!(call(&mut s, &entry_window(L - 1)), 1);
    }

    #[test]
    fn long_below_the_mean_remains_long() {
        // Enter at L, then the next bar closes below the (new) mean but is not an exit.
        let mut s = PullbackMeanReversion202Strategy::new("SPY");
        let mut w = entry_window(L);
        assert_eq!(call(&mut s, &w), 1);
        w.push(91_000_000);
        assert_eq!(call(&mut s, &w), 1, "still LONG below the mean");
    }

    #[test]
    fn long_at_the_exact_mean_exits() {
        // 9 at 99M, 9 at 101M, 1 at 100M, latest 100M: mean 100M == close.
        let mut closes = vec![99_000_000; 9];
        closes.extend([101_000_000; 9]);
        closes.extend([100_000_000, 100_000_000]);
        let mut s = PullbackMeanReversion202Strategy::new("SPY");
        s.state = Position::Long;
        s.initialized = true;
        assert_eq!(call(&mut s, &closes), 0, "equality at the mean is an exit");
    }

    #[test]
    fn long_just_below_the_mean_holds_and_above_exits() {
        let mut base = vec![99_000_000; 9];
        base.extend([101_000_000; 9]);
        base.push(100_000_000);
        for (c, expect) in [(100_000_000 - 1, 1), (100_000_000 + 1, 0)] {
            let mut closes = base.clone();
            closes.push(c);
            let mut s = PullbackMeanReversion202Strategy::new("SPY");
            s.state = Position::Long;
            s.initialized = true;
            assert_eq!(call(&mut s, &closes), expect, "close {c}");
        }
    }

    #[test]
    fn zero_variance_never_enters() {
        let closes = vec![H; 25];
        assert!(run(&closes).iter().all(|&q| q == 0));
    }

    #[test]
    fn malformed_window_fails_closed_even_from_long() {
        for bad in [0_i64, -5_000_000] {
            let mut s = PullbackMeanReversion202Strategy::new("SPY");
            assert_eq!(call(&mut s, &entry_window(L)), 1);
            let mut w = entry_window(L);
            w[3] = bad;
            w.push(L);
            assert_eq!(call(&mut s, &w), 0, "close {bad} in the window -> FLAT");
        }
        let mut s = PullbackMeanReversion202Strategy::new("SPY");
        let mut w = bars(&entry_window(L));
        w[5] = bar(H, false);
        assert_eq!(
            qty(&s.on_bar(&ctx(w))),
            0,
            "an incomplete interior bar blocks entry"
        );
    }

    #[test]
    fn incomplete_latest_bar_cannot_create_or_end_a_transition() {
        let mut s = PullbackMeanReversion202Strategy::new("SPY");
        let mut w = bars(&entry_window(L));
        *w.last_mut().unwrap() = bar(L, false);
        assert_eq!(qty(&s.on_bar(&ctx(w))), 0, "no entry on an incomplete bar");
        // While LONG an incomplete bar that would be an exit does not exit.
        let mut s = PullbackMeanReversion202Strategy::new("SPY");
        assert_eq!(call(&mut s, &entry_window(L)), 1);
        let mut w = bars(&entry_window(L));
        w.push(bar(200_000_000, false));
        assert_eq!(
            qty(&s.on_bar(&ctx(w))),
            1,
            "state held through an incomplete bar"
        );
    }

    #[test]
    fn bars_older_than_the_latest_20_do_not_affect_the_signal() {
        let base = entry_window(L);
        let mut with_old = vec![1_i64; 7];
        with_old.extend(&base);
        let mut with_huge = vec![900_000_000_000_i64; 7];
        with_huge.extend(&base);
        let sig = |c: &[i64]| {
            let mut s = PullbackMeanReversion202Strategy::new("SPY");
            // Mark initialized so only the 20-bar window decides (no replay of the old bars).
            s.initialized = true;
            call(&mut s, c)
        };
        assert_eq!(sig(&base), 1);
        assert_eq!(sig(&with_old), 1);
        assert_eq!(sig(&with_huge), 1);
    }

    #[test]
    fn future_bars_cannot_affect_an_earlier_decision() {
        let closes: Vec<i64> = (0..120)
            .map(|i| {
                100_000_000 + ((i * 37) % 17) as i64 * 900_000 - ((i / 9) % 5) as i64 * 2_500_000
            })
            .collect();
        let prefix = run(&closes[..80]);
        let mut extended = closes[..80].to_vec();
        extended.extend([1, 900_000_000_000, 5]);
        let with_future = run(&extended);
        assert_eq!(&with_future[..80], &prefix[..]);
    }

    /// Quiet noise around 100M with a sharp dip every 23 bars (70M, then a slow
    /// recovery 85M, 92M): exercises entry, a multi-bar LONG hold and an exit.
    fn wave(n: usize) -> Vec<i64> {
        (0..n)
            .map(|i| {
                let noise = (((i * 41) % 31) as i64 - 15) * 300_000;
                match i % 23 {
                    5 => 70_000_000,
                    6 => 85_000_000,
                    7 => 92_000_000,
                    _ => 100_000_000 + noise,
                }
            })
            .collect()
    }

    #[test]
    fn no_short_target_is_ever_produced_and_state_survives_sequential_calls() {
        let closes = wave(200);
        let out = run(&closes);
        assert!(out.iter().all(|&q| q == 0 || q == 1));
        assert!(out.iter().any(|&q| q == 1), "fixture must exercise LONG");
        // LONG persists across calls: it is held for more than one consecutive bar somewhere.
        assert!(out.windows(2).any(|w| w[0] == 1 && w[1] == 1));
    }

    #[test]
    fn a_fresh_instance_begins_flat() {
        let mut s = PullbackMeanReversion202Strategy::new("SPY");
        assert_eq!(s.state, Position::Flat);
        assert!(!s.initialized);
        assert_eq!(call(&mut s, &[H; 5]), 0);
    }

    /// The state machine is a deterministic function of history: a fresh
    /// instance handed the full prefix window reproduces the bar-by-bar
    /// instance's state at every bar (first-call replay).
    #[test]
    fn fresh_instance_replay_equals_the_sequential_state() {
        let closes = wave(260);
        let sequential = run(&closes);
        assert!(sequential.iter().any(|&q| q == 1) && sequential.iter().any(|&q| q == 0));
        for t in 0..closes.len() {
            let mut fresh = PullbackMeanReversion202Strategy::new("SPY");
            assert_eq!(call(&mut fresh, &closes[..=t]), sequential[t], "bar {t}");
        }
    }

    #[test]
    fn repeating_the_same_bar_is_idempotent() {
        let mut s = PullbackMeanReversion202Strategy::new("SPY");
        let w = entry_window(L);
        let first = call(&mut s, &w);
        assert_eq!(call(&mut s, &w), first);
        let mut ex = w.clone();
        ex.push(H);
        let exited = call(&mut s, &ex);
        assert_eq!(exited, 0);
        assert_eq!(call(&mut s, &ex), 0);
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_hex() {
        let a = PullbackMeanReversion202Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            a,
            PullbackMeanReversion202Strategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            a,
            PullbackMeanReversion202Strategy::new("EFA").semantic_fingerprint()
        );
        assert_eq!(a.len(), 64);
        assert!(a
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }

    /// Mutation proof: every behavior-bearing semantic token binds the digest.
    #[test]
    fn fingerprint_changes_when_any_semantic_field_changes() {
        let fp =
            |name: &str, version: &str, tf: i64, lookback: i64, sigma: i64, tokens: [&str; 7]| {
                let mut b =
                    SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, name, version);
                b.push_str("SPY")
                    .push_i64(tf)
                    .push_i64(lookback)
                    .push_i64(sigma);
                for t in tokens {
                    b.push_str(t);
                }
                b.finish()
            };
        let tokens = [
            "population_variance_divisor_n",
            "entry_flat:close<=mean-sigma*std",
            "exit_long:close>=mean",
            "direction:long_flat",
            "malformed_window:fail_closed_flat",
            "incomplete_latest:hold_state",
            "state_recovery:first_call_window_replay_v1",
        ];
        let live = PullbackMeanReversion202Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            live,
            fp(NAME, VERSION, TIMEFRAME_SECS, 20, 2, tokens),
            "recipe mirrors the engine"
        );
        assert_ne!(live, fp(NAME, VERSION, TIMEFRAME_SECS, 10, 2, tokens));
        assert_ne!(live, fp(NAME, VERSION, TIMEFRAME_SECS, 20, 3, tokens));
        assert_ne!(live, fp(NAME, "0.1.1", TIMEFRAME_SECS, 20, 2, tokens));
        assert_ne!(live, fp(NAME, VERSION, 3_600, 20, 2, tokens));
        for i in 0..tokens.len() {
            let mut mutated = tokens;
            mutated[i] = "mutated";
            assert_ne!(
                live,
                fp(NAME, VERSION, TIMEFRAME_SECS, 20, 2, mutated),
                "token {i}"
            );
        }
    }

    #[test]
    fn required_history_is_exactly_the_lookback_in_both_authorities() {
        let s = PullbackMeanReversion202Strategy::new("SPY");
        assert_eq!(s.required_history_bars(), LOOKBACK);
        assert_eq!(
            meta().data_requirements.unwrap().minimum_completed_bars,
            LOOKBACK
        );
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
        assert_eq!(
            qty(&PullbackMeanReversion202Strategy::new("SPY").on_bar(&ctx(bars(&entry_window(L))))),
            1
        );
        let out = PullbackMeanReversion202Strategy::new("SPY").on_bar(&ctx(bars(&entry_window(L))));
        assert_eq!(out.targets.len(), 1);
        assert_eq!(out.targets[0].symbol, "SPY");
    }
}
