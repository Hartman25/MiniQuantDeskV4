#![forbid(unsafe_code)]

//! Alpaca broker adapter.
//!
//! # Modules
//! - `types`    — raw Alpaca v2 trade-update event shapes (serde wire types).
//! - `normalize` — converts raw Alpaca events into canonical `BrokerEvent`.
//!
//! The `AlpacaBrokerAdapter` struct satisfies `mqk_execution::BrokerAdapter`.
//! Network wiring (`reqwest` feature) is not yet implemented; the stub adapter
//! returns `BrokerError::Transient` for outbound operations while
//! `normalize_trade_update` is fully implemented and contract-tested.

pub mod normalize;
pub mod types;

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
