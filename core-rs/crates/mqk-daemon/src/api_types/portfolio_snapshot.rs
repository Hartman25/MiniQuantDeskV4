//! Canonical broker-snapshot portfolio surfaces.
//!
//! Extracted from `api_types.rs` (MT-07B).
//! Routes: `/api/v1/portfolio/positions`, `/api/v1/portfolio/orders/open`,
//!         `/api/v1/portfolio/fills`

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// /api/v1/portfolio/positions  /api/v1/portfolio/orders/open  /api/v1/portfolio/fills
// Canonical broker-snapshot portfolio surfaces (Cluster 2)
// ---------------------------------------------------------------------------

/// One broker-layer position row.
///
/// Fields with no broker-snapshot equivalent are emitted as `null`:
/// - `strategy_id`: `null` — positions are not attributed to a strategy at
///   the broker snapshot level.
/// - `mark_price`, `unrealized_pnl`, `realized_pnl_today`: `null` — mark-to-
///   market data is not present in the broker snapshot.
/// - `drift`: `null` — reconcile-level position drift is not assessed at the
///   broker snapshot layer.
/// - `broker_qty`: same as `qty` — the row IS the broker view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioPositionRow {
    pub symbol: String,
    /// `null` — broker-snapshot positions have no strategy attribution.
    pub strategy_id: Option<String>,
    pub qty: i64,
    pub avg_price: f64,
    /// `null` — mark prices are not present in the broker snapshot.
    pub mark_price: Option<f64>,
    /// `null` — broker snapshot has no unrealized PnL.
    pub unrealized_pnl: Option<f64>,
    /// `null` — broker snapshot has no today-only realized PnL.
    pub realized_pnl_today: Option<f64>,
    /// Equals `qty` — this row is sourced from the broker view.
    pub broker_qty: i64,
    /// `null` — reconcile-level drift is not assessed at broker snapshot layer.
    pub drift: Option<bool>,
}

/// Response wrapper for `/api/v1/portfolio/positions`.
///
/// `snapshot_state`:
/// - `"active"` — broker snapshot is present; `rows` is authoritative (may be
///   empty when the account holds no positions).
/// - `"no_snapshot"` — no broker snapshot is loaded; `rows` is always empty
///   and must NOT be treated as authoritative zero.
///
/// PORT-05: `snapshot_source` and `session_boundary` make restart-aware
/// supervision explicit for operators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioPositionsResponse {
    pub snapshot_state: String,
    pub captured_at_utc: Option<String>,
    pub rows: Vec<PortfolioPositionRow>,
    /// PORT-05: How this snapshot was produced.
    /// - `"synthetic"` — paper mode; derived from local OMS + portfolio engine.
    /// - `"external"` — Alpaca REST fetch (external broker).
    /// - `null` when `snapshot_state = "no_snapshot"`.
    pub snapshot_source: Option<String>,
    /// PORT-05: Persistence boundary for this surface.
    ///
    /// Always `"in_memory_only"`: the broker snapshot is held in-memory and is
    /// reset on every daemon restart.  After a restart this surface returns
    /// `"no_snapshot"` until a fresh snapshot is loaded.  No durable history
    /// of positions is maintained by the daemon.
    pub session_boundary: String,
}

/// One broker-layer open-order row.
///
/// - `strategy_id`: `null` — broker snapshot has no strategy attribution.
/// - `filled_qty`: `null` — broker snapshot does not track partial fills per order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioOpenOrderRow {
    /// Client order ID assigned by this daemon (= `client_order_id` in broker snapshot).
    pub internal_order_id: String,
    pub symbol: String,
    /// `null` — open orders are not strategy-attributed at the broker snapshot layer.
    pub strategy_id: Option<String>,
    pub side: String,
    pub status: String,
    pub requested_qty: i64,
    /// `null` — partial fill quantity is not tracked in the broker snapshot.
    pub filled_qty: Option<i64>,
    pub entered_at: String,
}

/// Response wrapper for `/api/v1/portfolio/orders/open`.
///
/// PORT-05: `snapshot_source` and `session_boundary` make restart-aware
/// supervision explicit for operators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioOpenOrdersResponse {
    pub snapshot_state: String,
    pub captured_at_utc: Option<String>,
    pub rows: Vec<PortfolioOpenOrderRow>,
    /// PORT-05: How this snapshot was produced. `null` when no snapshot.
    /// `"synthetic"` (paper/local OMS) or `"external"` (Alpaca REST).
    pub snapshot_source: Option<String>,
    /// PORT-05: Always `"in_memory_only"` — lost on daemon restart.
    pub session_boundary: String,
}

/// One broker-layer fill row.
///
/// - `strategy_id`: `null` — broker snapshot has no strategy attribution.
/// - `applied`: `true` — fills present in the broker snapshot are by definition
///   already applied.
/// - `broker_exec_id`: equals `fill_id` (= `broker_fill_id` from broker API).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioFillRow {
    pub fill_id: String,
    /// Client order ID of the order that generated this fill.
    pub internal_order_id: String,
    pub symbol: String,
    /// `null` — fills are not strategy-attributed at the broker snapshot layer.
    pub strategy_id: Option<String>,
    pub side: String,
    pub qty: i64,
    pub price: f64,
    /// Equals `fill_id` — uses broker fill ID as the execution ID.
    pub broker_exec_id: String,
    /// `true` — fills in the snapshot are already applied.
    pub applied: bool,
    pub at: String,
}

/// Response wrapper for `/api/v1/portfolio/fills`.
///
/// PORT-05: `snapshot_source` and `session_boundary` make restart-aware
/// supervision explicit for operators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioFillsResponse {
    pub snapshot_state: String,
    pub captured_at_utc: Option<String>,
    pub rows: Vec<PortfolioFillRow>,
    /// PORT-05: How this snapshot was produced. `null` when no snapshot.
    /// `"synthetic"` (paper/local OMS) or `"external"` (Alpaca REST).
    pub snapshot_source: Option<String>,
    /// PORT-05: Always `"in_memory_only"` — lost on daemon restart.
    pub session_boundary: String,
}
