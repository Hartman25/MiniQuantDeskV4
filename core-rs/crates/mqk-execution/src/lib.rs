#![forbid(unsafe_code)]

//! Execution-side types + order intent generation utilities.

mod engine;
pub mod gateway;
mod id_map;
mod order_router;
mod prices;
mod reconcile_guard;
pub mod types;

// Optional submodules used by runtime boundary wiring / OMS integration.
pub mod oms;

pub use engine::targets_to_order_intents;

// Re-export core strategy/execution types.
pub use types::{
    equity_instrument, ExecutionDecision, ExecutionIntent, ExecutionIntentV2, OrderIntent,
    OrderIntentV2, Side, StrategyOutput, TargetPosition,
};

// Price fixed-point helpers expected by testkit.
pub use prices::{micros_to_price, price_to_micros, MICROS_PER_UNIT};

// Reconcile freshness guard expected by testkit.
pub use reconcile_guard::ReconcileFreshnessGuard;

// Re-export the broker-facing contract types that downstream crates use.
pub use id_map::BrokerOrderMap;

pub use order_router::{
    BrokerAdapter, BrokerCancelResponse, BrokerEvent, BrokerInvokeToken, BrokerReplaceRequest,
    BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
};

pub use gateway::{
    intent_id_to_client_order_id, BrokerGateway, GateRefusal, IntegrityGate, OutboxClaimToken,
    ReconcileGate, RiskGate, UnknownOrder,
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
pub fn broker_positions_to_map(
    xs: &[mqk_schemas::BrokerPosition],
) -> std::collections::BTreeMap<String, i64> {
    let mut book = std::collections::BTreeMap::<String, i64>::new();
    for x in xs {
        if let Ok(q) = x.qty.parse::<i64>() {
            book.insert(x.symbol.clone(), q);
        }
    }
    book
}

#[cfg(feature = "runtime-boundary")]
pub mod wiring;

use std::collections::BTreeMap;

/// Helper for tests/examples: build a symbol->qty map from an iterator.
#[must_use]
pub fn position_book<I, S>(items: I) -> BTreeMap<String, i64>
where
    I: IntoIterator<Item = (S, i64)>,
    S: Into<String>,
{
    items
        .into_iter()
        .map(|(sym, qty)| (sym.into(), qty))
        .collect()
}
