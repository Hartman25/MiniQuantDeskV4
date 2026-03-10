#![forbid(unsafe_code)]
#![cfg(any(test, feature = "testkit"))]

use mqk_execution::gateway::{BrokerGateway, IntegrityGate, ReconcileGate, RiskGate};
use mqk_execution::wiring::build_gateway;

use mqk_broker_paper::LockedPaperBroker;

#[derive(Clone, Copy)]
pub struct PassGate;

impl IntegrityGate for PassGate {
    fn is_armed(&self) -> bool {
        true
    }
}
impl RiskGate for PassGate {
    fn evaluate_gate(&self) -> mqk_execution::RiskDecision {
        mqk_execution::RiskDecision::Allow
    }
}
impl ReconcileGate for PassGate {
    fn is_clean(&self) -> bool {
        true
    }
}

/// TESTKIT ONLY.
/// This exists to run deterministic paper loops in validation harnesses.
/// MUST NOT be used by production binaries.
pub fn paper_gateway_for_testkit_validation(
) -> BrokerGateway<LockedPaperBroker, PassGate, PassGate, PassGate> {
    build_gateway(LockedPaperBroker::new(), PassGate, PassGate, PassGate)
}
