#![forbid(unsafe_code)]

use crate::gateway::{BrokerGateway, IntegrityGate, ReconcileGate, RiskGate};
use crate::order_router::BrokerAdapter;

/// Runtime-only wiring function to construct a BrokerGateway using the production constructor.
///
/// This is gated behind the `runtime-boundary` feature so non-runtime crates cannot call it by default.
pub fn build_gateway<B, IG, RG, RecG>(
    broker: B,
    integrity: IG,
    risk: RG,
    reconcile: RecG,
) -> BrokerGateway<B, IG, RG, RecG>
where
    B: BrokerAdapter,
    IG: IntegrityGate,
    RG: RiskGate,
    RecG: ReconcileGate,
{
    // Calls the crate-private constructor; safe because this module lives inside mqk-execution.
    BrokerGateway::new(broker, integrity, risk, reconcile)
}
