//! Alert and event-feed route handlers (CC-06).
//!
//! Contains: alerts_active, events_feed.
//!
//! # Source model
//!
//! ## `/api/v1/alerts/active`
//!
//! Source: `build_fault_signals()` — current in-memory computation from
//! `StatusSnapshot` + `ReconcileStatusSnapshot` + DB-backed risk-block state
//! (falls back to `false` when no DB, consistent with `system/status`).
//!
//! `truth_state` is always `"active"`: the computation uses in-memory state
//! that is always present.  Empty `rows` = genuinely no current fault
//! conditions (healthy state, not absence of source).
//!
//! ## `/api/v1/events/feed`
//!
//! Source: `postgres.runs` (runtime lifecycle transitions) +
//! `postgres.audit_events` (operator actions, topic=`'operator'`).
//! Same source as `ops/operator-timeline` but limited to 50 most-recent rows.
//!
//! `truth_state` = `"active"` when DB pool present;
//! `"backend_unavailable"` when no DB pool.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use sqlx::Row;

use crate::api_types::{
    ActiveAlertRow, ActiveAlertsResponse, EventFeedRow, EventsFeedResponse, RuntimeErrorResponse,
};
use crate::state::AppState;

use super::helpers::{build_fault_signals, runtime_error_response};

// ---------------------------------------------------------------------------
// GET /api/v1/alerts/active
// ---------------------------------------------------------------------------

pub(crate) async fn alerts_active(State(st): State<Arc<AppState>>) -> Response {
    let status = match st.current_status_snapshot().await {
        Ok(snap) => snap,
        Err(err) => return runtime_error_response(err),
    };
    let reconcile = st.current_reconcile_snapshot().await;

    // Risk-blocked state requires a DB query.  Falls back to false when no DB,
    // matching the behaviour of `GET /api/v1/system/status`.
    let risk_blocked = if let Some(db) = st.db.as_ref() {
        mqk_db::load_risk_block_state(db)
            .await
            .ok()
            .flatten()
            .is_some_and(|risk| risk.blocked)
    } else {
        false
    };

    let fault_signals = build_fault_signals(&status, &reconcile, risk_blocked);

    let rows: Vec<ActiveAlertRow> = fault_signals
        .into_iter()
        .map(|s| ActiveAlertRow {
            alert_id: s.class.clone(),
            severity: s.severity,
            class: s.class,
            summary: s.summary,
            detail: s.detail,
            source: "daemon.runtime_state".to_string(),
        })
        .collect();

    let alert_count = rows.len();

    (
        StatusCode::OK,
        Json(ActiveAlertsResponse {
            canonical_route: "/api/v1/alerts/active".to_string(),
            truth_state: "active".to_string(),
            backend: "daemon.runtime_state".to_string(),
            alert_count,
            rows,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/events/feed
// ---------------------------------------------------------------------------

pub(crate) async fn events_feed(State(st): State<Arc<AppState>>) -> Response {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(EventsFeedResponse {
                canonical_route: "/api/v1/events/feed".to_string(),
                truth_state: "backend_unavailable".to_string(),
                backend: "unavailable".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    // --- Runs: emit one row per durable lifecycle transition timestamp ---
    let runs = match sqlx::query(
        r#"
        select run_id, started_at_utc, armed_at_utc, running_at_utc, stopped_at_utc, halted_at_utc
        from runs
        order by started_at_utc desc, run_id desc
        limit 20
        "#,
    )
    .fetch_all(db)
    .await
    {
        Ok(r) => r,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("events/feed runs query failed: {err}"),
                    fault_class: "events.feed.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response()
        }
    };

    let mut rows: Vec<EventFeedRow> = Vec::new();

    for row in &runs {
        let run_id: uuid::Uuid = row.get("run_id");
        let started_at_utc: chrono::DateTime<chrono::Utc> = row.get("started_at_utc");
        let armed_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("armed_at_utc");
        let running_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("running_at_utc");
        let stopped_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("stopped_at_utc");
        let halted_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("halted_at_utc");

        let run_id_str = run_id.to_string();

        rows.push(EventFeedRow {
            event_id: format!("runs:{}:started_at_utc", run_id),
            ts_utc: started_at_utc.to_rfc3339(),
            kind: "runtime_transition".to_string(),
            detail: "CREATED".to_string(),
            run_id: Some(run_id_str.clone()),
            provenance_ref: format!("runs:{}:started_at_utc", run_id),
        });
        if let Some(ts) = armed_at_utc {
            rows.push(EventFeedRow {
                event_id: format!("runs:{}:armed_at_utc", run_id),
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                detail: "ARMED".to_string(),
                run_id: Some(run_id_str.clone()),
                provenance_ref: format!("runs:{}:armed_at_utc", run_id),
            });
        }
        if let Some(ts) = running_at_utc {
            rows.push(EventFeedRow {
                event_id: format!("runs:{}:running_at_utc", run_id),
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                detail: "RUNNING".to_string(),
                run_id: Some(run_id_str.clone()),
                provenance_ref: format!("runs:{}:running_at_utc", run_id),
            });
        }
        if let Some(ts) = stopped_at_utc {
            rows.push(EventFeedRow {
                event_id: format!("runs:{}:stopped_at_utc", run_id),
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                detail: "STOPPED".to_string(),
                run_id: Some(run_id_str.clone()),
                provenance_ref: format!("runs:{}:stopped_at_utc", run_id),
            });
        }
        if let Some(ts) = halted_at_utc {
            rows.push(EventFeedRow {
                event_id: format!("runs:{}:halted_at_utc", run_id),
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                detail: "HALTED".to_string(),
                run_id: Some(run_id_str.clone()),
                provenance_ref: format!("runs:{}:halted_at_utc", run_id),
            });
        }
    }

    // --- Operator audit events ---
    let operator_events = match sqlx::query(
        r#"
        select event_id, run_id, ts_utc, event_type
        from audit_events
        where topic = 'operator'
        order by ts_utc desc
        limit 50
        "#,
    )
    .fetch_all(db)
    .await
    {
        Ok(r) => r,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("events/feed audit query failed: {err}"),
                    fault_class: "events.feed.query_failed".to_string(),
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

        rows.push(EventFeedRow {
            event_id: format!("audit_events:{}", event_id),
            ts_utc: ts_utc.to_rfc3339(),
            kind: "operator_action".to_string(),
            detail: event_type,
            run_id: run_id.map(|id| id.to_string()),
            provenance_ref: format!("audit_events:{}", event_id),
        });
    }

    // Sort newest-first and cap at 50 rows.
    rows.sort_by(|a, b| b.ts_utc.cmp(&a.ts_utc));
    rows.truncate(50);

    (
        StatusCode::OK,
        Json(EventsFeedResponse {
            canonical_route: "/api/v1/events/feed".to_string(),
            truth_state: "active".to_string(),
            backend: "postgres.runs+postgres.audit_events".to_string(),
            rows,
        }),
    )
        .into_response()
}
