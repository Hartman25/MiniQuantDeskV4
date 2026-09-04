use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};

pub(crate) const NAME: &str = "mean_reversion";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 3_600; // 1H
const LOOKBACK: usize = 20;
const EXTREME_BPS: i64 = 120; // 1.20%

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic mean reversion engine using close deviation from trailing average.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: LOOKBACK,
    })
}

#[derive(Clone, Debug)]
pub struct MeanReversionStrategy {
    symbol: String,
}

impl MeanReversionStrategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        if recent.len() < LOOKBACK {
            return 0;
        }

        let last = match recent.last() {
            Some(x) if x.is_complete => x,
            _ => return 0,
        };

        let window = &recent[recent.len() - LOOKBACK..];
        let sum: i128 = window.iter().map(|b| b.close_micros as i128).sum();
        let avg: i128 = sum / LOOKBACK as i128;
        if avg <= 0 {
            return 0;
        }

        let last_px = last.close_micros as i128;
        let diff = last_px - avg;
        let bps = (diff * 10_000) / avg;

        if bps >= EXTREME_BPS as i128 {
            -1
        } else if bps <= -(EXTREME_BPS as i128) {
            1
        } else {
            0
        }
    }
}

impl Strategy for MeanReversionStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(NAME, TIMEFRAME_SECS)
    }

    fn semantic_fingerprint(&self) -> String {
        SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, VERSION)
            .push_str(&self.symbol)
            .push_i64(TIMEFRAME_SECS)
            .push_i64(LOOKBACK as i64)
            .push_i64(EXTREME_BPS)
            .finish()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let qty = Self::signal_from_recent(&ctx.recent.bars);
        StrategyOutput {
            targets: vec![TargetPosition {
                symbol: self.symbol.clone(),
                qty,
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BarStub, RecentBarsWindow, StrategyContext};

    fn bar(close_micros: i64, is_complete: bool) -> BarStub {
        BarStub::new(0, is_complete, close_micros, 1)
    }

    fn ctx_with_bars(bars: Vec<BarStub>) -> StrategyContext {
        let len = bars.len().max(1);
        StrategyContext::new(TIMEFRAME_SECS, 0, RecentBarsWindow::new(len, bars))
    }

    /// MR-01: fewer than LOOKBACK bars → insufficient history → qty=0.
    #[test]
    fn mr01_insufficient_history_returns_zero() {
        let bars: Vec<BarStub> = (0..LOOKBACK - 1).map(|_| bar(100_000_000, true)).collect();
        assert_eq!(
            MeanReversionStrategy::signal_from_recent(&bars),
            0,
            "MR-01: len < LOOKBACK → 0"
        );
    }

    /// MR-02: exactly LOOKBACK bars, all equal → neutral (no deviation) → qty=0.
    #[test]
    fn mr02_exact_lookback_length_neutral() {
        let bars: Vec<BarStub> = (0..LOOKBACK).map(|_| bar(100_000_000, true)).collect();
        assert_eq!(
            MeanReversionStrategy::signal_from_recent(&bars),
            0,
            "MR-02: exact-length neutral window → 0"
        );
    }

    /// MR-03: last bar incomplete → qty=0 regardless of deviation.
    #[test]
    fn mr03_incomplete_last_bar_returns_zero() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK - 1).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(50_000_000, false));
        assert_eq!(
            MeanReversionStrategy::signal_from_recent(&bars),
            0,
            "MR-03: incomplete last bar → 0"
        );
    }

    /// MR-04: long/positive entry — price far below trailing average → qty=+1.
    #[test]
    fn mr04_price_far_below_average_is_long_entry() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK - 1).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(90_000_000, true)); // ~-954 bps vs avg, well past -120bps threshold
        assert_eq!(
            MeanReversionStrategy::signal_from_recent(&bars),
            1,
            "MR-04: deep discount → long entry"
        );
    }

    /// MR-05: short/negative entry — price far above trailing average → qty=-1.
    #[test]
    fn mr05_price_far_above_average_is_short_entry() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK - 1).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(110_000_000, true)); // ~+945 bps vs avg, well past +120bps threshold
        assert_eq!(
            MeanReversionStrategy::signal_from_recent(&bars),
            -1,
            "MR-05: deep premium → short entry"
        );
    }

    /// MR-06: boundary — bps exactly = +EXTREME_BPS (120) → short entry (`>=` is inclusive).
    #[test]
    fn mr06_positive_boundary_equality_triggers_short() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK - 2).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(98_800_000, true));
        bars.push(bar(101_200_000, true));
        // avg = 100_000_000 exactly; diff = 1_200_000; bps = 120 exactly.
        assert_eq!(
            MeanReversionStrategy::signal_from_recent(&bars),
            -1,
            "MR-06: bps==120 (inclusive boundary) → short"
        );
    }

    /// MR-07: just under the positive boundary — bps=119 → neutral, no signal.
    #[test]
    fn mr07_just_under_positive_boundary_is_neutral() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK - 2).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(98_810_000, true));
        bars.push(bar(101_190_000, true));
        // avg = 100_000_000 exactly; diff = 1_190_000; bps = 119 exactly (< 120).
        assert_eq!(
            MeanReversionStrategy::signal_from_recent(&bars),
            0,
            "MR-07: bps==119 stays below threshold → neutral"
        );
    }

    /// MR-08: boundary — bps exactly = -EXTREME_BPS (120) → long entry (`<=` is inclusive).
    #[test]
    fn mr08_negative_boundary_equality_triggers_long() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK - 2).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(101_200_000, true));
        bars.push(bar(98_800_000, true));
        // avg = 100_000_000 exactly; diff = -1_200_000; bps = -120 exactly.
        assert_eq!(
            MeanReversionStrategy::signal_from_recent(&bars),
            1,
            "MR-08: bps==-120 (inclusive boundary) → long"
        );
    }

    /// MR-09: only the trailing LOOKBACK bars participate; older bars outside the
    /// window must not perturb the average.
    #[test]
    fn mr09_lookback_window_excludes_older_bars() {
        let mut bars: Vec<BarStub> = (0..5).map(|_| bar(1, true)).collect(); // extreme outliers, outside window
        bars.extend((0..LOOKBACK).map(|_| bar(100_000_000, true))); // neutral window
        assert_eq!(bars.len(), 5 + LOOKBACK);
        assert_eq!(
            MeanReversionStrategy::signal_from_recent(&bars),
            0,
            "MR-09: outlier bars before the window are excluded"
        );
    }

    /// MR-10: malformed/zero-price input — avg<=0 fails closed to neutral, not panic.
    #[test]
    fn mr10_zero_price_window_fails_closed_to_zero() {
        let bars: Vec<BarStub> = (0..LOOKBACK).map(|_| bar(0, true)).collect();
        assert_eq!(
            MeanReversionStrategy::signal_from_recent(&bars),
            0,
            "MR-10: avg<=0 → fail-closed to 0"
        );
    }

    /// MR-11: malformed/negative-price input — avg<=0 fails closed to neutral.
    #[test]
    fn mr11_negative_price_window_fails_closed_to_zero() {
        let bars: Vec<BarStub> = (0..LOOKBACK).map(|_| bar(-1_000_000, true)).collect();
        assert_eq!(
            MeanReversionStrategy::signal_from_recent(&bars),
            0,
            "MR-11: negative avg → fail-closed to 0"
        );
    }

    /// MR-12: on_bar wraps the raw signal into a TargetPosition for the strategy's symbol.
    #[test]
    fn mr12_on_bar_emits_target_position_for_symbol() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK - 1).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(90_000_000, true));
        let ctx = ctx_with_bars(bars);
        let mut s = MeanReversionStrategy::new("AAPL");
        let out = s.on_bar(&ctx);
        assert_eq!(out.targets.len(), 1, "MR-12: exactly one target");
        assert_eq!(out.targets[0].symbol, "AAPL", "MR-12: target symbol matches");
        assert_eq!(out.targets[0].qty, 1, "MR-12: qty matches raw signal");
    }
}
