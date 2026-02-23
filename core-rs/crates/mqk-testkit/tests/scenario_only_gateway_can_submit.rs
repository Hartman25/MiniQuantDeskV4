//! Scenario: Only BrokerGateway Can Submit — Patch L1 + PATCH A1 + PATCH A2 + PATCH A3
//!
//! # Invariant under test
//! There is exactly ONE code path that can result in broker submit/cancel/replace,
//! and it ALWAYS enforces: integrity armed + risk allowed + reconcile clean.
//! Furthermore, `submit` requires an `OutboxClaimToken` proving outbox-first dispatch.
//!
//! ## Compile-time enforcement (by design)
//! `OrderRouter` in `mqk-execution` is `pub(crate)` and is NOT re-exported
//! from the crate's public API. This file cannot import or construct
//! `OrderRouter` — any attempt would be a compile error. The only available
//! public API is `BrokerGateway`.
//!
//! ## PATCH A1 — capability token
//! `BrokerAdapter` methods require `_token: &BrokerInvokeToken`. The token
//! type is `pub` (nameable in trait impls) but its inner field is `pub(crate)`.
//! This file cannot construct `BrokerInvokeToken(())`.
//!
//! ## PATCH A2 — non-forgeable gate evaluation
//! `submit / cancel / replace` accept NO gate argument. Gate verdicts are
//! evaluated internally from owned `IntegrityGate / RiskGate / ReconcileGate`
//! evaluator objects wired at construction time. Callers cannot inject a
//! hand-crafted "all-clear" verdict struct — `GateVerdicts` is removed.
//! In tests, boolean stubs (`BoolGate`) control gate state; in production,
//! real engine implementations are wired in.
//!
//! ## PATCH A3 — outbox-first enforcement
//! `submit` requires `claim: &OutboxClaimToken` as its first argument.
//! The token's `_priv` field is `pub(crate)` — struct-literal construction is
//! a compile error from external crates. Callers must use
//! `OutboxClaimToken::from_claimed_row(outbox_id, idempotency_key)`, explicitly
//! declaring that they have a row claimed via `mqk_db::outbox_claim_batch`.
//!
//! ## Runtime enforcement (tested here)
//! Every `BrokerGateway` method evaluates all three gates.
//! A single failing gate produces `GateRefusal`; all must pass.

use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerGateway, BrokerInvokeToken, BrokerReplaceRequest,
    BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse, GateRefusal, IntegrityGate,
    OutboxClaimToken, ReconcileGate, RiskGate,
};

// ---------------------------------------------------------------------------
// Minimal mock broker
// ---------------------------------------------------------------------------

struct OkBroker;

impl BrokerAdapter for OkBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
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
        _token: &BrokerInvokeToken,
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
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerReplaceResponse, Box<dyn std::error::Error>> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 1,
            status: "replaced".to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Boolean gate stubs (PATCH A2)
// ---------------------------------------------------------------------------

/// Test stub that returns a fixed bool for every gate query.
///
/// Used to construct `BrokerGateway` instances with controlled gate state.
/// Not forgeable in the `GateVerdicts` sense — the gate IS the object;
/// callers cannot pass a pre-computed verdict at call time.
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

type TestGateway = BrokerGateway<OkBroker, BoolGate, BoolGate, BoolGate>;

fn make_gateway(integrity: bool, risk: bool, reconcile: bool) -> TestGateway {
    BrokerGateway::new(
        OkBroker,
        BoolGate(integrity),
        BoolGate(risk),
        BoolGate(reconcile),
    )
}

// ---------------------------------------------------------------------------
// Request fixtures
// ---------------------------------------------------------------------------

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

/// Stub claim token for tests (PATCH A3).
///
/// In production, `outbox_id` and `idempotency_key` come from a row returned
/// by `mqk_db::outbox_claim_batch`. Here we use fixed values for unit testing.
fn make_claim() -> OutboxClaimToken {
    OutboxClaimToken::from_claimed_row(1, "ord-test")
}

// ---------------------------------------------------------------------------
// DoD: A single gateway is the only place broker actions can be invoked.
// ---------------------------------------------------------------------------

#[test]
fn all_gates_clear_submit_succeeds() {
    let result = make_gateway(true, true, true).submit(&make_claim(), submit_req());
    assert!(result.is_ok(), "expected Ok, got {result:?}");
    assert_eq!(result.unwrap().status, "submitted");
}

#[test]
fn all_gates_clear_cancel_succeeds() {
    let result = make_gateway(true, true, true).cancel("ord-test");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, "cancelled");
}

#[test]
fn all_gates_clear_replace_succeeds() {
    let result = make_gateway(true, true, true).replace(replace_req());
    assert!(result.is_ok());
    assert_eq!(result.unwrap().status, "replaced");
}

// ---------------------------------------------------------------------------
// DoD: Gate refusal — integrity not armed
// ---------------------------------------------------------------------------

#[test]
fn integrity_disarmed_blocks_submit() {
    let err = make_gateway(false, true, true)
        .submit(&make_claim(), submit_req())
        .unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::IntegrityDisarmed);
}

#[test]
fn integrity_disarmed_blocks_cancel() {
    let err = make_gateway(false, true, true)
        .cancel("ord-test")
        .unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::IntegrityDisarmed);
}

#[test]
fn integrity_disarmed_blocks_replace() {
    let err = make_gateway(false, true, true)
        .replace(replace_req())
        .unwrap_err();
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
    let err = make_gateway(true, false, true)
        .submit(&make_claim(), submit_req())
        .unwrap_err();
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
    let err = make_gateway(true, true, false)
        .submit(&make_claim(), submit_req())
        .unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::ReconcileNotClean);
}

// ---------------------------------------------------------------------------
// DoD: Gate check order — integrity first, risk before reconcile
// ---------------------------------------------------------------------------

#[test]
fn gate_check_order_integrity_first() {
    // All three gates false: integrity must be reported first.
    let err = make_gateway(false, false, false)
        .submit(&make_claim(), submit_req())
        .unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::IntegrityDisarmed);
}

#[test]
fn gate_check_order_risk_before_reconcile() {
    // Integrity OK but risk + reconcile both failing: risk must be reported first.
    let err = make_gateway(true, false, false)
        .submit(&make_claim(), submit_req())
        .unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::RiskBlocked);
}

// ---------------------------------------------------------------------------
// PATCH A1 + A2 + A3: Compile-time bypass proofs (documented)
// ---------------------------------------------------------------------------

#[test]
fn bypass_is_impossible_compile_time_documented() {
    // OrderRouter is pub(crate) — cannot be imported or constructed externally:
    //
    //   use mqk_execution::order_router::OrderRouter; // ERROR: private module
    //
    // BrokerInvokeToken inner field is pub(crate) — cannot be constructed:
    //
    //   let _token = BrokerInvokeToken(()); // ERROR: private constructor
    //
    // GateVerdicts is removed (PATCH A2) — cannot be supplied to submit/cancel/replace:
    //
    //   gw.submit(&claim, req, &GateVerdicts::all_clear()) // ERROR: wrong arg count
    //
    // OutboxClaimToken _priv field is pub(crate) — cannot be struct-constructed:
    //
    //   OutboxClaimToken { _priv: (), outbox_id: 1, … } // ERROR: private field
    //
    // This test passes trivially; its value is that it *compiles at all*,
    // meaning only BrokerGateway is available to external callers.
    let gw: TestGateway = make_gateway(true, true, true);
    let _ = gw; // gateway is constructible and operable
}

#[test]
fn gate_verdict_cannot_be_forged_externally_documented() {
    // Before PATCH A2, callers supplied a GateVerdicts struct with explicit bools:
    //
    //   gw.submit(req, &GateVerdicts { integrity_armed: true, … }) // FORGEABLE
    //
    // After PATCH A2, submit/cancel/replace take no gate argument. Gate state
    // is evaluated from the evaluator objects wired into BrokerGateway at
    // construction time. Callers cannot inject a "clean" verdict at call time.
    //
    // After PATCH A3, submit takes (claim: &OutboxClaimToken, req). There are
    // exactly two arguments; any verdict struct would be a compile error.
    //
    // This test passes trivially; the non-forgeability is structural.
    let gw = make_gateway(true, true, true);
    let claim = OutboxClaimToken::from_claimed_row(1, "ord-test");
    let result = gw.submit(&claim, submit_req()); // claim + req, no verdict — compiles
    assert!(result.is_ok());
}

#[test]
fn broker_invoke_token_is_nameable_but_not_constructible_externally() {
    // BrokerInvokeToken is pub — can be imported and named in trait impls.
    // Its inner field is pub(crate) — cannot be constructed here:
    //
    //   let _token = BrokerInvokeToken(()); // ERROR: private constructor
    //
    // This test passes trivially; its significance is that it *compiles*
    // with BrokerInvokeToken imported but not constructed.
    fn _accepts_token_ref(_: &BrokerInvokeToken) {}
}

#[test]
fn submit_requires_outbox_claim_token_documented() {
    // BrokerGateway::submit requires &OutboxClaimToken as its first argument (PATCH A3).
    // The token cannot be constructed via struct literal from outside mqk-execution:
    //
    //   OutboxClaimToken { _priv: (), outbox_id: 1, … } // ERROR: private field
    //
    // Callers must use OutboxClaimToken::from_claimed_row(outbox_id, idempotency_key),
    // explicitly declaring that they have a claimed outbox row from
    // mqk_db::outbox_claim_batch. This makes outbox-first dispatch a named,
    // structural API requirement rather than an invisible convention.
    //
    // This test passes trivially; the requirement is enforced at every call site.
    let claim = OutboxClaimToken::from_claimed_row(42, "ord-test");
    assert_eq!(claim.outbox_id, 42);
    assert_eq!(claim.idempotency_key, "ord-test");
    let result = make_gateway(true, true, true).submit(&claim, submit_req());
    assert!(result.is_ok());
}
