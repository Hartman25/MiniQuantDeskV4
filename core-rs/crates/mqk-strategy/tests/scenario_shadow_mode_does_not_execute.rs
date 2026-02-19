use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::*;

struct Dummy;
impl Strategy for Dummy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("dummy", 60)
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

#[test]
fn scenario_shadow_mode_does_not_execute() {
    let mut host = StrategyHost::new(ShadowMode::On);
    host.register(Box::new(Dummy)).unwrap();

    let recent = RecentBarsWindow::new(
        3,
        vec![
            BarStub::new(1000, true, 500_000_000, 1000),
            BarStub::new(1060, true, 501_000_000, 1200),
            BarStub::new(1120, true, 502_000_000, 900),
        ],
    );

    let ctx = StrategyContext::new(60, 1, recent);
    let r = host.on_bar(&ctx).unwrap();

    assert_eq!(r.intents.mode, IntentMode::Shadow);
    assert!(!r.intents.should_execute());

    // Strategy still produced outputs (for parity checks/logging).
    assert_eq!(r.intents.output.targets.len(), 1);
    assert_eq!(r.intents.output.targets[0].symbol, "SPY");
    assert_eq!(r.intents.output.targets[0].target_qty, 1);
}
