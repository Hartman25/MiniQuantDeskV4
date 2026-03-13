//! Request and response types for all mqk-daemon HTTP endpoints.
//!
//! These types are `Serialize + Deserialize` so they can be JSON-encoded
//! by Axum and decoded by tests.  No business logic lives here.

use mqk_runtime::observability::ExecutionSnapshot;
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

// ---------------------------------------------------------------------------
// /api/v1 summary spine — GUI alignment patch
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatusResponse {
    pub environment: String,
    pub runtime_status: String,
    pub broker_status: String,
    pub db_status: String,
    pub market_data_health: String,
    pub reconcile_status: String,
    pub integrity_status: String,
    pub audit_writer_status: String,
    pub last_heartbeat: Option<String>,
    pub deadman_status: String,
    pub loop_latency_ms: Option<u64>,
    pub active_account_id: Option<String>,
    pub config_profile: Option<String>,
    pub has_warning: bool,
    pub has_critical: bool,
    pub strategy_armed: bool,
    pub execution_armed: bool,
    pub live_routing_enabled: bool,
    pub kill_switch_active: bool,
    pub risk_halt_active: bool,
    pub integrity_halt_active: bool,
    pub daemon_reachable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightStatusResponse {
    pub daemon_reachable: bool,
    pub db_reachable: bool,
    pub broker_config_present: bool,
    pub market_data_config_present: bool,
    pub audit_writer_ready: bool,
    pub runtime_idle: bool,
    pub strategy_disarmed: bool,
    pub execution_disarmed: bool,
    pub live_routing_disabled: bool,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionSummaryResponse {
    pub active_orders: usize,
    pub pending_orders: usize,
    pub dispatching_orders: usize,
    pub reject_count_today: usize,
    pub cancel_replace_count_today: usize,
    pub avg_ack_latency_ms: Option<u64>,
    pub stuck_orders: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionOrderRowResponse {
    pub internal_order_id: String,
    pub broker_order_id: Option<String>,
    pub symbol: String,
    pub strategy_id: String,
    pub side: String,
    pub order_type: String,
    pub requested_qty: f64,
    pub filled_qty: f64,
    pub current_status: String,
    pub current_stage: String,
    pub age_ms: i64,
    pub has_warning: bool,
    pub has_critical: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioSummaryResponse {
    pub account_equity: f64,
    pub cash: f64,
    pub long_market_value: f64,
    pub short_market_value: f64,
    pub daily_pnl: f64,
    pub buying_power: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskSummaryResponse {
    pub gross_exposure: f64,
    pub net_exposure: f64,
    pub concentration_pct: f64,
    pub daily_pnl: f64,
    pub drawdown_pct: f64,
    pub loss_limit_utilization_pct: f64,
    pub kill_switch_active: bool,
    pub active_breaches: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskDenialRowResponse {
    pub id: String,
    pub at: String,
    pub strategy_id: String,
    pub symbol: String,
    pub rule: String,
    pub message: String,
    pub severity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconcileSummaryResponse {
    pub status: String,
    pub last_run_at: Option<String>,
    pub snapshot_watermark_ms: Option<i64>,
    pub mismatched_positions: usize,
    pub mismatched_orders: usize,
    pub mismatched_fills: usize,
    pub unmatched_broker_events: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconcileMismatchRowResponse {
    pub id: String,
    pub domain: String,
    pub symbol: String,
    pub internal_value: String,
    pub broker_value: String,
    pub status: String,
    pub note: String,
}

// ---------------------------------------------------------------------------
// /api/v1/diagnostics/snapshot (B4)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsSnapshotResponse {
    pub snapshot: Option<ExecutionSnapshot>,
}
