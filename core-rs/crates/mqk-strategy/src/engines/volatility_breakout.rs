use crate::{
    BarStub, Strategy, StrategyContext, StrategyMeta, StrategyOutput, StrategySpec, TargetPosition,
};

const NAME: &str = "volatility_breakout";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 3_600; // 1H
const LOOKBACK: usize = 20;

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic breakout engine using prior-window min/max closes.",
    )
}

#[derive(Clone, Debug)]
pub struct VolatilityBreakoutStrategy {
    symbol: String,
}

impl VolatilityBreakoutStrategy {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
        }
    }

    fn signal_from_recent(recent: &[BarStub]) -> i64 {
        if recent.len() < LOOKBACK + 1 {
            return 0;
        }

        let last = match recent.last() {
            Some(x) if x.is_complete => x,
            _ => return 0,
        };

        let prior = &recent[recent.len() - (LOOKBACK + 1)..recent.len() - 1];

        let mut min_close = i64::MAX;
        let mut max_close = i64::MIN;

        for bar in prior {
            min_close = min_close.min(bar.close_micros);
            max_close = max_close.max(bar.close_micros);
        }

        if last.close_micros > max_close {
            1
        } else if last.close_micros < min_close {
            -1
        } else {
            0
        }
    }
}

impl Strategy for VolatilityBreakoutStrategy {
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
