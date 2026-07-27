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
use crate::runtime_strategy_conflict_mode::{
    effective_mode, resolve_conflict_policy_mode_from_env,
};
use crate::state::{AppState, BrokerKind};

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Closed vocabulary. `db_unavailable` / `query_failed` / `not_found` mirror
/// the durable-portfolio/allocation routes; `active` means the mode/plan
/// lookup completed (a valid response was produced, whether or not a plan
/// exists yet); `invalid_configuration` means the env var is set to an
/// unrecognized value (mode still runs as `off`, but the operator must be
/// told why).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConflictTruthState {
    Active,
    InvalidConfiguration,
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
    pub bar_end_ts: Option<i64>,
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
    pub checked_at_utc: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ConflictPlansListResponse {
    pub truth_state: ConflictTruthState,
    pub run_id: Option<String>,
    pub plans: Vec<ConflictPlanRow>,
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
        bar_end_ts: rec.bar_end_ts,
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

    match mqk_db::fetch_recent_runtime_strategy_conflict_plans(db, run.run_id, 1).await {
        Ok(plans) => {
            if let Some(latest) = plans.into_iter().next() {
                response.latest_plan_id = Some(latest.plan_id.to_string());
                response.latest_plan_created_at_utc = Some(latest.created_at_utc.to_rfc3339());
                response.latest_plan_symbol_group_count = Some(latest.symbol_group_count);
                response.latest_plan_candidate_count = Some(latest.candidate_count);
                response.latest_plan_selected_count = Some(latest.selected_count);
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "strategy_conflict_status_plan_query_failed");
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
            let plans = records.iter().map(plan_row_from_record).collect();
            (
                StatusCode::OK,
                Json(ConflictPlansListResponse {
                    truth_state: ConflictTruthState::Active,
                    run_id: Some(run.run_id.to_string()),
                    plans,
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
                checked_at_utc,
            }),
        )
            .into_response();
    };

    match mqk_db::fetch_runtime_strategy_conflict_plan(db, plan_id).await {
        Ok(Some((plan_record, candidate_records))) => (
            StatusCode::OK,
            Json(ConflictPlanDetailResponse {
                truth_state: ConflictTruthState::Active,
                plan: Some(plan_row_from_record(&plan_record)),
                candidates: candidate_records
                    .iter()
                    .map(candidate_row_from_record)
                    .collect(),
                checked_at_utc,
            }),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::OK,
            Json(ConflictPlanDetailResponse {
                truth_state: ConflictTruthState::NotFound,
                plan: None,
                candidates: vec![],
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
                    checked_at_utc,
                }),
            )
                .into_response()
        }
    }
}
