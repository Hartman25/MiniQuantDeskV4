//! AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-DURABLE-SESSION-COORDINATOR: the
//! durable daily-operation coordinator tick.
//!
//! This module owns every decision the production session controller used
//! to make with the process-local `locally_started: bool` (see
//! `session_controller.rs`'s former `run_session_controller_tick`):
//!
//! - canonical session-plan resolution (`autonomous_daily_operation`)
//! - canonical assignment / runtime-binding resolution
//!   (`multi_symbol_config` / `autonomous_runtime_context`)
//! - immutable operation identity derivation and race-safe create/recover
//!   (`mqk_db::autonomous_daily_operation`)
//! - preopen strict daily-data-readiness evaluation, without any provider
//!   call (`crate::daily_data_readiness`)
//! - typed autonomous arm (`AppState::try_autonomous_arm_typed`)
//! - the canonical start call (`AppState::start_execution_runtime`) and its
//!   typed retry classification (`autonomous_retry_policy`)
//! - durable start-attempt counting and bounded backoff
//! - runtime-ownership reconciliation and recovery after an unsafe-vs-safe
//!   run termination
//! - the canonical stop call (`AppState::stop_execution_runtime`) at session
//!   close, with durable stop-attempt evidence
//! - operator-managed-run preservation
//!
//! It never bypasses `AppState::start_execution_runtime`'s gate chain,
//! never invents a second start/stop gate, never fabricates a broker
//! lifecycle event, and never transitions to `completed` /
//! `completed_no_trade` / `completed_with_activity` (Phase E owns outcome
//! finalization). It does not read the wall clock: every caller supplies
//! `now_utc`.
//!
//! # AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-LIFECYCLE-CLOSURE-REPAIR-01
//!
//! This closure repair fixes twelve D2 lifecycle defects without beginning
//! Phase D3 (completed-bar task cutover):
//!
//! 1. Recovery retries no longer take the illegal `recovery_retrying ->
//!    start_retrying` edge — only `awaiting_open` transitions to
//!    `start_retrying` before the canonical start call
//!    ([`attempt_canonical_start`]).
//! 2. Start success, running state, run binding, `started_at_utc`, retry
//!    clearing, and the transition event are one atomic DB transaction
//!    (`mqk_db::transition_autonomous_daily_operation_to_running`), never a
//!    split `transition` + `record_running_started` sequence.
//! 3. Session-close reconciliation ([`handle_session_close`]) never
//!    represents a durably active orphaned run as stopped — it fetches the
//!    durable run row before ever recording `stopped_at_utc`.
//! 4. Every stop retry ([`retry_stop`]) re-proves run-ownership
//!    immediately before the canonical stop call, not once at routing time.
//! 5. Stop completion is restart-safe and idempotent
//!    (`mqk_db::record_autonomous_runtime_stopped`).
//! 6. The first observation of an operation created already past close
//!    reaches `stopping` via the legal `awaiting_open -> stopping` edge,
//!    never a fabricated `manual_intervention_required` jump.
//! 7. A durable, bounded blocker signature
//!    (`state_blocker_signature`, migration `0052`) makes manual/blocked
//!    dedup depend on `(state, state_reason_code, state_blocker_signature)`
//!    together, never `state_reason_code` alone.
//! 8. A same-day identity conflict durably transitions the *existing*
//!    operation to `manual_intervention_required` exactly once
//!    ([`handle_identity_conflict`]).
//! 9. A durable `DISARMED` arm state blocks recovery scheduling before any
//!    retry timestamp is persisted ([`handle_running`]).
//! 10. `run_durable_session_controller_tick` (`session_controller.rs`)
//!     projects every coordinator outcome onto `AutonomousSessionTruth`.
//! 11. A manual-intervention transition reports whether it was newly
//!     applied this tick (`ManualInterventionRequired.newly_applied`), so
//!     `session_controller.rs` can send exactly one critical notification
//!     per newly-applied blocker and zero for an unchanged one.
//! 12. An isolated end-to-end fixture proves a full coordinator-driven
//!     start and stop through the real production path
//!     (`scenario_autonomous_paper_day_lifecycle_auton12.rs`).
//!
//! # AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-NONTRADING-RECOVERY-AND-RUNNING-CONFIRMATION-01
//!
//! Phase D2 final closure, without beginning Phase D3:
//!
//! 1. A nontrading-day calendar result (`NotApplicable`) routes through the
//!    same relevant-existing-operation lookup the resolution-failure path
//!    already used, rather than returning immediately
//!    ([`resolve_or_reconcile_on_nontrading_day`]).
//! 2. [`mqk_db::fetch_relevant_open_autonomous_daily_operation`] also treats
//!    a bound-but-not-durably-stopped run (`run_id is not null and
//!    stopped_at_utc is null`) as relevant regardless of current state or
//!    window.
//! 3. Running-transition commit confirmation
//!    ([`running_transition_event_matches`]) queries the exact expected
//!    `transition_seq` via
//!    [`mqk_db::fetch_autonomous_daily_operation_event_at_sequence`], never
//!    scanning an ascending, 100-event-capped list.
//! 4. [`legal_degraded_target_after_uncertain_running_transition`] selects a
//!    legal degraded target after an uncertain running-transition store
//!    error, never attempting the illegal `running ->
//!    manual_intervention_required` edge.
//! 5. Every reason applied by
//!    [`reconcile_existing_operation_against_relevant_lookup`] now carries a
//!    full D1 typed blocker signature, never `None`.
//!
//! # Testability
//!
//! Unlike a fully effect-injected design, this coordinator drives the real
//! `AppState`/`mqk_db` seams directly (the same seams `start_execution_runtime`,
//! `try_autonomous_arm_typed`, and every existing gate-chain scenario test in
//! this crate already use as their test surface) rather than a second,
//! parallel fake-effects abstraction. Tests construct a real (test-scoped)
//! `AppState` against the isolated test Postgres and inject `now_utc` and
//! env-driven configuration, exactly as `scenario_daily_data_readiness_start_gate_01.rs`
//! and `scenario_autonomous_completed_bar_driver_01.rs` already do for the
//! gate chain and the completed-bar driver respectively. The individual
//! phase helpers below (`attempt_canonical_start`, `handle_running`,
//! `handle_session_close`, ...) are `pub` so tests can drive one phase in
//! isolation against a hand-built operation row without resolving a full
//! session plan every time.

use std::sync::Arc;

use anyhow::Context;
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::autonomous_daily_coverage_authority::{
    check_coverage_authority, check_operation_pristine, construct_coverage_bound_detail,
    coverage_construction_inputs_from_operation, resolve_current_coverage_policy_inputs,
    write_and_confirm_coverage_authority, CoverageAuthorityCheck, CoverageAuthorityEnsureResult,
    PristineCheckOutcome, REASON_COVERAGE_AUTHORITY_CONFLICT,
    REASON_COVERAGE_AUTHORITY_MISSING_AFTER_ACTIVITY, REASON_COVERAGE_AUTHORITY_UNREADABLE,
    REASON_COVERAGE_POLICY_CONSTRUCTION_FAILED, REASON_COVERAGE_POLICY_RESOLUTION_UNAVAILABLE,
};
use super::autonomous_daily_operation::{
    derive_assignment_identity, derive_autonomous_daily_operation_id,
    derive_runtime_binding_identity, resolve_autonomous_daily_session_plan_from_env,
    AutonomousDailyPlanTiming, AutonomousDailySessionPlan, AutonomousDailySessionPlanResolution,
};
use super::autonomous_retry_policy::{
    blocker_signature, classify_autonomous_reason, coordinator_reason_from_arm_rejection,
    coordinator_reason_from_runtime_lifecycle_error, next_retry_at, AutonomousBlockerIdentity,
    AutonomousCoordinatorReason, AutonomousRetryClass,
};
use super::autonomous_runtime_context::{
    resolve_autonomous_runtime_context, ResolvedAutonomousRuntimeContext,
};
use super::lifecycle::AutonomousArmOutcome;
use super::{AppState, MultiSymbolRuntimeConfig};

use mqk_db::{
    AutonomousDailyOperationRecord, AutonomousDailyTransitionOutcome,
    CreateAutonomousDailyOperationArgs, CreateOrRecoverAutonomousDailyOperationOutcome,
    TransitionAutonomousDailyOperationArgs, STATE_AWAITING_OPEN, STATE_AWAITING_PREOPEN,
    STATE_MANUAL_INTERVENTION_REQUIRED, STATE_PREFLIGHT_BLOCKED, STATE_PREPARING_DATA,
    STATE_RECOVERY_RETRYING, STATE_RUNNING, STATE_START_RETRYING, STATE_STOPPING,
    STATE_STOP_RETRYING,
};

const MAX_BOUNDED_DETAIL: usize = 4000;
const MAX_REASON_CODE: usize = 128;
const MAX_BLOCKER_SIGNATURE: usize = 128;

fn bounded_detail(s: impl Into<String>) -> String {
    let mut s = s.into();
    if s.len() > MAX_BOUNDED_DETAIL {
        s.truncate(MAX_BOUNDED_DETAIL);
    }
    s
}

fn bounded_reason(s: &str) -> String {
    if s.len() > MAX_REASON_CODE {
        s[..MAX_REASON_CODE].to_string()
    } else {
        s.to_string()
    }
}

fn bounded_signature(s: String) -> String {
    if s.len() > MAX_BLOCKER_SIGNATURE {
        s[..MAX_BLOCKER_SIGNATURE].to_string()
    } else {
        s
    }
}

fn parse_market_date(s: &str) -> anyhow::Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| {
        anyhow::anyhow!("autonomous_daily_coordinator: invalid market_date '{s}': {e}")
    })
}

// ---------------------------------------------------------------------------
// D2.2 — Injected coordinator tick input/outcome
// ---------------------------------------------------------------------------

/// Injected tick input. The coordinator reads no wall clock of its own —
/// the production session loop passes `Utc::now()` once per tick; tests
/// inject a fixed instant.
pub struct AutonomousDailyCoordinatorTickInput<'a> {
    pub state: &'a Arc<AppState>,
    pub now_utc: DateTime<Utc>,
}

/// Typed, bounded outcome of one coordinator tick (D2.20). Never a
/// free-form string — every reason code carried here is a stable,
/// closed-set label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutonomousDailyCoordinatorTickOutcome {
    /// No applicable trading operation today (weekend / exchange holiday).
    NotApplicable {
        reason_code: &'static str,
    },
    /// Calendar truth could not be established, or a configured override is
    /// invalid.
    CalendarBlocked {
        reason_code: &'static str,
    },
    /// Assignment or runtime-binding resolution failed before an operation
    /// identity could even be computed.
    IdentityBlocked {
        reason_code: &'static str,
    },
    WaitingForPreopen,
    PreparingData,
    AwaitingOpen,
    PreflightBlocked {
        reason_code: &'static str,
    },
    RetryNotDue,
    StartAttempted,
    Started {
        run_id: Uuid,
    },
    Running {
        run_id: Uuid,
    },
    RecoveryScheduled,
    Recovered {
        run_id: Uuid,
    },
    /// REPAIR 11: `newly_applied` is `true` only when this exact tick
    /// durably applied a new (or changed) manual/controller-degraded
    /// transition — `false` for an unchanged blocker still being reported
    /// (a passthrough of already-persisted truth, or a same-blocker replay
    /// this tick chose not to re-write). `log_coordinator_outcome`
    /// (`session_controller.rs`) uses this — never process-local memory —
    /// as the sole authority for whether to send one critical notification.
    ManualInterventionRequired {
        reason_code: &'static str,
        newly_applied: bool,
    },
    StopAttempted,
    RuntimeStopped,
    /// Stopped and awaiting a later coordinator tick's finalization attempt
    /// -- either finalization has not been attempted yet this tick, or the
    /// E2B classifier reported `NotEligible` (e.g. a matching local runtime
    /// is still active this tick per E1 §3.2 condition 4). No write, no
    /// notification.
    AwaitingOutcomeFinalization,
    /// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-
    /// INTEGRATION-AND-NOTIFICATION: this tick's E2B finalization attempt
    /// durably reached (or confirmed reaching, via an authoritative re-read)
    /// a terminal `completed_no_trade`/`completed_with_activity` state for
    /// the first time. `session_controller.rs`'s `log_coordinator_outcome`
    /// sends exactly one outcome notification for this variant -- the CAS
    /// success itself (`Finalized`, never `AlreadyFinalized`) is the sole
    /// dedup authority, never process-local memory.
    OutcomeFinalized {
        operation_id: Uuid,
        run_id: Option<Uuid>,
        outcome_reason_code: String,
    },
    /// The operation was already terminal (a complete automatic
    /// `completed_no_trade`/`completed_with_activity` row, or the generic
    /// administrative `completed` state) before this tick began -- read-only
    /// projection, no classifier re-run, no notification.
    OutcomeAlreadyFinalized {
        state: String,
        outcome_reason_code: Option<String>,
    },
    /// The E2B classifier could not resolve a terminal outcome this tick;
    /// the durable `evidence_degraded` blocker was applied, confirmed
    /// applied, or refreshed in place. `newly_applied` is the durable-CAS-
    /// derived dedup authority `log_coordinator_outcome` uses to decide
    /// whether to send exactly one warning notification -- mirrors the
    /// existing `ManualInterventionRequired.newly_applied` pattern.
    OutcomeEvidenceDegraded {
        operation_id: Uuid,
        run_id: Option<Uuid>,
        reason_code: &'static str,
        newly_applied: bool,
    },
    /// A previously `evidence_degraded` (post-stop) operation's evidence now
    /// resolves cleanly; the operation took the existing
    /// `evidence_degraded -> stopping` edge this tick. A later tick performs
    /// the terminal CAS. No notification -- this is not itself a terminal
    /// or blocker outcome.
    OutcomeRecoveredToStopping,
    /// This tick's finalization attempt could not read (or confirm a write
    /// against) the database. No write was claimed persisted; no
    /// notification. Retried on a future tick per the E1 §9 database-failure
    /// contract.
    OutcomeFinalizationDatabaseUnavailable,
    /// This tick's finalization attempt found the operation already
    /// terminal at a different state/outcome than this attempt's own
    /// classification would have produced -- never rewritten, no repeated
    /// notification.
    OutcomeFinalizationConflict,
}

// ---------------------------------------------------------------------------
// D2.4-D2.6 — Top-level tick: identity resolution + create/recover
// ---------------------------------------------------------------------------

/// Production/test entry point: resolve one canonical session plan,
/// canonical assignment, canonical runtime binding, and immutable operation
/// identity for `input.now_utc`; race-safely create or recover the daily
/// operation; then drive it through exactly one durable CAS transition
/// step.
pub async fn tick_autonomous_daily_coordinator(
    input: AutonomousDailyCoordinatorTickInput<'_>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    let AutonomousDailyCoordinatorTickInput { state, now_utc } = input;

    // Calendar resolution is pure calendar math — it must never require a
    // DB call, preserving the pre-existing "zero DB call on a weekend/
    // holiday tick" guarantee. `deployment_mode`/`adapter_id` are likewise
    // pure `AppState` reads with no DB dependency.
    let deployment_mode = state.deployment_mode().as_db_mode();
    let adapter_id = state.adapter_id().to_string();

    let timing = AutonomousDailyPlanTiming::production_default();
    let resolution = resolve_autonomous_daily_session_plan_from_env(now_utc, &timing);

    let plan = match resolution {
        AutonomousDailySessionPlanResolution::NotApplicable { reason_code, .. } => {
            // REPAIR 1/3 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-NONTRADING-
            // RECOVERY-AND-RUNNING-CONFIRMATION-01): a nontrading-day
            // (weekend/exchange holiday) calendar result must not abandon an
            // existing unresolved durable operation. Only when a DB is
            // actually configured is there anything to look up a relevant
            // existing operation against.
            return match state.db.clone() {
                None => Ok(AutonomousDailyCoordinatorTickOutcome::NotApplicable {
                    reason_code: reason_code.as_str(),
                }),
                Some(pool) => {
                    resolve_or_reconcile_on_nontrading_day(
                        state,
                        &pool,
                        deployment_mode,
                        &adapter_id,
                        now_utc,
                        reason_code.as_str(),
                    )
                    .await
                }
            };
        }
        AutonomousDailySessionPlanResolution::Blocked { reason_code, .. } => {
            // REPAIR 1/2 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-FAILSAFE-
            // RECOVERY-CLOSURE-01): only when a DB is actually configured is
            // there anything to look up a relevant existing operation
            // against — a total DB outage still reports the ordinary
            // CalendarBlocked outcome here, exactly as before this patch,
            // rather than fabricating a DB-dependent degrade attempt.
            return match state.db.clone() {
                None => Ok(AutonomousDailyCoordinatorTickOutcome::CalendarBlocked {
                    reason_code: reason_code.as_str(),
                }),
                Some(pool) => {
                    resolve_or_degrade_on_resolution_failure(
                        state,
                        &pool,
                        deployment_mode,
                        &adapter_id,
                        now_utc,
                        "calendar_resolution_unavailable",
                        AutonomousDailyCoordinatorTickOutcome::CalendarBlocked {
                            reason_code: reason_code.as_str(),
                        },
                    )
                    .await
                }
            };
        }
        AutonomousDailySessionPlanResolution::Applicable(plan) => plan,
    };

    let Some(pool) = state.db.clone() else {
        // No operation can exist without a DB, so there is nothing to dedup
        // against here. A total DB outage is deliberately never
        // deduplicated to silence — the operator must keep hearing about it.
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "database_not_configured_or_invalid",
                newly_applied: true,
            },
        );
    };

    let config = match crate::state::build_multi_symbol_runtime_config_from_env() {
        Ok(config) => config,
        Err(_err) => {
            return resolve_or_degrade_on_resolution_failure(
                state,
                &pool,
                deployment_mode,
                &adapter_id,
                now_utc,
                "assignment_resolution_unavailable",
                AutonomousDailyCoordinatorTickOutcome::IdentityBlocked {
                    reason_code: "assignment_missing",
                },
            )
            .await;
        }
    };

    let runtime_context = match resolve_autonomous_runtime_context(state).await {
        Ok(ctx) => ctx,
        Err(_err) => {
            return resolve_or_degrade_on_resolution_failure(
                state,
                &pool,
                deployment_mode,
                &adapter_id,
                now_utc,
                "runtime_binding_resolution_unavailable",
                AutonomousDailyCoordinatorTickOutcome::IdentityBlocked {
                    reason_code: "runtime_binding_unresolved",
                },
            )
            .await;
        }
    };

    let assignment_identity = derive_assignment_identity(&config);
    let runtime_binding_identity =
        derive_runtime_binding_identity(&runtime_context.effective_runtime_binding);
    let operation_id = derive_autonomous_daily_operation_id(
        &plan,
        deployment_mode,
        &adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
    );

    let (operation, created) = match create_or_recover(
        state,
        &pool,
        &plan,
        operation_id,
        deployment_mode,
        &adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
        now_utc,
    )
    .await?
    {
        Ok(pair) => pair,
        Err(outcome) => return Ok(outcome),
    };

    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-COVERAGE-ANCHOR-AND-RUN-
    // LINEAGE-FOUNDATION closure (REPAIR 6): deterministic two-step
    // rendezvous for the coordinator/adapter live concurrency proof only.
    // `operation_visible` is notified immediately after `create_or_recover`
    // above has durably committed the operation row -- and before this tick
    // proceeds to bind or verify the coverage authority -- exactly the
    // window a concurrently scheduled completed-bar adapter tick could
    // observe the operation row before this coordinator tick has bound its
    // authority. The coordinator then awaits `proceed` before continuing.
    // Production never installs this hook (`AppState`'s hook slot defaults
    // to `None`), so the production cost is exactly one uncontended async
    // mutex lock per coordinator tick, and the coordinator never waits.
    if let Some(hook) = state.coverage_authority_pre_bind_test_hook_for_test().await {
        hook.operation_visible.notify_waiters();
        hook.proceed.notified().await;
    }

    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-COVERAGE-ANCHOR-AND-RUN-
    // LINEAGE-FOUNDATION (§6a): ensure the operation-scoped coverage anchor
    // exists (or matches) before this tick may proceed to `dispatch_by_state`
    // -- runs for both newly created and recovered operations, strictly
    // after `create_or_recover` and strictly before any state-handler call.
    if let Some(blocked_outcome) = ensure_coverage_authority(
        state,
        &pool,
        &operation,
        &config,
        &runtime_context,
        &assignment_identity,
        &runtime_binding_identity,
        now_utc,
    )
    .await?
    {
        return Ok(blocked_outcome);
    }

    // REPAIR 6: an operation first observed already past close never
    // fabricates a running/stopping history it never had, but it also
    // never invents a manual-intervention jump when the honest legal edge
    // (`awaiting_open -> stopping`, added by D2) already exists. Zero start
    // calls, zero stop calls: the operation never had a run_id, so no local
    // runtime could possibly be attributed to it.
    if created && now_utc >= plan.effective_operation_close_utc {
        let (updated, _) = apply_transition(
            &pool,
            &operation,
            STATE_STOPPING,
            None,
            None,
            now_utc,
            None,
            "operation first observed at or after effective_operation_close_utc; \
             no runtime was ever started by this coordinator for this operation",
        )
        .await?;
        mqk_db::record_autonomous_runtime_stopped(&pool, updated.operation_id, now_utc).await?;
        return Ok(AutonomousDailyCoordinatorTickOutcome::AwaitingOutcomeFinalization);
    }

    dispatch_by_state(state, &pool, operation, &plan, now_utc).await
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-FAILSAFE-RECOVERY-CLOSURE-01
// REPAIR 1/2 — existing-operation fallback under current resolution failure
// ---------------------------------------------------------------------------

/// Bounded detail text shared by
/// [`reconcile_existing_operation_against_relevant_lookup`]'s two callers.
const RESOLUTION_FAILURE_RECONCILE_DETAIL: &str =
    "current calendar/assignment/registry/runtime-context resolution failed this \
     tick; a relevant existing durable operation was found, so lifecycle control \
     is preserved using persisted operation truth alone rather than erased";

/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-NONTRADING-RECOVERY-AND-RUNNING-
/// CONFIRMATION-01 REPAIR 1/3: `now_utc` resolved to a nontrading day
/// (weekend/exchange holiday) this tick.
const NONTRADING_DAY_RECONCILE_DETAIL: &str =
    "today resolved to a nontrading day (weekend or exchange holiday); a relevant \
     existing durable operation was found, so lifecycle control is preserved using \
     persisted operation truth alone rather than abandoned";

/// REPAIR 2: when current calendar/assignment/registry/runtime-context
/// resolution fails this tick, look up whether a relevant existing durable
/// operation already exists for this exact slot family (REPAIR 1). If none
/// exists, there is nothing to preserve lifecycle control over — return the
/// ordinary typed blocked outcome unchanged (no row is created, no
/// start/stop occurs). If one exists, hand off to
/// [`reconcile_existing_operation_against_relevant_lookup`] rather than
/// reporting the blocked outcome directly: a current configuration failure
/// must never erase lifecycle control over an already-created operation.
async fn resolve_or_degrade_on_resolution_failure(
    state: &Arc<AppState>,
    pool: &PgPool,
    deployment_mode: &str,
    adapter_id: &str,
    now_utc: DateTime<Utc>,
    degrade_reason_code: &'static str,
    blocked_outcome: AutonomousDailyCoordinatorTickOutcome,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    let existing = mqk_db::fetch_relevant_open_autonomous_daily_operation(
        pool,
        deployment_mode,
        adapter_id,
        now_utc,
    )
    .await?;
    match existing {
        None => Ok(blocked_outcome),
        Some(operation) => {
            reconcile_existing_operation_against_relevant_lookup(
                state,
                pool,
                operation,
                degrade_reason_code,
                now_utc,
                RESOLUTION_FAILURE_RECONCILE_DETAIL,
            )
            .await
        }
    }
}

/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-NONTRADING-RECOVERY-AND-RUNNING-
/// CONFIRMATION-01 REPAIR 1/3: a nontrading-day calendar result (weekend or
/// exchange holiday) must not abandon an existing unresolved durable
/// operation. Looks up whether a relevant existing operation already exists
/// for this exact slot family (REPAIR 2's extended
/// `fetch_relevant_open_autonomous_daily_operation`). If none exists, the
/// ordinary `NotApplicable` outcome is reported unchanged — no row is
/// created, no arm/start/stop call occurs, and a normal weekend/holiday with
/// no unresolved operation remains quiet. If one exists, hand off to
/// [`reconcile_existing_operation_against_relevant_lookup`]: today being a
/// nontrading day never derives a new operation identity and never requires
/// current assignment/registry/runtime-context resolution merely to stop or
/// reconcile the existing operation.
async fn resolve_or_reconcile_on_nontrading_day(
    state: &Arc<AppState>,
    pool: &PgPool,
    deployment_mode: &str,
    adapter_id: &str,
    now_utc: DateTime<Utc>,
    reason_code: &'static str,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    let existing = mqk_db::fetch_relevant_open_autonomous_daily_operation(
        pool,
        deployment_mode,
        adapter_id,
        now_utc,
    )
    .await?;
    match existing {
        None => Ok(AutonomousDailyCoordinatorTickOutcome::NotApplicable { reason_code }),
        Some(operation) => {
            reconcile_existing_operation_against_relevant_lookup(
                state,
                pool,
                operation,
                reason_code,
                now_utc,
                NONTRADING_DAY_RECONCILE_DETAIL,
            )
            .await
        }
    }
}

/// REPAIR 1/2/3/6: given a relevant existing operation and a current
/// calendar/assignment/registry/runtime-context resolution failure *or* a
/// nontrading-day calendar result, never create another operation, never
/// rewrite immutable identity, and never attempt a new runtime start or
/// recovery start without freshly proven canonical configuration — use
/// persisted operation truth alone to maintain safe lifecycle control.
///
/// - At or after the operation's own persisted `effective_operation_close_utc`
///   (and not already stopping/terminal): canonical close/stop
///   reconciliation via [`handle_session_close`], which needs only the
///   durable operation and current local runtime ownership — the current
///   external registry/override does not need to be readable merely to
///   stop the exact runtime already bound to this operation.
/// - Already `stopping`/`stop_retrying`: continue restart-safe stop
///   reconciliation via [`handle_stopping`] using the operation's own
///   persisted `postclose_finalize_utc` — no current assignment or binding
///   resolution required.
/// - `running` (still before close): degrade to `controller_degraded` via
///   its one legal edge — no runtime interaction at all (never stops an
///   unrelated runtime, never attaches an operator-managed one). This also
///   covers the case where the existing operation is unexpectedly before
///   its persisted effective close on a current nontrading day: it is never
///   started or recovered, only degraded.
/// - Already blocked/degraded (`manual_intervention_required` /
///   `controller_degraded` / `evidence_degraded`): refresh the reason in
///   place (REPAIR 4), remaining in the same state.
/// - Any other pre-running state with a legal edge to
///   `manual_intervention_required`: take it. No legal edge: report the
///   failure without forcing an illegal transition.
///
/// REPAIR 6: the applied/refreshed reason always carries a full D1 typed
/// blocker signature (never `None`) bound to the operation's own identity —
/// the same operation + same failure reason always produces the same
/// signature (no repeated event/notification for an unchanged blocker), and
/// a changed reason produces a changed signature (exactly one refresh
/// event). No free-form filesystem/registry/environment detail, timestamp,
/// or secret ever enters the signature.
async fn reconcile_existing_operation_against_relevant_lookup(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: AutonomousDailyOperationRecord,
    reason_code: &'static str,
    now_utc: DateTime<Utc>,
    detail: &'static str,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    if now_utc >= operation.effective_operation_close_utc
        && !matches!(
            operation.state.as_str(),
            STATE_STOPPING
                | STATE_STOP_RETRYING
                | STATE_MANUAL_INTERVENTION_REQUIRED
                | mqk_db::STATE_COMPLETED
                | mqk_db::STATE_COMPLETED_NO_TRADE
                | mqk_db::STATE_COMPLETED_WITH_ACTIVITY
                | mqk_db::STATE_CALENDAR_UNAVAILABLE
        )
    {
        return handle_session_close(state, pool, operation, now_utc).await;
    }

    let reason = AutonomousCoordinatorReason::UnclassifiedFailClosed {
        fault_class: reason_code,
    };
    let signature = blocker_signature_for(&operation, &reason);

    match operation.state.as_str() {
        STATE_STOPPING | STATE_STOP_RETRYING => {
            let postclose_finalize_utc = operation.postclose_finalize_utc;
            handle_stopping(state, pool, operation, postclose_finalize_utc, now_utc).await
        }
        STATE_RUNNING => {
            let newly_applied = apply_manual_if_changed(
                pool,
                &operation,
                signature.reason_code,
                signature.stable_context.clone(),
                now_utc,
                detail,
                mqk_db::STATE_CONTROLLER_DEGRADED,
            )
            .await?;
            Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: signature.reason_code,
                    newly_applied,
                },
            )
        }
        // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-
        // INTEGRATION-AND-NOTIFICATION (E3.11): a stopped, finalization-
        // eligible operation this same resolution-failure tick already
        // degraded via `handle_outcome_finalization` (routed above through
        // the `STATE_STOPPING`/`STATE_STOP_RETRYING` arm) must never be
        // abandoned to this generic resolution-failure blocker on a later
        // tick -- that would silently overwrite E2B's own durable
        // `unknown_*` reason/signature with an unrelated one and produce a
        // spurious "newly applied" write on every such tick. Route back into
        // the same finalization seam instead, exactly like the
        // `STATE_STOPPING`/`STATE_STOP_RETRYING` arm above.
        mqk_db::STATE_EVIDENCE_DEGRADED if operation.stopped_at_utc.is_some() => {
            handle_outcome_finalization(state, pool, operation, now_utc).await
        }
        STATE_MANUAL_INTERVENTION_REQUIRED
        | mqk_db::STATE_CONTROLLER_DEGRADED
        | mqk_db::STATE_EVIDENCE_DEGRADED => {
            let target: &'static str = match operation.state.as_str() {
                mqk_db::STATE_CONTROLLER_DEGRADED => mqk_db::STATE_CONTROLLER_DEGRADED,
                mqk_db::STATE_EVIDENCE_DEGRADED => mqk_db::STATE_EVIDENCE_DEGRADED,
                _ => STATE_MANUAL_INTERVENTION_REQUIRED,
            };
            let newly_applied = apply_manual_if_changed(
                pool,
                &operation,
                signature.reason_code,
                signature.stable_context.clone(),
                now_utc,
                detail,
                target,
            )
            .await?;
            Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: signature.reason_code,
                    newly_applied,
                },
            )
        }
        other => {
            if mqk_db::is_legal_operation_transition(
                Some(other),
                STATE_MANUAL_INTERVENTION_REQUIRED,
            ) {
                let newly_applied = apply_manual_if_changed(
                    pool,
                    &operation,
                    signature.reason_code,
                    signature.stable_context.clone(),
                    now_utc,
                    detail,
                    STATE_MANUAL_INTERVENTION_REQUIRED,
                )
                .await?;
                Ok(
                    AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                        reason_code: signature.reason_code,
                        newly_applied,
                    },
                )
            } else {
                Ok(
                    AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                        reason_code: signature.reason_code,
                        newly_applied: false,
                    },
                )
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn create_or_recover(
    state: &Arc<AppState>,
    pool: &PgPool,
    plan: &AutonomousDailySessionPlan,
    operation_id: Uuid,
    deployment_mode: &str,
    adapter_id: &str,
    assignment_identity: &str,
    runtime_binding_identity: &str,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<
    Result<(AutonomousDailyOperationRecord, bool), AutonomousDailyCoordinatorTickOutcome>,
> {
    let initial_state = initial_state_for_plan(plan, now_utc);
    let create_args = CreateAutonomousDailyOperationArgs {
        operation_id,
        market_date: parse_market_date(&plan.market_date)?,
        deployment_mode: deployment_mode.to_string(),
        adapter_id: adapter_id.to_string(),
        session_plan_identity: plan.session_plan_identity.clone(),
        assignment_identity: assignment_identity.to_string(),
        runtime_binding_identity: runtime_binding_identity.to_string(),
        calendar_source: plan.calendar_source.clone(),
        calendar_coverage_state: plan.calendar_coverage_state.clone(),
        schedule_source: plan.schedule_source.as_str().to_string(),
        effective_operation_open_utc: plan.effective_operation_open_utc,
        effective_operation_close_utc: plan.effective_operation_close_utc,
        exchange_session_open_utc: plan.exchange_session_open_utc,
        exchange_session_close_utc: plan.exchange_session_close_utc,
        exchange_is_early_close: plan.exchange_is_early_close,
        previous_trading_date: parse_market_date(&plan.previous_trading_date)?,
        preopen_start_utc: plan.preopen_start_utc,
        postclose_finalize_utc: plan.postclose_finalize_utc,
        initial_state: initial_state.to_string(),
        data_refresh_state: "not_started".to_string(),
        occurred_at_utc: now_utc,
        bounded_detail: bounded_detail("autonomous daily coordinator: operation created"),
        stop_attempt_count: 0,
    };

    match mqk_db::create_or_recover_autonomous_daily_operation(pool, &create_args).await? {
        CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => Ok(Ok((record, true))),
        CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(record) => {
            Ok(Ok((record, false)))
        }
        CreateOrRecoverAutonomousDailyOperationOutcome::IdentityConflict {
            existing_operation_id,
            ..
        } => {
            let outcome = handle_identity_conflict(
                state,
                pool,
                existing_operation_id,
                assignment_identity,
                runtime_binding_identity,
                now_utc,
            )
            .await?;
            Ok(Err(outcome))
        }
    }
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-COVERAGE-ANCHOR-AND-RUN-LINEAGE-
// FOUNDATION (§6a): coordinator ensure-authority seam.
// ---------------------------------------------------------------------------

/// E2A closure REPAIR 6: deterministic two-step rendezvous for the real
/// coordinator/adapter inter-task concurrency proof only (mirrors
/// [`super::autonomous_completed_bar_driver::AutonomousCompletedBarPostClaimTestHook`]'s
/// established pattern). `operation_visible` is notified immediately after
/// `create_or_recover` commits the operation row and before this tick binds
/// or verifies the coverage authority; the coordinator then awaits
/// `proceed` before continuing. Production never installs this hook
/// (`AppState`'s hook slot defaults to `None`), so the production cost is
/// exactly one uncontended async mutex lock per coordinator tick, and the
/// coordinator never waits.
#[derive(Default)]
pub struct AutonomousCoverageAuthorityPreBindTestHook {
    pub operation_visible: tokio::sync::Notify,
    pub proceed: tokio::sync::Notify,
}

/// Ensure the operation-scoped `autonomous_daily_coverage_bound` authority
/// exists and matches this tick's freshly-resolved policy, before the
/// coordinator may proceed to `dispatch_by_state`.
///
/// Returns `Ok(None)` when the tick may proceed (exact replay matched, or a
/// pristine anchor was freshly bound). Returns `Ok(Some(outcome))` when the
/// tick must stop here -- the returned outcome is already the durably
/// applied (or refreshed) blocker disposition.
#[allow(clippy::too_many_arguments)]
async fn ensure_coverage_authority(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    config: &MultiSymbolRuntimeConfig,
    runtime_context: &ResolvedAutonomousRuntimeContext,
    assignment_identity: &str,
    runtime_binding_identity: &str,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<Option<AutonomousDailyCoordinatorTickOutcome>> {
    let readiness_context = crate::daily_data_readiness::load_readiness_context_from_env();

    let policy = match resolve_current_coverage_policy_inputs(
        config,
        &runtime_context.effective_runtime_binding,
        &readiness_context.strategy_registry,
    ) {
        Ok(policy) => policy,
        Err(_) => {
            return Ok(Some(
                apply_coverage_blocker(
                    state,
                    pool,
                    operation,
                    REASON_COVERAGE_POLICY_RESOLUTION_UNAVAILABLE,
                    mqk_db::STATE_CONTROLLER_DEGRADED,
                    now_utc,
                )
                .await?,
            ));
        }
    };

    let Some(inputs) = coverage_construction_inputs_from_operation(
        operation,
        assignment_identity,
        runtime_binding_identity,
        &policy,
    ) else {
        return Ok(Some(
            apply_coverage_blocker(
                state,
                pool,
                operation,
                REASON_COVERAGE_POLICY_CONSTRUCTION_FAILED,
                mqk_db::STATE_CONTROLLER_DEGRADED,
                now_utc,
            )
            .await?,
        ));
    };

    let fresh = match construct_coverage_bound_detail(
        readiness_context.calendar_provider.as_ref(),
        &inputs,
    ) {
        Ok(fresh) => fresh,
        Err(_) => {
            return Ok(Some(
                apply_coverage_blocker(
                    state,
                    pool,
                    operation,
                    REASON_COVERAGE_POLICY_CONSTRUCTION_FAILED,
                    mqk_db::STATE_CONTROLLER_DEGRADED,
                    now_utc,
                )
                .await?,
            ));
        }
    };

    match check_coverage_authority(pool, operation.operation_id, &fresh).await? {
        CoverageAuthorityCheck::Compatible(_) => Ok(None),
        CoverageAuthorityCheck::NotBound => {
            // Missing anchor: only a pristine (no prior activity) operation
            // may bind one now -- an operation with any activity signal
            // must never receive a retroactively fabricated anchor.
            match check_operation_pristine(pool, operation).await {
                Ok(PristineCheckOutcome::Pristine) => {
                    match write_and_confirm_coverage_authority(pool, &fresh, now_utc).await? {
                        CoverageAuthorityEnsureResult::Bound(_) => Ok(None),
                        CoverageAuthorityEnsureResult::Conflict
                        | CoverageAuthorityEnsureResult::Unreadable => Ok(Some(
                            apply_coverage_blocker(
                                state,
                                pool,
                                operation,
                                REASON_COVERAGE_AUTHORITY_CONFLICT,
                                mqk_db::STATE_CONTROLLER_DEGRADED,
                                now_utc,
                            )
                            .await?,
                        )),
                    }
                }
                Ok(PristineCheckOutcome::HasActivity) | Err(_) => Ok(Some(
                    apply_coverage_blocker(
                        state,
                        pool,
                        operation,
                        REASON_COVERAGE_AUTHORITY_MISSING_AFTER_ACTIVITY,
                        mqk_db::STATE_EVIDENCE_DEGRADED,
                        now_utc,
                    )
                    .await?,
                )),
            }
        }
        CoverageAuthorityCheck::Unreadable => Ok(Some(
            apply_coverage_blocker(
                state,
                pool,
                operation,
                REASON_COVERAGE_AUTHORITY_UNREADABLE,
                mqk_db::STATE_CONTROLLER_DEGRADED,
                now_utc,
            )
            .await?,
        )),
        CoverageAuthorityCheck::Invalid | CoverageAuthorityCheck::Conflict => Ok(Some(
            apply_coverage_blocker(
                state,
                pool,
                operation,
                REASON_COVERAGE_AUTHORITY_CONFLICT,
                mqk_db::STATE_CONTROLLER_DEGRADED,
                now_utc,
            )
            .await?,
        )),
    }
}

/// Apply the durable fail-closed disposition for a coverage-authority
/// problem, reusing the exact D1 blocker-signature mechanism and the same
/// source-aligned state-mapping shape [`handle_identity_conflict`] already
/// uses for the identity-conflict class: `running` degrades to
/// `running_target` (`controller_degraded` for an unreadable/invalid/
/// conflicting anchor, `evidence_degraded` for missing-after-activity, per
/// §6a); an already blocked/degraded operation refreshes its reason/
/// signature in place; every other nonterminal state with a legal edge to
/// `manual_intervention_required` takes it; a terminal state is never
/// mutated. Close priority: at or after `effective_operation_close_utc`,
/// canonical close/stop reconciliation always takes precedence over a fresh
/// coverage blocker -- a coverage-authority problem must never strand a
/// runtime past close.
async fn apply_coverage_blocker(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    reason_code: &'static str,
    running_target: &'static str,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    if mqk_db::is_terminal_operation_state(&operation.state) {
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code,
                newly_applied: false,
            },
        );
    }

    if now_utc >= operation.effective_operation_close_utc
        && !matches!(
            operation.state.as_str(),
            STATE_STOPPING | STATE_STOP_RETRYING | STATE_MANUAL_INTERVENTION_REQUIRED
        )
    {
        // Close priority: matching runtime stop/close authority always
        // retains priority over a fresh coverage blocker -- never strand a
        // runtime past close merely because coverage authority is missing
        // or in conflict.
        return handle_session_close(state, pool, operation.clone(), now_utc).await;
    }

    let reason = AutonomousCoordinatorReason::UnclassifiedFailClosed {
        fault_class: reason_code,
    };
    let signature = blocker_signature(
        &reason,
        &AutonomousBlockerIdentity {
            operation_id: Some(operation.operation_id),
            ..Default::default()
        },
    );

    let detail = "autonomous daily coordinator: coverage authority could not be ensured for this \
                  operation this tick";

    let target_state: &'static str = match operation.state.as_str() {
        STATE_RUNNING => running_target,
        STATE_MANUAL_INTERVENTION_REQUIRED => STATE_MANUAL_INTERVENTION_REQUIRED,
        mqk_db::STATE_CONTROLLER_DEGRADED => mqk_db::STATE_CONTROLLER_DEGRADED,
        mqk_db::STATE_EVIDENCE_DEGRADED => mqk_db::STATE_EVIDENCE_DEGRADED,
        _ => STATE_MANUAL_INTERVENTION_REQUIRED,
    };

    if !mqk_db::is_legal_operation_transition(Some(&operation.state), target_state)
        && operation.state.as_str() != target_state
    {
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: signature.reason_code,
                newly_applied: false,
            },
        );
    }

    let newly_applied = apply_manual_if_changed(
        pool,
        operation,
        signature.reason_code,
        signature.stable_context.clone(),
        now_utc,
        detail,
        target_state,
    )
    .await?;
    Ok(
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: signature.reason_code,
            newly_applied,
        },
    )
}

/// REPAIR 8 / REPAIR 3 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-FAILSAFE-
/// RECOVERY-CLOSURE-01): durable identity-conflict truth. The daily slot
/// already holds a row whose immutable identity fields differ from the
/// freshly computed expected values (e.g. an operator changed
/// strategy/symbol/timeframe config and restarted mid-day). This never
/// creates a second row and never rewrites the existing row's immutable
/// identity fields; it durably CAS-transitions the *existing* operation
/// exactly once, deduplicated by `(state, state_reason_code,
/// state_blocker_signature)`, and survives a process restart because that
/// dedup is entirely DB-driven.
///
/// REPAIR 3: a `running` existing operation is never left unpersisted —
/// `running -> manual_intervention_required` is not a legal edge, but
/// `running -> controller_degraded` is, so a running conflict durably
/// degrades in place rather than being silently dropped. An existing
/// operation that is already blocked/degraded
/// (`manual_intervention_required` / `controller_degraded` /
/// `evidence_degraded`) refreshes its reason/signature in place (REPAIR 4)
/// and stays in its current state, per the binding contract's "remaining in
/// the same blocked/degraded state" requirement. Every other pre-running or
/// closing state with a legal edge to `manual_intervention_required`
/// (`awaiting_preopen`, `preparing_data`, `awaiting_open`,
/// `preflight_blocked`, `start_retrying`, `recovery_retrying`, `stopping`,
/// `stop_retrying`) continues to transition there, unchanged from before.
///
/// REPAIR 3 also closes a second gap: an identity conflict that recurs
/// every tick (the freshly computed identity never again matches the
/// existing row's immutable identity — e.g. an operator's config edit was
/// never reverted) must never strand the runtime the existing operation
/// already bound. Once `now_utc` reaches the existing operation's own
/// persisted `effective_operation_close_utc`, canonical close/stop
/// reconciliation ([`handle_session_close`]) takes priority over further
/// conflict bookkeeping — exactly like the ordinary dispatch path — so the
/// matching runtime is still canonically stopped even though every tick
/// since the conflict first appeared observed the same identity mismatch.
async fn handle_identity_conflict(
    state: &Arc<AppState>,
    pool: &PgPool,
    existing_operation_id: Uuid,
    assignment_identity: &str,
    runtime_binding_identity: &str,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    const UNCHANGED: AutonomousDailyCoordinatorTickOutcome =
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "operation_identity_conflict",
            newly_applied: false,
        };

    let Some(existing) =
        mqk_db::fetch_autonomous_daily_operation_by_id(pool, existing_operation_id).await?
    else {
        // Nothing durable to reconcile against; report the conflict without
        // fabricating a transition against a row that does not exist.
        return Ok(UNCHANGED);
    };

    if mqk_db::is_terminal_operation_state(&existing.state) {
        // Terminal: no lifecycle mutation, nothing started.
        return Ok(UNCHANGED);
    }

    if now_utc >= existing.effective_operation_close_utc
        && !matches!(
            existing.state.as_str(),
            STATE_STOPPING
                | STATE_STOP_RETRYING
                | STATE_MANUAL_INTERVENTION_REQUIRED
                | mqk_db::STATE_COMPLETED
                | mqk_db::STATE_COMPLETED_NO_TRADE
                | mqk_db::STATE_COMPLETED_WITH_ACTIVITY
                | mqk_db::STATE_CALENDAR_UNAVAILABLE
        )
    {
        return handle_session_close(state, pool, existing, now_utc).await;
    }

    let reason = AutonomousCoordinatorReason::OperationIdentityConflict;
    let signature = blocker_signature(
        &reason,
        &AutonomousBlockerIdentity {
            operation_id: Some(existing_operation_id),
            assignment_identity: Some(assignment_identity),
            runtime_binding_identity: Some(runtime_binding_identity),
            ..Default::default()
        },
    );

    let detail = "a same-day operation identity conflict was detected: the daily slot already \
                  holds a row whose immutable identity fields differ from the freshly computed \
                  expected values";

    let target_state: &'static str = match existing.state.as_str() {
        // REPAIR 3: running's only legal degraded edge.
        STATE_RUNNING => mqk_db::STATE_CONTROLLER_DEGRADED,
        // Already blocked/degraded: refresh in place (REPAIR 4 self-loop
        // via `apply_manual_if_changed`), never escalate straight to
        // manual_intervention_required merely because a conflict recomputed
        // with a different signature.
        STATE_MANUAL_INTERVENTION_REQUIRED => STATE_MANUAL_INTERVENTION_REQUIRED,
        mqk_db::STATE_CONTROLLER_DEGRADED => mqk_db::STATE_CONTROLLER_DEGRADED,
        mqk_db::STATE_EVIDENCE_DEGRADED => mqk_db::STATE_EVIDENCE_DEGRADED,
        _ => STATE_MANUAL_INTERVENTION_REQUIRED,
    };

    if !mqk_db::is_legal_operation_transition(Some(&existing.state), target_state)
        && existing.state.as_str() != target_state
    {
        // No legal edge from the existing operation's current state to the
        // chosen target this tick. Report the conflict without forcing an
        // illegal transition; a later tick (once the existing operation
        // reaches a state with a legal edge) will durably record it.
        return Ok(UNCHANGED);
    }

    let newly_applied = apply_manual_if_changed(
        pool,
        &existing,
        signature.reason_code,
        signature.stable_context.clone(),
        now_utc,
        detail,
        target_state,
    )
    .await?;
    Ok(
        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
            reason_code: "operation_identity_conflict",
            newly_applied,
        },
    )
}

/// D2.6: the narrowest truthful legal initial state for `now_utc` against
/// `plan`'s boundaries. `stopping` is not a legal initial state (§ graph),
/// so an operation first observed at or after
/// `effective_operation_close_utc` still seeds as `awaiting_open` — the
/// caller (`tick_autonomous_daily_coordinator`) immediately records the one
/// honest follow-up transition to `stopping` (REPAIR 6) for that specific
/// case.
fn initial_state_for_plan(
    plan: &AutonomousDailySessionPlan,
    now_utc: DateTime<Utc>,
) -> &'static str {
    if now_utc < plan.preopen_start_utc {
        STATE_AWAITING_PREOPEN
    } else if now_utc < plan.effective_operation_open_utc {
        STATE_PREPARING_DATA
    } else {
        STATE_AWAITING_OPEN
    }
}

// ---------------------------------------------------------------------------
// D2.7 — CAS transition helper
// ---------------------------------------------------------------------------

/// Apply one CAS transition against `operation`'s current
/// `(state, state_version)`. Bails (fails the tick) on `StaleState` /
/// `NotFound` / `IllegalTransition` — these indicate either a concurrent
/// writer (should not happen: the coordinator is the sole writer of this
/// operation's lifecycle state) or a coordinator logic defect, never a
/// condition to paper over silently. `Applied` and `AlreadyApplied` both
/// return the resulting record — an idempotent replay is a legitimate
/// success, not an error. The returned `bool` is `true` only for `Applied`
/// (a genuinely new write this call), `false` for `AlreadyApplied` (an
/// idempotent replay that wrote nothing new) — REPAIR 11's sole source of
/// "was this newly applied" truth.
#[allow(clippy::too_many_arguments)]
pub async fn apply_transition(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    new_state: &str,
    reason_code: Option<&str>,
    blocker_signature: Option<String>,
    occurred_at_utc: DateTime<Utc>,
    run_id: Option<Uuid>,
    detail: &str,
) -> anyhow::Result<(AutonomousDailyOperationRecord, bool)> {
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        new_state: new_state.to_string(),
        reason_code: reason_code.map(bounded_reason),
        blocker_signature: blocker_signature.map(bounded_signature),
        occurred_at_utc,
        run_id,
        bounded_detail: bounded_detail(detail),
    };
    match mqk_db::transition_autonomous_daily_operation(pool, &args).await? {
        AutonomousDailyTransitionOutcome::Applied(record) => Ok((record, true)),
        AutonomousDailyTransitionOutcome::AlreadyApplied(record) => Ok((record, false)),
        AutonomousDailyTransitionOutcome::StaleState {
            actual_state,
            actual_state_version,
        } => {
            anyhow::bail!(
                "autonomous_daily_coordinator: stale transition for {}: expected {}@{}, actual {}@{}",
                operation.operation_id,
                operation.state,
                operation.state_version,
                actual_state,
                actual_state_version
            )
        }
        AutonomousDailyTransitionOutcome::NotFound => {
            anyhow::bail!(
                "autonomous_daily_coordinator: operation {} not found during transition",
                operation.operation_id
            )
        }
        AutonomousDailyTransitionOutcome::IllegalTransition => {
            anyhow::bail!(
                "autonomous_daily_coordinator: illegal transition {} -> {} for {}",
                operation.state,
                new_state,
                operation.operation_id
            )
        }
    }
}

/// REPAIR 11 / REPAIR 4 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-FAILSAFE-
/// RECOVERY-CLOSURE-01): apply a manual-intervention/controller-degraded/
/// preflight-blocked transition only if `(target_state, reason_code,
/// blocker_signature)` differs from what the operation already durably
/// carries — the sole guard against a repeated critical notification for an
/// unchanged blocker. Returns whether the transition was newly applied this
/// tick.
///
/// REPAIR 4: when `target_state == operation.state` (a same-state refresh —
/// e.g. `manual_intervention_required -> manual_intervention_required` with
/// a changed reason/signature), this is not a state transition and the pure
/// [`mqk_db::is_legal_operation_transition`] graph has no self-loop edges,
/// so [`apply_transition`] would report `IllegalTransition` and this
/// function would bail. Such a same-state refresh instead goes through the
/// dedicated atomic [`mqk_db::refresh_autonomous_daily_operation_blocker`]
/// CAS, which updates `state_reason_code`/`state_blocker_signature` in
/// place without touching `state`.
#[allow(clippy::too_many_arguments)]
async fn apply_manual_if_changed(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    reason_code: &'static str,
    blocker_signature: Option<String>,
    now_utc: DateTime<Utc>,
    detail: &str,
    target_state: &'static str,
) -> anyhow::Result<bool> {
    if operation.state.as_str() == target_state
        && operation.state_reason_code.as_deref() == Some(reason_code)
        && operation.state_blocker_signature.as_deref() == blocker_signature.as_deref()
    {
        return Ok(false);
    }

    if operation.state.as_str() == target_state {
        return refresh_blocker_same_state(
            pool,
            operation,
            reason_code,
            blocker_signature,
            now_utc,
            detail,
        )
        .await;
    }

    let (_, newly_applied) = apply_transition(
        pool,
        operation,
        target_state,
        Some(reason_code),
        blocker_signature,
        now_utc,
        None,
        detail,
    )
    .await?;
    Ok(newly_applied)
}

/// REPAIR 4: same-state blocker refresh via the dedicated
/// [`mqk_db::refresh_autonomous_daily_operation_blocker`] CAS. Returns
/// whether the refresh was newly applied this tick (`false` for an
/// idempotent replay or a value that already matched durably).
async fn refresh_blocker_same_state(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    reason_code: &'static str,
    blocker_signature: Option<String>,
    now_utc: DateTime<Utc>,
    detail: &str,
) -> anyhow::Result<bool> {
    let args = mqk_db::RefreshAutonomousDailyOperationBlockerArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        reason_code: bounded_reason(reason_code),
        blocker_signature: blocker_signature.map(bounded_signature),
        occurred_at_utc: now_utc,
        bounded_detail: bounded_detail(detail),
    };
    match mqk_db::refresh_autonomous_daily_operation_blocker(pool, &args).await? {
        mqk_db::RefreshAutonomousDailyOperationBlockerOutcome::Applied(_) => Ok(true),
        mqk_db::RefreshAutonomousDailyOperationBlockerOutcome::AlreadyApplied(_) => Ok(false),
        mqk_db::RefreshAutonomousDailyOperationBlockerOutcome::StaleState {
            actual_state,
            actual_state_version,
        } => {
            anyhow::bail!(
                "autonomous_daily_coordinator: stale blocker refresh for {}: expected {}@{}, \
                 actual {}@{}",
                operation.operation_id,
                operation.state,
                operation.state_version,
                actual_state,
                actual_state_version
            )
        }
        mqk_db::RefreshAutonomousDailyOperationBlockerOutcome::NotFound => {
            anyhow::bail!(
                "autonomous_daily_coordinator: operation {} not found during blocker refresh",
                operation.operation_id
            )
        }
        mqk_db::RefreshAutonomousDailyOperationBlockerOutcome::IllegalTarget => {
            anyhow::bail!(
                "autonomous_daily_coordinator: '{}' is not a blocker-refresh-eligible state for {}",
                operation.state,
                operation.operation_id
            )
        }
    }
}

fn blocker_signature_for(
    operation: &AutonomousDailyOperationRecord,
    reason: &AutonomousCoordinatorReason,
) -> super::autonomous_retry_policy::AutonomousBlockerSignature {
    blocker_signature(
        reason,
        &AutonomousBlockerIdentity {
            operation_id: Some(operation.operation_id),
            assignment_identity: Some(&operation.assignment_identity),
            runtime_binding_identity: Some(&operation.runtime_binding_identity),
            ..Default::default()
        },
    )
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3-COMPLETED-BAR-TASK-CUTOVER-AND-SUPERVISION
// D3.14 — durable critical completed-bar-driver outcome application
//
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D3-SUPERVISOR-AND-CRITICAL-OUTCOME-
// CLOSURE-01 (REPAIR 4/7/8): the original D3.14 applier only recognized the
// evidence/runtime-ownership outcomes; every manual/configuration blocker
// (invalid authorization, binding/registry rejection, terminal provider
// rejection, provider-setup failure, unexpected/future bar, missing
// exchange-session truth, non-remediable readiness blocker) fell through
// the benign `_ => None` arm and never reached durable operator truth. This
// closure adds one closed classification over every typed outcome, a
// coordinator-owned durable permanent-task-failure helper, and bounds every
// persisted detail to static reason codes plus bar/numeric identity — no
// free-form provider/SQL/panic text is ever persisted as operator authority.
// ---------------------------------------------------------------------------

/// Which degrade edge a critical completed-bar blocker takes from a
/// `running` operation. Evidence corruption/unresolved-claim truth degrades
/// `running -> evidence_degraded`; every control-side blocker
/// (runtime-ownership, manual/configuration, permanent task failure)
/// degrades `running -> controller_degraded` (mirrors [`handle_running`]'s
/// existing mismatched-run degrade target — `running ->
/// manual_intervention_required` is not a legal edge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletedBarCriticalClass {
    Evidence,
    Control,
}

/// REPAIR 7 (closed classification): what one typed completed-bar driver
/// outcome means for durable operation truth. Constructed only by
/// [`classify_completed_bar_driver_outcome`]; never parsed from rendered
/// text.
enum CompletedBarOutcomeDisposition {
    /// Benign/waiting or transient — zero DB writes, zero transitions.
    NoDurableEffect,
    /// A critical blocker that must reach durable operation truth once.
    Critical {
        class: CompletedBarCriticalClass,
        fault_class: &'static str,
        detail: String,
    },
}

/// Static label for an [`AutonomousBindingRejection`] — never carries
/// operator-config text.
fn binding_rejection_label(
    rejection: &super::autonomous_completed_bar_driver::AutonomousBindingRejection,
) -> &'static str {
    use super::autonomous_completed_bar_driver::AutonomousBindingRejection as R;
    match rejection {
        R::AssignmentIdentityMismatch => "assignment_identity_mismatch",
        R::RuntimeBindingIdentityMismatch => "runtime_binding_identity_mismatch",
        R::BlankTargetSymbol => "blank_target_symbol",
        R::BlankStrategyId => "blank_strategy_id",
        R::MissingTimeframeBinding => "missing_timeframe_binding",
        R::UnsupportedTimeframe => "unsupported_timeframe",
        R::MultiSymbolAssignmentNotExactlyBound => "multi_symbol_assignment_not_exactly_bound",
    }
}

/// Static label for an [`AutonomousDriverSetupRejection`] — the free-form
/// path/IO/registry payload each variant carries is deliberately dropped
/// (REPAIR 8: no filesystem/registry text as operator authority).
pub(super) fn driver_setup_rejection_label(
    rejection: &super::autonomous_completed_bar_driver::AutonomousDriverSetupRejection,
) -> &'static str {
    use super::autonomous_completed_bar_driver::AutonomousDriverSetupRejection as R;
    match rejection {
        R::InstrumentRegistryUnavailable(_) => "instrument_registry_unavailable",
        R::InstrumentRegistryInvalid(_) => "instrument_registry_invalid",
        R::ProviderRegistryUnavailable(_) => "provider_registry_unavailable",
        R::ProviderUnknownOrDisabled(_) => "provider_unknown_or_disabled",
        R::ProviderConstructionFailed(_) => "provider_construction_failed",
    }
}

/// REPAIR 7: classify a readiness-blocker vector by exact stable
/// reason-code equality only (never substring/regex/display text).
///
/// `expected_latest_bar_missing` is the one wait-for-condition code the
/// completed-bar path can legitimately remain in (the bar simply has not
/// been published/ingested yet) — same convention as
/// [`classify_readiness_report`]'s `LatestCompletedBarPending`. An empty
/// vector means the evaluation itself could not name a blocker
/// (`db_unavailable`/`query_failed` readiness states, or a ready-but-newer-
/// bar mismatch after a poll) — transient, never a durable degrade. Any
/// other code is non-remediable by waiting and fails closed durably; the
/// first such code becomes the blocker's own fault class, mirroring
/// [`classify_readiness_report`]'s use of the readiness module's closed
/// reason-code set.
fn first_non_remediable_readiness_blocker(blockers: &[&'static str]) -> Option<&'static str> {
    blockers
        .iter()
        .copied()
        .find(|b| *b != crate::daily_data_readiness::REASON_EXPECTED_LATEST_BAR_MISSING)
}

/// REPAIR 7: one closed, source-grounded classification of every
/// [`AutonomousCompletedBarDriverOutcome`]. Exhaustive by construction — a
/// new outcome variant fails compilation here rather than silently landing
/// in a benign default.
fn classify_completed_bar_driver_outcome(
    outcome: &super::autonomous_completed_bar_driver::AutonomousCompletedBarDriverOutcome,
) -> CompletedBarOutcomeDisposition {
    use super::autonomous_completed_bar_driver::AutonomousCompletedBarDriverOutcome as O;
    use CompletedBarCriticalClass as Class;
    use CompletedBarOutcomeDisposition as D;

    match outcome {
        // Benign/waiting: no lifecycle transition.
        O::NotApplicable { .. }
        | O::OutsideOperationWindow
        | O::AuthorizationDisabled
        | O::PollNotDue
        | O::PollSucceededNoNewBar
        | O::NoNewCompletedBar
        | O::ProviderLaggingExpectedBar { .. }
        | O::BarObserved { .. }
        | O::AlreadyDispatched { .. }
        | O::DispatchCompleted { .. } => D::NoDurableEffect,

        // Transient: retried on a later tick, no manual lifecycle transition.
        O::PollFailedTransient { .. } => D::NoDurableEffect,

        // Evidence blockers: a running operation reaches evidence_degraded.
        O::DispatchClaimUnresolved { status } => D::Critical {
            class: Class::Evidence,
            fault_class: "completed_bar_dispatch_claim_unresolved",
            detail: format!("dispatch claim status={status}"),
        },
        // D4 REPAIR 2/4: the strategy callback returned a result, but the
        // exact durable evaluation-lineage evidence this claim's
        // deterministic identity requires could not be confirmed, or the
        // completion write itself could not be durably confirmed. Both fail
        // closed identically to an unresolved dispatch claim — never
        // reported as success, never automatically redispatched.
        O::DispatchEvaluationEvidenceMissing {
            bar_end_ts,
            reason_code,
        } => D::Critical {
            class: Class::Evidence,
            fault_class: "completed_bar_dispatch_evaluation_evidence_missing",
            detail: format!("bar_end_ts={bar_end_ts} reason_code={reason_code}"),
        },
        O::DispatchCompletionUnconfirmed {
            bar_end_ts,
            reason_code,
        } => D::Critical {
            class: Class::Evidence,
            fault_class: "completed_bar_dispatch_completion_unconfirmed",
            detail: format!("bar_end_ts={bar_end_ts} reason_code={reason_code}"),
        },
        O::ObservedBarEvidenceInconsistent {
            expected_end_ts,
            reason_code,
        } => D::Critical {
            class: Class::Evidence,
            fault_class: "completed_bar_observed_evidence_inconsistent",
            detail: format!("expected_end_ts={expected_end_ts} reason_code={reason_code}"),
        },
        O::ObservedBarSequenceInconsistent {
            expected_end_ts,
            current_last_completed_bar_ts,
        } => D::Critical {
            class: Class::Evidence,
            fault_class: "completed_bar_observed_sequence_inconsistent",
            detail: format!(
                "expected_end_ts={expected_end_ts} \
                 current_last_completed_bar_ts={current_last_completed_bar_ts:?}"
            ),
        },
        // REPAIR 8: the outcome's free-form persistence-failure detail is
        // deliberately not persisted as operator authority.
        O::EvidencePersistenceFailed { .. } => D::Critical {
            class: Class::Evidence,
            fault_class: "completed_bar_evidence_persistence_failed",
            detail: "reason_code=completed_bar_evidence_persistence_failed".to_string(),
        },

        // Runtime-ownership blocker: a running operation reaches
        // controller_degraded.
        O::RuntimeDispatchNotReady { reason_code } => D::Critical {
            class: Class::Control,
            fault_class: "completed_bar_runtime_dispatch_not_ready",
            detail: format!("reason_code={reason_code}"),
        },

        // Manual/configuration blockers: durably applied fail-closed truth
        // once. Details are static labels plus bounded bar/numeric identity
        // only (REPAIR 8) — the free-form payloads some variants carry are
        // dropped, never persisted.
        O::ExchangeSessionTruthMissing => D::Critical {
            class: Class::Control,
            fault_class: "completed_bar_exchange_session_truth_missing",
            detail: "reason_code=completed_bar_exchange_session_truth_missing".to_string(),
        },
        O::AuthorizationInvalid { reason_code, .. } => D::Critical {
            class: Class::Control,
            fault_class: "completed_bar_authorization_invalid",
            detail: format!("flag={reason_code}"),
        },
        O::BindingBlocked { rejection } => D::Critical {
            class: Class::Control,
            fault_class: "completed_bar_binding_blocked",
            detail: format!("rejection={}", binding_rejection_label(rejection)),
        },
        O::RegistryBlocked { rejection } => D::Critical {
            class: Class::Control,
            fault_class: "completed_bar_registry_blocked",
            detail: format!("rejection={}", rejection.status_label()),
        },
        O::Unsupported { .. } => D::Critical {
            class: Class::Control,
            fault_class: "completed_bar_provider_unsupported",
            detail: "reason_code=completed_bar_provider_unsupported".to_string(),
        },
        O::PollFailedTerminal { .. } => D::Critical {
            class: Class::Control,
            fault_class: "completed_bar_poll_failed_terminal",
            detail: "reason_code=completed_bar_poll_failed_terminal".to_string(),
        },
        O::ProviderSetupBlocked { rejection } => D::Critical {
            class: Class::Control,
            fault_class: "completed_bar_provider_setup_blocked",
            detail: format!("rejection={}", driver_setup_rejection_label(rejection)),
        },
        O::UnexpectedOrFutureBar {
            returned_bar_ts,
            expected_end_ts,
        } => D::Critical {
            class: Class::Control,
            fault_class: "completed_bar_unexpected_or_future_bar",
            detail: format!("returned_bar_ts={returned_bar_ts} expected_end_ts={expected_end_ts}"),
        },

        // Readiness blockers: exact reason-code equality only. A vector
        // whose every code is wait-remediable (or an empty vector — a
        // transient db_unavailable/query_failed evaluation, or a ready-but-
        // newer-bar mismatch) stays benign; the first non-remediable code
        // fails closed durably under its own readiness reason code.
        O::ReadinessBlocked { blockers } | O::ReadinessBlockedAfterPoll { blockers } => {
            match first_non_remediable_readiness_blocker(blockers) {
                Some(blocker) => D::Critical {
                    class: Class::Control,
                    fault_class: blocker,
                    detail: format!("reason_code={blocker}"),
                },
                None => D::NoDurableEffect,
            }
        }
    }
}

/// Shared durable applier for every critical completed-bar blocker
/// (driver-outcome, production-adapter, and permanent-task-failure paths).
/// One legal fail-closed edge per call, full D1 typed blocker signature,
/// same-state-refresh dedup, zero duplicate events for an unchanged
/// blocker:
///
/// - `running` -> `evidence_degraded` (Evidence class) or
///   `controller_degraded` (Control class);
/// - a pre-runtime pollable state (`awaiting_preopen` / `preparing_data` /
///   `awaiting_open` / `preflight_blocked` / `start_retrying`) ->
///   `manual_intervention_required`;
/// - an already manual/controller/evidence-degraded snapshot -> same-state
///   blocker refresh in place (never a re-target);
/// - `stopping` / `stop_retrying` / `completed*` / any other state -> no
///   lifecycle mutation at all (stopping and terminal truth remain
///   authoritative), returned as `Ok(None)`.
///
/// Never creates a completed-state transition, never starts/stops a
/// runtime, and never makes a provider/broker/order call.
async fn apply_completed_bar_critical_blocker(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    class: CompletedBarCriticalClass,
    fault_class: &'static str,
    detail: &str,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<Option<bool>> {
    let target_state = if operation.state == STATE_RUNNING {
        match class {
            CompletedBarCriticalClass::Evidence => mqk_db::STATE_EVIDENCE_DEGRADED,
            CompletedBarCriticalClass::Control => mqk_db::STATE_CONTROLLER_DEGRADED,
        }
    } else {
        match operation.state.as_str() {
            STATE_AWAITING_PREOPEN
            | STATE_PREPARING_DATA
            | STATE_AWAITING_OPEN
            | STATE_PREFLIGHT_BLOCKED
            | STATE_START_RETRYING => STATE_MANUAL_INTERVENTION_REQUIRED,
            mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED => {
                mqk_db::STATE_MANUAL_INTERVENTION_REQUIRED
            }
            mqk_db::STATE_CONTROLLER_DEGRADED => mqk_db::STATE_CONTROLLER_DEGRADED,
            mqk_db::STATE_EVIDENCE_DEGRADED => mqk_db::STATE_EVIDENCE_DEGRADED,
            // stopping / stop_retrying / completed* / calendar_unavailable /
            // recovery_retrying / unknown: no lifecycle mutation — those
            // states' own truth stays authoritative over this blocker.
            _ => return Ok(None),
        }
    };

    let reason = AutonomousCoordinatorReason::UnclassifiedFailClosed { fault_class };
    let signature = blocker_signature_for(operation, &reason);
    let newly_applied = apply_manual_if_changed(
        pool,
        operation,
        signature.reason_code,
        signature.stable_context.clone(),
        now_utc,
        detail,
        target_state,
    )
    .await?;
    Ok(Some(newly_applied))
}

/// Durably degrade `operation` for a critical completed-bar-driver outcome.
/// A benign/waiting or transient outcome is a no-op — this function never
/// touches the DB for those. Classification is the closed typed
/// [`classify_completed_bar_driver_outcome`] map; application is the shared
/// [`apply_completed_bar_critical_blocker`] edge (one transition/event for
/// a changed blocker, zero duplicate events for an unchanged one, no
/// direct SQL in the caller). Returns `Ok(None)` when nothing was applied,
/// or `Ok(Some(newly_applied))` for a critical outcome that reached (or
/// idempotently matched) durable truth.
pub async fn apply_completed_bar_driver_outcome(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    outcome: &super::autonomous_completed_bar_driver::AutonomousCompletedBarDriverOutcome,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<Option<bool>> {
    match classify_completed_bar_driver_outcome(outcome) {
        CompletedBarOutcomeDisposition::NoDurableEffect => Ok(None),
        CompletedBarOutcomeDisposition::Critical {
            class,
            fault_class,
            detail,
        } => {
            apply_completed_bar_critical_blocker(
                pool,
                operation,
                class,
                fault_class,
                &detail,
                now_utc,
            )
            .await
        }
    }
}

/// REPAIR 7 (adapter-level critical truth): durably apply a production-
/// adapter blocker that occurs before the Phase C driver can even be
/// invoked (`IdentityUnresolved` — canonical assignment/runtime-binding
/// resolution failed; `RegistryUnavailable` — the canonical instrument
/// registry could not be loaded/validated). Both are control-side
/// manual/configuration blockers; `detail` must already be bounded
/// static-code text (the task module passes `reason_code=`/`rejection=`
/// labels only, never registry/filesystem payloads).
pub async fn apply_completed_bar_adapter_blocker(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    fault_class: &'static str,
    detail: &str,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<Option<bool>> {
    apply_completed_bar_critical_blocker(
        pool,
        operation,
        CompletedBarCriticalClass::Control,
        fault_class,
        detail,
        now_utc,
    )
    .await
}

/// Stable reason code for a permanently-failed completed-bar task (REPAIR 4).
pub const REASON_COMPLETED_BAR_TASK_PERMANENTLY_FAILED: &str =
    "completed_bar_task_permanently_failed";

/// REPAIR 4: durably record that the supervised completed-bar task has
/// permanently failed (restart budget exhausted, or supervisor panic) on
/// the one currently relevant durable operation.
///
/// - Fetches the one relevant operation via the accepted bounded lookup;
///   creates no operation when none exists (`Ok(None)`).
/// - Applies one legal fail-closed transition via the shared applier:
///   `running -> controller_degraded`; a pre-runtime pollable state ->
///   `manual_intervention_required`; an already-degraded state -> same-
///   state blocker refresh; `stopping`/`stop_retrying`/`completed*` -> no
///   lifecycle mutation (stopping remains authoritative).
/// - Uses the full D1 typed blocker signature and the same dedup as every
///   other blocker: first application produces one event, an identical
///   re-application produces zero new events.
/// - Makes no provider call, no broker call, starts/stops no runtime,
///   submits no order, and performs no outcome finalization.
pub async fn apply_completed_bar_task_permanent_failure(
    state: &Arc<AppState>,
    pool: &PgPool,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<Option<bool>> {
    let deployment_mode = state.deployment_mode().as_db_mode();
    let adapter_id = state.adapter_id().to_string();
    let Some(operation) = mqk_db::fetch_relevant_open_autonomous_daily_operation(
        pool,
        deployment_mode,
        &adapter_id,
        now_utc,
    )
    .await?
    else {
        return Ok(None);
    };

    apply_completed_bar_critical_blocker(
        pool,
        &operation,
        CompletedBarCriticalClass::Control,
        REASON_COMPLETED_BAR_TASK_PERMANENTLY_FAILED,
        "reason_code=completed_bar_task_permanently_failed",
        now_utc,
    )
    .await
}

// ---------------------------------------------------------------------------
// D2 — Per-state dispatch
// ---------------------------------------------------------------------------

pub async fn dispatch_by_state(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: AutonomousDailyOperationRecord,
    plan: &AutonomousDailySessionPlan,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    // D2.17: close handling takes priority over every other state-specific
    // action once the effective operation window has closed, for every
    // state that has not already reached stopping/manual/terminal truth.
    //
    // AUTONOMOUS-DAILY-STOPPING-EVIDENCE-DEGRADED-OSCILLATION-01: an
    // `evidence_degraded` operation whose runtime already durably stopped
    // (`stopped_at_utc` set -- the post-stop finalization-evidence-gap shape
    // E3.2 routes into `attempt_evidence_degraded_recovery`/
    // `handle_outcome_finalization` via the dedicated arm below) has
    // `stopped_at_utc` proving only that the autonomous runtime stop
    // obligation was durably recorded/satisfied for this operation --
    // nothing more. It is not proof of zero unacked outbox, a clean
    // reconcile, or any other recovery-safety predicate; those remain
    // independently (and repeatedly) checked by their own authorities every
    // tick -- `attempt_evidence_degraded_recovery`'s own outbox/inbox/
    // reconcile checks, and (REPAIR-01) its own
    // `effective_operation_close_utc` fresh-start boundary. Routing this
    // shape back through `handle_session_close` every tick instead
    // re-requests `stopping` from `reconcile_durable_run_without_local_
    // owner`, which `handle_stopping` then immediately reclassifies back to
    // `evidence_degraded` -- an unbounded oscillation, never a fresh proof
    // (`apply_evidence_degraded_blocker`'s CAS is a genuine no-op once the
    // reason/signature is unchanged, so the dedicated arm converges; this
    // guard is what prevented that arm from ever being reached post-close).
    // A mid-run `evidence_degraded` row (`stopped_at_utc` still `None`) is
    // deliberately NOT exempted here: its runtime may still be genuinely
    // active and must still be stopped by `handle_session_close` at close,
    // exactly as before.
    let evidence_degraded_already_stopped = operation.state.as_str()
        == mqk_db::STATE_EVIDENCE_DEGRADED
        && operation.stopped_at_utc.is_some();
    if now_utc >= plan.effective_operation_close_utc
        && !evidence_degraded_already_stopped
        && !matches!(
            operation.state.as_str(),
            STATE_STOPPING
                | STATE_STOP_RETRYING
                | STATE_MANUAL_INTERVENTION_REQUIRED
                | mqk_db::STATE_COMPLETED
                | mqk_db::STATE_COMPLETED_NO_TRADE
                | mqk_db::STATE_COMPLETED_WITH_ACTIVITY
                | mqk_db::STATE_CALENDAR_UNAVAILABLE
        )
    {
        return handle_session_close(state, pool, operation, now_utc).await;
    }

    match operation.state.as_str() {
        STATE_AWAITING_PREOPEN => {
            if now_utc >= plan.preopen_start_utc {
                apply_transition(
                    pool,
                    &operation,
                    STATE_PREPARING_DATA,
                    None,
                    None,
                    now_utc,
                    None,
                    "preopen window reached",
                )
                .await?;
                Ok(AutonomousDailyCoordinatorTickOutcome::PreparingData)
            } else {
                Ok(AutonomousDailyCoordinatorTickOutcome::WaitingForPreopen)
            }
        }
        STATE_PREPARING_DATA => handle_preparing_data(state, pool, &operation, plan, now_utc).await,
        STATE_PREFLIGHT_BLOCKED => {
            handle_preflight_blocked(state, pool, &operation, plan, now_utc).await
        }
        STATE_AWAITING_OPEN => {
            attempt_canonical_start(state, pool, operation, now_utc, STATE_AWAITING_OPEN).await
        }
        STATE_START_RETRYING => {
            attempt_canonical_start(state, pool, operation, now_utc, STATE_START_RETRYING).await
        }
        STATE_RUNNING => handle_running(state, pool, operation, now_utc).await,
        STATE_RECOVERY_RETRYING => {
            attempt_canonical_start(state, pool, operation, now_utc, STATE_RECOVERY_RETRYING).await
        }
        STATE_STOPPING | STATE_STOP_RETRYING => {
            handle_stopping(state, pool, operation, plan.postclose_finalize_utc, now_utc).await
        }
        STATE_MANUAL_INTERVENTION_REQUIRED => {
            let reason_code = operation
                .state_reason_code
                .as_deref()
                .unwrap_or("manual_intervention_required");
            Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: bounded_static_reason(reason_code),
                    newly_applied: false,
                },
            )
        }
        mqk_db::STATE_CALENDAR_UNAVAILABLE => {
            Ok(AutonomousDailyCoordinatorTickOutcome::CalendarBlocked {
                reason_code: "calendar_unavailable",
            })
        }
        // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-
        // INTEGRATION-AND-NOTIFICATION (E3.2/E3.9): a `completed*` row
        // observed on an ordinary tick is already-terminal durable truth --
        // read-only projection, no classifier re-run (no DB call beyond the
        // row already fetched this tick), no notification.
        mqk_db::STATE_COMPLETED
        | mqk_db::STATE_COMPLETED_NO_TRADE
        | mqk_db::STATE_COMPLETED_WITH_ACTIVITY => Ok(
            AutonomousDailyCoordinatorTickOutcome::OutcomeAlreadyFinalized {
                state: operation.state.clone(),
                outcome_reason_code: operation.outcome.clone(),
            },
        ),
        // AUTONOMOUS-DAILY-CONTROLLER-DEGRADED-RECOVERY-01: `controller_degraded`
        // must not be a permanent re-projection stub. Every applicable tick
        // re-reads the authoritative durable run row via the same
        // `reconcile_durable_run_without_local_owner` helper `handle_session_
        // close`/`retry_stop` already use for this exact question ("is the
        // run genuinely terminal, or still active without a local owner?"),
        // and selects the existing legal next transition from current truth
        // -- never a hardcoded favorable outcome. A run still armed/running,
        // an unacked outbox row, or a dirty global reconcile status all fail
        // closed (to `manual_intervention_required`, a legal edge from
        // `controller_degraded`); only a terminal run with zero unresolved
        // economic evidence legally advances to `stopping`.
        mqk_db::STATE_CONTROLLER_DEGRADED => {
            let Some(expected_run_id) = operation.run_id else {
                let newly_applied = apply_manual_if_changed(
                    pool,
                    &operation,
                    "controller_degraded_missing_run_id",
                    None,
                    now_utc,
                    "operation is controller_degraded but carries no run_id to reconcile \
                     against; failing closed rather than guessing",
                    STATE_MANUAL_INTERVENTION_REQUIRED,
                )
                .await?;
                return Ok(
                    AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                        reason_code: "controller_degraded_missing_run_id",
                        newly_applied,
                    },
                );
            };
            reconcile_durable_run_without_local_owner(pool, &operation, expected_run_id, now_utc)
                .await
        }
        // E3.2: `evidence_degraded` with a durable `stopped_at_utc` is the
        // post-stop finalization-evidence-gap case (E1 contract §3.3) --
        // route to E2B's recovery/classification helper so a later tick can
        // repair and finalize. A mid-run `evidence_degraded` row
        // (`stopped_at_utc IS NULL`) is never finalization-eligible (E1 §3.3)
        // and keeps its existing mid-run manual-intervention projection
        // unchanged.
        mqk_db::STATE_EVIDENCE_DEGRADED => {
            if operation.stopped_at_utc.is_some() {
                match attempt_evidence_degraded_recovery(state, pool, &operation, now_utc).await? {
                    Some(outcome) => Ok(outcome),
                    None => handle_outcome_finalization(state, pool, operation, now_utc).await,
                }
            } else {
                Ok(
                    AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                        reason_code: "controller_degraded",
                        newly_applied: false,
                    },
                )
            }
        }
        other => {
            anyhow::bail!("autonomous_daily_coordinator: unknown operation state '{other}'")
        }
    }
}

/// `operation.state_reason_code` is a `String` read from the DB; the
/// coordinator's outcome type carries `&'static str` reason codes for
/// zero-allocation callers. Reason codes this coordinator itself writes are
/// always one of the closed-set literals below; anything else observed
/// (e.g. a legacy/foreign reason code) falls back to a bounded static
/// label rather than leaking a dynamically-allocated string through a
/// `&'static str` field.
fn bounded_static_reason(reason_code: &str) -> &'static str {
    match reason_code {
        "operation_identity_conflict" => "operation_identity_conflict",
        "operator_managed_run_active" => "operator_managed_run_active",
        "runtime_run_id_mismatch" => "runtime_run_id_mismatch",
        "durable_active_run_without_local_owner" => "durable_active_run_without_local_owner",
        "runtime_ended_unsafely" => "runtime_ended_unsafely",
        "mismatched_runtime_at_close" => "mismatched_runtime_at_close",
        "unresolved_stop_failure_at_postclose_finalize" => {
            "unresolved_stop_failure_at_postclose_finalize"
        }
        "session_closed_before_first_observation" => "session_closed_before_first_observation",
        "start_succeeded_without_run_id" => "start_succeeded_without_run_id",
        "durable_transition_unconfirmed_after_start" => {
            "durable_transition_unconfirmed_after_start"
        }
        "database_not_configured_or_invalid" => "database_not_configured_or_invalid",
        "integrity_halted" => "integrity_halted",
        "durable_arm_disarmed" => "durable_arm_disarmed",
        "run_row_fetch_failed" => "run_row_fetch_failed",
        "running_operation_missing_run_id" => "running_operation_missing_run_id",
        "arm_state_read_failed" => "arm_state_read_failed",
        "controller_degraded_missing_run_id" => "controller_degraded_missing_run_id",
        "unresolved_outbox_at_run_reconcile" => "unresolved_outbox_at_run_reconcile",
        "reconcile_dirty" => "reconcile_dirty",
        "evidence_degraded_recovery_unresolved_outbox" => {
            "evidence_degraded_recovery_unresolved_outbox"
        }
        "evidence_degraded_recovery_unresolved_inbox" => {
            "evidence_degraded_recovery_unresolved_inbox"
        }
        "evidence_degraded_recovery_reconcile_dirty" => "evidence_degraded_recovery_reconcile_dirty",
        _ => "manual_intervention_required",
    }
}

// ---------------------------------------------------------------------------
// D2.9 — Preopen strict readiness (no provider calls)
// ---------------------------------------------------------------------------

async fn handle_preparing_data(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    plan: &AutonomousDailySessionPlan,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    let config = match crate::state::build_multi_symbol_runtime_config_from_env() {
        Ok(config) => config,
        Err(_err) => {
            return classify_and_apply_preopen_blocker(
                pool,
                operation,
                AutonomousCoordinatorReason::AssignmentMissing,
                now_utc,
            )
            .await;
        }
    };
    let runtime_context = match resolve_autonomous_runtime_context(state).await {
        Ok(ctx) => ctx,
        Err(err) => {
            let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
            return classify_and_apply_preopen_blocker(pool, operation, reason, now_utc).await;
        }
    };

    let readiness_context = crate::daily_data_readiness::load_readiness_context_from_env();
    let report = crate::daily_data_readiness::evaluate_readiness_with_binding(
        state.db.as_ref(),
        &config,
        &runtime_context.effective_runtime_binding,
        &readiness_context,
        now_utc,
    )
    .await;

    if report.start_allowed {
        if now_utc >= plan.effective_operation_open_utc {
            apply_transition(
                pool,
                operation,
                STATE_AWAITING_OPEN,
                None,
                None,
                now_utc,
                None,
                "strict daily data readiness ready at effective operation open",
            )
            .await?;
            Ok(AutonomousDailyCoordinatorTickOutcome::AwaitingOpen)
        } else {
            Ok(AutonomousDailyCoordinatorTickOutcome::PreparingData)
        }
    } else {
        let reason = classify_readiness_report(&report);
        classify_and_apply_preopen_blocker(pool, operation, reason, now_utc).await
    }
}

async fn handle_preflight_blocked(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    plan: &AutonomousDailySessionPlan,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    let config = match crate::state::build_multi_symbol_runtime_config_from_env() {
        Ok(config) => config,
        Err(_err) => {
            // REPAIR 6 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-FAILSAFE-
            // RECOVERY-CLOSURE-01): assignment loss from `preflight_blocked`
            // previously returned an unpersisted `PreflightBlocked`
            // outcome, never applying the typed manual blocker. Routed
            // through the same classification path `handle_preparing_data`
            // already uses, so a durably lost assignment reaches
            // `manual_intervention_required` with a reason and full
            // signature — one transition, one notification.
            return classify_and_apply_preopen_blocker(
                pool,
                operation,
                AutonomousCoordinatorReason::AssignmentMissing,
                now_utc,
            )
            .await;
        }
    };
    let runtime_context = match resolve_autonomous_runtime_context(state).await {
        Ok(ctx) => ctx,
        Err(err) => {
            let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
            return classify_and_apply_preopen_blocker(pool, operation, reason, now_utc).await;
        }
    };
    let readiness_context = crate::daily_data_readiness::load_readiness_context_from_env();
    let report = crate::daily_data_readiness::evaluate_readiness_with_binding(
        state.db.as_ref(),
        &config,
        &runtime_context.effective_runtime_binding,
        &readiness_context,
        now_utc,
    )
    .await;

    if report.start_allowed && now_utc >= plan.effective_operation_open_utc {
        // preflight_blocked's only legal forward edge is start_retrying —
        // there is no legal edge back to awaiting_open.
        let (updated, _) = apply_transition(
            pool,
            operation,
            STATE_START_RETRYING,
            None,
            None,
            now_utc,
            None,
            "preopen readiness resolved; entering start sequence",
        )
        .await?;
        return attempt_canonical_start(state, pool, updated, now_utc, STATE_START_RETRYING).await;
    }
    if report.start_allowed {
        return Ok(AutonomousDailyCoordinatorTickOutcome::PreflightBlocked {
            reason_code: "awaiting_session_open",
        });
    }
    let reason = classify_readiness_report(&report);
    classify_and_apply_preopen_blocker(pool, operation, reason, now_utc).await
}

/// Classify a blocked [`crate::daily_data_readiness::DailyDataReadinessReport`]
/// into a typed coordinator reason. `REASON_EXPECTED_LATEST_BAR_MISSING` is
/// the one wait-for-condition data reason the evaluator emits today (a
/// remediable "not published yet" condition); a per-assignment
/// `readiness_state` of `db_unavailable`/`query_failed` is a transient
/// evaluation failure; every other blocker fails closed to
/// `ManualInterventionRequired`, per this codebase's conservative-fallback
/// convention.
fn classify_readiness_report(
    report: &crate::daily_data_readiness::DailyDataReadinessReport,
) -> AutonomousCoordinatorReason {
    if report.top_level_blocker.is_some() {
        return AutonomousCoordinatorReason::AssignmentMissing;
    }
    let mut db_unavailable_or_query_failed = false;
    let mut has_latest_bar_pending = false;
    let mut other_blocker: Option<&'static str> = None;
    for assignment in &report.assignments {
        if matches!(
            assignment.readiness_state,
            "db_unavailable" | "query_failed"
        ) {
            db_unavailable_or_query_failed = true;
        }
        for blocker in &assignment.blockers {
            if *blocker == crate::daily_data_readiness::REASON_EXPECTED_LATEST_BAR_MISSING {
                has_latest_bar_pending = true;
            } else if other_blocker.is_none() {
                other_blocker = Some(blocker);
            }
        }
    }
    if let Some(blocker) = other_blocker {
        AutonomousCoordinatorReason::UnclassifiedFailClosed {
            fault_class: blocker,
        }
    } else if db_unavailable_or_query_failed {
        AutonomousCoordinatorReason::TemporaryDatabaseOperationFailure
    } else if has_latest_bar_pending {
        AutonomousCoordinatorReason::LatestCompletedBarPending
    } else {
        AutonomousCoordinatorReason::UnclassifiedFailClosed {
            fault_class: "daily_data_readiness_blocked",
        }
    }
}

async fn classify_and_apply_preopen_blocker(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    reason: AutonomousCoordinatorReason,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    let signature = blocker_signature_for(operation, &reason);
    match classify_autonomous_reason(&reason) {
        AutonomousRetryClass::WaitForCondition => {
            // REPAIR 4: `apply_manual_if_changed` now durably refreshes the
            // reason/signature in place when the operation is already
            // `preflight_blocked` but the readiness blocker itself changed
            // (e.g. a different symbol's data went stale) — it no longer
            // silently drops the changed reason the way a bare
            // `state != PREFLIGHT_BLOCKED` guard once did.
            apply_manual_if_changed(
                pool,
                operation,
                signature.reason_code,
                signature.stable_context.clone(),
                now_utc,
                "preopen readiness not yet satisfied",
                STATE_PREFLIGHT_BLOCKED,
            )
            .await?;
            Ok(AutonomousDailyCoordinatorTickOutcome::PreflightBlocked {
                reason_code: signature.reason_code,
            })
        }
        AutonomousRetryClass::RetryableTransient => {
            // Transient DB/evaluation failure: reevaluate next tick. No
            // transition, no start-attempt-counter involvement, no
            // provider call.
            Ok(AutonomousDailyCoordinatorTickOutcome::PreflightBlocked {
                reason_code: signature.reason_code,
            })
        }
        AutonomousRetryClass::ManualInterventionRequired
        | AutonomousRetryClass::SessionTerminal => {
            let newly_applied = apply_manual_if_changed(
                pool,
                operation,
                signature.reason_code,
                signature.stable_context.clone(),
                now_utc,
                "preopen readiness blocked by a manual-intervention condition",
                STATE_MANUAL_INTERVENTION_REQUIRED,
            )
            .await?;
            Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: signature.reason_code,
                    newly_applied,
                },
            )
        }
    }
}

// ---------------------------------------------------------------------------
// D2.10-D2.13 — Canonical start sequence (shared by awaiting_open /
// start_retrying / recovery_retrying)
// ---------------------------------------------------------------------------

pub async fn attempt_canonical_start(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: AutonomousDailyOperationRecord,
    now_utc: DateTime<Utc>,
    from_state: &'static str,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    // D2.16: an operator-managed run active before autonomous ownership is
    // never stopped, attached, or bypassed with a synthetic start event.
    // Only applicable before this operation has ever bound a run_id of its
    // own (awaiting_open / start_retrying / preflight_blocked entry) —
    // recovery_retrying always has a prior run_id and reaches this
    // function only after `handle_running` has already ruled out the
    // operator-managed case.
    if operation.run_id.is_none() && state.locally_owned_run_id().await.is_some() {
        let newly_applied = apply_manual_if_changed(
            pool,
            &operation,
            "operator_managed_run_active",
            None,
            now_utc,
            "a locally-owned execution run already exists that this operation never started; \
             the coordinator will not stop it, attach it, or attempt a new start",
            STATE_MANUAL_INTERVENTION_REQUIRED,
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "operator_managed_run_active",
                newly_applied,
            },
        );
    }

    // D2.11: retry-timing gate — no call before the due time, and no
    // attempt-counter increment for a tick that never calls start.
    if let Some(next_retry_utc) = operation.next_retry_utc {
        if now_utc < next_retry_utc {
            return Ok(AutonomousDailyCoordinatorTickOutcome::RetryNotDue);
        }
    }

    // REPAIR 1: only `awaiting_open` transitions to `start_retrying` before
    // the canonical start call below. `start_retrying` stays put by
    // construction (it is already the current state); `recovery_retrying`
    // must also stay put — `recovery_retrying -> start_retrying` is not a
    // legal edge in the durable state graph and must never be taken merely
    // to share this one call site.
    let operation = if from_state == STATE_AWAITING_OPEN {
        apply_transition(
            pool,
            &operation,
            STATE_START_RETRYING,
            None,
            None,
            now_utc,
            None,
            "entering canonical start sequence",
        )
        .await?
        .0
    } else {
        operation
    };

    // D2.10: typed arm precedes the canonical start call; an arm failure is
    // never counted as a start attempt (it is "arm pending without a start
    // call").
    match state.try_autonomous_arm_typed().await {
        Ok(AutonomousArmOutcome::AlreadyArmed)
        | Ok(AutonomousArmOutcome::ArmedFromPersistedState) => {}
        Err(rejection) => {
            let reason = coordinator_reason_from_arm_rejection(&rejection);
            return classify_start_sequence_failure(pool, &operation, reason, now_utc, 1).await;
        }
    }

    // D2.11: the start attempt is counted only now — immediately before the
    // canonical start call itself.
    let attempt_number = match mqk_db::record_start_attempt(
        pool,
        operation.operation_id,
        now_utc,
        None,
    )
    .await?
    {
        mqk_db::RecordStartAttemptOutcome::Recorded {
            start_attempt_count,
        } => start_attempt_count.max(0) as u64,
        mqk_db::RecordStartAttemptOutcome::NotFound => {
            anyhow::bail!(
                "autonomous_daily_coordinator: operation {} vanished before start-attempt recording",
                operation.operation_id
            )
        }
    };

    match state.start_execution_runtime().await {
        Ok(snapshot) => {
            let Some(run_id) = snapshot.active_run_id else {
                let _ = state.stop_execution_runtime().await;
                let newly_applied = apply_manual_if_changed(
                    pool,
                    &operation,
                    "start_succeeded_without_run_id",
                    None,
                    now_utc,
                    "start_execution_runtime returned Ok without an active_run_id",
                    STATE_MANUAL_INTERVENTION_REQUIRED,
                )
                .await?;
                return Ok(
                    AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                        reason_code: "start_succeeded_without_run_id",
                        newly_applied,
                    },
                );
            };
            if state.locally_owned_run_id().await != Some(run_id) {
                // D2.12 crash-window policy: a successful start whose
                // durable operation binding cannot be confirmed must never
                // be presented as running.
                let _ = state.stop_execution_runtime().await;
                let newly_applied = apply_manual_if_changed(
                    pool,
                    &operation,
                    "durable_transition_unconfirmed_after_start",
                    None,
                    now_utc,
                    "start_execution_runtime succeeded but local ownership does not match the \
                     returned run_id; refusing to present the operation as running",
                    STATE_MANUAL_INTERVENTION_REQUIRED,
                )
                .await?;
                return Ok(
                    AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                        reason_code: "durable_transition_unconfirmed_after_start",
                        newly_applied,
                    },
                );
            }

            // REPAIR 2: state, run_id, started_at_utc, retry-clear, and the
            // transition event all commit atomically in one DB transaction.
            let to_running_args = mqk_db::TransitionAutonomousDailyOperationToRunningArgs {
                operation_id: operation.operation_id,
                expected_state: operation.state.clone(),
                expected_state_version: operation.state_version,
                run_id,
                started_at_utc: now_utc,
                occurred_at_utc: now_utc,
                bounded_detail: bounded_detail("canonical start succeeded"),
            };
            // REPAIR 5 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-FAILSAFE-
            // RECOVERY-CLOSURE-01): the store call itself (not just its
            // typed outcome) can return `Err` — a DB/transaction/connection
            // error. Never propagate that with an uncontained `?` while the
            // runtime the canonical start call just started may still be
            // active: `handle_running_transition_store_error` re-reads,
            // best-effort stops, and fails closed instead.
            match mqk_db::transition_autonomous_daily_operation_to_running(pool, &to_running_args)
                .await
            {
                Ok(AutonomousDailyTransitionOutcome::Applied(_))
                | Ok(AutonomousDailyTransitionOutcome::AlreadyApplied(_)) => {
                    // A fresh run under an already-`evidence_degraded`
                    // operation is a recovery, not this operation's first
                    // start of the day -- reported the same as
                    // `recovery_retrying`'s own successful restart.
                    if matches!(
                        from_state,
                        STATE_RECOVERY_RETRYING | mqk_db::STATE_EVIDENCE_DEGRADED
                    ) {
                        Ok(AutonomousDailyCoordinatorTickOutcome::Recovered { run_id })
                    } else {
                        Ok(AutonomousDailyCoordinatorTickOutcome::Started { run_id })
                    }
                }
                Ok(AutonomousDailyTransitionOutcome::StaleState { .. })
                | Ok(AutonomousDailyTransitionOutcome::NotFound)
                | Ok(AutonomousDailyTransitionOutcome::IllegalTransition) => {
                    // The runtime started, but the atomic durable running
                    // transition could not be confirmed. Best-effort stop;
                    // the operation is never presented as running and the
                    // run is never silently adopted.
                    let _ = state.stop_execution_runtime().await;
                    let newly_applied = apply_manual_if_changed(
                        pool,
                        &operation,
                        "durable_transition_unconfirmed_after_start",
                        None,
                        now_utc,
                        "start_execution_runtime succeeded but the atomic durable running \
                         transition could not be confirmed; best-effort stop issued, the \
                         operation is never presented as running",
                        STATE_MANUAL_INTERVENTION_REQUIRED,
                    )
                    .await?;
                    Ok(
                        AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                            reason_code: "durable_transition_unconfirmed_after_start",
                            newly_applied,
                        },
                    )
                }
                Err(store_err) => {
                    handle_running_transition_store_error(
                        state, pool, &operation, run_id, from_state, store_err, now_utc,
                    )
                    .await
                }
            }
        }
        Err(err) => {
            let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
            classify_start_sequence_failure(pool, &operation, reason, now_utc, attempt_number).await
        }
    }
}

/// REPAIR 5: does a matching `-> running` transition event exist at exactly
/// `expected_transition_seq`, binding `expected_run_id` and (where
/// available) sourced from `expected_from_state`? Used only to corroborate
/// an authoritative re-read after
/// `transition_autonomous_daily_operation_to_running`'s store call itself
/// returned `Err` — never treated as sufficient proof on its own without
/// the accompanying `state`/`run_id`/`started_at_utc` checks on the
/// re-read row.
///
/// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-NONTRADING-RECOVERY-AND-RUNNING-
/// CONFIRMATION-01 REPAIR 4: queries the exact expected `transition_seq`
/// directly via [`mqk_db::fetch_autonomous_daily_operation_event_at_sequence`]
/// — never scans an ascending, `[1, 100]`-capped event list. A valid running
/// transition committing after more than 100 earlier events on the same
/// operation must still be confirmed; a wrong sequence or a wrong event
/// `run_id`/`from_state` must be rejected.
async fn running_transition_event_matches(
    pool: &PgPool,
    operation_id: Uuid,
    expected_run_id: Uuid,
    expected_transition_seq: i64,
    expected_from_state: &str,
) -> anyhow::Result<bool> {
    let event = mqk_db::fetch_autonomous_daily_operation_event_at_sequence(
        pool,
        operation_id,
        expected_transition_seq,
    )
    .await?;
    Ok(event
        .map(|e| {
            e.to_state == STATE_RUNNING
                && e.run_id == Some(expected_run_id)
                && e.from_state == expected_from_state
        })
        .unwrap_or(false))
}

/// REPAIR 5 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-FAILSAFE-RECOVERY-
/// CLOSURE-01): handle `transition_autonomous_daily_operation_to_running`'s
/// store call itself returning `Err` (a DB/transaction/connection error,
/// distinct from its typed outcome). The canonical start call already
/// succeeded and `run_id` may genuinely be active — this function never
/// silently swallows that condition and never leaves it active and
/// unmanaged.
///
/// 1. First attempts an authoritative re-read by operation ID. If the
///    re-read proves the exact running transition committed (matching
///    `state`, `run_id`, a present `started_at_utc`, and a matching
///    transition event) despite an uncertain client-side result, the
///    durable truth is accepted — the transaction may have committed even
///    though the client observed an error.
/// 2. If the re-read succeeds but does not prove the exact commit,
///    best-effort stops the exact locally owned run and attempts to
///    persist controller-degraded-equivalent (`manual_intervention_required`)
///    truth against the freshly re-read row.
/// 3. If the re-read itself fails, best-effort stops the local runtime and
///    still projects a bounded `ManualInterventionRequired` outcome against
///    the pre-attempt `operation` snapshot (best-effort persist; the
///    session controller's read-surface projection, REPAIR 7, is the
///    fallback truth channel if that persist attempt also fails). This is
///    the one narrow window this coordinator cannot make fully durable —
///    DB confirmation unavailable *and* DB degradation-persistence
///    unavailable at the same time — documented here rather than silently
///    guessed past.
/// 4. If the operation row itself has vanished, that is a data-integrity
///    defect, not a recoverable condition: best-effort stop, then bail.
#[allow(clippy::too_many_arguments)]
pub async fn handle_running_transition_store_error(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    run_id: Uuid,
    from_state: &'static str,
    store_err: anyhow::Error,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    match mqk_db::fetch_autonomous_daily_operation_by_id(pool, operation.operation_id).await {
        Ok(Some(current)) => {
            let commit_confirmed = current.state.as_str() == STATE_RUNNING
                && current.run_id == Some(run_id)
                && current.started_at_utc.is_some()
                && running_transition_event_matches(
                    pool,
                    current.operation_id,
                    run_id,
                    current.state_version,
                    operation.state.as_str(),
                )
                .await
                .unwrap_or(false);

            if commit_confirmed {
                return Ok(if from_state == STATE_RECOVERY_RETRYING {
                    AutonomousDailyCoordinatorTickOutcome::Recovered { run_id }
                } else {
                    AutonomousDailyCoordinatorTickOutcome::Started { run_id }
                });
            }

            // REPAIR 5 (NONTRADING-RECOVERY-AND-RUNNING-CONFIRMATION-01):
            // re-read does not prove the exact running transition — never
            // leave the run active and unmanaged, and never attempt an
            // illegal degraded target (the re-read row may genuinely be
            // `running`, whose only legal manual-adjacent edge is
            // `controller_degraded`, never `manual_intervention_required`).
            let _ = state.stop_execution_runtime().await;
            let reason = AutonomousCoordinatorReason::UnclassifiedFailClosed {
                fault_class: "durable_transition_unconfirmed_after_start",
            };
            let signature = blocker_signature_for(&current, &reason);
            let detail = "the atomic running-transition store call returned an error and a \
                          re-read did not prove the exact transition committed; best-effort \
                          stop issued, the operation is never presented as running";
            let newly_applied =
                match legal_degraded_target_after_uncertain_running_transition(&current) {
                    Some(target_state) => {
                        apply_manual_if_changed(
                            pool,
                            &current,
                            signature.reason_code,
                            signature.stable_context.clone(),
                            now_utc,
                            detail,
                            target_state,
                        )
                        .await?
                    }
                    None => false,
                };
            Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: signature.reason_code,
                    newly_applied,
                },
            )
        }
        Ok(None) => {
            let _ = state.stop_execution_runtime().await;
            anyhow::bail!(
                "autonomous_daily_coordinator: operation {} vanished after a running-transition \
                 store error ({store_err})",
                operation.operation_id
            )
        }
        Err(_reread_err) => {
            // Narrow, documented ambiguity: neither DB confirmation nor DB
            // degradation-persistence is available. Best-effort stop the
            // runtime and still report a bounded typed outcome so the
            // session controller (REPAIR 7) projects degraded operator
            // truth even if this persist attempt also fails. `operation` is
            // the pre-attempt snapshot (always `start_retrying` or
            // `recovery_retrying` here), which always has a legal edge to
            // `manual_intervention_required`.
            let _ = state.stop_execution_runtime().await;
            let reason = AutonomousCoordinatorReason::UnclassifiedFailClosed {
                fault_class: "durable_transition_unconfirmed_after_start",
            };
            let signature = blocker_signature_for(operation, &reason);
            let newly_applied = apply_manual_if_changed(
                pool,
                operation,
                signature.reason_code,
                signature.stable_context.clone(),
                now_utc,
                "the atomic running-transition store call returned an error and the \
                 authoritative re-read also failed; best-effort stop issued, never claiming \
                 running",
                STATE_MANUAL_INTERVENTION_REQUIRED,
            )
            .await
            .unwrap_or(true);
            Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: signature.reason_code,
                    newly_applied,
                },
            )
        }
    }
}

/// REPAIR 5 (NONTRADING-RECOVERY-AND-RUNNING-CONFIRMATION-01): choose the
/// one legal target state for degrading an operation whose running-transition
/// commit could not be confirmed after a store error. `current` is the
/// freshly re-read row — its `state` may genuinely be `running` (the CAS
/// committed; only the confirming event lookup failed), a
/// blocker-refresh-eligible degraded state (an earlier tick already degraded
/// it), or a pre-running state (the CAS never committed). Returns `None`
/// only when no legal edge exists at all — this function never attempts an
/// illegal transition (in particular, `running -> manual_intervention_required`
/// is not a legal edge; `running -> controller_degraded` is).
fn legal_degraded_target_after_uncertain_running_transition(
    current: &AutonomousDailyOperationRecord,
) -> Option<&'static str> {
    if current.state.as_str() == STATE_RUNNING {
        return Some(mqk_db::STATE_CONTROLLER_DEGRADED);
    }
    if mqk_db::is_blocker_refresh_eligible_state(&current.state) {
        return Some(match current.state.as_str() {
            STATE_PREFLIGHT_BLOCKED => STATE_PREFLIGHT_BLOCKED,
            STATE_MANUAL_INTERVENTION_REQUIRED => STATE_MANUAL_INTERVENTION_REQUIRED,
            mqk_db::STATE_CONTROLLER_DEGRADED => mqk_db::STATE_CONTROLLER_DEGRADED,
            mqk_db::STATE_EVIDENCE_DEGRADED => mqk_db::STATE_EVIDENCE_DEGRADED,
            _ => unreachable!("is_blocker_refresh_eligible_state matched an unmapped state"),
        });
    }
    if mqk_db::is_legal_operation_transition(
        Some(&current.state),
        STATE_MANUAL_INTERVENTION_REQUIRED,
    ) {
        return Some(STATE_MANUAL_INTERVENTION_REQUIRED);
    }
    None
}

async fn classify_start_sequence_failure(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    reason: AutonomousCoordinatorReason,
    now_utc: DateTime<Utc>,
    attempt_number: u64,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    let signature = blocker_signature_for(operation, &reason);
    match classify_autonomous_reason(&reason) {
        AutonomousRetryClass::WaitForCondition => {
            apply_transition(
                pool,
                operation,
                STATE_PREFLIGHT_BLOCKED,
                Some(signature.reason_code),
                signature.stable_context.clone(),
                now_utc,
                None,
                "canonical start call returned a wait-for-condition truth",
            )
            .await?;
            Ok(AutonomousDailyCoordinatorTickOutcome::PreflightBlocked {
                reason_code: signature.reason_code,
            })
        }
        AutonomousRetryClass::RetryableTransient => {
            let next_retry = next_retry_at(now_utc, attempt_number);
            mqk_db::record_retry_timing(
                pool,
                operation.operation_id,
                Some(next_retry),
                Some(signature.reason_code),
                now_utc,
            )
            .await?;
            Ok(AutonomousDailyCoordinatorTickOutcome::StartAttempted)
        }
        AutonomousRetryClass::ManualInterventionRequired
        | AutonomousRetryClass::SessionTerminal => {
            let newly_applied = apply_manual_if_changed(
                pool,
                operation,
                signature.reason_code,
                signature.stable_context.clone(),
                now_utc,
                "canonical start call returned a manual-intervention truth",
                STATE_MANUAL_INTERVENTION_REQUIRED,
            )
            .await?;
            Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: signature.reason_code,
                    newly_applied,
                },
            )
        }
    }
}

// ---------------------------------------------------------------------------
// D2.15 — Runtime ownership reconciliation
// ---------------------------------------------------------------------------

pub async fn handle_running(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: AutonomousDailyOperationRecord,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    let Some(expected_run_id) = operation.run_id else {
        let newly_applied = apply_manual_if_changed(
            pool,
            &operation,
            "running_operation_missing_run_id",
            None,
            now_utc,
            "operation is running but carries no run_id",
            mqk_db::STATE_CONTROLLER_DEGRADED,
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "running_operation_missing_run_id",
                newly_applied,
            },
        );
    };

    let local_run_id = state.locally_owned_run_id().await;
    if local_run_id == Some(expected_run_id) {
        return Ok(AutonomousDailyCoordinatorTickOutcome::Running {
            run_id: expected_run_id,
        });
    }
    if local_run_id.is_some() {
        let newly_applied = apply_manual_if_changed(
            pool,
            &operation,
            "runtime_run_id_mismatch",
            None,
            now_utc,
            "the locally-owned run does not match this operation's durable run_id",
            mqk_db::STATE_CONTROLLER_DEGRADED,
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "runtime_run_id_mismatch",
                newly_applied,
            },
        );
    }

    // No local runtime. Determine whether the durable run row is still
    // active/halted (a crash-window inconsistency: manual) or genuinely,
    // safely terminal (eligible for recovery). Shared with the
    // `evidence_degraded` same-session recovery path
    // (`attempt_evidence_degraded_recovery`) so a future new unsafe-
    // termination signal added here is never silently absent there.
    if let Some(outcome) = check_terminated_run_safe_to_recover(
        state,
        pool,
        &operation,
        expected_run_id,
        now_utc,
        mqk_db::STATE_CONTROLLER_DEGRADED,
    )
    .await?
    {
        return Ok(outcome);
    }

    // Terminal run, no unsafe-termination truth, session still open:
    // schedule a bounded recovery retry.
    let (updated, _) = apply_transition(
        pool,
        &operation,
        STATE_RECOVERY_RETRYING,
        Some("runtime_ended_without_halt"),
        None,
        now_utc,
        None,
        "local runtime ended without an unsafe-termination signal; scheduling recovery retry",
    )
    .await?;
    let next_retry = next_retry_at(now_utc, (updated.start_attempt_count.max(0) as u64) + 1);
    mqk_db::record_retry_timing(
        pool,
        updated.operation_id,
        Some(next_retry),
        Some("runtime_ended_without_halt"),
        now_utc,
    )
    .await?;
    Ok(AutonomousDailyCoordinatorTickOutcome::RecoveryScheduled)
}

/// Extracted from [`handle_running`] (PAPER-SOAK-4DAY-20260818-01 EVIDENCE-
/// DEGRADED-RECOVERY-01): the one shared proof that a run with no matching
/// local runtime handle is genuinely, *safely* terminal — never merely
/// "not currently `ARMED`/`RUNNING`" (that would also pass a durably
/// `HALTED` run, whose sticky halt is never safe to route back toward
/// `running`), and never ended via an unsafe-termination signal (a durable
/// `DISARMED` arm state, an operator halt/kill-switch, or a WS continuity
/// gap). Returns `Ok(None)` when the run is proven safely terminal;
/// `Ok(Some(outcome))` with the caller's own legal degraded `target`
/// otherwise. The caller is responsible for the "local runtime still
/// exactly matches this run" short-circuit first — that is a legitimate
/// success case for a caller whose own current state is `running`, but is
/// itself a contradiction for a caller (`evidence_degraded`) that should
/// never have a matching local runtime at all, so it is not shared here.
async fn check_terminated_run_safe_to_recover(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    expected_run_id: Uuid,
    now_utc: DateTime<Utc>,
    target: &'static str,
) -> anyhow::Result<Option<AutonomousDailyCoordinatorTickOutcome>> {
    let run_row = match mqk_db::fetch_run(pool, expected_run_id).await {
        Ok(row) => row,
        Err(_err) => {
            let newly_applied = apply_manual_if_changed(
                pool,
                operation,
                "run_row_fetch_failed",
                None,
                now_utc,
                "failed to fetch the durable run row while reconciling run ownership",
                target,
            )
            .await?;
            return Ok(Some(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "run_row_fetch_failed",
                    newly_applied,
                },
            ));
        }
    };
    // Strictly `Stopped` -- not merely "not Armed/Running". A durably
    // `HALTED` run (reachable via the sticky, non-CAS-guarded `halt_run`)
    // must never be treated as safe to recover just because it is also not
    // "active"; it is grouped with the active case under the same existing
    // reason code, since neither shape is a safely proven stop.
    let run_is_safely_terminal = matches!(run_row.status, mqk_db::RunStatus::Stopped);
    if !run_is_safely_terminal {
        let newly_applied = apply_manual_if_changed(
            pool,
            operation,
            "durable_active_run_without_local_owner",
            None,
            now_utc,
            "the durable run row is not durably STOPPED (still armed/running, or durably \
             HALTED) but no local runtime owns it",
            target,
        )
        .await?;
        return Ok(Some(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "durable_active_run_without_local_owner",
                newly_applied,
            },
        ));
    }

    // REPAIR 9: resolve durable DISARMED truth before ever presenting a run
    // as safe to recover — never postponed until the next canonical start
    // attempt, and never auto-armed merely to perform this read.
    let durable_disarmed = match mqk_db::load_arm_state(pool).await {
        Ok(Some((ref state_str, _))) => state_str != "ARMED",
        Ok(None) => false,
        Err(_err) => {
            let newly_applied = apply_manual_if_changed(
                pool,
                operation,
                "arm_state_read_failed",
                None,
                now_utc,
                "failed to read durable arm state while reconciling an ended run; failing \
                 closed rather than scheduling an unsafe recovery",
                target,
            )
            .await?;
            return Ok(Some(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "arm_state_read_failed",
                    newly_applied,
                },
            ));
        }
    };

    // `integrity.disarmed` (in-memory) is deliberately not consulted here:
    // it is the fail-closed default at every daemon boot, not evidence of
    // an unsafe termination on its own. The durable DB arm-state row
    // (`durable_disarmed` above) is the authoritative truth for this check.
    let unsafe_termination = durable_disarmed
        || {
            let ig = state.integrity.read().await;
            ig.halted
        }
        || matches!(
            state.alpaca_ws_continuity().await,
            super::AlpacaWsContinuityState::GapDetected { .. }
        );
    if unsafe_termination {
        let reason_code: &'static str = if durable_disarmed {
            "durable_arm_disarmed"
        } else {
            "runtime_ended_unsafely"
        };
        let newly_applied = apply_manual_if_changed(
            pool,
            operation,
            reason_code,
            None,
            now_utc,
            "the local runtime ended via an operator halt/kill-switch, a durable DISARMED arm \
             state, or a WS continuity gap; this is never automatically retried",
            target,
        )
        .await?;
        return Ok(Some(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code,
                newly_applied,
            },
        ));
    }

    Ok(None)
}

// ---------------------------------------------------------------------------
// PAPER-SOAK-4DAY-20260818-01 EVIDENCE-DEGRADED-RECOVERY-01 — same-session
// recovery from `evidence_degraded` back to `running`
// ---------------------------------------------------------------------------
//
// `evidence_degraded` -> `running` is a legal edge in the durable operation
// state graph (`mqk_db::is_legal_operation_transition`) that, before this
// repair, no production caller ever requested. The graph's own commentary
// distinguishes two distinct shapes of `evidence_degraded`:
//
//   - mid-run (`stopped_at_utc IS NULL`, reached only from `running` via a
//     critical evidence fault): never finalization-eligible, stays exactly
//     where it is pending manual intervention. This repair does not touch
//     that shape at all -- unchanged.
//   - post-stop (`stopped_at_utc` set, reached via `stopping`/`stop_retrying`
//     -> the E2B classifier's `EvidenceBlocked` outcome): the "finalization-
//     evidence-gap" case -- the run genuinely ended, but the day's outcome
//     cannot yet be proven. Of the eight closed `unknown_*` reason codes,
//     exactly one (`unknown_incomplete_bar_coverage`) describes a condition
//     that can legitimately clear with more time inside the same session
//     (bars may still arrive before the session's own window closes); every
//     other reason is a genuine identity/data/safety contradiction that
//     stays manual. This function is the missing recovery attempt for that
//     one narrow, closed case -- a fresh run under the *same* operation_id
//     (never a duplicate daily operation), gated on the identical run-
//     termination-safety proof `handle_running` already requires before
//     ever scheduling `recovery_retrying`, plus this operation's own zero-
//     unresolved-outbox / zero-unresolved-inbox / clean-global-reconcile
//     proof. `classify_autonomous_daily_outcome` and
//     `apply_evidence_degraded_blocker` are completely unmodified; a row
//     that is not recovery-eligible this tick falls through to exactly the
//     existing `handle_outcome_finalization` behavior.
// ---------------------------------------------------------------------------

/// `Ok(None)`: recovery does not apply this tick -- the caller must fall
/// through to the existing `handle_outcome_finalization` behavior,
/// unchanged. `Ok(Some(outcome))`: a recovery decision was made this tick
/// (scheduled, attempted, or refused with a specific diagnostic reason).
async fn attempt_evidence_degraded_recovery(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<Option<AutonomousDailyCoordinatorTickOutcome>> {
    // Exactly one closed reason code is recovery-eligible. Every other
    // `unknown_*` reason (identity unavailable, run lineage unavailable,
    // unresolved dispatch claim, missing evaluation evidence, order
    // evidence conflict, database unavailable, or -- critically --
    // `unknown_runtime_stop_unproven`, where attempting a fresh start could
    // race a still-ambiguous prior runtime) is never eligible here.
    if operation.state_reason_code.as_deref()
        != Some(super::autonomous_daily_outcome::REASON_UNKNOWN_INCOMPLETE_BAR_COVERAGE)
    {
        return Ok(None);
    }
    // AUTONOMOUS-DAILY-STOPPING-EVIDENCE-DEGRADED-OSCILLATION-01-REPAIR-01:
    // recovery is a fresh start, so it is gated on the same
    // `effective_operation_close_utc` boundary every other fresh-start path
    // in this file already enforces (D2.17 and its siblings) -- never on
    // `postclose_finalize_utc`. `postclose_finalize_utc` is a *later*
    // grace-period deadline for an in-progress *stop* to finish
    // (`handle_stopping`'s own boundary); it was never a legal start
    // boundary. Gating this check on it alone left the full
    // `postclose_finalize_delay` window (15 minutes) after close during
    // which this arm could still schedule or genuinely attempt a fresh
    // start -- there is no time left to observe bars once the session's own
    // close has passed, and finalization remains the sole authority from
    // that instant on.
    if now_utc >= operation.effective_operation_close_utc {
        return Ok(None);
    }

    if let Some(expected_run_id) = operation.run_id {
        // `evidence_degraded` never legitimately has a matching (or any)
        // local runtime handle -- unlike `running`'s own recovery path,
        // there is no "still exactly this run" success case to short-
        // circuit here; any local ownership at all is a contradiction with
        // this operation's own durable `stopped_at_utc` truth.
        if state.locally_owned_run_id().await.is_some() {
            let newly_applied = apply_manual_if_changed(
                pool,
                operation,
                "runtime_run_id_mismatch",
                None,
                now_utc,
                "a local runtime handle exists while this operation's durable state says its \
                 prior run already stopped; failing closed rather than assuming which is true",
                STATE_MANUAL_INTERVENTION_REQUIRED,
            )
            .await?;
            return Ok(Some(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "runtime_run_id_mismatch",
                    newly_applied,
                },
            ));
        }

        if let Some(outcome) = check_terminated_run_safe_to_recover(
            state,
            pool,
            operation,
            expected_run_id,
            now_utc,
            STATE_MANUAL_INTERVENTION_REQUIRED,
        )
        .await?
        {
            return Ok(Some(outcome));
        }

        let unacked = mqk_db::outbox_list_unacked_for_run(pool, expected_run_id)
            .await
            .context("attempt_evidence_degraded_recovery: outbox_list_unacked_for_run failed")?;
        if !unacked.is_empty() {
            let newly_applied = apply_manual_if_changed(
                pool,
                operation,
                "evidence_degraded_recovery_unresolved_outbox",
                None,
                now_utc,
                "the prior run is durably STOPPED, but it still has unacked outbox rows; never \
                 attempting a fresh start while an order may still be in flight",
                STATE_MANUAL_INTERVENTION_REQUIRED,
            )
            .await?;
            return Ok(Some(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "evidence_degraded_recovery_unresolved_outbox",
                    newly_applied,
                },
            ));
        }

        let unapplied = mqk_db::inbox_load_unapplied_for_run(pool, expected_run_id)
            .await
            .context("attempt_evidence_degraded_recovery: inbox_load_unapplied_for_run failed")?;
        if !unapplied.is_empty() {
            let newly_applied = apply_manual_if_changed(
                pool,
                operation,
                "evidence_degraded_recovery_unresolved_inbox",
                None,
                now_utc,
                "the prior run is durably STOPPED, but it still has unapplied inbox rows; never \
                 attempting a fresh start while broker evidence is not fully applied",
                STATE_MANUAL_INTERVENTION_REQUIRED,
            )
            .await?;
            return Ok(Some(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "evidence_degraded_recovery_unresolved_inbox",
                    newly_applied,
                },
            ));
        }
    }

    let reconcile_dirty = match mqk_db::load_reconcile_status_state(pool)
        .await
        .context("attempt_evidence_degraded_recovery: load_reconcile_status_state failed")?
    {
        Some(r) => {
            r.status != "ok"
                || r.mismatched_positions != 0
                || r.mismatched_orders != 0
                || r.mismatched_fills != 0
        }
        // No durable reconcile status at all is not evidence of agreement --
        // fail closed rather than assume clean.
        None => true,
    };
    if reconcile_dirty {
        let newly_applied = apply_manual_if_changed(
            pool,
            operation,
            "evidence_degraded_recovery_reconcile_dirty",
            None,
            now_utc,
            "the global broker/local reconcile status is not currently clean; never attempting \
             a fresh start while local/broker truth may disagree",
            STATE_MANUAL_INTERVENTION_REQUIRED,
        )
        .await?;
        return Ok(Some(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "evidence_degraded_recovery_reconcile_dirty",
                newly_applied,
            },
        ));
    }

    // Every proof clean: this tick may schedule or perform a fresh start,
    // exactly mirroring `running`'s own `recovery_retrying` two-phase
    // pattern (schedule a bounded backoff first; a later, due tick performs
    // the real canonical start call) -- without inventing a dedicated
    // intermediate DB state, by self-looping the retry-timing fields while
    // `state` remains `evidence_degraded`.
    if operation.next_retry_utc.is_none() {
        let next_retry =
            next_retry_at(now_utc, (operation.start_attempt_count.max(0) as u64) + 1);
        mqk_db::record_retry_timing(
            pool,
            operation.operation_id,
            Some(next_retry),
            Some("evidence_degraded_recovery_scheduled"),
            now_utc,
        )
        .await?;
        return Ok(Some(AutonomousDailyCoordinatorTickOutcome::RecoveryScheduled));
    }

    // `attempt_canonical_start` re-checks `next_retry_utc` itself and
    // returns `RetryNotDue` cheaply (no arm/start call) if called early --
    // reused unmodified, including its own operator-managed-run guard, its
    // typed arm call, its atomic running-transition CAS (which itself
    // re-validates `is_legal_operation_transition`), and its own retry-
    // backoff scheduling on failure.
    Ok(Some(
        attempt_canonical_start(
            state,
            pool,
            operation.clone(),
            now_utc,
            mqk_db::STATE_EVIDENCE_DEGRADED,
        )
        .await?,
    ))
}

// ---------------------------------------------------------------------------
// D2.17-D2.18 — Session close and durable stop retries
// ---------------------------------------------------------------------------

/// REPAIR 3/4 shared helper: reconcile truth for an operation that durably
/// bound `expected_run_id` but currently has no matching local runtime.
/// Never records `stopped_at_utc` and never presents the operation as
/// stopped without first confirming the durable run row is actually
/// terminal — an active orphaned run (or an unreadable run row) always
/// fails closed to a manual/controller-degraded truth instead.
async fn reconcile_durable_run_without_local_owner(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    expected_run_id: Uuid,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    // `running` has a legal edge straight to `controller_degraded`; every
    // other state this helper can be entered from (`stopping`,
    // `stop_retrying`, or an already-`controller_degraded` retry) does not,
    // but does have a legal edge to `manual_intervention_required`.
    let target: &'static str = if operation.state.as_str() == STATE_RUNNING {
        mqk_db::STATE_CONTROLLER_DEGRADED
    } else {
        STATE_MANUAL_INTERVENTION_REQUIRED
    };

    let run_row = match mqk_db::fetch_run(pool, expected_run_id).await {
        Ok(row) => row,
        Err(_err) => {
            let newly_applied = apply_manual_if_changed(
                pool,
                operation,
                "run_row_fetch_failed",
                None,
                now_utc,
                "failed to fetch the durable run row while reconciling stop-time ownership; \
                 failing closed rather than assuming stopped",
                target,
            )
            .await?;
            return Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "run_row_fetch_failed",
                    newly_applied,
                },
            );
        }
    };

    let run_is_active = matches!(
        run_row.status,
        mqk_db::RunStatus::Armed | mqk_db::RunStatus::Running
    );
    if run_is_active {
        let newly_applied = apply_manual_if_changed(
            pool,
            operation,
            "durable_active_run_without_local_owner",
            None,
            now_utc,
            "the durable run row is still armed/running but no local runtime owns it; never \
             presented as stopped",
            target,
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "durable_active_run_without_local_owner",
                newly_applied,
            },
        );
    }

    // AUTONOMOUS-DAILY-CONTROLLER-DEGRADED-RECOVERY-01: a terminal run row
    // alone is not sufficient proof that it is safe to present this
    // operation as stopped -- an unacked outbox row (still PENDING/CLAIMED/
    // DISPATCHING/SENT/FAILED/AMBIGUOUS) means an order may still be in
    // flight to or from the broker with no local runtime left to resolve
    // it, and a dirty global reconcile status means local/broker truth is
    // not currently known to agree. Both fail closed, never silently
    // ignored, applying to every caller of this helper (session-close,
    // stop-retry, and the controller_degraded same-state refresh alike).
    let unacked = mqk_db::outbox_list_unacked_for_run(pool, expected_run_id)
        .await
        .context("reconcile_durable_run_without_local_owner: outbox_list_unacked_for_run failed")?;
    if !unacked.is_empty() {
        let newly_applied = apply_manual_if_changed(
            pool,
            operation,
            "unresolved_outbox_at_run_reconcile",
            None,
            now_utc,
            "the durable run row is terminal, but it still has unacked outbox rows; failing \
             closed rather than presenting this operation as safely stopped",
            target,
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "unresolved_outbox_at_run_reconcile",
                newly_applied,
            },
        );
    }

    let reconcile_dirty = match mqk_db::load_reconcile_status_state(pool)
        .await
        .context("reconcile_durable_run_without_local_owner: load_reconcile_status_state failed")?
    {
        Some(r) => {
            r.status != "ok"
                || r.mismatched_positions != 0
                || r.mismatched_orders != 0
                || r.mismatched_fills != 0
        }
        // No durable reconcile status at all is not evidence of agreement --
        // fail closed rather than assume clean.
        None => true,
    };
    if reconcile_dirty {
        let newly_applied = apply_manual_if_changed(
            pool,
            operation,
            "reconcile_dirty",
            None,
            now_utc,
            "the durable run row is terminal, but the global broker/local reconcile status is \
             not currently clean; failing closed rather than presenting this operation as \
             safely stopped",
            target,
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "reconcile_dirty",
                newly_applied,
            },
        );
    }

    // Terminal durable run, zero unresolved economic evidence: safe to
    // record stopped without ever calling the canonical stop path.
    let updated = if matches!(
        operation.state.as_str(),
        STATE_STOPPING | STATE_STOP_RETRYING
    ) {
        operation.clone()
    } else {
        apply_transition(
            pool,
            operation,
            STATE_STOPPING,
            None,
            None,
            now_utc,
            None,
            "durable run already terminal at reconciliation time; no local runtime to stop",
        )
        .await?
        .0
    };
    mqk_db::record_autonomous_runtime_stopped(pool, updated.operation_id, now_utc).await?;
    Ok(AutonomousDailyCoordinatorTickOutcome::RuntimeStopped)
}

pub async fn handle_session_close(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: AutonomousDailyOperationRecord,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    let local_run_id = state.locally_owned_run_id().await;

    match (operation.run_id, local_run_id) {
        (Some(expected), Some(actual)) if expected == actual => {
            let (updated, _) = apply_transition(
                pool,
                &operation,
                STATE_STOPPING,
                None,
                None,
                now_utc,
                None,
                "effective operation window closed; stopping the matching autonomous runtime",
            )
            .await?;
            retry_stop(state, pool, updated, now_utc).await
        }
        (Some(_), Some(_)) => {
            // Mismatched runtime: never stopped by the coordinator.
            let target: &'static str = if operation.state.as_str() == STATE_RUNNING {
                mqk_db::STATE_CONTROLLER_DEGRADED
            } else {
                STATE_MANUAL_INTERVENTION_REQUIRED
            };
            let newly_applied = apply_manual_if_changed(
                pool,
                &operation,
                "mismatched_runtime_at_close",
                None,
                now_utc,
                "session closed with a locally-owned runtime that does not match this \
                 operation's run_id; the coordinator will not stop it",
                target,
            )
            .await?;
            Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "mismatched_runtime_at_close",
                    newly_applied,
                },
            )
        }
        (None, Some(_)) => {
            // D2.16: an operator-managed run is preserved, never stopped,
            // never attached.
            let newly_applied = apply_manual_if_changed(
                pool,
                &operation,
                "operator_managed_run_active",
                None,
                now_utc,
                "session closed with a locally-owned run this operation never started; the \
                 coordinator will not stop it",
                STATE_MANUAL_INTERVENTION_REQUIRED,
            )
            .await?;
            Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "operator_managed_run_active",
                    newly_applied,
                },
            )
        }
        (None, None) => {
            // This operation never bound a run_id and none exists locally:
            // no ambiguity, no run row to reconcile against.
            let (updated, _) = apply_transition(
                pool,
                &operation,
                STATE_STOPPING,
                None,
                None,
                now_utc,
                None,
                "no autonomous runtime existed at session close",
            )
            .await?;
            mqk_db::record_autonomous_runtime_stopped(pool, updated.operation_id, now_utc).await?;
            Ok(AutonomousDailyCoordinatorTickOutcome::RuntimeStopped)
        }
        (Some(expected), None) => {
            // REPAIR 3: a durable run_id is bound but no local runtime
            // exists — never assume stopped without confirming the durable
            // run row is actually terminal.
            reconcile_durable_run_without_local_owner(pool, &operation, expected, now_utc).await
        }
    }
}

pub async fn handle_stopping(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: AutonomousDailyOperationRecord,
    postclose_finalize_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-
    // INTEGRATION-AND-NOTIFICATION (E3.1/E3.11): the durable "runtime
    // concluded" signal is now routed into E2B's finalization classifier
    // rather than reported as a static, permanently-repeating
    // `AwaitingOutcomeFinalization` no-op. This single call site is reached
    // by every caller of `handle_stopping` -- `dispatch_by_state`'s ordinary
    // per-tick routing and `reconcile_existing_operation_against_relevant_
    // lookup`'s fallback-lookup routing alike -- so a stopped, finalization-
    // eligible operation discovered through either path is never abandoned.
    if operation.stopped_at_utc.is_some() {
        return handle_outcome_finalization(state, pool, operation, now_utc).await;
    }
    if now_utc >= postclose_finalize_utc {
        let newly_applied = apply_manual_if_changed(
            pool,
            &operation,
            "unresolved_stop_failure_at_postclose_finalize",
            None,
            now_utc,
            "the operation's runtime stop remained unresolved at postclose_finalize_utc",
            STATE_MANUAL_INTERVENTION_REQUIRED,
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "unresolved_stop_failure_at_postclose_finalize",
                newly_applied,
            },
        );
    }
    if let Some(next_retry_utc) = operation.next_retry_utc {
        if now_utc < next_retry_utc {
            return Ok(AutonomousDailyCoordinatorTickOutcome::RetryNotDue);
        }
    }
    retry_stop(state, pool, operation, now_utc).await
}

pub async fn retry_stop(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: AutonomousDailyOperationRecord,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    // REPAIR 4: re-read ownership immediately before every stop call —
    // never rely on a routing decision made on an earlier tick. A runtime
    // that appeared, disappeared, or changed identity since the last tick
    // must never be stopped (or left un-reconciled) merely because the
    // operation remains `stop_retrying`.
    let local_run_id = state.locally_owned_run_id().await;

    match (operation.run_id, local_run_id) {
        (Some(expected), Some(actual)) if expected == actual => {
            // Ownership confirmed: fall through to the canonical stop call.
        }
        (Some(_), Some(_)) => {
            let newly_applied = apply_manual_if_changed(
                pool,
                &operation,
                "mismatched_runtime_at_close",
                None,
                now_utc,
                "a locally-owned runtime that does not match this operation's run_id appeared \
                 before a stop retry; the coordinator will not stop it",
                STATE_MANUAL_INTERVENTION_REQUIRED,
            )
            .await?;
            return Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "mismatched_runtime_at_close",
                    newly_applied,
                },
            );
        }
        (None, Some(_)) => {
            let newly_applied = apply_manual_if_changed(
                pool,
                &operation,
                "operator_managed_run_active",
                None,
                now_utc,
                "a locally-owned run this operation never started appeared before a stop \
                 retry; the coordinator will not stop it",
                STATE_MANUAL_INTERVENTION_REQUIRED,
            )
            .await?;
            return Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "operator_managed_run_active",
                    newly_applied,
                },
            );
        }
        (Some(expected), None) => {
            // The local runtime disappeared between ticks (e.g. reaped by
            // an earlier failed stop). Reconcile against the durable run
            // row rather than assuming stopped or calling stop again.
            return reconcile_durable_run_without_local_owner(pool, &operation, expected, now_utc)
                .await;
        }
        (None, None) => {
            // No run was ever bound and none exists locally: already
            // effectively stopped, without a second stop call.
            mqk_db::record_autonomous_runtime_stopped(pool, operation.operation_id, now_utc)
                .await?;
            return Ok(AutonomousDailyCoordinatorTickOutcome::RuntimeStopped);
        }
    }

    let attempt_number =
        match mqk_db::record_stop_attempt(pool, operation.operation_id, now_utc).await? {
            mqk_db::RecordStopAttemptOutcome::Recorded { stop_attempt_count } => {
                stop_attempt_count.max(0) as u64
            }
            mqk_db::RecordStopAttemptOutcome::NotFound => {
                anyhow::bail!(
                "autonomous_daily_coordinator: operation {} vanished before stop-attempt recording",
                operation.operation_id
            )
            }
        };

    match state.stop_execution_runtime().await {
        Ok(_) => {
            // REPAIR 5: restart-safe, idempotent stop completion — never
            // rewinds an already-recorded stopped_at_utc, and clears stale
            // retry/error/blocker evidence atomically.
            mqk_db::record_autonomous_runtime_stopped(pool, operation.operation_id, now_utc)
                .await?;
            Ok(AutonomousDailyCoordinatorTickOutcome::RuntimeStopped)
        }
        Err(err) => {
            if operation.state.as_str() != STATE_STOP_RETRYING {
                apply_transition(
                    pool,
                    &operation,
                    STATE_STOP_RETRYING,
                    None,
                    None,
                    now_utc,
                    None,
                    "canonical stop attempt failed; scheduling retry",
                )
                .await?;
            }
            let next_retry = next_retry_at(now_utc, attempt_number);
            mqk_db::record_retry_timing(
                pool,
                operation.operation_id,
                Some(next_retry),
                Some(err.fault_class()),
                now_utc,
            )
            .await?;
            Ok(AutonomousDailyCoordinatorTickOutcome::StopAttempted)
        }
    }
}

// ---------------------------------------------------------------------------
// AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-
// INTEGRATION-AND-NOTIFICATION -- outcome finalization routing
// ---------------------------------------------------------------------------

/// E3.1: the durable matching-local-runtime fact, derived exactly as E1
/// contract §3.2 condition 4 requires -- `operation.run_id` compared against
/// `AppState::locally_owned_run_id()`. Never `locally_started`, task
/// liveness, a process-local bar counter, or GUI state. `operation.run_id ==
/// None` (never started) is never "matching" -- there is nothing to match.
async fn matching_local_runtime_active(
    state: &Arc<AppState>,
    operation: &AutonomousDailyOperationRecord,
) -> bool {
    match operation.run_id {
        Some(expected) => state.locally_owned_run_id().await == Some(expected),
        None => false,
    }
}

/// E3.2/E3.3: route one finalization-eligible tick (`stopping`/
/// `stop_retrying` with `stopped_at_utc` present, or `evidence_degraded`
/// with `stopped_at_utc` present) into E2B's accepted
/// `classify_and_finalize_autonomous_daily_operation`. Callers must only
/// invoke this when `operation.stopped_at_utc.is_some()` -- this function
/// performs no additional eligibility gating of its own beyond what E2B's
/// own `check_finalization_eligibility` already re-verifies internally, and
/// it invokes E2B at most once per call.
///
/// E3.3: current policy inputs are resolved fresh, once per finalization
/// attempt, from exactly the same production authorities
/// `ensure_coverage_authority` already threads through
/// `resolve_current_coverage_policy_inputs` -- no second environment parser,
/// no duplicate runtime-binding algorithm, no cached process-local policy,
/// no provider network call.
///
/// E3.4: when current assignment/config/runtime-binding resolution itself
/// fails (before evidence gathering can even begin, since there is no real
/// `MultiSymbolRuntimeConfig`/`EffectiveRuntimeBinding` to construct
/// `AutonomousDailyFinalizationPolicyInputs` from), this function persists
/// `unknown_assignment_identity_unavailable` via E2B's own
/// `persist_autonomous_daily_finalization_blocker` seam -- never a
/// fabricated empty configuration, never a second, coordinator-owned
/// blocker writer.
// Visibility note (AUTONOMOUS-DAILY-STALE-EVIDENCE-DEGRADED-FINALIZATION-01):
// `pub(crate)` solely so `routes::autonomous_daily_operator`'s narrow,
// explicit stale-operation finalize route can invoke the exact same
// finalization codepath ordinary coordinator ticks use, on a specific
// operation record the ambiguity-detecting `fetch_relevant_open_
// autonomous_daily_operation` can never select for a normal tick once more
// than one PAPER/adapter operation is simultaneously "relevant". No logic
// in this function changed for that route's sake.
pub(crate) async fn handle_outcome_finalization(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: AutonomousDailyOperationRecord,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    let context = super::autonomous_daily_outcome::AutonomousDailyFinalizationContext {
        matching_local_runtime_active: matching_local_runtime_active(state, &operation).await,
    };

    // AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-MATCHING-RUNTIME-POLICY-FAILURE-
    // GATE-REPAIR-01: a matching local runtime is still active -- finalization
    // is not eligible regardless of whether current policy/config/runtime-
    // context resolution succeeds or fails this tick (E1 contract §3.2
    // condition 4). Return before config/runtime-context resolution and
    // before the policy-failure blocker seam are ever reached: zero DB
    // writes, zero notifications -- exactly the `NotEligible` truth E2B's own
    // eligibility check would have produced had policy resolution succeeded
    // this tick.
    if context.matching_local_runtime_active {
        return Ok(AutonomousDailyCoordinatorTickOutcome::AwaitingOutcomeFinalization);
    }

    let config = match crate::state::build_multi_symbol_runtime_config_from_env() {
        Ok(config) => config,
        Err(_) => {
            let outcome = super::autonomous_daily_outcome::persist_autonomous_daily_finalization_blocker(
                pool,
                &operation,
                now_utc,
                super::autonomous_daily_outcome::AutonomousDailyUnknownReason::AssignmentIdentityUnavailable,
                context,
            )
            .await?;
            return Ok(project_finalization_outcome(outcome));
        }
    };

    let runtime_context = match resolve_autonomous_runtime_context(state).await {
        Ok(ctx) => ctx,
        Err(_) => {
            let outcome = super::autonomous_daily_outcome::persist_autonomous_daily_finalization_blocker(
                pool,
                &operation,
                now_utc,
                super::autonomous_daily_outcome::AutonomousDailyUnknownReason::AssignmentIdentityUnavailable,
                context,
            )
            .await?;
            return Ok(project_finalization_outcome(outcome));
        }
    };

    let readiness_context = crate::daily_data_readiness::load_readiness_context_from_env();

    let policy_inputs = super::autonomous_daily_outcome::AutonomousDailyFinalizationPolicyInputs {
        calendar_provider: readiness_context.calendar_provider.as_ref(),
        config: &config,
        binding: &runtime_context.effective_runtime_binding,
        strategy_registry: &readiness_context.strategy_registry,
    };

    let outcome =
        super::autonomous_daily_outcome::classify_and_finalize_autonomous_daily_operation(
            pool,
            operation.operation_id,
            now_utc,
            context,
            &policy_inputs,
        )
        .await?;

    Ok(project_finalization_outcome(outcome))
}

/// E3.5/E3.9: project E2B's [`AutonomousDailyFinalizationOutcome`] into a
/// bounded typed [`AutonomousDailyCoordinatorTickOutcome`] variant -- carries
/// only bounded typed facts (operation state/outcome/reason codes,
/// `newly_applied`), never a raw `anyhow` error, SQL text, connection
/// string, filesystem path, provider payload, or panic string.
fn project_finalization_outcome(
    outcome: super::autonomous_daily_outcome::AutonomousDailyFinalizationOutcome,
) -> AutonomousDailyCoordinatorTickOutcome {
    use super::autonomous_daily_outcome::AutonomousDailyFinalizationOutcome as FinalizationOutcome;
    match outcome {
        FinalizationOutcome::NotEligible => {
            AutonomousDailyCoordinatorTickOutcome::AwaitingOutcomeFinalization
        }
        FinalizationOutcome::AlreadyFinalized(record) => {
            AutonomousDailyCoordinatorTickOutcome::OutcomeAlreadyFinalized {
                state: record.state,
                outcome_reason_code: record.outcome,
            }
        }
        FinalizationOutcome::Finalized { outcome, record } => {
            AutonomousDailyCoordinatorTickOutcome::OutcomeFinalized {
                operation_id: record.operation_id,
                run_id: record.run_id,
                outcome_reason_code: outcome,
            }
        }
        FinalizationOutcome::EvidenceDegraded {
            reason_code,
            newly_applied,
            record,
        } => AutonomousDailyCoordinatorTickOutcome::OutcomeEvidenceDegraded {
            operation_id: record.operation_id,
            run_id: record.run_id,
            reason_code: reason_code.as_str(),
            newly_applied,
        },
        FinalizationOutcome::RecoveredToStopping(_) => {
            AutonomousDailyCoordinatorTickOutcome::OutcomeRecoveredToStopping
        }
        FinalizationOutcome::DatabaseUnavailable => {
            AutonomousDailyCoordinatorTickOutcome::OutcomeFinalizationDatabaseUnavailable
        }
        FinalizationOutcome::Conflict => {
            AutonomousDailyCoordinatorTickOutcome::OutcomeFinalizationConflict
        }
    }
}
