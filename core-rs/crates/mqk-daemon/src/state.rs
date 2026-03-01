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
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
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
            let ts = chrono::Utc::now().timestamp_millis();
            let _ = bus.send(BusMsg::Heartbeat { ts_millis: ts });
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
