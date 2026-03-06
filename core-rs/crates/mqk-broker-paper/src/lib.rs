#![forbid(unsafe_code)]

//! Deterministic in-memory "paper" broker adapter with explicit bar-driven fills.
//!
//! Design constraints:
//! - No wall-clock reads, no RNG.
//! - All timestamps are set to 0; callers may attach timing metadata elsewhere.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use mqk_execution::{
    types::Side as ExecSide, BrokerAdapter, BrokerCancelResponse, BrokerEvent, BrokerInvokeToken,
    BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
};
use mqk_reconcile::{BrokerSnapshot, OrderSnapshot, OrderStatus};

mod fill_engine;
pub mod types;

use fill_engine::{Bar, DeterministicFillEngine, PaperOrderState};

#[derive(Debug, Default)]
pub struct LockedPaperBroker {
    inner: Mutex<PaperInner>,
}

#[derive(Debug)]
struct PaperInner {
    /// Open orders keyed by broker_order_id.
    open: BTreeMap<String, PaperOrderState>,
    /// Closed order ids (terminal state observed). Kept for idempotent cancel/replace.
    closed: BTreeSet<String>,
    /// Deterministic fill engine.
    engine: DeterministicFillEngine,
    /// FIFO event queue (deterministic append order).
    events: Vec<BrokerEvent>,
}

impl Default for PaperInner {
    fn default() -> Self {
        Self {
            open: BTreeMap::new(),
            closed: BTreeSet::new(),
            engine: DeterministicFillEngine::new(),
            events: Vec::new(),
        }
    }
}

impl LockedPaperBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drive fills by providing a bar. The deterministic fill engine will emit events
    /// based on the configured fill spec for each open order.
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
            inner.events.extend(evs);

            if ord.remaining_qty == 0 {
                inner.closed.insert(k);
            } else {
                inner.open.insert(k, ord);
            }
        }
    }

    fn exec_side_from_signed_qty(qty: i32) -> ExecSide {
        if qty >= 0 {
            ExecSide::Buy
        } else {
            ExecSide::Sell
        }
    }

    fn order_snapshot_from_state(oid: &str, s: &PaperOrderState) -> OrderSnapshot {
        let filled = s.original_qty.saturating_sub(s.remaining_qty);
        let status = if s.remaining_qty == 0 {
            OrderStatus::Filled
        } else if filled > 0 {
            OrderStatus::PartiallyFilled
        } else {
            OrderStatus::New
        };

        OrderSnapshot {
            order_id: oid.to_string(),
            symbol: s.symbol.clone(),
            side: match s.side {
                ExecSide::Buy => mqk_reconcile::Side::Buy,
                ExecSide::Sell => mqk_reconcile::Side::Sell,
            },
            qty: s.original_qty,
            filled_qty: filled,
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
            fetched_at_ms: 0,
            orders: Self::list_orders_map(&inner),
            positions: BTreeMap::new(),
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

        // Deterministic identity mapping: broker_order_id == internal order_id.
        let broker_order_id = req.order_id.clone();

        // Side is inferred from sign; fill engine uses absolute quantity.
        let side = Self::exec_side_from_signed_qty(req.quantity);
        let abs_qty: i64 = (req.quantity as i64).abs();

        let state = PaperOrderState::new(req.order_id.clone(), req.symbol.clone(), side, abs_qty);

        inner.events.push(BrokerEvent::Ack {
            broker_message_id: format!("ack:{}", broker_order_id),
            internal_order_id: req.order_id,
            broker_order_id: Some(broker_order_id.clone()),
        });

        inner.open.insert(broker_order_id.clone(), state);

        Ok(BrokerSubmitResponse {
            broker_order_id,
            status: "accepted".to_string(),
            submitted_at: 0,
        })
    }

    fn cancel_order(
        &self,
        broker_order_id: &str,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerCancelResponse, Box<dyn std::error::Error + 'static>> {
        let mut inner = self.inner.lock().expect("poisoned mutex");

        let oid = broker_order_id.to_string();

        // Idempotent cancel.
        if inner.closed.contains(&oid) {
            return Ok(BrokerCancelResponse {
                broker_order_id: oid,
                status: "cancelled".to_string(),
                cancelled_at: 0,
            });
        }

        if inner.open.remove(&oid).is_some() {
            inner.closed.insert(oid.clone());
            inner.events.push(BrokerEvent::CancelAck {
                broker_message_id: format!("cancel:{}", oid),
                internal_order_id: oid.clone(),
                broker_order_id: Some(oid.clone()),
            });
        }

        Ok(BrokerCancelResponse {
            broker_order_id: oid,
            status: "cancelled".to_string(),
            cancelled_at: 0,
        })
    }

    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerReplaceResponse, Box<dyn std::error::Error + 'static>> {
        let mut inner = self.inner.lock().expect("poisoned mutex");

        let oid = req.broker_order_id.clone();

        // Idempotent replace against terminal orders is a no-op but still "ok".
        if inner.closed.contains(&oid) {
            inner.events.push(BrokerEvent::ReplaceAck {
                broker_message_id: format!("replace:{}", oid),
                internal_order_id: oid.clone(),
                broker_order_id: Some(oid.clone()),
            });
            return Ok(BrokerReplaceResponse {
                broker_order_id: oid,
                status: "replaced".to_string(),
                replaced_at: 0,
            });
        }

        if let Some(o) = inner.open.get_mut(&oid) {
            let side = Self::exec_side_from_signed_qty(req.quantity);
            let new_abs: i64 = (req.quantity as i64).abs();
            o.side = side;
            o.original_qty = new_abs;
            o.remaining_qty = new_abs;
        }

        inner.events.push(BrokerEvent::ReplaceAck {
            broker_message_id: format!("replace:{}", oid),
            internal_order_id: oid.clone(),
            broker_order_id: Some(oid.clone()),
        });

        Ok(BrokerReplaceResponse {
            broker_order_id: oid,
            status: "replaced".to_string(),
            replaced_at: 0,
        })
    }
}
