//! mqk-integrity
//!
//! PATCH 08 – Data Integrity + Lookahead Protection
//!
//! Architectural decisions:
//! - No lookahead ever (reject incomplete bars)
//! - Fail on gap if gap_tolerance = 0
//! - Stale feed disarms system
//! - Feed disagreement policy enforced
//!
//! Pure deterministic logic. No IO, no wall-clock. Runtime provides now_tick and bar_end_ts.

mod arm_state; // Patch L7 — sticky disarm + fail-closed boot
mod engine;
mod types;

pub use arm_state::{ArmState, DisarmReason}; // Patch L7
pub use engine::{evaluate_bar, tick_feed};
pub use types::*;
