//! Scenario: Cancel/Replace Provenance Check — EB-2
//!
//! # Invariant under test
//!
//! `BrokerGateway::cancel` and `BrokerGateway::replace` require the internal
//! order ID to be present in the caller-supplied `BrokerOrderMap`. If the
//! mapping is absent — because the order was never submitted by this system,
//! or has already been deregistered — the gateway returns [`UnknownOrder`]
//! before routing to the broker.
//!
//! Gate evaluation happens BEFORE the map lookup. A gate failure is therefore
//! distinguishable from a provenance failure by error type.

use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerGateway, BrokerInvokeToken, BrokerOrderMap,
    BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
    GateRefusal, IntegrityGate, ReconcileGate, RiskGate, UnknownOrder,
};

// ---------------------------------------------------------------------------
// Stubs
// ---------------------------------------------------------------------------

struct AlwaysOkBroker;

impl BrokerAdapter for AlwaysOkBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerSubmitResponse, Box<dyn std::error::Error>> {
        Ok(BrokerSubmitResponse {
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

struct BoolGate(bool);
impl IntegrityGate for BoolGate {
    fn is_armed(&self) -> bool {
        self.0
    }
}
impl RiskGate for BoolGate {
    fn is_allowed(&self) -> bool {
        self.0
    }
}
impl ReconcileGate for BoolGate {
    fn is_clean(&self) -> bool {
        self.0
    }
}

type TestGateway = BrokerGateway<AlwaysOkBroker, BoolGate, BoolGate, BoolGate>;

fn all_clear() -> TestGateway {
    BrokerGateway::new(
        AlwaysOkBroker,
        BoolGate(true),
        BoolGate(true),
        BoolGate(true),
    )
}

fn integrity_down() -> TestGateway {
    BrokerGateway::new(
        AlwaysOkBroker,
        BoolGate(false),
        BoolGate(true),
        BoolGate(true),
    )
}

// ---------------------------------------------------------------------------
// cancel — provenance
// ---------------------------------------------------------------------------

#[test]
fn cancel_registered_order_succeeds() {
    let mut map = BrokerOrderMap::new();
    map.register("ord-1", "b-ord-1");
    assert!(all_clear().cancel("ord-1", &map).is_ok());
}

#[test]
fn cancel_unknown_order_refused() {
    // Order was never submitted — map is empty.
    let map = BrokerOrderMap::new();
    let err = all_clear().cancel("unknown-ord", &map).unwrap_err();
    let refused = err.downcast::<UnknownOrder>().expect("UnknownOrder");
    assert_eq!(refused.internal_id, "unknown-ord");
    assert!(refused.to_string().contains("CANCEL_REPLACE_REFUSED"));
}

#[test]
fn cancel_deregistered_order_refused() {
    // Simulate an order that was submitted, filled, then deregistered.
    let mut map = BrokerOrderMap::new();
    map.register("ord-1", "b-ord-1");
    map.deregister("ord-1");
    let err = all_clear().cancel("ord-1", &map).unwrap_err();
    err.downcast::<UnknownOrder>()
        .expect("UnknownOrder — deregistered order must be refused");
}

// ---------------------------------------------------------------------------
// replace — provenance
// ---------------------------------------------------------------------------

#[test]
fn replace_registered_order_succeeds() {
    let mut map = BrokerOrderMap::new();
    map.register("ord-1", "b-ord-1");
    assert!(all_clear()
        .replace("ord-1", &map, 20, None, "day".to_string())
        .is_ok());
}

#[test]
fn replace_unknown_order_refused() {
    let map = BrokerOrderMap::new();
    let err = all_clear()
        .replace("unknown-ord", &map, 20, None, "day".to_string())
        .unwrap_err();
    let refused = err.downcast::<UnknownOrder>().expect("UnknownOrder");
    assert_eq!(refused.internal_id, "unknown-ord");
}

#[test]
fn replace_deregistered_order_refused() {
    let mut map = BrokerOrderMap::new();
    map.register("ord-2", "b-ord-2");
    map.deregister("ord-2");
    let err = all_clear()
        .replace("ord-2", &map, 10, Some(100_000_000), "gtc".to_string())
        .unwrap_err();
    err.downcast::<UnknownOrder>()
        .expect("UnknownOrder — deregistered order must be refused");
}

// ---------------------------------------------------------------------------
// Gate evaluated BEFORE map lookup
// ---------------------------------------------------------------------------

#[test]
fn gate_failure_before_map_lookup_on_cancel() {
    // Empty map — if map lookup ran first it would also fail (UnknownOrder).
    // Gate failure (GateRefusal) must win.
    let map = BrokerOrderMap::new();
    let err = integrity_down().cancel("ord-1", &map).unwrap_err();
    let refusal = err.downcast::<GateRefusal>().expect("GateRefusal");
    assert_eq!(*refusal, GateRefusal::IntegrityDisarmed);
}

#[test]
fn gate_failure_before_map_lookup_on_replace() {
    let map = BrokerOrderMap::new();
    let err = integrity_down()
        .replace("ord-1", &map, 20, None, "day".to_string())
        .unwrap_err();
    let refusal = err.downcast::<GateRefusal>().expect("GateRefusal");
    assert_eq!(*refusal, GateRefusal::IntegrityDisarmed);
}
