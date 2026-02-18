//! PATCH 15b — Config hash stability test
//!
//! Validates: docs/specs/config_layering_and_hashing.md section 4 (hashing determinism)
//!
//! GREEN when:
//! - `load_layered_yaml_from_strings` called twice on the same inputs returns
//!   identical config_hash.
//! - Reordering keys within YAML doesn't change the hash (canonicalization).
//! - Different values produce different hashes (collision resistance sanity).
//! - Multiple merge layers produce stable hash regardless of call order.

use mqk_config::load_layered_yaml_from_strings;

const BASE_YAML: &str = r#"
engine:
  engine_id: "MAIN"
  mode: "PAPER"
risk:
  daily_loss_limit: 0.02
  max_drawdown: 0.18
broker:
  name: "alpaca"
  keys_env:
    api_key: "ALPACA_API_KEY_MAIN"
    api_secret: "ALPACA_API_SECRET_MAIN"
"#;

/// Same content as BASE_YAML but with keys in different order.
const BASE_YAML_REORDERED: &str = r#"
risk:
  max_drawdown: 0.18
  daily_loss_limit: 0.02
broker:
  keys_env:
    api_secret: "ALPACA_API_SECRET_MAIN"
    api_key: "ALPACA_API_KEY_MAIN"
  name: "alpaca"
engine:
  mode: "PAPER"
  engine_id: "MAIN"
"#;

const OVERLAY_YAML: &str = r#"
engine:
  mode: "LIVE"
risk:
  daily_loss_limit: 0.01
"#;

#[test]
fn same_input_produces_identical_hash() {
    let a = load_layered_yaml_from_strings(&[BASE_YAML]).unwrap();
    let b = load_layered_yaml_from_strings(&[BASE_YAML]).unwrap();

    assert_eq!(
        a.config_hash, b.config_hash,
        "same YAML input must produce identical hash"
    );
    assert_eq!(
        a.canonical_json, b.canonical_json,
        "canonical JSON must be identical for same input"
    );
}

#[test]
fn reordered_keys_produce_same_hash() {
    let original = load_layered_yaml_from_strings(&[BASE_YAML]).unwrap();
    let reordered = load_layered_yaml_from_strings(&[BASE_YAML_REORDERED]).unwrap();

    assert_eq!(
        original.config_hash, reordered.config_hash,
        "reordering keys in YAML must not change the hash (canonicalization)"
    );
    assert_eq!(
        original.canonical_json, reordered.canonical_json,
        "canonical JSON must be identical regardless of key ordering in source"
    );
}

#[test]
fn different_values_produce_different_hash() {
    let a = load_layered_yaml_from_strings(&[BASE_YAML]).unwrap();

    let modified = r#"
engine:
  engine_id: "EXP"
  mode: "PAPER"
risk:
  daily_loss_limit: 0.05
  max_drawdown: 0.30
broker:
  name: "alpaca"
  keys_env:
    api_key: "ALPACA_API_KEY_EXP"
    api_secret: "ALPACA_API_SECRET_EXP"
"#;
    let b = load_layered_yaml_from_strings(&[modified]).unwrap();

    assert_ne!(
        a.config_hash, b.config_hash,
        "different config values must produce different hashes"
    );
}

#[test]
fn merged_layers_produce_stable_hash() {
    let a = load_layered_yaml_from_strings(&[BASE_YAML, OVERLAY_YAML]).unwrap();
    let b = load_layered_yaml_from_strings(&[BASE_YAML, OVERLAY_YAML]).unwrap();

    assert_eq!(
        a.config_hash, b.config_hash,
        "same merge layers must produce identical hash"
    );

    // Verify the overlay actually took effect
    let mode = a
        .config_json
        .pointer("/engine/mode")
        .and_then(|v| v.as_str())
        .unwrap();
    assert_eq!(mode, "LIVE", "overlay should override base engine.mode");

    let dll = a
        .config_json
        .pointer("/risk/daily_loss_limit")
        .and_then(|v| v.as_f64())
        .unwrap();
    assert!(
        (dll - 0.01).abs() < 1e-9,
        "overlay should override base daily_loss_limit"
    );
}

#[test]
fn hash_is_64_hex_chars() {
    let loaded = load_layered_yaml_from_strings(&[BASE_YAML]).unwrap();

    // SHA-256 produces 32 bytes = 64 hex characters
    assert_eq!(
        loaded.config_hash.len(),
        64,
        "SHA-256 hash should be 64 hex chars"
    );
    assert!(
        loaded.config_hash.chars().all(|c| c.is_ascii_hexdigit()),
        "hash should contain only hex digits"
    );
}

#[test]
fn empty_config_produces_stable_hash() {
    let a = load_layered_yaml_from_strings(&["{}"]).unwrap();
    let b = load_layered_yaml_from_strings(&["{}"]).unwrap();

    assert_eq!(
        a.config_hash, b.config_hash,
        "empty configs must produce identical hash"
    );
}
