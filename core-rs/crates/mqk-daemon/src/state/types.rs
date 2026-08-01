//! Pure data types for mqk-daemon shared runtime state.
//!
//! Contains: BusMsg, BuildInfo, StatusSnapshot, ReconcileStatusSnapshot,
//! RestartTruthSnapshot, RuntimeLifecycleError, StateIntegrityGate,
//! ReconcileTruthGate, DaemonOrchestrator alias, ExecutionLoopCommand,
//! ExecutionLoopExit, ExecutionLoopHandle, OperatorAuthMode, DeploymentMode,
//! BrokerKind, BrokerSnapshotTruthSource, StrategyMarketDataSource,
//! AlpacaWsContinuityState, AcceptedArtifactProvenance.

use std::fmt;
use std::sync::Arc;

use mqk_execution::{IntegrityGate, ReconcileGate};
use mqk_integrity::IntegrityState;
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, RwLock};
use tokio::task::JoinHandle;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// BusMsg — SSE event bus payload
// ---------------------------------------------------------------------------

/// Messages broadcast over the internal event bus and surfaced as SSE events.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BusMsg {
    Heartbeat { ts_millis: i64 },
    Status(StatusSnapshot),
    LogLine { level: String, msg: String },
}

// ---------------------------------------------------------------------------
// BuildInfo
// ---------------------------------------------------------------------------

/// Static build metadata included in health / status responses.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildInfo {
    pub service: &'static str,
    pub version: &'static str,
}

// ---------------------------------------------------------------------------
// StatusSnapshot
// ---------------------------------------------------------------------------

/// Point-in-time snapshot of daemon state, returned by GET /v1/status and
/// carried inside SSE `status` events.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatusSnapshot {
    pub daemon_uptime_secs: u64,
    pub active_run_id: Option<Uuid>,
    /// "idle" | "running" | "halted" | "unknown"
    pub state: String,
    pub notes: Option<String>,
    /// Reflects `IntegrityState::is_execution_blocked()` negation: true = armed.
    pub integrity_armed: bool,
    /// Durable deadman truth for the current daemon run lifecycle.
    pub deadman_status: String,
    pub deadman_last_heartbeat_utc: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReconcileStatusSnapshot {
    pub status: String,
    pub last_run_at: Option<String>,
    pub snapshot_watermark_ms: Option<i64>,
    pub mismatched_positions: usize,
    pub mismatched_orders: usize,
    pub mismatched_fills: usize,
    pub unmatched_broker_events: usize,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestartTruthSnapshot {
    pub local_owned_run_id: Option<Uuid>,
    pub durable_active_run_id: Option<Uuid>,
    pub durable_active_without_local_ownership: bool,
}

// ---------------------------------------------------------------------------
// AcceptedArtifactProvenance — TV-01C
// ---------------------------------------------------------------------------

/// Provenance of a promoted artifact that was accepted at `start_execution_runtime`.
///
/// Stored in `AppState::accepted_artifact` and surfaced via
/// `GET /api/v1/system/run-artifact`.  Cleared on stop/halt.
///
/// `None` in AppState when no run is active, no artifact was configured, or
/// the intake outcome was not `Accepted` — all fail-closed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcceptedArtifactProvenance {
    /// Content-addressed artifact identity (sha256-derived).
    pub artifact_id: String,
    /// Artifact type string (e.g. `"signal_pack"`).
    pub artifact_type: String,
    /// Promotion stage the artifact was promoted to (e.g. `"paper"`).
    pub stage: String,
    /// Producing system identifier (e.g. `"research-py/promote.py"`).
    pub produced_by: String,
}

#[derive(Debug)]
pub enum RuntimeLifecycleError {
    ServiceUnavailable {
        fault_class: &'static str,
        message: String,
    },
    Forbidden {
        fault_class: &'static str,
        gate: String,
        message: String,
    },
    Conflict {
        fault_class: &'static str,
        message: String,
    },
    Internal {
        fault_class: &'static str,
        message: String,
    },
}

impl RuntimeLifecycleError {
    pub(crate) fn service_unavailable(
        fault_class: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self::ServiceUnavailable {
            fault_class,
            message: message.into(),
        }
    }

    pub(crate) fn forbidden(
        fault_class: &'static str,
        gate: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::Forbidden {
            fault_class,
            gate: gate.into(),
            message: message.into(),
        }
    }

    pub(crate) fn conflict(fault_class: &'static str, message: impl Into<String>) -> Self {
        Self::Conflict {
            fault_class,
            message: message.into(),
        }
    }

    pub(crate) fn internal(context: &'static str, err: impl fmt::Display) -> Self {
        Self::Internal {
            fault_class: context,
            message: format!("{context}: {err}"),
        }
    }

    pub fn fault_class(&self) -> &'static str {
        match self {
            Self::ServiceUnavailable { fault_class, .. }
            | Self::Forbidden { fault_class, .. }
            | Self::Conflict { fault_class, .. }
            | Self::Internal { fault_class, .. } => fault_class,
        }
    }
}

impl fmt::Display for RuntimeLifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ServiceUnavailable { message, .. } => f.write_str(message),
            Self::Forbidden { message, .. } => f.write_str(message),
            Self::Conflict { message, .. } => f.write_str(message),
            Self::Internal { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for RuntimeLifecycleError {}

#[derive(Clone)]
pub(crate) struct StateIntegrityGate {
    pub(crate) integrity: Arc<RwLock<IntegrityState>>,
}

impl IntegrityGate for StateIntegrityGate {
    fn is_armed(&self) -> bool {
        self.integrity
            .try_read()
            .map(|guard| !guard.is_execution_blocked())
            .unwrap_or(false)
    }
}

#[derive(Clone)]
pub(crate) struct ReconcileTruthGate {
    pub(crate) reconcile_status: Arc<RwLock<ReconcileStatusSnapshot>>,
}

impl ReconcileGate for ReconcileTruthGate {
    fn is_clean(&self) -> bool {
        self.reconcile_status
            .try_read()
            .map(|snapshot| snapshot.status == "ok")
            .unwrap_or(false)
    }
}

/// Type alias for the daemon execution orchestrator.
pub(crate) type DaemonOrchestrator = mqk_runtime::orchestrator::ExecutionOrchestrator<
    super::broker::DaemonBroker,
    StateIntegrityGate,
    mqk_runtime::runtime_risk::RuntimeRiskGate,
    ReconcileTruthGate,
    mqk_runtime::orchestrator::WallClock,
>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExecutionLoopCommand {
    Run,
    Stop,
}

#[derive(Debug)]
pub(crate) struct ExecutionLoopExit {
    pub(crate) note: Option<String>,
}

#[derive(Debug)]
pub(crate) struct ExecutionLoopHandle {
    pub(crate) run_id: Uuid,
    pub(crate) stop_tx: watch::Sender<ExecutionLoopCommand>,
    pub(crate) join_handle: JoinHandle<ExecutionLoopExit>,
}

/// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 2: explicit local
/// loop-ownership states, replacing the prior `Option<ExecutionLoopHandle>`
/// (which could only distinguish "no owner" from "owner installed" — it had
/// no way to represent "a start attempt has claimed this slot but has not
/// finished building the handle yet"). A start attempt must move
/// `Empty -> Reserved { run_id }` atomically before it does any further
/// work, then only ever install `Active(handle)` for that exact `run_id`.
///
/// `Reserved` is a genuinely distinct state from `Active`: a status reader
/// observing `Reserved` must never report the run as running (requirement
/// 10 — "Reserved is starting or omitted, never running"), and a
/// compare-and-clear for a failed run A must never clear `Reserved`/`Active`
/// for a different run B (requirement 2's "conflicting" rule).
#[derive(Debug)]
pub(crate) enum ExecutionLoopSlot {
    /// No local owner. The only state a new start attempt may reserve from.
    Empty,
    /// A start attempt for `run_id` has claimed the slot but has not yet
    /// installed a running handle. Conflicts with every other run_id,
    /// including another `Reserved` or `Active` value.
    Reserved { run_id: Uuid },
    /// The execution loop is actually running for `run_id`.
    Active(ExecutionLoopHandle),
}

/// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 7: typed failure from
/// `AppState::install_active_execution_loop_slot`, replacing what was
/// previously a panic on a duplicate/misordered call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallActiveExecutionLoopSlotError {
    /// The slot is `Empty` — `reserve_execution_loop_slot` was never called
    /// (or was already released) for this run_id.
    NoReservation,
    /// The slot is `Reserved` for a *different* run_id.
    ReservedForDifferentRun { reserved_run_id: Uuid },
    /// The slot is already `Active` — a second install was attempted.
    AlreadyActive { active_run_id: Uuid },
}

impl fmt::Display for InstallActiveExecutionLoopSlotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoReservation => {
                write!(f, "execution loop slot has no reservation to install into")
            }
            Self::ReservedForDifferentRun { reserved_run_id } => write!(
                f,
                "execution loop slot is reserved for a different run_id: {reserved_run_id}"
            ),
            Self::AlreadyActive { active_run_id } => write!(
                f,
                "execution loop slot is already active for run_id: {active_run_id}"
            ),
        }
    }
}

// NOTE: `RuntimeStartPhase` lives in `crate::daily_data_readiness` (it must
// be `pub` there — the `RuntimeStartEffects` trait it's part of is driven
// from integration tests outside this crate's `pub(crate)` boundary), not
// here. `ExecutionLoopSlot` above stays `pub(crate)` since nothing outside
// this crate needs it directly.

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperatorAuthMode {
    TokenRequired(String),
    ExplicitDevNoToken,
    MissingTokenFailClosed,
}

impl OperatorAuthMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::TokenRequired(_) => "token_required",
            Self::ExplicitDevNoToken => "explicit_dev_no_token",
            Self::MissingTokenFailClosed => "missing_token_fail_closed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DeploymentMode {
    Backtest,
    Paper,
    LiveShadow,
    LiveCapital,
}

impl DeploymentMode {
    pub fn as_db_mode(&self) -> &'static str {
        match self {
            Self::Backtest => "BACKTEST",
            Self::Paper => "PAPER",
            Self::LiveShadow => "LIVE-SHADOW",
            Self::LiveCapital => "LIVE-CAPITAL",
        }
    }

    pub fn as_api_label(&self) -> &'static str {
        match self {
            Self::Backtest => "backtest",
            Self::Paper => "paper",
            Self::LiveShadow => "live-shadow",
            Self::LiveCapital => "live-capital",
        }
    }
}

/// Typed broker implementation selector — deliberately distinct from deployment policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BrokerKind {
    /// In-process bar-driven paper fill engine (`LockedPaperBroker`).
    Paper,
    /// Alpaca v2 REST + WebSocket external broker (`AlpacaBrokerAdapter`).
    Alpaca,
}

impl BrokerKind {
    /// Canonical lowercase string for DB records, API responses, and logging.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Paper => "paper",
            Self::Alpaca => "alpaca",
        }
    }

    /// Parse from the `MQK_DAEMON_ADAPTER_ID` env-var string (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "paper" => Some(Self::Paper),
            "alpaca" => Some(Self::Alpaca),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// AP-04: BrokerSnapshotTruthSource
// ---------------------------------------------------------------------------

/// Determines how the daemon populates `broker_snapshot`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BrokerSnapshotTruthSource {
    /// Paper broker: snapshot is synthesized from local OMS + portfolio state.
    Synthetic,
    /// Alpaca (external broker): snapshot must come from the AP-03 REST fetch.
    External,
}

impl BrokerSnapshotTruthSource {
    /// Canonical lowercase string for API responses and logging.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Synthetic => "synthetic",
            Self::External => "external",
        }
    }

    /// Derive the snapshot truth source from a parsed broker kind.
    pub(crate) fn from_broker_kind(kind: Option<BrokerKind>) -> Self {
        match kind {
            Some(BrokerKind::Alpaca) => Self::External,
            Some(BrokerKind::Paper) | None => Self::Synthetic,
        }
    }
}

// ---------------------------------------------------------------------------
// AP-04B: StrategyMarketDataSource
// ---------------------------------------------------------------------------

/// Strategy market-data source policy — where strategy signals get price data.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StrategyMarketDataSource {
    /// No market-data subsystem is wired. Strategy price feeds are not available.
    NotConfigured,
    /// PT-DAY-01: External signal ingestion is wired for this deployment.
    ///
    /// Strategy signals may be posted via `POST /api/v1/strategy/signal` when
    /// an active run is present, armed, and not suppressed.  The signal producer
    /// is responsible for consuming real market data and computing the signal.
    /// The daemon accepts and enqueues the signal for broker-backed execution.
    ExternalSignalIngestion,
}

impl StrategyMarketDataSource {
    /// Health string for `market_data_health` in API responses.
    pub fn as_health_str(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::ExternalSignalIngestion => "signal_ingestion_ready",
        }
    }
}

// ---------------------------------------------------------------------------
// AP-05: AlpacaWsContinuityState
// ---------------------------------------------------------------------------

/// AP-05: Daemon-owned Alpaca websocket continuity truth.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AlpacaWsContinuityState {
    /// Broker kind is not Alpaca; websocket continuity does not apply.
    NotApplicable,
    /// Alpaca broker selected; no cursor persisted yet.
    ColdStartUnproven,
    /// WS stream was live at the last cursor persist.
    Live {
        last_message_id: String,
        last_event_at: String,
    },
    /// A continuity gap was detected.
    GapDetected {
        last_message_id: Option<String>,
        last_event_at: Option<String>,
        detail: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AutonomousRecoveryResumeSource {
    ColdStart,
    PersistedCursor,
}

impl AutonomousRecoveryResumeSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ColdStart => "cold_start",
            Self::PersistedCursor => "persisted_cursor",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AutonomousSessionTruth {
    Clear,
    StartRefused {
        detail: String,
    },
    RecoveryRetrying {
        resume_source: AutonomousRecoveryResumeSource,
        detail: String,
    },
    RecoverySucceeded {
        resume_source: AutonomousRecoveryResumeSource,
        detail: String,
    },
    RecoveryFailed {
        resume_source: AutonomousRecoveryResumeSource,
        detail: String,
    },
    /// BRK-GAP-01: A WS gap was detected during a prior session.  WS connectivity
    /// has been re-established and the cursor repaired to `Live`.  FILL/PARTIAL_FILL
    /// events from the gap window may be recovered via REST catch-up on the next
    /// orchestrator tick.  Non-fill lifecycle events (Ack, CancelAck, ReplaceAck,
    /// Reject) from the gap window are permanently unrecoverable from the Alpaca
    /// REST API.  Operator reconcile/repair is required before full lifecycle truth
    /// can be claimed.
    WsGapPartialRecovery {
        resume_source: AutonomousRecoveryResumeSource,
        detail: String,
    },
    RunEndedUnexpectedly {
        detail: String,
    },
    StopFailed {
        detail: String,
    },
    StoppedAtBoundary {
        detail: String,
    },
    /// EXEC-OBS-LIVENESS-01: the autonomous session controller task exited
    /// (panic or unexpected loop exit).  Autonomous paper execution is now
    /// UNMANAGED.  The daemon is still up but no session controller is alive.
    ControllerExited {
        detail: String,
    },
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3: the supervised completed-bar
    /// driver task permanently failed after exhausting its bounded restart
    /// budget. Autonomous completed-bar dispatch is now UNMANAGED. The
    /// daemon and the session controller are still up.
    CompletedBarDriverExited {
        detail: String,
    },
}

// ---------------------------------------------------------------------------
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: run-scoped lifecycle truth
// ---------------------------------------------------------------------------

/// Phase 7A: the single, immutable, run-scoped dynamic-selection lifecycle
/// truth committed at run start and cleared on every lifecycle exit path
/// (stop/halt/shutdown/reap/failed start). Process-local only — no durable
/// persistence in this patch (Phase 8).
///
/// Every field corresponds to exactly one `run_id`, frozen at the moment
/// `evaluate_dynamic_selection_start_gate` returned its outcome for that
/// run. A later run receives a brand-new value here (an unconditional
/// overwrite on commit), never an in-place mutation of this one — so a
/// restart can never observe a mix of two runs' selection truth.
///
/// `plan`/`host_pool` are wrapped in `Arc` so this whole struct stays
/// cheaply `Clone` (for `AppState::dynamic_selection_runtime_snapshot`)
/// even though `DynamicSelectionHostPool` itself derives neither `Clone`
/// nor `Debug` — `Arc<T>: Clone` holds regardless of `T`'s own bounds.
#[derive(Clone)]
pub struct DynamicSelectionRuntimeState {
    pub run_id: Uuid,
    pub disposition: crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition,
    pub configured_mode: mqk_portfolio::DynamicSelectionMode,
    pub effective_mode: mqk_portfolio::DynamicSelectionMode,
    pub live_lock_applied: bool,
    /// `Some` only for `ShadowAllowed`, `ShadowInvalid` (when a plan was
    /// actually built), and `PaperEnforcedAllowed`. `None` for `Off` and for
    /// any refusal reached before a plan could be built (context-incoherent,
    /// DB-unavailable, config-unavailable).
    pub plan: Option<Arc<mqk_portfolio::DynamicSelectionPlan>>,
    /// Bounded `(symbol, strategy_id, timeframe_secs)` projection of
    /// `plan`'s selected rows — the exact triples
    /// `DynamicSelectionHostPool::build` would key on. Empty for `Off` and
    /// for any disposition with no selected pair.
    pub selected_pairs: Vec<(String, String, i64)>,
    /// `Some` only for `PaperEnforcedAllowed` — the only disposition that
    /// owns a host pool. Shadow never builds one (it has no economic
    /// authority to withhold).
    pub host_pool: Option<Arc<crate::dynamic_selection_host_pool::DynamicSelectionHostPool>>,
    pub reasons: Vec<crate::dynamic_selection_start_gate::DynamicSelectionStartGateReason>,
    /// Always `false` in this patch. Bundle 7 dynamic selection is not
    /// dispatch-authoritative under any disposition here — this field exists
    /// so a future status surface never has to fabricate this invariant.
    pub approved_for_live: bool,
}

impl DynamicSelectionRuntimeState {
    pub fn selected_pair_count(&self) -> usize {
        self.selected_pairs.len()
    }

    pub fn plan_present(&self) -> bool {
        self.plan.is_some()
    }

    pub fn host_pool_present(&self) -> bool {
        self.host_pool.is_some()
    }
}

/// Manual `Debug` impl: `DynamicSelectionHostPool` itself derives neither
/// `Clone` nor `Debug` (it is not meant to be cloned or dumped whole), so
/// this prints only its presence/size — never its internal `StrategyHost`
/// contents.
impl fmt::Debug for DynamicSelectionRuntimeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DynamicSelectionRuntimeState")
            .field("run_id", &self.run_id)
            .field("disposition", &self.disposition)
            .field("configured_mode", &self.configured_mode)
            .field("effective_mode", &self.effective_mode)
            .field("live_lock_applied", &self.live_lock_applied)
            .field("plan_present", &self.plan.is_some())
            .field("selected_pairs", &self.selected_pairs)
            .field(
                "host_pool",
                &self
                    .host_pool
                    .as_ref()
                    .map(|p| format!("<present, {} host(s)>", p.len()))
                    .unwrap_or_else(|| "<absent>".to_string()),
            )
            .field("reasons", &self.reasons)
            .field("approved_for_live", &self.approved_for_live)
            .finish()
    }
}

/// Phase 7A test-only fault-injection points for the atomic start-commit
/// sequence in `AppState::start_execution_runtime` /
/// `ProductionRuntimeStartEffects`. `None` in production and for every test
/// that does not explicitly install one via
/// `AppState::set_dynamic_selection_fault_seam_for_test` — the real sequence
/// always runs unmodified. Loop-ownership-conflict cleanup is proven via the
/// real conflict path (a pre-populated `execution_loop` handle), not a seam
/// variant here, since that failure mode already exists and is directly
/// reachable without injection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DynamicSelectionLifecycleFaultSeam {
    AfterSelectionEvaluation,
    AfterRunRowCreation,
    AfterOrchestratorConstruction,
    AfterRunArmBeginInitialTick,
    AfterProcessLocalSelectionCommit,
    ImmediatelyBeforeLoopSpawn,
}

impl AlpacaWsContinuityState {
    /// Canonical lowercase status string for API responses and logging.
    pub fn as_status_str(&self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::ColdStartUnproven => "cold_start_unproven",
            Self::Live { .. } => "live",
            Self::GapDetected { .. } => "gap_detected",
        }
    }

    /// `true` only when WS continuity is explicitly proven (`Live`).
    pub fn is_continuity_proven(&self) -> bool {
        matches!(self, Self::Live { .. })
    }

    /// Derive continuity state from a raw persisted broker-cursor JSON string.
    pub fn from_cursor_json(broker_kind: Option<BrokerKind>, cursor_json: Option<&str>) -> Self {
        let Some(BrokerKind::Alpaca) = broker_kind else {
            return Self::NotApplicable;
        };
        let Some(json) = cursor_json else {
            return Self::ColdStartUnproven;
        };
        match serde_json::from_str::<mqk_broker_alpaca::types::AlpacaFetchCursor>(json) {
            Ok(cursor) => Self::from_fetch_cursor(&cursor),
            Err(e) => Self::GapDetected {
                last_message_id: None,
                last_event_at: None,
                detail: format!("broker cursor parse failed at daemon startup: {e}"),
            },
        }
    }

    pub(crate) fn from_fetch_cursor(cursor: &mqk_broker_alpaca::types::AlpacaFetchCursor) -> Self {
        // BRK-00R-02: delegate to the runtime-owned seam so continuity authority
        // lives in mqk-runtime, not duplicated here.  The daemon converts the
        // runtime-owned WsLifecycleContinuity to its own AlpacaWsContinuityState
        // (adding NotApplicable for non-Alpaca paths, handled by from_cursor_json).
        use mqk_runtime::alpaca_inbound::{ws_continuity_from_cursor, WsLifecycleContinuity};
        match ws_continuity_from_cursor(cursor) {
            WsLifecycleContinuity::ColdStartUnproven => Self::ColdStartUnproven,
            WsLifecycleContinuity::Live {
                last_message_id,
                last_event_at,
            } => Self::Live {
                last_message_id,
                last_event_at,
            },
            WsLifecycleContinuity::GapDetected {
                last_message_id,
                last_event_at,
                detail,
            } => Self::GapDetected {
                last_message_id,
                last_event_at,
                detail,
            },
        }
    }
}
