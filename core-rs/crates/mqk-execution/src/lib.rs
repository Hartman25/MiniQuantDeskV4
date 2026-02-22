//! mqk-execution
//!
//! PATCH 05: Execution Engine Contract (Target Position Model)
//! - Strategies output target positions (not orders)
//! - Engine converts (current_positions, targets) -> order intents
//! - Pure deterministic logic, no broker wiring
//!
//! The `order_router` module provides the thin boundary between the internal
//! execution engine and external broker adapters.
//!
//! PATCH L1: Single Submission Choke-Point
//! - `order_router` is crate-private (never re-exported)
//! - `BrokerGateway` is the only public path to broker operations
//! - Gate checks enforced before every broker operation

mod engine;
mod types;

// OMS state machine — Patch L4.
pub mod oms;

// Crate-private: prevents external bypass.
mod gateway;
mod order_router;

pub use engine::targets_to_order_intents;

pub use types::{
    ExecutionDecision, ExecutionIntent, OrderIntent, Side, StrategyOutput, TargetPosition,
};

// --- Patch L1: choke-point exports ---

/// The single public gateway for all broker operations.
/// `OrderRouter` is intentionally NOT exported.
pub use gateway::{intent_id_to_client_order_id, BrokerGateway, GateRefusal, GateVerdicts};

/// Broker adapter trait + request/response types.
/// External crates may implement `BrokerAdapter` and build request structs,
/// but can only route them through `BrokerGateway`.
pub use order_router::{
    BrokerAdapter, BrokerCancelResponse, BrokerReplaceRequest, BrokerReplaceResponse,
    BrokerSubmitRequest, BrokerSubmitResponse,
};

use std::collections::BTreeMap;

/// Canonical type for current positions, keyed by symbol.
/// Signed quantity: +long, -short.
pub type PositionBook = BTreeMap<String, i64>;

/// Helper to build a PositionBook with minimal boilerplate in tests/callers.
pub fn position_book<I, S>(items: I) -> PositionBook
where
    I: IntoIterator<Item = (S, i64)>,
    S: Into<String>,
{
    let mut book = PositionBook::new();
    for (sym, qty) in items {
        book.insert(sym.into(), qty);
    }
    book
}
