use std::fmt;

/// A target position for a single symbol.
/// Signed quantity: +long, -short, 0 = flat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetPosition {
    pub symbol: String,
    pub target_qty: i64,
}

impl TargetPosition {
    pub fn new<S: Into<String>>(symbol: S, target_qty: i64) -> Self {
        Self {
            symbol: symbol.into(),
            target_qty,
        }
    }
}

/// Strategy output contract for PATCH 05.
/// Target-position model: strategy does NOT submit orders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrategyOutput {
    pub targets: Vec<TargetPosition>,
}

impl StrategyOutput {
    pub fn new(targets: Vec<TargetPosition>) -> Self {
        Self { targets }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::Buy => write!(f, "BUY"),
            Side::Sell => write!(f, "SELL"),
        }
    }
}

/// Minimal order intent (no broker fields).
/// Quantity is always positive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderIntent {
    pub symbol: String,
    pub side: Side,
    pub qty: i64,
}

impl OrderIntent {
    pub fn new<S: Into<String>>(symbol: S, side: Side, qty: i64) -> Self {
        debug_assert!(qty > 0, "OrderIntent.qty must be > 0");
        Self {
            symbol: symbol.into(),
            side,
            qty,
        }
    }
}

/// Engine decision for a single evaluation tick.
/// No side effects; caller is responsible for persistence/broker wiring later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionDecision {
    pub intents: Vec<OrderIntent>,
}

impl ExecutionDecision {
    pub fn empty() -> Self {
        Self { intents: vec![] }
    }
}
