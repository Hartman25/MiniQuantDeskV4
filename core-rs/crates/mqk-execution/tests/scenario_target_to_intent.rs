#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use mqk_execution::{
    targets_to_order_intents, ExecutionDecision, Side, StrategyOutput, TargetPosition,
};

#[test]
fn scenario_target_to_intent() {
    // Current broker positions (signed qty):
    // TSLA: -10 (short 10) -> target 0 => BUY 10
    // AAPL: +50 (long 50)  -> target 0 => SELL 50
    // MSFT: 0              -> target 20 => BUY 20
    let mut current_qty: BTreeMap<String, i64> = BTreeMap::new();
    current_qty.insert("TSLA".to_string(), -10);
    current_qty.insert("AAPL".to_string(), 50);
    current_qty.insert("MSFT".to_string(), 0);

    let output = StrategyOutput {
        targets: vec![
            TargetPosition {
                symbol: "TSLA".to_string(),
                qty: 0,
            },
            TargetPosition {
                symbol: "AAPL".to_string(),
                qty: 0,
            },
            TargetPosition {
                symbol: "MSFT".to_string(),
                qty: 20,
            },
        ],
    };

    let decision = targets_to_order_intents(&output.targets, &current_qty);

    let intents = match decision {
        ExecutionDecision::PlaceOrders(intents) => intents,
        ExecutionDecision::Noop => panic!("expected PlaceOrders, got Noop"),
        ExecutionDecision::HaltAndDisarm { reason } => panic!("unexpected HaltAndDisarm: {reason}"),
    };

    assert_eq!(intents.len(), 3);

    // Deterministic order is symbol-sorted by the engine's BTreeMap union: AAPL, MSFT, TSLA.
    assert_eq!(intents[0].symbol, "AAPL");
    assert_eq!(intents[0].side, Side::Sell);
    assert_eq!(intents[0].qty, 50);

    assert_eq!(intents[1].symbol, "MSFT");
    assert_eq!(intents[1].side, Side::Buy);
    assert_eq!(intents[1].qty, 20);

    assert_eq!(intents[2].symbol, "TSLA");
    assert_eq!(intents[2].side, Side::Buy);
    assert_eq!(intents[2].qty, 10);
}
