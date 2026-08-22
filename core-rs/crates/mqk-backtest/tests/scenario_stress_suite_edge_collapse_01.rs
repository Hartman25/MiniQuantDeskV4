//! FINAL-P9-ROBUSTNESS-SEMANTICS-01 -- edge-collapse semantics for
//! `PROMOTION-STRESS-SUITE-AUTHORITY-01`'s `cost_stress_2x`/`cost_stress_3x`
//! scenarios: robustness cannot mean only "didn't go bankrupt". A candidate
//! that was genuinely profitable at baseline but whose economic edge
//! disappears entirely (net non-positive return) under 2x/3x transaction
//! costs must fail, even when it clears the conservative drawdown/
//! bankruptcy bar.

use mqk_backtest::{run_backtest_stress_suite, BacktestBar, BacktestConfig, BacktestEngine};
use mqk_strategy::{Strategy, StrategyContext, StrategyOutput, StrategySpec, TargetPosition};

const M: i64 = 1_000_000;

fn bar(end_ts: i64, price_micros: i64) -> BacktestBar {
    BacktestBar::new("ES", end_ts, price_micros, price_micros, price_micros, price_micros, 1_000)
}

/// Buys `qty` shares at bar 0, holds, sells (goes flat) at bar 1 -- a single
/// round trip capturing exactly the bar0->bar1 price move, minus costs.
struct OneRoundTrip {
    qty: i64,
    bar_idx: u64,
}

impl Strategy for OneRoundTrip {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("OneRoundTrip", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        let target = if self.bar_idx == 0 { self.qty } else { 0 };
        self.bar_idx += 1;
        StrategyOutput::new(vec![TargetPosition::new("ES", target)])
    }
}

/// A thin, genuine edge: $0.10/share on a 100-share round trip = $10.00
/// gross edge. `per_share_micros` commission is tuned so 1x costs ($8.00
/// round trip) leave a real $2.00 net profit, but 2x/3x costs ($16.00/
/// $24.00 round trip) exceed the edge entirely -- turning a genuinely
/// profitable baseline into a net loss under stress.
fn edge_config() -> BacktestConfig {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 100_000_000;
    cfg.commission.per_share_micros = 40_000; // $0.04/share/side
    cfg
}

fn edge_bars() -> Vec<BacktestBar> {
    // Causal (next-bar) execution: the buy SIGNAL at bar0 fills at bar1's
    // price; the flatten SIGNAL at bar1 fills at bar2's price. The genuine
    // $0.10/share edge is therefore the bar1->bar2 move, not bar0->bar1.
    vec![
        bar(1_700_000_000, 100 * M),                  // signal bar (price irrelevant to the fill)
        bar(1_700_000_060, 100 * M),                   // entry fill price $100.00
        bar(1_700_000_120, 100 * M + 100_000),         // exit fill price $100.10
        bar(1_700_000_180, 100 * M + 100_000),         // fold-end bar, unchanged
    ]
}

fn make_strategy() -> Box<dyn Strategy> {
    Box::new(OneRoundTrip { qty: 100, bar_idx: 0 })
}

fn run_baseline(config: &BacktestConfig, bars: &[BacktestBar]) -> mqk_backtest::BacktestReport {
    let mut engine = BacktestEngine::new(config.clone());
    engine.add_strategy(make_strategy()).unwrap();
    engine.run(bars).expect("engine.run must succeed")
}

#[test]
fn baseline_is_genuinely_profitable_fixture_precondition() {
    let config = edge_config();
    let bars = edge_bars();
    let report = run_baseline(&config, &bars);
    let final_equity = report.equity_curve.last().unwrap().1;
    assert!(
        final_equity > config.initial_cash_micros,
        "fixture precondition: baseline must be genuinely profitable at 1x costs \
         (final_equity_micros={final_equity}, initial_cash_micros={})",
        config.initial_cash_micros
    );
}

#[test]
fn cost_stress_2x_collapses_the_edge_and_fails() {
    let config = edge_config();
    let bars = edge_bars();
    let report = run_baseline(&config, &bars);

    let output = run_backtest_stress_suite(&report, &config, &bars, make_strategy);
    let cost_2x = output.scenarios.iter().find(|s| s.name == "cost_stress_2x").unwrap();
    assert!(
        !cost_2x.passed,
        "2x transaction costs must exceed the thin real edge and fail: {cost_2x:?}"
    );
    assert!(
        cost_2x.reason.as_deref().unwrap_or_default().contains("economic edge collapsed"),
        "must fail via the edge-collapse reason specifically, not bankruptcy/drawdown: {cost_2x:?}"
    );
    // Never bankrupt, never breaches conservative drawdown -- proves this is
    // a genuinely NEW failure mode, not a re-labeled existing one.
    assert!(cost_2x.final_equity_micros > 0);
    assert!(cost_2x.max_drawdown_fraction < 0.10);
}

#[test]
fn cost_stress_3x_also_collapses_the_edge_and_fails() {
    let config = edge_config();
    let bars = edge_bars();
    let report = run_baseline(&config, &bars);

    let output = run_backtest_stress_suite(&report, &config, &bars, make_strategy);
    let cost_3x = output.scenarios.iter().find(|s| s.name == "cost_stress_3x").unwrap();
    assert!(!cost_3x.passed, "3x transaction costs must a fortiori fail: {cost_3x:?}");
    assert!(cost_3x.reason.as_deref().unwrap_or_default().contains("economic edge collapsed"));
}

/// Positive control: a candidate with a genuinely large edge relative to
/// costs clears 2x/3x cost stress without triggering the new check.
#[test]
fn large_edge_survives_cost_stress() {
    let mut config = edge_config();
    config.commission.per_share_micros = 1_000; // trivial commission vs. a $0.10/share edge
    let bars = edge_bars();
    let report = run_baseline(&config, &bars);

    let output = run_backtest_stress_suite(&report, &config, &bars, make_strategy);
    for name in ["cost_stress_2x", "cost_stress_3x"] {
        let outcome = output.scenarios.iter().find(|s| s.name == name).unwrap();
        assert!(outcome.passed, "a genuinely large edge must survive {name}: {outcome:?}");
    }
}
