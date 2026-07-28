// core-rs/crates/mqk-daemon/src/routes/strategy_conflict.rs
//
// MULTI-STRATEGY-CONFLICT-POLICY-01 Phase D: read-only conflict-policy truth.
//
// GET /api/v1/strategy/conflict/status
// GET /api/v1/strategy/conflict/plans?limit=&run_id=
// GET /api/v1/strategy/conflict/plans/:plan_id
//
// GET-only: no route in this file ever inserts, updates, or deletes a row.
// `approved_for_live` is always `false`. `null` always means unavailable —
// never a fabricated zero/default. Run resolution mirrors
// `routes/durable_portfolio.rs` / `routes/portfolio_allocation.rs` exactly:
// an explicit `?run_id=` query param, or else the latest durable PAPER run
// for this engine.
//
// AUTHORITY-AND-EVIDENCE-REPAIR-01 Defect 6: every route in this file now
// runs the shared `conflict_evidence_validation` validator before
// projecting a persisted row as `active` truth. A malformed plan is
// surfaced as `invalid_evidence` with bounded blockers, never silently
// treated as active current truth, and status never falls back from a
// malformed latest plan to an older one.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::durable_portfolio::{resolve_run, RunResolution};
use crate::conflict_evidence_validation::{
    evidence_validation_for_candidate_limit_exceeded, validate_plan_with_candidates,
    MAX_CANDIDATES_PER_PLAN,
};
use crate::runtime_strategy_conflict_mode::{
    effective_mode, resolve_conflict_policy_mode_from_env,
};
use crate::state::{AppState, BrokerKind};
use mqk_db::BoundedConflictPlanFetch;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Closed vocabulary. `db_unavailable` / `query_failed` / `not_found` mirror
/// the durable-portfolio/allocation routes; `active` means the mode/plan
/// lookup completed AND (when a plan was inspected) that plan passed the
/// shared evidence validator; `invalid_configuration` means the env var is
/// set to an unrecognized value (mode still runs as `off`, but the operator
/// must be told why); `invalid_evidence` means a persisted plan/candidate
/// row failed the shared read-side validator and was refused, never
/// projected as active truth.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConflictTruthState {
    Active,
    InvalidConfiguration,
    InvalidEvidence,
    DbUnavailable,
    QueryFailed,
    NotFound,
}

#[derive(Debug, Serialize)]
pub(crate) struct ConflictStatusResponse {
    pub truth_state: ConflictTruthState,
    pub mode_configured: String,
    pub mode_effective: String,
    pub invalid_configuration: Option<String>,
    pub live_lock_applied: bool,
    /// Always `false` — hard invariant, never overridable.
    pub approved_for_live: bool,
    /// Truthful statement of the current runtime's known limitation: the
    /// native strategy runtime dispatches at most one strategy host across
    /// every configured symbol today, so this policy is normally a no-op —
    /// see docs/specs/multi_strategy_conflict_policy_01a.
    pub current_runtime_limitation: String,
    pub run_id: Option<String>,
    pub latest_plan_id: Option<String>,
    pub latest_plan_created_at_utc: Option<String>,
    pub latest_plan_symbol_group_count: Option<i32>,
    pub latest_plan_candidate_count: Option<i32>,
    pub latest_plan_selected_count: Option<i32>,
    /// Non-empty only when `truth_state == invalid_evidence`: why the
    /// latest plan for this run was refused rather than projected active.
    pub evidence_blockers: Vec<String>,
    pub checked_at_utc: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ConflictPlanCandidateRow {
    pub symbol: String,
    pub strategy_id: String,
    pub timeframe_secs: i64,
    pub side: String,
    pub qty: i64,
    pub current_qty: i64,
    pub proposed_target_qty: Option<i64>,
    /// `None` on a pre-0057 legacy row.
    pub order_type: Option<String>,
    /// `None` on a pre-0057 legacy row.
    pub time_in_force: Option<String>,
    pub limit_price: Option<i64>,
    /// `None` on a pre-0057 legacy row -- distinct from `Some(false)`.
    pub bar_present: Option<bool>,
    pub bar_symbol: Option<String>,
    pub bar_strategy_id: Option<String>,
    pub bar_timeframe: Option<String>,
    pub bar_end_ts: Option<i64>,
    pub close_micros: Option<i64>,
    pub selected: bool,
    pub disposition: String,
    pub reason_code: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ConflictPlanRow {
    pub plan_id: String,
    pub cycle_id: String,
    pub run_id: String,
    pub mode: String,
    /// `None` on a pre-0057 legacy row, or when this row was excluded from
    /// a list because it failed the shared shape validator.
    pub configured_mode: Option<String>,
    pub market_date: String,
    pub policy_schema_version: String,
    pub symbol_group_count: i32,
    pub candidate_count: i32,
    pub selected_count: i32,
    pub refused_count: i32,
    pub truth_state: String,
    pub blockers: Vec<String>,
    pub created_at_utc: String,
    /// Always `false` — hard invariant, never overridable.
    pub approved_for_live: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct ConflictPlanDetailResponse {
    pub truth_state: ConflictTruthState,
    pub plan: Option<ConflictPlanRow>,
    pub candidates: Vec<ConflictPlanCandidateRow>,
    /// Non-empty only when `truth_state == invalid_evidence`.
    pub evidence_blockers: Vec<String>,
    pub checked_at_utc: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ConflictPlansListResponse {
    pub truth_state: ConflictTruthState,
    pub run_id: Option<String>,
    /// Only rows that passed the shared shape validator — a malformed row
    /// is never silently mixed into this list.
    pub plans: Vec<ConflictPlanRow>,
    /// How many rows were successfully read and failed the shared evidence
    /// validator. `0` means every successfully-read row was structurally
    /// sound. Defect 5: reserved *only* for rows that were actually read —
    /// a query failure or a vanished row is never counted here.
    pub excluded_malformed_count: i64,
    /// How many summary rows vanished (were deleted) between the summary
    /// read and the per-row detail read — distinguishable from
    /// `excluded_malformed_count` because a race is not evidence
    /// corruption.
    pub excluded_vanished_count: i64,
    pub checked_at_utc: String,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: &'static str,
    detail: String,
}

const CURRENT_RUNTIME_LIMITATION: &str = "the native strategy runtime dispatches at most one \
strategy host across every configured symbol today; this policy is normally a no-op until \
Bundle 7 (dynamic strategy-symbol selection, not started) introduces real multi-strategy \
competition";

fn plan_row_from_record(rec: &mqk_db::RuntimeStrategyConflictPlanRecord) -> ConflictPlanRow {
    ConflictPlanRow {
        plan_id: rec.plan_id.to_string(),
        cycle_id: rec.cycle_id.to_string(),
        run_id: rec.run_id.to_string(),
        mode: rec.mode.clone(),
        configured_mode: rec.configured_mode.clone(),
        market_date: rec.market_date.clone(),
        policy_schema_version: rec.policy_schema_version.clone(),
        symbol_group_count: rec.symbol_group_count,
        candidate_count: rec.candidate_count,
        selected_count: rec.selected_count,
        refused_count: rec.refused_count,
        truth_state: rec.truth_state.clone(),
        blockers: rec.blockers.clone(),
        created_at_utc: rec.created_at_utc.to_rfc3339(),
        approved_for_live: false,
    }
}

fn candidate_row_from_record(
    rec: &mqk_db::RuntimeStrategyConflictCandidateRecord,
) -> ConflictPlanCandidateRow {
    ConflictPlanCandidateRow {
        symbol: rec.symbol.clone(),
        strategy_id: rec.strategy_id.clone(),
        timeframe_secs: rec.timeframe_secs,
        side: rec.side.clone(),
        qty: rec.qty,
        current_qty: rec.current_qty,
        proposed_target_qty: rec.proposed_target_qty,
        order_type: rec.order_type.clone(),
        time_in_force: rec.time_in_force.clone(),
        limit_price: rec.limit_price,
        bar_present: rec.bar_present,
        bar_symbol: rec.bar_symbol.clone(),
        bar_strategy_id: rec.bar_strategy_id.clone(),
        bar_timeframe: rec.bar_timeframe.clone(),
        bar_end_ts: rec.bar_end_ts,
        close_micros: rec.close_micros,
        selected: rec.selected,
        disposition: rec.disposition.clone(),
        reason_code: rec.reason_code.clone(),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/strategy/conflict/status
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct StatusParams {
    pub run_id: Option<String>,
}

pub(crate) async fn strategy_conflict_status(
    State(st): State<Arc<AppState>>,
    Query(params): Query<StatusParams>,
) -> impl IntoResponse {
    let checked_at_utc = Utc::now().to_rfc3339();
    let resolution = resolve_conflict_policy_mode_from_env();
    let broker_kind = BrokerKind::parse(st.adapter_id());
    let eff = effective_mode(&resolution, st.deployment_mode(), broker_kind);

    let mut response = ConflictStatusResponse {
        truth_state: ConflictTruthState::Active,
        mode_configured: eff.configured_mode.as_str().to_string(),
        mode_effective: eff.effective_mode.as_str().to_string(),
        invalid_configuration: eff.invalid_configuration.clone(),
        live_lock_applied: eff.live_lock_applied,
        approved_for_live: false,
        current_runtime_limitation: CURRENT_RUNTIME_LIMITATION.to_string(),
        run_id: None,
        latest_plan_id: None,
        latest_plan_created_at_utc: None,
        latest_plan_symbol_group_count: None,
        latest_plan_candidate_count: None,
        latest_plan_selected_count: None,
        evidence_blockers: vec![],
        checked_at_utc: checked_at_utc.clone(),
    };
    if eff.invalid_configuration.is_some() {
        response.truth_state = ConflictTruthState::InvalidConfiguration;
    }

    let explicit_run_id = match params.run_id.as_deref().map(|s| s.parse::<Uuid>()) {
        None => None,
        Some(Ok(id)) => Some(id),
        Some(Err(_)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: "invalid_request",
                    detail: "run_id query parameter is not a valid UUID".to_string(),
                }),
            )
                .into_response();
        }
    };

    let Some(db) = st.db.as_ref() else {
        response.truth_state = ConflictTruthState::DbUnavailable;
        return (StatusCode::OK, Json(response)).into_response();
    };

    let run = match resolve_run(db, explicit_run_id).await {
        RunResolution::Found(r) => *r,
        RunResolution::QueryFailed => {
            response.truth_state = ConflictTruthState::QueryFailed;
            return (StatusCode::OK, Json(response)).into_response();
        }
        RunResolution::NotFound => {
            response.truth_state = ConflictTruthState::NotFound;
            return (StatusCode::OK, Json(response)).into_response();
        }
    };
    response.run_id = Some(run.run_id.to_string());

    // Defect 6: inspect and validate the latest plan *plus its candidates*
    // before projecting any of its fields. A malformed latest plan is
    // surfaced as invalid_evidence -- never silently skipped in favor of
    // an older, valid plan.
    let latest_summary =
        match mqk_db::fetch_recent_runtime_strategy_conflict_plans(db, run.run_id, 1).await {
            Ok(plans) => plans.into_iter().next(),
            Err(err) => {
                tracing::warn!(error = %err, "strategy_conflict_status_plan_query_failed");
                response.truth_state = ConflictTruthState::QueryFailed;
                return (StatusCode::OK, Json(response)).into_response();
            }
        };

    let Some(latest_summary) = latest_summary else {
        // No plan yet for this run -- honest empty, still active.
        return (StatusCode::OK, Json(response)).into_response();
    };

    match mqk_db::fetch_runtime_strategy_conflict_plan_for_read(
        db,
        latest_summary.plan_id,
        MAX_CANDIDATES_PER_PLAN,
    )
    .await
    {
        Ok(BoundedConflictPlanFetch::Complete(plan_record, candidate_records)) => {
            let validation = validate_plan_with_candidates(&plan_record, &candidate_records);
            if !validation.valid {
                response.truth_state = ConflictTruthState::InvalidEvidence;
                response.evidence_blockers = validation.blockers;
                return (StatusCode::OK, Json(response)).into_response();
            }
            response.latest_plan_id = Some(plan_record.plan_id.to_string());
            response.latest_plan_created_at_utc = Some(plan_record.created_at_utc.to_rfc3339());
            response.latest_plan_symbol_group_count = Some(plan_record.symbol_group_count);
            response.latest_plan_candidate_count = Some(plan_record.candidate_count);
            response.latest_plan_selected_count = Some(plan_record.selected_count);
        }
        Ok(BoundedConflictPlanFetch::CandidateLimitExceeded {
            observed_at_least, ..
        }) => {
            // Defect 2: the durable plan exceeds the shared bounded-read
            // limit -- fail closed to invalid_evidence with no fallback to
            // an older plan, exactly like any other malformed-evidence
            // outcome. No candidate data was fetched.
            let validation = evidence_validation_for_candidate_limit_exceeded(observed_at_least);
            response.truth_state = ConflictTruthState::InvalidEvidence;
            response.evidence_blockers = validation.blockers;
        }
        Ok(BoundedConflictPlanFetch::NotFound) => {
            // The summary row vanished between the two reads (concurrent
            // delete elsewhere is not a route this file exposes -- treat
            // defensively as not_found rather than fabricating a plan).
            response.truth_state = ConflictTruthState::NotFound;
        }
        Err(err) => {
            tracing::warn!(error = %err, plan_id = %latest_summary.plan_id, "strategy_conflict_status_plan_detail_query_failed");
            response.truth_state = ConflictTruthState::QueryFailed;
        }
    }

    (StatusCode::OK, Json(response)).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/strategy/conflict/plans?limit=&run_id=
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct PlansListParams {
    pub limit: Option<i64>,
    pub run_id: Option<String>,
}

pub(crate) async fn strategy_conflict_plans(
    State(st): State<Arc<AppState>>,
    Query(params): Query<PlansListParams>,
) -> impl IntoResponse {
    let checked_at_utc = Utc::now().to_rfc3339();

    let explicit_run_id = match params.run_id.as_deref().map(|s| s.parse::<Uuid>()) {
        None => None,
        Some(Ok(id)) => Some(id),
        Some(Err(_)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: "invalid_request",
                    detail: "run_id query parameter is not a valid UUID".to_string(),
                }),
            )
                .into_response();
        }
    };

    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(ConflictPlansListResponse {
                truth_state: ConflictTruthState::DbUnavailable,
                run_id: None,
                plans: vec![],
                excluded_malformed_count: 0,
                excluded_vanished_count: 0,
                checked_at_utc,
            }),
        )
            .into_response();
    };

    let run = match resolve_run(db, explicit_run_id).await {
        RunResolution::Found(r) => *r,
        RunResolution::QueryFailed => {
            return (
                StatusCode::OK,
                Json(ConflictPlansListResponse {
                    truth_state: ConflictTruthState::QueryFailed,
                    run_id: None,
                    plans: vec![],
                    excluded_malformed_count: 0,
                    excluded_vanished_count: 0,
                    checked_at_utc,
                }),
            )
                .into_response();
        }
        RunResolution::NotFound => {
            return (
                StatusCode::OK,
                Json(ConflictPlansListResponse {
                    truth_state: ConflictTruthState::NotFound,
                    run_id: None,
                    plans: vec![],
                    excluded_malformed_count: 0,
                    excluded_vanished_count: 0,
                    checked_at_utc,
                }),
            )
                .into_response();
        }
    };

    // Bounded: default 20, minimum 1, maximum 100 (mirrors
    // durable_portfolio.rs / portfolio_allocation.rs's list clamp).
    let limit = params.limit.unwrap_or(20).clamp(1, 100);

    match mqk_db::fetch_recent_runtime_strategy_conflict_plans(db, run.run_id, limit).await {
        Ok(records) => {
            // Defect 5: a per-row detail/candidate query *error* is not
            // malformed evidence -- it means this route could not prove
            // anything about the list at all, so the whole response fails
            // closed to `query_failed` with no active list projection,
            // rather than silently folding the failure into
            // `excluded_malformed_count` under an otherwise-`active`
            // truth_state. A row that merely *vanished* between the two
            // reads (a benign concurrent-delete race) remains
            // distinguishable from a row that was read and failed
            // validation. `limit` is bounded to at most 100, so the extra
            // per-row candidate fetch needed for the full check is a
            // bounded cost, not an unbounded N+1 -- correctness over one
            // saved query per row.
            let mut plans = Vec::with_capacity(records.len());
            let mut excluded_malformed_count = 0i64;
            let mut excluded_vanished_count = 0i64;
            for rec in &records {
                match mqk_db::fetch_runtime_strategy_conflict_plan_for_read(
                    db,
                    rec.plan_id,
                    MAX_CANDIDATES_PER_PLAN,
                )
                .await
                {
                    Ok(BoundedConflictPlanFetch::Complete(plan_record, candidate_records)) => {
                        if validate_plan_with_candidates(&plan_record, &candidate_records).valid {
                            plans.push(plan_row_from_record(rec));
                        } else {
                            excluded_malformed_count += 1;
                        }
                    }
                    Ok(BoundedConflictPlanFetch::CandidateLimitExceeded { .. }) => {
                        // Defect 2: a successfully-read over-limit plan is
                        // malformed evidence, never active -- and never
                        // projected with partial candidate data.
                        excluded_malformed_count += 1;
                    }
                    Ok(BoundedConflictPlanFetch::NotFound) => {
                        excluded_vanished_count += 1;
                    }
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            plan_id = %rec.plan_id,
                            "strategy_conflict_plans_row_detail_query_failed"
                        );
                        return (
                            StatusCode::OK,
                            Json(ConflictPlansListResponse {
                                truth_state: ConflictTruthState::QueryFailed,
                                run_id: Some(run.run_id.to_string()),
                                plans: vec![],
                                excluded_malformed_count: 0,
                                excluded_vanished_count: 0,
                                checked_at_utc,
                            }),
                        )
                            .into_response();
                    }
                }
            }
            (
                StatusCode::OK,
                Json(ConflictPlansListResponse {
                    truth_state: ConflictTruthState::Active,
                    run_id: Some(run.run_id.to_string()),
                    plans,
                    excluded_malformed_count,
                    excluded_vanished_count,
                    checked_at_utc,
                }),
            )
                .into_response()
        }
        Err(err) => {
            tracing::warn!(error = %err, run_id = %run.run_id, "strategy_conflict_plans_query_failed");
            (
                StatusCode::OK,
                Json(ConflictPlansListResponse {
                    truth_state: ConflictTruthState::QueryFailed,
                    run_id: Some(run.run_id.to_string()),
                    plans: vec![],
                    excluded_malformed_count: 0,
                    excluded_vanished_count: 0,
                    checked_at_utc,
                }),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/strategy/conflict/plans/:plan_id
// ---------------------------------------------------------------------------

/// Fixed bounded message for an invalid `plan_id` path param — never echoes
/// the caller-supplied raw value back onto the wire.
const INVALID_PLAN_ID_MESSAGE: &str = "plan_id path parameter is not a valid UUID";

pub(crate) async fn strategy_conflict_plan_by_id(
    State(st): State<Arc<AppState>>,
    Path(plan_id_raw): Path<String>,
) -> impl IntoResponse {
    let checked_at_utc = Utc::now().to_rfc3339();

    let Ok(plan_id) = plan_id_raw.parse::<Uuid>() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorBody {
                error: "invalid_request",
                detail: INVALID_PLAN_ID_MESSAGE.to_string(),
            }),
        )
            .into_response();
    };

    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(ConflictPlanDetailResponse {
                truth_state: ConflictTruthState::DbUnavailable,
                plan: None,
                candidates: vec![],
                evidence_blockers: vec![],
                checked_at_utc,
            }),
        )
            .into_response();
    };

    match mqk_db::fetch_runtime_strategy_conflict_plan_for_read(
        db,
        plan_id,
        MAX_CANDIDATES_PER_PLAN,
    )
    .await
    {
        Ok(BoundedConflictPlanFetch::Complete(plan_record, candidate_records)) => {
            let validation = validate_plan_with_candidates(&plan_record, &candidate_records);
            if !validation.valid {
                return (
                    StatusCode::OK,
                    Json(ConflictPlanDetailResponse {
                        truth_state: ConflictTruthState::InvalidEvidence,
                        plan: None,
                        candidates: vec![],
                        evidence_blockers: validation.blockers,
                        checked_at_utc,
                    }),
                )
                    .into_response();
            }
            (
                StatusCode::OK,
                Json(ConflictPlanDetailResponse {
                    truth_state: ConflictTruthState::Active,
                    plan: Some(plan_row_from_record(&plan_record)),
                    candidates: candidate_records
                        .iter()
                        .map(candidate_row_from_record)
                        .collect(),
                    evidence_blockers: vec![],
                    checked_at_utc,
                }),
            )
                .into_response()
        }
        Ok(BoundedConflictPlanFetch::CandidateLimitExceeded {
            observed_at_least, ..
        }) => {
            // Defect 2: over-limit plan -- invalid_evidence, no partial
            // candidate projection.
            let validation = evidence_validation_for_candidate_limit_exceeded(observed_at_least);
            (
                StatusCode::OK,
                Json(ConflictPlanDetailResponse {
                    truth_state: ConflictTruthState::InvalidEvidence,
                    plan: None,
                    candidates: vec![],
                    evidence_blockers: validation.blockers,
                    checked_at_utc,
                }),
            )
                .into_response()
        }
        Ok(BoundedConflictPlanFetch::NotFound) => (
            StatusCode::OK,
            Json(ConflictPlanDetailResponse {
                truth_state: ConflictTruthState::NotFound,
                plan: None,
                candidates: vec![],
                evidence_blockers: vec![],
                checked_at_utc,
            }),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(error = %err, plan_id = %plan_id, "strategy_conflict_plan_by_id_query_failed");
            (
                StatusCode::OK,
                Json(ConflictPlanDetailResponse {
                    truth_state: ConflictTruthState::QueryFailed,
                    plan: None,
                    candidates: vec![],
                    evidence_blockers: vec![],
                    checked_at_utc,
                }),
            )
                .into_response()
        }
    }
}
