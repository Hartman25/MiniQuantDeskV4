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

/// Broker-bound execution intent produced by the order router layer.
///
/// This is the translation of an internal `OrderIntent` into the richer
/// broker-agnostic struct required by `OrderRouter` / `BrokerAdapter`.
/// Fields use `i32` quantity (broker APIs rarely exceed i32 range) and
/// carry the broker-protocol fields (`order_type`, `time_in_force`) that
/// the pure execution engine intentionally omits.
///
/// # Patch L9 — integer micros
///
/// `limit_price` is expressed in **integer micros** (1 unit = 1_000_000 micros).
/// Use [`crate::micros_to_price`] only at the broker wire boundary when the
/// value must be serialised to `f64` for a REST API.  No `f64` appears on the
/// execution decision surface itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionIntent {
    /// Internal order identifier (UUID string).
    pub order_id: String,
    /// Instrument symbol (e.g. "AAPL").
    pub symbol: String,
    /// Signed quantity: positive = buy, negative = sell.
    pub quantity: i32,
    /// Order type string passed to broker (e.g. "market", "limit").
    pub order_type: String,
    /// Limit price in integer micros (1 unit = 1_000_000).
    /// `None` for market orders.
    pub limit_price: Option<i64>,
    /// Time-in-force string (e.g. "day", "gtc").
    pub time_in_force: String,
}
