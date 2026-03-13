//! Order Router: crate-private broker delegation layer.
//!
//! This module is intentionally NOT re-exported from `lib.rs`.
//! External crates must use [`crate::BrokerGateway`], which is the only
//! public path to broker operations and enforces all gate checks.
//!
//! `OrderRouter` and its methods are `pub(crate)` — they cannot be
//! constructed or called from outside `mqk-execution`.

use crate::broker_error::BrokerError;

/// Convenience alias used throughout this module.
type Result<T> = std::result::Result<T, BrokerError>;

// ---------------------------------------------------------------------------
// BrokerEvent — canonical inbound broker event type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillIdentityStrength {
    /// Stable, broker-native economic fill identity.
    StrongBrokerNative,
    /// No broker-native fill identity was provided; only message identity exists.
    WeakMessageDerived,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BrokerEventIdentity {
    pub broker_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker_fill_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_identity_strength: Option<FillIdentityStrength>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker_sequence_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker_timestamp: Option<String>,
}

/// A broker-sourced lifecycle event for an in-flight order.
///
/// Produced by [`BrokerAdapter::fetch_events`] and persisted to `oms_inbox`
/// via JSON serialisation before being applied to the OMS state machine and
/// portfolio.  The `broker_message_id` is the deduplication key for inbox
/// insertion.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrokerEvent {
    Ack {
        broker_message_id: String,
        internal_order_id: String,
        /// RT-9: the broker/exchange-assigned order ID carried in the Ack.
        /// `None` for adapters that do not distinguish broker ID from internal ID
        /// (e.g. paper). `Some` for live adapters where the exchange assigns its
        /// own ID asynchronously. When `Some`, Phase 3b updates `BrokerOrderMap`
        /// with the authoritative ID.
        broker_order_id: Option<String>,
    },
    PartialFill {
        broker_message_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        broker_fill_id: Option<String>,
        internal_order_id: String,
        /// RT-9: broker-assigned order ID associated with this fill event.
        broker_order_id: Option<String>,
        symbol: String,
        side: crate::types::Side,
        delta_qty: i64,
        price_micros: i64,
        fee_micros: i64,
    },
    Fill {
        broker_message_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        broker_fill_id: Option<String>,
        internal_order_id: String,
        /// RT-9: broker-assigned order ID associated with this fill event.
        broker_order_id: Option<String>,
        symbol: String,
        side: crate::types::Side,
        delta_qty: i64,
        price_micros: i64,
        fee_micros: i64,
    },
    CancelAck {
        broker_message_id: String,
        internal_order_id: String,
        /// RT-9: broker-assigned order ID for the cancelled order.
        broker_order_id: Option<String>,
    },
    CancelReject {
        broker_message_id: String,
        internal_order_id: String,
        /// RT-9: broker-assigned order ID for the rejected cancel.
        broker_order_id: Option<String>,
    },
    ReplaceAck {
        broker_message_id: String,
        internal_order_id: String,
        /// RT-9: broker-assigned order ID for the replaced order.
        broker_order_id: Option<String>,
        /// P1-03: authoritative post-replace total quantity.
        /// Equals filled_qty_at_replace + new_open_leaves.
        /// Used by the OMS to update `OmsOrder::total_qty` so subsequent fills
        /// validate against the amended order size rather than the original.
        new_total_qty: i64,
    },
    ReplaceReject {
        broker_message_id: String,
        internal_order_id: String,
        /// RT-9: broker-assigned order ID for the rejected replace.
        broker_order_id: Option<String>,
    },
    Reject {
        broker_message_id: String,
        internal_order_id: String,
        /// RT-9: broker-assigned order ID for the rejected order.
        broker_order_id: Option<String>,
    },
}

impl BrokerEvent {
    /// The deduplication key used for inbox insertion.
    pub fn broker_message_id(&self) -> &str {
        match self {
            Self::Ack {
                broker_message_id, ..
            }
            | Self::PartialFill {
                broker_message_id, ..
            }
            | Self::Fill {
                broker_message_id, ..
            }
            | Self::CancelAck {
                broker_message_id, ..
            }
            | Self::CancelReject {
                broker_message_id, ..
            }
            | Self::ReplaceAck {
                broker_message_id, ..
            }
            | Self::ReplaceReject {
                broker_message_id, ..
            }
            | Self::Reject {
                broker_message_id, ..
            } => broker_message_id.as_str(),
        }
    }

    /// Optional economic fill identity carried by this event.
    ///
    /// This value is distinct from `broker_message_id`: a broker can emit
    /// multiple transport messages that refer to the same underlying fill.
    /// Adapters should populate this only when the broker supplies a truthful
    /// fill identifier.
    pub fn broker_fill_id(&self) -> Option<&str> {
        match self {
            Self::PartialFill { broker_fill_id, .. } | Self::Fill { broker_fill_id, .. } => {
                broker_fill_id.as_deref()
            }
            _ => None,
        }
    }

    /// Strength classification of fill identity semantics for this event.
    ///
    /// - `Some(StrongBrokerNative)` for fill events that carry a stable
    ///   broker-native economic fill id.
    /// - `Some(WeakMessageDerived)` for fill events that do not carry a
    ///   broker-native fill id and would otherwise fall back to transport
    ///   message identity.
    /// - `None` for non-fill lifecycle events.
    pub fn fill_identity_strength(&self) -> Option<FillIdentityStrength> {
        match self {
            Self::PartialFill { broker_fill_id, .. } | Self::Fill { broker_fill_id, .. } => {
                Some(if broker_fill_id.is_some() {
                    FillIdentityStrength::StrongBrokerNative
                } else {
                    FillIdentityStrength::WeakMessageDerived
                })
            }
            _ => None,
        }
    }

    /// Canonical identity tuple for this broker event.
    pub fn identity(&self) -> BrokerEventIdentity {
        BrokerEventIdentity {
            broker_message_id: self.broker_message_id().to_string(),
            broker_fill_id: self.broker_fill_id().map(ToString::to_string),
            fill_identity_strength: self.fill_identity_strength(),
            broker_sequence_id: None,
            broker_timestamp: None,
        }
    }

    /// The broker/exchange-assigned order ID carried in this event, if any.
    ///
    /// `None` for adapters that do not distinguish broker ID from internal ID
    /// (e.g. paper).  `Some` for live adapters.  Phase 3b uses this value to
    /// update `BrokerOrderMap` when a real Ack arrives.
    pub fn broker_order_id(&self) -> Option<&str> {
        match self {
            Self::Ack {
                broker_order_id, ..
            }
            | Self::PartialFill {
                broker_order_id, ..
            }
            | Self::Fill {
                broker_order_id, ..
            }
            | Self::CancelAck {
                broker_order_id, ..
            }
            | Self::CancelReject {
                broker_order_id, ..
            }
            | Self::ReplaceAck {
                broker_order_id, ..
            }
            | Self::ReplaceReject {
                broker_order_id, ..
            }
            | Self::Reject {
                broker_order_id, ..
            } => broker_order_id.as_deref(),
        }
    }

    /// The system-assigned order ID this event pertains to.
    pub fn internal_order_id(&self) -> &str {
        match self {
            Self::Ack {
                internal_order_id, ..
            }
            | Self::PartialFill {
                internal_order_id, ..
            }
            | Self::Fill {
                internal_order_id, ..
            }
            | Self::CancelAck {
                internal_order_id, ..
            }
            | Self::CancelReject {
                internal_order_id, ..
            }
            | Self::ReplaceAck {
                internal_order_id, ..
            }
            | Self::ReplaceReject {
                internal_order_id, ..
            }
            | Self::Reject {
                internal_order_id, ..
            } => internal_order_id.as_str(),
        }
    }
}

// ---------------------------------------------------------------------------
// Public request / response types (external crates need these to build reqs)
// ---------------------------------------------------------------------------

/// Broker-agnostic order submission request.
///
/// `limit_price` is in **integer micros** (Patch L9). Use `crate::micros_to_price`
/// only when serialising to a broker REST payload.
#[derive(Debug, Clone)]
pub struct BrokerSubmitRequest {
    pub order_id: String,
    pub symbol: String,
    /// Direction of the order. Quantity is always positive; side carries direction.
    pub side: crate::types::Side,
    pub quantity: i32,
    pub order_type: String,
    /// Limit price in integer micros (1 unit = 1_000_000). `None` for market orders.
    pub limit_price: Option<i64>,
    pub time_in_force: String,
}

/// Broker-agnostic order submission response.
#[derive(Debug, Clone)]
pub struct BrokerSubmitResponse {
    pub broker_order_id: String,
    pub submitted_at: u64,
    pub status: String,
}

/// Broker-agnostic order cancellation response.
#[derive(Debug, Clone)]
pub struct BrokerCancelResponse {
    pub broker_order_id: String,
    pub cancelled_at: u64,
    pub status: String,
}

/// Broker-agnostic order replacement request.
///
/// `limit_price` is in **integer micros** (Patch L9).
#[derive(Debug, Clone)]
pub struct BrokerReplaceRequest {
    pub broker_order_id: String,
    pub quantity: i32,
    /// Limit price in integer micros (1 unit = 1_000_000). `None` for market orders.
    pub limit_price: Option<i64>,
    pub time_in_force: String,
}

/// Broker-agnostic order replacement response.
#[derive(Debug, Clone)]
pub struct BrokerReplaceResponse {
    pub broker_order_id: String,
    pub replaced_at: u64,
    pub status: String,
}

// ---------------------------------------------------------------------------
// PATCH A1 — Capability token: compile-time broker bypass prevention
// ---------------------------------------------------------------------------

/// Unforgeable capability token required by every [`BrokerAdapter`] method.
///
/// # Contract
/// - The type is `pub` so external crates can **name** it in trait
///   implementations (`fn submit_order(&self, req: …, _token: &BrokerInvokeToken)`).
/// - The inner field is `pub(crate)`, so external crates **cannot construct**
///   a `BrokerInvokeToken`. The only valid constructor is inside
///   `mqk-execution` itself.
/// - [`crate::BrokerGateway`] is the only internal site that manufactures the
///   token, making it the **single compile-time choke-point** for all broker
///   operations.
///
/// # What external crates can and cannot do
/// ```text
/// ✅  use mqk_execution::BrokerInvokeToken;               // naming: allowed
/// ✅  fn submit_order(…, _token: &BrokerInvokeToken) {…}  // impl trait: allowed
/// ❌  BrokerInvokeToken(())                               // construction: compile error
/// ❌  broker.submit_order(req, &BrokerInvokeToken(()))    // direct call: compile error
/// ```
pub struct BrokerInvokeToken(pub(crate) ());

#[cfg(any(test, feature = "testkit"))]
impl BrokerInvokeToken {
    /// Escape hatch for adapter unit tests outside `mqk-execution`.
    ///
    /// Only available under `#[cfg(test)]` or `feature = "testkit"`.
    /// Must not appear in production code paths.
    pub fn for_test() -> Self {
        Self(())
    }
}

// ---------------------------------------------------------------------------
// BrokerAdapter trait (public — external crates implement this)
// ---------------------------------------------------------------------------

/// Trait that all broker adapters must implement.
///
/// Declared `pub` so external crates can provide implementations (paper,
/// live, mock), but routing always flows through `BrokerGateway`.
///
/// # PATCH A1 — compile-time bypass prevention
/// Every method requires `_token: &BrokerInvokeToken`. External crates can
/// implement the trait (they can name the type) but cannot call the methods
/// (they cannot construct the token). Only `BrokerGateway` creates the token.
pub trait BrokerAdapter {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerSubmitResponse>;

    fn cancel_order(
        &self,
        order_id: &str,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerCancelResponse>;

    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerReplaceResponse>;

    /// Poll the broker for new lifecycle events since `cursor`.
    ///
    /// `cursor` is the last-consumed cursor value returned by a prior call, or
    /// `None` to start from the beginning.  The adapter returns all events that
    /// follow the cursor position together with the new cursor value to pass on
    /// the next call.  Returning `None` as the new cursor means no events were
    /// produced and no cursor advancement is needed.
    ///
    /// The orchestrator persists every event to `oms_inbox` with dedup on
    /// `broker_message_id` BEFORE advancing the cursor in DB, so a crash
    /// between the two steps is safe: on restart the orchestrator re-fetches
    /// from the old cursor and the inbox dedup prevents double-apply.
    fn fetch_events(
        &self,
        cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> Result<(Vec<BrokerEvent>, Option<String>)>;
}

// ---------------------------------------------------------------------------
// OrderRouter (crate-private)
// ---------------------------------------------------------------------------

/// Crate-private router that delegates directly to a broker adapter.
///
/// Cannot be constructed or called from outside `mqk-execution`.
/// All external broker operations must go through `BrokerGateway`.
pub(crate) struct OrderRouter<B: BrokerAdapter> {
    broker: B,
}

impl<B: BrokerAdapter> OrderRouter<B> {
    pub(crate) fn new(broker: B) -> Self {
        Self { broker }
    }

    pub(crate) fn route_submit(&self, req: BrokerSubmitRequest) -> Result<BrokerSubmitResponse> {
        self.broker.submit_order(req, &BrokerInvokeToken(()))
    }

    pub(crate) fn route_cancel(&self, order_id: &str) -> Result<BrokerCancelResponse> {
        self.broker.cancel_order(order_id, &BrokerInvokeToken(()))
    }

    pub(crate) fn route_replace(&self, req: BrokerReplaceRequest) -> Result<BrokerReplaceResponse> {
        self.broker.replace_order(req, &BrokerInvokeToken(()))
    }

    pub(crate) fn route_fetch_events(
        &self,
        cursor: Option<&str>,
    ) -> Result<(Vec<BrokerEvent>, Option<String>)> {
        self.broker.fetch_events(cursor, &BrokerInvokeToken(()))
    }
}

// ---------------------------------------------------------------------------
// Internal unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MockBroker {
        submitted: RefCell<HashMap<String, String>>,
    }

    impl BrokerAdapter for MockBroker {
        fn submit_order(
            &self,
            req: BrokerSubmitRequest,
            _token: &BrokerInvokeToken,
        ) -> Result<BrokerSubmitResponse> {
            self.submitted
                .borrow_mut()
                .insert(req.order_id.clone(), req.symbol.clone());
            Ok(BrokerSubmitResponse {
                broker_order_id: format!("broker-{}", req.order_id),
                submitted_at: 1_000_000,
                status: "acknowledged".to_string(),
            })
        }

        fn cancel_order(
            &self,
            order_id: &str,
            _token: &BrokerInvokeToken,
        ) -> Result<BrokerCancelResponse> {
            Ok(BrokerCancelResponse {
                broker_order_id: format!("broker-{order_id}"),
                cancelled_at: 1_000_000,
                status: "cancelled".to_string(),
            })
        }

        fn replace_order(
            &self,
            req: BrokerReplaceRequest,
            _token: &BrokerInvokeToken,
        ) -> Result<BrokerReplaceResponse> {
            Ok(BrokerReplaceResponse {
                broker_order_id: req.broker_order_id,
                replaced_at: 1_000_000,
                status: "replaced".to_string(),
            })
        }

        fn fetch_events(
            &self,
            _cursor: Option<&str>,
            _token: &BrokerInvokeToken,
        ) -> Result<(Vec<BrokerEvent>, Option<String>)> {
            Ok((vec![], None))
        }
    }

    #[test]
    fn route_submit_delegates_to_broker() {
        let router = OrderRouter::new(MockBroker::default());
        let req = BrokerSubmitRequest {
            order_id: "ord-1".to_string(),
            symbol: "AAPL".to_string(),
            side: crate::types::Side::Buy,
            quantity: 100,
            order_type: "limit".to_string(),
            limit_price: Some(150_000_000), // $150.00 in micros
            time_in_force: "day".to_string(),
        };
        let resp = router.route_submit(req).unwrap();
        assert_eq!(resp.broker_order_id, "broker-ord-1");
        assert_eq!(resp.status, "acknowledged");
    }

    #[test]
    fn route_cancel_delegates_to_broker() {
        let router = OrderRouter::new(MockBroker::default());
        let resp = router.route_cancel("ord-1").unwrap();
        assert_eq!(resp.status, "cancelled");
    }

    #[test]
    fn route_replace_delegates_to_broker() {
        let router = OrderRouter::new(MockBroker::default());
        let req = BrokerReplaceRequest {
            broker_order_id: "broker-ord-1".to_string(),
            quantity: 200,
            limit_price: Some(151_000_000), // $151.00 in micros
            time_in_force: "gtc".to_string(),
        };
        let resp = router.route_replace(req).unwrap();
        assert_eq!(resp.status, "replaced");
    }
    #[test]
    fn fill_identity_strength_is_strong_when_broker_fill_id_present() {
        let ev = BrokerEvent::Fill {
            broker_message_id: "msg-1".to_string(),
            broker_fill_id: Some("econ-1".to_string()),
            internal_order_id: "ord-1".to_string(),
            broker_order_id: Some("brk-1".to_string()),
            symbol: "AAPL".to_string(),
            side: crate::types::Side::Buy,
            delta_qty: 1,
            price_micros: 100_000_000,
            fee_micros: 0,
        };
        assert_eq!(
            ev.fill_identity_strength(),
            Some(FillIdentityStrength::StrongBrokerNative)
        );
        assert_eq!(
            ev.identity().fill_identity_strength,
            ev.fill_identity_strength()
        );
    }

    #[test]
    fn fill_identity_strength_is_weak_without_broker_fill_id() {
        let ev = BrokerEvent::PartialFill {
            broker_message_id: "msg-2".to_string(),
            broker_fill_id: None,
            internal_order_id: "ord-1".to_string(),
            broker_order_id: Some("brk-1".to_string()),
            symbol: "AAPL".to_string(),
            side: crate::types::Side::Buy,
            delta_qty: 1,
            price_micros: 100_000_000,
            fee_micros: 0,
        };
        assert_eq!(
            ev.fill_identity_strength(),
            Some(FillIdentityStrength::WeakMessageDerived)
        );
    }
}
