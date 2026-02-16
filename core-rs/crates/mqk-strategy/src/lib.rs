//! mqk-strategy
//!
//! PATCH 10 – Strategy Plugin Framework (Tier A)
//!
//! Contract (doc-aligned):
//! - Strategies output TARGET POSITIONS; core converts to orders.
//! - Strategy hook: on_bar -> StrategyOutput (target positions)
//! - Context provides bounded recent bars window; no DB/broker access.
//! - Shadow mode: strategy runs but cannot trade; emits SHADOW intents.
//! - Determinism required (event stream + config + seed). (Seed/stream wired later; host is deterministic.)

mod host;
mod types;

pub use host::*;
pub use types::*;
