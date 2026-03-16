//! BKT-05P: Strategy identity proof.
//!
//! Proves that `BacktestReport` carries correct strategy identity fields:
//!
//! - I1: `strategy_name` matches the registered strategy's `StrategySpec::name`
//! - I2: `run_id` is non-nil
//! - I3: `run_id` is stable across identical replays (deterministic)
//! - I4: Different strategy names produce different `run_id` values
//! - I5: Same strategy name + different config → different `run_id`
//! - I6: No strategy registered → `strategy_name` is empty string

use mqk_backtest::{derive_run_id, BacktestBar, BacktestConfig, BacktestEngine, CommissionModel};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn bar(ts: i64) -> BacktestBar {
    BacktestBar::new("SPY", ts, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1_000)
}

struct NamedStrategy(&'static str);

impl Strategy for NamedStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(self.0, 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput::new(vec![TargetPosition::new("SPY", 1)])
    }
}

fn run_with_name(name: &'static str) -> mqk_backtest::BacktestReport {
    let bars = vec![bar(1_700_000_060)];
    let mut cfg = BacktestConfig::test_defaults();
    cfg.commission = CommissionModel::ZERO;
    cfg.max_gross_exposure_mult_micros = 5_000_000;
    let mut engine = BacktestEngine::new(cfg);
    engine.add_strategy(Box::new(NamedStrategy(name))).unwrap();
    engine.run(&bars).unwrap()
}

// ---------------------------------------------------------------------------
// I1: strategy_name matches spec name
// ---------------------------------------------------------------------------

#[test]
fn strategy_name_matches_spec() {
    let report = run_with_name("my_scalper_v1");
    assert_eq!(
        report.strategy_name, "my_scalper_v1",
        "strategy_name must match StrategySpec::name"
    );
}

// ---------------------------------------------------------------------------
// I2: run_id is non-nil
// ---------------------------------------------------------------------------

#[test]
fn run_id_is_not_nil() {
    let report = run_with_name("my_scalper_v1");
    assert_ne!(
        report.run_id,
        Uuid::nil(),
        "run_id must not be nil"
    );
}

// ---------------------------------------------------------------------------
// I3: run_id is stable across identical replays
// ---------------------------------------------------------------------------

#[test]
fn run_id_is_stable_across_replays() {
    let r1 = run_with_name("my_scalper_v1");
    let r2 = run_with_name("my_scalper_v1");
    assert_eq!(
        r1.run_id, r2.run_id,
        "run_id must be identical across identical replays"
    );
    assert_eq!(r1.strategy_name, r2.strategy_name);
}

// ---------------------------------------------------------------------------
// I4: Different strategy names → different run_ids
// ---------------------------------------------------------------------------

#[test]
fn different_strategy_names_produce_different_run_ids() {
    let r1 = run_with_name("strategy_alpha");
    let r2 = run_with_name("strategy_beta");
    assert_ne!(
        r1.run_id, r2.run_id,
        "different strategy names must produce different run_ids"
    );
}

// ---------------------------------------------------------------------------
// I5: Same strategy name + different config → different run_id
// ---------------------------------------------------------------------------

#[test]
fn different_config_produces_different_run_id() {
    let bars = vec![bar(1_700_000_060)];

    let mut cfg_a = BacktestConfig::test_defaults();
    cfg_a.commission = CommissionModel::ZERO;
    cfg_a.max_gross_exposure_mult_micros = 5_000_000;

    let mut cfg_b = BacktestConfig::test_defaults();
    cfg_b.commission = CommissionModel::ZERO;
    cfg_b.max_gross_exposure_mult_micros = 5_000_000;
    cfg_b.initial_cash_micros = 50_000_000_000; // different cash → different config_id

    let mut engine_a = BacktestEngine::new(cfg_a);
    engine_a
        .add_strategy(Box::new(NamedStrategy("same_name")))
        .unwrap();
    let r_a = engine_a.run(&bars).unwrap();

    let mut engine_b = BacktestEngine::new(cfg_b);
    engine_b
        .add_strategy(Box::new(NamedStrategy("same_name")))
        .unwrap();
    let r_b = engine_b.run(&bars).unwrap();

    assert_eq!(r_a.strategy_name, r_b.strategy_name);
    assert_ne!(
        r_a.run_id, r_b.run_id,
        "same strategy name + different config must produce different run_ids"
    );
}

// ---------------------------------------------------------------------------
// I6: No strategy registered → strategy_name is empty
// ---------------------------------------------------------------------------

#[test]
fn no_strategy_produces_empty_name() {
    // Pass zero bars so on_bar is never called (avoids NotRegistered error).
    let cfg = BacktestConfig::test_defaults();
    let mut engine = BacktestEngine::new(cfg);
    // deliberately no add_strategy call
    let report = engine.run(&[]).unwrap();
    assert_eq!(
        report.strategy_name, "",
        "no strategy registered must yield empty strategy_name"
    );
}

// ---------------------------------------------------------------------------
// I7: derive_run_id pure function — stable and sensitive
// ---------------------------------------------------------------------------

#[test]
fn derive_run_id_is_stable() {
    let cfg = BacktestConfig::test_defaults();
    let config_id = cfg.config_id();
    let id1 = derive_run_id("strat_a", &config_id);
    let id2 = derive_run_id("strat_a", &config_id);
    assert_eq!(id1, id2, "derive_run_id must be stable for same inputs");
    assert_ne!(id1, Uuid::nil(), "derive_run_id must not be nil");
}

#[test]
fn derive_run_id_differs_on_strategy_name() {
    let cfg = BacktestConfig::test_defaults();
    let config_id = cfg.config_id();
    let id_a = derive_run_id("strat_a", &config_id);
    let id_b = derive_run_id("strat_b", &config_id);
    assert_ne!(id_a, id_b, "different strategy names must yield different run_ids");
}
