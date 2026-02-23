//! mqk-risk
//!
//! PATCH 07 – Risk Engine Enforcement
//!
//! Goals:
//! - Daily loss limit enforcement
//! - Max drawdown guard
//! - Reject storm protection
//! - PDT auto mode enforcement
//! - Kill switch behavior
//!
//! Deterministic, pure logic. No IO, no time, no broker calls.

mod engine;
mod types;

pub mod pdt;

pub use engine::{evaluate, tick, validate_equity_input, validate_order_qty}; // Patch L10
pub use pdt::{
    clear_pdt_flag, evaluate_pdt, record_day_trade, tick_pdt, to_pdt_context, PdtDecision,
    PdtInput, PdtPolicy, PdtReason, PdtState, PDT_DAY_TRADE_THRESHOLD, PDT_DEFAULT_WINDOW_DAYS,
    PDT_MIN_EQUITY_MICROS,
};
pub use types::*;
