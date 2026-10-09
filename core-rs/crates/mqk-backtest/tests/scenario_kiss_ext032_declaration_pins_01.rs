//! The non-executable `M1-KISS-EXT032-ETF-01` predeclaration pins engine facts. This test
//! recomputes them from the registered engine and the one capital-fraction fingerprint
//! constructor, so a drifted engine, calendar binding, symbol list or sizing contract fails here
//! before any trial could be registered. It reads no market data and registers nothing.

use mqk_strategy::engines::{register_builtin_strategies_with_sizing, REGISTERED_STRATEGY_IDS};
use mqk_strategy::{capital_fraction_semantic_fingerprint, PluginRegistry, Strategy, TargetSizing};
use serde_json::Value;
use std::path::PathBuf;

const NAME: &str = "pre_holiday_two_session_long_v1";

fn declaration() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../research-py/experiments/m1_native_trend_campaign/PREDECLARED_KISS_EXT032_ETF_01.json");
    serde_json::from_str(&std::fs::read_to_string(&path).expect("declaration is committed"))
        .unwrap()
}

fn engine(symbol: &str) -> Box<dyn Strategy> {
    let mut reg = PluginRegistry::new();
    register_builtin_strategies_with_sizing(&mut reg, symbol, 1, None, None).unwrap();
    reg.instantiate(NAME).unwrap()
}

#[test]
fn declared_fingerprints_equal_the_registered_engine_and_the_capital_fraction_constructor() {
    let d = declaration();
    let wrapper = &d["strategy_fingerprints"]["wrapper_inputs"];
    let bps = wrapper["allocation_fraction_bps"].as_i64().unwrap();
    let capital = wrapper["initial_allocated_capital_micros"]
        .as_i64()
        .unwrap();
    assert_eq!((bps, capital), (1_000, 100_000_000_000));
    let per_symbol = d["strategy_fingerprints"]["per_symbol"]
        .as_object()
        .unwrap();
    assert_eq!(per_symbol.len(), 4);
    for symbol in ["SPY", "QQQ", "IWM", "DIA"] {
        let inner = engine(symbol).semantic_fingerprint();
        let wrapped = capital_fraction_semantic_fingerprint(
            &inner,
            bps,
            capital,
            &TargetSizing::equity_default(),
        );
        let pinned = &per_symbol[symbol];
        assert_eq!(
            pinned["inner_semantic_fingerprint"].as_str().unwrap(),
            inner,
            "{symbol}"
        );
        assert_eq!(
            pinned["capital_fraction_wrapped_semantic_fingerprint"]
                .as_str()
                .unwrap(),
            wrapped,
            "{symbol}"
        );
        assert_ne!(inner, wrapped);
    }
}

#[test]
fn declared_trial_list_history_and_timeframe_match_the_engine() {
    let d = declaration();
    assert!(REGISTERED_STRATEGY_IDS.contains(&NAME));
    let hyps = d["hypotheses"].as_array().unwrap();
    assert_eq!(hyps.len(), 1, "exactly one hypothesis");
    let h = &hyps[0];
    assert_eq!(h["strategy_id"].as_str().unwrap(), NAME);
    let probe = engine("SPY");
    assert_eq!(
        h["required_history_bars"].as_u64().unwrap() as usize,
        probe.required_history_bars()
    );
    assert_eq!(
        h["timeframe_secs"].as_i64().unwrap(),
        probe.spec().timeframe_secs
    );
    let trials = d["universe"]["trials"].as_array().unwrap();
    let got: Vec<(i64, &str, &str)> = trials
        .iter()
        .map(|t| {
            (
                t["order"].as_i64().unwrap(),
                t["strategy_id"].as_str().unwrap(),
                t["symbol"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (1, NAME, "SPY"),
            (2, NAME, "QQQ"),
            (3, NAME, "IWM"),
            (4, NAME, "DIA")
        ],
        "four trials, frozen order, no other symbol or strategy"
    );
    assert_eq!(d["universe"]["max_trials"].as_u64().unwrap(), 4);
}

#[test]
fn the_declaration_is_not_executable_and_names_its_blocker() {
    let d = declaration();
    assert_eq!(d["execution_gate"]["executable"], Value::Bool(false));
    assert_eq!(
        d["execution_gate"]["blocker"].as_str().unwrap(),
        "OPERATOR_EXECUTION_AUTHORIZATION_REQUIRED"
    );
    assert_eq!(
        d["batch_stopping_rule"]["economic_attempts"]
            .as_u64()
            .unwrap(),
        0
    );
    assert_eq!(
        d["evidence_grade"]["grade"].as_str().unwrap(),
        "EXPOSED_DEVELOPMENT"
    );
}
