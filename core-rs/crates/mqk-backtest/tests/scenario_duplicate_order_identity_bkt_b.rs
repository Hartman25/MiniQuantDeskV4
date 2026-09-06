//! W06-BACKTEST-ORDER-IDENTITY-UNIQUENESS-01 (Patch B): a run-scoped
//! duplicate-order-identity fence. Independent review proved the same
//! deterministic `order_id`/`fill_id` pair could be produced twice within a
//! single run (e.g. an intermediate physical row of a same-`end_ts` batch
//! re-deriving the same `(signal_ts, symbol, side, intent_seq)` tuple as
//! that batch's own final row), with portfolio/economic state applied more
//! than once. `BacktestEngine::run` now fails closed with
//! `BacktestError::DuplicateOrderId` the moment a colliding identity is
//! about to be constructed -- before any risk evaluation, pending-queue
//! insertion, fill, or portfolio/economics mutation for that intent.

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEngine, BacktestError, BacktestFill, BacktestOrderSide,
    OrderStatus,
};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn bar(symbol: &str, end_ts: i64, open: i64, high: i64, low: i64, close: i64) -> BacktestBar {
    BacktestBar::new(symbol, end_ts, open, high, low, close, 1_000)
}

fn flat_bar(symbol: &str, end_ts: i64, price: i64) -> BacktestBar {
    bar(symbol, end_ts, price, price, price, price)
}

/// Emits the target list registered for the current physical tick (1-based
/// across the whole run, regardless of which symbol's row triggered it), or
/// an empty list if this tick has none scripted.
struct TickScript {
    schedule: Vec<(u64, Vec<TargetPosition>)>,
    tick: u64,
}

impl TickScript {
    fn new(schedule: Vec<(u64, Vec<TargetPosition>)>) -> Self {
        Self { schedule, tick: 0 }
    }
}

impl Strategy for TickScript {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("tick_script", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.tick += 1;
        let targets = self
            .schedule
            .iter()
            .find(|(t, _)| *t == self.tick)
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        StrategyOutput::new(targets)
    }
}

fn wide_cfg() -> BacktestConfig {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 5_000_000; // 5x — permissive, cap never binds
    cfg
}

// ---------------------------------------------------------------------------
// B1 — historical exact duplicate reproduction
// ---------------------------------------------------------------------------

/// B1: two physical rows of the SAME same-`end_ts` batch (an ordinary,
/// non-replay strategy that naively re-asserts the same complete target on
/// every physical row, ignorant of the "only the batch's final row is the
/// real decision" convention) independently derive the identical
/// `(signal_ts, symbol, side, intent_seq)` tuple -- reproducing, from first
/// principles and independent of `ResearchOosReplayStrategy`/Patch A, the
/// exact class of collision independent review found. Must fail closed
/// BEFORE the second occurrence can reach risk evaluation, pending-queue
/// insertion, or any fill/portfolio/economics application.
#[test]
fn b1_duplicate_order_identity_within_same_batch_fails_closed_before_second_application() {
    let bars = vec![flat_bar("AAA", 60, 100_000_000), flat_bar("BBB", 60, 50_000_000)];
    let script = TickScript::new(vec![
        (1, vec![TargetPosition::new("AAA", 10)]),
        (2, vec![TargetPosition::new("AAA", 10)]),
    ]);
    let mut engine = BacktestEngine::new(wide_cfg());
    engine.add_strategy(Box::new(script)).unwrap();
    let err = engine.run(&bars).unwrap_err();

    let expected_order_id = BacktestFill::make_order_id(60, "AAA", true, 0);
    assert_eq!(
        err,
        BacktestError::DuplicateOrderId {
            order_id: expected_order_id,
            signal_ts: 60,
            symbol: "AAA".to_string(),
            side: BacktestOrderSide::Buy,
            intent_seq: 0,
        }
    );
}

// ---------------------------------------------------------------------------
// B2 — a single legitimate order is unaffected by the fence
// ---------------------------------------------------------------------------

/// B2: an ordinary single order (re-asserted, unchanged, on the following
/// bar so no spurious second delta arises) still produces exactly one order
/// and one fill.
#[test]
fn b2_single_legitimate_order_produces_one_unchanged_fill() {
    let bars = vec![flat_bar("AAA", 60, 100_000_000), flat_bar("AAA", 120, 100_000_000)];
    let script = TickScript::new(vec![
        (1, vec![TargetPosition::new("AAA", 10)]),
        (2, vec![TargetPosition::new("AAA", 10)]), // same target reasserted -- zero delta
    ]);
    let mut engine = BacktestEngine::new(wide_cfg());
    engine.add_strategy(Box::new(script)).unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(report.orders.len(), 1, "{:?}", report.orders);
    assert_eq!(report.fills.len(), 1, "{:?}", report.fills);
    assert_eq!(report.orders[0].qty, 10);
    assert_eq!(report.fills[0].order_id, report.orders[0].order_id);
}

// ---------------------------------------------------------------------------
// B3-B6 — the identity FORMULA distinguishes each field independently
// ---------------------------------------------------------------------------

/// B3: different `signal_ts` -> distinct identity.
#[test]
fn b3_different_signal_ts_is_distinct() {
    assert_ne!(
        BacktestFill::make_order_id(60, "AAA", true, 0),
        BacktestFill::make_order_id(120, "AAA", true, 0),
    );
}

/// B4: different `symbol` -> distinct identity.
#[test]
fn b4_different_symbol_is_distinct() {
    assert_ne!(
        BacktestFill::make_order_id(60, "AAA", true, 0),
        BacktestFill::make_order_id(60, "BBB", true, 0),
    );
}

/// B5: different `side` -> distinct identity.
#[test]
fn b5_different_side_is_distinct() {
    assert_ne!(
        BacktestFill::make_order_id(60, "AAA", true, 0),
        BacktestFill::make_order_id(60, "AAA", false, 0),
    );
}

/// B6: different `intent_seq` -> distinct identity.
#[test]
fn b6_different_intent_seq_is_distinct() {
    assert_ne!(
        BacktestFill::make_order_id(60, "AAA", true, 0),
        BacktestFill::make_order_id(60, "AAA", true, 1),
    );
}

// ---------------------------------------------------------------------------
// B7 — qty is NOT identity-bearing: a differing qty must still collide
// ---------------------------------------------------------------------------

/// B7: two intents sharing every identity-bearing field (`signal_ts`,
/// `symbol`, `side`, `intent_seq`) but DIFFERENT `qty` must still collide
/// and fail closed -- the identity formula must never be silently widened
/// to include qty as a way of "avoiding" the collision.
#[test]
fn b7_differing_qty_with_identical_identity_fields_still_collides() {
    let bars = vec![flat_bar("AAA", 60, 100_000_000), flat_bar("BBB", 60, 50_000_000)];
    let script = TickScript::new(vec![
        (1, vec![TargetPosition::new("AAA", 10)]),
        (2, vec![TargetPosition::new("AAA", 15)]), // same identity fields, different qty
    ]);
    let mut engine = BacktestEngine::new(wide_cfg());
    engine.add_strategy(Box::new(script)).unwrap();
    let err = engine.run(&bars).unwrap_err();

    assert_eq!(
        err,
        BacktestError::DuplicateOrderId {
            order_id: BacktestFill::make_order_id(60, "AAA", true, 0),
            signal_ts: 60,
            symbol: "AAA".to_string(),
            side: BacktestOrderSide::Buy,
            intent_seq: 0,
        }
    );
}

// ---------------------------------------------------------------------------
// B8-B9 — whole-run uniqueness invariants on an ordinary, well-behaved run
// ---------------------------------------------------------------------------

/// Every timestamp here is a single-symbol, single-physical-row batch (AAA
/// and BBB never share an `end_ts`) -- deliberately sidesteps the
/// same-batch multi-row identity collision class entirely (see B1/B7 above,
/// and Patch A's docs on why an "ordinary" non-opted-in strategy cannot
/// safely reassert a nonzero-delta target on more than one physical row of
/// the same batch under the frozen `targets_to_order_intents` contract).
/// This fixture is only exercising the fence's absence of FALSE positives
/// across a genuinely valid multi-order, multi-symbol run.
fn ordinary_multi_day_bars_and_script() -> (Vec<BacktestBar>, TickScript) {
    let bars = vec![
        flat_bar("AAA", 60, 100_000_000),
        flat_bar("BBB", 120, 50_000_000),
        flat_bar("AAA", 180, 105_000_000),
        flat_bar("BBB", 240, 52_000_000),
    ];
    let script = TickScript::new(vec![
        (1, vec![TargetPosition::new("AAA", 10)]), // order1: BUY AAA 10 @ ts=60
        (2, vec![TargetPosition::new("BBB", 5)]),   // order2: BUY BBB 5 @ ts=120
        // ts=180: resolves+fills order1 (AAA bar, 180>60), then a new decision.
        (3, vec![TargetPosition::new("AAA", 4)]), // order3: SELL AAA 6 @ ts=180 (stays pending)
        // ts=240: resolves+fills order2 (BBB bar, 240>120), then a new decision.
        (4, vec![TargetPosition::new("BBB", 0)]), // order4: SELL BBB 5 @ ts=240 (stays pending)
    ]);
    (bars, script)
}

/// B8: a valid ordinary multi-day, multi-symbol backtest produces only
/// unique `fill_id` values.
#[test]
fn b8_ordinary_backtest_all_fill_ids_unique() {
    let (bars, script) = ordinary_multi_day_bars_and_script();
    let mut engine = BacktestEngine::new(wide_cfg());
    engine.add_strategy(Box::new(script)).unwrap();
    let report = engine.run(&bars).unwrap();

    assert!(!report.fills.is_empty());
    let unique: HashSet<_> = report.fills.iter().map(|f| f.fill_id).collect();
    assert_eq!(unique.len(), report.fills.len(), "every fill_id must be unique across the run");
}

/// B9: every `Filled` order's `order_id` corresponds to exactly one fill.
#[test]
fn b9_each_filled_order_has_exactly_one_corresponding_fill() {
    let (bars, script) = ordinary_multi_day_bars_and_script();
    let mut engine = BacktestEngine::new(wide_cfg());
    engine.add_strategy(Box::new(script)).unwrap();
    let report = engine.run(&bars).unwrap();

    let filled: Vec<_> = report.orders.iter().filter(|o| o.status == OrderStatus::Filled).collect();
    assert!(!filled.is_empty());
    for order in filled {
        let matching = report.fills.iter().filter(|f| f.order_id == order.order_id).count();
        assert_eq!(matching, 1, "order {:?} must have exactly one fill", order.order_id);
    }
}
