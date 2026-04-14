//! Control-plane route handlers.
//!
//! Contains: run_start, run_stop, run_halt (via run_lifecycle), integrity_arm,
//! integrity_disarm, ops_action, ops_catalog, ops_mode_change_guidance,
//! build_mode_change_guidance.

// MT-07C: run lifecycle handlers extracted to reduce file size.
mod run_lifecycle;
pub(crate) use run_lifecycle::{run_halt, run_start, run_stop};

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use tracing::info;

use crate::api_types::{
    ActionCatalogEntry, ActionCatalogResponse, IntegrityResponse, ModeChangeGuidanceResponse,
    ModeChangeRestartTruth, ModeTransitionEntry, OperatorActionAuditFields, OperatorActionResponse,
    OpsActionRequest, PendingRestartIntentSnapshot, RestartWorkflowTruth,
};
use crate::mode_transition::{evaluate_mode_transition, ModeTransitionVerdict};
use crate::notify::{CriticalAlertPayload, OperatorNotifyPayload, RunStatusPayload};
use crate::parity_evidence::{evaluate_parity_evidence_guarded, ParityEvidenceOutcome};
use crate::state::DeploymentMode;
use crate::state::{AppState, BusMsg, RuntimeLifecycleError, DAEMON_ENGINE_ID};

use super::helpers::{runtime_error_response, write_operator_audit_event};

// ---------------------------------------------------------------------------
// POST /v1/integrity/arm
// ---------------------------------------------------------------------------

pub(crate) async fn integrity_arm(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = false;
        ig.halted = false;
    }

    if let Some(db) = st.db.as_ref() {
        if let Err(err) =
            mqk_db::persist_arm_state_canonical(db, mqk_db::ArmState::Armed, None).await
        {
            return runtime_error_response(RuntimeLifecycleError::Internal {
                fault_class: "control.persistence.integrity_arm",
                message: format!("integrity/arm persist_arm_state failed: {err}"),
            });
        }
    }

    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return runtime_error_response(err),
    };

    info!("integrity/arm");
    let _ = st.bus.send(BusMsg::LogLine {
        level: "INFO".to_string(),
        msg: "integrity armed".to_string(),
    });

    // LO-03G: Write durable operator audit event for arm action.
    // Non-fatal: audit write failure does not block the arm.
    let arm_audit_uuid =
        write_operator_audit_event(&st, status.active_run_id, "control.arm", "ARMED")
            .await
            .ok()
            .flatten();

    st.discord_notifier
        .notify_operator_action(&OperatorNotifyPayload {
            action_key: "control.arm".to_string(),
            disposition: "applied".to_string(),
            environment: Some(st.deployment_mode().as_api_label().to_string()),
            ts_utc: Utc::now().to_rfc3339(),
            provenance_ref: arm_audit_uuid
                .map(|id| format!("audit_events:{}", id))
                .or_else(|| {
                    if st.db.is_some() {
                        Some("sys_arm_state".to_string())
                    } else {
                        None
                    }
                }),
            run_id: status.active_run_id.map(|id| id.to_string()),
        })
        .await;

    (
        StatusCode::OK,
        Json(IntegrityResponse {
            armed: true,
            active_run_id: status.active_run_id,
            state: status.state,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /v1/integrity/disarm
// ---------------------------------------------------------------------------

pub(crate) async fn integrity_disarm(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    {
        let mut ig = st.integrity.write().await;
        ig.disarmed = true;
    }

    if let Some(db) = st.db.as_ref() {
        if let Err(err) = mqk_db::persist_arm_state_canonical(
            db,
            mqk_db::ArmState::Disarmed,
            Some(mqk_db::DisarmReason::OperatorDisarm),
        )
        .await
        {
            return runtime_error_response(RuntimeLifecycleError::Internal {
                fault_class: "control.persistence.integrity_disarm",
                message: format!("integrity/disarm persist_arm_state failed: {err}"),
            });
        }
    }

    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return runtime_error_response(err),
    };

    info!("integrity/disarm");
    let _ = st.bus.send(BusMsg::LogLine {
        level: "WARN".to_string(),
        msg: "integrity DISARMED".to_string(),
    });

    // LO-03G: Write durable operator audit event for disarm action.
    // Non-fatal: audit write failure does not block the disarm.
    let disarm_audit_uuid =
        write_operator_audit_event(&st, status.active_run_id, "control.disarm", "DISARMED")
            .await
            .ok()
            .flatten();

    st.discord_notifier
        .notify_operator_action(&OperatorNotifyPayload {
            action_key: "control.disarm".to_string(),
            disposition: "applied".to_string(),
            environment: Some(st.deployment_mode().as_api_label().to_string()),
            ts_utc: Utc::now().to_rfc3339(),
            provenance_ref: disarm_audit_uuid
                .map(|id| format!("audit_events:{}", id))
                .or_else(|| {
                    if st.db.is_some() {
                        Some("sys_arm_state".to_string())
                    } else {
                        None
                    }
                }),
            run_id: status.active_run_id.map(|id| id.to_string()),
        })
        .await;

    (
        StatusCode::OK,
        Json(IntegrityResponse {
            armed: false,
            active_run_id: status.active_run_id,
            state: status.state,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a deployment-mode API label string to a typed [`DeploymentMode`].
///
/// Returns `None` for any unrecognised string so the caller can return a
/// structured 400 rather than panicking.
fn parse_deployment_mode(s: &str) -> Option<DeploymentMode> {
    match s {
        "paper" => Some(DeploymentMode::Paper),
        "live-shadow" => Some(DeploymentMode::LiveShadow),
        "live-capital" => Some(DeploymentMode::LiveCapital),
        "backtest" => Some(DeploymentMode::Backtest),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// POST /api/v1/ops/action
// ---------------------------------------------------------------------------

pub(crate) async fn ops_action(
    State(st): State<Arc<AppState>>,
    Json(body): Json<OpsActionRequest>,
) -> Response {
    let action_key = body.action_key.as_str();

    match action_key {
        "arm-execution" | "arm-strategy" => {
            {
                let mut ig = st.integrity.write().await;
                ig.disarmed = false;
                ig.halted = false;
            }
            if let Some(db) = st.db.as_ref() {
                if let Err(err) =
                    mqk_db::persist_arm_state_canonical(db, mqk_db::ArmState::Armed, None).await
                {
                    return runtime_error_response(RuntimeLifecycleError::Internal {
                        fault_class: "ops.action.arm",
                        message: format!("ops/action arm persist failed: {err}"),
                    });
                }
            }
            info!("ops/action arm");
            let _ = st.bus.send(BusMsg::LogLine {
                level: "INFO".to_string(),
                msg: "ops/action: integrity armed".to_string(),
            });
            // LO-03G: Write durable operator audit event for arm action.
            // Non-fatal: audit write failure does not block the arm.
            let arm_run_id = st.locally_owned_run_id().await;
            let arm_audit_uuid =
                write_operator_audit_event(&st, arm_run_id, "control.arm", "ARMED")
                    .await
                    .ok()
                    .flatten();
            let mut arm_durable_targets = if st.db.is_some() {
                vec!["sys_arm_state".to_string()]
            } else {
                vec![]
            };
            if arm_audit_uuid.is_some() {
                arm_durable_targets.push("audit_events".to_string());
            }
            let response = OperatorActionResponse {
                requested_action: "control.arm".to_string(),
                accepted: true,
                disposition: "applied".to_string(),
                resulting_integrity_state: Some("ARMED".to_string()),
                resulting_desired_armed: Some(true),
                blockers: vec![],
                warnings: vec![],
                environment: Some(st.deployment_mode().as_api_label().to_string()),
                scope: Some("daemon_instance".to_string()),
                audit: OperatorActionAuditFields {
                    durable_db_write: st.db.is_some(),
                    durable_targets: arm_durable_targets,
                    audit_event_id: arm_audit_uuid.map(|id| id.to_string()),
                },
                pending_restart_intent: None,
            };
            st.discord_notifier
                .notify_operator_action(&OperatorNotifyPayload {
                    action_key: "control.arm".to_string(),
                    disposition: "applied".to_string(),
                    environment: response.environment.clone(),
                    ts_utc: Utc::now().to_rfc3339(),
                    provenance_ref: arm_audit_uuid
                        .map(|id| format!("audit_events:{}", id))
                        .or_else(|| {
                            if st.db.is_some() {
                                Some("sys_arm_state".to_string())
                            } else {
                                None
                            }
                        }),
                    run_id: arm_run_id.map(|id| id.to_string()),
                })
                .await;
            (StatusCode::OK, Json(response)).into_response()
        }

        "disarm-execution" | "disarm-strategy" => {
            {
                let mut ig = st.integrity.write().await;
                ig.disarmed = true;
            }
            if let Some(db) = st.db.as_ref() {
                if let Err(err) = mqk_db::persist_arm_state_canonical(
                    db,
                    mqk_db::ArmState::Disarmed,
                    Some(mqk_db::DisarmReason::OperatorDisarm),
                )
                .await
                {
                    return runtime_error_response(RuntimeLifecycleError::Internal {
                        fault_class: "ops.action.disarm",
                        message: format!("ops/action disarm persist failed: {err}"),
                    });
                }
            }
            info!("ops/action disarm");
            let _ = st.bus.send(BusMsg::LogLine {
                level: "WARN".to_string(),
                msg: "ops/action: integrity DISARMED".to_string(),
            });
            // LO-03G: Write durable operator audit event for disarm action.
            // Non-fatal: audit write failure does not block the disarm.
            let disarm_run_id = st.locally_owned_run_id().await;
            let disarm_audit_uuid =
                write_operator_audit_event(&st, disarm_run_id, "control.disarm", "DISARMED")
                    .await
                    .ok()
                    .flatten();
            let mut disarm_durable_targets = if st.db.is_some() {
                vec!["sys_arm_state".to_string()]
            } else {
                vec![]
            };
            if disarm_audit_uuid.is_some() {
                disarm_durable_targets.push("audit_events".to_string());
            }
            let response = OperatorActionResponse {
                requested_action: "control.disarm".to_string(),
                accepted: true,
                disposition: "applied".to_string(),
                resulting_integrity_state: Some("DISARMED".to_string()),
                resulting_desired_armed: Some(false),
                blockers: vec![],
                warnings: vec![],
                environment: Some(st.deployment_mode().as_api_label().to_string()),
                scope: Some("daemon_instance".to_string()),
                audit: OperatorActionAuditFields {
                    durable_db_write: st.db.is_some(),
                    durable_targets: disarm_durable_targets,
                    audit_event_id: disarm_audit_uuid.map(|id| id.to_string()),
                },
                pending_restart_intent: None,
            };
            st.discord_notifier
                .notify_operator_action(&OperatorNotifyPayload {
                    action_key: "control.disarm".to_string(),
                    disposition: "applied".to_string(),
                    environment: response.environment.clone(),
                    ts_utc: Utc::now().to_rfc3339(),
                    provenance_ref: disarm_audit_uuid
                        .map(|id| format!("audit_events:{}", id))
                        .or_else(|| {
                            if st.db.is_some() {
                                Some("sys_arm_state".to_string())
                            } else {
                                None
                            }
                        }),
                    run_id: disarm_run_id.map(|id| id.to_string()),
                })
                .await;
            (StatusCode::OK, Json(response)).into_response()
        }

        "start-system" => match st.start_execution_runtime().await {
            Ok(snapshot) => {
                info!("ops/action start-system");
                let audit_uuid = if let Some(run_id) = snapshot.active_run_id {
                    write_operator_audit_event(&st, Some(run_id), "run.start", "RUNNING")
                        .await
                        .ok()
                        .flatten()
                } else {
                    None
                };
                let response = OperatorActionResponse {
                    requested_action: "start-system".to_string(),
                    accepted: true,
                    disposition: "applied".to_string(),
                    resulting_integrity_state: None,
                    resulting_desired_armed: None,
                    blockers: vec![],
                    warnings: vec![],
                    environment: Some(st.deployment_mode().as_api_label().to_string()),
                    scope: Some("daemon_instance".to_string()),
                    audit: OperatorActionAuditFields {
                        durable_db_write: st.db.is_some(),
                        durable_targets: if st.db.is_some() {
                            vec!["audit_events".to_string()]
                        } else {
                            vec![]
                        },
                        audit_event_id: None,
                    },
                    pending_restart_intent: None,
                };
                let ts = Utc::now().to_rfc3339();
                let run_id_str = snapshot.active_run_id.map(|id| id.to_string());
                st.discord_notifier
                    .notify_operator_action(&OperatorNotifyPayload {
                        action_key: "run.start".to_string(),
                        disposition: "applied".to_string(),
                        environment: response.environment.clone(),
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
                        environment: response.environment.clone(),
                        note: None,
                        ts_utc: ts,
                    })
                    .await;
                (StatusCode::OK, Json(response)).into_response()
            }
            Err(err) => runtime_error_response(err),
        },

        "stop-system" => match st.stop_execution_runtime().await {
            Ok(snapshot) => {
                info!("ops/action stop-system");
                let audit_uuid =
                    write_operator_audit_event(&st, snapshot.active_run_id, "run.stop", "STOPPED")
                        .await
                        .ok()
                        .flatten();
                let response = OperatorActionResponse {
                    requested_action: "stop-system".to_string(),
                    accepted: true,
                    disposition: "applied".to_string(),
                    resulting_integrity_state: None,
                    resulting_desired_armed: None,
                    blockers: vec![],
                    warnings: vec![],
                    environment: Some(st.deployment_mode().as_api_label().to_string()),
                    scope: Some("daemon_instance".to_string()),
                    audit: OperatorActionAuditFields {
                        durable_db_write: st.db.is_some(),
                        durable_targets: if st.db.is_some() {
                            vec!["audit_events".to_string()]
                        } else {
                            vec![]
                        },
                        audit_event_id: None,
                    },
                    pending_restart_intent: None,
                };
                let ts = Utc::now().to_rfc3339();
                let run_id_str = snapshot.active_run_id.map(|id| id.to_string());
                st.discord_notifier
                    .notify_operator_action(&OperatorNotifyPayload {
                        action_key: "run.stop".to_string(),
                        disposition: "applied".to_string(),
                        environment: response.environment.clone(),
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
                        environment: response.environment.clone(),
                        note: None,
                        ts_utc: ts,
                    })
                    .await;
                (StatusCode::OK, Json(response)).into_response()
            }
            Err(err) => runtime_error_response(err),
        },

        "kill-switch" => match st.halt_execution_runtime().await {
            Ok(snapshot) => {
                info!("ops/action kill-switch");
                let audit_uuid =
                    write_operator_audit_event(&st, snapshot.active_run_id, "run.halt", "HALTED")
                        .await
                        .ok()
                        .flatten();
                let response = OperatorActionResponse {
                    requested_action: "kill-switch".to_string(),
                    accepted: true,
                    disposition: "applied".to_string(),
                    resulting_integrity_state: None,
                    resulting_desired_armed: None,
                    blockers: vec![],
                    warnings: vec![],
                    environment: Some(st.deployment_mode().as_api_label().to_string()),
                    scope: Some("daemon_instance".to_string()),
                    audit: OperatorActionAuditFields {
                        durable_db_write: st.db.is_some(),
                        durable_targets: if st.db.is_some() {
                            vec!["audit_events".to_string()]
                        } else {
                            vec![]
                        },
                        audit_event_id: None,
                    },
                    pending_restart_intent: None,
                };
                let ts = Utc::now().to_rfc3339();
                let run_id_str = snapshot.active_run_id.map(|id| id.to_string());
                st.discord_notifier
                    .notify_operator_action(&OperatorNotifyPayload {
                        action_key: "run.halt".to_string(),
                        disposition: "applied".to_string(),
                        environment: response.environment.clone(),
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
                        summary: "Runtime halted via kill-switch; dispatch is fail-closed."
                            .to_string(),
                        detail: None,
                        environment: response.environment.clone(),
                        run_id: run_id_str.clone(),
                        ts_utc: ts.clone(),
                    })
                    .await;
                // DIS-02: structured run lifecycle summary.
                st.discord_notifier
                    .notify_run_status(&RunStatusPayload {
                        event: "run.halted".to_string(),
                        run_id: run_id_str,
                        environment: response.environment.clone(),
                        note: Some("kill-switch; dispatch fail-closed".to_string()),
                        ts_utc: ts,
                    })
                    .await;
                (StatusCode::OK, Json(response)).into_response()
            }
            Err(err) => runtime_error_response(err),
        },

        // OPS-CONTROL-01: Persisted restart-intent workflow.
        //
        // Evaluates the requested (current → target) mode transition via the
        // canonical seam.  Persists a durable restart intent to sys_restart_intent
        // when the transition is admissible_with_restart.  Returns 409 for refused
        // and fail_closed verdicts — no intent is written.
        "request-mode-change" => {
            let Some(target_str) = body.target_mode.as_deref() else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(OperatorActionResponse {
                        requested_action: "request-mode-change".to_string(),
                        accepted: false,
                        disposition: "missing_target_mode".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec!["target_mode is required for request-mode-change \
                             (e.g. \"live-shadow\"); see GET /api/v1/ops/mode-change-guidance \
                             for available transitions."
                            .to_string()],
                        warnings: vec![],
                        environment: Some(st.deployment_mode().as_api_label().to_string()),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                    }),
                )
                    .into_response();
            };

            let Some(target) = parse_deployment_mode(target_str) else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(OperatorActionResponse {
                        requested_action: "request-mode-change".to_string(),
                        accepted: false,
                        disposition: "invalid_target_mode".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![format!(
                            "Unknown target_mode '{}'; valid values: \
                             paper, live-shadow, live-capital, backtest.",
                            target_str
                        )],
                        warnings: vec![],
                        environment: Some(st.deployment_mode().as_api_label().to_string()),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                    }),
                )
                    .into_response();
            };

            let current = st.deployment_mode();
            let verdict = evaluate_mode_transition(current, target);

            match &verdict {
                ModeTransitionVerdict::SameMode => (
                    StatusCode::OK,
                    Json(OperatorActionResponse {
                        requested_action: "request-mode-change".to_string(),
                        accepted: true,
                        disposition: "no_op".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![],
                        warnings: vec![format!(
                            "Current mode is already '{}'; no transition is needed.",
                            current.as_api_label()
                        )],
                        environment: Some(current.as_api_label().to_string()),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                    }),
                )
                    .into_response(),

                ModeTransitionVerdict::AdmissibleWithRestart { .. } => {
                    let Some(db) = st.db.as_ref() else {
                        return runtime_error_response(RuntimeLifecycleError::ServiceUnavailable {
                            fault_class: "ops.action.request_mode_change.no_db",
                            message: "DB is required to persist restart intent durably; \
                                      request-mode-change is not accepted without a DB connection."
                                .to_string(),
                        });
                    };

                    let ts_utc = chrono::Utc::now();
                    let intent_id = uuid::Uuid::new_v5(
                        &uuid::Uuid::NAMESPACE_DNS,
                        format!(
                            "mqk-daemon.restart-intent.v1|{}|{}|{}|{}",
                            DAEMON_ENGINE_ID,
                            current.as_api_label(),
                            target.as_api_label(),
                            ts_utc.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
                        )
                        .as_bytes(),
                    );

                    let note = body.reason.clone().unwrap_or_default();

                    if let Err(err) = mqk_db::insert_restart_intent(
                        db,
                        &mqk_db::NewRestartIntent {
                            intent_id,
                            engine_id: DAEMON_ENGINE_ID.to_string(),
                            from_mode: current.as_api_label().to_string(),
                            to_mode: target.as_api_label().to_string(),
                            transition_verdict: verdict.as_str().to_string(),
                            initiated_by: "operator".to_string(),
                            initiated_at_utc: ts_utc,
                            note: note.clone(),
                        },
                    )
                    .await
                    {
                        return runtime_error_response(RuntimeLifecycleError::Internal {
                            fault_class: "ops.action.request_mode_change.insert",
                            message: format!("Failed to persist restart intent: {err}"),
                        });
                    }

                    info!(
                        intent_id = %intent_id,
                        from = current.as_api_label(),
                        to = target.as_api_label(),
                        "ops/action request-mode-change: restart intent persisted"
                    );

                    let intent_snapshot = PendingRestartIntentSnapshot {
                        intent_id: intent_id.to_string(),
                        from_mode: current.as_api_label().to_string(),
                        to_mode: target.as_api_label().to_string(),
                        transition_verdict: verdict.as_str().to_string(),
                        initiated_by: "operator".to_string(),
                        initiated_at_utc: ts_utc.to_rfc3339(),
                        note,
                    };

                    (
                        StatusCode::OK,
                        Json(OperatorActionResponse {
                            requested_action: "request-mode-change".to_string(),
                            accepted: true,
                            disposition: "pending_restart".to_string(),
                            resulting_integrity_state: None,
                            resulting_desired_armed: None,
                            blockers: vec![],
                            warnings: vec!["Mode change requires a controlled daemon restart. \
                                 See GET /api/v1/ops/mode-change-guidance for preconditions."
                                .to_string()],
                            environment: Some(current.as_api_label().to_string()),
                            scope: Some("daemon_instance".to_string()),
                            audit: OperatorActionAuditFields {
                                durable_db_write: true,
                                durable_targets: vec!["sys_restart_intent".to_string()],
                                audit_event_id: Some(intent_id.to_string()),
                            },
                            pending_restart_intent: Some(intent_snapshot),
                        }),
                    )
                        .into_response()
                }

                ModeTransitionVerdict::Refused { .. }
                | ModeTransitionVerdict::FailClosed { .. } => (
                    StatusCode::CONFLICT,
                    Json(OperatorActionResponse {
                        requested_action: "request-mode-change".to_string(),
                        accepted: false,
                        disposition: format!("blocked_{}", verdict.as_str()),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![verdict.reason().to_string()],
                        warnings: vec![],
                        environment: Some(current.as_api_label().to_string()),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                    }),
                )
                    .into_response(),
            }
        }

        // OPS-CONTROL-01: Cancel a pending restart intent.
        //
        // Transitions the most recent pending sys_restart_intent record for this
        // engine to "cancelled".  Fails closed when no DB is available or when
        // no pending intent exists.
        "cancel-mode-transition" => {
            let Some(db) = st.db.as_ref() else {
                return runtime_error_response(RuntimeLifecycleError::ServiceUnavailable {
                    fault_class: "ops.action.cancel_mode_transition.no_db",
                    message: "DB is required to manage restart intents; \
                              cancel-mode-transition is not accepted without a DB connection."
                        .to_string(),
                });
            };

            match mqk_db::fetch_pending_restart_intent_for_engine(db, DAEMON_ENGINE_ID).await {
                Err(err) => runtime_error_response(RuntimeLifecycleError::Internal {
                    fault_class: "ops.action.cancel_mode_transition.fetch",
                    message: format!("Failed to fetch pending restart intent: {err}"),
                }),

                Ok(None) => (
                    StatusCode::CONFLICT,
                    Json(OperatorActionResponse {
                        requested_action: "cancel-mode-transition".to_string(),
                        accepted: false,
                        disposition: "no_pending_intent".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![
                            "No pending mode-transition intent found; nothing to cancel."
                                .to_string(),
                        ],
                        warnings: vec![],
                        environment: Some(st.deployment_mode().as_api_label().to_string()),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                    }),
                )
                    .into_response(),

                Ok(Some(intent)) => {
                    let cancel_ts = chrono::Utc::now();
                    match mqk_db::update_restart_intent_status(
                        db,
                        intent.intent_id,
                        "cancelled",
                        cancel_ts,
                    )
                    .await
                    {
                        Err(err) => runtime_error_response(RuntimeLifecycleError::Internal {
                            fault_class: "ops.action.cancel_mode_transition.update",
                            message: format!("Failed to cancel restart intent: {err}"),
                        }),

                        Ok(_) => {
                            info!(
                                intent_id = %intent.intent_id,
                                "ops/action cancel-mode-transition: intent cancelled"
                            );
                            (
                                StatusCode::OK,
                                Json(OperatorActionResponse {
                                    requested_action: "cancel-mode-transition".to_string(),
                                    accepted: true,
                                    disposition: "intent_cancelled".to_string(),
                                    resulting_integrity_state: None,
                                    resulting_desired_armed: None,
                                    blockers: vec![],
                                    warnings: vec![],
                                    environment: Some(
                                        st.deployment_mode().as_api_label().to_string(),
                                    ),
                                    scope: Some("daemon_instance".to_string()),
                                    audit: OperatorActionAuditFields {
                                        durable_db_write: true,
                                        durable_targets: vec!["sys_restart_intent".to_string()],
                                        audit_event_id: None,
                                    },
                                    pending_restart_intent: None,
                                }),
                            )
                                .into_response()
                        }
                    }
                }
            }
        }

        "change-system-mode" => {
            let guidance = build_mode_change_guidance(&st).await;
            (StatusCode::CONFLICT, Json(guidance)).into_response()
        }

        _ => (
            StatusCode::BAD_REQUEST,
            Json(OperatorActionResponse {
                requested_action: body.action_key.clone(),
                accepted: false,
                disposition: "unknown_action".to_string(),
                resulting_integrity_state: None,
                resulting_desired_armed: None,
                blockers: vec![format!(
                    "Unknown action_key '{}'; accepted keys: arm-execution, arm-strategy, \
                     disarm-execution, disarm-strategy, start-system, stop-system, kill-switch, \
                     request-mode-change, cancel-mode-transition",
                    body.action_key
                )],
                warnings: vec![],
                environment: Some(st.deployment_mode().as_api_label().to_string()),
                scope: Some("daemon_instance".to_string()),
                audit: OperatorActionAuditFields {
                    durable_db_write: false,
                    durable_targets: vec![],
                    audit_event_id: None,
                },
                pending_restart_intent: None,
            }),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/ops/catalog
// ---------------------------------------------------------------------------

pub(crate) async fn ops_catalog(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let (is_disarmed, is_halted_integrity) = {
        let ig = st.integrity.read().await;
        (ig.disarmed, ig.halted)
    };

    let state_str = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot.state,
        Err(_) => "idle".to_string(),
    };

    let armed = !is_disarmed && !is_halted_integrity;
    let halted = is_halted_integrity || state_str == "halted";
    let running = state_str == "running";
    let idle = state_str == "idle";

    // OPS-CONTROL-02: Check for a pending restart intent so the
    // cancel-mode-transition catalog entry can reflect real availability.
    let has_pending_intent = if let Some(db) = st.db.as_ref() {
        mqk_db::fetch_pending_restart_intent_for_engine(db, DAEMON_ENGINE_ID)
            .await
            .ok()
            .flatten()
            .is_some()
    } else {
        false
    };

    let actions = vec![
        ActionCatalogEntry {
            action_key: "arm-execution".to_string(),
            label: "Arm Execution".to_string(),
            level: 1,
            description:
                "Arm the execution integrity gate. Required before any live order dispatch."
                    .to_string(),
            requires_reason: false,
            confirm_text: "Confirm: arm execution gate".to_string(),
            enabled: !armed && !halted,
            disabled_reason: if armed {
                Some("Execution is already armed.".to_string())
            } else if halted {
                Some("Cannot arm while system is halted.".to_string())
            } else {
                None
            },
        },
        ActionCatalogEntry {
            action_key: "disarm-execution".to_string(),
            label: "Disarm Execution".to_string(),
            level: 1,
            description:
                "Disarm the execution integrity gate. Stops new order dispatch immediately."
                    .to_string(),
            requires_reason: false,
            confirm_text: "Confirm: disarm execution gate".to_string(),
            enabled: armed,
            disabled_reason: if !armed {
                Some("Execution is already disarmed.".to_string())
            } else {
                None
            },
        },
        ActionCatalogEntry {
            action_key: "start-system".to_string(),
            label: "Start System".to_string(),
            level: 1,
            description: "Start the execution runtime. System must be idle to start.".to_string(),
            requires_reason: false,
            confirm_text: "Confirm: start execution runtime".to_string(),
            enabled: idle && !halted,
            disabled_reason: if halted {
                Some("Cannot start while system is halted.".to_string())
            } else if running {
                Some("System is already running.".to_string())
            } else if !idle {
                Some("System must be idle to start.".to_string())
            } else {
                None
            },
        },
        ActionCatalogEntry {
            action_key: "stop-system".to_string(),
            label: "Stop System".to_string(),
            level: 2,
            description:
                "Stop the execution runtime gracefully. Drains pending outbox before halting."
                    .to_string(),
            requires_reason: false,
            confirm_text: "Confirm: stop execution runtime".to_string(),
            enabled: running,
            disabled_reason: if !running {
                Some("System is not currently running.".to_string())
            } else {
                None
            },
        },
        ActionCatalogEntry {
            action_key: "kill-switch".to_string(),
            label: "Kill Switch".to_string(),
            level: 3,
            description:
                "Immediately halt all execution and disarm. Use only in emergency. Requires reason."
                    .to_string(),
            requires_reason: true,
            confirm_text:
                "Type CONFIRM to activate kill switch -- this halts all execution immediately"
                    .to_string(),
            enabled: !halted,
            disabled_reason: if halted {
                Some("System is already halted.".to_string())
            } else {
                None
            },
        },
        // OPS-CONTROL-02: Mode-transition workflow actions.
        ActionCatalogEntry {
            action_key: "request-mode-change".to_string(),
            label: "Request Mode Change".to_string(),
            level: 2,
            description: "Persist a restart intent for an admissible deployment-mode transition. \
                           Requires target_mode in the request body. Check \
                           GET /api/v1/ops/mode-change-guidance for available transitions and \
                           preconditions before submitting."
                .to_string(),
            requires_reason: false,
            confirm_text: "Confirm: persist mode-change restart intent".to_string(),
            enabled: !halted,
            disabled_reason: if halted {
                Some(
                    "Cannot request a mode change while the system is halted. \
                     Halt must be cleared before a restart intent can be persisted."
                        .to_string(),
                )
            } else {
                None
            },
        },
        ActionCatalogEntry {
            action_key: "cancel-mode-transition".to_string(),
            label: "Cancel Mode Transition".to_string(),
            level: 2,
            description: "Cancel the current pending mode-transition restart intent. \
                           Removes the durable intent from sys_restart_intent so the \
                           operator can start fresh or proceed without a pending transition."
                .to_string(),
            requires_reason: false,
            confirm_text: "Confirm: cancel pending mode-transition intent".to_string(),
            enabled: has_pending_intent,
            disabled_reason: if !has_pending_intent {
                if st.db.is_none() {
                    Some(
                        "Backend unavailable; cannot query for pending mode-transition intent."
                            .to_string(),
                    )
                } else {
                    Some("No pending mode-transition intent to cancel.".to_string())
                }
            } else {
                None
            },
        },
    ];

    (
        StatusCode::OK,
        Json(ActionCatalogResponse {
            canonical_route: "/api/v1/ops/catalog".to_string(),
            actions,
        }),
    )
}

// ---------------------------------------------------------------------------
// CC-03: /api/v1/ops/mode-change-guidance
// ---------------------------------------------------------------------------

pub(crate) async fn build_mode_change_guidance(st: &AppState) -> ModeChangeGuidanceResponse {
    let restart_truth = match st.restart_truth_snapshot().await {
        Ok(snapshot) => Some(ModeChangeRestartTruth {
            local_owned_run_id: snapshot.local_owned_run_id,
            durable_active_run_id: snapshot.durable_active_run_id,
            durable_active_without_local_ownership: snapshot.durable_active_without_local_ownership,
        }),
        Err(_) => None,
    };

    // C3: Surface the current parity evidence state so operators planning a
    // mode transition see the live-trust ceiling on this surface without
    // consulting a second endpoint.  Same evaluator as C1/C2/parity-evidence.
    let parity_outcome = evaluate_parity_evidence_guarded();
    let parity_evidence_state = match &parity_outcome {
        ParityEvidenceOutcome::NotConfigured => "not_configured",
        ParityEvidenceOutcome::Absent => "absent",
        ParityEvidenceOutcome::Invalid { .. } => "invalid",
        ParityEvidenceOutcome::Present {
            live_trust_complete: true,
            ..
        } => "complete",
        ParityEvidenceOutcome::Present {
            live_trust_complete: false,
            ..
        } => "incomplete",
        ParityEvidenceOutcome::Unavailable { .. } => "unavailable",
    }
    .to_string();
    let live_trust_complete = match &parity_outcome {
        ParityEvidenceOutcome::Present {
            live_trust_complete,
            ..
        } => Some(*live_trust_complete),
        _ => None,
    };

    // CC-03A: Build the canonical transition verdict for every possible target
    // mode from the current mode.  All semantics come from the canonical seam
    // in `mode_transition::evaluate_mode_transition`; no route-local transition
    // logic is permitted here.
    let current_mode = st.deployment_mode();
    let all_target_modes = [
        DeploymentMode::Paper,
        DeploymentMode::LiveShadow,
        DeploymentMode::LiveCapital,
        DeploymentMode::Backtest,
    ];
    let transition_verdicts: Vec<ModeTransitionEntry> = all_target_modes
        .iter()
        .map(|&target| {
            let verdict = evaluate_mode_transition(current_mode, target);
            ModeTransitionEntry {
                target_mode: target.as_api_label().to_string(),
                verdict: verdict.as_str().to_string(),
                reason: verdict.reason().to_string(),
                preconditions: verdict
                    .preconditions()
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            }
        })
        .collect();

    // CC-03C: Mount durable restart workflow truth from sys_restart_intent
    // (CC-03B).  Fail-closed when no DB is available.
    let restart_workflow = if st.db.is_none() {
        RestartWorkflowTruth {
            truth_state: "backend_unavailable".to_string(),
            pending_intent: None,
        }
    } else {
        match st.load_pending_restart_intent().await {
            Some(row) => RestartWorkflowTruth {
                truth_state: "active".to_string(),
                pending_intent: Some(PendingRestartIntentSnapshot {
                    intent_id: row.intent_id.to_string(),
                    from_mode: row.from_mode,
                    to_mode: row.to_mode,
                    transition_verdict: row.transition_verdict,
                    initiated_by: row.initiated_by,
                    initiated_at_utc: row.initiated_at_utc.to_rfc3339(),
                    note: row.note,
                }),
            },
            None => RestartWorkflowTruth {
                truth_state: "no_pending".to_string(),
                pending_intent: None,
            },
        }
    };

    ModeChangeGuidanceResponse {
        canonical_route: "/api/v1/ops/mode-change-guidance".to_string(),
        current_mode: current_mode.as_api_label().to_string(),
        transition_permitted: false,
        transition_refused_reason:
            "Mode transitions require a controlled daemon restart with configuration reload. \
             Hot switching is not supported in the current architecture."
                .to_string(),
        preconditions: vec![
            "The daemon must be disarmed before restart \
             (POST /api/v1/ops/action {\"action_key\":\"disarm-execution\"})."
                .to_string(),
            "All open positions must be flat or explicitly acknowledged before shutdown."
                .to_string(),
            "All pending outbox orders must be drained or cancelled before shutdown."
                .to_string(),
            "The target deployment mode must be set in the daemon configuration file \
             before restart."
                .to_string(),
        ],
        operator_next_steps: vec![
            "Step 1: POST /api/v1/ops/action {\"action_key\":\"disarm-execution\"} — disarm the daemon.".to_string(),
            "Step 2: Verify no open positions or pending outbox orders remain.".to_string(),
            "Step 3: Update the daemon configuration file with the target deployment mode.".to_string(),
            "Step 4: Stop the daemon process (SIGTERM or service stop command).".to_string(),
            "Step 5: Confirm the daemon exited cleanly (exit code 0; no active run remains in DB).".to_string(),
            "Step 6: Restart the daemon with the updated configuration.".to_string(),
            "Step 7: Verify GET /api/v1/health returns ok=true and confirm new mode via GET /api/v1/ops/mode-change-guidance.".to_string(),
        ],
        restart_truth,
        transition_verdicts,
        restart_workflow,
        parity_evidence_state,
        live_trust_complete,
    }
}

pub(crate) async fn ops_mode_change_guidance(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    (StatusCode::OK, Json(build_mode_change_guidance(&st).await)).into_response()
}
