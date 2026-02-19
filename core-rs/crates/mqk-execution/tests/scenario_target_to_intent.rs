use mqk_execution::{
    position_book, targets_to_order_intents, Side, StrategyOutput, TargetPosition,
};

#[test]
fn scenario_strategy_targets_convert_to_order_intents_deterministically() {
    // GIVEN current portfolio state (signed quantities)
    let current = position_book([
        ("AAPL", 50),  // long 50
        ("MSFT", 0),   // flat
        ("TSLA", -10), // short 10
    ]);

    // AND strategy outputs target positions (no direct orders)
    let output = StrategyOutput::new(vec![
        TargetPosition::new("TSLA", 0),  // cover short -> BUY 10
        TargetPosition::new("AAPL", 0),  // flatten -> SELL 50
        TargetPosition::new("MSFT", 20), // open long -> BUY 20
    ]);

    // WHEN engine converts targets -> intents
    let decision = targets_to_order_intents(&current, &output);

    // THEN intents are correct and ordered deterministically by symbol
    assert_eq!(decision.intents.len(), 3);

    assert_eq!(decision.intents[0].symbol, "AAPL");
    assert_eq!(decision.intents[0].side, Side::Sell);
    assert_eq!(decision.intents[0].qty, 50);

    assert_eq!(decision.intents[1].symbol, "MSFT");
    assert_eq!(decision.intents[1].side, Side::Buy);
    assert_eq!(decision.intents[1].qty, 20);

    assert_eq!(decision.intents[2].symbol, "TSLA");
    assert_eq!(decision.intents[2].side, Side::Buy);
    assert_eq!(decision.intents[2].qty, 10);
}
