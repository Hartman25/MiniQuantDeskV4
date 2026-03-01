//! Scenario: Non-bypassable Broker Submit Gate — EB-1
//!
//! Proves three invariants from the perspective of external code
//! (integration test: compiled as a separate binary, not part of the crate).
//!
//! # Invariant 1 — compile-time: OrderRouter is crate-private
//!
//! `OrderRouter` is declared `pub(crate)` and is NOT re-exported from
//! `mqk_execution::lib`. External code cannot name, construct, or call it.
//! There is no runtime test for a compile-time error; the invariant is
//! documented here and enforced by the type system. Attempting to write:
//!
//! ```text
//! use mqk_execution::order_router::OrderRouter;   // ERROR: module not public
//! ```
//!
//! produces a compile error.
//!
//! # Invariant 2 — compile-time: BrokerInvokeToken cannot be constructed externally
//!
//! `BrokerInvokeToken` is re-exported so external crates can name it in
//! `BrokerAdapter` implementations. Its inner field is `pub(crate)`, so
//! struct-literal construction is forbidden outside `mqk-execution`:
//!
//! ```text
//! BrokerInvokeToken(())   // ERROR: tuple struct field is private
//! ```
//!
//! The only valid `BrokerInvokeToken` is manufactured inside `BrokerGateway`,
//! making it the single compile-time choke-point for broker operations.
//!
//! # Invariant 3 — runtime (covered below): gate evaluation is non-bypassable
//!
//! Every `submit`, `cancel`, and `replace` call evaluates all three gates
//! in order (integrity → risk → reconcile) and returns `GateRefusal` if any
//! fails. Gate state is stored in the gateway; callers cannot inject a verdict.

use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerGateway, BrokerInvokeToken, BrokerOrderMap,
    BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
    GateRefusal, IntegrityGate, OutboxClaimToken, ReconcileGate, RiskGate,
};

// ---------------------------------------------------------------------------
// Stubs (written from external-crate perspective)
// ---------------------------------------------------------------------------

struct AlwaysOkBroker;

impl BrokerAdapter for AlwaysOkBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
        // Note: `_token` is received here but CANNOT be constructed by this
        // external code. The token only arrives because BrokerGateway created it.
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

/// Boolean gate stub. Implements all three gate traits.
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

type TestGateway = BrokerGateway<AlwaysOkBroker, BoolGate, BoolGate, BoolGate>;

fn make_gateway(integrity: bool, risk: bool, reconcile: bool) -> TestGateway {
    BrokerGateway::new(
        AlwaysOkBroker,
        BoolGate(integrity),
        BoolGate(risk),
        BoolGate(reconcile),
    )
}

fn submit_req() -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: "ord-1".to_string(),
        symbol: "AAPL".to_string(),
        quantity: 10,
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
    }
}

/// A map with "ord-1" registered — needed for gate-pass replace/cancel tests.
fn registered_map() -> BrokerOrderMap {
    let mut m = BrokerOrderMap::new();
    m.register("ord-1", "b-ord-1");
    m
}

/// An empty map — sufficient for gate-blocking tests (gate fires before map lookup).
fn empty_map() -> BrokerOrderMap {
    BrokerOrderMap::new()
}

fn claim() -> OutboxClaimToken {
    // External code MUST use from_claimed_row. Struct literal fails to compile:
    //   OutboxClaimToken { _priv: (), outbox_id: 1, idempotency_key: "ord-1".into() }
    //   ^ error[E0451]: field `_priv` of struct `OutboxClaimToken` is private
    OutboxClaimToken::from_claimed_row(1, "ord-1")
}

// ---------------------------------------------------------------------------
// submit — all three gates
// ---------------------------------------------------------------------------

#[test]
fn all_gates_pass_submit_succeeds() {
    let res = make_gateway(true, true, true).submit(&claim(), submit_req());
    assert!(res.is_ok(), "all gates pass: submit must succeed");
}

#[test]
fn integrity_gate_blocks_submit() {
    let err = make_gateway(false, true, true)
        .submit(&claim(), submit_req())
        .unwrap_err();
    let refusal = err.downcast::<GateRefusal>().expect("GateRefusal");
    assert_eq!(*refusal, GateRefusal::IntegrityDisarmed);
}

#[test]
fn risk_gate_blocks_submit() {
    let err = make_gateway(true, false, true)
        .submit(&claim(), submit_req())
        .unwrap_err();
    let refusal = err.downcast::<GateRefusal>().expect("GateRefusal");
    assert_eq!(*refusal, GateRefusal::RiskBlocked);
}

#[test]
fn reconcile_gate_blocks_submit() {
    let err = make_gateway(true, true, false)
        .submit(&claim(), submit_req())
        .unwrap_err();
    let refusal = err.downcast::<GateRefusal>().expect("GateRefusal");
    assert_eq!(*refusal, GateRefusal::ReconcileNotClean);
}

#[test]
fn integrity_evaluated_before_risk_and_reconcile_on_submit() {
    // When all gates are false, integrity must be reported first.
    let err = make_gateway(false, false, false)
        .submit(&claim(), submit_req())
        .unwrap_err();
    let refusal = err.downcast::<GateRefusal>().expect("GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::IntegrityDisarmed,
        "integrity must be the first gate evaluated"
    );
}

// ---------------------------------------------------------------------------
// cancel — gate enforcement
// ---------------------------------------------------------------------------

#[test]
fn all_gates_pass_cancel_succeeds() {
    let res = make_gateway(true, true, true).cancel("ord-1", &registered_map());
    assert!(res.is_ok(), "all gates pass: cancel must succeed");
}

#[test]
fn integrity_gate_blocks_cancel() {
    // Gate fires before map lookup; empty map is fine here.
    let err = make_gateway(false, true, true)
        .cancel("ord-1", &empty_map())
        .unwrap_err();
    let refusal = err.downcast::<GateRefusal>().expect("GateRefusal");
    assert_eq!(*refusal, GateRefusal::IntegrityDisarmed);
}

#[test]
fn risk_gate_blocks_cancel() {
    let err = make_gateway(true, false, true)
        .cancel("ord-1", &empty_map())
        .unwrap_err();
    let refusal = err.downcast::<GateRefusal>().expect("GateRefusal");
    assert_eq!(*refusal, GateRefusal::RiskBlocked);
}

#[test]
fn reconcile_gate_blocks_cancel() {
    let err = make_gateway(true, true, false)
        .cancel("ord-1", &empty_map())
        .unwrap_err();
    let refusal = err.downcast::<GateRefusal>().expect("GateRefusal");
    assert_eq!(*refusal, GateRefusal::ReconcileNotClean);
}

// ---------------------------------------------------------------------------
// replace — gate enforcement
// ---------------------------------------------------------------------------

#[test]
fn all_gates_pass_replace_succeeds() {
    let res = make_gateway(true, true, true).replace(
        "ord-1",
        &registered_map(),
        20,
        None,
        "day".to_string(),
    );
    assert!(res.is_ok(), "all gates pass: replace must succeed");
}

#[test]
fn integrity_gate_blocks_replace() {
    let err = make_gateway(false, true, true)
        .replace("ord-1", &empty_map(), 20, None, "day".to_string())
        .unwrap_err();
    let refusal = err.downcast::<GateRefusal>().expect("GateRefusal");
    assert_eq!(*refusal, GateRefusal::IntegrityDisarmed);
}

#[test]
fn risk_gate_blocks_replace() {
    let err = make_gateway(true, false, true)
        .replace("ord-1", &empty_map(), 20, None, "day".to_string())
        .unwrap_err();
    let refusal = err.downcast::<GateRefusal>().expect("GateRefusal");
    assert_eq!(*refusal, GateRefusal::RiskBlocked);
}

#[test]
fn reconcile_gate_blocks_replace() {
    let err = make_gateway(true, true, false)
        .replace("ord-1", &empty_map(), 20, None, "day".to_string())
        .unwrap_err();
    let refusal = err.downcast::<GateRefusal>().expect("GateRefusal");
    assert_eq!(*refusal, GateRefusal::ReconcileNotClean);
}
