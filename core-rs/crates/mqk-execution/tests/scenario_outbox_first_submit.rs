//! Scenario: Outbox-first Submit Enforcement — EB-3
//!
//! # Invariant under test
//!
//! `BrokerGateway::submit` uses `claim.idempotency_key` as the broker-side
//! `order_id`, overriding any value in `req.order_id`. Callers cannot inject
//! a free-form order ID — the broker always sees the key from the outbox claim.
//!
//! This prevents a dispatcher from submitting with an ID that was never
//! recorded in the outbox (which would break idempotency and audit tracing).

use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerGateway, BrokerInvokeToken, BrokerOrderMap,
    BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
    IntegrityGate, OutboxClaimToken, ReconcileGate, RiskGate,
};

// ---------------------------------------------------------------------------
// Capturing broker stub — records the order_id it actually received
// ---------------------------------------------------------------------------

/// A broker stub whose `broker_order_id` in the response encodes the
/// `order_id` it received: `"b-{order_id}"`. This lets tests assert which
/// key was actually sent to the broker.
struct EchoBroker;

impl BrokerAdapter for EchoBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerSubmitResponse, Box<dyn std::error::Error>> {
        Ok(BrokerSubmitResponse {
            // Echo back the order_id so tests can verify which key reached the broker.
            broker_order_id: format!("b-{}", req.order_id),
            submitted_at: 1,
            status: "ok".to_string(),
        })
    }

    fn cancel_order(
        &self,
        order_id: &str,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerCancelResponse, Box<dyn std::error::Error>> {
        Ok(BrokerCancelResponse {
            broker_order_id: order_id.to_string(),
            cancelled_at: 1,
            status: "ok".to_string(),
        })
    }

    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerReplaceResponse, Box<dyn std::error::Error>> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 1,
            status: "ok".to_string(),
        })
    }
}

struct Open;
impl IntegrityGate for Open {
    fn is_armed(&self) -> bool {
        true
    }
}
impl RiskGate for Open {
    fn is_allowed(&self) -> bool {
        true
    }
}
impl ReconcileGate for Open {
    fn is_clean(&self) -> bool {
        true
    }
}

fn all_clear() -> BrokerGateway<EchoBroker, Open, Open, Open> {
    BrokerGateway::new(EchoBroker, Open, Open, Open)
}

fn registered_map(internal_id: &str, broker_id: &str) -> BrokerOrderMap {
    let mut m = BrokerOrderMap::new();
    m.register(internal_id, broker_id);
    m
}

// ---------------------------------------------------------------------------
// EB-3 core invariant
// ---------------------------------------------------------------------------

#[test]
fn submit_uses_claim_idempotency_key_not_req_order_id() {
    // The claim carries "outbox-key" (from the outbox row's idempotency_key).
    // The request carries a DIFFERENT order_id ("caller-key").
    // The broker must see "outbox-key" — not "caller-key".
    let claim = OutboxClaimToken::from_claimed_row(42, "outbox-key");
    let req = BrokerSubmitRequest {
        order_id: "caller-key".to_string(), // must be overridden
        symbol: "AAPL".to_string(),
        quantity: 10,
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
    };

    let resp = all_clear().submit(&claim, req).unwrap();

    // EchoBroker returns "b-{order_id_it_received}", so this assertion
    // proves the outbox key (not the caller's key) reached the broker.
    assert_eq!(
        resp.broker_order_id, "b-outbox-key",
        "broker must receive claim.idempotency_key, not req.order_id"
    );
}

#[test]
fn submit_when_req_order_id_matches_claim_key_succeeds_unchanged() {
    // When the caller happens to provide the correct key, the override is a no-op.
    let claim = OutboxClaimToken::from_claimed_row(7, "order-abc");
    let req = BrokerSubmitRequest {
        order_id: "order-abc".to_string(),
        symbol: "MSFT".to_string(),
        quantity: 5,
        order_type: "limit".to_string(),
        limit_price: Some(300_000_000),
        time_in_force: "gtc".to_string(),
    };

    let resp = all_clear().submit(&claim, req).unwrap();
    assert_eq!(resp.broker_order_id, "b-order-abc");
}

#[test]
fn submit_other_fields_from_req_are_preserved() {
    // Overriding order_id must not corrupt other request fields.
    let claim = OutboxClaimToken::from_claimed_row(1, "key-1");
    let req = BrokerSubmitRequest {
        order_id: "wrong".to_string(),
        symbol: "TSLA".to_string(),
        quantity: 25,
        order_type: "limit".to_string(),
        limit_price: Some(200_000_000), // $200 in micros
        time_in_force: "ioc".to_string(),
    };

    // Just verify submit succeeds — the EchoBroker doesn't validate fields,
    // but the call exercises the struct-update path without panicking.
    let resp = all_clear().submit(&claim, req).unwrap();
    assert_eq!(resp.broker_order_id, "b-key-1");
    assert_eq!(resp.status, "ok");
}

// ---------------------------------------------------------------------------
// EB-3 does not affect cancel / replace (those use BrokerOrderMap, not the claim)
// ---------------------------------------------------------------------------

#[test]
fn cancel_still_uses_broker_order_map_after_eb3() {
    let map = registered_map("ord-1", "b-ord-1");
    assert!(all_clear().cancel("ord-1", &map).is_ok());
}

#[test]
fn replace_still_uses_broker_order_map_after_eb3() {
    let map = registered_map("ord-1", "b-ord-1");
    assert!(
        all_clear()
            .replace("ord-1", &map, 20, None, "day".to_string())
            .is_ok()
    );
}
