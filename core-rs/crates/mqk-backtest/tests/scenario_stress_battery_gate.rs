//! B5-3: Mandatory Stress Battery Gate
//!
//! A named promotion gate: every test in this file must pass before a strategy
//! is considered eligible for promotion from backtest to live.
//!
//! Five gate properties, all referencing `BacktestConfig::conservative_defaults()`
//! as the authoritative promotion configuration:
//!
//! 1. **Slippage floor matches base.yaml** — `conservative_defaults().stress.slippage_bps == 5`
//!    (base.yaml `execution.base_slippage_bps: 5`).
//!
//! 2. **Volatility multiplier matches base.yaml** — `conservative_defaults().stress.volatility_mult_bps == 5_000`
//!    (base.yaml `execution.volatility_multiplier: 0.5` → 5_000 bps = 50 % of spread).
//!
//! 3. **BUY fill under conservative config is strictly worse than under test defaults** —
//!    higher fill price is measurable from the same spread bar.
//!
//! 4. **SELL fill under conservative config is strictly worse than under test defaults** —
//!    lower fill price is measurable from the same spread bar.
//!
//! 5. **Conservative config produces strictly lower final equity than test defaults** —
//!    combined flat + volatility slippage degrades the round-trip P&L by a
//!    quantifiable and deterministic amount.
//!
//! # Slippage arithmetic for the test bar
//!
//! Bar: open=$500, high=$510, low=$490, close=$500 (spread = $20 each side).
//!
//! ```text
//! bar_spread_bps    = (510 − 490) × 10_000 / 500  = 400 bps
//! vol_component     = 400 × 5_000 / 10_000         = 200 bps
//! effective_slip    = 5 (floor) + 200               = 205 bps
//!
//! BUY  base = HIGH = $510; adj = $510 × 205 / 10_000 = $10.455 → fill = $520.455
//! SELL base = LOW  = $490; adj = $490 × 205 / 10_000 = $10.045 → fill = $479.955
//!
//! 10-share round-trip:
//!   conservative cost  = 10 × ($520.455 − $479.955) = $405.00 → equity = $99,595
//!   test_defaults cost = 10 × ($510     − $490    )  = $200.00 → equity = $99,800
//! ```
//!
//! All five assertions are mathematically provable from the above.

use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

// ---------------------------------------------------------------------------
// Strategy stubs
// ---------------------------------------------------------------------------

/// HoldQty: opens `qty` long on bar 1 and holds indefinitely.
/// Used for single-bar BUY fill tests.
struct HoldQty {
    qty: i64,
    bar_idx: u64,
}

impl HoldQty {
    fn new(qty: i64) -> Self {
        Self { qty, bar_idx: 0 }
    }
}

impl Strategy for HoldQty {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("HoldQty", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        // Always target qty — on bar 1 opens; on subsequent bars holds (no intent).
        StrategyOutput::new(vec![TargetPosition::new("SPY", self.qty)])
    }
}

/// RoundTrip: buys `qty` on bar 1, flattens on bar 2.
struct RoundTrip {
    qty: i64,
    bar_idx: u64,
}

impl RoundTrip {
    fn new(qty: i64) -> Self {
        Self { qty, bar_idx: 0 }
    }
}

impl Strategy for RoundTrip {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("RoundTrip", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", self.qty)]),
            _ => StrategyOutput::new(vec![TargetPosition::new("SPY", 0)]),
        }
    }
}

// ---------------------------------------------------------------------------
// Bar helper
// ---------------------------------------------------------------------------

/// Spread bar: close=$500, high=$510, low=$490.
///
/// Bar spread in bps of close = (510 − 490) × 10_000 / 500 = 400 bps.
/// With conservative_defaults (volatility_mult_bps = 5_000):
///   vol_component     = 400 × 5_000 / 10_000 = 200 bps
///   effective_slippage = 5 (floor) + 200       = 205 bps
fn spread_bar(ts: i64) -> BacktestBar {
    BacktestBar::new(
        "SPY",
        ts,
        500_000_000, // open  = $500
        510_000_000, // high  = $510  (+$10 above close)
        490_000_000, // low   = $490  (-$10 below close)
        500_000_000, // close = $500
        1_000,
    )
}

// ---------------------------------------------------------------------------
// Gate 1: slippage floor matches base.yaml
// ---------------------------------------------------------------------------

/// GATE 1 of 5.
///
/// `conservative_defaults().stress.slippage_bps` must equal 5, mirroring
/// `config/defaults/base.yaml` `execution.base_slippage_bps: 5`.
///
/// Changing this value without updating base.yaml breaks the backtest/live
/// alignment invariant and must block promotion.
#[test]
fn gate_slippage_floor_matches_base_yaml() {
    let cfg = BacktestConfig::conservative_defaults();
    assert_eq!(
        cfg.stress.slippage_bps, 5,
        "conservative_defaults slippage_bps must be 5 (mirrors base.yaml execution.base_slippage_bps)"
    );
}

// ---------------------------------------------------------------------------
// Gate 2: volatility multiplier matches base.yaml
// ---------------------------------------------------------------------------

/// GATE 2 of 5.
///
/// `conservative_defaults().stress.volatility_mult_bps` must equal 5_000,
/// mirroring `config/defaults/base.yaml` `execution.volatility_multiplier: 0.5`
/// (0.5 expressed as bps = 5_000 bps = 50 % of the bar spread).
///
/// Changing this value without updating base.yaml breaks the alignment invariant.
#[test]
fn gate_volatility_mult_matches_base_yaml() {
    let cfg = BacktestConfig::conservative_defaults();
    assert_eq!(
        cfg.stress.volatility_mult_bps, 5_000,
        "conservative_defaults volatility_mult_bps must be 5_000 \
         (mirrors base.yaml execution.volatility_multiplier: 0.5)"
    );
}

// ---------------------------------------------------------------------------
// Gate 3: BUY fill under conservative config is strictly worse than test defaults
// ---------------------------------------------------------------------------

/// GATE 3 of 5.
///
/// A BUY fill under `conservative_defaults` must be at a strictly higher price
/// than the same fill under `test_defaults` (zero slippage).
///
/// Expected values for the spread bar:
/// - test_defaults BUY fill  = HIGH = 510_000_000
/// - conservative BUY fill   = HIGH + 205 bps adj = 520_455_000
#[test]
fn gate_buy_fill_price_is_worse_under_conservative_config() {
    let bar = spread_bar(1_700_000_060);

    // Run with test_defaults (0 slippage).
    let mut engine_test = BacktestEngine::new(BacktestConfig::test_defaults());
    engine_test.add_strategy(Box::new(HoldQty::new(10))).unwrap();
    let report_test = engine_test.run(&[bar.clone()]).unwrap();

    // Run with conservative_defaults (205 effective bps on this bar).
    let mut engine_cons = BacktestEngine::new(BacktestConfig::conservative_defaults());
    engine_cons.add_strategy(Box::new(HoldQty::new(10))).unwrap();
    let report_cons = engine_cons.run(&[bar.clone()]).unwrap();

    assert_eq!(report_test.fills.len(), 1, "test_defaults: expected 1 BUY fill");
    assert_eq!(report_cons.fills.len(), 1, "conservative_defaults: expected 1 BUY fill");

    let test_price = report_test.fills[0].price_micros;
    let cons_price = report_cons.fills[0].price_micros;

    // BUY slippage raises the fill price: conservative must be strictly worse.
    assert!(
        cons_price > test_price,
        "conservative BUY fill ({} micros) must be > test BUY fill ({} micros) — \
         stress slippage must produce a measurably worse entry price",
        cons_price,
        test_price,
    );
}

// ---------------------------------------------------------------------------
// Gate 4: SELL fill under conservative config is strictly worse than test defaults
// ---------------------------------------------------------------------------

/// GATE 4 of 5.
///
/// A SELL fill under `conservative_defaults` must be at a strictly lower price
/// than the same fill under `test_defaults` (zero slippage).
///
/// Expected values for the spread bar:
/// - test_defaults SELL fill  = LOW = 490_000_000
/// - conservative SELL fill   = LOW − 205 bps adj = 479_955_000
#[test]
fn gate_sell_fill_price_is_worse_under_conservative_config() {
    let buy_bar = spread_bar(1_700_000_060);
    let sell_bar = spread_bar(1_700_000_120);

    // Run with test_defaults (0 slippage).
    let mut engine_test = BacktestEngine::new(BacktestConfig::test_defaults());
    engine_test.add_strategy(Box::new(RoundTrip::new(10))).unwrap();
    let report_test = engine_test.run(&[buy_bar.clone(), sell_bar.clone()]).unwrap();

    // Run with conservative_defaults (205 effective bps on these bars).
    let mut engine_cons = BacktestEngine::new(BacktestConfig::conservative_defaults());
    engine_cons
        .add_strategy(Box::new(RoundTrip::new(10)))
        .unwrap();
    let report_cons = engine_cons.run(&[buy_bar.clone(), sell_bar.clone()]).unwrap();

    assert_eq!(report_test.fills.len(), 2, "test_defaults: expected 2 fills (buy + sell)");
    assert_eq!(report_cons.fills.len(), 2, "conservative_defaults: expected 2 fills (buy + sell)");

    let test_sell_price = report_test.fills[1].price_micros;
    let cons_sell_price = report_cons.fills[1].price_micros;

    // SELL slippage lowers the fill price: conservative must be strictly worse.
    assert!(
        cons_sell_price < test_sell_price,
        "conservative SELL fill ({} micros) must be < test SELL fill ({} micros) — \
         stress slippage must produce a measurably worse exit price",
        cons_sell_price,
        test_sell_price,
    );
}

// ---------------------------------------------------------------------------
// Gate 5: Conservative config produces strictly lower final equity than test defaults
// ---------------------------------------------------------------------------

/// GATE 5 of 5.
///
/// Running the same round-trip strategy under `conservative_defaults` must
/// produce a strictly lower final equity than under `test_defaults`.
///
/// This is the combined-effect gate: flat slippage + volatility slippage both
/// increase buy cost and reduce sell proceeds, lowering the final equity.
///
/// Expected values (initial equity = $100,000):
/// - test_defaults final equity  = $99,800  (loss = $200 from 10× spread)
/// - conservative final equity   = $99,595  (loss = $405 from 10× spread + slip)
///
/// Promotion config must never produce outcomes as optimistic as the permissive
/// test config. A failing gate here means slippage is not being applied.
#[test]
fn gate_conservative_config_produces_lower_final_equity() {
    let buy_bar = spread_bar(1_700_000_060);
    let sell_bar = spread_bar(1_700_000_120);

    // Run with test_defaults (0 slippage, no risk limits, no integrity).
    let mut engine_test = BacktestEngine::new(BacktestConfig::test_defaults());
    engine_test.add_strategy(Box::new(RoundTrip::new(10))).unwrap();
    let report_test = engine_test
        .run(&[buy_bar.clone(), sell_bar.clone()])
        .unwrap();

    // Run with conservative_defaults (205 bps effective, risk limits active, integrity on).
    let mut engine_cons = BacktestEngine::new(BacktestConfig::conservative_defaults());
    engine_cons
        .add_strategy(Box::new(RoundTrip::new(10)))
        .unwrap();
    let report_cons = engine_cons
        .run(&[buy_bar.clone(), sell_bar.clone()])
        .unwrap();

    // Conservative run must not halt: daily loss $405 is well within the $2,000 limit.
    assert!(
        !report_cons.halted,
        "conservative run must not halt for this scenario; \
         halt_reason = {:?}",
        report_cons.halt_reason
    );
    assert!(
        !report_cons.execution_blocked,
        "conservative run must not block execution (integrity should pass on clean consecutive bars)"
    );

    let test_equity = report_test.equity_curve.last().unwrap().1;
    let cons_equity = report_cons.equity_curve.last().unwrap().1;

    // Combined stress effect: conservative final equity must be strictly lower.
    assert!(
        cons_equity < test_equity,
        "conservative final equity ({} micros) must be < test final equity ({} micros) — \
         promotion config must produce measurably more conservative outcomes",
        cons_equity,
        test_equity,
    );
}
