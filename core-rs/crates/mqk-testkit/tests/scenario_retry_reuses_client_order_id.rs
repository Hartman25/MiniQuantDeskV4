//! Scenario: Retry Reuses client_order_id — Patch L2
//!
//! # Invariants under test (purely in-process, no DB or network required)
//!
//! 1. `intent_id_to_client_order_id` is **deterministic**: same input always
//!    produces the same output, no matter how many times it is called.
//! 2. **Different** intent IDs produce different client_order_ids.
//! 3. A retry path that derives the same `client_order_id` is treated by the
//!    broker as a no-op: `FakeBroker::submit_count` stays at 1 regardless of
//!    how many retries are attempted.
//! 4. Two independent intents are each submitted exactly once; retrying both
//!    does not increase the total submit count.

use mqk_execution::intent_id_to_client_order_id;
use mqk_testkit::FakeBroker;
use serde_json::json;

// ---------------------------------------------------------------------------
// Derivation correctness
// ---------------------------------------------------------------------------

#[test]
fn derivation_is_deterministic() {
    let id = "run-abc123_intent_buy_SPY_100";
    assert_eq!(
        intent_id_to_client_order_id(id),
        intent_id_to_client_order_id(id),
        "same intent_id must always produce the same client_order_id"
    );
}

#[test]
fn different_intent_ids_produce_different_keys() {
    let k1 = intent_id_to_client_order_id("run-abc_intent_001");
    let k2 = intent_id_to_client_order_id("run-abc_intent_002");
    assert_ne!(
        k1, k2,
        "different intent_ids must produce different client_order_ids"
    );
}

// ---------------------------------------------------------------------------
// Retry idempotency through FakeBroker
// ---------------------------------------------------------------------------

#[test]
fn retry_reuses_key_broker_submit_count_stays_one() {
    let intent_id = "run-xyz_intent_buy_AAPL_10";
    let order_json = json!({"symbol": "AAPL", "side": "BUY", "qty": 10});

    let mut broker = FakeBroker::new();

    // First attempt: derive the key and submit once.
    let key = intent_id_to_client_order_id(intent_id);
    broker.submit(&key, order_json.clone());
    assert_eq!(broker.submit_count(), 1, "first submit must register");

    // Three retries — each derives the identical key; broker ignores them.
    for _ in 0..3 {
        let retry_key = intent_id_to_client_order_id(intent_id);
        assert_eq!(retry_key, key, "retry must produce the identical key");
        broker.submit(&retry_key, order_json.clone());
    }

    assert_eq!(
        broker.submit_count(),
        1,
        "broker must remain at exactly 1 submit after all retries"
    );
}

#[test]
fn two_distinct_intents_each_submitted_once_retries_are_noop() {
    let intent_a = "run-test_intent_buy_SPY_100";
    let intent_b = "run-test_intent_buy_QQQ_50";
    let json_a = json!({"symbol": "SPY", "qty": 100});
    let json_b = json!({"symbol": "QQQ", "qty": 50});

    let mut broker = FakeBroker::new();

    let key_a = intent_id_to_client_order_id(intent_a);
    let key_b = intent_id_to_client_order_id(intent_b);

    // Submit both intents once.
    broker.submit(&key_a, json_a.clone());
    broker.submit(&key_b, json_b.clone());
    assert_eq!(
        broker.submit_count(),
        2,
        "two distinct intents must register as 2 submits"
    );

    // Retry both — each must be a no-op.
    broker.submit(&intent_id_to_client_order_id(intent_a), json_a);
    broker.submit(&intent_id_to_client_order_id(intent_b), json_b);
    assert_eq!(
        broker.submit_count(),
        2,
        "retries must not increase total submit count"
    );
}
