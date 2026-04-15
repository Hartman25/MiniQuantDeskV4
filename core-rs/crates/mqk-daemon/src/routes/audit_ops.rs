//! Audit and operator-timeline route handlers.
//!
//! Contains: audit_operator_actions, audit_artifacts, ops_operator_timeline.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use sqlx::Row;

use crate::api_types::{
    AuditArtifactRow, AuditArtifactsResponse, OperatorActionAuditRow, OperatorActionsAuditResponse,
    OperatorTimelineResponse, OperatorTimelineRow, RuntimeErrorResponse,
};
use crate::state::AppState;

use super::helpers::runtime_transition_for_action;

// ---------------------------------------------------------------------------
// GET /api/v1/audit/operator-actions
// ---------------------------------------------------------------------------

pub(crate) async fn audit_operator_actions(State(st): State<Arc<AppState>>) -> Response {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(OperatorActionsAuditResponse {
                canonical_route: "/api/v1/audit/operator-actions".to_string(),
                truth_state: "backend_unavailable".to_string(),
                backend: "unavailable".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    let rows = match sqlx::query(
        r#"
        select event_id, run_id, ts_utc, event_type
        from audit_events
        where topic = 'operator'
        order by ts_utc desc
        limit 200
        "#,
    )
    .fetch_all(db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("audit/operator-actions query failed: {err}"),
                    fault_class: "audit.operator_actions.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response()
        }
    };

    let rows = rows
        .into_iter()
        .map(|row| {
            let event_id: uuid::Uuid = row.get("event_id");
            let run_id: Option<uuid::Uuid> = row.get("run_id");
            let ts_utc: chrono::DateTime<chrono::Utc> = row.get("ts_utc");
            let event_type: String = row.get("event_type");
            OperatorActionAuditRow {
                audit_event_id: event_id.to_string(),
                ts_utc: ts_utc.to_rfc3339(),
                requested_action: event_type.clone(),
                disposition: "applied".to_string(),
                run_id: run_id.map(|v| v.to_string()),
                runtime_transition: runtime_transition_for_action(&event_type),
                provenance_ref: format!("audit_events:{}", event_id),
            }
        })
        .collect();

    (
        StatusCode::OK,
        Json(OperatorActionsAuditResponse {
            canonical_route: "/api/v1/audit/operator-actions".to_string(),
            truth_state: "active".to_string(),
            backend: "postgres.audit_events".to_string(),
            rows,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/audit/artifacts
// ---------------------------------------------------------------------------

pub(crate) async fn audit_artifacts(State(st): State<Arc<AppState>>) -> Response {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(AuditArtifactsResponse {
                canonical_route: "/api/v1/audit/artifacts".to_string(),
                truth_state: "backend_unavailable".to_string(),
                backend: "unavailable".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    let runs = match sqlx::query(
        r#"
        select run_id, started_at_utc
        from runs
        order by started_at_utc desc, run_id desc
        limit 200
        "#,
    )
    .fetch_all(db)
    .await
    {
        Ok(runs) => runs,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("audit/artifacts query failed: {err}"),
                    fault_class: "audit.artifacts.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response()
        }
    };

    let rows = runs
        .into_iter()
        .map(|row| {
            let run_id: uuid::Uuid = row.get("run_id");
            let started_at_utc: chrono::DateTime<chrono::Utc> = row.get("started_at_utc");
            AuditArtifactRow {
                artifact_id: format!("run-config:{}", run_id),
                artifact_type: "run_config".to_string(),
                run_id: run_id.to_string(),
                created_at_utc: started_at_utc.to_rfc3339(),
                provenance_ref: format!("runs:{}", run_id),
            }
        })
        .collect();

    (
        StatusCode::OK,
        Json(AuditArtifactsResponse {
            canonical_route: "/api/v1/audit/artifacts".to_string(),
            truth_state: "active".to_string(),
            backend: "postgres.runs".to_string(),
            rows,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/ops/operator-timeline
// ---------------------------------------------------------------------------

pub(crate) async fn ops_operator_timeline(State(st): State<Arc<AppState>>) -> Response {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(OperatorTimelineResponse {
                canonical_route: "/api/v1/ops/operator-timeline".to_string(),
                truth_state: "backend_unavailable".to_string(),
                backend: "unavailable".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    let runs = match sqlx::query(
        r#"
        select run_id, started_at_utc, armed_at_utc, running_at_utc, stopped_at_utc, halted_at_utc
        from runs
        order by started_at_utc desc, run_id desc
        limit 200
        "#,
    )
    .fetch_all(db)
    .await
    {
        Ok(runs) => runs,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("ops/operator-timeline runs query failed: {err}"),
                    fault_class: "ops.operator_timeline.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response()
        }
    };

    let mut rows: Vec<OperatorTimelineRow> = Vec::new();
    for row in &runs {
        let run_id: uuid::Uuid = row.get("run_id");
        let started_at_utc: chrono::DateTime<chrono::Utc> = row.get("started_at_utc");
        let armed_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("armed_at_utc");
        let running_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("running_at_utc");
        let stopped_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("stopped_at_utc");
        let halted_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("halted_at_utc");

        rows.push(OperatorTimelineRow {
            ts_utc: started_at_utc.to_rfc3339(),
            kind: "runtime_transition".to_string(),
            run_id: Some(run_id.to_string()),
            detail: "CREATED".to_string(),
            provenance_ref: format!("runs:{}:started_at_utc", run_id),
            audit_event_id: None,
        });
        if let Some(ts) = armed_at_utc {
            rows.push(OperatorTimelineRow {
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                run_id: Some(run_id.to_string()),
                detail: "ARMED".to_string(),
                provenance_ref: format!("runs:{}:armed_at_utc", run_id),
                audit_event_id: None,
            });
        }
        if let Some(ts) = running_at_utc {
            rows.push(OperatorTimelineRow {
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                run_id: Some(run_id.to_string()),
                detail: "RUNNING".to_string(),
                provenance_ref: format!("runs:{}:running_at_utc", run_id),
                audit_event_id: None,
            });
        }
        if let Some(ts) = stopped_at_utc {
            rows.push(OperatorTimelineRow {
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                run_id: Some(run_id.to_string()),
                detail: "STOPPED".to_string(),
                provenance_ref: format!("runs:{}:stopped_at_utc", run_id),
                audit_event_id: None,
            });
        }
        if let Some(ts) = halted_at_utc {
            rows.push(OperatorTimelineRow {
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                run_id: Some(run_id.to_string()),
                detail: "HALTED".to_string(),
                provenance_ref: format!("runs:{}:halted_at_utc", run_id),
                audit_event_id: None,
            });
        }
    }

    let operator_events = match sqlx::query(
        r#"
        select event_id, run_id, ts_utc, event_type
        from audit_events
        where topic = 'operator'
        order by ts_utc desc
        limit 200
        "#,
    )
    .fetch_all(db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("ops/operator-timeline operator events query failed: {err}"),
                    fault_class: "ops.operator_timeline.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response()
        }
    };

    for row in operator_events {
        let event_id: uuid::Uuid = row.get("event_id");
        let run_id: Option<uuid::Uuid> = row.get("run_id");
        let ts_utc: chrono::DateTime<chrono::Utc> = row.get("ts_utc");
        let event_type: String = row.get("event_type");

        rows.push(OperatorTimelineRow {
            ts_utc: ts_utc.to_rfc3339(),
            kind: "operator_action".to_string(),
            run_id: run_id.map(|id| id.to_string()),
            detail: event_type,
            provenance_ref: format!("audit_events:{}", event_id),
            // OPTR-03: surface the raw audit event UUID as a first-class
            // correlation key so consumers can join directly to
            // /api/v1/audit/operator-actions without parsing provenance_ref.
            audit_event_id: Some(event_id.to_string()),
        });
    }

    rows.sort_by(|a, b| b.ts_utc.cmp(&a.ts_utc));

    (
        StatusCode::OK,
        Json(OperatorTimelineResponse {
            canonical_route: "/api/v1/ops/operator-timeline".to_string(),
            truth_state: "active".to_string(),
            backend: "postgres.runs+postgres.audit_events".to_string(),
            rows,
        }),
    )
        .into_response()
}
