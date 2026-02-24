//! Scenario: Stale reconcile blocks broker dispatch — Patch B3
//!
//! # Invariant under test
//!
//! `ReconcileFreshnessGuard` is the production `ReconcileGate` implementation.
//! It fails **closed** (blocks dispatch) whenever:
//!
//! - No clean reconcile has ever been recorded (fail-closed at boot).
//! - The last clean reconcile is older than `freshness_bound_ms` (stale).
//! - The most recent reconcile result was dirty (clears the timestamp).
//!
//! When wired into `BrokerGateway`, all three conditions produce
//! `GateRefusal::ReconcileNotClean`.
//!
//! Clock is injected via `std::cell::Cell<i64>` for deterministic time
//! control without spawning threads or mocking system time.
//!
//! All tests are pure in-process; no DB or network required.

use std::cell::Cell;

use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerGateway, BrokerInvokeToken, BrokerReplaceRequest,
    BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse, GateRefusal, IntegrityGate,
    OutboxClaimToken, ReconcileFreshnessGuard, RiskGate,
};

// ---------------------------------------------------------------------------
// Fixtures
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

/// Integrity and risk stubs — always pass, so reconcile state is the
/// only variable under test.
struct AlwaysArmed;
impl IntegrityGate for AlwaysArmed {
    fn is_armed(&self) -> bool {
        true
    }
}

struct AlwaysAllowed;
impl RiskGate for AlwaysAllowed {
    fn is_allowed(&self) -> bool {
        true
    }
}

/// Freshness bound used across all tests: 5 seconds.
const BOUND_MS: i64 = 5_000;

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

fn make_claim() -> OutboxClaimToken {
    OutboxClaimToken::from_claimed_row(1, "ord-test")
}

// ---------------------------------------------------------------------------
// 1. No reconcile ever run → dispatch blocked (fail-closed at boot)
// ---------------------------------------------------------------------------

#[test]
fn dispatch_blocked_when_reconcile_never_ran() {
    let now_ms = Cell::new(1_000_000_i64);
    // record_reconcile_result is never called — guard starts with None.
    let guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    let gw = BrokerGateway::new(OkBroker, AlwaysArmed, AlwaysAllowed, guard);

    let err = gw.submit(&make_claim(), submit_req()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::ReconcileNotClean);
}

// ---------------------------------------------------------------------------
// 2. Clean reconcile within bound → dispatch allowed
// ---------------------------------------------------------------------------

#[test]
fn dispatch_allowed_after_clean_reconcile_within_bound() {
    let now_ms = Cell::new(1_000_000_i64);
    let mut guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    guard.record_reconcile_result(true); // clean at T=1_000_000 ms

    // Advance 1 second — well within the 5-second freshness bound.
    now_ms.set(1_001_000);
    let gw = BrokerGateway::new(OkBroker, AlwaysArmed, AlwaysAllowed, guard);

    let result = gw.submit(&make_claim(), submit_req());
    assert!(
        result.is_ok(),
        "clean reconcile within bound must allow dispatch"
    );
}

// ---------------------------------------------------------------------------
// 3. Clean reconcile is stale → dispatch blocked
// ---------------------------------------------------------------------------

#[test]
fn dispatch_blocked_when_clean_reconcile_is_stale() {
    let now_ms = Cell::new(1_000_000_i64);
    let mut guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    guard.record_reconcile_result(true); // clean at T=1_000_000 ms

    // Advance 6 seconds — past the 5-second freshness bound.
    now_ms.set(1_006_000);
    let gw = BrokerGateway::new(OkBroker, AlwaysArmed, AlwaysAllowed, guard);

    let err = gw.submit(&make_claim(), submit_req()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::ReconcileNotClean);
}

// ---------------------------------------------------------------------------
// 4. Re-recording clean reconcile after stale → dispatch unblocked
// ---------------------------------------------------------------------------

#[test]
fn dispatch_unblocked_after_fresh_reconcile_refreshes_guard() {
    let now_ms = Cell::new(1_000_000_i64);
    let mut guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    guard.record_reconcile_result(true); // clean at T=1_000_000 ms

    // Advance past bound — guard would be stale.
    now_ms.set(1_006_000);

    // Re-record clean at T=1_006_000 — guard is fresh again (elapsed=0).
    guard.record_reconcile_result(true);
    let gw = BrokerGateway::new(OkBroker, AlwaysArmed, AlwaysAllowed, guard);

    let result = gw.submit(&make_claim(), submit_req());
    assert!(
        result.is_ok(),
        "re-recording clean reconcile after stale must unblock dispatch"
    );
}

// ---------------------------------------------------------------------------
// 5. Dirty reconcile clears timestamp → dispatch blocked immediately
// ---------------------------------------------------------------------------

#[test]
fn dispatch_blocked_immediately_after_dirty_reconcile() {
    let now_ms = Cell::new(1_000_000_i64);
    let mut guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    guard.record_reconcile_result(true); // clean at T=1_000_000 ms

    // Dirty result — clears the clean timestamp regardless of elapsed time.
    guard.record_reconcile_result(false);
    let gw = BrokerGateway::new(OkBroker, AlwaysArmed, AlwaysAllowed, guard);

    let err = gw.submit(&make_claim(), submit_req()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::ReconcileNotClean);
}

// ---------------------------------------------------------------------------
// 6. Elapsed exactly equals bound → dispatch allowed (inclusive boundary)
// ---------------------------------------------------------------------------

#[test]
fn dispatch_allowed_at_exact_freshness_bound() {
    let now_ms = Cell::new(1_000_000_i64);
    let mut guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    guard.record_reconcile_result(true); // clean at T=1_000_000 ms

    // Advance exactly BOUND_MS: elapsed == bound, still fresh (<=).
    now_ms.set(1_000_000 + BOUND_MS);
    let gw = BrokerGateway::new(OkBroker, AlwaysArmed, AlwaysAllowed, guard);

    let result = gw.submit(&make_claim(), submit_req());
    assert!(
        result.is_ok(),
        "elapsed == bound must allow dispatch (inclusive <= boundary)"
    );
}

// ---------------------------------------------------------------------------
// 7. Elapsed one ms past bound → dispatch blocked
// ---------------------------------------------------------------------------

#[test]
fn dispatch_blocked_one_ms_past_freshness_bound() {
    let now_ms = Cell::new(1_000_000_i64);
    let mut guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    guard.record_reconcile_result(true); // clean at T=1_000_000 ms

    // Advance BOUND_MS + 1: elapsed strictly exceeds bound, stale.
    now_ms.set(1_000_000 + BOUND_MS + 1);
    let gw = BrokerGateway::new(OkBroker, AlwaysArmed, AlwaysAllowed, guard);

    let err = gw.submit(&make_claim(), submit_req()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("expected GateRefusal");
    assert_eq!(*refusal, GateRefusal::ReconcileNotClean);
}
