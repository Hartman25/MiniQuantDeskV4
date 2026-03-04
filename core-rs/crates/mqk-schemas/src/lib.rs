use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope<T> {
    pub event_id: Uuid,
    pub run_id: Uuid,
    pub engine_id: String,
    pub ts_utc: DateTime<Utc>,
    pub correlation_id: Uuid,
    pub causation_id: Option<Uuid>,
    pub topic: String,
    pub event_type: String,
    pub payload: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bar {
    pub ts_close_utc: DateTime<Utc>,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerOrder {
    pub broker_order_id: String,
    pub client_order_id: String,
    pub symbol: String,
    pub side: String,
    pub r#type: String,
    pub status: String,
    pub qty: String,
    pub limit_price: Option<String>,
    pub stop_price: Option<String>,
    pub created_at_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerFill {
    pub broker_fill_id: String,
    pub broker_order_id: String,
    pub client_order_id: String,
    pub symbol: String,
    pub side: String,
    pub qty: String,
    pub price: String,
    pub fee: String,
    pub ts_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerPosition {
    pub symbol: String,
    pub qty: String,
    pub avg_price: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerAccount {
    pub equity: String,
    pub cash: String,
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerSnapshot {
    pub captured_at_utc: DateTime<Utc>,
    pub account: BrokerAccount,
    pub orders: Vec<BrokerOrder>,
    pub fills: Vec<BrokerFill>,
    pub positions: Vec<BrokerPosition>,
}

// ---------------------------------------------------------------------------
// Multi-asset primitives (forward-compatible)
// ---------------------------------------------------------------------------

/// Supported asset classes.
///
/// This is intentionally small and stable; additional classes can be added
/// later without changing the core execution semantics.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetClass {
    Equity,
    Option,
    Future,
    Crypto,
}

/// A deterministic fixed-point quantity type at 1e-6 scale.
///
/// Motivation: equities start as integer shares, but future assets (crypto,
/// some brokers, fractional equity) require fractional quantities.
///
/// Scale: 1.0 unit = 1_000_000 `QtyMicros`.
///
/// **Equity invariant** (recommended): quantities should be multiples of
/// 1_000_000 when `asset_class == Equity`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct QtyMicros(i64);

impl QtyMicros {
    pub const ZERO: QtyMicros = QtyMicros(0);

    /// Construct from raw 1e-6 units.
    #[inline]
    pub const fn new(raw: i64) -> Self {
        QtyMicros(raw)
    }

    /// Extract the underlying raw i64.
    #[inline]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// True if this is an exact whole unit (multiple of 1_000_000).
    #[inline]
    pub const fn is_whole(self) -> bool {
        self.0 % 1_000_000 == 0
    }
}

/// A unique instrument identifier.
///
/// Today we key most things by `symbol` (equities). This type lets us expand
/// to derivatives/crypto while staying explicit about what is being traded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Instrument {
    /// Human-facing symbol (e.g. "AAPL", "SPY", "BTC/USD").
    pub symbol: String,
    /// Coarse asset class.
    pub asset_class: AssetClass,
    /// Venue/exchange identifier (optional but recommended for futures/crypto).
    pub venue: Option<String>,
    /// ISO currency code (e.g. "USD").
    pub currency: String,
    /// Contract specification for derivatives.
    pub contract: ContractSpec,
}

/// Contract details for non-spot instruments.
///
/// Equity is the default (no extra fields). Options/futures carry enough
/// metadata to uniquely identify contracts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContractSpec {
    /// Spot equities / ETFs.
    Equity,
    /// Listed equity/index options.
    Option {
        underlying: String,
        expiry_yyyymmdd: String,
        strike_micros: i64,
        right: OptionRight,
        multiplier: i32,
    },
    /// Futures.
    Future {
        root: String,
        expiry_yyyymm: String,
        multiplier: i32,
        tick_size_micros: i64,
    },
    /// Spot crypto (pair symbol is usually enough; venue matters).
    Crypto,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OptionRight {
    Call,
    Put,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderType {
    Market,
    Limit,
    Stop,
    StopLimit,
}

/// Broker-agnostic order specification.
///
/// `qty` is **always positive**; `side` determines direction.
/// Prices are integer micros (1 unit = 1_000_000), matching the execution
/// boundary invariant used elsewhere in the repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderSpec {
    pub client_order_id: String,
    pub instrument: Instrument,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub qty: QtyMicros,
    pub limit_price_micros: Option<i64>,
    pub stop_price_micros: Option<i64>,
    pub time_in_force: String,
}

/// Broker-agnostic position snapshot for an instrument.
///
/// `qty` is signed: +long, -short, 0 = flat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub instrument: Instrument,
    pub qty: i64,
    /// Average entry price in integer micros.
    pub avg_price_micros: Option<i64>,
}
