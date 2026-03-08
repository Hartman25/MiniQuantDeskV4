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

pub mod engines;
pub mod plugin_registry;

pub use host::*;
pub use plugin_registry::{PluginRegistry, RegistryError, StrategyFactory, StrategyMeta};
pub use types::*;

// Re-export execution-facing output types so engine modules and downstream
// callers can use mqk-strategy as the main strategy-layer boundary.
pub use mqk_execution::{StrategyOutput, TargetPosition};
