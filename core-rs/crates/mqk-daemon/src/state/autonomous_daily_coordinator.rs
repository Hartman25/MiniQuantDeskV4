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
    ManualInterventionRequired {
        reason_code: &'static str,
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
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "database_not_configured_or_invalid",
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

    // D2.6: an operation first observed already past close never fabricates
    // a running/stopping history it never had. `create_or_recover` always
    // seeds a fresh row with the narrowest truthful legal initial state
    // (`awaiting_open`, see `initial_state_for_plan`); if that first
    // observation is already past `effective_operation_close_utc`, record
    // this truthfully as one immediate, honest transition rather than
    // silently inventing a `running` step that never happened.
    if created && now_utc >= plan.effective_operation_close_utc {
        apply_transition(
            &pool,
            &operation,
            STATE_MANUAL_INTERVENTION_REQUIRED,
            Some("session_closed_before_first_observation"),
            now_utc,
            None,
            "operation first observed at or after effective_operation_close_utc; \
             no runtime was ever started by this coordinator for this operation",
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "session_closed_before_first_observation",
            },
        );
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
        CreateOrRecoverAutonomousDailyOperationOutcome::IdentityConflict { .. } => Ok(Err(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "operation_identity_conflict",
            },
        )),
    }
}

/// D2.6: the narrowest truthful legal initial state for `now_utc` against
/// `plan`'s boundaries. `stopping` is not a legal initial state (§ graph),
/// so an operation first observed at or after
/// `effective_operation_close_utc` still seeds as `awaiting_open` — the
/// caller (`tick_autonomous_daily_coordinator`) immediately records the one
/// honest follow-up transition to `manual_intervention_required` for that
/// specific case.
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
/// success, not an error.
pub async fn apply_transition(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    new_state: &str,
    reason_code: Option<&str>,
    occurred_at_utc: DateTime<Utc>,
    run_id: Option<Uuid>,
    detail: &str,
) -> anyhow::Result<AutonomousDailyOperationRecord> {
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        new_state: new_state.to_string(),
        reason_code: reason_code.map(bounded_reason),
        occurred_at_utc,
        run_id,
        bounded_detail: bounded_detail(detail),
    };
    match mqk_db::transition_autonomous_daily_operation(pool, &args).await? {
        AutonomousDailyTransitionOutcome::Applied(record)
        | AutonomousDailyTransitionOutcome::AlreadyApplied(record) => Ok(record),
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
        let updated = apply_transition(
            pool,
            operation,
            STATE_START_RETRYING,
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
            if operation.state_reason_code.as_deref() != Some(signature.reason_code) {
                apply_transition(
                    pool,
                    operation,
                    STATE_MANUAL_INTERVENTION_REQUIRED,
                    Some(signature.reason_code),
                    now_utc,
                    None,
                    "preopen readiness blocked by a manual-intervention condition",
                )
                .await?;
            }
            Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: signature.reason_code,
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
        apply_transition(
            pool,
            &operation,
            STATE_MANUAL_INTERVENTION_REQUIRED,
            Some("operator_managed_run_active"),
            now_utc,
            None,
            "a locally-owned execution run already exists that this operation never started; \
             the coordinator will not stop it, attach it, or attempt a new start",
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "operator_managed_run_active",
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

    let operation = if operation.state.as_str() != STATE_START_RETRYING {
        apply_transition(
            pool,
            &operation,
            STATE_START_RETRYING,
            None,
            now_utc,
            None,
            "entering canonical start sequence",
        )
        .await?
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
                apply_transition(
                    pool,
                    &operation,
                    STATE_MANUAL_INTERVENTION_REQUIRED,
                    Some("start_succeeded_without_run_id"),
                    now_utc,
                    None,
                    "start_execution_runtime returned Ok without an active_run_id",
                )
                .await?;
                return Ok(
                    AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                        reason_code: "start_succeeded_without_run_id",
                    },
                );
            };
            if state.locally_owned_run_id().await != Some(run_id) {
                // D2.12 crash-window policy: a successful start whose
                // durable operation binding cannot be confirmed must never
                // be presented as running.
                let _ = state.stop_execution_runtime().await;
                apply_transition(
                    pool,
                    &operation,
                    STATE_MANUAL_INTERVENTION_REQUIRED,
                    Some("durable_transition_unconfirmed_after_start"),
                    now_utc,
                    None,
                    "start_execution_runtime succeeded but local ownership does not match the \
                     returned run_id; refusing to present the operation as running",
                )
                .await?;
                return Ok(
                    AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                        reason_code: "durable_transition_unconfirmed_after_start",
                    },
                );
            }
            apply_transition(
                pool,
                &operation,
                STATE_RUNNING,
                None,
                now_utc,
                Some(run_id),
                "canonical start succeeded",
            )
            .await?;
            mqk_db::record_running_started(pool, operation.operation_id, now_utc).await?;
            if from_state == STATE_RECOVERY_RETRYING {
                Ok(AutonomousDailyCoordinatorTickOutcome::Recovered { run_id })
            } else {
                Ok(AutonomousDailyCoordinatorTickOutcome::Started { run_id })
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
            if operation.state_reason_code.as_deref() != Some(signature.reason_code) {
                apply_transition(
                    pool,
                    operation,
                    STATE_MANUAL_INTERVENTION_REQUIRED,
                    Some(signature.reason_code),
                    now_utc,
                    None,
                    "canonical start call returned a manual-intervention truth",
                )
                .await?;
            }
            Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: signature.reason_code,
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
        apply_transition(
            pool,
            &operation,
            mqk_db::STATE_CONTROLLER_DEGRADED,
            Some("running_operation_missing_run_id"),
            now_utc,
            None,
            "operation is running but carries no run_id",
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "running_operation_missing_run_id",
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
        apply_transition(
            pool,
            &operation,
            mqk_db::STATE_CONTROLLER_DEGRADED,
            Some("runtime_run_id_mismatch"),
            now_utc,
            None,
            "the locally-owned run does not match this operation's durable run_id",
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "runtime_run_id_mismatch",
            },
        );
    }

    // No local runtime. Determine whether the durable run row is still
    // active (a crash-window inconsistency: manual) or genuinely terminal
    // (eligible for recovery), and whether termination was unsafe (halt /
    // kill switch / durable disarm / WS gap — never retried).
    let run_row = match mqk_db::fetch_run(pool, expected_run_id).await {
        Ok(row) => row,
        Err(_err) => {
            apply_transition(
                pool,
                &operation,
                mqk_db::STATE_CONTROLLER_DEGRADED,
                Some("run_row_fetch_failed"),
                now_utc,
                None,
                "failed to fetch the durable run row while reconciling running-state ownership",
            )
            .await?;
            return Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "run_row_fetch_failed",
                },
            );
        }
    };
    let run_is_active = matches!(
        run_row.status,
        mqk_db::RunStatus::Armed | mqk_db::RunStatus::Running
    );
    if run_is_active {
        apply_transition(
            pool,
            &operation,
            mqk_db::STATE_CONTROLLER_DEGRADED,
            Some("durable_active_run_without_local_owner"),
            now_utc,
            None,
            "the durable run row is still armed/running but no local runtime owns it",
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "durable_active_run_without_local_owner",
            },
        );
    }

    // `integrity.disarmed` is deliberately not consulted here: it is the
    // fail-closed default at every daemon boot (per
    // `AppState::try_autonomous_arm_typed`'s own doc), not evidence of an
    // unsafe termination on its own -- a durable DISARMED arm state is
    // instead caught precisely by the arm gate the very next time recovery
    // attempts a canonical start (`attempt_canonical_start` ->
    // `try_autonomous_arm_typed` -> `AutonomousArmRejection::DurableDisarmed`),
    // without duplicating that classification here.
    let unsafe_termination = {
        let ig = state.integrity.read().await;
        ig.halted
    } || matches!(
        state.alpaca_ws_continuity().await,
        super::AlpacaWsContinuityState::GapDetected { .. }
    );
    if unsafe_termination {
        apply_transition(
            pool,
            &operation,
            mqk_db::STATE_CONTROLLER_DEGRADED,
            Some("runtime_ended_unsafely"),
            now_utc,
            None,
            "the local runtime ended via an operator halt/kill-switch or a WS continuity gap; \
             this is never automatically retried",
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "runtime_ended_unsafely",
            },
        );
    }

    // Terminal run, no unsafe-termination truth, session still open:
    // schedule a bounded recovery retry.
    let updated = apply_transition(
        pool,
        &operation,
        STATE_RECOVERY_RETRYING,
        Some("runtime_ended_without_halt"),
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

pub async fn handle_session_close(
    state: &Arc<AppState>,
    pool: &PgPool,
    operation: AutonomousDailyOperationRecord,
    now_utc: DateTime<Utc>,
) -> anyhow::Result<AutonomousDailyCoordinatorTickOutcome> {
    let local_run_id = state.locally_owned_run_id().await;

    match (operation.run_id, local_run_id) {
        (Some(expected), Some(actual)) if expected == actual => {
            let updated = apply_transition(
                pool,
                &operation,
                STATE_STOPPING,
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
            let target = if operation.state.as_str() == STATE_RUNNING {
                mqk_db::STATE_CONTROLLER_DEGRADED
            } else {
                STATE_MANUAL_INTERVENTION_REQUIRED
            };
            apply_transition(
                pool,
                &operation,
                target,
                Some("mismatched_runtime_at_close"),
                now_utc,
                None,
                "session closed with a locally-owned runtime that does not match this \
                 operation's run_id; the coordinator will not stop it",
            )
            .await?;
            Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "mismatched_runtime_at_close",
                },
            )
        }
        (None, Some(_)) => {
            // D2.16: an operator-managed run is preserved, never stopped,
            // never attached.
            apply_transition(
                pool,
                &operation,
                STATE_MANUAL_INTERVENTION_REQUIRED,
                Some("operator_managed_run_active"),
                now_utc,
                None,
                "session closed with a locally-owned run this operation never started; the \
                 coordinator will not stop it",
            )
            .await?;
            Ok(
                AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                    reason_code: "operator_managed_run_active",
                },
            )
        }
        (_, None) => {
            // No matching local runtime — either this operation's runtime
            // never started, or it already ended. Never call
            // stop_execution_runtime for a runtime that does not exist
            // locally; record stopped_at_utc directly.
            let updated = apply_transition(
                pool,
                &operation,
                STATE_STOPPING,
                None,
                now_utc,
                None,
                "no autonomous runtime existed at session close",
            )
            .await?;
            mqk_db::record_stopped_at(pool, updated.operation_id, now_utc).await?;
            Ok(AutonomousDailyCoordinatorTickOutcome::RuntimeStopped)
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
        apply_transition(
            pool,
            &operation,
            STATE_MANUAL_INTERVENTION_REQUIRED,
            Some("unresolved_stop_failure_at_postclose_finalize"),
            now_utc,
            None,
            "the operation's runtime stop remained unresolved at postclose_finalize_utc",
        )
        .await?;
        return Ok(
            AutonomousDailyCoordinatorTickOutcome::ManualInterventionRequired {
                reason_code: "unresolved_stop_failure_at_postclose_finalize",
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
            mqk_db::record_stopped_at(pool, operation.operation_id, now_utc).await?;
            Ok(AutonomousDailyCoordinatorTickOutcome::RuntimeStopped)
        }
        Err(err) => {
            if operation.state.as_str() != STATE_STOP_RETRYING {
                apply_transition(
                    pool,
                    &operation,
                    STATE_STOP_RETRYING,
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
