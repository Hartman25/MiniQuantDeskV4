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

pub use engine::{evaluate, tick};
pub use types::*;
