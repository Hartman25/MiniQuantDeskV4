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
// Authoritative operator control actions — DMON-06
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorActionAuditFields {
    /// Whether this action produced a durable DB write that the daemon can prove.
    pub durable_db_write: bool,
    /// Human-readable write target(s) for the durable state update.
    pub durable_targets: Vec<String>,
    /// Optional audit/event id if emitted by current architecture.
    pub audit_event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorActionResponse {
    /// Explicit action contract identifier (e.g., "control.arm").
    pub requested_action: String,
    /// Whether the daemon accepted this action request.
    pub accepted: bool,
    /// Disposition summary (e.g., "applied", "rejected", "not_authoritative").
    pub disposition: String,
    /// Resulting arming state where known by current architecture.
    pub resulting_integrity_state: Option<String>,
    /// Resulting desired armed state where known by current architecture.
    pub resulting_desired_armed: Option<bool>,
    /// Blockers that caused rejection.
    pub blockers: Vec<String>,
    /// Non-blocking warnings for operator visibility.
    pub warnings: Vec<String>,
    /// Daemon environment/profile scope if known.
    pub environment: Option<String>,
    /// Action scope (local/cluster/etc.) where known.
    pub scope: Option<String>,
    /// Auditability metadata that this daemon can currently prove.
    pub audit: OperatorActionAuditFields,
}

// ---------------------------------------------------------------------------
// Trading read APIs — DAEMON-1
// ---------------------------------------------------------------------------

use mqk_schemas::{BrokerAccount, BrokerFill, BrokerOrder, BrokerPosition, BrokerSnapshot};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingAccountResponse {
    /// Explicit snapshot truth state for operator-honest read semantics.
    ///
    /// - `no_snapshot` = no broker snapshot is loaded.
    /// - `stale_snapshot` = reconcile has flagged snapshot freshness as stale.
    /// - `current_snapshot` = daemon has a currently-usable broker snapshot.
    pub snapshot_state: String,
    pub snapshot_captured_at_utc: Option<String>,
    pub account: Option<BrokerAccount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingPositionsResponse {
    pub snapshot_state: String,
    pub snapshot_captured_at_utc: Option<String>,
    pub positions: Option<Vec<BrokerPosition>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingOrdersResponse {
    pub snapshot_state: String,
    pub snapshot_captured_at_utc: Option<String>,
    pub orders: Option<Vec<BrokerOrder>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingFillsResponse {
    pub snapshot_state: String,
    pub snapshot_captured_at_utc: Option<String>,
    pub fills: Option<Vec<BrokerFill>>,
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
    pub environment: Option<String>,
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
    pub live_routing_enabled: Option<bool>,
    pub kill_switch_active: bool,
    pub risk_halt_active: bool,
    pub integrity_halt_active: bool,
    pub daemon_reachable: bool,
    pub fault_signals: Vec<FaultSignal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultSignal {
    pub class: String,
    pub severity: String,
    pub summary: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeErrorResponse {
    pub error: String,
    pub fault_class: String,
    pub gate: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightStatusResponse {
    pub daemon_reachable: bool,
    pub db_reachable: Option<bool>,
    pub broker_config_present: Option<bool>,
    pub market_data_config_present: Option<bool>,
    pub audit_writer_ready: Option<bool>,
    pub runtime_idle: Option<bool>,
    pub strategy_disarmed: bool,
    pub execution_disarmed: bool,
    pub live_routing_disabled: bool,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionSummaryResponse {
    pub has_snapshot: bool,
    pub active_orders: usize,
    pub pending_orders: usize,
    pub dispatching_orders: usize,
    pub reject_count_today: usize,
    pub cancel_replace_count_today: Option<usize>,
    pub avg_ack_latency_ms: Option<u64>,
    pub stuck_orders: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioSummaryResponse {
    pub has_snapshot: bool,
    pub account_equity: Option<f64>,
    pub cash: Option<f64>,
    pub long_market_value: Option<f64>,
    pub short_market_value: Option<f64>,
    pub daily_pnl: Option<f64>,
    pub buying_power: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskSummaryResponse {
    pub has_snapshot: bool,
    pub gross_exposure: Option<f64>,
    pub net_exposure: Option<f64>,
    pub concentration_pct: Option<f64>,
    pub daily_pnl: Option<f64>,
    pub drawdown_pct: Option<f64>,
    pub loss_limit_utilization_pct: Option<f64>,
    pub kill_switch_active: bool,
    pub active_breaches: usize,
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
pub struct SessionStateResponse {
    pub daemon_mode: String,
    pub operator_auth_mode: String,
    pub strategy_allowed: bool,
    pub execution_allowed: bool,
    pub system_trading_window: String,
    pub market_session: String,
    pub exchange_calendar_state: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFingerprintResponse {
    pub config_hash: String,
    pub risk_policy_version: String,
    pub strategy_bundle_version: String,
    pub build_version: String,
    pub environment_profile: String,
    pub runtime_generation_id: String,
    pub last_restart_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigDiffRow {
    pub diff_id: String,
    pub changed_at: String,
    pub changed_domain: String,
    pub before_version: String,
    pub after_version: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySummaryRow {
    pub strategy_id: String,
    pub enabled: bool,
    pub armed: bool,
    pub health: String,
    pub universe: String,
    pub pending_intents: usize,
    pub open_positions: usize,
    pub today_pnl: f64,
    pub drawdown_pct: f64,
    pub regime: String,
    pub throttle_state: String,
    pub last_decision_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySuppressionRow {
    pub suppression_id: String,
    pub strategy_id: String,
    pub state: String,
    pub trigger_domain: String,
    pub trigger_reason: String,
    pub started_at: String,
    pub cleared_at: Option<String>,
    pub note: String,
}

// ---------------------------------------------------------------------------
// /api/v1/diagnostics/snapshot (B4)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsSnapshotResponse {
    pub snapshot: Option<ExecutionSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditProvenanceRef {
    pub correlation_id: Option<String>,
    pub parent_event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRegistryEntry {
    pub event_id: String,
    pub run_id: String,
    pub ts_utc: String,
    pub artifact_key: Option<String>,
    pub artifact_uri: Option<String>,
    pub artifact_kind: Option<String>,
    pub topic: String,
    pub event_type: String,
    pub provenance: AuditProvenanceRef,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRegistryResponse {
    pub entries: Vec<ArtifactRegistryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorTimelineEntry {
    pub event_id: String,
    pub run_id: String,
    pub ts_utc: String,
    pub event_type: String,
    pub action: Option<String>,
    pub state: Option<String>,
    pub source: String,
    pub provenance: AuditProvenanceRef,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorTimelineResponse {
    pub entries: Vec<OperatorTimelineEntry>,
}
