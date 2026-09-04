use crate::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use crate::{
    BarStub, Strategy, StrategyContext, StrategyDataRequirements, StrategyMeta, StrategyOutput,
    StrategySpec, TargetPosition,
};

pub(crate) const NAME: &str = "swing_momentum";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 86_400; // 1D
const LOOKBACK: usize = 20;
const ENTRY_BPS: i64 = 150; // 1.50%

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic daily swing momentum engine using last-close vs trailing average.",
    )
    .with_data_requirements(StrategyDataRequirements {
        minimum_completed_bars: LOOKBACK,
    })
}

#[derive(Clone, Debug)]
pub struct SwingMomentumStrategy {
    symbol: String,
}

impl SwingMomentumStrategy {
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

        if bps >= ENTRY_BPS as i128 {
            1
        } else if bps <= -(ENTRY_BPS as i128) {
            -1
        } else {
            0
        }
    }
}

impl Strategy for SwingMomentumStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(NAME, TIMEFRAME_SECS)
    }

    fn semantic_fingerprint(&self) -> String {
        SemanticIdentityBuilder::new(SEMANTIC_IDENTITY_SCHEMA_V1, NAME, VERSION)
            .push_str(&self.symbol)
            .push_i64(TIMEFRAME_SECS)
            .push_i64(LOOKBACK as i64)
            .push_i64(ENTRY_BPS)
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

    /// SM-01: fewer than LOOKBACK bars → insufficient lookback → qty=0.
    #[test]
    fn sm01_insufficient_lookback_returns_zero() {
        let bars: Vec<BarStub> = (0..LOOKBACK - 1).map(|_| bar(100_000_000, true)).collect();
        assert_eq!(
            SwingMomentumStrategy::signal_from_recent(&bars),
            0,
            "SM-01: len < LOOKBACK → 0"
        );
    }

    /// SM-02: exactly LOOKBACK bars, all equal → neutral (no deviation) → qty=0.
    #[test]
    fn sm02_exact_lookback_length_neutral() {
        let bars: Vec<BarStub> = (0..LOOKBACK).map(|_| bar(100_000_000, true)).collect();
        assert_eq!(
            SwingMomentumStrategy::signal_from_recent(&bars),
            0,
            "SM-02: exact-length neutral window → 0"
        );
    }

    /// SM-03: last bar incomplete → qty=0 regardless of deviation.
    #[test]
    fn sm03_incomplete_last_bar_returns_zero() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK - 1).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(150_000_000, false));
        assert_eq!(
            SwingMomentumStrategy::signal_from_recent(&bars),
            0,
            "SM-03: incomplete last bar → 0"
        );
    }

    /// SM-04: close well above the trailing average → qty=+1.
    #[test]
    fn sm04_close_above_average_is_positive() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK - 1).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(110_000_000, true)); // ~+945 bps vs avg, well past +150bps threshold
        assert_eq!(
            SwingMomentumStrategy::signal_from_recent(&bars),
            1,
            "SM-04: close far above average → +1"
        );
    }

    /// SM-05: close well below the trailing average → qty=-1.
    #[test]
    fn sm05_close_below_average_is_negative() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK - 1).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(90_000_000, true)); // ~-954 bps vs avg, well past -150bps threshold
        assert_eq!(
            SwingMomentumStrategy::signal_from_recent(&bars),
            -1,
            "SM-05: close far below average → -1"
        );
    }

    /// SM-06: boundary — bps exactly = +ENTRY_BPS (150) → +1 (`>=` is inclusive).
    #[test]
    fn sm06_positive_boundary_equality_triggers_long() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK - 2).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(98_500_000, true));
        bars.push(bar(101_500_000, true));
        // avg = 100_000_000 exactly; diff = 1_500_000; bps = 150 exactly.
        assert_eq!(
            SwingMomentumStrategy::signal_from_recent(&bars),
            1,
            "SM-06: bps==150 (inclusive boundary) → +1"
        );
    }

    /// SM-07: just under the positive boundary — bps=149 → neutral, no signal.
    #[test]
    fn sm07_just_under_positive_boundary_is_neutral() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK - 2).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(98_510_000, true));
        bars.push(bar(101_490_000, true));
        // avg = 100_000_000 exactly; diff = 1_490_000; bps = 149 exactly (< 150).
        assert_eq!(
            SwingMomentumStrategy::signal_from_recent(&bars),
            0,
            "SM-07: bps==149 stays below threshold → neutral"
        );
    }

    /// SM-08: boundary — bps exactly = -ENTRY_BPS (150) → -1 (`<=` is inclusive).
    #[test]
    fn sm08_negative_boundary_equality_triggers_short() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK - 2).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(101_500_000, true));
        bars.push(bar(98_500_000, true));
        // avg = 100_000_000 exactly; diff = -1_500_000; bps = -150 exactly.
        assert_eq!(
            SwingMomentumStrategy::signal_from_recent(&bars),
            -1,
            "SM-08: bps==-150 (inclusive boundary) → -1"
        );
    }

    /// SM-09: only the trailing LOOKBACK bars participate — neither older bars nor
    /// future data can leak into the average; outliers before the window must not
    /// perturb the result.
    #[test]
    fn sm09_lookback_window_excludes_older_bars() {
        let mut bars: Vec<BarStub> = (0..5).map(|_| bar(1, true)).collect(); // extreme outliers, outside window
        bars.extend((0..LOOKBACK).map(|_| bar(100_000_000, true))); // neutral window
        assert_eq!(bars.len(), 5 + LOOKBACK);
        assert_eq!(
            SwingMomentumStrategy::signal_from_recent(&bars),
            0,
            "SM-09: outlier bars before the window are excluded"
        );
    }

    /// SM-10: malformed/zero-price input — avg<=0 fails closed to neutral, not panic.
    #[test]
    fn sm10_zero_price_window_fails_closed_to_zero() {
        let bars: Vec<BarStub> = (0..LOOKBACK).map(|_| bar(0, true)).collect();
        assert_eq!(
            SwingMomentumStrategy::signal_from_recent(&bars),
            0,
            "SM-10: avg<=0 → fail-closed to 0"
        );
    }

    /// SM-11: malformed/negative-price input — avg<=0 fails closed to neutral.
    #[test]
    fn sm11_negative_price_window_fails_closed_to_zero() {
        let bars: Vec<BarStub> = (0..LOOKBACK).map(|_| bar(-1_000_000, true)).collect();
        assert_eq!(
            SwingMomentumStrategy::signal_from_recent(&bars),
            0,
            "SM-11: negative avg → fail-closed to 0"
        );
    }

    /// SM-12: on_bar wraps the raw signal into a TargetPosition for the strategy's
    /// symbol, preserving the fixed {-1,0,1} sizing semantics (no variable sizing).
    #[test]
    fn sm12_on_bar_emits_target_position_for_symbol() {
        let mut bars: Vec<BarStub> = (0..LOOKBACK - 1).map(|_| bar(100_000_000, true)).collect();
        bars.push(bar(110_000_000, true));
        let ctx = ctx_with_bars(bars);
        let mut s = SwingMomentumStrategy::new("AAPL");
        let out = s.on_bar(&ctx);
        assert_eq!(out.targets.len(), 1, "SM-12: exactly one target");
        assert_eq!(out.targets[0].symbol, "AAPL", "SM-12: target symbol matches");
        assert_eq!(out.targets[0].qty, 1, "SM-12: qty matches raw signal");
    }
}
