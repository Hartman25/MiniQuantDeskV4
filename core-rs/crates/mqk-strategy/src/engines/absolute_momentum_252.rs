use super::window::complete_positive_tail;
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

pub(crate) const NAME: &str = "absolute_momentum_252";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
/// The reference close is exactly this many completed bars before the latest.
const MOMENTUM_BARS: usize = 252;
/// Completed closes needed: the latest plus the reference 252 bars earlier.
const REQUIRED_BARS: usize = MOMENTUM_BARS + 1;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily long/flat absolute momentum: long one share while the latest completed close is strictly above the close 252 completed bars earlier, otherwise flat. Never short.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: REQUIRED_BARS,
    })
}

#[derive(Clone, Debug)]
pub struct AbsoluteMomentum252Strategy {
    symbol: String,
}

impl AbsoluteMomentum252Strategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    /// `1` iff `close[t] > close[t-252]` (strict); `0` on equality, on fewer
    /// than 253 bars, and on any incomplete bar or non-positive close in the
    /// 253-bar window.
    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        let Some(window) = complete_positive_tail(recent, REQUIRED_BARS) else {
            return 0;
        };
        let latest = window[REQUIRED_BARS - 1].close_micros;
        let reference = window[0].close_micros;
        i64::from(latest > reference)
    }
}

impl Strategy for AbsoluteMomentum252Strategy {
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
            .push_i64(MOMENTUM_BARS as i64)
            .push_str("entry_long:close>close_252_bars_earlier_strict")
            .push_str("equality:flat")
            .push_str("direction:long_flat")
            .push_str("malformed_window:fail_closed_flat")
            .push_str("incomplete_latest:flat")
            .finish()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let qty = Self::signal_from_recent(&ctx.recent.bars);
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
    use crate::{RecentBarsWindow, StrategyContext};

    const P: i64 = 100_000_000;

    fn bar(close_micros: i64, is_complete: bool) -> BarStub {
        BarStub::new(0, is_complete, close_micros, 1)
    }

    /// 253 bars: reference close first, `middle` for the 251 interior bars, latest last.
    fn series(reference: i64, middle: i64, latest: i64) -> Vec<BarStub> {
        let mut v = vec![bar(reference, true)];
        v.extend((0..REQUIRED_BARS - 2).map(|_| bar(middle, true)));
        v.push(bar(latest, true));
        v
    }

    fn sig(bars: &[BarStub]) -> i64 {
        AbsoluteMomentum252Strategy::signal_from_recent(bars)
    }

    fn ctx(bars: Vec<BarStub>) -> StrategyContext {
        let len = bars.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, bars))
    }

    #[test]
    fn required_history_is_253_and_declared_in_meta() {
        let s = AbsoluteMomentum252Strategy::new("SPY");
        assert_eq!(s.required_history_bars(), 253);
        assert_eq!(
            meta().data_requirements.unwrap().minimum_completed_bars,
            253
        );
    }

    #[test]
    fn fewer_than_253_completed_bars_is_flat() {
        let rising: Vec<BarStub> = (0..REQUIRED_BARS - 1)
            .map(|i| bar(P + i as i64 * 1_000_000, true))
            .collect();
        assert_eq!(rising.len(), 252);
        assert_eq!(
            sig(&rising),
            0,
            "a strongly rising 252-bar history is still flat"
        );
        assert_eq!(sig(&[]), 0);
    }

    #[test]
    fn exact_boundary_just_inside_and_just_outside() {
        assert_eq!(
            sig(&series(P, P, P + 1)),
            1,
            "one micro above the reference is long"
        );
        assert_eq!(sig(&series(P, P, P)), 0, "equality is flat");
        assert_eq!(sig(&series(P, P, P - 1)), 0, "one micro below is flat");
        assert_eq!(
            sig(&series(P, 5 * P, P + 1)),
            1,
            "interior closes do not matter"
        );
    }

    #[test]
    fn reference_is_exactly_252_bars_before_the_latest() {
        let mut bars = series(P, P, P + 1);
        // The bar one position later (251 bars before the latest) is not the reference.
        bars[1] = bar(P + 5, true);
        assert_eq!(sig(&bars), 1);
        // Shift: an extra older bar changes nothing; the reference stays at index len-253.
        let mut longer = vec![bar(10 * P, true)];
        longer.extend(series(P, P, P + 1));
        assert_eq!(longer.len(), 254);
        assert_eq!(sig(&longer), 1, "older-than-required history has no effect");
        let mut longer2 = vec![bar(1, true); 40];
        longer2.extend(series(P, P, P));
        assert_eq!(sig(&longer2), 0);
    }

    #[test]
    fn incomplete_latest_or_interior_bar_and_non_positive_close_fail_closed() {
        let mut bars = series(P, P, 2 * P);
        *bars.last_mut().unwrap() = bar(2 * P, false);
        assert_eq!(
            sig(&bars),
            0,
            "incomplete latest bar cannot create an action"
        );
        let mut bars = series(P, P, 2 * P);
        bars[100] = bar(P, false);
        assert_eq!(sig(&bars), 0);
        let mut bars = series(P, P, 2 * P);
        bars[100] = bar(0, true);
        assert_eq!(sig(&bars), 0);
        let mut bars = series(P, P, 2 * P);
        bars[0] = bar(-1, true);
        assert_eq!(sig(&bars), 0, "non-positive reference close");
        let mut bars = series(P, P, 2 * P);
        *bars.last_mut().unwrap() = bar(0, true);
        assert_eq!(sig(&bars), 0, "non-positive latest close");
    }

    /// The decision at bar T depends only on bars up to and including T.
    #[test]
    fn future_bars_are_not_used() {
        let all: Vec<BarStub> = (0..400)
            .map(|i| {
                bar(
                    P + (i % 17) * 900_000 + (i / 5) * 120_000 - (i % 7) * 400_000,
                    true,
                )
            })
            .collect();
        for t in REQUIRED_BARS - 1..all.len() {
            assert_eq!(
                sig(&all[..=t]),
                sig(&all[t + 1 - REQUIRED_BARS..=t]),
                "bar {t}"
            );
        }
        let mut extended = all.clone();
        extended.push(bar(1, true));
        for t in REQUIRED_BARS - 1..all.len() {
            assert_eq!(sig(&extended[..=t]), sig(&all[..=t]), "bar {t}");
        }
    }

    #[test]
    fn never_shorts_and_emits_one_symbol_target() {
        let mut s = AbsoluteMomentum252Strategy::new("SPY");
        assert_eq!(s.spec(), StrategySpec::new(NAME, TIMEFRAME_SECS));
        let out = s.on_bar(&ctx(series(P, P, 2 * P)));
        assert_eq!(out.targets.len(), 1);
        assert_eq!(out.targets[0].symbol, "SPY");
        assert_eq!(out.targets[0].qty.to_whole_units_checked().unwrap(), 1);
        for (r, l) in [(100, 1), (1, 100), (100, 100), (500, 20)] {
            let out = s.on_bar(&ctx(series(r * 1_000_000, r * 1_000_000, l * 1_000_000)));
            assert!(out.targets[0].qty.raw() >= 0);
        }
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_hex() {
        let a = AbsoluteMomentum252Strategy::new("SPY").semantic_fingerprint();
        assert_eq!(
            a,
            AbsoluteMomentum252Strategy::new("SPY").semantic_fingerprint()
        );
        assert_ne!(
            a,
            AbsoluteMomentum252Strategy::new("EFA").semantic_fingerprint()
        );
        assert_eq!(a.len(), 64);
        assert!(a
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }

    /// Mutation proof: every behavior-changing semantic is bound.
    #[test]
    fn fingerprint_changes_when_any_semantic_field_changes() {
        let fp = |name: &str, version: &str, tf: i64, bars: i64, rule: &str, eq: &str| {
            SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, name, version)
                .push_str("SPY")
                .push_i64(tf)
                .push_i64(bars)
                .push_str(rule)
                .push_str(eq)
                .push_str("direction:long_flat")
                .push_str("malformed_window:fail_closed_flat")
                .push_str("incomplete_latest:flat")
                .finish()
        };
        let live = AbsoluteMomentum252Strategy::new("SPY").semantic_fingerprint();
        let rule = "entry_long:close>close_252_bars_earlier_strict";
        assert_eq!(
            live,
            fp(NAME, VERSION, TIMEFRAME_SECS, 252, rule, "equality:flat"),
            "recipe mirrors the engine"
        );
        assert_ne!(
            live,
            fp(NAME, VERSION, TIMEFRAME_SECS, 251, rule, "equality:flat")
        );
        assert_ne!(
            live,
            fp(NAME, VERSION, TIMEFRAME_SECS, 252, rule, "equality:long")
        );
        assert_ne!(
            live,
            fp(
                NAME,
                VERSION,
                TIMEFRAME_SECS,
                252,
                "entry_long:close>=close_252_bars_earlier",
                "equality:flat"
            )
        );
        assert_ne!(
            live,
            fp(NAME, "0.1.1", TIMEFRAME_SECS, 252, rule, "equality:flat")
        );
        assert_ne!(live, fp(NAME, VERSION, 3_600, 252, rule, "equality:flat"));
    }
}
