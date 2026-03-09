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
//! **Compile-time (PATCH A3 / FC-2):** `BrokerGateway::submit` requires an
//! `&OutboxClaimToken`. The token is defined in `mqk-db` with a `pub(crate)`
//! constructor; the only production path to obtain one is through
//! `mqk_db::outbox_claim_batch`, which couples each token to a real DB row lock.
//!
//! **Compile-time + runtime (EB-2):** `cancel` and `replace` require an
//! internal order ID and a `&BrokerOrderMap`. The gateway resolves the broker
//! ID internally and returns [`UnknownOrder`] if the mapping is absent —
//! preventing cancel/replace of orders not submitted by this system.
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

use crate::broker_error::BrokerError;
use crate::id_map::BrokerOrderMap;
use crate::order_router::{
    BrokerAdapter, BrokerCancelResponse, BrokerReplaceRequest, BrokerReplaceResponse,
    BrokerSubmitRequest, BrokerSubmitResponse, OrderRouter,
};

// FC-2: OutboxClaimToken now lives in mqk-db (the only crate whose
// `outbox_claim_batch` function constructs it).  Re-exported below so
// existing `use mqk_execution::OutboxClaimToken` imports continue to work.
pub use mqk_db::OutboxClaimToken;

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
// SubmitError (A3)
// ---------------------------------------------------------------------------

/// Error returned by [`BrokerGateway::submit`].
///
/// Distinguishes gate refusals (request never reached the broker) from
/// classified broker errors, enabling the orchestrator to apply per-class
/// outbox row disposition without downcasting.
#[derive(Debug)]
pub enum SubmitError {
    /// A gate evaluator refused the submit before the request was sent.
    Gate(GateRefusal),
    /// The broker adapter returned a classified error.
    Broker(BrokerError),
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubmitError::Gate(r) => write!(f, "SUBMIT_GATE_REFUSED: {r}"),
            SubmitError::Broker(e) => write!(f, "SUBMIT_BROKER_ERROR: {e}"),
        }
    }
}

impl std::error::Error for SubmitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SubmitError::Gate(r) => Some(r),
            SubmitError::Broker(e) => Some(e),
        }
    }
}

// ---------------------------------------------------------------------------
// UnknownOrder (EB-2)
// ---------------------------------------------------------------------------

/// Returned when `cancel` or `replace` targets an internal order ID that has
/// no entry in the [`BrokerOrderMap`] — i.e., the order was never submitted
/// by this system, or has already been deregistered (EB-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownOrder {
    /// The internal order ID that had no broker mapping.
    pub internal_id: String,
}

impl std::fmt::Display for UnknownOrder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CANCEL_REPLACE_REFUSED: no broker mapping for internal order '{}'",
            self.internal_id
        )
    }
}

impl std::error::Error for UnknownOrder {}

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
    /// `pub(crate)` — FC-3: external callers must use the production wiring path
    /// or the test escape hatch `BrokerGateway::for_test`.
    pub(crate) fn new(broker: B, integrity: IG, risk: RG, reconcile: RecG) -> Self {
        Self {
            router: OrderRouter::new(broker),
            integrity,
            risk,
            reconcile,
        }
    }

    /// Test-only constructor.
    ///
    /// The name is intentionally explicit: callers outside `mqk-execution` that
    /// use this function are declaring that they are constructing a gateway with
    /// stub gate evaluators for test purposes, not production wiring.
    ///
    /// In production, a gateway is constructed by the runtime orchestration layer
    /// using real engine objects wired behind the gate traits.
    ///
    /// FC-3: mirrors `OutboxClaimToken::for_test` — explicit naming makes the
    /// test/production distinction structural rather than invisible.
    ///
    /// RT-2: gated — not available in production builds without `testkit` feature.
    #[cfg(any(test, feature = "testkit"))]
    #[doc(hidden)]
    pub fn for_test(broker: B, integrity: IG, risk: RG, reconcile: RecG) -> Self {
        Self::new(broker, integrity, risk, reconcile)
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
    /// claimed outbox row (PATCH A3 / FC-2). Tokens are returned by
    /// `mqk_db::outbox_claim_batch`; the only test escape hatch is
    /// `OutboxClaimToken::for_test`. The claim's `idempotency_key` is used
    /// as the broker-side `order_id`, overriding any value in `req.order_id`
    /// (EB-3). Callers cannot inject a free-form broker order ID — it must
    /// come from the outbox. All three gates must also pass.
    pub fn submit(
        &self,
        claim: &OutboxClaimToken,
        req: BrokerSubmitRequest,
    ) -> Result<BrokerSubmitResponse, SubmitError> {
        self.enforce_gates().map_err(SubmitError::Gate)?;
        // EB-3: idempotency_key from the claimed outbox row is the authoritative
        // broker-side order_id. This prevents callers from submitting free-form
        // order IDs that were not recorded in the outbox.
        let submit_req = BrokerSubmitRequest {
            order_id: claim.idempotency_key.clone(),
            ..req
        };
        self.router
            .route_submit(submit_req)
            .map_err(SubmitError::Broker)
    }

    /// Cancel a broker order.
    ///
    /// `internal_id` is the system-assigned order ID registered in `order_map`
    /// after a successful submit. The gateway resolves it to the broker-assigned
    /// ID internally. Returns [`UnknownOrder`] if the mapping is absent (EB-2).
    /// All three gates must also pass.
    pub fn cancel(
        &self,
        internal_id: &str,
        order_map: &BrokerOrderMap,
    ) -> Result<BrokerCancelResponse, Box<dyn std::error::Error>> {
        self.enforce_gates()?;
        let broker_id = order_map.broker_id(internal_id).ok_or_else(|| {
            Box::new(UnknownOrder {
                internal_id: internal_id.to_string(),
            }) as Box<dyn std::error::Error>
        })?;
        self.router
            .route_cancel(broker_id)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
    }

    /// Fetch new broker events since `cursor`.
    ///
    /// This is a read-only operation; gate checks are NOT applied.  The system
    /// must be able to receive events even when disarmed (e.g. during crash
    /// recovery).  The orchestrator persists each event to `oms_inbox` with
    /// dedup on `broker_message_id` BEFORE advancing the cursor, so a crash
    /// between the two steps is safe.
    pub fn fetch_events(
        &self,
        cursor: Option<&str>,
    ) -> std::result::Result<
        (Vec<crate::order_router::BrokerEvent>, Option<String>),
        Box<dyn std::error::Error>,
    > {
        self.router
            .route_fetch_events(cursor)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
    }

    /// Replace a broker order.
    ///
    /// `internal_id` is the system-assigned order ID registered in `order_map`
    /// after a successful submit. The gateway resolves it to the broker-assigned
    /// ID internally. Returns [`UnknownOrder`] if the mapping is absent (EB-2).
    /// All three gates must also pass.
    pub fn replace(
        &self,
        internal_id: &str,
        order_map: &BrokerOrderMap,
        quantity: i32,
        limit_price: Option<i64>,
        time_in_force: String,
    ) -> Result<BrokerReplaceResponse, Box<dyn std::error::Error>> {
        self.enforce_gates()?;
        let broker_id = order_map.broker_id(internal_id).ok_or_else(|| {
            Box::new(UnknownOrder {
                internal_id: internal_id.to_string(),
            }) as Box<dyn std::error::Error>
        })?;
        self.router
            .route_replace(BrokerReplaceRequest {
                broker_order_id: broker_id.to_string(),
                quantity,
                limit_price,
                time_in_force,
            })
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
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
        ) -> Result<BrokerSubmitResponse, crate::broker_error::BrokerError> {
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
        ) -> Result<BrokerCancelResponse, crate::broker_error::BrokerError> {
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
        ) -> Result<BrokerReplaceResponse, crate::broker_error::BrokerError> {
            Ok(BrokerReplaceResponse {
                broker_order_id: req.broker_order_id,
                replaced_at: 1,
                status: "ok".to_string(),
            })
        }

        fn fetch_events(
            &self,
            _cursor: Option<&str>,
            _token: &BrokerInvokeToken,
        ) -> Result<
            (Vec<crate::order_router::BrokerEvent>, Option<String>),
            crate::broker_error::BrokerError,
        > {
            Ok((vec![], None))
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
            side: crate::types::Side::Buy,
            quantity: 10,
            order_type: "market".to_string(),
            limit_price: None,
            time_in_force: "day".to_string(),
        }
    }

    /// Stub claim token for unit tests. Uses the test escape hatch (FC-2).
    fn make_claim() -> OutboxClaimToken {
        OutboxClaimToken::for_test(1, "ord-1")
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
        let mut map = crate::id_map::BrokerOrderMap::new();
        map.register("ord-1", "b-ord-1");
        let res = make_gateway(true, true, true).cancel("ord-1", &map);
        assert!(res.is_ok());
    }

    #[test]
    fn integrity_disarmed_blocks_cancel() {
        // Gate is evaluated before map lookup; empty map is acceptable.
        let map = crate::id_map::BrokerOrderMap::new();
        let err = make_gateway(false, true, true)
            .cancel("ord-1", &map)
            .unwrap_err();
        assert!(err.to_string().contains("integrity disarmed"));
    }

    #[test]
    fn all_clear_replace_succeeds() {
        let mut map = crate::id_map::BrokerOrderMap::new();
        map.register("ord-1", "b-ord-1");
        let res =
            make_gateway(true, true, true).replace("ord-1", &map, 20, None, "day".to_string());
        assert!(res.is_ok());
    }
}
