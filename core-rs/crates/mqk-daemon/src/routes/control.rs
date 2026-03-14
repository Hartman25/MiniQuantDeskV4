use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

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
    state::{AppState, RestartTruthSnapshot, RuntimeLifecycleError},
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
        .route(
            "/api/v1/audit/operator-actions",
            get(operator_actions_audit),
        )
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
    audit_event_id: Option<Uuid>,
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

#[derive(Debug, Serialize)]
struct OperatorActionAuditRow {
    audit_ref: String,
    at: String,
    actor: String,
    action_key: String,
    environment: String,
    target_scope: String,
    result_state: String,
    warnings: Vec<String>,
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
            desired_armed,
            leader_holder_id: Some(holder_id),
            leader_epoch: Some(epoch),
            lease_expires_at_utc: Some(lease_expires_at.to_rfc3339()),
            lease_expired: Some(lease_expires_at <= now_utc),
            active_run_id: runtime_status.active_run_id,
            run_state: runtime_status.state.clone(),
            run_owned_locally: run_owned_locally,
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
            run_owned_locally: run_owned_locally,
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

    let audit_event_id = match persist_operator_action_audit(db, "control.disarm").await {
        Ok(id) => id,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("control/disarm audit persistence failed: {err}"),
            )
                .into_response();
        }
    };

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
            Some(audit_event_id),
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

    let audit_event_id = match persist_operator_action_audit(db, "control.arm").await {
        Ok(id) => id,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("control/arm audit persistence failed: {err}"),
            )
                .into_response();
        }
    };

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
            Some(audit_event_id),
        )),
    )
        .into_response()
}

async fn operator_actions_audit(State(state): State<Arc<AppState>>) -> Response {
    let Some(db) = state.db.as_ref() else {
        return (StatusCode::OK, Json(Vec::<OperatorActionAuditRow>::new())).into_response();
    };

    let rows: Vec<(Uuid, DateTime<Utc>, serde_json::Value)> = match sqlx::query_as(
        r#"
        SELECT event_id, ts_utc, payload
          FROM audit_events
         WHERE topic = 'operator_action'
         ORDER BY ts_utc DESC
         LIMIT 200
        "#,
    )
    .fetch_all(db)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("audit/operator-actions query failed: {err}"),
            )
                .into_response();
        }
    };

    let environment = std::env::var("MQK_ENV").unwrap_or_else(|_| "paper".to_string());
    let body: Vec<OperatorActionAuditRow> = rows
        .into_iter()
        .map(|(event_id, ts_utc, payload)| OperatorActionAuditRow {
            audit_ref: event_id.to_string(),
            at: ts_utc.to_rfc3339(),
            actor: payload
                .get("actor")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("operator")
                .to_string(),
            action_key: payload
                .get("requested_action")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            environment: environment.clone(),
            target_scope: "daemon_instance".to_string(),
            result_state: payload
                .get("disposition")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            warnings: payload
                .get("warnings")
                .and_then(serde_json::Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect();

    (StatusCode::OK, Json(body)).into_response()
}

async fn restart(State(state): State<Arc<AppState>>) -> Response {
    let restart_truth = match state.restart_truth_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => {
            return lifecycle_error_response(err);
        }
    };

    restart_not_authoritative_response(restart_truth)
}

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

async fn persist_operator_action_audit(
    db: &sqlx::PgPool,
    requested_action: &str,
) -> anyhow::Result<Uuid> {
    let run = mqk_db::fetch_latest_run_for_engine(db, "mqk-daemon", "PAPER").await?;
    let run_id = if let Some(run) = run {
        run.run_id
    } else {
        let run_id = Uuid::new_v4();
        mqk_db::insert_run(
            db,
            &mqk_db::NewRun {
                run_id,
                engine_id: "mqk-daemon".to_string(),
                mode: "PAPER".to_string(),
                started_at_utc: Utc::now(),
                git_hash: "daemon-control-audit".to_string(),
                config_hash: "daemon-control-audit".to_string(),
                config_json: json!({"source":"control.operator_action"}),
                host_fingerprint: "daemon-control".to_string(),
            },
        )
        .await?;
        run_id
    };

    let event_id = Uuid::new_v4();
    mqk_db::insert_audit_event(
        db,
        &mqk_db::NewAuditEvent {
            event_id,
            run_id,
            ts_utc: Utc::now(),
            topic: "operator_action".to_string(),
            event_type: requested_action.to_string(),
            payload: json!({
                "actor": "operator",
                "requested_action": requested_action,
                "accepted": true,
                "disposition": "applied",
                "warnings": [],
            }),
            hash_prev: None,
            hash_self: None,
        },
    )
    .await?;

    Ok(event_id)
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
