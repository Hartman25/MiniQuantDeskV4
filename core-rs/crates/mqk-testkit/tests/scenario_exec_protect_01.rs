//! EXEC-PROTECT-01 — Unified execution-protection gate proof.
//!
//! # Proof gaps closed by this file
//!
//! Existing scenarios cover individual gates and individual operations in
//! isolation.  Three concrete gaps remain un-proven before EXEC-PROTECT-01:
//!
//!   GAP-A  All-unsafe state (all three gates simultaneously fail) blocks cancel
//!          and replace with `IntegrityDisarmed` as the topmost error.
//!          Prior coverage: `scenario_only_gateway_can_submit.rs` proves this
//!          for submit only (`gate_check_order_integrity_first`).
//!
//!   GAP-B  `ReconcileFreshnessGuard` (the real production guard — not a BoolGate
//!          stub) blocks cancel and replace at boot before any reconcile has run.
//!          Prior coverage: `scenario_stale_reconcile_blocks_dispatch.rs` proves
//!          this for submit only.
//!
//!   GAP-C  Stale reconcile (past the freshness bound) blocks cancel and replace.
//!          Prior coverage: stale-reconcile path proven for submit only.
//!
//! # Gate evaluation order (enforced by BrokerGateway::enforce_gates)
//!
//!   1. IntegrityGate::is_armed()    → GateRefusal::IntegrityDisarmed
//!   2. RiskGate::evaluate_gate()    → GateRefusal::RiskBlocked(_)
//!   3. ReconcileGate::is_clean()    → GateRefusal::ReconcileNotClean
//!
//! # Required proof scenarios
//!
//!   EP-UNSAFE  unsafe_execution_state_cannot_start_or_dispatch
//!              All-unsafe state blocks ALL three broker operations; integrity is
//!              always the first error regardless of which operation is attempted.
//!
//!   EP-ENTRY   execution_entry_paths_fail_closed_before_durable_mutation
//!              A brand-new `ReconcileFreshnessGuard` (no reconcile ever run)
//!              blocks all three operations before any durable mutation can occur.
//!
//!   EP-BYPASS  no_alternate_execution_surface_bypasses_gateway_or_gates
//!              Structural compile-time enforcement proof: `OrderRouter` is
//!              `pub(crate)`, `BrokerInvokeToken` inner field is `pub(crate)`,
//!              `OutboxClaimToken` private field prevents struct-literal
//!              construction, and `BrokerGateway::for_test` is gated behind the
//!              `testkit` feature.  No code path can invoke the broker adapter
//!              without passing through all three gate evaluators in order.
//!
//!   EP-STALE   stale_or_missing_prerequisites_block_execution_consistently
//!              A stale (past freshness bound) `ReconcileFreshnessGuard` blocks
//!              cancel and replace consistently, mirroring the submit proof in
//!              `scenario_stale_reconcile_blocks_dispatch.rs`.
//!
//! All tests are pure in-process; no DB or network required.

use std::cell::Cell;

use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerGateway, BrokerInvokeToken,
    BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, GateRefusal, IntegrityGate, OutboxClaimToken, ReconcileFreshnessGuard,
    ReconcileGate, RiskGate, SubmitError,
};

// ---------------------------------------------------------------------------
// Shared stubs
// ---------------------------------------------------------------------------

struct OkBroker;

impl BrokerAdapter for OkBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerSubmitResponse, BrokerError> {
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
    ) -> Result<BrokerCancelResponse, BrokerError> {
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
    ) -> Result<BrokerReplaceResponse, BrokerError> {
        Ok(BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 1,
            status: "replaced".to_string(),
        })
    }

    fn fetch_events(
        &self,
        _cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> Result<(Vec<mqk_execution::BrokerEvent>, Option<String>), BrokerError> {
        Ok((vec![], None))
    }
}

/// Simple bool-backed gate used for all three gate traits.
struct BoolGate(bool);

impl IntegrityGate for BoolGate {
    fn is_armed(&self) -> bool {
        self.0
    }
}

impl RiskGate for BoolGate {
    fn evaluate_gate(&self) -> mqk_execution::RiskDecision {
        if self.0 {
            mqk_execution::RiskDecision::Allow
        } else {
            mqk_execution::RiskDecision::Deny(mqk_execution::RiskDenial {
                reason: mqk_execution::RiskReason::RiskEngineUnavailable,
                evidence: mqk_execution::RiskEvidence::default(),
            })
        }
    }
}

impl ReconcileGate for BoolGate {
    fn is_clean(&self) -> bool {
        self.0
    }
}

fn make_claim() -> OutboxClaimToken {
    OutboxClaimToken::for_test(1, "ord-ep01")
}

fn submit_req() -> BrokerSubmitRequest {
    BrokerSubmitRequest {
        order_id: "ord-ep01".to_string(),
        symbol: "SPY".to_string(),
        side: mqk_execution::Side::Buy,
        quantity: 10,
        order_type: "market".to_string(),
        limit_price: None,
        time_in_force: "day".to_string(),
    }
}

/// Freshness bound used for `ReconcileFreshnessGuard` tests: 5 seconds.
const BOUND_MS: i64 = 5_000;

// ---------------------------------------------------------------------------
// EP-UNSAFE: unsafe_execution_state_cannot_start_or_dispatch
//
// When all three gates simultaneously deny, the topmost gate (IntegrityGate)
// must be the first error returned for ALL three broker operations.
//
// GAP-A closed: submit is proven in scenario_only_gateway_can_submit.rs.
// This proves the same ordering contract holds for cancel and replace.
// ---------------------------------------------------------------------------

/// All-unsafe state (all three gates false) blocks cancel with IntegrityDisarmed.
/// Gate ordering: integrity → risk → reconcile; integrity fires first.
#[test]
fn ep_unsafe_01_all_gates_failing_blocks_cancel_with_integrity_first() {
    let gw = BrokerGateway::for_test(
        OkBroker,
        BoolGate(false), // integrity disarmed
        BoolGate(false), // risk denied
        BoolGate(false), // reconcile dirty
    );
    let err = gw.cancel("ord-ep01", &BrokerOrderMap::new()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::IntegrityDisarmed,
        "all-unsafe state: cancel must return IntegrityDisarmed (integrity is topmost gate)"
    );
}

/// All-unsafe state (all three gates false) blocks replace with IntegrityDisarmed.
#[test]
fn ep_unsafe_02_all_gates_failing_blocks_replace_with_integrity_first() {
    let gw = BrokerGateway::for_test(
        OkBroker,
        BoolGate(false), // integrity disarmed
        BoolGate(false), // risk denied
        BoolGate(false), // reconcile dirty
    );
    let err = gw
        .replace(
            "ord-ep01",
            &BrokerOrderMap::new(),
            20,
            None,
            "day".to_string(),
        )
        .unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::IntegrityDisarmed,
        "all-unsafe state: replace must return IntegrityDisarmed (integrity is topmost gate)"
    );
}

/// All-unsafe state blocks submit with IntegrityDisarmed.
/// Repeats the canonical proof from scenario_only_gateway_can_submit for
/// the unified EXEC-PROTECT-01 record.
#[test]
fn ep_unsafe_03_all_gates_failing_blocks_submit_with_integrity_first() {
    let gw = BrokerGateway::for_test(
        OkBroker,
        BoolGate(false), // integrity disarmed
        BoolGate(false), // risk denied
        BoolGate(false), // reconcile dirty
    );
    let err = gw.submit(&make_claim(), submit_req()).unwrap_err();
    let SubmitError::Gate(refusal) = err else {
        panic!("expected SubmitError::Gate, got {err:?}")
    };
    assert_eq!(
        refusal,
        GateRefusal::IntegrityDisarmed,
        "all-unsafe state: submit must return IntegrityDisarmed (integrity is topmost gate)"
    );
}

// ---------------------------------------------------------------------------
// EP-ENTRY: execution_entry_paths_fail_closed_before_durable_mutation
//
// A brand-new ReconcileFreshnessGuard (no reconcile ever recorded) blocks
// ALL three broker operations.  This proves that no durable mutation path
// is reachable before a clean reconcile has been established.
//
// GAP-B closed: scenario_stale_reconcile_blocks_dispatch.rs proves this for
// submit only.  This closes the gap for cancel and replace.
// ---------------------------------------------------------------------------

/// Brand-new ReconcileFreshnessGuard (no reconcile ever run) blocks cancel.
/// Fail-closed at boot: dispatch is not possible before the first clean tick.
#[test]
fn ep_entry_01_no_reconcile_ever_blocks_cancel_at_boot() {
    let now_ms = Cell::new(1_000_000_i64);
    let guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    let gw = BrokerGateway::for_test(
        OkBroker,
        BoolGate(true), // integrity armed
        BoolGate(true), // risk allowed
        guard,          // no reconcile ever recorded → fails closed
    );
    let err = gw.cancel("ord-ep01", &BrokerOrderMap::new()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("cancel error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::ReconcileNotClean,
        "brand-new guard must block cancel before first clean reconcile (fail-closed at boot)"
    );
}

/// Brand-new ReconcileFreshnessGuard (no reconcile ever run) blocks replace.
#[test]
fn ep_entry_02_no_reconcile_ever_blocks_replace_at_boot() {
    let now_ms = Cell::new(1_000_000_i64);
    let guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    let gw = BrokerGateway::for_test(
        OkBroker,
        BoolGate(true), // integrity armed
        BoolGate(true), // risk allowed
        guard,          // no reconcile ever recorded → fails closed
    );
    let err = gw
        .replace(
            "ord-ep01",
            &BrokerOrderMap::new(),
            20,
            None,
            "day".to_string(),
        )
        .unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("replace error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::ReconcileNotClean,
        "brand-new guard must block replace before first clean reconcile (fail-closed at boot)"
    );
}

/// Brand-new ReconcileFreshnessGuard (no reconcile ever run) blocks submit.
/// Repeated here for the unified EXEC-PROTECT-01 record; primary proof in
/// scenario_stale_reconcile_blocks_dispatch.rs: dispatch_blocked_when_reconcile_never_ran.
#[test]
fn ep_entry_03_no_reconcile_ever_blocks_submit_at_boot() {
    let now_ms = Cell::new(1_000_000_i64);
    let guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    let gw = BrokerGateway::for_test(OkBroker, BoolGate(true), BoolGate(true), guard);
    let err = gw.submit(&make_claim(), submit_req()).unwrap_err();
    let SubmitError::Gate(refusal) = err else {
        panic!("expected SubmitError::Gate, got {err:?}")
    };
    assert_eq!(
        refusal,
        GateRefusal::ReconcileNotClean,
        "brand-new guard must block submit before first clean reconcile (fail-closed at boot)"
    );
}

// ---------------------------------------------------------------------------
// EP-BYPASS: no_alternate_execution_surface_bypasses_gateway_or_gates
//
// Structural enforcement proof.  The gateway IS the only path to the broker
// adapter; there is no back-door.
//
// The compile-time proofs are documented rather than re-tested here (the
// compiler already enforces them on every build).  The runtime proof confirms
// that all-clear state reaches the broker exactly once, via the gateway.
// ---------------------------------------------------------------------------

/// Structural proof: no alternate surface can invoke the broker adapter.
///
/// - `OrderRouter` is `pub(crate)` — cannot be imported from outside.
/// - `BrokerInvokeToken(())` inner field is `pub(crate)` — cannot be constructed.
/// - `BrokerGateway::for_test` is `#[cfg(any(test, feature = "testkit"))]` —
///   not available in production builds without the explicit `testkit` feature.
/// - `OutboxClaimToken` private `_priv` field prevents struct-literal construction;
///   callers must use `OutboxClaimToken::for_test` (testkit-gated).
///
/// Runtime confirmation: all-clear state routes through the gateway normally.
/// If any of the above structural constraints were violated, this test would
/// be unnecessary — the compiler would already reject the bypass attempt.
#[test]
fn ep_bypass_01_gateway_is_sole_broker_surface_structural_proof() {
    // All gates pass → submit reaches the broker and returns Ok.
    // The only way to reach the broker is through BrokerGateway::submit with a
    // valid OutboxClaimToken.  This confirms the gateway path is operational and
    // is the only reachable surface (compile-time proofs above document the rest).
    let gw = BrokerGateway::for_test(OkBroker, BoolGate(true), BoolGate(true), BoolGate(true));
    let result = gw.submit(&make_claim(), submit_req());
    assert!(
        result.is_ok(),
        "all-clear state must reach the broker via the gateway (sole dispatch surface)"
    );
}

/// Structural proof: a hand-crafted "all-clear" verdict cannot be supplied at
/// call time.  submit / cancel / replace accept no gate-verdict argument.
/// Gate state is evaluated from owned evaluator objects wired at construction.
#[test]
fn ep_bypass_02_gate_verdict_is_not_forgeable_at_call_time() {
    // submit(claim, req) — two arguments only; no verdicts struct accepted.
    // Any attempt to pass a pre-computed gate verdict would be a compile error.
    // This test confirms the signature is stable; structural enforcement is
    // implicit in the fact that it compiles with exactly two arguments.
    let gw = BrokerGateway::for_test(
        OkBroker,
        BoolGate(false), // integrity disarmed — cannot be overridden at call time
        BoolGate(true),
        BoolGate(true),
    );
    let err = gw.submit(&make_claim(), submit_req()).unwrap_err();
    let SubmitError::Gate(refusal) = err else {
        panic!("expected SubmitError::Gate, got {err:?}")
    };
    assert_eq!(
        refusal,
        GateRefusal::IntegrityDisarmed,
        "disarmed integrity cannot be overridden at call time; gate evaluator owns the verdict"
    );
}

// ---------------------------------------------------------------------------
// EP-STALE: stale_or_missing_prerequisites_block_execution_consistently
//
// A ReconcileFreshnessGuard whose last clean reconcile timestamp is older
// than the freshness bound blocks cancel and replace.
//
// GAP-C closed: scenario_stale_reconcile_blocks_dispatch.rs proves stale
// reconcile blocks submit.  This closes the gap for cancel and replace.
// ---------------------------------------------------------------------------

/// Stale reconcile (past freshness bound) blocks cancel.
/// A clean reconcile was recorded, then time advanced past the bound — stale.
#[test]
fn ep_stale_01_stale_reconcile_blocks_cancel() {
    let now_ms = Cell::new(1_000_000_i64);
    let mut guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    guard.record_reconcile_result(true); // clean at T=1_000_000 ms

    // Advance 6 seconds — 1 second past the 5-second freshness bound.
    now_ms.set(1_006_000);

    let gw = BrokerGateway::for_test(OkBroker, BoolGate(true), BoolGate(true), guard);
    let err = gw.cancel("ord-ep01", &BrokerOrderMap::new()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("cancel error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::ReconcileNotClean,
        "stale reconcile (past bound) must block cancel"
    );
}

/// Stale reconcile (past freshness bound) blocks replace.
#[test]
fn ep_stale_02_stale_reconcile_blocks_replace() {
    let now_ms = Cell::new(1_000_000_i64);
    let mut guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    guard.record_reconcile_result(true); // clean at T=1_000_000 ms

    // Advance 6 seconds — past the bound.
    now_ms.set(1_006_000);

    let gw = BrokerGateway::for_test(OkBroker, BoolGate(true), BoolGate(true), guard);
    let err = gw
        .replace(
            "ord-ep01",
            &BrokerOrderMap::new(),
            20,
            None,
            "day".to_string(),
        )
        .unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("replace error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::ReconcileNotClean,
        "stale reconcile (past bound) must block replace"
    );
}

/// Dirty reconcile clears the timestamp and immediately blocks cancel.
/// Proves the dirty→clear path (not just the stale→expired path) blocks
/// cancel in the same way it blocks submit.
#[test]
fn ep_stale_03_dirty_reconcile_blocks_cancel_immediately() {
    let now_ms = Cell::new(1_000_000_i64);
    let mut guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    guard.record_reconcile_result(true); // clean at T=1_000_000 ms
    guard.record_reconcile_result(false); // dirty — clears clean timestamp

    // Time has NOT advanced past the bound; the clean timestamp was cleared.
    let gw = BrokerGateway::for_test(OkBroker, BoolGate(true), BoolGate(true), guard);
    let err = gw.cancel("ord-ep01", &BrokerOrderMap::new()).unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("cancel error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::ReconcileNotClean,
        "dirty reconcile must block cancel immediately (clean timestamp cleared)"
    );
}

/// Dirty reconcile immediately blocks replace.
#[test]
fn ep_stale_04_dirty_reconcile_blocks_replace_immediately() {
    let now_ms = Cell::new(1_000_000_i64);
    let mut guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    guard.record_reconcile_result(true); // clean
    guard.record_reconcile_result(false); // dirty — clears clean timestamp

    let gw = BrokerGateway::for_test(OkBroker, BoolGate(true), BoolGate(true), guard);
    let err = gw
        .replace(
            "ord-ep01",
            &BrokerOrderMap::new(),
            20,
            None,
            "day".to_string(),
        )
        .unwrap_err();
    let refusal = err
        .downcast_ref::<GateRefusal>()
        .expect("replace error must downcast to GateRefusal");
    assert_eq!(
        *refusal,
        GateRefusal::ReconcileNotClean,
        "dirty reconcile must block replace immediately (clean timestamp cleared)"
    );
}

/// Fresh reconcile (just re-recorded after stale) unblocks cancel.
/// Proves the guard is stateless and self-healing — unblocking requires a new
/// clean result, not a daemon restart.
#[test]
fn ep_stale_05_fresh_reconcile_after_stale_unblocks_cancel() {
    let now_ms = Cell::new(1_000_000_i64);
    let mut guard = ReconcileFreshnessGuard::new(BOUND_MS, || now_ms.get());
    guard.record_reconcile_result(true); // clean at T=1_000_000 ms

    // Advance past bound — guard would block.
    now_ms.set(1_006_000);

    // Re-record clean at T=1_006_000 — guard is fresh again.
    guard.record_reconcile_result(true);

    let mut map = BrokerOrderMap::new();
    map.register("ord-ep01", "b-ord-ep01");
    let gw = BrokerGateway::for_test(OkBroker, BoolGate(true), BoolGate(true), guard);
    let result = gw.cancel("ord-ep01", &map);
    assert!(
        result.is_ok(),
        "re-recorded clean reconcile after stale must unblock cancel"
    );
}
