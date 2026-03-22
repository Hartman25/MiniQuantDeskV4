use std::sync::Arc;

use chrono::{DateTime, Utc};

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;

use crate::{
    api_types::{OperatorActionAuditFields, OperatorActionResponse, RuntimeErrorResponse},
    notify::OperatorNotifyPayload,
    state::{AppState, RestartTruthSnapshot, RuntimeLifecycleError},
};

#[derive(Debug, Serialize)]
pub struct ControlStatus {
    pub daemon_mode: String,
    pub adapter_id: String,
    pub deployment_start_allowed: bool,
    pub deployment_blocker: Option<String>,
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
}

#[allow(clippy::too_many_arguments)]
fn operator_action_response(
    requested_action: &str,
    accepted: bool,
    disposition: &str,
    resulting_integrity_state: Option<&str>,
    resulting_desired_armed: Option<bool>,
    blockers: Vec<String>,
    warnings: Vec<String>,
    durable_targets: Vec<String>,
    audit_event_id: Option<uuid::Uuid>,
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
            audit_event_id: audit_event_id.map(|id| id.to_string()),
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
    let local_owned_run_id = state.locally_owned_run_id().await;
    let run_owned_locally = local_owned_run_id
        .zip(runtime_status.active_run_id)
        .is_some_and(|(local, active)| local == active);
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

    let now_utc = control_plane_now_utc();
    let lease_row: Option<(String, i64, DateTime<Utc>)> = match sqlx::query_as(
        r#"
            SELECT holder_id,
                   epoch,
                   lease_expires_at
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
        Some((holder_id, epoch, lease_expires_at)) => ControlStatus {
            daemon_mode: state.deployment_mode().as_api_label().to_string(),
            adapter_id: state.adapter_id().to_string(),
            deployment_start_allowed: state.deployment_readiness().start_allowed,
            deployment_blocker: state.deployment_readiness().blocker.clone(),
            desired_armed,
            leader_holder_id: Some(holder_id),
            leader_epoch: Some(epoch),
            lease_expires_at_utc: Some(lease_expires_at.to_rfc3339()),
            lease_expired: Some(lease_expires_at <= now_utc),
            active_run_id: runtime_status.active_run_id,
            run_state: runtime_status.state.clone(),
            run_owned_locally,
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
            daemon_mode: state.deployment_mode().as_api_label().to_string(),
            adapter_id: state.adapter_id().to_string(),
            deployment_start_allowed: state.deployment_readiness().start_allowed,
            deployment_blocker: state.deployment_readiness().blocker.clone(),
            desired_armed,
            leader_holder_id: None,
            leader_epoch: None,
            lease_expires_at_utc: None,
            lease_expired: None,
            active_run_id: runtime_status.active_run_id,
            run_state: runtime_status.state.clone(),
            run_owned_locally,
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
    if let Err(err) = mqk_db::persist_arm_state_canonical(
        db,
        mqk_db::ArmState::Disarmed,
        Some(mqk_db::DisarmReason::OperatorDisarm),
    )
    .await
    {
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
    let audit_event_id =
        match write_control_operator_audit_event(&state, "control.disarm", "DISARMED").await {
            Ok(id) => id,
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("control/disarm audit persistence failed: {err}"),
                )
                    .into_response();
            }
        };
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
            audit_event_id,
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
    if let Err(err) = mqk_db::persist_arm_state_canonical(db, mqk_db::ArmState::Armed, None).await {
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
    let audit_event_id =
        match write_control_operator_audit_event(&state, "control.arm", "ARMED").await {
            Ok(id) => id,
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("control/arm audit persistence failed: {err}"),
                )
                    .into_response();
            }
        };
    state
        .discord_notifier
        .notify_operator_action(&OperatorNotifyPayload {
            action_key: "control.arm".to_string(),
            disposition: "applied".to_string(),
            environment: Some(state.deployment_mode().as_api_label().to_string()),
            ts_utc: Utc::now().to_rfc3339(),
            provenance_ref: audit_event_id.map(|id| format!("audit_events:{}", id)),
            run_id: None,
        })
        .await;
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
            audit_event_id,
        )),
    )
        .into_response()
}

#[allow(dead_code)]
async fn restart(State(state): State<Arc<AppState>>) -> Response {
    let restart_truth = match state.restart_truth_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => {
            return lifecycle_error_response(err);
        }
    };

    restart_not_authoritative_response(restart_truth)
}

#[allow(dead_code)]
fn restart_not_authoritative_response(restart_truth: RestartTruthSnapshot) -> Response {
    let conflict_note = if restart_truth.durable_active_without_local_ownership {
        "durable active run exists without local runtime ownership; restart would overstate authority"
    } else if restart_truth.local_owned_run_id.is_some() {
        "local runtime is active but restart authority is not yet durable/proven"
    } else {
        "no active local runtime; restart intent is not authoritative"
    };

    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": "GATE_REFUSED: /control/restart is disabled because daemon-owned restart semantics are not authoritative yet",
            "gate": "restart_not_authoritative",
            "restart_authority": "not_authoritative",
            "requested_action": "restart",
            "achieved_action": "none",
            "control_truth": {
                "local_owned_run_id": restart_truth.local_owned_run_id,
                "durable_active_run_id": restart_truth.durable_active_run_id,
                "durable_active_without_local_ownership": restart_truth.durable_active_without_local_ownership,
                "note": conflict_note,
            }
        })),
    )
        .into_response()
}

async fn write_desired_armed(db: &sqlx::PgPool, desired_armed: bool) -> anyhow::Result<()> {
    let updated_at_utc = control_plane_now_utc();
    sqlx::query(
        r#"
        INSERT INTO runtime_control_state (id, desired_armed, updated_at)
        VALUES (1, $1, $2)
        ON CONFLICT (id) DO UPDATE
           SET desired_armed = excluded.desired_armed,
               updated_at    = excluded.updated_at
        "#,
    )
    .bind(desired_armed)
    .bind(updated_at_utc)
    .execute(db)
    .await?;

    Ok(())
}

fn control_plane_now_utc() -> DateTime<Utc> {
    Utc::now()
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

async fn write_control_operator_audit_event(
    state: &Arc<AppState>,
    event_type: &str,
    runtime_transition: &str,
) -> anyhow::Result<Option<uuid::Uuid>> {
    let Some(db) = state.db.as_ref() else {
        return Ok(None);
    };

    // IR-01: resolve run_id from real run only; no synthetic run creation.
    // If no active run and no durable run exist, return Ok(None) so the caller
    // represents the audit event as absent rather than anchored to a fake row.
    let run_id = if let Some(run_id) = state
        .current_status_snapshot()
        .await
        .ok()
        .and_then(|s| s.active_run_id)
    {
        run_id
    } else if let Some(run) =
        mqk_db::fetch_latest_run_for_engine(db, "mqk-daemon", state.deployment_mode().as_db_mode())
            .await?
    {
        run.run_id
    } else {
        // IR-01: no real run anchor exists; refuse durable audit write.
        // The arm/disarm primary writes already completed above; this path
        // controls only the secondary audit-event row.  Returning None is
        // honest: audit_event_id will be null in the response contract,
        // signalling that no durable audit record was created.
        return Ok(None);
    };

    // D1 — event_id is UUIDv5 derived from (run_id, event_type, ts_utc).
    // The wall-clock boundary is here and both ts_utc and event_id share it,
    // so there is no second-independent clock drift between ID and timestamp.
    let ts_utc = chrono::Utc::now();
    let event_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!(
            "mqk-daemon.control-audit.v1|{}|{}|{}",
            run_id,
            event_type,
            ts_utc.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        )
        .as_bytes(),
    );
    mqk_db::insert_audit_event(
        db,
        &mqk_db::NewAuditEvent {
            event_id,
            run_id,
            ts_utc,
            topic: "operator".to_string(),
            event_type: event_type.to_string(),
            payload: serde_json::json!({
                "requested_action": event_type,
                "accepted": true,
                "disposition": "applied",
                "runtime_transition": runtime_transition,
                "warnings": [],
                "source": "mqk-daemon.routes.control",
            }),
            hash_prev: None,
            hash_self: None,
        },
    )
    .await?;

    Ok(Some(event_id))
}

fn lifecycle_error_response(err: RuntimeLifecycleError) -> Response {
    match err {
        RuntimeLifecycleError::Forbidden {
            fault_class,
            gate,
            message,
        } => (
            StatusCode::FORBIDDEN,
            Json(RuntimeErrorResponse {
                error: message,
                fault_class: fault_class.to_string(),
                gate: Some(gate),
            }),
        )
            .into_response(),
        RuntimeLifecycleError::ServiceUnavailable {
            fault_class,
            message,
        } => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(RuntimeErrorResponse {
                error: message,
                fault_class: fault_class.to_string(),
                gate: None,
            }),
        )
            .into_response(),
        RuntimeLifecycleError::Conflict {
            fault_class,
            message,
        } => (
            StatusCode::CONFLICT,
            Json(RuntimeErrorResponse {
                error: message,
                fault_class: fault_class.to_string(),
                gate: None,
            }),
        )
            .into_response(),
        RuntimeLifecycleError::Internal {
            fault_class,
            message,
        } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(RuntimeErrorResponse {
                error: message,
                fault_class: fault_class.to_string(),
                gate: None,
            }),
        )
            .into_response(),
    }
}
