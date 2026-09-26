//! M7/M8 executable-contract identity negative controls (RC-M8-A).
//!
//! Model-level only: no gateway, broker, DB or daemon. Every case goes
//! through the public validators, so the same invariant is proven for the
//! intent model, the order-spec model, and the options permission classifier.
//!
//! | ID   | Invariant                                                             |
//! |------|-----------------------------------------------------------------------|
//! | CI01 | future expiry must be a real `YYYYMM`                                  |
//! | CI02 | option expiry must be a real `YYYYMMDD`                                |
//! | CI03 | futures/options quantities are whole contracts (never fractional)      |
//! | CI04 | pair legs (FX / crypto) must be distinct                               |
//! | CI05 | the options permission classifier applies CI02/CI03 to every leg       |

use mqk_execution::{
    classify_option_strategy_structure, ExecutionIntentV2, IntentV2Contract, OptionLegSide,
    OptionStrategyRefusal, OrderIntentV2, ProposedOptionLeg, ProposedOptionStrategy,
};
use mqk_schemas::{
    AssetClass, ContractSpec, Instrument, OptionRight, OrderSide, QtyMicros, QTY_MICROS_SCALE,
};

fn whole(units: i64) -> QtyMicros {
    QtyMicros::new(units * QTY_MICROS_SCALE)
}

fn future_instrument(expiry_yyyymm: &str) -> Instrument {
    Instrument {
        symbol: "MESM26".to_string(),
        asset_class: AssetClass::Future,
        venue: Some("CME".to_string()),
        currency: "USD".to_string(),
        contract: ContractSpec::Future {
            root: "MES".to_string(),
            expiry_yyyymm: expiry_yyyymm.to_string(),
            multiplier: 5,
            tick_size_micros: 250_000,
        },
    }
}

fn option_instrument(expiry_yyyymmdd: &str) -> Instrument {
    Instrument {
        symbol: "AAPL260619C00200000".to_string(),
        asset_class: AssetClass::Option,
        venue: Some("OPRA".to_string()),
        currency: "USD".to_string(),
        contract: ContractSpec::Option {
            underlying: "AAPL".to_string(),
            expiry_yyyymmdd: expiry_yyyymmdd.to_string(),
            strike_micros: 200_000_000,
            right: OptionRight::Call,
            multiplier: 100,
        },
    }
}

/// Validity through BOTH model entry points (intent + order spec).
fn model_valid(instrument: Instrument, qty: QtyMicros) -> bool {
    let intent = OrderIntentV2::new(instrument.clone(), OrderSide::Buy, qty).validate_model();
    let spec = ExecutionIntentV2::market("cid".to_string(), instrument, OrderSide::Buy, qty)
        .validate_model();
    assert_eq!(
        intent.valid, spec.valid,
        "intent and order-spec models must agree: {intent:?} vs {spec:?}"
    );
    intent.valid
}

#[test]
fn ci01_future_expiry_must_be_a_real_year_month() {
    for good in ["202606", "202612", "203001"] {
        assert!(model_valid(future_instrument(good), whole(1)), "{good:?}");
    }
    for bad in [
        "banana", "2026", "202613", "202600", "2026-06", "26_06", " 202606", "202606 ", "2026061",
        "",
    ] {
        assert!(!model_valid(future_instrument(bad), whole(1)), "{bad:?}");
    }
}

#[test]
fn ci02_option_expiry_must_be_a_real_calendar_date() {
    for good in ["20260619", "20280229", "20261231"] {
        assert!(model_valid(option_instrument(good), whole(1)), "{good:?}");
    }
    for bad in [
        "banana",
        "2026-06-19",
        "20260631",
        "20270229",
        "20261301",
        "20260600",
        "20260619 ",
        "2026061",
        "202606190",
        "",
    ] {
        assert!(!model_valid(option_instrument(bad), whole(1)), "{bad:?}");
    }
}

#[test]
fn ci03_futures_and_options_trade_whole_contracts_only() {
    let half = QtyMicros::new(500_000);
    let one_and_a_bit = QtyMicros::new(1_000_001);
    assert!(model_valid(future_instrument("202606"), whole(3)));
    assert!(model_valid(option_instrument("20260619"), whole(3)));
    for q in [half, one_and_a_bit] {
        assert!(!model_valid(future_instrument("202606"), q), "future {q}");
        assert!(!model_valid(option_instrument("20260619"), q), "option {q}");
    }
}

fn pair_intent(class: AssetClass, base: &str, quote: &str) -> OrderIntentV2 {
    let contract = match class {
        AssetClass::Forex => IntentV2Contract::ForexPair {
            base_currency: base.to_string(),
            quote_currency: quote.to_string(),
        },
        _ => IntentV2Contract::CryptoSpot {
            base_currency: base.to_string(),
            quote_currency: quote.to_string(),
        },
    };
    let instrument = Instrument {
        symbol: "PAIR".to_string(),
        asset_class: class,
        venue: None,
        currency: "USD".to_string(),
        contract: ContractSpec::Crypto,
    };
    OrderIntentV2::new(instrument, OrderSide::Buy, whole(1)).with_contract(contract)
}

#[test]
fn ci04_pair_legs_must_be_distinct() {
    for class in [AssetClass::Forex, AssetClass::Crypto] {
        assert!(
            pair_intent(class, "EUR", "USD").validate_model().valid,
            "{class:?} EUR/USD"
        );
        for (base, quote) in [("EUR", "EUR"), ("eur", "EUR"), (" USD ", "USD")] {
            assert!(
                !pair_intent(class, base, quote).validate_model().valid,
                "{class:?} {base:?}/{quote:?}"
            );
        }
    }
}

fn leg(
    side: OptionLegSide,
    right: OptionRight,
    strike: i64,
    expiry: &str,
    qty: QtyMicros,
) -> ProposedOptionLeg {
    ProposedOptionLeg {
        side,
        right,
        strike_micros: strike,
        expiry_yyyymmdd: expiry.to_string(),
        qty,
        multiplier: 100,
    }
}

fn strategy(legs: Vec<ProposedOptionLeg>) -> ProposedOptionStrategy {
    ProposedOptionStrategy {
        underlying: "AAPL".to_string(),
        legs,
        covering_share_qty: None,
        cash_secured_collateral_micros: None,
    }
}

fn is_invalid_leg(result: Result<impl std::fmt::Debug, OptionStrategyRefusal>) -> bool {
    matches!(result, Err(OptionStrategyRefusal::InvalidLeg { .. }))
}

#[test]
fn ci05_permission_classifier_refuses_malformed_expiry_and_fractional_contracts() {
    let long_call = |expiry: &str, qty: QtyMicros| {
        classify_option_strategy_structure(&strategy(vec![leg(
            OptionLegSide::Long,
            OptionRight::Call,
            200_000_000,
            expiry,
            qty,
        )]))
    };
    assert!(long_call("20260619", whole(1)).is_ok());
    for bad in ["banana", "2026-06-19", "20260631", "20270229", "20261301"] {
        assert!(is_invalid_leg(long_call(bad, whole(1))), "expiry {bad:?}");
    }
    for q in [QtyMicros::new(500_000), QtyMicros::new(1_000_001)] {
        assert!(is_invalid_leg(long_call("20260619", q)), "qty {q}");
    }

    // A vertical spread with equal fractional legs is not a permitted shape.
    let half = QtyMicros::new(500_000);
    let spread = strategy(vec![
        leg(
            OptionLegSide::Long,
            OptionRight::Call,
            200_000_000,
            "20260619",
            half,
        ),
        leg(
            OptionLegSide::Short,
            OptionRight::Call,
            210_000_000,
            "20260619",
            half,
        ),
    ]);
    assert!(is_invalid_leg(classify_option_strategy_structure(&spread)));

    // The whole-contract spread is still permitted (control).
    let ok = strategy(vec![
        leg(
            OptionLegSide::Long,
            OptionRight::Call,
            200_000_000,
            "20260619",
            whole(2),
        ),
        leg(
            OptionLegSide::Short,
            OptionRight::Call,
            210_000_000,
            "20260619",
            whole(2),
        ),
    ]);
    assert!(classify_option_strategy_structure(&ok).is_ok());
}
