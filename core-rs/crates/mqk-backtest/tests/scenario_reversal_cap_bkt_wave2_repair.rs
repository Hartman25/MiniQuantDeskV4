use mqk_backtest::{BacktestBar, BacktestConfig, BacktestEngine, OrderStatus};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

/// FINAL-WAVE-2-REPAIR mission Section 3F: a reversal order (current +40,
/// target -100, delta SELL 140) must not bypass the fill-time allocation
/// cap just because its first 40 shares close an existing long. Only the
/// risk-INCREASING residual (100 shares establishing new short risk) may be
/// subject to the gross-exposure cap check.
struct LongThenReverseToShort {
    bar_idx: u64,
}

impl LongThenReverseToShort {
    fn new() -> Self {
        Self { bar_idx: 0 }
    }
}

impl Strategy for LongThenReverseToShort {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("LongThenReverseToShort", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            // bar 1: open +40 (signal only; fills at bar 2).
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", 40)]),
            // bar 2: after the +40 fill lands, reverse to -100 (signal only;
            // fills at bar 3, once the +40 position is actually settled).
            2 => StrategyOutput::new(vec![TargetPosition::new("SPY", -100)]),
            _ => StrategyOutput::new(vec![]),
        }
    }
}

struct ShortThenReverseToLong {
    bar_idx: u64,
}

impl ShortThenReverseToLong {
    fn new() -> Self {
        Self { bar_idx: 0 }
    }
}

impl Strategy for ShortThenReverseToLong {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("ShortThenReverseToLong", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        match self.bar_idx {
            1 => StrategyOutput::new(vec![TargetPosition::new("SPY", -40)]),
            2 => StrategyOutput::new(vec![TargetPosition::new("SPY", 100)]),
            _ => StrategyOutput::new(vec![]),
        }
    }
}

fn flat_bars(n: usize) -> Vec<BacktestBar> {
    (0..n)
        .map(|i| {
            BacktestBar::new(
                "SPY",
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

/// current=+40, target=-100: closing the 40-share long is safe, but the
/// residual 100-share NEW short would breach a cap that only has $10,000 of
/// headroom on $100,000 equity at a $100 flat price. The entire SELL 140
/// order must be rejected -- it must not partially bypass the cap just
/// because its first 40 shares reduce existing risk.
#[test]
fn reversal_long_to_short_residual_cannot_bypass_gross_cap() {
    // Exactly 3 bars: bar 1 signals +40 (fills at bar 2), bar 2 signals the
    // reversal to -100 (resolved at bar 3). Bar 3's own on_bar call emits an
    // implicit flatten-to-0 intent too (StrategyOutput omitting a symbol
    // means target=0 under targets_to_order_intents, and the rejected
    // reversal leaves the position at +40, not flat) -- with no bar 4 to
    // resolve it, that third order surfaces as `UnfilledEndOfData` rather
    // than disappearing, so filters below key on status, not qty alone, to
    // stay unambiguous against it.
    let bars = flat_bars(3);

    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 100_000; // 0.10x -> $10,000 allowed on $100k equity

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(LongThenReverseToShort::new()))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    // Order 1 (+40 open) must fill: $4,000 notional fits under $10,000.
    let opens: Vec<_> = report
        .orders
        .iter()
        .filter(|o| o.qty == 40 && o.status == OrderStatus::Filled)
        .collect();
    assert_eq!(opens.len(), 1);

    // Order 2 (SELL 140, reversal) must be REJECTED in full: the
    // risk-increasing residual (100 shares * $100 = $10,000) plus the
    // existing $4,000 long exposure ($14,000) breaches the $10,000 cap.
    let reversal: Vec<_> = report
        .orders
        .iter()
        .filter(|o| o.qty == 140)
        .collect();
    assert_eq!(reversal.len(), 1, "expected exactly one SELL 140 reversal order");
    assert_eq!(
        reversal[0].status,
        OrderStatus::Rejected,
        "reversal residual must be capacity-checked -- it must not bypass the cap merely \
         because its first 40 shares close an existing long"
    );

    // The rejected reversal must not have mutated the position: still +40,
    // not -100. Confirmed indirectly -- exactly one fill (the +40 open) and
    // no fill for the 140-share order.
    assert_eq!(report.fills.len(), 1);
    assert_eq!(report.fills[0].inner.qty, 40);
}

/// Mirror case: current=-40, target=+100 (BUY 140). Covering the 40-share
/// short is safe; the residual 100-share NEW long would breach the same
/// $10,000 cap and must reject the whole order.
#[test]
fn reversal_short_to_long_residual_cannot_bypass_gross_cap() {
    let bars = flat_bars(3);

    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 100_000; // 0.10x -> $10,000 allowed

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(ShortThenReverseToLong::new()))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    let opens: Vec<_> = report
        .orders
        .iter()
        .filter(|o| o.qty == 40 && o.status == OrderStatus::Filled)
        .collect();
    assert_eq!(opens.len(), 1);

    let reversal: Vec<_> = report.orders.iter().filter(|o| o.qty == 140).collect();
    assert_eq!(reversal.len(), 1, "expected exactly one BUY 140 reversal order");
    assert_eq!(
        reversal[0].status,
        OrderStatus::Rejected,
        "reversal residual must be capacity-checked -- it must not bypass the cap merely \
         because its first 40 shares close an existing short"
    );

    assert_eq!(report.fills.len(), 1);
    assert_eq!(report.fills[0].inner.qty, 40);
}

/// Negative control: a reversal whose residual fits comfortably under the
/// cap must still execute in full (proves the fix isn't simply rejecting
/// all reversals).
#[test]
fn reversal_with_small_residual_still_fills() {
    let bars = flat_bars(3);

    let mut cfg = BacktestConfig::test_defaults();
    // 1.0x -> $100,000 allowed: residual of $10,000 fits easily.
    cfg.max_gross_exposure_mult_micros = 1_000_000;

    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(LongThenReverseToShort::new()))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.fills.len(), 2);
    assert_eq!(
        report.orders.iter().filter(|o| o.status == OrderStatus::Filled).count(),
        2
    );
}
