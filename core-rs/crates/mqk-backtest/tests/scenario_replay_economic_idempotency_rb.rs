//! Replay-backtest order/fill idempotency, proven on economic state.
//!
//! One logical order identity must never receive duplicate economic
//! execution. Prices move between entry and exit so a duplicated fill would
//! change realized P&L (a flat-price round trip would hide it). Expected
//! economics are hard-coded, not derived from the run under test.
//!
//! The backtest fill model is atomic: an order is filled whole or not at all
//! (see `BacktestEngine::resolve_one_pending_order`), so there is no
//! partial-fill progression to replay; `assert_atomic_single_fill` pins that.

use std::collections::{BTreeMap, HashSet};

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEngine, BacktestError, BacktestReport, OrderStatus,
    ReplaySemanticSpec, ResearchOosReplayStrategy,
};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

const DAY: i64 = 86_400;
const USD: i64 = 1_000_000;

fn semantic() -> ReplaySemanticSpec {
    ReplaySemanticSpec {
        replay_protocol_version: "research_oos_replay_bundle_v1".to_string(),
        strategy_id: "rb_idem_strategy_v1".to_string(),
        feature_columns: vec!["rb_xs_rank".to_string()],
        feature_transform: "cross_sectional_percentile_rank_rerank_of_authenticated_feature_v1"
            .to_string(),
        direction_policy: "cross_sectional_rank_long_only_v1".to_string(),
        rank_side_count: 1,
        long_only: true,
        borrow_model: None,
        max_gross_exposure: 1.0,
        timeframe: "1D".to_string(),
        equity_usd: 100_000.0,
        max_target_qty: None,
        max_position_notional_usd: None,
        trial_id: "rb-idem-trial-0001".to_string(),
    }
}

fn daily_config() -> BacktestConfig {
    BacktestConfig {
        timeframe_secs: DAY,
        ..BacktestConfig::test_defaults()
    }
}

fn flat(symbol: &str, day: i64, price_usd: i64) -> BacktestBar {
    let p = price_usd * USD;
    BacktestBar::new(symbol, DAY * day, p, p, p, p, 1_000)
}

/// Days 1..=4. `traded` symbols follow their given close path; `extras`
/// extra zero-target symbols add physical rows to every same-`end_ts` batch.
fn bars(traded: &[(&str, [i64; 4])], extras: usize) -> Vec<BacktestBar> {
    let mut out = Vec::new();
    for day in 1..=4i64 {
        for (sym, path) in traded {
            out.push(flat(sym, day, path[(day - 1) as usize]));
        }
        for k in 0..extras {
            out.push(flat(&format!("ZZ{k}"), day, 10));
        }
    }
    out
}

/// Day 1: enter `entries`; day 2 and day 4: no scheduled decision (carry);
/// day 3: explicit complete-target flatten of every traded symbol.
fn schedule(entries: &[(&str, i64)], extras: usize) -> BTreeMap<i64, Vec<TargetPosition>> {
    let zero_extras = |mut v: Vec<TargetPosition>| {
        for k in 0..extras {
            v.push(TargetPosition::whole(&format!("ZZ{k}"), 0));
        }
        v
    };
    let mut s = BTreeMap::new();
    s.insert(
        DAY,
        zero_extras(
            entries
                .iter()
                .map(|(sym, q)| TargetPosition::whole(*sym, *q))
                .collect(),
        ),
    );
    s.insert(
        3 * DAY,
        zero_extras(
            entries
                .iter()
                .map(|(sym, _)| TargetPosition::whole(*sym, 0))
                .collect(),
        ),
    );
    s
}

fn run_replay(
    bars: &[BacktestBar],
    schedule: BTreeMap<i64, Vec<TargetPosition>>,
) -> Result<BacktestReport, BacktestError> {
    let strategy = ResearchOosReplayStrategy::new(semantic(), schedule, bars);
    let mut engine = BacktestEngine::new(daily_config());
    engine.add_strategy(Box::new(strategy)).unwrap();
    engine.run(bars)
}

/// Fill-model invariants: unique order/fill identity; every `Filled` order has
/// exactly one fill of exactly its own quantity; every fill belongs to exactly
/// one `Filled` order.
fn assert_atomic_single_fill(report: &BacktestReport) {
    let fill_orders: HashSet<_> = report.fills.iter().map(|f| f.order_id).collect();
    let fill_ids: HashSet<_> = report.fills.iter().map(|f| f.fill_id).collect();
    assert_eq!(
        fill_orders.len(),
        report.fills.len(),
        "order_id fills twice"
    );
    assert_eq!(fill_ids.len(), report.fills.len(), "fill_id repeated");

    let filled: Vec<_> = report
        .orders
        .iter()
        .filter(|o| o.status == OrderStatus::Filled)
        .collect();
    let filled_ids: HashSet<_> = filled.iter().map(|o| o.order_id).collect();
    assert_eq!(
        filled_ids.len(),
        filled.len(),
        "order_id logged Filled twice"
    );
    assert_eq!(filled_ids, fill_orders, "Filled orders and fills disagree");
    for o in filled {
        let f = report
            .fills
            .iter()
            .find(|f| f.order_id == o.order_id)
            .unwrap();
        assert_eq!(f.inner.qty.to_whole_units_checked(), Some(o.qty));
    }
}

// Expected economics, single traded symbol AAA 100/110/120/130:
//   BUY 10 @ day-2 bar (110), SELL 10 @ day-4 bar (130) => +200 USD.
const AAA_PATH: [i64; 4] = [100, 110, 120, 130];
const BBB_PATH: [i64; 4] = [50, 55, 60, 65];
const INITIAL: i64 = 100_000 * USD;

/// RB-IDEM-01/02: extra physical rows per same-`end_ts` batch (K = 0, 1, 4,
/// including the carried day-2 batch while the position is held) must not
/// change the economics of the one real decision.
#[test]
fn rb01_02_extra_batch_rows_never_multiply_economics() {
    for extras in [0usize, 1, 4] {
        let b = bars(&[("AAA", AAA_PATH)], extras);
        let report = run_replay(&b, schedule(&[("AAA", 10)], extras))
            .unwrap_or_else(|e| panic!("extras={extras}: {e}"));

        assert_atomic_single_fill(&report);
        assert_eq!(
            report.orders.len(),
            2,
            "extras={extras}: {:?}",
            report.orders
        );
        assert_eq!(report.fills.len(), 2, "extras={extras}");
        let px: Vec<i64> = report.fills.iter().map(|f| f.inner.price_micros).collect();
        assert_eq!(px, vec![110 * USD, 130 * USD], "extras={extras}");
        assert_eq!(
            report.equity_curve.last().map(|(_, e)| *e),
            Some(INITIAL + 200 * USD),
            "extras={extras}"
        );
    }
}

/// RB-IDEM-03: genuinely distinct orders execute independently.
#[test]
fn rb03_distinct_orders_execute_independently() {
    let b = bars(&[("AAA", AAA_PATH), ("BBB", BBB_PATH)], 0);
    let report = run_replay(&b, schedule(&[("AAA", 10), ("BBB", 20)], 0)).unwrap();

    assert_atomic_single_fill(&report);
    assert_eq!(report.orders.len(), 4, "{:?}", report.orders);
    assert_eq!(report.fills.len(), 4);
    // AAA: 10 * (130 - 110) = 200 USD; BBB: 20 * (65 - 55) = 200 USD.
    assert_eq!(
        report.equity_curve.last().map(|(_, e)| *e),
        Some(INITIAL + 400 * USD)
    );
}

/// RB-IDEM-07: a fresh replay of identical inputs reproduces the identical
/// report (fills, order ids, equity curve).
#[test]
fn rb07_rerun_reproduces_identical_report() {
    let run = || {
        let b = bars(&[("AAA", AAA_PATH), ("BBB", BBB_PATH)], 2);
        run_replay(&b, schedule(&[("AAA", 10), ("BBB", 20)], 2)).unwrap()
    };
    let (first, second) = (run(), run());
    assert!(!first.fills.is_empty());
    assert_eq!(first, second);
}

/// Scripted strategy re-emitting the same complete target on every physical
/// row of a same-`end_ts` batch (an "ordinary" strategy retrying one order).
struct RetryEveryRow;

impl Strategy for RetryEveryRow {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("rb_retry_every_row", 60)
    }
    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput::new(vec![TargetPosition::whole("AAA", 10)])
    }
}

/// RB-IDEM-01/02/06: the same logical order retried N times in one batch
/// yields the identical fail-closed refusal for every N -- never a report
/// carrying a second economic application.
#[test]
fn rb_retry_same_identity_n_times_fails_closed_identically() {
    let mut refusals = Vec::new();
    for n in [2usize, 3, 5] {
        // Same end_ts for every row => one batch of n physical rows.
        let row = |sym: &str| {
            BacktestBar::new(sym, 60, 100 * USD, 100 * USD, 100 * USD, 100 * USD, 1_000)
        };
        let mut b = vec![row("AAA")];
        b.extend((1..n).map(|k| row(&format!("ZZ{k}"))));
        let mut engine = BacktestEngine::new(BacktestConfig::test_defaults());
        engine.add_strategy(Box::new(RetryEveryRow)).unwrap();
        match engine.run(&b) {
            Err(e @ BacktestError::DuplicateOrderId { .. }) => refusals.push(e),
            other => panic!("n={n}: expected DuplicateOrderId, got {other:?}"),
        }
    }
    assert!(refusals.windows(2).all(|w| w[0] == w[1]), "{refusals:?}");
}
