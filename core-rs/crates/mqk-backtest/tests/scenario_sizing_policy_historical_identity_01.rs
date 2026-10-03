//! Historical sizing/config identity is frozen as literal constants captured
//! from the pre-change baseline; the capital-fraction policy must change
//! identity only when selected.

use mqk_backtest::{BacktestConfig, SizingPolicy, StrategySizingConfig};

const DEFAULT_SIZING_CANONICAL: &str = "sz_tgt=1|sz_max_tgt=none|sz_max_notional=none";
const TEST_DEFAULTS_CONFIG_ID: &str = "46590a03-98ee-50d3-a143-64229284b956";
const CONSERVATIVE_DEFAULTS_CONFIG_ID: &str = "ce844445-e115-576d-8f60-c32fb355fcbd";

#[test]
fn legacy_fixed_quantity_identity_is_byte_identical_to_baseline() {
    assert_eq!(
        StrategySizingConfig::default_sizing().canonical_str(),
        DEFAULT_SIZING_CANONICAL
    );
    assert_eq!(
        BacktestConfig::test_defaults().config_id().to_string(),
        TEST_DEFAULTS_CONFIG_ID
    );
    assert_eq!(
        BacktestConfig::conservative_defaults()
            .config_id()
            .to_string(),
        CONSERVATIVE_DEFAULTS_CONFIG_ID
    );
}

#[test]
fn explicit_fixed_quantity_policy_equals_default_identity() {
    let mut c = BacktestConfig::test_defaults();
    c.sizing_policy = SizingPolicy::FixedQuantityV1;
    assert_eq!(c.config_id().to_string(), TEST_DEFAULTS_CONFIG_ID);
}

#[test]
fn capital_fraction_policy_changes_identity_and_binds_fraction() {
    let base = BacktestConfig::test_defaults();
    let mut a = base.clone();
    a.sizing_policy = SizingPolicy::capital_fraction_v1(1_000).unwrap();
    let mut b = base.clone();
    b.sizing_policy = SizingPolicy::capital_fraction_v1(1_001).unwrap();

    assert_ne!(a.config_id(), base.config_id());
    assert_ne!(a.config_id(), b.config_id());
    // Stable under re-derivation.
    assert_eq!(a.config_id(), a.clone().config_id());
}

#[test]
fn capital_fraction_identity_binds_initial_capital_and_caps() {
    let mut a = BacktestConfig::test_defaults();
    a.sizing_policy = SizingPolicy::capital_fraction_v1(2_500).unwrap();

    let mut cash = a.clone();
    cash.initial_cash_micros += 1_000_000;
    assert_ne!(a.config_id(), cash.config_id());

    let mut cap = a.clone();
    cap.sizing = StrategySizingConfig {
        target_qty: 1,
        max_target_qty: Some(5),
        max_position_notional_usd: None,
    };
    assert_ne!(a.config_id(), cap.config_id());
}

#[test]
fn canonical_suffix_is_empty_only_for_legacy_policy() {
    assert_eq!(SizingPolicy::FixedQuantityV1.canonical_suffix(), "");
    assert_eq!(
        SizingPolicy::capital_fraction_v1(2_500)
            .unwrap()
            .canonical_suffix(),
        "|sz_policy=fixed_initial_capital_fraction_v1|sz_frac_bps=2500"
    );
}
