//! Control-plane route handlers.
//!
//! Contains: run_start, run_stop, run_halt, integrity_arm, integrity_disarm,
//! ops_action, ops_catalog, ops_mode_change_guidance, build_mode_change_guidance.

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
    ModeChangeRestartTruth, OperatorActionAuditFields, OperatorActionResponse, OpsActionRequest,
};
use crate::notify::OperatorNotifyPayload;
use crate::state::{AppState, BusMsg, RuntimeLifecycleError};

use super::helpers::{runtime_error_response, write_operator_audit_event};

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
            st.discord_notifier
                .notify_operator_action(&OperatorNotifyPayload {
                    action_key: "run.start".to_string(),
                    disposition: "applied".to_string(),
                    environment: Some(st.deployment_mode().as_api_label().to_string()),
                    ts_utc: Utc::now().to_rfc3339(),
                    provenance_ref: audit_uuid.map(|id| format!("audit_events:{}", id)),
                    run_id: snapshot.active_run_id.map(|id| id.to_string()),
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
            st.discord_notifier
                .notify_operator_action(&OperatorNotifyPayload {
                    action_key: "run.stop".to_string(),
                    disposition: "applied".to_string(),
                    environment: Some(st.deployment_mode().as_api_label().to_string()),
                    ts_utc: Utc::now().to_rfc3339(),
                    provenance_ref: audit_uuid.map(|id| format!("audit_events:{}", id)),
                    run_id: snapshot.active_run_id.map(|id| id.to_string()),
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
            st.discord_notifier
                .notify_operator_action(&OperatorNotifyPayload {
                    action_key: "run.halt".to_string(),
                    disposition: "applied".to_string(),
                    environment: Some(st.deployment_mode().as_api_label().to_string()),
                    ts_utc: Utc::now().to_rfc3339(),
                    provenance_ref: audit_uuid.map(|id| format!("audit_events:{}", id)),
                    run_id: snapshot.active_run_id.map(|id| id.to_string()),
                })
                .await;
            (StatusCode::OK, Json(snapshot)).into_response()
        }
        Err(err) => runtime_error_response(err),
    }
}

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

    st.discord_notifier
        .notify_operator_action(&OperatorNotifyPayload {
            action_key: "control.arm".to_string(),
            disposition: "applied".to_string(),
            environment: Some(st.deployment_mode().as_api_label().to_string()),
            ts_utc: Utc::now().to_rfc3339(),
            provenance_ref: if st.db.is_some() {
                Some("sys_arm_state".to_string())
            } else {
                None
            },
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

    st.discord_notifier
        .notify_operator_action(&OperatorNotifyPayload {
            action_key: "control.disarm".to_string(),
            disposition: "applied".to_string(),
            environment: Some(st.deployment_mode().as_api_label().to_string()),
            ts_utc: Utc::now().to_rfc3339(),
            provenance_ref: if st.db.is_some() {
                Some("sys_arm_state".to_string())
            } else {
                None
            },
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
                    durable_targets: if st.db.is_some() {
                        vec!["sys_arm_state".to_string()]
                    } else {
                        vec![]
                    },
                    audit_event_id: None,
                },
            };
            st.discord_notifier
                .notify_operator_action(&OperatorNotifyPayload {
                    action_key: "control.arm".to_string(),
                    disposition: "applied".to_string(),
                    environment: response.environment.clone(),
                    ts_utc: Utc::now().to_rfc3339(),
                    provenance_ref: if st.db.is_some() {
                        Some("sys_arm_state".to_string())
                    } else {
                        None
                    },
                    run_id: None,
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
                    durable_targets: if st.db.is_some() {
                        vec!["sys_arm_state".to_string()]
                    } else {
                        vec![]
                    },
                    audit_event_id: None,
                },
            };
            st.discord_notifier
                .notify_operator_action(&OperatorNotifyPayload {
                    action_key: "control.disarm".to_string(),
                    disposition: "applied".to_string(),
                    environment: response.environment.clone(),
                    ts_utc: Utc::now().to_rfc3339(),
                    provenance_ref: if st.db.is_some() {
                        Some("sys_arm_state".to_string())
                    } else {
                        None
                    },
                    run_id: None,
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
                };
                st.discord_notifier
                    .notify_operator_action(&OperatorNotifyPayload {
                        action_key: "run.start".to_string(),
                        disposition: "applied".to_string(),
                        environment: response.environment.clone(),
                        ts_utc: Utc::now().to_rfc3339(),
                        provenance_ref: audit_uuid.map(|id| format!("audit_events:{}", id)),
                        run_id: snapshot.active_run_id.map(|id| id.to_string()),
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
                };
                st.discord_notifier
                    .notify_operator_action(&OperatorNotifyPayload {
                        action_key: "run.stop".to_string(),
                        disposition: "applied".to_string(),
                        environment: response.environment.clone(),
                        ts_utc: Utc::now().to_rfc3339(),
                        provenance_ref: audit_uuid.map(|id| format!("audit_events:{}", id)),
                        run_id: snapshot.active_run_id.map(|id| id.to_string()),
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
                };
                st.discord_notifier
                    .notify_operator_action(&OperatorNotifyPayload {
                        action_key: "run.halt".to_string(),
                        disposition: "applied".to_string(),
                        environment: response.environment.clone(),
                        ts_utc: Utc::now().to_rfc3339(),
                        provenance_ref: audit_uuid.map(|id| format!("audit_events:{}", id)),
                        run_id: snapshot.active_run_id.map(|id| id.to_string()),
                    })
                    .await;
                (StatusCode::OK, Json(response)).into_response()
            }
            Err(err) => runtime_error_response(err),
        },

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
                     disarm-execution, disarm-strategy, start-system, stop-system, kill-switch",
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

    ModeChangeGuidanceResponse {
        canonical_route: "/api/v1/ops/mode-change-guidance".to_string(),
        current_mode: st.deployment_mode().as_api_label().to_string(),
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
    }
}

pub(crate) async fn ops_mode_change_guidance(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    (StatusCode::OK, Json(build_mode_change_guidance(&st).await)).into_response()
}
