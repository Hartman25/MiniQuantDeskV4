//! `grammar_v1`: a Research/Backtest-only, name-addressed, closed-vocabulary rule engine.
//!
//! The strategy NAME is the complete specification: `grammar_v1__<template>__<param>_<int>...` in one canonical
//! spelling. Nothing outside the name can change behaviour, so a Research trial, a Backtest and a fingerprint are
//! reproducible from the name alone. Every template is a stateless, long/flat, exact integer (`i128`) function of the
//! trailing completed daily closes and fails closed to flat on any short, incomplete or non-positive window.
//!
//! Registration is explicit and Research-side only (`register_grammar_strategy_if_named` at the Backtest/scanner/CLI
//! seams). `register_builtin_strategies*` and therefore the daemon/runtime fleet do NOT know these names: making a
//! grammar strategy deployable needs a separate, explicitly authorized builtin registration plus Promotion evidence.

use super::window::complete_positive_tail;
use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, PluginRegistry, RegistryError, Strategy, StrategyContext, StrategyDataRequirements,
    StrategyMeta, StrategyOutput, StrategySpec, TargetPosition,
};
use mqk_execution::QtyMicros;

/// Every grammar strategy name starts with this prefix.
pub const PREFIX: &str = "grammar_v1__";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleSpec {
    /// Long while `close > SMA(window)` (exact: `window * close > sum`).
    SmaTrendGate { window: u32 },
    /// Long while `SMA(fast) > SMA(slow)` (exact cross-multiplication).
    DualSmaCross { fast: u32, slow: u32 },
    /// Long while `close > close[lookback sessions earlier]`.
    AbsMomentum { lookback: u32 },
    /// Long while `close >= (1 - proximity_bps/10000) * max(close, window)` and, when `trend_window > 0`,
    /// `close > SMA(trend_window)`.
    NearHigh {
        window: u32,
        proximity_bps: u32,
        trend_window: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrammarNameError(pub String);

impl std::fmt::Display for GrammarNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid grammar_v1 strategy name: {}", self.0)
    }
}

fn err<T>(msg: impl Into<String>) -> Result<T, GrammarNameError> {
    Err(GrammarNameError(msg.into()))
}

fn bounded(name: &str, v: u32, lo: u32, hi: u32) -> Result<u32, GrammarNameError> {
    if (lo..=hi).contains(&v) {
        Ok(v)
    } else {
        err(format!("{name}={v} outside [{lo}, {hi}]"))
    }
}

impl RuleSpec {
    /// Parse the one canonical spelling. Anything else (padding, reordering, extra or missing parameters, out-of-range
    /// values, unknown templates) is refused.
    pub fn parse(name: &str) -> Result<RuleSpec, GrammarNameError> {
        let Some(rest) = name.strip_prefix(PREFIX) else {
            return err("missing prefix");
        };
        let mut parts = rest.split("__");
        let template = parts.next().unwrap_or("");
        let mut params: Vec<(&str, u32)> = Vec::new();
        for tok in parts {
            let Some((key, val)) = tok.rsplit_once('_') else {
                return err(format!("malformed parameter {tok:?}"));
            };
            if key.is_empty() || val.is_empty() || !val.bytes().all(|b| b.is_ascii_digit()) {
                return err(format!("malformed parameter {tok:?}"));
            }
            let v: u32 = val
                .parse()
                .map_err(|_| GrammarNameError(format!("unparseable value in {tok:?}")))?;
            params.push((key, v));
        }
        let get = |k: &str| -> Result<u32, GrammarNameError> {
            params
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| *v)
                .ok_or_else(|| GrammarNameError(format!("missing parameter {k}")))
        };
        let expect = |n: usize| -> Result<(), GrammarNameError> {
            if params.len() == n {
                Ok(())
            } else {
                err(format!("{template} takes exactly {n} parameters"))
            }
        };
        let spec = match template {
            "sma_trend_gate" => {
                expect(1)?;
                RuleSpec::SmaTrendGate {
                    window: bounded("window", get("window")?, 2, 1000)?,
                }
            }
            "dual_sma_cross" => {
                expect(2)?;
                let fast = bounded("fast", get("fast")?, 2, 500)?;
                let slow = bounded("slow", get("slow")?, 3, 1000)?;
                if fast >= slow {
                    return err("dual_sma_cross requires fast < slow");
                }
                RuleSpec::DualSmaCross { fast, slow }
            }
            "abs_momentum_sessions" => {
                expect(1)?;
                RuleSpec::AbsMomentum {
                    lookback: bounded("lookback", get("lookback")?, 2, 1000)?,
                }
            }
            "near_high_proximity" => {
                expect(3)?;
                RuleSpec::NearHigh {
                    window: bounded("window", get("window")?, 20, 1000)?,
                    proximity_bps: bounded("proximity_bps", get("proximity_bps")?, 1, 5000)?,
                    trend_window: bounded("trend_window", get("trend_window")?, 0, 1000)?,
                }
            }
            other => return err(format!("unknown template {other:?}")),
        };
        if spec.canonical_name() != name {
            return err("not the canonical spelling");
        }
        Ok(spec)
    }

    pub fn canonical_name(&self) -> String {
        match *self {
            RuleSpec::SmaTrendGate { window } => format!("{PREFIX}sma_trend_gate__window_{window}"),
            RuleSpec::DualSmaCross { fast, slow } => format!("{PREFIX}dual_sma_cross__fast_{fast}__slow_{slow}"),
            RuleSpec::AbsMomentum { lookback } => format!("{PREFIX}abs_momentum_sessions__lookback_{lookback}"),
            RuleSpec::NearHigh { window, proximity_bps, trend_window } => format!(
                "{PREFIX}near_high_proximity__window_{window}__proximity_bps_{proximity_bps}__trend_window_{trend_window}"
            ),
        }
    }

    /// Completed daily closes needed before the first meaningful decision.
    pub fn required_history(&self) -> usize {
        match *self {
            RuleSpec::SmaTrendGate { window } => window as usize,
            RuleSpec::DualSmaCross { slow, .. } => slow as usize,
            RuleSpec::AbsMomentum { lookback } => lookback as usize + 1,
            RuleSpec::NearHigh {
                window,
                trend_window,
                ..
            } => window.max(trend_window) as usize,
        }
    }

    fn describe(&self) -> &'static str {
        match self {
            RuleSpec::SmaTrendGate { .. } => "signal:window*close>sum(window closes)",
            RuleSpec::DualSmaCross { .. } => "signal:slow*sum(fast)>fast*sum(slow)",
            RuleSpec::AbsMomentum { .. } => "signal:close>close[lookback sessions earlier]",
            RuleSpec::NearHigh { .. } => "signal:10000*close>=(10000-bps)*max(close,window)&&(trend==0||trend*close>sum(trend))",
        }
    }

    fn template_id(&self) -> &'static str {
        match self {
            RuleSpec::SmaTrendGate { .. } => "sma_trend_gate",
            RuleSpec::DualSmaCross { .. } => "dual_sma_cross",
            RuleSpec::AbsMomentum { .. } => "abs_momentum_sessions",
            RuleSpec::NearHigh { .. } => "near_high_proximity",
        }
    }

    /// `1` (long one share) or `0` (flat). Fails closed to `0` on a short, incomplete or non-positive window.
    pub fn signal_from_recent(&self, recent: &[BarStub]) -> i64 {
        let Some(win) = complete_positive_tail(recent, self.required_history()) else {
            return 0;
        };
        let close = |b: &BarStub| b.close_micros as i128;
        let sum = |bars: &[BarStub]| -> i128 { bars.iter().map(close).sum() };
        let last = close(&win[win.len() - 1]);
        let long = match *self {
            RuleSpec::SmaTrendGate { window } => window as i128 * last > sum(win),
            RuleSpec::DualSmaCross { fast, slow } => {
                let (f, s) = (fast as usize, slow as usize);
                slow as i128 * sum(&win[win.len() - f..])
                    > fast as i128 * sum(&win[win.len() - s..])
            }
            RuleSpec::AbsMomentum { lookback } => {
                last > close(&win[win.len() - 1 - lookback as usize])
            }
            RuleSpec::NearHigh {
                window,
                proximity_bps,
                trend_window,
            } => {
                let high = win[win.len() - window as usize..]
                    .iter()
                    .map(close)
                    .max()
                    .unwrap_or(0);
                let near = 10_000 * last >= (10_000 - proximity_bps as i128) * high;
                let trend = trend_window == 0
                    || trend_window as i128 * last > sum(&win[win.len() - trend_window as usize..]);
                near && trend
            }
        };
        i64::from(long)
    }
}

pub fn meta_for(spec: &RuleSpec) -> StrategyMeta {
    StrategyMeta::new(
        spec.canonical_name(),
        VERSION,
        TIMEFRAME_SECS,
        format!(
            "grammar_v1 long/flat {}: exact integer rule, name is the specification. Never short.",
            spec.template_id()
        ),
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: spec.required_history(),
    })
}

#[derive(Clone, Debug)]
pub struct GrammarRuleStrategy {
    symbol: String,
    spec: RuleSpec,
}

impl GrammarRuleStrategy {
    pub fn new(symbol: impl Into<String>, spec: RuleSpec) -> Self {
        Self {
            symbol: symbol.into(),
            spec,
        }
    }
}

impl Strategy for GrammarRuleStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(self.spec.canonical_name(), TIMEFRAME_SECS)
    }

    fn required_history_bars(&self) -> usize {
        self.spec.required_history()
    }

    fn semantic_fingerprint(&self) -> String {
        // The canonical name carries every parameter, so the identity binds them through it.
        SemanticIdentityBuilder::new(
            SEMANTIC_IDENTITY_SCHEMA_V1,
            &self.spec.canonical_name(),
            VERSION,
        )
        .push_str(&self.symbol)
        .push_i64(TIMEFRAME_SECS)
        .push_str(self.spec.template_id())
        .push_str(self.spec.describe())
        .push_str("direction:long_flat")
        .push_str("malformed_window:fail_closed_flat")
        .push_str("incomplete_latest:flat")
        .finish()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let qty = self.spec.signal_from_recent(&ctx.recent.bars);
        StrategyOutput {
            // Fixed one-share Equity signal (0/+1): never short.
            targets: vec![TargetPosition::new(
                self.symbol.clone(),
                QtyMicros::from_whole_units(qty).unwrap_or(QtyMicros::ZERO),
            )],
        }
    }
}

/// Register exactly the one named grammar strategy for `symbol`. A name without the prefix is a no-op; a malformed
/// grammar name registers nothing, so the caller's `instantiate` fails closed with "unknown strategy".
pub fn register_grammar_strategy_if_named(
    registry: &mut PluginRegistry,
    symbol: impl Into<String>,
    name: &str,
) -> Result<(), RegistryError> {
    if !name.starts_with(PREFIX) || registry.contains(name) {
        return Ok(());
    }
    let Ok(spec) = RuleSpec::parse(name) else {
        return Ok(());
    };
    let symbol = symbol.into();
    registry.register(meta_for(&spec), move || {
        Box::new(GrammarRuleStrategy::new(symbol.clone(), spec)) as Box<dyn Strategy>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RecentBarsWindow, StrategyContext};

    const P: i64 = 100_000_000;

    fn bar(close: i64, complete: bool) -> BarStub {
        BarStub::new(0, complete, close, 1)
    }

    fn bars(closes: &[i64]) -> Vec<BarStub> {
        closes.iter().map(|&c| bar(c, true)).collect()
    }

    fn spec(name: &str) -> RuleSpec {
        RuleSpec::parse(name).unwrap()
    }

    #[test]
    fn canonical_names_round_trip_and_only_canonical_spelling_parses() {
        for name in [
            "grammar_v1__sma_trend_gate__window_50",
            "grammar_v1__dual_sma_cross__fast_20__slow_100",
            "grammar_v1__abs_momentum_sessions__lookback_63",
            "grammar_v1__near_high_proximity__window_252__proximity_bps_500__trend_window_0",
        ] {
            assert_eq!(spec(name).canonical_name(), name);
        }
        for bad in [
            "grammar_v1__sma_trend_gate__window_050",
            "grammar_v1__sma_trend_gate__window_1",
            "grammar_v1__sma_trend_gate__window_1001",
            "grammar_v1__sma_trend_gate__window_5__window_6",
            "grammar_v1__sma_trend_gate",
            "grammar_v1__sma_trend_gate__period_5",
            "grammar_v1__dual_sma_cross__fast_50__slow_20",
            "grammar_v1__dual_sma_cross__slow_100__fast_20",
            "grammar_v1__abs_momentum_sessions__lookback_1",
            "grammar_v1__near_high_proximity__window_19__proximity_bps_5__trend_window_0",
            "grammar_v1__near_high_proximity__window_252__proximity_bps_0__trend_window_0",
            "grammar_v1__near_high_proximity__window_252__proximity_bps_5001__trend_window_0",
            "grammar_v1__martingale__x_1",
            "grammar_v2__sma_trend_gate__window_50",
            "trend_sma50",
            "grammar_v1__sma_trend_gate__window_+5",
            "grammar_v1__sma_trend_gate__window_5 ",
        ] {
            assert!(RuleSpec::parse(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn required_history_per_template() {
        assert_eq!(
            spec("grammar_v1__sma_trend_gate__window_50").required_history(),
            50
        );
        assert_eq!(
            spec("grammar_v1__dual_sma_cross__fast_20__slow_100").required_history(),
            100
        );
        assert_eq!(
            spec("grammar_v1__abs_momentum_sessions__lookback_63").required_history(),
            64
        );
        assert_eq!(
            spec(
                "grammar_v1__near_high_proximity__window_100__proximity_bps_300__trend_window_150"
            )
            .required_history(),
            150
        );
    }

    #[test]
    fn sma_gate_is_strictly_above_the_exact_mean() {
        let s = spec("grammar_v1__sma_trend_gate__window_4");
        assert_eq!(
            s.signal_from_recent(&bars(&[P, P, P, P])),
            0,
            "equality is flat"
        );
        assert_eq!(
            s.signal_from_recent(&bars(&[P, P, P, P + 1])),
            1,
            "one micro above the exact mean is long"
        );
        assert_eq!(
            s.signal_from_recent(&bars(&[P + 4, P, P, P])),
            0,
            "older high bar pulls the mean above the close"
        );
        assert_eq!(
            s.signal_from_recent(&bars(&[P, P, P])),
            0,
            "short history is flat"
        );
    }

    #[test]
    fn dual_cross_compares_exact_means_without_division() {
        let s = spec("grammar_v1__dual_sma_cross__fast_2__slow_4");
        assert_eq!(s.signal_from_recent(&bars(&[P, P, P, P])), 0);
        assert_eq!(s.signal_from_recent(&bars(&[P, P, P + 1, P + 1])), 1);
        assert_eq!(s.signal_from_recent(&bars(&[P + 9, P + 9, P, P])), 0);
        // fast mean (P+1 + P)/2 vs slow mean (3P+... )/4 equal -> flat
        assert_eq!(s.signal_from_recent(&bars(&[P + 2, P, P + 1, P + 1])), 0);
    }

    #[test]
    fn momentum_compares_with_the_close_exactly_lookback_sessions_earlier() {
        let s = spec("grammar_v1__abs_momentum_sessions__lookback_3");
        assert_eq!(s.signal_from_recent(&bars(&[P, 2 * P, 2 * P, P + 1])), 1);
        assert_eq!(
            s.signal_from_recent(&bars(&[P, 2 * P, 2 * P, P])),
            0,
            "equality is flat"
        );
        assert_eq!(s.signal_from_recent(&bars(&[2 * P, P, P, P + 5])), 0);
        assert_eq!(
            s.signal_from_recent(&bars(&[P, P, P])),
            0,
            "lookback+1 bars are required"
        );
    }

    #[test]
    fn near_high_boundary_and_trend_filter() {
        let s =
            spec("grammar_v1__near_high_proximity__window_20__proximity_bps_500__trend_window_0");
        let mut v = vec![100 * P; 19];
        v.push(95 * P);
        assert_eq!(
            s.signal_from_recent(&bars(&v)),
            1,
            "exactly 5% below the high is included"
        );
        *v.last_mut().unwrap() = 95 * P - 1;
        assert_eq!(s.signal_from_recent(&bars(&v)), 0);
        // trend filter is strict: a close exactly at the 5-bar mean is not above it
        let flat =
            spec("grammar_v1__near_high_proximity__window_20__proximity_bps_500__trend_window_5");
        assert_eq!(flat.signal_from_recent(&bars(&vec![100 * P; 20])), 0);
        let t =
            spec("grammar_v1__near_high_proximity__window_20__proximity_bps_500__trend_window_5");
        let mut w = vec![100 * P; 15];
        w.extend([90 * P, 90 * P, 90 * P, 90 * P]);
        w.push(98 * P);
        assert_eq!(
            t.signal_from_recent(&bars(&w)),
            1,
            "near the high and above the 5-bar mean"
        );
        let mut w2 = vec![100 * P; 15];
        w2.extend([101 * P, 101 * P, 101 * P, 101 * P]);
        w2.push(99 * P);
        assert_eq!(
            t.signal_from_recent(&bars(&w2)),
            0,
            "near the high but below the trend mean"
        );
    }

    #[test]
    fn malformed_or_incomplete_windows_fail_closed_to_flat() {
        let s = spec("grammar_v1__sma_trend_gate__window_3");
        let mut v = bars(&[P, P, 3 * P]);
        assert_eq!(s.signal_from_recent(&v), 1);
        v[2] = bar(3 * P, false);
        assert_eq!(
            s.signal_from_recent(&v),
            0,
            "incomplete latest bar can never act"
        );
        v[2] = bar(3 * P, true);
        v[0] = bar(0, true);
        assert_eq!(
            s.signal_from_recent(&v),
            0,
            "non-positive close in the window"
        );
        v[0] = bar(-5, true);
        assert_eq!(s.signal_from_recent(&v), 0);
        assert_eq!(s.signal_from_recent(&[]), 0);
    }

    /// Independent, naive reference for every template over a deterministic pseudo-random walk.
    fn reference(spec: &RuleSpec, closes: &[i64]) -> i64 {
        let n = closes.len();
        let sma_gt = |w: usize| -> Option<(i128, i128)> {
            (n >= w).then(|| {
                (
                    closes[n - w..].iter().map(|&c| c as i128).sum::<i128>(),
                    w as i128,
                )
            })
        };
        let long = match *spec {
            RuleSpec::SmaTrendGate { window } => sma_gt(window as usize)
                .map(|(sum, w)| w * closes[n - 1] as i128 > sum)
                .unwrap_or(false),
            RuleSpec::DualSmaCross { fast, slow } => {
                match (sma_gt(fast as usize), sma_gt(slow as usize)) {
                    (Some((fs, fw)), Some((ss, sw))) => fs * sw > ss * fw,
                    _ => false,
                }
            }
            RuleSpec::AbsMomentum { lookback } => {
                n > lookback as usize && closes[n - 1] > closes[n - 1 - lookback as usize]
            }
            RuleSpec::NearHigh {
                window,
                proximity_bps,
                trend_window,
            } => {
                let need = window.max(trend_window) as usize;
                n >= need && {
                    let hi = *closes[n - window as usize..].iter().max().unwrap() as i128;
                    let near =
                        (closes[n - 1] as i128) * 10_000 >= (10_000 - proximity_bps as i128) * hi;
                    let trend = trend_window == 0
                        || sma_gt(trend_window as usize)
                            .map(|(s, w)| w * closes[n - 1] as i128 > s)
                            .unwrap_or(false);
                    near && trend
                }
            }
        };
        i64::from(long)
    }

    #[test]
    fn matches_an_independent_reference_over_a_pseudo_random_walk() {
        let mut x: u64 = 0x2545_F491_4F6C_DD1D;
        let mut px: i64 = 100 * P;
        let mut closes = Vec::new();
        for _ in 0..600 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            px = (px + ((x % 3_000_001) as i64 - 1_500_000) * 20).max(P);
            closes.push(px);
        }
        for name in [
            "grammar_v1__sma_trend_gate__window_20",
            "grammar_v1__dual_sma_cross__fast_10__slow_50",
            "grammar_v1__abs_momentum_sessions__lookback_30",
            "grammar_v1__near_high_proximity__window_60__proximity_bps_300__trend_window_20",
            "grammar_v1__near_high_proximity__window_60__proximity_bps_100__trend_window_0",
        ] {
            let s = spec(name);
            let mut longs = 0;
            for end in 1..=closes.len() {
                let got = s.signal_from_recent(&bars(&closes[..end]));
                assert_eq!(got, reference(&s, &closes[..end]), "{name} at {end}");
                longs += got;
            }
            assert!(
                longs > 0 && (longs as usize) < closes.len(),
                "{name} must exercise both states"
            );
        }
    }

    #[test]
    fn fingerprint_is_deterministic_symbol_bound_and_changes_with_any_parameter() {
        let fp = |sym: &str, name: &str| {
            GrammarRuleStrategy::new(sym, spec(name)).semantic_fingerprint()
        };
        let a = fp(
            "SPY",
            "grammar_v1__near_high_proximity__window_60__proximity_bps_300__trend_window_20",
        );
        assert_eq!(
            a,
            fp(
                "SPY",
                "grammar_v1__near_high_proximity__window_60__proximity_bps_300__trend_window_20"
            )
        );
        assert_eq!(a.len(), 64);
        assert_ne!(
            a,
            fp(
                "QQQ",
                "grammar_v1__near_high_proximity__window_60__proximity_bps_300__trend_window_20"
            )
        );
        for other in [
            "grammar_v1__near_high_proximity__window_61__proximity_bps_300__trend_window_20",
            "grammar_v1__near_high_proximity__window_60__proximity_bps_301__trend_window_20",
            "grammar_v1__near_high_proximity__window_60__proximity_bps_300__trend_window_21",
        ] {
            assert_ne!(a, fp("SPY", other), "{other}");
        }
        assert_ne!(
            fp("SPY", "grammar_v1__sma_trend_gate__window_50"),
            fp("SPY", "grammar_v1__abs_momentum_sessions__lookback_50")
        );
    }

    #[test]
    fn registration_is_explicit_idempotent_and_fails_closed() {
        let mut reg = PluginRegistry::new();
        register_grammar_strategy_if_named(&mut reg, "SPY", "trend_sma50").unwrap();
        assert!(reg.is_empty(), "a non-grammar name registers nothing");
        let name = "grammar_v1__sma_trend_gate__window_50";
        register_grammar_strategy_if_named(&mut reg, "SPY", name).unwrap();
        register_grammar_strategy_if_named(&mut reg, "SPY", name).unwrap();
        assert_eq!(reg.len(), 1);
        let meta = reg.lookup(name).unwrap();
        assert_eq!(
            meta.data_requirements
                .as_ref()
                .unwrap()
                .minimum_completed_bars,
            50
        );
        let mut s = reg.instantiate_verified(name).unwrap();
        assert_eq!(s.required_history_bars(), 50);
        let ctx = |v: Vec<BarStub>| {
            let n = v.len();
            StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(n, v))
        };
        let mut rising: Vec<i64> = vec![P; 49];
        rising.push(2 * P);
        assert_eq!(
            s.on_bar(&ctx(bars(&rising))).targets[0].qty,
            QtyMicros::from_whole_units(1).unwrap()
        );
        assert_eq!(
            s.on_bar(&ctx(bars(&vec![P; 50]))).targets[0].qty,
            QtyMicros::ZERO
        );
        let mut bad = PluginRegistry::new();
        register_grammar_strategy_if_named(
            &mut bad,
            "SPY",
            "grammar_v1__sma_trend_gate__window_050",
        )
        .unwrap();
        assert!(
            bad.instantiate("grammar_v1__sma_trend_gate__window_050")
                .is_err(),
            "a malformed name stays unknown"
        );
    }
}
