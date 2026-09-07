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
//!
//! ## Authority boundary
//!
//! This crate owns account/order-level risk evaluation: kill-switch, PDT,
//! daily-loss-limit, and max-drawdown gates (`engine::evaluate`). Per-symbol
//! position-quantity caps and order-rate/day caps are enforced separately in
//! `mqk-daemon`'s runtime dispatch (`state/loop_runner.rs`), not here — that
//! split is intentional, not a gap: those caps are daemon-owned runtime
//! configuration (`MQK_PER_SYMBOL_MAX_POSITION_QTY` and related env-derived
//! limits) applied to per-tick target sizing, ahead of and independent of
//! this crate's account-level gate.

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
