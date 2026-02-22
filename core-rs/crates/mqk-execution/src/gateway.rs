//! Broker Gateway — the SINGLE choke-point for all broker operations.
//!
//! # Invariant (enforced at both compile-time and runtime)
//!
//! **Compile-time:** `OrderRouter` is `pub(crate)` and is never re-exported
//! from `lib.rs`. External crates have no way to construct one. The only
//! public API that reaches a broker adapter is `BrokerGateway`.
//!
//! **Runtime:** Every call to `submit / cancel / replace` evaluates three
//! gate verdicts in order and refuses with `GateRefusal` if any fails:
//!
//! 1. `integrity_armed`  — system integrity is not disarmed or halted
//! 2. `risk_allowed`     — risk engine returned Allow for this request
//! 3. `reconcile_clean`  — most recent reconcile report is Clean
//!
//! Callers evaluate each verdict from the respective engine and pass the
//! result here. The gateway is the final policy enforcer.

use crate::order_router::{
    BrokerAdapter, BrokerCancelResponse, BrokerReplaceRequest, BrokerReplaceResponse,
    BrokerSubmitRequest, BrokerSubmitResponse, OrderRouter,
};

// ---------------------------------------------------------------------------
// GateVerdicts
// ---------------------------------------------------------------------------

/// Pre-evaluated gate verdicts the caller must supply before every broker op.
///
/// | Field             | Source                                      |
/// |-------------------|---------------------------------------------|
/// | `integrity_armed` | `!IntegrityState::is_execution_blocked()`   |
/// | `risk_allowed`    | `RiskDecision::action == RiskAction::Allow` |
/// | `reconcile_clean` | `ReconcileReport::is_clean()`               |
#[derive(Debug, Clone)]
pub struct GateVerdicts {
    pub integrity_armed: bool,
    pub risk_allowed: bool,
    pub reconcile_clean: bool,
}

impl GateVerdicts {
    /// All gates clear — convenience helper for paper/test mode.
    pub fn all_clear() -> Self {
        Self {
            integrity_armed: true,
            risk_allowed: true,
            reconcile_clean: true,
        }
    }
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
// BrokerGateway
// ---------------------------------------------------------------------------

/// The SINGLE choke-point through which ALL broker operations must flow.
///
/// # Architecture
///
/// `BrokerGateway` owns a **private** `OrderRouter<B>`. Because `OrderRouter`
/// is `pub(crate)`, it cannot be constructed or accessed from any crate
/// outside `mqk-execution`. The only way external code can reach a broker
/// adapter is through the public methods defined here — all of which evaluate
/// the three gate checks before delegating.
///
/// ```text
/// External code
///     │
///     └──► BrokerGateway::submit / cancel / replace
///                │
///                ├── enforce_gates (integrity + risk + reconcile)
///                │        └── GateRefusal  ◄── refused here if any fails
///                │
///                └── OrderRouter::route_*  ◄── only reached if all clear
///                         └── BrokerAdapter::*
/// ```
pub struct BrokerGateway<B: BrokerAdapter> {
    /// Private: unreachable from outside `mqk-execution`.
    router: OrderRouter<B>,
}

impl<B: BrokerAdapter> BrokerGateway<B> {
    /// Create a gateway wrapping the given broker adapter.
    pub fn new(broker: B) -> Self {
        Self {
            router: OrderRouter::new(broker),
        }
    }

    /// Evaluate all three gate verdicts in order.
    /// Returns the first refusal encountered, or `Ok(())` if all pass.
    fn enforce_gates(verdicts: &GateVerdicts) -> Result<(), GateRefusal> {
        if !verdicts.integrity_armed {
            return Err(GateRefusal::IntegrityDisarmed);
        }
        if !verdicts.risk_allowed {
            return Err(GateRefusal::RiskBlocked);
        }
        if !verdicts.reconcile_clean {
            return Err(GateRefusal::ReconcileNotClean);
        }
        Ok(())
    }

    /// Submit a new broker order.
    ///
    /// All three gates must be clear. Returns `GateRefusal` if any gate fails.
    pub fn submit(
        &self,
        req: BrokerSubmitRequest,
        verdicts: &GateVerdicts,
    ) -> Result<BrokerSubmitResponse, Box<dyn std::error::Error>> {
        Self::enforce_gates(verdicts)?;
        self.router.route_submit(req)
    }

    /// Cancel a broker order.
    ///
    /// All three gates must be clear. Returns `GateRefusal` if any gate fails.
    pub fn cancel(
        &self,
        order_id: &str,
        verdicts: &GateVerdicts,
    ) -> Result<BrokerCancelResponse, Box<dyn std::error::Error>> {
        Self::enforce_gates(verdicts)?;
        self.router.route_cancel(order_id)
    }

    /// Replace a broker order.
    ///
    /// All three gates must be clear. Returns `GateRefusal` if any gate fails.
    pub fn replace(
        &self,
        req: BrokerReplaceRequest,
        verdicts: &GateVerdicts,
    ) -> Result<BrokerReplaceResponse, Box<dyn std::error::Error>> {
        Self::enforce_gates(verdicts)?;
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
        BrokerAdapter, BrokerCancelResponse, BrokerReplaceRequest, BrokerReplaceResponse,
        BrokerSubmitRequest, BrokerSubmitResponse,
    };

    struct AlwaysOkBroker;

    impl BrokerAdapter for AlwaysOkBroker {
        fn submit_order(
            &self,
            req: BrokerSubmitRequest,
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
        ) -> Result<BrokerReplaceResponse, Box<dyn std::error::Error>> {
            Ok(BrokerReplaceResponse {
                broker_order_id: req.broker_order_id,
                replaced_at: 1,
                status: "ok".to_string(),
            })
        }
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

    #[test]
    fn all_clear_submit_succeeds() {
        let gw = BrokerGateway::new(AlwaysOkBroker);
        let res = gw.submit(make_submit_req(), &GateVerdicts::all_clear());
        assert!(res.is_ok());
    }

    #[test]
    fn integrity_disarmed_blocks_submit() {
        let gw = BrokerGateway::new(AlwaysOkBroker);
        let verdicts = GateVerdicts {
            integrity_armed: false,
            risk_allowed: true,
            reconcile_clean: true,
        };
        let err = gw.submit(make_submit_req(), &verdicts).unwrap_err();
        assert!(err.to_string().contains("integrity disarmed"));
    }

    #[test]
    fn risk_blocked_blocks_submit() {
        let gw = BrokerGateway::new(AlwaysOkBroker);
        let verdicts = GateVerdicts {
            integrity_armed: true,
            risk_allowed: false,
            reconcile_clean: true,
        };
        let err = gw.submit(make_submit_req(), &verdicts).unwrap_err();
        assert!(err.to_string().contains("risk engine"));
    }

    #[test]
    fn reconcile_not_clean_blocks_submit() {
        let gw = BrokerGateway::new(AlwaysOkBroker);
        let verdicts = GateVerdicts {
            integrity_armed: true,
            risk_allowed: true,
            reconcile_clean: false,
        };
        let err = gw.submit(make_submit_req(), &verdicts).unwrap_err();
        assert!(err.to_string().contains("reconcile"));
    }

    #[test]
    fn integrity_checked_before_risk() {
        let gw = BrokerGateway::new(AlwaysOkBroker);
        let verdicts = GateVerdicts {
            integrity_armed: false,
            risk_allowed: false,
            reconcile_clean: false,
        };
        let err = gw.submit(make_submit_req(), &verdicts).unwrap_err();
        // Integrity is checked first.
        assert!(err.to_string().contains("integrity disarmed"));
    }

    #[test]
    fn all_clear_cancel_succeeds() {
        let gw = BrokerGateway::new(AlwaysOkBroker);
        let res = gw.cancel("ord-1", &GateVerdicts::all_clear());
        assert!(res.is_ok());
    }

    #[test]
    fn integrity_disarmed_blocks_cancel() {
        let gw = BrokerGateway::new(AlwaysOkBroker);
        let verdicts = GateVerdicts {
            integrity_armed: false,
            risk_allowed: true,
            reconcile_clean: true,
        };
        let err = gw.cancel("ord-1", &verdicts).unwrap_err();
        assert!(err.to_string().contains("integrity disarmed"));
    }

    #[test]
    fn all_clear_replace_succeeds() {
        let gw = BrokerGateway::new(AlwaysOkBroker);
        let req = BrokerReplaceRequest {
            broker_order_id: "b-ord-1".to_string(),
            quantity: 20,
            limit_price: None,
            time_in_force: "day".to_string(),
        };
        let res = gw.replace(req, &GateVerdicts::all_clear());
        assert!(res.is_ok());
    }
}
