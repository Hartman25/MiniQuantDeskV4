use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;

use crate::{
    api_types::{OperatorActionAuditFields, OperatorActionResponse},
    state::{AppState, RuntimeLifecycleError},
};

#[derive(Debug, Serialize)]
pub struct ControlStatus {
    pub desired_armed: bool,
    pub leader_holder_id: Option<String>,
    pub leader_epoch: Option<i64>,
    pub lease_expires_at_utc: Option<String>,
    pub lease_expired: Option<bool>,
    pub active_run_id: Option<uuid::Uuid>,
    pub run_state: String,
    pub run_owned_locally: bool,
    pub run_notes: Option<String>,
    pub reconcile_status: String,
    pub reconcile_notes: Option<String>,
    pub integrity_state: String,
    pub integrity_reason: Option<String>,
    pub risk_blocked: bool,
    pub risk_reason: Option<String>,
    pub deadman_armed_state: String,
    pub deadman_reason: Option<String>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/control/status", get(status))
        .route("/control/disarm", post(disarm))
        .route("/control/arm", post(arm))
        .route("/control/restart", post(restart))
}

fn operator_action_response(
    requested_action: &str,
    accepted: bool,
    disposition: &str,
    resulting_integrity_state: Option<&str>,
    resulting_desired_armed: Option<bool>,
    blockers: Vec<String>,
    warnings: Vec<String>,
    durable_targets: Vec<String>,
) -> OperatorActionResponse {
    OperatorActionResponse {
        requested_action: requested_action.to_string(),
        accepted,
        disposition: disposition.to_string(),
        resulting_integrity_state: resulting_integrity_state.map(ToString::to_string),
        resulting_desired_armed,
        blockers,
        warnings,
        environment: std::env::var("MQK_ENV").ok(),
        scope: Some("daemon_instance".to_string()),
        audit: OperatorActionAuditFields {
            durable_db_write: accepted,
            durable_targets,
            audit_event_id: None,
        },
    }
}

async fn status(State(state): State<Arc<AppState>>) -> Response {
    let Some(db) = state.db.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "control DB is not configured on this daemon",
        )
            .into_response();
    };

    let runtime_status = match state.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return lifecycle_error_response(err),
    };
    let reconcile_status = state.current_reconcile_snapshot().await;
    let (integrity_state, integrity_reason) = match mqk_db::load_arm_state(db).await {
        Ok(Some((state, reason))) => (state, reason),
        Ok(None) => ("DISARMED".to_string(), Some("BootDefault".to_string())),
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("control/status arm state query failed: {err}"),
            )
                .into_response();
        }
    };
    let risk_state = match mqk_db::load_risk_block_state(db).await {
        Ok(value) => value,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("control/status risk state query failed: {err}"),
            )
                .into_response();
        }
    };

    let desired_armed: bool = match sqlx::query_scalar(
        r#"
        SELECT desired_armed
          FROM runtime_control_state
         WHERE id = 1
        "#,
    )
    .fetch_optional(db)
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => false,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("control/status desired_armed query failed: {err}"),
            )
                .into_response();
        }
    };

    let lease_row: Option<(String, i64, chrono::DateTime<chrono::Utc>, bool)> =
        match sqlx::query_as(
            r#"
            SELECT holder_id,
                   epoch,
                   lease_expires_at,
                   lease_expires_at <= now() AS lease_expired
              FROM runtime_leader_lease
             WHERE id = 1
            "#,
        )
        .fetch_optional(db)
        .await
        {
            Ok(row) => row,
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("control/status runtime lease query failed: {err}"),
                )
                    .into_response();
            }
        };

    let arm_state_row: Option<(String, Option<String>)> = match sqlx::query_as(
        r#"
        SELECT state, reason
          FROM sys_arm_state
         WHERE sentinel_id = 1
        "#,
    )
    .fetch_optional(db)
    .await
    {
        Ok(row) => row,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("control/status arm-state query failed: {err}"),
            )
                .into_response();
        }
    };

    let (deadman_armed_state, deadman_reason) =
        arm_state_row.unwrap_or_else(|| ("DISARMED".to_string(), Some("BootDefault".to_string())));

    let response = match lease_row {
        Some((holder_id, epoch, lease_expires_at, lease_expired)) => ControlStatus {
            desired_armed,
            leader_holder_id: Some(holder_id),
            leader_epoch: Some(epoch),
            lease_expires_at_utc: Some(lease_expires_at.to_rfc3339()),
            lease_expired: Some(lease_expired),
            active_run_id: runtime_status.active_run_id,
            run_state: runtime_status.state.clone(),
            run_owned_locally: runtime_status.state == "running",
            run_notes: runtime_status.notes.clone(),
            reconcile_status: reconcile_status.status.clone(),
            reconcile_notes: reconcile_status.note.clone(),
            integrity_state: integrity_state.clone(),
            integrity_reason: integrity_reason.clone(),
            risk_blocked: risk_state.as_ref().is_some_and(|r| r.blocked),
            risk_reason: risk_state.as_ref().and_then(|r| r.reason.clone()),
            deadman_armed_state: deadman_armed_state.clone(),
            deadman_reason: deadman_reason.clone(),
        },
        None => ControlStatus {
            desired_armed,
            leader_holder_id: None,
            leader_epoch: None,
            lease_expires_at_utc: None,
            lease_expired: None,
            active_run_id: runtime_status.active_run_id,
            run_state: runtime_status.state.clone(),
            run_owned_locally: runtime_status.state == "running",
            run_notes: runtime_status.notes.clone(),
            reconcile_status: reconcile_status.status.clone(),
            reconcile_notes: reconcile_status.note.clone(),
            integrity_state,
            integrity_reason,
            risk_blocked: risk_state.as_ref().is_some_and(|r| r.blocked),
            risk_reason: risk_state.as_ref().and_then(|r| r.reason.clone()),
            deadman_armed_state,
            deadman_reason,
        },
    };

    (StatusCode::OK, Json(response)).into_response()
}

async fn disarm(State(state): State<Arc<AppState>>) -> Response {
    let Some(db) = state.db.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "control DB is not configured on this daemon",
        )
            .into_response();
    };

    if let Err(err) = write_desired_armed(db, false).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("control/disarm write failed: {err}"),
        )
            .into_response();
    }
    if let Err(err) = mqk_db::persist_arm_state(db, "DISARMED", Some("OperatorDisarm")).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("control/disarm persist arm state failed: {err}"),
        )
            .into_response();
    }

    {
        let mut integrity = state.integrity.write().await;
        integrity.disarmed = true;
    }

    publish_integrity_status(&state, false, "control: desired_armed=false").await;
    (
        StatusCode::OK,
        Json(operator_action_response(
            "control.disarm",
            true,
            "applied",
            Some("DISARMED"),
            Some(false),
            Vec::new(),
            Vec::new(),
            vec![
                "runtime_control_state.desired_armed".to_string(),
                "sys_arm_state.state".to_string(),
            ],
        )),
    )
        .into_response()
}

async fn arm(State(state): State<Arc<AppState>>) -> Response {
    let Some(db) = state.db.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "control DB is not configured on this daemon",
        )
            .into_response();
    };

    if let Err(err) = write_desired_armed(db, true).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("control/arm write failed: {err}"),
        )
            .into_response();
    }
    if let Err(err) = mqk_db::persist_arm_state(db, "ARMED", None).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("control/arm persist arm state failed: {err}"),
        )
            .into_response();
    }

    {
        let mut integrity = state.integrity.write().await;
        integrity.disarmed = false;
        integrity.halted = false;
    }

    publish_integrity_status(&state, true, "control: desired_armed=true").await;
    (
        StatusCode::OK,
        Json(operator_action_response(
            "control.arm",
            true,
            "applied",
            Some("ARMED"),
            Some(true),
            Vec::new(),
            Vec::new(),
            vec![
                "runtime_control_state.desired_armed".to_string(),
                "sys_arm_state.state".to_string(),
            ],
        )),
    )
        .into_response()
}

async fn restart() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(operator_action_response(
            "control.restart",
            false,
            "not_authoritative",
            None,
            None,
            vec!["restart_not_authoritative".to_string()],
            Vec::new(),
            Vec::new(),
        )),
    )
        .into_response()
}

async fn write_desired_armed(db: &sqlx::PgPool, desired_armed: bool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO runtime_control_state (id, desired_armed, updated_at)
        VALUES (1, $1, now())
        ON CONFLICT (id) DO UPDATE
           SET desired_armed = excluded.desired_armed,
               updated_at    = excluded.updated_at
        "#,
    )
    .bind(desired_armed)
    .execute(db)
    .await?;

    Ok(())
}

async fn publish_integrity_status(state: &Arc<AppState>, integrity_armed: bool, note: &str) {
    let mut snapshot = match state.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(_) => crate::state::StatusSnapshot {
            daemon_uptime_secs: crate::state::uptime_secs(),
            active_run_id: None,
            state: "unknown".to_string(),
            notes: None,
            integrity_armed,
            deadman_status: "unknown".to_string(),
            deadman_last_heartbeat_utc: None,
        },
    };
    snapshot.integrity_armed = integrity_armed;
    snapshot.notes = Some(note.to_string());
    state.publish_status(snapshot).await;
}

fn lifecycle_error_response(err: RuntimeLifecycleError) -> Response {
    match err {
        RuntimeLifecycleError::Forbidden { gate, message } => (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": message, "gate": gate })),
        )
            .into_response(),
        RuntimeLifecycleError::ServiceUnavailable(message) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": message })),
        )
            .into_response(),
        RuntimeLifecycleError::Conflict(message) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": message })),
        )
            .into_response(),
        RuntimeLifecycleError::Internal(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": message })),
        )
            .into_response(),
    }
}
