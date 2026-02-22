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
mod gate; // Patch L6 — arm/start gate + drift tick
mod types;
mod watermark; // Patch L8 — snapshot freshness + monotonicity watermark

pub mod snapshot_adapter;

pub use engine::{is_clean_reconcile, reconcile};

// Patch L6 — mandatory gate API for arm/start and periodic drift monitoring.
pub use gate::{check_arm_gate, check_start_gate, reconcile_tick, ArmStartGate, DriftAction};
pub use snapshot_adapter::{
    normalize, normalize_json, normalize_lenient, RawBrokerOrder, RawBrokerPosition,
    RawBrokerSnapshot, SnapshotAdapterError,
};
pub use types::*;
// Patch L8 — snapshot freshness + monotonicity enforcement.
pub use watermark::{SnapshotFreshness, SnapshotWatermark};
