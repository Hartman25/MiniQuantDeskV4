#![forbid(unsafe_code)]

//! Alpaca broker adapter (stub / placeholder).
//!
//! This crate compiles and satisfies the `mqk_execution::BrokerAdapter` trait.
//! The real API wiring can come later; for now these are deterministic stubs.

use mqk_execution::{
    BrokerAdapter, BrokerCancelResponse, BrokerEvent, BrokerInvokeToken, BrokerReplaceRequest,
    BrokerReplaceResponse, BrokerSubmitRequest, BrokerSubmitResponse,
};

type BoxError = Box<dyn std::error::Error>;

#[derive(Debug, Default)]
pub struct AlpacaBrokerAdapter;

impl BrokerAdapter for AlpacaBrokerAdapter {
    fn submit_order(
        &self,
        _req: BrokerSubmitRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerSubmitResponse, BoxError> {
        Err("Alpaca broker adapter not implemented".into())
    }

    fn cancel_order(
        &self,
        _broker_order_id: &str,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerCancelResponse, BoxError> {
        Err("Alpaca broker adapter not implemented".into())
    }

    fn replace_order(
        &self,
        _req: BrokerReplaceRequest,
        _token: &BrokerInvokeToken,
    ) -> Result<BrokerReplaceResponse, BoxError> {
        Err("Alpaca broker adapter not implemented".into())
    }

    fn fetch_events(&self, _token: &BrokerInvokeToken) -> Result<Vec<BrokerEvent>, BoxError> {
        Ok(Vec::new())
    }
}
