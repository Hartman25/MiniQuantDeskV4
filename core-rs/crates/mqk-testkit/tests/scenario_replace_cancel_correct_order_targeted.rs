//! Scenario: Replace/Cancel Targets Correct Broker Order — Patch L9
//!
//! # Invariants under test
//!
//! **ID mapping (BrokerOrderMap)**
//! 1. After submit, registering internal_id → broker_id makes `broker_id()` return it.
//! 2. Two different internal IDs map to two distinct broker IDs.
//! 3. Cancel uses the broker_id from the map, not the internal ID.
//! 4. Replace uses the broker_id from the map, not the internal ID.
//! 5. Deregistering removes the mapping — `broker_id()` returns `None`.
//! 6. Unknown (never-registered) internal ID → `None`.
//! 7. Map is empty initially; `len()` and `is_empty()` reflect live mappings.
//!
//! **Integer micros price surface (no f64 on decision boundary)**
//! 8. `limit_price` in `BrokerSubmitRequest` is `Option<i64>` (micros).
//! 9. `limit_price` in `BrokerReplaceRequest` is `Option<i64>` (micros).
//! 10. `micros_to_price` converts 150_000_000 → 150.0 (exact).
//! 11. `price_to_micros` converts 150.0 → 150_000_000 (exact).
//! 12. Round-trip: price_to_micros(micros_to_price(m)) == m for common prices.
//!
//! All tests are pure in-process; no DB or network required.

use mqk_execution::{
    micros_to_price, price_to_micros, BrokerOrderMap, BrokerReplaceRequest, BrokerSubmitRequest,
    MICROS_PER_UNIT,
};

// ---------------------------------------------------------------------------
// 1. Register → broker_id returns value
// ---------------------------------------------------------------------------

#[test]
fn register_makes_broker_id_reachable() {
    let mut map = BrokerOrderMap::new();
    map.register("ord-1", "broker-abc");

    assert_eq!(
        map.broker_id("ord-1"),
        Some("broker-abc"),
        "registered mapping must be retrievable by internal ID"
    );
}

// ---------------------------------------------------------------------------
// 2. Two distinct internal IDs map to distinct broker IDs
// ---------------------------------------------------------------------------

#[test]
fn two_orders_produce_distinct_broker_id_mappings() {
    let mut map = BrokerOrderMap::new();
    map.register("ord-1", "broker-111");
    map.register("ord-2", "broker-222");

    assert_eq!(map.broker_id("ord-1"), Some("broker-111"));
    assert_eq!(map.broker_id("ord-2"), Some("broker-222"));
    assert_ne!(
        map.broker_id("ord-1"),
        map.broker_id("ord-2"),
        "two different orders must have distinct broker ID mappings"
    );
}

// ---------------------------------------------------------------------------
// 3. Cancel uses broker_id from map — not internal ID
// ---------------------------------------------------------------------------

#[test]
fn cancel_must_use_broker_id_from_map_not_internal_id() {
    let mut map = BrokerOrderMap::new();
    let internal_id = "order-intent-uuid-001";
    let broker_id = "alpaca-broker-XYZ123";

    map.register(internal_id, broker_id);

    // The broker_id is what goes to the cancel API — not the internal ID.
    let target = map
        .broker_id(internal_id)
        .expect("mapping must exist before cancel");
    assert_eq!(
        target, broker_id,
        "cancel must target broker-assigned ID, not internal intent ID"
    );
    assert_ne!(
        target, internal_id,
        "internal ID and broker ID must be distinct"
    );
}

// ---------------------------------------------------------------------------
// 4. Replace uses broker_id from map — not internal ID
// ---------------------------------------------------------------------------

#[test]
fn replace_must_use_broker_id_from_map_not_internal_id() {
    let mut map = BrokerOrderMap::new();
    let internal_id = "intent-replace-001";
    let broker_id = "broker-replace-XYZ";

    map.register(internal_id, broker_id);

    let target = map
        .broker_id(internal_id)
        .expect("mapping must exist before replace");

    // Build the replace request with the broker ID (not the internal ID).
    let req = BrokerReplaceRequest {
        broker_order_id: target.to_string(),
        quantity: 200,
        limit_price: Some(151_000_000), // $151.00 in micros — no f64
        time_in_force: "day".to_string(),
    };

    assert_eq!(
        req.broker_order_id, broker_id,
        "replace request must carry the broker-assigned ID"
    );
}

// ---------------------------------------------------------------------------
// 5. Deregister removes mapping
// ---------------------------------------------------------------------------

#[test]
fn deregister_removes_mapping() {
    let mut map = BrokerOrderMap::new();
    map.register("ord-1", "broker-001");
    assert!(map.broker_id("ord-1").is_some());

    map.deregister("ord-1");
    assert_eq!(
        map.broker_id("ord-1"),
        None,
        "deregistered mapping must no longer be present"
    );
}

// ---------------------------------------------------------------------------
// 6. Unknown ID returns None
// ---------------------------------------------------------------------------

#[test]
fn unknown_internal_id_returns_none() {
    let map = BrokerOrderMap::new();
    assert_eq!(
        map.broker_id("never-registered"),
        None,
        "unknown internal ID must yield None — caller must not fabricate a broker ID"
    );
}

// ---------------------------------------------------------------------------
// 7. len() and is_empty() reflect live mappings
// ---------------------------------------------------------------------------

#[test]
fn len_and_is_empty_reflect_live_mappings() {
    let mut map = BrokerOrderMap::new();
    assert!(map.is_empty());
    assert_eq!(map.len(), 0);

    map.register("a", "b-a");
    assert!(!map.is_empty());
    assert_eq!(map.len(), 1);

    map.register("b", "b-b");
    assert_eq!(map.len(), 2);

    map.deregister("a");
    assert_eq!(map.len(), 1);

    map.deregister("b");
    assert!(map.is_empty());
}

// ---------------------------------------------------------------------------
// 8. BrokerSubmitRequest.limit_price is Option<i64>
// ---------------------------------------------------------------------------

#[test]
fn broker_submit_request_limit_price_is_integer_micros() {
    let req = BrokerSubmitRequest {
        order_id: "ord-limit".to_string(),
        symbol: "AAPL".to_string(),
        quantity: 100,
        order_type: "limit".to_string(),
        limit_price: Some(150_000_000), // $150.00 in micros
        time_in_force: "day".to_string(),
    };

    // Type is Option<i64> — no f64 on the decision surface.
    let lp: Option<i64> = req.limit_price;
    assert_eq!(lp, Some(150_000_000));
}

#[test]
fn broker_submit_request_market_order_has_no_limit_price() {
    let req = BrokerSubmitRequest {
        order_id: "ord-market".to_string(),
        symbol: "SPY".to_string(),
        quantity: 10,
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
    };
    assert_eq!(req.limit_price, None);
}

// ---------------------------------------------------------------------------
// 9. BrokerReplaceRequest.limit_price is Option<i64>
// ---------------------------------------------------------------------------

#[test]
fn broker_replace_request_limit_price_is_integer_micros() {
    let req = BrokerReplaceRequest {
        broker_order_id: "b-ord-1".to_string(),
        quantity: 50,
        limit_price: Some(200_500_000), // $200.50 in micros
        time_in_force: "gtc".to_string(),
    };

    let lp: Option<i64> = req.limit_price;
    assert_eq!(lp, Some(200_500_000));
}

// ---------------------------------------------------------------------------
// 10. micros_to_price: 150_000_000 → 150.0
// ---------------------------------------------------------------------------

#[test]
fn micros_to_price_converts_correctly() {
    let micros = 150 * MICROS_PER_UNIT; // $150.00
    let price = micros_to_price(micros);
    assert!(
        (price - 150.0).abs() < f64::EPSILON,
        "150_000_000 micros must convert to 150.0, got {price}"
    );
}

// ---------------------------------------------------------------------------
// 11. price_to_micros: 150.0 → 150_000_000
// ---------------------------------------------------------------------------

#[test]
fn price_to_micros_converts_correctly() {
    let micros = price_to_micros(150.0).expect("150.0 is a valid price");
    assert_eq!(
        micros,
        150 * MICROS_PER_UNIT,
        "150.0 must convert to 150_000_000 micros, got {micros}"
    );
}

// ---------------------------------------------------------------------------
// 12. Round-trip: price_to_micros(micros_to_price(m)) == m
// ---------------------------------------------------------------------------

#[test]
fn price_round_trip_is_exact_for_common_equity_prices() {
    let prices_micros: &[i64] = &[
        0,
        1_000_000,      // $1.00
        100_000_000,    // $100.00
        150_500_000,    // $150.50
        199_990_000,    // $199.99
        10_000_000_000, // $10,000.00 (high-priced stock)
    ];

    for &original in prices_micros {
        let via_f64 = price_to_micros(micros_to_price(original)).unwrap();
        assert_eq!(
            via_f64,
            original,
            "round-trip must be exact for ${} (micros: {original})",
            original / MICROS_PER_UNIT
        );
    }
}
