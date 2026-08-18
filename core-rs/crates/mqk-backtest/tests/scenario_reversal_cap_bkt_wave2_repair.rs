use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, BacktestReport, OrderStatus};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

/// P7B-REPAIR-03 (mission Section 3F / follow-up repair): a reversal order
/// (e.g. current +40, target -100, delta SELL 140) must be judged against
/// the gross exposure it will actually leave behind -- current exposure
/// minus the slice being closed, plus the new risk-increasing residual --
/// not against current-plus-new with the closed slice still counted. The
/// prior repair (P7B-REPAIR-02) correctly isolated the risk-increasing
/// residual's notional but still compared it against un-decremented current
/// gross exposure, which double-counts the exposure being closed and
/// spuriously rejects reversals that are actually within cap.
///
/// A scripted strategy indexed by call number: `on_bar` is invoked once per
/// physical bar row (regardless of symbol), so the script is a plain
/// per-call list of `(symbol, target_qty)` pairs, independent of which
/// symbol's row triggered that call. Each call's entry is the FULL desired
/// target set at that point, not a delta: `targets_to_order_intents` treats
/// any symbol omitted from a call's list as an explicit target of 0, so a
/// multi-symbol script must keep re-asserting every symbol that already
/// holds a filled position or it is implicitly flattened. A symbol whose
/// order is still pending (admitted but not yet filled) is safe to omit --
/// `current_qty` reflects only filled portfolio positions, so an omitted
/// still-pending symbol's delta is 0 either way.
struct ScriptedStrategy {
    script: Vec<Vec<(&'static str, i64)>>,
    call: usize,
}

impl ScriptedStrategy {
    fn new(script: Vec<Vec<(&'static str, i64)>>) -> Self {
        Self { script, call: 0 }
    }
}

impl Strategy for ScriptedStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("ScriptedStrategy", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        let targets = self.script.get(self.call).cloned().unwrap_or_default();
        self.call += 1;
        StrategyOutput::new(
            targets
                .into_iter()
                .map(|(sym, qty)| TargetPosition::new(sym, qty))
                .collect(),
        )
    }
}

/// `n` flat bars for `symbol` at a constant $100 OHLC -- no price drift, so
/// equity stays exactly at `initial_cash_micros` and every notional
/// computation in a test's hand-derived expectation is exact, not
/// approximate.
fn flat_bars(symbol: &str, n: usize) -> Vec<BacktestBar> {
    (0..n)
        .map(|i| {
            BacktestBar::new(
                symbol,
                1_700_000_060 + (i as i64) * 60,
                100_000_000,
                100_000_000,
                100_000_000,
                100_000_000,
                1000,
            )
        })
        .collect()
}

/// Runs a single-symbol (SPY) open-then-reversal scenario: bar 1 signals
/// `open_qty`, bar 2 signals `target_qty` (resolved at bar 3), under a gross
/// exposure cap of `mult_micros` (micros multiplier on $100k equity).
fn run_reversal_case(open_qty: i64, target_qty: i64, mult_micros: i64) -> BacktestReport {
    let bars = flat_bars("SPY", 3);

    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = mult_micros;

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(ScriptedStrategy::new(vec![
            vec![("SPY", open_qty)],
            vec![("SPY", target_qty)],
        ])))
        .unwrap();
    engine.run(&bars).unwrap()
}

fn assert_open_filled(report: &BacktestReport, open_qty: i64) {
    let expected_side = if open_qty >= 0 {
        mqk_backtest::BacktestOrderSide::Buy
    } else {
        mqk_backtest::BacktestOrderSide::Sell
    };
    let opens: Vec<_> = report
        .orders
        .iter()
        .filter(|o| o.qty == open_qty.abs() && o.side == expected_side && o.status == OrderStatus::Filled)
        .collect();
    assert_eq!(opens.len(), 1, "opening order must fill: {:?}", report.orders);
}

fn assert_reversal_status(report: &BacktestReport, delta_qty: i64, expected: OrderStatus) {
    let reversal: Vec<_> = report.orders.iter().filter(|o| o.qty == delta_qty).collect();
    assert_eq!(
        reversal.len(),
        1,
        "expected exactly one reversal order of qty {delta_qty}: {:?}",
        report.orders
    );
    assert_eq!(reversal[0].status, expected);
}

// ---------------------------------------------------------------------------
// Items 1-3: long (+40) -> short (-100), delta SELL 140.
//
// current gross ($4,000) - closing ($4,000) + residual ($10,000) = $10,000
// prospective gross. Cap set via `mult_micros` on $100k equity:
//   mult=110,000 (0.11x) -> cap $11,000 (above $10,000: allow)
//   mult=100,000 (0.10x) -> cap $10,000 (exactly $10,000: allow, boundary)
//   mult= 90,000 (0.09x) -> cap  $9,000 (below $10,000: reject)
// ---------------------------------------------------------------------------

#[test]
fn item1_long_to_short_below_cap_allows() {
    let report = run_reversal_case(40, -100, 110_000);
    assert_open_filled(&report, 40);
    assert_reversal_status(&report, 140, OrderStatus::Filled);
    assert_eq!(report.fills.len(), 2);
}

/// This is the exact scenario the pre-repair test wrongly asserted as
/// Rejected ($4,000 + $10,000 = $14,000). The correct prospective gross is
/// $4,000 - $4,000 + $10,000 = $10,000, which sits exactly at the cap and
/// must be allowed.
#[test]
fn item2_long_to_short_exactly_cap_allows() {
    let report = run_reversal_case(40, -100, 100_000);
    assert_open_filled(&report, 40);
    assert_reversal_status(&report, 140, OrderStatus::Filled);
    assert_eq!(report.fills.len(), 2);
}

#[test]
fn item3_long_to_short_above_cap_rejects() {
    let report = run_reversal_case(40, -100, 90_000);
    assert_open_filled(&report, 40);
    assert_reversal_status(&report, 140, OrderStatus::Rejected);
    assert_eq!(report.fills.len(), 1);
}

// ---------------------------------------------------------------------------
// Items 4-6: mirror -- short (-40) -> long (+100), delta BUY 140.
// Identical notional arithmetic (mark and fill price both $100), so the same
// three `mult_micros` boundary values apply.
// ---------------------------------------------------------------------------

#[test]
fn item4_short_to_long_below_cap_allows() {
    let report = run_reversal_case(-40, 100, 110_000);
    assert_open_filled(&report, -40);
    assert_reversal_status(&report, 140, OrderStatus::Filled);
    assert_eq!(report.fills.len(), 2);
}

#[test]
fn item5_short_to_long_exactly_cap_allows() {
    let report = run_reversal_case(-40, 100, 100_000);
    assert_open_filled(&report, -40);
    assert_reversal_status(&report, 140, OrderStatus::Filled);
    assert_eq!(report.fills.len(), 2);
}

#[test]
fn item6_short_to_long_above_cap_rejects() {
    let report = run_reversal_case(-40, 100, 90_000);
    assert_open_filled(&report, -40);
    assert_reversal_status(&report, 140, OrderStatus::Rejected);
    assert_eq!(report.fills.len(), 1);
}

// ---------------------------------------------------------------------------
// Item 7: pure reduction is unaffected -- it never enters the cap check at
// all (`is_risk_reducing` short-circuits it), so it must still fill even
// under a cap so tight that treating it as a fresh $4,000 risk-increasing
// order (the pre-P7B-REPAIR-02 bug) would have rejected it.
// ---------------------------------------------------------------------------

#[test]
fn item7_pure_reduction_bypasses_cap_unchanged() {
    let bars = flat_bars("SPY", 3);
    let mut cfg = BacktestConfig::test_defaults();
    // Exactly enough to admit the $4,000 open; a naive re-check of the full
    // $4,000 closing notional against the post-open $4,000 existing gross
    // ($8,000) would blow this cap if reduction weren't bypassed entirely.
    cfg.max_gross_exposure_mult_micros = 40_000; // 0.04x -> $4,000 cap

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(ScriptedStrategy::new(vec![
            vec![("SPY", 40)],
            vec![("SPY", 0)],
        ])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert_open_filled(&report, 40);
    let close: Vec<_> = report
        .orders
        .iter()
        .filter(|o| o.qty == 40 && o.side == mqk_backtest::BacktestOrderSide::Sell)
        .collect();
    assert_eq!(close.len(), 1, "expected exactly one closing SELL 40 order: {:?}", report.orders);
    assert_eq!(
        close[0].status,
        OrderStatus::Filled,
        "pure reduction must bypass the allocation cap entirely"
    );
    assert_eq!(report.fills.len(), 2);
}

// ---------------------------------------------------------------------------
// Item 8: pure increase from flat is unaffected by this repair -- reducing
// qty is always zero when there is no existing opposing position, so
// prospective gross collapses to the pre-repair `0 + residual` formula.
// ---------------------------------------------------------------------------

#[test]
fn item8_pure_increase_exactly_cap_allows() {
    let bars = flat_bars("SPY", 2);
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 40_000; // 0.04x -> $4,000 cap, exact fit

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(ScriptedStrategy::new(vec![vec![("SPY", 40)]])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert_open_filled(&report, 40);
    assert_eq!(report.fills.len(), 1);
}

#[test]
fn item8_pure_increase_above_cap_rejects() {
    let bars = flat_bars("SPY", 2);
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 39_000; // 0.039x -> $3,900 cap, just short

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(ScriptedStrategy::new(vec![vec![("SPY", 40)]])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    let opens: Vec<_> = report.orders.iter().filter(|o| o.qty == 40).collect();
    assert_eq!(opens.len(), 1);
    assert_eq!(opens[0].status, OrderStatus::Rejected);
    assert_eq!(report.fills.len(), 0);
}

// ---------------------------------------------------------------------------
// Item 9: an unrelated symbol's exposure must remain fully counted -- the
// fix only nets out the reversed/reducing symbol's own closing slice, never
// another symbol's contribution to gross exposure.
//
// AAPL opens 20 @ $100 ($2,000, held static) while SPY opens fresh 100
// shares ($10,000, a pure increase, not a reversal). Correct prospective
// gross = $2,000 (AAPL, unaffected by SPY's order) + $10,000 (SPY) =
// $12,000. Cap set to $11,000: correct behavior rejects the SPY order; a
// bug that dropped AAPL's exposure from the base would see only $10,000 and
// incorrectly allow it.
// ---------------------------------------------------------------------------

#[test]
fn item9_unrelated_symbol_exposure_remains_counted() {
    let bars = vec![
        BacktestBar::new("AAPL", 1_700_000_060, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1000),
        BacktestBar::new("SPY", 1_700_000_060, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1000),
        BacktestBar::new("AAPL", 1_700_000_120, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1000),
        BacktestBar::new("SPY", 1_700_000_120, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1000),
        BacktestBar::new("SPY", 1_700_000_180, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1000),
    ];

    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 110_000; // 0.11x -> $11,000 cap

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(ScriptedStrategy::new(vec![
            vec![("AAPL", 20)],           // call1 (AAPL@60): open AAPL signal
            vec![],                       // call2 (SPY@60): AAPL still pending, safe to omit
            vec![("AAPL", 20)],           // call3 (AAPL@120): AAPL just filled -- restate to avoid an implicit flatten
            vec![("AAPL", 20), ("SPY", 100)], // call4 (SPY@120): keep AAPL, open SPY signal
            vec![("AAPL", 20)],           // call5 (SPY@180): SPY rejects in this batch -- omit it (already 0), keep AAPL
        ])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    let aapl_open: Vec<_> = report.orders.iter().filter(|o| o.symbol == "AAPL").collect();
    assert_eq!(aapl_open.len(), 1, "expected exactly one AAPL order: {:?}", report.orders);
    assert_eq!(aapl_open[0].status, OrderStatus::Filled, "AAPL open must fill: {:?}", aapl_open);

    let spy_open: Vec<_> = report.orders.iter().filter(|o| o.symbol == "SPY").collect();
    assert_eq!(spy_open.len(), 1, "expected exactly one SPY order: {:?}", report.orders);
    assert_eq!(
        spy_open[0].status,
        OrderStatus::Rejected,
        "SPY must be rejected once AAPL's static $2,000 exposure is correctly counted \
         against the $11,000 cap alongside SPY's own $10,000 residual"
    );

    assert_eq!(report.fills.len(), 1, "only AAPL's fill should have landed");
}

// ---------------------------------------------------------------------------
// Item 10: multi-symbol reversal -- prospective gross must equal the other
// symbol's untouched gross plus the reversed symbol's own prospective final
// gross (current minus its own closing slice, plus its own residual). AAPL
// opens 20 @ $100 ($2,000, static) while SPY opens +40 then reverses to
// -100 (SPY's own prospective final gross = $4,000 - $4,000 + $10,000 =
// $10,000, per items 1-3). Correct total = $2,000 + $10,000 = $12,000.
// ---------------------------------------------------------------------------

/// `expect_reversal_filled` controls only the FINAL call's restatement of
/// SPY's target: once the reversal batch resolves, the strategy must
/// restate SPY at whatever it actually ended up holding (-100 if filled, or
/// back at its pre-reversal 40 if rejected) to avoid either duplicating the
/// signal or implicitly flattening it -- see the `ScriptedStrategy` doc
/// comment. The test author picks `mult_micros` to know this outcome in
/// advance; the strategy itself has no way to observe it.
fn run_multi_symbol_reversal_case(mult_micros: i64, expect_reversal_filled: bool) -> BacktestReport {
    let bars = vec![
        BacktestBar::new("AAPL", 1_700_000_060, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1000),
        BacktestBar::new("SPY", 1_700_000_060, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1000),
        BacktestBar::new("AAPL", 1_700_000_120, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1000),
        BacktestBar::new("SPY", 1_700_000_120, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1000),
        BacktestBar::new("SPY", 1_700_000_180, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1000),
    ];

    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = mult_micros;

    let spy_final = if expect_reversal_filled { -100 } else { 40 };

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(ScriptedStrategy::new(vec![
            vec![("AAPL", 20)],                    // call1 (AAPL@60): open AAPL signal
            vec![("SPY", 40)],                      // call2 (SPY@60): AAPL still pending, safe to omit; open SPY signal
            vec![("AAPL", 20), ("SPY", 40)],        // call3 (AAPL@120): both opens just filled -- restate both
            vec![("AAPL", 20), ("SPY", -100)],      // call4 (SPY@120): keep AAPL, SPY reversal signal
            vec![("AAPL", 20), ("SPY", spy_final)], // call5 (SPY@180): reversal resolves -- restate its actual outcome
        ])))
        .unwrap();
    engine.run(&bars).unwrap()
}

#[test]
fn item10_multi_symbol_reversal_exactly_cap_allows() {
    let report = run_multi_symbol_reversal_case(120_000, true); // 0.12x -> $12,000 cap

    let aapl_open: Vec<_> = report.orders.iter().filter(|o| o.symbol == "AAPL").collect();
    assert_eq!(aapl_open.len(), 1, "expected exactly one AAPL order: {:?}", report.orders);
    assert_eq!(aapl_open[0].status, OrderStatus::Filled);

    let spy_open: Vec<_> = report.orders.iter().filter(|o| o.symbol == "SPY" && o.qty == 40).collect();
    assert_eq!(spy_open.len(), 1);
    assert_eq!(spy_open[0].status, OrderStatus::Filled);

    let spy_reversal: Vec<_> = report.orders.iter().filter(|o| o.symbol == "SPY" && o.qty == 140).collect();
    assert_eq!(spy_reversal.len(), 1, "expected exactly one SPY reversal order: {:?}", report.orders);
    assert_eq!(
        spy_reversal[0].status,
        OrderStatus::Filled,
        "AAPL $2,000 + SPY prospective $10,000 = $12,000 sits exactly at the $12,000 cap"
    );

    assert_eq!(report.fills.len(), 3);
}

#[test]
fn item10_multi_symbol_reversal_above_cap_rejects() {
    let report = run_multi_symbol_reversal_case(110_000, false); // 0.11x -> $11,000 cap

    let aapl_open: Vec<_> = report.orders.iter().filter(|o| o.symbol == "AAPL").collect();
    assert_eq!(aapl_open.len(), 1, "expected exactly one AAPL order: {:?}", report.orders);
    assert_eq!(aapl_open[0].status, OrderStatus::Filled);

    let spy_open: Vec<_> = report.orders.iter().filter(|o| o.symbol == "SPY" && o.qty == 40).collect();
    assert_eq!(spy_open.len(), 1);
    assert_eq!(spy_open[0].status, OrderStatus::Filled);

    let spy_reversal: Vec<_> = report.orders.iter().filter(|o| o.symbol == "SPY" && o.qty == 140).collect();
    assert_eq!(spy_reversal.len(), 1, "expected exactly one SPY reversal order: {:?}", report.orders);
    assert_eq!(
        spy_reversal[0].status,
        OrderStatus::Rejected,
        "AAPL $2,000 + SPY prospective $10,000 = $12,000 breaches the $11,000 cap -- \
         a bug that ignored AAPL's static exposure would see only SPY's $10,000 and \
         incorrectly allow this"
    );

    // AAPL fill + SPY open fill only; the rejected reversal contributes none.
    assert_eq!(report.fills.len(), 2);
}

// ---------------------------------------------------------------------------
// Negative control retained from the original repair: a reversal whose
// residual fits comfortably under the cap must still execute in full.
// ---------------------------------------------------------------------------

#[test]
fn reversal_with_small_residual_still_fills() {
    let report = run_reversal_case(40, -100, 1_000_000); // 1.0x -> $100,000 cap
    assert_open_filled(&report, 40);
    assert_reversal_status(&report, 140, OrderStatus::Filled);
    assert_eq!(report.fills.len(), 2);
}
