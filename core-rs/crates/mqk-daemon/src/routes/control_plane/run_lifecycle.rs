//! Run lifecycle handlers: run_start, run_stop, run_halt.
//!
//! Extracted from control_plane.rs (MT-07C). Zero behavior change.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use tracing::info;

use crate::notify::{CriticalAlertPayload, OperatorNotifyPayload, RunStatusPayload};
use crate::state::AppState;

use super::super::helpers::{runtime_error_response, write_operator_audit_event};

// ---------------------------------------------------------------------------
// POST /v1/run/start
// ---------------------------------------------------------------------------

pub(crate) async fn run_start(State(st): State<Arc<AppState>>) -> Response {
    match st.start_execution_runtime().await {
        Ok(snapshot) => {
            info!(run_id = ?snapshot.active_run_id, "run/start");
            let audit_uuid = if let Some(run_id) = snapshot.active_run_id {
                write_operator_audit_event(&st, Some(run_id), "run.start", "RUNNING")
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            };
            let ts = Utc::now().to_rfc3339();
            let env = Some(st.deployment_mode().as_api_label().to_string());
            let run_id_str = snapshot.active_run_id.map(|id| id.to_string());
            st.discord_notifier
                .notify_operator_action(&OperatorNotifyPayload {
                    action_key: "run.start".to_string(),
                    disposition: "applied".to_string(),
                    environment: env.clone(),
                    ts_utc: ts.clone(),
                    provenance_ref: audit_uuid.map(|id| format!("audit_events:{}", id)),
                    run_id: run_id_str.clone(),
                })
                .await;
            // DIS-02: structured run lifecycle summary.
            st.discord_notifier
                .notify_run_status(&RunStatusPayload {
                    event: "run.started".to_string(),
                    run_id: run_id_str,
                    environment: env,
                    note: None,
                    ts_utc: ts,
                })
                .await;
            (StatusCode::OK, Json(snapshot)).into_response()
        }
        Err(err) => runtime_error_response(err),
    }
}

// ---------------------------------------------------------------------------
// POST /v1/run/stop
// ---------------------------------------------------------------------------

pub(crate) async fn run_stop(State(st): State<Arc<AppState>>) -> Response {
    match st.stop_execution_runtime().await {
        Ok(snapshot) => {
            info!("run/stop");
            let audit_uuid =
                write_operator_audit_event(&st, snapshot.active_run_id, "run.stop", "STOPPED")
                    .await
                    .ok()
                    .flatten();
            let ts = Utc::now().to_rfc3339();
            let env = Some(st.deployment_mode().as_api_label().to_string());
            let run_id_str = snapshot.active_run_id.map(|id| id.to_string());
            st.discord_notifier
                .notify_operator_action(&OperatorNotifyPayload {
                    action_key: "run.stop".to_string(),
                    disposition: "applied".to_string(),
                    environment: env.clone(),
                    ts_utc: ts.clone(),
                    provenance_ref: audit_uuid.map(|id| format!("audit_events:{}", id)),
                    run_id: run_id_str.clone(),
                })
                .await;
            // DIS-02: structured run lifecycle summary.
            st.discord_notifier
                .notify_run_status(&RunStatusPayload {
                    event: "run.stopped".to_string(),
                    run_id: run_id_str,
                    environment: env,
                    note: None,
                    ts_utc: ts,
                })
                .await;
            (StatusCode::OK, Json(snapshot)).into_response()
        }
        Err(err) => runtime_error_response(err),
    }
}

// ---------------------------------------------------------------------------
// POST /v1/run/halt
// ---------------------------------------------------------------------------

pub(crate) async fn run_halt(State(st): State<Arc<AppState>>) -> Response {
    match st.halt_execution_runtime().await {
        Ok(snapshot) => {
            info!("run/halt");
            let audit_uuid =
                write_operator_audit_event(&st, snapshot.active_run_id, "run.halt", "HALTED")
                    .await
                    .ok()
                    .flatten();
            let ts = Utc::now().to_rfc3339();
            let env = Some(st.deployment_mode().as_api_label().to_string());
            let run_id_str = snapshot.active_run_id.map(|id| id.to_string());
            st.discord_notifier
                .notify_operator_action(&OperatorNotifyPayload {
                    action_key: "run.halt".to_string(),
                    disposition: "applied".to_string(),
                    environment: env.clone(),
                    ts_utc: ts.clone(),
                    provenance_ref: audit_uuid.map(|id| format!("audit_events:{}", id)),
                    run_id: run_id_str.clone(),
                })
                .await;
            // DIS-01: critical alert for the halt fault signal.
            st.discord_notifier
                .notify_critical_alert(&CriticalAlertPayload {
                    alert_class: "runtime.halt.operator_or_safety".to_string(),
                    severity: "critical".to_string(),
                    summary: "Runtime halted; dispatch is fail-closed.".to_string(),
                    detail: None,
                    environment: env.clone(),
                    run_id: run_id_str.clone(),
                    ts_utc: ts.clone(),
                })
                .await;
            // DIS-02: structured run lifecycle summary.
            st.discord_notifier
                .notify_run_status(&RunStatusPayload {
                    event: "run.halted".to_string(),
                    run_id: run_id_str,
                    environment: env,
                    note: Some("dispatch fail-closed".to_string()),
                    ts_utc: ts,
                })
                .await;
            (StatusCode::OK, Json(snapshot)).into_response()
        }
        Err(err) => runtime_error_response(err),
    }
}
