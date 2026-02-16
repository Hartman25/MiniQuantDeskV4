//! mqk-reconcile
//!
//! PATCH 09 – Reconciliation Engine
//!
//! Architectural decisions:
//! - Broker snapshot reconciliation required before LIVE
//! - Divergence triggers HALT
//! - Unknown broker order triggers HALT
//! - Position mismatch triggers HALT
//! - Clean reconcile required before arming
//!
//! Deterministic, pure logic. No IO. No broker calls.

mod engine;
mod types;

pub use engine::{is_clean_reconcile, reconcile};
pub use types::*;
