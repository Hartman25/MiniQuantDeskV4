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

// ---------------------------------------------------------------------------
// RT-8: LockedPaperBroker — implements BrokerAdapter for production wiring
// ---------------------------------------------------------------------------

/// Thread-safe paper broker adapter that implements [`mqk_execution::BrokerAdapter`].
///
/// Wraps a `Mutex`-protected event queue. On each broker operation (submit,
/// cancel, replace) an acknowledgement event is queued in-process. The
/// orchestrator drains the queue via `fetch_events`, which is then persisted
/// to `oms_inbox` for deterministic replay.
///
/// # Determinism guarantees
/// - No randomness; no timestamps; no real I/O.
/// - `broker_order_id` == `internal_order_id` (the system-assigned order ID
///   is reused directly — no separate broker sequence space for paper).
/// - `broker_message_id` is derived as `"paper:<event>:<order_id>"`.
/// - Duplicate `submit_order` calls with the same `order_id` are idempotent:
///   only one `Ack` event is queued regardless of how many times submit is
///   called (matches the outbox's idempotency key contract).
pub struct LockedPaperBroker {
    inner: std::sync::Mutex<PaperInner>,
}

struct PaperInner {
    /// Order IDs that have already been submitted (for idempotent re-submit).
    submitted: std::collections::BTreeSet<String>,
    /// Events waiting to be returned by `fetch_events`.
    pending: Vec<mqk_execution::BrokerEvent>,
}

impl LockedPaperBroker {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(PaperInner {
                submitted: Default::default(),
                pending: Vec::new(),
            }),
        }
    }
}

impl Default for LockedPaperBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl mqk_execution::BrokerAdapter for LockedPaperBroker {
    fn submit_order(
        &self,
        req: mqk_execution::BrokerSubmitRequest,
        _token: &mqk_execution::BrokerInvokeToken,
    ) -> std::result::Result<mqk_execution::BrokerSubmitResponse, Box<dyn std::error::Error>> {
        let mut inner = self.inner.lock().expect("paper broker lock poisoned");

        // Idempotent: queue exactly one Ack per order_id.
        if inner.submitted.insert(req.order_id.clone()) {
            inner.pending.push(mqk_execution::BrokerEvent::Ack {
                broker_message_id: format!("paper:ack:{}", req.order_id),
                internal_order_id: req.order_id.clone(),
                // RT-9: paper broker reuses internal_order_id as broker_order_id;
                // Phase 1 already registers this pair, so None avoids redundancy.
                broker_order_id: None,
            });
        }

        Ok(mqk_execution::BrokerSubmitResponse {
            broker_order_id: req.order_id,
            submitted_at: 0,
            status: "paper-accepted".to_string(),
        })
    }

    fn cancel_order(
        &self,
        order_id: &str,
        _token: &mqk_execution::BrokerInvokeToken,
    ) -> std::result::Result<mqk_execution::BrokerCancelResponse, Box<dyn std::error::Error>> {
        let mut inner = self.inner.lock().expect("paper broker lock poisoned");
        inner.pending.push(mqk_execution::BrokerEvent::CancelAck {
            broker_message_id: format!("paper:cancel-ack:{}", order_id),
            internal_order_id: order_id.to_string(),
            broker_order_id: None,
        });
        Ok(mqk_execution::BrokerCancelResponse {
            broker_order_id: order_id.to_string(),
            cancelled_at: 0,
            status: "paper-cancelled".to_string(),
        })
    }

    fn replace_order(
        &self,
        req: mqk_execution::BrokerReplaceRequest,
        _token: &mqk_execution::BrokerInvokeToken,
    ) -> std::result::Result<mqk_execution::BrokerReplaceResponse, Box<dyn std::error::Error>> {
        let mut inner = self.inner.lock().expect("paper broker lock poisoned");
        inner.pending.push(mqk_execution::BrokerEvent::ReplaceAck {
            broker_message_id: format!("paper:replace-ack:{}", req.broker_order_id),
            internal_order_id: req.broker_order_id.clone(),
        });
        Ok(mqk_execution::BrokerReplaceResponse {
            broker_order_id: req.broker_order_id,
            replaced_at: 0,
            status: "paper-replaced".to_string(),
        })
    }

    /// Drain and return all pending events.
    ///
    /// Events are produced synchronously by submit/cancel/replace and queued
    /// in-process. Each call fully drains the queue — the orchestrator persists
    /// every event to `oms_inbox` with dedup on `broker_message_id`, so
    /// re-delivery is safe.
    fn fetch_events(
        &self,
        _token: &mqk_execution::BrokerInvokeToken,
    ) -> std::result::Result<Vec<mqk_execution::BrokerEvent>, Box<dyn std::error::Error>> {
        let mut inner = self.inner.lock().expect("paper broker lock poisoned");
        Ok(std::mem::take(&mut inner.pending))
    }
}

// ---------------------------------------------------------------------------
// Convenience constructors for common values used by tests/examples.
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
