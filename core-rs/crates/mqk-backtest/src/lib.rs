//! mqk-backtest
//!
//! PATCH 11 – Backtest Engine (Event-Sourced Replay)
//!
//! Pipeline: BAR -> STRATEGY -> EXECUTION -> PORTFOLIO -> RISK
//!
//! - Deterministic replay (same bars + config => identical results)
//! - No lookahead (incomplete bars rejected)
//! - Conservative fill pricing (worst-case ambiguity: BUY@HIGH, SELL@LOW)
//! - Stress profiles (slippage basis points)
//! - Shadow mode support (strategy runs but trades not executed)
//! - Risk enforcement via mqk-risk (daily loss, drawdown, PDT, reject storm)
//! - FIFO portfolio accounting via mqk-portfolio

pub mod corporate_actions; // Patch B4
mod engine;
pub mod types;

pub use corporate_actions::{CorporateActionPolicy, ForbidEntry}; // Patch B4
pub use engine::{BacktestEngine, BacktestError};
pub use types::{BacktestBar, BacktestConfig, BacktestReport, StressProfile};
