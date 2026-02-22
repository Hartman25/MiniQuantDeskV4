use mqk_config::{load_layered_yaml_from_strings, report_unused_keys, ConfigMode, UnusedKeyPolicy};

/// PATCH 26 — scenario_unused_keys_warn_or_fail
///
/// Validates:
/// 1) Unused keys are detected in WARN mode but do not error.
/// 2) Unused keys cause failure in FAIL mode.
/// 3) Keys that are known to be consumed in a mode are not flagged.
/// 4) Exact-leaf consumption does NOT accidentally consume sibling keys.
/// 5) Deterministic ordering of unused pointers.
///
/// IMPORTANT:
/// The consumed-pointer registry must reflect what code ACTUALLY reads today.
/// As of now, PAPER/BACKTEST consume only the minimal engine isolation keys.

#[test]
fn warn_mode_reports_unused_keys_without_error() {
    let yaml = r#"
engine:
  engine_id: "MAIN"

broker:
  keys_env:
    api_key: "ALPACA_API_KEY_MAIN"
    api_secret: "ALPACA_API_SECRET_MAIN"

risk:
  max_gross_exposure: 1.0

unused_section:
  foo: 123
  bar: 456
"#;

    let loaded = load_layered_yaml_from_strings(&[yaml]).expect("config load must succeed");

    let report = report_unused_keys(
        ConfigMode::Paper,
        &loaded.config_json,
        UnusedKeyPolicy::Warn,
    )
    .expect("warn mode must not error");

    assert!(!report.is_clean(), "report should detect unused keys");

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
engine:
  engine_id: "MAIN"

broker:
  keys_env:
    api_key: "ALPACA_API_KEY_MAIN"
    api_secret: "ALPACA_API_SECRET_MAIN"

risk:
  max_gross_exposure: 1.0

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
fn only_consumed_keys_are_clean_in_paper_mode() {
    // A config containing ONLY keys that are currently consumed in PAPER mode.
    // This should produce a clean report.
    let yaml = r#"
engine:
  engine_id: "MAIN"

broker:
  keys_env:
    api_key: "ALPACA_API_KEY_MAIN"
    api_secret: "ALPACA_API_SECRET_MAIN"

risk:
  max_gross_exposure: 1.0
"#;

    let loaded = load_layered_yaml_from_strings(&[yaml]).expect("config load must succeed");

    let report = report_unused_keys(
        ConfigMode::Paper,
        &loaded.config_json,
        UnusedKeyPolicy::Warn,
    )
    .expect("warn mode must not error");

    assert!(
        report.is_clean(),
        "config should be clean when it only uses consumed keys"
    );
}

#[test]
fn exact_leaf_consumption_does_not_consume_sibling_keys() {
    // PAPER consumes /risk/max_gross_exposure.
    // It must NOT treat /risk/max_gross_exposure_extra as consumed.
    let yaml = r#"
engine:
  engine_id: "MAIN"

broker:
  keys_env:
    api_key: "ALPACA_API_KEY_MAIN"
    api_secret: "ALPACA_API_SECRET_MAIN"

risk:
  max_gross_exposure: 1.0
  max_gross_exposure_extra: 999
"#;

    let loaded = load_layered_yaml_from_strings(&[yaml]).expect("config load must succeed");

    let report = report_unused_keys(
        ConfigMode::Paper,
        &loaded.config_json,
        UnusedKeyPolicy::Warn,
    )
    .expect("warn mode must not error");

    assert!(
        report
            .unused_leaf_pointers
            .contains(&"/risk/max_gross_exposure_extra".to_string()),
        "sibling key must remain unused"
    );
}

#[test]
fn deterministic_unused_pointer_ordering() {
    let yaml = r#"
engine:
  engine_id: "MAIN"

broker:
  keys_env:
    api_key: "ALPACA_API_KEY_MAIN"
    api_secret: "ALPACA_API_SECRET_MAIN"

risk:
  max_gross_exposure: 1.0

unused:
  b: 2
  a: 1
"#;

    let loaded = load_layered_yaml_from_strings(&[yaml]).expect("config load must succeed");

    let report = report_unused_keys(
        ConfigMode::Paper,
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
