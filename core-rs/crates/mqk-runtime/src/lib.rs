//! mqk-runtime
//!
//! FC-1: Authoritative Runtime Boundary.
//!
//! This crate owns the single execution path from DB outbox → broker → inbox
//! → OMS state machine → portfolio state.  No other code path may submit to
//! the broker.

pub mod orchestrator;

pub mod wiring_paper;
