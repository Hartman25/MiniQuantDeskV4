//! Deployment sizing contract (FixedInitialCapitalFractionV1): strict parsing,
//! legacy identity preservation, and the capital-fraction registry that only a
//! durable-state runtime can instantiate (Paper stays inactive).

use mqk_runtime::native_strategy::{
    build_plugin_registry_from_inputs, resolve_deployment_sizing_contract, NativeStrategyBootstrap,
    StrategyBootstrapInputs,
};
use mqk_strategy::{RegistryError, RestartRecovery, SizingPolicy};

const CF: &str = "fixed_initial_capital_fraction_v1";

fn inputs(policy: Option<&str>, bps: Option<&str>, cap: Option<&str>) -> StrategyBootstrapInputs {
    StrategyBootstrapInputs {
        symbol: "SPY".to_string(),
        raw_sizing_policy: policy.map(str::to_string),
        raw_allocation_fraction_bps: bps.map(str::to_string),
        raw_allocated_capital_micros: cap.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn legacy_inputs_resolve_to_fixed_quantity_with_empty_identity_token() {
    for i in [
        inputs(None, None, None),
        inputs(Some("fixed_quantity_v1"), None, None),
        inputs(Some(" fixed_quantity_v1 "), None, None),
    ] {
        let c = resolve_deployment_sizing_contract(&i).unwrap();
        assert_eq!(c.policy, SizingPolicy::FixedQuantityV1);
        assert_eq!(c.allocated_capital_micros, None);
        assert_eq!(c.identity_token(), "");
    }
    // Legacy registry build is unchanged: still builds with the one-share default.
    assert!(build_plugin_registry_from_inputs(&inputs(None, None, None)).is_ok());
    assert!(
        build_plugin_registry_from_inputs(&inputs(Some("fixed_quantity_v1"), None, None)).is_ok()
    );
}

#[test]
fn malformed_partial_and_mixed_contracts_are_refused() {
    let cases: &[(Option<&str>, Option<&str>, Option<&str>)] = &[
        (None, Some("2500"), Some("100000000000")),
        (None, Some("2500"), None),
        (None, None, Some("100000000000")),
        (Some("fixed_quantity_v1"), Some("2500"), None),
        (Some("fixed_quantity_v1"), None, Some("100000000000")),
        (Some("unknown_policy_v9"), None, None),
        (Some(CF), None, None),
        (Some(CF), Some("2500"), None),
        (Some(CF), None, Some("100000000000")),
        (Some(CF), Some("0"), Some("100000000000")),
        (Some(CF), Some("10001"), Some("100000000000")),
        (Some(CF), Some("-5"), Some("100000000000")),
        (Some(CF), Some("25.5"), Some("100000000000")),
        (Some(CF), Some("0.25"), Some("100000000000")),
        (Some(CF), Some("1e3"), Some("100000000000")),
        (Some(CF), Some("+2500"), Some("100000000000")),
        (Some(CF), Some(""), Some("100000000000")),
        (Some(CF), Some("2500"), Some("0")),
        (Some(CF), Some("2500"), Some("-1")),
        (Some(CF), Some("2500"), Some("1.5")),
        (Some(CF), Some("2500"), Some("99999999999999999999999")),
        (
            Some(CF),
            Some("99999999999999999999999"),
            Some("100000000000"),
        ),
    ];
    for (p, b, c) in cases {
        let i = inputs(*p, *b, *c);
        assert!(
            resolve_deployment_sizing_contract(&i).is_err(),
            "must refuse {p:?} {b:?} {c:?}"
        );
        assert!(
            build_plugin_registry_from_inputs(&i).is_err(),
            "registry build must refuse {p:?} {b:?} {c:?}"
        );
    }
}

#[test]
fn valid_capital_fraction_contract_is_bound_and_requires_durable_state() {
    let i = inputs(Some(CF), Some("2500"), Some("100000000000"));
    let c = resolve_deployment_sizing_contract(&i).unwrap();
    assert_eq!(c.policy, SizingPolicy::capital_fraction_v1(2500).unwrap());
    assert_eq!(c.allocated_capital_micros, Some(100_000_000_000));
    assert_eq!(
        c.identity_token(),
        "sz_policy=fixed_initial_capital_fraction_v1|sz_frac_bps=2500|sz_capital_micros=100000000000"
    );

    // Identity changes with every contract parameter.
    let tok = |b: &str, cap: &str| {
        resolve_deployment_sizing_contract(&inputs(Some(CF), Some(b), Some(cap)))
            .unwrap()
            .identity_token()
    };
    assert_ne!(tok("2500", "100000000000"), tok("2501", "100000000000"));
    assert_ne!(tok("2500", "100000000000"), tok("2500", "100000000001"));

    // The registry builds, but every entry requires durable held-state
    // recovery: the stateless seam refuses it with an explicit reason, and
    // nothing falls back to a fixed or one-share quantity.
    let registry = build_plugin_registry_from_inputs(&i).expect("capital-fraction registry builds");
    assert!(!registry.is_empty());
    for meta in registry.list() {
        // An unrecoverable engine stays unrecoverable; every other engine now
        // requires durable state. Neither is stateless-instantiable.
        let expected = if meta.name == "pullback_mean_reversion_20_2" {
            RestartRecovery::NotRecoverable
        } else {
            RestartRecovery::DurableStateRequired
        };
        assert_eq!(meta.restart_recovery, expected, "{}", meta.name);
        let Err(e) = registry.instantiate_verified(&meta.name) else {
            panic!("stateless instantiation of {} must be refused", meta.name)
        };
        assert!(
            matches!(
                e,
                RegistryError::DurableStateRequired { .. }
                    | RegistryError::NotRestartRecoverable { .. }
            ),
            "{e}"
        );
    }
    let bootstrap =
        NativeStrategyBootstrap::bootstrap(Some(&["swing_momentum".to_string()]), &registry);
    assert!(
        bootstrap.is_failed(),
        "the daemon bootstrap stays fail-closed"
    );
    assert!(bootstrap.failure_reason().unwrap().contains("durable"));
}

#[test]
fn contract_is_independent_of_target_qty_inputs() {
    let mut i = inputs(Some(CF), Some("2500"), Some("100000000000"));
    i.raw_target_qty = Some("7".to_string());
    let c = resolve_deployment_sizing_contract(&i).unwrap();
    assert!(c.policy.is_capital_fraction());
    assert!(build_plugin_registry_from_inputs(&i).is_err());
}
