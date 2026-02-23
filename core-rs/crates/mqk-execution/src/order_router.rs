//! Order Router: crate-private broker delegation layer.
//!
//! This module is intentionally NOT re-exported from `lib.rs`.
//! External crates must use [`crate::BrokerGateway`], which is the only
//! public path to broker operations and enforces all gate checks.
//!
//! `OrderRouter` and its methods are `pub(crate)` — they cannot be
//! constructed or called from outside `mqk-execution`.

/// Convenience alias used throughout this module.
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

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
    }

    #[test]
    fn route_submit_delegates_to_broker() {
        let router = OrderRouter::new(MockBroker::default());
        let req = BrokerSubmitRequest {
            order_id: "ord-1".to_string(),
            symbol: "AAPL".to_string(),
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
}
