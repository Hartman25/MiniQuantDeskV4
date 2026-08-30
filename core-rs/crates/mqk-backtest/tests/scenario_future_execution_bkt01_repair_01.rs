//! BKT-FUTURE-EXECUTION-01-REPAIR-01: closes two determinism/provenance
//! blockers found in independent review of BKT-FUTURE-EXECUTION-01.
//!
//! Blocker 1 -- same-timestamp multi-symbol execution order dependence:
//! when two already-pending orders for different symbols both become
//! eligible in the same timestamp batch, and available allocation-cap
//! capacity permits only one of them, the physical row order of that
//! batch's bars must never decide which one gets filled. Tests below prove
//! the accepted/rejected split (and every downstream fill/position/cash/
//! equity consequence) is identical across row-order permutations, both
//! when the cap binds and when it doesn't.
//!
//! Blocker 2 -- execution model must be part of replay identity: `run_id`
//! must encode *how* signals were turned into fills, not just
//! strategy/config/input-data/economics, so a same-bar-fill engine and a
//! future-target-symbol-bar-fill engine can never collide on identity for
//! otherwise-identical inputs.
//!
//! Also covers the mission's terminal-pending-status truthfulness audit:
//! a pending order left over because the run halted early must be labeled
//! differently from one left over because the dataset was genuinely
//! exhausted.

use mqk_backtest::{
    derive_input_data_hash, derive_run_id_with_economics, derive_run_id_with_execution_model,
    BacktestBar, BacktestConfig, BacktestEngine, BacktestInstrumentEconomics, BacktestOrderSide,
    OrderStatus, BACKTEST_EXECUTION_MODEL_ID,
};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn bar(symbol: &str, end_ts: i64, open: i64, high: i64, low: i64, close: i64) -> BacktestBar {
    BacktestBar::new(symbol, end_ts, open, high, low, close, 1_000)
}

fn flat_bar(symbol: &str, end_ts: i64, price: i64) -> BacktestBar {
    bar(symbol, end_ts, price, price, price, price)
}

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

/// Canonical (symbol -> (price, signal_ts, fill_ts, status)) view of a
/// report's fills, keyed so row-order permutations can be compared without
/// caring about `Vec` iteration order.
fn fills_by_symbol(
    report: &mqk_backtest::BacktestReport,
) -> BTreeMap<String, (i64, i64, i64)> {
    report
        .fills
        .iter()
        .map(|f| (f.symbol.clone(), (f.price_micros, f.signal_ts, f.fill_ts)))
        .collect()
}

/// Canonical (symbol -> status) view of the two *original* signal-time
/// orders (signal_ts == 940 in every fixture below), excluding any
/// incidental order the strategy's own restatement logic may also emit.
fn original_order_status_by_symbol(
    report: &mqk_backtest::BacktestReport,
) -> BTreeMap<String, OrderStatus> {
    report
        .orders
        .iter()
        .filter(|o| o.signal_ts == 940)
        .map(|o| (o.symbol.clone(), o.status.clone()))
        .collect()
}

// ---------------------------------------------------------------------------
// Blocker 1 -- cap-binding adversarial permutation
// ---------------------------------------------------------------------------

/// Two already-pending orders (AAPL, AMD; same `signal_ts`) become eligible
/// in the exact same fill-timestamp batch. Allocation-cap capacity permits
/// exactly one of them. Row order A lists AAPL's fill bar before AMD's;
/// row order B reverses only that pair (the signal batch is left
/// untouched, isolating the one variable under test). The accepted order,
/// the rejected order, every fill, and the resulting equity curve must be
/// byte-identical between A and B.
#[test]
fn cap_binding_same_timestamp_permutation_produces_identical_result() {
    let signal_aapl = bar("AAPL", 940, 100_000_000, 100_000_000, 100_000_000, 100_000_000);
    let signal_amd = bar("AMD", 940, 200_000_000, 200_000_000, 200_000_000, 200_000_000);
    // Notional at fill: AAPL 550 * 100_000_000 = 55_000_000_000.
    //                   AMD  275 * 200_000_000 = 55_000_000_000.
    // Symmetric so neither symbol has an intrinsic size advantage.
    let fill_aapl = bar("AAPL", 1_000, 100_000_000, 100_000_000, 100_000_000, 100_000_000);
    let fill_amd = bar("AMD", 1_000, 200_000_000, 200_000_000, 200_000_000, 200_000_000);

    let schedule = || {
        vec![
            (1, vec![TargetPosition::new("AAPL", 550)]),
            (2, vec![TargetPosition::new("AMD", 275)]),
        ]
    };

    let mut cfg = BacktestConfig::test_defaults();
    // Equity = 100_000_000_000 (default). 0.6x -> allowed gross exposure =
    // 60_000_000_000. Exactly one 55_000_000_000 fill fits; a second
    // 55_000_000_000 fill (next = 110_000_000_000) does not.
    cfg.max_gross_exposure_mult_micros = 600_000;

    // Ordering A: AAPL's fill bar precedes AMD's.
    let bars_a = [
        signal_aapl.clone(),
        signal_amd.clone(),
        fill_aapl.clone(),
        fill_amd.clone(),
    ];
    // Ordering B: only the fill-timestamp pair is reversed.
    let bars_b = [signal_aapl, signal_amd, fill_amd, fill_aapl];

    let mut engine_a = BacktestEngine::new(cfg.clone());
    engine_a.add_strategy(Box::new(TickScript::new(schedule()))).unwrap();
    let report_a = engine_a.run(&bars_a).unwrap();

    let mut engine_b = BacktestEngine::new(cfg);
    engine_b.add_strategy(Box::new(TickScript::new(schedule()))).unwrap();
    let report_b = engine_b.run(&bars_b).unwrap();

    // Exactly one of the two competing orders fills, in both orderings.
    assert_eq!(report_a.fills.len(), 1, "cap must bind: only one order fills (ordering A)");
    assert_eq!(report_b.fills.len(), 1, "cap must bind: only one order fills (ordering B)");

    // The accepted order, its price/timestamps, must be identical.
    assert_eq!(
        fills_by_symbol(&report_a),
        fills_by_symbol(&report_b),
        "the same symbol must win the scarce allocation-cap capacity regardless of row order"
    );

    // The rejected order (by symbol) must be identical.
    let statuses_a = original_order_status_by_symbol(&report_a);
    let statuses_b = original_order_status_by_symbol(&report_b);
    assert_eq!(
        statuses_a, statuses_b,
        "accepted/rejected split must be identical regardless of row order"
    );
    assert_eq!(
        statuses_a.values().filter(|s| **s == OrderStatus::Filled).count(),
        1
    );
    assert_eq!(
        statuses_a.values().filter(|s| **s == OrderStatus::Rejected).count(),
        1
    );

    // Zero commission, flat bars, worst-case-priced fill exactly at the
    // flat mark: the accepted fill has no equity impact, and the rejected
    // one never touches cash/positions at all -- final equity must be
    // identical (and equal to starting cash) in both orderings.
    assert_eq!(
        report_a.equity_curve.last(),
        report_b.equity_curve.last(),
        "final equity must be identical regardless of row order"
    );
    assert_eq!(
        report_a.equity_curve.last().map(|(_, eq)| *eq),
        Some(cfg_initial_cash()),
        "the single accepted fill is fee-free and priced exactly at the mark"
    );
}

fn cfg_initial_cash() -> i64 {
    BacktestConfig::test_defaults().initial_cash_micros
}

// ---------------------------------------------------------------------------
// Blocker 1 -- same fixture shape, cap does NOT bind: both must still fill
// identically regardless of row order (companion to the binding case above,
// and to the pre-existing `test11a` in scenario_future_execution_bkt01.rs).
// ---------------------------------------------------------------------------

#[test]
fn non_binding_cap_same_timestamp_permutation_both_fill_identically() {
    let signal_aapl = bar("AAPL", 940, 100_000_000, 100_000_000, 100_000_000, 100_000_000);
    let signal_amd = bar("AMD", 940, 200_000_000, 200_000_000, 200_000_000, 200_000_000);
    let fill_aapl = bar("AAPL", 1_000, 100_000_000, 100_000_000, 100_000_000, 100_000_000);
    let fill_amd = bar("AMD", 1_000, 200_000_000, 200_000_000, 200_000_000, 200_000_000);

    let schedule = || {
        vec![
            (1, vec![TargetPosition::new("AAPL", 550)]),
            (2, vec![TargetPosition::new("AMD", 275)]),
        ]
    };

    let bars_a = [
        signal_aapl.clone(),
        signal_amd.clone(),
        fill_aapl.clone(),
        fill_amd.clone(),
    ];
    let bars_b = [signal_aapl, signal_amd, fill_amd, fill_aapl];

    let mut engine_a = BacktestEngine::new(wide_cfg());
    engine_a.add_strategy(Box::new(TickScript::new(schedule()))).unwrap();
    let report_a = engine_a.run(&bars_a).unwrap();

    let mut engine_b = BacktestEngine::new(wide_cfg());
    engine_b.add_strategy(Box::new(TickScript::new(schedule()))).unwrap();
    let report_b = engine_b.run(&bars_b).unwrap();

    assert_eq!(report_a.fills.len(), 2, "generous cap: both orders fill (ordering A)");
    assert_eq!(report_b.fills.len(), 2, "generous cap: both orders fill (ordering B)");
    assert_eq!(fills_by_symbol(&report_a), fills_by_symbol(&report_b));
    assert_eq!(
        original_order_status_by_symbol(&report_a),
        original_order_status_by_symbol(&report_b),
    );
    assert_eq!(report_a.equity_curve.last(), report_b.equity_curve.last());
}

// ---------------------------------------------------------------------------
// Blocker 2 -- execution model is part of replay identity
// ---------------------------------------------------------------------------

fn econ_equity() -> BacktestInstrumentEconomics {
    BacktestInstrumentEconomics::equity()
}

fn econ_explicit_equity() -> BacktestInstrumentEconomics {
    // Explicit multiplier=1, no margin -- value-equal to `econ_equity()` but
    // constructed independently, to prove the collapsing rule is about the
    // *value*, not which call site produced it.
    BacktestInstrumentEconomics::new(1, None, None).unwrap()
}

fn econ_non_default() -> BacktestInstrumentEconomics {
    BacktestInstrumentEconomics::new(50, Some(1_000_000), Some(500_000)).unwrap()
}

/// Test 1 -- Identical inputs (including execution_model_id) -> identical run_id.
#[test]
fn identical_inputs_produce_identical_run_id_under_new_model() {
    let cfg = BacktestConfig::test_defaults();
    let config_id = cfg.config_id();
    let hash = derive_input_data_hash(&[]);
    let id1 =
        derive_run_id_with_execution_model("strat", &config_id, &hash, &econ_equity(), BACKTEST_EXECUTION_MODEL_ID);
    let id2 =
        derive_run_id_with_execution_model("strat", &config_id, &hash, &econ_equity(), BACKTEST_EXECUTION_MODEL_ID);
    assert_eq!(id1, id2, "identical inputs must yield identical run_id");
}

/// Test 2 -- Legacy (execution-model-unaware) identity and the new
/// execution-model-aware identity must differ for the exact same
/// strategy/config/input/economics -- otherwise a same-bar-fill engine and
/// this future-bar-fill engine could collide on run_id despite producing
/// different fills/equity/P&L.
#[test]
fn legacy_identity_and_new_execution_model_identity_differ_for_same_inputs() {
    let cfg = BacktestConfig::test_defaults();
    let config_id = cfg.config_id();
    let hash = derive_input_data_hash(&[]);

    let legacy_id = derive_run_id_with_economics("strat", &config_id, &hash, &econ_equity());
    let new_id = derive_run_id_with_execution_model(
        "strat",
        &config_id,
        &hash,
        &econ_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
    );

    assert_ne!(
        legacy_id, new_id,
        "execution-model-aware run_id must never collide with the legacy execution-model-unaware run_id"
    );
}

/// Test 3 -- Default equity and explicit (multiplier=1, no margin) economics
/// remain identity-equivalent under the new execution-model-aware function.
#[test]
fn default_and_explicit_equity_economics_remain_identity_equivalent_under_new_model() {
    let cfg = BacktestConfig::test_defaults();
    let config_id = cfg.config_id();
    let hash = derive_input_data_hash(&[]);

    let id_default = derive_run_id_with_execution_model(
        "strat",
        &config_id,
        &hash,
        &econ_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
    );
    let id_explicit = derive_run_id_with_execution_model(
        "strat",
        &config_id,
        &hash,
        &econ_explicit_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
    );

    assert_eq!(
        id_default, id_explicit,
        "default equity and explicit multiplier=1/no-margin must remain identity-equivalent"
    );
}

/// Test 4 -- Non-default multiplier/margin remains run-id-distinct under the
/// new execution-model-aware function.
#[test]
fn non_default_economics_remains_distinct_under_new_model() {
    let cfg = BacktestConfig::test_defaults();
    let config_id = cfg.config_id();
    let hash = derive_input_data_hash(&[]);

    let id_equity = derive_run_id_with_execution_model(
        "strat",
        &config_id,
        &hash,
        &econ_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
    );
    let id_non_default = derive_run_id_with_execution_model(
        "strat",
        &config_id,
        &hash,
        &econ_non_default(),
        BACKTEST_EXECUTION_MODEL_ID,
    );

    assert_ne!(
        id_equity, id_non_default,
        "non-default economics must still produce a distinct run_id"
    );
}

/// Test 5 -- Changing execution_model_id alone (all other inputs fixed)
/// changes replay identity.
#[test]
fn changing_execution_model_id_changes_replay_identity() {
    let cfg = BacktestConfig::test_defaults();
    let config_id = cfg.config_id();
    let hash = derive_input_data_hash(&[]);

    let id_a = derive_run_id_with_execution_model(
        "strat",
        &config_id,
        &hash,
        &econ_equity(),
        "future_target_symbol_bar_v1",
    );
    let id_b = derive_run_id_with_execution_model(
        "strat",
        &config_id,
        &hash,
        &econ_equity(),
        "some_other_execution_model_v2",
    );

    assert_ne!(
        id_a, id_b,
        "changing execution_model_id alone must change the derived run_id"
    );
}

/// Test 6 -- `BacktestReport` reports the exact execution model that
/// produced it, and the engine's real `run_id` matches what
/// `derive_run_id_with_semantic_identity` would independently compute from
/// the report's own fields.
///
/// BACKTEST-STRATEGY-SEMANTIC-RUN-IDENTITY-01: `BacktestEngine::run` moved
/// from the semantic-unaware `v4` derivation
/// (`derive_run_id_with_execution_model`) to the semantic-aware `v5`
/// derivation (`derive_run_id_with_semantic_identity`) -- see
/// `scenario_strategy_semantic_run_identity_01.rs` for the full negative
/// control suite on that change.
#[test]
fn report_carries_truthful_execution_model_and_run_id_matches_independent_derivation() {
    let bars = [flat_bar("AAPL", 1_000, 100_000_000)];
    let mut engine = BacktestEngine::new(BacktestConfig::test_defaults());
    engine.add_strategy(Box::new(TickScript::new(vec![]))).unwrap();
    let report = engine.run(&bars).unwrap();

    assert_eq!(
        report.execution_model_id, BACKTEST_EXECUTION_MODEL_ID,
        "report must truthfully carry the execution model that actually produced it"
    );

    let expected_run_id = mqk_backtest::derive_run_id_with_semantic_identity(
        &report.strategy_name,
        &report.config_id,
        &report.input_data_hash,
        &BacktestInstrumentEconomics::equity(),
        &report.execution_model_id,
        &report.strategy_semantic_fingerprint,
    );
    assert_eq!(
        report.run_id, expected_run_id,
        "engine's real run_id must match independent v5 derivation from the report's own fields"
    );
}

// ---------------------------------------------------------------------------
// Terminal pending-order status truthfulness (halt vs. end-of-data)
// ---------------------------------------------------------------------------

/// A pending order for a symbol that never gets a chance to resolve because
/// the run halts early (a *different* symbol's loss trips the daily loss
/// limit) must be labeled `CanceledOnHalt`, not `UnfilledEndOfData` -- the
/// run terminated early, it did not exhaust its input data with the order
/// still outstanding.
#[test]
fn pending_order_orphaned_by_early_halt_is_labeled_canceled_on_halt() {
    const M: i64 = 1_000_000;
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 5_000_000;
    cfg.initial_cash_micros = 10_000 * M;
    cfg.daily_loss_limit_micros = 500 * M;

    let bars = [
        bar("MSFT", 1_000, 300_000_000, 300_000_000, 300_000_000, 300_000_000), // tick1: MSFT signal (forever pending)
        flat_bar("AAPL", 1_000, 100 * M), // tick2: AAPL buy signal
        flat_bar("AAPL", 1_060, 100 * M), // tick3: AAPL fills @ $100
        flat_bar("AAPL", 1_120, 40 * M),  // tick4: crash -> daily loss halt
    ];
    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(TickScript::new(vec![
            (1, vec![TargetPosition::new("MSFT", 5)]),
            (2, vec![TargetPosition::new("AAPL", 10)]),
            (3, vec![TargetPosition::new("AAPL", 10)]), // hold once resolved
            (4, vec![TargetPosition::new("AAPL", 0)]),  // sell intent trips the halt
        ])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert!(report.halted, "daily loss limit must halt the run");

    let msft_order = report
        .orders
        .iter()
        .find(|o| o.symbol == "MSFT")
        .expect("MSFT's forever-pending order must still be recorded");
    assert_eq!(
        msft_order.status,
        OrderStatus::CanceledOnHalt,
        "a pending order orphaned by an early halt must not be reported as end-of-data"
    );
    assert_eq!(msft_order.side, BacktestOrderSide::Buy);
    assert_eq!(msft_order.qty, 5);
}

/// Contrast case: a pending order left over because the dataset genuinely
/// ran out (no halt at all) must still be labeled `UnfilledEndOfData`.
#[test]
fn pending_order_orphaned_by_end_of_data_without_halt_is_labeled_unfilled_end_of_data() {
    let bars = [flat_bar("AAPL", 1_000, 100_000_000)];
    let mut engine = BacktestEngine::new(wide_cfg());
    engine
        .add_strategy(Box::new(TickScript::new(vec![(
            1,
            vec![TargetPosition::new("AAPL", 5)],
        )])))
        .unwrap();
    let report = engine.run(&bars).unwrap();

    assert!(!report.halted, "this run must not halt");
    assert_eq!(report.orders.len(), 1);
    assert_eq!(report.orders[0].status, OrderStatus::UnfilledEndOfData);
}
