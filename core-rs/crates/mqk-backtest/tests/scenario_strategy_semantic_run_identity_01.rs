//! BACKTEST-STRATEGY-SEMANTIC-RUN-IDENTITY-01: `run_id` must bind to the
//! strategy's semantic fingerprint, not just strategy_name/config/input
//! data/economics/execution model. Two materially different strategy
//! semantics sharing a strategy_name and config must never collide on
//! `run_id`; identical semantics must remain stable and reproducible.
//!
//! Result metrics (P&L, order count, fills, equity) never participate in
//! `run_id` derivation -- only inputs known before the run executes.

use mqk_backtest::{
    derive_input_data_hash, derive_run_id_with_execution_model,
    derive_run_id_with_semantic_identity, BacktestConfig, BacktestEngine,
    BacktestInstrumentEconomics, BACKTEST_EXECUTION_MODEL_ID,
};
use mqk_execution::StrategyOutput;
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

fn econ_equity() -> BacktestInstrumentEconomics {
    BacktestInstrumentEconomics::equity()
}

fn econ_non_default() -> BacktestInstrumentEconomics {
    BacktestInstrumentEconomics::new(50, Some(1_000_000), Some(500_000)).unwrap()
}

/// A strategy whose `spec()` (name/timeframe) is fixed but whose
/// `semantic_fingerprint()` is caller-controlled -- lets tests simulate two
/// materially different strategy semantics under an identical name/config.
struct FingerprintedStrategy {
    fingerprint: String,
}

impl Strategy for FingerprintedStrategy {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("fp_strategy", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        StrategyOutput::new(vec![])
    }

    fn semantic_fingerprint(&self) -> String {
        self.fingerprint.clone()
    }
}

/// Test 1 -- semantic fingerprint A vs B alone (all other inputs fixed)
/// changes `run_id`.
#[test]
fn semantic_fingerprint_alone_changes_run_id() {
    let cfg = BacktestConfig::test_defaults();
    let config_id = cfg.config_id();
    let hash = derive_input_data_hash(&[]);

    let id_a = derive_run_id_with_semantic_identity(
        "strat",
        &config_id,
        &hash,
        &econ_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
        "fingerprint-a",
    );
    let id_b = derive_run_id_with_semantic_identity(
        "strat",
        &config_id,
        &hash,
        &econ_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
        "fingerprint-b",
    );

    assert_ne!(
        id_a, id_b,
        "different strategy semantic fingerprints must produce different run_id"
    );
}

/// Test 2 -- identical inputs (including identical fingerprint) reproduce an
/// identical `run_id`.
#[test]
fn identical_semantic_fingerprint_reproduces_identical_run_id() {
    let cfg = BacktestConfig::test_defaults();
    let config_id = cfg.config_id();
    let hash = derive_input_data_hash(&[]);

    let id1 = derive_run_id_with_semantic_identity(
        "strat",
        &config_id,
        &hash,
        &econ_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
        "fingerprint-a",
    );
    let id2 = derive_run_id_with_semantic_identity(
        "strat",
        &config_id,
        &hash,
        &econ_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
        "fingerprint-a",
    );

    assert_eq!(id1, id2, "identical inputs must yield identical run_id");
}

/// Test 3 -- new semantic-aware (`v5`) identity never collides with the
/// prior semantic-unaware (`v4`) identity for the same inputs.
#[test]
fn semantic_aware_identity_differs_from_legacy_execution_model_identity() {
    let cfg = BacktestConfig::test_defaults();
    let config_id = cfg.config_id();
    let hash = derive_input_data_hash(&[]);

    let legacy_id = derive_run_id_with_execution_model(
        "strat",
        &config_id,
        &hash,
        &econ_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
    );
    let new_id = derive_run_id_with_semantic_identity(
        "strat",
        &config_id,
        &hash,
        &econ_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
        "fingerprint-a",
    );

    assert_ne!(
        legacy_id, new_id,
        "v5 semantic-aware run_id must never collide with the legacy v4 execution-model-aware run_id"
    );
}

/// Test 4 -- input-data change still changes `run_id` (all other inputs,
/// including the semantic fingerprint, fixed).
#[test]
fn input_data_change_still_changes_run_id() {
    let cfg = BacktestConfig::test_defaults();
    let config_id = cfg.config_id();
    let hash_a = derive_input_data_hash(&[]);
    let hash_b = derive_input_data_hash(&[mqk_backtest::BacktestBar::new(
        "AAPL", 1_000, 100, 110, 90, 105, 1_000,
    )]);

    let id_a = derive_run_id_with_semantic_identity(
        "strat",
        &config_id,
        &hash_a,
        &econ_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
        "fingerprint-a",
    );
    let id_b = derive_run_id_with_semantic_identity(
        "strat",
        &config_id,
        &hash_b,
        &econ_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
        "fingerprint-a",
    );

    assert_ne!(id_a, id_b, "input-data change must still change run_id");
}

/// Test 5 -- `BacktestConfig` change still changes `run_id`.
#[test]
fn config_change_still_changes_run_id() {
    let cfg_a = BacktestConfig::test_defaults();
    let mut cfg_b = BacktestConfig::test_defaults();
    cfg_b.max_gross_exposure_mult_micros = 5_000_000;
    let hash = derive_input_data_hash(&[]);

    let id_a = derive_run_id_with_semantic_identity(
        "strat",
        &cfg_a.config_id(),
        &hash,
        &econ_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
        "fingerprint-a",
    );
    let id_b = derive_run_id_with_semantic_identity(
        "strat",
        &cfg_b.config_id(),
        &hash,
        &econ_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
        "fingerprint-a",
    );

    assert_ne!(id_a, id_b, "config change must still change run_id");
}

/// Test 6 -- execution-model change still changes `run_id`.
#[test]
fn execution_model_change_still_changes_run_id() {
    let cfg = BacktestConfig::test_defaults();
    let config_id = cfg.config_id();
    let hash = derive_input_data_hash(&[]);

    let id_a = derive_run_id_with_semantic_identity(
        "strat",
        &config_id,
        &hash,
        &econ_equity(),
        "future_target_symbol_bar_v1",
        "fingerprint-a",
    );
    let id_b = derive_run_id_with_semantic_identity(
        "strat",
        &config_id,
        &hash,
        &econ_equity(),
        "some_other_execution_model_v2",
        "fingerprint-a",
    );

    assert_ne!(id_a, id_b, "execution-model change must still change run_id");
}

/// Test 7 -- economics change still changes `run_id`.
#[test]
fn economics_change_still_changes_run_id() {
    let cfg = BacktestConfig::test_defaults();
    let config_id = cfg.config_id();
    let hash = derive_input_data_hash(&[]);

    let id_equity = derive_run_id_with_semantic_identity(
        "strat",
        &config_id,
        &hash,
        &econ_equity(),
        BACKTEST_EXECUTION_MODEL_ID,
        "fingerprint-a",
    );
    let id_non_default = derive_run_id_with_semantic_identity(
        "strat",
        &config_id,
        &hash,
        &econ_non_default(),
        BACKTEST_EXECUTION_MODEL_ID,
        "fingerprint-a",
    );

    assert_ne!(
        id_equity, id_non_default,
        "economics change must still change run_id"
    );
}

/// Test 8 -- the real `BacktestEngine::run` production path uses the
/// semantic-aware (`v5`) derivation: the engine's real `run_id` matches an
/// independent `derive_run_id_with_semantic_identity` call built from the
/// report's own fields, AND changing only the strategy's semantic
/// fingerprint (same strategy_name, same config, same bars) changes the
/// engine's real `run_id`.
#[test]
fn real_engine_report_uses_semantic_aware_derivation() {
    let bars = [mqk_backtest::BacktestBar::new(
        "AAPL", 1_000, 100, 100, 100, 100, 1_000,
    )];

    let mut engine_a = BacktestEngine::new(BacktestConfig::test_defaults());
    engine_a
        .add_strategy(Box::new(FingerprintedStrategy {
            fingerprint: "fingerprint-a".to_string(),
        }))
        .unwrap();
    let report_a = engine_a.run(&bars).unwrap();

    assert_eq!(
        report_a.strategy_semantic_fingerprint, "fingerprint-a",
        "report must carry the exact fingerprint of the strategy instance that ran"
    );

    let independent_id = derive_run_id_with_semantic_identity(
        &report_a.strategy_name,
        &report_a.config_id,
        &report_a.input_data_hash,
        &BacktestInstrumentEconomics::new(
            report_a.economics.contract_multiplier,
            report_a.economics.initial_margin_micros,
            report_a.economics.maintenance_margin_micros,
        )
        .unwrap(),
        &report_a.execution_model_id,
        &report_a.strategy_semantic_fingerprint,
    );
    assert_eq!(
        report_a.run_id, independent_id,
        "engine's real run_id must match independent v5 derivation from the report's own fields"
    );

    let mut engine_b = BacktestEngine::new(BacktestConfig::test_defaults());
    engine_b
        .add_strategy(Box::new(FingerprintedStrategy {
            fingerprint: "fingerprint-b".to_string(),
        }))
        .unwrap();
    let report_b = engine_b.run(&bars).unwrap();

    assert_eq!(report_a.strategy_name, report_b.strategy_name);
    assert_eq!(report_a.config_id, report_b.config_id);
    assert_eq!(report_a.input_data_hash, report_b.input_data_hash);
    assert_ne!(
        report_a.run_id, report_b.run_id,
        "two real engine runs sharing strategy_name/config/input but differing only in \
         strategy semantic fingerprint must never collide on run_id"
    );
}
