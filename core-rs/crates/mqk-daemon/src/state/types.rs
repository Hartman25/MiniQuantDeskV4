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
    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01: the outcome of this
    /// task releasing the orchestrator's runtime leadership lease before
    /// exiting, if it held one. `None` only when the task exited without
    /// ever owning a lease to release (not applicable to any current exit
    /// path — every exit, pre- or post-barrier, releases first — but kept
    /// `Option` rather than assumed-`Some` so a future exit path that
    /// genuinely never acquires the lease is not forced to fabricate an
    /// outcome). Never reduced to a log line: the caller that joins this
    /// handle (`install_active_runtime`'s conflict-cleanup path, or
    /// `clear_local_runtime_for_run`'s stop/halt/shutdown/reap path) must
    /// fold this into its own structured truth instead of discarding it.
    pub(crate) leadership_release_outcome: Option<Result<(), String>>,
}

#[derive(Debug)]
pub(crate) struct ExecutionLoopHandle {
    pub(crate) run_id: Uuid,
    pub(crate) stop_tx: watch::Sender<ExecutionLoopCommand>,
    pub(crate) join_handle: JoinHandle<ExecutionLoopExit>,
}

/// BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement 2:
/// immutable metadata bound to a run the instant local ownership moves from
/// `Reserved` to `Starting`. Never mutated in place after construction — a
/// later run's metadata is always a brand-new `Arc`, never an update of this
/// one, so a restart or a racing rollback can never observe a mix of two
/// runs' authoritative truth.
///
/// Some fields duplicate data that also lives in a separate "economic
/// mirror" field on `AppState` (`accepted_artifact`,
/// `native_strategy_bootstrap`) rather than physically relocating that
/// storage — moving those would touch the hot tick-body read paths this
/// patch's root-cause design rule keeps out of scope. `dynamic_selection` is
/// the one field an existing production getter (`AppState::dynamic_
/// selection_runtime_snapshot`) genuinely projects from (requirement 2
/// invariant 10, replacing the prior independently-authoritative field of
/// the same name). The remaining duplicated fields below (`accepted_
/// artifact`, `native_bootstrap_present`, `frozen_assignments`, `frozen_
/// assignments_source`, `approved_for_live`) exist to satisfy requirement 2
/// invariant 8 ("Active metadata includes ...") — making the ownership
/// state genuinely self-describing — and are exercised by this patch's own
/// tests (`state::ownership_state_machine_tests::
/// active_status_requires_matching_run_metadata_handle`); invariant 11 says
/// compatibility getters *may* project from them, not must, and
/// `AppState::accepted_artifact_provenance` deliberately keeps reading the
/// mirror directly instead — `scenario_artifact_provenance_tv01cd.rs`'s
/// AP-01..AP-06 route contract is driven entirely through the mirror-only
/// `set_accepted_artifact_for_test` seam, so switching that getter's source
/// would break real, already-established route-contract proof. `#[allow(
/// dead_code)]` below is therefore honest, not a lint workaround for
/// something that should be wired differently.
#[derive(Debug)]
pub(crate) struct RunStartMetadata {
    #[allow(dead_code)]
    pub(crate) run_id: Uuid,
    /// TV-01C provenance, mirrored here as the ownership-authoritative copy.
    #[allow(dead_code)]
    pub(crate) accepted_artifact: Option<AcceptedArtifactProvenance>,
    /// `true` when B1A native strategy bootstrap was constructed for this
    /// run. The bootstrap object itself is not `Clone` and stays solely in
    /// the separate `AppState::native_strategy_bootstrap` economic mirror —
    /// this is a presence witness, not a duplicate value.
    #[allow(dead_code)]
    pub(crate) native_bootstrap_present: bool,
    /// The frozen dynamic-selection outcome for this run — the sole
    /// authoritative source `AppState::dynamic_selection_runtime_snapshot`
    /// projects from (requirement 2 invariant 10, replacing the prior
    /// independently-authoritative `dynamic_selection_runtime` field).
    /// `None` only for a disposition that never built one.
    ///
    /// Interior-mutable (unlike every other field here) purely so the
    /// narrow test-only seam `AppState::commit_dynamic_selection_runtime_
    /// state` can attach a sentinel value to an *already-established*
    /// `Starting`/`Active` binding (e.g. one built by `establish_db_backed_
    /// active_run_for_test`) without disturbing that binding's real
    /// `ExecutionLoopHandle`. Production code always sets this once, at
    /// metadata-construction time, and never mutates it again.
    pub(crate) dynamic_selection: tokio::sync::RwLock<Option<DynamicSelectionRuntimeState>>,
    /// ATOMICITY-SINGLE-SNAPSHOT-REPAIR's frozen per-symbol legacy
    /// assignment vector for this exact start attempt.
    #[allow(dead_code)]
    pub(crate) frozen_assignments: Vec<super::SymbolStrategyAssignment>,
    /// Diagnostic label for where `frozen_assignments` was resolved from
    /// (e.g. `"start_attempt_snapshot"`, `"test_fixture"`).
    #[allow(dead_code)]
    pub(crate) frozen_assignments_source: &'static str,
    /// Always `false` in this patch — Bundle 7 dynamic selection carries no
    /// live dispatch authority under any disposition (mirrors
    /// `DynamicSelectionRuntimeState::approved_for_live`).
    #[allow(dead_code)]
    pub(crate) approved_for_live: bool,
}

/// Bounded, structured detail for a [`LocalRuntimeOwnership::Degraded`]
/// state — requirement 4's "local authority removed even when DB transition
/// fails, with degraded truth": a durable DB transition (`stop_run`/
/// `halt_run`) failed after local ownership for `run_id` had already been
/// fully released.
#[derive(Debug, Clone)]
pub(crate) struct BoundedLifecycleDegradation {
    pub(crate) operation: &'static str,
    pub(crate) detail: String,
}

impl fmt::Display for BoundedLifecycleDegradation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "durable transition failed ({}): {}",
            self.operation, self.detail
        )
    }
}

/// BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement 2: the
/// single source-of-truth local runtime lifecycle-ownership state machine,
/// replacing the prior split of three independently-mutated authorities —
/// `ExecutionLoopSlot` (`Empty`/`Reserved`/`Active`), `run_start_commit_
/// owner` (a bare `Option<Uuid>` tag for a separately-stored legacy field
/// bundle), and `dynamic_selection_runtime` (an independently-committed/
/// cleared `Option<DynamicSelectionRuntimeState>`). All three are now one
/// enum behind one lock: `Idle` / `Reserved` / `Starting` / `Active` /
/// `Degraded`.
///
/// Every non-`Idle` variant carries the exact `run_id` it is scoped to, so a
/// compare-and-clear for run A can never mutate state that legitimately
/// belongs to a different run B (requirement 2 invariant 5 / requirement 4's
/// "run A cannot clear B").
#[derive(Debug)]
pub(crate) enum LocalRuntimeOwnership {
    /// No active run. The only state a new start attempt may reserve from.
    Idle,
    /// A start attempt for `run_id` has claimed the slot but has not yet
    /// built runtime metadata. Conflicts with every other run_id, including
    /// another `Reserved`/`Starting`/`Active`/`Degraded` value.
    Reserved { run_id: Uuid },
    /// Runtime metadata and economic-mirror fields have been prepared and
    /// committed for `run_id`, but the execution loop task has not yet
    /// cleared the startup barrier (requirement 3) — never reported as
    /// running (requirement 2 invariant 7).
    Starting {
        run_id: Uuid,
        metadata: Arc<RunStartMetadata>,
    },
    /// The execution loop is installed and has cleared the startup barrier
    /// for `run_id`. The only state `status()` may report as "running".
    Active {
        run_id: Uuid,
        metadata: Arc<RunStartMetadata>,
        handle: ExecutionLoopHandle,
    },
    /// Local ownership for `run_id` was released (no local authority
    /// remains) but a subsequent durable/truth transition could not be
    /// confirmed — an honest degraded note, never silently presented as
    /// clean `Idle`.
    Degraded {
        run_id: Uuid,
        detail: BoundedLifecycleDegradation,
    },
}

impl LocalRuntimeOwnership {
    /// The `run_id` this state is scoped to, or `None` for `Idle`.
    pub(crate) fn owned_run_id(&self) -> Option<Uuid> {
        match self {
            Self::Idle => None,
            Self::Reserved { run_id }
            | Self::Starting { run_id, .. }
            | Self::Active { run_id, .. }
            | Self::Degraded { run_id, .. } => Some(*run_id),
        }
    }
}

/// Typed failure from `AppState::prepare_starting_metadata_and_mirrors` —
/// the `Reserved { run_id } -> Starting { run_id, metadata }` transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrepareStartingMetadataError {
    /// The slot is `Idle` — `reserve_runtime_ownership` was never called (or
    /// was already released) for this run_id.
    NoReservation,
    /// The slot is `Reserved` for a *different* run_id.
    ReservedForDifferentRun { reserved_run_id: Uuid },
    /// The slot is already past `Reserved` (`Starting`/`Active`/`Degraded`)
    /// — a duplicate/misordered call.
    NotReserved,
}

impl fmt::Display for PrepareStartingMetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoReservation => write!(f, "runtime ownership has no reservation to prepare"),
            Self::ReservedForDifferentRun { reserved_run_id } => write!(
                f,
                "runtime ownership is reserved for a different run_id: {reserved_run_id}"
            ),
            Self::NotReserved => write!(
                f,
                "runtime ownership is no longer Reserved (already Starting/Active/Degraded)"
            ),
        }
    }
}

/// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 1 requirement 3:
/// structured truth about the just-spawned task that `install_active_
/// runtime` stopped and joined on any conflict/mismatch, instead of
/// discarding the join result (`let _ = handle.join_handle.await;`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstallRuntimeTaskCleanup {
    /// `Ok(())` when `handle.join_handle.await` resolved normally. `Err(_)`
    /// when the join itself failed (the spawned task panicked) — never
    /// discarded via `let _ =`.
    pub(crate) join_outcome: Result<(), String>,
    /// The task's own `ExecutionLoopExit::leadership_release_outcome` when
    /// the join succeeded. `None` when the join itself failed (a panicked
    /// task's exit value is unavailable — its release outcome, if any, is
    /// genuinely unknown, not silently assumed `Ok`).
    pub(crate) leadership_release_outcome: Option<Result<(), String>>,
}

impl InstallRuntimeTaskCleanup {
    /// `true` when either the join failed (task panicked — its cleanup
    /// truth is unknown) or the task reported a failed leadership release.
    /// Callers use this to decide whether the resulting rollback/degraded
    /// truth must be surfaced rather than treated as an ordinary clean
    /// refusal.
    pub(crate) fn is_degraded(&self) -> bool {
        self.join_outcome.is_err()
            || self
                .leadership_release_outcome
                .as_ref()
                .is_some_and(|r| r.is_err())
    }
}

/// Typed failure from `AppState::install_active_runtime` — the `Starting {
/// run_id, metadata } -> Active { run_id, metadata, handle }` transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstallActiveRuntimeError {
    /// The slot is not `Starting` for this run_id at all (`Idle`/`Reserved`).
    NotStarting { cleanup: InstallRuntimeTaskCleanup },
    /// The slot is `Starting` for a *different* run_id.
    StartingForDifferentRun {
        starting_run_id: Uuid,
        cleanup: InstallRuntimeTaskCleanup,
    },
    /// The slot is already `Active` — a second install was attempted.
    AlreadyActive {
        active_run_id: Uuid,
        cleanup: InstallRuntimeTaskCleanup,
    },
}

impl InstallActiveRuntimeError {
    /// Consumes `self`, returning the task-side cleanup truth every variant
    /// carries — the caller (`ProductionRuntimeStartEffects::spawn_loop`)
    /// folds this into its own structured rollback truth instead of
    /// matching on the specific refusal reason.
    pub(crate) fn into_cleanup(self) -> InstallRuntimeTaskCleanup {
        match self {
            Self::NotStarting { cleanup }
            | Self::StartingForDifferentRun { cleanup, .. }
            | Self::AlreadyActive { cleanup, .. } => cleanup,
        }
    }
}

impl fmt::Display for InstallActiveRuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotStarting { .. } => write!(f, "runtime ownership is not Starting"),
            Self::StartingForDifferentRun {
                starting_run_id, ..
            } => write!(
                f,
                "runtime ownership is Starting for a different run_id: {starting_run_id}"
            ),
            Self::AlreadyActive { active_run_id, .. } => write!(
                f,
                "runtime ownership is already active for run_id: {active_run_id}"
            ),
        }
    }
}

/// BUNDLE-7-PHASE-7A-CORE-ATOMIC-STATE-MACHINE-CLOSURE requirement 4:
/// bounded reason codes for `AppState::clear_local_runtime_for_run` /
/// `AppState::clear_currently_owned_local_runtime`, recorded only for
/// diagnostics/tracing — every reason clears identically, none changes
/// clearing behavior. Covers every exit path that goes through the
/// run_id-scoped unified authority; `reap_finished_execution_loop` and the
/// finished-loop branch of `take_execution_loop_for_control` clear directly
/// via the same underlying mirror-clearing primitive without a reason tag
/// (they extract by construction — a finished handle — never by run_id
/// lookup, so there is no ambiguous "which reason" to record).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleClearReason {
    /// Failed start (reservation, run-link persist, runtime-effects, or
    /// barrier/install failure) — rolled back via `RuntimeStartEffects::
    /// rollback_local_effects`.
    FailedStart,
    OperatorStop,
    OperatorHalt,
    Shutdown,
}

/// Structured result of `AppState::clear_local_runtime_for_run`.
#[derive(Debug, Default)]
pub(crate) struct LocalRuntimeClearOutcome {
    /// `true` when this call actually matched and cleared `run_id` — a
    /// different owner, or an already-`Idle` slot, leaves this `false` and
    /// touches nothing (requirement 4's "run A cannot clear B").
    pub(crate) cleared: bool,
    /// `true` when an `Active` handle was found and had not yet finished —
    /// this call sent it the stop signal and joined it.
    pub(crate) stopped_live_handle: bool,
    /// `Some(detail)` when joining the handle failed (a panic in the task).
    pub(crate) join_error: Option<String>,
    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01: the joined task's
    /// own `ExecutionLoopExit::leadership_release_outcome`, when the join
    /// succeeded. `None` when no live handle was found, or when the join
    /// itself failed (`join_error` is `Some` instead).
    pub(crate) leadership_release_outcome: Option<Result<(), String>>,
}

impl LocalRuntimeClearOutcome {
    /// `true` when the joined task's cleanup could not be confirmed clean —
    /// a join failure (panic) or a reported leadership-release failure.
    /// Callers (stop/halt/shutdown/reap) must treat this as degraded truth,
    /// never as a plain successful `Idle` transition.
    pub(crate) fn is_degraded(&self) -> bool {
        self.join_error.is_some()
            || self
                .leadership_release_outcome
                .as_ref()
                .is_some_and(|r| r.is_err())
    }
}

// NOTE: `RuntimeStartPhase` lives in `crate::daily_data_readiness` (it must
// be `pub` there — the `RuntimeStartEffects` trait it's part of is driven
// from integration tests outside this crate's `pub(crate)` boundary), not
// here. `LocalRuntimeOwnership` above stays `pub(crate)` since nothing
// outside this crate needs it directly.

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
/// `plan` is wrapped in `Arc` so this whole struct stays cheaply `Clone`
/// (for `AppState::dynamic_selection_runtime_snapshot`).
///
/// PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE: the real, mutable
/// [`crate::dynamic_selection_host_pool::DynamicSelectionHostPool`] is never
/// stored here. This struct is snapshotted (cloned) freely for status reads,
/// so it can never be the pool's exclusive mutable owner — the one built
/// pool is moved wholesale into the execution loop task instead (see
/// `RuntimeStrategyDispatchAuthority`, `state/loop_runner.rs`). `host_pool_
/// present`/`plan_id` are the immutable, cheaply-cloned status witnesses of
/// that fact, not the object itself.
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
    /// Deterministic UUIDv5 identity minted from `plan` via
    /// `mqk_portfolio::canonical_plan_identity_material` (Part 2). `Some`
    /// exactly when `plan` is `Some`.
    pub plan_id: Option<Uuid>,
    /// Bounded `(symbol, strategy_id, timeframe_secs)` projection of
    /// `plan`'s selected rows — the exact triples
    /// `DynamicSelectionHostPool::build` would key on. Empty for `Off` and
    /// for any disposition with no selected pair.
    pub selected_pairs: Vec<(String, String, i64)>,
    /// `true` only for `PaperEnforcedAllowed` — the only disposition that
    /// builds a host pool. Status witness only: the real pool is owned
    /// exclusively by the execution loop task, never by this struct.
    pub host_pool_present: bool,
    pub reasons: Vec<crate::dynamic_selection_start_gate::DynamicSelectionStartGateReason>,
    /// Always `false` in this patch. Bundle 7 dynamic selection is not
    /// dispatch-authoritative under any disposition here — this field exists
    /// so a future status surface never has to fabricate this invariant.
    pub approved_for_live: bool,
    /// Phase 7C Part 2: `true` only when durable plan evidence was actually
    /// written (`Inserted` or `AlreadyExists`) for this start attempt.
    /// `false` for `Off`, for any disposition reached before a plan could be
    /// built, and for a `PayloadCollision`/write failure that a non-blocking
    /// disposition (Shadow*) tolerated rather than refusing start on. Never
    /// `true` when evidence was not actually durably persisted.
    pub evidence_persisted: bool,
    /// Phase 7C Part 3: the read-side validation outcome code (see
    /// `crate::dynamic_selection_evidence_validator::DynamicSelectionEvidenceValidationState::code`)
    /// captured when this start attempt read-validated its own evidence
    /// (`PaperEnforcedAllowed` only). `None` for every other disposition —
    /// Shadow* and refused starts persist evidence but do not gate
    /// activation on read-validation.
    pub evidence_validation_state: Option<String>,
}

impl DynamicSelectionRuntimeState {
    pub fn selected_pair_count(&self) -> usize {
        self.selected_pairs.len()
    }

    pub fn plan_present(&self) -> bool {
        self.plan.is_some()
    }

    pub fn host_pool_present(&self) -> bool {
        self.host_pool_present
    }
}

/// Manual `Debug` impl kept for source-stability across the Phase 7B field
/// change (was needed to hide `DynamicSelectionHostPool` internals; the
/// pool no longer lives here at all, but the explicit impl is retained
/// rather than switching call sites to a derive).
impl fmt::Debug for DynamicSelectionRuntimeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DynamicSelectionRuntimeState")
            .field("run_id", &self.run_id)
            .field("disposition", &self.disposition)
            .field("configured_mode", &self.configured_mode)
            .field("effective_mode", &self.effective_mode)
            .field("live_lock_applied", &self.live_lock_applied)
            .field("plan_present", &self.plan.is_some())
            .field("plan_id", &self.plan_id)
            .field("selected_pairs", &self.selected_pairs)
            .field("host_pool_present", &self.host_pool_present)
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
    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 3 row 11: after
    /// a genuine `arm_run` success, before `begin_run` is called, perturbs
    /// the durable run to `Stopped` via the same real `mqk_db::stop_run`
    /// production callers use — never a fake/injected error. The real
    /// `begin_run` call immediately afterward then organically fails
    /// (`begin_run invalid state: STOPPED`) because the row it reads back
    /// no longer matches its own precondition.
    PerturbRunStoppedBeforeBegin,
    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 3 row 12: after
    /// a genuine `begin_run` success, before the initial `heartbeat_run`
    /// call, perturbs the durable run to `Stopped` the same way — the real
    /// `heartbeat_run` call immediately afterward organically fails
    /// (`heartbeat_run invalid state: STOPPED`).
    PerturbRunStoppedBeforeInitialHeartbeat,
    /// PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 3 row 27
    /// ("durable rollback query failure"): fires at the same point as
    /// `AfterOrchestratorConstruction`, but its real action is deleting the
    /// run row itself (via the real `mqk_db` delete, not a fake) before
    /// returning the error. `rollback_failed_start_attempt`'s own
    /// `fetch_run` call then organically fails (`RowNotFound`) because the
    /// row genuinely no longer exists.
    DeleteRunRowBeforeArm,
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
