//! Broker Gateway — the SINGLE choke-point for all broker operations.
//!
//! # Invariants (enforced at both compile-time and runtime)
//!
//! **Compile-time (PATCH A1):** `OrderRouter` is `pub(crate)` and is never
//! re-exported from `lib.rs`. `BrokerAdapter` methods require a
//! `&BrokerInvokeToken` that only `BrokerGateway` can construct.
//!
//! **Compile-time (PATCH A2):** Gate checks are evaluated by the stored gate
//! evaluator objects (`IG`, `RG`, `RecG`). There is no caller-supplied verdict
//! struct with forgeable booleans. `submit / cancel / replace` accept no gate
//! argument — the gateway evaluates each gate internally. Callers cannot inject
//! a "clean" verdict; they must wire in real engine state via the gate traits.
//!
//! **Compile-time (PATCH A3):** `BrokerGateway::submit` requires an
//! `&OutboxClaimToken`. The token's `_priv` field is `pub(crate)`, preventing
//! struct-literal construction outside this crate. Callers must use
//! `OutboxClaimToken::from_claimed_row`, explicitly declaring outbox provenance.
//!
//! **Runtime:** Every call to `submit / cancel / replace` invokes three gate
//! evaluators in order and refuses with `GateRefusal` if any returns `false`:
//!
//! 1. `IntegrityGate::is_armed()`  — system integrity is not disarmed or halted
//! 2. `RiskGate::is_allowed()`     — risk engine returned Allow for this request
//! 3. `ReconcileGate::is_clean()`  — most recent reconcile report is Clean
//!
//! Real engine implementations wire their subsystem state behind these traits.
//! Test doubles use simple boolean stubs.

use crate::order_router::{
    BrokerAdapter, BrokerCancelResponse, BrokerReplaceRequest, BrokerReplaceResponse,
    BrokerSubmitRequest, BrokerSubmitResponse, OrderRouter,
};

// ---------------------------------------------------------------------------
// Gate evaluator traits (PATCH A2)
// ---------------------------------------------------------------------------

/// Evaluates whether system integrity is currently armed (execution-allowed).
///
/// Implement with real `IntegrityState` or `mqk-integrity` state in production.
/// Use a bool stub in tests.
///
/// # Contract
/// Returns `true` only when execution is permitted: integrity is armed, no
/// active kill-switch, and no halt signal is in effect.
pub trait IntegrityGate {
    fn is_armed(&self) -> bool;
}

/// Evaluates whether the risk engine currently allows order submission.
///
/// Implement with real `RiskDecision` output in production.
pub trait RiskGate {
    fn is_allowed(&self) -> bool;
}

/// Evaluates whether the most recent reconcile report is clean.
///
/// Implement with real `ReconcileReport` in production.
pub trait ReconcileGate {
    fn is_clean(&self) -> bool;
}

// ---------------------------------------------------------------------------
// GateRefusal
// ---------------------------------------------------------------------------

/// The reason a broker operation was refused at the gateway.
///
/// Implements `std::error::Error` so it can be boxed and propagated through
/// `Box<dyn Error>` chains without extra wrapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateRefusal {
    IntegrityDisarmed,
    RiskBlocked,
    ReconcileNotClean,
}

impl std::fmt::Display for GateRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateRefusal::IntegrityDisarmed => {
                write!(f, "GATE_REFUSED: integrity disarmed or halted")
            }
            GateRefusal::RiskBlocked => {
                write!(f, "GATE_REFUSED: risk engine did not allow")
            }
            GateRefusal::ReconcileNotClean => {
                write!(f, "GATE_REFUSED: reconcile is not clean")
            }
        }
    }
}

impl std::error::Error for GateRefusal {}

// ---------------------------------------------------------------------------
// OutboxClaimToken (PATCH A3)
// ---------------------------------------------------------------------------

/// Proof that a broker submit originates from a claimed outbox row.
///
/// # Contract
/// Callers must obtain this token **after** successfully claiming an outbox row
/// via `mqk_db::outbox_claim_batch`, then pass the claimed row's `outbox_id`
/// and `idempotency_key` to [`OutboxClaimToken::from_claimed_row`].
///
/// The `_priv` field is `pub(crate)`, so external code **cannot** construct
/// this type via struct literal:
///
/// ```text
/// ✅  OutboxClaimToken::from_claimed_row(id, key)   // allowed: public constructor
/// ❌  OutboxClaimToken { _priv: (), outbox_id: 1, … } // ERROR: private field
/// ```
///
/// Passing fabricated values to `from_claimed_row` bypasses the protocol and
/// is a contract violation; the DB-level `FOR UPDATE SKIP LOCKED` claim is the
/// authoritative guard. The token makes outbox provenance an **explicit,
/// named API requirement** rather than an invisible convention.
///
/// # Why `#[non_exhaustive]` is not used here
/// `#[non_exhaustive]` carries the semantic "this struct may gain new fields."
/// Our intent is a **capability-token pattern** (controlled constructor), not
/// an extensibility hint. The `pub(crate) _priv` field is the correct tool;
/// the lint is suppressed intentionally.
#[allow(clippy::manual_non_exhaustive)]
#[derive(Debug, Clone)]
pub struct OutboxClaimToken {
    /// The DB row ID of the claimed outbox entry.
    pub outbox_id: i64,
    /// The idempotency key (= `client_order_id`) of the claimed outbox entry.
    pub idempotency_key: String,
    /// Prevents struct-literal construction outside this crate (PATCH A3).
    pub(crate) _priv: (),
}

impl OutboxClaimToken {
    /// Construct a claim token from a successfully claimed outbox row.
    ///
    /// `outbox_id` and `idempotency_key` must come from a row returned by
    /// `mqk_db::outbox_claim_batch`. Supplying fabricated values violates the
    /// outbox-first contract.
    pub fn from_claimed_row(outbox_id: i64, idempotency_key: impl Into<String>) -> Self {
        Self {
            outbox_id,
            idempotency_key: idempotency_key.into(),
            _priv: (),
        }
    }
}

// ---------------------------------------------------------------------------
// BrokerGateway
// ---------------------------------------------------------------------------

/// The SINGLE choke-point through which ALL broker operations must flow.
///
/// # Architecture
///
/// `BrokerGateway` owns:
/// - A **private** `OrderRouter<B>` (the `pub(crate)` broker delegation layer).
/// - Three gate evaluators: `IG` (`IntegrityGate`), `RG` (`RiskGate`),
///   `RecG` (`ReconcileGate`).
///
/// Because gate state is evaluated by owned evaluator objects, no caller can
/// supply a hand-crafted "all-clear" verdict at call time (PATCH A2).
/// In production, wire real engine state behind these traits. In tests, use
/// boolean stubs.
///
/// ```text
/// External code
///     │
///     └──► BrokerGateway::submit(claim: &OutboxClaimToken, req)  (PATCH A3)
///                │
///                ├── claim: outbox row was claimed before dispatch
///                ├── IG::is_armed()    → GateRefusal::IntegrityDisarmed
///                ├── RG::is_allowed()  → GateRefusal::RiskBlocked
///                ├── RecG::is_clean()  → GateRefusal::ReconcileNotClean
///                │
///                └── OrderRouter::route_*  ◄── only reached if all gates pass
///                         └── BrokerAdapter::*(…, &BrokerInvokeToken(()))
/// ```
pub struct BrokerGateway<B, IG, RG, RecG>
where
    B: BrokerAdapter,
    IG: IntegrityGate,
    RG: RiskGate,
    RecG: ReconcileGate,
{
    /// Private: unreachable from outside `mqk-execution`.
    router: OrderRouter<B>,
    integrity: IG,
    risk: RG,
    reconcile: RecG,
}

impl<B, IG, RG, RecG> BrokerGateway<B, IG, RG, RecG>
where
    B: BrokerAdapter,
    IG: IntegrityGate,
    RG: RiskGate,
    RecG: ReconcileGate,
{
    /// Create a gateway wrapping the given broker adapter and gate evaluators.
    ///
    /// Pass real engine objects in production; pass boolean stubs in tests.
    pub fn new(broker: B, integrity: IG, risk: RG, reconcile: RecG) -> Self {
        Self {
            router: OrderRouter::new(broker),
            integrity,
            risk,
            reconcile,
        }
    }

    /// Evaluate all three gates in order.
    /// Returns the first refusal encountered, or `Ok(())` if all pass.
    fn enforce_gates(&self) -> Result<(), GateRefusal> {
        if !self.integrity.is_armed() {
            return Err(GateRefusal::IntegrityDisarmed);
        }
        if !self.risk.is_allowed() {
            return Err(GateRefusal::RiskBlocked);
        }
        if !self.reconcile.is_clean() {
            return Err(GateRefusal::ReconcileNotClean);
        }
        Ok(())
    }

    /// Submit a new broker order.
    ///
    /// Requires an [`OutboxClaimToken`] proving the order originated from a
    /// claimed outbox row (PATCH A3). All three gates must also pass.
    /// Gate state is evaluated from the stored evaluators — no verdict can be
    /// injected by the caller.
    pub fn submit(
        &self,
        _claim: &OutboxClaimToken,
        req: BrokerSubmitRequest,
    ) -> Result<BrokerSubmitResponse, Box<dyn std::error::Error>> {
        self.enforce_gates()?;
        self.router.route_submit(req)
    }

    /// Cancel a broker order.
    ///
    /// All three gates must pass. Returns `GateRefusal` if any gate fails.
    pub fn cancel(
        &self,
        order_id: &str,
    ) -> Result<BrokerCancelResponse, Box<dyn std::error::Error>> {
        self.enforce_gates()?;
        self.router.route_cancel(order_id)
    }

    /// Replace a broker order.
    ///
    /// All three gates must pass. Returns `GateRefusal` if any gate fails.
    pub fn replace(
        &self,
        req: BrokerReplaceRequest,
    ) -> Result<BrokerReplaceResponse, Box<dyn std::error::Error>> {
        self.enforce_gates()?;
        self.router.route_replace(req)
    }
}

// ---------------------------------------------------------------------------
// Idempotency derivation
// ---------------------------------------------------------------------------

/// Derive the stable `client_order_id` for a given intent ID.
///
/// This is the **canonical** derivation point: every call-site — first submit
/// or any subsequent retry — must use this function. Because the mapping is
/// deterministic (same `intent_id` ⟹ same output), retries automatically
/// reuse the same key, preventing broker-side duplicate submission.
///
/// The `client_order_id` is the `intent_id` itself. No hash or transformation
/// is applied: intent IDs are already stable, unique, run-scoped identifiers.
pub fn intent_id_to_client_order_id(intent_id: &str) -> String {
    intent_id.to_string()
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order_router::{
        BrokerAdapter, BrokerCancelResponse, BrokerInvokeToken, BrokerReplaceRequest,
        BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
    };

    // -- Broker stub ---------------------------------------------------------

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

    // -- Gate stubs ----------------------------------------------------------

    /// Boolean gate stub for tests. Implements all three gate traits.
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

    // -- Helpers -------------------------------------------------------------

    type TestGateway = BrokerGateway<AlwaysOkBroker, BoolGate, BoolGate, BoolGate>;

    fn make_gateway(integrity: bool, risk: bool, reconcile: bool) -> TestGateway {
        BrokerGateway::new(
            AlwaysOkBroker,
            BoolGate(integrity),
            BoolGate(risk),
            BoolGate(reconcile),
        )
    }

    fn make_submit_req() -> BrokerSubmitRequest {
        BrokerSubmitRequest {
            order_id: "ord-1".to_string(),
            symbol: "AAPL".to_string(),
            quantity: 10,
            order_type: "market".to_string(),
            limit_price: None,
            time_in_force: "day".to_string(),
        }
    }

    /// Stub claim token for unit tests (PATCH A3).
    fn make_claim() -> OutboxClaimToken {
        OutboxClaimToken::from_claimed_row(1, "ord-1")
    }

    // -- Gate pass/fail tests -----------------------------------------------

    #[test]
    fn all_clear_submit_succeeds() {
        let res = make_gateway(true, true, true).submit(&make_claim(), make_submit_req());
        assert!(res.is_ok());
    }

    #[test]
    fn integrity_disarmed_blocks_submit() {
        let err = make_gateway(false, true, true)
            .submit(&make_claim(), make_submit_req())
            .unwrap_err();
        assert!(err.to_string().contains("integrity disarmed"));
    }

    #[test]
    fn risk_blocked_blocks_submit() {
        let err = make_gateway(true, false, true)
            .submit(&make_claim(), make_submit_req())
            .unwrap_err();
        assert!(err.to_string().contains("risk engine"));
    }

    #[test]
    fn reconcile_not_clean_blocks_submit() {
        let err = make_gateway(true, true, false)
            .submit(&make_claim(), make_submit_req())
            .unwrap_err();
        assert!(err.to_string().contains("reconcile"));
    }

    #[test]
    fn integrity_checked_before_risk() {
        // All three gates false: integrity must be reported first.
        let err = make_gateway(false, false, false)
            .submit(&make_claim(), make_submit_req())
            .unwrap_err();
        assert!(err.to_string().contains("integrity disarmed"));
    }

    #[test]
    fn all_clear_cancel_succeeds() {
        let res = make_gateway(true, true, true).cancel("ord-1");
        assert!(res.is_ok());
    }

    #[test]
    fn integrity_disarmed_blocks_cancel() {
        let err = make_gateway(false, true, true).cancel("ord-1").unwrap_err();
        assert!(err.to_string().contains("integrity disarmed"));
    }

    #[test]
    fn all_clear_replace_succeeds() {
        let req = BrokerReplaceRequest {
            broker_order_id: "b-ord-1".to_string(),
            quantity: 20,
            limit_price: None,
            time_in_force: "day".to_string(),
        };
        let res = make_gateway(true, true, true).replace(req);
        assert!(res.is_ok());
    }
}
