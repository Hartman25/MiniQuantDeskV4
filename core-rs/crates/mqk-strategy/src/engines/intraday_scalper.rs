use crate::{
    BarStub, Strategy, StrategyContext, StrategyMeta, StrategyOutput, StrategySpec, TargetPosition,
};

const NAME: &str = "intraday_scalper";
const VERSION: &str = "0.1.0";
const TIMEFRAME_SECS: i64 = 300; // 5m
const LOOKBACK: usize = 5;
const MICRO_MOVE_BPS: i64 = 20; // 0.20%

pub fn meta() -> StrategyMeta {
    StrategyMeta::new(
        NAME,
        VERSION,
        TIMEFRAME_SECS,
        "Deterministic intraday scalp engine using short-horizon close displacement.",
    )
}

#[derive(Clone, Debug)]
pub struct IntradayScalperStrategy {
    symbol: String,
}

impl IntradayScalperStrategy {
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

        let first = &recent[recent.len() - LOOKBACK];
        if first.close_micros <= 0 {
            return 0;
        }

        let diff = last.close_micros as i128 - first.close_micros as i128;
        let bps = (diff * 10_000) / first.close_micros as i128;

        if bps >= MICRO_MOVE_BPS as i128 {
            1
        } else if bps <= -(MICRO_MOVE_BPS as i128) {
            -1
        } else {
            0
        }
    }
}

impl Strategy for IntradayScalperStrategy {
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
