//! ASSET-CORE-03-RISK-ROUTER-FOUNDATION-01-COMBINED.
//!
//! Pure model-level tests only. These tests do not construct a gateway,
//! broker adapter, runtime, DB pool, OMS row, daemon route, or risk engine.

use mqk_execution::{
    asset_risk_policy_for_asset_class, default_asset_risk_policies, equity_instrument,
    evaluate_asset_risk_for_order_intent_v2,
    evaluate_asset_risk_for_order_intent_v2_with_caller_routing_request, AssetRiskPolicyState,
    AssetRiskRouteDecision, IntentV2Contract, OrderIntentV2, ASSET_RISK_NON_EQUITY_ROUTING_ENABLED,
    ASSET_RISK_POLICY_SOURCE, ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED,
};
use mqk_schemas::{
    AssetClass, ContractSpec, Instrument, OptionRight, OrderSide, OrderType, QtyMicros,
};

const ONE_UNIT: i64 = 1_000_000;

fn qty(units: i64) -> QtyMicros {
    QtyMicros::new(units * ONE_UNIT)
}

fn equity_stock_intent() -> OrderIntentV2 {
    OrderIntentV2::new(equity_instrument("AAPL"), OrderSide::Buy, qty(10))
        .with_instrument_id("equity:US:AAPL")
}

fn etf_as_equity_intent() -> OrderIntentV2 {
    OrderIntentV2::new(equity_instrument("SPY"), OrderSide::Buy, qty(5))
        .with_instrument_id("equity:US:SPY")
        .with_contract(IntentV2Contract::Equity {
            instrument_kind: Some("etf".to_string()),
        })
}

fn crypto_intent() -> OrderIntentV2 {
    let instrument = Instrument {
        symbol: "BTC/USD".to_string(),
        asset_class: AssetClass::Crypto,
        venue: Some("coinbase".to_string()),
        currency: "USD".to_string(),
        contract: ContractSpec::Crypto,
    };
    OrderIntentV2::new(instrument, OrderSide::Buy, QtyMicros::new(250_000)).with_contract(
        IntentV2Contract::CryptoSpot {
            base_currency: "BTC".to_string(),
            quote_currency: "USD".to_string(),
        },
    )
}

fn future_intent() -> OrderIntentV2 {
    let instrument = Instrument {
        symbol: "MESM26".to_string(),
        asset_class: AssetClass::Future,
        venue: Some("CME".to_string()),
        currency: "USD".to_string(),
        contract: ContractSpec::Future {
            root: "MES".to_string(),
            expiry_yyyymm: "202606".to_string(),
            multiplier: 5,
            tick_size_micros: 250_000,
        },
    };
    OrderIntentV2::new(instrument, OrderSide::Buy, qty(1))
}

fn option_intent() -> OrderIntentV2 {
    let instrument = Instrument {
        symbol: "AAPL260619C00200000".to_string(),
        asset_class: AssetClass::Option,
        venue: Some("OPRA".to_string()),
        currency: "USD".to_string(),
        contract: ContractSpec::Option {
            underlying: "AAPL".to_string(),
            expiry_yyyymmdd: "20260619".to_string(),
            strike_micros: 200_000_000,
            right: OptionRight::Call,
            multiplier: 100,
        },
    };
    OrderIntentV2::new(instrument, OrderSide::Buy, qty(1))
}

fn forex_intent() -> OrderIntentV2 {
    let instrument = Instrument {
        symbol: "EUR/USD".to_string(),
        asset_class: AssetClass::Forex,
        venue: Some("IDEALPRO".to_string()),
        currency: "USD".to_string(),
        contract: ContractSpec::Crypto,
    };
    OrderIntentV2::new(instrument, OrderSide::Buy, qty(10)).with_contract(
        IntentV2Contract::ForexPair {
            base_currency: "EUR".to_string(),
            quote_currency: "USD".to_string(),
        },
    )
}

#[test]
fn asset_risk_policy_source_is_static_model_only() {
    assert_eq!(ASSET_RISK_POLICY_SOURCE, "static_default_foundation");
    const {
        assert!(!ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED);
        assert!(!ASSET_RISK_NON_EQUITY_ROUTING_ENABLED);
    }
}

#[test]
fn default_asset_risk_policies_keep_non_equity_disabled_or_research_only() {
    let policies = default_asset_risk_policies();

    let equity = policies
        .iter()
        .find(|p| p.asset_class == "equity")
        .expect("equity policy");
    assert_eq!(equity.state, AssetRiskPolicyState::Enabled);
    assert!(equity.paper_trading_enabled);
    assert!(!equity.live_trading_enabled);

    for asset_class in ["crypto", "future", "option", "forex"] {
        let policy = policies
            .iter()
            .find(|p| p.asset_class == asset_class)
            .unwrap_or_else(|| panic!("{asset_class} policy"));
        assert_eq!(policy.state, AssetRiskPolicyState::Disabled);
        assert!(!policy.paper_trading_enabled, "{asset_class}");
        assert!(!policy.live_trading_enabled, "{asset_class}");
    }

    let rates = policies
        .iter()
        .find(|p| p.asset_class == "rates_fixed_income")
        .expect("rates/fixed-income scaffold policy");
    assert_eq!(rates.state, AssetRiskPolicyState::ResearchOnly);
    assert!(!rates.paper_trading_enabled);
    assert!(!rates.live_trading_enabled);
}

#[test]
fn etf_is_policy_modeled_as_equity_kind_not_standalone_asset_class() {
    let intent = etf_as_equity_intent();
    assert_eq!(intent.instrument.asset_class, AssetClass::Equity);
    assert_eq!(intent.instrument.contract, ContractSpec::Equity);
    assert_eq!(
        intent.contract,
        IntentV2Contract::Equity {
            instrument_kind: Some("etf".to_string())
        }
    );

    let evaluation = evaluate_asset_risk_for_order_intent_v2(&intent);
    assert_eq!(evaluation.decision, AssetRiskRouteDecision::AllowedEquity);
    assert_eq!(evaluation.policy.asset_class, "etf_as_equity");
    assert_eq!(evaluation.policy.reason_code, "etf_inherits_equity_policy");
}

#[test]
fn equity_order_intent_v2_routes_to_model_level_allowed_equity() {
    let evaluation = evaluate_asset_risk_for_order_intent_v2(&equity_stock_intent());
    assert!(evaluation.validation.valid, "{evaluation:?}");
    assert_eq!(evaluation.decision, AssetRiskRouteDecision::AllowedEquity);
    assert_eq!(evaluation.policy.asset_class, "equity");
    assert_eq!(evaluation.reason_code, "equity_model_candidate");
}

#[test]
fn crypto_order_intent_v2_routes_to_disabled_asset_class() {
    let evaluation = evaluate_asset_risk_for_order_intent_v2(&crypto_intent());
    assert!(evaluation.validation.valid, "{evaluation:?}");
    assert_eq!(
        evaluation.decision,
        AssetRiskRouteDecision::DisabledAssetClass
    );
    assert_eq!(evaluation.policy.asset_class, "crypto");
}

#[test]
fn future_order_intent_v2_routes_to_disabled_asset_class() {
    let evaluation = evaluate_asset_risk_for_order_intent_v2(&future_intent());
    assert!(evaluation.validation.valid, "{evaluation:?}");
    assert_eq!(
        evaluation.decision,
        AssetRiskRouteDecision::DisabledAssetClass
    );
    assert_eq!(evaluation.policy.asset_class, "future");
}

#[test]
fn option_order_intent_v2_routes_to_disabled_asset_class() {
    let evaluation = evaluate_asset_risk_for_order_intent_v2(&option_intent());
    assert!(evaluation.validation.valid, "{evaluation:?}");
    assert_eq!(
        evaluation.decision,
        AssetRiskRouteDecision::DisabledAssetClass
    );
    assert_eq!(evaluation.policy.asset_class, "option");
}

#[test]
fn forex_order_intent_v2_routes_to_disabled_asset_class() {
    let evaluation = evaluate_asset_risk_for_order_intent_v2(&forex_intent());
    assert!(evaluation.validation.valid, "{evaluation:?}");
    assert_eq!(
        evaluation.decision,
        AssetRiskRouteDecision::DisabledAssetClass
    );
    assert_eq!(evaluation.policy.asset_class, "forex");
}

#[test]
fn invalid_qty_intent_routes_to_invalid_before_policy_routing() {
    let mut intent = equity_stock_intent();
    intent.qty = QtyMicros::ZERO;

    let evaluation = evaluate_asset_risk_for_order_intent_v2(&intent);
    assert!(!evaluation.validation.valid, "{evaluation:?}");
    assert_eq!(evaluation.decision, AssetRiskRouteDecision::Invalid);
    assert_eq!(evaluation.validation.reason_code, "invalid_qty");
    assert_eq!(evaluation.reason_code, "intent_invalid");
}

#[test]
fn missing_limit_and_stop_prices_route_to_invalid_before_policy_routing() {
    let missing_limit = evaluate_asset_risk_for_order_intent_v2(
        &equity_stock_intent().with_order_type(OrderType::Limit),
    );
    assert!(!missing_limit.validation.valid, "{missing_limit:?}");
    assert_eq!(missing_limit.decision, AssetRiskRouteDecision::Invalid);
    assert_eq!(missing_limit.validation.reason_code, "missing_limit_price");

    let missing_stop = evaluate_asset_risk_for_order_intent_v2(
        &equity_stock_intent().with_order_type(OrderType::Stop),
    );
    assert!(!missing_stop.validation.valid, "{missing_stop:?}");
    assert_eq!(missing_stop.decision, AssetRiskRouteDecision::Invalid);
    assert_eq!(missing_stop.validation.reason_code, "missing_stop_price");
}

#[test]
fn caller_flag_cannot_force_disabled_non_equity_to_allowed() {
    let evaluation =
        evaluate_asset_risk_for_order_intent_v2_with_caller_routing_request(&crypto_intent(), true);
    assert!(evaluation.validation.valid, "{evaluation:?}");
    assert_eq!(
        evaluation.decision,
        AssetRiskRouteDecision::DisabledAssetClass
    );
    assert!(!evaluation.policy.paper_trading_enabled);
    assert!(!evaluation.policy.live_trading_enabled);
}

#[test]
fn policy_summary_marks_required_dependencies_for_non_equity() {
    let crypto = asset_risk_policy_for_asset_class("crypto");
    assert!(crypto.requires_session_profile);
    assert!(crypto.requires_currency_conversion);
    assert!(!crypto.requires_contract_multiplier);

    let future = asset_risk_policy_for_asset_class("future");
    assert!(future.requires_margin_model);
    assert!(future.requires_contract_multiplier);
    assert!(future.requires_session_profile);

    let option = asset_risk_policy_for_asset_class("option");
    assert!(option.requires_margin_model);
    assert!(option.requires_contract_multiplier);
    assert!(option.requires_session_profile);

    let forex = asset_risk_policy_for_asset_class("forex");
    assert!(forex.requires_margin_model);
    assert!(forex.requires_currency_conversion);
    assert!(forex.requires_session_profile);
}

#[test]
fn unknown_policy_name_is_unsupported_not_routable() {
    let policy = asset_risk_policy_for_asset_class("commodity_swap");
    assert_eq!(policy.state, AssetRiskPolicyState::Unsupported);
    assert!(!policy.paper_trading_enabled);
    assert!(!policy.live_trading_enabled);
}
