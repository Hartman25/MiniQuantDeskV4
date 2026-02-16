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
fn scenario_timeframe_mismatch_rejection() {
    let mut host = StrategyHost::new(ShadowMode::Off);
    host.register(Box::new(Dummy)).unwrap();

    let recent = RecentBarsWindow::new(5, vec![]);
    let ctx = StrategyContext::new(300, 1, recent); // mismatch: expected 60

    let err = host.on_bar(&ctx).unwrap_err();
    assert_eq!(
        err,
        StrategyHostError::TimeframeMismatch {
            expected_secs: 60,
            got_secs: 300
        }
    );
}
