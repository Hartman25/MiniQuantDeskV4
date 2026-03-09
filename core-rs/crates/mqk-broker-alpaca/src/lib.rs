#![forbid(unsafe_code)]

//! Alpaca broker adapter (stub / placeholder).
//!
//! This crate compiles and satisfies the `mqk_execution::BrokerAdapter` trait.
//! The real API wiring can come later; for now these are deterministic stubs.

use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerEvent, BrokerInvokeToken,
    BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
};

#[derive(Debug, Default)]
pub struct AlpacaBrokerAdapter;

impl BrokerAdapter for AlpacaBrokerAdapter {
    fn submit_order(
        &self,
        _req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerSubmitResponse, BrokerError> {
        Err(BrokerError::Transient {
            detail: "Alpaca broker adapter not yet implemented".to_string(),
        })
    }

    fn cancel_order(
        &self,
        _broker_order_id: &str,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerCancelResponse, BrokerError> {
        Err(BrokerError::Transient {
            detail: "Alpaca broker adapter not yet implemented".to_string(),
        })
    }

    fn replace_order(
        &self,
        _req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerReplaceResponse, BrokerError> {
        Err(BrokerError::Transient {
            detail: "Alpaca broker adapter not yet implemented".to_string(),
        })
    }

    fn fetch_events(
        &self,
        _cursor: Option<&str>,
        _token: &BrokerInvokeToken,
    ) -> Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
        Ok((Vec::new(), None))
    }
}
