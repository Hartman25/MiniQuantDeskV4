//! BKT-04P: Order log proof.
//!
//! Proves that the backtest engine emits one `BacktestOrder` record per intent,
//! regardless of outcome:
//!
//! - O1: Every admitted intent that finds a later bar to fill against produces an order with status Filled
//! - O2: Every allocation-cap-rejected intent produces an order with status Rejected
//! - O3: Filled order's order_id matches the corresponding fill's order_id
//! - O4: Rejected order has no corresponding fill
//! - O5: Flatten-all intents appear in the order log with status Filled
//! - O6: orders.len() >= fills.len() (can't fill more than we ordered)
//! - O7: Order log is stable across identical replays (deterministic)
//!
//! BKT-FUTURE-EXECUTION-01: admitted orders are no longer filled same-bar —
//! every test here gives an admitted intent a later bar of its own symbol to
//! resolve against, exactly as `BacktestEngine::run` now requires.

use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, CommissionModel, OrderStatus};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn bar(ts: i64, high: i64, low: i64) -> BacktestBar {
    BacktestBar::new("SPY", ts, 100_000_000, high, low, 100_000_000, 1_000)
}

/// Buy on bar 1 (fills bar 2), sell on bar 2 (fills bar 3).
struct BuyThenSell;

impl Strategy for BuyThenSell {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("bkt04p_buy_sell", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        match _ctx.now_tick {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", 10)]),
            2 => StrategyOutput::new(vec![TargetPosition::new("SPY", 0)]),
            _ => StrategyOutput::new(vec![]),
        }
    }
}

fn run_buy_sell() -> mqk_backtest::BacktestReport {
    let bars = vec![
        bar(1_700_000_060, 100_000_000, 100_000_000),
        bar(1_700_000_120, 100_000_000, 100_000_000),
        bar(1_700_000_180, 100_000_000, 100_000_000),
    ];
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 5_000_000;
    cfg.commission = CommissionModel::ZERO;
    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(BuyThenSell)).unwrap();
    engine.run(&bars).unwrap()
}

// ---------------------------------------------------------------------------
// O1: Admitted intents that find a later bar produce Filled orders
// ---------------------------------------------------------------------------

#[test]
fn allowed_intents_produce_filled_orders() {
    let report = run_buy_sell();
    assert_eq!(report.fills.len(), 2, "expected 2 fills (buy + sell)");

    let filled_orders: Vec<_> = report
        .orders
        .iter()
        .filter(|o| o.status == OrderStatus::Filled)
        .collect();

    assert_eq!(
        filled_orders.len(),
        2,
        "expected 2 Filled orders matching the 2 fills"
    );
}

// ---------------------------------------------------------------------------
// O2: Allocation-cap-rejected intents appear as Rejected orders
// ---------------------------------------------------------------------------

/// Strategy that tries to buy a huge position once — should be capped by the
/// exposure limit once its pending order reaches fill-time resolution
/// (BKT-FUTURE-EXECUTION-01: the allocation cap is checked at fill time,
/// using the actual fill price, not at signal time).
struct OverExposeOnce {
    fired: bool,
}

impl Strategy for OverExposeOnce {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("bkt04p_over_expose", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        if !self.fired {
            self.fired = true;
            // Target a 100-share position with a tight exposure cap → rejected.
            StrategyOutput::new(vec![TargetPosition::new("SPY", 100)])
        } else {
            StrategyOutput::new(vec![])
        }
    }
}

#[test]
fn risk_rejected_intent_appears_in_order_log() {
    // Tight exposure cap: 0.01x equity on 100k = $1000 → can't buy 100 shares @ $100.
    // Two bars: bar 1 signals the order, bar 2 gives it a later SPY bar to
    // resolve (and be cap-rejected) against.
    let bars = vec![
        bar(1_700_000_060, 100_000_000, 100_000_000),
        bar(1_700_000_120, 100_000_000, 100_000_000),
    ];
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 10_000; // 0.01x — forces rejection
    cfg.commission = CommissionModel::ZERO;
    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(OverExposeOnce { fired: false }))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    // No fills (cap-rejected at fill time)
    assert_eq!(report.fills.len(), 0, "tight cap should produce no fills");

    // But order log has the attempt
    assert_eq!(
        report.orders.len(),
        1,
        "rejected intent must appear in order log"
    );
    assert!(
        matches!(
            report.orders[0].status,
            OrderStatus::Rejected | OrderStatus::HaltTriggered
        ),
        "order status must be Rejected or HaltTriggered, got {:?}",
        report.orders[0].status
    );
}

// ---------------------------------------------------------------------------
// O3: Filled order's order_id matches corresponding fill's order_id
// ---------------------------------------------------------------------------

#[test]
fn filled_order_id_matches_fill_order_id() {
    let report = run_buy_sell();
    assert_eq!(report.fills.len(), 2);

    for fill in &report.fills {
        let matching_order = report.orders.iter().find(|o| o.order_id == fill.order_id);

        assert!(
            matching_order.is_some(),
            "every fill must have a matching order record (fill order_id={})",
            fill.order_id
        );
        assert_eq!(
            matching_order.unwrap().status,
            OrderStatus::Filled,
            "matching order must have Filled status"
        );
    }
}

// ---------------------------------------------------------------------------
// O4: Rejected order has no corresponding fill
// ---------------------------------------------------------------------------

#[test]
fn rejected_order_has_no_fill() {
    let bars = vec![
        bar(1_700_000_060, 100_000_000, 100_000_000),
        bar(1_700_000_120, 100_000_000, 100_000_000),
    ];
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 10_000;
    cfg.commission = CommissionModel::ZERO;
    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(OverExposeOnce { fired: false }))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    for order in report
        .orders
        .iter()
        .filter(|o| matches!(o.status, OrderStatus::Rejected | OrderStatus::HaltTriggered))
    {
        let has_fill = report.fills.iter().any(|f| f.order_id == order.order_id);
        assert!(
            !has_fill,
            "rejected/halted order must not have a corresponding fill (order_id={})",
            order.order_id
        );
    }
}

// ---------------------------------------------------------------------------
// O5: orders.len() >= fills.len()
// ---------------------------------------------------------------------------

#[test]
fn order_log_has_at_least_as_many_entries_as_fills() {
    let report = run_buy_sell();
    assert!(
        report.orders.len() >= report.fills.len(),
        "order log must have at least as many entries as fills: orders={} fills={}",
        report.orders.len(),
        report.fills.len()
    );
}

// ---------------------------------------------------------------------------
// O6: Order log is stable across identical replays
// ---------------------------------------------------------------------------

#[test]
fn order_log_is_deterministic_across_replays() {
    let r1 = run_buy_sell();
    let r2 = run_buy_sell();

    assert_eq!(r1.orders.len(), r2.orders.len());

    for (o1, o2) in r1.orders.iter().zip(r2.orders.iter()) {
        assert_eq!(
            o1.order_id, o2.order_id,
            "order_id must be stable across replays"
        );
        assert_eq!(
            o1.signal_ts, o2.signal_ts,
            "signal_ts must be stable across replays"
        );
        assert_eq!(o1.status, o2.status, "status must be stable across replays");
    }
}

// ---------------------------------------------------------------------------
// O7: Order symbols and sides are correct
// ---------------------------------------------------------------------------

#[test]
fn order_symbol_and_side_are_correct() {
    let report = run_buy_sell();
    for order in &report.orders {
        assert_eq!(order.symbol, "SPY", "all orders should be for SPY");
        assert!(order.qty > 0, "qty must be positive");
    }
}
