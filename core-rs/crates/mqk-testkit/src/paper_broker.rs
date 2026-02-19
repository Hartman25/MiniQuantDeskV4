//! Deterministic paper broker for the orchestrator MVP.
//!
//! PATCH 23: Fill model is "fill at bar close" (matches the simplest
//! backtest assumption and is consistent with mqk-execution's target model).
//! No randomness, no network I/O.

use serde::{Deserialize, Serialize};

/// A broker-level order acknowledgement.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrokerAck {
    pub order_id: String,
    pub symbol: String,
    pub side: String,
    pub qty: i64,
    pub status: String,
}

/// A broker-level fill event.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrokerFill {
    pub order_id: String,
    pub fill_id: String,
    pub symbol: String,
    pub side: String,
    pub qty: i64,
    pub price_micros: i64,
    pub fee_micros: i64,
}

/// Deterministic paper broker: accepts order intents from the execution layer,
/// immediately acknowledges and fills at a deterministic price (bar close).
///
/// Maintains running counters for deterministic IDs.
pub struct PaperBroker {
    next_order_id: u64,
    next_fill_id: u64,
    acks: Vec<BrokerAck>,
    fills: Vec<BrokerFill>,
}

impl Default for PaperBroker {
    fn default() -> Self {
        Self::new()
    }
}

impl PaperBroker {
    pub fn new() -> Self {
        Self {
            next_order_id: 1,
            next_fill_id: 1,
            acks: Vec::new(),
            fills: Vec::new(),
        }
    }

    /// Submit an order intent and receive immediate deterministic ack + fill.
    ///
    /// `fill_price_micros` is provided by the orchestrator (bar close price).
    pub fn submit_order(
        &mut self,
        symbol: &str,
        side: &str,
        qty: i64,
        fill_price_micros: i64,
    ) -> (BrokerAck, BrokerFill) {
        let order_id = format!("ORD-{:06}", self.next_order_id);
        self.next_order_id += 1;

        let fill_id = format!("FILL-{:06}", self.next_fill_id);
        self.next_fill_id += 1;

        let ack = BrokerAck {
            order_id: order_id.clone(),
            symbol: symbol.to_string(),
            side: side.to_string(),
            qty,
            status: "FILLED".to_string(),
        };

        let fill = BrokerFill {
            order_id,
            fill_id,
            symbol: symbol.to_string(),
            side: side.to_string(),
            qty,
            price_micros: fill_price_micros,
            fee_micros: 0,
        };

        self.acks.push(ack.clone());
        self.fills.push(fill.clone());

        (ack, fill)
    }

    pub fn ack_count(&self) -> usize {
        self.acks.len()
    }

    pub fn fill_count(&self) -> usize {
        self.fills.len()
    }

    pub fn acks(&self) -> &[BrokerAck] {
        &self.acks
    }

    pub fn fills(&self) -> &[BrokerFill] {
        &self.fills
    }
}
