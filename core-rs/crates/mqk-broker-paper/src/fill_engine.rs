#![forbid(unsafe_code)]

use mqk_execution::types::Side;
use mqk_execution::BrokerEvent;

/// Minimal OHLC bar snapshot used for deterministic paper fills.
#[derive(Clone, Debug)]
pub struct Bar {
    pub symbol: String,
    pub end_ts_utc_rfc3339: String,
    pub open_micros: i64,
    pub high_micros: i64,
    pub low_micros: i64,
    pub close_micros: i64,
}

#[derive(Clone, Copy, Debug)]
pub enum FillMode {
    BarClose,
    BarOpen,
}

#[derive(Clone, Copy, Debug)]
pub struct FillSpec {
    pub mode: FillMode,
    /// Flat fee per fill (micros). Keep deterministic.
    pub fee_micros: i64,
}

impl Default for FillSpec {
    fn default() -> Self {
        Self {
            mode: FillMode::BarClose,
            fee_micros: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PaperOrderState {
    pub internal_order_id: String,
    pub symbol: String,
    pub side: Side,
    pub remaining_qty: i64,
    pub fill_seq: u64,
}

impl PaperOrderState {
    pub fn new(internal_order_id: String, symbol: String, side: Side, qty: i64) -> Self {
        Self {
            internal_order_id,
            symbol,
            side,
            remaining_qty: qty,
            fill_seq: 0,
        }
    }
}

/// Deterministic fill engine.
///
/// Current semantics (simple on purpose):
/// - Treat every order as a market order.
/// - Fill entire remaining quantity at bar close (default) or bar open.
/// - Emit a single `BrokerEvent::Fill` with a stable `broker_message_id`.
#[derive(Clone, Copy, Debug)]
pub struct DeterministicFillEngine {
    pub spec: FillSpec,
}

impl DeterministicFillEngine {
    pub fn new(spec: FillSpec) -> Self {
        Self { spec }
    }

    pub fn price_for_bar(&self, bar: &Bar) -> i64 {
        match self.spec.mode {
            FillMode::BarClose => bar.close_micros,
            FillMode::BarOpen => bar.open_micros,
        }
    }

    pub fn apply_bar_to_order(&self, bar: &Bar, ord: &mut PaperOrderState) -> Vec<BrokerEvent> {
        if ord.symbol != bar.symbol || ord.remaining_qty <= 0 {
            return vec![];
        }

        let px = self.price_for_bar(bar);
        let qty = ord.remaining_qty;

        ord.remaining_qty = 0;
        ord.fill_seq = ord.fill_seq.saturating_add(1);

        let broker_message_id = format!("paper:fill:{}:{}", ord.internal_order_id, ord.fill_seq);

        vec![BrokerEvent::Fill {
            broker_message_id,
            internal_order_id: ord.internal_order_id.clone(),
            broker_order_id: None,
            symbol: ord.symbol.clone(),
            side: ord.side,
            delta_qty: qty,
            price_micros: px,
            fee_micros: self.spec.fee_micros,
        }]
    }
}
