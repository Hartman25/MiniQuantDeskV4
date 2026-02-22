//! Deterministic in-memory "paper" broker adapter.
//!
//! Design decisions (kept intentionally simple/deterministic):
//! - `broker_order_id` is exactly `client_order_id`.
//! - `broker_message_id` is a stable string derived from request inputs:
//!     - submit:  "paper:msg:submit:{client_order_id}"
//!     - cancel:  "paper:msg:cancel:{client_order_id}"
//!     - replace: "paper:msg:replace:{client_order_id}:{new_qty}"
//!     - snapshot:"paper:msg:snapshot"
//! - No randomness. No timestamps.
//! - Fills are not auto-generated. If you later need fills, add an explicit
//!   deterministic "apply_fill" method and derive `broker_fill_id` from
//!   (client_order_id, fill_seq).
//!
//! This crate is intended to satisfy the Broker Adapter Contract (V4):
//! submit/cancel/replace + fetch snapshots (orders/positions/account-ish).
//! For Patch 25, we implement submit/cancel/list_orders/positions/snapshot
//! and keep replace as a minimal deterministic stub.

use std::collections::BTreeMap;

use mqk_reconcile::{BrokerSnapshot, OrderSnapshot, OrderStatus, Side};

pub mod types;

use types::{BrokerMessageId, CancelRequest, ReplaceRequest, SubmitOrder, SubmitResponse};

#[derive(Clone, Debug, Default)]
pub struct PaperBroker {
    orders: BTreeMap<String, OrderSnapshot>, // keyed by broker_order_id (== client_order_id)
    positions: BTreeMap<String, i64>,        // symbol -> qty_signed
}

impl PaperBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Submit a new order.
    ///
    /// Deterministic behavior:
    /// - If an order with the same `client_order_id` already exists, we treat submit as idempotent
    ///   and return the same response (no mutation).
    pub fn submit(&mut self, req: SubmitOrder) -> SubmitResponse {
        let broker_order_id = req.client_order_id.clone();
        let msg = BrokerMessageId::new(format!("paper:msg:submit:{}", req.client_order_id));

        if let Some(existing) = self.orders.get(&broker_order_id) {
            return SubmitResponse {
                broker_message_id: msg,
                broker_order_id,
                snapshot: existing.clone(),
            };
        }

        // Minimal "accepted" model: this broker accepts immediately.
        let snap = OrderSnapshot::new(
            broker_order_id.clone(),
            req.symbol,
            req.side,
            req.qty,
            0,
            OrderStatus::Accepted,
        );

        self.orders.insert(broker_order_id.clone(), snap.clone());

        SubmitResponse {
            broker_message_id: msg,
            broker_order_id,
            snapshot: snap,
        }
    }

    /// Cancel an order (idempotent).
    pub fn cancel(&mut self, req: CancelRequest) -> BrokerMessageId {
        let msg = BrokerMessageId::new(format!("paper:msg:cancel:{}", req.client_order_id));

        if let Some(ord) = self.orders.get_mut(&req.client_order_id) {
            ord.status = OrderStatus::Canceled;
        }

        msg
    }

    /// Replace an order's quantity (minimal deterministic stub).
    ///
    /// If the order doesn't exist, this is a no-op but still returns a deterministic message id.
    pub fn replace(&mut self, req: ReplaceRequest) -> BrokerMessageId {
        let msg = BrokerMessageId::new(format!(
            "paper:msg:replace:{}:{}",
            req.client_order_id, req.new_qty
        ));

        if let Some(ord) = self.orders.get_mut(&req.client_order_id) {
            ord.qty = req.new_qty;
        }

        msg
    }

    /// Deterministic listing: BTreeMap iteration order is stable.
    pub fn list_orders(&self) -> Vec<OrderSnapshot> {
        self.orders.values().cloned().collect()
    }

    pub fn positions(&self) -> BTreeMap<String, i64> {
        self.positions.clone()
    }

    /// Set a position deterministically for test setup / scenario wiring.
    pub fn set_position(&mut self, symbol: impl Into<String>, qty_signed: i64) {
        self.positions.insert(symbol.into(), qty_signed);
    }

    /// Produce a broker snapshot compatible with mqk-reconcile.
    pub fn snapshot(&self) -> (BrokerMessageId, BrokerSnapshot) {
        let msg = BrokerMessageId::new("paper:msg:snapshot".to_string());
        let snap = BrokerSnapshot {
            orders: self.orders.clone(),
            positions: self.positions.clone(),
            fetched_at_ms: 0,
        };
        (msg, snap)
    }

    /// Helper for tests: create a "local view" that matches this broker snapshot.
    pub fn as_local_snapshot(&self) -> mqk_reconcile::LocalSnapshot {
        mqk_reconcile::LocalSnapshot {
            orders: self.orders.clone(),
            positions: self.positions.clone(),
        }
    }
}

/// Convenience constructors for common values used by tests/examples.
pub fn buy(symbol: impl Into<String>, qty: i64, client_order_id: impl Into<String>) -> SubmitOrder {
    SubmitOrder {
        client_order_id: client_order_id.into(),
        symbol: symbol.into(),
        side: Side::Buy,
        qty,
    }
}

pub fn sell(
    symbol: impl Into<String>,
    qty: i64,
    client_order_id: impl Into<String>,
) -> SubmitOrder {
    SubmitOrder {
        client_order_id: client_order_id.into(),
        symbol: symbol.into(),
        side: Side::Sell,
        qty,
    }
}
