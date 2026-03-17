//! Shared runtime state for mqk-daemon.
//!
//! All types here are `Clone`-able (via `Arc` or copy). Handlers receive
//! `State<Arc<AppState>>` from Axum; this module owns daemon-local runtime
//! lifecycle control plus durable status reconstruction.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use chrono::Utc;
use mqk_broker_paper::LockedPaperBroker;
use mqk_execution::{
    oms::state_machine::{OmsEvent, OmsOrder, OrderState},
    wiring::build_gateway,
    BrokerAdapter, BrokerCancelResponse, BrokerError, BrokerEvent, BrokerInvokeToken,
    BrokerOrderMap, BrokerReplaceRequest, BrokerReplaceResponse, BrokerSubmitRequest,
    BrokerSubmitResponse, IntegrityGate, ReconcileGate,
};
use mqk_integrity::{CalendarSpec, IntegrityState};
use mqk_portfolio::{apply_entry, LedgerEntry, PortfolioState};
use mqk_reconcile::{ReconcileDiff, SnapshotFreshness, SnapshotWatermark};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::{broadcast, watch, Mutex, RwLock};
use tokio::task::JoinHandle;
use uuid::Uuid;

const DAEMON_ENGINE_ID: &str = "mqk-daemon";
const DEFAULT_DAEMON_DEPLOYMENT_MODE: &str = "paper";
const DEFAULT_DAEMON_ADAPTER_ID: &str = "paper";
const DAEMON_RUN_CONFIG_HASH_PREFIX: &str = "daemon-runtime";
const EXECUTION_LOOP_INTERVAL: Duration = Duration::from_secs(1);
const DEADMAN_TTL_SECONDS: i64 = 5;
/// DMON-06: background reconcile tick interval.  30 s gives the execution loop
/// sufficient time to populate execution_snapshot before the first tick fires.
const RECONCILE_TICK_INTERVAL: Duration = Duration::from_secs(30);
const DEV_ALLOW_NO_OPERATOR_TOKEN_ENV: &str = "MQK_DEV_ALLOW_NO_OPERATOR_TOKEN";
const DAEMON_DEPLOYMENT_MODE_ENV: &str = "MQK_DAEMON_DEPLOYMENT_MODE";
const DAEMON_ADAPTER_ID_ENV: &str = "MQK_DAEMON_ADAPTER_ID";

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
    fn service_unavailable(fault_class: &'static str, message: impl Into<String>) -> Self {
        Self::ServiceUnavailable {
            fault_class,
            message: message.into(),
        }
    }

    fn forbidden(
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

    fn conflict(fault_class: &'static str, message: impl Into<String>) -> Self {
        Self::Conflict {
            fault_class,
            message: message.into(),
        }
    }

    fn internal(context: &'static str, err: impl fmt::Display) -> Self {
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
struct StateIntegrityGate {
    integrity: Arc<RwLock<IntegrityState>>,
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
struct ReconcileTruthGate {
    reconcile_status: Arc<RwLock<ReconcileStatusSnapshot>>,
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
///
/// The broker type parameter is `DaemonBroker` — the enum-based dispatch seam
/// introduced in AP-02.  This alias is intentionally no longer paper-specific;
/// any broker variant in `DaemonBroker` can be used here without changing this alias.
type DaemonOrchestrator = mqk_runtime::orchestrator::ExecutionOrchestrator<
    DaemonBroker,
    StateIntegrityGate,
    mqk_runtime::runtime_risk::RuntimeRiskGate,
    ReconcileTruthGate,
    mqk_runtime::orchestrator::WallClock,
>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExecutionLoopCommand {
    Run,
    Stop,
}

#[derive(Debug)]
struct ExecutionLoopExit {
    note: Option<String>,
}

#[derive(Debug)]
struct ExecutionLoopHandle {
    run_id: Uuid,
    stop_tx: watch::Sender<ExecutionLoopCommand>,
    join_handle: JoinHandle<ExecutionLoopExit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperatorAuthMode {
    TokenRequired(String),
    ExplicitDevNoToken,
    MissingTokenFailClosed,
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
///
/// `DeploymentMode` encodes *operating policy* (paper / live-shadow / live-capital).
/// `BrokerKind` encodes *which broker implementation* satisfies that policy.
/// The two are separate so the same policy can be satisfied by different adapters
/// (e.g. `Paper` policy + `Alpaca` broker for a future live-shadow mode) without
/// conflating mode selection with adapter construction.
///
/// Only `Paper` is currently wired into daemon execution.  `Alpaca` is defined so
/// the daemon can parse and reject it with a typed, explicit error rather than a
/// raw string comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BrokerKind {
    /// In-process bar-driven paper fill engine (`LockedPaperBroker`).
    Paper,
    /// Alpaca v2 REST + WebSocket external broker (`AlpacaBrokerAdapter`).
    /// Wiring into daemon execution is deferred to AP-02+.
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
    /// Returns `None` for unrecognised values so callers can fail-closed explicitly
    /// without resorting to raw string comparisons.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "paper" => Some(Self::Paper),
            "alpaca" => Some(Self::Alpaca),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// DaemonBroker — enum-dispatch seam (AP-02)
// ---------------------------------------------------------------------------

/// Broker dispatch seam for the daemon execution orchestrator.
///
/// Each variant wraps a concrete broker adapter.  The enum satisfies the
/// `BrokerAdapter` type parameter of `DaemonOrchestrator`, making the
/// orchestrator broker-agnostic at the type level.
///
/// Only `Paper` is currently constructable via `build_daemon_broker`.
/// Adding a new variant here and extending `build_daemon_broker` is the
/// only change required to wire a new broker into daemon execution.
pub(crate) enum DaemonBroker {
    Paper(LockedPaperBroker),
}

impl fmt::Debug for DaemonBroker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Paper(_) => f.write_str("DaemonBroker::Paper"),
        }
    }
}

impl BrokerAdapter for DaemonBroker {
    fn submit_order(
        &self,
        req: BrokerSubmitRequest,
        token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerSubmitResponse, BrokerError> {
        match self {
            Self::Paper(b) => b.submit_order(req, token),
        }
    }

    fn cancel_order(
        &self,
        order_id: &str,
        token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerCancelResponse, BrokerError> {
        match self {
            Self::Paper(b) => b.cancel_order(order_id, token),
        }
    }

    fn replace_order(
        &self,
        req: BrokerReplaceRequest,
        token: &BrokerInvokeToken,
    ) -> std::result::Result<BrokerReplaceResponse, BrokerError> {
        match self {
            Self::Paper(b) => b.replace_order(req, token),
        }
    }

    fn fetch_events(
        &self,
        cursor: Option<&str>,
        token: &BrokerInvokeToken,
    ) -> std::result::Result<(Vec<BrokerEvent>, Option<String>), BrokerError> {
        match self {
            Self::Paper(b) => b.fetch_events(cursor, token),
        }
    }
}

/// Construct the `DaemonBroker` variant for the given `BrokerKind`.
///
/// This is the **single construction seam** for broker adapters in the daemon.
/// Nothing else in the daemon instantiates a concrete broker adapter directly;
/// all paths go through this function so that adding/removing broker support
/// only requires changing this one site.
///
/// Fails closed for any kind that is not currently wired into daemon execution.
fn build_daemon_broker(
    broker_kind: Option<BrokerKind>,
) -> Result<DaemonBroker, RuntimeLifecycleError> {
    match broker_kind {
        Some(BrokerKind::Paper) => Ok(DaemonBroker::Paper(LockedPaperBroker::new())),
        Some(BrokerKind::Alpaca) => Err(RuntimeLifecycleError::service_unavailable(
            "runtime.start_refused.broker_not_wired",
            "broker 'alpaca' is not yet wired into daemon execution",
        )),
        None => Err(RuntimeLifecycleError::service_unavailable(
            "runtime.start_refused.broker_unrecognised",
            "unrecognised broker adapter; cannot construct execution broker",
        )),
    }
}

#[derive(Clone, Debug)]
pub struct DeploymentReadiness {
    pub start_allowed: bool,
    pub blocker: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RuntimeSelection {
    pub deployment_mode: DeploymentMode,
    /// Parsed broker implementation kind; `None` when the adapter-id string is
    /// unrecognised (treated as fail-closed by `deployment_mode_readiness`).
    /// Use this field for typed dispatch — do NOT use `adapter_id` for logic.
    pub broker_kind: Option<BrokerKind>,
    /// Raw adapter identifier string (e.g. `"paper"`, `"alpaca"`).
    /// Used for DB run records, API responses, and operator-visible logging.
    /// Does not drive broker construction; use `broker_kind` for typed dispatch.
    pub adapter_id: String,
    pub run_config_hash: String,
    pub readiness: DeploymentReadiness,
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

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

/// Cloneable (Arc) handle shared across all Axum handlers.
#[derive(Clone)]
pub struct AppState {
    /// Broadcast bus for SSE.
    pub bus: broadcast::Sender<BusMsg>,
    /// Static build metadata.
    pub build: BuildInfo,
    /// Durable DB connection for control/lease surfaces.
    pub db: Option<PgPool>,
    /// Stable identity for this daemon process.
    pub node_id: String,
    /// Mutable status cache. Routes reconstruct truth from DB + owned loop and
    /// only use this for notes / the last published status snapshot.
    pub status: Arc<RwLock<StatusSnapshot>>,
    /// Integrity engine state (arm / disarm).
    pub integrity: Arc<RwLock<IntegrityState>>,
    /// Latest broker snapshot known to the daemon (in-memory for now).
    pub broker_snapshot: Arc<RwLock<Option<mqk_schemas::BrokerSnapshot>>>,
    /// Latest execution pipeline snapshot from the owned loop.
    pub execution_snapshot: Arc<RwLock<Option<mqk_runtime::observability::ExecutionSnapshot>>>,
    /// Per-order side cache (order_id → reconcile Side) populated from outbox
    /// order_json at bootstrap and refreshed every execution tick (DMON-05).
    pub local_order_sides: Arc<RwLock<BTreeMap<String, mqk_reconcile::Side>>>,
    /// Latest monotonic reconcile result known to the daemon.
    reconcile_status: Arc<RwLock<ReconcileStatusSnapshot>>,
    /// Operator auth posture for privileged routes.
    pub operator_auth: OperatorAuthMode,
    /// Runtime adapter/deployment selection resolved from config/env at bootstrap.
    runtime_selection: RuntimeSelection,
    /// The single daemon-owned execution loop handle, if any.
    execution_loop: Arc<Mutex<Option<ExecutionLoopHandle>>>,
    /// Serializes start/stop/halt transitions so the daemon never spawns duplicates.
    lifecycle_op: Arc<Mutex<()>>,
    /// Authoritative exchange calendar spec derived from deployment mode at construction.
    /// `NyseWeekdays` for live-equity modes; `AlwaysOn` for paper/backtest.
    calendar_spec: CalendarSpec,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    /// Create in-process application state for local development and tests.
    ///
    /// The real daemon startup path uses [`Self::new_with_db`], which resolves
    /// environment-derived operator auth and now fails closed by default.
    pub fn new() -> Self {
        Self::new_inner(OperatorAuthMode::ExplicitDevNoToken, None)
    }

    /// Create application state with an explicit operator-auth mode.
    pub fn new_with_operator_auth(operator_auth: OperatorAuthMode) -> Self {
        Self::new_inner(operator_auth, None)
    }

    /// Create application state with an explicit operator token.
    ///
    /// `None` is an explicit dev/test opt-in to no-token operator access; it
    /// does not represent the environment-derived production default.
    pub fn new_with_token(token: Option<String>) -> Self {
        let operator_auth = match token {
            Some(token) => OperatorAuthMode::TokenRequired(token),
            None => OperatorAuthMode::ExplicitDevNoToken,
        };
        Self::new_inner(operator_auth, None)
    }

    /// Create application state with a live DB pool.
    pub fn new_with_db(db: PgPool) -> Self {
        Self::new_inner(operator_auth_mode_from_env(), Some(db))
    }

    /// Create application state with an explicit deployment mode for tests.
    ///
    /// Named with `_for_test_` to signal intent; production code must derive
    /// the deployment mode from environment via [`Self::new_with_db`].
    pub fn new_for_test_with_mode(mode: DeploymentMode) -> Self {
        let mut state = Self::new_inner(OperatorAuthMode::ExplicitDevNoToken, None);
        // Override runtime_selection and calendar_spec to use the specified mode.
        // broker_kind is retained from the default Paper selection — the test helper
        // overrides policy (mode) while keeping the adapter unchanged.
        state.runtime_selection = RuntimeSelection {
            deployment_mode: mode,
            broker_kind: state.runtime_selection.broker_kind,
            adapter_id: state.runtime_selection.adapter_id.clone(),
            run_config_hash: state.runtime_selection.run_config_hash.clone(),
            readiness: state.runtime_selection.readiness.clone(),
        };
        state.calendar_spec = match mode {
            DeploymentMode::LiveShadow | DeploymentMode::LiveCapital => CalendarSpec::NyseWeekdays,
            DeploymentMode::Paper | DeploymentMode::Backtest => CalendarSpec::AlwaysOn,
        };
        state
    }

    fn new_inner(operator_auth: OperatorAuthMode, db: Option<PgPool>) -> Self {
        let (bus, _rx) = broadcast::channel::<BusMsg>(1024);

        let build = BuildInfo {
            service: "mqk-daemon",
            version: env!("CARGO_PKG_VERSION"),
        };

        let initial_status = StatusSnapshot {
            daemon_uptime_secs: uptime_secs(),
            active_run_id: None,
            state: "idle".to_string(),
            notes: Some("runtime idle; explicit arm and start required".to_string()),
            integrity_armed: false,
            deadman_status: "inactive".to_string(),
            deadman_last_heartbeat_utc: None,
        };

        let mut boot_integrity = IntegrityState::new();
        boot_integrity.disarmed = true;

        let runtime_selection = runtime_selection_from_env();

        // Derive calendar spec from deployment mode.  Live-equity modes use
        // the authoritative NYSE calendar; paper/backtest are always-on.
        let calendar_spec = match runtime_selection.deployment_mode {
            DeploymentMode::LiveShadow | DeploymentMode::LiveCapital => CalendarSpec::NyseWeekdays,
            DeploymentMode::Paper | DeploymentMode::Backtest => CalendarSpec::AlwaysOn,
        };

        Self {
            bus,
            node_id: default_node_id(build.service),
            build,
            db,
            status: Arc::new(RwLock::new(initial_status)),
            integrity: Arc::new(RwLock::new(boot_integrity)),
            broker_snapshot: Arc::new(RwLock::new(None)),
            execution_snapshot: Arc::new(RwLock::new(None)),
            local_order_sides: Arc::new(RwLock::new(BTreeMap::new())),
            reconcile_status: Arc::new(RwLock::new(initial_reconcile_status())),
            operator_auth,
            runtime_selection,
            execution_loop: Arc::new(Mutex::new(None)),
            lifecycle_op: Arc::new(Mutex::new(())),
            calendar_spec,
        }
    }

    pub fn operator_auth_mode(&self) -> &OperatorAuthMode {
        &self.operator_auth
    }

    pub fn runtime_selection(&self) -> &RuntimeSelection {
        &self.runtime_selection
    }

    pub fn deployment_mode(&self) -> DeploymentMode {
        self.runtime_selection.deployment_mode
    }

    /// Authoritative exchange calendar spec for this deployment mode.
    pub fn calendar_spec(&self) -> CalendarSpec {
        self.calendar_spec
    }

    pub fn adapter_id(&self) -> &str {
        &self.runtime_selection.adapter_id
    }

    pub fn run_config_hash(&self) -> &str {
        &self.runtime_selection.run_config_hash
    }

    pub fn deployment_readiness(&self) -> &DeploymentReadiness {
        &self.runtime_selection.readiness
    }

    pub async fn current_reconcile_snapshot(&self) -> ReconcileStatusSnapshot {
        if let Some(db) = self.db.as_ref() {
            if let Ok(Some(durable)) = mqk_db::load_reconcile_status_state(db).await {
                return ReconcileStatusSnapshot {
                    status: durable.status,
                    last_run_at: durable.last_run_at_utc.map(|ts| ts.to_rfc3339()),
                    snapshot_watermark_ms: durable.snapshot_watermark_ms,
                    mismatched_positions: durable.mismatched_positions.max(0) as usize,
                    mismatched_orders: durable.mismatched_orders.max(0) as usize,
                    mismatched_fills: durable.mismatched_fills.max(0) as usize,
                    unmatched_broker_events: durable.unmatched_broker_events.max(0) as usize,
                    note: durable.note,
                };
            }
        }
        self.reconcile_status.read().await.clone()
    }

    pub async fn current_execution_snapshot(
        &self,
    ) -> Option<mqk_runtime::observability::ExecutionSnapshot> {
        self.execution_snapshot.read().await.clone()
    }

    pub async fn current_broker_snapshot(&self) -> Option<mqk_schemas::BrokerSnapshot> {
        self.broker_snapshot.read().await.clone()
    }

    pub async fn current_local_order_sides(&self) -> BTreeMap<String, mqk_reconcile::Side> {
        self.local_order_sides.read().await.clone()
    }

    pub async fn restart_truth_snapshot(
        &self,
    ) -> Result<RestartTruthSnapshot, RuntimeLifecycleError> {
        let local_owned_run_id = self.active_owned_run_id().await;
        let durable_active_run_id = match self.db.as_ref() {
            Some(db) => mqk_db::fetch_active_run_for_engine(
                db,
                DAEMON_ENGINE_ID,
                self.deployment_mode().as_db_mode(),
            )
            .await
            .map_err(|err| {
                RuntimeLifecycleError::internal("restart active-run lookup failed", err)
            })?
            .map(|run| run.run_id),
            None => None,
        };

        let durable_active_without_local_ownership =
            durable_active_run_id.is_some() && local_owned_run_id != durable_active_run_id;

        Ok(RestartTruthSnapshot {
            local_owned_run_id,
            durable_active_run_id,
            durable_active_without_local_ownership,
        })
    }

    pub async fn current_status_snapshot(&self) -> Result<StatusSnapshot, RuntimeLifecycleError> {
        let reaped = self.reap_finished_execution_loop().await?;
        let reaped_note = reaped.and_then(|exit| exit.note);
        let local_owned_run_id = self.active_owned_run_id().await;
        let integrity = self.integrity.read().await;
        let mut integrity_armed = !integrity.is_execution_blocked();
        let mut locally_halted = integrity.halted;
        drop(integrity);

        if let Some(db) = self.db.as_ref() {
            if let Ok(Some((state, reason))) = mqk_db::load_arm_state(db).await {
                integrity_armed = state == "ARMED";
                locally_halted = matches!(reason.as_deref(), Some("OperatorHalt"));
            }
        }
        let cached_notes = self.status.read().await.notes.clone();

        let snapshot = match self.db.as_ref() {
            Some(db) => {
                let latest = mqk_db::fetch_latest_run_for_engine(
                    db,
                    DAEMON_ENGINE_ID,
                    self.deployment_mode().as_db_mode(),
                )
                .await
                .map_err(|err| {
                    RuntimeLifecycleError::internal(
                        "current_status_snapshot run lookup failed",
                        err,
                    )
                })?;
                match latest {
                    Some(run) => match run.status {
                        mqk_db::RunStatus::Running | mqk_db::RunStatus::Armed => {
                            let deadman = self.deadman_truth_for_run(run.run_id).await?;
                            match local_owned_run_id {
                                Some(local_run_id) if local_run_id == run.run_id => StatusSnapshot {
                                    daemon_uptime_secs: uptime_secs(),
                                    active_run_id: Some(run.run_id),
                                    state: "running".to_string(),
                                    notes: Some("daemon owns active execution loop".to_string()),
                                    integrity_armed,
                                    deadman_status: deadman.status,
                                    deadman_last_heartbeat_utc: deadman.last_heartbeat_utc,
                                },
                                Some(local_run_id) => StatusSnapshot {
                                    daemon_uptime_secs: uptime_secs(),
                                    active_run_id: Some(run.run_id),
                                    state: "unknown".to_string(),
                                    notes: Some(format!(
                                        "durable run {durable_run} is active but local ownership points to {local_run_id}",
                                        durable_run = run.run_id
                                    )),
                                    integrity_armed,
                                    deadman_status: deadman.status,
                                    deadman_last_heartbeat_utc: deadman.last_heartbeat_utc,
                                },
                                None => StatusSnapshot {
                                    daemon_uptime_secs: uptime_secs(),
                                    active_run_id: Some(run.run_id),
                                    state: "unknown".to_string(),
                                    notes: Some(
                                        "durable run is active but this daemon does not own a live execution loop"
                                            .to_string(),
                                    ),
                                    integrity_armed,
                                    deadman_status: deadman.status,
                                    deadman_last_heartbeat_utc: deadman.last_heartbeat_utc,
                                },
                            }
                        }
                        mqk_db::RunStatus::Halted => StatusSnapshot {
                            daemon_uptime_secs: uptime_secs(),
                            active_run_id: Some(run.run_id),
                            state: "halted".to_string(),
                            notes: reaped_note
                                .clone()
                                .or_else(|| Some("durable run halted".to_string())),
                            integrity_armed,
                            deadman_status: "expired".to_string(),
                            deadman_last_heartbeat_utc: run
                                .last_heartbeat_utc
                                .map(|ts| ts.to_rfc3339()),
                        },
                        mqk_db::RunStatus::Created | mqk_db::RunStatus::Stopped => {
                            StatusSnapshot {
                                daemon_uptime_secs: uptime_secs(),
                                active_run_id: None,
                                state: if local_owned_run_id.is_some() {
                                    "unknown".to_string()
                                } else if locally_halted {
                                    "halted".to_string()
                                } else {
                                    "idle".to_string()
                                },
                                notes: if local_owned_run_id.is_some() {
                                    Some("local execution loop present but durable run is not active".to_string())
                                } else {
                                    reaped_note.clone().or(cached_notes)
                                },
                                integrity_armed,
                                deadman_status: "inactive".to_string(),
                                deadman_last_heartbeat_utc: run
                                    .last_heartbeat_utc
                                    .map(|ts| ts.to_rfc3339()),
                            }
                        }
                    },
                    None => StatusSnapshot {
                        daemon_uptime_secs: uptime_secs(),
                        active_run_id: None,
                        state: if local_owned_run_id.is_some() {
                            "unknown".to_string()
                        } else if locally_halted {
                            "halted".to_string()
                        } else {
                            "idle".to_string()
                        },
                        notes: if local_owned_run_id.is_some() {
                            Some(
                                "local execution loop present but no durable daemon run exists"
                                    .to_string(),
                            )
                        } else {
                            reaped_note.clone().or(cached_notes)
                        },
                        integrity_armed,
                        deadman_status: "inactive".to_string(),
                        deadman_last_heartbeat_utc: None,
                    },
                }
            }
            None => StatusSnapshot {
                daemon_uptime_secs: uptime_secs(),
                active_run_id: None,
                state: if local_owned_run_id.is_some() {
                    "running".to_string()
                } else if locally_halted {
                    "halted".to_string()
                } else {
                    "idle".to_string()
                },
                notes: if local_owned_run_id.is_some() {
                    Some("daemon owns active execution loop".to_string())
                } else {
                    reaped_note.or(cached_notes)
                },
                integrity_armed,
                deadman_status: "unavailable".to_string(),
                deadman_last_heartbeat_utc: None,
            },
        };

        self.publish_status(snapshot.clone()).await;
        Ok(snapshot)
    }

    pub async fn start_execution_runtime(
        self: &Arc<Self>,
    ) -> Result<StatusSnapshot, RuntimeLifecycleError> {
        let _op = self.lifecycle_op.lock().await;
        self.reap_finished_execution_loop().await?;

        if !self.deployment_readiness().start_allowed {
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.start_refused.deployment_mode_unproven",
                "deployment_mode",
                self.deployment_readiness()
                    .blocker
                    .clone()
                    .unwrap_or_else(|| "deployment mode is not start-ready".to_string()),
            ));
        }

        if self.integrity.read().await.is_execution_blocked() {
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.control_refusal.integrity_disarmed",
                "integrity_armed",
                "GATE_REFUSED: integrity disarmed or halted; arm integrity first",
            ));
        }

        if let Some(run_id) = self.active_owned_run_id().await {
            return Err(RuntimeLifecycleError::conflict(
                "runtime.control_refusal.already_owned",
                format!("runtime already active under local ownership: {run_id}"),
            ));
        }

        let db = self.db_pool()?;
        if let Some(active) = mqk_db::fetch_active_run_for_engine(
            &db,
            DAEMON_ENGINE_ID,
            self.deployment_mode().as_db_mode(),
        )
        .await
        .map_err(|err| RuntimeLifecycleError::internal("start active-run lookup failed", err))?
        {
            return Err(RuntimeLifecycleError::conflict(
                "runtime.truth_mismatch.durable_active_without_local_owner",
                format!(
                    "durable active run exists without local ownership: {}",
                    active.run_id
                ),
            ));
        }

        let latest = mqk_db::fetch_latest_run_for_engine(
            &db,
            DAEMON_ENGINE_ID,
            self.deployment_mode().as_db_mode(),
        )
        .await
        .map_err(|err| RuntimeLifecycleError::internal("start latest-run lookup failed", err))?;

        let run_id = match latest.as_ref() {
            Some(run) => match run.status {
                mqk_db::RunStatus::Created | mqk_db::RunStatus::Stopped => run.run_id,
                mqk_db::RunStatus::Halted => {
                    return Err(RuntimeLifecycleError::conflict(
                        "runtime.start_refused.halted_lifecycle",
                        format!(
                            "durable run {} is halted; operator must clear the halted lifecycle before starting again",
                            run.run_id
                        ),
                    ))
                }
                mqk_db::RunStatus::Armed | mqk_db::RunStatus::Running => {
                    return Err(RuntimeLifecycleError::conflict(
                        "runtime.start_refused.durable_run_active",
                        format!("durable run {} is already active", run.run_id),
                    ))
                }
            },
            None => {
                let run_id = self.next_daemon_run_id(&db).await?;
                mqk_db::insert_run(
                    &db,
                    &mqk_db::NewRun {
                        run_id,
                        engine_id: DAEMON_ENGINE_ID.to_string(),
                        mode: self.deployment_mode().as_db_mode().to_string(),
                        started_at_utc: Utc::now(),
                        git_hash: "UNKNOWN".to_string(),
                        config_hash: self.run_config_hash().to_string(),
                        config_json: serde_json::json!({
                            "runtime": "mqk-daemon",
                            "adapter": self.adapter_id(),
                            "mode": self.deployment_mode().as_db_mode(),
                        }),
                        host_fingerprint: self.node_id.clone(),
                    },
                )
                .await
                .map_err(|err| RuntimeLifecycleError::internal("start insert_run failed", err))?;
                run_id
            }
        };

        let mut orchestrator = self
            .build_execution_orchestrator(db.clone(), run_id)
            .await?;

        if let Err(err) = mqk_db::arm_run(&db, run_id).await {
            let _ = orchestrator.release_runtime_leadership().await;
            return Err(RuntimeLifecycleError::internal("start arm_run failed", err));
        }
        if let Err(err) = mqk_db::begin_run(&db, run_id).await {
            let _ = orchestrator.release_runtime_leadership().await;
            return Err(RuntimeLifecycleError::internal(
                "start begin_run failed",
                err,
            ));
        }
        if let Err(err) = mqk_db::heartbeat_run(&db, run_id, Utc::now()).await {
            let _ = orchestrator.release_runtime_leadership().await;
            return Err(RuntimeLifecycleError::internal(
                "start initial heartbeat failed",
                err,
            ));
        }
        if let Err(err) = orchestrator.tick().await {
            let message = err.to_string();
            let _ = orchestrator.release_runtime_leadership().await;
            if message.contains("RUNTIME_LEASE") {
                return Err(RuntimeLifecycleError::conflict(
                    "runtime.start_refused.service_unavailable",
                    format!("runtime leader lease unavailable: {message}"),
                ));
            }
            return Err(RuntimeLifecycleError::internal(
                "start initial tick failed",
                err,
            ));
        }

        // DMON-06: Pre-populate execution_snapshot from the initial orchestrator
        // state (after the first tick) so the reconcile tick has a valid non-empty
        // local snapshot on its first fire rather than falling back to empty.
        if let Ok(initial_snapshot) = orchestrator.snapshot().await {
            *self.execution_snapshot.write().await = Some(initial_snapshot);
        }

        let handle = spawn_execution_loop(Arc::clone(self), orchestrator, run_id);
        {
            let mut lock = self.execution_loop.lock().await;
            if lock.is_some() {
                return Err(RuntimeLifecycleError::conflict(
                    "runtime.start_refused.local_ownership_conflict",
                    "runtime ownership changed while starting; refusing duplicate loop",
                ));
            }
            *lock = Some(handle);
        }

        // DMON-06: Spawn background reconcile tick so local order-drift results
        // are published to AppState.reconcile_status and surfaced via the
        // /api/v1/reconcile/status and /api/v1/system/status endpoints.
        {
            let snap_arc = Arc::clone(&self.execution_snapshot);
            let sides_arc = Arc::clone(&self.local_order_sides);
            let broker_arc = Arc::clone(&self.broker_snapshot);
            let local_fn = move || {
                let snapshot = snap_arc.try_read().ok().and_then(|g| g.clone());
                if let Some(snapshot) = snapshot {
                    let sides = sides_arc.try_read().map(|g| g.clone()).unwrap_or_default();
                    reconcile_local_snapshot_from_runtime_with_sides(&snapshot, &sides)
                } else {
                    mqk_reconcile::LocalSnapshot::empty()
                }
            };
            let broker_fn = move || {
                let schema = broker_arc.try_read().ok().and_then(|g| g.clone())?;
                reconcile_broker_snapshot_from_schema(&schema).ok()
            };
            spawn_reconcile_tick(
                Arc::clone(self),
                local_fn,
                broker_fn,
                RECONCILE_TICK_INTERVAL,
            );
        }

        let snapshot = StatusSnapshot {
            daemon_uptime_secs: uptime_secs(),
            active_run_id: Some(run_id),
            state: "running".to_string(),
            notes: Some("daemon owns active execution loop".to_string()),
            integrity_armed: self.integrity_armed().await,
            deadman_status: "healthy".to_string(),
            deadman_last_heartbeat_utc: Some(Utc::now().to_rfc3339()),
        };
        self.publish_status(snapshot.clone()).await;
        Ok(snapshot)
    }

    pub async fn stop_execution_runtime(
        self: &Arc<Self>,
    ) -> Result<StatusSnapshot, RuntimeLifecycleError> {
        let _op = self.lifecycle_op.lock().await;
        self.reap_finished_execution_loop().await?;
        let handle = match self.take_execution_loop_for_control().await? {
            Some(handle) => handle,
            None => {
                if let Some(db) = self.db.as_ref() {
                    if let Some(active) = mqk_db::fetch_active_run_for_engine(
                        db,
                        DAEMON_ENGINE_ID,
                        self.deployment_mode().as_db_mode(),
                    )
                    .await
                    .map_err(|err| {
                        RuntimeLifecycleError::internal("stop active-run lookup failed", err)
                    })? {
                        return Err(RuntimeLifecycleError::conflict(
                            "runtime.truth_mismatch.durable_active_without_local_owner",
                            format!(
                                "durable active run exists without local ownership: {}",
                                active.run_id
                            ),
                        ));
                    }
                }
                return self.current_status_snapshot().await;
            }
        };

        let run_id = handle.run_id;
        let _ = handle.stop_tx.send(ExecutionLoopCommand::Stop);
        let _ = handle
            .join_handle
            .await
            .map_err(|err| RuntimeLifecycleError::internal("stop join failed", err))?;

        let db = self.db_pool()?;
        let run = mqk_db::fetch_run(&db, run_id)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("stop fetch_run failed", err))?;
        if matches!(
            run.status,
            mqk_db::RunStatus::Armed | mqk_db::RunStatus::Running
        ) {
            mqk_db::stop_run(&db, run_id)
                .await
                .map_err(|err| RuntimeLifecycleError::internal("stop_run failed", err))?;
        }

        let snapshot = self.current_status_snapshot().await?;
        Ok(snapshot)
    }

    pub async fn halt_execution_runtime(
        self: &Arc<Self>,
    ) -> Result<StatusSnapshot, RuntimeLifecycleError> {
        let _op = self.lifecycle_op.lock().await;
        self.reap_finished_execution_loop().await?;

        let handle = self.take_execution_loop_for_control().await?;
        if handle.is_none() {
            if let Some(db) = self.db.as_ref() {
                if let Some(active) = mqk_db::fetch_active_run_for_engine(
                    db,
                    DAEMON_ENGINE_ID,
                    self.deployment_mode().as_db_mode(),
                )
                .await
                .map_err(|err| {
                    RuntimeLifecycleError::internal("halt active-run lookup failed", err)
                })? {
                    return Err(RuntimeLifecycleError::conflict(
                        "runtime.truth_mismatch.durable_active_without_local_owner",
                        format!(
                            "durable active run exists without local ownership: {}",
                            active.run_id
                        ),
                    ));
                }
            }
        }

        {
            let mut integrity = self.integrity.write().await;
            integrity.disarmed = true;
            integrity.halted = true;
        }

        let db = self.db_pool()?;
        if let Some(handle) = handle {
            let run_id = handle.run_id;
            let _ = handle.stop_tx.send(ExecutionLoopCommand::Stop);
            let _ = handle
                .join_handle
                .await
                .map_err(|err| RuntimeLifecycleError::internal("halt join failed", err))?;

            mqk_db::halt_run(&db, run_id, Utc::now())
                .await
                .map_err(|err| RuntimeLifecycleError::internal("halt_run failed", err))?;
        }
        mqk_db::persist_arm_state_canonical(
            &db,
            mqk_db::ArmState::Disarmed,
            Some(mqk_db::DisarmReason::OperatorHalt),
        )
        .await
        .map_err(|err| RuntimeLifecycleError::internal("persist_arm_state failed", err))?;

        let snapshot = StatusSnapshot {
            daemon_uptime_secs: uptime_secs(),
            active_run_id: self.current_status_snapshot().await?.active_run_id,
            state: "halted".to_string(),
            notes: Some("operator halt asserted; execution loop disarmed".to_string()),
            integrity_armed: false,
            deadman_status: "expired".to_string(),
            deadman_last_heartbeat_utc: None,
        };
        self.publish_status(snapshot.clone()).await;
        Ok(snapshot)
    }

    pub async fn stop_for_shutdown(self: &Arc<Self>) {
        if let Some(handle) = self.take_execution_loop_for_shutdown().await {
            let run_id = handle.run_id;
            let _ = handle.stop_tx.send(ExecutionLoopCommand::Stop);
            match handle.join_handle.await {
                Ok(_) => {
                    if let Some(db) = self.db.as_ref() {
                        if let Ok(run) = mqk_db::fetch_run(db, run_id).await {
                            if matches!(
                                run.status,
                                mqk_db::RunStatus::Armed | mqk_db::RunStatus::Running
                            ) {
                                if let Err(err) = mqk_db::stop_run(db, run_id).await {
                                    tracing::warn!("shutdown stop_run failed for {run_id}: {err}");
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!("shutdown join failed for {run_id}: {err}");
                }
            }
        }
    }

    async fn integrity_armed(&self) -> bool {
        !self.integrity.read().await.is_execution_blocked()
    }

    fn db_pool(&self) -> Result<PgPool, RuntimeLifecycleError> {
        self.db.clone().ok_or_else(|| {
            RuntimeLifecycleError::service_unavailable(
                "runtime.start_refused.service_unavailable",
                "runtime DB is not configured on this daemon",
            )
        })
    }

    async fn next_daemon_run_id(&self, db: &PgPool) -> Result<Uuid, RuntimeLifecycleError> {
        let generation: i64 = sqlx::query_scalar(
            r#"
            SELECT COALESCE(COUNT(*), 0)::bigint + 1
              FROM runs
             WHERE engine_id = $1
               AND mode = $2
            "#,
        )
        .bind(DAEMON_ENGINE_ID)
        .bind(self.deployment_mode().as_db_mode())
        .fetch_one(db)
        .await
        .map_err(|err| RuntimeLifecycleError::internal("next_daemon_run_id failed", err))?;

        Ok(Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            format!(
                "mqk-daemon.run.v2|{}|{}|{}|{}",
                self.node_id,
                DAEMON_ENGINE_ID,
                self.deployment_mode().as_db_mode(),
                generation
            )
            .as_bytes(),
        ))
    }

    /// DMON-03/04: Recover OMS order map, per-order side cache, and portfolio
    /// from durable DB truth (outbox submitted rows + applied inbox events).
    ///
    /// The orchestrator's Phase 3 will separately process UNAPPLIED inbox rows
    /// (crash window), so there is no double-apply risk.
    async fn recover_oms_and_portfolio(
        db: &PgPool,
        run_id: Uuid,
        initial_equity_micros: i64,
    ) -> Result<
        (
            BTreeMap<String, OmsOrder>,
            BTreeMap<String, mqk_reconcile::Side>,
            PortfolioState,
        ),
        RuntimeLifecycleError,
    > {
        let submitted = mqk_db::outbox_load_submitted_for_run(db, run_id)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("outbox_load_submitted_for_run", err))?;
        let applied = mqk_db::inbox_load_all_applied_for_run(db, run_id)
            .await
            .map_err(|err| {
                RuntimeLifecycleError::internal("inbox_load_all_applied_for_run", err)
            })?;

        // Build OMS orders and side map from submitted outbox rows.
        let mut oms_orders: BTreeMap<String, OmsOrder> = BTreeMap::new();
        let mut sides: BTreeMap<String, mqk_reconcile::Side> = BTreeMap::new();
        for row in &submitted {
            let Some(symbol) = outbox_json_symbol(&row.order_json) else {
                continue;
            };
            let Some(qty) = outbox_json_qty(&row.order_json) else {
                continue;
            };
            let side = outbox_json_side(&row.order_json);
            let order_id = row.idempotency_key.clone();
            sides.insert(order_id.clone(), side);
            oms_orders.insert(order_id.clone(), OmsOrder::new(&order_id, symbol, qty));
        }

        // Reconstruct portfolio starting from initial equity.
        let mut portfolio = PortfolioState::new(initial_equity_micros);

        // Apply APPLIED inbox events to advance OMS state and portfolio.
        for row in &applied {
            let event: BrokerEvent = match serde_json::from_value(row.message_json.clone()) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let internal_id = event.internal_order_id().to_string();
            let oms_event = broker_event_to_oms_event(&event);
            if let Some(order) = oms_orders.get_mut(&internal_id) {
                // Ignore transition errors; the orchestrator's Phase 3 will handle
                // unapplied events in the crash window.
                let _ = order.apply(&oms_event, Some(&row.broker_message_id));
            }
            if let Some(fill) = broker_event_to_portfolio_fill(&event) {
                apply_entry(&mut portfolio, LedgerEntry::Fill(fill));
            }
        }

        // Remove terminal orders from both maps.
        oms_orders.retain(|_, o| !o.state.is_terminal());
        sides.retain(|order_id, _| oms_orders.contains_key(order_id));

        Ok((oms_orders, sides, portfolio))
    }

    async fn build_execution_orchestrator(
        &self,
        db: PgPool,
        run_id: Uuid,
    ) -> Result<DaemonOrchestrator, RuntimeLifecycleError> {
        let run = mqk_db::fetch_run(&db, run_id)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("fetch_run failed", err))?;
        let initial_equity_micros = run
            .config_json
            .pointer("/risk/initial_equity_micros")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);

        // DMON-03 + DMON-04: Recover OMS orders, side cache, and portfolio from
        // durable DB truth (submitted outbox rows + applied inbox events).
        let (oms_orders, recovered_sides, portfolio) =
            Self::recover_oms_and_portfolio(&db, run_id, initial_equity_micros).await?;

        // Seed the shared side cache so reconcile closures have side info immediately.
        {
            let mut sides_lock = self.local_order_sides.write().await;
            *sides_lock = recovered_sides.clone();
        }

        // DMON-01: If broker_snapshot is absent, synthesize one from recovered DB
        // truth so the reconcile gate has a consistent starting point.  For paper
        // mode the broker's view IS the local OMS, so the synthesis is exact.
        let broker_seed = {
            let broker_snapshot_guard = self.broker_snapshot.read().await;
            if let Some(existing) = broker_snapshot_guard.clone() {
                existing
            } else {
                drop(broker_snapshot_guard);
                let now = Utc::now();
                let synth = synthesize_paper_broker_snapshot(
                    &oms_orders,
                    &recovered_sides,
                    &portfolio,
                    now,
                );
                *self.broker_snapshot.write().await = Some(synth.clone());
                synth
            }
        };

        let mut order_map = BrokerOrderMap::new();
        let existing = mqk_db::broker_map_load(&db)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("broker_map_load failed", err))?;
        for (internal_id, broker_id) in existing {
            order_map.register(&internal_id, &broker_id);
        }

        let broker_cursor = mqk_db::load_broker_cursor(&db, self.adapter_id())
            .await
            .map_err(|err| RuntimeLifecycleError::internal("load_broker_cursor failed", err))?;

        let daemon_broker = build_daemon_broker(self.runtime_selection.broker_kind)?;

        let gateway = build_gateway(
            daemon_broker,
            StateIntegrityGate {
                integrity: Arc::clone(&self.integrity),
            },
            mqk_runtime::runtime_risk::RuntimeRiskGate::from_run_config(
                &run.config_json,
                initial_equity_micros,
                0,
                0,
            ),
            ReconcileTruthGate {
                reconcile_status: Arc::clone(&self.reconcile_status),
            },
        );

        let broker_snapshots = Arc::clone(&self.broker_snapshot);
        let broker_seed_reconcile =
            reconcile_broker_snapshot_from_schema(&broker_seed).map_err(|err| {
                RuntimeLifecycleError::service_unavailable(
                    "runtime.start_refused.service_unavailable",
                    err.to_string(),
                )
            })?;

        // DMON-02: Allow absent execution_snapshot; use an empty local snapshot
        // seed rather than refusing to start.
        let local_seed_reconcile = {
            let local_snapshot_guard = self.execution_snapshot.read().await;
            if let Some(snap) = local_snapshot_guard.clone() {
                let sides = self.local_order_sides.read().await;
                reconcile_local_snapshot_from_runtime_with_sides(&snap, &sides)
            } else {
                mqk_reconcile::LocalSnapshot::empty()
            }
        };

        // DMON-05: Closures read from the shared side cache so that each
        // reconcile tick uses the current order-side mapping.
        let local_snapshots = Arc::clone(&self.execution_snapshot);
        let side_cache_for_local = Arc::clone(&self.local_order_sides);
        let local_snapshot_provider = move || {
            let Some(snapshot) = local_snapshots
                .try_read()
                .ok()
                .and_then(|snapshot| snapshot.clone())
            else {
                return local_seed_reconcile.clone();
            };
            let sides = side_cache_for_local
                .try_read()
                .map(|g| g.clone())
                .unwrap_or_default();
            reconcile_local_snapshot_from_runtime_with_sides(&snapshot, &sides)
        };

        let broker_snapshot_provider = move || {
            let Some(schema_snapshot) = broker_snapshots
                .try_read()
                .ok()
                .and_then(|snapshot| snapshot.clone())
            else {
                return broker_seed_reconcile.clone();
            };

            reconcile_broker_snapshot_from_schema(&schema_snapshot)
                .unwrap_or_else(|_| broker_seed_reconcile.clone())
        };

        Ok(mqk_runtime::orchestrator::ExecutionOrchestrator::new(
            db,
            gateway,
            order_map,
            oms_orders,
            portfolio,
            run_id,
            self.node_id.clone(),
            self.adapter_id(),
            broker_cursor,
            mqk_runtime::orchestrator::WallClock,
            Box::new(local_snapshot_provider),
            Box::new(broker_snapshot_provider),
        ))
    }

    async fn active_owned_run_id(&self) -> Option<Uuid> {
        let lock = self.execution_loop.lock().await;
        lock.as_ref()
            .filter(|handle| !handle.join_handle.is_finished())
            .map(|handle| handle.run_id)
    }

    pub async fn locally_owned_run_id(&self) -> Option<Uuid> {
        self.active_owned_run_id().await
    }

    async fn take_execution_loop_for_control(
        &self,
    ) -> Result<Option<ExecutionLoopHandle>, RuntimeLifecycleError> {
        let handle = {
            let mut lock = self.execution_loop.lock().await;
            lock.take()
        };

        match handle {
            Some(handle) if !handle.join_handle.is_finished() => Ok(Some(handle)),
            Some(handle) => {
                let exit = handle
                    .join_handle
                    .await
                    .map_err(|err| RuntimeLifecycleError::internal("loop reap failed", err))?;
                self.publish_status(StatusSnapshot {
                    daemon_uptime_secs: uptime_secs(),
                    active_run_id: None,
                    state: "idle".to_string(),
                    notes: exit.note,
                    integrity_armed: self.integrity_armed().await,
                    deadman_status: "inactive".to_string(),
                    deadman_last_heartbeat_utc: None,
                })
                .await;
                Ok(None)
            }
            None => Ok(None),
        }
    }

    async fn take_execution_loop_for_shutdown(&self) -> Option<ExecutionLoopHandle> {
        let mut lock = self.execution_loop.lock().await;
        lock.take()
    }

    async fn reap_finished_execution_loop(
        &self,
    ) -> Result<Option<ExecutionLoopExit>, RuntimeLifecycleError> {
        let handle = {
            let mut lock = self.execution_loop.lock().await;
            if lock
                .as_ref()
                .is_some_and(|handle| handle.join_handle.is_finished())
            {
                lock.take()
            } else {
                None
            }
        };

        match handle {
            Some(handle) => {
                let exit = handle
                    .join_handle
                    .await
                    .map_err(|err| RuntimeLifecycleError::internal("loop join failed", err))?;
                Ok(Some(exit))
            }
            None => Ok(None),
        }
    }

    pub async fn publish_status(&self, snapshot: StatusSnapshot) {
        {
            let mut status = self.status.write().await;
            *status = snapshot.clone();
        }
        let _ = self.bus.send(BusMsg::Status(snapshot));
    }

    pub async fn publish_reconcile_snapshot(&self, snapshot: ReconcileStatusSnapshot) {
        if let Some(db) = self.db.as_ref() {
            let _ = mqk_db::persist_reconcile_status_state(
                db,
                &mqk_db::PersistReconcileStatusState {
                    status: &snapshot.status,
                    last_run_at_utc: snapshot
                        .last_run_at
                        .as_deref()
                        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                        .map(|ts| ts.with_timezone(&Utc)),
                    snapshot_watermark_ms: snapshot.snapshot_watermark_ms,
                    mismatched_positions: snapshot.mismatched_positions as i32,
                    mismatched_orders: snapshot.mismatched_orders as i32,
                    mismatched_fills: snapshot.mismatched_fills as i32,
                    unmatched_broker_events: snapshot.unmatched_broker_events as i32,
                    note: snapshot.note.as_deref(),
                    updated_at_utc: Utc::now(),
                },
            )
            .await;
        }
        let mut status = self.reconcile_status.write().await;
        *status = snapshot;
    }
}

// ---------------------------------------------------------------------------
// Test-only helpers (pub so integration tests in tests/ can reach them;
// never called from production code paths).
// ---------------------------------------------------------------------------

impl AppState {
    /// Inject a never-finishing fake execution loop so that
    /// `current_status_snapshot()` returns `state == "running"` without
    /// a real DB or orchestrator.  Used by PROD-02 route scenario tests.
    ///
    /// **Never call this outside of tests.**
    pub async fn inject_running_loop_for_test(&self, run_id: Uuid) {
        let (stop_tx, _stop_rx) = watch::channel(ExecutionLoopCommand::Run);
        let join_handle: JoinHandle<ExecutionLoopExit> = tokio::spawn(async {
            // Sleep for a day — the test will complete long before this fires.
            tokio::time::sleep(std::time::Duration::from_secs(86_400)).await;
            ExecutionLoopExit { note: None }
        });
        let handle = ExecutionLoopHandle {
            run_id,
            stop_tx,
            join_handle,
        };
        let mut lock = self.execution_loop.lock().await;
        *lock = Some(handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_selection_defaults_to_paper_ready() {
        let selection = runtime_selection_from_env_values(None, None);
        assert_eq!(selection.deployment_mode, DeploymentMode::Paper);
        // broker_kind is the typed separation; adapter_id is the display string.
        assert_eq!(selection.broker_kind, Some(BrokerKind::Paper));
        assert_eq!(selection.adapter_id, "paper");
        assert!(selection.readiness.start_allowed);
        assert!(selection.readiness.blocker.is_none());
    }

    #[test]
    fn runtime_selection_fail_closes_unproven_modes() {
        let selection = runtime_selection_from_env_values(Some("live-capital"), Some("alpaca"));
        assert_eq!(selection.deployment_mode, DeploymentMode::LiveCapital);
        // BrokerKind is parsed even when the mode is unproven.
        assert_eq!(selection.broker_kind, Some(BrokerKind::Alpaca));
        assert!(!selection.readiness.start_allowed);
        assert!(selection
            .readiness
            .blocker
            .as_deref()
            .unwrap_or("")
            .contains("unsupported/unproven"));
    }

    #[test]
    fn runtime_selection_fail_closes_paper_with_nonpaper_adapter() {
        // Paper policy + Alpaca broker: recognised combination, but not yet supported.
        // Must fail-closed via typed dispatch, not string comparison.
        let selection = runtime_selection_from_env_values(Some("paper"), Some("alpaca"));
        assert_eq!(selection.deployment_mode, DeploymentMode::Paper);
        assert_eq!(selection.broker_kind, Some(BrokerKind::Alpaca));
        assert!(!selection.readiness.start_allowed);
        assert!(selection
            .readiness
            .blocker
            .as_deref()
            .unwrap_or("")
            .contains("requires broker 'paper'"));
    }

    #[test]
    fn unknown_broker_adapter_string_is_fail_closed() {
        // Unrecognised adapter-id (not "paper" or "alpaca") must fail-closed.
        // broker_kind is None; deployment_mode_readiness treats None as fail-closed.
        let selection =
            runtime_selection_from_env_values(Some("paper"), Some("interactive-brokers"));
        assert_eq!(selection.deployment_mode, DeploymentMode::Paper);
        assert_eq!(
            selection.broker_kind, None,
            "unrecognised adapter yields None broker_kind"
        );
        assert_eq!(selection.adapter_id, "interactive-brokers");
        assert!(!selection.readiness.start_allowed);
        assert!(selection
            .readiness
            .blocker
            .as_deref()
            .is_some_and(|msg| !msg.is_empty()));
    }

    // ── build_daemon_broker factory tests ────────────────────────────────

    #[test]
    fn build_daemon_broker_paper_succeeds() {
        let result = build_daemon_broker(Some(BrokerKind::Paper));
        assert!(
            result.is_ok(),
            "Paper broker must construct successfully; got: {:?}",
            result.as_ref().err().map(|e| e.to_string())
        );
        // Confirm the variant is Paper.
        let DaemonBroker::Paper(_) = result.unwrap();
    }

    #[test]
    fn build_daemon_broker_alpaca_is_fail_closed() {
        let result = build_daemon_broker(Some(BrokerKind::Alpaca));
        assert!(
            result.is_err(),
            "Alpaca broker must fail closed (not yet wired)"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not yet wired"),
            "error must explain alpaca is not wired; got: {err_msg}"
        );
    }

    #[test]
    fn build_daemon_broker_unknown_is_fail_closed() {
        let result = build_daemon_broker(None);
        assert!(result.is_err(), "Unknown broker (None) must fail closed");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("unrecognised"),
            "error must mention unrecognised; got: {err_msg}"
        );
    }

    #[test]
    fn reconcile_truth_gate_allows_only_ok_status() {
        let reconcile_status = Arc::new(RwLock::new(initial_reconcile_status()));
        let gate = ReconcileTruthGate {
            reconcile_status: Arc::clone(&reconcile_status),
        };

        assert!(!gate.is_clean(), "unknown reconcile must fail closed");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        rt.block_on(async {
            reconcile_status.write().await.status = "dirty".to_string();
        });
        assert!(!gate.is_clean(), "dirty reconcile must block dispatch");

        rt.block_on(async {
            reconcile_status.write().await.status = "stale".to_string();
        });
        assert!(!gate.is_clean(), "stale reconcile must block dispatch");

        rt.block_on(async {
            reconcile_status.write().await.status = "ok".to_string();
        });
        assert!(gate.is_clean(), "ok reconcile may allow dispatch");
    }
}

#[derive(Debug, Clone)]
struct DeadmanTruth {
    status: String,
    last_heartbeat_utc: Option<String>,
}

impl AppState {
    async fn deadman_truth_for_run(
        &self,
        run_id: Uuid,
    ) -> Result<DeadmanTruth, RuntimeLifecycleError> {
        let db = self.db_pool()?;
        let now = Utc::now();
        let halted = mqk_db::enforce_deadman_or_halt(&db, run_id, DEADMAN_TTL_SECONDS, now)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("deadman enforce failed", err))?;
        let run = mqk_db::fetch_run(&db, run_id)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("deadman fetch_run failed", err))?;

        if halted {
            mqk_db::persist_arm_state_canonical(
                &db,
                mqk_db::ArmState::Disarmed,
                Some(mqk_db::DisarmReason::DeadmanExpired),
            )
            .await
            .map_err(|err| {
                RuntimeLifecycleError::internal("deadman persist_arm_state failed", err)
            })?;
            {
                let mut integrity = self.integrity.write().await;
                integrity.disarmed = true;
                integrity.halted = true;
            }
        }

        let status = match run.status {
            mqk_db::RunStatus::Running => {
                let expired = mqk_db::deadman_expired(&db, run_id, DEADMAN_TTL_SECONDS, now)
                    .await
                    .map_err(|err| RuntimeLifecycleError::internal("deadman check failed", err))?;
                if expired {
                    "expired"
                } else {
                    "healthy"
                }
            }
            mqk_db::RunStatus::Halted => "expired",
            mqk_db::RunStatus::Armed | mqk_db::RunStatus::Created | mqk_db::RunStatus::Stopped => {
                "inactive"
            }
        }
        .to_string();

        Ok(DeadmanTruth {
            status,
            last_heartbeat_utc: run.last_heartbeat_utc.map(|ts| ts.to_rfc3339()),
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_node_id(service: &str) -> String {
    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "UNKNOWN_HOST".to_string());
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "UNKNOWN_USER".to_string());
    format!("{service}|{host}|{user}|pid={}", std::process::id())
}

pub fn operator_auth_mode_from_env_values(
    operator_token: Option<&str>,
    dev_allow_no_token: Option<&str>,
) -> OperatorAuthMode {
    if let Some(token) = operator_token
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        return OperatorAuthMode::TokenRequired(token.to_string());
    }

    #[cfg(debug_assertions)]
    {
        if dev_allow_no_token
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
        {
            return OperatorAuthMode::ExplicitDevNoToken;
        }
    }

    #[cfg(not(debug_assertions))]
    {
        let _ = dev_allow_no_token;
    }

    OperatorAuthMode::MissingTokenFailClosed
}

fn operator_auth_mode_from_env() -> OperatorAuthMode {
    let operator_token = std::env::var("MQK_OPERATOR_TOKEN").ok();
    let dev_allow_no_token = std::env::var(DEV_ALLOW_NO_OPERATOR_TOKEN_ENV).ok();
    operator_auth_mode_from_env_values(operator_token.as_deref(), dev_allow_no_token.as_deref())
}

fn runtime_selection_from_env_values(
    mode: Option<&str>,
    adapter_id: Option<&str>,
) -> RuntimeSelection {
    let deployment_mode = parse_deployment_mode(mode).unwrap_or_else(|| {
        parse_deployment_mode(Some(DEFAULT_DAEMON_DEPLOYMENT_MODE))
            .expect("default deployment mode must be valid")
    });
    let adapter = adapter_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_DAEMON_ADAPTER_ID)
        .to_ascii_lowercase();

    // Parse the adapter string into the typed BrokerKind.  Unknown strings yield
    // None, which deployment_mode_readiness treats as fail-closed.  The raw
    // adapter string is preserved for DB records and operator-visible responses.
    let broker_kind = BrokerKind::parse(&adapter);

    let readiness = deployment_mode_readiness(deployment_mode, broker_kind);

    RuntimeSelection {
        deployment_mode,
        broker_kind,
        adapter_id: adapter,
        run_config_hash: format!(
            "{}-{}-{}-v1",
            DAEMON_RUN_CONFIG_HASH_PREFIX,
            deployment_mode.as_api_label(),
            if readiness.start_allowed {
                "ready"
            } else {
                "blocked"
            }
        ),
        readiness,
    }
}

fn runtime_selection_from_env() -> RuntimeSelection {
    let mode = std::env::var(DAEMON_DEPLOYMENT_MODE_ENV).ok();
    let adapter_id = std::env::var(DAEMON_ADAPTER_ID_ENV).ok();
    runtime_selection_from_env_values(mode.as_deref(), adapter_id.as_deref())
}

fn parse_deployment_mode(raw: Option<&str>) -> Option<DeploymentMode> {
    let value = raw?.trim().to_ascii_lowercase();
    match value.as_str() {
        "paper" => Some(DeploymentMode::Paper),
        "backtest" => Some(DeploymentMode::Backtest),
        "live-shadow" | "live_shadow" | "liveshadow" => Some(DeploymentMode::LiveShadow),
        "live-capital" | "live_capital" | "livecapital" | "live" => {
            Some(DeploymentMode::LiveCapital)
        }
        _ => None,
    }
}

/// Evaluate whether the (policy, broker) combination may be started.
///
/// This is the single canonical gate that maps typed `(DeploymentMode, BrokerKind)`
/// pairs to allowed/blocked states.  String comparisons are intentionally absent;
/// all dispatch is through typed enums.
///
/// Supported combinations:
///   Paper + Paper  →  allowed (current only)
///
/// All other combinations, including future Alpaca combinations, remain
/// fail-closed until explicitly wired and proven in a later AP patch.
fn deployment_mode_readiness(
    mode: DeploymentMode,
    broker_kind: Option<BrokerKind>,
) -> DeploymentReadiness {
    match (mode, broker_kind) {
        // ── Only supported combination ────────────────────────────────────
        (DeploymentMode::Paper, Some(BrokerKind::Paper)) => DeploymentReadiness {
            start_allowed: true,
            blocker: None,
        },
        // ── Paper mode with a recognised but unsupported broker ───────────
        (DeploymentMode::Paper, Some(other_broker)) => DeploymentReadiness {
            start_allowed: false,
            blocker: Some(format!(
                "deployment mode 'paper' currently requires broker 'paper'; \
                 got broker '{}' — combination not yet supported",
                other_broker.as_str()
            )),
        },
        // ── Paper mode with an unrecognised adapter-id string ─────────────
        (DeploymentMode::Paper, None) => DeploymentReadiness {
            start_allowed: false,
            blocker: Some(
                "deployment mode 'paper' currently requires broker 'paper'; \
                 set MQK_DAEMON_ADAPTER_ID to a recognised broker adapter"
                    .to_string(),
            ),
        },
        // ── Unproven deployment modes (all broker kinds) ──────────────────
        (
            DeploymentMode::Backtest | DeploymentMode::LiveShadow | DeploymentMode::LiveCapital,
            _,
        ) => DeploymentReadiness {
            start_allowed: false,
            blocker: Some(format!(
                "deployment mode '{}' is unsupported/unproven in current daemon architecture; refusing start fail-closed",
                mode.as_api_label()
            )),
        },
    }
}

/// Monotonically increasing uptime since first call (process lifetime).
pub fn uptime_secs() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs()
}

/// Spawn a background task that emits a heartbeat SSE every `interval`.
pub fn spawn_heartbeat(bus: broadcast::Sender<BusMsg>, interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            let ts = Utc::now().timestamp_millis();
            let _ = bus.send(BusMsg::Heartbeat { ts_millis: ts });
        }
    });
}

fn initial_reconcile_status() -> ReconcileStatusSnapshot {
    ReconcileStatusSnapshot {
        status: "unknown".to_string(),
        last_run_at: None,
        snapshot_watermark_ms: None,
        mismatched_positions: 0,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some(
            "reconcile truth not yet proven; fail closed until a fresh broker snapshot is accepted"
                .to_string(),
        ),
    }
}

fn parse_signed_qty(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = trimmed.parse::<i64>() {
        return Some(value);
    }

    let (sign, magnitude) = if let Some(rest) = trimmed.strip_prefix('-') {
        (-1_i64, rest)
    } else if let Some(rest) = trimmed.strip_prefix('+') {
        (1_i64, rest)
    } else {
        (1_i64, trimmed)
    };

    let (whole, frac) = magnitude.split_once('.')?;
    if frac.chars().any(|c| c != '0') {
        return None;
    }
    let base = whole.parse::<i64>().ok()?;
    Some(sign * base)
}

fn reconcile_side_from_schema(raw: &str) -> mqk_reconcile::Side {
    if raw.eq_ignore_ascii_case("sell") {
        mqk_reconcile::Side::Sell
    } else {
        mqk_reconcile::Side::Buy
    }
}

fn reconcile_order_status_from_schema(raw: &str) -> mqk_reconcile::OrderStatus {
    if raw.eq_ignore_ascii_case("new") {
        mqk_reconcile::OrderStatus::New
    } else if raw.eq_ignore_ascii_case("accepted") {
        mqk_reconcile::OrderStatus::Accepted
    } else if raw.eq_ignore_ascii_case("partially_filled")
        || raw.eq_ignore_ascii_case("partial_fill")
    {
        mqk_reconcile::OrderStatus::PartiallyFilled
    } else if raw.eq_ignore_ascii_case("filled") {
        mqk_reconcile::OrderStatus::Filled
    } else if raw.eq_ignore_ascii_case("canceled") || raw.eq_ignore_ascii_case("cancelled") {
        mqk_reconcile::OrderStatus::Canceled
    } else if raw.eq_ignore_ascii_case("rejected") {
        mqk_reconcile::OrderStatus::Rejected
    } else {
        mqk_reconcile::OrderStatus::Unknown
    }
}

/// DMON-05: like `reconcile_local_snapshot_from_runtime` but also includes
/// active orders, using the side cache to supply the required `Side` field.
pub(crate) fn reconcile_local_snapshot_from_runtime_with_sides(
    snapshot: &mqk_runtime::observability::ExecutionSnapshot,
    sides: &BTreeMap<String, mqk_reconcile::Side>,
) -> mqk_reconcile::LocalSnapshot {
    let positions = snapshot
        .portfolio
        .positions
        .iter()
        .map(|pos| (pos.symbol.clone(), pos.net_qty))
        .collect();

    let orders = snapshot
        .active_orders
        .iter()
        .map(|order| {
            let side = sides
                .get(&order.order_id)
                .cloned()
                .unwrap_or(mqk_reconcile::Side::Buy);
            let status = oms_execution_status_to_reconcile(&order.status);
            let snap = mqk_reconcile::OrderSnapshot {
                order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side,
                qty: order.total_qty,
                filled_qty: order.filled_qty,
                status,
            };
            (order.order_id.clone(), snap)
        })
        .collect();

    mqk_reconcile::LocalSnapshot { orders, positions }
}

fn oms_execution_status_to_reconcile(status: &str) -> mqk_reconcile::OrderStatus {
    let raw = status.to_ascii_lowercase();
    if raw == "filled" {
        mqk_reconcile::OrderStatus::Filled
    } else if raw == "canceled" || raw == "cancelled" {
        mqk_reconcile::OrderStatus::Canceled
    } else if raw == "rejected" {
        mqk_reconcile::OrderStatus::Rejected
    } else {
        mqk_reconcile::OrderStatus::Unknown
    }
}

fn outbox_json_symbol(json: &serde_json::Value) -> Option<String> {
    json.get("symbol")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn outbox_json_qty(json: &serde_json::Value) -> Option<i64> {
    // Accept both "qty" and "quantity" keys; value must be positive.
    let raw = json.get("qty").or_else(|| json.get("quantity"))?;
    let n = raw.as_i64()?;
    if n > 0 {
        Some(n)
    } else {
        None
    }
}

fn outbox_json_side(json: &serde_json::Value) -> mqk_reconcile::Side {
    match json.get("side").and_then(|v| v.as_str()) {
        Some(s) if s.eq_ignore_ascii_case("sell") => mqk_reconcile::Side::Sell,
        _ => mqk_reconcile::Side::Buy,
    }
}

fn broker_event_to_oms_event(event: &BrokerEvent) -> OmsEvent {
    match event {
        BrokerEvent::Ack { .. } => OmsEvent::Ack,
        BrokerEvent::PartialFill { delta_qty, .. } => OmsEvent::PartialFill {
            delta_qty: *delta_qty,
        },
        BrokerEvent::Fill { delta_qty, .. } => OmsEvent::Fill {
            delta_qty: *delta_qty,
        },
        BrokerEvent::CancelAck { .. } => OmsEvent::CancelAck,
        BrokerEvent::CancelReject { .. } => OmsEvent::CancelReject,
        BrokerEvent::ReplaceAck { new_total_qty, .. } => OmsEvent::ReplaceAck {
            new_total_qty: *new_total_qty,
        },
        BrokerEvent::ReplaceReject { .. } => OmsEvent::ReplaceReject,
        BrokerEvent::Reject { .. } => OmsEvent::Reject,
    }
}

fn broker_event_to_portfolio_fill(event: &BrokerEvent) -> Option<mqk_portfolio::Fill> {
    match event {
        BrokerEvent::Fill {
            symbol,
            side,
            delta_qty,
            price_micros,
            fee_micros,
            ..
        }
        | BrokerEvent::PartialFill {
            symbol,
            side,
            delta_qty,
            price_micros,
            fee_micros,
            ..
        } => {
            let portfolio_side = match side {
                mqk_execution::types::Side::Buy => mqk_portfolio::Side::Buy,
                mqk_execution::types::Side::Sell => mqk_portfolio::Side::Sell,
            };
            Some(mqk_portfolio::Fill {
                symbol: symbol.clone(),
                side: portfolio_side,
                qty: *delta_qty,
                price_micros: *price_micros,
                fee_micros: *fee_micros,
            })
        }
        _ => None,
    }
}

fn oms_state_to_broker_status(state: &OrderState) -> &'static str {
    match state {
        OrderState::Open => "new",
        OrderState::PartiallyFilled => "partially_filled",
        OrderState::Filled => "filled",
        OrderState::CancelPending => "pending_cancel",
        OrderState::Cancelled => "canceled",
        OrderState::ReplacePending => "pending_replace",
        OrderState::Rejected => "rejected",
    }
}

/// DMON-01: Synthesize a `BrokerSnapshot` from recovered OMS + portfolio truth.
/// Used when no prior broker snapshot exists in memory (cold start / first tick).
fn synthesize_paper_broker_snapshot(
    oms_orders: &BTreeMap<String, OmsOrder>,
    sides: &BTreeMap<String, mqk_reconcile::Side>,
    portfolio: &PortfolioState,
    now: chrono::DateTime<Utc>,
) -> mqk_schemas::BrokerSnapshot {
    let orders: Vec<mqk_schemas::BrokerOrder> = oms_orders
        .values()
        .map(|order| {
            let side_str = sides
                .get(&order.order_id)
                .map(|s| match s {
                    mqk_reconcile::Side::Buy => "buy",
                    mqk_reconcile::Side::Sell => "sell",
                })
                .unwrap_or("buy");
            mqk_schemas::BrokerOrder {
                broker_order_id: order.order_id.clone(),
                client_order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side: side_str.to_string(),
                r#type: "market".to_string(),
                status: oms_state_to_broker_status(&order.state).to_string(),
                qty: order.total_qty.to_string(),
                limit_price: None,
                stop_price: None,
                created_at_utc: now,
            }
        })
        .collect();

    let positions: Vec<mqk_schemas::BrokerPosition> = portfolio
        .positions
        .iter()
        .filter_map(|(symbol, pos)| {
            let net: i64 = pos.lots.iter().map(|l| l.qty_signed).sum();
            if net == 0 {
                None
            } else {
                Some(mqk_schemas::BrokerPosition {
                    symbol: symbol.clone(),
                    qty: net.to_string(),
                    avg_price: "0".to_string(),
                })
            }
        })
        .collect();

    let cash_whole = portfolio.cash_micros / 1_000_000;
    let account = mqk_schemas::BrokerAccount {
        equity: cash_whole.to_string(),
        cash: cash_whole.to_string(),
        currency: "USD".to_string(),
    };

    mqk_schemas::BrokerSnapshot {
        captured_at_utc: now,
        account,
        orders,
        fills: vec![],
        positions,
    }
}

/// DMON-05 (tick): Synthesize a paper-broker snapshot from the latest execution
/// snapshot and side cache.  Called every execution tick to keep broker_snapshot
/// in sync with the live OMS so the reconcile gate never sees local vs. broker
/// drift in paper mode.
fn synthesize_broker_snapshot_from_execution(
    snapshot: &mqk_runtime::observability::ExecutionSnapshot,
    sides: &BTreeMap<String, mqk_reconcile::Side>,
    now: chrono::DateTime<Utc>,
) -> mqk_schemas::BrokerSnapshot {
    let orders: Vec<mqk_schemas::BrokerOrder> = snapshot
        .active_orders
        .iter()
        .map(|order| {
            let side_str = sides
                .get(&order.order_id)
                .map(|s| match s {
                    mqk_reconcile::Side::Buy => "buy",
                    mqk_reconcile::Side::Sell => "sell",
                })
                .unwrap_or("buy");
            mqk_schemas::BrokerOrder {
                broker_order_id: order
                    .broker_order_id
                    .clone()
                    .unwrap_or_else(|| order.order_id.clone()),
                client_order_id: order.order_id.clone(),
                symbol: order.symbol.clone(),
                side: side_str.to_string(),
                r#type: "market".to_string(),
                status: order.status.to_ascii_lowercase(),
                qty: order.total_qty.to_string(),
                limit_price: None,
                stop_price: None,
                created_at_utc: now,
            }
        })
        .collect();

    let positions: Vec<mqk_schemas::BrokerPosition> = snapshot
        .portfolio
        .positions
        .iter()
        .map(|pos| mqk_schemas::BrokerPosition {
            symbol: pos.symbol.clone(),
            qty: pos.net_qty.to_string(),
            avg_price: "0".to_string(),
        })
        .collect();

    let cash_whole = snapshot.portfolio.cash_micros / 1_000_000;
    let account = mqk_schemas::BrokerAccount {
        equity: cash_whole.to_string(),
        cash: cash_whole.to_string(),
        currency: "USD".to_string(),
    };

    mqk_schemas::BrokerSnapshot {
        captured_at_utc: now,
        account,
        orders,
        fills: vec![],
        positions,
    }
}

pub(crate) fn reconcile_broker_snapshot_from_schema(
    snapshot: &mqk_schemas::BrokerSnapshot,
) -> Result<mqk_reconcile::BrokerSnapshot, &'static str> {
    let fetched_at_ms = snapshot.captured_at_utc.timestamp_millis();
    if fetched_at_ms <= 0 {
        return Err("broker snapshot timestamp is invalid; refusing ambiguous broker truth");
    }

    let mut positions = BTreeMap::new();
    for position in &snapshot.positions {
        let qty = parse_signed_qty(&position.qty).ok_or(
            "broker snapshot contains non-integer position qty; refusing ambiguous broker truth",
        )?;
        positions.insert(position.symbol.clone(), qty);
    }

    let mut orders = BTreeMap::new();
    for order in &snapshot.orders {
        let qty = parse_signed_qty(&order.qty).ok_or(
            "broker snapshot contains non-integer order qty; refusing ambiguous broker truth",
        )?;
        let order_id = if order.client_order_id.trim().is_empty() {
            order.broker_order_id.clone()
        } else {
            order.client_order_id.clone()
        };
        orders.insert(
            order_id.clone(),
            mqk_reconcile::OrderSnapshot::new(
                order_id,
                order.symbol.clone(),
                reconcile_side_from_schema(&order.side),
                qty,
                0,
                reconcile_order_status_from_schema(&order.status),
            ),
        );
    }

    Ok(mqk_reconcile::BrokerSnapshot {
        orders,
        positions,
        fetched_at_ms,
    })
}

fn reconcile_unknown_status(note: impl Into<String>) -> ReconcileStatusSnapshot {
    ReconcileStatusSnapshot {
        note: Some(note.into()),
        ..initial_reconcile_status()
    }
}

fn reconcile_last_run_at(fetched_at_ms: i64) -> Option<String> {
    chrono::DateTime::<Utc>::from_timestamp_millis(fetched_at_ms).map(|ts| ts.to_rfc3339())
}

fn reconcile_counts(report: &mqk_reconcile::ReconcileReport) -> (usize, usize, usize, usize) {
    let mut mismatched_positions = 0;
    let mut mismatched_orders = 0;
    let mut mismatched_fills = 0;
    let mut unmatched_broker_events = 0;

    for diff in &report.diffs {
        match diff {
            ReconcileDiff::PositionQtyMismatch { .. } => mismatched_positions += 1,
            ReconcileDiff::OrderMismatch { .. }
            | ReconcileDiff::LocalOrderMissingAtBroker { .. } => mismatched_orders += 1,
            ReconcileDiff::UnknownOrder { .. } => {
                mismatched_orders += 1;
                unmatched_broker_events += 1;
            }
            ReconcileDiff::UnknownBrokerFill { .. } => {
                mismatched_fills += 1;
                unmatched_broker_events += 1;
            }
        }
    }

    (
        mismatched_positions,
        mismatched_orders,
        mismatched_fills,
        unmatched_broker_events,
    )
}

fn reconcile_status_from_report(
    report: &mqk_reconcile::ReconcileReport,
    broker: &mqk_reconcile::BrokerSnapshot,
    watermark: &SnapshotWatermark,
) -> ReconcileStatusSnapshot {
    let (mismatched_positions, mismatched_orders, mismatched_fills, unmatched_broker_events) =
        reconcile_counts(report);

    ReconcileStatusSnapshot {
        status: if report.is_clean() {
            "ok".to_string()
        } else {
            "dirty".to_string()
        },
        last_run_at: reconcile_last_run_at(broker.fetched_at_ms),
        snapshot_watermark_ms: Some(watermark.last_accepted_ms()),
        mismatched_positions,
        mismatched_orders,
        mismatched_fills,
        unmatched_broker_events,
        note: if report.is_clean() {
            None
        } else {
            Some("monotonic reconcile detected drift; dispatch remains blocked".to_string())
        },
    }
}

fn reconcile_status_from_stale(
    stale: &mqk_reconcile::StaleBrokerSnapshot,
    watermark: &SnapshotWatermark,
) -> ReconcileStatusSnapshot {
    let (last_run_at, note) = match stale.freshness {
        SnapshotFreshness::Stale {
            watermark_ms,
            got_ms,
        } => (
            reconcile_last_run_at(got_ms),
            format!(
                "stale broker snapshot rejected by reconcile watermark: watermark_ms={watermark_ms} got_ms={got_ms}"
            ),
        ),
        SnapshotFreshness::NoTimestamp => (
            None,
            "broker snapshot has no timestamp; reconcile ordering is ambiguous and remains fail-closed"
                .to_string(),
        ),
        SnapshotFreshness::Fresh => (
            None,
            "reconcile stale-state construction received a fresh snapshot unexpectedly"
                .to_string(),
        ),
    };

    ReconcileStatusSnapshot {
        status: "stale".to_string(),
        snapshot_watermark_ms: Some(watermark.last_accepted_ms()),
        last_run_at,
        mismatched_positions: 0,
        mismatched_orders: 0,
        mismatched_fills: 0,
        unmatched_broker_events: 0,
        note: Some(note),
    }
}

fn preserve_fail_closed_reconcile_status(
    previous: &ReconcileStatusSnapshot,
    note: impl Into<String>,
) -> ReconcileStatusSnapshot {
    let mut preserved = previous.clone();
    preserved.note = Some(note.into());
    preserved
}

async fn publish_reconcile_failure(
    state: &Arc<AppState>,
    reconcile: ReconcileStatusSnapshot,
    note: &str,
) {
    state.publish_reconcile_snapshot(reconcile).await;
    {
        let mut ig = state.integrity.write().await;
        ig.disarmed = true;
        ig.halted = true;
    }

    if let Some(db) = state.db.as_ref() {
        let _ = mqk_db::persist_arm_state_canonical(
            db,
            mqk_db::ArmState::Disarmed,
            Some(mqk_db::DisarmReason::ReconcileDrift),
        )
        .await;
        let _ =
            mqk_db::persist_risk_block_state(db, true, Some("RECONCILE_BLOCKED"), Utc::now()).await;
    }

    let active_run_id = state.status.read().await.active_run_id;
    let snapshot = StatusSnapshot {
        daemon_uptime_secs: uptime_secs(),
        active_run_id,
        state: "halted".to_string(),
        notes: Some(note.to_string()),
        integrity_armed: false,
        deadman_status: "unknown".to_string(),
        deadman_last_heartbeat_utc: None,
    };
    state.publish_status(snapshot).await;
    let _ = state.bus.send(BusMsg::LogLine {
        level: "ERROR".to_string(),
        msg: note.to_string(),
    });
}

fn spawn_execution_loop(
    state: Arc<AppState>,
    mut orchestrator: DaemonOrchestrator,
    run_id: Uuid,
) -> ExecutionLoopHandle {
    let (stop_tx, mut stop_rx) = watch::channel(ExecutionLoopCommand::Run);
    let snapshot_cache = Arc::clone(&state.execution_snapshot);
    // DMON-05: shared caches updated every tick so reconcile closures stay current.
    let broker_snapshot_cache = Arc::clone(&state.broker_snapshot);
    let side_cache = Arc::clone(&state.local_order_sides);
    let db = state.db.clone();
    let integrity = Arc::clone(&state.integrity);

    let join_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(EXECUTION_LOOP_INTERVAL);
        loop {
            tokio::select! {
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() == ExecutionLoopCommand::Stop {
                        break;
                    }
                }
                _ = ticker.tick() => {
                    if let Some(ref pool) = db {
                        let now = Utc::now();
                        match mqk_db::enforce_deadman_or_halt(pool, run_id, DEADMAN_TTL_SECONDS, now).await {
                            Ok(true) => {
                                let _ = mqk_db::persist_arm_state_canonical(
                                    pool,
                                    mqk_db::ArmState::Disarmed,
                                    Some(mqk_db::DisarmReason::DeadmanExpired),
                                )
                                .await;
                                {
                                    let mut ig = integrity.write().await;
                                    ig.disarmed = true;
                                    ig.halted = true;
                                }
                                if let Err(release_err) = orchestrator.release_runtime_leadership().await {
                                    tracing::warn!("runtime_lease_release_failed error={release_err}");
                                }
                                return ExecutionLoopExit {
                                    note: Some("execution loop halted: deadman expired".to_string()),
                                };
                            }
                            Ok(false) => {}
                            Err(err) => {
                                tracing::error!("execution_loop_deadman_check_failed error={err}");
                                let _ = mqk_db::halt_run(pool, run_id, now).await;
                                let _ = mqk_db::persist_arm_state_canonical(
                                    pool,
                                    mqk_db::ArmState::Disarmed,
                                    Some(mqk_db::DisarmReason::DeadmanSupervisorFailure),
                                )
                                .await;
                                {
                                    let mut ig = integrity.write().await;
                                    ig.disarmed = true;
                                    ig.halted = true;
                                }
                                if let Err(release_err) = orchestrator.release_runtime_leadership().await {
                                    tracing::warn!("runtime_lease_release_failed error={release_err}");
                                }
                                return ExecutionLoopExit {
                                    note: Some(format!("execution loop halted: deadman check failed: {err}")),
                                };
                            }
                        }
                    }

                    if let Err(err) = orchestrator.tick().await {
                        tracing::error!("execution_loop_halt error={err}");
                        // Safety net: durably halt the run so it cannot stay in a
                        // zombie RUNNING state after the loop exits.  For cases
                        // where tick() already called persist_halt_and_disarm
                        // internally (reconcile drift, recovery quarantine, etc.),
                        // halt_run is idempotent.  For generic DB errors it
                        // prevents a run that is permanently Running with no loop.
                        if let Some(ref pool) = db {
                            let now = Utc::now();
                            let _ = mqk_db::halt_run(pool, run_id, now).await;
                        }
                        {
                            let mut ig = integrity.write().await;
                            ig.halted = true;
                        }
                        if let Err(release_err) = orchestrator.release_runtime_leadership().await {
                            tracing::warn!("runtime_lease_release_failed error={release_err}");
                        }
                        return ExecutionLoopExit {
                            note: Some(format!("execution loop halted: {err}")),
                        };
                    }

                    if let Some(ref pool) = db {
                        let now = Utc::now();
                        // Deadman pre-check: if the heartbeat is already stale, the
                        // execution loop must not refresh it.  Sending a fresh heartbeat
                        // for an expired run would mask the expiry from the status surface
                        // and create a zombie loop. Exit and let the operator surface
                        // detect + persist the expired state on the next status query.
                        if let Ok(true) =
                            mqk_db::deadman_expired(pool, run_id, DEADMAN_TTL_SECONDS, now).await
                        {
                            tracing::error!(
                                run_id = %run_id,
                                "execution_loop_deadman_expired: heartbeat stale, self-terminating without refresh"
                            );
                            if let Err(release_err) =
                                orchestrator.release_runtime_leadership().await
                            {
                                tracing::warn!("runtime_lease_release_failed error={release_err}");
                            }
                            return ExecutionLoopExit {
                                note: Some("execution loop exited: deadman expired".to_string()),
                            };
                        }
                        if let Err(err) = mqk_db::heartbeat_run(pool, run_id, now).await {
                            tracing::error!("execution_loop_heartbeat_failed error={err}");
                            let _ = mqk_db::halt_run(pool, run_id, now).await;
                            let _ = mqk_db::persist_arm_state_canonical(
                                pool,
                                mqk_db::ArmState::Disarmed,
                                Some(mqk_db::DisarmReason::DeadmanHeartbeatPersistFailed),
                            )
                            .await;
                            {
                                let mut ig = integrity.write().await;
                                ig.disarmed = true;
                                ig.halted = true;
                            }
                            if let Err(release_err) = orchestrator.release_runtime_leadership().await {
                                tracing::warn!("runtime_lease_release_failed error={release_err}");
                            }
                            return ExecutionLoopExit {
                                note: Some(format!("execution loop heartbeat failed: {err}")),
                            };
                        }
                    }

                    match orchestrator.snapshot().await.context("snapshot failed") {
                        Ok(snapshot) => {
                            // DMON-05: refresh side cache from outbox, then synthesize
                            // broker snapshot from local OMS truth so paper-mode
                            // reconcile always sees a consistent local == broker view.
                            if let Some(ref pool) = db {
                                if let Ok(outbox_rows) =
                                    mqk_db::outbox_list_unacked_for_run(pool, run_id).await
                                {
                                    let mut sides = side_cache.write().await;
                                    for row in &outbox_rows {
                                        sides.insert(
                                            row.idempotency_key.clone(),
                                            outbox_json_side(&row.order_json),
                                        );
                                    }
                                }
                                let sides_snapshot = side_cache.read().await.clone();
                                let now = Utc::now();
                                let synth = synthesize_broker_snapshot_from_execution(
                                    &snapshot,
                                    &sides_snapshot,
                                    now,
                                );
                                *broker_snapshot_cache.write().await = Some(synth);
                            }
                            *snapshot_cache.write().await = Some(snapshot);
                        }
                        Err(err) => {
                            tracing::warn!("execution_snapshot_refresh_failed error={err}");
                        }
                    }
                }
            }
        }

        if let Err(err) = orchestrator.release_runtime_leadership().await {
            tracing::warn!("runtime_lease_release_failed error={err}");
        }

        ExecutionLoopExit {
            note: Some("execution loop stopped".to_string()),
        }
    });

    ExecutionLoopHandle {
        run_id,
        stop_tx,
        join_handle,
    }
}

/// Spawn a background task that periodically runs a reconcile tick (R3-1).
pub fn spawn_reconcile_tick<L, B>(
    state: Arc<AppState>,
    local_fn: L,
    broker_fn: B,
    interval: Duration,
) where
    L: Fn() -> mqk_reconcile::LocalSnapshot + Send + 'static,
    B: Fn() -> Option<mqk_reconcile::BrokerSnapshot> + Send + 'static,
{
    tokio::spawn(async move {
        // Use interval_at so the first tick fires after one full `interval`
        // rather than immediately.  This prevents the reconcile from writing
        // DB state before execution_snapshot is populated and avoids polluting
        // shared test DBs during short-lived lifecycle integration tests.
        let start = tokio::time::Instant::now() + interval;
        let mut ticker = tokio::time::interval_at(start, interval);
        let mut watermark = SnapshotWatermark::new();
        loop {
            ticker.tick().await;
            let local = local_fn();
            let Some(broker) = broker_fn() else {
                let previous = state.current_reconcile_snapshot().await;
                let reconcile = if previous.status == "dirty" {
                    preserve_fail_closed_reconcile_status(
                        &previous,
                        "broker snapshot absent; retaining prior dirty reconcile state under fail-closed semantics",
                    )
                } else {
                    reconcile_unknown_status(
                        "broker snapshot absent; reconcile ordering is not proven and remains fail-closed",
                    )
                };
                publish_reconcile_failure(
                    &state,
                    reconcile,
                    "reconcile broker snapshot absent - system disarmed (REC-01R)",
                )
                .await;
                continue;
            };

            match mqk_reconcile::reconcile_monotonic(&mut watermark, &local, &broker) {
                Ok(report) if report.is_clean() => {
                    state
                        .publish_reconcile_snapshot(reconcile_status_from_report(
                            &report, &broker, &watermark,
                        ))
                        .await;
                }
                Ok(report) => {
                    publish_reconcile_failure(
                        &state,
                        reconcile_status_from_report(&report, &broker, &watermark),
                        "reconcile drift detected - system disarmed (REC-01R)",
                    )
                    .await;
                }
                Err(stale) => {
                    let previous = state.current_reconcile_snapshot().await;
                    let reconcile = if previous.status == "dirty" {
                        preserve_fail_closed_reconcile_status(
                            &previous,
                            format!(
                                "stale broker snapshot rejected; retaining prior dirty reconcile state: {}",
                                reconcile_status_from_stale(&stale, &watermark)
                                    .note
                                    .unwrap_or_else(|| "stale broker snapshot rejected".to_string())
                            ),
                        )
                    } else {
                        reconcile_status_from_stale(&stale, &watermark)
                    };
                    publish_reconcile_failure(
                        &state,
                        reconcile,
                        "stale broker snapshot rejected by monotonic reconcile - system disarmed (REC-01R)",
                    )
                    .await;
                }
            }
        }
    });
}
