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
//! - A candidate whose entire profit comes from one calendar month, one
//!   year, or one regime bucket fails month_year_regime_concentration.
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
        3,
        "DSR/PBO sensitivity, the P7A/P7B economic replay stress, and the genuine shuffled \
         placebo are honestly deferred by this pure, engine-only function -- each requires \
         separate subprocess/filesystem composition, see \
         dsr_pbo_sensitivity_scenario/p7a_p7b_economic_replay_stress_scenario/\
         genuine_shuffled_placebo_scenario and merge_dsr_pbo_sensitivity"
    );
    assert!(output.deferred.iter().any(|d| d.name == "dsr_pbo_sensitivity"));
    assert!(output.deferred.iter().any(|d| d.name == "p7a_p7b_economic_replay_stress"));
    assert!(output.deferred.iter().any(|d| d.name == "genuine_shuffled_placebo"));
    assert!(
        !output.is_complete(),
        "must not be complete until every deferred scenario is merged in"
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
        .find(|s| s.name == "month_year_regime_concentration")
        .unwrap();
    assert!(concentration.applicable, "must span 2+ months: {concentration:?}");
    assert!(
        !concentration.passed,
        "all profit concentrated in one of two months must fail: {concentration:?}"
    );
    assert!(concentration.reason.as_deref().unwrap_or_default().contains("month:"));
}

/// FINAL-P9-ROBUSTNESS-SEMANTICS-01: profit concentrated in one YEAR across
/// multiple distinct months (of that same year) must fail the YEAR
/// dimension, even though MONTH concentration alone would not catch a
/// cross-month-but-single-year pattern.
#[test]
fn rg01l_profit_concentrated_in_one_year_across_multiple_months_fails() {
    use chrono::NaiveDate;
    fn ts(y: i32, m: u32, d: u32) -> i64 {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp()
    }

    // Year 2024: two distinct months, BOTH profitable (so month concentration
    // alone is not egregious). Year 2025: flat, no gain. All real profit is
    // concentrated in year 2024 across 4 distinct calendar months total.
    let bars = vec![
        flat_bar("ES", ts(2024, 1, 10), 500),
        flat_bar("ES", ts(2024, 1, 20), 520),
        flat_bar("ES", ts(2024, 3, 10), 520),
        flat_bar("ES", ts(2024, 3, 20), 540),
        flat_bar("ES", ts(2025, 1, 10), 540),
        flat_bar("ES", ts(2025, 1, 20), 540),
        flat_bar("ES", ts(2025, 3, 10), 540),
        flat_bar("ES", ts(2025, 3, 20), 540),
    ];
    let (report, config) = run(
        &bars,
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 10, sell_at_idx: 100 }),
    );

    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 10, sell_at_idx: 100 })
    });

    let concentration = output
        .scenarios
        .iter()
        .find(|s| s.name == "month_year_regime_concentration")
        .unwrap();
    assert!(concentration.applicable);
    assert!(
        !concentration.passed,
        "all profit concentrated in one of two years must fail even though it spans 4 months: \
         {concentration:?}"
    );
    assert!(concentration.reason.as_deref().unwrap_or_default().contains("year:"));
}

/// FINAL-P9-ROBUSTNESS-SEMANTICS-01: an execution delay that turns a
/// genuinely profitable round trip into a loss must fail
/// `execution_delay_stress`, even though it clears bankruptcy/drawdown.
#[test]
fn rg01n_execution_delay_destroys_profitability_fails() {
    // Baseline round trip captures bar1->bar2 (+$0.50/share, profitable).
    // The extra 1-bar delay this scenario adds shifts the SAME round trip
    // to bar2->bar3 (-$1.50/share, a real loss) -- proven empirically below
    // (bar_idx-to-fill-bar timing is an engine implementation detail this
    // test does not assume, only observes).
    let bars = vec![
        flat_bar("ES", 1_700_000_000, 100),
        flat_bar("ES", 1_700_000_060, 100),
        flat_bar("ES", 1_700_000_120, 100), // priced below via micros override
        flat_bar("ES", 1_700_000_180, 99),
        flat_bar("ES", 1_700_000_240, 99),
    ];
    // Override bar2's price to $100.50 (flat_bar only takes whole USD).
    let mut bars = bars;
    bars[2] = BacktestBar::new("ES", 1_700_000_120, 100_500_000, 100_500_000, 100_500_000, 100_500_000, 1_000);

    let make = || -> Box<dyn Strategy> {
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 100, sell_at_idx: 2 })
    };
    let (report, config) = run(&bars, make());
    let baseline_final = report.equity_curve.last().unwrap().1;
    assert!(
        baseline_final > config.initial_cash_micros,
        "fixture precondition: baseline must be genuinely profitable (final_equity_micros={baseline_final})"
    );

    let output = run_robustness_gauntlet(&report, &config, &bars, make);
    let delay = output.scenarios.iter().find(|s| s.name == "execution_delay_stress").unwrap();
    assert!(
        !delay.passed,
        "an execution delay that turns a real profit into a real loss must fail: {delay:?}"
    );
    assert!(
        delay.reason.as_deref().unwrap_or_default().contains("economic edge collapsed"),
        "must fail via the edge-collapse reason, not bankruptcy/drawdown: {delay:?}"
    );
}

/// FINAL-P9-ROBUSTNESS-SEMANTICS-01: excluding the symbol that supplies ALL
/// of the candidate's profit must fail `symbol_leave_one_out` even when the
/// remaining (other-symbol-only) result is merely FLAT, not negative --
/// zero net profitability is sufficient to fail, per the mission's explicit
/// instruction. Distinct from `rg01b` (which fails via a drawdown breach,
/// not an edge-collapse-to-flat).
#[test]
fn rg01o_leave_one_out_removes_positive_result_fails_even_when_flat() {
    // ES: genuine, sustained profit. SPY: perfectly flat (zero net change)
    // -- excluding ES leaves only SPY's flat result.
    let es = vec![
        flat_bar("ES", 1_700_000_060, 500),
        flat_bar("ES", 1_700_000_120, 500),
        flat_bar("ES", 1_700_000_180, 520),
        flat_bar("ES", 1_700_000_240, 520),
    ];
    let spy = vec![
        flat_bar("SPY", 1_700_000_060, 500),
        flat_bar("SPY", 1_700_000_120, 500),
        flat_bar("SPY", 1_700_000_180, 500),
        flat_bar("SPY", 1_700_000_240, 500),
    ];
    let bars = interleave(es, spy);
    let make = || -> Box<dyn Strategy> { Box::new(TwoSymbolBuyHoldSell { bar_idx: 0, qty: 10, sell_at_idx: 100 }) };
    let (report, config) = run(&bars, make());
    let baseline_final = report.equity_curve.last().unwrap().1;
    assert!(
        baseline_final > config.initial_cash_micros,
        "fixture precondition: baseline (ES+SPY) must be genuinely profitable"
    );

    let output = run_robustness_gauntlet(&report, &config, &bars, make);
    let leave_one_out = output.scenarios.iter().find(|s| s.name == "symbol_leave_one_out").unwrap();
    assert!(
        !leave_one_out.passed,
        "excluding ES (all of the real profit) must fail even though SPY alone is merely flat, \
         never negative: {leave_one_out:?}"
    );
    assert!(
        leave_one_out.reason.as_deref().unwrap_or_default().contains("economic edge collapsed"),
        "must fail via the edge-collapse reason, not a drawdown breach: {leave_one_out:?}"
    );
}

/// FINAL-P9-ROBUSTNESS-SEMANTICS-01: a neighboring parameter point (higher
/// slippage) that turns a thin, genuine edge into a loss must fail
/// `parameter_neighborhood_execution`, even though it clears drawdown.
#[test]
fn rg01p_parameter_neighborhood_point_becomes_non_profitable_fails() {
    // Thin genuine edge: $0.15/share gross on a 100-share round trip ($15
    // total) -- profitable at zero slippage (the baseline/first grid
    // point), but the neighboring +10bps slippage point's round-trip
    // adverse-price cost (~$20 on $100 shares) exceeds it.
    let bars = vec![
        flat_bar("ES", 1_700_000_000, 100),
        flat_bar("ES", 1_700_000_060, 100),
        BacktestBar::new(
            "ES", 1_700_000_120, 100_150_000, 100_150_000, 100_150_000, 100_150_000, 1_000,
        ),
        flat_bar("ES", 1_700_000_180, 100),
    ];
    let make = || -> Box<dyn Strategy> {
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 100, sell_at_idx: 2 })
    };
    let (report, config) = run(&bars, make());
    let baseline_final = report.equity_curve.last().unwrap().1;
    assert!(
        baseline_final > config.initial_cash_micros,
        "fixture precondition: baseline must be genuinely (thinly) profitable at zero slippage"
    );

    let output = run_robustness_gauntlet(&report, &config, &bars, make);
    let neighborhood =
        output.scenarios.iter().find(|s| s.name == "parameter_neighborhood_execution").unwrap();
    assert!(
        !neighborhood.passed,
        "a neighboring slippage point that erodes a thin edge into a loss must fail: \
         {neighborhood:?}"
    );
    assert!(
        neighborhood.reason.as_deref().unwrap_or_default().contains("economic edge collapsed"),
        "must fail via the edge-collapse reason, not a drawdown breach: {neighborhood:?}"
    );
}

/// FINAL-P9-ROBUSTNESS-SEMANTICS-01: profit concentrated in one REGIME
/// bucket while multiple regimes exist must fail the REGIME dimension, even
/// when MONTH concentration alone would not catch it (the profitable months
/// are evenly split, each at exactly the 0.5 ceiling boundary, so MONTH
/// passes) -- proving regime concentration catches a failure mode month/year
/// alone cannot.
#[test]
fn rg01m_profit_concentrated_in_one_regime_while_multiple_regimes_exist_fails() {
    use chrono::NaiveDate;
    fn ts(y: i32, m: u32, d: u32) -> i64 {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp()
    }
    // 10 bars/month on odd days 1,3,...,19 -- comfortably clears
    // MarketRegimePolicy::conservative_defaults().min_bars (8) per month.
    fn month_days() -> [u32; 10] {
        [1, 3, 5, 7, 9, 11, 13, 15, 17, 19]
    }

    let mut bars = Vec::new();
    let mut price: i64 = 100; // +1/bar over 10 bars ~9% move -- clears the 5% BullTrend threshold
    // Jan: strong, consistent uptrend -> BullTrend, genuine profit.
    for d in month_days() {
        bars.push(flat_bar("ES", ts(2024, 1, d), price));
        price += 1;
    }
    // Feb: flat -> Sideways, zero gain.
    for d in month_days() {
        bars.push(flat_bar("ES", ts(2024, 2, d), price));
    }
    // Mar: strong, consistent uptrend again -> BullTrend, genuine profit
    // (same magnitude as Jan, so MONTH concentration sits exactly at the 0.5
    // boundary and passes).
    for d in month_days() {
        bars.push(flat_bar("ES", ts(2024, 3, d), price));
        price += 1;
    }
    // Apr: flat -> Sideways, zero gain.
    for d in month_days() {
        bars.push(flat_bar("ES", ts(2024, 4, d), price));
    }

    let (report, config) = run(
        &bars,
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 10, sell_at_idx: 1000 }),
    );
    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 10, sell_at_idx: 1000 })
    });

    let concentration = output
        .scenarios
        .iter()
        .find(|s| s.name == "month_year_regime_concentration")
        .unwrap();
    assert!(concentration.applicable);
    assert!(
        !concentration.passed,
        "all profit concentrated in the bull_trend regime bucket must fail even though month \
         concentration alone sits at the boundary: {concentration:?}"
    );
    let reason = concentration.reason.as_deref().unwrap_or_default();
    assert!(reason.contains("regime:"), "got: {reason}");
    assert!(!reason.contains("month:"), "month dimension must PASS (0.5 boundary): {reason}");
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
fn rg01h_is_complete_only_after_both_deferred_scenarios_are_merged() {
    use mqk_backtest::RobustnessScenarioOutcome;

    let bars = healthy_single_symbol_bars();
    let (report, config) = run(
        &bars,
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 1, sell_at_idx: 3 }),
    );

    let output = run_robustness_gauntlet(&report, &config, &bars, || {
        Box::new(SingleSymbolBuyHoldSell { symbol: "ES", bar_idx: 0, qty: 1, sell_at_idx: 3 })
    });
    assert!(!output.is_complete(), "must be incomplete before either deferred scenario is merged");

    let after_dsr_pbo = output.merge_dsr_pbo_sensitivity(RobustnessScenarioOutcome {
        name: "dsr_pbo_sensitivity".to_string(),
        applicable: true,
        passed: true,
        reason: None,
        detail: "test-fabricated evaluated outcome".to_string(),
        research_trial_id: Some("rg01h_test_trial".to_string()),
        evidence: None,
    });
    assert_eq!(
        after_dsr_pbo.deferred.len(),
        2,
        "merging dsr_pbo_sensitivity must clear only its own deferred entry"
    );
    assert!(!after_dsr_pbo.is_complete(), "must still be incomplete: two scenarios remain deferred");

    let after_p7a_p7b = after_dsr_pbo.merge_dsr_pbo_sensitivity(RobustnessScenarioOutcome {
        name: "p7a_p7b_economic_replay_stress".to_string(),
        applicable: true,
        passed: true,
        reason: None,
        detail: "test-fabricated evaluated outcome".to_string(),
        research_trial_id: Some("rg01h_test_trial".to_string()),
        evidence: None,
    });
    assert_eq!(
        after_p7a_p7b.deferred.len(),
        1,
        "merging p7a_p7b_economic_replay_stress must clear only its own deferred entry"
    );
    assert!(!after_p7a_p7b.is_complete(), "must still be incomplete: one scenario remains deferred");

    let merged = after_p7a_p7b.merge_dsr_pbo_sensitivity(RobustnessScenarioOutcome {
        name: "genuine_shuffled_placebo".to_string(),
        applicable: true,
        passed: true,
        reason: None,
        detail: "test-fabricated evaluated outcome".to_string(),
        research_trial_id: Some("rg01h_test_trial".to_string()),
        evidence: None,
    });
    assert!(merged.deferred.is_empty(), "merging all three must clear every deferred entry");
    assert!(merged.is_complete(), "must be complete once every required scenario is present");
}
