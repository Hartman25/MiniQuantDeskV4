//! CUTOVER-1D-A3 correction: registry-v2 asset-class truth reaches the
//! production strategy bootstrap, and Crypto sizing is explicit and exact.
//!
//! Every case drives `bootstrap_with_effective_binding_from_inputs` (the body
//! of the production `bootstrap_with_effective_binding`) or that function via
//! the real process-env entry point -- never `IntradayScalperStrategy::with_sizing`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use mqk_md::instrument_registry_v2::{
    ContractDefinitionV2, InstrumentDefinitionV2, InstrumentEconomicsMetadataV2,
    InstrumentMetadataV2, InstrumentRegistryV2, SESSION_PROFILE_CRYPTO_24_7,
};
use mqk_runtime::native_strategy::{
    bootstrap_with_effective_binding, bootstrap_with_effective_binding_from_inputs,
    build_plugin_registry_from_inputs, NativeStrategyBootstrap, StrategyBootstrapInputs,
};
use mqk_strategy::{BarStub, RecentBarsWindow, StrategyBarResult};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn instrument(symbol: &str, asset_class: &str) -> InstrumentDefinitionV2 {
    InstrumentDefinitionV2 {
        instrument_id: format!("{asset_class}:GLOBAL:{}", symbol.replace('/', "")),
        symbol: symbol.to_string(),
        asset_class: asset_class.to_string(),
        instrument_kind: None,
        venue: Some("GLOBAL".to_string()),
        currency: "USD".to_string(),
        quote_currency: Some("USD".to_string()),
        provider_symbols: BTreeMap::new(),
        broker_symbols: BTreeMap::from([("alpaca".to_string(), symbol.to_string())]),
        enabled: false,
        paper_trading_enabled: true,
        live_trading_enabled: false,
        timeframes: vec!["5m".to_string()],
        contract: Some(ContractDefinitionV2::CryptoPair {
            base: "BTC".to_string(),
            quote: "USD".to_string(),
        }),
        metadata: InstrumentMetadataV2::default(),
        notes: None,
        allow_enabled_non_equity_for_testing: false,
        economics: Some(InstrumentEconomicsMetadataV2 {
            contract_multiplier: None,
            initial_margin_micros: None,
            maintenance_margin_micros: None,
            quantity_increment_micros: Some(100),
            min_trade_qty_micros: Some(100),
            price_tick_micros: Some(1_000_000),
            session_profile: Some(SESSION_PROFILE_CRYPTO_24_7.to_string()),
        }),
    }
}

/// Write a registry-v2 file and return its path (unique per process + tag).
fn registry_file(tag: &str, instruments: Vec<InstrumentDefinitionV2>) -> String {
    let registry = InstrumentRegistryV2 {
        schema_version: 1,
        instruments,
    };
    let path = std::env::temp_dir().join(format!("mqk_a3c_{}_{tag}.json", std::process::id()));
    std::fs::write(&path, serde_json::to_string(&registry).unwrap()).unwrap();
    path.to_string_lossy().into_owned()
}

fn btc_inputs(path: &str, raw_target: Option<&str>) -> StrategyBootstrapInputs {
    StrategyBootstrapInputs {
        symbol: "BTC/USD".to_string(),
        trading_registry_v2_path: Some(path.to_string()),
        raw_target_qty: raw_target.map(str::to_string),
        raw_max_target_qty: None,
        raw_max_notional_usd: None,
    }
}

fn rising_window() -> RecentBarsWindow {
    // 25 bps over the 5-bar lookback (> the 20 bps scalper threshold).
    let closes = [
        100_000_000,
        100_050_000,
        100_100_000,
        100_150_000,
        100_250_000,
    ];
    let bars = closes
        .iter()
        .enumerate()
        .map(|(i, c)| BarStub::new(1_000_000 + 300 * i as i64, true, *c, 100))
        .collect();
    RecentBarsWindow::new(5, bars)
}

fn fleet(id: &str) -> Vec<String> {
    vec![id.to_string()]
}

fn eval(bootstrap: &mut NativeStrategyBootstrap) -> Option<StrategyBarResult> {
    bootstrap.invoke_on_bar_from_window(1, rising_window())
}

#[test]
fn crypto_bootstrap_produces_exact_micro_target_for_explicit_size() {
    let path = registry_file("exact", vec![instrument("BTC/USD", "crypto")]);
    for id in ["intraday_scalper"] {
        let (mut b, binding) = bootstrap_with_effective_binding_from_inputs(
            Some(&fleet(id)),
            &btc_inputs(&path, Some("0.0001")),
        );
        assert!(b.is_active(), "{:?}", b.failure_reason());
        assert_eq!(
            binding.effective_runtime_target_symbol.as_deref(),
            Some("BTC/USD")
        );
        let result = eval(&mut b).expect("active bootstrap evaluates");
        let targets = &result.intents.output.targets;
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].symbol, "BTC/USD");
        assert_eq!(targets[0].qty.raw(), 100, "0.0001 BTC == 100 QtyMicros");
    }
    // A different exact size flows through unchanged (not a fixed constant).
    let (mut b, _) = bootstrap_with_effective_binding_from_inputs(
        Some(&fleet("intraday_scalper")),
        &btc_inputs(&path, Some("0.000300")),
    );
    assert_eq!(
        eval(&mut b).unwrap().intents.output.targets[0].qty.raw(),
        300
    );
}

#[test]
fn crypto_bootstrap_without_valid_explicit_size_fails_closed_and_cannot_decide() {
    let path = registry_file("refuse", vec![instrument("BTC/USD", "crypto")]);
    let refused: [Option<&str>; 9] = [
        None,
        Some(""),
        Some("   "),
        Some("abc"),
        Some("0"),
        Some("-0.0001"),
        Some("0.0000001"), // > 6 decimals
        Some("0.00005"),   // exact, but not a multiple of the registry increment
        Some("0.00001"),   // exact, but below the registry minimum trade quantity
    ];
    for raw in refused {
        let (mut b, _) = bootstrap_with_effective_binding_from_inputs(
            Some(&fleet("intraday_scalper")),
            &btc_inputs(&path, raw),
        );
        assert!(b.is_failed(), "size {raw:?} must fail closed");
        assert!(b.failure_reason().is_some());
        assert!(
            eval(&mut b).is_none(),
            "size {raw:?}: a failed bootstrap must not produce any decision input"
        );
    }
    // Never a default of one unit: the only way to size is explicit.
    let (b, _) = bootstrap_with_effective_binding_from_inputs(
        Some(&fleet("intraday_scalper")),
        &btc_inputs(&path, None),
    );
    assert!(b.failure_reason().unwrap().contains("MissingExplicitSize"));
}

#[test]
fn crypto_registers_only_sizing_configurable_identities() {
    let path = registry_file("ids", vec![instrument("BTC/USD", "crypto")]);
    let registry = build_plugin_registry_from_inputs(&btc_inputs(&path, Some("0.0001"))).unwrap();
    let names: Vec<_> = registry.list().iter().map(|m| m.name.clone()).collect();
    assert_eq!(names, ["intraday_scalper", "intraday_short_scalper"]);
    for id in ["swing_momentum", "mean_reversion", "volatility_breakout"] {
        let (b, _) = bootstrap_with_effective_binding_from_inputs(
            Some(&fleet(id)),
            &btc_inputs(&path, Some("0.0001")),
        );
        assert!(b.is_failed(), "{id} must not trade a Crypto instrument");
    }
}

#[test]
fn registry_failures_and_unsupported_classes_fail_closed() {
    // Unreadable registry path.
    let (b, _) = bootstrap_with_effective_binding_from_inputs(
        Some(&fleet("intraday_scalper")),
        &btc_inputs("/nonexistent/mqk_a3c_registry.json", Some("0.0001")),
    );
    assert!(b.is_failed());

    // Test-only bypass flag is never production sizing authority.
    let mut bypass = instrument("BTC/USD", "crypto");
    bypass.allow_enabled_non_equity_for_testing = true;
    let path = registry_file("bypass", vec![bypass]);
    let (b, _) = bootstrap_with_effective_binding_from_inputs(
        Some(&fleet("intraday_scalper")),
        &btc_inputs(&path, Some("0.0001")),
    );
    assert!(b.is_failed());

    // Non-crypto, non-equity class has no sizing policy.
    let mut fx = instrument("EUR/USD", "forex");
    fx.contract = Some(ContractDefinitionV2::ForexPair {
        base: "EUR".to_string(),
        quote: "USD".to_string(),
    });
    fx.paper_trading_enabled = false;
    fx.economics = None;
    let path = registry_file("forex", vec![fx]);
    let mut inputs = btc_inputs(&path, Some("1"));
    inputs.symbol = "EUR/USD".to_string();
    let (b, _) =
        bootstrap_with_effective_binding_from_inputs(Some(&fleet("intraday_scalper")), &inputs);
    assert!(
        b.failure_reason()
            .unwrap()
            .contains("no supported target-sizing policy"),
        "{:?}",
        b.failure_reason()
    );

    // Crypto row lacking economics: validated-never-chosen needs the economics.
    let mut no_econ = instrument("BTC/USD", "crypto");
    no_econ.economics = None;
    let path = registry_file("noecon", vec![no_econ]);
    let (b, _) = bootstrap_with_effective_binding_from_inputs(
        Some(&fleet("intraday_scalper")),
        &btc_inputs(&path, Some("0.0001")),
    );
    assert!(b.is_failed());
}

#[test]
fn sizing_refusal_is_irrelevant_when_no_strategy_is_selected() {
    let path = registry_file("dormant", vec![instrument("BTC/USD", "crypto")]);
    let (b, _) = bootstrap_with_effective_binding_from_inputs(None, &btc_inputs(&path, None));
    assert!(b.is_dormant());
}

#[test]
fn asset_class_is_registry_truth_never_symbol_spelling() {
    // "BTC/USD" absent from the registry (and no registry at all) keeps
    // historical Equity sizing semantics; crypto behavior is never inferred
    // from the spelling. Admission (registry-v2 / legacy-Equity gate) is the
    // authority that refuses such a symbol.
    let path = registry_file("other", vec![instrument("ETH/USD", "crypto")]);
    for path in [Some(path), None] {
        let inputs = StrategyBootstrapInputs {
            symbol: "BTC/USD".to_string(),
            trading_registry_v2_path: path,
            raw_target_qty: None,
            raw_max_target_qty: None,
            raw_max_notional_usd: None,
        };
        let (mut b, _) =
            bootstrap_with_effective_binding_from_inputs(Some(&fleet("intraday_scalper")), &inputs);
        assert!(b.is_active());
        assert_eq!(
            eval(&mut b).unwrap().intents.output.targets[0].qty.raw(),
            1_000_000
        );
    }
    // The inverse: a registry crypto row named like an equity ticker is Crypto.
    let path = registry_file("spelling", vec![instrument("AAPL", "crypto")]);
    let mut inputs = btc_inputs(&path, None);
    inputs.symbol = "AAPL".to_string();
    let (b, _) =
        bootstrap_with_effective_binding_from_inputs(Some(&fleet("intraday_scalper")), &inputs);
    assert!(b.is_failed());
}

#[test]
fn equity_bootstrap_is_unchanged_by_the_registry_seam() {
    let mut equity = instrument("AAPL", "equity");
    equity.contract = None;
    equity.economics = None;
    let path = registry_file("equity", vec![equity]);
    for reg_path in [Some(path), None] {
        for (raw, expected) in [
            (None, 1_000_000),
            (Some("7"), 7_000_000),
            (Some("0"), 1_000_000),
        ] {
            let inputs = StrategyBootstrapInputs {
                symbol: "AAPL".to_string(),
                trading_registry_v2_path: reg_path.clone(),
                raw_target_qty: raw.map(str::to_string),
                raw_max_target_qty: None,
                raw_max_notional_usd: None,
            };
            let (mut b, _) = bootstrap_with_effective_binding_from_inputs(
                Some(&fleet("intraday_scalper")),
                &inputs,
            );
            assert!(b.is_active());
            assert_eq!(
                eval(&mut b).unwrap().intents.output.targets[0].qty.raw(),
                expected,
                "equity target {raw:?}"
            );
        }
    }
    // Full built-in set + fingerprints match the explicit-sizing registration.
    let inputs = StrategyBootstrapInputs {
        symbol: "AAPL".to_string(),
        trading_registry_v2_path: None,
        raw_target_qty: Some("7".to_string()),
        raw_max_target_qty: Some("3".to_string()),
        raw_max_notional_usd: Some("900".to_string()),
    };
    let built = build_plugin_registry_from_inputs(&inputs).unwrap();
    let mut explicit = mqk_strategy::PluginRegistry::new();
    mqk_strategy::engines::register_builtin_strategies_with_sizing(
        &mut explicit,
        "AAPL",
        7,
        Some(3),
        Some(900),
    )
    .unwrap();
    for id in [
        "swing_momentum",
        "mean_reversion",
        "volatility_breakout",
        "intraday_scalper",
    ] {
        assert_eq!(
            built.instantiate(id).unwrap().semantic_fingerprint(),
            explicit.instantiate(id).unwrap().semantic_fingerprint(),
            "{id}"
        );
    }
    assert_eq!(built.list().len(), 5);
}

/// The real production entry point: process env -> bootstrap -> strategy ->
/// TargetPosition.
#[test]
fn production_env_entry_point_sizes_crypto_exactly_or_refuses() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = registry_file("env", vec![instrument("BTC/USD", "crypto")]);
    let names = [
        "MQK_STRATEGY_SYMBOL",
        "MQK_TRADING_INSTRUMENT_REGISTRY_V2_PATH",
        "MQK_STRATEGY_TARGET_QTY",
        "MQK_STRATEGY_MAX_TARGET_QTY",
        "MQK_STRATEGY_MAX_POSITION_NOTIONAL_USD",
    ];
    let saved: Vec<_> = names.iter().map(|n| (*n, std::env::var(n).ok())).collect();
    for n in names {
        std::env::remove_var(n);
    }
    std::env::set_var("MQK_STRATEGY_SYMBOL", "BTC/USD");
    std::env::set_var("MQK_TRADING_INSTRUMENT_REGISTRY_V2_PATH", &path);

    let ids = fleet("intraday_scalper");

    // Missing explicit size: no decision can be produced.
    let (mut refused, _) = bootstrap_with_effective_binding(Some(&ids));
    let refused_is_failed = refused.is_failed();
    let refused_result = eval(&mut refused).is_none();

    // Explicit exact size: 0.0001 BTC == raw 100.
    std::env::set_var("MQK_STRATEGY_TARGET_QTY", "0.0001");
    let (mut ok, _) = bootstrap_with_effective_binding(Some(&ids));
    let ok_active = ok.is_active();
    let ok_raw = eval(&mut ok).map(|r| r.intents.output.targets[0].qty.raw());

    for (n, v) in saved {
        match v {
            Some(v) => std::env::set_var(n, v),
            None => std::env::remove_var(n),
        }
    }
    assert!(refused_is_failed && refused_result);
    assert!(ok_active);
    assert_eq!(ok_raw, Some(100));
}
