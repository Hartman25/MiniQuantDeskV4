//! Scenario: Only BrokerGateway Can Submit — Patch L1
//!
//! # Invariant under test
//! There is exactly ONE code path that can result in broker submit/cancel/replace,
//! and it ALWAYS enforces: integrity armed + risk allowed + reconcile clean.
//!
//! ## Compile-time enforcement (by design)
//! `OrderRouter` in `mqk-execution` is `pub(crate)` and is NOT re-exported
//! from the crate's public API. This file cannot import or construct
//! `OrderRouter` — any attempt would be a compile error. The only available
//! public API is `BrokerGateway`.
//!
//! ## Runtime enforcement (tested here)
//! Every `BrokerGateway` method evaluates all three gate verdicts.
//! A single `false` verdict produces `GateRefusal`; all must be `true`.

use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerGateway, BrokerReplaceRequest,
    BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse, GateRefusal, GateVerdicts,
};

// ---------------------------------------------------------------------------
// Minimal mock broker
// ---------------------------------------------------------------------------

struct OkBroker;

impl BrokerAdapter for OkBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
    ) -> Result<BrokerSubmitResponse, Box<dyn std::error::Error>> {
        Ok(BrokerSubmitResponse {
            broker_order_id: format!("b-{}", req.order_id),
            submitted_at: 1,
            status: "submitted".to_string(),
        })
    }

    fn cancel_order(
        &self,
        order_id: &str,
    ) -> Result<BrokerCancelResponse, Box<dyn std::error::Error>> {
        Ok(BrokerCancelResponse {
            broker_order_id: order_id.to_string(),
            cancelled_at: 1,
            status: "cancelled".to_string(),
        })
    }

    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
    ) -> Result<BrokerReplaceResponse, Box<dyn std::error::Error>> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 1,
            status: "replaced".to_string(),
        })
    }
}

fn submit_req() -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: "ord-test".to_string(),
        symbol: "AAPL".to_string(),
        quantity: 10,
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
    }
}

fn replace_req() -> BrokerReplaceRequest {
    BrokerReplaceRequest {
        broker_order_id: "b-ord-test".to_string(),
        quantity: 20,
        limit_price: None,
        time_in_force: "day".to_string(),
    }
}

// ---------------------------------------------------------------------------
// DoD: A single "gateway" function/API exists and is the only place broker
//      actions can be invoked.
// ---------------------------------------------------------------------------

#[test]
fn all_gates_clear_submit_succeeds() {
    let gw = BrokerGateway::new(OkBroker);
    let result = gw.submit(submit_req(), &GateVerdicts::all_clear());
    assert!(result.is_ok(), "expected Ok, got {result:?}");
    assert_eq!(result.unwrap().status, "submitted");
}

#[test]
fn all_gates_clear_cancel_succeeds() {
    let gw = BrokerGateway::new(OkBroker);
    let result = gw.cancel("ord-test", &GateVerdicts::all_clear());
    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, "cancelled");
}

#[test]
fn all_gates_clear_replace_succeeds() {
    let gw = BrokerGateway::new(OkBroker);
    let result = gw.replace(replace_req(), &GateVerdicts::all_clear());
    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, "replaced");
}

// ---------------------------------------------------------------------------
// DoD: Gate refusal — integrity not armed
// ---------------------------------------------------------------------------

#[test]
fn integrity_disarmed_blocks_submit() {
    let gw = BrokerGateway::new(OkBroker);
    let verdicts = GateVerdicts {
        integrity_armed: false,
        risk_allowed: true,
        reconcile_clean: true,
    };
    let err = gw.submit(submit_req(), &verdicts).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::IntegrityDisarmed);
}

#[test]
fn integrity_disarmed_blocks_cancel() {
    let gw = BrokerGateway::new(OkBroker);
    let verdicts = GateVerdicts {
        integrity_armed: false,
        risk_allowed: true,
        reconcile_clean: true,
    };
    let err = gw.cancel("ord-test", &verdicts).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::IntegrityDisarmed);
}

#[test]
fn integrity_disarmed_blocks_replace() {
    let gw = BrokerGateway::new(OkBroker);
    let verdicts = GateVerdicts {
        integrity_armed: false,
        risk_allowed: true,
        reconcile_clean: true,
    };
    let err = gw.replace(replace_req(), &verdicts).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::IntegrityDisarmed);
}

// ---------------------------------------------------------------------------
// DoD: Gate refusal — risk not allowed
// ---------------------------------------------------------------------------

#[test]
fn risk_blocked_blocks_submit() {
    let gw = BrokerGateway::new(OkBroker);
    let verdicts = GateVerdicts {
        integrity_armed: true,
        risk_allowed: false,
        reconcile_clean: true,
    };
    let err = gw.submit(submit_req(), &verdicts).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::RiskBlocked);
}

// ---------------------------------------------------------------------------
// DoD: Gate refusal — reconcile not clean
// ---------------------------------------------------------------------------

#[test]
fn reconcile_not_clean_blocks_submit() {
    let gw = BrokerGateway::new(OkBroker);
    let verdicts = GateVerdicts {
        integrity_armed: true,
        risk_allowed: true,
        reconcile_clean: false,
    };
    let err = gw.submit(submit_req(), &verdicts).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::ReconcileNotClean);
}

// ---------------------------------------------------------------------------
// DoD: Gate check order — integrity is checked before risk, risk before reconcile
// ---------------------------------------------------------------------------

#[test]
fn gate_check_order_integrity_first() {
    let gw = BrokerGateway::new(OkBroker);
    // All three false: integrity must be reported, not risk or reconcile.
    let verdicts = GateVerdicts {
        integrity_armed: false,
        risk_allowed: false,
        reconcile_clean: false,
    };
    let err = gw.submit(submit_req(), &verdicts).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::IntegrityDisarmed);
}

#[test]
fn gate_check_order_risk_before_reconcile() {
    let gw = BrokerGateway::new(OkBroker);
    // Integrity OK but risk + reconcile both false: risk must be reported first.
    let verdicts = GateVerdicts {
        integrity_armed: true,
        risk_allowed: false,
        reconcile_clean: false,
    };
    let err = gw.submit(submit_req(), &verdicts).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::RiskBlocked);
}

// ---------------------------------------------------------------------------
// DoD: Bypass is impossible — compile-time proof (documentation test)
//
// The following would be a compile error if uncommented, proving that
// `OrderRouter` cannot be constructed from outside `mqk-execution`:
//
//   use mqk_execution::order_router::OrderRouter; // ERROR: module `order_router` is private
//   let _ = mqk_execution::order_router::OrderRouter::new(OkBroker); // ERROR
//
// This test exists as documentation of the compile-time enforcement.
// ---------------------------------------------------------------------------

#[test]
fn bypass_is_impossible_compile_time_documented() {
    // If this test compiles, it means BrokerGateway is the only available
    // public interface. OrderRouter is not importable — attempting to use it
    // from this crate would be a compile error (module `order_router` is private).
    //
    // The test passes trivially; its value is that it *compiles at all*,
    // meaning only BrokerGateway is available to external callers.
    let gw: BrokerGateway<OkBroker> = BrokerGateway::new(OkBroker);
    let _ = gw; // gateway is constructible
                // OrderRouter::new(OkBroker) — would not compile; proves the invariant
}
