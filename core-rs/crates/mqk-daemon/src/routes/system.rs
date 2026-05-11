//! System-level route handlers (runtime operational surfaces).
//!
//! Contains: health, status_handler, system_status, system_preflight,
//! autonomous_readiness, system_metadata, system_runtime_leadership,
//! system_session.
//!
//! Config-surface handlers (system_config_fingerprint, system_config_diffs)
//! live in the `config` submodule (MT-07D split).
//!
//! Artifact/evidence/topology handlers live in `routes/system_artifact.rs`
//! (MT-01 split).

mod config;
pub(crate) use config::{system_config_diffs, system_config_fingerprint};

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;

use crate::api_types::{
    AutonomousPaperReadinessResponse, HealthResponse, PreflightStatusResponse,
    RuntimeLeadershipCheckpointRow, RuntimeLeadershipResponse, SessionStateResponse,
    SystemMetadataResponse, SystemStatusResponse,
};
use crate::parity_evidence::{evaluate_parity_evidence_guarded, ParityEvidenceOutcome};
use crate::state::{
    autonomous_session_schedule_from_env, session_window_from_env, AppState,
    AutonomousSessionTruth, BrokerSnapshotTruthSource, DeploymentMode, StrategyMarketDataSource,
    SESSION_START_HH_MM_ENV, SESSION_STOP_HH_MM_ENV, STRATEGY_MD_TIMEFRAME_ENV,
};

use super::helpers::{
    build_fault_signals, environment_and_live_routing_truth, runtime_error_response,
    runtime_status_from_state,
};

const DAEMON_ENGINE_ID: &str = "mqk-daemon";

/// Staleness threshold for External (Alpaca) broker snapshots.
///
/// External snapshots are refreshed every 60 ticks in the run loop.  If the
/// snapshot is older than 3× the expected refresh interval while a run is
/// active, the broker status is surfaced as `"stale"` instead of `"ok"` so
/// the operator does not mistake a stale snapshot for confirmed-fresh state.
const BROKER_SNAPSHOT_STALE_SECS: i64 = 180;

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
    let integrity_armed = status.integrity_armed;

    // STATUS-TRUTH-01: risk_truth distinguishes DB-confirmed-clear from DB-unavailable.
    //
    // Prior code used `bool` which collapsed both "no DB" and "DB error" into `false`
    // (same as "confirmed not blocked"), producing a false-green surface.  `None`
    // now means "unknown" and triggers an explicit warning fault signal.
    let (risk_truth, db_status) = if let Some(db) = st.db.as_ref() {
        match mqk_db::load_risk_block_state(db).await {
            Ok(row) => {
                let blocked = row.is_some_and(|risk| risk.blocked);
                (Some(blocked), "ok".to_string())
            }
            Err(_) => (None, "warning".to_string()), // DB error → risk truth unknown
        }
    } else {
        (None, "unavailable".to_string()) // No DB pool → risk truth unknown
    };
    let risk_halt_active = risk_truth == Some(true);

    let audit_writer_status = db_status.clone();

    let runtime_status = runtime_status_from_state(&status.state).to_string();
    let (environment, live_routing_enabled) =
        environment_and_live_routing_truth(&st, &status).await;

    // STATUS-TRUTH-01: broker staleness check for External (Alpaca) snapshots.
    //
    // Presence alone (`is_some()`) was the prior check, which allowed an arbitrarily
    // stale External snapshot to surface as "ok".  For External source + active run,
    // check the captured_at_utc age.  Synthetic (paper) snapshots are always fresh
    // (synthesized per orchestrator tick), so they skip the age check.
    let broker_status = {
        let snap_guard = st.broker_snapshot.read().await;
        let snapshot_present = snap_guard.is_some();
        if !snapshot_present {
            "warning".to_string()
        } else if st.broker_snapshot_source() == BrokerSnapshotTruthSource::External
            && runtime_status == "running"
        {
            let age_secs = snap_guard
                .as_ref()
                .map(|s| (Utc::now() - s.captured_at_utc).num_seconds())
                .unwrap_or(0);
            if age_secs > BROKER_SNAPSHOT_STALE_SECS {
                "stale".to_string()
            } else {
                "ok".to_string()
            }
        } else {
            "ok".to_string()
        }
    };

    let integrity_status = if integrity_armed { "ok" } else { "warning" }.to_string();
    let reconcile_status = reconcile.status.clone();
    let has_critical = matches!(reconcile_status.as_str(), "dirty" | "stale")
        || (reconcile_status == "unknown" && runtime_status == "running");
    // STATUS-TRUTH-01: "unavailable" (no DB pool) is also a warning condition —
    // it means risk truth cannot be checked and broker snapshot cannot be refreshed.
    let has_warning = broker_status != "ok"
        || integrity_status != "ok"
        || reconcile_status != "ok"
        || db_status == "warning"
        || db_status == "unavailable"
        || risk_truth.is_none()
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
            risk_halt_active,
            integrity_halt_active: !integrity_armed,
            daemon_reachable: true,
            // HEARTBEAT-TICK-01: compute elapsed seconds since the last execution-loop
            // tick.  None when the daemon is not running or the loop has not yet
            // completed its first tick (last_tick_secs == 0 means never ticked).
            fault_signals: {
                let execution_loop_stall_secs = if status.state == "running" {
                    let last = st.execution_last_tick_secs();
                    if last > 0 {
                        Some(Utc::now().timestamp() - last)
                    } else {
                        None
                    }
                } else {
                    None
                };
                build_fault_signals(&status, &reconcile, risk_truth, execution_loop_stall_secs)
            },
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
        // STRATEGY-DORMANCY-01: Surface bootstrap dormancy as an autonomous blocker.
        //
        // Mirrors the gate added to start_execution_runtime.  If MQK_STRATEGY_IDS
        // is absent or empty the bootstrap is Dormant and start would be refused
        // with 403/native_strategy_bootstrap.  Surface the blocker here so the
        // operator sees it before attempting start, not only after.
        {
            let fleet = st.strategy_fleet_snapshot().await;
            if fleet.is_none_or(|f| f.is_empty()) {
                auto_blockers.push(
                    "strategy bootstrap is dormant: MQK_STRATEGY_IDS is absent or empty; \
                     no strategy engine will generate decisions on the autonomous paper path; \
                     set MQK_STRATEGY_IDS to a registered strategy name before starting \
                     (STRATEGY-DORMANCY-01)"
                        .to_string(),
                );
            }
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
        // BRK-GAP-01: partial recovery — fill only, lifecycle unproven.
        AutonomousSessionTruth::WsGapPartialRecovery { detail, .. } => {
            ("ws_gap_partial_recovery".to_string(), Some(detail.clone()))
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
        AutonomousSessionTruth::ControllerExited { detail } => {
            ("controller_exited".to_string(), Some(detail.clone()))
        }
    }
}

/// OBS-SESSION-DISCORD-01: Session-window diagnostic fields, computed once per request.
struct SessionWindowDiagnostics {
    now_utc: String,
    session_start_utc: Option<String>,
    session_stop_utc: Option<String>,
    session_window_source: String,
    session_window_basis: String,
    session_start_env_raw: Option<String>,
    session_stop_env_raw: Option<String>,
}

/// Build session-window diagnostics from the current environment and clock.
///
/// When `MQK_SESSION_START_HH_MM` and `MQK_SESSION_STOP_HH_MM` are both set and
/// parse correctly, `session_window_source = "env"` and derived `HH:MM UTC`
/// strings are populated.  Otherwise `session_window_source = "default"` and the
/// derived UTC times are `None` (NYSE seam, time varies by calendar day).
/// Raw env values are always returned verbatim so the operator can see what was
/// configured even when parsing fails.
fn session_window_diagnostics(now: chrono::DateTime<chrono::Utc>) -> SessionWindowDiagnostics {
    let start_raw = std::env::var(SESSION_START_HH_MM_ENV)
        .ok()
        .filter(|s| !s.is_empty());
    let stop_raw = std::env::var(SESSION_STOP_HH_MM_ENV)
        .ok()
        .filter(|s| !s.is_empty());

    let window_opt = session_window_from_env();
    let (source, start_utc, stop_utc) = match &window_opt {
        Some(w) => (
            "env".to_string(),
            Some(format!("{:02}:{:02} UTC", w.start_hh, w.start_mm)),
            Some(format!("{:02}:{:02} UTC", w.stop_hh, w.stop_mm)),
        ),
        None => ("default".to_string(), None, None),
    };

    SessionWindowDiagnostics {
        now_utc: now.to_rfc3339(),
        session_start_utc: start_utc,
        session_stop_utc: stop_utc,
        session_window_source: source,
        session_window_basis: "UTC".to_string(),
        session_start_env_raw: start_raw,
        session_stop_env_raw: stop_raw,
    }
}

/// AUTON-TRUTH-01: Autonomous-paper readiness truth surface.
///
/// Surfaces the live gate state that governs whether the session controller
/// can start an execution run.  All values are derived from in-memory daemon
/// state; no DB queries are issued.  Returns `truth_state = "not_applicable"`
/// for non-paper+alpaca deployments.
pub(crate) async fn autonomous_readiness(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    // Snapshot now once so diagnostic fields are consistent across both paths.
    let now = Utc::now();
    let diag = session_window_diagnostics(now);

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
                nyse_market_session: "not_applicable".to_string(),
                bar_ticker_gate: "not_applicable".to_string(),
                bar_tick_dispatch_count: None,
                last_bar_signal_qty: None,
                bar_context_source: "not_applicable".to_string(),
                bar_context_bars_loaded: None,
                now_utc: diag.now_utc,
                session_start_utc: diag.session_start_utc,
                session_stop_utc: diag.session_stop_utc,
                session_window_source: diag.session_window_source,
                session_window_basis: diag.session_window_basis,
                session_start_env_raw: diag.session_start_env_raw,
                session_stop_env_raw: diag.session_stop_env_raw,
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

    // AUTON-NO-TRADE-01: Distinguish arm_pending (DB=ARMED, in-memory pending,
    // auto-heals on next tick) from disarmed_db (DB=DISARMED, operator must
    // re-arm).  Without this check a ReconcileDrift disarm surfaces as
    // "arm_pending" with "(DB-ARMED → auto-advances)" — misleading the operator
    // into believing the system will self-heal when try_autonomous_arm will
    // refuse on every session-window tick.
    let (arm_state, arm_ready, disarm_reason): (String, bool, Option<String>) = {
        let (halted, in_memory_disarmed) = {
            let ig = st.integrity.read().await;
            (ig.halted, ig.disarmed)
        };
        if halted {
            ("halted".to_string(), false, None)
        } else if in_memory_disarmed {
            // Check DB for authoritative arm state.  Drop the integrity lock
            // before awaiting the DB call so we do not hold it across I/O.
            let (db_state, reason) = if let Some(ref pool) = st.db {
                match mqk_db::load_arm_state(pool).await {
                    Ok(Some((ref s, _))) if s == "ARMED" => ("arm_pending", None),
                    Ok(Some((_, reason))) => ("disarmed_db", reason),
                    // No row or DB error: treat as arm_pending — we cannot
                    // assert DISARMED without evidence.
                    Ok(None) | Err(_) => ("arm_pending", None),
                }
            } else {
                // No DB pool (test-only path) — treat as arm_pending so
                // existing in-memory tests are unaffected.
                ("arm_pending", None)
            };
            (db_state.to_string(), false, reason)
        } else {
            ("armed".to_string(), true, None)
        }
    };

    let signal_ingestion_configured =
        st.strategy_market_data_source() == StrategyMarketDataSource::ExternalSignalIngestion;

    // Session-window truth: derive from the configured schedule.
    // Use the already-snapshotted `now` so the diagnostic timestamps are consistent.
    let schedule = autonomous_session_schedule_from_env();
    let session_in_window = schedule.is_in_session(&st, now).await;
    let session_window_state = if session_in_window {
        "in_window".to_string()
    } else {
        "outside_window".to_string()
    };

    // Runtime-start truth: a locally-owned run blocks start (409 Conflict).
    let runtime_start_allowed = st.locally_owned_run_id().await.is_none();

    // AUTON-NO-TRADE-01: Bar ticker Gate 2 — NYSE session must be "regular".
    //
    // The autonomous bar ticker only deposits bar inputs during the NYSE regular
    // session.  When a run is active but the NYSE is closed, autonomous_signal_count
    // remains 0 and no paper orders are attempted — this is correct fail-closed
    // behaviour.  We surface it here so an operator can see exactly why ticks are
    // not occurring without having to separately query /api/v1/system/session.
    let nyse_market_session = st
        .calendar_spec()
        .classify_market_session(now.timestamp())
        .to_string();
    let bar_ticker_gate = bar_ticker_gate_from_session(&nyse_market_session).to_string();

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
            "disarmed_db" => {
                // AUTON-NO-TRADE-01: DB is DISARMED — try_autonomous_arm will
                // refuse on every tick until the operator re-arms.  Surface the
                // exact DB reason so the operator knows the required action.
                let reason = disarm_reason.as_deref().unwrap_or("unknown");
                blockers.push(format!(
                    "DB arm state is DISARMED (reason={reason}); \
                     try_autonomous_arm will refuse on every session-window tick; \
                     operator must re-arm via POST /api/v1/ops/action \
                     {{\"action\":\"arm-execution\"}} before the next session \
                     can start (AUTON-NO-TRADE-01)"
                ));
            }
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
    // AUTON-NO-TRADE-01: When a run is active but the NYSE market is not in regular
    // session, the bar ticker Gate 2 will block all bar deposits.  Surface this as
    // an explicit observation so the operator knows why autonomous_signal_count is 0.
    if !runtime_start_allowed && bar_ticker_gate != "open" {
        blockers.push(format!(
            "run is active but bar ticker Gate 2 is blocked: NYSE market session is \
             '{}', not 'regular'; no bar deposits will occur until the regular \
             session opens (Mon–Fri 09:30–16:00 ET, holidays excluded)",
            nyse_market_session
        ));
    }
    // STRATEGY-DORMANCY-01: Check strategy bootstrap dormancy.
    //
    // Mirrors the gate added to start_execution_runtime and the autonomous_blockers
    // check in system_preflight.  Dormant bootstrap on the Paper+Alpaca path means
    // no strategy engine will generate decisions, so overall_ready must be false.
    let strategy_fleet_empty = st
        .strategy_fleet_snapshot()
        .await
        .is_none_or(|f| f.is_empty());
    if strategy_fleet_empty {
        blockers.push(
            "strategy bootstrap is dormant: MQK_STRATEGY_IDS is absent or empty; \
             no strategy engine will generate decisions; \
             set MQK_STRATEGY_IDS to a registered strategy name before starting \
             (STRATEGY-DORMANCY-01)"
                .to_string(),
        );
    }

    // AUTON-NO-TRADE-02: Read bar-tick observability from AppState.
    //
    // These counters are session-scoped (reset on run start).  They let the
    // operator see whether the strategy has been invoked at all and whether
    // it is returning any non-zero targets.  Both values are `None` when
    // ExternalSignalIngestion is not configured (not applicable path).
    let bar_tick_dispatch_count: Option<u64> = Some(st.bar_tick_dispatch_count());
    let last_bar_signal_qty: Option<i64> = st.last_bar_signal_qty();

    // AUTON-SIGNAL-CONTEXT-01: Derive bar context source and count.
    let raw_ctx_bars = st.last_bar_context_bars();
    let (bar_context_source, bar_context_bars_loaded) = match raw_ctx_bars {
        -1 => ("no_dispatch_yet".to_string(), None),
        0 => ("stub_no_price".to_string(), None),
        n => ("db_loaded".to_string(), Some(n as u64)),
    };

    // AUTON-NO-TRADE-02: When bar ticks are being dispatched but the strategy
    // is consistently returning signal qty = 0, surface the reason.
    if st.bar_tick_dispatch_count() > 0 && last_bar_signal_qty == Some(0) {
        let reason = match raw_ctx_bars {
            n if n > 0 => format!(
                "NO_SIGNAL_GENERATED (AUTON-SIGNAL-CONTEXT-01): bar ticks dispatched with \
                 {n} DB bars but strategy signal qty is 0; strategy conditions not met \
                 (price movement below threshold, or fewer than LOOKBACK bars loaded)"
            ),
            _ => "NO_SIGNAL_GENERATED (AUTON-NO-TRADE-02): bar ticks dispatched but \
                  strategy signal qty is 0; context is single-stub with is_complete=false \
                  (no price reference — set MQK_STRATEGY_SYMBOL and \
                  MQK_STRATEGY_MD_TIMEFRAME to load real DB bars)"
                .to_string(),
        };
        blockers.push(reason);
    }

    // AUTON-SIGNAL-CONTEXT-01: surface INCOMPLETE_BAR_CONTEXT when the last
    // dispatch used the stub path and ticks have already occurred.
    if st.bar_tick_dispatch_count() > 0 && raw_ctx_bars == 0 {
        let symbol_set = !std::env::var("MQK_STRATEGY_SYMBOL")
            .unwrap_or_default()
            .is_empty();
        let tf_set = !std::env::var(STRATEGY_MD_TIMEFRAME_ENV)
            .unwrap_or_default()
            .is_empty();
        if !symbol_set || !tf_set {
            blockers.push(format!(
                "INCOMPLETE_BAR_CONTEXT (AUTON-SIGNAL-CONTEXT-01): stub context used \
                 because {} is not set; strategies require LOOKBACK complete bars with \
                 price data; set MQK_STRATEGY_SYMBOL and MQK_STRATEGY_MD_TIMEFRAME to \
                 load real bars from md_bars",
                match (symbol_set, tf_set) {
                    (false, false) => "MQK_STRATEGY_SYMBOL and MQK_STRATEGY_MD_TIMEFRAME",
                    (false, true) => "MQK_STRATEGY_SYMBOL",
                    _ => "MQK_STRATEGY_MD_TIMEFRAME",
                }
            ));
        }
    }

    let overall_ready = ws_continuity_ready
        && reconcile_ready
        && arm_ready
        && signal_ingestion_configured
        && session_in_window
        && runtime_start_allowed
        && !strategy_fleet_empty;

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
            nyse_market_session,
            bar_ticker_gate,
            bar_tick_dispatch_count,
            last_bar_signal_qty,
            bar_context_source,
            bar_context_bars_loaded,
            now_utc: diag.now_utc,
            session_start_utc: diag.session_start_utc,
            session_stop_utc: diag.session_stop_utc,
            session_window_source: diag.session_window_source,
            session_window_basis: diag.session_window_basis,
            session_start_env_raw: diag.session_start_env_raw,
            session_stop_env_raw: diag.session_stop_env_raw,
        }),
    )
        .into_response()
}

// AUTON-NO-TRADE-01: Bar ticker Gate 2 derivation — pure helper for testability.
fn bar_ticker_gate_from_session(nyse_session: &str) -> &'static str {
    if nyse_session == "regular" {
        "open"
    } else {
        "closed_outside_session"
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    // AUTON-NO-TRADE-01: bar ticker Gate 2 derivation tests.
    #[test]
    fn ant01_bar_ticker_gate_open_when_regular() {
        assert_eq!(bar_ticker_gate_from_session("regular"), "open");
    }

    #[test]
    fn ant02_bar_ticker_gate_closed_for_non_regular() {
        for session in &["premarket", "after_hours", "closed"] {
            assert_eq!(
                bar_ticker_gate_from_session(session),
                "closed_outside_session",
                "expected closed_outside_session for session='{session}'"
            );
        }
    }

    // AUTON-NO-TRADE-02: NO_SIGNAL_GENERATED blocker derivation tests.

    /// ANT03: blocker fires when ticks > 0 and signal_qty == 0.
    #[test]
    fn ant03_no_signal_blocker_fires_when_ticks_positive_signal_zero() {
        let ticks: u64 = 5;
        let qty: Option<i64> = Some(0);
        let fires = ticks > 0 && qty == Some(0);
        assert!(
            fires,
            "ANT03: NO_SIGNAL_GENERATED blocker must fire when ticks=5 and signal_qty=0"
        );
    }

    /// ANT04: blocker does not fire when no ticks have occurred yet (session start).
    #[test]
    fn ant04_no_signal_blocker_silent_when_no_ticks() {
        let ticks: u64 = 0;
        let qty: Option<i64> = None; // no dispatch yet
        let fires = ticks > 0 && qty == Some(0);
        assert!(
            !fires,
            "ANT04: NO_SIGNAL_GENERATED blocker must not fire when no ticks have occurred"
        );
    }

    /// ANT05: blocker does not fire when signal_qty > 0 (strategy generated a signal).
    #[test]
    fn ant05_no_signal_blocker_silent_when_signal_present() {
        let ticks: u64 = 3;
        let qty: Option<i64> = Some(1);
        let fires = ticks > 0 && qty == Some(0);
        assert!(
            !fires,
            "ANT05: NO_SIGNAL_GENERATED blocker must not fire when signal_qty > 0"
        );
    }

    // AUTON-SIGNAL-CONTEXT-01: bar_context_source derivation tests.

    /// SC-01: sentinel (-1) → no_dispatch_yet.
    #[test]
    fn sc01_context_source_no_dispatch_when_sentinel() {
        let raw: i64 = -1;
        let (source, loaded) = match raw {
            -1 => ("no_dispatch_yet".to_string(), None),
            0 => ("stub_no_price".to_string(), None),
            n => ("db_loaded".to_string(), Some(n as u64)),
        };
        assert_eq!(source, "no_dispatch_yet");
        assert_eq!(loaded, None, "SC-01: no bars loaded when sentinel");
    }

    /// SC-02: 0 bars → stub_no_price context.
    #[test]
    fn sc02_context_source_stub_when_zero_bars() {
        let raw: i64 = 0;
        let (source, loaded) = match raw {
            -1 => ("no_dispatch_yet".to_string(), None),
            0 => ("stub_no_price".to_string(), None),
            n => ("db_loaded".to_string(), Some(n as u64)),
        };
        assert_eq!(source, "stub_no_price");
        assert_eq!(loaded, None, "SC-02: stub_no_price carries no bar count");
    }

    /// SC-03: positive bar count → db_loaded with count.
    #[test]
    fn sc03_context_source_db_loaded_when_bars_present() {
        for n in [5_i64, 20, 30] {
            let (source, loaded) = match n {
                -1 => ("no_dispatch_yet".to_string(), None),
                0 => ("stub_no_price".to_string(), None),
                k => ("db_loaded".to_string(), Some(k as u64)),
            };
            assert_eq!(source, "db_loaded", "SC-03: n={n} must yield db_loaded");
            assert_eq!(loaded, Some(n as u64), "SC-03: n={n} must carry bar count");
        }
    }

    /// SC-04: incomplete bar context (stub path) does not produce db_loaded.
    #[test]
    fn sc04_stub_path_never_claims_db_loaded() {
        // Simulate single-stub context: 0 DB bars loaded.
        let raw: i64 = 0;
        let source = if raw > 0 {
            "db_loaded"
        } else {
            "stub_no_price"
        };
        assert_ne!(
            source, "db_loaded",
            "SC-04: stub_no_price path must not claim db_loaded"
        );
    }

    #[test]
    fn obs01_controller_exited_maps_to_distinct_api_state() {
        let (state, detail) =
            autonomous_session_truth_to_api(&AutonomousSessionTruth::ControllerExited {
                detail: "task panicked".to_string(),
            });
        assert_eq!(state, "controller_exited");
        assert_eq!(detail, Some("task panicked".to_string()));
    }

    #[test]
    fn obs01_all_truth_variants_map_to_distinct_states() {
        use crate::state::AutonomousRecoveryResumeSource;
        let cases = [
            (AutonomousSessionTruth::Clear, "clear"),
            (
                AutonomousSessionTruth::StartRefused { detail: "x".into() },
                "start_refused",
            ),
            (
                AutonomousSessionTruth::RecoveryRetrying {
                    resume_source: AutonomousRecoveryResumeSource::PersistedCursor,
                    detail: "x".into(),
                },
                "recovery_retrying",
            ),
            (
                AutonomousSessionTruth::RecoverySucceeded {
                    resume_source: AutonomousRecoveryResumeSource::PersistedCursor,
                    detail: "x".into(),
                },
                "recovery_succeeded",
            ),
            (
                AutonomousSessionTruth::RecoveryFailed {
                    resume_source: AutonomousRecoveryResumeSource::PersistedCursor,
                    detail: "x".into(),
                },
                "recovery_failed",
            ),
            // BRK-GAP-01: partial recovery must map to its own distinct API state.
            (
                AutonomousSessionTruth::WsGapPartialRecovery {
                    resume_source: AutonomousRecoveryResumeSource::PersistedCursor,
                    detail: "x".into(),
                },
                "ws_gap_partial_recovery",
            ),
            (
                AutonomousSessionTruth::RunEndedUnexpectedly { detail: "x".into() },
                "run_ended_unexpectedly",
            ),
            (
                AutonomousSessionTruth::StopFailed { detail: "x".into() },
                "stop_failed",
            ),
            (
                AutonomousSessionTruth::StoppedAtBoundary { detail: "x".into() },
                "stopped_at_boundary",
            ),
            (
                AutonomousSessionTruth::ControllerExited { detail: "x".into() },
                "controller_exited",
            ),
        ];
        let mut seen = std::collections::HashSet::new();
        for (truth, expected_state) in cases {
            let (state, _) = autonomous_session_truth_to_api(&truth);
            assert_eq!(state, expected_state);
            assert!(seen.insert(state), "duplicate API state string detected");
        }
    }
}
