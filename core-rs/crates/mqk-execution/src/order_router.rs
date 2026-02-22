//! Order Router: Deterministic execution boundary between internal engine and broker adapters.
//!
//! # Purpose
//! This module defines the thin, immutable boundary through which all order execution
//! requests must pass. It isolates the core execution engine from broker-specific
//! implementations, ensuring that routing logic remains deterministic and free of
//! strategy, risk, or accounting concerns.
//!
//! # Why This Boundary Exists
//! - Enforces separation of concerns between order generation (strategy/risk) and order delivery (broker)
//! - Provides a single choke-point for logging, metrics, and pre-flight validation
//! - Enables pluggable broker adapters (paper, Alpaca, etc.) without core engine changes
//!
//! # Why It Must Remain Thin
//! - Preserves deterministic behavior required for backtesting and simulation
//! - Avoids embedding business logic that belongs in risk or strategy modules
//! - Keeps the routing layer verifiable and low-risk
//!
//! # Enabling Paper Broker and Future Adapters
//! The `BrokerAdapter` trait allows any broker implementation (paper, Alpaca, etc.)
//! to be injected into the `OrderRouter`. The router itself remains agnostic to
//! the underlying broker, simply translating internal `ExecutionIntent` types into
//! broker-agnostic request structs. This design allows:
//! - Paper trading via a `PaperBroker` that simulates fills
//! - Live Alpaca integration via an `AlpacaBroker` that makes HTTP calls
//! - Testing via `MockBroker` as shown in the unit tests

use crate::types::ExecutionIntent;

/// Convenience alias so all public items in this module can use `Result<T>`
/// without spelling out the error type everywhere.
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Broker-agnostic order submission request
#[derive(Debug, Clone)]
pub struct BrokerSubmitRequest {
    /// Internal order identifier
    pub order_id: String,
    /// Instrument identifier (symbol)
    pub symbol: String,
    /// Order quantity (positive for buy, negative for sell)
    pub quantity: i32,
    /// Order type (market, limit, etc.) - simplified for boundary
    pub order_type: String,
    /// Limit price (if applicable)
    pub limit_price: Option<f64>,
    /// Time in force
    pub time_in_force: String,
}

/// Broker-agnostic order submission response
#[derive(Debug, Clone)]
pub struct BrokerSubmitResponse {
    /// Broker-assigned order identifier
    pub broker_order_id: String,
    /// Timestamp of submission acknowledgment
    pub submitted_at: u64,
    /// Status of the submission
    pub status: String,
}

/// Broker-agnostic order cancellation response
#[derive(Debug, Clone)]
pub struct BrokerCancelResponse {
    /// Broker-assigned order identifier
    pub broker_order_id: String,
    /// Timestamp of cancellation acknowledgment
    pub cancelled_at: u64,
    /// Status of the cancellation
    pub status: String,
}

/// Broker-agnostic order replacement request
#[derive(Debug, Clone)]
pub struct BrokerReplaceRequest {
    /// Existing broker-assigned order identifier
    pub broker_order_id: String,
    /// New quantity (positive for buy, negative for sell)
    pub quantity: i32,
    /// New limit price (if applicable)
    pub limit_price: Option<f64>,
    /// New time in force
    pub time_in_force: String,
}

/// Broker-agnostic order replacement response
#[derive(Debug, Clone)]
pub struct BrokerReplaceResponse {
    /// Broker-assigned order identifier (may be new if replaced)
    pub broker_order_id: String,
    /// Timestamp of replacement acknowledgment
    pub replaced_at: u64,
    /// Status of the replacement
    pub status: String,
}

/// Trait that all broker adapters must implement
///
/// This trait defines the minimal interface required for order routing.
/// Implementations handle the actual communication with broker systems
/// (REST APIs, FIX connections, etc.) while remaining opaque to the router.
pub trait BrokerAdapter {
    /// Submit a new order to the broker
    fn submit_order(&self, req: BrokerSubmitRequest) -> Result<BrokerSubmitResponse>;

    /// Cancel an existing order
    fn cancel_order(&self, order_id: &str) -> Result<BrokerCancelResponse>;

    /// Replace/modify an existing order
    fn replace_order(&self, req: BrokerReplaceRequest) -> Result<BrokerReplaceResponse>;
}

/// Deterministic order router that delegates to a broker adapter
///
/// This struct serves as the immutable boundary layer between internal
/// execution intents and external broker systems. It performs minimal,
/// deterministic transformations and delegates all broker-specific
/// communication to the injected `BrokerAdapter`.
pub struct OrderRouter<B: BrokerAdapter> {
    broker: B,
}

impl<B: BrokerAdapter> OrderRouter<B> {
    /// Create a new order router with the given broker adapter
    pub fn new(broker: B) -> Self {
        Self { broker }
    }

    /// Route an execution intent as a new order submission
    ///
    /// # Arguments
    /// * `intent` - Internal execution intent from the execution engine
    ///
    /// # Returns
    /// * `Ok(BrokerSubmitResponse)` - Broker acknowledgment
    /// * `Err` - Routing or broker error
    pub fn route_submit(&self, intent: ExecutionIntent) -> Result<BrokerSubmitResponse> {
        // Convert internal intent to broker-agnostic request
        let req = BrokerSubmitRequest {
            order_id: intent.order_id,
            symbol: intent.symbol,
            quantity: intent.quantity,
            order_type: intent.order_type,
            limit_price: intent.limit_price,
            time_in_force: intent.time_in_force,
        };

        // Delegate to broker adapter
        self.broker.submit_order(req)
    }

    /// Route an order cancellation request
    ///
    /// # Arguments
    /// * `order_id` - Internal order identifier to cancel
    ///
    /// # Returns
    /// * `Ok(BrokerCancelResponse)` - Broker acknowledgment
    /// * `Err` - Routing or broker error
    pub fn route_cancel(&self, order_id: &str) -> Result<BrokerCancelResponse> {
        self.broker.cancel_order(order_id)
    }

    /// Route an execution intent as an order replacement
    ///
    /// # Arguments
    /// * `intent` - Internal execution intent containing updated parameters
    ///
    /// # Returns
    /// * `Ok(BrokerReplaceResponse)` - Broker acknowledgment
    /// * `Err` - Routing or broker error
    pub fn route_replace(&self, intent: ExecutionIntent) -> Result<BrokerReplaceResponse> {
        // Convert internal intent to broker-agnostic replace request
        let req = BrokerReplaceRequest {
            broker_order_id: intent.order_id, // Note: This assumes internal ID matches broker ID
            quantity: intent.quantity,
            limit_price: intent.limit_price,
            time_in_force: intent.time_in_force,
        };

        self.broker.replace_order(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// Mock broker for testing the order router
    ///
    /// This implementation records submitted orders for verification
    /// and returns deterministic responses.
    #[derive(Default)]
    struct MockBroker {
        submitted_orders: RefCell<HashMap<String, BrokerSubmitRequest>>,
    }

    impl BrokerAdapter for MockBroker {
        fn submit_order(&self, req: BrokerSubmitRequest) -> Result<BrokerSubmitResponse> {
            // Record the submitted order
            self.submitted_orders
                .borrow_mut()
                .insert(req.order_id.clone(), req.clone());

            // Return deterministic response
            Ok(BrokerSubmitResponse {
                broker_order_id: format!("broker-{}", req.order_id),
                submitted_at: 1234567890,
                status: "acknowledged".to_string(),
            })
        }

        fn cancel_order(&self, order_id: &str) -> Result<BrokerCancelResponse> {
            Ok(BrokerCancelResponse {
                broker_order_id: format!("broker-{}", order_id),
                cancelled_at: 1234567890,
                status: "cancelled".to_string(),
            })
        }

        fn replace_order(&self, req: BrokerReplaceRequest) -> Result<BrokerReplaceResponse> {
            Ok(BrokerReplaceResponse {
                broker_order_id: req.broker_order_id,
                replaced_at: 1234567890,
                status: "replaced".to_string(),
            })
        }
    }

    #[test]
    fn test_route_submit_delegates_to_broker() {
        // Arrange
        let mock_broker = MockBroker::default();
        let router = OrderRouter::new(mock_broker);
        let intent = ExecutionIntent {
            order_id: "test-123".to_string(),
            symbol: "AAPL".to_string(),
            quantity: 100,
            order_type: "limit".to_string(),
            limit_price: Some(150.0),
            time_in_force: "day".to_string(),
        };

        // Act
        let response = router.route_submit(intent.clone()).unwrap();

        // Assert
        assert_eq!(response.broker_order_id, "broker-test-123");
        assert_eq!(response.status, "acknowledged");

        // Verify the broker received the request
        let submitted = router
            .broker
            .submitted_orders
            .borrow()
            .get("test-123")
            .cloned()
            .unwrap();
        assert_eq!(submitted.symbol, "AAPL");
        assert_eq!(submitted.quantity, 100);
        assert_eq!(submitted.limit_price, Some(150.0));
    }

    #[test]
    fn test_route_cancel_delegates_to_broker() {
        // Arrange
        let mock_broker = MockBroker::default();
        let router = OrderRouter::new(mock_broker);

        // Act
        let response = router.route_cancel("test-123").unwrap();

        // Assert
        assert_eq!(response.broker_order_id, "broker-test-123");
        assert_eq!(response.status, "cancelled");
    }

    #[test]
    fn test_route_replace_delegates_to_broker() {
        // Arrange
        let mock_broker = MockBroker::default();
        let router = OrderRouter::new(mock_broker);
        let intent = ExecutionIntent {
            order_id: "test-123".to_string(),
            symbol: "AAPL".to_string(),
            quantity: 200,
            order_type: "limit".to_string(),
            limit_price: Some(151.0),
            time_in_force: "gtc".to_string(),
        };

        // Act
        let response = router.route_replace(intent).unwrap();

        // Assert
        assert_eq!(response.broker_order_id, "test-123");
        assert_eq!(response.status, "replaced");
    }
}
