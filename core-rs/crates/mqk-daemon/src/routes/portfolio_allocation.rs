// core-rs/crates/mqk-daemon/src/routes/portfolio_allocation.rs
//
// RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase H: read-only allocation truth.
//
// GET /api/v1/portfolio/allocation/status
// GET /api/v1/portfolio/allocation/plans?limit=&run_id=
// GET /api/v1/portfolio/allocation/plans/:plan_id   (whole-unit; refuses fractional)
// GET /api/v2/portfolio/allocation/plans/:plan_id   (qty_micros_v1 exact)
//
// GET-only: no route in this file ever inserts, updates, or deletes a row.
// `approved_for_live` is always `false`. `null` always means unavailable —
// never a fabricated zero/default. Run resolution mirrors
// `routes/durable_portfolio.rs` exactly: an explicit `?run_id=` query param,
// or else the latest durable PAPER run for this engine.

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
use crate::runtime_opportunity_mode::{
    effective_mode, resolve_runtime_opportunity_allocation_mode_from_env,
    RuntimeOpportunityAllocationMode,
};
use crate::state::{AppState, BrokerKind};

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Closed vocabulary. `db_unavailable` / `query_failed` / `not_found` mirror
/// the durable-portfolio routes; `active` means the mode/plan lookup
/// completed (a valid response was produced, whether or not a plan exists
/// yet); `invalid_configuration` means the env var is set to an unrecognized
/// value (mode still runs as `off`, but the operator must be told why).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AllocationTruthState {
    Active,
    InvalidConfiguration,
    DbUnavailable,
    QueryFailed,
    NotFound,
}

#[derive(Debug, Serialize)]
pub(crate) struct AllocationStatusResponse {
    pub truth_state: AllocationTruthState,
    pub mode_configured: String,
    pub mode_effective: String,
    pub invalid_configuration: Option<String>,
    pub live_lock_applied: bool,
    /// Always `false` — hard invariant, never overridable.
    pub approved_for_live: bool,
    /// `"none"` | `"shadow"` | `"paper_enforced"` — mirrors `mode_effective`,
    /// spelled out for GUI clarity per the task's required field name.
    pub runtime_influence: String,
    pub run_id: Option<String>,
    pub latest_plan_id: Option<String>,
    pub latest_plan_created_at_utc: Option<String>,
    pub latest_plan_candidate_count: Option<i32>,
    pub latest_plan_allowed_count: Option<i32>,
    pub checked_at_utc: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct AllocationPlanCandidateRow {
    pub symbol: String,
    pub strategy_id: String,
    pub input_score: f64,
    pub target_weight: f64,
    /// V1 quantity contract: whole asset units, never null. A fractional plan
    /// is refused by the V1 route (see
    /// `/api/v2/portfolio/allocation/plans/:plan_id`), never truncated.
    pub current_qty: i64,
    pub strategy_target_qty: i64,
    pub allocation_target_qty: i64,
    pub final_target_qty: i64,
    pub disposition: String,
    pub reason_code: String,
    pub evaluation_price_micros: i64,
}

/// Exact-quantity (`qty_micros_v1`) candidate row: raw `QtyMicros`
/// (1.0 unit == 1_000_000) is the only quantity authority.
#[derive(Debug, Serialize)]
pub(crate) struct AllocationPlanCandidateRowV2 {
    pub symbol: String,
    pub strategy_id: String,
    pub input_score: f64,
    pub target_weight: f64,
    pub current_qty_micros: i64,
    pub strategy_target_qty_micros: i64,
    pub allocation_target_qty_micros: i64,
    pub final_target_qty_micros: i64,
    pub disposition: String,
    pub reason_code: String,
    pub evaluation_price_micros: i64,
}

#[derive(Debug, Serialize)]
pub(crate) struct AllocationPlanRow {
    pub plan_id: String,
    pub cycle_id: String,
    pub run_id: String,
    pub mode: String,
    pub opportunity_artifact_id: String,
    pub source_snapshot_id: Option<String>,
    pub equity_micros: i64,
    pub candidate_count: i32,
    pub allowed_count: i32,
    pub gross_weight: f64,
    pub net_weight: f64,
    pub truth_state: String,
    pub blockers: Vec<String>,
    pub created_at_utc: String,
    /// Always `false` — hard invariant, never overridable.
    pub approved_for_live: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct AllocationPlanDetailResponse {
    pub truth_state: AllocationTruthState,
    pub plan: Option<AllocationPlanRow>,
    pub candidates: Vec<AllocationPlanCandidateRow>,
    pub checked_at_utc: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct AllocationPlanDetailResponseV2 {
    pub quantity_schema_version: String,
    pub truth_state: AllocationTruthState,
    pub plan: Option<AllocationPlanRow>,
    pub candidates: Vec<AllocationPlanCandidateRowV2>,
    pub checked_at_utc: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct AllocationPlansListResponse {
    pub truth_state: AllocationTruthState,
    pub run_id: Option<String>,
    pub plans: Vec<AllocationPlanRow>,
    pub checked_at_utc: String,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: &'static str,
    detail: String,
}

fn micros_to_f64(v: i64) -> f64 {
    v as f64 / mqk_db::RUNTIME_OPPORTUNITY_SCALE
}

fn plan_row_from_record(rec: &mqk_db::RuntimeOpportunityAllocationPlanRecord) -> AllocationPlanRow {
    AllocationPlanRow {
        plan_id: rec.plan_id.to_string(),
        cycle_id: rec.cycle_id.to_string(),
        run_id: rec.run_id.to_string(),
        mode: rec.mode.clone(),
        opportunity_artifact_id: rec.opportunity_artifact_id.clone(),
        source_snapshot_id: rec.source_snapshot_id.map(|id| id.to_string()),
        equity_micros: rec.equity_micros,
        candidate_count: rec.candidate_count,
        allowed_count: rec.allowed_count,
        gross_weight: micros_to_f64(rec.gross_weight_micros),
        net_weight: micros_to_f64(rec.net_weight_micros),
        truth_state: rec.truth_state.clone(),
        blockers: rec.blockers.clone(),
        created_at_utc: rec.created_at_utc.to_rfc3339(),
        approved_for_live: false,
    }
}

/// V1 projection; `None` when any quantity has no whole-unit form.
fn candidate_row_from_record(
    rec: &mqk_db::RuntimeOpportunityAllocationCandidateRecord,
) -> Option<AllocationPlanCandidateRow> {
    Some(AllocationPlanCandidateRow {
        symbol: rec.symbol.clone(),
        strategy_id: rec.strategy_id.clone(),
        input_score: micros_to_f64(rec.input_score_micros),
        target_weight: micros_to_f64(rec.target_weight_micros),
        current_qty: rec.current_qty.to_whole_units_checked()?,
        strategy_target_qty: rec.strategy_target_qty.to_whole_units_checked()?,
        allocation_target_qty: rec.allocation_target_qty.to_whole_units_checked()?,
        final_target_qty: rec.final_target_qty.to_whole_units_checked()?,
        disposition: rec.disposition.clone(),
        reason_code: rec.reason_code.clone(),
        evaluation_price_micros: rec.evaluation_price_micros,
    })
}

fn candidate_row_v2_from_record(
    rec: &mqk_db::RuntimeOpportunityAllocationCandidateRecord,
) -> AllocationPlanCandidateRowV2 {
    AllocationPlanCandidateRowV2 {
        symbol: rec.symbol.clone(),
        strategy_id: rec.strategy_id.clone(),
        input_score: micros_to_f64(rec.input_score_micros),
        target_weight: micros_to_f64(rec.target_weight_micros),
        current_qty_micros: rec.current_qty.raw(),
        strategy_target_qty_micros: rec.strategy_target_qty.raw(),
        allocation_target_qty_micros: rec.allocation_target_qty.raw(),
        final_target_qty_micros: rec.final_target_qty.raw(),
        disposition: rec.disposition.clone(),
        reason_code: rec.reason_code.clone(),
        evaluation_price_micros: rec.evaluation_price_micros,
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/portfolio/allocation/status
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct StatusParams {
    pub run_id: Option<String>,
}

pub(crate) async fn portfolio_allocation_status(
    State(st): State<Arc<AppState>>,
    Query(params): Query<StatusParams>,
) -> impl IntoResponse {
    let checked_at_utc = Utc::now().to_rfc3339();
    let resolution = resolve_runtime_opportunity_allocation_mode_from_env();
    let broker_kind = BrokerKind::parse(st.adapter_id());
    let eff = effective_mode(&resolution, st.deployment_mode(), broker_kind);
    let runtime_influence = match eff.effective_mode {
        RuntimeOpportunityAllocationMode::Off => "none",
        RuntimeOpportunityAllocationMode::Shadow => "shadow",
        RuntimeOpportunityAllocationMode::PaperEnforced => "paper_enforced",
    };

    let mut response = AllocationStatusResponse {
        truth_state: AllocationTruthState::Active,
        mode_configured: eff.configured_mode.as_str().to_string(),
        mode_effective: eff.effective_mode.as_str().to_string(),
        invalid_configuration: eff.invalid_configuration.clone(),
        live_lock_applied: eff.live_lock_applied,
        approved_for_live: false,
        runtime_influence: runtime_influence.to_string(),
        run_id: None,
        latest_plan_id: None,
        latest_plan_created_at_utc: None,
        latest_plan_candidate_count: None,
        latest_plan_allowed_count: None,
        checked_at_utc: checked_at_utc.clone(),
    };
    if eff.invalid_configuration.is_some() {
        response.truth_state = AllocationTruthState::InvalidConfiguration;
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
        response.truth_state = AllocationTruthState::DbUnavailable;
        return (StatusCode::OK, Json(response)).into_response();
    };

    let run = match resolve_run(db, explicit_run_id).await {
        RunResolution::Found(r) => *r,
        RunResolution::QueryFailed => {
            response.truth_state = AllocationTruthState::QueryFailed;
            return (StatusCode::OK, Json(response)).into_response();
        }
        RunResolution::NotFound => {
            response.truth_state = AllocationTruthState::NotFound;
            return (StatusCode::OK, Json(response)).into_response();
        }
    };
    response.run_id = Some(run.run_id.to_string());

    match mqk_db::fetch_recent_runtime_opportunity_allocation_plans(db, run.run_id, 1).await {
        Ok(plans) => {
            if let Some(latest) = plans.into_iter().next() {
                response.latest_plan_id = Some(latest.plan_id.to_string());
                response.latest_plan_created_at_utc = Some(latest.created_at_utc.to_rfc3339());
                response.latest_plan_candidate_count = Some(latest.candidate_count);
                response.latest_plan_allowed_count = Some(latest.allowed_count);
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "portfolio_allocation_status_plan_query_failed");
            response.truth_state = AllocationTruthState::QueryFailed;
        }
    }

    (StatusCode::OK, Json(response)).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/portfolio/allocation/plans?limit=&run_id=
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct PlansListParams {
    pub limit: Option<i64>,
    pub run_id: Option<String>,
}

pub(crate) async fn portfolio_allocation_plans(
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
            Json(AllocationPlansListResponse {
                truth_state: AllocationTruthState::DbUnavailable,
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
                Json(AllocationPlansListResponse {
                    truth_state: AllocationTruthState::QueryFailed,
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
                Json(AllocationPlansListResponse {
                    truth_state: AllocationTruthState::NotFound,
                    run_id: None,
                    plans: vec![],
                    checked_at_utc,
                }),
            )
                .into_response();
        }
    };

    // Bounded: default 20, minimum 1, maximum 100 (mirrors
    // durable_portfolio.rs's snapshots-list clamp).
    let limit = params.limit.unwrap_or(20).clamp(1, 100);

    match mqk_db::fetch_recent_runtime_opportunity_allocation_plans(db, run.run_id, limit).await {
        Ok(records) => {
            let plans = records.iter().map(plan_row_from_record).collect();
            (
                StatusCode::OK,
                Json(AllocationPlansListResponse {
                    truth_state: AllocationTruthState::Active,
                    run_id: Some(run.run_id.to_string()),
                    plans,
                    checked_at_utc,
                }),
            )
                .into_response()
        }
        Err(err) => {
            tracing::warn!(error = %err, run_id = %run.run_id, "portfolio_allocation_plans_query_failed");
            (
                StatusCode::OK,
                Json(AllocationPlansListResponse {
                    truth_state: AllocationTruthState::QueryFailed,
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
// GET /api/v1/portfolio/allocation/plans/:plan_id   (whole-unit; refuses fractional)
// GET /api/v2/portfolio/allocation/plans/:plan_id   (qty_micros_v1 exact)
// ---------------------------------------------------------------------------

/// Fixed bounded message for an invalid `plan_id` path param — never echoes
/// the caller-supplied raw value back onto the wire.
const INVALID_PLAN_ID_MESSAGE: &str = "plan_id path parameter is not a valid UUID";

/// Outcome of resolving one plan for the by-id routes.
enum PlanFetch {
    BadRequest,
    State(AllocationTruthState),
    Found(
        Box<mqk_db::RuntimeOpportunityAllocationPlanRecord>,
        Vec<mqk_db::RuntimeOpportunityAllocationCandidateRecord>,
    ),
}

async fn fetch_plan(st: &AppState, plan_id_raw: &str) -> PlanFetch {
    let Ok(plan_id) = plan_id_raw.parse::<Uuid>() else {
        return PlanFetch::BadRequest;
    };
    let Some(db) = st.db.as_ref() else {
        return PlanFetch::State(AllocationTruthState::DbUnavailable);
    };
    match mqk_db::fetch_runtime_opportunity_allocation_plan(db, plan_id).await {
        Ok(Some((plan_record, candidate_records))) => {
            PlanFetch::Found(Box::new(plan_record), candidate_records)
        }
        Ok(None) => PlanFetch::State(AllocationTruthState::NotFound),
        Err(err) => {
            tracing::warn!(error = %err, plan_id = %plan_id, "portfolio_allocation_plan_by_id_query_failed");
            PlanFetch::State(AllocationTruthState::QueryFailed)
        }
    }
}

fn invalid_plan_id_response() -> axum::response::Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorBody {
            error: "invalid_request",
            detail: INVALID_PLAN_ID_MESSAGE.to_string(),
        }),
    )
        .into_response()
}

/// V1 (whole-unit) plan detail. Fails closed with 409 when any candidate
/// quantity is fractional; never truncates it or reports it as null/zero.
pub(crate) async fn portfolio_allocation_plan_by_id(
    State(st): State<Arc<AppState>>,
    Path(plan_id_raw): Path<String>,
) -> impl IntoResponse {
    let checked_at_utc = Utc::now().to_rfc3339();
    let (truth_state, plan, candidates) = match fetch_plan(&st, &plan_id_raw).await {
        PlanFetch::BadRequest => return invalid_plan_id_response(),
        PlanFetch::State(truth_state) => (truth_state, None, vec![]),
        PlanFetch::Found(plan_record, candidate_records) => {
            let Some(candidates) = candidate_records
                .iter()
                .map(candidate_row_from_record)
                .collect::<Option<Vec<_>>>()
            else {
                return super::execution_order_analysis::v1_fractional_refusal(
                    "/api/v2/portfolio/allocation/plans/:plan_id",
                    "this allocation plan",
                );
            };
            (
                AllocationTruthState::Active,
                Some(plan_row_from_record(&plan_record)),
                candidates,
            )
        }
    };
    (
        StatusCode::OK,
        Json(AllocationPlanDetailResponse {
            truth_state,
            plan,
            candidates,
            checked_at_utc,
        }),
    )
        .into_response()
}

/// V2 (`qty_micros_v1`) plan detail: exact `*_qty_micros` for every candidate.
pub(crate) async fn portfolio_allocation_plan_by_id_v2(
    State(st): State<Arc<AppState>>,
    Path(plan_id_raw): Path<String>,
) -> impl IntoResponse {
    let checked_at_utc = Utc::now().to_rfc3339();
    let (truth_state, plan, candidates) = match fetch_plan(&st, &plan_id_raw).await {
        PlanFetch::BadRequest => return invalid_plan_id_response(),
        PlanFetch::State(truth_state) => (truth_state, None, vec![]),
        PlanFetch::Found(plan_record, candidate_records) => (
            AllocationTruthState::Active,
            Some(plan_row_from_record(&plan_record)),
            candidate_records
                .iter()
                .map(candidate_row_v2_from_record)
                .collect(),
        ),
    };
    (
        StatusCode::OK,
        Json(AllocationPlanDetailResponseV2 {
            quantity_schema_version: crate::api_types::QUANTITY_SCHEMA_VERSION_QTY_MICROS_V1
                .to_string(),
            truth_state,
            plan,
            candidates,
            checked_at_utc,
        }),
    )
        .into_response()
}
