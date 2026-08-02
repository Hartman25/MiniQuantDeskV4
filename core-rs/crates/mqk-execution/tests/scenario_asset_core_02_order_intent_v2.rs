//! ASSET-CORE-02B-ORDER-INTENT-V2-SAFE-GAP-CLOSURE-01.
//!
//! Pure model-level tests only for the bracket/OCO gap identified by
//! `ASSET-CORE-02-03A-CURRENT-COMPLETION-AUDIT-01`. These fixtures do not
//! construct a gateway, broker adapter, runtime, DB pool, OMS row, or daemon
//! route.

use mqk_execution::{equity_instrument, BracketLegs, IntentV2Routability, OrderIntentV2};
use mqk_schemas::{AssetClass, ContractSpec, Instrument, OrderSide, QtyMicros};

const ONE_UNIT: i64 = 1_000_000;

fn qty(units: i64) -> QtyMicros {
    QtyMicros::new(units * ONE_UNIT)
}

fn equity_intent() -> OrderIntentV2 {
    OrderIntentV2::new(equity_instrument("AAPL"), OrderSide::Buy, qty(10))
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
        mqk_execution::IntentV2Contract::CryptoSpot {
            base_currency: "BTC".to_string(),
            quote_currency: "USD".to_string(),
        },
    )
}

#[test]
fn equity_intent_without_bracket_is_unaffected_regression() {
    let validation = equity_intent().validate_model();
    assert!(validation.valid, "{validation:?}");
    assert_eq!(
        validation.routability,
        IntentV2Routability::EquityRoutableCandidate
    );
    assert_eq!(validation.reason_code, "equity_model_candidate");
}

#[test]
fn equity_intent_with_valid_bracket_legs_is_structurally_valid_but_non_routable() {
    let intent = equity_intent()
        .with_bracket_legs(BracketLegs::new(Some(150 * ONE_UNIT), Some(140 * ONE_UNIT)));

    let validation = intent.validate_model();
    assert!(validation.valid, "{validation:?}");
    assert_eq!(
        validation.routability,
        IntentV2Routability::DisabledAssetClass
    );
    assert_eq!(
        validation.reason_code,
        "bracket_oco_model_only_not_executable"
    );
}

#[test]
fn equity_intent_with_only_take_profit_leg_is_structurally_valid_but_non_routable() {
    let intent = equity_intent().with_bracket_legs(BracketLegs::new(Some(150 * ONE_UNIT), None));

    let validation = intent.validate_model();
    assert!(validation.valid, "{validation:?}");
    assert_eq!(
        validation.routability,
        IntentV2Routability::DisabledAssetClass
    );
    assert_eq!(
        validation.reason_code,
        "bracket_oco_model_only_not_executable"
    );
}

#[test]
fn equity_intent_with_only_stop_loss_leg_is_structurally_valid_but_non_routable() {
    let intent = equity_intent().with_bracket_legs(BracketLegs::new(None, Some(140 * ONE_UNIT)));

    let validation = intent.validate_model();
    assert!(validation.valid, "{validation:?}");
    assert_eq!(
        validation.routability,
        IntentV2Routability::DisabledAssetClass
    );
    assert_eq!(
        validation.reason_code,
        "bracket_oco_model_only_not_executable"
    );
}

#[test]
fn bracket_with_neither_leg_set_fails_validation() {
    let intent = equity_intent().with_bracket_legs(BracketLegs::new(None, None));

    let validation = intent.validate_model();
    assert!(!validation.valid, "{validation:?}");
    assert_eq!(validation.routability, IntentV2Routability::Invalid);
    assert_eq!(validation.reason_code, "bracket_requires_at_least_one_leg");
}

#[test]
fn bracket_with_non_positive_take_profit_price_fails_validation() {
    let intent = equity_intent().with_bracket_legs(BracketLegs::new(Some(0), Some(140 * ONE_UNIT)));

    let validation = intent.validate_model();
    assert!(!validation.valid, "{validation:?}");
    assert_eq!(validation.routability, IntentV2Routability::Invalid);
    assert_eq!(validation.reason_code, "invalid_bracket_take_profit_price");
}

#[test]
fn bracket_with_negative_stop_loss_price_fails_validation() {
    let intent =
        equity_intent().with_bracket_legs(BracketLegs::new(Some(150 * ONE_UNIT), Some(-1)));

    let validation = intent.validate_model();
    assert!(!validation.valid, "{validation:?}");
    assert_eq!(validation.routability, IntentV2Routability::Invalid);
    assert_eq!(validation.reason_code, "invalid_bracket_stop_loss_price");
}

#[test]
fn crypto_intent_with_bracket_legs_reports_bracket_reason_not_asset_class_reason() {
    // Bracket/OCO is non-executable for every asset class today, so the
    // bracket-specific reason code takes precedence over the generic
    // asset-class-disabled reason when both would independently apply.
    let intent = crypto_intent().with_bracket_legs(BracketLegs::new(
        Some(70_000 * ONE_UNIT),
        Some(60_000 * ONE_UNIT),
    ));

    let validation = intent.validate_model();
    assert!(validation.valid, "{validation:?}");
    assert_eq!(
        validation.routability,
        IntentV2Routability::DisabledAssetClass
    );
    assert_eq!(
        validation.reason_code,
        "bracket_oco_model_only_not_executable"
    );
}

#[test]
fn invalid_bracket_is_checked_before_qty_is_no_longer_relevant_but_qty_still_validates_first() {
    // Invalid qty must still fail closed even when bracket legs are present
    // and would otherwise be valid — common-field validation runs first.
    let mut intent =
        equity_intent().with_bracket_legs(BracketLegs::new(Some(150 * ONE_UNIT), None));
    intent.qty = QtyMicros::ZERO;

    let validation = intent.validate_model();
    assert!(!validation.valid, "{validation:?}");
    assert_eq!(validation.routability, IntentV2Routability::Invalid);
    assert_eq!(validation.reason_code, "invalid_qty");
}

#[test]
fn bracket_legs_builder_round_trips_fields() {
    let bracket = BracketLegs::new(Some(150 * ONE_UNIT), Some(140 * ONE_UNIT));
    assert_eq!(bracket.take_profit_limit_price_micros, Some(150 * ONE_UNIT));
    assert_eq!(bracket.stop_loss_stop_price_micros, Some(140 * ONE_UNIT));
    assert!(bracket.oco);

    let intent = equity_intent().with_bracket_legs(bracket.clone());
    assert_eq!(intent.bracket, Some(bracket));
}
