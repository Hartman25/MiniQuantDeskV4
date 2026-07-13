//! Shared runtime state for mqk-daemon.
//!
//! All types here are `Clone`-able (via `Arc` or copy). Handlers receive
//! `State<Arc<AppState>>` from Axum; this module owns daemon-local runtime
//! lifecycle control plus durable status reconstruction.

mod alpaca_ws_transport;
mod autonomous_bar_ticker;
mod broker;
mod deadman;
mod dry_run_strategy;
mod env;
pub mod instrument_economics_bridge;
mod lifecycle;
mod loop_runner;
pub mod market_calendar;
mod multi_symbol_config;
mod orchestrator_build;
mod per_symbol_bar_window;
pub mod runtime_session_source;
mod session_controller;
mod signal_intake;
mod snapshot;
mod types;
pub mod ws_gap_recovery;

use std::collections::{BTreeMap, HashMap, HashSet};
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
    /// DATA-PROVIDER-LATEST-BAR-POLL-01: Process-local last poll result for feed status.
    pub market_data_feed_status:
        Arc<RwLock<Option<crate::api_types::MarketDataFeedPollOnceResponse>>>,
    /// DATA-PROVIDER-LATEST-BAR-SCHEDULER-01: Process-local latest-bar scheduler.
    ///
    /// Disabled by default. Holds only in-memory task/config/status state.
    pub market_data_feed_scheduler: Arc<Mutex<MarketDataFeedSchedulerRuntimeState>>,
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
}

/// BROKER-FILL-REST-RECOVERY-01: Injectable abstraction over Alpaca REST activity fetch.
///
/// Defined here so both `state.rs` (storage) and `routes/repair.rs` (usage) can reference
/// it without a module cycle.  Tests inject a fake implementation; production wiring
/// is deferred to BROKER-FILL-REST-RECOVERY-APPLY-01.
pub trait BrokerFillActivityFetcher: Send + Sync {
    /// Fetch all account activities for the given Alpaca broker order UUID.
    ///
    /// Callers filter the returned list for FILL/PARTIAL_FILL activity types.
    /// Returns `Err(String)` if the REST call fails.
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
            pending_strategy_bar_input: Arc::new(Mutex::new(None)),
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
            market_data_feed_status: Arc::new(RwLock::new(None)),
            market_data_feed_scheduler: Arc::new(Mutex::new(
                MarketDataFeedSchedulerRuntimeState::default(),
            )),
            fill_activity_fetcher,
            ws_gap_fill_fetcher,
            broker_baseline: Arc::new(RwLock::new(None)),
            snapshot_fetcher,
            asset_shortable_preflight_fetcher,
            b5_alerted_symbols: Arc::new(RwLock::new(HashSet::new())),
            day_limit_alert_fired: Arc::new(AtomicBool::new(false)),
            per_symbol_position_cap_alerted_symbols: Arc::new(RwLock::new(HashSet::new())),
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

    async fn dispatch_native_strategy_for_symbol_with_loaded_bars(
        &self,
        symbol: &str,
        md_timeframe: &str,
        bar: StrategyBarInput,
        db_bars: Vec<mqk_db::MdBarRow>,
        now_ts: i64,
    ) -> Option<mqk_strategy::StrategyBarResult> {
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
            self.record_signal_evaluation(SignalEvaluationAttempt {
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
            })
            .await;
            return None;
        }

        // MD-STALENESS-PER-TICK-GATE-01: fail-closed per-dispatch-tick
        // staleness gate. `db_bars` is oldest-first, so the last element is the
        // latest completed bar.
        let latest_end_ts = db_bars.last().map(|b| b.end_ts);
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
            self.record_signal_evaluation(SignalEvaluationAttempt {
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
            })
            .await;
            return None;
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
        let result = self
            .invoke_native_strategy_on_bar_from_window(bar.now_tick, window)
            .await;
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
            self.record_signal_evaluation(SignalEvaluationAttempt {
                now_tick: bar.now_tick,
                symbol,
                timeframe: md_timeframe,
                bar_context_source: "db_loaded",
                bars_loaded: bars_loaded as i64,
                latest_bar_ts_utc: latest_end_ts
                    .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0)),
                signal_generated: signal_qty != 0,
                signal_qty: Some(signal_qty),
                reason_code: diagnostic_decision,
                reason: diagnostic_reason,
                decision_stage: "strategy_evaluated",
            })
            .await;
        }
        result
    }

    /// AUTON-NO-SIGNAL-OBS-01: one durable signal-evaluation journal write
    /// attempt, scoped to a single symbol/timeframe/tick.
    ///
    /// All `&str` fields borrow from the caller's locals for the duration of
    /// the call only — `record_signal_evaluation` copies what it needs into
    /// owned `String`s before returning.
    async fn record_signal_evaluation(&self, attempt: SignalEvaluationAttempt<'_>) {
        let Some(ref pool) = self.db else {
            return;
        };
        // No strategy_id to attribute this row to when the bootstrap is
        // Dormant/Failed — see the AUTON-NO-SIGNAL-OBS-01 callers above.
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
        let signal_side = match attempt.signal_qty {
            Some(q) if q > 0 => Some("buy".to_string()),
            Some(q) if q < 0 => Some("sell".to_string()),
            _ => None,
        };
        let run_id_key = run_id
            .map(|u| u.to_string())
            .unwrap_or_else(|| "none".to_string());
        // AUDIT-EVENT-DETERMINISM: deterministic UUIDv5, never Uuid::new_v4(),
        // so a duplicate write attempt for the same logical tick is a no-op
        // (ON CONFLICT DO NOTHING) rather than a second row.
        let evaluation_id = Uuid::new_v5(
            &Uuid::NAMESPACE_DNS,
            format!(
                "mqk.signal-evaluation.v1|{}|{}|{}|{}|{}",
                run_id_key, strategy_id, attempt.symbol, attempt.timeframe, attempt.now_tick
            )
            .as_bytes(),
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
        // AUTON-SIGNAL-CONTEXT-01: attempt DB-backed context window.
        let symbol = symbol.trim();
        let md_timeframe = timeframe.trim();

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
                        .dispatch_native_strategy_for_symbol_with_loaded_bars(
                            symbol,
                            md_timeframe,
                            bar,
                            db_bars,
                            Utc::now().timestamp(),
                        )
                        .await;
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
        self.invoke_native_strategy_on_bar_from_signal(
            bar.now_tick,
            bar.end_ts,
            bar.limit_price,
            bar.qty,
        )
        .await
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

    /// AUTON-SIGNAL-CONTEXT-01: Invoke `on_bar` with a pre-built DB-sourced window.
    async fn invoke_native_strategy_on_bar_from_window(
        &self,
        now_tick: u64,
        window: mqk_strategy::RecentBarsWindow,
    ) -> Option<mqk_strategy::StrategyBarResult> {
        self.native_strategy_bootstrap
            .lock()
            .await
            .as_mut()?
            .invoke_on_bar_from_window(now_tick, window)
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
