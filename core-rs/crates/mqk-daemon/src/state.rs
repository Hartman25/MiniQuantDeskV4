//! Shared runtime state for mqk-daemon.
//!
//! All types here are `Clone`-able (via `Arc` or copy). Handlers receive
//! `State<Arc<AppState>>` from Axum; this module owns nothing async itself.

use std::sync::Arc;
use std::time::Duration;

use mqk_integrity::IntegrityState;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
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
    /// "idle" | "running" | "halted"
    pub state: String,
    pub notes: Option<String>,
    /// Reflects `IntegrityState::is_execution_blocked()` negation: true = armed.
    pub integrity_armed: bool,
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
    /// Mutable run/status state.
    pub status: Arc<RwLock<StatusSnapshot>>,
    /// Integrity engine state (arm / disarm).
    pub integrity: Arc<RwLock<IntegrityState>>,
    /// Latest broker snapshot known to the daemon (in-memory for now).
    ///
    /// DAEMON-1: read-only trading APIs surface this snapshot. A later patch
    /// wires ingestion from broker/reconcile pipelines.
    pub broker_snapshot: Arc<RwLock<Option<mqk_schemas::BrokerSnapshot>>>,
    /// B4: Latest execution pipeline snapshot.
    ///
    /// Updated after every successful orchestrator tick by `spawn_execution_loop`.
    /// `None` until the first tick completes (or if no execution loop is running).
    /// Read-only from HTTP handlers — never written outside the execution loop.
    pub execution_snapshot: Arc<RwLock<Option<mqk_runtime::observability::ExecutionSnapshot>>>,
    /// S7-1: Bearer token required on all operator (POST/DELETE) routes.
    ///
    /// `None`  — no token configured; operator routes are unauthenticated
    ///            (dev / loopback-only mode).  Set `MQK_OPERATOR_TOKEN` in the
    ///            environment to enable authentication.
    /// `Some(t)` — every operator request must supply `Authorization: Bearer t`
    ///             or the request is rejected with `401 Unauthorized`.
    pub operator_token: Option<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    /// Create application state, reading `MQK_OPERATOR_TOKEN` from the environment.
    ///
    /// An empty string is treated the same as absent (no authentication).
    pub fn new() -> Self {
        let token = std::env::var("MQK_OPERATOR_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());
        Self::new_inner(token)
    }

    /// Create application state with an explicit operator token.
    ///
    /// Pass `None` to disable token authentication (dev / test mode).
    /// Pass `Some(token)` to require that token on all operator routes.
    pub fn new_with_token(token: Option<String>) -> Self {
        Self::new_inner(token)
    }

    fn new_inner(operator_token: Option<String>) -> Self {
        let (bus, _rx) = broadcast::channel::<BusMsg>(1024);

        let initial_status = StatusSnapshot {
            daemon_uptime_secs: uptime_secs(),
            active_run_id: None,
            state: "idle".to_string(),
            notes: Some("placeholder status; wire run loop next".to_string()),
            integrity_armed: false, // Patch C1: fail-closed at boot
        };

        // Patch C1: start disarmed so an operator arm is required before any run.
        let mut boot_integrity = IntegrityState::new();
        boot_integrity.disarmed = true;

        Self {
            bus,
            build: BuildInfo {
                service: "mqk-daemon",
                version: env!("CARGO_PKG_VERSION"),
            },
            status: Arc::new(RwLock::new(initial_status)),
            integrity: Arc::new(RwLock::new(boot_integrity)),
            broker_snapshot: Arc::new(RwLock::new(None)),
            execution_snapshot: Arc::new(RwLock::new(None)),
            operator_token,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
            let ts = chrono::Utc::now().timestamp_millis(); // allow: ops-metadata — SSE heartbeat timestamp, not enforcement
            let _ = bus.send(BusMsg::Heartbeat { ts_millis: ts });
        }
    });
}

/// FD-2: Spawn a background task that drives `ExecutionOrchestrator::tick` on
/// the given interval.  This is the single authoritative execution path.
///
/// On any `tick` error the loop logs and halts — no silent swallowing.
///
/// B4: If `snapshot_cache` is `Some`, the cache is refreshed after every
/// successful tick so the diagnostics route always serves a recent snapshot.
/// Pass `None` to disable snapshot collection (e.g. in tests that only need
/// tick behavior).
pub fn spawn_execution_loop<B, IG, RG, RecG, TS>(
    mut orchestrator: mqk_runtime::orchestrator::ExecutionOrchestrator<B, IG, RG, RecG, TS>,
    interval: Duration,
    snapshot_cache: Option<Arc<RwLock<Option<mqk_runtime::observability::ExecutionSnapshot>>>>,
) where
    B: mqk_execution::BrokerAdapter + Send + 'static,
    IG: mqk_execution::IntegrityGate + Send + 'static,
    RG: mqk_execution::RiskGate + Send + 'static,
    RecG: mqk_execution::ReconcileGate + Send + 'static,
    TS: mqk_db::TimeSource + Send + 'static,
{
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            if let Err(e) = orchestrator.tick().await {
                tracing::error!("execution_loop_halt error={}", e);
                return;
            }
            // B4: refresh snapshot cache after each successful tick.
            if let Some(ref cache) = snapshot_cache {
                match orchestrator.snapshot().await {
                    Ok(snap) => {
                        *cache.write().await = Some(snap);
                    }
                    Err(e) => {
                        tracing::warn!("b4_snapshot_collection_failed error={}", e);
                    }
                }
            }
        }
    });
}

/// Spawn a background task that periodically runs a reconcile tick (R3-1).
///
/// On each interval:
/// - Calls `local_fn()` and `broker_fn()` to obtain fresh snapshots.
/// - If `broker_fn` returns `None`, the tick is skipped (no snapshot yet).
/// - Calls [`mqk_reconcile::reconcile_tick`] on the snapshot pair.
/// - If [`mqk_reconcile::DriftAction::HaltAndDisarm`] is returned the task:
///   1. Sets `integrity.disarmed = true` (blocks all broker submissions).
///   2. Sets `status.state = "halted"` and `status.integrity_armed = false`.
///   3. Broadcasts `BusMsg::LogLine { level: "ERROR" }` on the SSE bus.
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
        loop {
            ticker.tick().await;
            let local = local_fn();
            let Some(broker) = broker_fn() else {
                continue;
            };
            let action = mqk_reconcile::reconcile_tick(&local, &broker);
            if action.requires_halt_and_disarm() {
                {
                    let mut ig = state.integrity.write().await;
                    ig.disarmed = true;
                }
                {
                    let mut st = state.status.write().await;
                    st.state = "halted".to_string();
                    st.integrity_armed = false;
                }
                let _ = state.bus.send(BusMsg::LogLine {
                    level: "ERROR".to_string(),
                    msg: "reconcile drift detected — system disarmed (R3-1)".to_string(),
                });
            }
        }
    });
}
