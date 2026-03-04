#![forbid(unsafe_code)]

//! Execution-side types + order intent generation utilities.

mod engine;
pub mod gateway;
mod id_map;
mod order_router;
pub mod types;

pub use engine::targets_to_order_intents;

// Re-export core strategy/execution types.
pub use types::{
    equity_instrument, ExecutionDecision, ExecutionIntent, ExecutionIntentV2, OrderIntent,
    OrderIntentV2, Side, StrategyOutput, TargetPosition,
};

// Re-export the broker-facing contract types that downstream crates use.
// (Do NOT re-export OrderRouter itself.)
pub use id_map::BrokerOrderMap;

pub use order_router::{
    BrokerAdapter, BrokerCancelResponse, BrokerEvent, BrokerInvokeToken, BrokerReplaceRequest,
    BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
};

pub use gateway::{
    BrokerGateway, GateRefusal, IntegrityGate, OutboxClaimToken, ReconcileGate, RiskGate,
    UnknownOrder,
};

// --- Patch L1: choke-point exports ---

/// deterministic helper: stable sort of (symbol, qty) positions.
pub fn stable_sort_positions(mut xs: Vec<TargetPosition>) -> Vec<TargetPosition> {
    xs.sort_by(|a, b| a.symbol.cmp(&b.symbol).then(a.qty.cmp(&b.qty)));
    xs
}

/// deterministic helper: stable sort order intents by client_id.
pub fn stable_sort_intents(mut xs: Vec<OrderIntent>) -> Vec<OrderIntent> {
    xs.sort_by(|a, b| a.client_order_id.cmp(&b.client_order_id));
    xs
}

/// Convert a list of `TargetPosition` into a map symbol -> qty.
pub fn targets_to_map(xs: &[TargetPosition]) -> std::collections::BTreeMap<String, i64> {
    let mut book = std::collections::BTreeMap::<String, i64>::new();
    for x in xs {
        book.insert(x.symbol.clone(), x.qty);
    }
    book
}

/// Convert a list of `BrokerPosition` into a map symbol -> qty.
///
/// This is used in tests and in the runtime boundary when calculating deltas.
pub fn broker_positions_to_map(
    xs: &[mqk_schemas::BrokerPosition],
) -> std::collections::BTreeMap<String, i64> {
    let mut book = std::collections::BTreeMap::<String, i64>::new();
    for x in xs {
        // broker snapshot qty comes in as string; parse as i64 shares for now.
        // This stays conservative until V2 execution is fully wired.
        if let Ok(q) = x.qty.parse::<i64>() {
            book.insert(x.symbol.clone(), q);
        }
    }
    book
}

#[cfg(feature = "runtime-boundary")]
pub mod wiring;
