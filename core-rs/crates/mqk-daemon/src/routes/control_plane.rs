//! Control-plane route handlers.
//!
//! Contains: run_start, run_stop, run_halt (via run_lifecycle), integrity_arm,
//! integrity_disarm, ops_action, ops_catalog, ops_mode_change_guidance,
//! build_mode_change_guidance.

// MT-07C: run lifecycle handlers extracted to reduce file size.
mod run_lifecycle;
pub(crate) use run_lifecycle::{run_halt, run_start, run_stop};

use std::{collections::BTreeSet, sync::Arc};

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use tracing::info;

use crate::api_types::{
    ActionCatalogEntry, ActionCatalogResponse, CapturedAccountEquityBaselineSnapshot,
    IntegrityResponse, ModeChangeGuidanceResponse, ModeChangeRestartTruth, ModeTransitionEntry,
    OperatorActionAuditFields, OperatorActionResponse, OpsActionRequest,
    PendingRestartIntentSnapshot, RestartWorkflowTruth,
};
use crate::mode_transition::{evaluate_mode_transition, ModeTransitionVerdict};
use crate::notify::{
    CriticalAlertPayload, OperatorNotifyPayload, RunStatusPayload, TestAlertPayload,
    TradeEventPayload,
};
use crate::parity_evidence::{evaluate_parity_evidence_guarded, ParityEvidenceOutcome};
use crate::state::DeploymentMode;
use crate::state::{
    AppState, BusMsg, MarketCalendarProvider, NyseWeekdaysProvider, RuntimeLifecycleError,
    DAEMON_ENGINE_ID,
};

use super::helpers::{
    check_arm_safety, parse_decimal_micros, runtime_error_response, write_operator_audit_event,
};

// ---------------------------------------------------------------------------
// POST /v1/integrity/arm
// ---------------------------------------------------------------------------

pub(crate) async fn integrity_arm(State(st): State<Arc<AppState>>) -> Response {
    // CTRL-ARM-01: preflight before any state mutation.
    if let Err((gate, blockers)) = check_arm_safety(&st, st.db.as_ref()).await {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": format!("GATE_REFUSED: arm blocked by {gate}"),
                "gate": gate,
                "accepted": false,
                "disposition": "refused",
                "blockers": blockers,
            })),
        )
            .into_response();
    }

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
            // CTRL-ARM-01: preflight before any state mutation.
            if let Err((gate, blockers)) = check_arm_safety(&st, st.db.as_ref()).await {
                return (
                    StatusCode::FORBIDDEN,
                    Json(OperatorActionResponse {
                        requested_action: "control.arm".to_string(),
                        accepted: false,
                        disposition: "refused".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers,
                        warnings: vec![format!("arm blocked by gate: {gate}")],
                        environment: Some(st.deployment_mode().as_api_label().to_string()),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            }
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
                captured_baseline: None,
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
                captured_baseline: None,
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
                        audit_event_id: audit_uuid.map(|id| id.to_string()),
                    },
                    pending_restart_intent: None,
                    captured_baseline: None,
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
                        audit_event_id: audit_uuid.map(|id| id.to_string()),
                    },
                    pending_restart_intent: None,
                    captured_baseline: None,
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
                        audit_event_id: audit_uuid.map(|id| id.to_string()),
                    },
                    pending_restart_intent: None,
                    captured_baseline: None,
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
                        captured_baseline: None,
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
                        captured_baseline: None,
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
                        captured_baseline: None,
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
                            captured_baseline: None,
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
                        captured_baseline: None,
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
                        captured_baseline: None,
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
                                    captured_baseline: None,
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

        // AUTON-PAPER-OPS-04: Clear a halted run so a fresh start can proceed.
        //
        // Gate sequence (fail-closed):
        //   1. DB required — cannot verify or mutate run state without DB.
        //   2. Latest run must exist — nothing to clear otherwise.
        //   3. Latest run must be HALTED — state guard; rejects if already
        //      stopped or still running so the operator cannot accidentally
        //      terminate a live run via this path.
        //   4. clear_halted_run transitions HALTED → STOPPED in DB.
        //
        // After success the operator must still re-arm and re-start; this
        // action clears only the durable run blocking state.  Integrity
        // in-memory halt flag is NOT cleared here — that requires arm-execution.
        "clear-halted-run" => {
            let Some(db) = st.db.as_ref() else {
                return runtime_error_response(RuntimeLifecycleError::ServiceUnavailable {
                    fault_class: "ops.action.clear_halted_run.no_db",
                    message: "DB is required to clear a halted run; \
                              clear-halted-run is not accepted without a DB connection."
                        .to_string(),
                });
            };

            let latest = match mqk_db::fetch_latest_run_for_engine(
                db,
                DAEMON_ENGINE_ID,
                st.deployment_mode().as_db_mode(),
            )
            .await
            {
                Ok(Some(run)) => run,
                Ok(None) => {
                    return (
                        StatusCode::CONFLICT,
                        Json(OperatorActionResponse {
                            requested_action: "clear-halted-run".to_string(),
                            accepted: false,
                            disposition: "no_run_found".to_string(),
                            resulting_integrity_state: None,
                            resulting_desired_armed: None,
                            blockers: vec![
                                "No run found for this engine and mode; nothing to clear."
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
                            captured_baseline: None,
                        }),
                    )
                        .into_response();
                }
                Err(err) => {
                    return runtime_error_response(RuntimeLifecycleError::internal(
                        "clear_halted_run fetch failed",
                        err,
                    ))
                }
            };

            if !matches!(latest.status, mqk_db::RunStatus::Halted) {
                return (
                    StatusCode::CONFLICT,
                    Json(OperatorActionResponse {
                        requested_action: "clear-halted-run".to_string(),
                        accepted: false,
                        disposition: "run_not_halted".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![format!(
                            "The latest run {} is in state '{}', not 'HALTED'; \
                             clear-halted-run can only be applied to a HALTED run.",
                            latest.run_id,
                            latest.status.as_str(),
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
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            }

            let run_id = latest.run_id;
            match mqk_db::clear_halted_run_and_reset_stale_claims(db, run_id, Utc::now()).await {
                Ok(mqk_db::ClearHaltedRunOutcome::ActiveRuntimeLease {
                    holder_id,
                    epoch,
                    lease_expires_at,
                }) => {
                    // PAPER-SOAK-STALE-CLAIM-RECOVERY-03 Layer A: an
                    // unexpired runtime leader lease is still-live dispatch
                    // authority even though the run is durably HALTED — the
                    // prior runtime process may not have observed the halt
                    // yet. Zero mutation: runs.status, the CLAIMED rows, and
                    // the lease row are all untouched.
                    info!(
                        run_id = %run_id,
                        holder_id = %holder_id,
                        epoch,
                        lease_expires_at = %lease_expires_at.to_rfc3339(),
                        "ops/action clear-halted-run refused: active runtime lease"
                    );
                    (
                        StatusCode::CONFLICT,
                        Json(OperatorActionResponse {
                            requested_action: "clear-halted-run".to_string(),
                            accepted: false,
                            disposition: "active_runtime_lease".to_string(),
                            resulting_integrity_state: None,
                            resulting_desired_armed: None,
                            blockers: vec![format!(
                                "runtime.clear_halted.active_runtime_lease: run {run_id} still \
                                 has an unexpired runtime leader lease (holder={holder_id}, \
                                 epoch={epoch}, expires_at={}); the prior runtime process may \
                                 still be able to act. Wait for it to release the lease (normal \
                                 shutdown) or for the lease to expire (crashed process), then \
                                 retry clear-halted-run.",
                                lease_expires_at.to_rfc3339(),
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
                            captured_baseline: None,
                        }),
                    )
                        .into_response()
                }
                Ok(mqk_db::ClearHaltedRunOutcome::StaleRunTransition { actual_status }) => {
                    // A concurrent transition (e.g. a second operator clear)
                    // won the race between this route's precondition fetch
                    // above and the guarded transaction. Same disposition as
                    // the ordinary "not currently HALTED" precondition check.
                    (
                        StatusCode::CONFLICT,
                        Json(OperatorActionResponse {
                            requested_action: "clear-halted-run".to_string(),
                            accepted: false,
                            disposition: "run_not_halted".to_string(),
                            resulting_integrity_state: None,
                            resulting_desired_armed: None,
                            blockers: vec![format!(
                                "The latest run {run_id} is in state '{actual_status}', not \
                                 'HALTED'; clear-halted-run can only be applied to a HALTED run."
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
                            captured_baseline: None,
                        }),
                    )
                        .into_response()
                }
                Ok(mqk_db::ClearHaltedRunOutcome::Success { stale_claims_reset }) => {
                    let reset_count = stale_claims_reset;
                    info!(
                        run_id = %run_id,
                        stale_claims_reset = reset_count,
                        "ops/action clear-halted-run"
                    );
                    let audit_uuid = write_operator_audit_event(
                        &st,
                        Some(run_id),
                        "run.clear_halted",
                        "STOPPED",
                    )
                    .await
                    .ok()
                    .flatten();
                    let _ = st.bus.send(BusMsg::LogLine {
                        level: "INFO".to_string(),
                        msg: format!(
                            "ops/action: halted run {run_id} cleared to STOPPED \
                             ({reset_count} stale CLAIMED outbox row(s) reset to PENDING); \
                             operator must re-arm and re-start"
                        ),
                    });
                    // PAPER-SOAK-STALE-CLAIM-RECOVERY-02: the same atomic
                    // transaction that clears the halt also resets this
                    // run's stale CLAIMED outbox rows, so oms_outbox is a
                    // real durable write target whenever any were reset.
                    let mut durable_targets = vec!["runs".to_string()];
                    if reset_count > 0 {
                        durable_targets.push("oms_outbox".to_string());
                    }
                    if audit_uuid.is_some() {
                        durable_targets.push("audit_events".to_string());
                    }
                    (
                        StatusCode::OK,
                        Json(OperatorActionResponse {
                            requested_action: "clear-halted-run".to_string(),
                            accepted: true,
                            disposition: "halted_run_cleared".to_string(),
                            resulting_integrity_state: None,
                            resulting_desired_armed: None,
                            blockers: vec![],
                            warnings: vec![format!(
                                "Halted run cleared to STOPPED ({reset_count} stale CLAIMED \
                                 outbox row(s) reset to PENDING). You must re-arm \
                                 (arm-execution) and then start (start-system) to begin \
                                 a new execution cycle."
                            )],
                            environment: Some(st.deployment_mode().as_api_label().to_string()),
                            scope: Some("daemon_instance".to_string()),
                            audit: OperatorActionAuditFields {
                                durable_db_write: true,
                                durable_targets,
                                audit_event_id: audit_uuid.map(|id| id.to_string()),
                            },
                            pending_restart_intent: None,
                            captured_baseline: None,
                        }),
                    )
                        .into_response()
                }
                Err(err) => runtime_error_response(RuntimeLifecycleError::internal(
                    "clear_halted_run_and_reset_stale_claims update failed",
                    err,
                )),
            }
        }

        // OBS-SESSION-DISCORD-01: Operator test of Discord webhook delivery.
        //
        // Fires a clearly-labelled "[TEST]" notification to the configured webhook.
        // Does NOT mutate any trading, runtime, arm, or integrity state.
        // Requires operator bearer token (route is in the authenticated sub-router).
        // Response never includes the webhook URL.
        "test-discord-alert" => {
            let ts = Utc::now().to_rfc3339();
            let env_label = st.deployment_mode().as_api_label().to_string();
            let notifier_status = st.discord_notifier.status();

            let (disposition, warnings) = if !notifier_status.configured {
                (
                    "noop_unconfigured".to_string(),
                    vec![
                        "Discord notifier not configured; DISCORD_WEBHOOK_URL is absent or \
                         empty — set it to enable delivery"
                            .to_string(),
                    ],
                )
            } else {
                // Delivery is best-effort; result is informational only.
                let delivered = st
                    .discord_notifier
                    .notify_test_alert(&TestAlertPayload {
                        environment: Some(env_label.clone()),
                        ts_utc: ts.clone(),
                        note: "operator test via test-discord-alert route; \
                               no trading state affected"
                            .to_string(),
                    })
                    .await;
                let disposition = if delivered {
                    "delivery_attempted".to_string()
                } else {
                    "delivery_failed".to_string()
                };
                let mut warns = vec![];
                if !delivered {
                    warns.push(
                        "Discord test-alert delivery failed; check DISCORD_WEBHOOK_URL \
                         and network connectivity"
                            .to_string(),
                    );
                }
                (disposition, warns)
            };

            info!(
                disposition = %disposition,
                "ops/action test-discord-alert"
            );

            (
                StatusCode::OK,
                Json(OperatorActionResponse {
                    requested_action: "test-discord-alert".to_string(),
                    accepted: true,
                    disposition,
                    resulting_integrity_state: None,
                    resulting_desired_armed: None,
                    blockers: vec![],
                    warnings,
                    environment: Some(env_label),
                    scope: Some("daemon_instance".to_string()),
                    audit: OperatorActionAuditFields {
                        durable_db_write: false,
                        durable_targets: vec![],
                        audit_event_id: None,
                    },
                    pending_restart_intent: None,
                    captured_baseline: None,
                }),
            )
                .into_response()
        }

        // PAPER-SAFE-FLATTEN-ROUTE-01: Operator paper flatten route.
        //
        // Submits reduce-to-zero sell orders for open long positions via the
        // normal OMS/outbox/broker adapter path.  Paper mode only.
        //
        // Gate sequence (fail-closed):
        //   1. deployment_mode == Paper   — non-paper rejected (covers live_routing gate)
        //   2. DB required                — cannot submit without durable outbox
        //   3. reconcile_status == "ok"   — dirty reconcile blocks flatten
        //   4. durable_arm_state == ARMED — not armed, halt, or disarmed → block
        //   5. active_run_id present      — no active run → block
        //   6. runtime_state == "running" — not running → block
        //   7. execution_snapshot present — position truth unavailable → block
        //   8. target positions non-empty — nothing to flatten → block
        //   9. outbox_enqueue per symbol  — normal path; idempotent by UUIDv5 key
        "flatten-paper-positions" => {
            let env_label = st.deployment_mode().as_api_label().to_string();

            // Gate 1: paper mode only.
            if !matches!(st.deployment_mode(), DeploymentMode::Paper) {
                return (
                    StatusCode::FORBIDDEN,
                    Json(OperatorActionResponse {
                        requested_action: "flatten-paper-positions".to_string(),
                        accepted: false,
                        disposition: "not_paper_mode".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![format!(
                            "flatten-paper-positions is paper-only; \
                             current mode is '{}' — live routing is not blocked here",
                            env_label
                        )],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            }

            // Gate 2: DB required for durable outbox write.
            let Some(db) = st.db.as_ref() else {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(OperatorActionResponse {
                        requested_action: "flatten-paper-positions".to_string(),
                        accepted: false,
                        disposition: "db_unavailable".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec!["flatten-paper-positions requires a DB connection for \
                             durable outbox writes"
                            .to_string()],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            };

            // Gate 3: reconcile must be ok.
            let reconcile = st.current_reconcile_snapshot().await;
            if reconcile.status != "ok" {
                return (
                    StatusCode::FORBIDDEN,
                    Json(OperatorActionResponse {
                        requested_action: "flatten-paper-positions".to_string(),
                        accepted: false,
                        disposition: "reconcile_not_ok".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![format!(
                            "flatten-paper-positions refused: reconcile status is '{}' \
                             (expected 'ok'); resolve mismatches before flattening",
                            reconcile.status
                        )],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            }

            // Gate 4: durable arm state must be ARMED.
            let (durable_arm_state, _durable_arm_reason) = match mqk_db::load_arm_state(db).await {
                Ok(Some((state, reason))) => (state, reason),
                Ok(None) => {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(OperatorActionResponse {
                            requested_action: "flatten-paper-positions".to_string(),
                            accepted: false,
                            disposition: "not_armed".to_string(),
                            resulting_integrity_state: None,
                            resulting_desired_armed: None,
                            blockers: vec![
                                "flatten-paper-positions refused: durable arm state is absent; \
                                 arm the system before flattening"
                                    .to_string(),
                            ],
                            warnings: vec![],
                            environment: Some(env_label),
                            scope: Some("daemon_instance".to_string()),
                            audit: OperatorActionAuditFields {
                                durable_db_write: false,
                                durable_targets: vec![],
                                audit_event_id: None,
                            },
                            pending_restart_intent: None,
                            captured_baseline: None,
                        }),
                    )
                        .into_response();
                }
                Err(err) => {
                    return runtime_error_response(RuntimeLifecycleError::Internal {
                        fault_class: "ops.flatten.arm_state_load",
                        message: format!("flatten-paper-positions: arm state load failed: {err}"),
                    });
                }
            };
            if durable_arm_state != "ARMED" {
                return (
                    StatusCode::FORBIDDEN,
                    Json(OperatorActionResponse {
                        requested_action: "flatten-paper-positions".to_string(),
                        accepted: false,
                        disposition: "not_armed".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![format!(
                            "flatten-paper-positions refused: durable arm state is '{}'; \
                             arm the system before flattening",
                            durable_arm_state
                        )],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            }

            // Gates 5 & 6: active run + running state.
            let status = match st.current_status_snapshot().await {
                Ok(s) => s,
                Err(err) => return runtime_error_response(err),
            };
            let Some(active_run_id) = status.active_run_id else {
                return (
                    StatusCode::CONFLICT,
                    Json(OperatorActionResponse {
                        requested_action: "flatten-paper-positions".to_string(),
                        accepted: false,
                        disposition: "no_active_run".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec!["flatten-paper-positions refused: no active durable run; \
                             start the system before flattening"
                            .to_string()],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            };
            if status.state != "running" {
                return (
                    StatusCode::CONFLICT,
                    Json(OperatorActionResponse {
                        requested_action: "flatten-paper-positions".to_string(),
                        accepted: false,
                        disposition: "not_running".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![format!(
                            "flatten-paper-positions refused: runtime state is '{}'; \
                             system must be running to flatten",
                            status.state
                        )],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            }

            // Gate 7: execution snapshot must be present for position truth.
            let Some(exec_snap) = st.current_execution_snapshot().await else {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(OperatorActionResponse {
                        requested_action: "flatten-paper-positions".to_string(),
                        accepted: false,
                        disposition: "no_position_truth".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![
                            "flatten-paper-positions refused: execution snapshot is unavailable; \
                             position truth cannot be determined"
                                .to_string(),
                        ],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            };

            // Collect non-flat positions, filtered by symbol if provided.
            // Duplicate or blank symbols make position truth ambiguous, so the
            // route fails closed before enqueuing any close orders.
            let symbol_filter = body.symbol.as_deref().map(|s| s.trim().to_uppercase());
            let mut seen_position_symbols = BTreeSet::new();
            let raw_positions_to_flatten: Vec<(String, String, i64)> = exec_snap
                .portfolio
                .positions
                .iter()
                .filter(|p| p.net_qty != 0)
                .filter(|p| {
                    symbol_filter
                        .as_deref()
                        .is_none_or(|f| p.symbol.to_uppercase() == f)
                })
                .map(|p| {
                    let normalized = p.symbol.trim().to_ascii_uppercase();
                    (p.symbol.clone(), normalized, p.net_qty)
                })
                .collect::<Vec<_>>();
            let mut malformed_positions = Vec::new();
            let mut duplicate_positions = Vec::new();
            let positions_to_flatten: Vec<(String, i64)> = raw_positions_to_flatten
                .into_iter()
                .filter_map(|(raw_symbol, normalized, net_qty)| {
                    if normalized.is_empty() {
                        malformed_positions.push(raw_symbol);
                        return None;
                    }
                    if !seen_position_symbols.insert(normalized.clone()) {
                        duplicate_positions.push(normalized);
                        return None;
                    }
                    Some((raw_symbol, net_qty))
                })
                .collect();
            if !malformed_positions.is_empty() || !duplicate_positions.is_empty() {
                let mut blockers = Vec::new();
                if !malformed_positions.is_empty() {
                    blockers.push(format!(
                        "flatten-paper-positions refused: malformed blank position symbol(s): {}",
                        malformed_positions.join(", ")
                    ));
                }
                if !duplicate_positions.is_empty() {
                    blockers.push(format!(
                        "flatten-paper-positions refused: duplicate position snapshot row(s): {}",
                        duplicate_positions.join(", ")
                    ));
                }
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(OperatorActionResponse {
                        requested_action: "flatten-paper-positions".to_string(),
                        accepted: false,
                        disposition: "ambiguous_position_snapshot".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers,
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            }

            // Gate 8: must have at least one non-flat position to flatten.
            if positions_to_flatten.is_empty() {
                return (
                    StatusCode::CONFLICT,
                    Json(OperatorActionResponse {
                        requested_action: "flatten-paper-positions".to_string(),
                        accepted: false,
                        disposition: "no_positions".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![match symbol_filter.as_deref() {
                            Some(sym) => format!(
                                "flatten-paper-positions refused: no non-flat position found for '{sym}'"
                            ),
                            None => "flatten-paper-positions refused: no non-flat positions found".to_string(),
                        }],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            }

            // Gate 9: submit close orders for all positions via normal outbox path.
            // Long positions produce sells; short positions produce buy-to-cover.
            let ts_secs = Utc::now().timestamp();
            let mut enqueued_symbols: Vec<String> = vec![];
            let mut already_pending_symbols: Vec<String> = vec![];
            let mut failed_symbols: Vec<String> = vec![];
            let mut warnings: Vec<String> = vec![];

            for (symbol, net_qty) in &positions_to_flatten {
                let (key, order_json) =
                    crate::pre_event_flatten::build_operator_flatten_close_order_json(
                        symbol,
                        *net_qty,
                        ts_secs,
                        active_run_id,
                    );
                match mqk_db::outbox_enqueue(db, active_run_id, &key, order_json).await {
                    Ok(true) => {
                        info!(
                            run_id = %active_run_id,
                            symbol = %symbol,
                            net_qty = %net_qty,
                            idempotency_key = %key,
                            "operator_flatten_close_enqueued"
                        );
                        enqueued_symbols.push(symbol.clone());
                        let side = if *net_qty > 0 { "sell" } else { "buy" };
                        warnings.push(format!(
                            "enqueued: symbol={symbol} qty={} side={side} key={key}",
                            net_qty.abs()
                        ));
                    }
                    Ok(false) => {
                        info!(
                            run_id = %active_run_id,
                            symbol = %symbol,
                            idempotency_key = %key,
                            "operator_flatten_close_already_pending"
                        );
                        already_pending_symbols.push(symbol.clone());
                        warnings.push(format!("already_pending: symbol={symbol} key={key}"));
                    }
                    Err(err) => {
                        tracing::warn!(
                            run_id = %active_run_id,
                            symbol = %symbol,
                            error = %err,
                            "operator_flatten_close_enqueue_failed"
                        );
                        failed_symbols.push(symbol.clone());
                        warnings.push(format!("enqueue_failed: symbol={symbol} error={err}"));
                    }
                }
            }

            // Write durable audit event for the flatten action (best-effort, non-fatal).
            let flatten_audit_uuid = write_operator_audit_event(
                &st,
                Some(active_run_id),
                "ops.flatten_paper",
                "FLATTEN_SUBMITTED",
            )
            .await
            .ok()
            .flatten();

            let accepted = !enqueued_symbols.is_empty() || !already_pending_symbols.is_empty();

            // DISCORD-TRADE-LIFECYCLE-ALERTS-01: best-effort alert when flatten is accepted.
            // Fires when at least one symbol was enqueued or already pending.
            // Does not fire on refusals — those are covered by gate-level blockers.
            if accepted {
                let notifier = st.discord_notifier.clone();
                let run_str = Some(format!("{:.8}", active_run_id.to_string()));
                let env = Some(env_label.clone());
                let enqueued_str = enqueued_symbols.join(",");
                let already_str = already_pending_symbols.join(",");
                let audit_ref = flatten_audit_uuid
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| "none".to_string());
                let ts = chrono::Utc::now().to_rfc3339();
                tokio::spawn(async move {
                    notifier
                        .notify_trade_event(&TradeEventPayload {
                            stage: "flatten.requested".to_string(),
                            run_id: run_str,
                            symbol: if enqueued_str.is_empty() {
                                None
                            } else {
                                Some(enqueued_str.clone())
                            },
                            side: Some("sell".to_string()),
                            qty: None,
                            price_micros: None,
                            order_id: None,
                            detail: Some(format!(
                                "live_routing_enabled=false enqueued=[{enqueued_str}] \
                                 already_pending=[{already_str}] audit={audit_ref}"
                            )),
                            environment: env,
                            summary: format!(
                                "paper flatten requested: enqueued=[{enqueued_str}] \
                                 live_routing_enabled=false"
                            ),
                            ts_utc: ts,
                        })
                        .await;
                });
            }

            let disposition = if !failed_symbols.is_empty()
                && enqueued_symbols.is_empty()
                && already_pending_symbols.is_empty()
            {
                "all_enqueue_failed".to_string()
            } else if !failed_symbols.is_empty() {
                "partial_enqueue_failed".to_string()
            } else if !already_pending_symbols.is_empty() && enqueued_symbols.is_empty() {
                "all_already_pending".to_string()
            } else {
                "enqueued".to_string()
            };

            let mut final_blockers = vec![];
            if !failed_symbols.is_empty() {
                final_blockers.push(format!("enqueue failed for: {}", failed_symbols.join(", ")));
            }

            let mut durable_targets = vec!["oms_outbox".to_string()];
            if flatten_audit_uuid.is_some() {
                durable_targets.push("audit_events".to_string());
            }

            (
                if accepted {
                    StatusCode::OK
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                },
                Json(OperatorActionResponse {
                    requested_action: "flatten-paper-positions".to_string(),
                    accepted,
                    disposition,
                    resulting_integrity_state: None,
                    resulting_desired_armed: None,
                    blockers: final_blockers,
                    warnings,
                    environment: Some(env_label),
                    scope: Some("daemon_instance".to_string()),
                    audit: OperatorActionAuditFields {
                        durable_db_write: accepted,
                        durable_targets,
                        audit_event_id: flatten_audit_uuid.map(|id| id.to_string()),
                    },
                    pending_restart_intent: None,
                    captured_baseline: None,
                }),
            )
                .into_response()
        }

        // ---------------------------------------------------------------
        // PAPER-DAILY-PNL-CAPTURE-01B: explicit, operator-controlled
        // capture of one `sys_account_equity_baseline` row for a single
        // `trading_date`, read from the daemon's real `broker_snapshot`.
        // Never auto-triggered, never calls a broker/provider, never
        // touches `oms_outbox`/`oms_inbox`. See
        // docs/specs/paper_daily_pnl_capture_01a_current_truth_action_design.md.
        // ---------------------------------------------------------------
        "capture-account-equity-baseline" => {
            let env_label = st.deployment_mode().as_api_label().to_string();

            // Gate 1: DB required for the durable baseline write.
            let Some(db) = st.db.as_ref() else {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(OperatorActionResponse {
                        requested_action: "capture-account-equity-baseline".to_string(),
                        accepted: false,
                        disposition: "db_unavailable".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec!["capture-account-equity-baseline requires a DB \
                             connection for a durable baseline write"
                            .to_string()],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            };

            // Gate 2: a current broker snapshot is required as the source
            // of truth for equity/cash -- this action never calls a
            // broker or provider itself.
            let snap = st.broker_snapshot.read().await.clone();
            let Some(snapshot) = snap else {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(OperatorActionResponse {
                        requested_action: "capture-account-equity-baseline".to_string(),
                        accepted: false,
                        disposition: "no_broker_snapshot".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec!["capture-account-equity-baseline requires a current \
                             broker account snapshot; none is present"
                            .to_string()],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            };

            // Gate 3: reason required -- this action writes durable
            // financial-truth data, unlike most other ops/action arms.
            let reason = body.reason.clone().unwrap_or_default();
            if reason.trim().is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(OperatorActionResponse {
                        requested_action: "capture-account-equity-baseline".to_string(),
                        accepted: false,
                        disposition: "missing_reason".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec!["capture-account-equity-baseline requires a non-blank \
                             'reason' acknowledging this is a baseline capture"
                            .to_string()],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            }

            // Gate 4: trading_date required.
            let Some(trading_date_str) = body.trading_date.as_deref() else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(OperatorActionResponse {
                        requested_action: "capture-account-equity-baseline".to_string(),
                        accepted: false,
                        disposition: "missing_trading_date".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec!["capture-account-equity-baseline requires \
                             'trading_date' (\"YYYY-MM-DD\")"
                            .to_string()],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            };

            // Gate 5: trading_date must parse as a strict YYYY-MM-DD date.
            let Ok(trading_date) =
                chrono::NaiveDate::parse_from_str(trading_date_str.trim(), "%Y-%m-%d")
            else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(OperatorActionResponse {
                        requested_action: "capture-account-equity-baseline".to_string(),
                        accepted: false,
                        disposition: "invalid_trading_date".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![format!(
                            "trading_date '{trading_date_str}' is not a valid YYYY-MM-DD date"
                        )],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            };

            // Gate 6: trading_date must be a real NYSE trading day. Probed
            // at 18:00 UTC -- safely within NYSE regular hours under both
            // EST and EDT -- matching `resolve_daily_pnl`'s
            // `most_recent_trading_day_before` convention exactly
            // (core-rs/crates/mqk-daemon/src/routes/portfolio.rs).
            let probe = trading_date
                .and_hms_opt(18, 0, 0)
                .expect("18:00:00 is always a valid time")
                .and_utc();
            if !NyseWeekdaysProvider.session_for(probe).is_trading_day {
                return (
                    StatusCode::FORBIDDEN,
                    Json(OperatorActionResponse {
                        requested_action: "capture-account-equity-baseline".to_string(),
                        accepted: false,
                        disposition: "non_trading_day".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec![format!(
                            "trading_date '{trading_date}' is not a real NYSE trading day"
                        )],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            }

            // Gate 7: broker-reported equity/cash must parse as decimals.
            // Never fabricates a value on a parse failure.
            let (Some(equity_micros), Some(cash_micros)) = (
                parse_decimal_micros(&snapshot.account.equity),
                parse_decimal_micros(&snapshot.account.cash),
            ) else {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(OperatorActionResponse {
                        requested_action: "capture-account-equity-baseline".to_string(),
                        accepted: false,
                        disposition: "unparseable_account_values".to_string(),
                        resulting_integrity_state: None,
                        resulting_desired_armed: None,
                        blockers: vec!["broker_snapshot account equity/cash could not be \
                             parsed as a decimal value"
                            .to_string()],
                        warnings: vec![],
                        environment: Some(env_label),
                        scope: Some("daemon_instance".to_string()),
                        audit: OperatorActionAuditFields {
                            durable_db_write: false,
                            durable_targets: vec![],
                            audit_event_id: None,
                        },
                        pending_restart_intent: None,
                        captured_baseline: None,
                    }),
                )
                    .into_response();
            };

            let currency = snapshot.account.currency.clone();
            let captured_by = "operator:capture-account-equity-baseline".to_string();
            let broker_snapshot_source = st.broker_snapshot_source().as_str().to_string();
            let captured_at_utc = Utc::now();

            // Deterministic audit ID: identical inputs (including equity/
            // cash) reproduce the same UUID, matching
            // `.claude/rules/audit_repo_truth_rules.md`'s "re-run of the
            // same logical event -> same audit ID" rule.
            let audit_event_id = uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_DNS,
                format!(
                    "mqk.account-equity-baseline.v1|{trading_date}|{equity_micros}|\
                     {cash_micros}|{currency}|{captured_by}|{broker_snapshot_source}"
                )
                .as_bytes(),
            );

            if let Err(err) = mqk_db::upsert_account_equity_baseline(
                db,
                mqk_db::UpsertAccountEquityBaselineArgs {
                    trading_date,
                    equity_micros,
                    cash_micros,
                    currency: currency.clone(),
                    captured_at_utc,
                    captured_by: captured_by.clone(),
                    broker_snapshot_source: broker_snapshot_source.clone(),
                    audit_event_id,
                },
            )
            .await
            {
                return runtime_error_response(RuntimeLifecycleError::Internal {
                    fault_class: "ops.action.capture_account_equity_baseline.upsert",
                    message: format!("Failed to persist account equity baseline: {err}"),
                });
            }

            info!(
                trading_date = %trading_date,
                audit_event_id = %audit_event_id,
                "ops/action capture-account-equity-baseline: baseline row persisted"
            );

            (
                StatusCode::OK,
                Json(OperatorActionResponse {
                    requested_action: "capture-account-equity-baseline".to_string(),
                    accepted: true,
                    disposition: "applied".to_string(),
                    resulting_integrity_state: None,
                    resulting_desired_armed: None,
                    blockers: vec![],
                    warnings: vec![],
                    environment: Some(env_label),
                    scope: Some("daemon_instance".to_string()),
                    audit: OperatorActionAuditFields {
                        durable_db_write: true,
                        durable_targets: vec!["sys_account_equity_baseline".to_string()],
                        audit_event_id: Some(audit_event_id.to_string()),
                    },
                    pending_restart_intent: None,
                    captured_baseline: Some(CapturedAccountEquityBaselineSnapshot {
                        trading_date: trading_date.to_string(),
                        equity: equity_micros as f64 / mqk_portfolio::MICROS_SCALE as f64,
                        cash: cash_micros as f64 / mqk_portfolio::MICROS_SCALE as f64,
                        currency,
                        captured_at_utc: captured_at_utc.to_rfc3339(),
                        captured_by,
                        broker_snapshot_source,
                        audit_event_id: audit_event_id.to_string(),
                    }),
                }),
            )
                .into_response()
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
                     request-mode-change, cancel-mode-transition, clear-halted-run, \
                     test-discord-alert, flatten-paper-positions, \
                     capture-account-equity-baseline",
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
                captured_baseline: None,
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

    // AUTON-PAPER-OPS-04: Check for a durable halted run so the clear-halted-run
    // catalog entry reflects real availability.  One extra SELECT at catalog read
    // time — same pattern as has_pending_intent above.
    let has_halted_run = if let Some(db) = st.db.as_ref() {
        mqk_db::fetch_latest_run_for_engine(db, DAEMON_ENGINE_ID, st.deployment_mode().as_db_mode())
            .await
            .ok()
            .flatten()
            .map(|r| matches!(r.status, mqk_db::RunStatus::Halted))
            .unwrap_or(false)
    } else {
        false
    };

    // DESKTOP-10: start-system must reflect the same deployment gate that
    // start_execution_runtime enforces.  deployment_readiness() is a cheap
    // in-memory read; every other readiness surface (preflight, status,
    // session) already carries this truth.  Without this check, the catalog
    // reports start-system=enabled for a paper+paper (or otherwise
    // deployment-blocked) daemon that is idle and not halted — presenting a
    // clickable "Start System" button to the operator while preflight shows
    // an active blocker.
    let dr = st.deployment_readiness();
    let deployment_ready = dr.start_allowed;

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
            description:
                "Start the execution runtime. System must be idle and deployment-ready to start."
                    .to_string(),
            requires_reason: false,
            confirm_text: "Confirm: start execution runtime".to_string(),
            enabled: idle && !halted && deployment_ready,
            disabled_reason: if !deployment_ready {
                dr.blocker
                    .clone()
                    .or_else(|| Some("Deployment is not start-ready.".to_string()))
            } else if halted {
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
        // AUTON-PAPER-OPS-04: Operator path to clear a halted run so a fresh
        // start can proceed without manual DB surgery.
        ActionCatalogEntry {
            action_key: "clear-halted-run".to_string(),
            label: "Clear Halted Run".to_string(),
            level: 3,
            description: "Transition the most recent HALTED run to STOPPED, unblocking \
                           a fresh start. Use only after reviewing the halt reason. \
                           You must re-arm and re-start after clearing."
                .to_string(),
            requires_reason: false,
            confirm_text: "Confirm: clear halted run and allow fresh start".to_string(),
            enabled: has_halted_run,
            disabled_reason: if !has_halted_run {
                if st.db.is_none() {
                    Some("Backend unavailable; cannot query for halted run state.".to_string())
                } else {
                    Some("No halted run found; nothing to clear.".to_string())
                }
            } else {
                None
            },
        },
        // PAPER-SAFE-FLATTEN-ROUTE-01: Paper-only operator flatten path.
        ActionCatalogEntry {
            action_key: "flatten-paper-positions".to_string(),
            label: "Flatten Paper Positions".to_string(),
            level: 3,
            description: "Submit market close orders for all open paper positions \
                           via the normal OMS/outbox/broker adapter path. \
                           Paper mode only. Requires armed + running + reconcile ok. \
                           Provide symbol in body to flatten a single position."
                .to_string(),
            requires_reason: true,
            confirm_text: "Confirm: submit flatten orders for open paper positions".to_string(),
            enabled: running && armed && matches!(st.deployment_mode(), DeploymentMode::Paper),
            disabled_reason: if !matches!(st.deployment_mode(), DeploymentMode::Paper) {
                Some(format!(
                    "flatten-paper-positions is paper-only; current mode is '{}'",
                    st.deployment_mode().as_api_label()
                ))
            } else if halted {
                Some("Cannot flatten while system is halted.".to_string())
            } else if !armed {
                Some("System must be armed to flatten positions.".to_string())
            } else if !running {
                Some("System must be running to flatten positions.".to_string())
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
