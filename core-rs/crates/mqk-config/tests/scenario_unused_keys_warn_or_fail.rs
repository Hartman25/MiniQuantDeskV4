use mqk_config::{load_layered_yaml_from_strings, report_unused_keys, ConfigMode, UnusedKeyPolicy};

/// PATCH 26 — scenario_unused_keys_warn_or_fail
///
/// Validates:
/// 1) Unused keys are detected in WARN mode but do not error.
/// 2) Unused keys cause failure in FAIL mode.
/// 3) Keys under consumed prefixes are not flagged.
/// 4) Deterministic ordering of unused pointers.

#[test]
fn warn_mode_reports_unused_keys_without_error() {
    let yaml = r#"
runtime:
  mode: backtest

data:
  timeframe: "1m"

unused_section:
  foo: 123
  bar: 456
"#;

    let loaded = load_layered_yaml_from_strings(&[yaml]).expect("config load must succeed");

    let report = report_unused_keys(
        ConfigMode::Backtest,
        &loaded.config_json,
        UnusedKeyPolicy::Warn,
    )
    .expect("warn mode must not error");

    assert!(!report.is_clean(), "report should detect unused keys");

    // Expect at least these leaf pointers.
    assert!(
        report
            .unused_leaf_pointers
            .contains(&"/unused_section/foo".to_string()),
        "missing unused pointer for foo"
    );

    assert!(
        report
            .unused_leaf_pointers
            .contains(&"/unused_section/bar".to_string()),
        "missing unused pointer for bar"
    );
}

#[test]
fn fail_mode_errors_on_unused_keys() {
    let yaml = r#"
runtime:
  mode: live

unused_section:
  foo: 1
"#;

    let loaded = load_layered_yaml_from_strings(&[yaml]).expect("config load must succeed");

    let result = report_unused_keys(ConfigMode::Live, &loaded.config_json, UnusedKeyPolicy::Fail);

    assert!(
        result.is_err(),
        "fail policy must error when unused keys exist"
    );

    let msg = format!("{:?}", result.err().unwrap());
    assert!(
        msg.contains("CONFIG_UNUSED_KEYS"),
        "error message should contain CONFIG_UNUSED_KEYS"
    );
}

#[test]
fn consumed_prefix_covers_nested_keys() {
    let yaml = r#"
risk:
  limits:
    max_notional: 100000
    max_positions: 5
"#;

    let loaded = load_layered_yaml_from_strings(&[yaml]).expect("config load must succeed");

    // In Backtest mode, "/risk" is consumed.
    let report = report_unused_keys(
        ConfigMode::Backtest,
        &loaded.config_json,
        UnusedKeyPolicy::Warn,
    )
    .expect("warn mode must not error");

    assert!(
        report.is_clean(),
        "risk subtree should be considered consumed in Backtest mode"
    );
}

#[test]
fn deterministic_unused_pointer_ordering() {
    let yaml = r#"
unused:
  b: 2
  a: 1
"#;

    let loaded = load_layered_yaml_from_strings(&[yaml]).expect("config load must succeed");

    let report = report_unused_keys(
        ConfigMode::Backtest,
        &loaded.config_json,
        UnusedKeyPolicy::Warn,
    )
    .expect("warn mode must not error");

    assert_eq!(
        report.unused_leaf_pointers,
        vec!["/unused/a".to_string(), "/unused/b".to_string()],
        "unused pointers must be sorted deterministically"
    );
}
