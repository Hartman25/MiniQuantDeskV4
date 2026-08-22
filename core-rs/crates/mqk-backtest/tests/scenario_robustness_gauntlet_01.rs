//! P9 `BKT-ROBUSTNESS-GAUNTLET-01` — real, deterministic robustness
//! evidence.
//!
//! Validates:
//! - A healthy, genuinely robust candidate clears every applicable
//!   scenario.
//! - A candidate whose entire result depends on ONE symbol (the other
//!   symbol alone breaches the conservative drawdown ceiling) fails
//!   symbol_leave_one_out for real -- not a mocked assertion.
//! - A single-symbol candidate reports symbol_leave_one_out as
//!   `applicable: false`, never a fabricated pass or fail.
//! - A candidate whose entire profit comes from one calendar month fails
//!   month_and_regime_concentration.
//! - The placebo (temporal-offset) scenario genuinely distinguishes a real
//!   directional signal from its own temporally-decorrelated version.
//! - Two identical inputs produce byte-identical gauntlet output
//!   (determinism -- no RNG anywhere).
//! - `conservative_capacity_stress` is a real, present scenario that a
//!   healthy candidate clears.
//! - `is_complete()` is `false` until a separately-computed
//!   `dsr_pbo_sensitivity` outcome is merged in via
//!   `merge_dsr_pbo_sensitivity`, then `true`.

use mqk_backtest::{
    run_robustness_gauntlet, BacktestBar, BacktestConfig, BacktestEngine, BacktestReport,
};
use mqk_strategy::{Strategy, StrategyContext, StrategyOutput, StrategySpec, TargetPosition};

const M: i64 = 1_000_000;

fn flat_bar(symbol: &str, end_ts: i64, price_usd: i64) -> BacktestBar {
    let p = price_usd * M;
    BacktestBar::new(symbol, end_ts, p, p, p, p, 1_000)
}

fn cfg_with_wide_cap() -> BacktestConfig {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 100_000_000;
    cfg
}

fn run(bars: &[BacktestBar], strategy: Box<dyn Strategy>) -> (BacktestReport, BacktestConfig) {
    let config = cfg_with_wide_cap();
    let mut engine = BacktestEngine::new(config.clone());
    engine.add_strategy(strategy).unwrap();
    let report = engine.run(bars).expect("engine.run must succeed");
    (report, config)
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Buys `qty` of `symbol` on bar 1, holds, sells starting bar `sell_at_idx`.
struct SingleSymbolBuyHoldSell {
    symbol: &'static str,
    bar_idx: u64,
    qty: i64,
    sell_at_idx: u64,
}

impl Strategy for SingleSymbolBuyHoldSell {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("Rg01SingleSymbol", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        let target = if self.bar_idx < self.sell_at_idx { self.qty } else { 0 };
        StrategyOutput::new(vec![TargetPosition::new(self.symbol, target)])
    }
}

/// Buys `qty` of BOTH symbols on bar 1, holds, sells both starting bar
/// `sell_at_idx`.
struct TwoSymbolBuyHoldSell {
    bar_idx: u64,
    qty: i64,
    sell_at_idx: u64,
}

impl Strategy for TwoSymbolBuyHoldSell {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("Rg01TwoSymbol", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        let target = if self.bar_idx < self.sell_at_idx { self.qty } else { 0 };
        StrategyOutput::new(vec![
            TargetPosition::new("ES", target),
            TargetPosition::new("SPY", target),
        ])
    }
}

/// Interleave two single-symbol bar sequences on shared timestamps.
fn interleave(a: Vec<BacktestBar>, b: Vec<BacktestBar>) -> Vec<BacktestBar> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    for (x, y) in a.into_iter().zip(b) {
        out.push(x);
        out.push(y);
    }
    out
}

fn healthy_single_symbol_bars() -> Vec<BacktestBar> {
    vec![
        flat_bar("ES", 1_700_000_060, 500),
        flat_bar("ES", 1_700_000_120, 501),
        flat_bar("ES", 1_700_000_180, 502),
        flat_bar("ES", 1_700_000_240, 503),
    ]
}

/// ES gains modestly; SPY collapses. Net (both symbols) drawdown clears
/// the conservative bar, but SPY ALONE (with ES excluded) breaches it.
fn dependent_on_one_symbol_bars() -> Vec<BacktestBar> {
    let es = vec![
        flat_bar("ES", 1_700_000_060, 500),
        flat_bar("ES", 1_700_000_120, 500),
        flat_bar("ES", 1_700_000_180, 520),
        flat_bar("ES", 1_700_000_240, 520),
    ];
    let spy = vec![
        flat_bar("SPY", 1_700_000_060, 500),
        flat_bar("SPY", 1_700_000_120, 500),
        flat_bar("SPY", 1_700_000_180, 150),
        flat_bar("SPY", 1_700_000_240, 150),
    ];
    interleave(es, spy)
}

/// A real, sustained directional uptrend across many bars -- gives the
/// placebo scenario an actual, non-trivial edge to distinguish.
fn trending_bars(n: usize) -> Vec<BacktestBar> {
    let mut bars = Vec::with_capacity(n);
    let mut price = 500i64;
    for i in 0..n {
        bars.push(flat_bar("ES", 1_700_000_000 + (i as i64) * 60, price));
        price += 2; // steady uptrend
    }
    bars
}

/// Always-long strategy: targets `qty` from the first bar onward and never
/// sells -- rides the whole trend.
struct AlwaysLong {
    qty: i64,
}

impl Strategy for AlwaysLong {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("Rg01AlwaysLong", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput::new(vec![TargetPosition::new("ES", self.qty)])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn rg01a_healthy_single_symbol_candidate_reports_leave_one_out_not_applicable() {
    let bars = healthy_single_symbol_bars();
    let (report, config) = run(
        &bars,
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 1, sell_at_idx: 3 }),
    );

    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 1, sell_at_idx: 3 })
    });

    let leave_one_out = output
        .scenarios
        .iter()
        .find(|s| s.name == "symbol_leave_one_out")
        .unwrap();
    assert!(!leave_one_out.applicable, "single-symbol run must report not-applicable");

    assert_eq!(
        output.deferred.len(),
        1,
        "DSR/PBO sensitivity is honestly deferred by this pure, engine-only function -- it \
         requires separate subprocess/filesystem composition, see \
         dsr_pbo_sensitivity_scenario/merge_dsr_pbo_sensitivity"
    );
    assert!(output.deferred.iter().any(|d| d.name == "dsr_pbo_sensitivity"));
    assert!(
        !output.is_complete(),
        "must not be complete until dsr_pbo_sensitivity is merged in"
    );
}

#[test]
fn rg01b_result_dependent_on_one_symbol_fails_leave_one_out() {
    let bars = dependent_on_one_symbol_bars();
    let (report, config) = run(&bars, Box::new(TwoSymbolBuyHoldSell { bar_idx: 0, qty: 100, sell_at_idx: 3 }));

    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(TwoSymbolBuyHoldSell { bar_idx: 0, qty: 100, sell_at_idx: 3 })
    });

    let leave_one_out = output
        .scenarios
        .iter()
        .find(|s| s.name == "symbol_leave_one_out")
        .unwrap();
    assert!(leave_one_out.applicable);
    assert!(
        !leave_one_out.passed,
        "excluding the profitable ES symbol must expose SPY's real loss and fail: {leave_one_out:?}"
    );
    assert!(!output.all_applicable_passed());
}

#[test]
fn rg01c_placebo_distinguishes_real_trend_from_temporal_offset() {
    let bars = trending_bars(20);
    let (report, config) = run(&bars, Box::new(AlwaysLong { qty: 10 }));

    let output = run_robustness_gauntlet(&report, &config, &bars, || Box::new(AlwaysLong { qty: 10 }));

    let placebo = output
        .scenarios
        .iter()
        .find(|s| s.name == "placebo_temporal_offset")
        .unwrap();
    assert!(
        placebo.passed,
        "a real, sustained directional edge must beat its own temporally-offset placebo: {placebo:?}"
    );
}

#[test]
fn rg01d_deterministic_across_identical_inputs() {
    let bars = healthy_single_symbol_bars();
    let (report, config) = run(
        &bars,
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 1, sell_at_idx: 3 }),
    );

    let make = || -> Box<dyn Strategy> {
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 1, sell_at_idx: 3 })
    };
    let output1 = run_robustness_gauntlet(&report, &config, &bars, make);
    let output2 = run_robustness_gauntlet(&report, &config, &bars, make);
    assert_eq!(output1, output2);
}

#[test]
fn rg01f_profit_concentrated_in_one_month_fails_concentration_scenario() {
    use chrono::NaiveDate;
    fn ts(y: i32, m: u32, d: u32) -> i64 {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp()
    }

    // Month 1: flat (no gain). Month 2: all the profit. Two distinct
    // months, entirely concentrated in one -> must fail.
    let bars = vec![
        flat_bar("ES", ts(2024, 1, 10), 500),
        flat_bar("ES", ts(2024, 1, 20), 500),
        flat_bar("ES", ts(2024, 3, 10), 500),
        flat_bar("ES", ts(2024, 3, 20), 600),
    ];
    let (report, config) = run(
        &bars,
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 10, sell_at_idx: 3 }),
    );

    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 10, sell_at_idx: 3 })
    });

    let concentration = output
        .scenarios
        .iter()
        .find(|s| s.name == "month_and_regime_concentration")
        .unwrap();
    assert!(concentration.applicable, "must span 2+ months: {concentration:?}");
    assert!(
        !concentration.passed,
        "all profit concentrated in one of two months must fail: {concentration:?}"
    );
}

#[test]
fn rg01e_healthy_candidate_clears_every_applicable_scenario() {
    let bars = healthy_single_symbol_bars();
    let (report, config) = run(
        &bars,
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 1, sell_at_idx: 3 }),
    );

    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 1, sell_at_idx: 3 })
    });

    for s in &output.scenarios {
        if s.applicable {
            assert!(s.passed, "expected {} to pass: {s:?}", s.name);
        }
    }
    assert!(output.all_applicable_passed());
}

#[test]
fn rg01g_conservative_capacity_stress_present_and_passes_for_healthy_candidate() {
    let bars = healthy_single_symbol_bars();
    let (report, config) = run(
        &bars,
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 1, sell_at_idx: 3 }),
    );

    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 1, sell_at_idx: 3 })
    });

    let capacity = output
        .scenarios
        .iter()
        .find(|s| s.name == "conservative_capacity_stress")
        .expect("conservative_capacity_stress must be a real, present scenario");
    assert!(capacity.applicable);
    assert!(capacity.passed, "healthy candidate must clear reduced-capacity conservative bar: {capacity:?}");
}

#[test]
fn rg01h_is_complete_only_after_dsr_pbo_sensitivity_is_merged() {
    use mqk_backtest::RobustnessScenarioOutcome;

    let bars = healthy_single_symbol_bars();
    let (report, config) = run(
        &bars,
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 1, sell_at_idx: 3 }),
    );

    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 1, sell_at_idx: 3 })
    });
    assert!(!output.is_complete(), "must be incomplete before DSR/PBO sensitivity is merged");

    let merged = output.merge_dsr_pbo_sensitivity(RobustnessScenarioOutcome {
        name: "dsr_pbo_sensitivity".to_string(),
        applicable: true,
        passed: true,
        reason: None,
        detail: "test-fabricated evaluated outcome".to_string(),
        research_trial_id: Some("rg01h_test_trial".to_string()),
    });
    assert!(merged.deferred.is_empty(), "merging must clear the deferred entry");
    assert!(merged.is_complete(), "must be complete once every required scenario is present");
}
