//! Broker Gateway - the SINGLE choke-point for all broker operations.
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
//! argument - the gateway evaluates each gate internally. Callers cannot inject
//! a "clean" verdict; they must wire in real engine state via the gate traits.
//!
//! **Compile-time (PATCH A3 / FC-2):** `BrokerGateway::submit` requires an
//! `&OutboxClaimToken`. The token is defined in `mqk-db` with a `pub(crate)`
//! constructor; the only production path to obtain one is through
//! `mqk_db::outbox_claim_batch`, which couples each token to a real DB row lock.
//!
//! **Compile-time + runtime (EB-2):** `cancel` and `replace` require an
//! internal order ID and a `&BrokerOrderMap`. The gateway resolves the broker
//! ID internally and returns [`UnknownOrder`] if the mapping is absent -
//! preventing cancel/replace of orders not submitted by this system.
//!
//! **Runtime:** Every call to `submit / cancel / replace` invokes three gate
//! evaluators in order and refuses with `GateRefusal` if any returns `false`:
//!
//! 1. `IntegrityGate::is_armed()`  - system integrity is not disarmed or halted
//! 2. `RiskGate::evaluate_gate()`  - risk engine returned Allow for this request
//! 3. `ReconcileGate::is_clean()`  - most recent reconcile report is Clean
//!
//! Real engine implementations wire their subsystem state behind these traits.
//! Test doubles use simple boolean stubs.
use crate::broker_error::BrokerError;
use crate::id_map::BrokerOrderMap;
use crate::order_router::{
    AssetClass, BrokerAdapter, BrokerCancelResponse, BrokerReplaceRequest, BrokerReplaceResponse,
    BrokerSubmitRequest, BrokerSubmitResponse, OrderRouter,
};
use crate::risk_decision::{RiskDecision, RiskDenial};
use serde::{Deserialize, Serialize};
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
/// Read-only report of the risk engine's sticky halt state
/// (RISK-ENGINE-HALTED-VISIBILITY-01).
///
/// This is distinct from a per-request [`RiskDecision`]: `RiskState.halted`
/// is a sticky flag that, once set by the risk engine, remains `true` across
/// subsequent ticks regardless of the transient `sys_risk_block_state` DB
/// flag (which is reset every orchestrator tick at Phase 0).
///
/// `Unavailable` means the implementor cannot report sticky-halt state at
/// all (e.g. fail-closed gates, or test stubs that only implement
/// `evaluate_gate`). It is not a claim that the engine is *not* halted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskEngineHaltStatus {
    /// The implementor can report the engine's sticky halt flag.
    Known { halted: bool },
    /// The implementor cannot report sticky-halt state.
    Unavailable,
}
/// Per-order risk request context (RISK-FLATTEN-ON-HALT-01).
///
/// Carries the minimal, runtime-computed context a [`RiskGate`] needs to
/// distinguish a genuine risk-reducing close/flatten order from a new or
/// risk-increasing order. This type is independent of `mqk-risk` internals —
/// gate implementations translate it into whatever request shape their
/// underlying engine expects.
///
/// `is_risk_reducing` must only be set `true` by the caller after verifying,
/// against live portfolio state, that the order is an exact-or-smaller close
/// of an existing opposite-direction position. It must never be derived from
/// caller-supplied flags (e.g. `signal_source`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RiskRequestContext {
    pub is_risk_reducing: bool,
}

/// Evaluates whether the risk engine currently allows order submission.
///
/// Implementations must be deterministic, side-effect free, and fail-closed:
/// any state where the engine cannot be consulted must return
/// `RiskDecision::Deny` with `RiskReason::RiskEngineUnavailable`.
///
/// # Fail-closed contract
///
/// Implementors MUST NOT silently downgrade a denial to `Allow`.
/// The gateway treats `Deny` as a hard refusal regardless of which
/// `RiskReason` variant is present.
pub trait RiskGate {
    fn evaluate_gate(&self) -> RiskDecision;

    /// Read-only report of the engine's sticky halt state.
    ///
    /// Default implementation reports `Unavailable` — this is a read-only
    /// observability accessor and must never mutate engine state to answer.
    /// Only implementors that hold real `RiskState` should override this.
    fn sticky_halt_status(&self) -> RiskEngineHaltStatus {
        RiskEngineHaltStatus::Unavailable
    }

    /// Evaluate the gate for a specific per-order request context
    /// (RISK-FLATTEN-ON-HALT-01).
    ///
    /// Default implementation ignores `ctx` and delegates to
    /// [`evaluate_gate`](RiskGate::evaluate_gate), preserving existing
    /// behavior for gates that have not been updated to consider
    /// per-request risk-reducing context.
    fn evaluate_gate_for_request(&self, _ctx: RiskRequestContext) -> RiskDecision {
        self.evaluate_gate()
    }

    /// Record a real hard broker reject (RRA-3,
    /// RUNTIME-RISK-DYNAMIC-STATE-AUTHORITY-01).
    ///
    /// Called by [`BrokerGateway::submit_with_context`] ONLY when the broker
    /// adapter returns [`crate::broker_error::BrokerError::Reject`] — a
    /// confirmed hard business reject, never a transport/rate-limit/
    /// transient/auth/ambiguous outcome. Default implementation is a no-op,
    /// safe for gates that have no reject-storm state to track (test
    /// stubs, permissive gates).
    fn record_broker_reject(&self) {}
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
///
/// `RiskBlocked` carries a [`RiskDenial`] with the structured reason and
/// supporting evidence, enabling the orchestrator and diagnostics layer to
/// surface the exact denial reason to operators without re-evaluating the gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateRefusal {
    IntegrityDisarmed,
    /// The risk gate denied the request. The [`RiskDenial`] contains the
    /// structured reason code and supporting evidence.
    RiskBlocked(RiskDenial),
    ReconcileNotClean,
    /// The order's asset class is not enabled for broker dispatch.
    ///
    /// Only `AssetClass::Equity` is supported on the canonical MAIN path.
    /// All other asset classes are rejected here before any broker adapter
    /// is invoked (MULTI-ASSET-ROUTING-GUARD-01).
    AssetClassDisabled {
        asset_class: AssetClass,
    },
}
impl std::fmt::Display for GateRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateRefusal::IntegrityDisarmed => {
                write!(f, "GATE_REFUSED: integrity disarmed or halted")
            }
            GateRefusal::RiskBlocked(denial) => {
                write!(
                    f,
                    "GATE_REFUSED: risk engine did not allow [{}] {}",
                    denial.reason_code(),
                    denial.reason_summary()
                )
            }
            GateRefusal::ReconcileNotClean => {
                write!(f, "GATE_REFUSED: reconcile is not clean")
            }
            GateRefusal::AssetClassDisabled { asset_class } => {
                write!(
                    f,
                    "GATE_REFUSED: asset class {:?} is disabled — only Equity is supported on the canonical dispatch path",
                    asset_class
                )
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
/// no entry in the [`BrokerOrderMap`] - i.e., the order was never submitted
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
///                ├── RG::evaluate_gate() → GateRefusal::RiskBlocked(denial)
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
    /// `pub(crate)` - FC-3: external callers must use the production wiring path
    /// or the test escape hatch `BrokerGateway::for_test`.
    #[cfg(any(test, feature = "testkit", feature = "runtime-boundary"))]
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
    /// FC-3: mirrors `OutboxClaimToken::for_test` - explicit naming makes the
    /// test/production distinction structural rather than invisible.
    ///
    /// RT-2: gated - not available in production builds without `testkit` feature.
    #[cfg(any(test, feature = "testkit"))]
    #[doc(hidden)]
    pub fn for_test(broker: B, integrity: IG, risk: RG, reconcile: RecG) -> Self {
        Self::new(broker, integrity, risk, reconcile)
    }
    /// Evaluate all three gates in order.
    /// Returns the first refusal encountered, or `Ok(())` if all pass.
    ///
    /// Gate evaluation order:
    /// 1. `IntegrityGate::is_armed()`   - system integrity / halt state
    /// 2. `RiskGate::evaluate_gate_for_request(ctx)` - structured risk decision (B2),
    ///    with per-order risk-reducing context (RISK-FLATTEN-ON-HALT-01)
    /// 3. `ReconcileGate::is_clean()`   - reconcile drift
    fn enforce_gates(&self, ctx: RiskRequestContext) -> Result<(), GateRefusal> {
        if !self.integrity.is_armed() {
            return Err(GateRefusal::IntegrityDisarmed);
        }
        match self.risk.evaluate_gate_for_request(ctx) {
            RiskDecision::Allow => {}
            RiskDecision::Deny(denial) => {
                return Err(GateRefusal::RiskBlocked(denial));
            }
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
    /// (EB-3). Callers cannot inject a free-form broker order ID - it must
    /// come from the outbox. All three gates must also pass.
    pub fn submit(
        &self,
        claim: &OutboxClaimToken,
        req: BrokerSubmitRequest,
    ) -> Result<BrokerSubmitResponse, SubmitError> {
        self.submit_with_context(claim, req, RiskRequestContext::default())
    }
    /// Submit a new broker order with explicit per-order risk context
    /// (RISK-FLATTEN-ON-HALT-01).
    ///
    /// Identical to [`submit`](Self::submit) except the risk gate is evaluated
    /// via `RiskGate::evaluate_gate_for_request(ctx)` instead of
    /// `RiskGate::evaluate_gate()`, allowing a verified risk-reducing
    /// close/flatten order to pass a sticky risk-engine halt.
    pub fn submit_with_context(
        &self,
        claim: &OutboxClaimToken,
        req: BrokerSubmitRequest,
        ctx: RiskRequestContext,
    ) -> Result<BrokerSubmitResponse, SubmitError> {
        // MULTI-ASSET-ROUTING-GUARD-01: reject disabled asset classes before
        // any gate evaluation or broker adapter invocation.
        if req.asset_class != AssetClass::Equity {
            return Err(SubmitError::Gate(GateRefusal::AssetClassDisabled {
                asset_class: req.asset_class,
            }));
        }
        self.enforce_gates(ctx).map_err(SubmitError::Gate)?;
        // EB-3: idempotency_key from the claimed outbox row is the authoritative
        // broker-side order_id. This prevents callers from submitting free-form
        // order IDs that were not recorded in the outbox.
        let submit_req = BrokerSubmitRequest {
            order_id: claim.idempotency_key.clone(),
            ..req
        };
        match self.router.route_submit(submit_req) {
            Ok(resp) => Ok(resp),
            Err(err) => {
                // RRA-3: only a confirmed hard broker business reject counts
                // toward reject-storm protection. Transport/RateLimit/
                // Transient/AuthSession/AmbiguousSubmit/
                // InboundContinuityUnproven are deliberately excluded — none
                // of them is a broker-confirmed reject.
                if matches!(err, BrokerError::Reject { .. }) {
                    self.risk.record_broker_reject();
                }
                Err(SubmitError::Broker(err))
            }
        }
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
    ) -> Result<BrokerCancelResponse, Box<dyn std::error::Error + Send + Sync>> {
        self.enforce_gates(RiskRequestContext::default())?;
        let broker_id = order_map.broker_id(internal_id).ok_or_else(|| {
            Box::new(UnknownOrder {
                internal_id: internal_id.to_string(),
            }) as Box<dyn std::error::Error + Send + Sync>
        })?;
        self.router
            .route_cancel(broker_id)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }
    /// Read-only report of the risk engine's sticky halt state
    /// (RISK-ENGINE-HALTED-VISIBILITY-01).
    ///
    /// This is a passthrough to `RG::sticky_halt_status()`. It does not
    /// evaluate gates, does not mutate any state, and has no effect on
    /// `submit` / `cancel` / `replace` enforcement.
    pub fn risk_engine_sticky_halt(&self) -> RiskEngineHaltStatus {
        self.risk.sticky_halt_status()
    }
    /// Record a real hard broker reject against the risk engine's
    /// reject-storm window (RR4, RUNTIME-RISK-INBOUND-REJECT-AUTHORITY-01).
    ///
    /// This is a passthrough to `RG::record_broker_reject()`, exposed so the
    /// orchestrator's inbound event-apply path (Phase 3: canonical
    /// `BrokerEvent::Reject` events fetched via `fetch_events` and applied
    /// through the durable oms_inbox) can record a reject-storm hit for the
    /// SAME risk engine `submit_with_context` already records synchronous
    /// hard rejects into. Callers MUST only invoke this for a genuine,
    /// first-time, current-run-owned `BrokerEvent::Reject` application —
    /// never for `CancelReject`/`ReplaceReject`, and never more than once
    /// per physical reject.
    pub fn record_broker_reject(&self) {
        self.risk.record_broker_reject();
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
        crate::broker_error::BrokerError,
    > {
        self.router.route_fetch_events(cursor)
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
        quantity: i64,
        limit_price: Option<i64>,
        time_in_force: String,
    ) -> Result<BrokerReplaceResponse, Box<dyn std::error::Error + Send + Sync>> {
        self.enforce_gates(RiskRequestContext::default())?;
        let broker_id = order_map.broker_id(internal_id).ok_or_else(|| {
            Box::new(UnknownOrder {
                internal_id: internal_id.to_string(),
            }) as Box<dyn std::error::Error + Send + Sync>
        })?;
        self.router
            .route_replace(BrokerReplaceRequest {
                broker_order_id: broker_id.to_string(),
                quantity,
                limit_price,
                time_in_force,
            })
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }
}
// ---------------------------------------------------------------------------
// Idempotency derivation
// ---------------------------------------------------------------------------
/// Derive the stable `client_order_id` for a given intent ID.
///
/// This is the **canonical** derivation point: every call-site - first submit
/// or any subsequent retry - must use this function. Because the mapping is
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
        fn evaluate_gate(&self) -> crate::risk_decision::RiskDecision {
            if self.0 {
                crate::risk_decision::RiskDecision::Allow
            } else {
                crate::risk_decision::RiskDecision::Deny(crate::risk_decision::RiskDenial {
                    reason: crate::risk_decision::RiskReason::RiskEngineUnavailable,
                    evidence: crate::risk_decision::RiskEvidence::default(),
                })
            }
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
            asset_class: AssetClass::Equity,
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
    // RISK-FLATTEN-ON-HALT-01: gates that do not override
    // `evaluate_gate_for_request` must behave identically regardless of the
    // supplied `RiskRequestContext`, and `submit` must remain equivalent to
    // `submit_with_context(.., RiskRequestContext::default())`.
    #[test]
    fn default_evaluate_gate_for_request_ignores_context() {
        let gate = BoolGate(false);
        let via_evaluate_gate = gate.evaluate_gate();
        let via_default_ctx = gate.evaluate_gate_for_request(RiskRequestContext::default());
        let via_reducing_ctx = gate.evaluate_gate_for_request(RiskRequestContext {
            is_risk_reducing: true,
        });
        assert_eq!(via_evaluate_gate, via_default_ctx);
        assert_eq!(via_evaluate_gate, via_reducing_ctx);
    }
    #[test]
    fn submit_is_equivalent_to_submit_with_default_context() {
        let gw = make_gateway(true, true, true);
        let via_submit = gw.submit(&make_claim(), make_submit_req());
        let via_with_context = gw.submit_with_context(
            &make_claim(),
            make_submit_req(),
            RiskRequestContext::default(),
        );
        assert!(via_submit.is_ok());
        assert!(via_with_context.is_ok());
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
    // -------------------------------------------------------------------
    // RRA-3: reject events reach RiskGate::record_broker_reject exactly
    // when (and only when) the broker returns a confirmed hard Reject.
    // -------------------------------------------------------------------
    struct ErrorBroker(crate::broker_error::BrokerError);
    impl BrokerAdapter for ErrorBroker {
        fn submit_order(
            &self,
            _req: BrokerSubmitRequest,
            _token: &BrokerInvokeToken,
        ) -> Result<BrokerSubmitResponse, crate::broker_error::BrokerError> {
            Err(self.0.clone())
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

    /// Gate that is always all-clear but counts `record_broker_reject` calls.
    struct CountingRejectGate(std::sync::atomic::AtomicU32);
    impl CountingRejectGate {
        fn new() -> Self {
            Self(std::sync::atomic::AtomicU32::new(0))
        }
        fn count(&self) -> u32 {
            self.0.load(std::sync::atomic::Ordering::SeqCst)
        }
    }
    impl IntegrityGate for CountingRejectGate {
        fn is_armed(&self) -> bool {
            true
        }
    }
    impl ReconcileGate for CountingRejectGate {
        fn is_clean(&self) -> bool {
            true
        }
    }
    impl RiskGate for CountingRejectGate {
        fn evaluate_gate(&self) -> crate::risk_decision::RiskDecision {
            crate::risk_decision::RiskDecision::Allow
        }
        fn record_broker_reject(&self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn make_gateway_with_error(
        err: crate::broker_error::BrokerError,
    ) -> BrokerGateway<ErrorBroker, CountingRejectGate, CountingRejectGate, CountingRejectGate>
    {
        // Single shared gate instance used for all three roles so the same
        // counter is reachable — split via Arc-free duplication is not
        // needed because CountingRejectGate is stateless besides the
        // counter, but only ONE of the three positions is actually the
        // RiskGate consulted for `record_broker_reject`; the other two
        // roles are inert (always-pass) copies.
        BrokerGateway::new(
            ErrorBroker(err),
            CountingRejectGate::new(),
            CountingRejectGate::new(),
            CountingRejectGate::new(),
        )
    }

    #[test]
    fn hard_reject_increments_record_broker_reject_exactly_once() {
        let gw = make_gateway_with_error(crate::broker_error::BrokerError::Reject {
            code: "insufficient_qty".to_string(),
            detail: "reject".to_string(),
        });
        let err = gw
            .submit(&make_claim(), make_submit_req())
            .expect_err("Reject must surface as SubmitError::Broker");
        assert!(matches!(err, SubmitError::Broker(BrokerError::Reject { .. })));
        assert_eq!(gw.risk.count(), 1, "exactly one Reject must record exactly one count");
    }

    #[test]
    fn transport_error_does_not_increment_reject_count() {
        let gw = make_gateway_with_error(crate::broker_error::BrokerError::Transport {
            non_delivery_proven: true,
            detail: "conn refused".to_string(),
        });
        let _ = gw.submit(&make_claim(), make_submit_req());
        assert_eq!(gw.risk.count(), 0, "Transport is not a confirmed hard reject");
    }

    #[test]
    fn rate_limit_error_does_not_increment_reject_count() {
        let gw = make_gateway_with_error(crate::broker_error::BrokerError::RateLimit {
            retry_after_ms: Some(500),
            non_delivery_proven: true,
            detail: "429".to_string(),
        });
        let _ = gw.submit(&make_claim(), make_submit_req());
        assert_eq!(gw.risk.count(), 0, "RateLimit is not a confirmed hard reject");
    }

    #[test]
    fn transient_error_does_not_increment_reject_count() {
        let gw = make_gateway_with_error(crate::broker_error::BrokerError::Transient {
            detail: "5xx".to_string(),
        });
        let _ = gw.submit(&make_claim(), make_submit_req());
        assert_eq!(gw.risk.count(), 0, "Transient is not a confirmed hard reject");
    }

    #[test]
    fn auth_session_error_does_not_increment_reject_count() {
        let gw = make_gateway_with_error(crate::broker_error::BrokerError::AuthSession {
            detail: "expired".to_string(),
        });
        let _ = gw.submit(&make_claim(), make_submit_req());
        assert_eq!(gw.risk.count(), 0, "AuthSession halt policy is unchanged by reject counting");
    }

    #[test]
    fn ambiguous_submit_error_does_not_increment_reject_count() {
        let gw = make_gateway_with_error(crate::broker_error::BrokerError::AmbiguousSubmit {
            detail: "timeout after send".to_string(),
        });
        let _ = gw.submit(&make_claim(), make_submit_req());
        assert_eq!(
            gw.risk.count(),
            0,
            "AmbiguousSubmit quarantine/halt policy is unchanged by reject counting"
        );
    }

    #[test]
    fn gate_refusal_does_not_reach_broker_or_increment_reject_count() {
        // Risk gate itself refused (integrity disarmed) — the broker
        // adapter is never invoked, so no reject can be recorded.
        let gw = BrokerGateway::new(
            ErrorBroker(crate::broker_error::BrokerError::Reject {
                code: "would_be_ignored".to_string(),
                detail: "unreachable".to_string(),
            }),
            BoolGate(false), // integrity disarmed
            CountingRejectGate::new(),
            CountingRejectGate::new(),
        );
        let err = gw
            .submit(&make_claim(), make_submit_req())
            .expect_err("integrity disarmed must refuse before broker invocation");
        assert!(matches!(err, SubmitError::Gate(GateRefusal::IntegrityDisarmed)));
        assert_eq!(
            gw.risk.count(),
            0,
            "a gate refusal must never reach the broker adapter, so no reject can be recorded"
        );
    }

    #[test]
    fn n_rejects_reach_threshold_and_next_new_risk_is_refused() {
        // Simulates the RiskGate-side reject-storm contract: after the Nth
        // recorded reject, RiskGate::evaluate_gate begins denying. This
        // proves the gateway calls record_broker_reject on every Reject
        // (not just the first), by driving a gate whose evaluate_gate
        // denies once record count >= 3.
        struct ThresholdGate(std::sync::atomic::AtomicU32);
        impl IntegrityGate for ThresholdGate {
            fn is_armed(&self) -> bool {
                true
            }
        }
        impl ReconcileGate for ThresholdGate {
            fn is_clean(&self) -> bool {
                true
            }
        }
        impl RiskGate for ThresholdGate {
            fn evaluate_gate(&self) -> crate::risk_decision::RiskDecision {
                if self.0.load(std::sync::atomic::Ordering::SeqCst) >= 3 {
                    crate::risk_decision::RiskDecision::Deny(crate::risk_decision::RiskDenial {
                        reason: crate::risk_decision::RiskReason::MaxOrderSizeExceeded,
                        evidence: crate::risk_decision::RiskEvidence::default(),
                    })
                } else {
                    crate::risk_decision::RiskDecision::Allow
                }
            }
            fn record_broker_reject(&self) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let gw: BrokerGateway<ErrorBroker, ThresholdGate, ThresholdGate, ThresholdGate> =
            BrokerGateway::new(
                ErrorBroker(crate::broker_error::BrokerError::Reject {
                    code: "x".to_string(),
                    detail: "x".to_string(),
                }),
                ThresholdGate(std::sync::atomic::AtomicU32::new(0)),
                ThresholdGate(std::sync::atomic::AtomicU32::new(0)),
                ThresholdGate(std::sync::atomic::AtomicU32::new(0)),
            );

        for i in 0..3 {
            let err = gw.submit(&make_claim(), make_submit_req());
            assert!(err.is_err(), "reject #{i} must still surface as a broker error");
        }
        // 3 rejects recorded; a 4th NEW submit is now refused by the gate
        // itself, before the broker adapter is invoked again.
        let err = gw
            .submit(&make_claim(), make_submit_req())
            .expect_err("threshold reached: next new-risk submit must be gate-refused");
        assert!(matches!(err, SubmitError::Gate(GateRefusal::RiskBlocked(_))));
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
