//! `POST /api/v1/autonomous/daily-operation/retry` — AUTONOMOUS-DAILY-
//! OPERATOR-RETRY-01.
//!
//! A narrow, explicit operator recovery action for a daily autonomous
//! operation stuck in `manual_intervention_required` because of a
//! PRE-START / PREFLIGHT condition (typically daily-data-readiness) that
//! has since been repaired. The August 11, 2026 incident this closes: a
//! market-data readiness failure durably blocked the day's operation; the
//! data was repaired through normal provider ingest; but the durable
//! operation stayed `manual_intervention_required` forever because no
//! ordinary coordinator tick ever re-evaluates that state — it is treated
//! as sticky durable truth and simply re-projected.
//!
//! This route reuses the *existing* legal DB transition graph
//! (`manual_intervention_required -> preparing_data`,
//! `mqk_db::is_legal_operation_transition`) rather than adding a new one. It
//! is deliberately not a blind reset: every mutation is gated by
//! independent, typed, closed-set proof that this exact operation belongs
//! to the narrow pristine-pre-start recoverable class.
//!
//! Hard invariants this route must never violate (`CLAUDE.md`,
//! `execution_rules.md`):
//! - never calls `start_execution_runtime` / any start seam
//! - never calls `try_autonomous_arm_typed` or otherwise mutates arm state
//! - never clears `integrity.halted`, the kill switch, or reconcile-dirty
//! - never submits an order
//! - only ever transitions `manual_intervention_required -> preparing_data`
//!   for a `deployment_mode == "PAPER"` operation, via the existing durable
//!   CAS transition (`mqk_db::transition_autonomous_daily_operation`)
//!
//! Re-entering at `preparing_data` is deliberate (`CLAUDE.md` durable-flow
//! discipline): the ordinary coordinator tick
//! (`autonomous_daily_coordinator::dispatch_by_state`) then re-runs its own
//! full strict readiness -> `awaiting_open` -> typed arm -> canonical start
//! sequence exactly as it would for any other operation — this route never
//! shortcuts that chain.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::api_types::{AutonomousDailyOperationRetryRequest, AutonomousDailyOperationRetryResponse};
use crate::state::autonomous_daily_operation::{
    derive_assignment_identity, derive_autonomous_daily_operation_id,
    derive_runtime_binding_identity, resolve_autonomous_daily_session_plan_from_env,
    AutonomousDailyPlanTiming, AutonomousDailySessionPlanResolution,
};
use crate::state::autonomous_runtime_context::resolve_autonomous_runtime_context;
use crate::state::AppState;

use mqk_db::{
    AutonomousDailyOperationRecord, AutonomousDailyTransitionOutcome,
    TransitionAutonomousDailyOperationArgs, STATE_MANUAL_INTERVENTION_REQUIRED,
    STATE_PREPARING_DATA,
};

const CANONICAL_ROUTE: &str = "/api/v1/autonomous/daily-operation/retry";

// ---------------------------------------------------------------------------
// §14 — closed-set manual-intervention retry eligibility classification
// ---------------------------------------------------------------------------

/// AUTONOMOUS-DAILY-OPERATOR-RETRY-01 §14: closed-set eligibility
/// classification for a `manual_intervention_required` operation's stored
/// `state_reason_code`. Exact static-string membership only — never a
/// substring or regex match against free-form error text, and never a
/// second, weaker copy of `autonomous_retry_policy::classify_autonomous_reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManualRetryEligibility {
    RecoverablePreflight,
    UnsafeRuntimeHistory,
    AdministrativeOrIdentityConflict,
    UnknownFailClosed,
}

/// Exact closed set of pre-start / daily-data-readiness reason codes this
/// route may recover. Every entry here is a named constant sourced directly
/// from `daily_data_readiness` (the same constants
/// `autonomous_daily_coordinator::classify_readiness_report` classifies from
/// `handle_preparing_data` / `handle_preflight_blocked` — the two PRE-START
/// handlers, strictly before any arm or start attempt) plus
/// `"assignment_missing"`, the one non-readiness pre-start reason those same
/// two handlers can also apply, plus one legacy pre-start reason from the
/// older `market_data_freshness` gate inside `start_execution_runtime`
/// (PAPER-SOAK-FRIDAY-LEGACY-FRESHNESS-OPERATOR-RETRY-RECOVERY-01):
/// `"runtime.start_refused.market_data_not_fresh"` — the exact fault_class
/// string stored verbatim as `state_reason_code` when that gate refuses (see
/// `autonomous_retry_policy::reason_code_str`'s `UnclassifiedFailClosed`
/// arm). Like every other entry here, it is still a PRE-START condition
/// gated strictly before any arm/start attempt, and it remains covered by
/// every one of this route's other independent safety checks (pristine
/// history, session window, identity match, and — critically — a fresh
/// re-run of canonical `daily_data_readiness` before any mutation, since
/// this legacy gate is not itself re-evaluated). Deliberately excludes
/// every reason that implies halt / kill-switch / reconcile / runtime-
/// ownership / arm history, even though a handful of those can theoretically
/// also surface from `resolve_autonomous_runtime_context` inside the same
/// two handlers — this route must never be the seam that papers over one of
/// those. Also deliberately excludes the newer sibling fault_class from the
/// same legacy gate, `"runtime.start_refused.latest_completed_bar_pending"`
/// (OPENING-BAR-FRESHNESS-AUTHORITY-REPAIR-01): that one classifies
/// `WaitForCondition`, not `ManualInterventionRequired`, so it can never
/// durably reach this route's stored `state_reason_code` in the first
/// place — including it here would be unaudited, unused breadth.
const RECOVERABLE_PREFLIGHT_REASON_CODES: &[&str] = &[
    "assignment_missing",
    "runtime.start_refused.market_data_not_fresh",
    crate::daily_data_readiness::REASON_REQUIRED_ASSIGNMENTS_MISSING,
    crate::daily_data_readiness::REASON_ASSIGNMENT_RESOLUTION_FAILED,
    crate::daily_data_readiness::REASON_RUNTIME_STRATEGY_ASSIGNMENT_MISMATCH,
    crate::daily_data_readiness::REASON_RUNTIME_STRATEGY_SYMBOL_BINDING_MISMATCH,
    crate::daily_data_readiness::REASON_RUNTIME_STRATEGY_TIMEFRAME_MISMATCH,
    crate::daily_data_readiness::REASON_STRATEGY_REQUIREMENT_UNKNOWN,
    crate::daily_data_readiness::REASON_ASSET_CLASS_UNKNOWN,
    crate::daily_data_readiness::REASON_PROVIDER_PROVENANCE_INVALID,
    crate::daily_data_readiness::REASON_PROVIDER_SYMBOL_MISMATCH,
    crate::daily_data_readiness::REASON_PROVIDER_ID_MISMATCH,
    crate::daily_data_readiness::REASON_PROVIDER_UNKNOWN,
    crate::daily_data_readiness::REASON_PROVIDER_INGEST_TIME_FUTURE,
    crate::daily_data_readiness::REASON_PROVIDER_DISABLED,
    crate::daily_data_readiness::REASON_PROVIDER_CAPABILITY_MISMATCH,
    crate::daily_data_readiness::REASON_PROVIDER_TIMESTAMP_CONVENTION_UNVERIFIED,
    crate::daily_data_readiness::REASON_CALENDAR_UNAVAILABLE,
    crate::daily_data_readiness::REASON_UNSUPPORTED_TIMEFRAME,
    crate::daily_data_readiness::REASON_UNSUPPORTED_INTRADAY_CONTINUITY,
    crate::daily_data_readiness::REASON_MARKET_DATA_MISSING,
    crate::daily_data_readiness::REASON_INSUFFICIENT_HISTORY,
    crate::daily_data_readiness::REASON_DUPLICATE_TIMESTAMP,
    crate::daily_data_readiness::REASON_INTERIOR_GAP,
    crate::daily_data_readiness::REASON_LATEST_BAR_FUTURE,
];

/// Exact closed set of reason codes that imply runtime / halt / reconcile /
/// arm history — never recoverable by this route regardless of the pristine
/// check's own outcome (defense in depth: the pristine check alone should
/// already exclude every one of these in practice).
const UNSAFE_RUNTIME_HISTORY_REASON_CODES: &[&str] = &[
    "runtime_run_id_mismatch",
    "durable_active_run_without_local_owner",
    "runtime_ended_without_halt",
    "integrity_halted",
    "kill_switch_active",
    "durable_arm_disarmed",
    "ws_gap_detected",
    "reconcile_dirty",
    "reconcile_stale",
    "dispatch_claim_unresolved",
    "observed_bar_evidence_inconsistent",
    "native_strategy_bootstrap_missing",
    "native_strategy_bootstrap_dormant",
    "native_strategy_bootstrap_failed",
    "readiness_evidence_persist_failed",
    "readiness_run_link_persist_failed",
];

pub(crate) fn classify_manual_retry_eligibility(
    reason_code: Option<&str>,
) -> ManualRetryEligibility {
    let Some(reason_code) = reason_code else {
        return ManualRetryEligibility::UnknownFailClosed;
    };
    if reason_code == "operation_identity_conflict" {
        return ManualRetryEligibility::AdministrativeOrIdentityConflict;
    }
    if UNSAFE_RUNTIME_HISTORY_REASON_CODES.contains(&reason_code) {
        return ManualRetryEligibility::UnsafeRuntimeHistory;
    }
    if RECOVERABLE_PREFLIGHT_REASON_CODES.contains(&reason_code) {
        return ManualRetryEligibility::RecoverablePreflight;
    }
    ManualRetryEligibility::UnknownFailClosed
}

// ---------------------------------------------------------------------------
// §12/§13 (revised) — retry-specific prestart-observation-safe evidence check
// ---------------------------------------------------------------------------

/// PAPER-SOAK-FRIDAY-PRESTART-OBSERVATION-RETRY-SEMANTICS-01: this route's
/// own retry-safety contract, deliberately distinct from
/// `autonomous_daily_coverage_authority::check_operation_pristine`, which
/// this function never calls and which remains completely unmodified.
/// `check_operation_pristine` gates a different question — whether the
/// coordinator may bind a *coverage anchor* — and is intentionally stricter
/// there: it must refuse to fabricate an anchor after *any* durable
/// evidence at all, including a mere observed bar, so it treats
/// `bars_observed != 0` as activity. This route's own original design intent
/// (`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`'s
/// `AUTONOMOUS-DAILY-OPERATOR-RETRY-01` entry) was narrower: refuse only on
/// genuine runtime/economic evidence — `run_id`, `started_at_utc`, bar
/// *dispatch*, or validated run lineage — not on mere data preparation. The
/// original implementation reused `check_operation_pristine` wholesale for
/// convenience, which incidentally also refused a pristine-pre-start
/// operation whose only durable evidence was a completed-bar *observation*.
///
/// A bar observation alone is provably not runtime/economic activity: the
/// completed-bar driver's `PrepareDataOnly` mode — the only mode legal in
/// any state this route can ever see (`awaiting_preopen`/`preparing_data`/
/// `awaiting_open`/`preflight_blocked`/`start_retrying`; never `running`,
/// see `state::autonomous_completed_bar_task::select_driver_mode_for_state`)
/// — records the observation and then stops; by construction (see its own
/// match arm in `state::autonomous_completed_bar_driver::
/// observe_and_dispatch_if_ready`) it never creates a dispatch claim,
/// deposits a pending strategy bar, or invokes native strategy code. Only
/// `RunningDispatch` mode (gated behind `operation.state == running` and a
/// bound `run_id`) can do any of those, and this route already refuses
/// unconditionally on `run_id.is_some()` below.
///
/// This function therefore reuses the exact same two DB-backed activity
/// queries `check_operation_pristine` uses (`count_autonomous_daily_bar_
/// dispatch_claims`, `fetch_and_validate_autonomous_daily_operation_run_
/// lineage` — never a second, independently-derived query) plus the same
/// `run_id`/`started_at_utc`/`bars_dispatched`/`last_dispatched_bar_ts`
/// field checks, but never inspects `bars_observed` or
/// `last_completed_bar_ts`. No boolean flag on the coverage-anchor
/// function — a separate type for a separate contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrestartRetrySafety {
    /// No genuine runtime/economic activity evidence found — safe for this
    /// route to re-enter the coordinator pipeline at `preparing_data`.
    Safe,
    /// At least one runtime/economic activity signal is present — this
    /// route must never mutate.
    UnsafeActivity,
}

pub(crate) async fn check_prestart_retry_safety(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
) -> anyhow::Result<PrestartRetrySafety> {
    if operation.run_id.is_some()
        || operation.started_at_utc.is_some()
        || operation.bars_dispatched != 0
        || operation.last_dispatched_bar_ts.is_some()
    {
        return Ok(PrestartRetrySafety::UnsafeActivity);
    }

    let claim_count =
        mqk_db::count_autonomous_daily_bar_dispatch_claims(pool, operation.operation_id).await?;
    if claim_count > 0 {
        return Ok(PrestartRetrySafety::UnsafeActivity);
    }

    let lineage =
        mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage(pool, operation).await?;
    match lineage {
        Ok(lineage) if lineage.is_empty() => Ok(PrestartRetrySafety::Safe),
        Ok(_) => Ok(PrestartRetrySafety::UnsafeActivity),
        // A contradictory lineage is itself activity evidence this check
        // cannot safely wave through as safe.
        Err(_) => Ok(PrestartRetrySafety::UnsafeActivity),
    }
}

// ---------------------------------------------------------------------------
// Response construction — every branch keeps the fixed safety fields
// (`runtime_started` / `arm_modified` / `halt_changed` / `reconcile_changed`
// / `orders_submitted`) at their permanently-false/zero value: this route
// never touches any of them on any path.
// ---------------------------------------------------------------------------

fn base_response(operation_id: Uuid, truth_state: &str) -> AutonomousDailyOperationRetryResponse {
    AutonomousDailyOperationRetryResponse {
        canonical_route: CANONICAL_ROUTE.to_string(),
        truth_state: truth_state.to_string(),
        operation_id: operation_id.to_string(),
        previous_state: None,
        new_state: None,
        previous_reason_code: None,
        runtime_started: false,
        arm_modified: false,
        halt_changed: false,
        reconcile_changed: false,
        orders_submitted: 0,
        message: None,
    }
}

fn refusal_response(
    operation: &AutonomousDailyOperationRecord,
    truth_state: &str,
    message: impl Into<String>,
) -> AutonomousDailyOperationRetryResponse {
    let mut resp = base_response(operation.operation_id, truth_state);
    resp.previous_state = Some(operation.state.clone());
    resp.previous_reason_code = operation.state_reason_code.clone();
    resp.message = Some(message.into());
    resp
}

// ---------------------------------------------------------------------------
// POST /api/v1/autonomous/daily-operation/retry
// ---------------------------------------------------------------------------

pub(crate) async fn autonomous_daily_operation_retry(
    State(st): State<Arc<AppState>>,
    Json(body): Json<AutonomousDailyOperationRetryRequest>,
) -> Response {
    // Uses the same test-overridable clock the strict daily-data-readiness
    // start gate uses (`AppState::daily_data_readiness_now`) rather than a
    // raw `Utc::now()` — this route re-runs that exact same readiness
    // evaluation, and sharing its clock seam keeps both hermetically
    // testable against one fixed instant.
    let now_utc = st.daily_data_readiness_now().await;

    let Some(pool) = st.db.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(base_response(body.operation_id, "backend_unavailable")),
        )
            .into_response();
    };

    let operation = match mqk_db::fetch_autonomous_daily_operation_by_id(&pool, body.operation_id)
        .await
    {
        Ok(Some(op)) => op,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(base_response(body.operation_id, "not_found")),
            )
                .into_response();
        }
        Err(err) => {
            tracing::error!("autonomous_daily_operation_retry: fetch failed: {err}");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(base_response(body.operation_id, "backend_unavailable")),
            )
                .into_response();
        }
    };

    // §9: identity safety — an operator-supplied expected_market_date, when
    // present, must match the target row exactly.
    if let Some(expected_market_date) = &body.expected_market_date {
        if operation.market_date.format("%Y-%m-%d").to_string() != *expected_market_date {
            return (
                StatusCode::CONFLICT,
                Json(refusal_response(
                    &operation,
                    "not_recoverable",
                    "expected_market_date does not match the target operation's market_date",
                )),
            )
                .into_response();
        }
    }

    // §24: paper only.
    if !operation.deployment_mode.eq_ignore_ascii_case("PAPER") {
        return (
            StatusCode::FORBIDDEN,
            Json(refusal_response(
                &operation,
                "not_authorized",
                "operator retry is refused for a non-paper deployment_mode",
            )),
        )
            .into_response();
    }

    // §27: idempotency. `preparing_data` is deliberately NOT special-cased
    // as "already_recovered" here — this row's *current* state alone cannot
    // prove it got there via a prior call to this route rather than
    // ordinary, unrelated lifecycle progress (a fabricated distinction
    // CLAUDE.md's no-fabricated-truth rule forbids). Any non-manual state
    // uniformly refuses as `not_manual`, without mutation — an acceptable,
    // explicitly sanctioned idempotent-replay response. The one truthful
    // "already_recovered" signal this route ever reports is the DB's own
    // `AlreadyApplied` CAS outcome below, which proves this exact transition
    // was already durably applied.
    if operation.state != STATE_MANUAL_INTERVENTION_REQUIRED {
        return (
            StatusCode::CONFLICT,
            Json(refusal_response(
                &operation,
                "not_manual",
                "operation is not currently in manual_intervention_required",
            )),
        )
            .into_response();
    }

    // §12/§13 (revised, PAPER-SOAK-FRIDAY-PRESTART-OBSERVATION-RETRY-
    // SEMANTICS-01): retry-specific safety only — no run_id, no started_at,
    // no bars dispatched, no dispatch claim, no validated run lineage. A
    // mere prestart bar *observation* (`bars_observed`/
    // `last_completed_bar_ts`) is deliberately not checked here — see
    // `check_prestart_retry_safety`'s doc comment for why that is safe and
    // why this is not the same contract as coverage-anchor pristine.
    match check_prestart_retry_safety(&pool, &operation).await {
        Ok(PrestartRetrySafety::Safe) => {}
        Ok(PrestartRetrySafety::UnsafeActivity) => {
            return (
                StatusCode::CONFLICT,
                Json(refusal_response(
                    &operation,
                    "not_recoverable",
                    "operation has runtime activity attached; not eligible for this route",
                )),
            )
                .into_response();
        }
        Err(err) => {
            tracing::error!("autonomous_daily_operation_retry: prestart retry safety check failed: {err}");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(base_response(operation.operation_id, "backend_unavailable")),
            )
                .into_response();
        }
    }

    // §22: session window safety.
    if now_utc >= operation.effective_operation_close_utc {
        return (
            StatusCode::CONFLICT,
            Json(refusal_response(
                &operation,
                "session_closed",
                "operation's authorized session window has already closed",
            )),
        )
            .into_response();
    }

    // §14: closed-set blocker-origin eligibility.
    match classify_manual_retry_eligibility(operation.state_reason_code.as_deref()) {
        ManualRetryEligibility::RecoverablePreflight => {}
        ManualRetryEligibility::UnsafeRuntimeHistory => {
            return (
                StatusCode::CONFLICT,
                Json(refusal_response(
                    &operation,
                    "not_recoverable",
                    "blocker reason implies runtime/halt/reconcile/arm history; refused",
                )),
            )
                .into_response();
        }
        ManualRetryEligibility::AdministrativeOrIdentityConflict => {
            return (
                StatusCode::CONFLICT,
                Json(refusal_response(
                    &operation,
                    "not_recoverable",
                    "blocker reason is an administrative/identity conflict; refused",
                )),
            )
                .into_response();
        }
        ManualRetryEligibility::UnknownFailClosed => {
            return (
                StatusCode::CONFLICT,
                Json(refusal_response(
                    &operation,
                    "not_recoverable",
                    "blocker reason is not in the closed recoverable set; fails closed",
                )),
            )
                .into_response();
        }
    }

    // §23: current canonical identity must still match this exact operation.
    let config = match crate::state::build_multi_symbol_runtime_config_from_env() {
        Ok(config) => config,
        Err(_) => {
            return (
                StatusCode::CONFLICT,
                Json(refusal_response(
                    &operation,
                    "still_blocked",
                    "current assignment configuration could not be resolved",
                )),
            )
                .into_response();
        }
    };
    let runtime_context = match resolve_autonomous_runtime_context(&st).await {
        Ok(ctx) => ctx,
        Err(_) => {
            return (
                StatusCode::CONFLICT,
                Json(refusal_response(
                    &operation,
                    "still_blocked",
                    "current runtime binding could not be resolved",
                )),
            )
                .into_response();
        }
    };
    let timing = AutonomousDailyPlanTiming::production_default();
    let plan = match resolve_autonomous_daily_session_plan_from_env(now_utc, &timing) {
        AutonomousDailySessionPlanResolution::Applicable(plan) => plan,
        _ => {
            return (
                StatusCode::CONFLICT,
                Json(refusal_response(
                    &operation,
                    "still_blocked",
                    "current calendar/session-plan resolution is not applicable",
                )),
            )
                .into_response();
        }
    };
    let deployment_mode = st.deployment_mode().as_db_mode();
    let adapter_id = st.adapter_id().to_string();
    let assignment_identity = derive_assignment_identity(&config);
    let runtime_binding_identity =
        derive_runtime_binding_identity(&runtime_context.effective_runtime_binding);
    let current_operation_id = derive_autonomous_daily_operation_id(
        &plan,
        deployment_mode,
        &adapter_id,
        &assignment_identity,
        &runtime_binding_identity,
    );
    if current_operation_id != operation.operation_id {
        return (
            StatusCode::CONFLICT,
            Json(refusal_response(
                &operation,
                "not_recoverable",
                "operation identity no longer matches current canonical configuration",
            )),
        )
            .into_response();
    }

    // §15/§17: re-run the canonical read-only readiness evaluation — never a
    // watered-down duplicate.
    let readiness_context = crate::daily_data_readiness::load_readiness_context_from_env();
    let report = crate::daily_data_readiness::evaluate_readiness_with_binding(
        Some(&pool),
        &config,
        &runtime_context.effective_runtime_binding,
        &readiness_context,
        now_utc,
    )
    .await;
    if !report.start_allowed {
        return (
            StatusCode::CONFLICT,
            Json(refusal_response(
                &operation,
                "still_blocked",
                "canonical daily-data-readiness evaluation is still blocked",
            )),
        )
            .into_response();
    }

    // §18/§19/§20/§21: canonical durable CAS transition,
    // manual_intervention_required -> preparing_data only. Never
    // `apply_transition` here — that helper bails via `anyhow` on a stale
    // CAS, which this route must instead map to a bounded `409 conflict`.
    let previous_reason_code = operation.state_reason_code.clone();
    let args = TransitionAutonomousDailyOperationArgs {
        operation_id: operation.operation_id,
        expected_state: operation.state.clone(),
        expected_state_version: operation.state_version,
        new_state: STATE_PREPARING_DATA.to_string(),
        reason_code: None,
        blocker_signature: None,
        occurred_at_utc: now_utc,
        run_id: None,
        bounded_detail: "operator_retry_after_preflight_repair: operator-initiated recovery \
            from manual_intervention_required after preflight/readiness repair; canonical \
            coordinator pipeline re-entered at preparing_data"
            .to_string(),
    };
    match mqk_db::transition_autonomous_daily_operation(&pool, &args).await {
        Ok(AutonomousDailyTransitionOutcome::Applied(updated)) => {
            // Best-effort: clears any stale `next_retry_utc`/`last_error` so
            // a later coordinator tick's `attempt_canonical_start` retry-
            // timing gate cannot reject on stale state. Never touches
            // `state`/`state_version`, so its own failure cannot desync the
            // CAS transition that already committed above.
            if let Err(err) =
                mqk_db::clear_retry_timing(&pool, operation.operation_id, now_utc).await
            {
                tracing::warn!(
                    "autonomous_daily_operation_retry: clear_retry_timing failed after \
                     successful transition (non-fatal): {err}"
                );
            }
            let mut resp = base_response(updated.operation_id, "recovered");
            resp.previous_state = Some(STATE_MANUAL_INTERVENTION_REQUIRED.to_string());
            resp.new_state = Some(updated.state.clone());
            resp.previous_reason_code = previous_reason_code;
            resp.message = Some(
                "operation re-entered the canonical coordinator pipeline at preparing_data"
                    .to_string(),
            );
            (StatusCode::OK, Json(resp)).into_response()
        }
        Ok(AutonomousDailyTransitionOutcome::AlreadyApplied(updated)) => (
            StatusCode::OK,
            Json(refusal_response(&updated, "already_recovered", "idempotent replay")),
        )
            .into_response(),
        Ok(AutonomousDailyTransitionOutcome::StaleState { .. }) => (
            StatusCode::CONFLICT,
            Json(base_response(operation.operation_id, "conflict")),
        )
            .into_response(),
        Ok(AutonomousDailyTransitionOutcome::NotFound) => (
            StatusCode::NOT_FOUND,
            Json(base_response(operation.operation_id, "not_found")),
        )
            .into_response(),
        Ok(AutonomousDailyTransitionOutcome::IllegalTransition) => (
            StatusCode::CONFLICT,
            Json(refusal_response(
                &operation,
                "not_recoverable",
                "manual_intervention_required -> preparing_data is not a legal transition \
                 from this operation's current durable state",
            )),
        )
            .into_response(),
        Err(err) => {
            tracing::error!("autonomous_daily_operation_retry: transition failed: {err}");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(base_response(operation.operation_id, "backend_unavailable")),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod manual_retry_eligibility_tests {
    use super::*;

    /// PAPER-SOAK-FRIDAY-LEGACY-FRESHNESS-OPERATOR-RETRY-RECOVERY-01: the
    /// 2026-08-14 incident this closes. The legacy `market_data_freshness`
    /// gate's own fault_class string is stored verbatim as
    /// `state_reason_code` (via `AutonomousCoordinatorReason::
    /// UnclassifiedFailClosed { fault_class }`, see
    /// `autonomous_retry_policy::reason_code_str`), so this route must
    /// recognize the exact dotted string, not a bare `"market_data_not_fresh"`.
    #[test]
    fn legacy_market_data_not_fresh_is_recoverable_preflight() {
        assert_eq!(
            classify_manual_retry_eligibility(Some("runtime.start_refused.market_data_not_fresh")),
            ManualRetryEligibility::RecoverablePreflight
        );
    }

    /// OPENING-BAR-FRESHNESS-AUTHORITY-REPAIR-01 added a second fault_class
    /// from the same `market_data_freshness` source
    /// (`runtime.start_refused.latest_completed_bar_pending`), but that one
    /// classifies `WaitForCondition` in `autonomous_retry_policy` and can
    /// never durably land in `manual_intervention_required` — it must stay
    /// out of this route's closed recoverable set. Widening for it would be
    /// dead-weight at best and an unaudited seam at worst.
    #[test]
    fn latest_completed_bar_pending_is_not_widened_into_recoverable() {
        assert_eq!(
            classify_manual_retry_eligibility(Some(
                "runtime.start_refused.latest_completed_bar_pending"
            )),
            ManualRetryEligibility::UnknownFailClosed
        );
    }

    /// Negative test #1: an unrelated/unaudited `runtime.start_refused.*`
    /// fault_class must still fail closed — no prefix or substring match.
    #[test]
    fn unrelated_runtime_start_refused_reason_fails_closed() {
        assert_eq!(
            classify_manual_retry_eligibility(Some(
                "runtime.start_refused.some_future_gate_not_yet_audited"
            )),
            ManualRetryEligibility::UnknownFailClosed
        );
    }

    /// Negative tests #2-#5: reasons implying runtime/halt/reconcile/arm
    /// history stay refused regardless of this patch.
    #[test]
    fn unsafe_runtime_history_reasons_stay_refused() {
        for reason in [
            "reconcile_dirty",
            "integrity_halted",
            "kill_switch_active",
            "runtime_run_id_mismatch",
        ] {
            assert_eq!(
                classify_manual_retry_eligibility(Some(reason)),
                ManualRetryEligibility::UnsafeRuntimeHistory,
                "{reason} must remain UnsafeRuntimeHistory"
            );
        }
    }

    /// No widening of the eligibility surface itself: the recoverable set
    /// must contain the new legacy code exactly once and nothing else new.
    #[test]
    fn recoverable_set_contains_the_new_legacy_code_exactly_once() {
        let occurrences = RECOVERABLE_PREFLIGHT_REASON_CODES
            .iter()
            .filter(|&&r| r == "runtime.start_refused.market_data_not_fresh")
            .count();
        assert_eq!(occurrences, 1);
    }
}

/// PAPER-SOAK-FRIDAY-PRESTART-OBSERVATION-RETRY-SEMANTICS-01: DB-backed
/// proof for `check_prestart_retry_safety`, and for the invariant that
/// `check_operation_pristine` remains completely unmodified. Every test
/// requires `MQK_DATABASE_URL` (the port-5434 local test DB only) and skips
/// (not fails) without it, matching this crate's other inline DB-backed
/// test modules (e.g. `state::lifecycle::real_production_effects_matrix_
/// tests`). Run with:
///   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
///   cargo test -p mqk-daemon --lib routes::autonomous_daily_operator::prestart_retry_safety_tests \
///   -- --test-threads=1 --nocapture
#[cfg(test)]
mod prestart_retry_safety_tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
    use mqk_db::{
        CreateAutonomousDailyOperationArgs, CreateOrRecoverAutonomousDailyOperationOutcome,
        RecordCompletedBarObservedOutcome, TransitionAutonomousDailyOperationToRunningArgs,
        STATE_AWAITING_OPEN, STATE_START_RETRYING,
    };

    async fn db_pool_or_skip() -> Option<PgPool> {
        let Ok(url) = std::env::var("MQK_DATABASE_URL") else {
            return None;
        };
        if !url.contains(":5434") {
            panic!(
                "prestart_retry_safety_tests: MQK_DATABASE_URL must be the port-5434 local \
                 test DB, refusing to run against: {url}"
            );
        }
        Some(mqk_db::testkit_db_pool().await.expect("test db pool"))
    }

    fn unique_suffix() -> String {
        Uuid::new_v4().to_string().replace('-', "")[..10].to_string()
    }

    fn test_operation_id(seed: &str) -> Uuid {
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, seed.as_bytes())
    }

    async fn create_test_operation(
        pool: &PgPool,
        adapter_id: &str,
        occurred_at_utc: chrono::DateTime<Utc>,
    ) -> AutonomousDailyOperationRecord {
        let market_date = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
        let open = Utc.with_ymd_and_hms(2026, 8, 14, 13, 30, 0).unwrap();
        let close = open + ChronoDuration::hours(6) + ChronoDuration::minutes(30);
        let preopen = open - ChronoDuration::minutes(30);
        let postclose = close + ChronoDuration::minutes(15);
        let previous_trading_date = market_date - ChronoDuration::days(3);
        let operation_id = test_operation_id(&format!("prestart-retry-safety|{adapter_id}"));
        let args = CreateAutonomousDailyOperationArgs {
            operation_id,
            market_date,
            deployment_mode: "PAPER".to_string(),
            adapter_id: adapter_id.to_string(),
            session_plan_identity: format!("prestart-retry-safety-plan|{adapter_id}"),
            assignment_identity: "prestart-retry-safety-assignment".to_string(),
            runtime_binding_identity: "prestart-retry-safety-binding".to_string(),
            calendar_source: "nyse_weekdays_heuristic".to_string(),
            calendar_coverage_state: "active".to_string(),
            schedule_source: "nyse_weekdays_heuristic".to_string(),
            effective_operation_open_utc: open,
            effective_operation_close_utc: close,
            exchange_session_open_utc: open,
            exchange_session_close_utc: close,
            exchange_is_early_close: false,
            previous_trading_date,
            preopen_start_utc: preopen,
            postclose_finalize_utc: postclose,
            initial_state: STATE_PREPARING_DATA.to_string(),
            data_refresh_state: "not_started".to_string(),
            occurred_at_utc,
            bounded_detail: "prestart retry safety test fixture".to_string(),
            stop_attempt_count: 0,
        };
        match mqk_db::create_or_recover_autonomous_daily_operation(pool, &args)
            .await
            .expect("create operation")
        {
            CreateOrRecoverAutonomousDailyOperationOutcome::Created(record) => record,
            CreateOrRecoverAutonomousDailyOperationOutcome::Recovered(record) => record,
            CreateOrRecoverAutonomousDailyOperationOutcome::IdentityConflict { .. } => {
                panic!("unexpected identity conflict in prestart retry safety test fixture")
            }
        }
    }

    async fn cleanup(pool: &PgPool, operation_id: Uuid) {
        let _ = sqlx::query(
            "delete from sys_autonomous_daily_operation_events where operation_id = $1",
        )
        .bind(operation_id)
        .execute(pool)
        .await;
        let _ = sqlx::query("delete from sys_autonomous_daily_operations where operation_id = $1")
            .bind(operation_id)
            .execute(pool)
            .await;
    }

    /// Positive: a genuinely pristine-pre-start operation whose only
    /// durable evidence is one completed-bar *observation* — the exact
    /// production shape the completed-bar driver's `PrepareDataOnly` mode
    /// produces — is `Safe`.
    #[tokio::test]
    async fn bars_observed_only_is_safe() {
        let Some(pool) = db_pool_or_skip().await else {
            return;
        };
        let adapter_id = format!("safety-observed-{}", unique_suffix());
        let t0 = Utc.with_ymd_and_hms(2026, 8, 14, 13, 0, 0).unwrap();
        let operation = create_test_operation(&pool, &adapter_id, t0).await;

        let observed =
            mqk_db::record_completed_bar_observed(&pool, operation.operation_id, 1_800_000_000, t0)
                .await
                .expect("record observed ok");
        assert!(matches!(
            observed,
            RecordCompletedBarObservedOutcome::Recorded { bars_observed: 1 }
        ));

        let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refreshed.bars_observed, 1);

        assert_eq!(
            check_prestart_retry_safety(&pool, &refreshed).await.unwrap(),
            PrestartRetrySafety::Safe
        );

        cleanup(&pool, operation.operation_id).await;
    }

    /// Invariant preservation: `check_operation_pristine` itself is
    /// completely untouched by this patch and still reports `HasActivity`
    /// for the exact same bars-observed-only fixture the previous test
    /// proves is `Safe` for retry purposes — the coverage-anchor contract
    /// must remain exactly as strict as before this patch.
    #[tokio::test]
    async fn coverage_pristine_check_is_unaffected_and_still_reports_has_activity() {
        let Some(pool) = db_pool_or_skip().await else {
            return;
        };
        use crate::state::autonomous_daily_coverage_authority::{
            check_operation_pristine, PristineCheckOutcome,
        };

        let adapter_id = format!("safety-pristine-{}", unique_suffix());
        let t0 = Utc.with_ymd_and_hms(2026, 8, 14, 13, 0, 0).unwrap();
        let operation = create_test_operation(&pool, &adapter_id, t0).await;
        mqk_db::record_completed_bar_observed(&pool, operation.operation_id, 1_800_000_000, t0)
            .await
            .expect("record observed ok");
        let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            check_operation_pristine(&pool, &refreshed).await.unwrap(),
            PristineCheckOutcome::HasActivity,
            "check_operation_pristine must remain unchanged: bars_observed != 0 is still \
             activity for the coverage-anchor contract"
        );

        cleanup(&pool, operation.operation_id).await;
    }

    /// Negative: a dispatch claim (even uncompleted) is genuine
    /// runtime-adjacent evidence and must remain unsafe.
    #[tokio::test]
    async fn dispatch_claim_present_is_unsafe() {
        let Some(pool) = db_pool_or_skip().await else {
            return;
        };
        let adapter_id = format!("safety-claim-{}", unique_suffix());
        let t0 = Utc.with_ymd_and_hms(2026, 8, 14, 13, 0, 0).unwrap();
        let operation = create_test_operation(&pool, &adapter_id, t0).await;
        let bar_end_ts = 1_800_000_300_i64;
        mqk_db::record_completed_bar_observed(&pool, operation.operation_id, bar_end_ts, t0)
            .await
            .expect("record observed ok");
        mqk_db::claim_autonomous_daily_bar_dispatch(
            &pool,
            operation.operation_id,
            "ZZPRESTART",
            "5m",
            bar_end_ts,
            t0,
        )
        .await
        .expect("claim ok");

        let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            check_prestart_retry_safety(&pool, &refreshed).await.unwrap(),
            PrestartRetrySafety::UnsafeActivity
        );

        cleanup(&pool, operation.operation_id).await;
    }

    /// Negative: a completed dispatch (`bars_dispatched > 0`,
    /// `last_dispatched_bar_ts` set) is genuine economic-path evidence and
    /// must remain unsafe.
    #[tokio::test]
    async fn bars_dispatched_present_is_unsafe() {
        let Some(pool) = db_pool_or_skip().await else {
            return;
        };
        let adapter_id = format!("safety-dispatched-{}", unique_suffix());
        let t0 = Utc.with_ymd_and_hms(2026, 8, 14, 13, 0, 0).unwrap();
        let operation = create_test_operation(&pool, &adapter_id, t0).await;
        let bar_end_ts = 1_800_000_600_i64;
        mqk_db::record_completed_bar_observed(&pool, operation.operation_id, bar_end_ts, t0)
            .await
            .expect("record observed ok");
        mqk_db::claim_autonomous_daily_bar_dispatch(
            &pool,
            operation.operation_id,
            "ZZPRESTART",
            "5m",
            bar_end_ts,
            t0,
        )
        .await
        .expect("claim ok");
        mqk_db::complete_autonomous_daily_bar_dispatch(
            &pool,
            operation.operation_id,
            "ZZPRESTART",
            "5m",
            bar_end_ts,
            t0 + ChronoDuration::seconds(1),
            None,
        )
        .await
        .expect("complete ok");

        let refreshed = mqk_db::fetch_autonomous_daily_operation_by_id(&pool, operation.operation_id)
            .await
            .unwrap()
            .unwrap();
        assert!(refreshed.bars_dispatched > 0);
        assert!(refreshed.last_dispatched_bar_ts.is_some());
        assert_eq!(
            check_prestart_retry_safety(&pool, &refreshed).await.unwrap(),
            PrestartRetrySafety::UnsafeActivity
        );

        cleanup(&pool, operation.operation_id).await;
    }

    /// Negative: `run_id`/`started_at_utc` present — a genuine canonical
    /// start, driven through the real legal transition chain, never faked
    /// via raw SQL — must remain unsafe.
    #[tokio::test]
    async fn run_id_and_started_at_present_is_unsafe() {
        let Some(pool) = db_pool_or_skip().await else {
            return;
        };
        let adapter_id = format!("safety-running-{}", unique_suffix());
        let t0 = Utc.with_ymd_and_hms(2026, 8, 14, 13, 0, 0).unwrap();
        let operation = create_test_operation(&pool, &adapter_id, t0).await;

        let v2 = match mqk_db::transition_autonomous_daily_operation(
            &pool,
            &TransitionAutonomousDailyOperationArgs {
                operation_id: operation.operation_id,
                expected_state: STATE_PREPARING_DATA.to_string(),
                expected_state_version: operation.state_version,
                new_state: STATE_AWAITING_OPEN.to_string(),
                reason_code: None,
                blocker_signature: None,
                occurred_at_utc: t0,
                run_id: None,
                bounded_detail: "test: preopen readiness satisfied".to_string(),
            },
        )
        .await
        .unwrap()
        {
            AutonomousDailyTransitionOutcome::Applied(record) => record,
            other => panic!("expected Applied, got {other:?}"),
        };
        let v3 = match mqk_db::transition_autonomous_daily_operation(
            &pool,
            &TransitionAutonomousDailyOperationArgs {
                operation_id: operation.operation_id,
                expected_state: STATE_AWAITING_OPEN.to_string(),
                expected_state_version: v2.state_version,
                new_state: STATE_START_RETRYING.to_string(),
                reason_code: None,
                blocker_signature: None,
                occurred_at_utc: t0,
                run_id: None,
                bounded_detail: "test: entering start sequence".to_string(),
            },
        )
        .await
        .unwrap()
        {
            AutonomousDailyTransitionOutcome::Applied(record) => record,
            other => panic!("expected Applied, got {other:?}"),
        };

        let run_id = Uuid::new_v4();
        let v4 = match mqk_db::transition_autonomous_daily_operation_to_running(
            &pool,
            &TransitionAutonomousDailyOperationToRunningArgs {
                operation_id: operation.operation_id,
                expected_state: STATE_START_RETRYING.to_string(),
                expected_state_version: v3.state_version,
                run_id,
                started_at_utc: t0,
                occurred_at_utc: t0,
                bounded_detail: "test: canonical start succeeded".to_string(),
            },
        )
        .await
        .unwrap()
        {
            AutonomousDailyTransitionOutcome::Applied(record) => record,
            other => panic!("expected Applied, got {other:?}"),
        };
        assert_eq!(v4.run_id, Some(run_id));
        assert!(v4.started_at_utc.is_some());

        assert_eq!(
            check_prestart_retry_safety(&pool, &v4).await.unwrap(),
            PrestartRetrySafety::UnsafeActivity
        );

        cleanup(&pool, operation.operation_id).await;
    }
}
