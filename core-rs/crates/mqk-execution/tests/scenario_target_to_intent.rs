#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use mqk_execution::{
    targets_to_order_intents, ExecutionDecision, QtyMicros, Side, StrategyOutput, TargetPosition,
    QTY_MICROS_SCALE,
};

fn whole(units: i64) -> QtyMicros {
    QtyMicros::from_whole_units(units).unwrap()
}

#[test]
fn scenario_target_to_intent() {
    // Current broker positions (signed qty):
    // TSLA: -10 (short 10) -> target 0 => BUY 10
    // AAPL: +50 (long 50)  -> target 0 => SELL 50
    // MSFT: 0              -> target 20 => BUY 20
    let mut current_qty: BTreeMap<String, QtyMicros> = BTreeMap::new();
    current_qty.insert("TSLA".to_string(), whole(-10));
    current_qty.insert("AAPL".to_string(), whole(50));
    current_qty.insert("MSFT".to_string(), whole(0));

    let output = StrategyOutput {
        targets: vec![
            TargetPosition::whole("TSLA".to_string(), 0),
            TargetPosition::whole("AAPL".to_string(), 0),
            TargetPosition::whole("MSFT".to_string(), 20),
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
    assert_eq!(intents[0].qty, whole(50));

    assert_eq!(intents[1].symbol, "MSFT");
    assert_eq!(intents[1].side, Side::Buy);
    assert_eq!(intents[1].qty, whole(20));

    assert_eq!(intents[2].symbol, "TSLA");
    assert_eq!(intents[2].side, Side::Buy);
    assert_eq!(intents[2].qty, whole(10));
}

/// A3-1: one whole share is exactly `QTY_MICROS_SCALE` raw micros, and the
/// whole-unit client order id is byte-identical to the historical i64 form.
#[test]
fn a3_1_one_share_target_is_scale_micros_and_whole_id_is_unchanged() {
    let t = TargetPosition::whole("SPY", 1);
    assert_eq!(t.qty, QtyMicros::new(QTY_MICROS_SCALE));
    assert_eq!(t.qty, QtyMicros::from_whole_units(1).unwrap());

    let decision = targets_to_order_intents(&[t], &BTreeMap::new());
    let ExecutionDecision::PlaceOrders(intents) = decision else {
        panic!("expected PlaceOrders");
    };
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].side, Side::Buy);
    assert_eq!(intents[0].qty.raw(), QTY_MICROS_SCALE);
    assert_eq!(intents[0].client_order_id, "tgt:SPY:Buy:1");
}

/// A3-1: whole-unit equity delta vectors (buy / sell / flat / reversal) keep
/// their exact economic result.
#[test]
fn a3_1_whole_equity_delta_vectors_unchanged() {
    let cases: [(i64, i64, Option<(Side, i64)>); 7] = [
        (0, 10, Some((Side::Buy, 10))),
        (10, 0, Some((Side::Sell, 10))),
        (10, 10, None),
        (-5, 5, Some((Side::Buy, 10))),
        (5, -5, Some((Side::Sell, 10))),
        (0, -3, Some((Side::Sell, 3))),
        (-3, 0, Some((Side::Buy, 3))),
    ];
    for (cur, tgt, want) in cases {
        let mut book = BTreeMap::new();
        book.insert("X".to_string(), whole(cur));
        let got = targets_to_order_intents(&[TargetPosition::whole("X", tgt)], &book);
        match (got, want) {
            (ExecutionDecision::Noop, None) => {}
            (ExecutionDecision::PlaceOrders(v), Some((side, qty))) => {
                assert_eq!(v.len(), 1, "cur={cur} tgt={tgt}");
                assert_eq!(v[0].side, side, "cur={cur} tgt={tgt}");
                assert_eq!(v[0].qty, whole(qty), "cur={cur} tgt={tgt}");
            }
            (g, w) => panic!("cur={cur} tgt={tgt}: got {g:?}, want {w:?}"),
        }
    }
}

/// A3-1: a fractional target/current delta is carried exactly, never rounded.
#[test]
fn a3_1_fractional_delta_is_exact() {
    let mut book = BTreeMap::new();
    book.insert("BTC/USD".to_string(), QtyMicros::new(250));
    let got = targets_to_order_intents(
        &[TargetPosition::new("BTC/USD", QtyMicros::new(100))],
        &book,
    );
    let ExecutionDecision::PlaceOrders(v) = got else {
        panic!("expected PlaceOrders");
    };
    assert_eq!(v[0].side, Side::Sell);
    assert_eq!(v[0].qty, QtyMicros::new(150));
    assert_eq!(v[0].client_order_id, "tgt:BTC/USD:Sell:0.00015");
}

/// A3-1: overflow in the checked delta / negation fails closed as
/// `HaltAndDisarm`, never wraps.
#[test]
fn a3_1_delta_overflow_and_negation_edges_fail_closed() {
    // i64::MAX - (-1) overflows subtraction.
    let mut book = BTreeMap::new();
    book.insert("X".to_string(), QtyMicros::new(-1));
    let got =
        targets_to_order_intents(&[TargetPosition::new("X", QtyMicros::new(i64::MAX))], &book);
    assert!(
        matches!(got, ExecutionDecision::HaltAndDisarm { .. }),
        "{got:?}"
    );

    // i64::MIN - 0 does not overflow subtraction but cannot be negated.
    let got = targets_to_order_intents(
        &[TargetPosition::new("X", QtyMicros::new(i64::MIN))],
        &BTreeMap::new(),
    );
    assert!(
        matches!(got, ExecutionDecision::HaltAndDisarm { .. }),
        "{got:?}"
    );

    // Underflow: i64::MIN - 1.
    let mut book = BTreeMap::new();
    book.insert("X".to_string(), QtyMicros::new(1));
    let got =
        targets_to_order_intents(&[TargetPosition::new("X", QtyMicros::new(i64::MIN))], &book);
    assert!(
        matches!(got, ExecutionDecision::HaltAndDisarm { .. }),
        "{got:?}"
    );

    // Whole-unit constructor overflow refuses.
    assert!(TargetPosition::from_whole_units("X", i64::MAX).is_none());
}
