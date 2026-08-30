//! Shared runtime state for mqk-daemon.
//!
//! All types here are `Clone`-able (via `Arc` or copy). Handlers receive
//! `State<Arc<AppState>>` from Axum; this module owns daemon-local runtime
//! lifecycle control plus durable status reconstruction.

mod alpaca_ws_transport;
mod autonomous_bar_ticker;
pub mod autonomous_completed_bar_driver;
pub mod autonomous_completed_bar_task;
pub mod autonomous_daily_coordinator;
pub mod autonomous_daily_coverage_authority;
pub mod autonomous_daily_operation;
pub mod autonomous_daily_outcome;
pub mod autonomous_retry_policy;
pub mod autonomous_runtime_context;
mod broker;
mod deadman;
mod dry_run_strategy;
mod env;
#[cfg(test)]
mod hermetic_positive_proofs;
pub mod instrument_economics_bridge;
mod lifecycle;
mod loop_runner;
pub mod market_calendar;
pub mod market_data_latest_bar;
mod multi_symbol_config;
mod orchestrator_build;
mod paper_portfolio_accounting;
mod per_symbol_bar_window;
pub mod required_market_data_autofresh;
pub mod runtime_session_source;
mod session_controller;
mod signal_intake;
mod snapshot;
mod types;
pub mod ws_gap_recovery;

/// BUNDLE-7-PHASE-7A-SINGLE-FROZEN-FLEET-AUTHORITY-CLOSURE: one process-wide
/// lock for every test that mutates the process-global `MQK_STRATEGY_SYMBOL`
/// / `MQK_STRATEGY_IDS` / `MQK_STRATEGY_MD_TIMEFRAME` env vars, so tests in
/// different modules (`state::lifecycle`, `state::multi_symbol_config`) that
/// each read/write these same env vars never race each other under `cargo
/// test`'s default parallelism.
#[cfg(test)]
pub(crate) mod shared_test_locks {
    pub(crate) fn strategy_fleet_env_test_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }
}

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::backtest_jobs::{new_job_store, BacktestJobStore};
use crate::ingest_jobs::{new_ingest_job_store, IngestJobStore};
use crate::strategy_scan_jobs::{new_strategy_scan_job_store, StrategyScanJobStore};

use chrono::{DateTime, Utc};
use mqk_broker_alpaca::types::AlpacaFetchCursor;
use mqk_broker_alpaca::AlpacaBrokerAdapter;
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
pub use autonomous_bar_ticker::{
    spawn_autonomous_bar_ticker, BAR_INTERVAL_SECS_ENV, DEFAULT_QTY_ENV,
};
pub use autonomous_completed_bar_task::{
    resolve_completed_bar_tick_cadence, resolve_completed_bar_tick_cadence_from_env,
    select_driver_mode_for_state, spawn_autonomous_completed_bar_driver_task,
    tick_autonomous_completed_bar_driver_from_state, AutonomousCompletedBarProductionTickOutcome,
    AutonomousCompletedBarTaskRuntime, AutonomousCompletedBarTaskSpawnOutcome,
    AutonomousCompletedBarTaskTruth, CompletedBarTaskCadenceError, COMPLETED_BAR_TICK_SECS_ENV,
};
pub use autonomous_daily_operation::{
    derive_assignment_identity, derive_autonomous_daily_operation_id,
    derive_runtime_binding_identity, project_autonomous_daily_operation_read_model,
    resolve_autonomous_daily_session_plan, resolve_autonomous_daily_session_plan_from_env,
    resolve_fixed_window_override_config_from_env, AutonomousDailyOperationEventReadModel,
    AutonomousDailyOperationReadModel, AutonomousDailyPlanReason, AutonomousDailyPlanTiming,
    AutonomousDailyScheduleSource, AutonomousDailySessionPlan,
    AutonomousDailySessionPlanResolution, FixedWindowOverrideConfig,
};
use broker::{
    build_asset_shortable_preflight_fetcher_from_env, build_fill_activity_fetcher_from_env,
    build_snapshot_fetcher_from_env, build_ws_gap_fill_fetcher_from_env,
};
pub use broker::{DeploymentReadiness, RuntimeSelection, StrategyFleetEntry};
pub use dry_run_strategy::{
    dry_run_strategy_ids_from_env, evaluate_dry_run_strategies, evaluate_dry_run_strategy,
    DryRunStrategyDiagnostic, DRY_RUN_STRATEGY_IDS_ENV,
};
pub use env::{operator_auth_mode_from_env_values, spawn_heartbeat, uptime_secs};
pub use instrument_economics_bridge::{
    bridge_instrument_registry_v2_to_economics, instrument_v2_to_economics,
    InstrumentEconomicsBridgeResult, InstrumentEconomicsBridgeSummary,
};
pub use lifecycle::{AutonomousArmOutcome, AutonomousArmRejection};
pub use loop_runner::spawn_reconcile_tick;
pub use market_calendar::{
    classify_crypto_continuous_session, classify_equity_us_regular_session,
    classify_forex_weekday_continuous_session, classify_futures_globex_session,
    instrument_session_state_for_profile, resolve_session_profile_for_instrument_metadata,
    route_instrument_session_for_metadata, supported_session_profiles, ExchangeCalendarDay,
    ExchangeCalendarMeta, ExchangeDayStatus, ExchangeSourceState, ExchangeSourcedCalendarProvider,
    ExchangeUnavailablePolicy, FixedWindowOverrideProvider, FuturesSessionWindows,
    InstrumentSessionRoute, MarketCalendarProvider, MarketSessionProfile, MarketSessionState,
    MarketSessionTruth, MarketVenueSessionKind, NyseWeekdaysProvider, SessionAuthority,
    SessionProfileResolution, SessionProfileResolutionTruth, SessionProfileStatus,
};
pub use multi_symbol_config::{
    build_legacy_single_symbol_config, build_legacy_single_symbol_config_from_env,
    build_multi_symbol_config_from_watchlist_artifact, build_multi_symbol_runtime_config_from_env,
    build_multi_symbol_runtime_config_from_env_and_watchlist, MultiSymbolConfigError,
    MultiSymbolConfigSource, MultiSymbolRuntimeConfig, SymbolStrategyAssignment,
    MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION,
};
// BUNDLE-7-PHASE-7A-SINGLE-FROZEN-FLEET-AUTHORITY-CLOSURE requirement 4:
// crate-internal-only raw-input seam so `StartAttemptAuthoritySnapshot::
// resolve` (state/lifecycle.rs) can read the watchlist artifact and legacy
// env vars exactly once and derive both `MultiSymbolRuntimeConfig` and the
// premarket freshness gate's required-symbol vector from that one read —
// sourcing `legacy_strategy_id` from the one frozen fleet capture instead of
// a second `MQK_STRATEGY_IDS` read. (The non-fleet-aware
// `read_multi_symbol_config_raw_inputs_from_env` remains `pub(crate)` in
// `multi_symbol_config.rs` for its own internal caller,
// `build_multi_symbol_runtime_config_from_env` — not re-exported here since
// nothing else in the crate calls it via this path anymore.)
pub(crate) use multi_symbol_config::read_multi_symbol_config_raw_inputs_from_env_and_fleet;
pub use per_symbol_bar_window::{
    classify_bar_staleness, load_recent_completed_bars_for_symbol_window,
    per_symbol_loaded_bars_from_rows, EmptySymbolError, PerSymbolBarInput, PerSymbolBarWindow,
    PerSymbolBarWindowError, PerSymbolLoadedBars, PerSymbolPendingBarInputs,
};
pub use runtime_session_source::{
    evaluate_runtime_session_source_active_decision, evaluate_runtime_session_source_candidate,
    evaluate_runtime_session_source_from_registry_path, parse_runtime_session_source_mode,
    runtime_session_source_mode_from_env, runtime_session_source_summary,
    RuntimeSessionSourceActiveDecision, RuntimeSessionSourceEvaluation, RuntimeSessionSourceMode,
    RuntimeSessionSourceModeParse, RuntimeSessionSourceSummary, RUNTIME_SESSION_SOURCE_ENV,
};
pub use session_controller::{
    autonomous_session_schedule_from_env, run_durable_session_controller_tick,
    run_session_controller_tick, session_window_from_env, spawn_autonomous_session_controller,
    AutonomousSessionSchedule, SessionWindow, SESSION_START_HH_MM_ENV, SESSION_STOP_HH_MM_ENV,
};
pub(crate) use snapshot::{
    reconcile_broker_snapshot_from_schema, reconcile_local_snapshot_from_runtime_with_sides,
    recover_oms_and_portfolio,
};
pub use types::{
    AcceptedArtifactProvenance, AlpacaWsContinuityState, AutonomousRecoveryResumeSource,
    AutonomousSessionTruth, BrokerKind, BrokerSnapshotTruthSource, BuildInfo, BusMsg,
    DeploymentMode, DynamicSelectionLifecycleFaultSeam, DynamicSelectionRuntimeState,
    OperatorAuthMode, ReconcileStatusSnapshot, RestartTruthSnapshot, RuntimeLifecycleError,
    StatusSnapshot, StrategyMarketDataSource,
};
pub(crate) use types::{
    BoundedLifecycleDegradation, ExecutionLoopCommand, ExecutionLoopExit, ExecutionLoopHandle,
    InstallActiveRuntimeError, InstallRuntimeTaskCleanup, LifecycleClearReason,
    LocalRuntimeClearOutcome, LocalRuntimeOwnership, PrepareStartingMetadataError,
    RunStartMetadata,
};
pub use ws_gap_recovery::WsGapRecoveryOutcome;
// Internal (crate-visible) re-exports used across this module.
#[cfg(test)]
use broker::alpaca_base_url_for_mode;
#[cfg(test)]
use broker::build_daemon_broker;
#[cfg(test)]
use env::runtime_selection_from_env_values;
use env::{
    deployment_mode_readiness, initial_reconcile_status, initial_ws_continuity_for_broker,
    operator_auth_mode_from_env, runtime_selection_from_env,
};
use mqk_runtime::native_strategy::NativeStrategyBootstrap;
#[cfg(test)]
use types::ReconcileTruthGate;

pub(crate) const DAEMON_ENGINE_ID: &str = "mqk-daemon";
const DEFAULT_DAEMON_DEPLOYMENT_MODE: &str = "paper";
const DEFAULT_DAEMON_ADAPTER_ID: &str = "paper";
const DAEMON_RUN_CONFIG_HASH_PREFIX: &str = "daemon-runtime";
const EXECUTION_LOOP_INTERVAL: Duration = Duration::from_secs(1);
// DEADMAN-EXPIRED-AFTER-START-01: TTL must exceed the maximum blocking
// duration of a single orchestrator tick.  Phase 2 (fetch_events) makes a
// synchronous Alpaca REST call with no HTTP timeout; a smoke run observed a
// 33-second block.  RUNTIME_LEASE_TTL_SECS=90 already accommodates the
// maximum single-phase block.  DEADMAN_TTL must be consistent: set to 120 so
// a loop tick that blocks up to RUNTIME_LEASE_TTL_SECS (90 s) does not cause
// a false deadman expiration on the next pre-tick check.  With TTL=120 a
// truly dead loop is still detected within 2 minutes.
const DEADMAN_TTL_SECONDS: i64 = 120;
/// DMON-06: background reconcile tick interval.
const RECONCILE_TICK_INTERVAL: Duration = Duration::from_secs(30);
/// AUTON-PAPER-RISK-03: execution-loop ticks between External broker snapshot refreshes.
/// At 1 s/tick this is 60 s — fresh enough for paper reconcile without hammering the API.
const EXTERNAL_SNAPSHOT_REFRESH_TICKS: u32 = 60;

/// RR3 (RUNTIME-RISK-ACCOUNT-FRESHNESS-AUTHORITY-01): maximum age of an
/// External broker-snapshot equity reading that `DaemonAccountAuthority` may
/// treat as current for account-level risk gating.
///
/// This is a DEDICATED risk-freshness policy, deliberately NOT
/// `mqk_runtime::orchestrator::TERMINAL_FILL_SETTLE_GRACE_SECS` (a
/// different-domain post-terminal-fill *reconciliation* settlement grace
/// window — see that constant's doc for why the two must not be conflated).
///
/// Derived directly from this loop's own periodic External snapshot refresh
/// cadence: `loop_runner.rs` refreshes the cached snapshot every
/// `EXTERNAL_SNAPSHOT_REFRESH_TICKS` ticks of `EXECUTION_LOOP_INTERVAL`
/// (nominally 60 s), and that refresh runs AFTER `orch.tick()`'s risk
/// evaluation in the same loop iteration — so the snapshot backing risk
/// evaluation can legitimately be up to one full refresh cadence old right
/// before the next refresh fires. One additional `EXECUTION_LOOP_INTERVAL`
/// is added as margin: Phase 2 of `orch.tick()` makes a real broker REST
/// call every tick, so ordinary per-tick processing routinely adds a small
/// amount of wall-clock time beyond the nominal 1 s interval, and a bound
/// with zero margin would risk a false `Stale` denial from that ordinary
/// jitter alone rather than from a genuinely stale snapshot. A tick that
/// overruns far beyond this (e.g. the ~33 s single Phase-2 block observed in
/// a smoke run — see `DEADMAN_TTL_SECONDS` doc above) SHOULD still trigger
/// `Stale`: that is a genuinely old snapshot and correct fail-closed
/// behavior, not a false halt.
const ACCOUNT_RISK_FRESHNESS_BOUND_SECS: i64 =
    (EXTERNAL_SNAPSHOT_REFRESH_TICKS as i64 + 1) * EXECUTION_LOOP_INTERVAL.as_secs() as i64;
/// AUTON-SIGNAL-CONTEXT-01: DB timeframe string for loading completed bars for
/// autonomous strategy context (e.g. "1D", "5m").  Must match the `timeframe`
/// column value in `md_bars` for the configured symbol.  If absent, the daemon
/// falls back to the single-stub context path (no signal expected).
pub const STRATEGY_MD_TIMEFRAME_ENV: &str = "MQK_STRATEGY_MD_TIMEFRAME";

/// AUTON-SIGNAL-CONTEXT-01: Number of recent completed bars to load per dispatch.
/// 30 covers every built-in strategy's maximum lookback (20) with headroom.
const STRATEGY_CONTEXT_LOAD_LIMIT: i64 = 30;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerSymbolTargetState {
    pub symbol: String,
    pub strategy_id: String,
    pub current_qty: i64,
    pub target_qty: i64,
    pub delta: i64,
    pub no_order_reason: String,
    pub last_decision_id: Option<String>,
    pub last_decision_disposition: Option<String>,
    pub updated_at_utc: String,
}

fn normalize_per_symbol_target_state_key(symbol: &str) -> Option<String> {
    let key = symbol.trim().to_ascii_uppercase();
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

/// Process-local latest-bar scheduler runtime state.
///
/// This is intentionally not durable in this patch. The scheduler is disabled
/// at boot and becomes active only after the operator start route installs a
/// task handle and request configuration here.
pub struct MarketDataFeedSchedulerRuntimeState {
    pub running: bool,
    pub provider_id: Option<String>,
    pub timeframe: Option<mqk_md::Timeframe>,
    pub symbols: Vec<String>,
    pub dry_run: bool,
    pub allow_provider_api_calls: bool,
    pub provider_registry_path: Option<String>,
    pub last_poll_ts: Option<i64>,
    pub next_poll_ts: Option<i64>,
    pub latest_expected_closed_bar_ts: Option<i64>,
    pub last_result: Option<crate::api_types::MarketDataFeedPollOnceResponse>,
    pub last_error: Option<String>,
    pub started_at_ts: Option<i64>,
    pub stopped_at_ts: Option<i64>,
    pub poll_count: u64,
    pub inserted_count: u64,
    pub unchanged_or_skipped_count: u64,
    pub error_count: u64,
    pub stop_tx: Option<watch::Sender<bool>>,
    pub task: Option<JoinHandle<()>>,
}

impl Default for MarketDataFeedSchedulerRuntimeState {
    fn default() -> Self {
        Self {
            running: false,
            provider_id: None,
            timeframe: None,
            symbols: Vec::new(),
            dry_run: true,
            allow_provider_api_calls: false,
            provider_registry_path: None,
            last_poll_ts: None,
            next_poll_ts: None,
            latest_expected_closed_bar_ts: None,
            last_result: None,
            last_error: None,
            started_at_ts: None,
            stopped_at_ts: None,
            poll_count: 0,
            inserted_count: 0,
            unchanged_or_skipped_count: 0,
            error_count: 0,
            stop_tx: None,
            task: None,
        }
    }
}

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
    /// PER-SYMBOL-TARGET-STATE-01: In-memory, observability-only target state.
    ///
    /// Keyed by normalized symbol (trimmed, uppercased) for deterministic
    /// per-symbol replacement. This is not persisted and has no execution
    /// authority.
    per_symbol_target_state: Arc<RwLock<BTreeMap<String, PerSymbolTargetState>>>,
    /// Latest monotonic reconcile result known to the daemon.
    reconcile_status: Arc<RwLock<ReconcileStatusSnapshot>>,
    /// Operator auth posture for privileged routes.
    pub operator_auth: OperatorAuthMode,
    /// Runtime adapter/deployment selection resolved from config/env at bootstrap.
    runtime_selection: RuntimeSelection,
    /// BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement 2:
    /// the single authoritative local runtime lifecycle-ownership state
    /// machine — `Idle`/`Reserved`/`Starting`/`Active`/`Degraded`. Replaces
    /// the prior split of `execution_loop` (`ExecutionLoopSlot`),
    /// `run_start_commit_owner`, and `dynamic_selection_runtime` into one
    /// lock. See `LocalRuntimeOwnership`.
    runtime_ownership: Arc<Mutex<LocalRuntimeOwnership>>,
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
    /// DAILY-DATA-READINESS-01C-ENFORCEMENT-01: Injectable wall-clock
    /// override for the strict daily-data readiness start gate.
    ///
    /// `None` in production — the gate reads `Utc::now()` directly. Set to a
    /// fixed instant in tests so a start-gate `ready` verdict can be proven
    /// deterministically against seeded `md_bars` fixtures, without racing
    /// the real wall clock (§C.12: injected clocks are a required test seam).
    daily_data_readiness_clock_override: Arc<RwLock<Option<DateTime<Utc>>>>,
    /// DAILY-DATA-READINESS-01C-ENFORCEMENT-01 test seam (§C.12 "injected
    /// event writers"): forces the strict readiness start gate's pre-start
    /// evidence persist outcome regardless of the real DB write result.
    /// `None` in production — the gate uses the real
    /// `persist_pre_start_readiness_evidence` outcome. Set in tests to prove
    /// the evidence-failure policy (§C.9) deterministically, without needing
    /// to actually break a live DB connection mid-test.
    daily_data_readiness_evidence_override: Arc<RwLock<Option<bool>>>,
    /// REPAIR 1 (DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01): process-local
    /// monotonic sequence number, incremented once per actual strict
    /// daily-data-readiness start-gate evaluation. Combined with the
    /// full-precision evaluation timestamp in `compute_evaluation_id` so two
    /// start attempts with otherwise-identical inputs (same wall-clock
    /// second/minute, same binding, same assignment set) always receive
    /// distinct `evaluation_id`s — never a minute-bucket collision.
    daily_data_readiness_attempt_seq: Arc<AtomicU64>,
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
    /// MULTI-SYMBOL-DAY-ORDER-CAP-01: Per-run, per-symbol order intake counter
    /// (cap #4, design doc §6 "Cap #4 — per_symbol_day_order_count_limit").
    ///
    /// Keyed by symbol (trimmed, uppercased). Incremented alongside
    /// `day_signal_count` on every new outbox enqueue (Gate 7 Ok(true)).
    /// Reset (cleared) at the start of each new execution run, at the same
    /// point `day_signal_count` is reset to 0.
    day_signal_count_by_symbol: Arc<RwLock<HashMap<String, u32>>>,
    /// MULTI-SYMBOL-DAY-ORDER-CAP-01: Optional per-symbol daily order count
    /// limit (cap #4). `None` (the default, from an unset
    /// `MQK_PER_SYMBOL_DAY_ORDER_LIMIT`) disables Gate 1f entirely — all
    /// existing dispositions and the account-wide `day_signal_limit` (Gate 1)
    /// are unaffected either way. Read once at construction; overridable via
    /// `set_per_symbol_day_order_limit_for_test`.
    per_symbol_day_order_limit: Arc<RwLock<Option<u32>>>,
    /// MULTI-SYMBOL-CAPITAL-CAPS-01: Optional per-symbol maximum target
    /// position quantity (cap #2, design doc §6 "Cap #2 —
    /// per_symbol_max_position_qty"). `None` (the default, from an unset
    /// `MQK_PER_SYMBOL_MAX_POSITION_QTY`) disables the B1C target-qty clamp
    /// entirely. Read once at construction; overridable via
    /// `set_per_symbol_max_position_qty_for_test`.
    per_symbol_max_position_qty: Arc<RwLock<Option<i64>>>,
    /// MULTI-SYMBOL-CAPITAL-CAPS-01: Optional per-tick maximum number of
    /// newly-accepted decisions (cap #6, design doc §6 "Cap #6 —
    /// max_new_orders_per_tick"). `None` (the default, from an unset
    /// `MQK_MAX_NEW_ORDERS_PER_TICK`) is unbounded — every configured symbol
    /// is dispatched every tick (today's implicit behavior). Read once at
    /// construction; overridable via `set_max_new_orders_per_tick_for_test`.
    max_new_orders_per_tick: Arc<RwLock<Option<u32>>>,
    /// MD-STALENESS-PER-TICK-GATE-01: Optional override for the per-symbol
    /// bar-staleness threshold (cap #9, design doc §6
    /// "per_symbol_bar_staleness_guard"), in seconds. Used by
    /// `dispatch_native_strategy_for_symbol_with_bar` to fail-closed-block
    /// strategy dispatch for a symbol/tick whose latest completed bar is
    /// stale or missing. `None` (the default, from an unset
    /// `MQK_PER_SYMBOL_BAR_STALENESS_SECS`) means "use the timeframe-aware
    /// default" — daily/1D keeps
    /// `market_data_freshness::MD_FRESHNESS_STALE_SECS`, while intraday uses
    /// `MQK_INTRADAY_BAR_MAX_AGE_SECS` or its fail-closed default. Unlike caps
    /// #2/#4/#6, this gate is always-on and cannot be disabled. Read once at
    /// construction; overridable via
    /// `set_per_symbol_bar_staleness_secs_for_test`.
    per_symbol_bar_staleness_secs: Arc<RwLock<Option<i64>>>,
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
    /// A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01: test-only panic
    /// injection seam. When set to `Some(symbol)`, the canonical per-symbol
    /// dispatch implementation
    /// ([`Self::dispatch_native_strategy_for_symbol_with_bar_and_facts`])
    /// panics deterministically for that exact symbol (trimmed,
    /// case-sensitive match) before doing any other work, letting tests
    /// exercise the real production dispatch seam's panic-isolation
    /// behavior without depending on an actual strategy-engine bug. `None`
    /// (the permanent production value) never fires.
    panic_on_symbol_for_test: Arc<Mutex<Option<String>>>,
    /// A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01: counts every call into
    /// the canonical per-symbol dispatch implementation
    /// ([`Self::dispatch_native_strategy_for_symbol_with_bar_and_facts`]),
    /// including one that panics. Always incremented (harmless in
    /// production, mirrors `bar_tick_dispatch_count`'s always-on counting);
    /// tests use it to prove fault isolation never retries a symbol.
    dispatch_call_count_for_test: Arc<AtomicU32>,
    /// B1B: Pending strategy bar input deposited by the signal route for the
    /// execution loop to consume on its next tick.
    ///
    /// `None` when no bar is pending (normal state between signals).
    /// Overwritten by each new deposit (single slot: new bar supersedes any
    /// unconsumed prior bar).  Consumed atomically (set to `None`) by
    /// `tick_strategy_dispatch`.
    pending_strategy_bar_input: Arc<Mutex<Option<StrategyBarInput>>>,
    /// D4.4: test-only rendezvous hook for the completed-bar driver's
    /// post-claim/pre-dispatch concurrency proof (see
    /// [`autonomous_completed_bar_driver::AutonomousCompletedBarPostClaimTestHook`]).
    /// `None` in production and for every test that does not explicitly
    /// install it; read once per `RunningDispatch` claim, never blocking
    /// when absent.
    completed_bar_post_claim_test_hook: Arc<
        Mutex<
            Option<Arc<autonomous_completed_bar_driver::AutonomousCompletedBarPostClaimTestHook>>,
        >,
    >,
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-COVERAGE-ANCHOR-AND-RUN-
    /// LINEAGE-FOUNDATION closure REPAIR 6: test-only rendezvous hook for
    /// the coordinator's post-`create_or_recover`/pre-coverage-authority
    /// concurrency proof (see
    /// [`autonomous_daily_coordinator::AutonomousCoverageAuthorityPreBindTestHook`]).
    /// `None` in production and for every test that does not explicitly
    /// install it; read once per coordinator tick, never blocking when
    /// absent.
    coverage_authority_pre_bind_test_hook: Arc<
        Mutex<
            Option<Arc<autonomous_daily_coordinator::AutonomousCoverageAuthorityPreBindTestHook>>,
        >,
    >,
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4-EVALUATION-LINEAGE-AND-
    /// AUTONOMOUS-PREOPEN-CLOSURE-01 REPAIR 4 test seam: when `true`,
    /// `claim_and_dispatch_observed_bar` simulates a
    /// `complete_autonomous_daily_bar_dispatch` store error immediately
    /// after a confirmed evaluation, instead of performing the real write —
    /// so the mandatory authoritative re-read-on-store-error path can be
    /// proven deterministically. `false` in production and for every test
    /// that does not explicitly install it; the real completion write always
    /// runs when this is `false`.
    completed_bar_completion_fault_test_hook: Arc<AtomicBool>,
    /// REPAIR 7: test-only clock seam for the supervised completed-bar
    /// task's own production tick (`autonomous_completed_bar_task::run_one_production_tick`).
    /// `None` in production and for every test that does not explicitly
    /// install it — production always captures `Utc::now()` once per task
    /// tick. When `Some`, the supervised task's tick uses this caller-
    /// supplied instant instead, while still going through the identical
    /// task ownership, supervisor, cancellation, production adapter, and
    /// durable operation lookup real production ticks use.
    completed_bar_task_clock_override: Arc<Mutex<Option<DateTime<Utc>>>>,
    /// B3: Unix-second timestamp of the last `deposit_strategy_bar_input` call.
    ///
    /// Set to `input.end_ts` on every deposit; never cleared on stop/restart.
    /// Zero means no bar input has been deposited in this daemon process lifetime.
    /// Read by `/api/v1/strategy/summary` to surface honest `last_decision_time`.
    last_bar_input_ts: Arc<AtomicI64>,
    /// AUTON-NO-TRADE-01: Sum of target quantities from the last bar dispatch.
    ///
    /// Set after each `tick_strategy_dispatch` that yields a bar result.  Zero
    /// means the strategy returned no net-positive targets (signal = hold/flat).
    /// `i64::MIN` means no bar has been dispatched this session (sentinel).
    /// Surfaced in `/api/v1/strategy/summary` as `last_bar_signal_qty`.
    last_bar_signal_qty: Arc<AtomicI64>,
    /// AUTON-NO-TRADE-01: Total bar ticks dispatched to the native strategy this session.
    ///
    /// Incremented each time `tick_strategy_dispatch` fires a real bar result.
    /// Zero means no bar has been dispatched yet.  Reset on run-start via
    /// `reset_bar_tick_counters`.  Surfaced in `/api/v1/strategy/summary`.
    bar_tick_dispatch_count: Arc<AtomicU64>,
    /// AUTON-SIGNAL-CONTEXT-01: Number of DB bars used in the most recent
    /// `tick_strategy_dispatch` call.
    ///
    /// `-1` (sentinel) means no dispatch has occurred yet.
    /// `0` means the last dispatch used the single-stub fallback (no DB bars).
    /// `> 0` means that many completed bars were loaded from `md_bars`.
    ///
    /// Surfaced in `/api/v1/autonomous/readiness` as `bar_context_bars_loaded`.
    last_bar_context_bars: Arc<AtomicI64>,
    /// STRATEGY-DECISION-OBSERVABILITY-01: Diagnostic snapshot from the most
    /// recent native strategy bar dispatch.
    ///
    /// `None` until the first bar is dispatched.  Replaced atomically on each
    /// dispatch.  Read-only; does not affect the decision path.
    last_strategy_diagnostics: Arc<Mutex<Option<mqk_strategy::IntradayScalperDiagnostics>>>,
    /// MULTI-STRATEGY-DRY-RUN-STATUS-01: Latest dry-run secondary-strategy
    /// diagnostic snapshot.
    ///
    /// Empty `Vec` when `MQK_DRY_RUN_STRATEGY_IDS` is unset (default-off) or
    /// when no dry-run evaluation has run yet this process lifetime.
    /// Replaced wholesale on each tick's dry-run evaluation — never appended
    /// — so storage is bounded by the configured dry-run strategy count, not
    /// by tick count. Diagnostics only: every entry has `submitted == false`
    /// (see `state/dry_run_strategy.rs` for the structural proof) and this
    /// field is never read by any decision/submission path.
    dry_run_diagnostics: Arc<RwLock<Vec<DryRunStrategyDiagnostic>>>,
    /// MULTI-STRATEGY-DRY-RUN-STATUS-01: Unix-second timestamp of the most
    /// recent write to `dry_run_diagnostics`. Zero (sentinel) means no
    /// dry-run evaluation has been stored yet this process lifetime.
    dry_run_diagnostics_evaluated_at: Arc<AtomicI64>,
    /// AUTON-PAPER-RISK-03: Alpaca adapter populated by the cold-fetch branch of
    /// `build_execution_orchestrator` (External source, `broker_snapshot` empty
    /// at entry); stays `None` on the pre-seeded path (e.g.
    /// `adopt-broker-position-baseline`) and for Synthetic source or before the
    /// first run start.
    ///
    /// PAPER-EAGER-SNAPSHOT-REFRESH-WIRE-01: this field is write-only in
    /// production now. The eager/periodic broker snapshot refresh in
    /// `loop_runner.rs` and the terminal-fill expiry refresher in
    /// `build_execution_orchestrator` both read `snapshot_fetcher` via
    /// `select_external_snapshot_fetcher` instead — that seam is populated
    /// once in `AppState::new()` independent of `broker_snapshot` seed state,
    /// so it does not go dead on the pre-seeded path. This field is retained
    /// for `scenario_external_snapshot_refresh_risk03.rs` field-mechanics
    /// proofs only.
    pub external_snapshot_refresher: Arc<RwLock<Option<Arc<AlpacaBrokerAdapter>>>>,
    /// HEARTBEAT-TICK-01: Unix-second timestamp of the last completed execution-loop tick.
    ///
    /// Written by `loop_runner` at the end of each successful tick (after
    /// orchestrator progress and snapshot commit).  Zero until the first tick
    /// completes.  Read by operator surfaces to detect a stalled or non-progressing
    /// loop while `status.state == "running"`.
    execution_last_tick_at: Arc<AtomicI64>,
    /// BACKTEST-DAEMON-JOBS-01: In-memory backtest job registry.
    ///
    /// Process-lifetime only. No DB persistence. Isolated from live/paper execution.
    pub backtest_jobs: BacktestJobStore,
    /// DATA-INGEST-DAEMON-JOBS-01: Market-data ingest job registry.
    ///
    /// Process-lifetime fallback when DB is not configured; DB-backed job
    /// history is used by ingest routes when `db` is present.
    /// Isolated from live/paper execution: no broker adapters, no OMS tables.
    pub ingest_jobs: IngestJobStore,
    /// STRATEGY-SCANNER-JOBS-GUI-01B: In-memory strategy scanner job registry.
    ///
    /// Process-lifetime only. No DB persistence. Isolated from live/paper
    /// execution: no broker adapters, no OMS tables, no arm_state dependency.
    /// Runs the same local-data-only scanner core as `mqk backtest
    /// scan-strategies` (`mqk_backtest::execute_strategy_scan` /
    /// `write_scan_artifacts`).
    pub strategy_scan_jobs: StrategyScanJobStore,
    /// STRATEGY-SCANNER-JOBS-GUI-01C: Root directory the read-only artifact
    /// route (`GET /api/v1/strategy-scans/artifact`) will serve from.
    ///
    /// Default: "exports/strategy_scans" (relative to daemon CWD). Override:
    /// MQK_STRATEGY_SCAN_ARTIFACT_ROOT env var. A requested `artifact_dir`
    /// that does not resolve inside this root is refused
    /// (`truth_state="path_rejected"`) — this route never reads an arbitrary
    /// file path.
    pub strategy_scan_artifact_root: String,
    /// STRATEGY-SCANNER-PROMOTION-01D: Root directory the read-only review
    /// artifact route (`GET /api/v1/strategy-scans/review-artifact`) will
    /// serve from.
    ///
    /// Default: "exports/strategy_reviews" (relative to daemon CWD).
    /// Override: MQK_STRATEGY_REVIEW_ARTIFACT_ROOT env var. A requested
    /// `review_dir` that does not resolve inside this root is refused
    /// (`truth_state="path_rejected"`) — this route never reads an
    /// arbitrary file path.
    pub strategy_review_artifact_root: String,
    /// PROMOTION-WALKFORWARD-GATE-WIRING-01: trusted, operator-configured
    /// path to the durable Research SQLite registry (the same database
    /// `mqk_research.exp_distributed.storage.ResearchResultStore` writes --
    /// see `mqk_promotion::research_registry::load_research_authority`).
    ///
    /// Sourced ONLY from `MQK_RESEARCH_REGISTRY_DB` -- never from request
    /// JSON, a query parameter, or any other caller-suppliable value; there
    /// is no fixed-path fallback. `None` means the P7C research-evidence
    /// gate is not configured on this daemon, and every evidence-requiring
    /// promotion transition fails closed (see
    /// `routes::strategy_promotions::strategy_promotion_transition`).
    pub research_registry_db_path: Option<String>,
    /// PROMOTION-WALKFORWARD-GATE-WIRING-01: root directory the P7C
    /// research-evidence gate reads `research_evidence_dir` /
    /// `research_judge_artifact_path` from. A requested path that does not
    /// resolve inside this root is refused -- same canonicalize+root-prefix
    /// pattern as `strategy_review_artifact_root`. Sourced ONLY from
    /// `MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT`; no fixed-path fallback.
    pub research_evidence_artifact_root: Option<String>,
    /// PROMOTION-WALKFORWARD-GATE-WIRING-01: explicit, versioned minimum
    /// Deflated/Probabilistic Sharpe Ratio the P7C gate requires (mirrors
    /// `mqk_promotion::PromotionConfig::min_deflated_sharpe_ratio`). Sourced
    /// ONLY from `MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO`; no hidden
    /// default -- `None` fails the gate closed rather than silently
    /// widening it.
    pub research_min_deflated_sharpe_ratio: Option<f64>,
    /// PROMOTION-WALKFORWARD-GATE-WIRING-01: explicit, versioned maximum
    /// Probability of Backtest Overfitting the P7C gate allows (mirrors
    /// `mqk_promotion::PromotionConfig::max_probability_backtest_overfitting`).
    /// Sourced ONLY from `MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING`;
    /// no hidden default.
    pub research_max_probability_backtest_overfitting: Option<f64>,
    /// PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE: root directory
    /// `mqk_promotion::resolve_backtest_evidence` searches for a candidate's
    /// canonical `BacktestReport`/`ArtifactLock`/`StressSuiteResult`
    /// evidence (`<this_root>/<backtest_run_id>/`, the exact convention
    /// `mqk_artifacts::init_run_artifacts` writes every run to). Sourced
    /// ONLY from `MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT` -- never from request
    /// JSON; `None` means the backtest-evidence gate is not configured on
    /// this daemon, and every evidence-requiring promotion transition fails
    /// closed.
    pub backtest_evidence_artifact_root: Option<String>,
    /// PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE: explicit,
    /// versioned promotion metrics thresholds (mirrors
    /// `mqk_promotion::PromotionConfig`'s five metrics fields; DSR/PBO reuse
    /// the existing `research_min_deflated_sharpe_ratio`/
    /// `research_max_probability_backtest_overfitting` fields above rather
    /// than duplicating them). Sourced ONLY from `MQK_PROMOTION_MIN_SHARPE`
    /// / `MQK_PROMOTION_MAX_MDD` / `MQK_PROMOTION_MIN_CAGR` /
    /// `MQK_PROMOTION_MIN_PROFIT_FACTOR` /
    /// `MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT`; no hidden default -- any
    /// one missing fails the gate closed rather than silently widening it.
    pub promotion_min_sharpe: Option<f64>,
    pub promotion_max_mdd: Option<f64>,
    pub promotion_min_cagr: Option<f64>,
    pub promotion_min_profit_factor: Option<f64>,
    pub promotion_min_profitable_months_pct: Option<f64>,
    /// DATA-INGEST-GUI-SYNC-ALL-01: Filesystem path to the canonical instrument registry.
    ///
    /// Read at route-time (not cached) by GET /api/v1/ingest/tracked-equities.
    /// Default: "config/instruments/equities.json" (relative to daemon CWD).
    /// Override: MQK_INSTRUMENT_REGISTRY_PATH env var.
    pub instrument_registry_path: String,
    /// INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED: Optional filesystem path to a
    /// separate `InstrumentRegistryV2` source, read only by the read-only
    /// GET /api/v1/backtests/economics-suggestion route.
    ///
    /// `None` (the default) means no v2 source is configured: the route's
    /// behavior is then exactly the pre-existing v1-equities-only behavior.
    /// `Some(path)` only when MQK_INSTRUMENT_REGISTRY_V2_PATH is explicitly
    /// set — there is no fixed-path fallback, so committing an example v2
    /// fixture file never silently changes route behavior. This path is never
    /// read by live/paper trading, broker adapters, risk gates, OMS, or
    /// ingestion — read-only backtest economics suggestions only.
    pub instrument_registry_v2_path: Option<String>,
    /// ASSET-CORE-04F: Optional filesystem path to a registry-v2 source
    /// consumed only by `GET /api/v1/portfolio/economics/status` when a
    /// caller explicitly requests `?registry_source=v2`.
    ///
    /// `None` (the default, unset `MQK_PORTFOLIO_ECONOMICS_REGISTRY_V2_PATH`)
    /// means the route always uses its pre-existing legacy v1-registry
    /// behavior regardless of what `registry_source` a caller passes -- there
    /// is no fixed-path fallback, so committing an example v2 fixture file
    /// never silently changes default route behavior. Deliberately separate
    /// from `instrument_registry_v2_path` above (read only by
    /// `GET /api/v1/backtests/economics-suggestion`) so the two read-only
    /// routes' v2 configuration can never be accidentally conflated under one
    /// operator-set env var. Never read by live/paper trading, broker
    /// adapters, risk gates, OMS, ingestion, or any runtime/risk/order path.
    pub portfolio_economics_registry_v2_path: Option<String>,
    /// DATA-PROVIDER-REGISTRY-01: Filesystem path to the canonical provider registry.
    ///
    /// Read at route-time (not cached) by provider dry-run handlers.
    /// Default: "config/providers/providers.json" (relative to daemon CWD).
    /// Override: MQK_PROVIDER_REGISTRY_PATH env var.
    pub provider_registry_path: String,
    /// INTRADAY-MD-REFRESHER-OPERATOR-SURFACE-01: Directory containing intraday refresh
    /// evidence files written by Refresh-IntradayMarketData.ps1.
    ///
    /// Read at route-time (not cached) by GET /api/v1/market-data/intraday-refresh/status.
    /// Default: "exports/market_data" (relative to daemon CWD).
    /// Override: MQK_MD_REFRESH_EVIDENCE_DIR env var.
    pub md_refresh_evidence_dir: String,
    /// CRYPTO-DATA-02C-KRAKEN-SCHEDULER-READINESS-STATUS-SURFACE-01: Filesystem
    /// path to the CRYPTO-DATA-02A scheduler rate-limit/cadence policy JSON.
    ///
    /// Read at route-time (not cached) by
    /// GET /api/v1/market-data/kraken-scheduler/readiness. Never mutated.
    /// Default: "docs/specs/crypto_data_02a_kraken_scheduler_rate_limit_decision.json"
    /// (relative to daemon CWD). Override: MQK_KRAKEN_SCHEDULER_POLICY_PATH env var.
    pub kraken_scheduler_policy_path: String,
    /// DATA-INGEST-DAEMON-PROVIDER-JOBS-01: Injectable provider client for sync jobs.
    ///
    /// `None` in production: the provider sync background task reads
    /// `TWELVEDATA_API_KEY` from the environment and constructs a real client.
    /// `Some(client)` in tests: the injected fake client is used directly,
    /// allowing zero-network test coverage without real TwelveData credentials.
    pub provider_client: Option<Arc<dyn mqk_md::HistoricalProvider>>,
    /// DATA-PROVIDER-LATEST-BAR-POLL-01: Injectable latest-bar provider client for poll-once.
    ///
    /// `None` in production: the poll-once route builds from the provider registry.
    /// `Some(client)` in tests: the injected capability-aware fake is used directly,
    /// allowing zero-network latest-bar tests without provider credentials.
    pub latest_bar_provider_client: Option<Arc<dyn mqk_md::MarketDataProvider>>,
    /// INGEST-JOB-CANCEL-STATUS-CONSTRAINT-REPAIR-01: Test-only synchronization
    /// barrier for reproducing the exact check-then-act race window in
    /// `run_real_provider_sync`'s per-symbol progress write (routes/ingest.rs):
    /// its in-memory record is read *before* this pause, and its durable
    /// persist happens *after* — the same gap a concurrent cancel can commit
    /// inside of in production.
    ///
    /// `None` in production: no effect. `Some(notify)` in tests: that one
    /// write pauses at `notify.notified()` immediately before its own
    /// `persist_ingest_job_record` call, so a test can deterministically let
    /// a concurrent cancel commit first, then release the stale write —
    /// reproducing the race with a real barrier instead of a timing sleep.
    pub ingest_job_persist_barrier_for_test: Option<Arc<tokio::sync::Notify>>,
    /// Companion signal to `ingest_job_persist_barrier_for_test`: notified
    /// the instant a background write reaches the pause point, so a test
    /// can deterministically wait for "the background task is now paused
    /// here" instead of guessing with a sleep before issuing its own
    /// concurrent write.
    pub ingest_job_persist_barrier_entered_for_test: Option<Arc<tokio::sync::Notify>>,
    /// PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01:
    /// Test-only synchronization barrier reproducing the exact window this
    /// patch closes: `mqk_db::enforce_deadman_or_halt` has already atomically
    /// committed `runs.status = HALTED` + `sys_arm_state = DISARMED`, but the
    /// execution-loop task (`state/loop_runner.rs` pre-tick deadman branch)
    /// has not yet performed its local integrity mutation, alert, or
    /// leadership release — i.e. the task is still a live, unfinished local
    /// owner even though the durable halt is already visible.
    ///
    /// `None` in production: no effect. `Some(notify)` in tests: the loop
    /// pauses at `notify.notified()` at exactly that point, so a test can
    /// deterministically drive the real `clear-halted-run` route while a
    /// genuine `spawn_execution_loop` task for the halted run is still
    /// locally owned — a real barrier instead of a timing sleep.
    pub deadman_local_quiescence_pause_for_test: Option<Arc<tokio::sync::Notify>>,
    /// Companion signal to `deadman_local_quiescence_pause_for_test`:
    /// notified the instant the loop reaches the pause point, so a test can
    /// deterministically wait for "the durable halt has committed and the
    /// loop is now paused here" instead of guessing with a sleep before
    /// issuing its own concurrent `clear-halted-run` call.
    pub deadman_local_quiescence_pause_entered_for_test: Option<Arc<tokio::sync::Notify>>,
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-TRUTH-AND-EVIDENCE-STATE-REPAIR-01:
    /// Test-only override forcing `gather_daily_operation_activity_counts`
    /// to report a downstream database read failure (`ActivityCounts::DatabaseUnavailable`)
    /// without touching the database, so scenario tests can drive the real
    /// single/history/summary daily-operation route handlers through a
    /// count-read failure deterministically. Always `false` in production —
    /// no production call site ever sets it.
    pub(crate) force_activity_counts_database_unavailable_for_test: bool,
    /// DATA-PROVIDER-LATEST-BAR-POLL-01: Process-local last poll result for feed status.
    pub market_data_feed_status:
        Arc<RwLock<Option<crate::api_types::MarketDataFeedPollOnceResponse>>>,
    /// DATA-PROVIDER-LATEST-BAR-SCHEDULER-01: Process-local latest-bar scheduler.
    ///
    /// Disabled by default. Holds only in-memory task/config/status state.
    pub market_data_feed_scheduler: Arc<Mutex<MarketDataFeedSchedulerRuntimeState>>,
    /// MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01: Process-local required-
    /// universe market-data freshness controller/scheduler.
    ///
    /// Disabled by default. Maintains every symbol/timeframe the currently
    /// configured autonomous Paper operation requires (derived from the same
    /// `required_symbols_with_source_from_env()` resolver as the ingest-plan
    /// and premarket readiness surfaces) by reusing the existing latest-bar
    /// poll seam per resolved provider/timeframe group. Not durable across
    /// restart — see `required_market_data_autofresh` module docs.
    pub required_universe_scheduler:
        Arc<Mutex<required_market_data_autofresh::RequiredUniverseSchedulerRuntimeState>>,
    /// BROKER-FILL-REST-RECOVERY-01: Injectable Alpaca fill activity fetcher.
    ///
    /// `None` when REST recovery is not configured on this daemon instance.
    /// Set via `set_fill_activity_fetcher_for_test` in tests; production wiring
    /// is deferred to BROKER-FILL-REST-RECOVERY-APPLY-01.
    pub fill_activity_fetcher: Option<Arc<dyn BrokerFillActivityFetcher>>,
    /// BRK-GAP-REST-RECOVERY-01: Injectable account-wide fill fetcher for WS gap recovery.
    ///
    /// Fetches all FILL/PARTIAL_FILL activities since a cursor position without
    /// filtering by order ID.  Used by the ws-gap-fill-recovery repair route.
    /// `None` when not configured (tests inject a fake; production wiring
    /// is a follow-up once the service function is proven safe).
    pub ws_gap_fill_fetcher: Option<Arc<dyn WsGapFillFetcher>>,
    /// BROKER-POSITION-BASELINE-ADOPTION-01: Adopted broker position baseline.
    ///
    /// Operator-confirmed local truth snapshot used by the reconcile tick as
    /// `local_fn()` when no execution run is active.  `None` at boot until the
    /// operator calls `POST /api/v1/ops/repair/adopt-broker-position-baseline`.
    ///
    /// Seeded from `sys_broker_position_baseline` at daemon boot by
    /// `seed_broker_baseline_from_db()`.  Written by the adoption route.
    pub broker_baseline: Arc<RwLock<Option<mqk_reconcile::LocalSnapshot>>>,
    /// BROKER-SNAPSHOT-REFRESH-FOR-BASELINE-01: On-demand broker snapshot fetcher.
    ///
    /// Used by the adopt-broker-position-baseline route when the in-memory broker
    /// snapshot is absent at adoption time (daemon idle — no active run).
    /// `None` when credentials are absent or broker kind is not Alpaca.
    /// Tests inject a fake implementation via `set_snapshot_fetcher_for_test`.
    pub snapshot_fetcher: Option<Arc<dyn BrokerSnapshotFetcher>>,
    /// SHORT-SIDE-EXTERNAL-SIGNAL-WIRING-01: read-only broker asset shortability
    /// preflight fetcher.
    ///
    /// `None` when the selected broker is unsupported or credentials are absent.
    /// Signal admission treats absence as fail-closed for short-open intents.
    pub asset_shortable_preflight_fetcher: Option<Arc<dyn BrokerAssetShortablePreflightFetcher>>,
    /// DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: Per-run set of symbols for which a
    /// B5 short-sale guard Discord alert has already been fired.
    ///
    /// Dedup: at most one alert per (run, symbol).  Prevents every-tick spam when
    /// the strategy repeatedly targets a short that the guard rejects.  Reset at
    /// run start alongside `day_signal_count`.
    b5_alerted_symbols: Arc<RwLock<HashSet<String>>>,
    /// DISCORD-SIGNAL-BLOCKED-GATE-ALERTS-01: Flag set on the first Discord alert
    /// for day-signal-limit-reached.  Prevents repeated alerts when multiple
    /// signals arrive after the limit is hit.  Reset at run start.
    day_limit_alert_fired: Arc<AtomicBool>,
    /// MULTI-SYMBOL-CAPITAL-CAPS-01: Per-run set of symbols for which a cap #2
    /// (`per_symbol_max_position_qty`) target-qty clamp Discord alert has
    /// already been fired.
    ///
    /// Dedup: at most one alert per (run, symbol). Reset at run start
    /// alongside `b5_alerted_symbols`.
    per_symbol_position_cap_alerted_symbols: Arc<RwLock<HashSet<String>>>,
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3: process-local completed-bar
    /// task ownership/liveness/cancellation handles. Constructed once per
    /// `AppState` and never reconstructed for the process lifetime.
    completed_bar_task: autonomous_completed_bar_task::AutonomousCompletedBarTaskRuntime,
    /// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: test-only
    /// fault-injection seam for the atomic start-commit sequence. `None` in
    /// production and for every test that does not explicitly install one.
    dynamic_selection_fault_seam: Arc<RwLock<Option<DynamicSelectionLifecycleFaultSeam>>>,
    /// PHASE-7A-FINAL-PRIVATE-PRODUCTION-EFFECTS-PROOF requirement 6: a
    /// narrow, always-`false`-in-production hermetic-broker override. When
    /// a test explicitly enables it (`#[cfg(test)]`-only setter), the one
    /// `build_daemon_broker` call site in `build_execution_orchestrator`
    /// that would refuse `BrokerKind::Paper` instead constructs
    /// `DaemonBroker::Paper(LockedPaperBroker::default())` directly — the
    /// same in-process, zero-network, zero-credential broker
    /// `build_daemon_broker` would build if it did not refuse Paper as a
    /// business-rule gate. This never weakens `build_daemon_broker` itself
    /// (the real gate is untouched and always applies whenever this flag is
    /// `false`, which is always true in production); it only lets a
    /// `#[cfg(test)]` caller reach a genuinely successful
    /// `ProductionRuntimeStartEffects` start hermetically, through the
    /// exact same production code path.
    hermetic_test_broker_override: Arc<RwLock<bool>>,
    /// PHASE-7A-FINAL-PRIVATE-PRODUCTION-EFFECTS-PROOF requirement 6:
    /// hermetic injection point for the barrier-cancellation / Active-
    /// install-failure matrix. Always `false` in production and for every
    /// test that does not explicitly enable it (setter is
    /// `#[cfg(test)]`-gated). See `install_active_runtime`.
    force_install_active_runtime_conflict: Arc<AtomicBool>,
    /// PHASE-7A-FINAL-PRIVATE-PRODUCTION-EFFECTS-PROOF requirement 6:
    /// hermetic injection point forcing `release_orchestrator_leadership`'s
    /// outcome to a simulated failure. Always `false` in production and for
    /// every test that does not explicitly enable it.
    force_leadership_release_failure: Arc<AtomicBool>,
    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 1 requirement 6:
    /// counts how many times `spawn_execution_loop`'s two pre-barrier exit
    /// branches (barrier-sender-dropped, stop-before-barrier-release)
    /// released the orchestrator's runtime leadership lease. Incremented
    /// unconditionally (a plain atomic increment, harmless in production);
    /// only the `#[cfg(test)]` reader is test-only. Exists solely so a test
    /// can prove "exactly once" for a given start attempt rather than
    /// inferring it from the absence of a second failure.
    pre_barrier_leadership_release_count: Arc<AtomicU32>,
    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 3 row 21:
    /// hermetic injection point forcing the spawned execution loop task to
    /// panic immediately after the startup barrier releases (before any
    /// economic work). Always `false` in production and for every test
    /// that does not explicitly enable it (setter is `#[cfg(test)]`-gated).
    force_execution_loop_panic: Arc<AtomicBool>,
    /// TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 Blocker 3: an ordered
    /// event log recorded directly at the real Bundle 6/Bundle 5/selected-
    /// host/legacy-dispatch/submission call sites inside the real execution
    /// loop tick body (`state/loop_runner.rs`) and the real dispatch
    /// backends (`state.rs`). Always present but always empty in production
    /// — every push call site is `#[cfg(test)]`-gated inline, so this field
    /// is inert (never written, never read) outside this crate's own test
    /// build. No separate/fake coordinator or economic pipeline: these are
    /// the exact production call sites, only observed.
    ///
    /// Neither read nor written by anything in a non-test build (both the
    /// push call sites and the snapshot/clear readers are `#[cfg(test)]`),
    /// so a plain `--lib` (non-test) compilation sees it as dead — allowed
    /// deliberately rather than restructuring a field that exists solely
    /// for this crate's own hermetic test build.
    #[allow(dead_code)]
    loop_call_trace: Arc<std::sync::Mutex<Vec<String>>>,
}

/// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 2: every locally-
/// prepared value a successful run-start attempt commits to `AppState` in
/// one call (`AppState::commit_run_start_bundle`), built up entirely from
/// local (unpublished) bindings before any of it is written. `None` fields
/// are legitimate (e.g. `execution_snapshot` when the orchestrator's first
/// snapshot call failed; `dynamic_selection_outcome` is always `Some` in
/// production but kept optional for callers that never evaluate dynamic
/// selection).
pub(crate) struct RunStartLocalBundle {
    pub(crate) execution_snapshot: Option<mqk_runtime::observability::ExecutionSnapshot>,
    pub(crate) accepted_artifact: Option<AcceptedArtifactProvenance>,
    pub(crate) native_strategy_bootstrap: Option<NativeStrategyBootstrap>,
    pub(crate) dynamic_selection_outcome: Option<DynamicSelectionRuntimeState>,
}

/// BROKER-FILL-REST-RECOVERY-01: Injectable abstraction over Alpaca REST activity fetch.
///
/// Defined here so both `state.rs` (storage) and `routes/repair.rs` (usage) can reference
/// it without a module cycle.  Tests inject a fake implementation; production wiring
/// is deferred to BROKER-FILL-REST-RECOVERY-APPLY-01.
pub trait BrokerFillActivityFetcher: Send + Sync {
    /// Fetch account activities for the given Alpaca broker order UUID.
    ///
    /// PAPER-SOAK-ALPACA-FILL-ECONOMIC-AUTHORITY-CLOSURE-01: Alpaca's Account
    /// Activities endpoint does not document `order_id` as a supported query
    /// parameter, so implementations must not rely on undocumented
    /// server-side filtering by it. The production implementation
    /// (`AlpacaBrokerAdapter::fetch_fill_activities_for_order`) paginates the
    /// account-wide FILL activity feed to exhaustion (or a bounded page
    /// limit, failing closed if exceeded) and filters to an exact
    /// `activity.order_id == broker_order_id` match locally.
    ///
    /// Callers must still filter the returned list for genuine FILL-class
    /// activities with a recognized subtype (`classify_fill_subtype`) and
    /// must independently re-verify exact `order_id` equality before any
    /// mutation — defense in depth against a test-injected or
    /// non-conforming implementation of this trait that does not uphold the
    /// exact-match contract. Returns `Err(String)` if the REST call fails.
    fn fetch_fill_activities_for_order(
        &self,
        broker_order_id: &str,
    ) -> Result<Vec<mqk_broker_alpaca::types::AlpacaOrderActivity>, String>;
}

/// BRK-GAP-REST-RECOVERY-01: Injectable abstraction over account-wide Alpaca fill fetch.
///
/// Unlike `BrokerFillActivityFetcher` (which filters by `order_id`), this trait
/// fetches all FILL and PARTIAL_FILL activities since a cursor position.  It is
/// the correct seam for WS gap recovery where the specific broker_order_ids that
/// were active during the gap window are not known up-front.
///
/// Tests inject a fake implementation.  Production wiring via `AlpacaWsGapFillFetcher`
/// in `state/broker.rs` once the service function is proven safe by scenario tests.
pub trait WsGapFillFetcher: Send + Sync {
    /// Fetch all FILL and PARTIAL_FILL account activities since `since_activity_id`.
    ///
    /// `since_activity_id`: if `Some`, only activities with an ID strictly after
    /// this value are returned (exclusive lower bound, ascending order, Alpaca
    /// `?after=` semantics).  If `None`, implementation returns recent activities
    /// from a reasonable default window.
    ///
    /// Returns `Err(String)` if the REST call fails; callers treat this as
    /// REST unavailable and fail closed (no mutation).
    fn fetch_fills_since(
        &self,
        since_activity_id: Option<&str>,
    ) -> Result<Vec<mqk_broker_alpaca::types::AlpacaOrderActivity>, String>;
}

/// BROKER-SNAPSHOT-REFRESH-FOR-BASELINE-01: Injectable on-demand broker snapshot fetcher.
///
/// Used by the adopt-broker-position-baseline route when the in-memory broker
/// snapshot is absent and a fresh authoritative read is needed before adoption.
///
/// Tests inject a fake implementation via `set_snapshot_fetcher_for_test`.
/// Production wiring via `build_snapshot_fetcher_from_env` in `state/broker.rs`.
pub trait BrokerSnapshotFetcher: Send + Sync {
    /// Fetch a point-in-time broker snapshot.
    ///
    /// Returns `Err(String)` if the fetch fails; the adoption route treats this
    /// as `repair.broker_snapshot_refresh_failed` and refuses adoption.
    fn fetch_snapshot(&self) -> Result<mqk_schemas::BrokerSnapshot, String>;
}

/// Read-only broker asset shortability metadata used as a short-entry
/// preflight input.
#[derive(Debug, Clone)]
pub struct BrokerAssetShortablePreflight {
    pub symbol: String,
    pub asset_class: String,
    pub tradable: bool,
    pub shortable: bool,
    pub marginable: Option<bool>,
    pub easy_to_borrow: Option<bool>,
    pub source: String,
}

/// Outcome of querying read-only broker asset shortability metadata.
#[derive(Debug, Clone)]
pub enum BrokerAssetShortablePreflightOutcome {
    Active(BrokerAssetShortablePreflight),
    NotConfigured,
    UnsupportedAdapter,
    SymbolNotFound,
    QueryFailed(String),
}

/// SHORT-SIDE-EXTERNAL-SIGNAL-WIRING-01: injectable read-only asset preflight.
///
/// Implementations must not submit orders or mutate broker/DB state.
pub trait BrokerAssetShortablePreflightFetcher: Send + Sync {
    fn fetch_asset_shortable_preflight(
        &self,
        symbol: &str,
    ) -> Result<Option<BrokerAssetShortablePreflight>, String>;
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
///
/// `Clone`: MULTI-SYMBOL-DISPATCH-LOOP-01's
/// [`AppState::tick_strategy_dispatch_multi_symbol`] takes the single
/// pending input once per tick and dispatches a clone of it to each
/// configured symbol.
#[derive(Debug, Clone)]
pub struct StrategyBarInput {
    pub now_tick: u64,
    pub end_ts: i64,
    pub limit_price: Option<i64>,
    pub qty: i64,
}

/// RUNTIME-OPPORTUNITY-ALLOCATION-01 authority repair (Phase A — exact
/// strategy bar authority): the exact immutable completed-bar facts a
/// strategy's `on_bar` evaluation was actually run against, captured at the
/// same point the DB bar window is loaded (never re-derived or re-fetched
/// afterward).
///
/// `symbol` / `timeframe` / `strategy_id` identify which dispatch produced
/// this bar; `bar_end_ts` / `close_micros` are copied verbatim from the last
/// (most recent) row of the exact `db_bars` window
/// [`AppState::dispatch_native_strategy_for_symbol_with_loaded_bars_and_facts`]
/// evaluated the strategy against. Any later-arriving bar in the DB cannot
/// change these fields — they are a snapshot in time, not a live query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluatedBarFacts {
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe: String,
    pub bar_end_ts: i64,
    pub close_micros: i64,
}

/// PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE Part 4: the outcome of
/// [`AppState::prepare_bar_window_for_symbol_timeframe`] — the one common
/// bar-window preparation authority both the legacy and selected-host
/// dispatch backends call. `Refused` covers the exact same missing/stale-bar
/// fail-closed cases the pre-Phase-7B single implementation returned `None`
/// for (already durably recorded via `record_signal_evaluation` before this
/// is returned) — no backend gets a second chance to reinterpret a refusal.
enum BarWindowPrepOutcome {
    Refused,
    Ready {
        window: mqk_strategy::RecentBarsWindow,
        /// The exact latest (most recent) completed bar this window was
        /// built from — copied verbatim into `EvaluatedBarFacts` by the
        /// caller, never re-derived.
        latest_bar_row: mqk_db::MdBarRow,
        bars_loaded: usize,
        diagnostic_decision: &'static str,
        diagnostic_reason: &'static str,
    },
}

/// PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE Part 5: every way a
/// selected-host dispatch result can fail coherence with its frozen
/// [`crate::dynamic_selection_dispatch_authority::SelectedDispatchBinding`].
/// A structural fault, never an ordinary no-signal condition — the caller
/// must submit zero decisions for the whole affected tick and halt/disarm
/// fail-closed rather than fall back to legacy dispatch.
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields are read via the derived `Debug` impl in tracing::error! diagnostics
pub(crate) enum SelectedHostDispatchFault {
    HostMissingAtDispatch {
        symbol: String,
        strategy_id: String,
        timeframe_secs: i64,
    },
    HostOnBarError {
        symbol: String,
        strategy_id: String,
        detail: String,
    },
    /// A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01: `host.on_bar` panicked
    /// (a Rust unwind, distinct from an ordinary `Err` return). Treated the
    /// same as [`Self::HostOnBarError`] — zero decisions this tick, whole-
    /// tick halt — per this backend's own frozen contract (Part 5 doc
    /// comment on `tick_strategy_dispatch_selected_hosts_with_bar_facts`):
    /// any host malfunction here is a structural fault, never a per-symbol-
    /// only condition. The panic itself never escapes as an unwind past
    /// this call site.
    HostOnBarPanicked {
        symbol: String,
        strategy_id: String,
        detail: String,
    },
    SpecNameMismatch {
        symbol: String,
        expected_strategy_id: String,
        got: String,
    },
    SpecTimeframeMismatch {
        symbol: String,
        expected_secs: i64,
        got_secs: i64,
    },
    TargetSymbolMismatch {
        expected_symbol: String,
        got_symbol: String,
    },
}

/// PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE Part 5: the pure
/// selected-host result coherence check, extracted from `AppState::
/// tick_strategy_dispatch_selected_hosts_with_bar_facts` so every mismatch
/// branch is directly unit-testable against a hand-built
/// [`mqk_strategy::StrategyBarResult`], without needing a deliberately-
/// corrupted [`crate::dynamic_selection_host_pool::DynamicSelectionHostPool`]
/// (which the pool's own `build()` cannot produce — every mismatch this
/// checks for is structurally prevented by `DynamicSelectionHostPool::
/// build`'s own construction-time checks, making this pure defense-in-depth
/// unreachable in practice via the accepted construction path, exactly like
/// the codebase's other "checked explicitly anyway, never assumed" IR2-style
/// guards).
pub(crate) fn check_selected_host_result_coherence(
    binding: &crate::dynamic_selection_dispatch_authority::SelectedDispatchBinding,
    bar_result: &mqk_strategy::StrategyBarResult,
) -> Result<(), SelectedHostDispatchFault> {
    if bar_result.spec.name != binding.strategy_id {
        return Err(SelectedHostDispatchFault::SpecNameMismatch {
            symbol: binding.symbol.clone(),
            expected_strategy_id: binding.strategy_id.clone(),
            got: bar_result.spec.name.clone(),
        });
    }
    if bar_result.spec.timeframe_secs != binding.timeframe_secs {
        return Err(SelectedHostDispatchFault::SpecTimeframeMismatch {
            symbol: binding.symbol.clone(),
            expected_secs: binding.timeframe_secs,
            got_secs: bar_result.spec.timeframe_secs,
        });
    }
    for t in &bar_result.intents.output.targets {
        if !t.symbol.trim().eq_ignore_ascii_case(binding.symbol.trim()) {
            return Err(SelectedHostDispatchFault::TargetSymbolMismatch {
                expected_symbol: binding.symbol.clone(),
                got_symbol: t.symbol.clone(),
            });
        }
    }
    Ok(())
}

impl SelectedHostDispatchFault {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::HostMissingAtDispatch { .. } => "selected_host_missing_at_dispatch",
            Self::HostOnBarError { .. } => "selected_host_on_bar_error",
            Self::HostOnBarPanicked { .. } => "selected_host_on_bar_panicked",
            Self::SpecNameMismatch { .. } => "selected_host_spec_name_mismatch",
            Self::SpecTimeframeMismatch { .. } => "selected_host_spec_timeframe_mismatch",
            Self::TargetSymbolMismatch { .. } => "selected_host_target_symbol_mismatch",
        }
    }
}

/// A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01: best-effort extraction of a
/// human-readable message from a caught panic payload. Rust panic payloads
/// are conventionally `&'static str` (a string-literal `panic!`) or `String`
/// (a formatted `panic!`); anything else (a deliberate `panic_any` with a
/// custom type) falls back to a fixed label rather than guessing.
fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01: evidence that the real
/// `Strategy::on_bar` callback panicked, returned by
/// [`AppState::invoke_native_strategy_host_on_bar`]. `strategy_id` is
/// captured before the bootstrap quarantines itself, since
/// `active_strategy_id()` is no longer available afterward.
struct NativeStrategyOnBarPanicFault {
    strategy_id: String,
    detail: String,
}

/// TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 Blocker 2: explicit authority
/// for [`AppState::record_signal_evaluation`] (and the shared bar-window
/// preparation gate it's called from,
/// [`AppState::prepare_bar_window_for_symbol_timeframe`]) — the caller states
/// which run/strategy a journal row belongs to rather than the writer
/// rediscovering it internally. `record_signal_evaluation` must never consult
/// `native_strategy_bootstrap` or `status.active_run_id` on its own; every
/// caller passes one of these two variants explicitly.
#[derive(Clone, Copy)]
enum SignalEvaluationAuthority<'a> {
    /// The exact pre-Phase-7B behavior: resolves the active
    /// `native_strategy_bootstrap`'s strategy_id and `status.active_run_id`
    /// at write time. Used only by the `Legacy`/`Off`/`Shadow` dispatch
    /// backend.
    Legacy,
    /// PHASE-7B `DynamicPaperEnforced`: the frozen dispatch authority's exact
    /// `run_id` and the exact selected binding's `strategy_id` — never
    /// rediscovered from `native_strategy_bootstrap` or the mutable status
    /// cache, which may name a different run or a different symbol's
    /// selected strategy entirely.
    Explicit { run_id: Uuid, strategy_id: &'a str },
}

/// AUTON-NO-SIGNAL-OBS-01: one durable signal-evaluation journal write
/// attempt, scoped to a single symbol/timeframe/tick.
///
/// Plain data carrier for [`AppState::record_signal_evaluation`] — bundles
/// the call site's already-locally-known context so the helper itself takes
/// one argument instead of a long positional list. Borrowed `&str` fields are
/// only used for the duration of the call.
struct SignalEvaluationAttempt<'a> {
    now_tick: u64,
    symbol: &'a str,
    timeframe: &'a str,
    /// `"db_loaded"`, `"no_bars_available"`, or `"stale_bars"`.
    bar_context_source: &'static str,
    bars_loaded: i64,
    latest_bar_ts_utc: Option<DateTime<Utc>>,
    /// `false` is informational, not an error: hold/flat signal or a
    /// pre-dispatch gate refused before `on_bar` ran.
    signal_generated: bool,
    /// `None` only when a pre-dispatch gate refused before `on_bar` ran.
    signal_qty: Option<i64>,
    reason_code: &'static str,
    reason: &'static str,
    /// `"pre_dispatch_gate"` or `"strategy_evaluated"`.
    decision_stage: &'static str,
}

/// AUTON-NO-TRADE-OFFHOURS-01B: one durable no-trade diagnostic write
/// attempt, scoped to a single `GET /api/v1/autonomous/readiness` poll.
///
/// Plain data carrier for [`AppState::record_no_trade_diagnostic`] — bundles
/// the caller's already-locally-known gate truth so the helper itself takes
/// one argument instead of a long positional list. Borrowed `&str` fields
/// are only used for the duration of the call.
pub struct NoTradeDiagnosticSnapshot<'a> {
    /// `None` when no active run exists at observation time — the common
    /// off-hours case, never a fabricated default.
    pub run_id: Option<Uuid>,
    pub mode: &'a str,
    /// `"in_window"` or `"outside_window"`.
    pub session_window_state: &'a str,
    pub runtime_start_allowed: bool,
    pub arm_state: &'a str,
    pub overall_ready: bool,
    pub reason_code: &'a str,
    pub reason: &'a str,
    pub stage: &'a str,
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

    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-TRUTH-AND-EVIDENCE-STATE-REPAIR-01:
    /// Test helper — force the daily-operation read routes' activity-count
    /// gather step to report `ActivityCounts::DatabaseUnavailable`
    /// deterministically, without requiring a real broken database
    /// connection at exactly the second query in that gather sequence.
    pub fn set_force_activity_counts_database_unavailable_for_test(&mut self, value: bool) {
        self.force_activity_counts_database_unavailable_for_test = value;
    }

    /// DATA-INGEST-DAEMON-PROVIDER-JOBS-01: Test helper — inject a fake provider client.
    ///
    /// Allows scenario tests to verify the real sync-provider job path without
    /// making real TwelveData HTTP calls or requiring credentials.
    pub fn set_provider_client_for_test(&mut self, client: Arc<dyn mqk_md::HistoricalProvider>) {
        self.provider_client = Some(client);
    }

    /// Test helper: inject a capability-aware latest-bar provider for poll-once.
    pub fn set_latest_bar_provider_client_for_test(
        &mut self,
        client: Arc<dyn mqk_md::MarketDataProvider>,
    ) {
        self.latest_bar_provider_client = Some(client);
    }

    /// Test helper: inject a synchronization barrier that pauses
    /// `run_real_provider_sync`'s per-symbol progress write immediately
    /// before its durable persist call, so a test can deterministically
    /// interleave a concurrent cancel's own DB commit ahead of that write.
    pub fn set_ingest_job_persist_barrier_for_test(&mut self, barrier: Arc<tokio::sync::Notify>) {
        self.ingest_job_persist_barrier_for_test = Some(barrier);
    }

    /// Test helper: pair with `set_ingest_job_persist_barrier_for_test` so a
    /// test can await "the background write is now paused" deterministically.
    pub fn set_ingest_job_persist_barrier_entered_for_test(
        &mut self,
        entered: Arc<tokio::sync::Notify>,
    ) {
        self.ingest_job_persist_barrier_entered_for_test = Some(entered);
    }

    /// Test helper: inject a synchronization barrier that pauses the
    /// execution loop's pre-tick deadman-halt branch immediately after the
    /// durable `HALTED` + `DISARMED` commit, before any local integrity
    /// mutation, alert, or leadership release — see
    /// `deadman_local_quiescence_pause_for_test` doc comment.
    pub fn set_deadman_local_quiescence_pause_for_test(
        &mut self,
        barrier: Arc<tokio::sync::Notify>,
    ) {
        self.deadman_local_quiescence_pause_for_test = Some(barrier);
    }

    /// Test helper: pair with `set_deadman_local_quiescence_pause_for_test`
    /// so a test can await "the loop is now paused post-halt-commit"
    /// deterministically.
    pub fn set_deadman_local_quiescence_pause_entered_for_test(
        &mut self,
        entered: Arc<tokio::sync::Notify>,
    ) {
        self.deadman_local_quiescence_pause_entered_for_test = Some(entered);
    }

    /// Test helper: inject a fill activity fetcher for BROKER-FILL-REST-RECOVERY-01 tests.
    ///
    /// Production wiring is deferred to BROKER-FILL-REST-RECOVERY-APPLY-01.
    pub fn set_fill_activity_fetcher_for_test(
        &mut self,
        fetcher: Arc<dyn BrokerFillActivityFetcher>,
    ) {
        self.fill_activity_fetcher = Some(fetcher);
    }

    /// BRK-GAP-REST-RECOVERY-01: Test helper — inject an account-wide gap fill fetcher.
    ///
    /// Allows scenario tests to inject a fake `WsGapFillFetcher` without real network
    /// calls.  Production wiring is a follow-up once the service function is proven.
    pub fn set_ws_gap_fill_fetcher_for_test(&mut self, fetcher: Arc<dyn WsGapFillFetcher>) {
        self.ws_gap_fill_fetcher = Some(fetcher);
    }

    /// BROKER-SNAPSHOT-REFRESH-FOR-BASELINE-01: Test helper — inject an on-demand
    /// broker snapshot fetcher.
    ///
    /// Allows scenario tests to inject a fake `BrokerSnapshotFetcher` without making
    /// real Alpaca HTTP calls.  Production wiring is via `build_snapshot_fetcher_from_env`.
    pub fn set_snapshot_fetcher_for_test(&mut self, fetcher: Arc<dyn BrokerSnapshotFetcher>) {
        self.snapshot_fetcher = Some(fetcher);
    }

    /// Test helper: inject a read-only asset shortability preflight fetcher.
    pub fn set_asset_shortable_preflight_fetcher_for_test(
        &mut self,
        fetcher: Arc<dyn BrokerAssetShortablePreflightFetcher>,
    ) {
        self.asset_shortable_preflight_fetcher = Some(fetcher);
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

    /// BROKER-POSITION-BASELINE-ADOPTION-01: Seed the in-memory broker baseline
    /// cache from the persisted `sys_broker_position_baseline` row at daemon boot.
    ///
    /// Must be called after `new_with_db` and before `spawn_reconcile_tick` so
    /// the reconcile tick's `local_fn` sees the adopted baseline immediately on
    /// the first tick.  No-ops when DB is absent or no baseline has been adopted.
    pub async fn seed_broker_baseline_from_db(&self) {
        let Some(pool) = self.db.as_ref() else {
            return;
        };
        let row = match mqk_db::load_broker_position_baseline(pool).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "BROKER-BASELINE-01: failed to load broker position baseline at boot; \
                     leaving baseline empty"
                );
                return;
            }
        };
        let Some(row) = row else {
            return;
        };
        // Deserialize the stored BrokerSnapshot JSON back to the schema type then
        // build the reconcile LocalSnapshot (same conversion the reconcile tick uses).
        let schema_snap: mqk_schemas::BrokerSnapshot =
            match serde_json::from_value(row.broker_snapshot_json.clone()) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "BROKER-BASELINE-01: stored broker snapshot JSON is malformed; \
                         ignoring stale baseline"
                    );
                    return;
                }
            };
        let broker_reconcile = match reconcile_broker_snapshot_from_schema(&schema_snap) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "BROKER-BASELINE-01: stored broker snapshot failed schema conversion; \
                     ignoring stale baseline"
                );
                return;
            }
        };
        let local_baseline = mqk_reconcile::LocalSnapshot {
            orders: broker_reconcile.orders,
            positions: broker_reconcile.positions,
        };
        tracing::info!(
            adopted_at = %row.adopted_at_utc,
            adopted_by = %row.adopted_by,
            "BROKER-BASELINE-01: seeded broker position baseline from DB"
        );
        *self.broker_baseline.write().await = Some(local_baseline);
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

        // BROKER-FILL-REST-PRODUCTION-WIRING-01: computed before moving runtime_selection
        // into the struct literal below.
        let fill_activity_fetcher = build_fill_activity_fetcher_from_env(
            runtime_selection.broker_kind,
            runtime_selection.deployment_mode,
        );

        // BRK-GAP-REST-RECOVERY-01: account-wide gap fill fetcher; mirrors
        // fill_activity_fetcher wiring.  None when Alpaca not configured or
        // credentials absent (fail-closed).
        let ws_gap_fill_fetcher = build_ws_gap_fill_fetcher_from_env(
            runtime_selection.broker_kind,
            runtime_selection.deployment_mode,
        );

        // BROKER-SNAPSHOT-REFRESH-FOR-BASELINE-01: on-demand snapshot fetcher for
        // the adopt-broker-position-baseline route when the cache is absent at idle.
        let snapshot_fetcher = build_snapshot_fetcher_from_env(
            runtime_selection.broker_kind,
            runtime_selection.deployment_mode,
        );
        let asset_shortable_preflight_fetcher = build_asset_shortable_preflight_fetcher_from_env(
            runtime_selection.broker_kind,
            runtime_selection.deployment_mode,
        );

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
            per_symbol_target_state: Arc::new(RwLock::new(BTreeMap::new())),
            reconcile_status: Arc::new(RwLock::new(initial_reconcile_status())),
            operator_auth,
            runtime_selection,
            runtime_ownership: Arc::new(Mutex::new(LocalRuntimeOwnership::Idle)),
            lifecycle_op: Arc::new(Mutex::new(())),
            calendar_spec,
            broker_snapshot_source,
            strategy_market_data_source,
            alpaca_ws_continuity: Arc::new(RwLock::new(initial_ws_continuity)),
            session_clock_override: Arc::new(RwLock::new(None)),
            daily_data_readiness_clock_override: Arc::new(RwLock::new(None)),
            daily_data_readiness_evidence_override: Arc::new(RwLock::new(None)),
            daily_data_readiness_attempt_seq: Arc::new(AtomicU64::new(0)),
            gap_escalation_pending: Arc::new(AtomicBool::new(false)),
            strategy_fleet: Arc::new(RwLock::new(strategy_fleet)),
            discord_notifier: DiscordNotifier::from_env(),
            day_signal_count: Arc::new(AtomicU32::new(0)),
            day_signal_count_by_symbol: Arc::new(RwLock::new(HashMap::new())),
            per_symbol_day_order_limit: Arc::new(RwLock::new(
                signal_intake::per_symbol_day_order_count_limit_from_env(),
            )),
            per_symbol_max_position_qty: Arc::new(RwLock::new(
                signal_intake::per_symbol_max_position_qty_from_env(),
            )),
            max_new_orders_per_tick: Arc::new(RwLock::new(
                signal_intake::max_new_orders_per_tick_from_env(),
            )),
            per_symbol_bar_staleness_secs: Arc::new(RwLock::new(
                signal_intake::per_symbol_bar_staleness_secs_from_env(),
            )),
            accepted_artifact: Arc::new(RwLock::new(None)),
            autonomous_session_truth: Arc::new(RwLock::new(AutonomousSessionTruth::Clear)),
            autonomous_history_degraded: Arc::new(AtomicBool::new(false)),
            native_strategy_bootstrap: Arc::new(Mutex::new(None)),
            panic_on_symbol_for_test: Arc::new(Mutex::new(None)),
            dispatch_call_count_for_test: Arc::new(AtomicU32::new(0)),
            pending_strategy_bar_input: Arc::new(Mutex::new(None)),
            completed_bar_post_claim_test_hook: Arc::new(Mutex::new(None)),
            coverage_authority_pre_bind_test_hook: Arc::new(Mutex::new(None)),
            completed_bar_completion_fault_test_hook: Arc::new(AtomicBool::new(false)),
            completed_bar_task_clock_override: Arc::new(Mutex::new(None)),
            last_bar_input_ts: Arc::new(AtomicI64::new(0)),
            last_bar_signal_qty: Arc::new(AtomicI64::new(i64::MIN)),
            bar_tick_dispatch_count: Arc::new(AtomicU64::new(0)),
            last_bar_context_bars: Arc::new(AtomicI64::new(-1)),
            last_strategy_diagnostics: Arc::new(Mutex::new(None)),
            dry_run_diagnostics: Arc::new(RwLock::new(Vec::new())),
            dry_run_diagnostics_evaluated_at: Arc::new(AtomicI64::new(0)),
            external_snapshot_refresher: Arc::new(RwLock::new(None)),
            execution_last_tick_at: Arc::new(AtomicI64::new(0)),
            backtest_jobs: new_job_store(),
            ingest_jobs: new_ingest_job_store(),
            strategy_scan_jobs: new_strategy_scan_job_store(),
            strategy_scan_artifact_root: std::env::var("MQK_STRATEGY_SCAN_ARTIFACT_ROOT")
                .unwrap_or_else(|_| "exports/strategy_scans".to_string()),
            strategy_review_artifact_root: std::env::var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT")
                .unwrap_or_else(|_| "exports/strategy_reviews".to_string()),
            research_registry_db_path: std::env::var("MQK_RESEARCH_REGISTRY_DB").ok(),
            research_evidence_artifact_root: std::env::var("MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT")
                .ok(),
            research_min_deflated_sharpe_ratio: std::env::var(
                "MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO",
            )
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok()),
            research_max_probability_backtest_overfitting: std::env::var(
                "MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING",
            )
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok()),
            backtest_evidence_artifact_root: std::env::var("MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT")
                .ok(),
            promotion_min_sharpe: std::env::var("MQK_PROMOTION_MIN_SHARPE")
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok()),
            promotion_max_mdd: std::env::var("MQK_PROMOTION_MAX_MDD")
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok()),
            promotion_min_cagr: std::env::var("MQK_PROMOTION_MIN_CAGR")
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok()),
            promotion_min_profit_factor: std::env::var("MQK_PROMOTION_MIN_PROFIT_FACTOR")
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok()),
            promotion_min_profitable_months_pct: std::env::var(
                "MQK_PROMOTION_MIN_PROFITABLE_MONTHS_PCT",
            )
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok()),
            instrument_registry_path: std::env::var("MQK_INSTRUMENT_REGISTRY_PATH")
                .unwrap_or_else(|_| "config/instruments/equities.json".to_string()),
            instrument_registry_v2_path: std::env::var("MQK_INSTRUMENT_REGISTRY_V2_PATH").ok(),
            portfolio_economics_registry_v2_path: std::env::var(
                "MQK_PORTFOLIO_ECONOMICS_REGISTRY_V2_PATH",
            )
            .ok(),
            provider_registry_path: std::env::var("MQK_PROVIDER_REGISTRY_PATH")
                .unwrap_or_else(|_| "config/providers/providers.json".to_string()),
            md_refresh_evidence_dir: std::env::var("MQK_MD_REFRESH_EVIDENCE_DIR")
                .unwrap_or_else(|_| "exports/market_data".to_string()),
            kraken_scheduler_policy_path: std::env::var("MQK_KRAKEN_SCHEDULER_POLICY_PATH")
                .unwrap_or_else(|_| {
                    "docs/specs/crypto_data_02a_kraken_scheduler_rate_limit_decision.json"
                        .to_string()
                }),
            provider_client: None,
            latest_bar_provider_client: None,
            ingest_job_persist_barrier_for_test: None,
            ingest_job_persist_barrier_entered_for_test: None,
            deadman_local_quiescence_pause_for_test: None,
            deadman_local_quiescence_pause_entered_for_test: None,
            force_activity_counts_database_unavailable_for_test: false,
            market_data_feed_status: Arc::new(RwLock::new(None)),
            market_data_feed_scheduler: Arc::new(Mutex::new(
                MarketDataFeedSchedulerRuntimeState::default(),
            )),
            required_universe_scheduler: Arc::new(Mutex::new(
                required_market_data_autofresh::RequiredUniverseSchedulerRuntimeState::default(),
            )),
            fill_activity_fetcher,
            ws_gap_fill_fetcher,
            broker_baseline: Arc::new(RwLock::new(None)),
            snapshot_fetcher,
            asset_shortable_preflight_fetcher,
            b5_alerted_symbols: Arc::new(RwLock::new(HashSet::new())),
            day_limit_alert_fired: Arc::new(AtomicBool::new(false)),
            per_symbol_position_cap_alerted_symbols: Arc::new(RwLock::new(HashSet::new())),
            completed_bar_task:
                autonomous_completed_bar_task::AutonomousCompletedBarTaskRuntime::default(),
            dynamic_selection_fault_seam: Arc::new(RwLock::new(None)),
            hermetic_test_broker_override: Arc::new(RwLock::new(false)),
            force_install_active_runtime_conflict: Arc::new(AtomicBool::new(false)),
            force_leadership_release_failure: Arc::new(AtomicBool::new(false)),
            pre_barrier_leadership_release_count: Arc::new(AtomicU32::new(0)),
            force_execution_loop_panic: Arc::new(AtomicBool::new(false)),
            loop_call_trace: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 Blocker 3: append one
    /// event to the real-loop call trace. `#[cfg(test)]`-gated at every call
    /// site inline (never called unconditionally), so this never runs
    /// outside this crate's own test build.
    #[cfg(test)]
    pub(crate) fn loop_call_trace_push_for_test(&self, event: impl Into<String>) {
        self.loop_call_trace
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(event.into());
    }

    #[cfg(test)]
    pub(crate) fn loop_call_trace_snapshot_for_test(&self) -> Vec<String> {
        self.loop_call_trace
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    #[cfg(test)]
    pub(crate) fn loop_call_trace_clear_for_test(&self) {
        self.loop_call_trace
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
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

    pub async fn record_per_symbol_target_state(&self, mut state: PerSymbolTargetState) {
        let Some(key) = normalize_per_symbol_target_state_key(&state.symbol) else {
            return;
        };
        state.symbol = key.clone();
        self.per_symbol_target_state
            .write()
            .await
            .insert(key, state);
    }

    pub async fn per_symbol_target_states(&self) -> Vec<PerSymbolTargetState> {
        self.per_symbol_target_state
            .read()
            .await
            .values()
            .cloned()
            .collect()
    }

    pub async fn clear_per_symbol_target_states(&self) {
        self.per_symbol_target_state.write().await.clear();
    }

    pub async fn per_symbol_target_state_for_symbol(
        &self,
        symbol: &str,
    ) -> Option<PerSymbolTargetState> {
        let key = normalize_per_symbol_target_state_key(symbol)?;
        self.per_symbol_target_state.read().await.get(&key).cloned()
    }

    pub fn strategy_market_data_source(&self) -> StrategyMarketDataSource {
        self.strategy_market_data_source
    }

    /// HEARTBEAT-TICK-01: Unix-second timestamp of the last completed execution-loop tick.
    ///
    /// Returns 0 when no tick has completed since daemon boot.  Callers check
    /// `== 0` to distinguish "never ticked" from a stale timestamp.
    pub fn execution_last_tick_secs(&self) -> i64 {
        self.execution_last_tick_at.load(Ordering::SeqCst)
    }

    /// HEARTBEAT-TICK-01: Record a completed execution-loop tick at `now_secs`.
    ///
    /// Called by `loop_runner` once per tick after orchestrator progress and
    /// snapshot commit succeed.  Not called on early-exit paths (deadman halt,
    /// WS gap halt, orchestrator error, heartbeat failure).
    pub(crate) fn record_execution_tick(&self, now_secs: i64) {
        self.execution_last_tick_at
            .store(now_secs, Ordering::SeqCst);
    }

    pub async fn alpaca_ws_continuity(&self) -> AlpacaWsContinuityState {
        self.alpaca_ws_continuity.read().await.clone()
    }

    /// The autonomous-session truth every operator read surface consumes.
    ///
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3-SUPERVISOR-AND-CRITICAL-OUTCOME-
    /// CLOSURE-01 (REPAIR 5): a permanently failed completed-bar task must
    /// remain operator-visible even while the still-running session
    /// controller keeps projecting its own per-tick outcomes onto the
    /// stored truth. When the task's process-local liveness is `Failed`,
    /// this getter returns `CompletedBarDriverExited` regardless of what
    /// the stored value currently says — `Running`/`Started`/
    /// `WaitingForPreopen` projections cannot hide it. The overlay clears
    /// itself only when a later explicitly successful spawn's worker is
    /// actually running again (the supervisor core sets liveness `Running`
    /// per generation); expected shutdown (`Stopped`) never triggers it.
    /// Writers and dedup logic use the stored value directly
    /// ([`Self::stored_autonomous_session_truth`]) — the overlay is
    /// read-surface truth, never lifecycle authority.
    pub async fn autonomous_session_truth(&self) -> AutonomousSessionTruth {
        let task_liveness = self.completed_bar_task.truth.read().await.liveness;
        if task_liveness
            == autonomous_completed_bar_driver::AutonomousCompletedBarDriverTaskLiveness::Failed
        {
            let stored = self.autonomous_session_truth.read().await.clone();
            if matches!(
                stored,
                AutonomousSessionTruth::CompletedBarDriverExited { .. }
            ) {
                return stored;
            }
            return AutonomousSessionTruth::CompletedBarDriverExited {
                detail: "completed-bar driver task permanently failed (restart budget \
                         exhausted or supervisor panic); unattended completed-bar dispatch \
                         is UNMANAGED"
                    .to_string(),
            };
        }
        self.autonomous_session_truth.read().await.clone()
    }

    /// The raw stored autonomous-session truth, without the REPAIR 5
    /// failed-task overlay. For writers/dedup paths that must observe what
    /// is actually stored (e.g. the session controller's
    /// WsGapPartialRecovery-preserving clear), never for operator read
    /// surfaces.
    pub async fn stored_autonomous_session_truth(&self) -> AutonomousSessionTruth {
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
                // BRK-GAP-01: gap cursor → WsGapPartialRecovery (not RecoverySucceeded).
                // Non-fill lifecycle events from the gap window are permanently
                // unrecoverable from Alpaca REST.
                let repair_truth = if matches!(
                    prev_cursor.trade_updates,
                    mqk_broker_alpaca::types::AlpacaTradeUpdatesResume::GapDetected { .. }
                ) {
                    AutonomousSessionTruth::WsGapPartialRecovery {
                        resume_source: resume_source.clone(),
                        detail: "ws_gap_detected: WS connectivity re-established after gap; \
fill_recovery_available via REST catch-up from preserved cursor position; \
lifecycle_recovery_unproven: Ack/CancelAck/ReplaceAck/Reject events from the gap window \
are permanently unrecoverable via Alpaca REST; \
operator_reconcile_or_repair_required"
                            .to_string(),
                    }
                } else {
                    AutonomousSessionTruth::RecoverySucceeded {
                        resume_source: resume_source.clone(),
                        detail: match resume_source {
                            AutonomousRecoveryResumeSource::PersistedCursor => {
                                "WS continuity restored from persisted broker cursor truth"
                                    .to_string()
                            }
                            AutonomousRecoveryResumeSource::ColdStart => {
                                "WS continuity established from cold-start cursor truth".to_string()
                            }
                        },
                    }
                };
                self.set_autonomous_session_truth(repair_truth).await;
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
            RuntimeRiskGate::from_run_config(&serde_json::json!({}), 1_000_000_000_i64);
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
        // No dispatch assignments needed — this proof only exercises the
        // gap-detection exit path, not per-symbol dispatch.
        // Test-only direct spawn: release the startup barrier immediately
        // so this proof's single tick runs without needing the full
        // reserve/prepare-metadata/install sequence.
        let (barrier_tx, barrier_rx) = tokio::sync::oneshot::channel();
        let _ = barrier_tx.send(());
        let handle = loop_runner::spawn_execution_loop(
            Arc::clone(self),
            orchestrator,
            run_id,
            crate::dynamic_selection_dispatch_authority::RuntimeStrategyDispatchAuthority::Legacy {
                assignments: Vec::new(),
            },
            barrier_rx,
        );

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

    /// DAILY-DATA-READINESS-01C-ENFORCEMENT-01 test seam: read the current
    /// clock the strict readiness start gate will use — the override if set,
    /// else `Utc::now()`. Named `_for_test`-adjacent (read-only) so
    /// production code can share the exact same resolution logic as tests
    /// that set the override.
    pub async fn daily_data_readiness_now(&self) -> DateTime<Utc> {
        match *self.daily_data_readiness_clock_override.read().await {
            Some(ts) => ts,
            None => Utc::now(),
        }
    }

    /// Test seam: inject a fixed instant for the strict daily-data readiness
    /// start gate, so a `ready` verdict can be proven deterministically
    /// against seeded `md_bars` fixtures without racing the real wall clock.
    /// `None` restores the production `Utc::now()` behavior. Never called in
    /// production code.
    pub async fn set_daily_data_readiness_clock_override_for_test(
        &self,
        ts: Option<DateTime<Utc>>,
    ) {
        *self.daily_data_readiness_clock_override.write().await = ts;
    }

    /// Test seam: read the current forced evidence-persist override, if any.
    pub async fn daily_data_readiness_evidence_override(&self) -> Option<bool> {
        *self.daily_data_readiness_evidence_override.read().await
    }

    /// Test seam: force the strict readiness start gate's pre-start evidence
    /// persist outcome to `Some(true)`/`Some(false)` regardless of the real
    /// DB write result — proves the §C.9 evidence-failure policy without
    /// needing to break a live DB connection mid-test. `None` restores the
    /// real write-result behavior. Never called in production code.
    pub async fn set_daily_data_readiness_evidence_override_for_test(&self, forced: Option<bool>) {
        *self.daily_data_readiness_evidence_override.write().await = forced;
    }

    /// REPAIR 1 (DAILY-DATA-READINESS-01C-CLOSURE-REPAIR-01): allocate the
    /// next process-local monotonic start-attempt sequence number. Called
    /// exactly once per actual strict daily-data-readiness start-gate
    /// evaluation (never for a configuration-preview GET evaluation) so
    /// `compute_evaluation_id` can distinguish otherwise-identical attempts.
    pub(crate) fn next_daily_data_readiness_attempt_seq(&self) -> u64 {
        self.daily_data_readiness_attempt_seq
            .fetch_add(1, Ordering::SeqCst)
    }

    pub async fn strategy_fleet_snapshot(&self) -> Option<Vec<StrategyFleetEntry>> {
        self.strategy_fleet.read().await.clone()
    }

    pub async fn set_strategy_fleet_for_test(&self, fleet: Option<Vec<StrategyFleetEntry>>) {
        *self.strategy_fleet.write().await = fleet;
    }

    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-PREPARE-VS-DISPATCH-MODE-01
    /// REPAIR 4: production-safe, read-only runtime-dispatch eligibility
    /// seam for `RunningDispatch`-mode driver ticks. Performs no runtime
    /// start/stop, no bootstrap creation, no mutation of pending bars, no
    /// re-bootstrap, and no provider/broker call — it only reports the
    /// actual locally owned run ID (via [`Self::locally_owned_run_id`]) and
    /// the current native-strategy bootstrap state, without exposing or
    /// cloning plugin internals.
    ///
    /// Distinct from [`Self::native_strategy_bootstrap_truth_state_for_test`],
    /// which remains test-only and is never called from this seam or from
    /// any other production path.
    pub async fn autonomous_strategy_dispatch_runtime_truth(
        &self,
    ) -> autonomous_completed_bar_driver::AutonomousStrategyDispatchRuntimeTruth {
        use autonomous_completed_bar_driver::AutonomousStrategyDispatchRuntimeTruth as Truth;
        let Some(run_id) = self.locally_owned_run_id().await else {
            return Truth::NoLocallyOwnedRun;
        };
        let bootstrap = self.native_strategy_bootstrap.lock().await;
        match bootstrap.as_ref() {
            None => Truth::NativeStrategyBootstrapMissing,
            Some(b) if b.is_dormant() => Truth::NativeStrategyBootstrapDormant,
            Some(b) if b.is_failed() => Truth::NativeStrategyBootstrapFailed,
            Some(_) => Truth::Active { run_id },
        }
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

    /// AUTON-PAPER-BLOCKER-02 test seam: returns `true` when no bar input is pending.
    ///
    /// Named `_for_test` to signal intent; never called in production code.
    /// Used by autonomous bar ticker tests to verify skip conditions without
    /// consuming the pending input via `tick_strategy_dispatch`.
    pub async fn pending_strategy_bar_input_is_none_for_test(&self) -> bool {
        self.pending_strategy_bar_input.lock().await.is_none()
    }

    /// MULTI-SYMBOL-DISPATCH-LOOP-01 test seam: `pub` re-export of the
    /// crate-private [`Self::try_claim_b5_alert`] (`signal_intake.rs`) for
    /// the M08 per-symbol B5 alert-dedup proof.
    ///
    /// Named `_for_test` to signal intent; never called in production code.
    pub async fn try_claim_b5_alert_for_test(&self, symbol: &str) -> bool {
        self.try_claim_b5_alert(symbol).await
    }

    /// MULTI-SYMBOL-CAPITAL-CAPS-01 test seam: `pub` re-export of the
    /// crate-private [`Self::try_claim_per_symbol_position_cap_alert`]
    /// (`signal_intake.rs`) for the cap #2 per-symbol-per-day alert-dedup
    /// proof.
    ///
    /// Named `_for_test` to signal intent; never called in production code.
    pub async fn try_claim_per_symbol_position_cap_alert_for_test(&self, symbol: &str) -> bool {
        self.try_claim_per_symbol_position_cap_alert(symbol).await
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

    /// D4.2: record `last_bar_input_ts` telemetry for a claim dispatched
    /// through the exact-input seam (`dispatch_native_strategy_for_symbol_with_bar`
    /// called directly, bypassing `pending_strategy_bar_input`). Mirrors the
    /// telemetry side effect of `deposit_strategy_bar_input` without
    /// touching the mailbox itself.
    pub(crate) fn record_exact_bar_input_ts(&self, end_ts: i64) {
        self.last_bar_input_ts.store(end_ts, Ordering::SeqCst);
    }

    /// A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01: install (or clear) the
    /// per-symbol dispatch panic-injection seam. Test seam only; never
    /// called in production. See `panic_on_symbol_for_test`'s field doc.
    pub async fn set_panic_on_symbol_for_test(&self, symbol: Option<String>) {
        *self.panic_on_symbol_for_test.lock().await = symbol;
    }

    /// A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01: total calls into the
    /// canonical per-symbol dispatch implementation this process lifetime
    /// (including calls that panicked). Test seam only.
    pub fn dispatch_call_count_for_test(&self) -> u32 {
        self.dispatch_call_count_for_test.load(Ordering::SeqCst)
    }

    /// D4.4: install (or clear) the completed-bar driver's post-claim
    /// concurrency-proof rendezvous hook. Test seam only; never called in
    /// production.
    pub async fn set_completed_bar_post_claim_test_hook_for_test(
        &self,
        hook: Option<Arc<autonomous_completed_bar_driver::AutonomousCompletedBarPostClaimTestHook>>,
    ) {
        *self.completed_bar_post_claim_test_hook.lock().await = hook;
    }

    /// D4.4: read the currently installed post-claim rendezvous hook, if
    /// any. Called on every `RunningDispatch` claim; `None` in production
    /// and in every test that has not explicitly installed one.
    pub(crate) async fn completed_bar_post_claim_test_hook_for_test(
        &self,
    ) -> Option<Arc<autonomous_completed_bar_driver::AutonomousCompletedBarPostClaimTestHook>> {
        self.completed_bar_post_claim_test_hook.lock().await.clone()
    }

    /// E2A closure REPAIR 6: install (or clear) the coordinator's post-
    /// `create_or_recover`/pre-coverage-authority concurrency-proof
    /// rendezvous hook. Test seam only; never called in production.
    pub async fn set_coverage_authority_pre_bind_test_hook_for_test(
        &self,
        hook: Option<Arc<autonomous_daily_coordinator::AutonomousCoverageAuthorityPreBindTestHook>>,
    ) {
        *self.coverage_authority_pre_bind_test_hook.lock().await = hook;
    }

    /// E2A closure REPAIR 6: read the currently installed pre-bind
    /// rendezvous hook, if any. Called once per coordinator tick,
    /// immediately after `create_or_recover` commits the operation row;
    /// `None` in production and in every test that has not explicitly
    /// installed one.
    pub(crate) async fn coverage_authority_pre_bind_test_hook_for_test(
        &self,
    ) -> Option<Arc<autonomous_daily_coordinator::AutonomousCoverageAuthorityPreBindTestHook>> {
        self.coverage_authority_pre_bind_test_hook
            .lock()
            .await
            .clone()
    }

    /// D4 REPAIR 4: install (or clear) the completion-store-fault test seam.
    /// Test seam only; never called in production.
    pub async fn set_completed_bar_completion_fault_for_test(&self, inject: bool) {
        self.completed_bar_completion_fault_test_hook
            .store(inject, Ordering::SeqCst);
    }

    /// D4 REPAIR 4: whether the completion-store-fault test seam is
    /// currently installed. `false` in production.
    pub(crate) fn completed_bar_completion_fault_injected_for_test(&self) -> bool {
        self.completed_bar_completion_fault_test_hook
            .load(Ordering::SeqCst)
    }

    /// D4 REPAIR 7: install (or clear) the supervised completed-bar task's
    /// clock override. Test seam only; never called in production. `None`
    /// restores the production behavior of capturing `Utc::now()` once per
    /// task tick.
    pub async fn set_completed_bar_task_clock_override_for_test(&self, ts: Option<DateTime<Utc>>) {
        *self.completed_bar_task_clock_override.lock().await = ts;
    }

    /// D4 REPAIR 7: resolve the instant one supervised completed-bar task
    /// tick should use — the installed test override if present, otherwise
    /// `Utc::now()` captured here (production's unchanged behavior).
    pub(crate) async fn completed_bar_task_tick_clock(&self) -> DateTime<Utc> {
        match *self.completed_bar_task_clock_override.lock().await {
            Some(ts) => ts,
            None => Utc::now(),
        }
    }

    /// AUTON-NO-TRADE-01: Record the outcome of a bar tick dispatch.
    ///
    /// Called from the execution loop after `tick_strategy_dispatch` returns
    /// a bar result.  `signal_qty` is the sum of all target quantities the
    /// strategy returned; zero means "no trade signal this tick".
    pub(crate) fn record_bar_tick_outcome(&self, signal_qty: i64) {
        self.last_bar_signal_qty.store(signal_qty, Ordering::SeqCst);
        self.bar_tick_dispatch_count.fetch_add(1, Ordering::SeqCst);
    }

    /// AUTON-NO-TRADE-01: Sum of target quantities from the last bar dispatch.
    ///
    /// `None` when no bar has been dispatched this session (sentinel = i64::MIN).
    /// Zero means strategy returned no net-positive targets (hold/flat signal).
    pub fn last_bar_signal_qty(&self) -> Option<i64> {
        let v = self.last_bar_signal_qty.load(Ordering::SeqCst);
        if v == i64::MIN {
            None
        } else {
            Some(v)
        }
    }

    /// AUTON-NO-TRADE-01: Count of bar ticks dispatched to the native strategy this session.
    pub fn bar_tick_dispatch_count(&self) -> u64 {
        self.bar_tick_dispatch_count.load(Ordering::SeqCst)
    }

    /// AUTON-NO-TRADE-02: Test helper — seed bar-tick observability counters directly.
    ///
    /// Allows integration tests to simulate a post-arm bar-tick session without
    /// running the real execution loop.  Does NOT change any gate or trading
    /// behaviour; only the read-only observability counters are set.
    ///
    /// `ctx_bars`:  -1 = no dispatch yet (sentinel), 0 = stub_no_price, N>0 = db_loaded.
    pub fn set_bar_tick_state_for_test(&self, dispatch_count: u64, signal_qty: i64, ctx_bars: i64) {
        // Map dispatch_count=0 back to the "no dispatch" sentinel for signal_qty.
        let stored_qty = if dispatch_count == 0 {
            i64::MIN
        } else {
            signal_qty
        };
        self.bar_tick_dispatch_count
            .store(dispatch_count, Ordering::SeqCst);
        self.last_bar_signal_qty.store(stored_qty, Ordering::SeqCst);
        self.last_bar_context_bars.store(ctx_bars, Ordering::SeqCst);
    }

    /// AUTON-NO-TRADE-01: Reset bar-tick counters on new run start.
    pub(crate) fn reset_bar_tick_counters(&self) {
        self.last_bar_signal_qty.store(i64::MIN, Ordering::SeqCst);
        self.bar_tick_dispatch_count.store(0, Ordering::SeqCst);
        self.last_bar_context_bars.store(-1, Ordering::SeqCst);
    }

    /// AUTON-SIGNAL-CONTEXT-01: Number of DB bars used in the most recent dispatch.
    ///
    /// `-1` = no dispatch yet; `0` = stub fallback; `> 0` = DB bars loaded.
    pub fn last_bar_context_bars(&self) -> i64 {
        self.last_bar_context_bars.load(Ordering::SeqCst)
    }

    /// STRATEGY-DECISION-OBSERVABILITY-01: Clone the most recent strategy diagnostic
    /// snapshot, or `None` if no bar has been dispatched this session.
    pub async fn last_strategy_diagnostics(
        &self,
    ) -> Option<mqk_strategy::IntradayScalperDiagnostics> {
        self.last_strategy_diagnostics.lock().await.clone()
    }

    /// STRATEGY-DECISION-OBSERVABILITY-01: Inject a diagnostic snapshot for tests.
    ///
    /// Allows test scenarios to simulate a post-dispatch state without running
    /// the full execution loop.
    pub async fn set_strategy_diagnostics_for_test(
        &self,
        diag: mqk_strategy::IntradayScalperDiagnostics,
    ) {
        *self.last_strategy_diagnostics.lock().await = Some(diag);
    }

    /// MULTI-STRATEGY-DRY-RUN-STATUS-01: Clone the latest dry-run secondary-
    /// strategy diagnostic snapshot.
    ///
    /// Empty when `MQK_DRY_RUN_STRATEGY_IDS` is unset (default-off) or when
    /// no dry-run evaluation has been stored yet this process lifetime.
    pub async fn dry_run_diagnostics(&self) -> Vec<DryRunStrategyDiagnostic> {
        self.dry_run_diagnostics.read().await.clone()
    }

    /// MULTI-STRATEGY-DRY-RUN-STATUS-01: Unix-second timestamp of the most
    /// recent dry-run diagnostic snapshot write. `0` (sentinel) means no
    /// dry-run evaluation has been stored yet this process lifetime.
    pub fn dry_run_diagnostics_evaluated_at(&self) -> i64 {
        self.dry_run_diagnostics_evaluated_at.load(Ordering::SeqCst)
    }

    /// MULTI-STRATEGY-DRY-RUN-STATUS-01: Replace the latest dry-run
    /// diagnostic snapshot wholesale (never appended), and record the
    /// wall-clock time of this write.
    ///
    /// Called once per tick from the execution loop's dry-run evaluation
    /// step (`loop_runner.rs`), after `evaluate_dry_run_strategies` returns —
    /// never from any decision/submission path. Replacing rather than
    /// appending keeps storage bounded by the configured dry-run strategy
    /// count, not by tick count.
    pub(crate) async fn set_dry_run_diagnostics(
        &self,
        diagnostics: Vec<DryRunStrategyDiagnostic>,
        evaluated_at_ts: i64,
    ) {
        *self.dry_run_diagnostics.write().await = diagnostics;
        self.dry_run_diagnostics_evaluated_at
            .store(evaluated_at_ts, Ordering::SeqCst);
    }

    /// MULTI-STRATEGY-DRY-RUN-STATUS-01 test seam: directly set the dry-run
    /// diagnostic snapshot without running the execution loop.
    ///
    /// Named `_for_test` to signal intent; never called in production code.
    pub async fn set_dry_run_diagnostics_for_test(
        &self,
        diagnostics: Vec<DryRunStrategyDiagnostic>,
        evaluated_at_ts: i64,
    ) {
        self.set_dry_run_diagnostics(diagnostics, evaluated_at_ts)
            .await;
    }

    /// Thin wrapper over
    /// [`Self::dispatch_native_strategy_for_symbol_with_loaded_bars_and_facts`]
    /// that discards the exact evaluated-bar facts. Preserves the exact
    /// pre-repair signature/behavior for the existing single-symbol dispatch
    /// path and the `_for_test` seams — callers that need the bar facts
    /// (RUNTIME-OPPORTUNITY-ALLOCATION-01 authority repair, Phase A) use the
    /// `_and_facts` variant directly instead.
    async fn dispatch_native_strategy_for_symbol_with_loaded_bars(
        &self,
        symbol: &str,
        md_timeframe: &str,
        bar: StrategyBarInput,
        db_bars: Vec<mqk_db::MdBarRow>,
        now_ts: i64,
    ) -> Option<mqk_strategy::StrategyBarResult> {
        self.dispatch_native_strategy_for_symbol_with_loaded_bars_and_facts(
            symbol,
            md_timeframe,
            bar,
            db_bars,
            now_ts,
        )
        .await
        .map(|(result, _facts)| result)
    }

    /// PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE Part 4: the common
    /// bar-window preparation authority shared by both dispatch backends
    /// (legacy native bootstrap, selected-host). Contains exactly the
    /// fail-closed missing/stale-bar gates, diagnostics computation, and
    /// [`mqk_strategy::RecentBarsWindow`] construction previously inlined in
    /// [`Self::dispatch_native_strategy_for_symbol_with_loaded_bars_and_facts`]
    /// — moved verbatim, not reimplemented, so both backends see byte-
    /// identical staleness/diagnostic behavior. Neither backend performs a
    /// second DB bar-window load after this call.
    async fn prepare_bar_window_for_symbol_timeframe(
        &self,
        symbol: &str,
        md_timeframe: &str,
        bar: &StrategyBarInput,
        db_bars: Vec<mqk_db::MdBarRow>,
        now_ts: i64,
        signal_evaluation_authority: SignalEvaluationAuthority<'_>,
    ) -> BarWindowPrepOutcome {
        if db_bars.is_empty() {
            // No completed bars for this symbol/timeframe.
            self.last_bar_context_bars.store(0, Ordering::SeqCst);
            // MD-STALENESS-PER-TICK-GATE-01: a missing bar is always stale.
            // Fail-closed: refuse dispatch rather than falling through to the
            // single-stub fallback, which could otherwise produce a signal from
            // no real price history.
            let staleness_cap_secs = self
                .per_symbol_bar_staleness_secs_for_timeframe(md_timeframe)
                .await;
            let now_utc = crate::market_data_freshness::unix_ts_to_rfc3339(now_ts);
            let no_order_reason =
                if crate::market_data_freshness::is_intraday_timeframe(md_timeframe) {
                    crate::market_data_freshness::REASON_CODE_INTRADAY_BAR_NOT_CURRENT
                } else {
                    crate::market_data_freshness::REASON_CODE_MARKET_DATA_MISSING
                };
            tracing::warn!(
                symbol = %symbol,
                timeframe = %md_timeframe,
                latest_completed_bar_ts = ?Option::<String>::None,
                now_utc = ?now_utc,
                age_seconds = ?Option::<i64>::None,
                max_allowed_age_seconds = ?staleness_cap_secs,
                no_order_reason = no_order_reason,
                "md_staleness_per_tick_gate_01: bar_data_missing — \
                 NO_MARKET_DATA: no completed bars in md_bars for this \
                 symbol/timeframe; strategy dispatch refused for this \
                 symbol/tick (fail-closed, no target/intent/order)"
            );
            // AUTON-NO-SIGNAL-OBS-01: durably record that this tick's
            // dispatch was refused before the strategy ever ran, so the
            // operator can see this after a restart, not just in logs.
            self.record_signal_evaluation(
                signal_evaluation_authority,
                SignalEvaluationAttempt {
                    now_tick: bar.now_tick,
                    symbol,
                    timeframe: md_timeframe,
                    bar_context_source: "no_bars_available",
                    bars_loaded: 0,
                    latest_bar_ts_utc: None,
                    signal_generated: false,
                    signal_qty: None,
                    reason_code: no_order_reason,
                    reason: "no completed bars in md_bars for this symbol/timeframe",
                    decision_stage: "pre_dispatch_gate",
                },
            )
            .await;
            return BarWindowPrepOutcome::Refused;
        }

        // MD-STALENESS-PER-TICK-GATE-01: fail-closed per-dispatch-tick
        // staleness gate. `db_bars` is oldest-first, so the last element is the
        // latest completed bar.
        //
        // RUNTIME-OPPORTUNITY-ALLOCATION-01 authority repair (Phase A): this
        // is also the exact completed bar the strategy below is evaluated
        // against — `latest_bar_row` is captured once, here, and copied
        // verbatim into `EvaluatedBarFacts` by the caller. Nothing after this
        // point may substitute a different bar.
        let latest_bar_row = db_bars.last().cloned();
        let latest_end_ts = latest_bar_row.as_ref().map(|b| b.end_ts);
        let staleness_cap_secs = self
            .per_symbol_bar_staleness_secs_for_timeframe(md_timeframe)
            .await;
        if classify_bar_staleness(latest_end_ts, now_ts, staleness_cap_secs) == Some(true) {
            let age_seconds = latest_end_ts.map(|ts| (now_ts - ts).max(0));
            let now_utc = crate::market_data_freshness::unix_ts_to_rfc3339(now_ts);
            let latest_completed_bar_ts =
                latest_end_ts.and_then(crate::market_data_freshness::unix_ts_to_rfc3339);
            let no_order_reason =
                if crate::market_data_freshness::is_intraday_timeframe(md_timeframe) {
                    crate::market_data_freshness::REASON_CODE_INTRADAY_BAR_STALE
                } else {
                    crate::market_data_freshness::REASON_CODE_BAR_DATA_STALE
                };
            tracing::warn!(
                symbol = %symbol,
                timeframe = %md_timeframe,
                latest_end_ts = ?latest_end_ts,
                latest_completed_bar_ts = ?latest_completed_bar_ts,
                now_utc = ?now_utc,
                age_seconds = ?age_seconds,
                max_allowed_age_seconds = ?staleness_cap_secs,
                no_order_reason = no_order_reason,
                "md_staleness_per_tick_gate_01: bar_data_stale — latest \
                 completed bar exceeds the per-symbol staleness threshold; \
                 strategy dispatch refused for this symbol/tick \
                 (fail-closed, no target/intent/order)"
            );
            // AUTON-NO-SIGNAL-OBS-01: durably record that this tick's
            // dispatch was refused before the strategy ever ran (stale bar),
            // so the operator can see this after a restart, not just in logs.
            self.record_signal_evaluation(
                signal_evaluation_authority,
                SignalEvaluationAttempt {
                    now_tick: bar.now_tick,
                    symbol,
                    timeframe: md_timeframe,
                    bar_context_source: "stale_bars",
                    bars_loaded: db_bars.len() as i64,
                    latest_bar_ts_utc: latest_end_ts
                        .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0)),
                    signal_generated: false,
                    signal_qty: None,
                    reason_code: no_order_reason,
                    reason: "latest completed bar exceeds the per-symbol staleness threshold",
                    decision_stage: "pre_dispatch_gate",
                },
            )
            .await;
            return BarWindowPrepOutcome::Refused;
        }

        let bars_loaded = db_bars.len();
        let stubs: Vec<mqk_strategy::BarStub> = db_bars
            .iter()
            .map(|b| mqk_strategy::BarStub::new(b.end_ts, b.is_complete, b.close_micros, b.volume))
            .collect();
        // STRATEGY-DECISION-OBSERVABILITY-01: compute diagnostic snapshot from
        // the bar window before consuming stubs into the strategy context.
        let diagnostics = mqk_strategy::intraday_scalper_compute_diagnostics(&stubs);
        // Copy out the two `&'static str` fields before `diagnostics` is moved
        // into `last_strategy_diagnostics` below — AUTON-NO-SIGNAL-OBS-01 reuses
        // this same already-live diagnostic text for the durable journal row.
        let diagnostic_decision = diagnostics.decision;
        let diagnostic_reason = diagnostics.reason;
        *self.last_strategy_diagnostics.lock().await = Some(diagnostics);
        let window = mqk_strategy::RecentBarsWindow::new(bars_loaded.max(1), stubs);
        self.last_bar_context_bars
            .store(bars_loaded as i64, Ordering::SeqCst);
        tracing::debug!(
            symbol = %symbol,
            timeframe = %md_timeframe,
            bars_loaded,
            "auton_signal_context_01: db_bar_window_built"
        );
        BarWindowPrepOutcome::Ready {
            window,
            latest_bar_row: latest_bar_row
                .expect("latest_bar_row is Some whenever db_bars was non-empty (checked above)"),
            bars_loaded,
            diagnostic_decision,
            diagnostic_reason,
        }
    }

    /// RUNTIME-OPPORTUNITY-ALLOCATION-01 authority repair (Phase A): the one
    /// canonical DB-backed dispatch implementation. Returns the
    /// [`EvaluatedBarFacts`] captured from the exact `db_bars` window the
    /// strategy was evaluated against, alongside the result — never a second,
    /// independently-derived bar lookup. `Some` only when the strategy
    /// actually ran against a real (non-empty, non-stale) completed-bar
    /// window; every early fail-closed return (missing/stale bars) yields
    /// `None`, matching the pre-repair `dispatch_native_strategy_for_symbol_with_loaded_bars`
    /// behavior exactly.
    ///
    /// PHASE-7B: this is the `Legacy` dispatch backend. Delegates the
    /// common bar-window preparation to
    /// [`Self::prepare_bar_window_for_symbol_timeframe`] — behavior is
    /// byte-identical to the pre-Phase-7B implementation.
    async fn dispatch_native_strategy_for_symbol_with_loaded_bars_and_facts(
        &self,
        symbol: &str,
        md_timeframe: &str,
        bar: StrategyBarInput,
        db_bars: Vec<mqk_db::MdBarRow>,
        now_ts: i64,
    ) -> Option<(mqk_strategy::StrategyBarResult, EvaluatedBarFacts)> {
        let (window, latest_bar_row, bars_loaded, diagnostic_decision, diagnostic_reason) =
            match self
                .prepare_bar_window_for_symbol_timeframe(
                    symbol,
                    md_timeframe,
                    &bar,
                    db_bars,
                    now_ts,
                    SignalEvaluationAuthority::Legacy,
                )
                .await
            {
                BarWindowPrepOutcome::Refused => return None,
                BarWindowPrepOutcome::Ready {
                    window,
                    latest_bar_row,
                    bars_loaded,
                    diagnostic_decision,
                    diagnostic_reason,
                } => (
                    window,
                    latest_bar_row,
                    bars_loaded,
                    diagnostic_decision,
                    diagnostic_reason,
                ),
            };
        // TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 Blocker 3: this is the
        // one real legacy native-bootstrap invocation call site (the
        // `Legacy`/`Off`/`Shadow` dispatch backend). A `DynamicPaperEnforced`
        // tick must never reach here.
        #[cfg(test)]
        self.loop_call_trace_push_for_test(format!("legacy_dispatch:{symbol}"));
        let now_tick = bar.now_tick;
        let result = match self
            .invoke_native_strategy_host_on_bar(move |bootstrap| {
                bootstrap.invoke_on_bar_from_window(now_tick, window)
            })
            .await
        {
            Ok(result) => result,
            Err(fault) => {
                let run_id = self.status.read().await.active_run_id;
                self.record_dispatch_panic_fault(
                    run_id,
                    &fault.strategy_id,
                    symbol,
                    md_timeframe,
                    now_tick,
                    &fault.detail,
                )
                .await;
                None
            }
        };
        // AUTON-NO-SIGNAL-OBS-01: the strategy's on_bar ran to completion —
        // durably record what it actually produced, whether or not that was a
        // trade signal. Skipped only when the bootstrap itself is missing
        // (Dormant/Failed/spec-read error), since there is no strategy_id to
        // attribute the row to in that case.
        if let Some(ref bar_result) = result {
            let signal_qty: i64 = bar_result
                .intents
                .output
                .targets
                .iter()
                .map(|t| t.qty)
                .sum();
            self.record_signal_evaluation(
                SignalEvaluationAuthority::Legacy,
                SignalEvaluationAttempt {
                    now_tick: bar.now_tick,
                    symbol,
                    timeframe: md_timeframe,
                    bar_context_source: "db_loaded",
                    bars_loaded: bars_loaded as i64,
                    latest_bar_ts_utc: DateTime::<Utc>::from_timestamp(latest_bar_row.end_ts, 0),
                    signal_generated: signal_qty != 0,
                    signal_qty: Some(signal_qty),
                    reason_code: diagnostic_decision,
                    reason: diagnostic_reason,
                    decision_stage: "strategy_evaluated",
                },
            )
            .await;
        }
        // RUNTIME-OPPORTUNITY-ALLOCATION-01 authority repair (Phase A): pair
        // the result with the exact bar it was evaluated against —
        // `latest_bar_row` is the exact row `prepare_bar_window_for_symbol_
        // timeframe` captured from this same `db_bars` window.
        result.map(|bar_result| {
            let facts = EvaluatedBarFacts {
                symbol: symbol.to_string(),
                strategy_id: bar_result.spec.name.clone(),
                timeframe: md_timeframe.to_string(),
                bar_end_ts: latest_bar_row.end_ts,
                close_micros: latest_bar_row.close_micros,
            };
            (bar_result, facts)
        })
    }

    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4-EVALUATION-LINEAGE-AND-
    /// AUTONOMOUS-PREOPEN-CLOSURE-01 REPAIR 1: the one canonical deterministic
    /// strategy-evaluation identity derivation. Both the signal-evaluation
    /// journal writer ([`Self::record_signal_evaluation`]) and the
    /// completed-bar dispatch-claim path
    /// (`autonomous_completed_bar_driver::claim_and_dispatch_observed_bar`)
    /// must call this same helper — never a second, independently-derived
    /// identity algorithm. Preserves the exact pre-existing seed format:
    /// `mqk.signal-evaluation.v1|{run_id-or-none}|{strategy_id}|{symbol}|{timeframe}|{now_tick}`.
    pub(crate) fn derive_strategy_signal_evaluation_id(
        run_id: Option<Uuid>,
        strategy_id: &str,
        symbol: &str,
        timeframe: &str,
        now_tick: u64,
    ) -> Uuid {
        let run_id_key = run_id
            .map(|u| u.to_string())
            .unwrap_or_else(|| "none".to_string());
        Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            format!(
                "mqk.signal-evaluation.v1|{run_id_key}|{strategy_id}|{symbol}|{timeframe}|{now_tick}"
            )
            .as_bytes(),
        )
    }

    /// AUTON-NO-SIGNAL-OBS-01: one durable signal-evaluation journal write
    /// attempt, scoped to a single symbol/timeframe/tick.
    ///
    /// All `&str` fields borrow from the caller's locals for the duration of
    /// the call only — `record_signal_evaluation` copies what it needs into
    /// owned `String`s before returning.
    async fn record_signal_evaluation(
        &self,
        authority: SignalEvaluationAuthority<'_>,
        attempt: SignalEvaluationAttempt<'_>,
    ) {
        let Some(ref pool) = self.db else {
            return;
        };
        // Blocker 2: authority is explicit, never rediscovered here. Legacy
        // resolves the active native bootstrap/status exactly as before;
        // Explicit (selected-host/PaperEnforced) uses only the caller's own
        // frozen run_id/strategy_id — never `native_strategy_bootstrap` or
        // `status.active_run_id`, which may name a different run or a
        // different symbol's selected strategy entirely.
        let (run_id, strategy_id) = match authority {
            SignalEvaluationAuthority::Legacy => {
                // No strategy_id to attribute this row to when the bootstrap
                // is Dormant/Failed — see the AUTON-NO-SIGNAL-OBS-01 callers
                // above.
                let Some(strategy_id) = self
                    .native_strategy_bootstrap
                    .lock()
                    .await
                    .as_ref()
                    .and_then(|b| b.active_strategy_id().map(|s| s.to_string()))
                else {
                    return;
                };
                let run_id = self.status.read().await.active_run_id;
                (run_id, strategy_id)
            }
            SignalEvaluationAuthority::Explicit {
                run_id,
                strategy_id,
            } => (Some(run_id), strategy_id.to_string()),
        };
        let signal_side = match attempt.signal_qty {
            Some(q) if q > 0 => Some("buy".to_string()),
            Some(q) if q < 0 => Some("sell".to_string()),
            _ => None,
        };
        // AUDIT-EVENT-DETERMINISM: deterministic UUIDv5, never Uuid::new_v4(),
        // so a duplicate write attempt for the same logical tick is a no-op
        // (ON CONFLICT DO NOTHING) rather than a second row. D4 REPAIR 1: the
        // one canonical identity helper, shared with the completed-bar
        // dispatch-claim path.
        let evaluation_id = Self::derive_strategy_signal_evaluation_id(
            run_id,
            &strategy_id,
            attempt.symbol,
            attempt.timeframe,
            attempt.now_tick,
        );
        let args = mqk_db::InsertStrategySignalEvaluationArgs {
            evaluation_id,
            ts_utc: Utc::now(),
            run_id,
            strategy_id,
            symbol: attempt.symbol.to_string(),
            timeframe: attempt.timeframe.to_string(),
            bar_context_source: attempt.bar_context_source.to_string(),
            bars_loaded: attempt.bars_loaded,
            latest_bar_ts_utc: attempt.latest_bar_ts_utc,
            signal_generated: attempt.signal_generated,
            signal_qty: attempt.signal_qty,
            signal_side,
            reason_code: attempt.reason_code.to_string(),
            reason: attempt.reason.to_string(),
            decision_stage: attempt.decision_stage.to_string(),
            source: "mqk-daemon.execution_loop".to_string(),
        };
        // Best-effort: a telemetry write failure must never panic or block
        // the dispatch/decision path — mirrors the TV-EXEC-01 fill-quality
        // telemetry write pattern (mqk-runtime orchestrator).
        if let Err(e) = mqk_db::insert_strategy_signal_evaluation(pool, &args).await {
            tracing::warn!(
                symbol = %attempt.symbol,
                timeframe = %attempt.timeframe,
                error = %e,
                "auton_no_signal_obs_01: strategy_signal_evaluations write failed (non-fatal)"
            );
        }
    }

    /// A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01: explicit durable fault
    /// evidence for a symbol whose real `Strategy::on_bar` callback
    /// panicked. `run_id`/`strategy_id` must be the exact identity captured
    /// from the shared native bootstrap BEFORE
    /// [`Self::invoke_native_strategy_host_on_bar`] quarantined it to
    /// `Failed` — by the time this is called, the bootstrap's own
    /// `active_strategy_id()` already returns `None`, so this cannot reuse
    /// [`Self::record_signal_evaluation`]'s `Legacy` branch (which
    /// re-resolves identity from current bootstrap state at write time).
    /// Writes the same durable row shape directly instead, through the one
    /// canonical identity helper
    /// ([`Self::derive_strategy_signal_evaluation_id`]) every other
    /// signal-evaluation journal writer already shares.
    async fn record_dispatch_panic_fault(
        &self,
        run_id: Option<Uuid>,
        strategy_id: &str,
        symbol: &str,
        timeframe: &str,
        now_tick: u64,
        detail: &str,
    ) {
        tracing::error!(
            symbol = %symbol,
            timeframe = %timeframe,
            strategy_id = %strategy_id,
            panic = %detail,
            "a1_multi_symbol_dispatch_panic_isolation_01: real strategy on_bar panicked; the \
             shared native strategy host is quarantined (Failed) for the rest of this run -- no \
             decision emitted for this symbol, and no further symbol/tick can reuse the \
             possibly-corrupted host"
        );
        let Some(ref pool) = self.db else {
            return;
        };
        let evaluation_id = Self::derive_strategy_signal_evaluation_id(
            run_id, strategy_id, symbol, timeframe, now_tick,
        );
        let args = mqk_db::InsertStrategySignalEvaluationArgs {
            evaluation_id,
            ts_utc: Utc::now(),
            run_id,
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe: timeframe.to_string(),
            bar_context_source: "dispatch_panicked".to_string(),
            bars_loaded: 0,
            latest_bar_ts_utc: None,
            signal_generated: false,
            signal_qty: None,
            signal_side: None,
            reason_code: "strategy_dispatch_panicked".to_string(),
            reason: "strategy evaluation panicked for this symbol; host quarantined, no decision \
                     emitted"
                .to_string(),
            decision_stage: "strategy_evaluated".to_string(),
            source: "mqk-daemon.execution_loop".to_string(),
        };
        if let Err(e) = mqk_db::insert_strategy_signal_evaluation(pool, &args).await {
            tracing::warn!(
                symbol = %symbol,
                timeframe = %timeframe,
                error = %e,
                "a1_multi_symbol_dispatch_panic_isolation_01: strategy_signal_evaluations write \
                 failed (non-fatal)"
            );
        }
    }

    /// A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01: the one production seam
    /// through which the shared `native_strategy_bootstrap`'s `on_bar` is
    /// ever invoked, whichever of the two per-symbol dispatch paths
    /// (DB-loaded bar window, or the single-stub signal fallback) calls it.
    /// `on_bar` is synchronous, so a panic inside it is caught in-place
    /// here — narrowly around the `call_on_bar` closure only. DB bar-window
    /// loading, diagnostics, and durable-journal writes in the surrounding
    /// caller are NOT wrapped in any catch and still propagate a real
    /// unwind on a genuine infrastructure panic, exactly as before this
    /// patch.
    ///
    /// Tier A holds exactly one mutable `StrategyHost`/`Box<dyn Strategy>`
    /// for the whole run ([`NativeStrategyBootstrap`]'s single-strategy
    /// policy), shared across every symbol dispatched this tick and every
    /// future tick. `Strategy::on_bar` takes `&mut self`; a panic
    /// mid-callback does not roll back a mutation the strategy already
    /// made, and nothing here can prove what state it left behind. So a
    /// caught panic permanently quarantines the bootstrap to `Failed`: the
    /// exact same (possibly-corrupted) object can never be invoked again
    /// this run — not by this symbol's siblings later in this tick, not by
    /// any future tick — until the run is restarted and re-bootstrapped.
    /// This quarantine is what makes catching the panic here (rather than
    /// letting it unwind the whole tick, as before this repair) safe.
    async fn invoke_native_strategy_host_on_bar(
        &self,
        call_on_bar: impl FnOnce(&mut NativeStrategyBootstrap) -> Option<mqk_strategy::StrategyBarResult>,
    ) -> Result<Option<mqk_strategy::StrategyBarResult>, NativeStrategyOnBarPanicFault> {
        let mut guard = self.native_strategy_bootstrap.lock().await;
        let Some(bootstrap) = guard.as_mut() else {
            return Ok(None);
        };
        let strategy_id_before = bootstrap.active_strategy_id().unwrap_or_default().to_string();
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| call_on_bar(bootstrap)));
        match outcome {
            Ok(result) => Ok(result),
            Err(panic_payload) => {
                let detail = panic_payload_message(&*panic_payload);
                // Re-acquire from `guard` (never moved) rather than reusing
                // the `bootstrap` reborrow consumed by the closure above.
                if let Some(bootstrap) = guard.as_mut() {
                    bootstrap.outcome =
                        mqk_runtime::native_strategy::NativeStrategyBootstrapOutcome::Failed {
                            strategy_id: strategy_id_before.clone(),
                            reason: format!(
                                "a1_multi_symbol_dispatch_panic_isolation_01: on_bar panicked \
                                 and was quarantined: {detail}"
                            ),
                        };
                }
                Err(NativeStrategyOnBarPanicFault {
                    strategy_id: strategy_id_before,
                    detail,
                })
            }
        }
    }

    /// AUTON-NO-TRADE-OFFHOURS-01B: best-effort, non-fatal durable snapshot
    /// of one `GET /api/v1/autonomous/readiness` verdict.
    ///
    /// Mirrors [`Self::record_signal_evaluation`]'s pattern exactly: a
    /// telemetry write failure must never panic or affect the readiness
    /// response already being returned to the caller. `diagnostic_id` is
    /// deterministic (`Uuid::new_v5`, minute-bucketed on `observed_at_utc`)
    /// so repeated polls while the same reason holds are a DB no-op rather
    /// than unbounded row growth. `run_id` is honest — `None` means no
    /// active run, never a fabricated default. `paper_order_attempted` and
    /// `live_order_attempted` are always `false`: this method only ever
    /// records why an order was NOT attempted.
    pub async fn record_no_trade_diagnostic(&self, snapshot: NoTradeDiagnosticSnapshot<'_>) {
        let Some(ref pool) = self.db else {
            return;
        };
        let observed_at_utc = Utc::now();
        let minute_bucket = observed_at_utc.format("%Y%m%d%H%M").to_string();
        // AUDIT-EVENT-DETERMINISM: deterministic UUIDv5, never
        // `Uuid::new_v4()`, so a duplicate write attempt for the same
        // logical (reason_code, stage, observing minute) is a no-op
        // (ON CONFLICT DO NOTHING) rather than a second row.
        let diagnostic_id = Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            format!(
                "mqk.no-trade-diagnostic.v1|{}|{}|{minute_bucket}",
                snapshot.reason_code, snapshot.stage
            )
            .as_bytes(),
        );
        let args = mqk_db::InsertAutonomousNoTradeDiagnosticArgs {
            diagnostic_id,
            observed_at_utc,
            run_id: snapshot.run_id,
            mode: snapshot.mode.to_string(),
            session_window_state: snapshot.session_window_state.to_string(),
            runtime_start_allowed: snapshot.runtime_start_allowed,
            arm_state: snapshot.arm_state.to_string(),
            overall_ready: snapshot.overall_ready,
            reason_code: snapshot.reason_code.to_string(),
            reason: snapshot.reason.to_string(),
            stage: snapshot.stage.to_string(),
            paper_order_attempted: false,
            live_order_attempted: false,
            source: "mqk-daemon.autonomous_readiness_route".to_string(),
        };
        if let Err(e) = mqk_db::insert_autonomous_no_trade_diagnostic(pool, &args).await {
            tracing::warn!(
                reason_code = %snapshot.reason_code,
                error = %e,
                "auton_no_trade_offhours_01b: autonomous_no_trade_diagnostics write failed (non-fatal)"
            );
        }
    }

    /// PER-SYMBOL-BAR-WINDOW-01 / MULTI-SYMBOL-DISPATCH-LOOP-01:
    /// symbol/timeframe-parameterized dispatch of one already-taken
    /// [`StrategyBarInput`].
    ///
    /// `symbol`/`timeframe` are trimmed before use. Does not read or write
    /// `pending_strategy_bar_input` — callers take the pending bar input
    /// before calling this, either once for one symbol
    /// ([`Self::tick_strategy_dispatch_for_symbol`]) or once for many symbols
    /// ([`Self::tick_strategy_dispatch_multi_symbol`]).
    async fn dispatch_native_strategy_for_symbol_with_bar(
        &self,
        symbol: &str,
        timeframe: &str,
        bar: StrategyBarInput,
    ) -> Option<mqk_strategy::StrategyBarResult> {
        self.dispatch_native_strategy_for_symbol_with_bar_and_facts(symbol, timeframe, bar)
            .await
            .map(|(result, _facts)| result)
    }

    /// RUNTIME-OPPORTUNITY-ALLOCATION-01 authority repair (Phase A): the one
    /// canonical implementation [`Self::dispatch_native_strategy_for_symbol_with_bar`]
    /// delegates to. Returns `Some((_, Some(facts)))` only on the DB-backed
    /// completed-bar path; the single-stub fallback (no DB pool, unset
    /// symbol/timeframe, or a DB read failure) has no real completed bar to
    /// bind a price to, so it returns `Some((_, None))` — callers must treat
    /// `None` facts the same as a missing bar for allocation-sizing purposes
    /// (fail closed for new/increasing buys only; sells are unaffected).
    async fn dispatch_native_strategy_for_symbol_with_bar_and_facts(
        &self,
        symbol: &str,
        timeframe: &str,
        bar: StrategyBarInput,
    ) -> Option<(mqk_strategy::StrategyBarResult, Option<EvaluatedBarFacts>)> {
        // AUTON-SIGNAL-CONTEXT-01: attempt DB-backed context window.
        let symbol = symbol.trim();
        let md_timeframe = timeframe.trim();

        self.dispatch_call_count_for_test
            .fetch_add(1, Ordering::SeqCst);

        // A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01: test-only injection
        // point, permanently `None` in production. See
        // `panic_on_symbol_for_test`'s field doc.
        if self.panic_on_symbol_for_test.lock().await.as_deref() == Some(symbol) {
            panic!("A1_TEST_INJECTED_PANIC for symbol {symbol}");
        }

        if let (Some(ref pool), true) = (&self.db, !symbol.is_empty() && !md_timeframe.is_empty()) {
            match mqk_db::fetch_recent_completed_bars_for_strategy(
                pool,
                symbol,
                md_timeframe,
                STRATEGY_CONTEXT_LOAD_LIMIT,
            )
            .await
            {
                Ok(db_bars) => {
                    return self
                        .dispatch_native_strategy_for_symbol_with_loaded_bars_and_facts(
                            symbol,
                            md_timeframe,
                            bar,
                            db_bars,
                            Utc::now().timestamp(),
                        )
                        .await
                        .map(|(result, facts)| (result, Some(facts)));
                }
                Err(e) => {
                    self.last_bar_context_bars.store(0, Ordering::SeqCst);
                    tracing::warn!(
                        symbol = %symbol,
                        timeframe = %md_timeframe,
                        error = %e,
                        "auton_signal_context_01: db_bar_load_failed; falling back to stub"
                    );
                }
            }
        } else if symbol.is_empty() || md_timeframe.is_empty() {
            tracing::debug!(
                symbol_set = !symbol.is_empty(),
                timeframe_set = !md_timeframe.is_empty(),
                "auton_signal_context_01: STRATEGY_CONTEXT_DB_LOAD_MISSING — \
                 MQK_STRATEGY_SYMBOL or MQK_STRATEGY_MD_TIMEFRAME not set; \
                 falling back to stub context"
            );
            // Do not overwrite a previously recorded non-negative value.
            // Only update if still at sentinel (no dispatch yet this session).
            let cur = self.last_bar_context_bars.load(Ordering::SeqCst);
            if cur < 0 {
                self.last_bar_context_bars.store(0, Ordering::SeqCst);
            }
        }

        // Fallback: single-stub context (B1B legacy path).
        // limit_price=None → is_complete=false → strategies return signal=0.
        // Correct conservative behaviour when no DB context is available.
        //
        // STRATEGY-DECISION-OBSERVABILITY-01: store a stub diagnostic showing
        // insufficient_bars so operators can see why no signal fired on this path.
        {
            let stub_bars: &[mqk_strategy::BarStub] = &[];
            let diagnostics = mqk_strategy::intraday_scalper_compute_diagnostics(stub_bars);
            *self.last_strategy_diagnostics.lock().await = Some(diagnostics);
        }
        let now_tick = bar.now_tick;
        let end_ts = bar.end_ts;
        let limit_price = bar.limit_price;
        let qty = bar.qty;
        match self
            .invoke_native_strategy_host_on_bar(move |bootstrap| {
                bootstrap.invoke_on_bar_from_signal(now_tick, end_ts, limit_price, qty)
            })
            .await
        {
            Ok(result) => result.map(|result| (result, None)),
            Err(fault) => {
                let run_id = self.status.read().await.active_run_id;
                self.record_dispatch_panic_fault(
                    run_id,
                    &fault.strategy_id,
                    symbol,
                    md_timeframe,
                    now_tick,
                    &fault.detail,
                )
                .await;
                None
            }
        }
    }

    /// PER-SYMBOL-BAR-WINDOW-01: symbol/timeframe-parameterized extraction of
    /// the dispatch body previously inlined in [`Self::tick_strategy_dispatch`].
    ///
    /// Takes the single pending [`StrategyBarInput`] — fail-closed `None` if
    /// none is pending — and dispatches it for `symbol`/`timeframe` via
    /// [`Self::dispatch_native_strategy_for_symbol_with_bar`]. The legacy
    /// single-symbol caller ([`Self::tick_strategy_dispatch`], which passes
    /// `MQK_STRATEGY_SYMBOL` / `MQK_STRATEGY_MD_TIMEFRAME`) is unchanged in
    /// behavior, return shape, and logging.
    pub async fn tick_strategy_dispatch_for_symbol(
        &self,
        symbol: &str,
        timeframe: &str,
    ) -> Option<mqk_strategy::StrategyBarResult> {
        let bar = self.pending_strategy_bar_input.lock().await.take()?;
        self.dispatch_native_strategy_for_symbol_with_bar(symbol, timeframe, bar)
            .await
    }

    /// Test seam for INTRADAY-MD-FRESHNESS-AUTONOMOUS-01.
    ///
    /// Exercises the same post-fetch dispatch/freshness path as production
    /// without requiring a DB connection or writing fixture rows. It does not
    /// read `pending_strategy_bar_input` and never calls providers, brokers, or
    /// order routes.
    pub async fn dispatch_native_strategy_for_symbol_with_loaded_bars_for_test(
        &self,
        symbol: &str,
        timeframe: &str,
        bar: StrategyBarInput,
        db_bars: Vec<mqk_db::MdBarRow>,
        now_ts: i64,
    ) -> Option<mqk_strategy::StrategyBarResult> {
        self.dispatch_native_strategy_for_symbol_with_loaded_bars(
            symbol.trim(),
            timeframe.trim(),
            bar,
            db_bars,
            now_ts,
        )
        .await
    }

    /// Test seam for DURABLE-PAPER-PORTFOLIO-AND-PNL-01C.
    ///
    /// Calls the real production acceptance seam
    /// ([`snapshot::accept_external_broker_snapshot`]) directly with an
    /// already-constructed `BrokerSnapshot`, so tests exercise the exact
    /// same in-memory-write-plus-durable-persist path the run-start
    /// cold-fetch and periodic-refresh call sites use, without requiring a
    /// live orchestrator, run, or broker fetch.
    pub async fn accept_external_broker_snapshot_for_test(
        &self,
        snapshot: mqk_schemas::BrokerSnapshot,
        run_id: Option<uuid::Uuid>,
        operation_id: Option<uuid::Uuid>,
    ) {
        snapshot::accept_external_broker_snapshot(self, snapshot, run_id, operation_id).await;
    }

    /// Test seam for the multi-symbol dispatch loop after DB rows are loaded.
    ///
    /// Mirrors [`Self::tick_strategy_dispatch_multi_symbol`] but accepts a
    /// per-symbol row map instead of reading `md_bars`, so tests can prove
    /// per-symbol freshness behavior without DB/provider/broker dependencies.
    pub async fn tick_strategy_dispatch_multi_symbol_with_loaded_bars_for_test(
        &self,
        assignments: &[SymbolStrategyAssignment],
        bar: StrategyBarInput,
        bars_by_symbol: &BTreeMap<String, Vec<mqk_db::MdBarRow>>,
        now_ts: i64,
    ) -> Vec<(SymbolStrategyAssignment, mqk_strategy::StrategyBarResult)> {
        let mut results = Vec::new();
        for assignment in assignments {
            let db_bars = bars_by_symbol
                .get(&assignment.symbol)
                .cloned()
                .unwrap_or_default();
            if let Some(bar_result) = self
                .dispatch_native_strategy_for_symbol_with_loaded_bars(
                    &assignment.symbol,
                    &assignment.timeframe,
                    bar.clone(),
                    db_bars,
                    now_ts,
                )
                .await
            {
                results.push((assignment.clone(), bar_result));
            }
        }
        results
    }

    /// B1B/AUTON-SIGNAL-CONTEXT-01: Execution loop dispatch seam.
    ///
    /// Called exclusively by the execution loop on each tick.  The loop is the
    /// canonical runtime-owned `on_bar` dispatch owner.
    ///
    /// AUTON-SIGNAL-CONTEXT-01: When `MQK_STRATEGY_SYMBOL` and
    /// `MQK_STRATEGY_MD_TIMEFRAME` are both set and the daemon has a DB pool,
    /// loads up to `STRATEGY_CONTEXT_LOAD_LIMIT` recent completed bars from
    /// `md_bars` and builds a real `RecentBarsWindow`.  This lets built-in
    /// strategies satisfy their LOOKBACK requirements using real price history.
    ///
    /// Falls back to the single-stub context (original B1B path) when:
    /// - env vars are absent (not configured)
    /// - DB pool is unavailable
    /// - DB query fails
    ///
    /// DB returns no completed bars, or stale completed bars for the effective
    /// timeframe cap, fail closed before strategy dispatch.
    ///
    /// Returns `Some(StrategyBarResult)` when a bar was pending AND the bootstrap
    /// is Active.  Returns `None` on most ticks (no pending bar) or when the
    /// bootstrap is absent / Dormant / Failed — all fail-closed.
    ///
    /// PER-SYMBOL-BAR-WINDOW-01: delegates to
    /// [`Self::tick_strategy_dispatch_for_symbol`] with the env-configured
    /// symbol/timeframe. Behavior, return shape, and logging are unchanged.
    pub async fn tick_strategy_dispatch(&self) -> Option<mqk_strategy::StrategyBarResult> {
        let symbol = std::env::var("MQK_STRATEGY_SYMBOL")
            .map(|v| v.trim().to_string())
            .unwrap_or_default();
        let md_timeframe = std::env::var(STRATEGY_MD_TIMEFRAME_ENV)
            .map(|v| v.trim().to_string())
            .unwrap_or_default();

        self.tick_strategy_dispatch_for_symbol(&symbol, &md_timeframe)
            .await
    }

    /// MULTI-SYMBOL-DISPATCH-LOOP-01: dispatch the native strategy across
    /// every symbol in `assignments`, sequentially, in the given order
    /// (design doc §5 Q1/Q4).
    ///
    /// Takes the single pending [`StrategyBarInput`] ONCE — fail-closed empty
    /// `Vec` if none is pending, matching the "no pending bar this tick"
    /// semantics of [`Self::tick_strategy_dispatch`]. `pending_strategy_bar_input`
    /// is a single account-wide slot (deposited by the signal route, design
    /// doc §4.4); it carries no symbol, so the same bar-tick signal
    /// (`now_tick` / `end_ts` / `limit_price` / `qty`) is cloned and
    /// dispatched once per [`SymbolStrategyAssignment`] via
    /// [`Self::dispatch_native_strategy_for_symbol_with_bar`]. Each symbol
    /// still gets its own DB bar-window lookup
    /// (`fetch_recent_completed_bars_for_strategy(symbol, timeframe, ...)`),
    /// so the strategy is evaluated against that symbol's own price history.
    ///
    /// Only `Some` results are collected, paired with the assignment that
    /// produced them — `None` (dormant or absent bootstrap, etc.) is skipped
    /// without affecting any other symbol (Q5). The returned `Vec` preserves
    /// `assignments` order.
    ///
    /// For [`MultiSymbolConfigSource::EnvSingleSymbolFallback`]
    /// (`assignments` has exactly one entry — the legacy
    /// `MQK_STRATEGY_SYMBOL` / `MQK_STRATEGY_MD_TIMEFRAME` pair), this call
    /// is behaviorally identical to [`Self::tick_strategy_dispatch`]: the
    /// same single `.take()`, the same single dispatch.
    ///
    /// A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01: a panic inside the real
    /// `Strategy::on_bar` callback is caught narrowly at the one seam that
    /// actually invokes it ([`Self::invoke_native_strategy_host_on_bar`]),
    /// not around this whole per-symbol dispatch future. Infrastructure
    /// (DB load, bar-window prep, journal writes) is NOT wrapped in any
    /// catch — a genuine infrastructure panic still unwinds this loop
    /// normally, exactly as before this patch, instead of being
    /// misclassified as an isolatable strategy fault. See
    /// [`Self::invoke_native_strategy_host_on_bar`]'s doc comment for why a
    /// caught `on_bar` panic quarantines the shared host rather than
    /// permitting sibling continuation against it.
    pub async fn tick_strategy_dispatch_multi_symbol(
        &self,
        assignments: &[SymbolStrategyAssignment],
    ) -> Vec<(SymbolStrategyAssignment, mqk_strategy::StrategyBarResult)> {
        let Some(bar) = self.pending_strategy_bar_input.lock().await.take() else {
            return Vec::new();
        };
        let mut results = Vec::new();
        for assignment in assignments {
            if let Some(bar_result) = self
                .dispatch_native_strategy_for_symbol_with_bar(
                    &assignment.symbol,
                    &assignment.timeframe,
                    bar.clone(),
                )
                .await
            {
                results.push((assignment.clone(), bar_result));
            }
        }
        results
    }

    /// RUNTIME-OPPORTUNITY-ALLOCATION-01 authority repair (Phase A): the
    /// production execution-loop dispatch seam. Identical dispatch order,
    /// bar-input handling, and per-symbol semantics to
    /// [`Self::tick_strategy_dispatch_multi_symbol`] — same single `.take()`
    /// of the pending bar input, same per-assignment loop, same "only `Some`
    /// results collected" filtering — but additionally carries each result's
    /// exact [`EvaluatedBarFacts`] (or `None` on the single-stub fallback
    /// path) so the caller can bind a same-cycle allocation decision to the
    /// exact bar its strategy evaluation used, without a second DB fetch.
    /// `loop_runner.rs`'s execution loop calls this instead of
    /// `tick_strategy_dispatch_multi_symbol`; the latter is retained for its
    /// existing test callers and does not duplicate this dispatch logic —
    /// both ultimately call
    /// [`Self::dispatch_native_strategy_for_symbol_with_bar_and_facts`].
    ///
    /// A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01: same narrow
    /// on_bar-only panic containment as
    /// [`Self::tick_strategy_dispatch_multi_symbol`] — see that method's
    /// doc comment and [`Self::invoke_native_strategy_host_on_bar`] for the
    /// isolation/quarantine argument.
    pub async fn tick_strategy_dispatch_multi_symbol_with_bar_facts(
        &self,
        assignments: &[SymbolStrategyAssignment],
    ) -> Vec<(
        SymbolStrategyAssignment,
        mqk_strategy::StrategyBarResult,
        Option<EvaluatedBarFacts>,
    )> {
        let Some(bar) = self.pending_strategy_bar_input.lock().await.take() else {
            return Vec::new();
        };
        let mut results = Vec::new();
        for assignment in assignments {
            if let Some((bar_result, facts)) = self
                .dispatch_native_strategy_for_symbol_with_bar_and_facts(
                    &assignment.symbol,
                    &assignment.timeframe,
                    bar.clone(),
                )
                .await
            {
                results.push((assignment.clone(), bar_result, facts));
            }
        }
        results
    }

    /// PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE Part 4: the
    /// `DynamicPaperEnforced` dispatch backend — the frozen selected-host
    /// authority is the ONLY strategy-evaluation authority when this is
    /// called; the legacy native bootstrap is never touched. Same single
    /// `.take()` of the pending bar input as
    /// [`Self::tick_strategy_dispatch_multi_symbol_with_bar_facts`], same
    /// per-binding sequential order (`bindings` is already in the frozen
    /// plan's deterministic symbol-ascending order — never re-sorted here),
    /// same one-DB-bar-window-load-per-selected-symbol/timeframe shape,
    /// reusing the exact common bar-window authority
    /// ([`Self::prepare_bar_window_for_symbol_timeframe`]) the legacy
    /// backend uses. Returns `Err` (never invented decisions) the instant a
    /// selected-host result fails Part 5 coherence — the caller must submit
    /// zero decisions for the whole tick and halt.
    pub(crate) async fn tick_strategy_dispatch_selected_hosts_with_bar_facts(
        &self,
        // TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 Blocker 2: the active
        // frozen dispatch authority's exact `run_id` — never the mutable
        // `status.active_run_id` cache. Every selected-host journal row this
        // call writes is bound to this exact value and each binding's own
        // `strategy_id`, never `native_strategy_bootstrap`.
        run_id: Uuid,
        bindings: &[crate::dynamic_selection_dispatch_authority::SelectedDispatchBinding],
        host_pool: &mut crate::dynamic_selection_host_pool::DynamicSelectionHostPool,
    ) -> Result<
        Vec<(
            SymbolStrategyAssignment,
            mqk_strategy::StrategyBarResult,
            Option<EvaluatedBarFacts>,
        )>,
        SelectedHostDispatchFault,
    > {
        let Some(bar) = self.pending_strategy_bar_input.lock().await.take() else {
            return Ok(Vec::new());
        };
        let mut results = Vec::new();
        for binding in bindings {
            let selected_authority = SignalEvaluationAuthority::Explicit {
                run_id,
                strategy_id: &binding.strategy_id,
            };
            let db_bars = match &self.db {
                Some(pool) => {
                    match mqk_db::fetch_recent_completed_bars_for_strategy(
                        pool,
                        &binding.symbol,
                        &binding.db_timeframe_label,
                        STRATEGY_CONTEXT_LOAD_LIMIT,
                    )
                    .await
                    {
                        Ok(rows) => rows,
                        Err(e) => {
                            // Part 4: a DB load error must not invoke a
                            // selected host using the legacy stub fallback —
                            // this binding produces no result this tick, no
                            // other binding is affected.
                            self.last_bar_context_bars.store(0, Ordering::SeqCst);
                            tracing::warn!(
                                symbol = %binding.symbol,
                                strategy_id = %binding.strategy_id,
                                error = %e,
                                "phase7b_selected_host_db_bar_load_failed: no stub fallback \
                                 for selected-host dispatch; binding skipped this tick"
                            );
                            // Blocker 2: this diagnostic is durably journaled
                            // under the exact selected authority, not the
                            // legacy bootstrap — the row must still name the
                            // real selected binding even though no strategy
                            // ever ran.
                            self.record_signal_evaluation(
                                selected_authority,
                                SignalEvaluationAttempt {
                                    now_tick: bar.now_tick,
                                    symbol: &binding.symbol,
                                    timeframe: &binding.db_timeframe_label,
                                    bar_context_source: "db_load_failed",
                                    bars_loaded: 0,
                                    latest_bar_ts_utc: None,
                                    signal_generated: false,
                                    signal_qty: None,
                                    reason_code: "selected_host_db_bar_load_failed",
                                    reason: "DB bar-window load failed for this selected binding; \
                                             no stub fallback",
                                    decision_stage: "pre_dispatch_gate",
                                },
                            )
                            .await;
                            continue;
                        }
                    }
                }
                None => {
                    // PaperEnforcedAllowed always requires a DB (readiness/
                    // promotion evaluation already required one to reach
                    // this disposition) — no stub fallback for selected
                    // hosts, ever.
                    continue;
                }
            };

            let now_ts = Utc::now().timestamp();
            let (window, latest_bar_row, bars_loaded, diagnostic_decision, diagnostic_reason) =
                match self
                    .prepare_bar_window_for_symbol_timeframe(
                        &binding.symbol,
                        &binding.db_timeframe_label,
                        &bar,
                        db_bars,
                        now_ts,
                        selected_authority,
                    )
                    .await
                {
                    BarWindowPrepOutcome::Refused => continue,
                    BarWindowPrepOutcome::Ready {
                        window,
                        latest_bar_row,
                        bars_loaded,
                        diagnostic_decision,
                        diagnostic_reason,
                    } => (
                        window,
                        latest_bar_row,
                        bars_loaded,
                        diagnostic_decision,
                        diagnostic_reason,
                    ),
                };

            let Some(host) = host_pool.get_mut(
                &binding.symbol,
                &binding.strategy_id,
                binding.timeframe_secs,
            ) else {
                return Err(SelectedHostDispatchFault::HostMissingAtDispatch {
                    symbol: binding.symbol.clone(),
                    strategy_id: binding.strategy_id.clone(),
                    timeframe_secs: binding.timeframe_secs,
                });
            };
            let ctx =
                mqk_strategy::StrategyContext::new(binding.timeframe_secs, bar.now_tick, window);
            // TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 Blocker 3: this is
            // the one real selected-host invocation call site — the ONLY
            // strategy-evaluation authority while `DynamicPaperEnforced` is
            // active.
            #[cfg(test)]
            self.loop_call_trace_push_for_test(format!(
                "host_call:{}:{}",
                binding.symbol, binding.strategy_id
            ));
            // A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01: `host.on_bar` is
            // synchronous, so a panic here is caught in-place rather than
            // propagated as an unwind through the surrounding async fn.
            // Unlike the legacy loop, a caught panic here does NOT continue
            // to the next binding -- this backend's own frozen contract
            // (Part 5, doc comment above) already treats an ordinary
            // `on_bar` `Err` as a whole-tick structural fault ("the caller
            // must submit zero decisions for the whole tick and halt"); a
            // panic is at least as severe a malfunction signal as a typed
            // Err from the exact same call, so it gets the same whole-tick
            // halt via `HostOnBarPanicked`, never more lenient treatment.
            // Each binding's host is a separate object in `host_pool`
            // (`DynamicSelectionHostPool` keys one `StrategyHost` per
            // `(symbol, strategy_id, timeframe_secs)`), so this panic cannot
            // corrupt any other binding's host regardless.
            //
            // Shares the same test-only injection seam as the legacy loop
            // (`panic_on_symbol_for_test`), resolved here (async) so the
            // synchronous `catch_unwind` closure below only needs a plain
            // bool. Permanently `false` in production.
            let inject_panic_for_test =
                self.panic_on_symbol_for_test.lock().await.as_deref() == Some(binding.symbol.as_str());
            let bar_result = match std::panic::catch_unwind(AssertUnwindSafe(|| {
                if inject_panic_for_test {
                    panic!("A1_TEST_INJECTED_PANIC for symbol {}", binding.symbol);
                }
                host.on_bar(&ctx)
            })) {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    return Err(SelectedHostDispatchFault::HostOnBarError {
                        symbol: binding.symbol.clone(),
                        strategy_id: binding.strategy_id.clone(),
                        detail: format!("{e:?}"),
                    })
                }
                Err(panic_payload) => {
                    return Err(SelectedHostDispatchFault::HostOnBarPanicked {
                        symbol: binding.symbol.clone(),
                        strategy_id: binding.strategy_id.clone(),
                        detail: panic_payload_message(&*panic_payload),
                    })
                }
            };

            // Part 5: selected-host result coherence — a mismatch here is a
            // structural fault, never an ordinary no-signal condition. Pure
            // helper (`check_selected_host_result_coherence`) so every
            // mismatch branch is directly unit-testable without needing a
            // deliberately-corrupted host pool.
            check_selected_host_result_coherence(binding, &bar_result)?;

            let signal_qty: i64 = bar_result
                .intents
                .output
                .targets
                .iter()
                .map(|t| t.qty)
                .sum();
            self.record_signal_evaluation(
                selected_authority,
                SignalEvaluationAttempt {
                    now_tick: bar.now_tick,
                    symbol: &binding.symbol,
                    timeframe: &binding.db_timeframe_label,
                    bar_context_source: "db_loaded",
                    bars_loaded: bars_loaded as i64,
                    latest_bar_ts_utc: DateTime::<Utc>::from_timestamp(latest_bar_row.end_ts, 0),
                    signal_generated: signal_qty != 0,
                    signal_qty: Some(signal_qty),
                    reason_code: diagnostic_decision,
                    reason: diagnostic_reason,
                    decision_stage: "strategy_evaluated",
                },
            )
            .await;

            let facts = EvaluatedBarFacts {
                symbol: binding.symbol.clone(),
                strategy_id: binding.strategy_id.clone(),
                timeframe: binding.db_timeframe_label.clone(),
                bar_end_ts: latest_bar_row.end_ts,
                close_micros: latest_bar_row.close_micros,
            };
            let assignment = SymbolStrategyAssignment {
                symbol: binding.symbol.clone(),
                strategy_id: binding.strategy_id.clone(),
                timeframe: binding.db_timeframe_label.clone(),
            };
            results.push((assignment, bar_result, Some(facts)));
        }
        Ok(results)
    }

    /// MULTI-SYMBOL-DISPATCH-LOOP-01 fail-closed symbol guard: retain only
    /// the `targets` whose `symbol` matches `dispatched_symbol`
    /// (case-insensitive, trimmed). Returns the number of targets dropped.
    ///
    /// The native strategy bootstrap's [`mqk_strategy::TargetPosition::symbol`]
    /// is fixed at construction time from `MQK_STRATEGY_SYMBOL`, independent
    /// of which symbol's bar window was just dispatched (see
    /// `docs/design/native_multi_symbol_dispatch.md`, per-symbol strategy
    /// bootstrap gap). A target whose symbol does not match the dispatched
    /// assignment would otherwise carry a qty computed from a *different*
    /// symbol's bars under the dispatched symbol's name — drop it rather
    /// than submit a misattributed decision.
    ///
    /// No-op for the legacy single-symbol case, where the bootstrap's baked
    /// symbol and the dispatched symbol are the same (`MQK_STRATEGY_SYMBOL`).
    pub fn retain_targets_matching_symbol(
        targets: &mut Vec<mqk_strategy::TargetPosition>,
        dispatched_symbol: &str,
    ) -> usize {
        let before = targets.len();
        let dispatched_symbol = dispatched_symbol.trim();
        targets.retain(|t| t.symbol.trim().eq_ignore_ascii_case(dispatched_symbol));
        before - targets.len()
    }

    /// MULTI-SYMBOL-CAPITAL-CAPS-01 cap #2 (`per_symbol_max_position_qty`,
    /// design doc §6): clamp each target's `qty` so `|qty| <= cap`,
    /// preserving sign. Targets already within the cap are left unchanged.
    ///
    /// `cap` must be positive — the caller (`per_symbol_max_position_qty`,
    /// sourced from `MQK_PER_SYMBOL_MAX_POSITION_QTY`) enforces this; `None`
    /// disables the clamp entirely rather than calling this with `cap <= 0`.
    ///
    /// Returns `(symbol, original_qty, clamped_qty)` for every target that
    /// was clamped, in `targets` order — empty when no target exceeds the
    /// cap (the common, default-disabled case). Callers use this to log
    /// `b1c_target_qty_clamped_per_symbol_cap` and fire a Discord alert.
    pub fn clamp_targets_to_per_symbol_position_cap(
        targets: &mut [mqk_strategy::TargetPosition],
        cap: i64,
    ) -> Vec<(String, i64, i64)> {
        let mut clamped = Vec::new();
        for t in targets.iter_mut() {
            if t.qty.abs() > cap {
                let original_qty = t.qty;
                t.qty = if t.qty < 0 { -cap } else { cap };
                clamped.push((t.symbol.clone(), original_qty, t.qty));
            }
        }
        clamped
    }

    /// MULTI-SYMBOL-CAPITAL-CAPS-01 cap #6 (`max_new_orders_per_tick`, design
    /// doc §6): given the configured per-tick new-order cap and the number of
    /// new orders already accepted earlier in this tick (artifact order,
    /// design doc §5 Q4), returns the `no_order_reason` that should be applied
    /// to the *next* symbol in iteration order, if it would otherwise produce
    /// a new order.
    ///
    /// `cap = None` (the default, from an unset `MQK_MAX_NEW_ORDERS_PER_TICK`)
    /// is unbounded — always returns `None` (no override; today's implicit
    /// behavior, every configured symbol is dispatched every tick).
    ///
    /// `Some("max_new_orders_per_tick_reached")` once
    /// `new_orders_this_tick >= cap` — the caller skips this symbol's decision
    /// derivation/submission entirely for the remainder of the tick. The
    /// symbol is **not lost**: its decisions are re-evaluated fresh next tick
    /// from then-current bar/position state (design doc §6, "no queuing
    /// mechanism is needed").
    ///
    /// `cap = Some(0)` means no new orders are accepted at all this tick — the
    /// first symbol in iteration order already returns
    /// `Some("max_new_orders_per_tick_reached")`.
    pub fn max_new_orders_per_tick_reason(
        new_orders_this_tick: u32,
        cap: Option<u32>,
    ) -> Option<&'static str> {
        match cap {
            Some(cap) if new_orders_this_tick >= cap => Some("max_new_orders_per_tick_reached"),
            _ => None,
        }
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

    pub fn broker_asset_shortable_preflight(
        &self,
        symbol: &str,
    ) -> BrokerAssetShortablePreflightOutcome {
        let Some(fetcher) = self.asset_shortable_preflight_fetcher.as_ref() else {
            return if self.runtime_selection.broker_kind == Some(BrokerKind::Alpaca) {
                BrokerAssetShortablePreflightOutcome::NotConfigured
            } else {
                BrokerAssetShortablePreflightOutcome::UnsupportedAdapter
            };
        };
        match fetcher.fetch_asset_shortable_preflight(symbol) {
            Ok(Some(asset)) => BrokerAssetShortablePreflightOutcome::Active(asset),
            Ok(None) => BrokerAssetShortablePreflightOutcome::SymbolNotFound,
            Err(err) => BrokerAssetShortablePreflightOutcome::QueryFailed(err),
        }
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
        // BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE: a `Degraded`
        // ownership state is a truthful, explicit note — never silently
        // presented as ordinary `idle`. Local authority has already been
        // fully removed by the time this state exists (requirement 4), so
        // no further reap/DB reconciliation is needed here; the honest
        // degraded detail is surfaced directly.
        if let LocalRuntimeOwnership::Degraded { run_id, detail } =
            &*self.runtime_ownership.lock().await
        {
            let snapshot = StatusSnapshot {
                daemon_uptime_secs: uptime_secs(),
                active_run_id: None,
                state: "degraded".to_string(),
                notes: Some(format!(
                    "local runtime ownership is degraded for run {run_id}: {detail}"
                )),
                integrity_armed: self.integrity_armed().await,
                deadman_status: "unknown".to_string(),
                deadman_last_heartbeat_utc: None,
            };
            self.publish_status(snapshot.clone()).await;
            return Ok(snapshot);
        }

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

    async fn active_owned_run_id(&self) -> Option<Uuid> {
        let lock = self.runtime_ownership.lock().await;
        match &*lock {
            LocalRuntimeOwnership::Active { run_id, handle, .. }
                if !handle.join_handle.is_finished() =>
            {
                Some(*run_id)
            }
            _ => None,
        }
    }

    pub async fn locally_owned_run_id(&self) -> Option<Uuid> {
        self.active_owned_run_id().await
    }

    async fn take_execution_loop_for_control(
        &self,
    ) -> Result<Option<ExecutionLoopHandle>, RuntimeLifecycleError> {
        let handle = {
            let mut lock = self.runtime_ownership.lock().await;
            // BUNDLE-7-PHASE-7A-TRUE-ATOMIC requirement 7: unconditional
            // replace-then-restore-if-wrong-variant instead of check-then-
            // replace-then-unreachable!() — the lock is held for the whole
            // span so behavior is identical, but this has no panic path.
            match std::mem::replace(&mut *lock, LocalRuntimeOwnership::Idle) {
                LocalRuntimeOwnership::Active { handle, .. } => Some(handle),
                other => {
                    *lock = other;
                    None
                }
            }
        };

        match handle {
            Some(handle) if !handle.join_handle.is_finished() => Ok(Some(handle)),
            Some(handle) => {
                let run_id = handle.run_id;
                let exit = handle
                    .join_handle
                    .await
                    .map_err(|err| RuntimeLifecycleError::internal("loop reap failed", err))?;
                // BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE
                // requirement 4: the loop is gone (finished on its own,
                // e.g. panic/supervisor exit) — no locally-owned run
                // remains, so every economic mirror for it is cleared too,
                // not only dynamic-selection authority.
                self.clear_economic_mirrors_for_run(run_id).await;
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

    async fn reap_finished_execution_loop(
        &self,
    ) -> Result<Option<ExecutionLoopExit>, RuntimeLifecycleError> {
        let handle = {
            let mut lock = self.runtime_ownership.lock().await;
            // BUNDLE-7-PHASE-7A-TRUE-ATOMIC requirement 7: unconditional
            // replace-then-restore-if-not-taken instead of check-then-
            // replace-then-unreachable!() — the lock is held for the whole
            // span so behavior is identical, but this has no panic path.
            match std::mem::replace(&mut *lock, LocalRuntimeOwnership::Idle) {
                LocalRuntimeOwnership::Active { handle, .. }
                    if handle.join_handle.is_finished() =>
                {
                    Some(handle)
                }
                other => {
                    *lock = other;
                    None
                }
            }
        };

        match handle {
            Some(handle) => {
                let run_id = handle.run_id;
                // BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE
                // requirement 4: reaped a finished loop directly (any
                // caller, not only stop/halt) — the run it belonged to no
                // longer has a local owner, so every economic mirror for it
                // is cleared too, regardless of whether the join itself
                // succeeded (PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01
                // Part 4: ownership was already unconditionally moved to
                // `Idle` above by the time this runs — mirrors must not be
                // left stale behind it either way).
                self.clear_economic_mirrors_for_run(run_id).await;
                let exit = match handle.join_handle.await {
                    Ok(exit) => exit,
                    Err(err) => {
                        // PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01
                        // Part 4 (R6 row 21/25): a panicked task's join
                        // failure must not leave ownership looking like a
                        // clean `Idle` — mark `Degraded` before returning
                        // the error, exactly like every other reap/stop/
                        // halt/shutdown join-failure path now does.
                        self.note_local_runtime_degraded(
                            run_id,
                            BoundedLifecycleDegradation {
                                operation: "reap_join",
                                detail: err.to_string(),
                            },
                        )
                        .await;
                        return Err(RuntimeLifecycleError::internal("loop join failed", err));
                    }
                };
                // PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 4: a
                // self-finished loop's own leadership-release outcome must
                // not be silently discarded by a caller that only inspects
                // `exit.note` — a reported release failure is degraded
                // truth, recorded here so every caller (stop, halt) gets it
                // for free instead of each having to re-check the reap
                // result individually.
                if let Some(Err(ref release_err)) = exit.leadership_release_outcome {
                    self.note_local_runtime_degraded(
                        run_id,
                        BoundedLifecycleDegradation {
                            operation: "reap_leadership_release",
                            detail: release_err.clone(),
                        },
                    )
                    .await;
                }
                Ok(Some(exit))
            }
            None => Ok(None),
        }
    }

    // -----------------------------------------------------------------
    // BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement 2:
    // the only legal `runtime_ownership` transitions. `reserve_runtime_
    // ownership` is the sole path from `Idle`; `prepare_starting_metadata_
    // and_mirrors` is the sole path from `Reserved` to `Starting`;
    // `install_active_runtime` is the sole path from `Starting` to
    // `Active`. All three compare explicitly rather than trusting caller
    // ordering, so a bug elsewhere cannot silently steal or overwrite a
    // different run's ownership.
    // -----------------------------------------------------------------

    /// Atomically claim ownership for `run_id`. `Idle -> Reserved{run_id}`
    /// on success. Refuses (leaving ownership untouched) when any other
    /// state is currently held, including a second reservation attempt for
    /// `run_id` itself (reservation is meant to happen at most once per
    /// attempt).
    pub(crate) async fn reserve_runtime_ownership(&self, run_id: Uuid) -> Result<(), Uuid> {
        let mut lock = self.runtime_ownership.lock().await;
        match &*lock {
            LocalRuntimeOwnership::Idle => {
                *lock = LocalRuntimeOwnership::Reserved { run_id };
                Ok(())
            }
            other => Err(other.owned_run_id().unwrap_or(run_id)),
        }
    }

    /// Build the immutable [`RunStartMetadata`] for `run_id`, write every
    /// economic-mirror field this attempt prepared, and transition
    /// ownership `Reserved{run_id} -> Starting{run_id, metadata}` as one
    /// step. Only valid immediately after `reserve_runtime_ownership
    /// (run_id)` has already succeeded — this function itself does not
    /// retry or wait, since the caller (`ProductionRuntimeStartEffects::
    /// start_runtime_effects`) already holds `AppState::lifecycle_op` for
    /// the whole attempt.
    pub(crate) async fn prepare_starting_metadata_and_mirrors(
        &self,
        run_id: Uuid,
        bundle: RunStartLocalBundle,
        frozen_assignments: Vec<SymbolStrategyAssignment>,
        frozen_assignments_source: &'static str,
    ) -> Result<Arc<RunStartMetadata>, PrepareStartingMetadataError> {
        // Economic mirrors: current separate storage, unaffected by the
        // ownership consolidation (root-cause design rule) — initialized
        // here, strictly after reservation, exactly as `commit_run_start_
        // bundle` did before this patch.
        let native_bootstrap_present = bundle.native_strategy_bootstrap.is_some();
        *self.execution_snapshot.write().await = bundle.execution_snapshot;
        *self.accepted_artifact.write().await = bundle.accepted_artifact.clone();
        *self.native_strategy_bootstrap.lock().await = bundle.native_strategy_bootstrap;
        self.day_signal_count.store(0, Ordering::SeqCst);
        self.reset_symbol_day_order_counts().await;
        self.reset_signal_blocked_alert_state();
        self.reset_bar_tick_counters();
        self.clear_per_symbol_target_states().await;

        let metadata = Arc::new(RunStartMetadata {
            run_id,
            accepted_artifact: bundle.accepted_artifact,
            native_bootstrap_present,
            dynamic_selection: tokio::sync::RwLock::new(bundle.dynamic_selection_outcome),
            frozen_assignments,
            frozen_assignments_source,
            approved_for_live: false,
        });

        let mut lock = self.runtime_ownership.lock().await;
        match &*lock {
            LocalRuntimeOwnership::Reserved { run_id: reserved } if *reserved == run_id => {
                *lock = LocalRuntimeOwnership::Starting {
                    run_id,
                    metadata: Arc::clone(&metadata),
                };
                Ok(metadata)
            }
            LocalRuntimeOwnership::Reserved { run_id: reserved } => {
                Err(PrepareStartingMetadataError::ReservedForDifferentRun {
                    reserved_run_id: *reserved,
                })
            }
            LocalRuntimeOwnership::Idle => Err(PrepareStartingMetadataError::NoReservation),
            _ => Err(PrepareStartingMetadataError::NotReserved),
        }
    }

    /// Install the running handle for `run_id`, consuming the `Starting`
    /// binding. Only valid from `Starting{run_id, metadata}` for the exact
    /// same `run_id`. `handle`'s task is already running by the time this
    /// is called — on any mismatch this stops and joins it before
    /// returning, rather than dropping the handle and leaking a detached
    /// task (requirement 3's "install failure cancels/stops/joins task").
    pub(crate) async fn install_active_runtime(
        &self,
        run_id: Uuid,
        handle: ExecutionLoopHandle,
    ) -> Result<(), InstallActiveRuntimeError> {
        // PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 1
        // requirement 3: the refusal reason is determined first (still
        // holding `lock`), then the just-spawned task is stopped and
        // joined, and only then is the reason paired with the join's
        // structured cleanup truth — never a discarded `let _ =
        // handle.join_handle.await;`.
        enum Reason {
            NotStarting,
            StartingForDifferentRun { starting_run_id: Uuid },
            AlreadyActive { active_run_id: Uuid },
        }

        let mut lock = self.runtime_ownership.lock().await;
        let reason = match &*lock {
            LocalRuntimeOwnership::Starting {
                run_id: starting,
                metadata,
            } if *starting == run_id => {
                // PHASE-7A-FINAL-PRIVATE-PRODUCTION-EFFECTS-PROOF
                // requirement 6: hermetic injection point for the
                // barrier-cancellation / Active-install-failure matrix
                // (R6 items 19/20). Always `false` in production and for
                // every test that does not explicitly enable it. When
                // enabled, this takes the exact same conflict-cleanup path
                // (stop + join the just-spawned handle, return `Err`) the
                // genuine `StartingForDifferentRun`/`AlreadyActive`
                // branches below already use — no new lifecycle code, only
                // a deterministic trigger for it.
                if self.install_active_runtime_conflict_forced() {
                    Reason::NotStarting
                } else {
                    let metadata = Arc::clone(metadata);
                    *lock = LocalRuntimeOwnership::Active {
                        run_id,
                        metadata,
                        handle,
                    };
                    return Ok(());
                }
            }
            LocalRuntimeOwnership::Starting {
                run_id: starting, ..
            } => Reason::StartingForDifferentRun {
                starting_run_id: *starting,
            },
            LocalRuntimeOwnership::Active { run_id: active, .. } => Reason::AlreadyActive {
                active_run_id: *active,
            },
            _ => Reason::NotStarting,
        };
        drop(lock);
        let _ = handle.stop_tx.send(ExecutionLoopCommand::Stop);
        let cleanup = match handle.join_handle.await {
            Ok(exit) => InstallRuntimeTaskCleanup {
                join_outcome: Ok(()),
                leadership_release_outcome: exit.leadership_release_outcome,
            },
            Err(join_err) => InstallRuntimeTaskCleanup {
                join_outcome: Err(join_err.to_string()),
                leadership_release_outcome: None,
            },
        };
        Err(match reason {
            Reason::NotStarting => InstallActiveRuntimeError::NotStarting { cleanup },
            Reason::StartingForDifferentRun { starting_run_id } => {
                InstallActiveRuntimeError::StartingForDifferentRun {
                    starting_run_id,
                    cleanup,
                }
            }
            Reason::AlreadyActive { active_run_id } => InstallActiveRuntimeError::AlreadyActive {
                active_run_id,
                cleanup,
            },
        })
    }

    /// PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01
    /// test-support helper: spawns a real `state/loop_runner.rs::
    /// spawn_execution_loop` task for `run_id` against this AppState's own
    /// (already-configured) `db` — production's pre-tick deadman/heartbeat/
    /// halt logic reads `AppState::db`, not the orchestrator's internal
    /// pool — installs it as the canonical `Active` local owner exactly the
    /// way `ProductionRuntimeStartEffects::spawn_loop` does (reserve ->
    /// prepare -> spawn -> install -> release startup barrier), and
    /// immediately releases the startup barrier so it begins real ticking.
    ///
    /// Uses a hermetic in-process `LockedPaperBroker` orchestrator (no
    /// network, no credentials) wired to a lazily-connected, never-actually-
    /// dialed pool of its own — mirrors the existing `spawn_barrier_test_loop`
    /// pattern in `ownership_state_machine_tests` below. This bypasses
    /// `start_execution_runtime()`'s deployment-mode-readiness gate (which
    /// refuses Paper+Paper) entirely, so DB-backed integration tests outside
    /// this crate can drive the real execution-loop task — including its
    /// pre-tick deadman-halt branch, which is gated on `AppState::db` — for
    /// a run this AppState's own `db` already knows about, without needing
    /// live Alpaca credentials or a mock Alpaca server.
    ///
    /// Panics on any reserve/prepare/install failure — this is test
    /// scaffolding for an already-known-good ownership slot, not a
    /// production path that must degrade gracefully.
    pub async fn install_real_execution_loop_for_test(self: &Arc<Self>, run_id: Uuid) {
        use mqk_broker_paper::LockedPaperBroker;
        use mqk_execution::BrokerOrderMap;
        use mqk_portfolio::PortfolioState;
        use mqk_reconcile::{BrokerSnapshot, LocalSnapshot};
        use mqk_runtime::orchestrator::WallClock;
        use mqk_runtime::runtime_risk::RuntimeRiskGate;

        self.reserve_runtime_ownership(run_id)
            .await
            .expect("install_real_execution_loop_for_test: reserve_runtime_ownership");
        let bundle = RunStartLocalBundle {
            execution_snapshot: None,
            accepted_artifact: None,
            native_strategy_bootstrap: None,
            dynamic_selection_outcome: None,
        };
        self.prepare_starting_metadata_and_mirrors(run_id, bundle, Vec::new(), "test")
            .await
            .expect("install_real_execution_loop_for_test: prepare_starting_metadata_and_mirrors");

        let integrity_gate = types::StateIntegrityGate {
            integrity: Arc::clone(&self.integrity),
        };
        let reconcile_gate = types::ReconcileTruthGate {
            reconcile_status: Arc::clone(&self.reconcile_status),
        };
        let risk_gate =
            RuntimeRiskGate::from_run_config(&serde_json::json!({}), 1_000_000_000_i64);
        let daemon_broker = broker::DaemonBroker::Paper(LockedPaperBroker::default());
        let gateway = mqk_execution::wiring::build_gateway(
            daemon_broker,
            integrity_gate,
            risk_gate,
            reconcile_gate,
        );
        let orchestrator_pool = sqlx::PgPool::connect_lazy(
            "postgresql://127.0.0.1:5432/mqk_local_quiescence_test_stub",
        )
        .expect("install_real_execution_loop_for_test: connect_lazy URL parse must succeed");
        let orchestrator = types::DaemonOrchestrator::new(
            orchestrator_pool,
            gateway,
            BrokerOrderMap::new(),
            BTreeMap::new(),
            PortfolioState::new(1_000_000_000_i64),
            run_id,
            "local-quiescence-test-dispatcher",
            "local-quiescence-test",
            None,
            WallClock,
            Box::new(LocalSnapshot::empty),
            Box::new(|| BrokerSnapshot::empty_at(0)),
        );

        let (barrier_tx, barrier_rx) = tokio::sync::oneshot::channel();
        let handle = loop_runner::spawn_execution_loop(
            Arc::clone(self),
            orchestrator,
            run_id,
            crate::dynamic_selection_dispatch_authority::RuntimeStrategyDispatchAuthority::Legacy {
                assignments: Vec::new(),
            },
            barrier_rx,
        );
        self.install_active_runtime(run_id, handle)
            .await
            .expect("install_real_execution_loop_for_test: install_active_runtime");
        let _ = barrier_tx.send(());
    }

    // -----------------------------------------------------------------
    // BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement 4:
    // one unified cleanup authority for every local runtime lifecycle exit
    // path.
    // -----------------------------------------------------------------

    /// Clear every economic-mirror field, unconditionally. Called only
    /// after ownership has already been proven to belong to the run being
    /// cleared (either by a run_id-scoped compare-and-clear on
    /// `runtime_ownership`, or because the caller just extracted the
    /// handle for that exact run) — this function itself performs no
    /// run_id check, matching `commit_run_start_bundle`'s pre-existing
    /// "caller already reserved" contract.
    async fn clear_economic_mirrors_for_run(&self, _run_id: Uuid) {
        *self.execution_snapshot.write().await = None;
        *self.accepted_artifact.write().await = None;
        *self.native_strategy_bootstrap.lock().await = None;
        self.day_signal_count.store(0, Ordering::SeqCst);
        self.reset_symbol_day_order_counts().await;
        self.reset_signal_blocked_alert_state();
        self.reset_bar_tick_counters();
        self.clear_per_symbol_target_states().await;
    }

    /// The single cleanup authority for every local runtime lifecycle exit
    /// path (failed start, run-link-persist failure, runtime-effects
    /// failure, barrier/install failure, stop, halt, shutdown, reap,
    /// loop panic/supervisor exit, restart/replacement). Clears
    /// authoritative ownership (`runtime_ownership`, including whatever
    /// `RunStartMetadata` — and therefore dynamic-selection truth — it
    /// carried) AND every economic-mirror field, scoped to `run_id`.
    /// Idempotent, and a no-op (leaving everything untouched) when `run_id`
    /// does not match the currently-owned run — requirement 4's "run A
    /// cannot clear B". Stops and joins any live `Active` handle found for
    /// `run_id` before clearing mirrors, so a still-ticking loop can never
    /// race a mirror clear.
    pub(crate) async fn clear_local_runtime_for_run(
        &self,
        run_id: Uuid,
        _reason: LifecycleClearReason,
    ) -> LocalRuntimeClearOutcome {
        let taken = {
            let mut lock = self.runtime_ownership.lock().await;
            if lock.owned_run_id() != Some(run_id) {
                None
            } else {
                Some(std::mem::replace(&mut *lock, LocalRuntimeOwnership::Idle))
            }
        };

        let Some(owned) = taken else {
            return LocalRuntimeClearOutcome::default();
        };

        let mut outcome = LocalRuntimeClearOutcome {
            cleared: true,
            ..Default::default()
        };

        if let LocalRuntimeOwnership::Active { handle, .. } = owned {
            outcome.stopped_live_handle = !handle.join_handle.is_finished();
            let _ = handle.stop_tx.send(ExecutionLoopCommand::Stop);
            match handle.join_handle.await {
                Ok(exit) => {
                    outcome.leadership_release_outcome = exit.leadership_release_outcome;
                }
                Err(err) => {
                    outcome.join_error = Some(err.to_string());
                }
            }
        }

        self.clear_economic_mirrors_for_run(run_id).await;
        outcome
    }

    /// Peek the currently-owned run_id (if any) and fully clear it via
    /// `clear_local_runtime_for_run` — requirement 4's "no blanket clear
    /// before owned run ID is known": the run_id being cleared is always
    /// read from live ownership first, never assumed or passed in blind.
    /// Returns `None` when ownership was already `Idle` (nothing to
    /// clear), or when ownership changed between the peek and the clear
    /// (e.g. a concurrent clear already ran) — the latter case leaves
    /// `LocalRuntimeClearOutcome::cleared == false` internally, so this
    /// wrapper simply reports "nothing was cleared by this call".
    pub(crate) async fn clear_currently_owned_local_runtime(
        &self,
        reason: LifecycleClearReason,
    ) -> Option<(Uuid, LocalRuntimeClearOutcome)> {
        let run_id = self.runtime_ownership.lock().await.owned_run_id()?;
        let outcome = self.clear_local_runtime_for_run(run_id, reason).await;
        if outcome.cleared {
            Some((run_id, outcome))
        } else {
            None
        }
    }

    /// Requirement 4: mark local truth honestly `Degraded` after ownership
    /// for `run_id` has already been fully cleared (ownership is `Idle`)
    /// but a subsequent durable transition (`stop_run`/`halt_run`) failed —
    /// "local authority removed even when DB transition fails, with
    /// degraded truth", never silently presented as clean `Idle`. A no-op
    /// if ownership is no longer `Idle` (a newer attempt has already
    /// claimed it — never overwritten).
    pub(crate) async fn note_local_runtime_degraded(
        &self,
        run_id: Uuid,
        detail: BoundedLifecycleDegradation,
    ) {
        let mut lock = self.runtime_ownership.lock().await;
        if matches!(&*lock, LocalRuntimeOwnership::Idle) {
            *lock = LocalRuntimeOwnership::Degraded { run_id, detail };
        }
    }

    // -----------------------------------------------------------------
    // DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: run-scoped
    // dynamic-selection lifecycle state, now projected from ownership
    // metadata (requirement 2 invariant 10) instead of an independent
    // field. Production code never calls the commit/clear helpers below —
    // the real start path builds `dynamic_selection` directly inside
    // `prepare_starting_metadata_and_mirrors`'s `RunStartMetadata`, and
    // every real lifecycle exit clears it as part of `clear_local_runtime_
    // for_run`. These remain narrow test-support seams (mirroring this
    // codebase's established `_for_test` convention), not a second
    // authority.
    // -----------------------------------------------------------------

    /// Read-only snapshot of the current dynamic-selection runtime truth,
    /// projected from ownership metadata. `None` when no run is active or
    /// the starting/active run's disposition has not committed one.
    /// Phase 7C Part 4: the current `LocalRuntimeOwnership` state, collapsed
    /// to the closed four-value operator vocabulary
    /// `idle`/`starting`/`running`/`degraded` (`Reserved` counts as
    /// `starting` — ownership exists but no run metadata is committed yet).
    /// Read-only; never mutates ownership.
    pub(crate) async fn local_runtime_lifecycle_label(&self) -> &'static str {
        match &*self.runtime_ownership.lock().await {
            LocalRuntimeOwnership::Idle => "idle",
            LocalRuntimeOwnership::Reserved { .. } | LocalRuntimeOwnership::Starting { .. } => {
                "starting"
            }
            LocalRuntimeOwnership::Active { .. } => "running",
            LocalRuntimeOwnership::Degraded { .. } => "degraded",
        }
    }

    /// Phase 7C Part 4: the `run_id` behind the current ownership state, for
    /// every non-`Idle` state (`Reserved`/`Starting`/`Active`/`Degraded` all
    /// carry one). `None` only for `Idle`.
    pub(crate) async fn local_runtime_owning_run_id(&self) -> Option<Uuid> {
        match &*self.runtime_ownership.lock().await {
            LocalRuntimeOwnership::Idle => None,
            LocalRuntimeOwnership::Reserved { run_id }
            | LocalRuntimeOwnership::Starting { run_id, .. }
            | LocalRuntimeOwnership::Active { run_id, .. }
            | LocalRuntimeOwnership::Degraded { run_id, .. } => Some(*run_id),
        }
    }

    pub async fn dynamic_selection_runtime_snapshot(&self) -> Option<DynamicSelectionRuntimeState> {
        let metadata = {
            let lock = self.runtime_ownership.lock().await;
            match &*lock {
                LocalRuntimeOwnership::Starting { metadata, .. }
                | LocalRuntimeOwnership::Active { metadata, .. } => Some(Arc::clone(metadata)),
                _ => None,
            }
        };
        match metadata {
            Some(metadata) => metadata.dynamic_selection.read().await.clone(),
            None => None,
        }
    }

    /// Test-support entry point: publish `state` as the owning run's
    /// dynamic-selection truth. If ownership already has a `Starting`/
    /// `Active` binding for `state.run_id` (e.g. one established by
    /// `establish_db_backed_active_run_for_test`), this attaches to that
    /// binding in place — the real `ExecutionLoopHandle`, if any, is left
    /// untouched. Otherwise this establishes a minimal `Active` binding
    /// with a trivial self-terminating task, so cleanup-contract tests can
    /// exercise stop/halt/shutdown/reap without a real start attempt.
    /// Always a full overwrite when establishing fresh — never a
    /// partial/in-place merge with a stale prior value for a *different*
    /// run_id.
    ///
    /// Genuinely test-only in purpose (see doc above) and, since
    /// PHASE-7A-FINAL-PRIVATE-PRODUCTION-EFFECTS-PROOF narrowed its one
    /// non-test caller (`commit_dynamic_selection_runtime_state_for_test`)
    /// to `#[cfg(test)]`, every remaining call site is already inside a
    /// `#[cfg(test)]` module — gating this function the same way removes
    /// the resulting dead-code warning in default-build production and
    /// matches its real, test-only role.
    #[cfg(test)]
    pub(crate) async fn commit_dynamic_selection_runtime_state(
        &self,
        state: DynamicSelectionRuntimeState,
    ) {
        let run_id = state.run_id;
        {
            let lock = self.runtime_ownership.lock().await;
            let existing_metadata = match &*lock {
                LocalRuntimeOwnership::Starting {
                    run_id: r,
                    metadata,
                } if *r == run_id => Some(Arc::clone(metadata)),
                LocalRuntimeOwnership::Active {
                    run_id: r,
                    metadata,
                    ..
                } if *r == run_id => Some(Arc::clone(metadata)),
                _ => None,
            };
            drop(lock);
            if let Some(metadata) = existing_metadata {
                *metadata.dynamic_selection.write().await = Some(state);
                return;
            }
        }

        let (stop_tx, mut stop_rx) = watch::channel(ExecutionLoopCommand::Run);
        let join_handle: JoinHandle<ExecutionLoopExit> = tokio::spawn(async move {
            tokio::select! {
                _ = stop_rx.changed() => ExecutionLoopExit {
                    note: Some("test loop stopped".to_string()),
                    leadership_release_outcome: None,
                },
                _ = tokio::time::sleep(std::time::Duration::from_secs(86_400)) => ExecutionLoopExit {
                    note: None,
                    leadership_release_outcome: None,
                },
            }
        });
        let handle = ExecutionLoopHandle {
            run_id,
            stop_tx,
            join_handle,
        };
        let metadata = Arc::new(RunStartMetadata {
            run_id,
            accepted_artifact: None,
            native_bootstrap_present: false,
            dynamic_selection: tokio::sync::RwLock::new(Some(state)),
            frozen_assignments: Vec::new(),
            frozen_assignments_source: "commit_dynamic_selection_runtime_state_test_seam",
            approved_for_live: false,
        });
        *self.runtime_ownership.lock().await = LocalRuntimeOwnership::Active {
            run_id,
            metadata,
            handle,
        };
    }

    /// Test-support: unconditionally release ownership regardless of which
    /// run currently holds it. Idempotent. Narrow-field-clear only — unlike
    /// `clear_local_runtime_for_run`, this does not stop/join a live
    /// handle before dropping it (any handle held is always this
    /// function's own trivial self-terminating test task, which completes
    /// on its own once its `stop_tx` is dropped). Never called from
    /// production or from any external integration test (unlike the
    /// `_for_test`-suffixed seams elsewhere in this file) — genuinely
    /// in-crate-test-only, hence `#[cfg(test)]`.
    #[cfg(test)]
    pub(crate) async fn clear_dynamic_selection_runtime_state(&self) {
        *self.runtime_ownership.lock().await = LocalRuntimeOwnership::Idle;
    }

    /// ATOMICITY-SINGLE-SNAPSHOT-REPAIR requirement 4: run_id-scoped
    /// compare-and-clear. Clears only when the currently-owned run matches
    /// `run_id` — a failed start attempt's rollback for run A must never
    /// clear a newer, already-active run B's committed state. Idempotent:
    /// clearing an already-`Idle` or already-mismatched value is a safe
    /// no-op. Genuinely in-crate-test-only, hence `#[cfg(test)]`.
    #[cfg(test)]
    pub(crate) async fn clear_dynamic_selection_runtime_state_for_run(&self, run_id: Uuid) {
        let mut lock = self.runtime_ownership.lock().await;
        if lock.owned_run_id() == Some(run_id) {
            *lock = LocalRuntimeOwnership::Idle;
        }
    }

    /// `true` when the installed test fault seam matches `seam`. Always
    /// `false` in production (no call site ever installs one outside tests).
    pub(crate) async fn dynamic_selection_fault_seam_is(
        &self,
        seam: DynamicSelectionLifecycleFaultSeam,
    ) -> bool {
        *self.dynamic_selection_fault_seam.read().await == Some(seam)
    }

    /// `true` only when a `#[cfg(test)]` caller has explicitly enabled the
    /// hermetic broker override. Always `false` in production — no
    /// production call site ever enables it, and the setter that could is
    /// itself `#[cfg(test)]`-gated (see `set_hermetic_test_broker_override_
    /// for_test` below), so it cannot exist in a default-build binary.
    pub(crate) async fn hermetic_test_broker_override_enabled(&self) -> bool {
        *self.hermetic_test_broker_override.read().await
    }

    /// PHASE-7A-FINAL-PRIVATE-PRODUCTION-EFFECTS-PROOF requirement 6: enable
    /// (or disable) the hermetic broker override for the real-effects
    /// success matrix. `pub(crate)` + `#[cfg(test)]` — reachable only from
    /// this crate's own test build.
    #[cfg(test)]
    pub(crate) async fn set_hermetic_test_broker_override_for_test(&self, enabled: bool) {
        *self.hermetic_test_broker_override.write().await = enabled;
    }

    /// `true` only when a `#[cfg(test)]` caller has explicitly enabled the
    /// forced install-conflict injection. Always `false` in production.
    pub(crate) fn install_active_runtime_conflict_forced(&self) -> bool {
        self.force_install_active_runtime_conflict
            .load(Ordering::SeqCst)
    }

    /// PHASE-7A-FINAL-PRIVATE-PRODUCTION-EFFECTS-PROOF requirement 6:
    /// enable (or disable) the forced install-conflict injection for the
    /// barrier-cancellation / Active-install-failure matrix. `pub(crate)` +
    /// `#[cfg(test)]` — reachable only from this crate's own test build.
    #[cfg(test)]
    pub(crate) fn set_install_active_runtime_conflict_for_test(&self, enabled: bool) {
        self.force_install_active_runtime_conflict
            .store(enabled, Ordering::SeqCst);
    }

    /// `true` only when a `#[cfg(test)]` caller has explicitly enabled the
    /// forced leadership-release-failure injection. Always `false` in
    /// production.
    pub(crate) fn leadership_release_failure_forced(&self) -> bool {
        self.force_leadership_release_failure.load(Ordering::SeqCst)
    }

    /// PHASE-7A-FINAL-PRIVATE-PRODUCTION-EFFECTS-PROOF requirement 6:
    /// enable (or disable) the forced leadership-release-failure injection.
    /// `pub(crate)` + `#[cfg(test)]` — reachable only from this crate's own
    /// test build.
    #[cfg(test)]
    pub(crate) fn set_leadership_release_failure_for_test(&self, enabled: bool) {
        self.force_leadership_release_failure
            .store(enabled, Ordering::SeqCst);
    }

    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 1 requirement 6:
    /// read back how many times a pre-barrier exit branch released the
    /// runtime leadership lease, for a test to assert "exactly once" rather
    /// than infer it. `pub(crate)` + `#[cfg(test)]` — never reachable from
    /// production code.
    #[cfg(test)]
    pub(crate) fn pre_barrier_leadership_release_count_for_test(&self) -> u32 {
        self.pre_barrier_leadership_release_count
            .load(Ordering::SeqCst)
    }

    /// `true` only when a `#[cfg(test)]` caller has explicitly enabled the
    /// forced execution-loop-panic injection. Always `false` in production.
    pub(crate) fn execution_loop_panic_forced(&self) -> bool {
        self.force_execution_loop_panic.load(Ordering::SeqCst)
    }

    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 3 row 21:
    /// enable (or disable) the forced execution-loop-panic injection.
    /// `pub(crate)` + `#[cfg(test)]` — reachable only from this crate's own
    /// test build.
    #[cfg(test)]
    pub(crate) fn set_execution_loop_panic_for_test(&self, enabled: bool) {
        self.force_execution_loop_panic
            .store(enabled, Ordering::SeqCst);
    }

    /// Test helper: install (or clear, via `None`) a fault-injection seam
    /// for the atomic dynamic-selection start-commit sequence.
    ///
    /// PHASE-7A-FINAL-PRIVATE-PRODUCTION-EFFECTS-PROOF requirement 5: this
    /// mutates a Phase 7A fault seam, so it must not be a default-build
    /// production-reachable API. `pub(crate)` + `#[cfg(test)]` — reachable
    /// only from this crate's own `#[cfg(test)]` modules, never from an
    /// external `tests/*.rs` integration test or from production code.
    #[cfg(test)]
    pub(crate) async fn set_dynamic_selection_fault_seam_for_test(
        &self,
        seam: Option<DynamicSelectionLifecycleFaultSeam>,
    ) {
        *self.dynamic_selection_fault_seam.write().await = seam;
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
    /// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 3: read-only test
    /// accessor for `accepted_artifact` — private in production (no
    /// operator route exposes raw provenance), but needed by sentinel-
    /// preservation tests to prove a losing start attempt's rollback left a
    /// different, legitimate owner's committed provenance untouched.
    /// PHASE-7A-FINAL-PRIVATE-PRODUCTION-EFFECTS-PROOF requirement 5: this
    /// plants active metadata used to prove Phase 7A sentinel-preservation,
    /// so it must not be default-build production-reachable.
    /// `pub(crate)` + `#[cfg(test)]` — this crate's own tests only.
    #[cfg(test)]
    pub(crate) async fn accepted_artifact_snapshot_for_test(
        &self,
    ) -> Option<AcceptedArtifactProvenance> {
        self.accepted_artifact.read().await.clone()
    }

    /// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 3: read-only test
    /// accessor for `day_signal_count`, for the same sentinel-preservation
    /// reason as `accepted_artifact_snapshot_for_test`.
    #[cfg(test)]
    pub(crate) fn day_signal_count_snapshot_for_test(&self) -> u32 {
        self.day_signal_count.load(Ordering::SeqCst)
    }

    /// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 3: plant a
    /// sentinel `accepted_artifact` value, for a sentinel-preservation test
    /// to prove a *different* run's failed/rolled-back start attempt never
    /// clears it.
    #[cfg(test)]
    pub(crate) async fn plant_accepted_artifact_for_test(
        &self,
        value: Option<AcceptedArtifactProvenance>,
    ) {
        *self.accepted_artifact.write().await = value;
    }

    /// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 3: plant a
    /// sentinel `day_signal_count` value, for the same reason as
    /// `plant_accepted_artifact_for_test`.
    #[cfg(test)]
    pub(crate) fn plant_day_signal_count_for_test(&self, value: u32) {
        self.day_signal_count.store(value, Ordering::SeqCst)
    }

    /// ATOMIC-OWNERSHIP-AND-ROLLBACK-TRUTH-01 requirement 3: test-only
    /// wrapper over the `pub(crate)` `commit_dynamic_selection_runtime_
    /// state`, so an in-crate test can plant a sentinel dynamic-selection
    /// value for a run without driving a real start attempt.
    ///
    /// PHASE-7A-FINAL-PRIVATE-PRODUCTION-EFFECTS-PROOF requirement 5:
    /// narrowed from a `pub` external-integration-test seam to
    /// `pub(crate)` + `#[cfg(test)]` — the one external test that used to
    /// call this (`scenario_bundle7_phase7a_final_atomic_ownership_and_
    /// rollback_truth_01.rs`) was moved in-crate alongside
    /// `drive_production_start_effects_for_test`.
    #[cfg(test)]
    pub(crate) async fn commit_dynamic_selection_runtime_state_for_test(
        &self,
        state: DynamicSelectionRuntimeState,
    ) {
        self.commit_dynamic_selection_runtime_state(state).await;
    }

    /// Inject a never-finishing fake execution loop for tests.
    pub async fn inject_running_loop_for_test(&self, run_id: Uuid) {
        let (stop_tx, mut stop_rx) = watch::channel(ExecutionLoopCommand::Run);
        let join_handle: JoinHandle<ExecutionLoopExit> = tokio::spawn(async move {
            tokio::select! {
                _ = stop_rx.changed() => ExecutionLoopExit {
                    note: Some("test loop stopped".to_string()),
                    leadership_release_outcome: None,
                },
                _ = tokio::time::sleep(std::time::Duration::from_secs(86_400)) => ExecutionLoopExit {
                    note: None,
                    leadership_release_outcome: None,
                },
            }
        });

        let handle = ExecutionLoopHandle {
            run_id,
            stop_tx,
            join_handle,
        };
        let metadata = Arc::new(RunStartMetadata {
            run_id,
            accepted_artifact: None,
            native_bootstrap_present: false,
            dynamic_selection: tokio::sync::RwLock::new(None),
            frozen_assignments: Vec::new(),
            frozen_assignments_source: "inject_running_loop_for_test",
            approved_for_live: false,
        });
        *self.runtime_ownership.lock().await = LocalRuntimeOwnership::Active {
            run_id,
            metadata,
            handle,
        };
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
        AutonomousSessionTruth::WsGapPartialRecovery {
            resume_source,
            detail,
        } => Some((
            "ws_gap_partial_recovery",
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
        AutonomousSessionTruth::ControllerExited { detail } => {
            Some(("controller_exited", None, detail.clone()))
        }
        AutonomousSessionTruth::CompletedBarDriverExited { detail } => {
            Some(("completed_bar_driver_exited", None, detail.clone()))
        }
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

    // -----------------------------------------------------------------------
    // BROKER-FILL-REST-PRODUCTION-WIRING-01 proof tests
    // -----------------------------------------------------------------------

    #[test]
    fn fill_fetcher_non_alpaca_broker_is_none() {
        // None broker_kind yields no fetcher — fail-closed.
        let fetcher = build_fill_activity_fetcher_from_env(None, DeploymentMode::Paper);
        assert!(fetcher.is_none(), "None broker_kind must yield no fetcher");

        // Paper broker (LockedPaperBroker) also yields no fetcher.
        let fetcher =
            build_fill_activity_fetcher_from_env(Some(BrokerKind::Paper), DeploymentMode::Paper);
        assert!(
            fetcher.is_none(),
            "BrokerKind::Paper must yield no fill fetcher"
        );
    }

    #[test]
    fn fill_fetcher_alpaca_missing_creds_is_none() {
        // When Alpaca credentials are absent the function returns None (fail-closed).
        // If credentials ARE present in this environment, prove the Some path instead.
        let fetcher =
            build_fill_activity_fetcher_from_env(Some(BrokerKind::Alpaca), DeploymentMode::Paper);
        if std::env::var(ALPACA_KEY_PAPER_ENV).is_ok() {
            assert!(
                fetcher.is_some(),
                "Alpaca+Paper with credentials must yield Some fetcher"
            );
        } else {
            assert!(
                fetcher.is_none(),
                "Alpaca+Paper without credentials must yield None"
            );
        }
    }

    #[test]
    fn fill_fetcher_default_appstate_has_no_fetcher() {
        // Default AppState (paper+paper env not set to alpaca) has no fetcher.
        // This proves the None path in new_inner for non-Alpaca configurations.
        let state = AppState::new();
        assert!(
            state.fill_activity_fetcher.is_none() || std::env::var(ALPACA_KEY_PAPER_ENV).is_ok(),
            "fill_activity_fetcher must be None when Alpaca credentials are absent"
        );
    }

    // -----------------------------------------------------------------------
    // BRK-GAP-REST-RECOVERY-01 production wiring proof tests
    // -----------------------------------------------------------------------

    #[test]
    fn ws_gap_fetcher_non_alpaca_broker_is_none() {
        // None broker_kind → no fetcher (fail-closed).
        let fetcher = build_ws_gap_fill_fetcher_from_env(None, DeploymentMode::Paper);
        assert!(
            fetcher.is_none(),
            "None broker_kind must yield no ws_gap fetcher"
        );

        // Paper broker (LockedPaperBroker) also yields no fetcher.
        let fetcher =
            build_ws_gap_fill_fetcher_from_env(Some(BrokerKind::Paper), DeploymentMode::Paper);
        assert!(
            fetcher.is_none(),
            "BrokerKind::Paper must yield no ws_gap fetcher"
        );
    }

    #[test]
    fn ws_gap_fetcher_alpaca_missing_creds_is_none_no_panic() {
        // When Alpaca credentials are absent the function returns None without panicking.
        // If credentials ARE present in this environment, prove the Some path instead.
        let fetcher =
            build_ws_gap_fill_fetcher_from_env(Some(BrokerKind::Alpaca), DeploymentMode::Paper);
        if std::env::var(ALPACA_KEY_PAPER_ENV).is_ok() {
            assert!(
                fetcher.is_some(),
                "Alpaca+Paper with credentials must yield Some ws_gap fetcher"
            );
        } else {
            assert!(
                fetcher.is_none(),
                "Alpaca+Paper without credentials must yield None ws_gap fetcher"
            );
        }
    }

    #[test]
    fn ws_gap_fetcher_default_appstate_has_no_fetcher() {
        // Default AppState built without Alpaca env vars has no ws_gap_fill_fetcher.
        // Proves the None path in new_inner for non-Alpaca configurations.
        let state = AppState::new();
        assert!(
            state.ws_gap_fill_fetcher.is_none() || std::env::var(ALPACA_KEY_PAPER_ENV).is_ok(),
            "ws_gap_fill_fetcher must be None when Alpaca credentials are absent"
        );
    }

    #[test]
    fn ws_gap_fetcher_test_setter_override_works() {
        use mqk_broker_alpaca::types::AlpacaOrderActivity;

        struct DummyFetcher;
        impl WsGapFillFetcher for DummyFetcher {
            fn fetch_fills_since(
                &self,
                _since: Option<&str>,
            ) -> Result<Vec<AlpacaOrderActivity>, String> {
                Ok(vec![])
            }
        }

        let mut state = AppState::new();
        assert!(
            state.ws_gap_fill_fetcher.is_none() || std::env::var(ALPACA_KEY_PAPER_ENV).is_ok(),
            "start without injected fetcher"
        );
        state.set_ws_gap_fill_fetcher_for_test(Arc::new(DummyFetcher));
        assert!(
            state.ws_gap_fill_fetcher.is_some(),
            "setter must inject the fetcher regardless of env"
        );
        // Confirm the fetcher is callable without panic.
        let result = state
            .ws_gap_fill_fetcher
            .as_deref()
            .unwrap()
            .fetch_fills_since(None);
        assert!(result.is_ok(), "dummy fetcher must return Ok");
    }
}

// ---------------------------------------------------------------------------
// BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE: `LocalRuntimeOwnership`
// state-machine and unified-cleanup proof. Hermetic — no DB, no broker, no
// credentials. The DB-backed reserve/rollback/loop-spawn end-to-end path is
// proven separately (real `ProductionRuntimeStartEffects`) in
// `scenario_bundle7_phase7a_final_atomic_ownership_and_rollback_truth_01.rs`
// and (fake `RuntimeStartEffects`) in
// `scenario_bundle7_phase7a_atomicity_repair_01.rs` /
// `scenario_daily_data_readiness_start_gate_01.rs`.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod ownership_state_machine_tests {
    use super::*;

    fn fresh_state() -> Arc<AppState> {
        Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            BrokerKind::Alpaca,
        ))
    }

    fn run_id_for(label: &str) -> Uuid {
        Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            format!("mqk-daemon.phase7a.core_state_machine.{label}").as_bytes(),
        )
    }

    /// A never-finishing fake handle for `run_id` — mirrors `inject_running_
    /// loop_for_test`'s construction, exposed here so tests can build a
    /// full `Active` binding directly against `runtime_ownership`.
    fn fake_handle(run_id: Uuid) -> ExecutionLoopHandle {
        let (stop_tx, mut stop_rx) = watch::channel(ExecutionLoopCommand::Run);
        let join_handle: JoinHandle<ExecutionLoopExit> = tokio::spawn(async move {
            tokio::select! {
                _ = stop_rx.changed() => ExecutionLoopExit {
                    note: Some("test loop stopped".to_string()),
                    leadership_release_outcome: None,
                },
                _ = tokio::time::sleep(std::time::Duration::from_secs(86_400)) => ExecutionLoopExit {
                    note: None,
                    leadership_release_outcome: None,
                },
            }
        });
        ExecutionLoopHandle {
            run_id,
            stop_tx,
            join_handle,
        }
    }

    fn sentinel_execution_snapshot(run_id: Uuid) -> mqk_runtime::observability::ExecutionSnapshot {
        mqk_runtime::observability::ExecutionSnapshot {
            run_id: Some(run_id),
            active_orders: vec![],
            pending_outbox: vec![],
            recent_inbox_events: vec![],
            portfolio: mqk_runtime::observability::PortfolioSnapshot {
                cash_micros: 0,
                realized_pnl_micros: 0,
                positions: vec![],
            },
            system_block_state: None,
            recent_risk_denials: vec![],
            has_recent_terminal_fill: false,
            risk_engine_sticky_halt: mqk_execution::RiskEngineHaltStatus::Unavailable,
            snapshot_at_utc: Utc::now(),
        }
    }

    // -----------------------------------------------------------------
    // Test 1: Idle -> Reserved -> Starting -> Active.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn idle_reserved_starting_active_transitions() {
        let state = fresh_state();
        let run_id = run_id_for("t1");

        assert!(matches!(
            *state.runtime_ownership.lock().await,
            LocalRuntimeOwnership::Idle
        ));

        state
            .reserve_runtime_ownership(run_id)
            .await
            .expect("Idle -> Reserved must succeed");
        assert!(matches!(
            *state.runtime_ownership.lock().await,
            LocalRuntimeOwnership::Reserved { run_id: r } if r == run_id
        ));
        assert!(
            state.locally_owned_run_id().await.is_none(),
            "Reserved must never report running"
        );

        let bundle = RunStartLocalBundle {
            execution_snapshot: None,
            accepted_artifact: None,
            native_strategy_bootstrap: None,
            dynamic_selection_outcome: None,
        };
        let metadata = state
            .prepare_starting_metadata_and_mirrors(run_id, bundle, Vec::new(), "test")
            .await
            .expect("Reserved -> Starting must succeed");
        assert_eq!(metadata.run_id, run_id);
        assert!(matches!(
            *state.runtime_ownership.lock().await,
            LocalRuntimeOwnership::Starting { run_id: r, .. } if r == run_id
        ));
        assert!(
            state.locally_owned_run_id().await.is_none(),
            "Starting must never report running"
        );

        let handle = fake_handle(run_id);
        state
            .install_active_runtime(run_id, handle)
            .await
            .expect("Starting -> Active must succeed");
        assert!(matches!(
            *state.runtime_ownership.lock().await,
            LocalRuntimeOwnership::Active { run_id: r, .. } if r == run_id
        ));
        assert_eq!(
            state.locally_owned_run_id().await,
            Some(run_id),
            "Active must report running"
        );

        // Cleanup: stop the fake loop directly (bypassing the DB-dependent
        // stop_execution_runtime path this hermetic test does not need).
        state
            .clear_local_runtime_for_run(run_id, LifecycleClearReason::OperatorStop)
            .await;
    }

    // -----------------------------------------------------------------
    // Test 2: duplicate reservation preserves owner.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn duplicate_reservation_preserves_owner() {
        let state = fresh_state();
        let run_id = run_id_for("t2");

        state.reserve_runtime_ownership(run_id).await.unwrap();
        let err = state
            .reserve_runtime_ownership(run_id)
            .await
            .expect_err("a second reservation for the same run_id must conflict");
        assert_eq!(err, run_id);
        assert!(matches!(
            *state.runtime_ownership.lock().await,
            LocalRuntimeOwnership::Reserved { run_id: r } if r == run_id
        ));

        // A reservation attempt for a *different* run_id must also conflict,
        // reporting the existing owner, and change nothing.
        let other_run_id = run_id_for("t2-other");
        let err2 = state
            .reserve_runtime_ownership(other_run_id)
            .await
            .expect_err("a conflicting run_id must also be refused");
        assert_eq!(err2, run_id);
        assert!(matches!(
            *state.runtime_ownership.lock().await,
            LocalRuntimeOwnership::Reserved { run_id: r } if r == run_id
        ));
    }

    // -----------------------------------------------------------------
    // Test 3 / 14: wrong-run install/clear refuses without mutation — "run A
    // cannot clear B".
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn wrong_run_install_and_clear_refuse_without_mutation() {
        let state = fresh_state();
        let run_a = run_id_for("t3-run-a");
        let run_b = run_id_for("t3-run-b");

        state.reserve_runtime_ownership(run_a).await.unwrap();
        let bundle = RunStartLocalBundle {
            execution_snapshot: None,
            accepted_artifact: None,
            native_strategy_bootstrap: None,
            dynamic_selection_outcome: None,
        };
        state
            .prepare_starting_metadata_and_mirrors(run_a, bundle, Vec::new(), "test")
            .await
            .unwrap();

        // Installing a handle for a *different* run_id (B) while A is
        // Starting must refuse and leave A's Starting binding untouched —
        // the mismatched handle is stopped+joined by the install call
        // itself (no detached task), not left dangling.
        let handle_b = fake_handle(run_b);
        let err = state
            .install_active_runtime(run_b, handle_b)
            .await
            .expect_err("installing for a different run_id must be refused");
        assert!(matches!(
            err,
            InstallActiveRuntimeError::StartingForDifferentRun {
                starting_run_id,
                ..
            } if starting_run_id == run_a
        ));
        assert!(matches!(
            *state.runtime_ownership.lock().await,
            LocalRuntimeOwnership::Starting { run_id: r, .. } if r == run_a
        ));

        // Installing A's own handle now succeeds — proves the failed B
        // install above left A's reservation completely intact.
        let handle_a = fake_handle(run_a);
        state.install_active_runtime(run_a, handle_a).await.unwrap();
        assert_eq!(state.locally_owned_run_id().await, Some(run_a));

        // clear_local_runtime_for_run(run_b, ..) must refuse to touch A's
        // Active state — "run A cannot clear B" (requirement 4).
        let outcome = state
            .clear_local_runtime_for_run(run_b, LifecycleClearReason::FailedStart)
            .await;
        assert!(!outcome.cleared, "run B must never clear run A's ownership");
        assert_eq!(
            state.locally_owned_run_id().await,
            Some(run_a),
            "run A's Active ownership must be completely unchanged"
        );

        // The matching run_id does clear it.
        let outcome_a = state
            .clear_local_runtime_for_run(run_a, LifecycleClearReason::OperatorStop)
            .await;
        assert!(outcome_a.cleared);
        assert!(state.locally_owned_run_id().await.is_none());
    }

    // -----------------------------------------------------------------
    // Test 15: reservation conflict preserves complete sentinel state —
    // every economic mirror and the existing owner's ownership metadata are
    // exactly as they were before the conflicting attempt.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn conflict_preserves_complete_sentinel_state() {
        let state = fresh_state();
        let run_a = run_id_for("t15-run-a");
        let run_b = run_id_for("t15-run-b");

        // Establish A as a full Active owner with sentinel values in every
        // economic mirror this patch's cleanup authority touches.
        let sentinel_artifact = AcceptedArtifactProvenance {
            artifact_id: "sentinel-artifact".to_string(),
            artifact_type: "sentinel".to_string(),
            stage: "sentinel".to_string(),
            produced_by: "core_state_machine_test".to_string(),
        };
        *state.execution_snapshot.write().await = Some(sentinel_execution_snapshot(run_a));
        *state.accepted_artifact.write().await = Some(sentinel_artifact.clone());
        *state.native_strategy_bootstrap.lock().await = Some(NativeStrategyBootstrap {
            outcome: mqk_runtime::native_strategy::NativeStrategyBootstrapOutcome::Dormant,
        });
        state.day_signal_count.store(4242, Ordering::SeqCst);
        state
            .day_signal_count_by_symbol
            .write()
            .await
            .insert("AAPL".to_string(), 7);
        assert!(state.try_claim_b5_alert_for_test("AAPL").await);
        assert!(state.try_claim_day_limit_alert());
        assert!(
            state
                .try_claim_per_symbol_position_cap_alert_for_test("AAPL")
                .await
        );
        state.set_bar_tick_state_for_test(9, 123, 4);
        state
            .record_per_symbol_target_state(PerSymbolTargetState {
                symbol: "AAPL".to_string(),
                strategy_id: "sentinel-strategy".to_string(),
                current_qty: 1,
                target_qty: 2,
                delta: 1,
                no_order_reason: String::new(),
                last_decision_id: None,
                last_decision_disposition: None,
                updated_at_utc: "2026-01-01T00:00:00Z".to_string(),
            })
            .await;

        state.reserve_runtime_ownership(run_a).await.unwrap();
        let metadata_a = Arc::new(RunStartMetadata {
            run_id: run_a,
            accepted_artifact: Some(sentinel_artifact.clone()),
            native_bootstrap_present: true,
            dynamic_selection: tokio::sync::RwLock::new(None),
            frozen_assignments: Vec::new(),
            frozen_assignments_source: "test_fixture",
            approved_for_live: false,
        });
        *state.runtime_ownership.lock().await = LocalRuntimeOwnership::Active {
            run_id: run_a,
            metadata: metadata_a,
            handle: fake_handle(run_a),
        };

        // A conflicting reservation for run B must be refused and must not
        // touch anything.
        let err = state
            .reserve_runtime_ownership(run_b)
            .await
            .expect_err("run B must be refused while A owns the slot");
        assert_eq!(err, run_a);

        // Every sentinel value must be byte-for-byte unchanged.
        assert_eq!(state.locally_owned_run_id().await, Some(run_a));
        assert_eq!(
            state
                .execution_snapshot
                .read()
                .await
                .as_ref()
                .map(|s| s.run_id),
            Some(Some(run_a))
        );
        assert_eq!(
            state.accepted_artifact.read().await.clone(),
            Some(sentinel_artifact)
        );
        assert!(state.native_strategy_bootstrap.lock().await.is_some());
        assert_eq!(state.day_signal_count.load(Ordering::SeqCst), 4242);
        assert_eq!(
            state
                .day_signal_count_by_symbol
                .read()
                .await
                .get("AAPL")
                .copied(),
            Some(7)
        );
        assert_eq!(state.bar_tick_dispatch_count.load(Ordering::SeqCst), 9);
        assert_eq!(state.last_bar_signal_qty.load(Ordering::SeqCst), 123);
        assert_eq!(state.last_bar_context_bars.load(Ordering::SeqCst), 4);
        assert_eq!(state.per_symbol_target_states().await.len(), 1);

        // Cleanup.
        state
            .clear_local_runtime_for_run(run_a, LifecycleClearReason::OperatorStop)
            .await;
    }

    // -----------------------------------------------------------------
    // Test 16: Reserved/Starting never reported running (status derivation).
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn reserved_and_starting_are_never_reported_running() {
        let state = fresh_state();
        let run_id = run_id_for("t16");

        state.reserve_runtime_ownership(run_id).await.unwrap();
        assert!(state.locally_owned_run_id().await.is_none());

        let bundle = RunStartLocalBundle {
            execution_snapshot: None,
            accepted_artifact: None,
            native_strategy_bootstrap: None,
            dynamic_selection_outcome: None,
        };
        state
            .prepare_starting_metadata_and_mirrors(run_id, bundle, Vec::new(), "test")
            .await
            .unwrap();
        assert!(
            state.locally_owned_run_id().await.is_none(),
            "Starting must never report running, even with metadata committed"
        );

        // A Degraded state must also never report running.
        state
            .clear_local_runtime_for_run(run_id, LifecycleClearReason::FailedStart)
            .await;
        state
            .note_local_runtime_degraded(
                run_id,
                BoundedLifecycleDegradation {
                    operation: "test_op",
                    detail: "test detail".to_string(),
                },
            )
            .await;
        assert!(state.locally_owned_run_id().await.is_none());
        let lock = state.runtime_ownership.lock().await;
        match &*lock {
            LocalRuntimeOwnership::Degraded { run_id: r, detail } => {
                assert_eq!(*r, run_id);
                assert_eq!(detail.operation, "test_op");
                assert_eq!(detail.detail, "test detail");
            }
            other => panic!("expected Degraded, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Test 17: Active status requires matching run_id/metadata/handle.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn active_status_requires_matching_run_metadata_handle() {
        let state = fresh_state();
        let run_id = run_id_for("t17");

        let sentinel_artifact = AcceptedArtifactProvenance {
            artifact_id: "t17-artifact".to_string(),
            artifact_type: "sentinel".to_string(),
            stage: "sentinel".to_string(),
            produced_by: "t17-test".to_string(),
        };
        let frozen = vec![SymbolStrategyAssignment {
            symbol: "AAPL".to_string(),
            strategy_id: "t17-strategy".to_string(),
            timeframe: "1Min".to_string(),
        }];

        state.reserve_runtime_ownership(run_id).await.unwrap();
        let bundle = RunStartLocalBundle {
            execution_snapshot: None,
            accepted_artifact: Some(sentinel_artifact.clone()),
            native_strategy_bootstrap: Some(NativeStrategyBootstrap {
                outcome: mqk_runtime::native_strategy::NativeStrategyBootstrapOutcome::Dormant,
            }),
            dynamic_selection_outcome: None,
        };
        state
            .prepare_starting_metadata_and_mirrors(
                run_id,
                bundle,
                frozen.clone(),
                "t17_test_source",
            )
            .await
            .unwrap();
        state
            .install_active_runtime(run_id, fake_handle(run_id))
            .await
            .unwrap();

        let lock = state.runtime_ownership.lock().await;
        match &*lock {
            LocalRuntimeOwnership::Active {
                run_id: r,
                metadata,
                handle,
            } => {
                // Requirement 2 invariant 8: Active metadata includes run
                // ID, accepted artifact, native bootstrap (presence),
                // dynamic-selection state, frozen assignments/source, and
                // approved_for_live=false — all three of run_id/
                // metadata.run_id/handle.run_id must agree.
                assert_eq!(*r, run_id);
                assert_eq!(metadata.run_id, run_id);
                assert_eq!(handle.run_id, run_id);
                assert_eq!(metadata.accepted_artifact, Some(sentinel_artifact));
                assert!(metadata.native_bootstrap_present);
                assert_eq!(metadata.frozen_assignments, frozen);
                assert_eq!(metadata.frozen_assignments_source, "t17_test_source");
                assert!(!metadata.approved_for_live);
            }
            other => panic!("expected Active, got {other:?}"),
        }
        drop(lock);

        state
            .clear_local_runtime_for_run(run_id, LifecycleClearReason::OperatorStop)
            .await;
    }

    // -----------------------------------------------------------------
    // Test 9: failed-start full mirror cleanup — every listed field/counter/
    // map is inspected.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn clear_local_runtime_for_run_clears_every_economic_mirror() {
        let state = fresh_state();
        let run_id = run_id_for("t9");

        // Plant every listed mirror field with a non-default sentinel.
        *state.execution_snapshot.write().await = Some(sentinel_execution_snapshot(run_id));
        *state.accepted_artifact.write().await = Some(AcceptedArtifactProvenance {
            artifact_id: "a".to_string(),
            artifact_type: "b".to_string(),
            stage: "c".to_string(),
            produced_by: "d".to_string(),
        });
        *state.native_strategy_bootstrap.lock().await = Some(NativeStrategyBootstrap {
            outcome: mqk_runtime::native_strategy::NativeStrategyBootstrapOutcome::Dormant,
        });
        state.day_signal_count.store(99, Ordering::SeqCst);
        state
            .day_signal_count_by_symbol
            .write()
            .await
            .insert("MSFT".to_string(), 3);
        assert!(state.try_claim_b5_alert_for_test("MSFT").await);
        assert!(state.try_claim_day_limit_alert());
        assert!(
            state
                .try_claim_per_symbol_position_cap_alert_for_test("MSFT")
                .await
        );
        state.set_bar_tick_state_for_test(5, 55, 2);
        state
            .record_per_symbol_target_state(PerSymbolTargetState {
                symbol: "MSFT".to_string(),
                strategy_id: "s".to_string(),
                current_qty: 0,
                target_qty: 1,
                delta: 1,
                no_order_reason: String::new(),
                last_decision_id: None,
                last_decision_disposition: None,
                updated_at_utc: "2026-01-01T00:00:00Z".to_string(),
            })
            .await;

        // Establish ownership for run_id (Active, with dynamic-selection
        // metadata) so `clear_local_runtime_for_run` actually matches.
        let dyn_state = DynamicSelectionRuntimeState {
            run_id,
            disposition:
                crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::Off,
            configured_mode: mqk_portfolio::DynamicSelectionMode::Off,
            effective_mode: mqk_portfolio::DynamicSelectionMode::Off,
            live_lock_applied: false,
            plan: None,
            plan_id: None,
            selected_pairs: Vec::new(),
            host_pool_present: false,
            reasons: Vec::new(),
            approved_for_live: false,
            evidence_persisted: false,
            evidence_validation_state: None,
        };
        *state.runtime_ownership.lock().await = LocalRuntimeOwnership::Active {
            run_id,
            metadata: Arc::new(RunStartMetadata {
                run_id,
                accepted_artifact: None,
                native_bootstrap_present: true,
                dynamic_selection: tokio::sync::RwLock::new(Some(dyn_state)),
                frozen_assignments: Vec::new(),
                frozen_assignments_source: "test_fixture",
                approved_for_live: false,
            }),
            handle: fake_handle(run_id),
        };
        assert!(state.dynamic_selection_runtime_snapshot().await.is_some());

        let outcome = state
            .clear_local_runtime_for_run(run_id, LifecycleClearReason::FailedStart)
            .await;
        assert!(outcome.cleared);
        assert!(outcome.stopped_live_handle);
        assert!(outcome.join_error.is_none());

        // Ownership.
        assert!(matches!(
            *state.runtime_ownership.lock().await,
            LocalRuntimeOwnership::Idle
        ));
        assert!(state.dynamic_selection_runtime_snapshot().await.is_none());

        // Every economic mirror.
        assert!(state.execution_snapshot.read().await.is_none());
        assert!(state.accepted_artifact.read().await.is_none());
        assert!(state.native_strategy_bootstrap.lock().await.is_none());
        assert_eq!(state.day_signal_count.load(Ordering::SeqCst), 0);
        assert!(state.day_signal_count_by_symbol.read().await.is_empty());
        assert!(
            state.try_claim_b5_alert_for_test("MSFT").await,
            "b5 alert dedup must be cleared (claim succeeds again)"
        );
        assert!(
            state.try_claim_day_limit_alert(),
            "day-limit alert dedup must be cleared"
        );
        assert!(
            state
                .try_claim_per_symbol_position_cap_alert_for_test("MSFT")
                .await,
            "per-symbol position-cap alert dedup must be cleared"
        );
        assert_eq!(state.bar_tick_dispatch_count.load(Ordering::SeqCst), 0);
        assert_eq!(state.last_bar_signal_qty.load(Ordering::SeqCst), i64::MIN);
        assert_eq!(state.last_bar_context_bars.load(Ordering::SeqCst), -1);
        assert!(state.per_symbol_target_states().await.is_empty());
    }

    // -----------------------------------------------------------------
    // Idempotency: clearing an already-Idle slot is a safe no-op.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn clear_local_runtime_for_run_on_idle_is_idempotent_noop() {
        let state = fresh_state();
        let run_id = run_id_for("t-idempotent");
        let outcome = state
            .clear_local_runtime_for_run(run_id, LifecycleClearReason::FailedStart)
            .await;
        assert!(!outcome.cleared);
        let outcome2 = state
            .clear_local_runtime_for_run(run_id, LifecycleClearReason::FailedStart)
            .await;
        assert!(!outcome2.cleared);
    }

    // -----------------------------------------------------------------
    // Requirement 3 (startup barrier) tests 4/5: spawn the real execution
    // loop task directly (a minimal in-process Paper orchestrator — no
    // network, no credentials, mirrors `run_loop_one_tick_for_test`'s
    // construction) and drive its startup barrier without going through
    // the full reserve/prepare-metadata/install sequence.
    // -----------------------------------------------------------------

    fn spawn_barrier_test_loop(
        state: &Arc<AppState>,
        run_id: Uuid,
        barrier_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> ExecutionLoopHandle {
        use mqk_broker_paper::LockedPaperBroker;
        use mqk_execution::BrokerOrderMap;
        use mqk_portfolio::PortfolioState;
        use mqk_reconcile::{BrokerSnapshot, LocalSnapshot};
        use mqk_runtime::orchestrator::WallClock;
        use mqk_runtime::runtime_risk::RuntimeRiskGate;

        let integrity_gate = types::StateIntegrityGate {
            integrity: Arc::clone(&state.integrity),
        };
        let reconcile_gate = types::ReconcileTruthGate {
            reconcile_status: Arc::clone(&state.reconcile_status),
        };
        let risk_gate =
            RuntimeRiskGate::from_run_config(&serde_json::json!({}), 1_000_000_000_i64);
        let daemon_broker = broker::DaemonBroker::Paper(LockedPaperBroker::default());
        let gateway = mqk_execution::wiring::build_gateway(
            daemon_broker,
            integrity_gate,
            risk_gate,
            reconcile_gate,
        );
        let pool =
            sqlx::PgPool::connect_lazy("postgresql://127.0.0.1:5432/mqk_phase7a_barrier_test_stub")
                .expect("connect_lazy URL parse must succeed");
        let orchestrator = types::DaemonOrchestrator::new(
            pool,
            gateway,
            BrokerOrderMap::new(),
            BTreeMap::new(),
            PortfolioState::new(1_000_000_000_i64),
            run_id,
            "phase7a-barrier-test-dispatcher",
            "phase7a-barrier-test",
            None,
            WallClock,
            Box::new(LocalSnapshot::empty),
            Box::new(|| BrokerSnapshot::empty_at(0)),
        );
        loop_runner::spawn_execution_loop(
            Arc::clone(state),
            orchestrator,
            run_id,
            crate::dynamic_selection_dispatch_authority::RuntimeStrategyDispatchAuthority::Legacy {
                assignments: Vec::new(),
            },
            barrier_rx,
        )
    }

    /// Test 4: the task performs zero economic work before the barrier is
    /// released — proven by never releasing it, confirming the task is
    /// still pending (not finished — it would only finish via one of the
    /// two `select!` arms, neither of which has fired yet), then stopping
    /// it directly and observing the "stopped before startup barrier
    /// release" exit note (never a ticker/deadman/dispatch-related note).
    #[tokio::test]
    async fn barrier_task_performs_zero_economic_work_before_release() {
        let state = fresh_state();
        let run_id = run_id_for("barrier-zero-work");
        let (barrier_tx, barrier_rx) = tokio::sync::oneshot::channel();
        let handle = spawn_barrier_test_loop(&state, run_id, barrier_rx);

        // Give the task ample opportunity to run if it were going to do any
        // economic work (several multiples of a single tokio yield).
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert!(
            !handle.join_handle.is_finished(),
            "the task must still be blocked on the barrier — it must not \
             have proceeded to build a ticker or done any economic work"
        );

        // Never release the barrier — stop the task directly instead,
        // proving it is genuinely waiting in the barrier select, not stuck
        // or already past it.
        let _ = handle.stop_tx.send(ExecutionLoopCommand::Stop);
        let exit = handle
            .join_handle
            .await
            .expect("task must join cleanly, never panic, when stopped before the barrier");
        assert_eq!(
            exit.note.as_deref(),
            Some("execution loop stopped before startup barrier release"),
            "the exit note must prove the stop-before-barrier path fired, \
             not any economic-loop exit path"
        );
        drop(barrier_tx);
    }

    /// Test 5: a cancelled barrier (sender dropped without ever releasing —
    /// e.g. `install_active_runtime`'s error path never sends) makes the
    /// task exit immediately and join cleanly, without needing an explicit
    /// stop signal at all.
    #[tokio::test]
    async fn barrier_cancelled_without_send_exits_and_joins() {
        let state = fresh_state();
        let run_id = run_id_for("barrier-cancelled");
        let (barrier_tx, barrier_rx) = tokio::sync::oneshot::channel();
        let handle = spawn_barrier_test_loop(&state, run_id, barrier_rx);

        // Cancel the barrier by dropping the sender — never send().
        drop(barrier_tx);

        let exit = tokio::time::timeout(std::time::Duration::from_secs(5), handle.join_handle)
            .await
            .expect("a cancelled barrier must let the task exit promptly, not hang")
            .expect("task must join cleanly, never panic, on a cancelled barrier");
        assert_eq!(
            exit.note.as_deref(),
            Some("execution loop cancelled before startup barrier release"),
            "the exit note must prove the barrier-cancelled path fired"
        );
    }

    /// Test 6 (`install_active_runtime` failure path): a handle refused by
    /// `install_active_runtime` (because ownership is `Starting` for a
    /// *different* run_id) is fully stopped and joined by that call itself
    /// — proven here by observing that a shared counter the task would only
    /// increment on exit has in fact been incremented by the time the
    /// install call returns, i.e. no detached task remains running in the
    /// background.
    #[tokio::test]
    async fn install_failure_leaves_no_detached_task() {
        let state = fresh_state();
        let run_a = run_id_for("install-failure-run-a");
        let run_b = run_id_for("install-failure-run-b");

        state.reserve_runtime_ownership(run_a).await.unwrap();
        let bundle = RunStartLocalBundle {
            execution_snapshot: None,
            accepted_artifact: None,
            native_strategy_bootstrap: None,
            dynamic_selection_outcome: None,
        };
        state
            .prepare_starting_metadata_and_mirrors(run_a, bundle, Vec::new(), "test")
            .await
            .unwrap();

        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let exited_clone = Arc::clone(&exited);
        let (stop_tx, mut stop_rx) = watch::channel(ExecutionLoopCommand::Run);
        let join_handle: JoinHandle<ExecutionLoopExit> = tokio::spawn(async move {
            let _ = stop_rx.changed().await;
            exited_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            ExecutionLoopExit {
                note: Some("test loop stopped".to_string()),
                leadership_release_outcome: None,
            }
        });
        let handle_b = ExecutionLoopHandle {
            run_id: run_b,
            stop_tx,
            join_handle,
        };

        let err = state
            .install_active_runtime(run_b, handle_b)
            .await
            .expect_err("installing for run_b while run_a is Starting must be refused");
        assert!(matches!(
            err,
            InstallActiveRuntimeError::StartingForDifferentRun { .. }
        ));
        // By the time `install_active_runtime` returned, it must already
        // have stopped and joined `handle_b` — no detached task.
        assert!(
            exited.load(std::sync::atomic::Ordering::SeqCst),
            "install_active_runtime must stop and join a refused handle \
             before returning, leaving no detached task"
        );

        state
            .clear_local_runtime_for_run(run_a, LifecycleClearReason::FailedStart)
            .await;
    }
}

// ---------------------------------------------------------------------------
// PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE Part 10: selected-host
// dispatch closure tests.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod phase7b_selected_host_dispatch_tests {
    use super::*;
    use crate::dynamic_selection_dispatch_authority::SelectedDispatchBinding;
    use crate::dynamic_selection_host_pool::DynamicSelectionHostPool;
    use mqk_strategy::{
        IntentMode, StrategyBarResult, StrategyIntents, StrategyOutput, StrategySpec,
        TargetPosition,
    };
    use sqlx::PgPool;

    fn test_plan_id() -> Uuid {
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.test.plan")
    }

    fn test_run_id() -> Uuid {
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.test.run")
    }

    fn binding(
        symbol: &str,
        strategy_id: &str,
        timeframe_secs: i64,
        label: &str,
    ) -> SelectedDispatchBinding {
        SelectedDispatchBinding {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe_secs,
            db_timeframe_label: label.to_string(),
            selection_reason_code: "test_fixture".to_string(),
            plan_id: test_plan_id(),
        }
    }

    fn bar_result(
        spec_name: &str,
        timeframe_secs: i64,
        target_symbol: &str,
        qty: i64,
    ) -> StrategyBarResult {
        StrategyBarResult {
            spec: StrategySpec::new(spec_name, timeframe_secs),
            semantic_fingerprint: format!("test-fixture:{spec_name}:{timeframe_secs}"),
            intents: StrategyIntents {
                mode: IntentMode::Live,
                output: StrategyOutput::new(vec![TargetPosition {
                    symbol: target_symbol.to_string(),
                    qty,
                }]),
            },
        }
    }

    // -----------------------------------------------------------------
    // Part 5: pure coherence-check tests (no DB, no host pool needed).
    // -----------------------------------------------------------------

    #[test]
    fn coherent_result_passes() {
        let b = binding("AAPL", "intraday_scalper", 300, "5m");
        let r = bar_result("intraday_scalper", 300, "AAPL", 10);
        assert!(check_selected_host_result_coherence(&b, &r).is_ok());
    }

    #[test]
    fn spec_name_mismatch_is_rejected() {
        let b = binding("AAPL", "intraday_scalper", 300, "5m");
        let r = bar_result("swing_momentum", 300, "AAPL", 10);
        let err = check_selected_host_result_coherence(&b, &r).unwrap_err();
        assert!(matches!(
            err,
            SelectedHostDispatchFault::SpecNameMismatch { .. }
        ));
        assert_eq!(err.code(), "selected_host_spec_name_mismatch");
    }

    #[test]
    fn spec_timeframe_mismatch_is_rejected() {
        let b = binding("AAPL", "intraday_scalper", 300, "5m");
        let r = bar_result("intraday_scalper", 3600, "AAPL", 10);
        let err = check_selected_host_result_coherence(&b, &r).unwrap_err();
        assert!(matches!(
            err,
            SelectedHostDispatchFault::SpecTimeframeMismatch { .. }
        ));
    }

    #[test]
    fn target_symbol_mismatch_is_rejected() {
        let b = binding("AAPL", "intraday_scalper", 300, "5m");
        let r = bar_result("intraday_scalper", 300, "MSFT", 10);
        let err = check_selected_host_result_coherence(&b, &r).unwrap_err();
        assert!(matches!(
            err,
            SelectedHostDispatchFault::TargetSymbolMismatch { .. }
        ));
    }

    #[test]
    fn target_symbol_match_is_case_insensitive_and_trimmed() {
        let b = binding("AAPL", "intraday_scalper", 300, "5m");
        let r = bar_result("intraday_scalper", 300, " aapl ", 10);
        assert!(check_selected_host_result_coherence(&b, &r).is_ok());
    }

    // -----------------------------------------------------------------
    // DB-backed dispatch tests. Port 5434 local test DB only; skipped
    // (never a hard failure) when unavailable, matching this crate's
    // established `db_pool_or_skip` convention.
    // -----------------------------------------------------------------

    async fn db_pool_or_skip(label: &str) -> Option<PgPool> {
        let Ok(url) = std::env::var("MQK_DATABASE_URL") else {
            eprintln!("{label}: MQK_DATABASE_URL not set; skipped");
            return None;
        };
        if !url.contains(":5434") {
            eprintln!("{label}: MQK_DATABASE_URL must be the port-5434 local test DB; skipped");
            return None;
        }
        let pool = match sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(&url)
            .await
        {
            Ok(pool) => pool,
            Err(e) => {
                eprintln!("{label}: could not connect to MQK_DATABASE_URL: {e}; skipped");
                return None;
            }
        };
        if let Err(e) = mqk_db::migrate(&pool).await {
            eprintln!("{label}: mqk_db::migrate failed: {e}; skipped");
            return None;
        }
        Some(pool)
    }

    async fn seed_bar(
        pool: &PgPool,
        symbol: &str,
        timeframe: &str,
        end_ts: i64,
        close_micros: i64,
    ) {
        sqlx::query(
            r#"
            insert into md_bars (
              symbol, timeframe, end_ts, open_micros, high_micros, low_micros,
              close_micros, volume, is_complete, provider_id, provider_source,
              provider_symbol, ingest_mode, ingested_at
            ) values ($1,$2,$3,$4,$4,$4,$4,1000,true,
                      'phase7b_test','phase7b_test',$1,'historical_sync',now())
            on conflict do nothing
            "#,
        )
        .bind(symbol)
        .bind(timeframe)
        .bind(end_ts)
        .bind(close_micros)
        .execute(pool)
        .await
        .expect("seed bar insert failed");
    }

    async fn cleanup_bars(pool: &PgPool, symbol: &str) {
        let _ =
            sqlx::query("delete from md_bars where symbol = $1 and provider_id = 'phase7b_test'")
                .bind(symbol)
                .execute(pool)
                .await;
    }

    fn hermetic_state_with_db(pool: &PgPool) -> Arc<AppState> {
        let mut state =
            AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Paper);
        state.db = Some(pool.clone());
        Arc::new(state)
    }

    fn recent_bar_ts() -> i64 {
        Utc::now().timestamp() - 60
    }

    /// Test 7: two selected symbols with different strategies each invoke
    /// the exact host once.
    #[tokio::test]
    async fn two_selected_symbols_different_strategies_each_invoked_once() {
        let Some(pool) = db_pool_or_skip("PHASE7B-07").await else {
            return;
        };
        let ts = recent_bar_ts();
        seed_bar(&pool, "PHASE7BAAPL", "5m", ts, 100_000_000).await;
        seed_bar(&pool, "PHASE7BMSFT", "1H", ts, 200_000_000).await;

        let keys = vec![
            (
                "PHASE7BAAPL".to_string(),
                "intraday_scalper".to_string(),
                300,
            ),
            (
                "PHASE7BMSFT".to_string(),
                "volatility_breakout".to_string(),
                3600,
            ),
        ];
        let mut host_pool = DynamicSelectionHostPool::build(&keys).expect("pool builds");
        let bindings = vec![
            binding("PHASE7BAAPL", "intraday_scalper", 300, "5m"),
            binding("PHASE7BMSFT", "volatility_breakout", 3600, "1H"),
        ];

        let state = hermetic_state_with_db(&pool);
        state
            .deposit_strategy_bar_input(StrategyBarInput {
                now_tick: 1,
                end_ts: ts,
                limit_price: Some(100_000_000),
                qty: 0,
            })
            .await;

        let results = state
            .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                test_run_id(),
                &bindings,
                &mut host_pool,
            )
            .await
            .expect("dispatch must not fault on a coherent selected batch");

        assert_eq!(
            results.len(),
            2,
            "each selected binding must produce a result"
        );
        let aapl = results
            .iter()
            .find(|(a, _, _)| a.symbol == "PHASE7BAAPL")
            .expect("AAPL result present");
        assert_eq!(aapl.1.spec.name, "intraday_scalper");
        assert_eq!(aapl.2.as_ref().unwrap().timeframe, "5m");
        let msft = results
            .iter()
            .find(|(a, _, _)| a.symbol == "PHASE7BMSFT")
            .expect("MSFT result present");
        assert_eq!(msft.1.spec.name, "volatility_breakout");
        assert_eq!(msft.2.as_ref().unwrap().timeframe, "1H");

        cleanup_bars(&pool, "PHASE7BAAPL").await;
        cleanup_bars(&pool, "PHASE7BMSFT").await;
    }

    // -----------------------------------------------------------------
    // A1-T7: selected-host path panic assessment.
    //
    // `DynamicSelectionHostPool` keys one isolated `StrategyHost` per
    // (symbol, strategy_id, timeframe_secs) -- a panic evaluating one
    // binding's host cannot corrupt any other binding's host object. But
    // this backend's own frozen Part 5 contract already treats an ordinary
    // `on_bar` `Err` as a whole-tick structural fault ("zero decisions for
    // the whole tick and halt"), never a per-symbol-only skip. A panic is
    // at least as severe a malfunction signal as a typed Err from the same
    // call, so it must get the same whole-tick-halt treatment, not the
    // legacy loop's "continue to siblings" behavior -- this is the
    // justified authority distinction the A1 mission allows. This test
    // proves the panic is contained (never escapes as a raw unwind) and
    // converted into the same class of fault (`HostOnBarPanicked`, treated
    // identically to `HostOnBarError` by every caller) that a genuine
    // `on_bar` error would already produce.
    // -----------------------------------------------------------------

    /// A1-T7: a panic inside `host.on_bar` for one binding must not escape
    /// as a raw unwind -- it is caught and converted into
    /// `SelectedHostDispatchFault::HostOnBarPanicked`, which the caller
    /// (`loop_runner.rs`) treats exactly like `HostOnBarError`: zero
    /// decisions, whole-tick halt. A sibling binding earlier in `bindings`
    /// still produces no result either (the whole tick is refused), which
    /// is the correct, deliberate behavior for this backend -- distinct
    /// from the legacy loop's per-symbol continuation.
    #[tokio::test]
    async fn a1_t7_selected_host_panic_is_contained_and_halts_whole_tick() {
        let Some(pool) = db_pool_or_skip("PHASE7B-A1T7").await else {
            return;
        };
        let ts = recent_bar_ts();
        seed_bar(&pool, "PHASE7BA1T7AAPL", "5m", ts, 100_000_000).await;
        seed_bar(&pool, "PHASE7BA1T7MSFT", "5m", ts, 200_000_000).await;

        let keys = vec![
            (
                "PHASE7BA1T7AAPL".to_string(),
                "intraday_scalper".to_string(),
                300,
            ),
            (
                "PHASE7BA1T7MSFT".to_string(),
                "intraday_scalper".to_string(),
                300,
            ),
        ];
        let mut host_pool = DynamicSelectionHostPool::build(&keys).expect("pool builds");
        let bindings = vec![
            binding("PHASE7BA1T7AAPL", "intraday_scalper", 300, "5m"),
            binding("PHASE7BA1T7MSFT", "intraday_scalper", 300, "5m"),
        ];

        let state = hermetic_state_with_db(&pool);
        state
            .set_panic_on_symbol_for_test(Some("PHASE7BA1T7MSFT".to_string()))
            .await;
        state
            .deposit_strategy_bar_input(StrategyBarInput {
                now_tick: 1,
                end_ts: ts,
                limit_price: Some(100_000_000),
                qty: 0,
            })
            .await;

        let err = state
            .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                test_run_id(),
                &bindings,
                &mut host_pool,
            )
            .await
            .expect_err("a panicking host must fail closed, not silently skip or produce a result");

        match &err {
            SelectedHostDispatchFault::HostOnBarPanicked {
                symbol,
                strategy_id,
                ..
            } => {
                assert_eq!(symbol, "PHASE7BA1T7MSFT");
                assert_eq!(strategy_id, "intraday_scalper");
            }
            other => panic!("expected HostOnBarPanicked, got {other:?}"),
        }
        assert_eq!(err.code(), "selected_host_on_bar_panicked");

        cleanup_bars(&pool, "PHASE7BA1T7AAPL").await;
        cleanup_bars(&pool, "PHASE7BA1T7MSFT").await;
    }

    // -----------------------------------------------------------------
    // A1-R7: legacy multi-symbol dispatch, real on_bar panic, durable
    // fault evidence via the production journal path.
    // -----------------------------------------------------------------

    struct AlwaysPanicOnBarStrategy {
        spec: StrategySpec,
    }

    impl mqk_strategy::Strategy for AlwaysPanicOnBarStrategy {
        fn spec(&self) -> StrategySpec {
            self.spec.clone()
        }

        fn on_bar(&mut self, _ctx: &mqk_strategy::StrategyContext) -> StrategyOutput {
            panic!("A1_REAL_ON_BAR_PANIC_LEGACY_DB_TEST");
        }
    }

    /// A real, Active bootstrap whose host unconditionally panics on
    /// `on_bar` -- used to prove durable fault evidence through the legacy
    /// (DB-loaded) dispatch path.
    fn legacy_panic_probe_bootstrap() -> NativeStrategyBootstrap {
        let mut host = mqk_strategy::StrategyHost::new(mqk_strategy::ShadowMode::Off);
        host.register(Box::new(AlwaysPanicOnBarStrategy {
            spec: StrategySpec::new("a1_legacy_panic_probe", 300),
        }))
        .expect("host registration must succeed");
        NativeStrategyBootstrap {
            outcome: mqk_runtime::native_strategy::NativeStrategyBootstrapOutcome::Active {
                host,
                strategy_id: "a1_legacy_panic_probe".to_string(),
            },
        }
    }

    /// A1-R7: when DB-backed journal infrastructure is available and a real
    /// `Strategy::on_bar` callback panics via the legacy multi-symbol
    /// dispatch path (`tick_strategy_dispatch_multi_symbol_with_bar_facts`
    /// -> `dispatch_native_strategy_for_symbol_with_loaded_bars_and_facts`
    /// -> `AppState::invoke_native_strategy_host_on_bar`), durable fault
    /// evidence is persisted through the production
    /// `strategy_signal_evaluations` journal path -- not merely a log line
    /// or an in-memory counter.
    #[tokio::test]
    async fn a1_r7_real_on_bar_panic_persists_durable_fault_evidence_via_legacy_path() {
        let Some(pool) = db_pool_or_skip("PHASE7B-A1R7").await else {
            return;
        };
        let ts = recent_bar_ts();
        seed_bar(&pool, "PHASE7BA1R7SYM", "5m", ts, 100_000_000).await;
        cleanup_signal_evaluations(&pool, "PHASE7BA1R7SYM").await;

        let state = hermetic_state_with_db(&pool);
        state
            .set_native_strategy_bootstrap_for_test(Some(legacy_panic_probe_bootstrap()))
            .await;
        state
            .deposit_strategy_bar_input(StrategyBarInput {
                now_tick: 42,
                end_ts: ts,
                limit_price: Some(100_000_000),
                qty: 0,
            })
            .await;

        let assignments = vec![SymbolStrategyAssignment {
            symbol: "PHASE7BA1R7SYM".to_string(),
            strategy_id: "a1_legacy_panic_probe".to_string(),
            timeframe: "5m".to_string(),
        }];
        let results = state
            .tick_strategy_dispatch_multi_symbol_with_bar_facts(&assignments)
            .await;
        assert!(
            results.is_empty(),
            "A1-R7: the panicking symbol must produce no result"
        );

        let rows = mqk_db::fetch_recent_strategy_signal_evaluations(&pool, 50)
            .await
            .expect("fetch_recent_strategy_signal_evaluations failed");
        let row = rows
            .iter()
            .find(|r| r.symbol == "PHASE7BA1R7SYM")
            .expect("A1-R7: a durable journal row must exist for the panicking symbol");
        assert_eq!(row.reason_code, "strategy_dispatch_panicked");
        assert_eq!(row.strategy_id, "a1_legacy_panic_probe");
        assert!(!row.signal_generated);
        assert_eq!(row.signal_qty, None);

        assert_eq!(
            state.native_strategy_bootstrap_truth_state_for_test().await,
            Some("failed"),
            "A1-R7: the shared host must be quarantined (Failed) after the real on_bar panic"
        );

        cleanup_bars(&pool, "PHASE7BA1R7SYM").await;
        cleanup_signal_evaluations(&pool, "PHASE7BA1R7SYM").await;
    }

    // -----------------------------------------------------------------
    // TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 Blocker 2: selected-host
    // signal-journal rows must be bound to the exact selected authority
    // (run_id from the frozen dispatch authority, strategy_id from the
    // exact selected binding) — never `native_strategy_bootstrap` or the
    // mutable `status.active_run_id` cache.
    // -----------------------------------------------------------------

    async fn cleanup_signal_evaluations(pool: &PgPool, symbol: &str) {
        let _ = sqlx::query("delete from strategy_signal_evaluations where symbol = $1")
            .bind(symbol)
            .execute(pool)
            .await;
    }

    /// A native bootstrap deliberately different from every selected
    /// binding's strategy_id — proves selected-host journal rows never fall
    /// back to it.
    async fn legacy_bootstrap_with_different_strategy() -> NativeStrategyBootstrap {
        let registry = mqk_runtime::native_strategy::build_daemon_plugin_registry();
        let ids = vec!["swing_momentum".to_string()];
        NativeStrategyBootstrap::bootstrap(Some(&ids), &registry)
    }

    /// Blocker 2 requirement 6: two selected bindings using different
    /// strategies, with the legacy bootstrap set to a third, unrelated
    /// strategy. Every inserted row must carry the exact selected
    /// strategy_id/symbol/timeframe/run_id for its own binding — never the
    /// legacy bootstrap's `swing_momentum`.
    #[tokio::test]
    async fn selected_host_journal_rows_use_exact_selected_authority_not_legacy_bootstrap() {
        let Some(pool) = db_pool_or_skip("PHASE7B-BLOCKER2-01").await else {
            return;
        };
        let ts = recent_bar_ts();
        seed_bar(&pool, "PHASE7BB2AAPL", "5m", ts, 100_000_000).await;
        seed_bar(&pool, "PHASE7BB2MSFT", "1H", ts, 200_000_000).await;
        cleanup_signal_evaluations(&pool, "PHASE7BB2AAPL").await;
        cleanup_signal_evaluations(&pool, "PHASE7BB2MSFT").await;

        let keys = vec![
            (
                "PHASE7BB2AAPL".to_string(),
                "intraday_scalper".to_string(),
                300,
            ),
            (
                "PHASE7BB2MSFT".to_string(),
                "volatility_breakout".to_string(),
                3600,
            ),
        ];
        let mut host_pool = DynamicSelectionHostPool::build(&keys).expect("pool builds");
        let bindings = vec![
            binding("PHASE7BB2AAPL", "intraday_scalper", 300, "5m"),
            binding("PHASE7BB2MSFT", "volatility_breakout", 3600, "1H"),
        ];

        let state = hermetic_state_with_db(&pool);
        // The legacy bootstrap names a strategy that is NOT selected for
        // either binding -- if the journal writer ever consulted it, these
        // rows would wrongly carry "swing_momentum".
        state
            .set_native_strategy_bootstrap_for_test(Some(
                legacy_bootstrap_with_different_strategy().await,
            ))
            .await;
        state
            .deposit_strategy_bar_input(StrategyBarInput {
                now_tick: 1,
                end_ts: ts,
                limit_price: Some(100_000_000),
                qty: 0,
            })
            .await;

        let authority_run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.blocker2.run_id");
        // native_strategy_bootstrap_truth_state_for_test / status.active_run_id
        // are deliberately left at their defaults (no active run established)
        // -- proves the journal row's run_id came from the explicit
        // authority parameter, not from `status.active_run_id`.
        let results = state
            .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                authority_run_id,
                &bindings,
                &mut host_pool,
            )
            .await
            .expect("dispatch must not fault on a coherent selected batch");
        assert_eq!(results.len(), 2, "both bindings must produce a result");

        let rows = mqk_db::fetch_recent_strategy_signal_evaluations(&pool, 50)
            .await
            .expect("fetch_recent_strategy_signal_evaluations failed");

        let aapl_row = rows
            .iter()
            .find(|r| r.symbol == "PHASE7BB2AAPL")
            .expect("AAPL journal row must exist");
        assert_eq!(
            aapl_row.strategy_id, "intraday_scalper",
            "AAPL row must carry its own selected strategy_id, not the legacy bootstrap's"
        );
        assert_ne!(
            aapl_row.strategy_id, "swing_momentum",
            "AAPL row must never fall back to the legacy bootstrap strategy"
        );
        assert_eq!(aapl_row.timeframe, "5m");
        assert_eq!(aapl_row.run_id, Some(authority_run_id));

        let msft_row = rows
            .iter()
            .find(|r| r.symbol == "PHASE7BB2MSFT")
            .expect("MSFT journal row must exist");
        assert_eq!(
            msft_row.strategy_id, "volatility_breakout",
            "MSFT row must carry its own selected strategy_id, not the legacy bootstrap's"
        );
        assert_ne!(
            msft_row.strategy_id, "swing_momentum",
            "MSFT row must never fall back to the legacy bootstrap strategy"
        );
        assert_eq!(msft_row.timeframe, "1H");
        assert_eq!(msft_row.run_id, Some(authority_run_id));

        cleanup_bars(&pool, "PHASE7BB2AAPL").await;
        cleanup_bars(&pool, "PHASE7BB2MSFT").await;
    }

    /// Blocker 2 requirement 6 (idempotency half): replaying the exact same
    /// selected-host dispatch (same bar, same `now_tick`, same bindings, same
    /// run_id) a second time must not create duplicate rows -- the
    /// deterministic `evaluation_id` recipe is unchanged by this repair.
    #[tokio::test]
    async fn selected_host_journal_replay_with_same_bar_is_idempotent() {
        let Some(pool) = db_pool_or_skip("PHASE7B-BLOCKER2-02").await else {
            return;
        };
        let ts = recent_bar_ts();
        seed_bar(&pool, "PHASE7BB2IDEM", "5m", ts, 100_000_000).await;
        cleanup_signal_evaluations(&pool, "PHASE7BB2IDEM").await;

        let keys = vec![(
            "PHASE7BB2IDEM".to_string(),
            "intraday_scalper".to_string(),
            300,
        )];
        let binding_row = binding("PHASE7BB2IDEM", "intraday_scalper", 300, "5m");
        let authority_run_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"phase7b.blocker2.idem.run_id");
        let state = hermetic_state_with_db(&pool);

        // First dispatch.
        let mut host_pool_1 = DynamicSelectionHostPool::build(&keys).expect("pool builds");
        state
            .deposit_strategy_bar_input(StrategyBarInput {
                now_tick: 7,
                end_ts: ts,
                limit_price: Some(100_000_000),
                qty: 0,
            })
            .await;
        state
            .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                authority_run_id,
                std::slice::from_ref(&binding_row),
                &mut host_pool_1,
            )
            .await
            .expect("first dispatch must not fault");

        // Second dispatch: identical now_tick/run_id/binding -- the
        // deterministic evaluation_id must collide, making the second
        // insert a no-op (ON CONFLICT DO NOTHING), not a second row.
        let mut host_pool_2 = DynamicSelectionHostPool::build(&keys).expect("pool builds");
        state
            .deposit_strategy_bar_input(StrategyBarInput {
                now_tick: 7,
                end_ts: ts,
                limit_price: Some(100_000_000),
                qty: 0,
            })
            .await;
        state
            .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                authority_run_id,
                std::slice::from_ref(&binding_row),
                &mut host_pool_2,
            )
            .await
            .expect("second (replayed) dispatch must not fault");

        let rows: Vec<_> = mqk_db::fetch_recent_strategy_signal_evaluations(&pool, 50)
            .await
            .expect("fetch_recent_strategy_signal_evaluations failed")
            .into_iter()
            .filter(|r| r.symbol == "PHASE7BB2IDEM")
            .collect();
        assert_eq!(
            rows.len(),
            1,
            "replaying the identical logical tick must be idempotent (exactly one row)"
        );
        assert_eq!(rows[0].run_id, Some(authority_run_id));
        assert_eq!(rows[0].strategy_id, "intraday_scalper");

        cleanup_bars(&pool, "PHASE7BB2IDEM").await;
    }

    /// Test 8: the same strategy selected for two symbols uses two isolated
    /// host instances and correct target symbols (no cross-symbol leakage).
    #[tokio::test]
    async fn same_strategy_two_symbols_uses_isolated_hosts() {
        let Some(pool) = db_pool_or_skip("PHASE7B-08").await else {
            return;
        };
        let ts = recent_bar_ts();
        seed_bar(&pool, "PHASE7BAAA", "5m", ts, 100_000_000).await;
        seed_bar(&pool, "PHASE7BBBB", "5m", ts, 100_000_000).await;

        let keys = vec![
            (
                "PHASE7BAAA".to_string(),
                "intraday_scalper".to_string(),
                300,
            ),
            (
                "PHASE7BBBB".to_string(),
                "intraday_scalper".to_string(),
                300,
            ),
        ];
        let mut host_pool = DynamicSelectionHostPool::build(&keys).expect("pool builds");
        assert_eq!(
            host_pool.len(),
            2,
            "one isolated host per symbol, same strategy"
        );
        let bindings = vec![
            binding("PHASE7BAAA", "intraday_scalper", 300, "5m"),
            binding("PHASE7BBBB", "intraday_scalper", 300, "5m"),
        ];

        let state = hermetic_state_with_db(&pool);
        state
            .deposit_strategy_bar_input(StrategyBarInput {
                now_tick: 1,
                end_ts: ts,
                limit_price: Some(100_000_000),
                qty: 0,
            })
            .await;

        let results = state
            .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                test_run_id(),
                &bindings,
                &mut host_pool,
            )
            .await
            .expect("dispatch must not fault");
        assert_eq!(results.len(), 2);
        for (assignment, result, facts) in &results {
            // Host isolation proof: every target this symbol's host emitted
            // belongs to that exact symbol -- never the other symbol's.
            for t in &result.intents.output.targets {
                assert_eq!(
                    t.symbol, assignment.symbol,
                    "a shared/leaked host would emit the wrong symbol's targets"
                );
            }
            assert_eq!(facts.as_ref().unwrap().symbol, assignment.symbol);
        }

        cleanup_bars(&pool, "PHASE7BAAA").await;
        cleanup_bars(&pool, "PHASE7BBBB").await;
    }

    /// Test 9: a mixed 5m + 1h selected batch uses exact per-binding bar
    /// windows and truthful per-symbol timeframe facts.
    #[tokio::test]
    async fn mixed_5m_and_1h_batch_uses_exact_per_binding_windows() {
        let Some(pool) = db_pool_or_skip("PHASE7B-09").await else {
            return;
        };
        let ts = recent_bar_ts();
        seed_bar(&pool, "PHASE7BMIX5", "5m", ts, 100_000_000).await;
        seed_bar(&pool, "PHASE7BMIX1", "1H", ts, 300_000_000).await;

        let keys = vec![
            (
                "PHASE7BMIX5".to_string(),
                "intraday_scalper".to_string(),
                300,
            ),
            (
                "PHASE7BMIX1".to_string(),
                "volatility_breakout".to_string(),
                3600,
            ),
        ];
        let mut host_pool = DynamicSelectionHostPool::build(&keys).expect("pool builds");
        let bindings = vec![
            binding("PHASE7BMIX5", "intraday_scalper", 300, "5m"),
            binding("PHASE7BMIX1", "volatility_breakout", 3600, "1H"),
        ];

        let state = hermetic_state_with_db(&pool);
        state
            .deposit_strategy_bar_input(StrategyBarInput {
                now_tick: 1,
                end_ts: ts,
                limit_price: Some(100_000_000),
                qty: 0,
            })
            .await;

        let results = state
            .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                test_run_id(),
                &bindings,
                &mut host_pool,
            )
            .await
            .expect("mixed-timeframe dispatch must not fault");
        assert_eq!(results.len(), 2);
        let five_m = results
            .iter()
            .find(|(a, _, _)| a.symbol == "PHASE7BMIX5")
            .unwrap();
        assert_eq!(five_m.2.as_ref().unwrap().timeframe, "5m");
        assert_eq!(five_m.2.as_ref().unwrap().close_micros, 100_000_000);
        let one_h = results
            .iter()
            .find(|(a, _, _)| a.symbol == "PHASE7BMIX1")
            .unwrap();
        assert_eq!(one_h.2.as_ref().unwrap().timeframe, "1H");
        assert_eq!(one_h.2.as_ref().unwrap().close_micros, 300_000_000);

        cleanup_bars(&pool, "PHASE7BMIX5").await;
        cleanup_bars(&pool, "PHASE7BMIX1").await;
    }

    /// Test 10: one pending bar input is consumed once for the whole tick,
    /// not once per binding.
    #[tokio::test]
    async fn one_pending_bar_consumed_once_for_whole_tick() {
        let Some(pool) = db_pool_or_skip("PHASE7B-10").await else {
            return;
        };
        let ts = recent_bar_ts();
        seed_bar(&pool, "PHASE7BONE1", "5m", ts, 100_000_000).await;
        seed_bar(&pool, "PHASE7BONE2", "5m", ts, 100_000_000).await;

        let keys = vec![
            (
                "PHASE7BONE1".to_string(),
                "intraday_scalper".to_string(),
                300,
            ),
            (
                "PHASE7BONE2".to_string(),
                "intraday_scalper".to_string(),
                300,
            ),
        ];
        let mut host_pool = DynamicSelectionHostPool::build(&keys).expect("pool builds");
        let bindings = vec![
            binding("PHASE7BONE1", "intraday_scalper", 300, "5m"),
            binding("PHASE7BONE2", "intraday_scalper", 300, "5m"),
        ];

        let state = hermetic_state_with_db(&pool);
        state
            .deposit_strategy_bar_input(StrategyBarInput {
                now_tick: 1,
                end_ts: ts,
                limit_price: Some(100_000_000),
                qty: 0,
            })
            .await;
        assert!(!state.pending_strategy_bar_input_is_none_for_test().await);

        let results = state
            .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                test_run_id(),
                &bindings,
                &mut host_pool,
            )
            .await
            .expect("dispatch must not fault");
        assert_eq!(
            results.len(),
            2,
            "one take() serves both bindings this tick"
        );
        assert!(
            state.pending_strategy_bar_input_is_none_for_test().await,
            "the single pending bar must be consumed, not left for a second tick"
        );

        // A second call with nothing pending must yield zero results, never
        // a stale re-dispatch of the same bar.
        let second = state
            .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                test_run_id(),
                &bindings,
                &mut host_pool,
            )
            .await
            .expect("no fault on an empty pending slot");
        assert!(second.is_empty());

        cleanup_bars(&pool, "PHASE7BONE1").await;
        cleanup_bars(&pool, "PHASE7BONE2").await;
    }

    /// Test 13: a missing host key (binding present, pool entry absent)
    /// fails closed with zero results, never a legacy fallback.
    #[tokio::test]
    async fn missing_host_key_fails_closed_zero_results() {
        let Some(pool) = db_pool_or_skip("PHASE7B-13").await else {
            return;
        };
        let ts = recent_bar_ts();
        seed_bar(&pool, "PHASE7BMISSING", "5m", ts, 100_000_000).await;

        // Empty pool -- the binding's key has no corresponding host.
        let mut host_pool = DynamicSelectionHostPool::build(&[]).expect("empty pool builds");
        let bindings = vec![binding("PHASE7BMISSING", "intraday_scalper", 300, "5m")];

        let state = hermetic_state_with_db(&pool);
        state
            .deposit_strategy_bar_input(StrategyBarInput {
                now_tick: 1,
                end_ts: ts,
                limit_price: Some(100_000_000),
                qty: 0,
            })
            .await;

        let err = state
            .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                test_run_id(),
                &bindings,
                &mut host_pool,
            )
            .await
            .expect_err("a missing host key must fail closed, not silently skip");
        assert!(matches!(
            err,
            SelectedHostDispatchFault::HostMissingAtDispatch { .. }
        ));

        cleanup_bars(&pool, "PHASE7BMISSING").await;
    }

    /// Test 25: missing bars invoke no host and produce no result for that
    /// binding (fail-closed, not an error) -- no other binding is affected.
    #[tokio::test]
    async fn missing_bars_invoke_no_host_and_produce_no_result() {
        let Some(pool) = db_pool_or_skip("PHASE7B-25").await else {
            return;
        };
        // No bars seeded at all for this symbol.
        let keys = vec![(
            "PHASE7BNOBARS".to_string(),
            "intraday_scalper".to_string(),
            300,
        )];
        let mut host_pool = DynamicSelectionHostPool::build(&keys).expect("pool builds");
        let bindings = vec![binding("PHASE7BNOBARS", "intraday_scalper", 300, "5m")];

        let state = hermetic_state_with_db(&pool);
        state
            .deposit_strategy_bar_input(StrategyBarInput {
                now_tick: 1,
                end_ts: Utc::now().timestamp(),
                limit_price: Some(100_000_000),
                qty: 0,
            })
            .await;

        let results = state
            .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                test_run_id(),
                &bindings,
                &mut host_pool,
            )
            .await
            .expect("missing bars must be a fail-closed no-op, not a fault");
        assert!(results.is_empty(), "no bars -> no host call -> no result");
    }

    /// Off/Legacy structural proof: `RuntimeStrategyDispatchAuthority::
    /// Legacy` carries no host pool at all, so the selected-host dispatch
    /// function is structurally unreachable for it -- the loop_runner.rs
    /// match arm never calls `tick_strategy_dispatch_selected_hosts_with_
    /// bar_facts` for `Legacy` (see loop_runner.rs and the Phase 7B guard
    /// script). This is proven by construction, not a runtime counter.
    #[test]
    fn legacy_authority_carries_no_host_pool() {
        let legacy =
            crate::dynamic_selection_dispatch_authority::RuntimeStrategyDispatchAuthority::Legacy {
                assignments: vec![SymbolStrategyAssignment {
                    symbol: "AAPL".to_string(),
                    strategy_id: "intraday_scalper".to_string(),
                    timeframe: "5m".to_string(),
                }],
            };
        assert!(!legacy.is_dynamic_paper_enforced());
    }
}
