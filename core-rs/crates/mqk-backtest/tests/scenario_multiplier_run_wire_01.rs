//! BACKTEST-MULTIPLIER-RUN-WIRE-01: engine-level proof that
//! `BacktestInstrumentEconomics` is wired into `BacktestEngine` via
//! `with_economics`, behind a backtest-only seam that never touches
//! `mqk_portfolio` or live/paper accounting.

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEngine, BacktestError, BacktestInstrumentEconomics,
    OrderStatus,
};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

const M: i64 = 1_000_000;

/// Buys `qty` of "ES" on bar 1, holds through bar 2, sells (closes) on bar 3,
/// then holds flat. `targets_to_order_intents` treats a symbol omitted from
/// the target list as an implicit target of 0, so the hold target must be
/// explicitly restated every bar rather than only on the entry bar.
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

/// Buys `qty` of "ES" on bar 1 only (used for the allocation-cap test).
struct BuyOnce {
    bar_idx: u64,
    qty: i64,
}

impl BuyOnce {
    fn new(qty: i64) -> Self {
        Self { bar_idx: 0, qty }
    }
}

impl Strategy for BuyOnce {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("BuyOnce", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 => StrategyOutput::new(vec![TargetPosition::new("ES", self.qty)]),
            _ => StrategyOutput::new(vec![]),
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

/// Allocation cap large enough to never bind in the P&L-scaling tests --
/// isolates "full run P&L/equity scaling" from cap interaction, which is
/// tested separately (and deliberately) in `bmw02c`.
fn cfg_with_wide_cap() -> BacktestConfig {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 100_000_000; // 100x equity
    cfg
}

// --- bmw01: default (unconfigured) economics preserves existing backtest ---

#[test]
fn bmw01_default_equity_economics_preserves_existing_backtest() {
    let bars = buy_hold_sell_bars();
    let mut engine = BacktestEngine::new(cfg_with_wide_cap());
    assert_eq!(engine.economics().contract_multiplier, 1);

    engine.add_strategy(Box::new(BuyHoldSell::new(10))).unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(
        report.fills.len(),
        2,
        "expect one buy fill and one sell fill"
    );
    // The parallel multiplier-aware curve is byte-identical to the existing,
    // mqk_portfolio-driven equity curve when no economics is configured.
    assert_eq!(
        engine.economics_equity_curve(),
        report.equity_curve.as_slice()
    );
}

#[test]
fn bmw01b_explicit_multiplier_one_equals_default() {
    let bars = buy_hold_sell_bars();

    let mut engine_default = BacktestEngine::new(cfg_with_wide_cap());
    engine_default
        .add_strategy(Box::new(BuyHoldSell::new(10)))
        .unwrap();
    let report_default = engine_default.run(&bars).unwrap();

    let mut engine_explicit = BacktestEngine::new(cfg_with_wide_cap())
        .with_economics(BacktestInstrumentEconomics::new(1, None, None).unwrap());
    engine_explicit
        .add_strategy(Box::new(BuyHoldSell::new(10)))
        .unwrap();
    let report_explicit = engine_explicit.run(&bars).unwrap();

    assert_eq!(report_default.equity_curve, report_explicit.equity_curve);
    assert_eq!(
        engine_default.economics_equity_curve(),
        engine_explicit.economics_equity_curve()
    );
    assert_eq!(
        engine_default.economics_realized_pnl_micros(),
        engine_explicit.economics_realized_pnl_micros()
    );

    // BACKTEST-REPORT-ECONOMICS-ARTIFACT-01: explicit multiplier=1 is
    // `is_default_equity()`, so report identity and the report's economics
    // section must match the unconfigured-default report exactly.
    assert_eq!(report_default.run_id, report_explicit.run_id);
    assert_eq!(report_default.config_id, report_explicit.config_id);
    assert_eq!(report_default.economics, report_explicit.economics);
}

// --- bmw02: synthetic multiplier=50/100 full-run proof ---

#[test]
fn bmw02_multiplier_50_scales_full_run_outputs() {
    let bars = buy_hold_sell_bars();
    let initial_cash = cfg_with_wide_cap().initial_cash_micros;

    let mut engine1 = BacktestEngine::new(cfg_with_wide_cap());
    engine1
        .add_strategy(Box::new(BuyHoldSell::new(10)))
        .unwrap();
    let report1 = engine1.run(&bars).unwrap();

    let mut engine50 = BacktestEngine::new(cfg_with_wide_cap())
        .with_economics(BacktestInstrumentEconomics::new(50, None, None).unwrap());
    engine50
        .add_strategy(Box::new(BuyHoldSell::new(10)))
        .unwrap();
    let report50 = engine50.run(&bars).unwrap();

    // `self.portfolio` (fills, fill count) is never touched by this seam.
    assert_eq!(report1.fills.len(), report50.fills.len());

    // BACKTEST-REPORT-ECONOMICS-ARTIFACT-01: report.equity_curve now surfaces
    // the multiplier-aware economics curve once multiplier != 1 -- closing the
    // gap this test originally documented (the underlying mqk_portfolio-driven
    // curve is still untouched; see `bmw01`'s multiplier=1 equality instead).
    assert_eq!(
        report50.equity_curve,
        engine50.economics_equity_curve().to_vec()
    );
    assert_ne!(
        report1.equity_curve, report50.equity_curve,
        "multiplier=50 must scale reported equity, not reproduce the multiplier=1 curve"
    );

    // Realized P&L: (4,510 - 4,500) * 10 = $100 at multiplier=1; x50 at multiplier=50.
    assert_eq!(engine1.economics_realized_pnl_micros(), 100 * M);
    assert_eq!(engine50.economics_realized_pnl_micros(), 50 * 100 * M);
    assert_eq!(report50.economics.realized_pnl_micros, 50 * 100 * M);
    assert_eq!(report50.economics.contract_multiplier, 50);

    // Final equity gain (position is flat again after bar 3) scales identically.
    let final_gain1 = engine1.economics_equity_curve().last().unwrap().1 - initial_cash;
    let final_gain50 = engine50.economics_equity_curve().last().unwrap().1 - initial_cash;
    assert_eq!(final_gain1, 100 * M);
    assert_eq!(final_gain50, 50 * 100 * M);

    // Report identity must diverge from the default-equity report once
    // economics is non-default (multiplier=50 here).
    assert_ne!(report1.run_id, report50.run_id);
    assert_eq!(
        report1.config_id, report50.config_id,
        "config_id stays config-only (economics is not a BacktestConfig field)"
    );
}

#[test]
fn bmw02b_multiplier_100_scales_full_run_outputs() {
    let bars = buy_hold_sell_bars();
    let initial_cash = cfg_with_wide_cap().initial_cash_micros;

    let mut engine1 = BacktestEngine::new(cfg_with_wide_cap());
    engine1
        .add_strategy(Box::new(BuyHoldSell::new(10)))
        .unwrap();
    let report1 = engine1.run(&bars).unwrap();

    let mut engine100 = BacktestEngine::new(cfg_with_wide_cap())
        .with_economics(BacktestInstrumentEconomics::new(100, None, None).unwrap());
    engine100
        .add_strategy(Box::new(BuyHoldSell::new(10)))
        .unwrap();
    let report100 = engine100.run(&bars).unwrap();

    assert_eq!(report1.fills.len(), report100.fills.len());

    // BACKTEST-REPORT-ECONOMICS-ARTIFACT-01: same closure as bmw02, at
    // multiplier=100 instead of 50.
    assert_eq!(
        report100.equity_curve,
        engine100.economics_equity_curve().to_vec()
    );
    assert_ne!(
        report1.equity_curve, report100.equity_curve,
        "multiplier=100 must scale reported equity, not reproduce the multiplier=1 curve"
    );

    assert_eq!(engine1.economics_realized_pnl_micros(), 100 * M);
    assert_eq!(engine100.economics_realized_pnl_micros(), 100 * 100 * M);
    assert_eq!(report100.economics.realized_pnl_micros, 100 * 100 * M);
    assert_eq!(report100.economics.contract_multiplier, 100);

    let final_gain1 = engine1.economics_equity_curve().last().unwrap().1 - initial_cash;
    let final_gain100 = engine100.economics_equity_curve().last().unwrap().1 - initial_cash;
    assert_eq!(final_gain1, 100 * M);
    assert_eq!(final_gain100, 100 * 100 * M);

    assert_ne!(report1.run_id, report100.run_id);
}

#[test]
fn bmw02c_allocation_cap_notional_scales_with_multiplier() {
    // Single bar: qty=100 @ $100 = $10,000 notional at multiplier=1.
    let bars = vec![flat_bar(1_700_000_060, 100)];

    let mut cfg = BacktestConfig::test_defaults();
    // 0.20x equity = $20,000 cap (default equity = $100,000).
    cfg.max_gross_exposure_mult_micros = 200_000;

    let mut engine1 = BacktestEngine::new(cfg.clone());
    engine1.add_strategy(Box::new(BuyOnce::new(100))).unwrap();
    let report1 = engine1.run(&bars).unwrap();
    assert_eq!(
        report1.fills.len(),
        1,
        "$10,000 notional fits the $20,000 cap at multiplier=1"
    );

    let mut engine50 = BacktestEngine::new(cfg)
        .with_economics(BacktestInstrumentEconomics::new(50, None, None).unwrap());
    engine50.add_strategy(Box::new(BuyOnce::new(100))).unwrap();
    let report50 = engine50.run(&bars).unwrap();
    assert_eq!(
        report50.fills.len(),
        0,
        "the same qty, scaled 50x by economics, breaches the $20,000 cap"
    );
    assert_eq!(report50.orders.len(), 1);
    assert_eq!(report50.orders[0].status, OrderStatus::Rejected);
}

// --- bmw03: invalid multiplier fails closed before any bar/artifact ---

#[test]
fn bmw03_invalid_multiplier_zero_fails_closed() {
    // Constructed directly (bypassing the validating `::new()`) to exercise
    // the engine's own defense-in-depth check.
    let mut engine = BacktestEngine::new(BacktestConfig::test_defaults()).with_economics(
        BacktestInstrumentEconomics {
            contract_multiplier: 0,
            initial_margin_micros: None,
            maintenance_margin_micros: None,
        },
    );
    let err = engine.run(&[]).unwrap_err();
    assert_eq!(err, BacktestError::InvalidEconomics { multiplier: 0 });
}

#[test]
fn bmw03b_invalid_multiplier_negative_fails_closed() {
    let mut engine = BacktestEngine::new(BacktestConfig::test_defaults()).with_economics(
        BacktestInstrumentEconomics {
            contract_multiplier: -7,
            initial_margin_micros: None,
            maintenance_margin_micros: None,
        },
    );
    let err = engine.run(&[]).unwrap_err();
    assert_eq!(err, BacktestError::InvalidEconomics { multiplier: -7 });
}

// --- bmw04: economics metadata is truthful and margin is inert ---

#[test]
fn bmw04_economics_metadata_round_trips_truthfully_and_margin_is_inert() {
    let bars = buy_hold_sell_bars();
    let econ_bare = BacktestInstrumentEconomics::new(50, None, None).unwrap();
    let econ_margin =
        BacktestInstrumentEconomics::new(50, Some(10_000 * M), Some(5_000 * M)).unwrap();

    let mut engine_bare = BacktestEngine::new(cfg_with_wide_cap()).with_economics(econ_bare);
    engine_bare
        .add_strategy(Box::new(BuyHoldSell::new(10)))
        .unwrap();
    let report_bare = engine_bare.run(&bars).unwrap();

    let mut engine_margin =
        BacktestEngine::new(cfg_with_wide_cap()).with_economics(econ_margin.clone());
    engine_margin
        .add_strategy(Box::new(BuyHoldSell::new(10)))
        .unwrap();
    let report_margin = engine_margin.run(&bars).unwrap();

    // BACKTEST-REPORT-ECONOMICS-ARTIFACT-01: the report's economics section
    // round-trips the configured margin metadata truthfully, and never
    // claims enforcement that does not exist.
    assert_eq!(report_margin.economics.contract_multiplier, 50);
    assert_eq!(
        report_margin.economics.initial_margin_micros,
        Some(10_000 * M)
    );
    assert_eq!(
        report_margin.economics.maintenance_margin_micros,
        Some(5_000 * M)
    );
    assert!(!report_margin.economics.margin_enforced);
    // Margin-bearing run_id must differ from the bare (margin-free) run_id at
    // the same multiplier -- margin metadata is identity-relevant even though
    // it never alters P&L/equity math.
    assert_ne!(report_bare.run_id, report_margin.run_id);

    // Truthful round-trip: the engine reports back exactly what was configured.
    assert_eq!(engine_margin.economics(), &econ_margin);
    assert_eq!(
        engine_margin.economics().initial_margin_micros,
        Some(10_000 * M)
    );
    assert_eq!(
        engine_margin.economics().maintenance_margin_micros,
        Some(5_000 * M)
    );

    // Margin metadata is scaffold-only: identical P&L/equity with or without it.
    assert_eq!(
        engine_bare.economics_realized_pnl_micros(),
        engine_margin.economics_realized_pnl_micros()
    );
    assert_eq!(
        engine_bare.economics_equity_curve(),
        engine_margin.economics_equity_curve()
    );
}
