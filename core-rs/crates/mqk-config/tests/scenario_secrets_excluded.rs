//! PATCH 15a — Config secrets exclusion test
//!
//! Validates: docs/specs/config_layering_and_hashing.md section 5 (secrets exclusion)
//!
//! GREEN when:
//! - Loading a YAML with `api_key: "sk-live-abc123"` as a literal value FAILS
//!   with CONFIG_SECRET_DETECTED.
//! - Loading with `api_key_env: "ALPACA_API_KEY_MAIN"` succeeds and config_json
//!   contains the env var name, not the secret value.

use mqk_config::load_layered_yaml_from_strings;

/// A config with a literal secret value embedded (violates spec).
const YAML_WITH_SECRET: &str = r#"
engine:
  engine_id: "MAIN"
broker:
  keys_env:
    api_key: "sk-live-abc123secretvalue"
    api_secret: "ALPACA_API_SECRET_MAIN"
"#;

/// A config with env var NAMES only (correct pattern per spec).
const YAML_WITH_ENV_NAMES: &str = r#"
engine:
  engine_id: "MAIN"
broker:
  keys_env:
    api_key: "ALPACA_API_KEY_MAIN"
    api_secret: "ALPACA_API_SECRET_MAIN"
"#;

/// AWS-style secret should also be caught.
const YAML_WITH_AWS_SECRET: &str = r#"
engine:
  engine_id: "MAIN"
broker:
  keys_env:
    api_key: "AKIAIOSFODNN7EXAMPLE"
    api_secret: "ALPACA_API_SECRET_MAIN"
"#;

/// PEM private key should be caught.
const YAML_WITH_PEM_SECRET: &str = r#"
engine:
  engine_id: "MAIN"
broker:
  tls_cert: "-----BEGIN RSA PRIVATE KEY-----\nfakekeydata\n-----END RSA PRIVATE KEY-----"
"#;

/// Secrets nested in arrays should also be detected.
const YAML_SECRET_IN_ARRAY: &str = r#"
engine:
  engine_id: "MAIN"
webhooks:
  - url: "https://example.com"
    token: "sk-proj-realtoken123"
"#;

#[test]
fn literal_secret_value_rejected() {
    let result = load_layered_yaml_from_strings(&[YAML_WITH_SECRET]);
    assert!(
        result.is_err(),
        "config with literal secret should be rejected"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("CONFIG_SECRET_DETECTED"),
        "error should contain CONFIG_SECRET_DETECTED, got: {err_msg}"
    );
}

#[test]
fn env_var_name_accepted() {
    let result = load_layered_yaml_from_strings(&[YAML_WITH_ENV_NAMES]);
    assert!(
        result.is_ok(),
        "config with env var names should be accepted, got err: {:?}",
        result.err()
    );

    let loaded = result.unwrap();

    // Verify that config_json contains the env var NAME, not a secret value
    let api_key = loaded
        .config_json
        .pointer("/broker/keys_env/api_key")
        .and_then(|v| v.as_str())
        .expect("api_key should be present in config_json");

    assert_eq!(
        api_key, "ALPACA_API_KEY_MAIN",
        "config_json should contain the env var name, not a resolved secret"
    );

    // Verify canonical_json also contains env var name
    assert!(
        loaded.canonical_json.contains("ALPACA_API_KEY_MAIN"),
        "canonical_json should contain env var name"
    );
    assert!(
        !loaded.canonical_json.contains("sk-"),
        "canonical_json must NOT contain secret-like prefix"
    );
}

#[test]
fn aws_key_prefix_rejected() {
    let result = load_layered_yaml_from_strings(&[YAML_WITH_AWS_SECRET]);
    assert!(
        result.is_err(),
        "config with AWS key prefix AKIA should be rejected"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("CONFIG_SECRET_DETECTED"),
        "error should contain CONFIG_SECRET_DETECTED, got: {err_msg}"
    );
}

#[test]
fn pem_private_key_rejected() {
    let result = load_layered_yaml_from_strings(&[YAML_WITH_PEM_SECRET]);
    assert!(
        result.is_err(),
        "config with PEM private key should be rejected"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("CONFIG_SECRET_DETECTED"),
        "error should contain CONFIG_SECRET_DETECTED, got: {err_msg}"
    );
}

#[test]
fn secret_in_array_rejected() {
    let result = load_layered_yaml_from_strings(&[YAML_SECRET_IN_ARRAY]);
    assert!(
        result.is_err(),
        "config with secret inside array should be rejected"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("CONFIG_SECRET_DETECTED"),
        "error should contain CONFIG_SECRET_DETECTED, got: {err_msg}"
    );
}

#[test]
fn merged_config_catches_secret_in_overlay() {
    // Base config is clean, but overlay introduces a secret
    let base = r#"
engine:
  engine_id: "MAIN"
broker:
  keys_env:
    api_key: "ALPACA_API_KEY_MAIN"
    api_secret: "ALPACA_API_SECRET_MAIN"
"#;

    let overlay = r#"
broker:
  keys_env:
    api_key: "sk-live-sneaky-override"
"#;

    let result = load_layered_yaml_from_strings(&[base, overlay]);
    assert!(
        result.is_err(),
        "merged config with secret in overlay should be rejected"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("CONFIG_SECRET_DETECTED"),
        "error should contain CONFIG_SECRET_DETECTED, got: {err_msg}"
    );
}
