//! PATCH 15f — Cross-engine isolation integration test
//!
//! Validates: PATCH 13 isolation guarantees end-to-end
//!
//! GREEN when:
//! - EngineStore<T> with MAIN and EXP entries returns None for wrong engine.
//! - EngineIsolation::from_config_json rejects config where broker key env var
//!   does not contain engine_id token.
//! - Separate engines get separate allocation cap computations.
//! - EngineStore mutations on one engine do not affect the other.

use mqk_isolation::{
    enforce_allocation_cap_micros, max_gross_exposure_allowed_micros, EngineId, EngineIsolation,
    EngineStore, MICROS_SCALE,
};
use serde_json::json;

#[test]
fn engine_store_isolates_state_per_engine() {
    let main = EngineId::new("MAIN");
    let exp = EngineId::new("EXP");

    let mut store: EngineStore<i64> = EngineStore::new();
    store.insert(main.clone(), 100);
    store.insert(exp.clone(), 200);

    // Each engine gets its own value
    assert_eq!(store.get(&main), Some(&100));
    assert_eq!(store.get(&exp), Some(&200));

    // Unknown engine returns None (no bleed)
    let unknown = EngineId::new("UNKNOWN");
    assert_eq!(store.get(&unknown), None);
}

#[test]
fn engine_store_mutation_does_not_bleed() {
    let main = EngineId::new("MAIN");
    let exp = EngineId::new("EXP");

    let mut store: EngineStore<Vec<String>> = EngineStore::new();
    store.insert(main.clone(), vec!["main_order_1".to_string()]);
    store.insert(exp.clone(), vec!["exp_order_1".to_string()]);

    // Mutate MAIN state
    if let Some(main_state) = store.get_mut(&main) {
        main_state.push("main_order_2".to_string());
    }

    // MAIN should have 2 entries
    assert_eq!(store.get(&main).unwrap().len(), 2);

    // EXP should be unaffected (still 1 entry)
    assert_eq!(store.get(&exp).unwrap().len(), 1);
    assert_eq!(store.get(&exp).unwrap()[0], "exp_order_1");
}

#[test]
fn engine_isolation_rejects_shared_key_names() {
    // Config where broker key env vars do NOT contain the engine_id token
    let config_shared_keys = json!({
        "engine": {"engine_id": "MAIN"},
        "broker": {
            "keys_env": {
                "api_key": "ALPACA_API_KEY_GENERIC",    // missing "MAIN" token
                "api_secret": "ALPACA_API_SECRET_GENERIC" // missing "MAIN" token
            }
        }
    });

    let result = EngineIsolation::from_config_json(&config_shared_keys);
    assert!(
        result.is_err(),
        "should reject config where broker key env var does not contain engine_id token"
    );

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("must include engine_id token"),
        "error should mention engine_id token requirement, got: {err_msg}"
    );
}

#[test]
fn engine_isolation_accepts_properly_scoped_keys() {
    // Config where broker key env vars DO contain the engine_id token
    let config_main = json!({
        "engine": {"engine_id": "MAIN"},
        "broker": {
            "keys_env": {
                "api_key": "ALPACA_API_KEY_MAIN",
                "api_secret": "ALPACA_API_SECRET_MAIN"
            }
        }
    });

    let (engine_id, isolation) = EngineIsolation::from_config_json(&config_main).unwrap();

    assert_eq!(engine_id.as_str(), "MAIN");
    assert_eq!(isolation.broker_api_key_env, "ALPACA_API_KEY_MAIN");
    assert_eq!(isolation.broker_api_secret_env, "ALPACA_API_SECRET_MAIN");
}

#[test]
fn exp_engine_rejects_main_keys() {
    // EXP engine should NOT accept MAIN-scoped key names
    let config_exp_with_main_keys = json!({
        "engine": {"engine_id": "EXP"},
        "broker": {
            "keys_env": {
                "api_key": "ALPACA_API_KEY_MAIN",
                "api_secret": "ALPACA_API_SECRET_MAIN"
            }
        }
    });

    let result = EngineIsolation::from_config_json(&config_exp_with_main_keys);
    assert!(
        result.is_err(),
        "EXP engine should reject MAIN-scoped broker keys"
    );
}

#[test]
fn exp_engine_accepts_exp_keys() {
    let config_exp = json!({
        "engine": {"engine_id": "EXP"},
        "broker": {
            "keys_env": {
                "api_key": "ALPACA_API_KEY_PAPER_EXP",
                "api_secret": "ALPACA_API_SECRET_PAPER_EXP"
            }
        }
    });

    let (engine_id, isolation) = EngineIsolation::from_config_json(&config_exp).unwrap();
    assert_eq!(engine_id.as_str(), "EXP");
    assert_eq!(isolation.broker_api_key_env, "ALPACA_API_KEY_PAPER_EXP");
}

#[test]
fn allocation_caps_independent_per_engine() {
    // MAIN: equity 100k, cap 1.0x => max 100k gross exposure
    let main_equity = 100_000 * MICROS_SCALE;
    let main_cap = 1_000_000; // 1.0x in micros
    let main_max = max_gross_exposure_allowed_micros(main_equity, main_cap);

    // EXP: equity 50k, cap 0.5x => max 25k gross exposure
    let exp_equity = 50_000 * MICROS_SCALE;
    let exp_cap = 500_000; // 0.5x in micros
    let exp_max = max_gross_exposure_allowed_micros(exp_equity, exp_cap);

    assert_eq!(main_max, 100_000 * MICROS_SCALE, "MAIN max should be 100k");
    assert_eq!(exp_max, 25_000 * MICROS_SCALE, "EXP max should be 25k");

    // MAIN can add 50k exposure, EXP cannot
    assert!(
        enforce_allocation_cap_micros(main_equity, 0, 50_000 * MICROS_SCALE, main_cap).is_ok(),
        "MAIN should allow 50k exposure"
    );
    assert!(
        enforce_allocation_cap_micros(exp_equity, 0, 50_000 * MICROS_SCALE, exp_cap).is_err(),
        "EXP should reject 50k exposure (exceeds 25k cap)"
    );
}

#[test]
fn missing_engine_id_rejected() {
    let config_no_engine_id = json!({
        "broker": {
            "keys_env": {
                "api_key": "ALPACA_API_KEY_MAIN",
                "api_secret": "ALPACA_API_SECRET_MAIN"
            }
        }
    });

    let result = EngineIsolation::from_config_json(&config_no_engine_id);
    assert!(
        result.is_err(),
        "config missing engine.engine_id should be rejected"
    );
}

#[test]
fn missing_broker_keys_rejected() {
    let config_no_keys = json!({
        "engine": {"engine_id": "MAIN"},
        "broker": {}
    });

    let result = EngineIsolation::from_config_json(&config_no_keys);
    assert!(
        result.is_err(),
        "config missing broker.keys_env should be rejected"
    );
}
