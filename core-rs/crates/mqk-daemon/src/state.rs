//! Shared runtime state for mqk-daemon.
//!
//! All types here are `Clone`-able (via `Arc` or copy). Handlers receive
//! `State<Arc<AppState>>` from Axum; this module owns daemon-local runtime
//! lifecycle control plus durable status reconstruction.

mod alpaca_ws_transport;
mod broker;
mod env;
mod loop_runner;
mod session_controller;
mod snapshot;
mod types;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use mqk_broker_alpaca::types::AlpacaFetchCursor;
use mqk_execution::{wiring::build_gateway, BrokerError, BrokerOrderMap};
use mqk_integrity::{CalendarSpec, IntegrityState};
use sqlx::PgPool;
use tokio::sync::{broadcast, watch, Mutex, RwLock};
use tokio::task::JoinHandle;
use uuid::Uuid;

// Re-export everything that external code (routes, tests, etc.) needs.
use crate::notify::{CriticalAlertPayload, DiscordNotifier};
pub use alpaca_ws_transport::{
    build_ws_auth_message, build_ws_subscribe_message, spawn_alpaca_paper_ws_task,
    ws_url_from_base_url,
};
pub use broker::{DeploymentReadiness, RuntimeSelection, StrategyFleetEntry};
pub use env::{operator_auth_mode_from_env_values, spawn_heartbeat, uptime_secs};
pub use loop_runner::spawn_reconcile_tick;
pub use session_controller::{
    autonomous_session_schedule_from_env, run_session_controller_tick, session_window_from_env,
    spawn_autonomous_session_controller, AutonomousSessionSchedule, SessionWindow,
    SESSION_START_HH_MM_ENV, SESSION_STOP_HH_MM_ENV,
};
pub(crate) use snapshot::{
    reconcile_broker_snapshot_from_schema, reconcile_local_snapshot_from_runtime_with_sides,
};
pub use types::{
    AcceptedArtifactProvenance, AlpacaWsContinuityState, AutonomousRecoveryResumeSource,
    AutonomousSessionTruth, BrokerKind, BrokerSnapshotTruthSource, BuildInfo, BusMsg,
    DeploymentMode, OperatorAuthMode, ReconcileStatusSnapshot, RestartTruthSnapshot,
    RuntimeLifecycleError, StatusSnapshot, StrategyMarketDataSource,
};
pub(crate) use types::{ExecutionLoopCommand, ExecutionLoopExit, ExecutionLoopHandle};
// Internal (crate-visible) re-exports used across this module.
use crate::artifact_intake::{
    evaluate_artifact_deployability, evaluate_artifact_intake_guarded, ArtifactIntakeOutcome,
    ENV_ARTIFACT_PATH,
};
use crate::capital_policy::{
    evaluate_capital_policy_from_env, evaluate_deployment_economics_from_env, CapitalPolicyOutcome,
    DeploymentEconomicsOutcome,
};
use crate::parity_evidence::{evaluate_parity_evidence_from_env, ParityEvidenceOutcome};
#[cfg(test)]
use broker::alpaca_base_url_for_mode;
use broker::{build_daemon_broker, DaemonBroker};
#[cfg(test)]
use env::runtime_selection_from_env_values;
use env::{
    deployment_mode_readiness, initial_reconcile_status, initial_ws_continuity_for_broker,
    operator_auth_mode_from_env, runtime_selection_from_env,
};
use mqk_runtime::native_strategy::{build_daemon_plugin_registry, NativeStrategyBootstrap};
use snapshot::{recover_oms_and_portfolio, synthesize_paper_broker_snapshot};
use types::{DaemonOrchestrator, ReconcileTruthGate, StateIntegrityGate};

pub(crate) const DAEMON_ENGINE_ID: &str = "mqk-daemon";
/// PT-AUTO-02: Maximum number of strategy signals accepted per execution run.
///
/// Provides a hard per-run intake bound on the paper+alpaca signal ingestion
/// path.  After this many distinct signals are enqueued (Gate 7 Ok(true)),
/// Gate 1d refuses further signals with 409/day_limit_reached until the next
/// run start resets the counter.
///
/// 100 signals per run is conservative for a supervised paper session.  It is
/// not an economics guarantee — it is a safety bound.
const MAX_AUTONOMOUS_SIGNALS_PER_RUN: u32 = 100;
const DEFAULT_DAEMON_DEPLOYMENT_MODE: &str = "paper";
const DEFAULT_DAEMON_ADAPTER_ID: &str = "paper";
const DAEMON_RUN_CONFIG_HASH_PREFIX: &str = "daemon-runtime";
const EXECUTION_LOOP_INTERVAL: Duration = Duration::from_secs(1);
const DEADMAN_TTL_SECONDS: i64 = 5;
/// DMON-06: background reconcile tick interval.
const RECONCILE_TICK_INTERVAL: Duration = Duration::from_secs(30);
const DEV_ALLOW_NO_OPERATOR_TOKEN_ENV: &str = "MQK_DEV_ALLOW_NO_OPERATOR_TOKEN";
const DAEMON_DEPLOYMENT_MODE_ENV: &str = "MQK_DAEMON_DEPLOYMENT_MODE";
const DAEMON_ADAPTER_ID_ENV: &str = "MQK_DAEMON_ADAPTER_ID";
// ENV-TRUTH-01: canonical paper credentials matching .env.local.example / base.yaml
const ALPACA_KEY_PAPER_ENV: &str = "ALPACA_API_KEY_PAPER";
const ALPACA_SECRET_PAPER_ENV: &str = "ALPACA_API_SECRET_PAPER";
const ALPACA_BASE_URL_PAPER_ENV: &str = "ALPACA_PAPER_BASE_URL";
// ENV-TRUTH-01: canonical live credentials matching .env.local.example
const ALPACA_KEY_LIVE_ENV: &str = "ALPACA_API_KEY_LIVE";
const ALPACA_SECRET_LIVE_ENV: &str = "ALPACA_API_SECRET_LIVE";

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
    /// Mutable status cache.
    pub status: Arc<RwLock<StatusSnapshot>>,
    /// Integrity engine state (arm / disarm).
    pub integrity: Arc<RwLock<IntegrityState>>,
    /// Latest broker snapshot known to the daemon (in-memory for now).
    pub broker_snapshot: Arc<RwLock<Option<mqk_schemas::BrokerSnapshot>>>,
    /// Latest execution pipeline snapshot from the owned loop.
    pub execution_snapshot: Arc<RwLock<Option<mqk_runtime::observability::ExecutionSnapshot>>>,
    /// Per-order side cache (order_id → reconcile Side).
    pub local_order_sides: Arc<RwLock<BTreeMap<String, mqk_reconcile::Side>>>,
    /// Latest monotonic reconcile result known to the daemon.
    reconcile_status: Arc<RwLock<ReconcileStatusSnapshot>>,
    /// Operator auth posture for privileged routes.
    pub operator_auth: OperatorAuthMode,
    /// Runtime adapter/deployment selection resolved from config/env at bootstrap.
    runtime_selection: RuntimeSelection,
    /// The single daemon-owned execution loop handle, if any.
    execution_loop: Arc<Mutex<Option<ExecutionLoopHandle>>>,
    /// Serializes start/stop/halt transitions.
    lifecycle_op: Arc<Mutex<()>>,
    /// Authoritative exchange calendar spec derived from deployment mode.
    calendar_spec: CalendarSpec,
    /// AP-04: How broker_snapshot is populated for this broker kind.
    pub broker_snapshot_source: BrokerSnapshotTruthSource,
    /// AP-04B: Strategy market-data source policy.
    pub strategy_market_data_source: StrategyMarketDataSource,
    /// AP-05: Daemon-owned Alpaca websocket continuity truth.
    alpaca_ws_continuity: Arc<RwLock<AlpacaWsContinuityState>>,
    /// PT-DAY-03: Injectable wall-clock override for NYSE session gate.
    ///
    /// `None` in production — route reads `Utc::now().timestamp()` directly.
    /// Set to a fixed timestamp in tests to make session-gate proof hermetic.
    session_clock_override: Arc<RwLock<Option<i64>>>,
    /// PT-DAY-04: Deduplication flag for WS continuity-gap operator escalation.
    ///
    /// `false` at boot and after each Live transition.  Set to `true` on the
    /// first GapDetected signal refusal.  Prevents notification spam when the
    /// gap persists across multiple signal POSTs — only the first refusal per
    /// gap window emits a Discord notification.
    gap_escalation_pending: Arc<AtomicBool>,
    /// CC-01: Configured strategy fleet.
    strategy_fleet: Arc<RwLock<Option<Vec<StrategyFleetEntry>>>>,
    /// OPS-NOTIFY-01: Best-effort Discord webhook notifier.  No-op when
    /// `DISCORD_WEBHOOK_URL` is unset.  Delivery failure does not affect
    /// primary daemon control truth.
    pub discord_notifier: DiscordNotifier,
    /// PT-AUTO-02: Per-run autonomous signal intake counter.
    ///
    /// Incremented on every new outbox enqueue (Gate 7 Ok(true)).  Reset to 0
    /// at the start of each new execution run in `start_execution_runtime`.
    /// Gate 1d refuses further signals once this reaches
    /// `MAX_AUTONOMOUS_SIGNALS_PER_RUN`.
    day_signal_count: Arc<AtomicU32>,
    /// TV-01C: Artifact provenance accepted at the most recent run start.
    ///
    /// Populated by `start_execution_runtime` when artifact intake evaluates to
    /// `Accepted`.  Cleared on stop/halt.  `None` when no run is active, no
    /// artifact was configured, or intake was not `Accepted` — all fail-closed.
    accepted_artifact: Arc<RwLock<Option<AcceptedArtifactProvenance>>>,
    /// AUTON-PAPER-02: current autonomous supervisory/recovery truth.
    ///
    /// Daemon-local only: this is current condition truth for operator surfaces,
    /// not durable history.  Cleared/overwritten as the controller and WS
    /// transport observe new facts.
    autonomous_session_truth: Arc<RwLock<AutonomousSessionTruth>>,
    /// AUTON-HIST-01: sticky flag set when autonomous session event persistence
    /// fails or is not possible (no DB configured).
    ///
    /// Once set, it is never cleared in-session — the operator must restart the
    /// daemon with a working DB to recover durable history.  Surfaced in
    /// `/api/v1/autonomous/readiness` as `autonomous_history_degraded`.
    autonomous_history_degraded: Arc<AtomicBool>,
    /// B1A: Native strategy runtime bootstrap for the current execution run.
    ///
    /// `None` when no run is active.  Set at run-start to the bootstrap outcome
    /// (Dormant / Active / Failed).  Cleared on stop/halt alongside
    /// `accepted_artifact`.  Active bootstrap holds the strategy host in shadow
    /// mode; bar ingestion is not yet wired (B1A constraint).
    native_strategy_bootstrap: Arc<Mutex<Option<NativeStrategyBootstrap>>>,
    /// B1B: Pending strategy bar input deposited by the signal route for the
    /// execution loop to consume on its next tick.
    ///
    /// `None` when no bar is pending (normal state between signals).
    /// Overwritten by each new deposit (single slot: new bar supersedes any
    /// unconsumed prior bar).  Consumed atomically (set to `None`) by
    /// `tick_strategy_dispatch`.
    pending_strategy_bar_input: Arc<Mutex<Option<StrategyBarInput>>>,
    /// B3: Unix-second timestamp of the last `deposit_strategy_bar_input` call.
    ///
    /// Set to `input.end_ts` on every deposit; never cleared on stop/restart.
    /// Zero means no bar input has been deposited in this daemon process lifetime.
    /// Read by `/api/v1/strategy/summary` to surface honest `last_decision_time`.
    last_bar_input_ts: Arc<AtomicI64>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// B1B: Raw bar input parameters for one native strategy `on_bar` dispatch.
///
/// Deposited by the signal route (ExternalSignalIngestion path) into
/// `AppState::pending_strategy_bar_input` after Gate 6 passes.
/// Consumed on the next execution loop tick by `AppState::tick_strategy_dispatch`.
///
/// Overwrite policy: a new deposit supersedes any prior unconsumed bar.
/// The `day_signal_limit` gate (Gate 1d) bounds the deposit rate so
/// supersession is rare in practice.
#[derive(Debug)]
pub struct StrategyBarInput {
    pub now_tick: u64,
    pub end_ts: i64,
    pub limit_price: Option<i64>,
    pub qty: i64,
}

/// AUTON-CALENDAR-01: Derive the authoritative CalendarSpec for a (mode, broker_kind) pair.
///
/// Paper+Alpaca uses `NyseWeekdays` — the broker is NYSE-backed via Alpaca and the
/// autonomous session controller already enforces NYSE regular-session boundaries.
/// Using `AlwaysOn` for this pair makes the `/api/v1/system/session` display lie:
/// it reports `market_session="regular"` on weekends and holidays while the controller
/// is correctly blocking all starts.  Giving Paper+Alpaca its honest calendar closes
/// the display/gate disagreement.
///
/// Paper+Paper (in-process fill engine) and Backtest keep `AlwaysOn` — those paths
/// run on synthetic time and are not bound to exchange hours.
fn calendar_spec_for_deployment(
    mode: DeploymentMode,
    broker_kind: Option<BrokerKind>,
) -> CalendarSpec {
    match mode {
        DeploymentMode::LiveShadow | DeploymentMode::LiveCapital => CalendarSpec::NyseWeekdays,
        DeploymentMode::Paper if broker_kind == Some(BrokerKind::Alpaca) => {
            CalendarSpec::NyseWeekdays
        }
        _ => CalendarSpec::AlwaysOn,
    }
}

impl AppState {
    pub fn new() -> Self {
        Self::new_inner(OperatorAuthMode::ExplicitDevNoToken, None)
    }

    pub fn new_with_operator_auth(operator_auth: OperatorAuthMode) -> Self {
        Self::new_inner(operator_auth, None)
    }

    pub fn new_with_token(token: Option<String>) -> Self {
        let operator_auth = match token {
            Some(token) => OperatorAuthMode::TokenRequired(token),
            None => OperatorAuthMode::ExplicitDevNoToken,
        };
        Self::new_inner(operator_auth, None)
    }

    pub fn new_with_db(db: PgPool) -> Self {
        Self::new_inner(operator_auth_mode_from_env(), Some(db))
    }

    pub fn new_with_db_and_operator_auth(db: PgPool, operator_auth: OperatorAuthMode) -> Self {
        Self::new_inner(operator_auth, Some(db))
    }

    pub fn new_for_test_with_broker_kind(kind: BrokerKind) -> Self {
        let mut state = Self::new_inner(OperatorAuthMode::ExplicitDevNoToken, None);
        // Recompute readiness for the requested broker kind so it reflects the
        // actual (mode, broker) pair, not the stale default paper+paper readiness.
        let readiness =
            deployment_mode_readiness(state.runtime_selection.deployment_mode, Some(kind));
        state.runtime_selection = RuntimeSelection {
            deployment_mode: state.runtime_selection.deployment_mode,
            broker_kind: Some(kind),
            adapter_id: kind.as_str().to_string(),
            run_config_hash: state.runtime_selection.run_config_hash.clone(),
            readiness,
        };
        state.broker_snapshot_source = BrokerSnapshotTruthSource::from_broker_kind(Some(kind));
        state.alpaca_ws_continuity = Arc::new(RwLock::new(match kind {
            BrokerKind::Alpaca => AlpacaWsContinuityState::ColdStartUnproven,
            BrokerKind::Paper => AlpacaWsContinuityState::NotApplicable,
        }));
        // PT-DAY-01: recompute signal ingestion policy for the new (mode, broker) pair.
        state.strategy_market_data_source = if state.runtime_selection.deployment_mode
            == DeploymentMode::Paper
            && kind == BrokerKind::Alpaca
        {
            StrategyMarketDataSource::ExternalSignalIngestion
        } else {
            StrategyMarketDataSource::NotConfigured
        };
        // AUTON-CALENDAR-01: Paper+Alpaca is NYSE-backed; give it the honest calendar.
        state.calendar_spec =
            calendar_spec_for_deployment(state.runtime_selection.deployment_mode, Some(kind));
        state
    }

    pub fn new_for_test_with_mode(mode: DeploymentMode) -> Self {
        let mut state = Self::new_inner(OperatorAuthMode::ExplicitDevNoToken, None);
        let broker_kind = state.runtime_selection.broker_kind;
        let readiness = deployment_mode_readiness(mode, broker_kind);
        state.runtime_selection = RuntimeSelection {
            deployment_mode: mode,
            broker_kind,
            adapter_id: state.runtime_selection.adapter_id.clone(),
            run_config_hash: state.runtime_selection.run_config_hash.clone(),
            readiness,
        };
        state.calendar_spec =
            calendar_spec_for_deployment(mode, state.runtime_selection.broker_kind);
        state
    }

    pub fn new_for_test_with_mode_and_broker(mode: DeploymentMode, kind: BrokerKind) -> Self {
        let mut state = Self::new_inner(OperatorAuthMode::ExplicitDevNoToken, None);
        let readiness = deployment_mode_readiness(mode, Some(kind));
        state.runtime_selection = RuntimeSelection {
            deployment_mode: mode,
            broker_kind: Some(kind),
            adapter_id: kind.as_str().to_string(),
            run_config_hash: state.runtime_selection.run_config_hash.clone(),
            readiness,
        };
        state.broker_snapshot_source = BrokerSnapshotTruthSource::from_broker_kind(Some(kind));
        state.alpaca_ws_continuity = Arc::new(RwLock::new(match kind {
            BrokerKind::Alpaca => AlpacaWsContinuityState::ColdStartUnproven,
            BrokerKind::Paper => AlpacaWsContinuityState::NotApplicable,
        }));
        state.calendar_spec = calendar_spec_for_deployment(mode, Some(kind));
        // PT-DAY-01: recompute signal ingestion policy for the explicit (mode, broker) pair.
        state.strategy_market_data_source =
            if mode == DeploymentMode::Paper && kind == BrokerKind::Alpaca {
                StrategyMarketDataSource::ExternalSignalIngestion
            } else {
                StrategyMarketDataSource::NotConfigured
            };
        state
    }

    /// Test constructor: Paper+Alpaca (or any mode/broker pair) with a real DB pool.
    ///
    /// Equivalent to `new_for_test_with_mode_and_broker` but wires the given DB pool
    /// so `seed_ws_continuity_from_db` and other DB-backed paths can be exercised
    /// in integration tests (BRK-07R).
    pub fn new_for_test_with_db_mode_and_broker(
        db: PgPool,
        mode: DeploymentMode,
        kind: BrokerKind,
    ) -> Self {
        let mut state = Self::new_for_test_with_mode_and_broker(mode, kind);
        state.db = Some(db);
        state
    }

    /// Test helper: override the adapter_id in the runtime selection.
    ///
    /// Used in DB-backed tests to give each test a unique adapter_id so they
    /// can write to `broker_event_cursor` without clobbering each other when
    /// running in parallel (BRK-07R).
    pub fn set_adapter_id_for_test(&mut self, adapter_id: &str) {
        self.runtime_selection.adapter_id = adapter_id.to_string();
    }

    /// BRK-07R: Seed WS continuity state from the last persisted broker cursor.
    ///
    /// Called at daemon boot (before the WS transport task starts) to give the
    /// operator an honest view of the prior session's ending state:
    ///
    /// - **No cursor in DB** → `ColdStartUnproven` (unchanged).
    /// - **Prior `Live` cursor** → demoted to `ColdStartUnproven`.  The WS must
    ///   re-establish connectivity after restart; `Live` is not earned until
    ///   the subscription is confirmed by the server.
    /// - **Prior `GapDetected` cursor** → kept as `GapDetected` so the
    ///   BRK-00R-04 gate immediately blocks start until the gap is resolved.
    /// - **Cursor parse error** → `GapDetected` (fail-closed).
    ///
    /// No-ops when:
    /// - Broker kind is not Alpaca (not on the WS ingest path).
    /// - No DB pool is present.
    pub async fn seed_ws_continuity_from_db(&self) {
        if self.runtime_selection.broker_kind != Some(BrokerKind::Alpaca) {
            return;
        }
        let Some(pool) = self.db.as_ref() else {
            return;
        };
        let cursor_json = match mqk_db::load_broker_cursor(pool, self.adapter_id()).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "BRK-07R: failed to load broker cursor at daemon boot; \
                     leaving ColdStartUnproven"
                );
                return;
            }
        };
        // Derive continuity from the cursor JSON then demote Live → ColdStartUnproven.
        // GapDetected is preserved so the BRK-00R-04 gate immediately reflects
        // the prior gap.
        let raw = AlpacaWsContinuityState::from_cursor_json(
            self.runtime_selection.broker_kind,
            cursor_json.as_deref(),
        );
        let boot_continuity = if matches!(raw, AlpacaWsContinuityState::Live { .. }) {
            AlpacaWsContinuityState::ColdStartUnproven
        } else {
            raw.clone()
        };
        tracing::debug!(
            continuity = ?boot_continuity,
            "BRK-07R: seeded WS continuity from persisted broker cursor"
        );
        *self.alpaca_ws_continuity.write().await = boot_continuity.clone();

        if cursor_json.is_some() {
            match raw {
                AlpacaWsContinuityState::Live { .. } => {
                    self
                        .set_autonomous_session_truth(AutonomousSessionTruth::RecoveryRetrying {
                            resume_source: AutonomousRecoveryResumeSource::PersistedCursor,
                            detail: "daemon restart loaded a persisted live Alpaca cursor; WS continuity must re-establish before autonomous paper start is allowed".to_string(),
                        })
                        .await;
                }
                AlpacaWsContinuityState::GapDetected { ref detail, .. } => {
                    self
                        .set_autonomous_session_truth(AutonomousSessionTruth::RecoveryRetrying {
                            resume_source: AutonomousRecoveryResumeSource::PersistedCursor,
                            detail: format!(
                                "daemon restart resumed from persisted broker cursor with an unresolved continuity gap: {detail}"
                            ),
                        })
                        .await;
                }
                AlpacaWsContinuityState::ColdStartUnproven
                | AlpacaWsContinuityState::NotApplicable => {}
            }
        }
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

        let calendar_spec = calendar_spec_for_deployment(
            runtime_selection.deployment_mode,
            runtime_selection.broker_kind,
        );

        let broker_snapshot_source =
            BrokerSnapshotTruthSource::from_broker_kind(runtime_selection.broker_kind);

        // PT-DAY-01: ExternalSignalIngestion wired for the honest paper+alpaca path.
        // Paper+alpaca is the only deployment where the signal ingestion route is
        // configured — it is the canonical broker-backed paper execution path.
        // All other modes remain NotConfigured until their own patch slices land.
        let strategy_market_data_source = if runtime_selection.deployment_mode
            == DeploymentMode::Paper
            && runtime_selection.broker_kind == Some(BrokerKind::Alpaca)
        {
            StrategyMarketDataSource::ExternalSignalIngestion
        } else {
            StrategyMarketDataSource::NotConfigured
        };

        let initial_ws_continuity = initial_ws_continuity_for_broker(runtime_selection.broker_kind);

        let strategy_fleet = std::env::var("MQK_STRATEGY_IDS").ok().map(|ids| {
            ids.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|id| StrategyFleetEntry {
                    strategy_id: id.to_string(),
                })
                .collect::<Vec<_>>()
        });

        Self {
            bus,
            node_id: env::default_node_id(build.service),
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
            broker_snapshot_source,
            strategy_market_data_source,
            alpaca_ws_continuity: Arc::new(RwLock::new(initial_ws_continuity)),
            session_clock_override: Arc::new(RwLock::new(None)),
            gap_escalation_pending: Arc::new(AtomicBool::new(false)),
            strategy_fleet: Arc::new(RwLock::new(strategy_fleet)),
            discord_notifier: DiscordNotifier::from_env(),
            day_signal_count: Arc::new(AtomicU32::new(0)),
            accepted_artifact: Arc::new(RwLock::new(None)),
            autonomous_session_truth: Arc::new(RwLock::new(AutonomousSessionTruth::Clear)),
            autonomous_history_degraded: Arc::new(AtomicBool::new(false)),
            native_strategy_bootstrap: Arc::new(Mutex::new(None)),
            pending_strategy_bar_input: Arc::new(Mutex::new(None)),
            last_bar_input_ts: Arc::new(AtomicI64::new(0)),
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

    pub fn calendar_spec(&self) -> CalendarSpec {
        self.calendar_spec
    }

    pub fn broker_snapshot_source(&self) -> BrokerSnapshotTruthSource {
        self.broker_snapshot_source
    }

    pub fn strategy_market_data_source(&self) -> StrategyMarketDataSource {
        self.strategy_market_data_source
    }

    pub async fn alpaca_ws_continuity(&self) -> AlpacaWsContinuityState {
        self.alpaca_ws_continuity.read().await.clone()
    }

    pub async fn autonomous_session_truth(&self) -> AutonomousSessionTruth {
        self.autonomous_session_truth.read().await.clone()
    }

    pub async fn set_autonomous_session_truth(&self, truth: AutonomousSessionTruth) {
        let current = self.autonomous_session_truth.read().await.clone();
        if current == truth {
            return;
        }
        *self.autonomous_session_truth.write().await = truth.clone();
        self.persist_autonomous_session_truth_event(&truth).await;
    }

    pub async fn clear_autonomous_session_truth(&self) {
        *self.autonomous_session_truth.write().await = AutonomousSessionTruth::Clear;
    }

    /// AUTON-PAPER-03 proof seam: repair WS continuity from the current
    /// persisted Alpaca broker cursor using the same backend cursor-repair
    /// contract as the WS transport, without requiring a real network session.
    ///
    /// Narrow scope only:
    /// - valid only for Paper+Alpaca
    /// - requires a configured DB
    /// - loads the persisted cursor for the current `adapter_id`
    /// - runs `advance_cursor_after_ws_establish(...)`
    /// - updates continuity + autonomous supervisory truth honestly
    ///
    /// This does not fake WS replay and does not bypass the persisted cursor /
    /// REST catch-up recovery model.
    pub async fn repair_ws_continuity_from_persisted_cursor_for_test(
        &self,
    ) -> Result<AlpacaFetchCursor, RuntimeLifecycleError> {
        if self.deployment_mode() != DeploymentMode::Paper
            || self.runtime_selection.broker_kind != Some(BrokerKind::Alpaca)
        {
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.test_refused.not_paper_alpaca",
                "deployment_mode",
                "repair_ws_continuity_from_persisted_cursor_for_test is only valid on Paper+Alpaca",
            ));
        }

        let db = self.db_pool()?;
        let cursor_json = mqk_db::load_broker_cursor(&db, self.adapter_id())
            .await
            .map_err(|err| RuntimeLifecycleError::internal("load_broker_cursor failed", err))?;
        let prev_cursor = match cursor_json {
            Some(json) => serde_json::from_str::<AlpacaFetchCursor>(&json).map_err(|err| {
                RuntimeLifecycleError::internal("broker cursor parse failed", err)
            })?,
            None => AlpacaFetchCursor::cold_start_unproven(None),
        };

        let resume_source = match &prev_cursor.trade_updates {
            mqk_broker_alpaca::types::AlpacaTradeUpdatesResume::ColdStartUnproven => {
                AutonomousRecoveryResumeSource::ColdStart
            }
            mqk_broker_alpaca::types::AlpacaTradeUpdatesResume::GapDetected { .. }
            | mqk_broker_alpaca::types::AlpacaTradeUpdatesResume::Live { .. } => {
                AutonomousRecoveryResumeSource::PersistedCursor
            }
        };

        if matches!(
            resume_source,
            AutonomousRecoveryResumeSource::PersistedCursor
        ) {
            self.set_autonomous_session_truth(AutonomousSessionTruth::RecoveryRetrying {
                resume_source: resume_source.clone(),
                detail: "repairing WS continuity from persisted broker cursor truth".to_string(),
            })
            .await;
        }

        match mqk_runtime::alpaca_inbound::advance_cursor_after_ws_establish(
            &db,
            self.adapter_id(),
            &prev_cursor,
            Utc::now(),
        )
        .await
        {
            Ok(repaired) => {
                self.update_ws_continuity(AlpacaWsContinuityState::from_fetch_cursor(&repaired))
                    .await;
                self.set_autonomous_session_truth(AutonomousSessionTruth::RecoverySucceeded {
                    resume_source: resume_source.clone(),
                    detail: match resume_source {
                        AutonomousRecoveryResumeSource::PersistedCursor => {
                            "WS continuity restored from persisted broker cursor truth".to_string()
                        }
                        AutonomousRecoveryResumeSource::ColdStart => {
                            "WS continuity established from cold-start cursor truth".to_string()
                        }
                    },
                })
                .await;
                Ok(repaired)
            }
            Err(err) => {
                let detail =
                    format!("failed to repair WS continuity from persisted broker cursor: {err}");
                self.set_autonomous_session_truth(AutonomousSessionTruth::RecoveryFailed {
                    resume_source,
                    detail: detail.clone(),
                })
                .await;
                self.update_ws_continuity(AlpacaWsContinuityState::GapDetected {
                    last_message_id: None,
                    last_event_at: None,
                    detail: detail.clone(),
                })
                .await;
                Err(RuntimeLifecycleError::service_unavailable(
                    "runtime.recovery_refused.cursor_repair_failed",
                    detail,
                ))
            }
        }
    }

    async fn persist_autonomous_session_truth_event(&self, truth: &AutonomousSessionTruth) {
        // AUTON-HIST-01: no DB means events are permanently lost for this
        // session.  Mark degraded so the operator can see it in the readiness
        // surface rather than silently losing history.
        let Some(db) = self.db.as_ref() else {
            self.autonomous_history_degraded
                .store(true, Ordering::SeqCst);
            tracing::warn!("persist_autonomous_session_truth_event: no DB configured; autonomous supervisor history will not be persisted (autonomous_history_degraded=true)");
            return;
        };
        let Some((event_type, resume_source, detail)) = autonomous_truth_event_parts(truth) else {
            return;
        };
        let ts_utc = Utc::now();
        let run_id = self.locally_owned_run_id().await;
        let id = format!(
            "{}:{}:{}",
            ts_utc.timestamp_micros(),
            event_type,
            run_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string())
        );
        let row = mqk_db::AutonomousSessionEventRow {
            id,
            ts_utc,
            event_type: event_type.to_string(),
            resume_source,
            detail,
            run_id,
            source: "mqk-daemon.state".to_string(),
        };
        // AUTON-HIST-01: DB write failure is non-fatal to execution but must be
        // operator-visible.  Mark degraded so the readiness surface reflects the
        // true persistence state.
        if let Err(err) = mqk_db::persist_autonomous_session_event(db, &row).await {
            self.autonomous_history_degraded
                .store(true, Ordering::SeqCst);
            tracing::warn!(error = %err, "persist_autonomous_session_event failed; autonomous_history_degraded=true");
        }
    }

    /// AUTON-HIST-01: True when at least one autonomous session event could not
    /// be persisted (no DB or DB write failure).  Sticky — not reset in-session.
    pub fn autonomous_history_degraded(&self) -> bool {
        self.autonomous_history_degraded.load(Ordering::SeqCst)
    }

    /// TV-01C: Return the artifact provenance accepted at the most recent run start.
    ///
    /// `None` when no run is active, no artifact was configured, or intake was
    /// not `Accepted`.  Always fail-closed — never synthesises positive provenance.
    pub async fn accepted_artifact_provenance(&self) -> Option<AcceptedArtifactProvenance> {
        self.accepted_artifact.read().await.clone()
    }

    /// TV-01C (test seam): directly set the accepted artifact provenance.
    ///
    /// Named `_for_test` to signal intent; not called in production code.
    /// Allows TV-01D proof tests to exercise the control-plane surface without
    /// requiring a full DB-backed run start.
    pub async fn set_accepted_artifact_for_test(&self, a: Option<AcceptedArtifactProvenance>) {
        *self.accepted_artifact.write().await = a;
    }

    pub async fn update_ws_continuity(&self, new_state: AlpacaWsContinuityState) {
        let current = self.alpaca_ws_continuity.read().await.clone();
        if current == AlpacaWsContinuityState::NotApplicable {
            return;
        }
        // PT-DAY-04: When WS continuity re-establishes Live, reset the gap
        // escalation flag so the next gap window can fire a fresh notification.
        if matches!(new_state, AlpacaWsContinuityState::Live { .. }) {
            self.gap_escalation_pending.store(false, Ordering::SeqCst);
        }
        // DIS-01: Emit a critical alert on the first GapDetected transition per
        // gap window.  try_claim_gap_escalation() is an atomic swap — exactly one
        // caller fires the notification even under concurrent WS/signal paths.
        // The flag resets when continuity returns to Live (above).
        //
        // This fires at the transport level (WS disconnect detected) rather than
        // waiting for the first signal refusal, giving the operator earlier notice.
        // If the gap was loaded from a persisted cursor at boot (BRK-07R) and
        // update_ws_continuity is not called in this session, strategy.rs claims
        // the escalation on the first signal refusal instead.
        if matches!(new_state, AlpacaWsContinuityState::GapDetected { .. })
            && self.try_claim_gap_escalation()
        {
            let detail = if let AlpacaWsContinuityState::GapDetected { ref detail, .. } = new_state
            {
                Some(detail.clone())
            } else {
                None
            };
            let notifier = self.discord_notifier.clone();
            let env = Some(self.deployment_mode().as_api_label().to_string());
            let run_id = self.locally_owned_run_id().await.map(|id| id.to_string());
            let ts = Utc::now().to_rfc3339();
            tokio::spawn(async move {
                notifier
                    .notify_critical_alert(&CriticalAlertPayload {
                        alert_class: "paper.ws_continuity.gap_detected".to_string(),
                        severity: "critical".to_string(),
                        summary: "Alpaca WS gap detected; fill delivery unreliable, \
                                  signal ingestion blocked until WS re-establishes Live."
                            .to_string(),
                        detail,
                        environment: env,
                        run_id,
                        ts_utc: ts,
                    })
                    .await;
            });
        }
        *self.alpaca_ws_continuity.write().await = new_state;
    }

    /// PT-AUTO-01: Returns `true` when the execution loop should self-halt due
    /// to a WS continuity gap on the broker-backed paper path.
    ///
    /// Policy:
    /// - Only applies when `strategy_market_data_source` is
    ///   `ExternalSignalIngestion` (paper+alpaca).  Other deployment modes are
    ///   not on the WS ingest path and are not affected.
    /// - `GapDetected` → `true`: fill tracking is broken; dispatching orders
    ///   without fill confirmation is unsound.  The loop must self-halt.
    /// - `ColdStartUnproven` → `false`: boot-time state expected before the
    ///   first WS session confirms subscription.  Signals are blocked at the
    ///   route layer (PT-DAY-02) but the execution loop itself is not yet
    ///   running in that state so a mid-loop halt is not applicable.
    /// - `Live` → `false`: WS continuity confirmed; normal operation.
    /// - `NotApplicable` → `false`: non-Alpaca path; WS continuity does not
    ///   apply to this deployment.
    pub async fn ws_continuity_gap_requires_halt(&self) -> bool {
        if self.strategy_market_data_source != StrategyMarketDataSource::ExternalSignalIngestion {
            return false;
        }
        matches!(
            *self.alpaca_ws_continuity.read().await,
            AlpacaWsContinuityState::GapDetected { .. }
        )
    }

    /// PT-AUTO-01B proof seam: constructs a minimal `DaemonOrchestrator` and
    /// runs the real execution loop until it exits naturally, then returns the
    /// loop exit note.
    ///
    /// Construction details:
    /// - `DaemonBroker::Paper` — no Alpaca credentials required.
    /// - Lazy `PgPool` — no real DB connection at construction time.
    ///   `release_runtime_leadership()` will fail (no real DB) and is logged as
    ///   `tracing::warn!` — that is expected and does not affect the proof.
    /// - `AppState.db` must be `None` in the caller (as it is for all
    ///   `new_for_test_with_*` constructors) so the deadman block is skipped
    ///   each tick and PT-AUTO-01 fires unobstructed.
    /// - `StateIntegrityGate` and `ReconcileTruthGate` are wired to `self`'s
    ///   arcs so halt effects (ig.disarmed, ig.halted) are observable on the
    ///   same AppState the caller inspects after the loop exits.
    ///
    /// Loop exit timing:
    /// - GapDetected path (PT-AUTO-01): exits before `orchestrator.tick()`,
    ///   within the first tick interval (~1 second).
    /// - Live / non-gap path: exits when `orchestrator.tick()` Phase-0 hits
    ///   the DB check on the lazy/disconnected pool (also ~1 second).
    ///
    /// Called only from PT-AUTO-01B proof tests.  Never called in production.
    /// Not cfg-gated: follows the `_for_test` naming convention established by
    /// `set_session_clock_ts_for_test` and `set_strategy_fleet_for_test`.
    pub async fn run_loop_one_tick_for_test(
        self: &Arc<Self>,
        run_id: uuid::Uuid,
    ) -> Option<String> {
        use std::collections::BTreeMap;

        use mqk_broker_paper::LockedPaperBroker;
        use mqk_execution::BrokerOrderMap;
        use mqk_portfolio::PortfolioState;
        use mqk_reconcile::{BrokerSnapshot, LocalSnapshot};
        use mqk_runtime::orchestrator::WallClock;
        use mqk_runtime::runtime_risk::RuntimeRiskGate;

        let integrity_gate = types::StateIntegrityGate {
            integrity: Arc::clone(&self.integrity),
        };
        let reconcile_gate = types::ReconcileTruthGate {
            reconcile_status: Arc::clone(&self.reconcile_status),
        };
        let risk_gate =
            RuntimeRiskGate::from_run_config(&serde_json::json!({}), 1_000_000_000_i64, 0, 0);
        let daemon_broker = broker::DaemonBroker::Paper(LockedPaperBroker::default());
        let gateway = mqk_execution::wiring::build_gateway(
            daemon_broker,
            integrity_gate,
            risk_gate,
            reconcile_gate,
        );
        // Lazy pool — constructed without connecting.  Only accessed by
        // orchestrator.release_runtime_leadership() in the halt path; that call
        // fails and is logged as tracing::warn! — expected and harmless.
        let pool = sqlx::PgPool::connect_lazy("postgresql://127.0.0.1:5432/mqk_ptauto01b_stub")
            .expect("connect_lazy URL parse must succeed");

        let orchestrator = types::DaemonOrchestrator::new(
            pool,
            gateway,
            BrokerOrderMap::new(),
            BTreeMap::new(),
            PortfolioState::new(1_000_000_000_i64),
            run_id,
            "ptauto01b-dispatcher",
            "ptauto01b",
            None,
            WallClock,
            Box::new(LocalSnapshot::empty),
            Box::new(|| BrokerSnapshot::empty_at(0)),
        );

        // `self` is an Arc — clone it directly for spawn_execution_loop.
        // The deadman block inside the loop checks `db` (from state.db), which
        // is None for all new_for_test_with_* AppState constructors, so the
        // deadman is skipped and PT-AUTO-01 fires clean on GapDetected.
        let handle = loop_runner::spawn_execution_loop(Arc::clone(self), orchestrator, run_id);

        // Await the loop exit.  Resolves as soon as the loop terminates.
        match handle.join_handle.await {
            Ok(exit) => exit.note,
            Err(_) => Some("join error".to_string()),
        }
    }

    /// PT-DAY-04: Attempt to claim the gap escalation for this gap window.
    ///
    /// Returns `true` on the first call after a gap begins (i.e., the caller
    /// should fire an operator notification).  Returns `false` on all subsequent
    /// calls until `update_ws_continuity(Live)` resets the flag.
    ///
    /// Uses an atomic swap so concurrent signal POSTs during the same gap window
    /// are safe: exactly one caller receives `true`.
    pub(crate) fn try_claim_gap_escalation(&self) -> bool {
        // swap(true) → returns the old value.  If old value was false this is
        // the first claim; return true (caller should notify).  If old value
        // was already true, return false (already notified; caller should not).
        !self.gap_escalation_pending.swap(true, Ordering::SeqCst)
    }

    /// PT-DAY-04: Returns `true` when a gap escalation has been claimed and not
    /// yet cleared by a Live transition.  Used by proof tests.
    pub fn gap_escalation_is_pending(&self) -> bool {
        self.gap_escalation_pending.load(Ordering::SeqCst)
    }

    // ---------------------------------------------------------------------------
    // PT-AUTO-02: Per-run autonomous signal intake bound
    // ---------------------------------------------------------------------------

    /// Returns the current per-run signal intake count.
    pub fn day_signal_count(&self) -> u32 {
        self.day_signal_count.load(Ordering::SeqCst)
    }

    /// Increment the per-run signal intake counter by one.
    ///
    /// Called from the strategy signal route on Gate 7 Ok(true) (new enqueue).
    /// Not called for duplicates (Ok(false)) or Gate failures.
    pub(crate) fn increment_day_signal_count(&self) {
        self.day_signal_count.fetch_add(1, Ordering::SeqCst);
    }

    /// Returns `true` when the per-run signal count has reached
    /// `MAX_AUTONOMOUS_SIGNALS_PER_RUN`.  Gate 1d refuses signals when true.
    pub fn day_signal_limit_exceeded(&self) -> bool {
        self.day_signal_count.load(Ordering::SeqCst) >= MAX_AUTONOMOUS_SIGNALS_PER_RUN
    }

    /// Test seam: set the day signal count to an arbitrary value.
    ///
    /// Named `_for_test` to signal intent; never called in production code.
    /// Used by PT-AUTO-02 proof tests to simulate a saturated counter without
    /// submitting 100 real signals.
    pub fn set_day_signal_count_for_test(&self, count: u32) {
        self.day_signal_count.store(count, Ordering::SeqCst);
    }

    /// PT-DAY-03: Returns the current wall-clock Unix timestamp used by the
    /// NYSE session gate in `strategy_signal`.
    ///
    /// Returns the injected override if one has been set (test-only seam);
    /// otherwise returns `Utc::now().timestamp()`.  Not in the [T] guard scope
    /// (that guard covers `mqk-db/src/` only).
    pub(crate) async fn session_now_ts(&self) -> i64 {
        if let Some(ts) = *self.session_clock_override.read().await {
            return ts;
        }
        chrono::Utc::now().timestamp() // allow: session-gate wall-clock
    }

    /// PT-DAY-03 test seam: inject a fixed timestamp for the NYSE session gate.
    ///
    /// Call before routing a request to make session-gate proof tests hermetic.
    /// Named `_for_test` to signal intent; never called in production code.
    /// Follows the same pattern as `set_strategy_fleet_for_test`.
    pub async fn set_session_clock_ts_for_test(&self, ts: i64) {
        *self.session_clock_override.write().await = Some(ts);
    }

    pub async fn strategy_fleet_snapshot(&self) -> Option<Vec<StrategyFleetEntry>> {
        self.strategy_fleet.read().await.clone()
    }

    pub async fn set_strategy_fleet_for_test(&self, fleet: Option<Vec<StrategyFleetEntry>>) {
        *self.strategy_fleet.write().await = fleet;
    }

    /// B1A test seam: read the current native strategy bootstrap truth state.
    ///
    /// Returns `None` if no bootstrap is stored (no active run).
    /// Returns `Some("dormant" | "active" | "failed")` when a run is active.
    /// Named `_for_test` to signal intent; never called in production code.
    pub async fn native_strategy_bootstrap_truth_state_for_test(&self) -> Option<&'static str> {
        self.native_strategy_bootstrap
            .lock()
            .await
            .as_ref()
            .map(|b| b.truth_state())
    }

    /// B1B test seam: inject a pre-built bootstrap for testing dispatch logic.
    ///
    /// Named `_for_test` to signal intent; never called in production code.
    pub async fn set_native_strategy_bootstrap_for_test(
        &self,
        bootstrap: Option<NativeStrategyBootstrap>,
    ) {
        *self.native_strategy_bootstrap.lock().await = bootstrap;
    }

    /// B1B: Invoke the native strategy `on_bar` callback from raw bar parameters.
    ///
    /// Fail-closed: no bootstrap stored (no active run) → `None`, no callback.
    /// Fail-closed: bootstrap is Dormant or Failed → `None`, no callback.
    /// Returns `Some(StrategyBarResult)` when the bootstrap is Active and the
    /// dispatch succeeds.
    ///
    /// Called by `tick_strategy_dispatch` (canonical loop path) and kept `pub`
    /// as a secondary test-seam.  Production `on_bar` dispatch flows through
    /// `tick_strategy_dispatch` (runtime-owned); direct callers are test-only.
    ///
    /// The result carries shadow-mode intents (B1B constraint: shadow mode until
    /// the decision submission bridge is wired in B1C).
    pub async fn invoke_native_strategy_on_bar_from_signal(
        &self,
        now_tick: u64,
        end_ts: i64,
        limit_price: Option<i64>,
        qty: i64,
    ) -> Option<mqk_strategy::StrategyBarResult> {
        self.native_strategy_bootstrap
            .lock()
            .await
            .as_mut()?
            .invoke_on_bar_from_signal(now_tick, end_ts, limit_price, qty)
    }

    /// B1B: Deposit a bar input for the execution loop to consume on its next tick.
    ///
    /// Called by the signal route (ExternalSignalIngestion path) after Gate 6.
    /// The execution loop's `tick_strategy_dispatch` is the canonical consumer —
    /// `on_bar` fires in the loop's tick context, not in the HTTP handler.
    ///
    /// Overwrite policy: a new deposit supersedes any prior unconsumed bar.
    /// B3: Also captures `input.end_ts` in `last_bar_input_ts` for telemetry.
    pub async fn deposit_strategy_bar_input(&self, input: StrategyBarInput) {
        self.last_bar_input_ts.store(input.end_ts, Ordering::SeqCst);
        *self.pending_strategy_bar_input.lock().await = Some(input);
    }

    /// B3: Returns the Unix-seconds timestamp of the last bar input deposit.
    ///
    /// Zero when no bar input has been deposited in this daemon process lifetime.
    /// Not cleared on run stop/start — reflects the last deposit ever made.
    /// Read by `/api/v1/strategy/summary` to surface honest `last_decision_time`.
    pub fn last_bar_input_ts(&self) -> i64 {
        self.last_bar_input_ts.load(Ordering::SeqCst)
    }

    /// B1B: Execution loop dispatch seam — take pending bar input and invoke `on_bar`.
    ///
    /// Called exclusively by the execution loop on each tick.  The loop is the
    /// canonical runtime-owned `on_bar` dispatch owner.
    ///
    /// Returns `Some(StrategyBarResult)` when a bar was pending AND the bootstrap
    /// is Active.  Returns `None` on most ticks (no pending bar), and when the
    /// bootstrap is absent / Dormant / Failed — all fail-closed.
    ///
    /// The result carries shadow-mode intents in B1B; B1C wires it to the outbox.
    pub async fn tick_strategy_dispatch(&self) -> Option<mqk_strategy::StrategyBarResult> {
        let bar = self.pending_strategy_bar_input.lock().await.take()?;
        self.invoke_native_strategy_on_bar_from_signal(
            bar.now_tick,
            bar.end_ts,
            bar.limit_price,
            bar.qty,
        )
        .await
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

    /// CC-03B: Load the most recent pending restart intent for this daemon engine.
    ///
    /// Returns `None` in two honest cases:
    /// - No DB pool configured on this daemon instance.
    /// - DB is present but no pending restart intent row exists for this engine.
    ///
    /// `None` must not be interpreted as "no restart was ever intended" —
    /// it only means no durable pending intent is recorded at this moment.
    /// Callers must not synthesise a restart intent from transient state when
    /// this returns `None`.
    pub async fn load_pending_restart_intent(&self) -> Option<mqk_db::RestartIntentRow> {
        let db = self.db.as_ref()?;
        mqk_db::fetch_pending_restart_intent_for_engine(db, DAEMON_ENGINE_ID)
            .await
            .ok()
            .flatten()
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

        if self.deployment_mode() == DeploymentMode::LiveCapital
            && !matches!(self.operator_auth, OperatorAuthMode::TokenRequired(_))
        {
            return Err(RuntimeLifecycleError::forbidden(
                "runtime.start_refused.capital_requires_operator_token",
                "operator_auth",
                "live-capital mode requires a real operator token; \
                 dev-no-token and missing-token modes are not permitted for capital execution",
            ));
        }

        if let Some(run_id) = self.active_owned_run_id().await {
            return Err(RuntimeLifecycleError::conflict(
                "runtime.control_refusal.already_owned",
                format!("runtime already active under local ownership: {run_id}"),
            ));
        }

        // BRK-00R-04: paper+alpaca WS continuity start gate.
        //
        // The Alpaca paper path requires proven WS continuity before runtime start.
        // ColdStartUnproven and GapDetected are not start-safe: no live WS cursor
        // has been established, so event delivery ordering cannot be trusted.
        //
        // Placed before db_pool() so the check is:
        //   - at the earliest honest enforcement point (continuity state is in-memory)
        //   - in-process testable without a database
        //   - before any DB resources or runtime lease are acquired
        //
        // Full WS transport implementation (subscribe/reconnect/cursor establishment)
        // remains open; this patch only moves the failure forward from first tick.
        if self.deployment_mode() == DeploymentMode::Paper
            && self.runtime_selection.broker_kind == Some(BrokerKind::Alpaca)
        {
            let continuity = self.alpaca_ws_continuity().await;
            if !continuity.is_continuity_proven() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.paper_alpaca_ws_continuity_unproven",
                    "alpaca_ws_continuity",
                    format!(
                        "paper+alpaca requires proven Alpaca WS continuity before starting; \
                         current state: '{}' (WS_CONTINUITY_UNPROVEN) — the WS transport \
                         must establish a live cursor before paper+alpaca can proceed; \
                         full WS transport work remains open",
                        continuity.as_status_str()
                    ),
                ));
            }
        }

        // BRK-09R: Reconcile truth start gate for broker-backed paper path.
        //
        // If the persisted reconcile status is "dirty" or "stale" — meaning the
        // prior session ended with a known broker/local drift condition — refuse
        // start until the operator has investigated and the reconcile state is
        // cleared to "ok" (or the DB row is absent, meaning no prior evidence).
        //
        // "unknown" is the initial in-memory state at fresh boot (no prior run),
        // and is allowed through: it carries no evidence of prior drift.
        //
        // Gate ordering: fires after the WS continuity gate so WS issues are
        // surfaced first.  A dirty reconcile AND a non-live WS yields the WS gate
        // as the named blocker; reconcile is only surfaced when WS is clean.
        //
        // current_reconcile_snapshot() reads from DB when available, falling back
        // to in-memory; it does not require db_pool() to be non-None.
        if self.deployment_mode() == DeploymentMode::Paper
            && self.runtime_selection.broker_kind == Some(BrokerKind::Alpaca)
        {
            let reconcile = self.current_reconcile_snapshot().await;
            if matches!(reconcile.status.as_str(), "dirty" | "stale") {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.reconcile_dirty",
                    "reconcile_truth",
                    format!(
                        "paper+alpaca cannot start with dirty or stale reconcile truth; \
                         current reconcile status: '{}'; the prior session's broker/local \
                         drift must be investigated and the reconcile state must be cleared \
                         before restarting; reconcile note: {}",
                        reconcile.status,
                        reconcile.note.as_deref().unwrap_or("none"),
                    ),
                ));
            }
        }

        // Live-capital WS continuity gate.
        //
        // Placed here — before db_pool() — so it is:
        //   - in-process testable without a database or real broker credentials
        //   - before any DB resources or run rows are acquired (prevents dangling
        //     "Created" run rows on a continuity-refused start)
        //   - symmetric with the Paper+Alpaca continuity gate above
        //
        // Previous position (after build_execution_orchestrator) required
        // orchestrator.release_runtime_leadership() on failure and could leave
        // a "Created" run row in the DB if the check failed after insert_run.
        if self.deployment_mode() == DeploymentMode::LiveCapital {
            let continuity = self.alpaca_ws_continuity().await;
            if !continuity.is_continuity_proven() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.capital_ws_continuity_unproven",
                    "alpaca_ws_continuity",
                    format!(
                        "live-capital requires proven Alpaca WS continuity before starting; \
                         current continuity state: '{}' — \
                         run in live-shadow mode to establish a proven cursor, \
                         then transition to capital",
                        continuity.as_status_str()
                    ),
                ));
            }
        }

        // TV-01 / TV-02C: Evaluate artifact intake exactly once.
        //
        // Hoisted here so the same evaluation result is used for:
        //   - TV-02C deployability gate (below)
        //   - TV-01C provenance capture at successful run start (further below)
        //
        // Evaluating twice would create a TOCTOU window: a file swap or env-var
        // mutation between the gate check and the provenance capture could let
        // a different artifact identity pass the gate while a different one is
        // recorded as the run's provenance.  Single evaluation closes that gap.
        let artifact_intake = evaluate_artifact_intake_guarded();

        // TV-02C: Artifact deployability gate.
        //
        // If MQK_ARTIFACT_PATH is configured and intake is Accepted, the artifact
        // must also pass the deployability gate (deployability_gate.json written by
        // the Python TV-02 pipeline) before runtime start is allowed.
        //
        // Contract:
        //   NotConfigured            → no artifact configured; gate not applicable; pass through.
        //   Accepted + Deployable    → minimum criteria met; pass through.
        //   Accepted + not Deployable→ fail-closed: block start with explicit reason.
        //   Invalid / Unavailable   → artifact configured but intake failed; fail-closed.
        //
        // Placed before db_pool() so it is:
        //   - in-process testable without a database
        //   - before any DB resources or run rows are acquired (no dangling rows on refusal)
        {
            match &artifact_intake {
                ArtifactIntakeOutcome::NotConfigured => {
                    // No artifact configured — deployability gate not applicable.
                }
                ArtifactIntakeOutcome::Accepted { artifact_id, .. } => {
                    let raw = std::env::var(ENV_ARTIFACT_PATH).unwrap_or_default();
                    let manifest_path = std::path::PathBuf::from(raw.trim());
                    let deployability =
                        evaluate_artifact_deployability(Some(&manifest_path), artifact_id);
                    if !deployability.is_deployable() {
                        return Err(RuntimeLifecycleError::forbidden(
                            "runtime.start_refused.artifact_not_deployable",
                            "artifact_deployability",
                            format!(
                                "configured artifact failed the deployability gate \
                                 (truth_state='{}'): artifact_id='{}' was accepted for intake \
                                 but did not pass minimum deployability/tradability criteria; \
                                 run the TV-02 Python gate on this artifact to produce a \
                                 deployability_gate.json that passes all checks",
                                deployability.truth_state(),
                                artifact_id,
                            ),
                        ));
                    }
                }
                ArtifactIntakeOutcome::Invalid { reason } => {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.artifact_intake_invalid",
                        "artifact_intake",
                        format!(
                            "artifact intake failed; runtime cannot proceed with a configured \
                             but invalid artifact: {reason}"
                        ),
                    ));
                }
                ArtifactIntakeOutcome::Unavailable { reason } => {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.artifact_intake_unavailable",
                        "artifact_intake",
                        format!(
                            "artifact intake evaluator failed; runtime cannot proceed when \
                             artifact state is unknown: {reason}"
                        ),
                    ));
                }
            }
        }

        // TV-03C: Parity evidence gate.
        //
        // If MQK_ARTIFACT_PATH is configured, parity evidence for the artifact
        // must exist in the same directory and be structurally valid before
        // runtime start is allowed.
        //
        // Contract:
        //   NotConfigured   → no artifact path configured; gate not applicable; pass through.
        //   Present { .. }  → parity_evidence.json readable and valid; pass through.
        //   Absent          → configured artifact has no parity evidence; fail-closed.
        //   Invalid { .. }  → parity_evidence.json exists but is invalid; fail-closed.
        //   Unavailable { .. } → evaluator failed; fail-closed.
        //
        // Placed after TV-02C (artifact deployability) and before TV-04A (capital policy)
        // so the evidence chain is verified before capital authorization runs.
        // Both TV-02C and TV-03C read MQK_ARTIFACT_PATH; absent path → NotConfigured on both.
        //
        // Cross-validation: when both intake and parity evidence are resolved, the
        // artifact_id embedded in parity_evidence.json must match the accepted intake
        // artifact_id.  This mirrors the TV-02C deployability gate cross-validation and
        // closes the artifact-associated evidence chain: parity evidence produced for a
        // different artifact must not satisfy this gate.  `artifact_intake` is the same
        // evaluation result used for TV-02C above (TOCTOU-safe, evaluated once).
        {
            let parity = evaluate_parity_evidence_from_env();
            // Artifact identity cross-validation: Present evidence for a different
            // artifact is not evidence for this artifact.
            if let (
                ArtifactIntakeOutcome::Accepted {
                    artifact_id: ref accepted_id,
                    ..
                },
                ParityEvidenceOutcome::Present {
                    artifact_id: ref parity_id,
                    ..
                },
            ) = (&artifact_intake, &parity)
            {
                if parity_id != accepted_id {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.parity_evidence_artifact_mismatch",
                        "parity_evidence",
                        format!(
                            "parity evidence artifact_id '{}' does not match the accepted \
                             intake artifact_id '{}'; the parity_evidence.json in the artifact \
                             directory was not produced for the configured artifact — re-run the \
                             TV-03 pipeline against the correct artifact",
                            parity_id, accepted_id
                        ),
                    ));
                }
            }
            if !parity.is_start_safe() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.parity_evidence_not_present",
                    "parity_evidence",
                    format!(
                        "parity evidence gate failed \
                         (truth_state='{}'): {}",
                        parity.truth_state(),
                        match &parity {
                            ParityEvidenceOutcome::Absent => {
                                "parity_evidence.json is absent in the artifact directory; \
                                 run the Python TV-03 pipeline to produce parity evidence \
                                 before starting the runtime"
                                    .to_string()
                            }
                            ParityEvidenceOutcome::Invalid { reason } => {
                                format!("parity_evidence.json is structurally invalid: {reason}")
                            }
                            ParityEvidenceOutcome::Unavailable { reason } => {
                                format!("parity evidence evaluator failed: {reason}")
                            }
                            _ => "parity evidence evaluation failed".to_string(),
                        }
                    ),
                ));
            }
        }

        // TV-04F: Live-capital requires an explicit capital allocation policy.
        //
        // Paper and LiveShadow modes are permissive: absent policy →
        // NotConfigured → gate not applicable; callers pass through.  This is
        // correct for simulation modes where capital policy enforcement is
        // optional at the operator's discretion.
        //
        // LiveCapital is semantically distinct: real capital requires an
        // explicit, operator-configured capital allocation policy before any
        // live-capital execution is authorized.  NotConfigured in live-capital
        // mode is fail-closed — the operator must explicitly configure and
        // enable a policy.  This prevents silent conflation of paper-safe
        // "no policy = no enforcement" with live-capital authorization.
        //
        // Gate ordering: placed after TV-03C (parity evidence) and before
        // TV-04A (policy validity check).  TV-04A then validates the policy
        // is enabled and structurally correct once TV-04F confirms it exists.
        if self.deployment_mode() == DeploymentMode::LiveCapital {
            let policy = evaluate_capital_policy_from_env();
            if matches!(policy, CapitalPolicyOutcome::NotConfigured) {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.live_capital_requires_capital_policy",
                    "live_capital_policy_required",
                    "live-capital mode requires an explicit capital allocation policy; \
                     set MQK_CAPITAL_POLICY_PATH to a valid capital_allocation_policy.json \
                     before starting live-capital execution; paper and live-shadow modes \
                     do not require a policy — this gate is live-capital-only and enforces \
                     the semantic distinction between paper safety and live-capital authorization",
                ));
            }
        }

        // TV-04A: Capital allocation policy gate.
        //
        // If MQK_CAPITAL_POLICY_PATH is configured, the policy file must be
        // valid and `enabled = true` before runtime start is allowed.
        //
        // Contract:
        //   NotConfigured → no policy configured; gate not applicable; pass through.
        //   Authorized    → policy valid and enabled; pass through.
        //   Denied        → policy present but enabled=false; fail-closed.
        //   PolicyInvalid → policy configured but structurally invalid; fail-closed.
        //   Unavailable   → reserved; fail-closed.
        //
        // Placed before db_pool() so the check is:
        //   - in-process testable without a database
        //   - before any DB resources or run rows are acquired (no dangling rows)
        //   - ordered after TV-02C (artifact deployability) so artifact refusals
        //     are surfaced before capital policy refusals
        {
            let policy = evaluate_capital_policy_from_env();
            if !policy.is_start_safe() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.capital_policy_not_authorized",
                    "capital_allocation_policy",
                    format!(
                        "capital allocation policy gate failed \
                         (truth_state='{}'): {}",
                        policy.truth_state(),
                        match &policy {
                            CapitalPolicyOutcome::Denied { reason } => reason.clone(),
                            CapitalPolicyOutcome::PolicyInvalid { reason } => {
                                format!("policy file is invalid: {reason}")
                            }
                            CapitalPolicyOutcome::Unavailable { reason } => {
                                format!("policy evaluator unavailable: {reason}")
                            }
                            _ => "capital policy evaluation failed".to_string(),
                        }
                    ),
                ));
            }
        }

        // TV-04D: Deployment economics gate.
        //
        // An enabled capital policy must carry a valid `max_portfolio_notional_usd`
        // before runtime start is allowed.  This gate is independent of TV-04A:
        // TV-04A checks whether the policy is enabled; TV-04D checks whether the
        // enabled policy specifies deployment economics bounds.
        //
        // Contract:
        //   NotConfigured      → no policy configured; gate not applicable; pass through.
        //   PolicyDisabled     → enabled=false; TV-04A already blocked; pass through.
        //   EconomicsSpecified → policy enabled + valid portfolio cap; pass through.
        //   EconomicsNotSpecified → policy enabled but no economics bound; fail-closed.
        //   PolicyInvalid      → policy configured but structurally invalid; fail-closed.
        //   Unavailable        → reserved; fail-closed.
        //
        // Placed immediately after TV-04A so that capital policy authorization
        // is confirmed before the economics bound is checked.  Placed before
        // db_pool() so the check is in-process testable without a database.
        {
            let economics = evaluate_deployment_economics_from_env();
            if !economics.is_start_safe() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.deployment_economics_not_specified",
                    "deployment_economics",
                    format!(
                        "deployment economics gate failed \
                         (truth_state='{}'): {}",
                        economics.truth_state(),
                        match &economics {
                            DeploymentEconomicsOutcome::EconomicsNotSpecified { reason } => {
                                reason.clone()
                            }
                            DeploymentEconomicsOutcome::PolicyInvalid { reason } => {
                                format!("economics policy file is invalid: {reason}")
                            }
                            DeploymentEconomicsOutcome::Unavailable { reason } => {
                                format!("economics evaluator unavailable: {reason}")
                            }
                            _ => "deployment economics evaluation failed".to_string(),
                        }
                    ),
                ));
            }
        }

        // B1A: Native strategy bootstrap gate.
        //
        // Evaluate the native strategy bootstrap from fleet truth (MQK_STRATEGY_IDS)
        // and the daemon plugin registry before acquiring any DB resources.
        //
        // Contract:
        //   Dormant (fleet absent/empty) → pass through.
        //   Active (fleet entry + registry match) → pass through; bootstrap stored.
        //   Failed (fleet entry present, not in registry) → fail-closed.
        //
        // Placed before db_pool() so it is:
        //   - in-process testable without a database
        //   - before any DB resources or run rows are acquired (no dangling rows)
        //   - ordered after all deployment/capital/policy gates (last pre-DB gate)
        //
        // The bootstrap is kept as a local binding and stored in AppState only
        // after a fully successful run start so the field is never left populated
        // by a failed start attempt.
        let native_strategy_bootstrap = {
            let fleet_ids = self.strategy_fleet_snapshot().await.map(|entries| {
                entries
                    .into_iter()
                    .map(|e| e.strategy_id)
                    .collect::<Vec<_>>()
            });
            let registry = build_daemon_plugin_registry();
            let bootstrap = NativeStrategyBootstrap::bootstrap(fleet_ids.as_deref(), &registry);
            if bootstrap.is_failed() {
                return Err(RuntimeLifecycleError::forbidden(
                    "runtime.start_refused.native_strategy_bootstrap_failed",
                    "native_strategy_bootstrap",
                    format!(
                        "native strategy bootstrap failed (truth_state='{}'): {}; \
                         ensure the strategy named in MQK_STRATEGY_IDS is registered \
                         in the daemon plugin registry before starting; \
                         operators must not set MQK_STRATEGY_IDS until the target \
                         strategy engine is wired into the registry",
                        bootstrap.truth_state(),
                        bootstrap.failure_reason().unwrap_or("unknown"),
                    ),
                ));
            }
            bootstrap
        };

        let db = self.db_pool()?;

        // B2A: DB strategy registry gate.
        //
        // When a native strategy is Active (plugin bootstrap passed), the strategy
        // must also be present AND enabled in the durable sys_strategy_registry.
        // This is the final activation authority: plugin presence is necessary but
        // not sufficient — registry truth is authoritative.
        //
        // Contract:
        //   Dormant bootstrap    → skip (no fleet configured; allowed).
        //   Active + enabled     → pass through.
        //   Active + disabled    → fail-closed (403, gate=strategy_registry).
        //   Active + missing     → fail-closed (403, gate=strategy_registry).
        //   Active + DB error    → fail-closed (503, gate=strategy_registry).
        //
        // Placed immediately after db_pool() so the gate runs once, before any
        // run rows are created or leadership is acquired.
        if let Some(strategy_id) = native_strategy_bootstrap.active_strategy_id() {
            match mqk_db::fetch_strategy_registry_entry(&db, strategy_id).await {
                Ok(Some(record)) if record.enabled => {
                    // Registered and enabled — pass through.
                }
                Ok(Some(_record)) => {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.strategy_registry_disabled",
                        "strategy_registry",
                        format!(
                            "native strategy '{strategy_id}' is registered but disabled \
                             in the strategy registry; enable the strategy in \
                             sys_strategy_registry before starting",
                        ),
                    ));
                }
                Ok(None) => {
                    return Err(RuntimeLifecycleError::forbidden(
                        "runtime.start_refused.strategy_registry_missing",
                        "strategy_registry",
                        format!(
                            "native strategy '{strategy_id}' is not registered in \
                             the strategy registry; insert an enabled row in \
                             sys_strategy_registry before starting",
                        ),
                    ));
                }
                Err(err) => {
                    return Err(RuntimeLifecycleError::internal(
                        "start strategy_registry lookup failed",
                        err,
                    ));
                }
            }
        }

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

        if let Ok(initial_snapshot) = orchestrator.snapshot().await {
            *self.execution_snapshot.write().await = Some(initial_snapshot);
        }

        // PT-AUTO-02: reset per-run signal intake counter at each new start so
        // the bound applies per execution run, not per daemon process lifetime.
        self.day_signal_count.store(0, Ordering::SeqCst);

        // TV-01C: capture artifact provenance at run start.
        //
        // Uses the artifact intake result evaluated once above (TV-01 hoist) —
        // the same identity that passed all pre-DB gates is the identity recorded
        // as this run's provenance.  No second evaluation; TOCTOU gap closed.
        //
        // Only `Accepted` carries positive provenance; all other outcomes leave
        // `accepted_artifact` as `None` (fail-closed: absent/invalid/unavailable
        // artifacts are not recorded as consumed).
        {
            let provenance = match artifact_intake {
                ArtifactIntakeOutcome::Accepted {
                    artifact_id,
                    artifact_type,
                    stage,
                    produced_by,
                } => Some(AcceptedArtifactProvenance {
                    artifact_id,
                    artifact_type,
                    stage,
                    produced_by,
                }),
                _ => None,
            };
            *self.accepted_artifact.write().await = provenance;
        }

        // B1A: store native strategy bootstrap for the active run.
        // Placed after all DB operations and the initial tick succeed so the
        // field is only populated when the run is fully live.
        *self.native_strategy_bootstrap.lock().await = Some(native_strategy_bootstrap);

        let handle = loop_runner::spawn_execution_loop(Arc::clone(self), orchestrator, run_id);
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

        // TV-01C: clear artifact provenance on stop — no active run means no active artifact.
        *self.accepted_artifact.write().await = None;
        // B1A: clear native strategy bootstrap on stop — host is not active without a run.
        *self.native_strategy_bootstrap.lock().await = None;

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

        // TV-01C: clear artifact provenance on halt — no active run means no active artifact.
        *self.accepted_artifact.write().await = None;
        // B1A: clear native strategy bootstrap on halt — host is not active without a run.
        *self.native_strategy_bootstrap.lock().await = None;

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

    /// AUTON-PAPER-01B: Pre-session autonomous arm seam.
    ///
    /// Attempts to advance in-memory integrity state from disarmed to armed by
    /// reading the persisted arm state from the DB.  Called by the autonomous
    /// session controller immediately before `start_execution_runtime` so the
    /// daily session can start without a manual operator arm.
    ///
    /// # Gate rules (fail-closed ordering)
    ///
    /// 1. `integrity.halted == true` → refuse unconditionally (operator halt
    ///    wins; not reversible by the controller).
    /// 2. `integrity.disarmed == false` → already armed; return `Ok(())`.
    /// 3. No DB configured → refuse (cannot verify prior session state).
    /// 4. No DB row → refuse (first-time install; operator must arm once).
    /// 5. DB state = `"ARMED"` → auto-arm: set `disarmed=false, halted=false`,
    ///    re-persist `Armed`, return `Ok(())`.
    /// 6. DB state = anything else (`"DISARMED"`) → refuse with stored reason.
    ///
    /// # Daily-cycle property
    ///
    /// `stop_execution_runtime` does NOT write `Disarmed` to the DB, so after a
    /// clean daily stop the DB remains `ARMED`.  On the next daemon boot the
    /// in-memory integrity state starts as `disarmed=true` (fail-closed), but
    /// the DB row carries the prior `ARMED` state → auto-arm succeeds → the
    /// session controller can start the next day without operator intervention.
    ///
    /// Only `halt_execution_runtime` writes `DISARMED` to the DB.  A halted
    /// daemon therefore requires manual operator arm before the controller can
    /// restart, which is the correct safety posture.
    pub async fn try_autonomous_arm(&self) -> Result<(), String> {
        // Gate 1: operator halt wins unconditionally.
        // Gate 2: already armed is idempotent success.
        {
            let ig = self.integrity.read().await;
            if ig.halted {
                return Err(
                    "operator halt asserted; autonomous arm refused (integrity.halted=true)"
                        .to_string(),
                );
            }
            if !ig.disarmed {
                return Ok(());
            }
        }

        // Gate 3: DB required to verify prior session state.
        let db = match self.db.as_ref() {
            Some(db) => db,
            None => {
                return Err(
                    "no DB configured; autonomous arm requires persisted arm state".to_string(),
                )
            }
        };

        // Gate 4/5/6: load prior arm state from the singleton row.
        let row = mqk_db::load_arm_state(db)
            .await
            .map_err(|err| format!("autonomous arm: load_arm_state failed: {err}"))?;

        match row {
            None => Err(
                "no prior arm state in DB; operator must arm manually at least once \
                 (first-time install or DB was wiped)"
                    .to_string(),
            ),
            Some((ref state_str, _)) if state_str == "ARMED" => {
                // Prior session ended cleanly (stop does not write DISARMED).
                // Advance in-memory integrity to armed.
                {
                    let mut ig = self.integrity.write().await;
                    ig.disarmed = false;
                    ig.halted = false;
                }
                // Re-persist Armed so another daemon restart also sees ARMED.
                mqk_db::persist_arm_state_canonical(db, mqk_db::ArmState::Armed, None)
                    .await
                    .map_err(|err| {
                        format!("autonomous arm: persist_arm_state_canonical failed: {err}")
                    })?;
                Ok(())
            }
            Some((_, reason)) => {
                let reason_str = reason.as_deref().unwrap_or("unknown");
                Err(format!(
                    "DB arm state is DISARMED (reason={reason_str}); autonomous arm refused"
                ))
            }
        }
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

    pub(crate) async fn lifecycle_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.lifecycle_op.lock().await
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

        let (oms_orders, recovered_sides, portfolio) =
            recover_oms_and_portfolio(&db, run_id, initial_equity_micros).await?;

        {
            let mut sides_lock = self.local_order_sides.write().await;
            *sides_lock = recovered_sides.clone();
        }

        // `AlpacaBrokerAdapter::new()` constructs a `reqwest::blocking::Client`
        // which temporarily creates and drops an internal Tokio runtime.  Tokio
        // 1.49 panics when any runtime is dropped inside an async context.
        // `block_in_place` moves execution off the async context so the drop
        // is safe.  Requires a multi-thread runtime (production and
        // `#[tokio::test(flavor = "multi_thread")]` tests both satisfy this).
        let daemon_broker = tokio::task::block_in_place(|| {
            build_daemon_broker(
                self.runtime_selection.broker_kind,
                self.runtime_selection.deployment_mode,
            )
        })?;

        let broker_seed = match self.broker_snapshot_source {
            BrokerSnapshotTruthSource::Synthetic => {
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
            }
            BrokerSnapshotTruthSource::External => {
                // If a snapshot is already present (pre-loaded by test
                // scaffolding, or retained from a prior loop tick), use it
                // directly and skip the blocking network fetch.  In a fresh
                // production process `broker_snapshot` is always `None` here,
                // so the fetch always runs in production.
                let seeded = self.broker_snapshot.read().await.clone();
                if let Some(existing) = seeded {
                    existing
                } else {
                    let now = Utc::now();
                    let fetched = tokio::task::block_in_place(|| {
                        match &daemon_broker {
                            DaemonBroker::Alpaca(adapter) => {
                                adapter.fetch_broker_snapshot(now).map_err(|err| match err {
                                    BrokerError::AuthSession { detail } => {
                                        RuntimeLifecycleError::forbidden(
                                            "runtime.start_refused.alpaca_snapshot_auth",
                                            "broker_snapshot_fetch",
                                            format!(
                                                "failed to fetch Alpaca broker snapshot before runtime start: {detail}"
                                            ),
                                        )
                                    }
                                    other => RuntimeLifecycleError::service_unavailable(
                                        "runtime.start_refused.alpaca_snapshot_unavailable",
                                        format!(
                                            "failed to fetch Alpaca broker snapshot before runtime start: {other}"
                                        ),
                                    ),
                                })
                            }
                            _ => Err(RuntimeLifecycleError::service_unavailable(
                                "runtime.start_refused.broker_snapshot_source_mismatch",
                                "external broker snapshot source requires Alpaca broker adapter construction",
                            )),
                        }
                    })?;

                    *self.broker_snapshot.write().await = Some(fetched.clone());
                    fetched
                }
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

        let ws_continuity = AlpacaWsContinuityState::from_cursor_json(
            self.runtime_selection.broker_kind,
            broker_cursor.as_deref(),
        );
        *self.alpaca_ws_continuity.write().await = ws_continuity;

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

        let local_seed_reconcile = {
            let local_snapshot_guard = self.execution_snapshot.read().await;
            if let Some(snap) = local_snapshot_guard.clone() {
                let sides = self.local_order_sides.read().await;
                reconcile_local_snapshot_from_runtime_with_sides(&snap, &sides)
            } else {
                mqk_reconcile::LocalSnapshot::empty()
            }
        };

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
// Test-only helpers
// ---------------------------------------------------------------------------

impl AppState {
    /// Inject a never-finishing fake execution loop for tests.
    pub async fn inject_running_loop_for_test(&self, run_id: Uuid) {
        let (stop_tx, mut stop_rx) = watch::channel(ExecutionLoopCommand::Run);
        let join_handle: JoinHandle<ExecutionLoopExit> = tokio::spawn(async move {
            tokio::select! {
                _ = stop_rx.changed() => ExecutionLoopExit {
                    note: Some("test loop stopped".to_string()),
                },
                _ = tokio::time::sleep(std::time::Duration::from_secs(86_400)) => ExecutionLoopExit {
                    note: None,
                },
            }
        });

        let handle = ExecutionLoopHandle {
            run_id,
            stop_tx,
            join_handle,
        };
        let mut lock = self.execution_loop.lock().await;
        *lock = Some(handle);
    }

    /// AUTON-PAPER-03B proof seam: establish a coherent DB-backed active run
    /// with local ownership for autonomous paper lifecycle tests.
    ///
    /// This is intentionally test-only and narrow. It uses the daemon's real DB
    /// run tables plus a locally owned injected loop so proof tests can exercise
    /// restart/gap/recovery truth on one connected lifecycle without requiring a
    /// live broker network session.
    pub async fn establish_db_backed_active_run_for_test(
        &self,
        run_id: Uuid,
    ) -> Result<(), RuntimeLifecycleError> {
        let db = self.db_pool()?;
        mqk_db::insert_run(
            &db,
            &mqk_db::NewRun {
                run_id,
                engine_id: DAEMON_ENGINE_ID.to_string(),
                mode: self.deployment_mode().as_db_mode().to_string(),
                started_at_utc: Utc::now(),
                git_hash: "TEST".to_string(),
                config_hash: self.run_config_hash().to_string(),
                config_json: serde_json::json!({
                    "runtime": "mqk-daemon",
                    "adapter": self.adapter_id(),
                    "mode": self.deployment_mode().as_db_mode(),
                    "proof": "AUTON-PAPER-03B",
                }),
                host_fingerprint: self.node_id.clone(),
            },
        )
        .await
        .map_err(|err| RuntimeLifecycleError::internal("test insert_run failed", err))?;
        mqk_db::arm_run(&db, run_id)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("test arm_run failed", err))?;
        mqk_db::begin_run(&db, run_id)
            .await
            .map_err(|err| RuntimeLifecycleError::internal("test begin_run failed", err))?;
        mqk_db::heartbeat_run(&db, run_id, Utc::now())
            .await
            .map_err(|err| RuntimeLifecycleError::internal("test heartbeat_run failed", err))?;
        self.inject_running_loop_for_test(run_id).await;
        self.publish_status(StatusSnapshot {
            daemon_uptime_secs: uptime_secs(),
            active_run_id: Some(run_id),
            state: "running".to_string(),
            notes: Some("test-established DB-backed active run".to_string()),
            integrity_armed: self.integrity_armed().await,
            deadman_status: "healthy".to_string(),
            deadman_last_heartbeat_utc: Some(Utc::now().to_rfc3339()),
        })
        .await;
        Ok(())
    }

    /// AUTON-PAPER-03B proof seam: apply the daemon's fail-closed continuity-gap
    /// halt consequences against the currently owned DB-backed run.
    pub async fn gap_halt_owned_runtime_for_test(
        &self,
    ) -> Result<Option<String>, RuntimeLifecycleError> {
        if !self.ws_continuity_gap_requires_halt().await {
            return Ok(None);
        }
        let handle = self.take_execution_loop_for_control().await?;
        let Some(handle) = handle else {
            return Ok(None);
        };
        let run_id = handle.run_id;
        let _ = handle.stop_tx.send(ExecutionLoopCommand::Stop);
        let _ = handle
            .join_handle
            .await
            .map_err(|err| RuntimeLifecycleError::internal("test gap-halt join failed", err))?;

        {
            let mut integrity = self.integrity.write().await;
            integrity.disarmed = true;
            integrity.halted = true;
        }

        let db = self.db_pool()?;
        mqk_db::halt_run(&db, run_id, Utc::now())
            .await
            .map_err(|err| RuntimeLifecycleError::internal("test gap-halt halt_run failed", err))?;

        let note = "paper+alpaca WS continuity gap detected; runtime self-halted".to_string();
        self.publish_status(StatusSnapshot {
            daemon_uptime_secs: uptime_secs(),
            active_run_id: Some(run_id),
            state: "halted".to_string(),
            notes: Some(note.clone()),
            integrity_armed: false,
            deadman_status: "expired".to_string(),
            deadman_last_heartbeat_utc: None,
        })
        .await;
        Ok(Some(note))
    }
}

fn autonomous_truth_event_parts(
    truth: &AutonomousSessionTruth,
) -> Option<(&'static str, Option<String>, String)> {
    match truth {
        AutonomousSessionTruth::Clear => None,
        AutonomousSessionTruth::StartRefused { detail } => {
            Some(("start_refused", None, detail.clone()))
        }
        AutonomousSessionTruth::RecoveryRetrying {
            resume_source,
            detail,
        } => Some((
            "recovery_retrying",
            Some(resume_source.as_str().to_string()),
            detail.clone(),
        )),
        AutonomousSessionTruth::RecoverySucceeded {
            resume_source,
            detail,
        } => Some((
            "recovery_succeeded",
            Some(resume_source.as_str().to_string()),
            detail.clone(),
        )),
        AutonomousSessionTruth::RecoveryFailed {
            resume_source,
            detail,
        } => Some((
            "recovery_failed",
            Some(resume_source.as_str().to_string()),
            detail.clone(),
        )),
        AutonomousSessionTruth::RunEndedUnexpectedly { detail } => {
            Some(("run_ended_unexpectedly", None, detail.clone()))
        }
        AutonomousSessionTruth::StopFailed { detail } => {
            Some(("stop_failed", None, detail.clone()))
        }
        AutonomousSessionTruth::StoppedAtBoundary { detail } => {
            Some(("stopped_at_boundary", None, detail.clone()))
        }
    }
}

// ---------------------------------------------------------------------------
// DeadmanTruth (private impl block)
// ---------------------------------------------------------------------------

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
// #[cfg(test)]
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mqk_execution::ReconcileGate;

    #[test]
    fn runtime_selection_defaults_to_paper_paper_blocked() {
        // PT-TRUTH-01: the default config (no env vars) resolves to paper+paper,
        // which is fail-closed.  Operator must set MQK_DAEMON_ADAPTER_ID=alpaca.
        let selection = runtime_selection_from_env_values(None, None);
        assert_eq!(selection.deployment_mode, DeploymentMode::Paper);
        assert_eq!(selection.broker_kind, Some(BrokerKind::Paper));
        assert_eq!(selection.adapter_id, "paper");
        assert!(
            !selection.readiness.start_allowed,
            "paper+paper default must be fail-closed after PT-TRUTH-01"
        );
        assert!(
            selection
                .readiness
                .blocker
                .as_deref()
                .is_some_and(|msg| msg.contains("alpaca")),
            "blocker must direct operator to alpaca; got: {:?}",
            selection.readiness.blocker
        );
    }

    #[test]
    fn runtime_selection_live_capital_alpaca_now_allowed() {
        let selection = runtime_selection_from_env_values(Some("live-capital"), Some("alpaca"));
        assert_eq!(selection.deployment_mode, DeploymentMode::LiveCapital);
        assert_eq!(selection.broker_kind, Some(BrokerKind::Alpaca));
        assert!(
            selection.readiness.start_allowed,
            "live-capital+alpaca must be allowed after AP-08; got: {:?}",
            selection.readiness.blocker
        );
        assert!(selection.readiness.blocker.is_none());
    }

    #[test]
    fn runtime_selection_live_capital_paper_still_blocked() {
        let selection = runtime_selection_from_env_values(Some("live-capital"), Some("paper"));
        assert_eq!(selection.deployment_mode, DeploymentMode::LiveCapital);
        assert_eq!(selection.broker_kind, Some(BrokerKind::Paper));
        assert!(!selection.readiness.start_allowed);
        assert!(selection
            .readiness
            .blocker
            .as_deref()
            .unwrap_or("")
            .contains("live-capital"));
    }

    #[test]
    fn runtime_selection_paper_alpaca_is_now_allowed() {
        let selection = runtime_selection_from_env_values(Some("paper"), Some("alpaca"));
        assert_eq!(selection.deployment_mode, DeploymentMode::Paper);
        assert_eq!(selection.broker_kind, Some(BrokerKind::Alpaca));
        assert!(
            selection.readiness.start_allowed,
            "paper+alpaca must be allowed after AP-06; got blocker: {:?}",
            selection.readiness.blocker
        );
        assert!(
            selection.readiness.blocker.is_none(),
            "no blocker expected for paper+alpaca"
        );
    }

    #[test]
    fn unknown_broker_adapter_string_is_fail_closed() {
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

    #[test]
    fn build_daemon_broker_paper_is_not_execution_path() {
        // BRK-10: LockedPaperBroker is not the canonical paper-trading execution path.
        // build_daemon_broker must refuse to construct it — fail closed — so the daemon
        // cannot accidentally route paper-mode execution through a broker that accepts
        // orders but has no fill mechanism.  The authoritative path is Paper+Alpaca.
        let result = build_daemon_broker(Some(BrokerKind::Paper), DeploymentMode::Paper);
        assert!(
            result.is_err(),
            "build_daemon_broker must refuse BrokerKind::Paper (not the canonical paper path)"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("alpaca"),
            "error must direct operator to the alpaca adapter; got: {err}"
        );
    }

    #[test]
    fn build_daemon_broker_alpaca_paper_mode_requires_credentials() {
        // ENV-TRUTH-01: paper mode reads ALPACA_API_KEY_PAPER (canonical .env.local name)
        if std::env::var(ALPACA_KEY_PAPER_ENV).is_ok() {
            let result = build_daemon_broker(Some(BrokerKind::Alpaca), DeploymentMode::Paper);
            assert!(
                result.is_ok(),
                "Alpaca broker must succeed when credentials are present"
            );
            return;
        }
        let result = build_daemon_broker(Some(BrokerKind::Alpaca), DeploymentMode::Paper);
        assert!(
            result.is_err(),
            "Alpaca broker must fail when credentials are absent"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains(ALPACA_KEY_PAPER_ENV),
            "error must mention canonical paper env var; got: {err_msg}"
        );
    }

    #[test]
    fn build_daemon_broker_alpaca_live_shadow_requires_credentials() {
        // ENV-TRUTH-01: live-shadow mode reads ALPACA_API_KEY_LIVE (canonical .env.local name)
        if std::env::var(ALPACA_KEY_LIVE_ENV).is_ok() {
            let result = build_daemon_broker(Some(BrokerKind::Alpaca), DeploymentMode::LiveShadow);
            assert!(
                result.is_ok(),
                "Alpaca live-shadow broker must succeed when credentials are present"
            );
            return;
        }
        let result = build_daemon_broker(Some(BrokerKind::Alpaca), DeploymentMode::LiveShadow);
        assert!(
            result.is_err(),
            "Alpaca live-shadow broker must fail when credentials are absent"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains(ALPACA_KEY_LIVE_ENV),
            "error must mention canonical live env var; got: {err_msg}"
        );
    }

    #[test]
    fn build_daemon_broker_alpaca_live_capital_requires_credentials() {
        // ENV-TRUTH-01: live-capital mode reads ALPACA_API_KEY_LIVE (canonical .env.local name)
        if std::env::var(ALPACA_KEY_LIVE_ENV).is_ok() {
            let result = build_daemon_broker(Some(BrokerKind::Alpaca), DeploymentMode::LiveCapital);
            assert!(
                result.is_ok(),
                "Alpaca+LiveCapital must succeed when credentials are present"
            );
            return;
        }
        let result = build_daemon_broker(Some(BrokerKind::Alpaca), DeploymentMode::LiveCapital);
        assert!(
            result.is_err(),
            "Alpaca+LiveCapital must fail when credentials are absent"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains(ALPACA_KEY_LIVE_ENV),
            "error must mention canonical live env var; got: {err_msg}"
        );
    }

    #[test]
    fn build_daemon_broker_unknown_is_fail_closed() {
        let result = build_daemon_broker(None, DeploymentMode::Paper);
        assert!(result.is_err(), "Unknown broker (None) must fail closed");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("unrecognised"),
            "error must mention unrecognised; got: {err_msg}"
        );
    }

    #[test]
    fn alpaca_paper_base_url_honors_override() {
        let base_url =
            alpaca_base_url_for_mode(DeploymentMode::Paper, Some(" http://127.0.0.1:18080 "))
                .expect("paper mode must resolve alpaca base url");
        assert_eq!(base_url, "http://127.0.0.1:18080");
    }

    #[test]
    fn alpaca_live_shadow_base_url_ignores_override_and_uses_canonical_live() {
        let base_url =
            alpaca_base_url_for_mode(DeploymentMode::LiveShadow, Some("http://127.0.0.1:18080"))
                .expect("live-shadow mode must resolve alpaca base url");
        assert_eq!(base_url, "https://api.alpaca.markets");
    }

    #[test]
    fn alpaca_live_capital_base_url_ignores_override_and_uses_canonical_live() {
        let base_url =
            alpaca_base_url_for_mode(DeploymentMode::LiveCapital, Some("http://127.0.0.1:18080"))
                .expect("live-capital mode must resolve alpaca base url");
        assert_eq!(base_url, "https://api.alpaca.markets");
    }

    #[test]
    fn env_truth_02_alpaca_live_base_url_env_var_is_not_authoritative() {
        // ENV-TRUTH-02: `ALPACA_LIVE_BASE_URL` is NOT read by the daemon.
        //
        // The daemon hardcodes the live endpoint to `https://api.alpaca.markets`
        // for all live modes (LiveShadow, LiveCapital).  Only the paper endpoint
        // is overridable via `ALPACA_PAPER_BASE_URL` (read by `build_daemon_broker`
        // as `ALPACA_BASE_URL_PAPER_ENV` for `DeploymentMode::Paper` only).
        //
        // An operator who sets `ALPACA_LIVE_BASE_URL` in their .env.local will
        // have no effect on the daemon's live broker URL.  The .env.local.example
        // entry for that var is explicitly commented out per ENV-TRUTH-02.
        for (mode, label) in [
            (DeploymentMode::LiveShadow, "live-shadow"),
            (DeploymentMode::LiveCapital, "live-capital"),
        ] {
            // No override provided — must use hardcoded canonical URL.
            let url_no_override = alpaca_base_url_for_mode(mode, None)
                .unwrap_or_else(|_| panic!("{label} must resolve live URL"));
            assert_eq!(
                url_no_override, "https://api.alpaca.markets",
                "ENV-TRUTH-02: {label} must use hardcoded live endpoint (no override)"
            );

            // Override provided — must be ignored (live URL is hardcoded).
            let url_with_override =
                alpaca_base_url_for_mode(mode, Some("https://some-other-url.example.com"))
                    .unwrap_or_else(|_| panic!("{label} must resolve live URL"));
            assert_eq!(
                url_with_override, "https://api.alpaca.markets",
                "ENV-TRUTH-02: {label} must ignore any override and use hardcoded live endpoint"
            );
        }

        // Confirm the paper endpoint IS overridable (canonical behavior since ENV-TRUTH-01).
        let paper_url_overridden =
            alpaca_base_url_for_mode(DeploymentMode::Paper, Some("http://127.0.0.1:18080"))
                .expect("paper mode must resolve alpaca base url");
        assert_eq!(
            paper_url_overridden, "http://127.0.0.1:18080",
            "ENV-TRUTH-02: paper endpoint must still honor ALPACA_PAPER_BASE_URL override"
        );
    }

    #[test]
    fn ap06_paper_alpaca_readiness_is_allowed() {
        let readiness = deployment_mode_readiness(DeploymentMode::Paper, Some(BrokerKind::Alpaca));
        assert!(
            readiness.start_allowed,
            "paper+alpaca must be allowed after AP-06; got: {:?}",
            readiness.blocker
        );
        assert!(readiness.blocker.is_none(), "no blocker expected");
    }

    #[test]
    fn pt_truth_01_paper_paper_is_fail_closed() {
        // PT-TRUTH-01: paper+paper is not an honest paper trading path.
        // LockedPaperBroker requires an external bar-feed (on_bar) that is not
        // wired in the daemon runtime.  The real paper route is paper+alpaca.
        let readiness = deployment_mode_readiness(DeploymentMode::Paper, Some(BrokerKind::Paper));
        assert!(
            !readiness.start_allowed,
            "paper+paper must be fail-closed after PT-TRUTH-01"
        );
        let blocker = readiness
            .blocker
            .expect("paper+paper must carry a blocker message");
        assert!(
            blocker.contains("alpaca"),
            "blocker must direct operator to alpaca broker; got: {blocker}"
        );
    }

    #[test]
    fn ap06_live_shadow_alpaca_was_blocked_now_allowed_by_ap07() {
        let readiness =
            deployment_mode_readiness(DeploymentMode::LiveShadow, Some(BrokerKind::Alpaca));
        assert!(
            readiness.start_allowed,
            "live-shadow+alpaca must be allowed after AP-07; got: {:?}",
            readiness.blocker
        );
    }

    #[test]
    fn ap06_live_capital_alpaca_was_blocked_now_allowed_by_ap08() {
        let readiness =
            deployment_mode_readiness(DeploymentMode::LiveCapital, Some(BrokerKind::Alpaca));
        assert!(
            readiness.start_allowed,
            "live-capital+alpaca must be allowed after AP-08; got: {:?}",
            readiness.blocker
        );
        assert!(
            readiness.blocker.is_none(),
            "allowed combination must carry no blocker message; got: {:?}",
            readiness.blocker
        );
    }

    #[test]
    fn ap06_live_shadow_paper_still_blocked() {
        let readiness =
            deployment_mode_readiness(DeploymentMode::LiveShadow, Some(BrokerKind::Paper));
        assert!(
            !readiness.start_allowed,
            "live-shadow+paper must remain fail-closed"
        );
    }

    #[test]
    fn ap06_runtime_selection_paper_alpaca_start_allowed() {
        let sel = runtime_selection_from_env_values(Some("paper"), Some("alpaca"));
        assert_eq!(sel.deployment_mode, DeploymentMode::Paper);
        assert_eq!(sel.broker_kind, Some(BrokerKind::Alpaca));
        assert!(
            sel.readiness.start_allowed,
            "paper+alpaca RuntimeSelection must be startable; got: {:?}",
            sel.readiness.blocker
        );
    }

    #[test]
    fn ap07_live_shadow_alpaca_readiness_is_allowed() {
        let readiness =
            deployment_mode_readiness(DeploymentMode::LiveShadow, Some(BrokerKind::Alpaca));
        assert!(
            readiness.start_allowed,
            "live-shadow+alpaca must be allowed after AP-07; got: {:?}",
            readiness.blocker
        );
        assert!(readiness.blocker.is_none(), "no blocker expected");
    }

    #[test]
    fn ap07_live_shadow_paper_is_explicitly_blocked() {
        let readiness =
            deployment_mode_readiness(DeploymentMode::LiveShadow, Some(BrokerKind::Paper));
        assert!(
            !readiness.start_allowed,
            "live-shadow+paper must be blocked (no real external truth)"
        );
        let blocker = readiness
            .blocker
            .expect("live-shadow+paper must have a blocker");
        assert!(
            blocker.contains("external broker"),
            "blocker must explain external broker requirement; got: {blocker}"
        );
    }

    #[test]
    fn ap07_live_shadow_unrecognised_adapter_is_blocked() {
        let readiness = deployment_mode_readiness(DeploymentMode::LiveShadow, None);
        assert!(
            !readiness.start_allowed,
            "live-shadow+unrecognised must be blocked"
        );
        assert!(readiness.blocker.is_some(), "must carry a blocker message");
    }

    #[test]
    fn ap07_live_capital_alpaca_was_blocked_now_allowed_by_ap08() {
        let readiness =
            deployment_mode_readiness(DeploymentMode::LiveCapital, Some(BrokerKind::Alpaca));
        assert!(
            readiness.start_allowed,
            "live-capital+alpaca must be allowed after AP-08; got: {:?}",
            readiness.blocker
        );
        assert!(
            readiness.blocker.is_none(),
            "allowed combination must carry no blocker; got: {:?}",
            readiness.blocker
        );
    }

    #[test]
    fn ap07_live_capital_paper_still_blocked() {
        let readiness =
            deployment_mode_readiness(DeploymentMode::LiveCapital, Some(BrokerKind::Paper));
        assert!(
            !readiness.start_allowed,
            "live-capital+paper must be blocked"
        );
    }

    #[test]
    fn ap07_paper_alpaca_remains_allowed() {
        // PT-TRUTH-01: paper+paper is now fail-closed (see pt_truth_01_paper_paper_is_fail_closed).
        // paper+alpaca is the honest paper trading route and must remain allowed.
        let pa = deployment_mode_readiness(DeploymentMode::Paper, Some(BrokerKind::Alpaca));
        assert!(pa.start_allowed, "paper+alpaca must remain allowed");
        assert!(pa.blocker.is_none(), "paper+alpaca must carry no blocker");
    }

    #[test]
    fn ap07_runtime_selection_live_shadow_alpaca_start_allowed() {
        let sel = runtime_selection_from_env_values(Some("live-shadow"), Some("alpaca"));
        assert_eq!(sel.deployment_mode, DeploymentMode::LiveShadow);
        assert_eq!(sel.broker_kind, Some(BrokerKind::Alpaca));
        assert!(
            sel.readiness.start_allowed,
            "live-shadow+alpaca RuntimeSelection must be startable; got: {:?}",
            sel.readiness.blocker
        );
    }

    #[test]
    fn ap07_calendar_spec_for_live_shadow_is_nyse_weekdays() {
        let state = AppState::new_for_test_with_mode(DeploymentMode::LiveShadow);
        assert_eq!(
            state.calendar_spec(),
            mqk_integrity::CalendarSpec::NyseWeekdays,
            "live-shadow must use NyseWeekdays calendar for honest session truth"
        );
    }

    #[test]
    fn ap07_live_shadow_alpaca_state_uses_external_snapshot_source() {
        let state = AppState::new_for_test_with_broker_kind(BrokerKind::Alpaca);
        assert_eq!(
            state.broker_snapshot_source(),
            BrokerSnapshotTruthSource::External,
            "live-shadow+alpaca must declare External snapshot source"
        );
    }

    #[test]
    fn ap08_live_capital_alpaca_readiness_is_allowed() {
        let readiness =
            deployment_mode_readiness(DeploymentMode::LiveCapital, Some(BrokerKind::Alpaca));
        assert!(
            readiness.start_allowed,
            "live-capital+alpaca must be allowed after AP-08; got: {:?}",
            readiness.blocker
        );
        assert!(
            readiness.blocker.is_none(),
            "no blocker expected for allowed pair"
        );
    }

    #[test]
    fn ap08_live_capital_paper_is_explicitly_blocked() {
        let readiness =
            deployment_mode_readiness(DeploymentMode::LiveCapital, Some(BrokerKind::Paper));
        assert!(
            !readiness.start_allowed,
            "live-capital+paper must remain fail-closed after AP-08"
        );
        let blocker = readiness
            .blocker
            .expect("live-capital+paper must carry a blocker message");
        assert!(
            blocker.contains("live-capital"),
            "blocker must name the live-capital restriction; got: {blocker}"
        );
    }

    #[test]
    fn ap08_live_capital_unrecognised_adapter_is_blocked() {
        let readiness = deployment_mode_readiness(DeploymentMode::LiveCapital, None);
        assert!(
            !readiness.start_allowed,
            "live-capital+None must be blocked"
        );
        assert!(readiness.blocker.is_some(), "must carry a blocker message");
    }

    #[test]
    fn ap08_runtime_selection_live_capital_alpaca_start_allowed() {
        let sel = runtime_selection_from_env_values(Some("live-capital"), Some("alpaca"));
        assert_eq!(sel.deployment_mode, DeploymentMode::LiveCapital);
        assert_eq!(sel.broker_kind, Some(BrokerKind::Alpaca));
        assert!(
            sel.readiness.start_allowed,
            "live-capital+alpaca RuntimeSelection must be startable; got: {:?}",
            sel.readiness.blocker
        );
        assert!(sel.readiness.blocker.is_none(), "no blocker expected");
    }

    #[test]
    fn ap08_capital_dev_no_token_is_blocked_by_start_gate() {
        let mode = DeploymentMode::LiveCapital;
        let auth = OperatorAuthMode::ExplicitDevNoToken;
        let gate_fires = mode == DeploymentMode::LiveCapital
            && !matches!(auth, OperatorAuthMode::TokenRequired(_));
        assert!(gate_fires, "dev-no-token must trigger capital token gate");

        let auth_token = OperatorAuthMode::TokenRequired("real-token".to_string());
        let gate_fires_for_token = mode == DeploymentMode::LiveCapital
            && !matches!(auth_token, OperatorAuthMode::TokenRequired(_));
        assert!(
            !gate_fires_for_token,
            "TokenRequired must not trigger capital token gate"
        );

        let auth_missing = OperatorAuthMode::MissingTokenFailClosed;
        let gate_fires_for_missing = mode == DeploymentMode::LiveCapital
            && !matches!(auth_missing, OperatorAuthMode::TokenRequired(_));
        assert!(
            gate_fires_for_missing,
            "MissingTokenFailClosed must also trigger capital token gate"
        );
    }

    #[test]
    fn ap08_calendar_spec_for_live_capital_is_nyse_weekdays() {
        let state = AppState::new_for_test_with_mode(DeploymentMode::LiveCapital);
        assert_eq!(
            state.calendar_spec(),
            mqk_integrity::CalendarSpec::NyseWeekdays,
            "live-capital must use NyseWeekdays calendar for honest session truth"
        );
    }

    #[test]
    fn ap08_live_shadow_unchanged_after_ap08() {
        let shadow_alpaca =
            deployment_mode_readiness(DeploymentMode::LiveShadow, Some(BrokerKind::Alpaca));
        assert!(
            shadow_alpaca.start_allowed,
            "live-shadow+alpaca must remain allowed after AP-08"
        );
        assert!(shadow_alpaca.blocker.is_none(), "no blocker expected");

        // paper+paper is fail-closed after PT-TRUTH-01 (see pt_truth_01_paper_paper_is_fail_closed).
        // paper+alpaca remains the honest paper route.
        let pa = deployment_mode_readiness(DeploymentMode::Paper, Some(BrokerKind::Alpaca));
        assert!(
            pa.start_allowed,
            "paper+alpaca must remain allowed after AP-08"
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
