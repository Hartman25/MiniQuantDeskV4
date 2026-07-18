//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3-COMPLETED-BAR-TASK-CUTOVER-AND-SUPERVISION.
//!
//! Production cutover: replaces the legacy blind-timer autonomous bar
//! ticker (`state::autonomous_bar_ticker`) with one supervised, exactly-one,
//! restart-bounded Tokio task that drives the accepted Phase C completed-bar
//! driver (`state::autonomous_completed_bar_driver::tick_autonomous_completed_bar_driver`)
//! on a fixed cadence.
//!
//! # Production authority
//!
//! ```text
//! supervised completed-bar task
//!   -> durable relevant operation (mqk_db::fetch_relevant_open_autonomous_daily_operation)
//!   -> explicit driver mode (select_driver_mode_for_state, from durable state only)
//!   -> exact canonical expected completed bar (Phase C driver, unchanged)
//!   -> durable dispatch claim (Phase C driver, unchanged)
//!   -> canonical native strategy dispatch (Phase C driver, unchanged)
//! ```
//!
//! This module never re-derives readiness, expected-bar identity, or
//! dispatch mechanics — it only supplies the production tick adapter
//! (`tick_autonomous_completed_bar_driver_from_state`) that resolves
//! canonical Bundle 2/Bundle 3 inputs and calls the accepted driver once,
//! plus the task-ownership/cancellation/supervision/restart machinery that
//! wraps it.
//!
//! # Task ownership
//!
//! At most one completed-bar task may be active per `AppState`
//! (`AppState::completed_bar_task_claimed`, a `compare_exchange`-guarded
//! atomic). A second `spawn_autonomous_completed_bar_driver_task` call while
//! one is active returns `AlreadyRunning` and spawns no second Tokio task.
//! The claim is released only when the supervised task permanently ends
//! (expected cancellation, or restart-budget exhaustion).
//!
//! # Supervision and restart
//!
//! `run_supervised_completed_bar_worker` awaits the inner cadence-loop
//! worker's `JoinHandle`. An expected cancellation (the worker returns
//! `Stopped`) ends the task cleanly with no restart and no failure alert. An
//! unexpected exit or panic (`Err(JoinError)`, or any other returned
//! liveness) increments a bounded restart counter, sends one warning-level
//! notification, and restarts after a bounded delay (30s / 60s / 120s).
//! After 3 restarts the task reports `Failed`, sends exactly one critical
//! notification, and does not restart again.
//!
//! # Legacy ticker disposition
//!
//! `state::autonomous_bar_ticker` remains in source for compatibility
//! tests only. `main.rs` no longer spawns it — see
//! `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3-COMPLETED-BAR-TASK-CUTOVER-AND-SUPERVISION`
//! production-cutover source guards in
//! `tests/scenario_autonomous_completed_bar_task_01.rs`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{watch, Mutex, RwLock};
use tracing::warn;
use uuid::Uuid;

use super::autonomous_completed_bar_driver::{
    load_driver_instruments, resolve_autonomous_provider_call_authorization_from_env,
    run_bounded_cadence_task, tick_autonomous_completed_bar_driver,
    AutonomousCompletedBarDriverInput, AutonomousCompletedBarDriverMode,
    AutonomousCompletedBarDriverOutcome, AutonomousCompletedBarDriverTaskConfig,
    AutonomousCompletedBarDriverTaskLiveness, AutonomousDriverSetupRejection,
    ProductionAutonomousAssignmentReadinessEvaluator,
    ProductionAutonomousLatestBarProviderResolver,
};
use super::autonomous_daily_coordinator::apply_completed_bar_driver_outcome;
use super::autonomous_daily_operation::{
    derive_assignment_identity, derive_runtime_binding_identity,
};
use super::autonomous_runtime_context::resolve_autonomous_runtime_context;
use super::types::{AutonomousSessionTruth, DeploymentMode, StrategyMarketDataSource};
use super::{AppState, BrokerKind};
use crate::daily_data_readiness;
use crate::notify::CriticalAlertPayload;

// ---------------------------------------------------------------------------
// D3.5 — task cadence configuration
// ---------------------------------------------------------------------------

/// Env var: completed-bar task cadence in seconds.
pub const COMPLETED_BAR_TICK_SECS_ENV: &str = "MQK_AUTONOMOUS_COMPLETED_BAR_TICK_SECS";
const DEFAULT_TICK_SECS: u64 = 15;
const MIN_TICK_SECS: u64 = 1;
const MAX_TICK_SECS: u64 = 300;

/// Typed startup-configuration truth for `MQK_AUTONOMOUS_COMPLETED_BAR_TICK_SECS`.
/// An invalid, zero, negative, or out-of-range value fails closed rather
/// than silently substituting an unrelated cadence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletedBarTaskCadenceError {
    /// The raw value could not be parsed as an integer.
    NotAnInteger { raw: String },
    /// The parsed value is outside `[MIN_TICK_SECS, MAX_TICK_SECS]`.
    OutOfRange { value: i64 },
}

/// Pure classification of the cadence from an already-read raw env value.
/// Absent or blank uses the bounded default (15s). Never reads env itself —
/// production callers use [`resolve_completed_bar_tick_cadence_from_env`].
pub fn resolve_completed_bar_tick_cadence(
    raw: Option<&str>,
) -> Result<Duration, CompletedBarTaskCadenceError> {
    match raw.map(str::trim) {
        None | Some("") => Ok(Duration::from_secs(DEFAULT_TICK_SECS)),
        Some(v) => {
            let parsed: i64 = v
                .parse()
                .map_err(|_| CompletedBarTaskCadenceError::NotAnInteger { raw: v.to_string() })?;
            if parsed < MIN_TICK_SECS as i64 || parsed > MAX_TICK_SECS as i64 {
                return Err(CompletedBarTaskCadenceError::OutOfRange { value: parsed });
            }
            Ok(Duration::from_secs(parsed as u64))
        }
    }
}

/// Production entry point: reads `MQK_AUTONOMOUS_COMPLETED_BAR_TICK_SECS`
/// once and delegates to the pure classifier.
pub fn resolve_completed_bar_tick_cadence_from_env(
) -> Result<Duration, CompletedBarTaskCadenceError> {
    resolve_completed_bar_tick_cadence(std::env::var(COMPLETED_BAR_TICK_SECS_ENV).ok().as_deref())
}

// ---------------------------------------------------------------------------
// D3.2 — explicit durable-state mode selection
// ---------------------------------------------------------------------------

/// Select the driver mode solely from the durable operation's own `state`
/// column. Never inferred from clock time, provider availability, runtime
/// bootstrap presence, or pending-bar state. Returns `None` when no
/// automated driver invocation is legal for this state (recovery/stopping/
/// manual/degraded/terminal/unknown).
pub fn select_driver_mode_for_state(state: &str) -> Option<AutonomousCompletedBarDriverMode> {
    match state {
        mqk_db::STATE_AWAITING_PREOPEN
        | mqk_db::STATE_PREPARING_DATA
        | mqk_db::STATE_AWAITING_OPEN
        | mqk_db::STATE_PREFLIGHT_BLOCKED
        | mqk_db::STATE_START_RETRYING => Some(AutonomousCompletedBarDriverMode::PrepareDataOnly),
        mqk_db::STATE_RUNNING => Some(AutonomousCompletedBarDriverMode::RunningDispatch),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// D3.1 — production driver-tick adapter
// ---------------------------------------------------------------------------

/// Typed, bounded outcome of one production completed-bar task tick.
#[derive(Debug, Clone, PartialEq)]
pub enum AutonomousCompletedBarProductionTickOutcome {
    /// Not Paper+Alpaca/`ExternalSignalIngestion`, or no DB configured.
    NotApplicable { reason_code: &'static str },
    /// No durable operation currently relevant to this adapter/deployment
    /// slot exists. No row is created by this adapter.
    NoRelevantOperation,
    /// The relevant operation's durable state permits no automated driver
    /// invocation this tick (D3.2).
    ModeNotApplicable { operation_id: Uuid, state: String },
    /// Canonical assignment or runtime-binding resolution failed before the
    /// driver could be invoked. Never a provider call.
    IdentityUnresolved {
        operation_id: Uuid,
        reason_code: &'static str,
    },
    /// The canonical instrument registry could not be loaded/validated.
    RegistryUnavailable {
        operation_id: Uuid,
        rejection: AutonomousDriverSetupRejection,
    },
    /// The accepted Phase C driver was invoked exactly once and returned a
    /// typed outcome.
    DriverOutcome {
        operation_id: Uuid,
        mode: AutonomousCompletedBarDriverMode,
        outcome: AutonomousCompletedBarDriverOutcome,
    },
}

/// Production adapter: reads no wall clock internally (`now_utc` is
/// caller-supplied), requires Paper+Alpaca/`ExternalSignalIngestion` and a
/// configured runtime DB, fetches the one relevant durable operation via the
/// accepted bounded lookup, resolves canonical assignment/runtime-binding
/// configuration and the canonical instrument registry, selects the driver
/// mode solely from durable operation state, and calls the accepted Phase C
/// driver exactly once. Never introduces a parallel readiness evaluator,
/// expected-bar calculation, or strategy dispatch — every one of those seams
/// is reused unchanged from Phase C.
pub async fn tick_autonomous_completed_bar_driver_from_state(
    state: &Arc<AppState>,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousCompletedBarProductionTickOutcome> {
    if state.deployment_mode() != DeploymentMode::Paper
        || state.runtime_selection().broker_kind != Some(BrokerKind::Alpaca)
        || state.strategy_market_data_source() != StrategyMarketDataSource::ExternalSignalIngestion
    {
        return Ok(AutonomousCompletedBarProductionTickOutcome::NotApplicable {
            reason_code: "not_paper_alpaca_external_signal_ingestion",
        });
    }

    let Some(pool) = state.db.clone() else {
        return Ok(AutonomousCompletedBarProductionTickOutcome::NotApplicable {
            reason_code: "database_not_configured",
        });
    };

    let deployment_mode = state.deployment_mode().as_db_mode();
    let adapter_id = state.adapter_id().to_string();

    let Some(operation) = mqk_db::fetch_relevant_open_autonomous_daily_operation(
        &pool,
        deployment_mode,
        &adapter_id,
        now_utc,
    )
    .await?
    else {
        return Ok(AutonomousCompletedBarProductionTickOutcome::NoRelevantOperation);
    };

    let Some(mode) = select_driver_mode_for_state(&operation.state) else {
        return Ok(
            AutonomousCompletedBarProductionTickOutcome::ModeNotApplicable {
                operation_id: operation.operation_id,
                state: operation.state.clone(),
            },
        );
    };

    let assignment_config = match crate::state::build_multi_symbol_runtime_config_from_env() {
        Ok(config) => config,
        Err(_err) => {
            return Ok(
                AutonomousCompletedBarProductionTickOutcome::IdentityUnresolved {
                    operation_id: operation.operation_id,
                    reason_code: "assignment_resolution_unavailable",
                },
            );
        }
    };

    let runtime_context = match resolve_autonomous_runtime_context(state).await {
        Ok(ctx) => ctx,
        Err(_err) => {
            return Ok(
                AutonomousCompletedBarProductionTickOutcome::IdentityUnresolved {
                    operation_id: operation.operation_id,
                    reason_code: "runtime_binding_resolution_unavailable",
                },
            );
        }
    };

    let instruments = match load_driver_instruments(&state.instrument_registry_path) {
        Ok(instruments) => instruments,
        Err(rejection) => {
            return Ok(
                AutonomousCompletedBarProductionTickOutcome::RegistryUnavailable {
                    operation_id: operation.operation_id,
                    rejection,
                },
            );
        }
    };

    // D3.1.10: resolve provider_id from the assignment's canonical
    // instrument binding. When the assignment is not exactly one symbol, or
    // no matching instrument exists, an empty provider_id is passed through
    // unchanged — the driver's own binding/registry admission checks (which
    // run first) fail closed on those conditions independently; this
    // adapter never duplicates that check.
    let provider_id: String = match assignment_config.symbols.as_slice() {
        [only] => instruments
            .iter()
            .find(|i| i.symbol.trim().eq_ignore_ascii_case(only.symbol.trim()))
            .map(|i| i.provider.clone())
            .unwrap_or_default(),
        _ => String::new(),
    };

    let assignment_identity = derive_assignment_identity(&assignment_config);
    let runtime_binding_identity =
        derive_runtime_binding_identity(&runtime_context.effective_runtime_binding);

    let readiness_context = daily_data_readiness::load_readiness_context_from_env();
    let readiness_evaluator = ProductionAutonomousAssignmentReadinessEvaluator::new(
        pool.clone(),
        assignment_config.clone(),
        runtime_context.effective_runtime_binding.clone(),
        readiness_context,
    );
    let provider_resolver =
        ProductionAutonomousLatestBarProviderResolver::new(state.provider_registry_path.clone());
    let authorization = resolve_autonomous_provider_call_authorization_from_env();

    let driver_outcome = tick_autonomous_completed_bar_driver(AutonomousCompletedBarDriverInput {
        state,
        pool: &pool,
        operation: &operation,
        assignment_config: &assignment_config,
        assignment_identity: &assignment_identity,
        runtime_binding: &runtime_context.effective_runtime_binding,
        runtime_binding_identity: &runtime_binding_identity,
        now_utc,
        authorization,
        instruments: &instruments,
        provider_id: &provider_id,
        provider_resolver: &provider_resolver,
        readiness_evaluator: &readiness_evaluator,
        mode,
    })
    .await?;

    // D3.14: durably degrade the relevant operation for any critical
    // (evidence/manual or runtime-ownership) outcome. No-op for every
    // benign/waiting or transient outcome.
    apply_completed_bar_driver_outcome(&pool, &operation, &driver_outcome, now_utc).await?;

    Ok(AutonomousCompletedBarProductionTickOutcome::DriverOutcome {
        operation_id: operation.operation_id,
        mode,
        outcome: driver_outcome,
    })
}

/// D3.13: bounded, stable classification code for task-truth bookkeeping.
/// Reason codes are never parsed as authority — this is purely an
/// operator-visible label.
fn production_outcome_code(outcome: &AutonomousCompletedBarProductionTickOutcome) -> &'static str {
    use AutonomousCompletedBarProductionTickOutcome as P;
    match outcome {
        P::NotApplicable { .. } => "not_applicable",
        P::NoRelevantOperation => "no_relevant_operation",
        P::ModeNotApplicable { .. } => "mode_not_applicable",
        P::IdentityUnresolved { .. } => "identity_unresolved",
        P::RegistryUnavailable { .. } => "registry_unavailable",
        P::DriverOutcome { outcome, .. } => driver_outcome_code(outcome),
    }
}

fn driver_outcome_code(outcome: &AutonomousCompletedBarDriverOutcome) -> &'static str {
    use AutonomousCompletedBarDriverOutcome as O;
    match outcome {
        O::NotApplicable { .. } => "driver_not_applicable",
        O::OutsideOperationWindow => "outside_operation_window",
        O::ExchangeSessionTruthMissing => "exchange_session_truth_missing",
        O::AuthorizationDisabled => "authorization_disabled",
        O::AuthorizationInvalid { .. } => "authorization_invalid",
        O::BindingBlocked { .. } => "binding_blocked",
        O::RegistryBlocked { .. } => "registry_blocked",
        O::ReadinessBlocked { .. } => "readiness_blocked",
        O::Unsupported { .. } => "unsupported",
        O::PollNotDue => "poll_not_due",
        O::PollSucceededNoNewBar => "poll_succeeded_no_new_bar",
        O::NoNewCompletedBar => "no_new_completed_bar",
        O::PollFailedTransient { .. } => "poll_failed_transient",
        O::PollFailedTerminal { .. } => "poll_failed_terminal",
        O::BarObserved { .. } => "bar_observed",
        O::AlreadyDispatched { .. } => "already_dispatched",
        O::DispatchClaimUnresolved { .. } => "dispatch_claim_unresolved",
        O::DispatchCompleted { .. } => "dispatch_completed",
        O::EvidencePersistenceFailed { .. } => "evidence_persistence_failed",
        O::ProviderLaggingExpectedBar { .. } => "provider_lagging_expected_bar",
        O::UnexpectedOrFutureBar { .. } => "unexpected_or_future_bar",
        O::ReadinessBlockedAfterPoll { .. } => "readiness_blocked_after_poll",
        O::ObservedBarEvidenceInconsistent { .. } => "observed_bar_evidence_inconsistent",
        O::ObservedBarSequenceInconsistent { .. } => "observed_bar_sequence_inconsistent",
        O::ProviderSetupBlocked { .. } => "provider_setup_blocked",
        O::RuntimeDispatchNotReady { .. } => "runtime_dispatch_not_ready",
    }
}

fn production_outcome_operation_id(
    outcome: &AutonomousCompletedBarProductionTickOutcome,
) -> Option<Uuid> {
    use AutonomousCompletedBarProductionTickOutcome as P;
    match outcome {
        P::NotApplicable { .. } | P::NoRelevantOperation => None,
        P::ModeNotApplicable { operation_id, .. }
        | P::IdentityUnresolved { operation_id, .. }
        | P::RegistryUnavailable { operation_id, .. }
        | P::DriverOutcome { operation_id, .. } => Some(*operation_id),
    }
}

fn production_outcome_mode(
    outcome: &AutonomousCompletedBarProductionTickOutcome,
) -> Option<AutonomousCompletedBarDriverMode> {
    match outcome {
        AutonomousCompletedBarProductionTickOutcome::DriverOutcome { mode, .. } => Some(*mode),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// D3.6 — process-local task ownership/liveness truth
// ---------------------------------------------------------------------------

/// Process-local task-runtime truth for the completed-bar task. Never
/// carries free-form provider/SQL/credential error text — only stable
/// static codes.
#[derive(Debug, Clone)]
pub struct AutonomousCompletedBarTaskTruth {
    pub generation: u64,
    pub liveness: AutonomousCompletedBarDriverTaskLiveness,
    pub mode: Option<AutonomousCompletedBarDriverMode>,
    pub operation_id: Option<Uuid>,
    pub last_tick_utc: Option<DateTime<Utc>>,
    pub last_outcome_code: Option<&'static str>,
    pub consecutive_error_count: u32,
    pub restart_count: u32,
    pub last_error_code: Option<&'static str>,
}

impl Default for AutonomousCompletedBarTaskTruth {
    fn default() -> Self {
        Self {
            generation: 0,
            liveness: AutonomousCompletedBarDriverTaskLiveness::NotStarted,
            mode: None,
            operation_id: None,
            last_tick_utc: None,
            last_outcome_code: None,
            consecutive_error_count: 0,
            restart_count: 0,
            last_error_code: None,
        }
    }
}

/// Process-local, per-`AppState` task runtime handles. Constructed once by
/// [`AppState::new_completed_bar_task_runtime_for_test`]/`new_inner` and
/// never reconstructed for the lifetime of the process.
#[derive(Clone)]
pub struct AutonomousCompletedBarTaskRuntime {
    pub(super) truth: Arc<RwLock<AutonomousCompletedBarTaskTruth>>,
    pub(super) claimed: Arc<AtomicBool>,
    pub(super) cancel: Arc<Mutex<Option<watch::Sender<bool>>>>,
}

impl Default for AutonomousCompletedBarTaskRuntime {
    fn default() -> Self {
        Self {
            truth: Arc::new(RwLock::new(AutonomousCompletedBarTaskTruth::default())),
            claimed: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(Mutex::new(None)),
        }
    }
}

// ---------------------------------------------------------------------------
// D3.7 — production spawn seam
// ---------------------------------------------------------------------------

/// Typed spawn outcome for [`spawn_autonomous_completed_bar_driver_task`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutonomousCompletedBarTaskSpawnOutcome {
    Started,
    NotApplicable,
    DatabaseUnavailable,
    InvalidConfiguration,
    AlreadyRunning,
}

/// Spawn the exactly-one supervised completed-bar task for Paper+Alpaca.
///
/// Spawn requirements (provider authorization is deliberately not one of
/// them — trusted local bars must remain usable when provider calls are
/// disabled):
/// - deployment mode = PAPER
/// - strategy market-data source = `ExternalSignalIngestion`
/// - DB configured
/// - valid task cadence (`MQK_AUTONOMOUS_COMPLETED_BAR_TICK_SECS`)
///
/// A second call while one task is already active returns `AlreadyRunning`
/// and spawns no second Tokio task (atomic `compare_exchange` claim below).
pub async fn spawn_autonomous_completed_bar_driver_task(
    state: Arc<AppState>,
) -> AutonomousCompletedBarTaskSpawnOutcome {
    if state.deployment_mode() != DeploymentMode::Paper
        || state.strategy_market_data_source() != StrategyMarketDataSource::ExternalSignalIngestion
    {
        return AutonomousCompletedBarTaskSpawnOutcome::NotApplicable;
    }
    if state.db.is_none() {
        return AutonomousCompletedBarTaskSpawnOutcome::DatabaseUnavailable;
    }
    let cadence = match resolve_completed_bar_tick_cadence_from_env() {
        Ok(cadence) => cadence,
        Err(_) => return AutonomousCompletedBarTaskSpawnOutcome::InvalidConfiguration,
    };

    if state
        .completed_bar_task
        .claimed
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return AutonomousCompletedBarTaskSpawnOutcome::AlreadyRunning;
    }

    let (cancel_tx, cancel_rx) = watch::channel(false);
    *state.completed_bar_task.cancel.lock().await = Some(cancel_tx);

    let worker_state = Arc::clone(&state);
    tokio::spawn(async move {
        run_supervised_completed_bar_worker(
            worker_state,
            cadence,
            cancel_rx,
            |state, generation| {
                move || {
                    let state = Arc::clone(&state);
                    async move {
                        run_one_production_tick(state, generation).await;
                    }
                }
            },
        )
        .await;
    });

    AutonomousCompletedBarTaskSpawnOutcome::Started
}

async fn run_one_production_tick(state: Arc<AppState>, generation: u64) {
    // D3.9/51: a stale worker generation must never overwrite newer
    // liveness truth. Checked once up front and again inside every write
    // helper below (defense in depth against a theoretical overlapping
    // worker).
    if state.completed_bar_task.truth.read().await.generation != generation {
        return;
    }

    let now_utc = Utc::now();
    match tick_autonomous_completed_bar_driver_from_state(&state, now_utc).await {
        Ok(outcome) => {
            let code = production_outcome_code(&outcome);
            let mode = production_outcome_mode(&outcome);
            let operation_id = production_outcome_operation_id(&outcome);
            record_tick_success(
                &state.completed_bar_task.truth,
                generation,
                now_utc,
                code,
                mode,
                operation_id,
            )
            .await;
        }
        Err(err) => {
            warn!(error = %err, "autonomous_completed_bar_task: tick failed");
            record_tick_error(
                &state.completed_bar_task.truth,
                generation,
                now_utc,
                "tick_failed",
            )
            .await;
        }
    }
}

async fn record_tick_success(
    truth: &Arc<RwLock<AutonomousCompletedBarTaskTruth>>,
    generation: u64,
    now_utc: DateTime<Utc>,
    outcome_code: &'static str,
    mode: Option<AutonomousCompletedBarDriverMode>,
    operation_id: Option<Uuid>,
) {
    let mut guard = truth.write().await;
    if guard.generation != generation {
        return;
    }
    guard.last_tick_utc = Some(now_utc);
    guard.last_outcome_code = Some(outcome_code);
    guard.mode = mode;
    guard.operation_id = operation_id;
    guard.consecutive_error_count = 0;
}

async fn record_tick_error(
    truth: &Arc<RwLock<AutonomousCompletedBarTaskTruth>>,
    generation: u64,
    now_utc: DateTime<Utc>,
    error_code: &'static str,
) {
    let mut guard = truth.write().await;
    if guard.generation != generation {
        return;
    }
    guard.last_tick_utc = Some(now_utc);
    guard.last_error_code = Some(error_code);
    guard.consecutive_error_count = guard.consecutive_error_count.saturating_add(1);
}

// ---------------------------------------------------------------------------
// D3.8/D3.9 — supervised worker loop
// ---------------------------------------------------------------------------

/// Bounded maximum worker restarts per daemon process.
const MAX_WORKER_RESTARTS: u32 = 3;
/// Bounded restart delay schedule, indexed by `restart_count - 1`.
const RESTART_DELAYS_SECS: [u64; 3] = [30, 60, 120];

/// Cancellation-aware, bounded-cadence tick loop, supervised for unexpected
/// worker exit or panic with a bounded restart budget. `tick_factory` builds
/// a fresh per-tick closure for each restart attempt (given the attempt's
/// generation number), so a test can inject deterministic panics/errors
/// without touching `AppState`'s production tick adapter at all.
///
/// Required per-restart-attempt order: capture `Utc::now()` once inside the
/// tick closure itself -> production adapter -> typed outcome handling ->
/// task-truth update. Ordinary tick errors are contained inside the tick
/// closure and never panic this loop; only a genuine worker panic or
/// unexpected exit reaches this function's own supervision.
pub async fn run_supervised_completed_bar_worker<Tick, Fut>(
    state: Arc<AppState>,
    cadence: Duration,
    mut cancel_rx: watch::Receiver<bool>,
    tick_factory: impl Fn(Arc<AppState>, u64) -> Tick,
) where
    Tick: Fn() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let environment = Some(state.deployment_mode().as_api_label().to_string());
    let mut restart_count: u32 = 0;

    loop {
        let generation = bump_generation_and_set_running(&state.completed_bar_task.truth).await;
        let on_tick = tick_factory(Arc::clone(&state), generation);
        let config = AutonomousCompletedBarDriverTaskConfig::new(cadence);
        let worker_cancel_rx = cancel_rx.clone();
        let handle = tokio::spawn(async move {
            run_bounded_cadence_task(config, worker_cancel_rx, on_tick).await
        });

        match handle.await {
            Ok(AutonomousCompletedBarDriverTaskLiveness::Stopped) => {
                // Expected cancellation: no restart, no failure alert.
                set_liveness_if_current_generation(
                    &state.completed_bar_task.truth,
                    generation,
                    AutonomousCompletedBarDriverTaskLiveness::Stopped,
                )
                .await;
                break;
            }
            Ok(_) | Err(_) => {
                restart_count += 1;
                record_restart_attempt(&state.completed_bar_task.truth, generation, restart_count)
                    .await;

                if restart_count > MAX_WORKER_RESTARTS {
                    set_liveness_if_current_generation(
                        &state.completed_bar_task.truth,
                        generation,
                        AutonomousCompletedBarDriverTaskLiveness::Failed,
                    )
                    .await;
                    notify_permanent_task_failure(&state, environment.clone()).await;
                    break;
                }

                notify_worker_restart(&state, environment.clone(), restart_count).await;

                let delay = Duration::from_secs(RESTART_DELAYS_SECS[(restart_count - 1) as usize]);
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    changed = cancel_rx.changed() => {
                        if changed.is_ok() && *cancel_rx.borrow() {
                            set_liveness_if_current_generation(
                                &state.completed_bar_task.truth,
                                generation,
                                AutonomousCompletedBarDriverTaskLiveness::Stopped,
                            )
                            .await;
                            break;
                        }
                    }
                }
            }
        }
    }

    state
        .completed_bar_task
        .claimed
        .store(false, Ordering::SeqCst);
}

async fn bump_generation_and_set_running(
    truth: &Arc<RwLock<AutonomousCompletedBarTaskTruth>>,
) -> u64 {
    let mut guard = truth.write().await;
    guard.generation += 1;
    guard.liveness = AutonomousCompletedBarDriverTaskLiveness::Running;
    guard.consecutive_error_count = 0;
    guard.generation
}

async fn set_liveness_if_current_generation(
    truth: &Arc<RwLock<AutonomousCompletedBarTaskTruth>>,
    generation: u64,
    liveness: AutonomousCompletedBarDriverTaskLiveness,
) {
    let mut guard = truth.write().await;
    if guard.generation == generation {
        guard.liveness = liveness;
    }
}

async fn record_restart_attempt(
    truth: &Arc<RwLock<AutonomousCompletedBarTaskTruth>>,
    generation: u64,
    restart_count: u32,
) {
    let mut guard = truth.write().await;
    if guard.generation == generation {
        guard.restart_count = restart_count;
    }
}

async fn notify_permanent_task_failure(state: &Arc<AppState>, environment: Option<String>) {
    state
        .set_autonomous_session_truth(AutonomousSessionTruth::CompletedBarDriverExited {
            detail: "completed-bar driver task exhausted its bounded restart budget and is now \
                     permanently stopped; unattended completed-bar dispatch is UNMANAGED"
                .to_string(),
        })
        .await;
    state
        .discord_notifier
        .notify_critical_alert(&CriticalAlertPayload {
            alert_class: "autonomous.completed_bar_task.failed".to_string(),
            severity: "critical".to_string(),
            summary: "Autonomous completed-bar driver task has permanently failed after \
                      exhausting its restart budget."
                .to_string(),
            detail: None,
            environment,
            run_id: None,
            ts_utc: Utc::now().to_rfc3339(),
        })
        .await;
}

async fn notify_worker_restart(
    state: &Arc<AppState>,
    environment: Option<String>,
    restart_count: u32,
) {
    warn!(
        restart_count,
        "autonomous_completed_bar_task: worker exited unexpectedly; scheduling bounded restart"
    );
    state
        .discord_notifier
        .notify_critical_alert(&CriticalAlertPayload {
            alert_class: "autonomous.completed_bar_task.restarting".to_string(),
            severity: "warning".to_string(),
            summary: format!(
                "Autonomous completed-bar driver task exited unexpectedly; restart attempt \
                 {restart_count} of {MAX_WORKER_RESTARTS} scheduled."
            ),
            detail: None,
            environment,
            run_id: None,
            ts_utc: Utc::now().to_rfc3339(),
        })
        .await;
}

// ---------------------------------------------------------------------------
// D3.10 — graceful shutdown
// ---------------------------------------------------------------------------

impl AppState {
    /// Request cancellation of the completed-bar task, if one is active.
    /// A no-op when no task has ever been spawned. Called from the
    /// daemon's existing shutdown path (`main.rs`) alongside
    /// `stop_for_shutdown()` — this cancels the completed-bar task only; it
    /// never touches the execution runtime.
    pub async fn cancel_completed_bar_task_for_shutdown(&self) {
        if let Some(tx) = self.completed_bar_task.cancel.lock().await.as_ref() {
            let _ = tx.send(true);
        }
    }

    /// Read-only production/test seam: the current process-local
    /// completed-bar task truth.
    pub async fn completed_bar_task_truth(&self) -> AutonomousCompletedBarTaskTruth {
        self.completed_bar_task.truth.read().await.clone()
    }

    /// Test seam: force the process-local completed-bar task truth's
    /// `generation`/`liveness`, independent of any real spawn. Named
    /// `_for_test` to signal intent; never called in production code.
    pub async fn set_completed_bar_task_generation_for_test(
        &self,
        generation: u64,
        liveness: AutonomousCompletedBarDriverTaskLiveness,
    ) {
        let mut guard = self.completed_bar_task.truth.write().await;
        guard.generation = generation;
        guard.liveness = liveness;
    }
}

/// Test seam: run exactly one production tick under an explicit generation,
/// exercising the same stale-generation guard the real supervised worker
/// uses. Named `_for_test` to signal intent; never called in production
/// code (the real call site is the closure built inside
/// `spawn_autonomous_completed_bar_driver_task`).
pub async fn run_one_production_tick_for_test(state: Arc<AppState>, generation: u64) {
    run_one_production_tick(state, generation).await;
}
