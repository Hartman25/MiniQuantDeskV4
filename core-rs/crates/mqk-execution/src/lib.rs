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
mod reconcile_guard; // Patch B3 — production ReconcileGate implementation.

// Patch L9 — integer micros + broker ID mapping.
mod id_map;
mod prices;

pub use engine::targets_to_order_intents;

pub use types::{
    ExecutionDecision, ExecutionIntent, OrderIntent, Side, StrategyOutput, TargetPosition,
};

// --- Patch L1: choke-point exports ---

/// The single public gateway for all broker operations.
/// `OrderRouter` is intentionally NOT exported.
///
/// Gate evaluator traits (`IntegrityGate`, `RiskGate`, `ReconcileGate`) are
/// exported so callers can wire real engine state. `GateVerdicts` is removed
/// (PATCH A2) — gates are no longer caller-supplied booleans.
///
/// `OutboxClaimToken` is exported so callers can construct the token after a
/// successful DB claim (PATCH A3). Its `_priv` field is `pub(crate)`, so it
/// cannot be constructed via struct literal outside `mqk-execution`.
pub use gateway::{
    intent_id_to_client_order_id, BrokerGateway, GateRefusal, IntegrityGate, OutboxClaimToken,
    ReconcileGate, RiskGate,
};

// Patch B3 — production ReconcileGate: fail-closed freshness guard with
// injectable clock for deterministic testing.
pub use reconcile_guard::ReconcileFreshnessGuard;

/// Broker adapter trait + request/response types.
/// External crates may implement `BrokerAdapter` and build request structs,
/// but can only route them through `BrokerGateway`.
///
/// `BrokerInvokeToken` is exported so external crates can **name** it in
/// trait implementations, but its inner field is `pub(crate)` — external
/// crates cannot construct one. See PATCH A1.
pub use order_router::{
    BrokerAdapter, BrokerCancelResponse, BrokerInvokeToken, BrokerReplaceRequest,
    BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
};

// --- Patch L9: integer micros price surface + broker ID mapping ---

/// In-memory map of internal order IDs → broker-assigned order IDs.
/// Required for cancel/replace to target the correct broker order.
pub use id_map::BrokerOrderMap;

/// Price conversion helpers: `i64` micros ↔ `f64` (wire boundary only).
/// `PricingError` is the error type returned by [`price_to_micros`] when the
/// input is non-finite or would overflow `i64` (PATCH A5).
pub use prices::{micros_to_price, price_to_micros, PricingError, MICROS_PER_UNIT};

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
