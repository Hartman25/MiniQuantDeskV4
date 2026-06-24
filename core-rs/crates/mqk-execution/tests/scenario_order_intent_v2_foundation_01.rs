//! ASSET-CORE-02-ORDER-INTENT-V2-FOUNDATION-01-COMBINED.
//!
//! Pure model-level tests only. These fixtures do not construct a gateway,
//! broker adapter, runtime, DB pool, OMS row, or daemon route.

use mqk_execution::{
    equity_instrument, ExecutionIntentV2, IntentV2Contract, IntentV2Routability, OrderIntentV2,
};
use mqk_schemas::{
    AssetClass, ContractSpec, Instrument, OptionRight, OrderSide, OrderType, QtyMicros,
};

const ONE_UNIT: i64 = 1_000_000;

fn qty(units: i64) -> QtyMicros {
    QtyMicros::new(units * ONE_UNIT)
}

fn order_intent_v2_equity_stock_fixture() -> OrderIntentV2 {
    OrderIntentV2::new(equity_instrument("AAPL"), OrderSide::Buy, qty(10))
        .with_instrument_id("equity:US:AAPL")
        .with_strategy_source("asset-core-02-test", "order_intent_v2_fixture")
}

fn order_intent_v2_etf_as_equity_fixture() -> OrderIntentV2 {
    OrderIntentV2::new(equity_instrument("SPY"), OrderSide::Buy, qty(5))
        .with_instrument_id("equity:US:SPY")
        .with_contract(IntentV2Contract::Equity {
            instrument_kind: Some("etf".to_string()),
        })
}

fn order_intent_v2_crypto_fixture() -> OrderIntentV2 {
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

fn order_intent_v2_future_fixture() -> OrderIntentV2 {
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

fn order_intent_v2_option_fixture() -> OrderIntentV2 {
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

fn order_intent_v2_forex_fixture() -> OrderIntentV2 {
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
fn order_intent_v2_equity_fixture_validates_as_model_level_candidate() {
    let validation = order_intent_v2_equity_stock_fixture().validate_model();
    assert!(validation.valid, "{validation:?}");
    assert_eq!(
        validation.routability,
        IntentV2Routability::EquityRoutableCandidate
    );
    assert_eq!(validation.reason_code, "equity_model_candidate");
}

#[test]
fn order_intent_v2_etf_fixture_validates_as_equity_not_separate_asset_class() {
    let intent = order_intent_v2_etf_as_equity_fixture();
    assert_eq!(intent.instrument.asset_class, AssetClass::Equity);
    assert_eq!(intent.instrument.contract, ContractSpec::Equity);
    assert_eq!(
        intent.contract,
        IntentV2Contract::Equity {
            instrument_kind: Some("etf".to_string())
        }
    );

    let validation = intent.validate_model();
    assert!(validation.valid, "{validation:?}");
    assert_eq!(
        validation.routability,
        IntentV2Routability::EquityRoutableCandidate
    );
}

#[test]
fn order_intent_v2_crypto_fixture_is_representable_but_disabled() {
    let validation = order_intent_v2_crypto_fixture().validate_model();
    assert!(validation.valid, "{validation:?}");
    assert_eq!(
        validation.routability,
        IntentV2Routability::DisabledAssetClass
    );
    assert_eq!(validation.reason_code, "asset_class_disabled");
}

#[test]
fn order_intent_v2_future_fixture_is_representable_but_disabled() {
    let validation = order_intent_v2_future_fixture().validate_model();
    assert!(validation.valid, "{validation:?}");
    assert_eq!(
        validation.routability,
        IntentV2Routability::DisabledAssetClass
    );
    assert_eq!(validation.reason_code, "asset_class_disabled");
}

#[test]
fn order_intent_v2_option_fixture_is_representable_but_disabled() {
    let validation = order_intent_v2_option_fixture().validate_model();
    assert!(validation.valid, "{validation:?}");
    assert_eq!(
        validation.routability,
        IntentV2Routability::DisabledAssetClass
    );
    assert_eq!(validation.reason_code, "asset_class_disabled");
}

#[test]
fn order_intent_v2_forex_fixture_is_representable_but_disabled() {
    let validation = order_intent_v2_forex_fixture().validate_model();
    assert!(validation.valid, "{validation:?}");
    assert_eq!(
        validation.routability,
        IntentV2Routability::DisabledAssetClass
    );
    assert_eq!(validation.reason_code, "asset_class_disabled");
}

#[test]
fn order_intent_v2_invalid_qty_fails_validation() {
    let mut intent = order_intent_v2_equity_stock_fixture();
    intent.qty = QtyMicros::ZERO;

    let validation = intent.validate_model();
    assert!(!validation.valid, "{validation:?}");
    assert_eq!(validation.routability, IntentV2Routability::Invalid);
    assert_eq!(validation.reason_code, "invalid_qty");
}

#[test]
fn order_intent_v2_missing_required_limit_price_fails_validation() {
    let intent = order_intent_v2_equity_stock_fixture().with_order_type(OrderType::Limit);

    let validation = intent.validate_model();
    assert!(!validation.valid, "{validation:?}");
    assert_eq!(validation.routability, IntentV2Routability::Invalid);
    assert_eq!(validation.reason_code, "missing_limit_price");
}

#[test]
fn order_intent_v2_caller_flag_cannot_make_disabled_non_equity_routable() {
    let validation =
        order_intent_v2_crypto_fixture().validate_model_with_caller_routing_request(true);
    assert!(validation.valid, "{validation:?}");
    assert_eq!(
        validation.routability,
        IntentV2Routability::DisabledAssetClass
    );
    assert_eq!(validation.reason_code, "asset_class_disabled");
}

#[test]
fn existing_equity_helper_remains_unchanged() {
    let instrument = equity_instrument("MSFT");
    assert_eq!(instrument.symbol, "MSFT");
    assert_eq!(instrument.asset_class, AssetClass::Equity);
    assert_eq!(instrument.venue, None);
    assert_eq!(instrument.currency, "USD");
    assert_eq!(instrument.contract, ContractSpec::Equity);
}

#[test]
fn execution_intent_v2_market_equity_validates_as_model_level_candidate() {
    let intent = ExecutionIntentV2::market(
        "exec-v2-1".to_string(),
        equity_instrument("AAPL"),
        OrderSide::Buy,
        qty(1),
    );

    let validation = intent.validate_model();
    assert!(validation.valid, "{validation:?}");
    assert_eq!(
        validation.routability,
        IntentV2Routability::EquityRoutableCandidate
    );
    assert_eq!(validation.reason_code, "equity_model_candidate");
}
