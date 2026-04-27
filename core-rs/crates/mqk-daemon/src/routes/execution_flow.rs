// core-rs/crates/mqk-daemon/src/routes/execution_flow.rs
//
// FLOW-01/FLOW-02/FLOW-03: GET /api/v1/execution/flow
//
// Read-only execution flow surface. Assembles a unified, time-ordered
// sequence of operator-visible lifecycle events from three existing durable
// tables (no new DB schema required):
//
//   oms_outbox                  → outbox lifecycle stages (enqueued → sent)
//   oms_order_lifecycle_events  → cancel / replace ack / reject
//   fill_quality_telemetry      → fill events
//
// Hard constraints observed:
// - Read-only. Does not authorize, block, approve, or retry anything.
// - No synthetic broker events. No fabricated lifecycle.
// - Distinguishes unavailable / empty / present truthfully.
// - Enforces a hard default limit (100) and max limit (200).

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api_types::{ExecutionFlowApiRow, ExecutionFlowResponse};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Constants — FLOW-06: retention / query performance guard
// ---------------------------------------------------------------------------

/// Default number of rows returned when the caller does not specify `limit`.
const DEFAULT_LIMIT: usize = 100;

/// Hard cap on rows returned. The caller cannot exceed this.
const MAX_LIMIT: usize = 200;

const CANONICAL: &str = "/api/v1/execution/flow";

const BACKEND_ACTIVE: &str =
    "postgres.oms_outbox+postgres.oms_order_lifecycle_events+postgres.fill_quality_telemetry";

// ---------------------------------------------------------------------------
// Query parameters — FLOW-05: filters
// ---------------------------------------------------------------------------

/// Query parameters supported by `GET /api/v1/execution/flow`.
///
/// All parameters are optional. When none are provided the handler resolves
/// the active run from daemon state (if available) and returns up to
/// `DEFAULT_LIMIT` rows for that run.
#[derive(Debug, Deserialize)]
pub(crate) struct FlowQueryParams {
    /// Filter to a specific durable run. Must be a valid UUID string.
    pub run_id: Option<String>,
    /// Filter to a specific order. Maps to `idempotency_key` in `oms_outbox`
    /// and `internal_order_id` in the other two source tables.
    pub order_id: Option<String>,
    /// Maximum rows to return. Clamped to [1, 200]. Default: 100.
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `GET /api/v1/execution/flow`
///
/// Returns a read-only, time-ordered list of execution lifecycle events
/// assembled from existing durable tables.
///
/// # Truth states
///
/// | truth_state      | HTTP | rows          | meaning                          |
/// |------------------|------|---------------|----------------------------------|
/// | `active`         | 200  | authoritative | DB present, run context resolved |
/// | `no_active_run`  | 200  | empty         | DB present, no run to scope to   |
/// | `no_db`          | 200  | empty         | no DB pool configured            |
///
/// Empty rows with `truth_state = "active"` means no matching events exist —
/// this is authoritative. The operator must not infer "no events" from
/// `no_active_run` or `no_db`.
pub(crate) async fn execution_flow(
    State(st): State<Arc<AppState>>,
    Query(params): Query<FlowQueryParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    // Input validation: run_id format check fires before any resource gate so
    // the caller receives a clear 400 (not no_db) on a malformed UUID.
    let explicit_run_id: Option<Uuid> = match params.run_id.as_deref() {
        Some(s) => match s.parse::<Uuid>() {
            Ok(id) => Some(id),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "run_id is not a valid UUID",
                        "provided": s,
                    })),
                )
                    .into_response();
            }
        },
        None => None,
    };

    // Gate 1: DB must be available.
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(ExecutionFlowResponse {
                canonical_route: CANONICAL.to_string(),
                truth_state: "no_db".to_string(),
                backend: "unavailable".to_string(),
                run_id: None,
                rows: vec![],
            }),
        )
            .into_response();
    };

    // Resolve run_id: explicit param takes priority, then fall back to
    // the active run from daemon state.
    let resolved_run_id: Option<Uuid> = if explicit_run_id.is_some() {
        explicit_run_id
    } else {
        match st.current_status_snapshot().await {
            Ok(snap) => snap.active_run_id,
            Err(_) => None,
        }
    };

    // Gate 2: if no run context and no order_id, we cannot scope the query
    // to authoritative data. Surface no_active_run rather than unbounded dump.
    if resolved_run_id.is_none() && params.order_id.is_none() {
        return (
            StatusCode::OK,
            Json(ExecutionFlowResponse {
                canonical_route: CANONICAL.to_string(),
                truth_state: "no_active_run".to_string(),
                backend: "unavailable".to_string(),
                run_id: None,
                rows: vec![],
            }),
        )
            .into_response();
    }

    let query = mqk_db::FlowQuery {
        run_id: resolved_run_id,
        internal_order_id: params.order_id.clone(),
        limit,
    };

    let db_rows = match mqk_db::fetch_execution_flow(db, query).await {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "flow_fetch_failed",
                    "detail": format!("{e:#}"),
                })),
            )
                .into_response();
        }
    };

    let api_rows: Vec<ExecutionFlowApiRow> = db_rows
        .into_iter()
        .map(|r| ExecutionFlowApiRow {
            row_id: r.row_id,
            ts_utc: r.ts_utc.to_rfc3339(),
            stage: r.stage,
            severity: r.severity,
            run_id: r.run_id.to_string(),
            internal_order_id: r.internal_order_id,
            broker_order_id: r.broker_order_id,
            symbol: r.symbol,
            message: r.message,
            source_table: r.source_table,
        })
        .collect();

    (
        StatusCode::OK,
        Json(ExecutionFlowResponse {
            canonical_route: CANONICAL.to_string(),
            truth_state: "active".to_string(),
            backend: BACKEND_ACTIVE.to_string(),
            run_id: resolved_run_id.map(|id| id.to_string()),
            rows: api_rows,
        }),
    )
        .into_response()
}
