#![forbid(unsafe_code)]

//! Execution-side types + order intent generation utilities.

pub mod broker_error;
mod engine;
pub mod gateway;
mod id_map;
mod order_router;
mod prices;
mod reconcile_guard;
pub mod risk_decision;
pub mod types;

// Optional submodules used by runtime boundary wiring / OMS integration.
pub mod oms;

pub use engine::targets_to_order_intents;

// Re-export core strategy/execution types.
pub use types::{
    ExecutionDecision, ExecutionIntent, OrderIntent, Side, StrategyOutput, TargetPosition,
};

// RESEARCH-NON-EQ-01: V2 multi-asset scaffold — NOT wired into canonical MAIN execution path.
//
// These types (OrderIntentV2, ExecutionIntentV2, equity_instrument) exist for
// forward-compatible multi-asset schema design only.  No orchestrator, runtime,
// or broker adapter path in MAIN imports or uses them.  Only "equity" is
// supported on the canonical execution path; Gate 0 at signal admission and
// asset_class_scope: "equity_only" on /api/v1/system/status are the active
// enforcement surfaces.  Do not wire V2 types into MAIN paths without a
// dedicated scope-reviewed patch.
pub use types::{equity_instrument, ExecutionIntentV2, OrderIntentV2};

// Price fixed-point helpers expected by testkit.
pub use prices::{micros_to_price, price_to_micros, MICROS_PER_UNIT};

// Reconcile freshness guard expected by testkit.
pub use reconcile_guard::ReconcileFreshnessGuard;

// Re-export the broker-facing contract types that downstream crates use.
pub use id_map::BrokerOrderMap;

pub use order_router::{
    BrokerAdapter, BrokerCancelResponse, BrokerEvent, BrokerEventIdentity, BrokerInvokeToken,
    BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
};

pub use gateway::{
    intent_id_to_client_order_id, BrokerGateway, GateRefusal, IntegrityGate, OutboxClaimToken,
    ReconcileGate, RiskGate, SubmitError, UnknownOrder,
};

pub use risk_decision::{RiskDecision, RiskDenial, RiskEvidence, RiskReason};

pub use broker_error::BrokerError;

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
