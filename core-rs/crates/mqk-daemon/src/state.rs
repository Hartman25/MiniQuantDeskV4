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
    wiring::build_gateway, BrokerOrderMap, IntegrityGate, ReconcileGate, RiskDecision, RiskGate,
};
use mqk_integrity::IntegrityState;
use mqk_portfolio::PortfolioState;
use mqk_reconcile::{ReconcileDiff, SnapshotFreshness, SnapshotWatermark};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::{broadcast, watch, Mutex, RwLock};
use tokio::task::JoinHandle;
use uuid::Uuid;

const DAEMON_ENGINE_ID: &str = "mqk-daemon";
const DAEMON_MODE: &str = "PAPER";
const DAEMON_ADAPTER_ID: &str = "paper";
const DAEMON_RUN_CONFIG_HASH: &str = "daemon-runtime-paper-v1";
const EXECUTION_LOOP_INTERVAL: Duration = Duration::from_secs(1);
const DEADMAN_TTL_SECONDS: i64 = 5;
const DEV_ALLOW_NO_OPERATOR_TOKEN_ENV: &str = "MQK_DEV_ALLOW_NO_OPERATOR_TOKEN";

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

#[derive(Debug)]
pub enum RuntimeLifecycleError {
    ServiceUnavailable(String),
    Forbidden { gate: String, message: String },
    Conflict(String),
    Internal(String),
}

impl RuntimeLifecycleError {
    fn internal(context: &str, err: impl fmt::Display) -> Self {
        Self::Internal(format!("{context}: {err}"))
    }
}

impl fmt::Display for RuntimeLifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ServiceUnavailable(msg) => f.write_str(msg),
            Self::Forbidden { message, .. } => f.write_str(message),
            Self::Conflict(msg) => f.write_str(msg),
            Self::Internal(msg) => f.write_str(msg),
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

#[derive(Clone, Copy)]
struct AllowRiskGate;

impl RiskGate for AllowRiskGate {
    fn evaluate_gate(&self) -> RiskDecision {
        RiskDecision::Allow
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

type DaemonOrchestrator = mqk_runtime::orchestrator::ExecutionOrchestrator<
    LockedPaperBroker,
    StateIntegrityGate,
    AllowRiskGate,
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
    /// Latest monotonic reconcile result known to the daemon.
    reconcile_status: Arc<RwLock<ReconcileStatusSnapshot>>,
    /// Operator auth posture for privileged routes.
    pub operator_auth: OperatorAuthMode,
    /// The single daemon-owned execution loop handle, if any.
    execution_loop: Arc<Mutex<Option<ExecutionLoopHandle>>>,
    /// Serializes start/stop/halt transitions so the daemon never spawns duplicates.
    lifecycle_op: Arc<Mutex<()>>,
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

        Self {
            bus,
            node_id: default_node_id(build.service),
            build,
            db,
            status: Arc::new(RwLock::new(initial_status)),
            integrity: Arc::new(RwLock::new(boot_integrity)),
            broker_snapshot: Arc::new(RwLock::new(None)),
            execution_snapshot: Arc::new(RwLock::new(None)),
            reconcile_status: Arc::new(RwLock::new(initial_reconcile_status())),
            operator_auth,
            execution_loop: Arc::new(Mutex::new(None)),
            lifecycle_op: Arc::new(Mutex::new(())),
        }
    }

    pub fn operator_auth_mode(&self) -> &OperatorAuthMode {
        &self.operator_auth
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

    pub async fn current_status_snapshot(&self) -> Result<StatusSnapshot, RuntimeLifecycleError> {
        let reaped = self.reap_finished_execution_loop().await?;
        let reaped_note = reaped.and_then(|exit| exit.note);
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

        if let Some(run_id) = self.active_owned_run_id().await {
            let deadman = self.deadman_truth_for_run(run_id).await?;
            let snapshot = StatusSnapshot {
                daemon_uptime_secs: uptime_secs(),
                active_run_id: Some(run_id),
                state: "running".to_string(),
                notes: Some("daemon owns active execution loop".to_string()),
                integrity_armed,
                deadman_status: deadman.status,
                deadman_last_heartbeat_utc: deadman.last_heartbeat_utc,
            };
            self.publish_status(snapshot.clone()).await;
            return Ok(snapshot);
        }

        let snapshot = match self.db.as_ref() {
            Some(db) => {
                let latest = mqk_db::fetch_latest_run_for_engine(db, DAEMON_ENGINE_ID, DAEMON_MODE)
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
                            StatusSnapshot {
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
                        mqk_db::RunStatus::Created | mqk_db::RunStatus::Stopped => StatusSnapshot {
                            daemon_uptime_secs: uptime_secs(),
                            active_run_id: None,
                            state: if locally_halted {
                                "halted".to_string()
                            } else {
                                "idle".to_string()
                            },
                            notes: reaped_note.clone().or(cached_notes),
                            integrity_armed,
                            deadman_status: "inactive".to_string(),
                            deadman_last_heartbeat_utc: run
                                .last_heartbeat_utc
                                .map(|ts| ts.to_rfc3339()),
                        },
                    },
                    None => StatusSnapshot {
                        daemon_uptime_secs: uptime_secs(),
                        active_run_id: None,
                        state: if locally_halted {
                            "halted".to_string()
                        } else {
                            "idle".to_string()
                        },
                        notes: reaped_note.clone().or(cached_notes),
                        integrity_armed,
                        deadman_status: "inactive".to_string(),
                        deadman_last_heartbeat_utc: None,
                    },
                }
            }
            None => StatusSnapshot {
                daemon_uptime_secs: uptime_secs(),
                active_run_id: None,
                state: if locally_halted {
                    "halted".to_string()
                } else {
                    "idle".to_string()
                },
                notes: reaped_note.or(cached_notes),
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

        if self.integrity.read().await.is_execution_blocked() {
            return Err(RuntimeLifecycleError::Forbidden {
                gate: "integrity_armed".to_string(),
                message: "GATE_REFUSED: integrity disarmed or halted; arm integrity first"
                    .to_string(),
            });
        }

        if let Some(run_id) = self.active_owned_run_id().await {
            return Err(RuntimeLifecycleError::Conflict(format!(
                "runtime already active under local ownership: {run_id}"
            )));
        }

        let db = self.db_pool()?;
        if let Some(active) =
            mqk_db::fetch_active_run_for_engine(&db, DAEMON_ENGINE_ID, DAEMON_MODE)
                .await
                .map_err(|err| {
                    RuntimeLifecycleError::internal("start active-run lookup failed", err)
                })?
        {
            return Err(RuntimeLifecycleError::Conflict(format!(
                "durable active run exists without local ownership: {}",
                active.run_id
            )));
        }

        let latest = mqk_db::fetch_latest_run_for_engine(&db, DAEMON_ENGINE_ID, DAEMON_MODE)
            .await
            .map_err(|err| {
                RuntimeLifecycleError::internal("start latest-run lookup failed", err)
            })?;

        let run_id = match latest.as_ref() {
            Some(run) => match run.status {
                mqk_db::RunStatus::Created | mqk_db::RunStatus::Stopped => run.run_id,
                mqk_db::RunStatus::Halted => {
                    return Err(RuntimeLifecycleError::Conflict(format!(
                        "durable run {} is halted; operator must clear the halted lifecycle before starting again",
                        run.run_id
                    )))
                }
                mqk_db::RunStatus::Armed | mqk_db::RunStatus::Running => {
                    return Err(RuntimeLifecycleError::Conflict(format!(
                        "durable run {} is already active",
                        run.run_id
                    )))
                }
            },
            None => {
                let run_id = self.next_daemon_run_id(&db).await?;
                mqk_db::insert_run(
                    &db,
                    &mqk_db::NewRun {
                        run_id,
                        engine_id: DAEMON_ENGINE_ID.to_string(),
                        mode: DAEMON_MODE.to_string(),
                        started_at_utc: Utc::now(),
                        git_hash: "UNKNOWN".to_string(),
                        config_hash: DAEMON_RUN_CONFIG_HASH.to_string(),
                        config_json: serde_json::json!({
                            "runtime": "mqk-daemon",
                            "adapter": DAEMON_ADAPTER_ID,
                            "mode": DAEMON_MODE,
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
                return Err(RuntimeLifecycleError::Conflict(format!(
                    "runtime leader lease unavailable: {message}"
                )));
            }
            return Err(RuntimeLifecycleError::internal(
                "start initial tick failed",
                err,
            ));
        }

        let handle = spawn_execution_loop(Arc::clone(self), orchestrator, run_id);
        {
            let mut lock = self.execution_loop.lock().await;
            if lock.is_some() {
                return Err(RuntimeLifecycleError::Conflict(
                    "runtime ownership changed while starting; refusing duplicate loop".to_string(),
                ));
            }
            *lock = Some(handle);
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
                    if let Some(active) =
                        mqk_db::fetch_active_run_for_engine(db, DAEMON_ENGINE_ID, DAEMON_MODE)
                            .await
                            .map_err(|err| {
                                RuntimeLifecycleError::internal(
                                    "stop active-run lookup failed",
                                    err,
                                )
                            })?
                    {
                        return Err(RuntimeLifecycleError::Conflict(format!(
                            "durable active run exists without local ownership: {}",
                            active.run_id
                        )));
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
                if let Some(active) =
                    mqk_db::fetch_active_run_for_engine(db, DAEMON_ENGINE_ID, DAEMON_MODE)
                        .await
                        .map_err(|err| {
                            RuntimeLifecycleError::internal("halt active-run lookup failed", err)
                        })?
                {
                    return Err(RuntimeLifecycleError::Conflict(format!(
                        "durable active run exists without local ownership: {}",
                        active.run_id
                    )));
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
        mqk_db::persist_arm_state(&db, "DISARMED", Some("OperatorHalt"))
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
            RuntimeLifecycleError::ServiceUnavailable(
                "runtime DB is not configured on this daemon".to_string(),
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
        .bind(DAEMON_MODE)
        .fetch_one(db)
        .await
        .map_err(|err| RuntimeLifecycleError::internal("next_daemon_run_id failed", err))?;

        Ok(Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            format!(
                "mqk-daemon.run.v2|{}|{}|{}|{}",
                self.node_id, DAEMON_ENGINE_ID, DAEMON_MODE, generation
            )
            .as_bytes(),
        ))
    }

    async fn build_execution_orchestrator(
        &self,
        db: PgPool,
        run_id: Uuid,
    ) -> Result<DaemonOrchestrator, RuntimeLifecycleError> {
        let mut order_map = BrokerOrderMap::new();
        let existing = mqk_db::broker_map_load(&db)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("broker_map_load failed", err))?;
        for (internal_id, broker_id) in existing {
            order_map.register(&internal_id, &broker_id);
        }

        let broker_cursor = mqk_db::load_broker_cursor(&db, DAEMON_ADAPTER_ID)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("load_broker_cursor failed", err))?;

        let gateway = build_gateway(
            LockedPaperBroker::new(),
            StateIntegrityGate {
                integrity: Arc::clone(&self.integrity),
            },
            AllowRiskGate,
            ReconcileTruthGate {
                reconcile_status: Arc::clone(&self.reconcile_status),
            },
        );

        Ok(mqk_runtime::orchestrator::ExecutionOrchestrator::new(
            db,
            gateway,
            order_map,
            BTreeMap::new(),
            PortfolioState::new(0),
            run_id,
            self.node_id.clone(),
            DAEMON_ADAPTER_ID,
            broker_cursor,
            mqk_runtime::orchestrator::WallClock,
            Box::new(mqk_reconcile::LocalSnapshot::empty),
            Box::new(mqk_reconcile::BrokerSnapshot::empty),
        ))
    }

    async fn active_owned_run_id(&self) -> Option<Uuid> {
        let lock = self.execution_loop.lock().await;
        lock.as_ref()
            .filter(|handle| !handle.join_handle.is_finished())
            .map(|handle| handle.run_id)
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

#[cfg(test)]
mod tests {
    use super::*;

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
            mqk_db::persist_arm_state(&db, "DISARMED", Some("DeadmanExpired"))
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
        let _ = mqk_db::persist_arm_state(db, "DISARMED", Some("ReconcileDrift")).await;
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
                                let _ = mqk_db::persist_arm_state(pool, "DISARMED", Some("DeadmanExpired")).await;
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
                                let _ = mqk_db::persist_arm_state(pool, "DISARMED", Some("DeadmanSupervisorFailure")).await;
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
                        if let Err(release_err) = orchestrator.release_runtime_leadership().await {
                            tracing::warn!("runtime_lease_release_failed error={release_err}");
                        }
                        return ExecutionLoopExit {
                            note: Some(format!("execution loop halted: {err}")),
                        };
                    }

                    if let Some(ref pool) = db {
                        let now = Utc::now();
                        if let Err(err) = mqk_db::heartbeat_run(pool, run_id, now).await {
                            tracing::error!("execution_loop_heartbeat_failed error={err}");
                            let _ = mqk_db::halt_run(pool, run_id, now).await;
                            let _ = mqk_db::persist_arm_state(pool, "DISARMED", Some("DeadmanHeartbeatPersistFailed")).await;
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
        let mut ticker = tokio::time::interval(interval);
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
