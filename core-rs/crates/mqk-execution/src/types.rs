use serde::{Deserialize, Serialize};

/// Strategy output is a set of target positions.
///
/// The engine decides on a target portfolio state; execution is computed as
/// deltas against broker positions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyOutput {
    pub targets: Vec<TargetPosition>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetPosition {
    pub symbol: String,
    /// Signed quantity. +long, -short.
    pub qty: i64,
}

impl StrategyOutput {
    #[inline]
    pub fn new(targets: Vec<TargetPosition>) -> Self {
        Self { targets }
    }
}

impl TargetPosition {
    #[inline]
    pub fn new<S: Into<String>>(symbol: S, qty: i64) -> Self {
        Self {
            symbol: symbol.into(),
            qty,
        }
    }
}

impl ExecutionDecision {
    /// Convenience accessor for tests/callers that want the order list.
    /// Returns an empty slice for Noop/HaltAndDisarm.
    #[inline]
    pub fn intents(&self) -> &[ExecutionIntent] {
        match self {
            ExecutionDecision::PlaceOrders(intents) => intents.as_slice(),
            ExecutionDecision::Noop | ExecutionDecision::HaltAndDisarm { .. } => &[],
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

/// An order intent is the minimal representation of “place an order”
/// that strategy code can generate without knowing broker specifics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderIntent {
    pub client_order_id: String,
    pub symbol: String,
    pub side: Side,
    /// Positive quantity.
    pub qty: i64,
    /// Optional limit price in integer micros.
    pub limit_price_micros: Option<i64>,
    /// Optional stop price in integer micros.
    pub stop_price_micros: Option<i64>,
    /// Time-in-force string (e.g. "day", "gtc").
    pub time_in_force: String,
}

/// Execution intent is derived from order intents and risk rules.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionIntent {
    pub client_order_id: String,
    pub symbol: String,
    pub side: Side,
    /// Positive quantity.
    pub qty: i64,
    /// Optional limit price in integer micros.
    pub limit_price_micros: Option<i64>,
    /// Optional stop price in integer micros.
    pub stop_price_micros: Option<i64>,
    /// Time-in-force string (e.g. "day", "gtc").
    pub time_in_force: String,
}

/// Execution decision describes what the engine will do for this tick.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionDecision {
    Noop,
    PlaceOrders(Vec<ExecutionIntent>),
    HaltAndDisarm { reason: String },
}

// ---------------------------------------------------------------------------
// Forward-compatible multi-asset types (V2)
// ---------------------------------------------------------------------------

use mqk_schemas::{
    AssetClass, ContractSpec, Instrument, OrderSide as CanonSide, OrderSpec,
    OrderType as CanonType, QtyMicros,
};

/// Create a canonical equity instrument (USD, no venue).
///
/// This is a bridge helper so existing symbol-based flows can produce a stable
/// `Instrument` without changing the current execution engine contract.
pub fn equity_instrument<S: Into<String>>(symbol: S) -> Instrument {
    Instrument {
        symbol: symbol.into(),
        asset_class: AssetClass::Equity,
        venue: None,
        currency: "USD".to_string(),
        contract: ContractSpec::Equity,
    }
}

/// V2 order intent keyed by `Instrument` and fractional quantity support.
///
/// Note: `qty` is always positive; `side` encodes direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderIntentV2 {
    pub instrument: Instrument,
    pub side: CanonSide,
    pub qty: QtyMicros,
}

impl OrderIntentV2 {
    pub fn new(instrument: Instrument, side: CanonSide, qty: QtyMicros) -> Self {
        debug_assert!(qty.raw() > 0, "OrderIntentV2.qty must be > 0");
        Self {
            instrument,
            side,
            qty,
        }
    }

    /// Strict equity guard: requires whole-unit quantity.
    pub fn assert_equity_whole_units(&self) {
        if self.instrument.asset_class == AssetClass::Equity {
            debug_assert!(
                self.qty.is_whole(),
                "equity qty must be whole units (multiple of 1_000_000 micros)"
            );
        }
    }
}

/// V2 execution intent aligns with `mqk-schemas::OrderSpec` for broker adapters.
///
/// This does NOT change the current execution pipeline yet; it simply provides
/// the shared contract needed to expand asset classes later without rewrites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionIntentV2 {
    pub spec: OrderSpec,
}

impl ExecutionIntentV2 {
    pub fn market(
        client_order_id: String,
        instrument: Instrument,
        side: CanonSide,
        qty: QtyMicros,
    ) -> Self {
        let spec = OrderSpec {
            client_order_id,
            instrument,
            side,
            order_type: CanonType::Market,
            qty,
            limit_price_micros: None,
            stop_price_micros: None,
            time_in_force: "DAY".to_string(),
        };
        Self { spec }
    }
}
