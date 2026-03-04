#![forbid(unsafe_code)]

//! Deterministic in-memory "paper" broker adapter with explicit bar-driven fills.
//!
//! Notes:
//! - This crate is intentionally deterministic: no wall-clock reads, no RNG.
//! - `BrokerSnapshot.fetched_at_ms` is set to 0 (callers may override via reconcile tick metadata).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerEvent, BrokerInvokeToken, BrokerReplaceRequest,
    BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
};
use mqk_reconcile::{BrokerSnapshot, OrderSnapshot, OrderStatus, Side as ReconSide};

mod fill_engine;
pub mod types;

use fill_engine::{Bar, DeterministicFillEngine, FillMode, FillSpec, PaperOrderState};
use mqk_execution::types::Side as ExecSide;

#[derive(Debug, Default)]
pub struct LockedPaperBroker {
    inner: Mutex<PaperInner>,
}

#[derive(Debug, Default)]
struct PaperInner {
    /// Open orders keyed by broker_order_id.
    open: BTreeMap<String, PaperOrderState>,
    /// Closed order ids (terminal state observed). Kept for idempotent cancel/replace.
    closed: BTreeSet<String>,
    /// Deterministic fill engine.
    engine: DeterministicFillEngine,
    /// FIFO-ish event queue (deterministic append order).
    events: Vec<BrokerEvent>,
}

impl LockedPaperBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drive fills by providing a bar. The deterministic fill engine will emit events
    /// based on the configured fill mode/spec for each open order.
    pub fn on_bar(&self, bar: Bar) {
        let mut inner = self.inner.lock().expect("poisoned mutex");

        // Collect keys first for stable iteration.
        let keys: Vec<String> = inner.open.keys().cloned().collect();

        for k in keys {
            // Temporarily remove to avoid borrowing `inner` immutably & mutably at the same time.
            let Some(mut ord) = inner.open.remove(&k) else {
                continue;
            };

            let evs = inner.engine.apply_bar_to_order(&bar, &mut ord);

            // Apply terminal transitions / reinsert.
            let mut terminal = false;
            for ev in evs {
                if matches!(ev, BrokerEvent::Fill { .. } | BrokerEvent::Canceled { .. }) {
                    // Fill can be partial; terminal is decided by remaining_qty below.
                }
                inner.events.push(ev);
            }

            if ord.remaining_qty == 0 {
                terminal = true;
            }

            if terminal {
                inner.closed.insert(k.clone());
            } else {
                inner.open.insert(k, ord);
            }
        }
    }

    /// Configure fill behavior for future orders.
    pub fn set_fill_mode(&self, mode: FillMode) {
        let mut inner = self.inner.lock().expect("poisoned mutex");
        inner.engine.set_fill_mode(mode);
    }

    /// Configure a per-symbol fill spec override.
    pub fn set_fill_spec(&self, symbol: &str, spec: FillSpec) {
        let mut inner = self.inner.lock().expect("poisoned mutex");
        inner.engine.set_fill_spec(symbol, spec);
    }

    fn exec_side_from_qty(qty: i64) -> ExecSide {
        if qty >= 0 {
            ExecSide::Buy
        } else {
            ExecSide::Sell
        }
    }

    fn recon_side_from_exec(side: ExecSide) -> ReconSide {
        match side {
            ExecSide::Buy => ReconSide::Buy,
            ExecSide::Sell => ReconSide::Sell,
        }
    }

    fn order_snapshot_from_state(order_id: &str, s: &PaperOrderState) -> OrderSnapshot {
        let status = if s.remaining_qty == 0 {
            OrderStatus::Filled
        } else {
            OrderStatus::Open
        };

        OrderSnapshot {
            order_id: order_id.to_string(),
            symbol: s.symbol.clone(),
            side: Self::recon_side_from_exec(s.side),
            quantity: s.original_qty,
            status,
        }
    }

    fn list_orders_map(inner: &PaperInner) -> BTreeMap<String, OrderSnapshot> {
        let mut m = BTreeMap::new();
        for (oid, s) in inner.open.iter() {
            m.insert(oid.clone(), Self::order_snapshot_from_state(oid, s));
        }
        m
    }

    /// Deterministic snapshot of broker state.
    pub fn snapshot(&self) -> BrokerSnapshot {
        let inner = self.inner.lock().expect("poisoned mutex");
        BrokerSnapshot {
            account_id: "paper".to_string(),
            fetched_at_ms: 0,
            orders: Self::list_orders_map(&inner),
            fills: Vec::new(),
        }
    }
}

impl BrokerAdapter for LockedPaperBroker {
    fn fetch_events(
        &self,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<Vec<BrokerEvent>, Box<dyn std::error::Error + 'static>> {
        let mut inner = self.inner.lock().expect("poisoned mutex");
        Ok(std::mem::take(&mut inner.events))
    }

    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, Box<dyn std::error::Error + 'static>> {
        let mut inner = self.inner.lock().expect("poisoned mutex");

        // Use order_id as broker_order_id for deterministic identity mapping.
        let broker_order_id = req.order_id.clone();

        // Side is inferred from sign; store absolute quantities in order state.
        let side = Self::exec_side_from_qty(req.quantity);
        let abs_qty: i64 = req.quantity.abs();

        let state = PaperOrderState {
            symbol: req.symbol.clone(),
            side,
            original_qty: abs_qty,
            remaining_qty: abs_qty,
            limit_price_micros: req.limit_price_micros,
        };

        // Emit ack deterministically.
        inner.events.push(BrokerEvent::Ack {
            internal_order_id: req.order_id.clone(),
            broker_order_id: Some(broker_order_id.clone()),
        });

        inner.open.insert(broker_order_id.clone(), state);

        Ok(BrokerSubmitResponse { broker_order_id })
    }

    fn cancel_order(
        &self,
        broker_order_id: &str,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerCancelResponse, Box<dyn std::error::Error + 'static>> {
        let mut inner = self.inner.lock().expect("poisoned mutex");

        // Idempotent cancel.
        if inner.closed.contains(broker_order_id) {
            return Ok(BrokerCancelResponse {
                canceled: true,
                broker_order_id: broker_order_id.to_string(),
            });
        }

        if inner.open.remove(broker_order_id).is_some() {
            inner.closed.insert(broker_order_id.to_string());
            inner.events.push(BrokerEvent::Canceled {
                internal_order_id: broker_order_id.to_string(),
                broker_order_id: Some(broker_order_id.to_string()),
            });
        }

        Ok(BrokerCancelResponse {
            canceled: true,
            broker_order_id: broker_order_id.to_string(),
        })
    }

    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerReplaceResponse, Box<dyn std::error::Error + 'static>> {
        let mut inner = self.inner.lock().expect("poisoned mutex");

        let broker_order_id = req.broker_order_id.clone();

        // Idempotent replace against terminal orders is a no-op but "ok".
        if inner.closed.contains(&broker_order_id) {
            inner.events.push(BrokerEvent::ReplaceAck {
                internal_order_id: broker_order_id.clone(),
                broker_order_id: Some(broker_order_id.clone()),
            });
            return Ok(BrokerReplaceResponse {
                replaced: true,
                broker_order_id,
            });
        }

        if let Some(o) = inner.open.get_mut(&broker_order_id) {
            // Replace semantics: update remaining_qty and limit if provided.
            // Quantity is absolute; side doesn't change on replace.
            let new_abs = req.quantity.abs();
            o.remaining_qty = new_abs;
            o.original_qty = new_abs;
            o.limit_price_micros = req.limit_price_micros;
        }

        inner.events.push(BrokerEvent::ReplaceAck {
            internal_order_id: broker_order_id.clone(),
            broker_order_id: Some(broker_order_id.clone()),
        });

        Ok(BrokerReplaceResponse {
            replaced: true,
            broker_order_id,
        })
    }
}
