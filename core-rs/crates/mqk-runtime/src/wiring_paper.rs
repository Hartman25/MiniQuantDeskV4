#![forbid(unsafe_code)]

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
    fn is_allowed(&self) -> bool {
        true
    }
}
impl ReconcileGate for PassGate {
    fn is_clean(&self) -> bool {
        true
    }
}

pub fn paper_gateway_for_validation(
) -> BrokerGateway<LockedPaperBroker, PassGate, PassGate, PassGate> {
    build_gateway(LockedPaperBroker::new(), PassGate, PassGate, PassGate)
}
