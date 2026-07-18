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

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgPool;
use uuid::Uuid;

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
use super::autonomous_runtime_context::resolve_autonomous_runtime_context;
use super::lifecycle::AutonomousArmOutcome;
use super::AppState;

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
    AwaitingOutcomeFinalization,
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

    let timing = AutonomousDailyPlanTiming::production_default();
    let resolution = resolve_autonomous_daily_session_plan_from_env(now_utc, &timing);

    let plan = match resolution {
        AutonomousDailySessionPlanResolution::NotApplicable { reason_code, .. } => {
            return Ok(AutonomousDailyCoordinatorTickOutcome::NotApplicable {
                reason_code: reason_code.as_str(),
            });
        }
        AutonomousDailySessionPlanResolution::Blocked { reason_code, .. } => {
            return Ok(AutonomousDailyCoordinatorTickOutcome::CalendarBlocked {
                reason_code: reason_code.as_str(),
            });
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
            return Ok(AutonomousDailyCoordinatorTickOutcome::IdentityBlocked {
                reason_code: "assignment_missing",
            });
        }
    };

    let runtime_context = match resolve_autonomous_runtime_context(state).await {
        Ok(ctx) => ctx,
        Err(_err) => {
            return Ok(AutonomousDailyCoordinatorTickOutcome::IdentityBlocked {
                reason_code: "runtime_binding_unresolved",
            });
        }
    };

    let assignment_identity = derive_assignment_identity(&config);
    let runtime_binding_identity =
        derive_runtime_binding_identity(&runtime_context.effective_runtime_binding);
    let deployment_mode = state.deployment_mode().as_db_mode();
    let adapter_id = state.adapter_id().to_string();
    let operation_id = derive_autonomous_daily_operation_id(
        &plan,
        deployment_mode,
        &adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
    );

    let (operation, created) = match create_or_recover(
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

#[allow(clippy::too_many_arguments)]
async fn create_or_recover(
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
            let newly_applied = handle_identity_conflict(
                pool,
                existing_operation_id,
                assignment_identity,
                runtime_binding_identity,
                now_utc,
            )
            .await?;
            Ok(Err(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "operation_identity_conflict",
                    newly_applied,
                },
            ))
        }
    }
}

/// REPAIR 8: durable identity-conflict truth. The daily slot already holds
/// a row whose immutable identity fields differ from the freshly computed
/// expected values (e.g. an operator changed strategy/symbol/timeframe
/// config and restarted mid-day). This never creates a second row and
/// never rewrites the existing row's immutable identity fields; it durably
/// CAS-transitions the *existing* operation to `manual_intervention_required`
/// exactly once, deduplicated by `(state, state_reason_code,
/// state_blocker_signature)`, and survives a process restart because that
/// dedup is entirely DB-driven.
async fn handle_identity_conflict(
    pool: &PgPool,
    existing_operation_id: Uuid,
    assignment_identity: &str,
    runtime_binding_identity: &str,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<bool> {
    let Some(existing) =
        mqk_db::fetch_autonomous_daily_operation_by_id(pool, existing_operation_id).await?
    else {
        // Nothing durable to reconcile against; report the conflict without
        // fabricating a transition against a row that does not exist.
        return Ok(false);
    };

    if mqk_db::is_terminal_operation_state(&existing.state) {
        // Terminal: no lifecycle mutation, nothing started.
        return Ok(false);
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

    if !mqk_db::is_legal_operation_transition(
        Some(&existing.state),
        STATE_MANUAL_INTERVENTION_REQUIRED,
    ) {
        // No legal edge from the existing operation's current state (e.g.
        // `running`) directly to manual intervention this tick. Report the
        // conflict without forcing an illegal transition; a later tick
        // (once the existing operation reaches a state with a legal edge)
        // will durably record it.
        return Ok(false);
    }

    apply_manual_if_changed(
        pool,
        &existing,
        signature.reason_code,
        signature.stable_context.clone(),
        now_utc,
        "a same-day operation identity conflict was detected: the daily slot already holds a \
         row whose immutable identity fields differ from the freshly computed expected values",
        STATE_MANUAL_INTERVENTION_REQUIRED,
    )
    .await
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

/// REPAIR 11: apply a manual-intervention/controller-degraded transition
/// only if `(target_state, reason_code, blocker_signature)` differs from
/// what the operation already durably carries — the sole guard against
/// both an illegal same-state CAS re-attempt (e.g. `controller_degraded ->
/// controller_degraded` is not a legal edge) and a repeated critical
/// notification for an unchanged blocker. Returns whether the transition
/// was newly applied this tick.
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
    if now_utc >= plan.effective_operation_close_utc
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
            handle_stopping(state, pool, operation, plan, now_utc).await
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
        mqk_db::STATE_COMPLETED
        | mqk_db::STATE_COMPLETED_NO_TRADE
        | mqk_db::STATE_COMPLETED_WITH_ACTIVITY => {
            Ok(AutonomousDailyCoordinatorTickOutcome::AwaitingOutcomeFinalization)
        }
        mqk_db::STATE_CONTROLLER_DEGRADED | mqk_db::STATE_EVIDENCE_DEGRADED => Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "controller_degraded",
                newly_applied: false,
            },
        ),
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
            return Ok(AutonomousDailyCoordinatorTickOutcome::PreflightBlocked {
                reason_code: "assignment_missing",
            });
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
            if operation.state.as_str() != STATE_PREFLIGHT_BLOCKED {
                apply_transition(
                    pool,
                    operation,
                    STATE_PREFLIGHT_BLOCKED,
                    Some(signature.reason_code),
                    signature.stable_context.clone(),
                    now_utc,
                    None,
                    "preopen readiness not yet satisfied",
                )
                .await?;
            }
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
            match mqk_db::transition_autonomous_daily_operation_to_running(pool, &to_running_args)
                .await?
            {
                AutonomousDailyTransitionOutcome::Applied(_)
                | AutonomousDailyTransitionOutcome::AlreadyApplied(_) => {
                    if from_state == STATE_RECOVERY_RETRYING {
                        Ok(AutonomousDailyCoordinatorTickOutcome::Recovered { run_id })
                    } else {
                        Ok(AutonomousDailyCoordinatorTickOutcome::Started { run_id })
                    }
                }
                AutonomousDailyTransitionOutcome::StaleState { .. }
                | AutonomousDailyTransitionOutcome::NotFound
                | AutonomousDailyTransitionOutcome::IllegalTransition => {
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
            }
        }
        Err(err) => {
            let reason = coordinator_reason_from_runtime_lifecycle_error(&err);
            classify_start_sequence_failure(pool, &operation, reason, now_utc, attempt_number).await
        }
    }
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
    // active (a crash-window inconsistency: manual) or genuinely terminal
    // (eligible for recovery).
    let run_row = match mqk_db::fetch_run(pool, expected_run_id).await {
        Ok(row) => row,
        Err(_err) => {
            let newly_applied = apply_manual_if_changed(
                pool,
                &operation,
                "run_row_fetch_failed",
                None,
                now_utc,
                "failed to fetch the durable run row while reconciling running-state ownership",
                mqk_db::STATE_CONTROLLER_DEGRADED,
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
            &operation,
            "durable_active_run_without_local_owner",
            None,
            now_utc,
            "the durable run row is still armed/running but no local runtime owns it",
            mqk_db::STATE_CONTROLLER_DEGRADED,
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "durable_active_run_without_local_owner",
                newly_applied,
            },
        );
    }

    // REPAIR 9: resolve durable DISARMED truth before scheduling any
    // recovery retry — never postponed until the next canonical start
    // attempt, and never auto-armed merely to perform this read.
    // Reaching `running` at all guarantees a prior successful
    // `try_autonomous_arm_typed` call, which always re-persists `ARMED` on
    // success -- so a missing arm-state row here is not real evidence of a
    // durable DISARMED transition (that would conflate "never armed" with
    // "explicitly disarmed"); only an explicit non-`ARMED` row counts.
    let durable_disarmed = match mqk_db::load_arm_state(pool).await {
        Ok(Some((ref state_str, _))) => state_str != "ARMED",
        Ok(None) => false,
        Err(_err) => {
            let newly_applied = apply_manual_if_changed(
                pool,
                &operation,
                "arm_state_read_failed",
                None,
                now_utc,
                "failed to read durable arm state while reconciling an ended run; failing \
                 closed rather than scheduling an unsafe recovery",
                mqk_db::STATE_CONTROLLER_DEGRADED,
            )
            .await?;
            return Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "arm_state_read_failed",
                    newly_applied,
                },
            );
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
            &operation,
            reason_code,
            None,
            now_utc,
            "the local runtime ended via an operator halt/kill-switch, a durable DISARMED arm \
             state, or a WS continuity gap; this is never automatically retried",
            mqk_db::STATE_CONTROLLER_DEGRADED,
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code,
                newly_applied,
            },
        );
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

    // Terminal durable run: safe to record stopped without ever calling
    // the canonical stop path.
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
    plan: &AutonomousDailySessionPlan,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    if operation.stopped_at_utc.is_some() {
        return Ok(AutonomousDailyCoordinatorTickOutcome::AwaitingOutcomeFinalization);
    }
    if now_utc >= plan.postclose_finalize_utc {
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
