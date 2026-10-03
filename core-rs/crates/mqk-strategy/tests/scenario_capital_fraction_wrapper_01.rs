//! Reference-bar selection of the capital-fraction wrapper, exercised directly
//! with hand-built contexts (the engine only ever feeds completed bars).

use mqk_execution::{QtyMicros, StrategyOutput, TargetPosition};
use mqk_strategy::{
    BarStub, CapitalFractionRefusal, CapitalFractionSizedStrategy, RecentBarsWindow, SizingPolicy,
    Strategy, StrategyContext, StrategySpec, TargetSizing,
};

const USD: i64 = 1_000_000;
const CAPITAL: i64 = 100_000 * USD;

struct AlwaysLong;

impl Strategy for AlwaysLong {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("always_long", 86_400)
    }
    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput::new(vec![TargetPosition::new(
            "SPY",
            QtyMicros::from_whole_units(1).unwrap(),
        )])
    }
}

fn wrapper() -> (
    CapitalFractionSizedStrategy,
    mqk_strategy::SizingAuditHandle,
) {
    CapitalFractionSizedStrategy::new(
        Box::new(AlwaysLong),
        SizingPolicy::capital_fraction_v1(2_500).unwrap(),
        CAPITAL,
        TargetSizing::equity_default(),
    )
    .unwrap()
}

fn ctx(bars: Vec<BarStub>) -> StrategyContext {
    StrategyContext::new(86_400, 0, RecentBarsWindow::new(10, bars))
}

#[test]
fn reference_is_the_most_recent_completed_bar_close() {
    let (mut w, audit) = wrapper();
    let out = w.on_bar(&ctx(vec![
        BarStub::new(1, true, 50 * USD, 1),
        BarStub::new(2, true, 100 * USD, 1),
        BarStub::new(3, true, 125 * USD, 1),
    ]));
    // budget 25_000 / 125 = 200 shares (not 500 from the oldest bar, not 250 from the middle one)
    assert_eq!(out.targets[0].qty, QtyMicros::from_whole_units(200).unwrap());
    let a = audit.snapshot();
    assert_eq!(a.entries[0].reference_bar_end_ts, 3);
    assert!(a.refusals.is_empty());
}

#[test]
fn an_incomplete_latest_bar_is_never_the_reference_and_refuses_with_zero_target() {
    let (mut w, audit) = wrapper();
    let out = w.on_bar(&ctx(vec![
        BarStub::new(1, true, 100 * USD, 1),
        BarStub::new(2, false, 125 * USD, 1),
    ]));
    assert_eq!(out.targets[0].qty, QtyMicros::ZERO, "no one-share fallback");
    let a = audit.snapshot();
    assert!(a.entries.is_empty());
    assert_eq!(a.refusals.len(), 1);
    assert_eq!(
        a.refusals[0].refusal,
        CapitalFractionRefusal::NoCompletedReferenceBar
    );
}

#[test]
fn an_empty_window_refuses_with_zero_target() {
    let (mut w, audit) = wrapper();
    let out = w.on_bar(&ctx(vec![]));
    assert_eq!(out.targets[0].qty, QtyMicros::ZERO);
    assert_eq!(audit.snapshot().refusals.len(), 1);
}
