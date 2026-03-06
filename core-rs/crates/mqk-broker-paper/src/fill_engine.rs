#![forbid(unsafe_code)]

//! Deterministic in-memory paper fill engine (bar-driven).
//!
//! Determinism rules:
//! - no wall clock reads
//! - no RNG
//! - stable broker_message_id generation via per-order monotonic fill_seq

use std::collections::BTreeMap;

use mqk_execution::types::Side as ExecSide;
use mqk_execution::BrokerEvent;

/// Fill pricing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillMode {
    /// Fill at bar close.
    #[default]
    Close,
    /// Fill at bar mid (hl2).
    #[allow(dead_code)]
    Mid,
}

/// Per-symbol fill configuration.
#[derive(Debug, Clone, Default)]
pub struct FillSpec {
    /// Slippage applied in integer micros:
    /// - Buy: +slippage
    /// - Sell: -slippage
    pub slippage_micros: i64,
}

/// Minimal OHLC bar snapshot used for deterministic paper fills.
#[derive(Clone, Debug)]
pub struct Bar {
    pub open_micros: i64,
    pub high_micros: i64,
    pub low_micros: i64,
    pub close_micros: i64,
}

/// In-memory per-order state used by the paper broker.
#[derive(Debug, Clone)]
pub struct PaperOrderState {
    pub internal_order_id: String,
    pub symbol: String,
    pub side: ExecSide,
    pub original_qty: i64,
    pub remaining_qty: i64,
    pub fill_seq: u64,
}

impl PaperOrderState {
    pub fn new(internal_order_id: String, symbol: String, side: ExecSide, abs_qty: i64) -> Self {
        Self {
            internal_order_id,
            symbol,
            side,
            original_qty: abs_qty,
            remaining_qty: abs_qty,
            fill_seq: 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct DeterministicFillEngine {
    mode: FillMode,
    specs: BTreeMap<String, FillSpec>,
}

impl DeterministicFillEngine {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
    pub fn set_fill_mode(&mut self, mode: FillMode) {
        self.mode = mode;
    }

    #[allow(dead_code)]
    pub fn set_fill_spec(&mut self, symbol: &str, spec: FillSpec) {
        self.specs.insert(symbol.to_string(), spec);
    }

    pub fn apply_bar_to_order(&mut self, bar: &Bar, ord: &mut PaperOrderState) -> Vec<BrokerEvent> {
        if ord.remaining_qty <= 0 {
            return Vec::new();
        }

        let spec = self.specs.get(&ord.symbol).cloned().unwrap_or_default();

        let base_px = match self.mode {
            FillMode::Close => bar.close_micros,
            FillMode::Mid => (bar.high_micros + bar.low_micros) / 2,
        };

        // Deterministic slippage: buy worse, sell worse.
        let px = match ord.side {
            ExecSide::Buy => base_px + spec.slippage_micros,
            ExecSide::Sell => base_px - spec.slippage_micros,
        };

        // Fill entire remaining qty on this bar (simple deterministic behavior).
        let fill_qty_abs = ord.remaining_qty;
        ord.remaining_qty = 0;

        // Signed delta_qty follows side.
        let delta_qty: i64 = match ord.side {
            ExecSide::Buy => fill_qty_abs,
            ExecSide::Sell => -fill_qty_abs,
        };

        ord.fill_seq = ord.fill_seq.saturating_add(1);
        let broker_message_id = format!("paper:fill:{}:{}", ord.internal_order_id, ord.fill_seq);

        vec![BrokerEvent::Fill {
            broker_message_id,
            internal_order_id: ord.internal_order_id.clone(),
            broker_order_id: Some(ord.internal_order_id.clone()),
            symbol: ord.symbol.clone(),
            side: ord.side,
            delta_qty,
            price_micros: px,
            fee_micros: 0,
        }]
    }
}
