//! Request and response types for all mqk-daemon HTTP endpoints.
//!
//! These types are `Serialize + Deserialize` so they can be JSON-encoded
//! by Axum and decoded by tests.  No business logic lives here.

use mqk_runtime::observability::ExecutionSnapshot;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
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
    /// Durable restart intent snapshot.  Present only when `action_key` is
    /// "request-mode-change" and the transition is `admissible_with_restart`
    /// (disposition = "pending_restart").  Null in all other cases.
    pub pending_restart_intent: Option<PendingRestartIntentSnapshot>,
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
    /// PT-AUTO-03: Autonomous signal intake count for this execution run.
    ///
    /// `None` when `ExternalSignalIngestion` is not configured for this deployment
    /// (i.e., not paper+alpaca).  Field is not applicable and carries no meaning.
    ///
    /// `Some(n)` for paper+alpaca: the number of distinct new outbox enqueues
    /// (Gate 7 Ok(true)) accepted so far this run.  Resets to 0 at each run start.
    /// When `autonomous_signal_limit_hit` is `Some(true)`, this value equals
    /// `MAX_AUTONOMOUS_SIGNALS_PER_RUN` (100) and Gate 1d is blocking all further
    /// signals until the next run start.
    pub autonomous_signal_count: Option<u32>,
    /// PT-AUTO-03: Whether the autonomous day signal intake limit has been reached.
    ///
    /// `None` when `ExternalSignalIngestion` is not configured (not applicable).
    ///
    /// `Some(true)` means Gate 1d is currently refusing all incoming signals with
    /// `409/day_limit_reached`.  No further signals will be accepted until the next
    /// `run/start` resets the counter.
    ///
    /// `Some(false)` means Gate 1d is not tripping; signal intake is still open
    /// (subject to all other gates).
    pub autonomous_signal_limit_hit: Option<bool>,
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
    // AUTON-TRUTH-02: Autonomous-paper readiness fields.
    //
    // Populated only for the canonical Paper+Alpaca deployment.
    // All fields are `None` / empty for other deployments — they carry no
    // meaning and must not be interpreted as pass/fail on non-paper+alpaca.
    /// True only for Paper+Alpaca. Determines whether the fields below apply.
    pub autonomous_readiness_applicable: bool,
    /// WS continuity proven: `Some(true)` only when `alpaca_ws_continuity == "live"`.
    /// `None` when not paper+alpaca.
    pub ws_continuity_ready: Option<bool>,
    /// Reconcile not dirty/stale: `Some(true)` when reconcile is neither "dirty" nor "stale".
    /// `None` when not paper+alpaca.
    pub reconcile_ready: Option<bool>,
    /// Autonomous arm state: `"armed"` | `"arm_pending"` | `"halted"` | `"not_applicable"`.
    ///
    /// - `"armed"` — in-memory integrity is armed; start can proceed.
    /// - `"arm_pending"` — disarmed in memory but not halted; the session
    ///   controller will call `try_autonomous_arm` (DB-ARMED → advances to armed).
    /// - `"halted"` — operator halt asserted; requires manual operator arm.
    /// - `"not_applicable"` — not paper+alpaca.
    pub autonomous_arm_state: String,
    /// Exact autonomous-paper blockers derived from the same gate order as
    /// `start_execution_runtime`.  Empty when not paper+alpaca or when all
    /// checks pass.  These are operator-actionable reasons why the next
    /// autonomous start attempt will refuse.
    pub autonomous_blockers: Vec<String>,
    /// Whether the current wall-clock time is inside the autonomous session window.
    /// `Some(true)` = in window, `Some(false)` = outside window.
    /// `None` when not paper+alpaca.
    pub session_in_window: Option<bool>,
}

// ---------------------------------------------------------------------------
// AUTON-TRUTH-01: GET /api/v1/autonomous/readiness
// ---------------------------------------------------------------------------

/// Autonomous-paper readiness truth surface.
///
/// Surfaces the real gate state that governs whether the session controller
/// can start an execution run on the canonical Paper+Alpaca path.  All field
/// values are derived directly from live daemon state; nothing is synthesised.
///
/// `truth_state`:
/// - `"active"` — deployment is Paper+Alpaca; all fields are authoritative.
/// - `"not_applicable"` — deployment is not Paper+Alpaca; autonomous readiness
///   does not apply.  All boolean fields are `false`; `blockers` contains
///   a single explanatory entry.
///
/// `overall_ready` is the conjunction of all individual readiness flags.  Only
/// `true` when every gate that `start_execution_runtime` enforces would pass
/// right now.  `false` does NOT mean the system is broken — it means at least
/// one gate would refuse start in its current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomousPaperReadinessResponse {
    pub canonical_route: String,
    /// `"active"` for paper+alpaca; `"not_applicable"` otherwise.
    pub truth_state: String,
    /// True when deployment is Paper+Alpaca (the canonical autonomous path).
    pub canonical_path: bool,
    /// Alpaca WS continuity: `"live"` | `"cold_start_unproven"` | `"gap_detected"` | `"not_applicable"`.
    pub ws_continuity: String,
    /// True only when `ws_continuity == "live"` (BRK-00R-04 gate).
    pub ws_continuity_ready: bool,
    /// Reconcile status: `"ok"` | `"dirty"` | `"stale"` | `"unknown"`.
    pub reconcile_status: String,
    /// True when reconcile is not `"dirty"` or `"stale"` (BRK-09R gate).
    pub reconcile_ready: bool,
    /// Autonomous supervisory state from `AppState::autonomous_session_truth()`.
    /// `"clear"` | `"start_refused"` | `"recovery_retrying"` | `"recovery_succeeded"`
    /// | `"recovery_failed"` | `"run_ended_unexpectedly"` | `"stop_failed"`
    /// | `"stopped_at_boundary"` | `"not_applicable"`.
    pub autonomous_session_state: String,
    /// Human-readable detail from the current autonomous supervisory truth, if any.
    pub autonomous_session_detail: Option<String>,
    /// Integrity arm state as known in-memory.
    /// `"armed"` | `"arm_pending"` | `"halted"` | `"not_applicable"`.
    pub arm_state: String,
    /// True when in-memory integrity is armed (`arm_state == "armed"`).
    pub arm_ready: bool,
    /// True when `ExternalSignalIngestion` is configured (always true for paper+alpaca).
    pub signal_ingestion_configured: bool,
    /// True when the current wall-clock time is inside the configured autonomous session
    /// window (NYSE regular session hours or the fixed UTC window from env vars).
    /// False when outside the window — the session controller will not attempt a start.
    pub session_in_window: bool,
    /// Human-readable session-window state: `"in_window"` | `"outside_window"`.
    pub session_window_state: String,
    /// True when no locally-owned execution run is active (`locally_owned_run_id()` returns
    /// `None`).  False means a run is already active; start would return 409 Conflict.
    pub runtime_start_allowed: bool,
    /// Exact reasons why an autonomous start would be refused right now, in gate order.
    /// Empty when all checks pass.
    pub blockers: Vec<String>,
    /// True only when every readiness gate would pass: ws_continuity_ready &&
    /// reconcile_ready && arm_ready && signal_ingestion_configured &&
    /// session_in_window && runtime_start_allowed.
    pub overall_ready: bool,
    /// AUTON-HIST-01: True when at least one autonomous session event could not
    /// be persisted (no DB configured or DB write failure).  Sticky — never
    /// cleared in-session.  When true, `/api/v1/events/feed` autonomous-session
    /// history is incomplete or absent.  Operator must restart with a working DB.
    pub autonomous_history_degraded: bool,
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
    /// PORT-05: Machine-readable truth state for operator supervision.
    ///
    /// - `"no_snapshot"` — no broker snapshot is loaded; all financial fields are
    ///   `null`.  Empty portfolio must NOT be inferred from this state.
    /// - `"active"` — a broker snapshot is present; fields derive from it.
    ///
    /// **`session_boundary = "in_memory_only"`** — the broker snapshot is held
    /// in-memory and reset on every daemon restart.  After a restart this surface
    /// returns `"no_snapshot"` until a fresh snapshot is loaded.
    pub truth_state: String,
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
    /// RECON-06: Machine-readable truth state disambiguating reconcile lifecycle.
    ///
    /// - `"never_run"` — the reconcile loop has not completed a tick since daemon
    ///   start.  `status = "unknown"` in this state.  Not the same as an error.
    /// - `"active"` — reconcile has completed at least one tick; `status` is
    ///   authoritative (`"ok"` or a mismatch count summary).
    /// - `"stale"` — the last reconcile result is too old to be considered
    ///   authoritative; operator must trigger a fresh snapshot.
    pub truth_state: String,
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
    /// RECON-06: Operator review guidance when mismatches are present.
    ///
    /// `None` when `rows` is empty (no review needed) or when truth_state is
    /// not `"active"` (not authoritative).
    ///
    /// `Some(guidance)` when `rows` is non-empty and truth_state is `"active"`:
    /// the guidance string explicitly names the required operator actions.
    pub review_workflow: Option<String>,
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
/// - `"not_wired"` — the daemon does not have an authoritative comparison
///   baseline available; `backend` is `"not_wired"` and empty `rows` **must
///   not** be treated as authoritative zero.
/// - `"active"` — the daemon compared current runtime-selection truth against
///   the latest durable daemon run in `postgres.runs`; `backend` names the
///   exact authoritative source and `rows` is authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigDiffsResponse {
    /// Stable route identity for downstream callers and tests.
    pub canonical_route: String,
    /// `"not_wired"` = no authoritative comparison baseline is available.
    pub truth_state: String,
    /// `"not_wired"` until the daemon can compare against durable run truth.
    pub backend: String,
    /// Empty when `truth_state == "not_wired"`.  Authoritative when `truth_state == "active"`.
    pub rows: Vec<ConfigDiffRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySummaryRow {
    /// Sourced: canonical strategy identity from `sys_strategy_registry`.
    pub strategy_id: String,
    /// Sourced: human-readable display name from `sys_strategy_registry.display_name`.
    /// CC-01C: surfaced from durable registry truth.
    pub display_name: String,
    /// Sourced: durable `enabled` flag from `sys_strategy_registry`.
    /// `true` = registered + active; `false` = registered + inactive (known but disabled).
    pub enabled: bool,
    /// Sourced: operator-assigned category from `sys_strategy_registry.kind`.
    /// Empty string when unclassified.  CC-01C: surfaced from durable registry truth.
    pub kind: String,
    /// Sourced: RFC3339 timestamp when this strategy was first registered.
    /// From `sys_strategy_registry.registered_at_utc`.  CC-01C: durable provenance.
    pub registered_at: String,
    /// Sourced: optional operator note from `sys_strategy_registry.note`.
    /// Empty string when none was recorded.  CC-01C: surfaced from durable registry truth.
    pub note: String,
    /// Sourced: reflects the current daemon integrity arm state at response time.
    pub armed: bool,
    /// `null` — no strategy health monitor is wired; honest null, not synthetic "ok".
    pub health_status: Option<String>,
    /// `null` — universe membership is not tracked by the daemon; honest null.
    pub universe_size: Option<usize>,
    /// `null` — intent pipeline metrics are not sourced from daemon state; honest null.
    pub pending_intents: Option<usize>,
    /// `null` — open position counts are not sourced from daemon state; honest null.
    pub open_positions: Option<usize>,
    /// `null` — no portfolio accounting is wired; honest null, not synthetic zero.
    pub today_pnl: Option<f64>,
    /// `null` — no drawdown tracking is wired; honest null, not synthetic zero.
    pub drawdown_pct: Option<f64>,
    /// `null` — no regime detector is wired; honest null, not synthetic string.
    pub regime: Option<String>,
    /// `null` — no throttle controller is wired; honest null, not synthetic "normal".
    pub throttle_state: Option<String>,
    pub last_decision_time: Option<String>,
}

/// Response wrapper for `/api/v1/strategy/summary`.
///
/// `truth_state` (CC-01B):
/// - `"no_db"` — DB unavailable; `rows` is empty and **must not** be treated as
///   authoritative.  Fail-closed: callers must not infer "no active strategies"
///   from this state.
/// - `"registry"` — reading from `postgres.sys_strategy_registry`; `rows` are
///   authoritative.  Empty `rows` means no strategies have been registered
///   (authoritative empty ≠ unavailable).  Each row carries the durable
///   `enabled` flag: `true` = registered + active; `false` = registered + inactive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySummaryResponse {
    pub canonical_route: String,
    pub backend: String,
    /// `"no_db"` = DB unavailable; rows empty and not authoritative (fail closed).
    /// `"registry"` = reading from postgres.sys_strategy_registry; rows authoritative.
    pub truth_state: String,
    /// Empty when `truth_state == "no_db"`.  Authoritative when `truth_state == "registry"`.
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
/// - `"no_db"` — no DB pool configured; source unavailable; rows is empty and
///   **must not** be treated as authoritative zero.
/// - `"active"` — DB present; rows are authoritative.  Empty `rows` means
///   no suppressions exist.  Non-empty rows are real durable records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySuppressionsResponse {
    pub canonical_route: String,
    pub backend: String,
    /// `"no_db"` = DB unavailable; rows empty and not authoritative.
    /// `"active"` = DB present; rows authoritative (empty = no suppressions).
    pub truth_state: String,
    /// Empty when `truth_state == "no_db"`.  Authoritative when `truth_state == "active"`.
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
    /// "request-mode-change" (persists restart intent when admissible),
    /// "cancel-mode-transition" (cancels a pending restart intent),
    /// "change-system-mode" (returns 409 — guidance only, preserved for compat).
    pub action_key: String,
    /// Optional reason string for audit trail. Not required by the dispatcher.
    pub reason: Option<String>,
    /// Required for "request-mode-change": target deployment mode label.
    /// One of: "paper", "live-shadow", "live-capital", "backtest".
    pub target_mode: Option<String>,
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
// /api/v1/ops/mode-change-guidance — controlled mode-transition workflow
// ---------------------------------------------------------------------------

/// Runtime state relevant to mode-transition safety decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeChangeRestartTruth {
    /// Run ID owned by this daemon instance at the time of the request, if any.
    pub local_owned_run_id: Option<Uuid>,
    /// Most recent durable active run ID from the DB, if any.
    pub durable_active_run_id: Option<Uuid>,
    /// True when a durable active run exists but is not owned by this instance.
    pub durable_active_without_local_ownership: bool,
}

// ---------------------------------------------------------------------------
// CC-03C: Mounted controlled restart workflow truth
// ---------------------------------------------------------------------------

/// A single durable pending restart intent surfaced at the control-plane.
///
/// Sourced exclusively from `sys_restart_intent` (CC-03B).  Fields are
/// intentionally the minimal operator-visible subset: full lifecycle fields
/// (completed_at_utc) are not surfaced here because the mounted surface only
/// shows the **pending** workflow state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingRestartIntentSnapshot {
    /// UUID of the durable intent record.
    pub intent_id: String,
    /// Current deployment mode at the time the intent was created.
    pub from_mode: String,
    /// Intended target deployment mode.
    pub to_mode: String,
    /// CC-03A canonical transition verdict string stored in the DB.
    /// One of: `"same_mode"`, `"admissible_with_restart"`, `"refused"`, `"fail_closed"`.
    pub transition_verdict: String,
    /// Who initiated this intent: `"operator"`, `"system"`, or `"recovery"`.
    pub initiated_by: String,
    /// RFC3339 UTC timestamp when the intent was initiated.
    pub initiated_at_utc: String,
    /// Optional operator note or provenance reference.  Empty string if none.
    pub note: String,
}

/// CC-03C: Mounted restart workflow truth for the operator control surface.
///
/// Sourced from `sys_restart_intent` (CC-03B).  Always present in
/// `ModeChangeGuidanceResponse`; truth state determines authority.
///
/// `truth_state` values:
/// - `"active"` — DB was reachable, a pending restart intent was found;
///   `pending_intent` is the authoritative durable record.
/// - `"no_pending"` — DB was reachable, no pending intent exists; honest
///   absence.  Must NOT be treated as "restart is safe to skip".
/// - `"backend_unavailable"` — no DB pool is configured; restart workflow
///   truth cannot be determined; fail-closed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestartWorkflowTruth {
    /// Authority state for this restart workflow surface.
    pub truth_state: String,
    /// Pending restart intent, present only when `truth_state == "active"`.
    pub pending_intent: Option<PendingRestartIntentSnapshot>,
}

/// CC-03A: Per-target canonical mode-transition verdict.
///
/// One entry per possible target [`crate::state::DeploymentMode`], derived
/// exclusively from [`crate::mode_transition::evaluate_mode_transition`].
/// Callers must treat this as read-only truth — not as a configuration surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeTransitionEntry {
    /// Target mode label (e.g. `"live-shadow"`).
    pub target_mode: String,
    /// Canonical verdict: one of `"same_mode"`, `"admissible_with_restart"`,
    /// `"refused"`, `"fail_closed"`.
    pub verdict: String,
    /// Human-readable explanation of the verdict.
    pub reason: String,
    /// Ordered operator preconditions.  Non-empty only when
    /// `verdict == "admissible_with_restart"`.
    pub preconditions: Vec<String>,
}

/// Response for GET /api/v1/ops/mode-change-guidance and for the
/// `change-system-mode` arm of POST /api/v1/ops/action (409 CONFLICT).
///
/// Mode transitions are **never** authoritative via API — there is no hot
/// switching.  This response provides the operator with an explicit,
/// authoritative workflow for executing a controlled restart-driven mode
/// change without guesswork.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeChangeGuidanceResponse {
    /// Self-identifying route: "/api/v1/ops/mode-change-guidance".
    pub canonical_route: String,
    /// Current deployment mode label (e.g. "paper", "live", "backtest").
    pub current_mode: String,
    /// Always false — mode transitions require a controlled daemon restart.
    pub transition_permitted: bool,
    /// Authoritative reason why hot switching is refused.
    pub transition_refused_reason: String,
    /// Conditions that must be satisfied before the daemon can be safely restarted.
    pub preconditions: Vec<String>,
    /// Ordered explicit steps the operator must follow for a safe mode transition.
    pub operator_next_steps: Vec<String>,
    /// Restart truth from the daemon's run registry.  None when no DB connection.
    pub restart_truth: Option<ModeChangeRestartTruth>,
    /// CC-03A: Canonical transition verdicts for every possible target mode,
    /// derived from [`crate::mode_transition::evaluate_mode_transition`].
    ///
    /// This field makes the mode-transition state machine observable at the
    /// API surface and ensures `build_mode_change_guidance` derives its
    /// transition semantics from the canonical seam rather than ad hoc logic.
    pub transition_verdicts: Vec<ModeTransitionEntry>,
    /// CC-03C: Durable restart workflow truth — the mounted, operator-visible
    /// controlled restart workflow state sourced from `sys_restart_intent`.
    ///
    /// Always present.  `truth_state` determines authority:
    /// `"active"` = pending intent found; `"no_pending"` = honest absence;
    /// `"backend_unavailable"` = no DB, fail-closed.
    pub restart_workflow: RestartWorkflowTruth,
}

// ---------------------------------------------------------------------------
// /api/v1/strategy/signal — PT-DAY-01: strategy-driven paper execution
// ---------------------------------------------------------------------------

/// Strategy signal submission request.
///
/// The caller (research-py or operator tooling) is responsible for computing
/// the signal from real market data.  The daemon validates the signal against
/// the current execution context and enqueues it for broker-backed dispatch.
///
/// `signal_id` is the caller-supplied idempotency key.  UUIDv5 derived from
/// (strategy_id, signal_ts, symbol, side, qty) is recommended to guarantee
/// deterministic deduplication across restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySignalRequest {
    /// Caller-supplied idempotency key; unique per signal intent.
    pub signal_id: String,
    /// Authoritative strategy identifier for attribution and suppression checks.
    pub strategy_id: String,
    pub symbol: String,
    /// Order direction: "buy" or "sell".
    pub side: String,
    /// Positive integer quantity (number or string representation).
    pub qty: serde_json::Value,
    /// Order type: "market" (default) or "limit".
    pub order_type: Option<String>,
    /// Time-in-force: "day" (default), "gtc", "ioc", "fok", "opg", "cls".
    pub time_in_force: Option<String>,
    /// Limit price in integer micros (required for limit orders; absent for market).
    pub limit_price: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySignalResponse {
    pub accepted: bool,
    /// Disposition: "enqueued" | "duplicate" | "rejected" | "unavailable" | "suppressed"
    ///              | "budget_denied" | "sizing_denied" | "exposure_denied"
    ///              | "exhaustion_denied" | "continuity_gap" | "outside_session"
    ///              | "day_limit_reached".
    pub disposition: String,
    pub signal_id: String,
    pub strategy_id: String,
    pub active_run_id: Option<Uuid>,
    pub blockers: Vec<String>,
    /// RTS-07: `true` when this submission placed a *new* execution intent in the
    /// outbox (Gate 7 `Ok(true)`).
    ///
    /// When `true`: an outbox row was written and carries
    /// `signal_source = "external_signal_ingestion"` as a provenance mark.
    /// The orchestrator's next Phase 1 tick will claim and dispatch it to the
    /// broker.  This is the only path that produces a pending execution intent.
    ///
    /// When `false`: no new outbox row was placed.  This covers gate failures,
    /// duplicate submissions (`disposition = "duplicate"`), and validation errors.
    /// The prior runtime state is unchanged.
    ///
    /// `#[serde(default)]` preserves backward compatibility: clients that do not
    /// send or receive this field deserialise it as `false`.
    #[serde(default)]
    pub intent_placed: bool,
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
///
/// PORT-05: `snapshot_source` and `session_boundary` make restart-aware
/// supervision explicit for operators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioPositionsResponse {
    pub snapshot_state: String,
    pub captured_at_utc: Option<String>,
    pub rows: Vec<PortfolioPositionRow>,
    /// PORT-05: How this snapshot was produced.
    /// - `"synthetic"` — paper mode; derived from local OMS + portfolio engine.
    /// - `"external"` — Alpaca REST fetch (external broker).
    /// - `null` when `snapshot_state = "no_snapshot"`.
    pub snapshot_source: Option<String>,
    /// PORT-05: Persistence boundary for this surface.
    ///
    /// Always `"in_memory_only"`: the broker snapshot is held in-memory and is
    /// reset on every daemon restart.  After a restart this surface returns
    /// `"no_snapshot"` until a fresh snapshot is loaded.  No durable history
    /// of positions is maintained by the daemon.
    pub session_boundary: String,
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
///
/// PORT-05: `snapshot_source` and `session_boundary` make restart-aware
/// supervision explicit for operators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioOpenOrdersResponse {
    pub snapshot_state: String,
    pub captured_at_utc: Option<String>,
    pub rows: Vec<PortfolioOpenOrderRow>,
    /// PORT-05: How this snapshot was produced. `null` when no snapshot.
    /// `"synthetic"` (paper/local OMS) or `"external"` (Alpaca REST).
    pub snapshot_source: Option<String>,
    /// PORT-05: Always `"in_memory_only"` — lost on daemon restart.
    pub session_boundary: String,
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
///
/// PORT-05: `snapshot_source` and `session_boundary` make restart-aware
/// supervision explicit for operators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioFillsResponse {
    pub snapshot_state: String,
    pub captured_at_utc: Option<String>,
    pub rows: Vec<PortfolioFillRow>,
    /// PORT-05: How this snapshot was produced. `null` when no snapshot.
    /// `"synthetic"` (paper/local OMS) or `"external"` (Alpaca REST).
    pub snapshot_source: Option<String>,
    /// PORT-05: Always `"in_memory_only"` — lost on daemon restart.
    pub session_boundary: String,
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

// ---------------------------------------------------------------------------
// /api/v1/metrics/dashboards (CC-05)
// ---------------------------------------------------------------------------

/// Metrics dashboard composed from existing truthful summary surfaces.
///
/// Gives an operator one endpoint for current performance and health KPIs
/// without hitting four separate summary routes.  All panels use explicit
/// truth_state semantics — None is never a fabricated zero.
///
/// Fields that are not derivable from current sources (daily_pnl, drawdown_pct,
/// loss_limit_utilization_pct) are always None.  This is intentional and honest:
/// the underlying summary routes also return None for these fields because the
/// data source does not exist yet.
///
/// # Panel truth states
///
/// - `portfolio_snapshot_state` / `risk_snapshot_state`: `"no_snapshot"` when
///   `broker_snapshot` is absent; `"active"` when present.
/// - `execution_snapshot_state`: `"no_snapshot"` when execution loop has not
///   started; `"active"` when execution loop is running with a snapshot.
/// - `reconcile_status`: always present (`"unknown"` before first reconcile tick).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsDashboardResponse {
    /// Canonical route for this surface.
    pub canonical_route: String,

    // --- Portfolio panel (from broker_snapshot) ---
    /// `"no_snapshot"` | `"active"`
    pub portfolio_snapshot_state: String,
    pub account_equity: Option<f64>,
    pub long_market_value: Option<f64>,
    pub short_market_value: Option<f64>,
    pub cash: Option<f64>,
    /// Not derivable from broker snapshot — always None in current sources.
    pub daily_pnl: Option<f64>,
    pub buying_power: Option<f64>,

    // --- Risk panel (from broker_snapshot positions + runtime state) ---
    /// `"no_snapshot"` | `"active"`
    pub risk_snapshot_state: String,
    pub gross_exposure: Option<f64>,
    pub net_exposure: Option<f64>,
    pub concentration_pct: Option<f64>,
    /// Not derivable from broker snapshot — always None in current sources.
    pub drawdown_pct: Option<f64>,
    /// Not derivable without a loss-limit config — always None in current sources.
    pub loss_limit_utilization_pct: Option<f64>,
    pub kill_switch_active: bool,
    pub active_breaches: usize,

    // --- Execution panel (from execution_snapshot / OMS) ---
    /// `"no_snapshot"` | `"active"`
    pub execution_snapshot_state: String,
    pub active_order_count: usize,
    pub pending_order_count: usize,
    pub dispatching_order_count: usize,
    pub reject_count_today: usize,

    // --- Reconcile panel ---
    /// `"ok"` | `"unknown"` | `"dirty"` | `"stale"` | `"unavailable"`
    pub reconcile_status: String,
    pub reconcile_last_run_at: Option<String>,
    /// Sum of all mismatch counts across positions, orders, fills, and broker events.
    pub reconcile_total_mismatches: usize,
}

// ---------------------------------------------------------------------------
// /api/v1/oms/overview (CC-04)
// ---------------------------------------------------------------------------

/// Single canonical OMS overview composed from mounted truth surfaces.
///
/// Gives an operator one endpoint to check current trading state without
/// piecing together scattered surfaces.  All lanes use explicit truth_state
/// semantics — absence of a snapshot is never silently treated as "zero".
///
/// # Lane semantics
///
/// - `runtime_*`: derived from StatusSnapshot — always present.
/// - `account_snapshot_state` / `portfolio_snapshot_state`: `"no_snapshot"`
///   when broker_snapshot is absent, `"active"` when present.
/// - `execution_has_snapshot`: false when execution loop has never started.
/// - `reconcile_*`: always present (defaults to `"unknown"` when unrun).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmsOverviewResponse {
    /// Canonical route for this surface.
    pub canonical_route: String,

    // --- Runtime lane ---
    /// `"idle"` | `"running"` | `"halted"` | `"unknown"`
    pub runtime_status: String,
    pub integrity_armed: bool,
    pub kill_switch_active: bool,
    pub daemon_mode: String,
    /// Count of active fault signals. Full detail at `GET /api/v1/system/status`.
    pub fault_signal_count: usize,

    // --- Account lane (from broker_snapshot) ---
    /// `"no_snapshot"` | `"active"`
    pub account_snapshot_state: String,
    /// Account equity as parsed f64. None when snapshot absent or parse fails.
    pub account_equity: Option<f64>,
    /// Account cash as parsed f64. None when snapshot absent or parse fails.
    pub account_cash: Option<f64>,

    // --- Portfolio lane (from broker_snapshot) ---
    /// `"no_snapshot"` | `"active"`
    pub portfolio_snapshot_state: String,
    /// UTC timestamp of broker snapshot capture. None when no snapshot.
    pub portfolio_snapshot_at_utc: Option<String>,
    pub position_count: usize,
    pub open_order_count: usize,
    pub fill_count: usize,

    // --- Execution lane (from execution_snapshot / OMS) ---
    /// false when execution loop has not started or no active run.
    pub execution_has_snapshot: bool,
    pub execution_active_orders: usize,
    pub execution_pending_orders: usize,

    // --- Reconcile lane ---
    /// `"ok"` | `"unknown"` | `"dirty"` | `"stale"` | `"unavailable"`
    pub reconcile_status: String,
    pub reconcile_last_run_at: Option<String>,
    /// Sum of mismatched_positions + mismatched_orders + mismatched_fills +
    /// unmatched_broker_events.
    pub reconcile_total_mismatches: usize,
}

// ---------------------------------------------------------------------------
// /api/v1/alerts/active (CC-06)
// ---------------------------------------------------------------------------

/// One active alert row sourced from current daemon fault signals.
///
/// An active alert is a current fault signal computed from live daemon state.
/// There is no persistent alert table, no alert lifecycle, and no ack state:
/// alerts exist while their underlying condition is present and disappear when
/// the condition is resolved.  `alert_id` is the fault signal class (a stable
/// slug), not a UUIDv4 — there is no durable alert registry.
///
/// Source: `build_fault_signals(StatusSnapshot, ReconcileStatusSnapshot, risk_blocked)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveAlertRow {
    /// Stable slug derived from the fault signal class.
    /// Identical to `class` — no persistent lifecycle ID exists.
    pub alert_id: String,
    /// `"warning"` | `"critical"`
    pub severity: String,
    /// Structured fault class (e.g., `"runtime.halt.operator_or_safety"`).
    pub class: String,
    /// Human-readable description of the current condition.
    pub summary: String,
    /// Optional detail string when the fault signal carries extra context.
    pub detail: Option<String>,
    /// Where this alert was computed from.
    /// Always `"daemon.runtime_state"` — in-memory computation, not DB-backed.
    pub source: String,
}

/// Response wrapper for `GET /api/v1/alerts/active`.
///
/// `truth_state`:
/// - `"active"` — always returned; the source is current in-memory daemon
///   state and is always available.  `rows` may be empty (no current alerts)
///   or populated with real fault-signal-backed alert rows.
///   Empty `rows` means the daemon has no current active fault conditions.
///   This is an authoritative "healthy" state, not an absence of source.
///
/// No ack/triage lifecycle exists.  Alerts do not persist beyond the lifetime
/// of their underlying condition.  Do not rely on `alert_id` across restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveAlertsResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// Always `"active"` — computed from live in-memory state at request time.
    pub truth_state: String,
    /// `"daemon.runtime_state"` — not DB-backed; computed from StatusSnapshot
    /// and ReconcileStatusSnapshot at request time.
    pub backend: String,
    /// Count of currently active alerts.  Equals `rows.len()`.
    pub alert_count: usize,
    /// Active alert rows.  Empty means no current fault conditions.
    pub rows: Vec<ActiveAlertRow>,
}

// ---------------------------------------------------------------------------
// /api/v1/events/feed (CC-06)
// ---------------------------------------------------------------------------

/// One recent event row from the operator/runtime event feed.
///
/// Events are sourced from three durable DB tables:
/// - `runs` — runtime lifecycle transitions (CREATED, ARMED, RUNNING,
///   STOPPED, HALTED).
/// - `audit_events` (topic=`'operator'`) — operator action events written
///   by `write_operator_audit_event` / `write_control_operator_audit_event`.
/// - `audit_events` (topic=`'signal_ingestion'`) — signal admission events
///   written by the strategy-signal route at Gate 7 `Ok(true)`.
/// - `sys_autonomous_session_events` — autonomous supervisor history events
///   written by `set_autonomous_session_truth` (AUTON-PAPER-02).
///
/// `event_id` equals `provenance_ref` and encodes the exact DB source row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFeedRow {
    /// Provenance reference for this event.
    /// Format: `"runs:{run_id}:{column}"` for runtime transitions,
    /// `"audit_events:{event_id}"` for operator and signal-admission actions,
    /// `"sys_autonomous_session_events:{id}"` for autonomous supervisor events.
    pub event_id: String,
    /// RFC 3339 timestamp.
    pub ts_utc: String,
    /// `"runtime_transition"` | `"operator_action"` | `"signal_admission"` | `"autonomous_session"`
    pub kind: String,
    /// Detail string (e.g., `"HALTED"`, `"control.arm"`).
    pub detail: String,
    /// Run ID associated with this event, if any.
    pub run_id: Option<String>,
    /// Stable provenance reference (equals `event_id`).
    pub provenance_ref: String,
}

/// Response wrapper for `GET /api/v1/events/feed`.
///
/// `truth_state`:
/// - `"active"` — DB pool is present; `rows` contains the most recent events
///   from `runs`, `audit_events`, and `sys_autonomous_session_events`;
///   authoritative.  Empty `rows` means no durable events exist yet.
/// - `"backend_unavailable"` — no DB pool configured; `rows` is always empty
///   and **must not** be treated as authoritative empty history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventsFeedResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// `"active"` = DB present, rows are authoritative recent events.
    /// `"backend_unavailable"` = no DB pool, rows empty, not authoritative.
    pub truth_state: String,
    /// `"postgres.runs+postgres.audit_events+postgres.sys_autonomous_session_events"` when active;
    /// `"unavailable"` when no DB pool.
    pub backend: String,
    /// Recent events sorted newest-first.  At most 50 rows.
    pub rows: Vec<EventFeedRow>,
}

// ---------------------------------------------------------------------------
// TV-EXEC-01: Fill-quality telemetry response types
// ---------------------------------------------------------------------------

/// One row in the fill-quality telemetry response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillQualityTelemetryRow {
    pub telemetry_id: Uuid,
    pub run_id: Uuid,
    pub internal_order_id: String,
    pub broker_order_id: Option<String>,
    pub broker_fill_id: Option<String>,
    pub broker_message_id: String,
    pub symbol: String,
    /// `"buy"` or `"sell"`
    pub side: String,
    pub ordered_qty: i64,
    pub fill_qty: i64,
    pub fill_price_micros: i64,
    /// `None` for market orders.
    pub reference_price_micros: Option<i64>,
    /// `None` when reference_price_micros is absent.
    pub slippage_bps: Option<i64>,
    pub submit_ts_utc: Option<String>,
    pub fill_received_at_utc: String,
    pub submit_to_fill_ms: Option<i64>,
    /// `"partial_fill"` or `"final_fill"`
    pub fill_kind: String,
    pub provenance_ref: String,
    pub created_at_utc: String,
}

/// Response wrapper for `GET /api/v1/execution/fill-quality`.
///
/// `truth_state`:
/// - `"active"` — DB pool and active run present; `rows` is authoritative.
///   Empty `rows` means no fills have been recorded for this run.
/// - `"no_active_run"` — daemon has a DB but no active run; `rows` is empty.
/// - `"no_db"` — no DB pool configured; `rows` is empty and not authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillQualityTelemetryResponse {
    pub canonical_route: String,
    /// See truth_state variants above.
    pub truth_state: String,
    /// `"postgres.fill_quality_telemetry"` when active; `"unavailable"` otherwise.
    pub backend: String,
    /// Most recent fills for the active run, newest-fill first. At most 100 rows.
    pub rows: Vec<FillQualityTelemetryRow>,
}

// ---------------------------------------------------------------------------
// TV-01B: Runtime artifact intake contract
// ---------------------------------------------------------------------------

/// Response for `GET /api/v1/system/artifact-intake`.
///
/// Surfaces the runtime artifact intake truth for the operator: whether a
/// promoted artifact has been configured, whether it is structurally valid,
/// and its identity if accepted.
///
/// `truth_state` values:
/// - `"not_configured"` — `MQK_ARTIFACT_PATH` is not set or empty; operator
///   has not provided an artifact.  Must NOT be treated as "no artifact needed".
/// - `"invalid"` — path is set but the file is unreadable, not valid JSON,
///   has wrong `schema_version`, or is missing required fields.  Fail-closed.
/// - `"accepted"` — the `promoted_manifest.json` is present and structurally
///   valid.  This is intake acceptance only — it does not imply deployability
///   or that any economic gate has been passed.
/// - `"unavailable"` — the intake evaluator itself could not run (e.g.,
///   unexpected evaluator failure).  Fail-closed: intake status is unknown.
///
/// This is the minimum honest runtime artifact intake contract (TV-01B).
/// TV-01C will thread `artifact_id` into run-start provenance.
/// TV-01D will prove the full promoted artifact → runtime consumption chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactIntakeResponse {
    /// Self-identifying route.
    pub canonical_route: String,
    /// Intake outcome: `"not_configured"` | `"invalid"` | `"accepted"` | `"unavailable"`.
    pub truth_state: String,
    /// Content-addressed artifact identity.  Non-null only when
    /// `truth_state == "accepted"`.
    pub artifact_id: Option<String>,
    /// Artifact type string (e.g. `"signal_pack"`).  Non-null only when
    /// `truth_state == "accepted"`.
    pub artifact_type: Option<String>,
    /// Promotion stage (e.g. `"paper"`).  Non-null only when
    /// `truth_state == "accepted"`.
    pub stage: Option<String>,
    /// Producing system identifier.  Non-null only when
    /// `truth_state == "accepted"`.
    pub produced_by: Option<String>,
    /// Human-readable reason for `"invalid"` or `"unavailable"` outcomes.
    /// Null for `"not_configured"` and `"accepted"`.  Callers must check
    /// `truth_state` to distinguish the two failure modes.
    pub invalid_reason: Option<String>,
    /// Path that was evaluated.  Null when `truth_state == "not_configured"`.
    pub evaluated_path: Option<String>,
}

// ---------------------------------------------------------------------------
// /api/v1/system/parity-evidence — TV-03A / TV-03B
// ---------------------------------------------------------------------------

/// TV-03A/TV-03B: Parity evidence manifest truth surface.
///
/// Reads `parity_evidence.json` (schema `parity-v1`) from the artifact
/// directory configured via `MQK_ARTIFACT_PATH` and returns the honest
/// parity-evidence state.  Written by the Python TV-03 pipeline.
///
/// `truth_state` values:
/// - `"not_configured"` — no artifact path configured; parity evidence gate
///   not applicable.
/// - `"absent"` — artifact path set but `parity_evidence.json` not found in
///   the artifact directory.  Absent evidence ≠ parity proven.
/// - `"invalid"` — `parity_evidence.json` found but structurally invalid.
/// - `"present"` — `parity_evidence.json` is valid and readable.
///   `live_trust_complete` is surfaced honestly (always `false` in current
///   builds).
/// - `"unavailable"` — evaluator itself could not run.
///
/// The operator surface guarantees:
/// - Absent, invalid, and unavailable are never conflated with "present".
/// - `live_trust_complete=false` is surfaced explicitly, not hidden.
/// - `evidence_available=false` is surfaced explicitly (no shadow run).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParityEvidenceResponse {
    /// Self-identifying route.
    pub canonical_route: String,
    /// `"not_configured"` | `"absent"` | `"invalid"` | `"present"` | `"unavailable"`.
    pub truth_state: String,
    /// Canonical artifact identity from the evidence file.
    /// Non-null only when `truth_state == "present"`.
    pub artifact_id: Option<String>,
    /// Whether the full parity proof chain is complete enough for live capital.
    /// Always `false` in current builds.  Non-null only when `truth_state == "present"`.
    pub live_trust_complete: Option<bool>,
    /// Whether a shadow evaluation run was actually performed.
    /// Non-null only when `truth_state == "present"`.
    pub evidence_available: Option<bool>,
    /// Human-readable description of what shadow evidence exists or is missing.
    /// Non-null only when `truth_state == "present"`.
    pub evidence_note: Option<String>,
    /// ISO-8601 UTC string when this parity evidence was produced.
    /// Non-null only when `truth_state == "present"`.
    pub produced_at_utc: Option<String>,
    /// Human-readable reason for invalid or unavailable states.
    /// Non-null only when `truth_state` is `"invalid"` or `"unavailable"`.
    pub invalid_reason: Option<String>,
    /// Artifact directory path that was evaluated.
    /// Non-null when `truth_state != "not_configured"`.
    pub evaluated_path: Option<String>,
}

// ---------------------------------------------------------------------------
// /v1/system/run-artifact — TV-01C
// ---------------------------------------------------------------------------

/// TV-01C: Artifact provenance accepted at the most recent `start_execution_runtime`.
///
/// `truth_state` values:
/// - `"active"` — an artifact was accepted at run start and the run is active;
///   all identity fields are populated.
/// - `"no_run"` — no run is active (daemon is idle/halted); artifact provenance
///   is not surfaced.  All identity fields are null.  Fail-closed.
///
/// This is distinct from `ArtifactIntakeResponse` (`/api/v1/system/artifact-intake`),
/// which evaluates the currently configured file on demand.  This route surfaces
/// what was actually accepted and consumed when the run started.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunArtifactProvenanceResponse {
    /// Self-identifying route.
    pub canonical_route: String,
    /// `"active"` | `"no_run"`.
    pub truth_state: String,
    /// Content-addressed artifact identity.  Non-null only when `truth_state == "active"`.
    pub artifact_id: Option<String>,
    /// Artifact type string.  Non-null only when `truth_state == "active"`.
    pub artifact_type: Option<String>,
    /// Promotion stage.  Non-null only when `truth_state == "active"`.
    pub stage: Option<String>,
    /// Producing system identifier.  Non-null only when `truth_state == "active"`.
    pub produced_by: Option<String>,
}

// ---------------------------------------------------------------------------
// JOUR-01: Paper trading journal and evidence surface
// ---------------------------------------------------------------------------

/// One durable signal-admission record from the journal.
///
/// Sourced from `audit_events` (topic=`'signal_ingestion'`, event_type=`'signal.admitted'`).
/// Written by the strategy signal route at Gate 7 `Ok(true)`.
///
/// Fields are extracted from the `payload` JSON column.  Parsing failure
/// for any field skips that row rather than emitting fabricated values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperJournalAdmissionRow {
    /// Audit event UUID (stable identifier for this admission record).
    pub event_id: String,
    /// RFC 3339 UTC timestamp when this signal was admitted.
    pub ts_utc: String,
    /// Caller-supplied signal idempotency key.
    pub signal_id: String,
    /// Originating strategy identifier.
    pub strategy_id: String,
    pub symbol: String,
    /// `"buy"` or `"sell"`
    pub side: String,
    /// Ordered quantity.
    pub qty: i64,
    /// Run ID this admission belongs to.
    pub run_id: String,
    /// Stable DB provenance reference: `"audit_events:{event_id}"`.
    pub provenance_ref: String,
}

/// Fill evidence lane of the paper journal.
///
/// `truth_state`:
/// - `"active"` — DB + active run; `rows` is authoritative fill history.
///   Empty `rows` = no fills yet recorded for this run.
/// - `"no_active_run"` — DB present but no active run; rows empty; not authoritative.
/// - `"no_db"` — no DB pool; rows empty; not authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperJournalFillsLane {
    pub truth_state: String,
    pub backend: String,
    pub rows: Vec<FillQualityTelemetryRow>,
}

/// Signal-admission history lane of the paper journal.
///
/// `truth_state`:
/// - `"active"` — DB + active run; `rows` is the durable admitted-signal log.
///   Empty `rows` = no signals admitted yet.
/// - `"no_active_run"` — DB present but no active run; rows empty; not authoritative.
/// - `"no_db"` — no DB pool; rows empty; not authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperJournalAdmissionsLane {
    pub truth_state: String,
    /// `"postgres.audit_events[topic=signal_ingestion]"` when active.
    /// `"unavailable"` otherwise.
    pub backend: String,
    pub rows: Vec<PaperJournalAdmissionRow>,
}

/// Response for `GET /api/v1/paper/journal`.
///
/// Unified paper-trading evidence surface for operator review.  Separates
/// fill evidence (what executed) from signal-admission history (what was
/// submitted and accepted into the outbox).
///
/// Both lanes carry independent `truth_state` values.  An operator can
/// answer:
/// - What fills were produced by this run? → `fills_lane`
/// - What signals were admitted for dispatch? → `admissions_lane`
///
/// Neither lane fabricates history.  If a lane is unavailable its `rows`
/// are empty and `truth_state` says so explicitly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperJournalResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// Active run ID when both lanes are `"active"`.  `None` otherwise.
    pub run_id: Option<String>,
    /// Fill evidence sourced from `postgres.fill_quality_telemetry`.
    pub fills_lane: PaperJournalFillsLane,
    /// Signal-admission history sourced from `postgres.audit_events`.
    pub admissions_lane: PaperJournalAdmissionsLane,
}

// ---------------------------------------------------------------------------
// /api/v1/execution/outbox — OPS-08 / EXEC-06: paper execution timeline
// ---------------------------------------------------------------------------

/// One row from the durable execution outbox for a run.
///
/// Fields extracted from `order_json` are `None` when the key is absent —
/// never fabricated.  `lifecycle_stage` is a display-friendly derivation
/// of `status` for operator readability.
///
/// Source: `postgres.oms_outbox` for the active run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionOutboxRow {
    /// Idempotency key assigned at signal intake or manual submit.
    pub idempotency_key: String,
    /// Run ID this outbox row belongs to.
    pub run_id: String,
    /// Durable status: `"PENDING"` | `"CLAIMED"` | `"DISPATCHING"` |
    /// `"SENT"` | `"ACKED"` | `"FAILED"` | `"AMBIGUOUS"`
    pub status: String,
    /// Display-friendly lifecycle stage derived from `status`.
    /// `"queued"` | `"claimed"` | `"submitting"` | `"sent_to_broker"` |
    /// `"acknowledged"` | `"failed"` | `"ambiguous"` | `"unknown"`
    pub lifecycle_stage: String,
    /// Symbol from `order_json["symbol"]`. `None` if absent.
    pub symbol: Option<String>,
    /// `"buy"` or `"sell"` from `order_json["side"]`. `None` if absent.
    pub side: Option<String>,
    /// Ordered qty from `order_json["qty"]`. `None` if absent.
    pub qty: Option<i64>,
    /// `"market"` or `"limit"` from `order_json["order_type"]`. `None` if absent.
    pub order_type: Option<String>,
    /// Originating strategy from `order_json["strategy_id"]`. `None` if absent
    /// (e.g., manual operator submit has no strategy attribution).
    pub strategy_id: Option<String>,
    /// Provenance mark from `order_json["signal_source"]`.
    /// `"external_signal_ingestion"` for strategy-driven intents; `None` for
    /// manual operator submits.
    pub signal_source: Option<String>,
    /// UTC timestamp when this intent was enqueued (durable).
    pub created_at_utc: String,
    /// UTC timestamp when the orchestrator claimed this row for dispatch.
    /// `None` if not yet claimed.
    pub claimed_at_utc: Option<String>,
    /// UTC timestamp when dispatch to broker began.
    /// `None` if not yet dispatching.
    pub dispatching_at_utc: Option<String>,
    /// UTC timestamp when the broker confirmed receipt.
    /// `None` if not yet sent.
    pub sent_at_utc: Option<String>,
}

/// Response wrapper for `GET /api/v1/execution/outbox`.
///
/// Surfaces the authoritative durable execution intent timeline for the
/// active run.  Operators can use this to understand what was submitted,
/// what is in-flight, what succeeded, and what failed — without relying
/// on ephemeral in-memory state.
///
/// `truth_state`:
/// - `"active"` — DB pool and active run present; `rows` is the authoritative
///   durable outbox for this run, ordered newest-first (at most 200 rows).
///   Empty `rows` means no execution intents have been enqueued yet in this run.
/// - `"no_active_run"` — DB pool present but no active run; `rows` is empty
///   and must NOT be treated as authoritative zero history.
/// - `"no_db"` — no DB pool configured; `rows` is empty and not authoritative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionOutboxResponse {
    /// Self-identifying canonical route.
    pub canonical_route: String,
    /// See truth_state variants above.
    pub truth_state: String,
    /// `"postgres.oms_outbox"` when active; `"unavailable"` otherwise.
    pub backend: String,
    /// Active run ID when `truth_state == "active"`. `None` otherwise.
    pub run_id: Option<String>,
    /// At most 200 rows, newest-first.  Authoritative only when `truth_state == "active"`.
    pub rows: Vec<ExecutionOutboxRow>,
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/orders/:order_id/timeline (Batch A5A)
// ---------------------------------------------------------------------------

/// One fill event row in the per-order execution timeline.
///
/// Source: `postgres.fill_quality_telemetry` for the active run.
/// Only fill events are represented; pre-fill outbox lifecycle events are not
/// joined to `internal_order_id` in the current schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderTimelineRow {
    /// Stable event identifier derived from `telemetry_id`.
    pub event_id: String,
    /// RFC 3339 timestamp when this fill event was received.
    pub ts_utc: String,
    /// Event kind: `"partial_fill"` | `"final_fill"`.
    pub stage: String,
    /// Data provenance: always `"fill_quality_telemetry"`.
    pub source: String,
    /// Human-readable summary (e.g. `"qty=50 @ $150.250000 (partial_fill)"`).
    pub detail: Option<String>,
    pub fill_qty: Option<i64>,
    pub fill_price_micros: Option<i64>,
    pub slippage_bps: Option<i64>,
    /// Always `"oms_inbox:{broker_message_id}"` from the fill row.
    pub provenance_ref: Option<String>,
}

/// Response wrapper for `GET /api/v1/execution/orders/:order_id/timeline`.
///
/// # Truth states
///
/// - `"active"` — DB + active run + at least one fill row found; `rows` is
///   authoritative and `backend` names the exact source table.
/// - `"no_fills_yet"` — DB + active run available, order is visible in the OMS
///   execution snapshot, but no fill rows exist yet; `rows` is empty.
/// - `"no_order"` — `order_id` was not found in any current authoritative source
///   (no active run, no snapshot, or no fill history).  `rows` is empty.
/// - `"no_db"` — no DB pool configured; `rows` is empty and not authoritative.
///
/// # Sources
///
/// - `symbol`, `requested_qty`, `filled_qty`, `current_status`, `current_stage`
///   — from the in-memory execution snapshot (ephemeral; not durable across restart).
/// - `rows` — from `postgres.fill_quality_telemetry` (durable, per active run).
///
/// # Honest limits
///
/// Timeline rows represent fill events only.  Pre-fill outbox lifecycle events
/// (queued/claimed/dispatching/sent) are not yet linked to `internal_order_id`
/// and are therefore absent from this surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderTimelineResponse {
    /// Self-identifying canonical route including the resolved `order_id`.
    pub canonical_route: String,
    pub truth_state: String,
    /// `"postgres.fill_quality_telemetry"` | `"unavailable"`.
    pub backend: String,
    pub order_id: String,
    /// `null` until the broker submit is confirmed.
    pub broker_order_id: Option<String>,
    /// From execution snapshot. `null` when snapshot is absent.
    pub symbol: Option<String>,
    /// From execution snapshot. `null` when snapshot is absent.
    pub requested_qty: Option<i64>,
    /// From execution snapshot. `null` when snapshot is absent.
    pub filled_qty: Option<i64>,
    /// Canonical OMS status from execution snapshot. `null` when snapshot is absent.
    pub current_status: Option<String>,
    /// Display-friendly stage derived from `current_status`. `null` when `current_status` is absent.
    pub current_stage: Option<String>,
    /// RFC 3339 timestamp of the most recent fill event. `null` when no fills have been received.
    pub last_event_at: Option<String>,
    /// Fill events for this order, oldest-first. At most 50 rows.
    /// Authoritative only when `truth_state == "active"`.
    pub rows: Vec<OrderTimelineRow>,
}

// ---------------------------------------------------------------------------
// GET /api/v1/execution/transport (Batch A2)
// ---------------------------------------------------------------------------

/// One outbox or inbox transport lane summary row.
///
/// Shape matches the GUI `TransportQueueRow` interface so it can be consumed
/// without mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportQueueRow {
    /// "outbox" | "inbox"
    pub queue_id: String,
    /// "outbox" | "inbox"
    pub direction: String,
    /// "idle" | "active" | "retrying" | "pending" | "applied"
    pub status: String,
    pub depth: usize,
    /// Age of oldest item in this lane, in milliseconds.
    pub oldest_age_ms: u64,
    pub retry_count: usize,
    pub duplicate_events: usize,
    pub orphaned_claims: usize,
    pub lag_ms: Option<u64>,
    pub last_activity_at: Option<String>,
    pub notes: String,
}

/// Response for `GET /api/v1/execution/transport`.
///
/// Shape matches the GUI `TransportSummary` interface (extra fields are
/// ignored by the GUI JSON consumer).
///
/// `truth_state`:
/// - `"active"` — an execution snapshot is present; all counts are authoritative.
/// - `"no_snapshot"` — no execution snapshot (run not started or daemon freshly
///   booted); all counts are zero and must NOT be read as authoritative-zero.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTransportResponse {
    pub canonical_route: String,
    pub truth_state: String,
    /// Total non-ACKED outbox rows in the current snapshot.
    pub outbox_depth: usize,
    /// Total recent inbox event rows in the current snapshot.
    pub inbox_depth: usize,
    /// Age of the oldest CLAIMED outbox row in milliseconds; 0 if none.
    pub max_claim_age_ms: u64,
    /// Count of FAILED + AMBIGUOUS outbox rows (proxy for dispatch retries).
    pub dispatch_retries: usize,
    /// Count of CLAIMED rows stale > 30 s (proxy for orphaned claims).
    pub orphaned_claims: usize,
    /// Always 0 — duplicate detection is not derivable from the in-memory snapshot.
    pub duplicate_inbox_events: usize,
    /// Per-lane queue summaries: [outbox, inbox] when snapshot is present, [] otherwise.
    pub queues: Vec<TransportQueueRow>,
}

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/quality (Batch A2)
// ---------------------------------------------------------------------------

/// Response for `GET /api/v1/market-data/quality`.
///
/// Shape matches the GUI `MarketDataQualitySummary` interface (extra fields
/// `canonical_route`, `truth_state`, `market_data_source`, `ws_continuity`
/// are ignored by the GUI JSON consumer).
///
/// `truth_state` is always `"active"` — this route derives from daemon in-memory
/// state which is always available.  Use `overall_health` to distinguish
/// configured vs not-configured states.
///
/// `overall_health`:
/// - `"ok"` — ExternalSignalIngestion + WS Live (stream confirmed healthy).
/// - `"warning"` — ExternalSignalIngestion + WS ColdStartUnproven or NotApplicable
///   (configured but continuity not yet proven).
/// - `"critical"` — ExternalSignalIngestion + WS GapDetected (active data gap).
/// - `"not_configured"` — no market-data source is wired
///   (`StrategyMarketDataSource::NotConfigured`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataQualityResponse {
    pub canonical_route: String,
    pub truth_state: String,
    /// "ok" | "warning" | "critical" | "not_configured" — maps to GUI `HealthState`.
    pub overall_health: String,
    /// Always 0 — freshness SLA is not tracked in current implementation.
    pub freshness_sla_ms: u64,
    /// Always 0 — stale symbol count is not tracked.
    pub stale_symbol_count: usize,
    /// Always 0 — missing bar count is not tracked.
    pub missing_bar_count: usize,
    /// Always 0 — venue disagreement is not tracked.
    pub venue_disagreement_count: usize,
    /// Always 0 — strategy blocks are not tracked here.
    pub strategy_blocks: usize,
    /// Always empty — no per-venue breakdown is available from in-memory state.
    pub venues: Vec<JsonValue>,
    /// Always empty — no per-issue tracking is available from in-memory state.
    pub issues: Vec<JsonValue>,
    /// "not_configured" | "signal_ingestion_ready" — raw source label.
    pub market_data_source: String,
    /// "not_applicable" | "cold_start_unproven" | "live" | "gap_detected".
    pub ws_continuity: String,
}
