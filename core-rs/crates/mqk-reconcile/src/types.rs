use std::collections::{BTreeMap, BTreeSet};

/// Micros scale (1e-6) used for prices and currency where needed.
pub const MICROS_SCALE: i64 = 1_000_000;

/// Minimal order status model for reconciliation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OrderStatus {
    New,
    Accepted,
    PartiallyFilled,
    Filled,
    Canceled,
    Rejected,
    Unknown,
}

/// Side is needed to compare intent in minimal form.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    Buy,
    Sell,
}

/// Order snapshot shape from either local engine or broker snapshot.
/// PATCH 09 keeps it minimal: only fields that can cause drift.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct OrderSnapshot {
    pub order_id: String,
    pub symbol: String,
    pub side: Side,
    pub qty: i64,
    pub filled_qty: i64,
    pub status: OrderStatus,
}

impl OrderSnapshot {
    pub fn new(
        order_id: impl Into<String>,
        symbol: impl Into<String>,
        side: Side,
        qty: i64,
        filled_qty: i64,
        status: OrderStatus,
    ) -> Self {
        Self {
            order_id: order_id.into(),
            symbol: symbol.into(),
            side,
            qty,
            filled_qty,
            status,
        }
    }
}

/// Position snapshot: signed quantity only (tight).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PositionSnapshot {
    pub symbol: String,
    pub qty_signed: i64,
}

impl PositionSnapshot {
    pub fn new(symbol: impl Into<String>, qty_signed: i64) -> Self {
        Self {
            symbol: symbol.into(),
            qty_signed,
        }
    }
}

/// Local state we believe to be true.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSnapshot {
    /// All locally-known open/working orders (or recently completed if you decide to include them).
    /// Keyed by order_id.
    pub orders: BTreeMap<String, OrderSnapshot>,

    /// Positions we believe we hold (symbol -> qty_signed).
    pub positions: BTreeMap<String, i64>,
}

impl LocalSnapshot {
    pub fn empty() -> Self {
        Self {
            orders: BTreeMap::new(),
            positions: BTreeMap::new(),
        }
    }

    pub fn known_order_ids(&self) -> BTreeSet<String> {
        self.orders.keys().cloned().collect()
    }
}

/// Broker snapshot as observed from broker API (outside this crate).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerSnapshot {
    /// Orders currently visible at broker (open/working and/or recent).
    pub orders: BTreeMap<String, OrderSnapshot>,

    /// Positions visible at broker (symbol -> qty_signed).
    pub positions: BTreeMap<String, i64>,
}

impl BrokerSnapshot {
    pub fn empty() -> Self {
        Self {
            orders: BTreeMap::new(),
            positions: BTreeMap::new(),
        }
    }
}

/// What the engine tells the runtime to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileAction {
    Clean,
    Halt,
}

/// Why we halted / what we observed. Stable ordering enforced by engine.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReconcileReason {
    UnknownBrokerOrder,
    PositionMismatch,
    OrderDrift,
}

/// Evidence of a mismatch (kept minimal but explicit).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReconcileDiff {
    UnknownOrder { order_id: String },

    PositionQtyMismatch {
        symbol: String,
        local_qty: i64,
        broker_qty: i64,
    },

    OrderMismatch {
        order_id: String,
        field: String,
        local: String,
        broker: String,
    },
}

/// Full report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconcileReport {
    pub action: ReconcileAction,
    pub reasons: Vec<ReconcileReason>,
    pub diffs: Vec<ReconcileDiff>,
}

impl ReconcileReport {
    pub fn clean() -> Self {
        Self {
            action: ReconcileAction::Clean,
            reasons: Vec::new(),
            diffs: Vec::new(),
        }
    }

    pub fn is_clean(&self) -> bool {
        self.action == ReconcileAction::Clean
    }
}
