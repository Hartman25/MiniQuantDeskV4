use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::state::{AppState, RuntimeLifecycleError};

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
}

#[derive(Debug, Deserialize)]
pub struct RestartRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RestartResponse {
    pub restart_id: String,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/control/status", get(status))
        .route("/control/disarm", post(disarm))
        .route("/control/arm", post(arm))
        .route("/control/restart", post(restart))
        .with_state(state)
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

    {
        let mut integrity = state.integrity.write().await;
        integrity.disarmed = true;
    }

    publish_integrity_status(&state, false, "control: desired_armed=false").await;
    StatusCode::NO_CONTENT.into_response()
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

    {
        let mut integrity = state.integrity.write().await;
        integrity.disarmed = false;
        integrity.halted = false;
    }

    publish_integrity_status(&state, true, "control: desired_armed=true").await;
    StatusCode::NO_CONTENT.into_response()
}

async fn restart(State(state): State<Arc<AppState>>, Json(req): Json<RestartRequest>) -> Response {
    let Some(db) = state.db.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "control DB is not configured on this daemon",
        )
            .into_response();
    };

    let restart_id = format!(
        "restart-{}-{}",
        state.node_id,
        chrono::Utc::now().timestamp_micros()
    );

    if let Err(err) = sqlx::query(
        r#"
        INSERT INTO runtime_restart_requests (restart_id, requested_by, requested_at, reason)
        VALUES ($1, $2, now(), $3)
        "#,
    )
    .bind(&restart_id)
    .bind(&state.node_id)
    .bind(req.reason.as_deref())
    .execute(db)
    .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("control/restart write failed: {err}"),
        )
            .into_response();
    }

    (StatusCode::ACCEPTED, Json(RestartResponse { restart_id })).into_response()
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
