//! BKT-03P: Commission/fee modeling proof.
//!
//! Proves that `CommissionModel` correctly computes fees and that the
//! backtest engine wires those fees into fills at execution time:
//!
//! - ZERO model produces fee_micros = 0 on every fill
//! - Per-share model produces the expected per-share fee
//! - Bps-of-notional model produces the expected percentage fee
//! - Combined model sums both components
//! - A run with a non-zero commission produces lower final equity than
//!   an identical run with ZERO commission
//! - `conservative_defaults()` has non-zero commission
//! - `fill.fee_micros` matches what `compute_fee` would predict

use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, CommissionModel};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn bar(ts: i64) -> BacktestBar {
    BacktestBar::new(
        "SPY",
        ts,
        100_000_000, // open  $100.00
        100_000_000, // high  $100.00
        100_000_000, // low   $100.00
        100_000_000, // close $100.00
        1_000,
    )
}

/// Buy 10 SPY on bar 1, hold forever.
struct BuyTen;

impl Strategy for BuyTen {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("bkt03p_buy_ten", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        if _ctx.now_tick == 1 {
            StrategyOutput::new(vec![TargetPosition::new("SPY", 10)])
        } else {
            StrategyOutput::new(vec![])
        }
    }
}

fn run_one_buy(commission: CommissionModel) -> mqk_backtest::BacktestReport {
    let bars = vec![bar(1_700_000_060)];
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 5_000_000; // 5x — permissive
    cfg.commission = commission;
    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BuyTen)).unwrap();
    engine.run(&bars).unwrap()
}

// ---------------------------------------------------------------------------
// T1: ZERO model — fee_micros == 0 on every fill
// ---------------------------------------------------------------------------

#[test]
fn zero_commission_produces_zero_fee() {
    let report = run_one_buy(CommissionModel::ZERO);
    assert_eq!(report.fills.len(), 1, "expected one BUY fill");
    let fill = &report.fills[0];
    assert_eq!(
        fill.fee_micros, 0,
        "ZERO commission model must produce zero fee"
    );
}

// ---------------------------------------------------------------------------
// T2: Per-share model — fee == per_share_micros * qty
// ---------------------------------------------------------------------------

#[test]
fn per_share_commission_matches_expected() {
    // $0.005/share = 5_000 micros/share; 10 shares → 50_000 micros = $0.05
    let model = CommissionModel {
        per_share_micros: 5_000,
        bps_of_notional: 0,
    };
    let report = run_one_buy(model.clone());
    assert_eq!(report.fills.len(), 1);

    let fill = &report.fills[0];
    let expected_fee = model.compute_fee(fill.qty, fill.price_micros);
    assert_eq!(
        fill.fee_micros, expected_fee,
        "per-share fee must match compute_fee({}, {})",
        fill.qty, fill.price_micros
    );
    assert!(fill.fee_micros > 0, "per-share fee must be positive");
    // 10 shares * 5_000 micros = 50_000 micros
    assert_eq!(fill.fee_micros, 50_000);
}

// ---------------------------------------------------------------------------
// T3: Bps-of-notional model — fee == notional * bps / 10_000
// ---------------------------------------------------------------------------

#[test]
fn bps_commission_matches_expected() {
    // 1 bps of notional; fill at $100.00 = 100_000_000 micros; qty=10
    // notional = 10 * 100_000_000 = 1_000_000_000 micros = $1000
    // fee = 1_000_000_000 * 1 / 10_000 = 100_000 micros = $0.10
    let model = CommissionModel {
        per_share_micros: 0,
        bps_of_notional: 1,
    };
    let report = run_one_buy(model.clone());
    assert_eq!(report.fills.len(), 1);

    let fill = &report.fills[0];
    let expected_fee = model.compute_fee(fill.qty, fill.price_micros);
    assert_eq!(
        fill.fee_micros, expected_fee,
        "bps fee must match compute_fee"
    );
    assert!(fill.fee_micros > 0, "bps fee must be positive");
    assert_eq!(fill.fee_micros, 100_000);
}

// ---------------------------------------------------------------------------
// T4: Combined model sums both components
// ---------------------------------------------------------------------------

#[test]
fn combined_commission_is_additive() {
    let model = CommissionModel {
        per_share_micros: 5_000,
        bps_of_notional: 1,
    };
    let report = run_one_buy(model.clone());
    assert_eq!(report.fills.len(), 1);

    let fill = &report.fills[0];
    let expected_fee = model.compute_fee(fill.qty, fill.price_micros);
    assert_eq!(fill.fee_micros, expected_fee);
    // 50_000 (per-share) + 100_000 (bps) = 150_000
    assert_eq!(fill.fee_micros, 150_000);
}

// ---------------------------------------------------------------------------
// T5: Non-zero commission reduces equity vs zero-commission run
// ---------------------------------------------------------------------------

#[test]
fn nonzero_commission_reduces_equity() {
    let zero_report = run_one_buy(CommissionModel::ZERO);
    let paid_report = run_one_buy(CommissionModel {
        per_share_micros: 5_000,
        bps_of_notional: 0,
    });

    let zero_equity = zero_report
        .equity_curve
        .last()
        .map(|(_, e)| *e)
        .unwrap_or(0);
    let paid_equity = paid_report
        .equity_curve
        .last()
        .map(|(_, e)| *e)
        .unwrap_or(0);

    assert!(
        paid_equity < zero_equity,
        "commission must reduce final equity: zero={zero_equity} paid={paid_equity}"
    );
}

// ---------------------------------------------------------------------------
// T6: conservative_defaults has non-zero commission
// ---------------------------------------------------------------------------

#[test]
fn conservative_defaults_has_nonzero_commission() {
    let cfg = BacktestConfig::conservative_defaults();
    assert!(
        cfg.commission.per_share_micros > 0 || cfg.commission.bps_of_notional > 0,
        "conservative_defaults must have a non-zero commission model"
    );
}

// ---------------------------------------------------------------------------
// T7: compute_fee pure-function unit tests
// ---------------------------------------------------------------------------

#[test]
fn compute_fee_zero_model_always_returns_zero() {
    let m = CommissionModel::ZERO;
    assert_eq!(m.compute_fee(0, 100_000_000), 0);
    assert_eq!(m.compute_fee(1, 100_000_000), 0);
    assert_eq!(m.compute_fee(1_000, 50_000_000), 0);
}

#[test]
fn compute_fee_zero_qty_returns_zero() {
    let m = CommissionModel {
        per_share_micros: 5_000,
        bps_of_notional: 2,
    };
    assert_eq!(
        m.compute_fee(0, 100_000_000),
        0,
        "zero qty must produce zero fee"
    );
}

#[test]
fn compute_fee_per_share_is_linear_in_qty() {
    let m = CommissionModel {
        per_share_micros: 1_000,
        bps_of_notional: 0,
    };
    // fee grows linearly with qty
    assert_eq!(m.compute_fee(1, 100_000_000), 1_000);
    assert_eq!(m.compute_fee(10, 100_000_000), 10_000);
    assert_eq!(m.compute_fee(100, 100_000_000), 100_000);
}
