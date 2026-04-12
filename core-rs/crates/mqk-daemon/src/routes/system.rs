//! System-level route handlers (runtime operational surfaces).
//!
//! Contains: health, status_handler, system_status, system_preflight,
//! autonomous_readiness, system_metadata, system_runtime_leadership,
//! system_session, system_config_fingerprint, system_config_diffs,
//! authoritative_config_diff_rows.
//!
//! Artifact/evidence/topology handlers live in `routes/system_artifact.rs`
//! (MT-01 split).

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use sqlx::Row;

use crate::api_types::{
    AutonomousPaperReadinessResponse, ConfigDiffRow, ConfigDiffsResponse,
    ConfigFingerprintResponse, HealthResponse, PreflightStatusResponse, RuntimeErrorResponse,
    RuntimeLeadershipCheckpointRow, RuntimeLeadershipResponse, SessionStateResponse,
    SystemMetadataResponse, SystemStatusResponse,
};
use crate::parity_evidence::{evaluate_parity_evidence_guarded, ParityEvidenceOutcome};
use crate::state::{
    autonomous_session_schedule_from_env, AppState, AutonomousSessionTruth, DeploymentMode,
    StrategyMarketDataSource,
};

use super::helpers::{
    build_fault_signals, environment_and_live_routing_truth, runtime_error_response,
    runtime_status_from_state,
};

const DAEMON_ENGINE_ID: &str = "mqk-daemon";

// ---------------------------------------------------------------------------
// GET /v1/health
// ---------------------------------------------------------------------------

pub(crate) async fn health(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(HealthResponse {
            ok: true,
            service: st.build.service,
            version: st.build.version,
        }),
    )
}

// ---------------------------------------------------------------------------
// GET /v1/status
// ---------------------------------------------------------------------------

pub(crate) async fn status_handler(State(st): State<Arc<AppState>>) -> Response {
    match st.current_status_snapshot().await {
        Ok(snapshot) => (StatusCode::OK, Json(snapshot)).into_response(),
        Err(err) => runtime_error_response(err),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/status
// ---------------------------------------------------------------------------

pub(crate) async fn system_status(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return runtime_error_response(err),
    };
    let reconcile = st.current_reconcile_snapshot().await;
    let snapshot_present = st.broker_snapshot.read().await.is_some();
    let integrity_armed = status.integrity_armed;

    let (risk_blocked, db_status) = if let Some(db) = st.db.as_ref() {
        let risk_result = mqk_db::load_risk_block_state(db).await;
        let db_ok = risk_result.is_ok();
        let risk_blocked = risk_result.ok().flatten().is_some_and(|risk| risk.blocked);
        let db_status = if db_ok { "ok" } else { "warning" }.to_string();
        (risk_blocked, db_status)
    } else {
        (false, "unavailable".to_string())
    };

    let audit_writer_status = db_status.clone();

    let runtime_status = runtime_status_from_state(&status.state).to_string();
    let (environment, live_routing_enabled) =
        environment_and_live_routing_truth(&st, &status).await;
    let broker_status = if snapshot_present { "ok" } else { "warning" }.to_string();
    let integrity_status = if integrity_armed { "ok" } else { "warning" }.to_string();
    let reconcile_status = reconcile.status.clone();
    let has_critical = matches!(reconcile_status.as_str(), "dirty" | "stale")
        || (reconcile_status == "unknown" && runtime_status == "running");
    let has_warning = broker_status != "ok"
        || integrity_status != "ok"
        || reconcile_status != "ok"
        || db_status == "warning"
        || status.notes.is_some()
        || reconcile.note.is_some();

    // PT-AUTO-03: Surface autonomous signal intake state on the paper+alpaca path.
    //
    // Only populated when ExternalSignalIngestion is configured (paper+alpaca).
    // For all other deployments these fields are None (not applicable).
    // Values are derived directly from the enforced production state so the operator
    // can see whether Gate 1d is currently blocking all further signals.
    let (autonomous_signal_count, autonomous_signal_limit_hit) =
        if st.strategy_market_data_source() == StrategyMarketDataSource::ExternalSignalIngestion {
            (
                Some(st.day_signal_count()),
                Some(st.day_signal_limit_exceeded()),
            )
        } else {
            (None, None)
        };

    // C1: Live-trust truth surface.
    //
    // Evaluate parity evidence using the same evaluator as the dedicated
    // /api/v1/system/parity-evidence route.  Surface the result on the primary
    // status surface so operators cannot observe deployment_start_allowed=true
    // on a live-shadow or live-capital deployment without also seeing that
    // live_trust_complete=false in all current builds.
    //
    // live_trust_complete is non-null only when evidence is Present (incomplete
    // or complete).  null elsewhere is not a positive trust claim.
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

    (
        StatusCode::OK,
        Json(SystemStatusResponse {
            environment,
            daemon_mode: st.deployment_mode().as_api_label().to_string(),
            adapter_id: st.adapter_id().to_string(),
            deployment_start_allowed: st.deployment_readiness().start_allowed,
            deployment_blocker: st.deployment_readiness().blocker.clone(),
            runtime_status,
            broker_status,
            broker_snapshot_source: st.broker_snapshot_source().as_str().to_string(),
            alpaca_ws_continuity: st.alpaca_ws_continuity().await.as_status_str().to_string(),
            db_status,
            market_data_health: st.strategy_market_data_source().as_health_str().to_string(),
            reconcile_status,
            integrity_status,
            audit_writer_status,
            last_heartbeat: status.deadman_last_heartbeat_utc.clone(),
            deadman_status: status.deadman_status.clone(),
            loop_latency_ms: None,
            active_account_id: None,
            config_profile: None,
            has_warning,
            has_critical,
            strategy_armed: integrity_armed,
            execution_armed: integrity_armed,
            live_routing_enabled,
            kill_switch_active: status.state == "halted",
            risk_halt_active: risk_blocked,
            integrity_halt_active: !integrity_armed,
            daemon_reachable: true,
            fault_signals: build_fault_signals(&status, &reconcile, risk_blocked),
            autonomous_signal_count,
            autonomous_signal_limit_hit,
            // B8: Canonical asset-class scope.  Hardcoded constant — not derived
            // from runtime state.  Only equities are wired end-to-end on the
            // current canonical path; this field makes that boundary explicit and
            // machine-readable so operators and strategy tooling cannot mistake
            // the absence of non-equity support for active capability.
            asset_class_scope: "equity_only".to_string(),
            // C1: Live-trust surface.  Derived from parity evidence evaluator.
            // parity_evidence_state distinguishes "incomplete" (evidence present
            // but live_trust_complete=false) from "complete" (trust proven) so
            // operators see the explicit trust ceiling on the primary surface.
            parity_evidence_state,
            live_trust_complete,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/preflight
// ---------------------------------------------------------------------------

pub(crate) async fn system_preflight(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return runtime_error_response(err),
    };
    let (integrity_armed, integrity_halted, integrity_disarmed) = {
        let ig = st.integrity.read().await;
        (!ig.is_execution_blocked(), ig.halted, ig.disarmed)
    };

    let strategy_disarmed = !integrity_armed;
    let execution_disarmed = !integrity_armed;

    let db_reachable: Option<bool> = if let Some(db) = st.db.as_ref() {
        Some(sqlx::query("SELECT 1").execute(db).await.is_ok())
    } else {
        None
    };

    let broker_config_present: Option<bool> = match st.adapter_id() {
        "" | "null" | "paper" => Some(false),
        _ => Some(true),
    };

    // PT-MD-01: strategy market-data is explicitly not configured in this build.
    // StrategyMarketDataSource::NotConfigured is the only defined variant; derive
    // the value from the actual policy rather than returning null, which would
    // imply "not checked" when the honest answer is "checked and absent."
    let market_data_config_present: Option<bool> =
        Some(st.strategy_market_data_source().as_health_str() != "not_configured");
    let audit_writer_ready: Option<bool> = db_reachable;

    // AUTON-TRUTH-02: Autonomous-paper readiness fields for Paper+Alpaca.
    //
    // Populated by re-using the same gate logic that start_execution_runtime
    // enforces, so this surface can never appear green while a real start
    // would refuse.  None/empty for non-paper+alpaca deployments.
    let is_paper_alpaca = st.deployment_mode() == DeploymentMode::Paper
        && st.strategy_market_data_source() == StrategyMarketDataSource::ExternalSignalIngestion;

    let (
        ws_continuity_ready,
        reconcile_ready,
        autonomous_arm_state,
        autonomous_blockers,
        session_in_window,
    ) = if is_paper_alpaca {
        let ws_continuity = st.alpaca_ws_continuity().await;
        let ws_ready = ws_continuity.is_continuity_proven();

        let reconcile = st.current_reconcile_snapshot().await;
        let rec_ready = !matches!(reconcile.status.as_str(), "dirty" | "stale");

        let arm_state = if integrity_halted {
            "halted".to_string()
        } else if integrity_disarmed {
            "arm_pending".to_string()
        } else {
            "armed".to_string()
        };

        let schedule = autonomous_session_schedule_from_env();
        let in_window = schedule.is_in_session(&st, Utc::now()).await;

        let mut auto_blockers = Vec::new();
        if !ws_ready {
            auto_blockers.push(format!(
                "WS continuity not proven (current: '{}'); paper+alpaca requires \
                     WS continuity=live before starting (BRK-00R-04)",
                ws_continuity.as_status_str()
            ));
        }
        if !rec_ready {
            auto_blockers.push(format!(
                "reconcile status is '{}'; paper+alpaca cannot start with dirty or stale \
                     reconcile truth (BRK-09R)",
                reconcile.status
            ));
        }
        if integrity_halted {
            auto_blockers.push(
                "integrity arm state is 'halted'; operator must arm manually before \
                     autonomous start is permitted"
                    .to_string(),
            );
        }
        if !in_window {
            auto_blockers.push(
                "current time is outside the autonomous session window; the session \
                     controller will not attempt a start until the window opens"
                    .to_string(),
            );
        }

        (
            Some(ws_ready),
            Some(rec_ready),
            arm_state,
            auto_blockers,
            Some(in_window),
        )
    } else {
        (None, None, "not_applicable".to_string(), Vec::new(), None)
    };

    // C2: Thread live-trust truth into the preflight surface.
    //
    // Preflight is the primary operator pre-start checklist.  Without these
    // fields an operator could read `deployment_start_allowed=true` on a
    // live-shadow or live-capital deployment and have no indication that
    // `live_trust_complete=false` in all current builds.  C1 added this truth
    // to `/api/v1/system/status`; C2 closes the same gap on preflight so the
    // operator does not need to consult two surfaces to see the full picture.
    //
    // The same evaluator (`evaluate_parity_evidence_guarded`) is used here and
    // on the status + parity-evidence routes, so all three surfaces stay in sync.
    let parity_outcome_pf = evaluate_parity_evidence_guarded();
    let parity_evidence_state = match &parity_outcome_pf {
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
    let live_trust_complete = match &parity_outcome_pf {
        ParityEvidenceOutcome::Present {
            live_trust_complete,
            ..
        } => Some(*live_trust_complete),
        _ => None,
    };

    let mut warnings = Vec::new();
    if status.notes.is_some() {
        warnings.push("Daemon status contains notes; verify runtime state.".to_string());
    }

    let mut blockers = Vec::new();
    if db_reachable == Some(false) {
        blockers.push("Database is not reachable.".to_string());
    }
    if execution_disarmed {
        blockers.push("Execution is disarmed at the integrity gate.".to_string());
    }
    if let Some(blocker) = st.deployment_readiness().blocker.clone() {
        blockers.push(blocker);
    }
    // Surface autonomous blockers in the main blockers list so the GUI
    // preflight gate shows them as first-class startup blockers.
    for b in &autonomous_blockers {
        blockers.push(b.clone());
    }

    (
        StatusCode::OK,
        Json(PreflightStatusResponse {
            daemon_reachable: true,
            daemon_mode: st.deployment_mode().as_api_label().to_string(),
            adapter_id: st.adapter_id().to_string(),
            deployment_start_allowed: st.deployment_readiness().start_allowed,
            db_reachable,
            broker_config_present,
            market_data_config_present,
            audit_writer_ready,
            runtime_idle: Some(status.state != "running"),
            strategy_disarmed,
            execution_disarmed,
            live_routing_disabled: true,
            warnings,
            blockers,
            autonomous_readiness_applicable: is_paper_alpaca,
            ws_continuity_ready,
            reconcile_ready,
            autonomous_arm_state,
            autonomous_blockers,
            session_in_window,
            parity_evidence_state,
            live_trust_complete,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// AUTON-TRUTH-01: GET /api/v1/autonomous/readiness
// ---------------------------------------------------------------------------

/// Converts `AutonomousSessionTruth` to a (state_str, detail) pair for API surfaces.
fn autonomous_session_truth_to_api(truth: &AutonomousSessionTruth) -> (String, Option<String>) {
    match truth {
        AutonomousSessionTruth::Clear => ("clear".to_string(), None),
        AutonomousSessionTruth::StartRefused { detail } => {
            ("start_refused".to_string(), Some(detail.clone()))
        }
        AutonomousSessionTruth::RecoveryRetrying { detail, .. } => {
            ("recovery_retrying".to_string(), Some(detail.clone()))
        }
        AutonomousSessionTruth::RecoverySucceeded { detail, .. } => {
            ("recovery_succeeded".to_string(), Some(detail.clone()))
        }
        AutonomousSessionTruth::RecoveryFailed { detail, .. } => {
            ("recovery_failed".to_string(), Some(detail.clone()))
        }
        AutonomousSessionTruth::RunEndedUnexpectedly { detail } => {
            ("run_ended_unexpectedly".to_string(), Some(detail.clone()))
        }
        AutonomousSessionTruth::StopFailed { detail } => {
            ("stop_failed".to_string(), Some(detail.clone()))
        }
        AutonomousSessionTruth::StoppedAtBoundary { detail } => {
            ("stopped_at_boundary".to_string(), Some(detail.clone()))
        }
    }
}

/// AUTON-TRUTH-01: Autonomous-paper readiness truth surface.
///
/// Surfaces the live gate state that governs whether the session controller
/// can start an execution run.  All values are derived from in-memory daemon
/// state; no DB queries are issued.  Returns `truth_state = "not_applicable"`
/// for non-paper+alpaca deployments.
pub(crate) async fn autonomous_readiness(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let is_paper_alpaca = st.deployment_mode() == DeploymentMode::Paper
        && st.strategy_market_data_source() == StrategyMarketDataSource::ExternalSignalIngestion;

    if !is_paper_alpaca {
        return (
            StatusCode::OK,
            Json(AutonomousPaperReadinessResponse {
                canonical_route: "/api/v1/autonomous/readiness".to_string(),
                truth_state: "not_applicable".to_string(),
                canonical_path: false,
                ws_continuity: st.alpaca_ws_continuity().await.as_status_str().to_string(),
                ws_continuity_ready: false,
                reconcile_status: "not_applicable".to_string(),
                reconcile_ready: false,
                autonomous_session_state: "not_applicable".to_string(),
                autonomous_session_detail: None,
                arm_state: "not_applicable".to_string(),
                arm_ready: false,
                signal_ingestion_configured: false,
                session_in_window: false,
                session_window_state: "not_applicable".to_string(),
                runtime_start_allowed: false,
                blockers: vec![
                    "deployment is not paper+alpaca; autonomous readiness only applies to \
                     the canonical Paper+Alpaca path"
                        .to_string(),
                ],
                overall_ready: false,
                autonomous_history_degraded: false,
            }),
        )
            .into_response();
    }

    // Gather live gate state from AppState in the same order that
    // start_execution_runtime enforces its gates.

    let ws_continuity = st.alpaca_ws_continuity().await;
    let ws_continuity_str = ws_continuity.as_status_str().to_string();
    let ws_continuity_ready = ws_continuity.is_continuity_proven();

    let reconcile = st.current_reconcile_snapshot().await;
    let reconcile_status_str = reconcile.status.clone();
    let reconcile_ready = !matches!(reconcile_status_str.as_str(), "dirty" | "stale");

    let autonomous_truth = st.autonomous_session_truth().await;
    let (autonomous_state_str, autonomous_detail) =
        autonomous_session_truth_to_api(&autonomous_truth);

    let (arm_state, arm_ready) = {
        let ig = st.integrity.read().await;
        if ig.halted {
            ("halted".to_string(), false)
        } else if ig.disarmed {
            // In-memory disarmed but not halted.  The session controller calls
            // try_autonomous_arm which checks the DB; if the prior session ended
            // cleanly (DB=ARMED), it will advance to armed automatically.
            // Surface as "arm_pending" — not yet armed, but may self-heal on the
            // next controller tick without operator intervention.
            ("arm_pending".to_string(), false)
        } else {
            ("armed".to_string(), true)
        }
    };

    let signal_ingestion_configured =
        st.strategy_market_data_source() == StrategyMarketDataSource::ExternalSignalIngestion;

    // Session-window truth: derive from the configured schedule.
    let schedule = autonomous_session_schedule_from_env();
    let session_in_window = schedule.is_in_session(&st, Utc::now()).await;
    let session_window_state = if session_in_window {
        "in_window".to_string()
    } else {
        "outside_window".to_string()
    };

    // Runtime-start truth: a locally-owned run blocks start (409 Conflict).
    let runtime_start_allowed = st.locally_owned_run_id().await.is_none();

    // Build blockers in gate order matching start_execution_runtime.
    let mut blockers = Vec::new();
    if !ws_continuity_ready {
        blockers.push(format!(
            "WS continuity not proven (current: '{}'); paper+alpaca requires \
             WS continuity=live before starting (BRK-00R-04)",
            ws_continuity_str
        ));
    }
    if !reconcile_ready {
        blockers.push(format!(
            "reconcile status is '{}'; paper+alpaca cannot start with dirty or stale \
             reconcile truth (BRK-09R)",
            reconcile_status_str
        ));
    }
    if !arm_ready {
        match arm_state.as_str() {
            "halted" => blockers.push(
                "integrity arm state is 'halted'; operator must arm manually before \
                 autonomous start is permitted"
                    .to_string(),
            ),
            "arm_pending" => blockers.push(
                "integrity is disarmed in memory; the session controller will call \
                 try_autonomous_arm on the next tick (DB-ARMED → auto-advances to armed)"
                    .to_string(),
            ),
            _ => {}
        }
    }
    if !signal_ingestion_configured {
        blockers.push(
            "ExternalSignalIngestion is not configured; signal ingestion path is absent"
                .to_string(),
        );
    }
    if !session_in_window {
        blockers.push(
            "current time is outside the autonomous session window; the session controller \
             will not attempt a start until the window opens"
                .to_string(),
        );
    }
    if !runtime_start_allowed {
        blockers.push(
            "a locally-owned execution run is already active; start would return 409 Conflict \
             — the session controller will not attempt a new start"
                .to_string(),
        );
    }

    let overall_ready = ws_continuity_ready
        && reconcile_ready
        && arm_ready
        && signal_ingestion_configured
        && session_in_window
        && runtime_start_allowed;

    let autonomous_history_degraded = st.autonomous_history_degraded();

    (
        StatusCode::OK,
        Json(AutonomousPaperReadinessResponse {
            canonical_route: "/api/v1/autonomous/readiness".to_string(),
            truth_state: "active".to_string(),
            canonical_path: true,
            ws_continuity: ws_continuity_str,
            ws_continuity_ready,
            reconcile_status: reconcile_status_str,
            reconcile_ready,
            autonomous_session_state: autonomous_state_str,
            autonomous_session_detail: autonomous_detail,
            arm_state,
            arm_ready,
            signal_ingestion_configured,
            session_in_window,
            session_window_state,
            runtime_start_allowed,
            blockers,
            overall_ready,
            autonomous_history_degraded,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/metadata
// ---------------------------------------------------------------------------

pub(crate) async fn system_metadata(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let integrity_armed = {
        let ig = st.integrity.read().await;
        !ig.is_execution_blocked()
    };
    let endpoint_status = if integrity_armed { "ok" } else { "warning" }.to_string();

    (
        StatusCode::OK,
        Json(SystemMetadataResponse {
            build_version: st.build.version.to_string(),
            api_version: "v1".to_string(),
            broker_adapter: st.adapter_id().to_string(),
            endpoint_status,
            daemon_mode: st.deployment_mode().as_api_label().to_string(),
            adapter_id: st.adapter_id().to_string(),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/runtime-leadership
// ---------------------------------------------------------------------------

pub(crate) async fn system_runtime_leadership(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return runtime_error_response(err),
    };
    let reconcile = st.current_reconcile_snapshot().await;

    let leader_node = "local".to_string();
    let leader_lease_state = match status.state.as_str() {
        "running" => "held",
        "unknown" => "contested",
        _ => "lost",
    }
    .to_string();

    let latest_run = if let Some(db) = st.db.as_ref() {
        mqk_db::fetch_latest_run_for_engine(db, DAEMON_ENGINE_ID, st.deployment_mode().as_db_mode())
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    let generation_id = status
        .active_run_id
        .map(|id| id.to_string())
        .or_else(|| latest_run.as_ref().map(|r| r.run_id.to_string()));

    let last_restart_at = latest_run.as_ref().map(|r| r.started_at_utc.to_rfc3339());

    let post_restart_recovery_state = match reconcile.status.as_str() {
        "ok" => "complete",
        "unknown" => "in_progress",
        _ => "degraded",
    }
    .to_string();

    let recovery_checkpoint = reconcile
        .last_run_at
        .as_deref()
        .unwrap_or("none")
        .to_string();

    let mut checkpoints: Vec<RuntimeLeadershipCheckpointRow> = Vec::new();
    if let Some(run) = &latest_run {
        checkpoints.push(RuntimeLeadershipCheckpointRow {
            checkpoint_id: run.run_id.to_string(),
            checkpoint_type: "restart".to_string(),
            timestamp: run.started_at_utc.to_rfc3339(),
            generation_id: run.run_id.to_string(),
            leader_node: leader_node.clone(),
            status: "ok".to_string(),
            note: format!(
                "Run started; mode={}; adapter={}",
                st.deployment_mode().as_api_label(),
                st.adapter_id()
            ),
        });
    }

    (
        StatusCode::OK,
        Json(RuntimeLeadershipResponse {
            leader_node,
            leader_lease_state,
            generation_id,
            restart_count_24h: if let Some(db) = st.db.as_ref() {
                mqk_db::count_runs_in_last_24h(
                    db,
                    DAEMON_ENGINE_ID,
                    st.deployment_mode().as_db_mode(),
                )
                .await
                .ok()
                .map(|n| n as u32)
            } else {
                None
            },
            last_restart_at,
            post_restart_recovery_state,
            recovery_checkpoint,
            checkpoints,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/session
// ---------------------------------------------------------------------------

pub(crate) async fn system_session(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let status = match st.current_status_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(err) => return runtime_error_response(err),
    };
    let strategy_allowed = status.integrity_armed;
    let execution_allowed =
        strategy_allowed && status.state == "running" && status.active_run_id.is_some();

    // C4: Live-trust truth on the session surface.
    //
    // `/api/v1/system/session` is the lightweight operator "can I execute now?"
    // check.  Without C4 an operator consulting only this surface on a
    // live-shadow or live-capital deployment would see `deployment_start_allowed`
    // with no visibility into the live-trust ceiling.  Status (C1), preflight
    // (C2), and mode-change-guidance (C3) already carry these fields; session is
    // the final primary operator surface that was missing them.
    //
    // The same `evaluate_parity_evidence_guarded()` evaluator is used across all
    // four surfaces so they cannot diverge.
    let parity_outcome_sess = evaluate_parity_evidence_guarded();
    let parity_evidence_state = match &parity_outcome_sess {
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
    let live_trust_complete = match &parity_outcome_sess {
        ParityEvidenceOutcome::Present {
            live_trust_complete,
            ..
        } => Some(*live_trust_complete),
        _ => None,
    };

    let calendar = st.calendar_spec();
    // AUTON-CALENDAR-01: use session_now_ts() so test-injected clocks propagate to
    // this display surface.  In production the override is None and it falls through
    // to Utc::now().timestamp() — identical behavior, but now hermetically testable.
    let now_ts = st.session_now_ts().await;
    (
        StatusCode::OK,
        Json(SessionStateResponse {
            daemon_mode: st.deployment_mode().as_db_mode().to_string(),
            adapter_id: st.adapter_id().to_string(),
            deployment_start_allowed: st.deployment_readiness().start_allowed,
            deployment_blocker: st.deployment_readiness().blocker.clone(),
            operator_auth_mode: st.operator_auth_mode().label().to_string(),
            strategy_allowed,
            execution_allowed,
            system_trading_window: if execution_allowed {
                "enabled".to_string()
            } else {
                "disabled".to_string()
            },
            market_session: calendar.classify_market_session(now_ts).to_string(),
            exchange_calendar_state: calendar.classify_exchange_calendar(now_ts).to_string(),
            calendar_spec_id: calendar.spec_id().to_string(),
            notes: vec![calendar.session_truth_note().to_string()],
            // C4: Live-trust ceiling fields — same evaluator as C1/C2/C3.
            parity_evidence_state,
            live_trust_complete,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/config-fingerprint
// ---------------------------------------------------------------------------

pub(crate) async fn system_config_fingerprint(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let latest_run = if let Some(db) = st.db.as_ref() {
        mqk_db::fetch_latest_run_for_engine(db, DAEMON_ENGINE_ID, st.deployment_mode().as_db_mode())
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    (
        StatusCode::OK,
        Json(ConfigFingerprintResponse {
            config_hash: latest_run
                .as_ref()
                .map(|run| run.config_hash.clone())
                .unwrap_or_else(|| st.run_config_hash().to_string()),
            adapter_id: st.adapter_id().to_string(),
            risk_policy_version: None,
            strategy_bundle_version: None,
            build_version: st.build.version.to_string(),
            environment_profile: st.deployment_mode().as_api_label().to_string(),
            runtime_generation_id: latest_run.as_ref().map(|run| run.run_id.to_string()),
            last_restart_at: latest_run
                .as_ref()
                .map(|run| run.started_at_utc.to_rfc3339()),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/config-diffs
// ---------------------------------------------------------------------------

pub(crate) async fn system_config_diffs(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(ConfigDiffsResponse {
                canonical_route: "/api/v1/system/config-diffs".to_string(),
                truth_state: "not_wired".to_string(),
                backend: "not_wired".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    let latest_run = match sqlx::query(
        r#"
        select
          run_id,
          engine_id,
          mode,
          started_at_utc,
          git_hash,
          config_hash,
          config_json,
          host_fingerprint,
          status,
          armed_at_utc,
          running_at_utc,
          stopped_at_utc,
          halted_at_utc,
          last_heartbeat_utc
        from runs
        where engine_id = $1
        order by started_at_utc desc, run_id desc
        limit 1
        "#,
    )
    .bind(DAEMON_ENGINE_ID)
    .fetch_optional(db)
    .await
    {
        Ok(Some(row)) => {
            let status = match mqk_db::RunStatus::parse(&row.get::<String, _>("status")) {
                Ok(status) => status,
                Err(err) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(RuntimeErrorResponse {
                            error: format!("system/config-diffs status parse failed: {err}"),
                            fault_class: "system.config_diffs.status_parse_failed".to_string(),
                            gate: None,
                        }),
                    )
                        .into_response();
                }
            };

            Some(mqk_db::RunRow {
                run_id: row.get("run_id"),
                engine_id: row.get("engine_id"),
                mode: row.get("mode"),
                started_at_utc: row.get("started_at_utc"),
                git_hash: row.get("git_hash"),
                config_hash: row.get("config_hash"),
                config_json: row.get("config_json"),
                host_fingerprint: row.get("host_fingerprint"),
                status,
                armed_at_utc: row.get("armed_at_utc"),
                running_at_utc: row.get("running_at_utc"),
                stopped_at_utc: row.get("stopped_at_utc"),
                halted_at_utc: row.get("halted_at_utc"),
                last_heartbeat_utc: row.get("last_heartbeat_utc"),
            })
        }
        Ok(None) => None,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("system/config-diffs query failed: {err}"),
                    fault_class: "system.config_diffs.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response();
        }
    };

    let Some(latest_run) = latest_run else {
        return (
            StatusCode::OK,
            Json(ConfigDiffsResponse {
                canonical_route: "/api/v1/system/config-diffs".to_string(),
                truth_state: "not_wired".to_string(),
                backend: "not_wired".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    let rows = authoritative_config_diff_rows(&st, &latest_run);

    (
        StatusCode::OK,
        Json(ConfigDiffsResponse {
            canonical_route: "/api/v1/system/config-diffs".to_string(),
            truth_state: "active".to_string(),
            backend: "postgres.runs+daemon.runtime_selection".to_string(),
            rows,
        }),
    )
        .into_response()
}

fn authoritative_config_diff_rows(
    st: &AppState,
    latest_run: &mqk_db::RunRow,
) -> Vec<ConfigDiffRow> {
    let mut rows = Vec::new();
    let changed_at = latest_run.started_at_utc.to_rfc3339();

    if latest_run.config_hash != st.run_config_hash() {
        rows.push(ConfigDiffRow {
            diff_id: format!("{}:config_hash", latest_run.run_id),
            changed_at: changed_at.clone(),
            changed_domain: "config".to_string(),
            before_version: latest_run.config_hash.clone(),
            after_version: st.run_config_hash().to_string(),
            summary: format!(
                "current daemon config_hash differs from latest durable run {}",
                latest_run.run_id
            ),
        });
    }

    if latest_run.mode != st.deployment_mode().as_db_mode() {
        rows.push(ConfigDiffRow {
            diff_id: format!("{}:deployment_mode", latest_run.run_id),
            changed_at: changed_at.clone(),
            changed_domain: "runtime".to_string(),
            before_version: latest_run.mode.clone(),
            after_version: st.deployment_mode().as_db_mode().to_string(),
            summary: format!(
                "current daemon deployment mode differs from latest durable run {}",
                latest_run.run_id
            ),
        });
    }

    if let Some(prior_adapter) = latest_run
        .config_json
        .get("adapter")
        .and_then(|value| value.as_str())
    {
        if prior_adapter != st.adapter_id() {
            rows.push(ConfigDiffRow {
                diff_id: format!("{}:adapter", latest_run.run_id),
                changed_at,
                changed_domain: "runtime".to_string(),
                before_version: prior_adapter.to_string(),
                after_version: st.adapter_id().to_string(),
                summary: format!(
                    "current daemon adapter differs from latest durable run {}",
                    latest_run.run_id
                ),
            });
        }
    }

    rows
}
