//! Request and response types for all mqk-daemon HTTP endpoints.
//!
//! These types are `Serialize + Deserialize` so they can be JSON-encoded
//! by Axum and decoded by tests.  No business logic lives here.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// /v1/health
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub ok: bool,
    pub service: &'static str,
    pub version: &'static str,
}

// ---------------------------------------------------------------------------
// Gate refusal (403) — Patch L1
// ---------------------------------------------------------------------------

/// Response body when a daemon route is refused due to a gate check failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRefusedResponse {
    pub error: String,
    /// Which gate failed: "integrity_armed" | "risk_allowed" | "reconcile_clean"
    pub gate: String,
}

// ---------------------------------------------------------------------------
// /v1/integrity/arm  /v1/integrity/disarm
// ---------------------------------------------------------------------------

/// Response for integrity arm / disarm endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityResponse {
    /// true = armed (execution allowed), false = disarmed (execution blocked).
    pub armed: bool,
    /// Active run ID at the moment of the call (if any).
    pub active_run_id: Option<Uuid>,
    /// Current run-lifecycle state ("idle" | "running" | "halted").
    pub state: String,
}

// ---------------------------------------------------------------------------
// Trading read APIs — DAEMON-1
// ---------------------------------------------------------------------------

use mqk_schemas::{BrokerAccount, BrokerFill, BrokerOrder, BrokerPosition, BrokerSnapshot};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingAccountResponse {
    /// Whether the daemon currently has any broker snapshot loaded in memory.
    pub has_snapshot: bool,
    pub account: BrokerAccount,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingPositionsResponse {
    pub has_snapshot: bool,
    pub positions: Vec<BrokerPosition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingOrdersResponse {
    pub has_snapshot: bool,
    pub orders: Vec<BrokerOrder>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingFillsResponse {
    pub has_snapshot: bool,
    pub fills: Vec<BrokerFill>,
}

/// Full raw snapshot (if available). This is intentionally read-only.
/// A later patch will wire snapshot ingestion from the broker/reconciler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingSnapshotResponse {
    pub snapshot: Option<BrokerSnapshot>,
}
