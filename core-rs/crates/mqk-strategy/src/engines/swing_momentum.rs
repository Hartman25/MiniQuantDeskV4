use crate::{
    BarStub, Strategy, StrategyContext, StrategyMeta, StrategyOutput, StrategySpec, TargetPosition,
};

const NAME: &str = "swing_momentum";
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
