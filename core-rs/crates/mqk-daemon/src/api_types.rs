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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorActionAuditRow {
    pub audit_event_id: String,
    pub ts_utc: String,
    pub requested_action: String,
    pub disposition: String,
    pub run_id: Option<String>,
    pub runtime_transition: Option<String>,
    pub provenance_ref: String,
}

/// Response wrapper for `/api/v1/audit/operator-actions`.
///
/// `truth_state`:
/// - `"active"` — durable operator-action history was queried from Postgres;
///   `backend` names the exact source table and `rows` is authoritative.
/// - `"backend_unavailable"` — no DB pool is configured, so durable history
///   could not be queried; `backend` is `"unavailable"` and empty `rows`
///   MUST NOT be treated as authoritative zero.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorActionsAuditResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub backend: String,
    pub rows: Vec<OperatorActionAuditRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditArtifactRow {
    pub artifact_id: String,
    pub artifact_type: String,
    pub run_id: String,
    pub created_at_utc: String,
    pub provenance_ref: String,
}

/// Response wrapper for `/api/v1/audit/artifacts`.
///
/// `truth_state`:
/// - `"active"` — durable artifact history was queried from Postgres; `rows`
///   is authoritative and `backend` names the exact source table.
/// - `"backend_unavailable"` — no DB pool is configured, so durable history
///   could not be queried; `backend` is `"unavailable"` and empty `rows`
///   MUST NOT be treated as authoritative zero.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditArtifactsResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub backend: String,
    pub rows: Vec<AuditArtifactRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorTimelineRow {
    pub ts_utc: String,
    pub kind: String,
    pub run_id: Option<String>,
    pub detail: String,
    pub provenance_ref: String,
}

/// Response wrapper for `/api/v1/ops/operator-timeline`.
///
/// `truth_state`:
/// - `"active"` — durable operator timeline history was queried from Postgres;
///   `rows` is authoritative and `backend` names the exact source table set.
/// - `"backend_unavailable"` — no DB pool is configured, so durable history
///   could not be queried; `backend` is `"unavailable"` and empty `rows`
///   MUST NOT be treated as authoritative zero.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorTimelineResponse {
    pub canonical_route: String,
    pub truth_state: String,
    pub backend: String,
    pub rows: Vec<OperatorTimelineRow>,
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
    pub daemon_mode: String,
    pub adapter_id: String,
    pub deployment_start_allowed: bool,
    pub deployment_blocker: Option<String>,
    pub runtime_status: String,
    pub broker_status: String,
    /// AP-04: How broker_snapshot truth is sourced.
    /// `"synthetic"` = paper (local OMS); `"external"` = Alpaca (AP-03 REST fetch).
    /// Independent of market_data_health / strategy feed policy.
    pub broker_snapshot_source: String,
    /// AP-05: Alpaca websocket continuity truth.
    /// `"not_applicable"` for Paper; `"cold_start_unproven"`, `"live"`, or
    /// `"gap_detected"` for Alpaca.  Only `"live"` indicates proven continuity;
    /// all other values are fail-closed.
    pub alpaca_ws_continuity: String,
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
    pub daemon_mode: String,
    pub adapter_id: String,
    pub deployment_start_allowed: bool,
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
pub struct ReconcileMismatchRow {
    pub id: String,
    pub domain: String,
    pub symbol: String,
    pub internal_value: String,
    pub broker_value: String,
    pub status: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconcileMismatchesResponse {
    pub truth_state: String,
    pub snapshot_at_utc: Option<String>,
    pub rows: Vec<ReconcileMismatchRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStateResponse {
    pub daemon_mode: String,
    pub adapter_id: String,
    pub deployment_start_allowed: bool,
    pub deployment_blocker: Option<String>,
    pub operator_auth_mode: String,
    pub strategy_allowed: bool,
    pub execution_allowed: bool,
    pub system_trading_window: String,
    /// Classified market session type: `"regular"` | `"premarket"` | `"after_hours"` | `"closed"`.
    /// For paper/backtest (always-on policy): always `"regular"`.
    pub market_session: String,
    /// Operational exchange calendar state: `"open"` | `"closed"` | `"holiday"`.
    /// For paper/backtest (always-on policy): always `"open"`.
    pub exchange_calendar_state: String,
    /// Stable identifier for the calendar spec driving this session response.
    /// `"always_on"` (paper/backtest) or `"nyse_weekdays"` (live/shadow).
    pub calendar_spec_id: String,
    /// Operator-facing notes describing the authority basis of session truth.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFingerprintResponse {
    pub config_hash: String,
    pub adapter_id: String,
    pub risk_policy_version: Option<String>,
    pub strategy_bundle_version: Option<String>,
    pub build_version: String,
    pub environment_profile: String,
    pub runtime_generation_id: Option<String>,
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

/// Response wrapper for `/api/v1/system/config-diffs`.
///
/// `truth_state`:
/// - `"not_wired"` — no durable config-diff persistence is implemented yet;
///   `rows` is always empty and **must not** be treated as authoritative zero.
/// - `"active"` — reserved for when durable config-diff tracking is wired.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigDiffsResponse {
    /// `"not_wired"` = no durable config-diff source exists; rows is empty and not authoritative.
    pub truth_state: String,
    /// Empty when `truth_state == "not_wired"`.  Authoritative when `truth_state == "active"`.
    pub rows: Vec<ConfigDiffRow>,
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

/// Response wrapper for `/api/v1/strategy/summary`.
///
/// `truth_state`:
/// - `"not_wired"` — no real strategy-fleet registry is implemented yet;
///   `rows` is always empty and **must not** be treated as strategy truth.
///   The former synthetic `daemon_integrity_gate` surrogate row has been
///   removed; it was daemon-integrity state masquerading as a strategy row.
/// - `"active"` — reserved for when a real strategy-fleet source is wired.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySummaryResponse {
    /// `"not_wired"` = no real strategy-fleet source exists; rows is empty and not authoritative.
    pub truth_state: String,
    /// Empty when `truth_state == "not_wired"`.  Authoritative when `truth_state == "active"`.
    pub rows: Vec<StrategySummaryRow>,
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

/// Response wrapper for `/api/v1/strategy/suppressions`.
///
/// `truth_state`:
/// - `"not_wired"` — no durable suppression persistence is implemented yet;
///   `rows` is always empty and **must not** be treated as authoritative zero.
/// - `"active"` — reserved for when durable suppression tracking is wired.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySuppressionsResponse {
    /// `"not_wired"` = no durable suppression source exists; rows is empty and not authoritative.
    pub truth_state: String,
    /// Empty when `truth_state == "not_wired"`.  Authoritative when `truth_state == "active"`.
    pub rows: Vec<StrategySuppressionRow>,
}

// ---------------------------------------------------------------------------
// /api/v1/system/runtime-leadership
// ---------------------------------------------------------------------------

/// One durable checkpoint event in the runtime lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeLeadershipCheckpointRow {
    pub checkpoint_id: String,
    /// "restart" | "leader_acquired" | "leader_lost" | "recovery_complete" | "snapshot_refresh"
    pub checkpoint_type: String,
    pub timestamp: String,
    pub generation_id: String,
    pub leader_node: String,
    /// "ok" | "warning" | "critical"
    pub status: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeLeadershipResponse {
    /// "local" for a single-node daemon; cluster node identifier otherwise.
    pub leader_node: String,
    /// "held" = running and owns the lease; "contested" = unknown state;
    /// "lost" = idle or halted.
    pub leader_lease_state: String,
    /// Unique identifier for the current runtime generation when authoritative
    /// runtime state exists. `null` when no active run or durable latest-run
    /// record is available; the daemon must not fabricate a placeholder ID.
    pub generation_id: Option<String>,
    /// Count of run starts in the last 24 h, sourced from the `runs` table
    /// (`started_at_utc > now() - interval '24 hours'`).
    /// `null` when no DB pool is configured; a real authoritative count otherwise.
    pub restart_count_24h: Option<u32>,
    /// UTC timestamp of the most recent run start, if known.
    pub last_restart_at: Option<String>,
    /// "complete" = reconcile confirmed clean post-restart;
    /// "in_progress" = reconcile not yet finished;
    /// "degraded" = reconcile found mismatches or is stale.
    pub post_restart_recovery_state: String,
    /// Reconcile timestamp or "none" when reconcile has not yet run.
    pub recovery_checkpoint: String,
    /// Ordered lifecycle checkpoint events (empty when DB unavailable).
    pub checkpoints: Vec<RuntimeLeadershipCheckpointRow>,
}

// ---------------------------------------------------------------------------
// /api/v1/system/metadata
// ---------------------------------------------------------------------------

/// Canonical system metadata surface.  All fields are derived from durable
/// daemon state at request time; no placeholders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetadataResponse {
    /// Daemon binary version from the build manifest.
    pub build_version: String,
    /// API version in use (currently "v1").
    pub api_version: String,
    /// Active broker adapter identifier (e.g. "paper", "alpaca").
    pub broker_adapter: String,
    /// Overall daemon endpoint health: "ok" if armed, "warning" otherwise.
    pub endpoint_status: String,
    /// Deployment mode label (paper/live/backtest).
    pub daemon_mode: String,
    /// Adapter ID — mirrors broker_adapter for GUI convenience.
    pub adapter_id: String,
}

// ---------------------------------------------------------------------------
// /api/v1/ops/action  — canonical operator action dispatcher
// ---------------------------------------------------------------------------

/// Request body for POST /api/v1/ops/action.
/// `action_key` is the canonical GUI action identifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpsActionRequest {
    /// Canonical action key: "arm-execution", "arm-strategy", "disarm-execution",
    /// "disarm-strategy", "start-system", "stop-system", "kill-switch",
    /// "change-system-mode" (returns 409 — not yet authoritative).
    pub action_key: String,
    /// Optional reason string for audit trail. Not required by the dispatcher.
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// /api/v1/ops/catalog — canonical Action Catalog
// ---------------------------------------------------------------------------

/// One entry in the canonical operator Action Catalog.
///
/// The catalog lists every action the daemon's `/api/v1/ops/action` dispatcher
/// can actually execute right now.  `enabled` reflects current runtime state;
/// `disabled_reason` explains why the action is unavailable when `enabled` is false.
///
/// `change-system-mode` is intentionally absent — it returns 409 from ops_action
/// (mode transitions require a controlled daemon restart).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionCatalogEntry {
    /// Canonical action identifier, e.g. "arm-execution".
    pub action_key: String,
    /// Human-readable label for operator UI.
    pub label: String,
    /// Severity level: 0 = informational, 1 = normal, 2 = elevated, 3 = emergency.
    pub level: u8,
    /// Human-readable description of what this action does.
    pub description: String,
    /// Whether this action requires an operator reason string.
    pub requires_reason: bool,
    /// Confirmation prompt text shown before the action executes.
    pub confirm_text: String,
    /// Whether this action is currently executable given system state.
    pub enabled: bool,
    /// Why the action is disabled; None when enabled is true.
    pub disabled_reason: Option<String>,
}

/// Response body for GET /api/v1/ops/catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionCatalogResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// All actions the daemon currently supports.  State-aware availability
    /// (enabled/disabled_reason) is computed from the live daemon state at
    /// request time.
    pub actions: Vec<ActionCatalogEntry>,
}

// ---------------------------------------------------------------------------
// /api/v1/execution/orders — canonical OMS order surface
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualOrderSubmitRequest {
    pub client_request_id: String,
    pub symbol: String,
    pub side: String,
    pub qty: serde_json::Value,
    pub order_type: Option<String>,
    pub time_in_force: Option<String>,
    pub limit_price: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualOrderSubmitResponse {
    pub accepted: bool,
    pub disposition: String,
    pub client_request_id: String,
    pub active_run_id: Option<Uuid>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualOrderCancelRequest {
    pub cancel_request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualOrderCancelResponse {
    pub accepted: bool,
    pub disposition: String,
    pub order_id: String,
    pub active_run_id: Option<Uuid>,
    pub blockers: Vec<String>,
}

/// One live order row sourced from the in-memory OMS runtime snapshot.
///
/// Fields that are not present in the OMS snapshot are emitted as `null`:
/// - `strategy_id`: `null` — no strategy attribution at the OMS layer.
/// - `side`: `null` — per-order side is not tracked in the OMS snapshot.
/// - `order_type`: `null` — order type is not captured in OMS state.
/// - `age_ms`: `null` — per-order creation time is not in the OMS snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionOrderRow {
    /// Internal (client) order identifier assigned by this daemon.
    pub internal_order_id: String,
    /// Broker-assigned order ID; `None` until the submit is confirmed.
    pub broker_order_id: Option<String>,
    pub symbol: String,
    /// `null` — OMS runtime has no strategy attribution per order.
    pub strategy_id: Option<String>,
    /// `null` — per-order side is not tracked in the OMS snapshot.
    pub side: Option<String>,
    /// `null` — order type is not captured at OMS snapshot level.
    pub order_type: Option<String>,
    pub requested_qty: i64,
    pub filled_qty: i64,
    /// Canonical OMS state: `"Open"` | `"PartiallyFilled"` | `"Filled"` |
    /// `"CancelPending"` | `"Cancelled"` | `"ReplacePending"` | `"Rejected"`
    pub current_status: String,
    /// Display-friendly lifecycle stage derived from `current_status`.
    pub current_stage: String,
    /// `null` — per-order creation timestamps are not in the OMS snapshot.
    pub age_ms: Option<u64>,
    pub has_warning: bool,
    /// `true` when `current_status == "Rejected"`.
    pub has_critical: bool,
    /// RFC 3339 timestamp of the execution snapshot that produced this row.
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// /api/v1/portfolio/positions  /api/v1/portfolio/orders/open  /api/v1/portfolio/fills
// Canonical broker-snapshot portfolio surfaces (Cluster 2)
// ---------------------------------------------------------------------------

/// One broker-layer position row.
///
/// Fields with no broker-snapshot equivalent are emitted as `null`:
/// - `strategy_id`: `null` — positions are not attributed to a strategy at
///   the broker snapshot level.
/// - `mark_price`, `unrealized_pnl`, `realized_pnl_today`: `null` — mark-to-
///   market data is not present in the broker snapshot.
/// - `drift`: `null` — reconcile-level position drift is not assessed at the
///   broker snapshot layer.
/// - `broker_qty`: same as `qty` — the row IS the broker view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioPositionRow {
    pub symbol: String,
    /// `null` — broker-snapshot positions have no strategy attribution.
    pub strategy_id: Option<String>,
    pub qty: i64,
    pub avg_price: f64,
    /// `null` — mark prices are not present in the broker snapshot.
    pub mark_price: Option<f64>,
    /// `null` — broker snapshot has no unrealized PnL.
    pub unrealized_pnl: Option<f64>,
    /// `null` — broker snapshot has no today-only realized PnL.
    pub realized_pnl_today: Option<f64>,
    /// Equals `qty` — this row is sourced from the broker view.
    pub broker_qty: i64,
    /// `null` — reconcile-level drift is not assessed at broker snapshot layer.
    pub drift: Option<bool>,
}

/// Response wrapper for `/api/v1/portfolio/positions`.
///
/// `snapshot_state`:
/// - `"active"` — broker snapshot is present; `rows` is authoritative (may be
///   empty when the account holds no positions).
/// - `"no_snapshot"` — no broker snapshot is loaded; `rows` is always empty
///   and must NOT be treated as authoritative zero.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioPositionsResponse {
    pub snapshot_state: String,
    pub captured_at_utc: Option<String>,
    pub rows: Vec<PortfolioPositionRow>,
}

/// One broker-layer open-order row.
///
/// - `strategy_id`: `null` — broker snapshot has no strategy attribution.
/// - `filled_qty`: `null` — broker snapshot does not track partial fills per order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioOpenOrderRow {
    /// Client order ID assigned by this daemon (= `client_order_id` in broker snapshot).
    pub internal_order_id: String,
    pub symbol: String,
    /// `null` — open orders are not strategy-attributed at the broker snapshot layer.
    pub strategy_id: Option<String>,
    pub side: String,
    pub status: String,
    pub requested_qty: i64,
    /// `null` — partial fill quantity is not tracked in the broker snapshot.
    pub filled_qty: Option<i64>,
    pub entered_at: String,
}

/// Response wrapper for `/api/v1/portfolio/orders/open`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioOpenOrdersResponse {
    pub snapshot_state: String,
    pub captured_at_utc: Option<String>,
    pub rows: Vec<PortfolioOpenOrderRow>,
}

/// One broker-layer fill row.
///
/// - `strategy_id`: `null` — broker snapshot has no strategy attribution.
/// - `applied`: `true` — fills present in the broker snapshot are by definition
///   already applied.
/// - `broker_exec_id`: equals `fill_id` (= `broker_fill_id` from broker API).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioFillRow {
    pub fill_id: String,
    /// Client order ID of the order that generated this fill.
    pub internal_order_id: String,
    pub symbol: String,
    /// `null` — fills are not strategy-attributed at the broker snapshot layer.
    pub strategy_id: Option<String>,
    pub side: String,
    pub qty: i64,
    pub price: f64,
    /// Equals `fill_id` — uses broker fill ID as the execution ID.
    pub broker_exec_id: String,
    /// `true` — fills in the snapshot are already applied.
    pub applied: bool,
    pub at: String,
}

/// Response wrapper for `/api/v1/portfolio/fills`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioFillsResponse {
    pub snapshot_state: String,
    pub captured_at_utc: Option<String>,
    pub rows: Vec<PortfolioFillRow>,
}

// ---------------------------------------------------------------------------
// /api/v1/risk/denials — canonical risk denial truth surface (Cluster 3)
// ---------------------------------------------------------------------------

/// One structured denial row from the risk gate.
///
/// Fields map 1:1 to the GUI `RiskDenialRow` type so the operator sees exact
/// denial evidence without transformation.
///
/// `strategy_id` is `None` / `null` at all times: the risk gate operates on
/// the order itself and has no access to which strategy generated it.  The
/// field is optional in the type contract so that it is honest (`null` in
/// JSON) rather than a placeholder empty string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskDenialRow {
    pub id: String,
    pub at: String,
    /// Always `null` — strategy attribution is not available on the risk gate
    /// path.  The gate sees the order, not the originating strategy.
    pub strategy_id: Option<String>,
    pub symbol: String,
    /// The risk rule that was violated, e.g. `"PositionLimitExceeded"`.
    pub rule: String,
    /// Human-readable denial message derived from `rule` + `evidence`.
    pub message: String,
    /// `"warning"` | `"critical"`.  Critical when the denial class is
    /// terminal (e.g. `RiskEngineUnavailable`, `CapitalLimitExceeded`).
    pub severity: String,
}

/// Response wrapper for `GET /api/v1/risk/denials`.
///
/// `truth_state` explicitly distinguishes three semantically different
/// response postures:
///
/// - `"active"` — execution loop is running AND a DB pool is available.
///   `denials` contains ONLY rows that are durably stored in
///   `sys_risk_denial_events`.  Restart-safe.  An empty `denials` array
///   means the risk gate has genuinely never denied any order in this
///   deployment (not just the current session).
///
/// - `"active_session_only"` — execution loop is running but NO DB pool is
///   available.  `denials` is populated from the in-memory ring buffer only.
///   NOT restart-safe: rows will be lost on daemon restart.  Returned only
///   in DB-less test environments; production deployments always have a pool.
///
/// - `"durable_history"` — execution loop is not currently running but the
///   DB has historical denial rows from a prior session.  `denials` is
///   durably sourced; restart-safe.  The GUI passes this through as
///   `ok: true` and renders the historical rows.
///
/// - `"no_snapshot"` — no durable rows exist and the loop is not running.
///   `denials` is always empty and **must not** be treated as authoritative
///   zero.  GUI IIFE emits `ok: false` → risk panel blocks.
///
/// The GUI IIFE blocks only on `"no_snapshot"` and `"not_wired"`.
/// `"active"`, `"active_session_only"`, and `"durable_history"` all pass
/// through as `ok: true`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskDenialsResponse {
    /// `"active"` = loop running + DB pool → durable rows only.
    /// `"active_session_only"` = loop running + no DB pool → ring buffer only.
    /// `"durable_history"` = loop not running, DB has historical rows.
    /// `"no_snapshot"` = no DB rows and loop not running.
    pub truth_state: String,
    /// UTC timestamp of the execution snapshot (present when loop is running).
    pub snapshot_at_utc: Option<String>,
    /// Denial rows.  Restart-safe when `truth_state` is `"active"` or
    /// `"durable_history"`.  Ephemeral when `"active_session_only"`.
    /// Always empty when `"no_snapshot"`.
    pub denials: Vec<RiskDenialRow>,
}

// ---------------------------------------------------------------------------
// /api/v1/diagnostics/snapshot (B4)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsSnapshotResponse {
    pub snapshot: Option<ExecutionSnapshot>,
}
