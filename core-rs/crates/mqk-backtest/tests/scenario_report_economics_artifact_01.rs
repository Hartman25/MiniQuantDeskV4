//! BACKTEST-REPORT-ECONOMICS-ARTIFACT-01: report/identity-level proof that
//! `BacktestReport` carries a truthful economics section, that
//! `report.equity_curve` becomes multiplier-aware once `contract_multiplier
//! != 1`, and that `run_id` becomes economics-sensitive for non-default
//! economics while staying byte-identical for the default (equity) case.
//!
//! This builds on the engine-wiring proof in `scenario_multiplier_run_wire_01.rs`
//! (BACKTEST-MULTIPLIER-RUN-WIRE-01), which proved `economics_equity_curve()`
//! and `economics_realized_pnl_micros()` exist and scale correctly on the
//! engine. This file proves the *report* (the thing artifacts are built from)
//! actually surfaces that truth.

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEconomicsReport, BacktestEngine, BacktestInstrumentEconomics,
    BacktestReport,
};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

const M: i64 = 1_000_000;

/// Buys `qty` of "ES" on bar 1, holds through bar 2, sells (closes) on bar 3,
/// then holds flat.
struct BuyHoldSell {
    bar_idx: u64,
    qty: i64,
}

impl BuyHoldSell {
    fn new(qty: i64) -> Self {
        Self { bar_idx: 0, qty }
    }
}

impl Strategy for BuyHoldSell {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BuyHoldSell", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 | 2 => StrategyOutput::new(vec![TargetPosition::new("ES", self.qty)]),
            _ => StrategyOutput::new(vec![TargetPosition::new("ES", 0)]),
        }
    }
}

fn flat_bar(end_ts: i64, price_usd: i64) -> BacktestBar {
    let p = price_usd * M;
    BacktestBar::new("ES", end_ts, p, p, p, p, 1_000)
}

/// 4 flat bars: buy fills @ $4,500 (bar1), sell fills @ $4,510 (bar3).
fn buy_hold_sell_bars() -> Vec<BacktestBar> {
    vec![
        flat_bar(1_700_000_060, 4_500),
        flat_bar(1_700_000_120, 4_505),
        flat_bar(1_700_000_180, 4_510),
        flat_bar(1_700_000_240, 4_515),
    ]
}

/// Allocation cap large enough to never bind -- isolates report/identity
/// behavior from cap interaction (covered separately in `bmw02c`).
fn cfg_with_wide_cap() -> BacktestConfig {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 100_000_000; // 100x equity
    cfg
}

fn run_with_economics(economics: Option<BacktestInstrumentEconomics>) -> BacktestReport {
    let bars = buy_hold_sell_bars();
    let mut engine = BacktestEngine::new(cfg_with_wide_cap());
    if let Some(econ) = economics {
        engine = engine.with_economics(econ);
    }
    engine.add_strategy(Box::new(BuyHoldSell::new(10))).unwrap();
    engine.run(&bars).unwrap()
}

// ---------------------------------------------------------------------------
// brea01: default equity report preserves output and identity
// ---------------------------------------------------------------------------

#[test]
fn brea01_default_equity_report_preserves_output() {
    let report = run_with_economics(None);

    // Default report economics must be equity-shaped: multiplier=1, no
    // margin scaffold, no enforcement. `realized_pnl_micros` is truthfully
    // non-zero here (the strategy actually traded) -- it is NOT compared
    // against the static `BacktestEconomicsReport::equity()` zero-state.
    assert_eq!(report.economics.contract_multiplier, 1);
    assert_eq!(report.economics.initial_margin_micros, None);
    assert_eq!(report.economics.maintenance_margin_micros, None);
    assert!(!report.economics.margin_enforced);
    assert_eq!(report.economics.realized_pnl_micros, 100 * M);

    // Explicit multiplier=1 (no margin) must be byte-identical in every
    // report field that economics could plausibly touch.
    let report_explicit = run_with_economics(Some(
        BacktestInstrumentEconomics::new(1, None, None).unwrap(),
    ));
    assert_eq!(report.equity_curve, report_explicit.equity_curve);
    assert_eq!(report.run_id, report_explicit.run_id);
    assert_eq!(report.config_id, report_explicit.config_id);
    assert_eq!(report.economics, report_explicit.economics);
}

// ---------------------------------------------------------------------------
// brea02: multiplier=50/100 report curve is economics-aware
// ---------------------------------------------------------------------------

#[test]
fn brea02_multiplier_50_report_curve_is_economics_aware() {
    let default_report = run_with_economics(None);
    let econ50 = BacktestInstrumentEconomics::new(50, None, None).unwrap();
    let report50 = run_with_economics(Some(econ50));

    // report.equity_curve must now be the multiplier-aware curve: same shape
    // (one point per bar), but different values once a position is held.
    assert_eq!(report50.equity_curve.len(), default_report.equity_curve.len());
    assert_ne!(
        report50.equity_curve, default_report.equity_curve,
        "multiplier=50 must change the reported equity curve"
    );

    // Realized P&L: (4,510-4,500)*10 = $100 at multiplier=1; x50 at multiplier=50.
    assert_eq!(default_report.economics.realized_pnl_micros, 100 * M);
    assert_eq!(report50.economics.realized_pnl_micros, 50 * 100 * M);
    assert_eq!(report50.economics.contract_multiplier, 50);

    // Final equity gain (flat again after bar 3) scales by exactly 50x.
    let initial_cash = cfg_with_wide_cap().initial_cash_micros;
    let gain_default = default_report.equity_curve.last().unwrap().1 - initial_cash;
    let gain50 = report50.equity_curve.last().unwrap().1 - initial_cash;
    assert_eq!(gain_default, 100 * M);
    assert_eq!(gain50, 50 * 100 * M);
}

#[test]
fn brea02b_multiplier_100_report_curve_is_economics_aware() {
    let default_report = run_with_economics(None);
    let econ100 = BacktestInstrumentEconomics::new(100, None, None).unwrap();
    let report100 = run_with_economics(Some(econ100));

    assert_eq!(report100.equity_curve.len(), default_report.equity_curve.len());
    assert_ne!(
        report100.equity_curve, default_report.equity_curve,
        "multiplier=100 must change the reported equity curve"
    );

    assert_eq!(default_report.economics.realized_pnl_micros, 100 * M);
    assert_eq!(report100.economics.realized_pnl_micros, 100 * 100 * M);
    assert_eq!(report100.economics.contract_multiplier, 100);

    let initial_cash = cfg_with_wide_cap().initial_cash_micros;
    let gain_default = default_report.equity_curve.last().unwrap().1 - initial_cash;
    let gain100 = report100.equity_curve.last().unwrap().1 - initial_cash;
    assert_eq!(gain_default, 100 * M);
    assert_eq!(gain100, 100 * 100 * M);
}

// ---------------------------------------------------------------------------
// brea04: config/run identity is economics-sensitive for non-default economics
// ---------------------------------------------------------------------------

#[test]
fn brea04_config_identity_includes_non_default_economics() {
    let default_report = run_with_economics(None);
    let report_mult = run_with_economics(Some(
        BacktestInstrumentEconomics::new(50, None, None).unwrap(),
    ));
    let report_margin_only = run_with_economics(Some(
        BacktestInstrumentEconomics::new(1, Some(10_000 * M), None).unwrap(),
    ));

    // `config_id` is a pure function of `BacktestConfig` only -- economics is
    // intentionally not a `BacktestConfig` field, so config_id never moves.
    assert_eq!(default_report.config_id, report_mult.config_id);
    assert_eq!(default_report.config_id, report_margin_only.config_id);

    // `run_id` is the full replay identity and DOES move for any economics
    // that differs from `BacktestInstrumentEconomics::equity()` -- multiplier
    // changes, and margin-only changes (which never alter P&L/equity), both
    // count, because the *reported* economics metadata genuinely differs.
    assert_ne!(default_report.run_id, report_mult.run_id);
    assert_ne!(default_report.run_id, report_margin_only.run_id);
    assert_ne!(
        report_mult.run_id, report_margin_only.run_id,
        "two different non-default economics values must not collide on run_id"
    );

    // Re-running with the exact same non-default economics must reproduce
    // the same run_id (determinism, not just "differs from default").
    let report_mult_again = run_with_economics(Some(
        BacktestInstrumentEconomics::new(50, None, None).unwrap(),
    ));
    assert_eq!(report_mult.run_id, report_mult_again.run_id);
}

// ---------------------------------------------------------------------------
// brea05: BacktestReport literals remain fixture-safe after the field addition
// ---------------------------------------------------------------------------

#[test]
fn brea05_backtest_report_literals_are_fixture_safe() {
    // This only compiles if `BacktestReport::test_fixture()` already supplies
    // a default for every field this literal omits -- including the new
    // `economics` field. If a future field addition ever made `economics`
    // exhaustively required here, this test file would fail to compile,
    // which is exactly the fixture-safety property BACKTEST-REPORT-FIXTURE-
    // READY-01-COMBINED established and this patch must not regress.
    let r = BacktestReport {
        strategy_name: "fixture_safe_probe".to_string(),
        ..BacktestReport::test_fixture()
    };
    assert_eq!(r.economics, BacktestEconomicsReport::equity());
    assert_eq!(r.strategy_name, "fixture_safe_probe");
}
