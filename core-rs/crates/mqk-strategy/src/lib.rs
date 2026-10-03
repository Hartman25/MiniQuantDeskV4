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
pub mod semantic_identity;
pub mod sized_strategy;
pub mod sizing;
pub mod sizing_state;

pub use host::*;
pub use plugin_registry::{
    PluginRegistry, RegistryError, RestartRecovery, StrategyDataRequirements, StrategyFactory,
    StrategyMeta,
};
pub use semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
pub use sized_strategy::{
    capital_fraction_semantic_fingerprint, CapitalFractionSizedStrategy, SizedEntryRecord,
    SizingAudit, SizingAuditHandle, SizingRefusalRecord, SizingStateHandle,
};
pub use sizing_state::{
    HeldSizingContract, HeldSizingRecord, HeldSizingRecoveryError, HeldSizingScope,
    HeldSizingStatus, HeldSizingTransition, HELD_SIZING_STATE_VERSION,
};
pub use sizing::{
    parse_positive_qty, resolve_capital_fraction_target, CapitalFractionRefusal,
    CapitalFractionResolution, SizingError, SizingPolicy, TargetSizing,
    ALLOCATION_FRACTION_BPS_DENOMINATOR, SIZING_POLICY_FIXED_INITIAL_CAPITAL_FRACTION_V1,
    SIZING_POLICY_FIXED_QUANTITY_V1,
};
pub use types::*;

// Re-export execution-facing output types so engine modules and downstream
// callers can use mqk-strategy as the main strategy-layer boundary.
pub use mqk_execution::{StrategyOutput, TargetPosition};

// STRATEGY-DECISION-OBSERVABILITY-01: re-export diagnostics so daemon can store
// and surface them without a direct dependency on the engine sub-module.
pub use engines::{intraday_scalper_compute_diagnostics, IntradayScalperDiagnostics};
