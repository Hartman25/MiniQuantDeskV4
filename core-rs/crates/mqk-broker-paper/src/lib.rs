#![forbid(unsafe_code)]

//! Deterministic in-memory "paper" broker adapter with explicit bar-driven fills.
//!
//! Design constraints:
//! - No wall-clock reads, no RNG.
//! - All timestamps are set to 0; callers may attach timing metadata elsewhere.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use mqk_execution::{
    types::Side as ExecSide, BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerEvent,
    BrokerInvokeToken, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse,
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
    /// Durable event log: each entry is `(seq, event)`.
    ///
    /// Events are NEVER drained.  `fetch_events(cursor)` returns only events
    /// whose seq > parsed cursor, simulating a durable broker event stream.
    /// This allows replay from any past cursor, which is required for
    /// restart-safety tests (Patch A2).
    events: Vec<(u64, BrokerEvent)>,
    /// Monotonically increasing counter; incremented before each push.
    next_seq: u64,
}

impl Default for PaperInner {
    fn default() -> Self {
        Self {
            open: BTreeMap::new(),
            closed: BTreeSet::new(),
            engine: DeterministicFillEngine::new(),
            events: Vec::new(),
            next_seq: 0,
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
            for ev in evs {
                inner.next_seq += 1;
                let seq = inner.next_seq;
                inner.events.push((seq, ev));
            }

            if ord.remaining_qty == 0 {
                inner.closed.insert(k);
            } else {
                inner.open.insert(k, ord);
            }
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
        cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
        let inner = self.inner.lock().expect("poisoned mutex");
        let since_seq: u64 = cursor.and_then(|s| s.parse().ok()).unwrap_or(0);
        let batch: Vec<(u64, BrokerEvent)> = inner
            .events
            .iter()
            .filter(|(seq, _)| *seq > since_seq)
            .map(|(seq, ev)| (*seq, ev.clone()))
            .collect();
        let new_cursor = batch.last().map(|(seq, _)| seq.to_string());
        let events = batch.into_iter().map(|(_, ev)| ev).collect();
        Ok((events, new_cursor))
    }

    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        let mut inner = self.inner.lock().expect("poisoned mutex");

        // Deterministic identity mapping: broker_order_id == internal order_id.
        let broker_order_id = req.order_id.clone();

        // P1-02:
        // Submit-side direction is explicit on the request; quantity is always positive.
        let side = req.side;
        let abs_qty: i64 = (req.quantity as i64).saturating_abs();

        let state = PaperOrderState::new(req.order_id.clone(), req.symbol.clone(), side, abs_qty);

        inner.next_seq += 1;
        let seq = inner.next_seq;
        inner.events.push((
            seq,
            BrokerEvent::Ack {
                broker_message_id: format!("ack:{}", broker_order_id),
                internal_order_id: req.order_id,
                broker_order_id: Some(broker_order_id.clone()),
            },
        ));

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
    ) -> std::result::Result<BrokerCancelResponse, BrokerError> {
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
            inner.next_seq += 1;
            let seq = inner.next_seq;
            inner.events.push((
                seq,
                BrokerEvent::CancelAck {
                    broker_message_id: format!("cancel:{}", oid),
                    internal_order_id: oid.clone(),
                    broker_order_id: Some(oid.clone()),
                },
            ));
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
    ) -> std::result::Result<BrokerReplaceResponse, BrokerError> {
        let mut inner = self.inner.lock().expect("poisoned mutex");

        let oid = req.broker_order_id.clone();

        // P1-03: Reject replace on terminal (cancelled/filled/rejected) orders.
        if inner.closed.contains(&oid) {
            inner.next_seq += 1;
            let seq = inner.next_seq;
            inner.events.push((
                seq,
                BrokerEvent::ReplaceReject {
                    broker_message_id: format!("replace:{}", oid),
                    internal_order_id: oid.clone(),
                    broker_order_id: Some(oid.clone()),
                },
            ));
            return Ok(BrokerReplaceResponse {
                broker_order_id: oid,
                status: "replace_rejected".to_string(),
                replaced_at: 0,
            });
        }

        if let Some(o) = inner.open.get_mut(&oid) {
            let new_abs: i64 = (req.quantity as i64).saturating_abs();

            // P1-03: Reject replace with zero open leaves (invalid quantity).
            if new_abs == 0 {
                inner.next_seq += 1;
                let seq = inner.next_seq;
                inner.events.push((
                    seq,
                    BrokerEvent::ReplaceReject {
                        broker_message_id: format!("replace:{}", oid),
                        internal_order_id: oid.clone(),
                        broker_order_id: Some(oid.clone()),
                    },
                ));
                return Ok(BrokerReplaceResponse {
                    broker_order_id: oid,
                    status: "replace_rejected".to_string(),
                    replaced_at: 0,
                });
            }

            // P1-03: Preserve already-filled quantity across replace.
            // `req.quantity` is the new OPEN leaves after the amend, not a
            // reset of cumulative fill history.
            //
            // Example:
            //   original_qty  = 100, remaining_qty = 60 → filled = 40
            //   replace qty   = 25  (new open leaves)
            //
            // Result:
            //   original_qty  = 65  (40 filled + 25 new leaves)
            //   remaining_qty = 25
            //   filled_qty    = 40  (preserved)
            //   new_total_qty = 65  (carried in ReplaceAck for OMS update)
            let filled = o.original_qty.saturating_sub(o.remaining_qty);
            let new_total_qty = filled.saturating_add(new_abs);
            o.original_qty = new_total_qty;
            o.remaining_qty = new_abs;

            inner.next_seq += 1;
            let seq = inner.next_seq;
            inner.events.push((
                seq,
                BrokerEvent::ReplaceAck {
                    broker_message_id: format!("replace:{}", oid),
                    internal_order_id: oid.clone(),
                    broker_order_id: Some(oid.clone()),
                    new_total_qty,
                },
            ));

            return Ok(BrokerReplaceResponse {
                broker_order_id: oid,
                status: "replaced".to_string(),
                replaced_at: 0,
            });
        }

        // P1-03: Unknown order — reject.
        inner.next_seq += 1;
        let seq = inner.next_seq;
        inner.events.push((
            seq,
            BrokerEvent::ReplaceReject {
                broker_message_id: format!("replace:{}", oid),
                internal_order_id: oid.clone(),
                broker_order_id: Some(oid.clone()),
            },
        ));
        Ok(BrokerReplaceResponse {
            broker_order_id: oid,
            status: "replace_rejected".to_string(),
            replaced_at: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_order_uses_explicit_side_and_positive_quantity() {
        let broker = LockedPaperBroker::new();
        let token = BrokerInvokeToken::for_test();
        let resp = broker
            .submit_order(
                BrokerSubmitRequest {
                    order_id: "ord-sell".to_string(),
                    symbol: "SPY".to_string(),
                    side: ExecSide::Sell,
                    quantity: 100,
                    order_type: "market".to_string(),
                    limit_price: None,
                    time_in_force: "day".to_string(),
                },
                &token,
            )
            .expect("submit must succeed");
        assert_eq!(resp.broker_order_id, "ord-sell");

        let snap = broker.snapshot();
        let ord = snap.orders.get("ord-sell").expect("order must exist");
        assert!(matches!(ord.side, mqk_reconcile::Side::Sell));
        assert_eq!(ord.qty, 100);
    }

    #[test]
    fn replace_order_preserves_existing_side_and_normalizes_quantity() {
        let broker = LockedPaperBroker::new();
        let token = BrokerInvokeToken::for_test();

        broker
            .submit_order(
                BrokerSubmitRequest {
                    order_id: "ord-1".to_string(),
                    symbol: "SPY".to_string(),
                    side: ExecSide::Sell,
                    quantity: 100,
                    order_type: "market".to_string(),
                    limit_price: None,
                    time_in_force: "day".to_string(),
                },
                &token,
            )
            .expect("submit must succeed");

        broker
            .replace_order(
                BrokerReplaceRequest {
                    broker_order_id: "ord-1".to_string(),
                    quantity: 75,
                    limit_price: None,
                    time_in_force: "day".to_string(),
                },
                &token,
            )
            .expect("replace must succeed");

        let snap = broker.snapshot();
        let ord = snap.orders.get("ord-1").expect("order must exist");
        assert!(matches!(ord.side, mqk_reconcile::Side::Sell));
        assert_eq!(ord.qty, 75);
    }

    #[test]
    fn replace_after_partial_fill_preserves_filled_qty_and_updates_remaining() {
        let broker = LockedPaperBroker::new();
        let token = BrokerInvokeToken::for_test();

        {
            let mut inner = broker.inner.lock().expect("poisoned mutex");
            inner.open.insert(
                "ord-pf".to_string(),
                PaperOrderState {
                    internal_order_id: "ord-pf".to_string(),
                    symbol: "SPY".to_string(),
                    side: ExecSide::Sell,
                    original_qty: 100,
                    remaining_qty: 60,
                    fill_seq: 1,
                },
            );
        }

        broker
            .replace_order(
                BrokerReplaceRequest {
                    broker_order_id: "ord-pf".to_string(),
                    quantity: 25,
                    limit_price: None,
                    time_in_force: "day".to_string(),
                },
                &token,
            )
            .expect("replace must succeed");

        let snap = broker.snapshot();
        let ord = snap.orders.get("ord-pf").expect("order must exist");
        assert!(matches!(ord.side, mqk_reconcile::Side::Sell));
        assert_eq!(
            ord.qty, 65,
            "qty must equal preserved filled + new remaining"
        );
        assert_eq!(
            ord.filled_qty, 40,
            "replace must preserve already-filled quantity"
        );
        assert!(matches!(ord.status, OrderStatus::PartiallyFilled));

        let inner = broker.inner.lock().expect("poisoned mutex");
        let state = inner.open.get("ord-pf").expect("state must remain open");
        assert_eq!(state.original_qty, 65);
        assert_eq!(state.remaining_qty, 25);
    }

    #[test]
    fn cancel_after_partial_fill_then_replace_does_not_resurrect_order() {
        let broker = LockedPaperBroker::new();
        let token = BrokerInvokeToken::for_test();

        {
            let mut inner = broker.inner.lock().expect("poisoned mutex");
            inner.open.insert(
                "ord-cxl".to_string(),
                PaperOrderState {
                    internal_order_id: "ord-cxl".to_string(),
                    symbol: "AAPL".to_string(),
                    side: ExecSide::Buy,
                    original_qty: 100,
                    remaining_qty: 40,
                    fill_seq: 1,
                },
            );
        }

        broker
            .cancel_order("ord-cxl", &token)
            .expect("cancel must succeed");

        broker
            .replace_order(
                BrokerReplaceRequest {
                    broker_order_id: "ord-cxl".to_string(),
                    quantity: 25,
                    limit_price: None,
                    time_in_force: "day".to_string(),
                },
                &token,
            )
            .expect("replace after cancel must return idempotent success");

        let snap = broker.snapshot();
        assert!(
            !snap.orders.contains_key("ord-cxl"),
            "replace after terminal cancel must not resurrect the order"
        );

        let (events, _cursor) = broker
            .fetch_events(None, &token)
            .expect("fetch events must succeed");
        assert!(events.iter().any(
            |ev| matches!(ev, BrokerEvent::CancelAck { internal_order_id, .. } if internal_order_id == "ord-cxl")
        ));
        // P1-03: replace after cancel must emit ReplaceReject (terminal order).
        assert!(events.iter().any(
            |ev| matches!(ev, BrokerEvent::ReplaceReject { internal_order_id, .. } if internal_order_id == "ord-cxl")
        ));
    }

    #[test]
    fn replace_after_terminal_fill_does_not_resurrect_order() {
        let broker = LockedPaperBroker::new();
        let token = BrokerInvokeToken::for_test();

        {
            let mut inner = broker.inner.lock().expect("poisoned mutex");
            inner.closed.insert("ord-filled".to_string());
        }

        broker
            .replace_order(
                BrokerReplaceRequest {
                    broker_order_id: "ord-filled".to_string(),
                    quantity: 10,
                    limit_price: None,
                    time_in_force: "day".to_string(),
                },
                &token,
            )
            .expect("replace against terminal order must be idempotent");

        let snap = broker.snapshot();
        assert!(!snap.orders.contains_key("ord-filled"));
    }
}
