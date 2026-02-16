use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::*;

struct DummyA;
impl Strategy for DummyA {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("dummyA", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput {
            targets: vec![TargetPosition {
                symbol: "SPY".to_string(),
                target_qty: 1,
            }],
        }
    }
}

struct DummyB;
impl Strategy for DummyB {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("dummyB", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput {
            targets: vec![TargetPosition {
                symbol: "SPY".to_string(),
                target_qty: 2,
            }],
        }
    }
}

#[test]
fn scenario_multi_strategy_rejection() {
    let mut host = StrategyHost::new(ShadowMode::Off);
    host.register(Box::new(DummyA)).unwrap();

    let err = host.register(Box::new(DummyB)).unwrap_err();
    assert_eq!(err, StrategyHostError::MultiStrategyNotAllowed);
}
