//! mqk-execution
//!
//! PATCH 05: Execution Engine Contract (Target Position Model)
//! - Strategies output target positions (not orders)
//! - Engine converts (current_positions, targets) -> order intents
//! - Pure deterministic logic, no broker wiring
//!
//! The `order_router` module provides the thin boundary between the internal
//! execution engine and external broker adapters.

mod engine;
mod types;

pub mod order_router;

pub use engine::targets_to_order_intents;
pub use order_router::{
    BrokerAdapter, BrokerCancelResponse, BrokerReplaceRequest, BrokerReplaceResponse,
    BrokerSubmitRequest, BrokerSubmitResponse, OrderRouter,
};
pub use types::{
    ExecutionDecision, ExecutionIntent, OrderIntent, Side, StrategyOutput, TargetPosition,
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
