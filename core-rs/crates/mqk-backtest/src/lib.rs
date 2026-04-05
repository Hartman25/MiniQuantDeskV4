//! mqk-backtest — Deterministic Backtest Engine (Event-Sourced Replay)
//!
//! Pipeline: BAR -> STRATEGY -> EXECUTION -> PORTFOLIO -> RISK
//!
//! - Deterministic replay (same bars + config + input hash => identical results)
//! - Input-data provenance: bar sequence hashed into run identity (BKT-PROV-01)
//! - No lookahead (incomplete bars rejected)
//! - Conservative fill pricing (worst-case ambiguity: BUY@HIGH, SELL@LOW)
//! - Stress profiles: flat slippage + volatility-spread component (Patch B5)
//! - Commission/fee modeling per fill (BKT-03P)
//! - Corporate action exclusion policy (Patch B4)
//! - Integrity gate: stale/gap/disagreement detection (PATCH 22 / B3)
//! - Shadow mode support (strategy runs but trades not executed)
//! - Risk enforcement via mqk-risk (daily loss, drawdown, PDT, reject storm)
//! - FIFO portfolio accounting via mqk-portfolio

pub mod corporate_actions; // Patch B4
mod engine;
pub mod loader;
pub mod types;

pub use corporate_actions::{CorporateActionPolicy, ForbidEntry}; // Patch B4
pub use engine::{BacktestEngine, BacktestError};
pub use loader::{load_csv_file, parse_csv_bars, LoadError};
pub use types::{
    derive_input_data_hash, derive_run_id, BacktestBar, BacktestConfig, BacktestFill,
    BacktestOrder, BacktestOrderSide, BacktestReport, CommissionModel, OrderStatus, StressProfile,
};
