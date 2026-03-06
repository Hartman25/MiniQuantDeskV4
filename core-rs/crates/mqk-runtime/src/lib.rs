//! mqk-runtime
//!
//! FC-1: Authoritative Runtime Boundary.
//!
//! This crate owns the single execution path from DB outbox → broker → inbox
//! → OMS state machine → portfolio state.  No other code path may submit to
//! the broker.

pub mod orchestrator;

// Patch 1: PassGate wiring must never exist in production builds.
// Only available to tests / explicit testkit builds.
#[cfg(any(test, feature = "testkit"))]
pub mod wiring_paper;
