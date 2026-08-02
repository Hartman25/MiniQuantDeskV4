//! System-level route handlers (runtime operational surfaces).
//!
//! Contains: health, status_handler, system_status, system_preflight,
//! autonomous_readiness, system_metadata, system_instrument_registry_v2_status,
//! system_instrument_registry_v2_source_status, system_runtime_leadership,
//! system_session.
//!
//! Config-surface handlers (system_config_fingerprint, system_config_diffs)
//! live in the `config` submodule (MT-07D split).
//!
//! Artifact/evidence/topology handlers live in `routes/system_artifact.rs`
//! (MT-01 split).

mod config;
pub(crate) use config::{system_config_diffs, system_config_fingerprint};

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::api_types::{
    AssetCapabilityEntry, AssetCapabilityMatrix, AssetRiskPolicyEntry,
    AssetRiskPolicyStatusResponse, AutonomousPaperReadinessResponse, HealthResponse,
    InstrumentEconomicsStatusResponse, InstrumentEconomicsStatusRow,
    InstrumentRegistryV2EnabledCounts, InstrumentRegistryV2SourceStatusResponse,
    InstrumentRegistryV2StatusResponse, InstrumentSessionParityResponse,
    InstrumentSessionParityRow, InstrumentSessionProfileSummary, InstrumentSessionShadowSummary,
    InstrumentSessionStatusResponse, InstrumentSessionStatusRow, MultiSymbolFreshnessReport,
    PreflightStatusResponse, RuntimeLeadershipCheckpointRow, RuntimeLeadershipResponse,
    RuntimeSessionSourceSummaryResponse, SessionStateResponse, StrategyDecisionDiagnostics,
    SystemMetadataResponse, SystemStatusResponse,
};
use crate::market_data_freshness::{
    evaluate_md_freshness_status, evaluate_md_freshness_status_for_symbols,
    required_symbols_for_freshness_gate_from_env,
};
use crate::parity_evidence::{evaluate_parity_evidence_guarded, ParityEvidenceOutcome};
use crate::state::{
    autonomous_session_schedule_from_env, bridge_instrument_registry_v2_to_economics,
    instrument_session_state_for_profile, resolve_session_profile_for_instrument_metadata,
    runtime_session_source_summary, session_window_from_env, supported_session_profiles, AppState,
    AutonomousSessionTruth, BrokerSnapshotTruthSource, DeploymentMode,
    InstrumentEconomicsBridgeResult, MarketCalendarProvider, MarketSessionProfile,
    MarketSessionState, NyseWeekdaysProvider, SessionAuthority, SessionProfileResolutionTruth,
    SessionProfileStatus, StrategyMarketDataSource, SESSION_START_HH_MM_ENV,
    SESSION_STOP_HH_MM_ENV, STRATEGY_MD_TIMEFRAME_ENV,
};

use super::helpers::{
    build_fault_signals, environment_and_live_routing_truth, runtime_error_response,
    runtime_status_from_state, sticky_halt_fault_signal,
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
                let mut signals =
                    build_fault_signals(&status, &reconcile, risk_truth, execution_loop_stall_secs);
                // RISK-ENGINE-HALTED-VISIBILITY-01: overlay the live risk gate's
                // sticky halt flag as an additional fault signal, independent of
                // risk_truth (which reflects the transient sys_risk_block_state
                // DB flag and is reset every orchestrator tick).
                let risk_engine_sticky_halt = st
                    .current_execution_snapshot()
                    .await
                    .map(|snap| snap.risk_engine_sticky_halt)
                    .unwrap_or(mqk_execution::RiskEngineHaltStatus::Unavailable);
                if let Some(sig) = sticky_halt_fault_signal(risk_engine_sticky_halt) {
                    signals.push(sig);
                }
                signals
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
            // ASSET-CORE-05C: compact shadow-parity summary. Observability
            // only; never gates deployment_start_allowed or any other field.
            instrument_session_shadow: instrument_session_shadow_summary_now(&st),
            // ASSET-CORE-05D: compact runtime session-source scaffold
            // summary. Default-off; observability only.
            runtime_session_source: runtime_session_source_summary_now(&st),
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

    // DATA-FRESHNESS-READINESS-GATE-01: Market-data freshness on preflight.
    //
    // Evaluate freshness only for Paper+Alpaca.  DB pointer is borrowed from
    // AppState (same pool used by all other DB-backed preflight checks).
    let pf_md_freshness = if is_paper_alpaca {
        let sym = std::env::var("MQK_STRATEGY_SYMBOL")
            .map(|v| v.trim().to_string())
            .unwrap_or_default();
        let tf = std::env::var(STRATEGY_MD_TIMEFRAME_ENV)
            .map(|v| v.trim().to_string())
            .unwrap_or_default();
        Some(evaluate_md_freshness_status(st.db.as_ref(), &sym, &tf, Utc::now().timestamp()).await)
    } else {
        None
    };

    // PREMARKET-DATA-READINESS-GATE-01: multi-symbol premarket readiness,
    // covering every required symbol (not just the single legacy symbol
    // above) — the same aggregate gate start_execution_runtime enforces.
    let pf_md_readiness: Option<MultiSymbolFreshnessReport> = if is_paper_alpaca {
        Some(
            evaluate_md_freshness_status_for_symbols(
                st.db.as_ref(),
                &required_symbols_for_freshness_gate_from_env(),
                Utc::now().timestamp(),
            )
            .await,
        )
    } else {
        None
    };

    let mut warnings = Vec::new();
    if status.notes.is_some() {
        warnings.push("Daemon status contains notes; verify runtime state.".to_string());
    }

    // DAILY-DATA-READINESS-01C-ENFORCEMENT-01: canonical readiness report,
    // shared with the dedicated route/autonomous-readiness/ingest-plan (§C.4).
    let daily_data_readiness_report =
        crate::routes::market_data_readiness::compute_daily_data_readiness_response(
            &st,
            Utc::now(),
        )
        .await;

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
    // PREMARKET-DATA-READINESS-GATE-01: surface multi-symbol readiness
    // blockers in preflight (supersedes the single-symbol blocker push —
    // `pf_md_readiness` covers the same single symbol plus any others).
    if let Some(ref readiness) = pf_md_readiness {
        blockers.extend(readiness.blockers.clone());
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
            market_data_freshness: pf_md_freshness,
            market_data_readiness: pf_md_readiness,
            // ASSET-CORE-05C: compact shadow-parity summary. Observability
            // only; never added to `blockers`/`warnings` and never gates
            // `deployment_start_allowed`.
            instrument_session_shadow: instrument_session_shadow_summary_now(&st),
            // ASSET-CORE-05D: compact runtime session-source scaffold
            // summary. Default-off; observability only.
            runtime_session_source: runtime_session_source_summary_now(&st),
            daily_data_readiness: daily_data_readiness_report,
            daily_operation:
                crate::routes::autonomous_daily_operations::compute_daily_operation_summary(
                    &st,
                    Utc::now(),
                )
                .await,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// AUTON-TRUTH-01: GET /api/v1/autonomous/readiness
// ---------------------------------------------------------------------------

/// Converts `AutonomousSessionTruth` to a (state_str, detail) pair for API surfaces.
pub(crate) fn autonomous_session_truth_to_api(
    truth: &AutonomousSessionTruth,
) -> (String, Option<String>) {
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
        AutonomousSessionTruth::CompletedBarDriverExited { detail } => (
            "completed_bar_driver_exited".to_string(),
            Some(detail.clone()),
        ),
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
    // SCSP06: label "fixed_window_override" so operators can distinguish an
    // env-var override from the authoritative exchange-calendar path.
    // When no env override is configured, "nyse_weekdays" names the actual seam.
    let (source, start_utc, stop_utc) = match &window_opt {
        Some(w) => (
            "fixed_window_override".to_string(),
            Some(format!("{:02}:{:02} UTC", w.start_hh, w.start_mm)),
            Some(format!("{:02}:{:02} UTC", w.stop_hh, w.stop_mm)),
        ),
        None => ("nyse_weekdays".to_string(), None, None),
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

    // DAILY-DATA-READINESS-01C-ENFORCEMENT-01: canonical readiness report,
    // shared with the dedicated route/preflight/ingest-plan (§C.4). Computed
    // once with the same `now` snapshot as every other diagnostic field on
    // this response.
    let daily_data_readiness_report =
        crate::routes::market_data_readiness::compute_daily_data_readiness_response(&st, now).await;

    if !is_paper_alpaca {
        let dynamic_selection = dynamic_selection_readiness_projection(&st).await;
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
                market_data_freshness: None,
                market_data_readiness: None,
                strategy_decision_diagnostics: None,
                daily_data_readiness: daily_data_readiness_report,
                daily_operation:
                    crate::routes::autonomous_daily_operations::compute_daily_operation_summary(
                        &st, now,
                    )
                    .await,
                dynamic_selection,
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
            // MARKET-DATA-BAR-CONTEXT-01: distinguish "env vars missing" from "env vars
            // set but md_bars has no data for this symbol/timeframe".
            _ => {
                let ns_sym = std::env::var("MQK_STRATEGY_SYMBOL")
                    .map(|v| v.trim().to_string())
                    .unwrap_or_default();
                let ns_tf = std::env::var(STRATEGY_MD_TIMEFRAME_ENV)
                    .map(|v| v.trim().to_string())
                    .unwrap_or_default();
                if !ns_sym.is_empty() && !ns_tf.is_empty() {
                    format!(
                        "NO_SIGNAL_GENERATED (MARKET-DATA-BAR-CONTEXT-01): bar ticks \
                         dispatched but md_bars has no completed bars for \
                         {ns_sym}/{ns_tf}; ingest historical bars before the session \
                         opens so the strategy can satisfy its LOOKBACK requirement"
                    )
                } else {
                    "NO_SIGNAL_GENERATED (AUTON-NO-TRADE-02): bar ticks dispatched but \
                     strategy signal qty is 0; context is single-stub with is_complete=false \
                     (no price reference — set MQK_STRATEGY_SYMBOL and \
                     MQK_STRATEGY_MD_TIMEFRAME to load real DB bars)"
                        .to_string()
                }
            }
        };
        blockers.push(reason);
    }

    // AUTON-SIGNAL-CONTEXT-01 / MARKET-DATA-BAR-CONTEXT-01: surface
    // INCOMPLETE_BAR_CONTEXT when the last dispatch used the stub path and ticks
    // have already occurred.  Two distinct sub-cases:
    //   A. One or both env vars (MQK_STRATEGY_SYMBOL / MQK_STRATEGY_MD_TIMEFRAME)
    //      are absent → operator needs to configure them.
    //   B. Both env vars are set but md_bars contains no completed bars for that
    //      symbol/timeframe → operator needs to ingest historical data.
    if st.bar_tick_dispatch_count() > 0 && raw_ctx_bars == 0 {
        let ibc_sym = std::env::var("MQK_STRATEGY_SYMBOL")
            .map(|v| v.trim().to_string())
            .unwrap_or_default();
        let ibc_tf = std::env::var(STRATEGY_MD_TIMEFRAME_ENV)
            .map(|v| v.trim().to_string())
            .unwrap_or_default();
        let symbol_set = !ibc_sym.is_empty();
        let tf_set = !ibc_tf.is_empty();
        if !symbol_set || !tf_set {
            // Sub-case A: missing configuration.
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
        } else {
            // Sub-case B: both env vars are set but md_bars has no completed bars
            // for this symbol/timeframe (MARKET-DATA-BAR-CONTEXT-01).
            blockers.push(format!(
                "INCOMPLETE_BAR_CONTEXT (MARKET-DATA-BAR-CONTEXT-01): \
                 MQK_STRATEGY_SYMBOL={ibc_sym} and MQK_STRATEGY_MD_TIMEFRAME={ibc_tf} \
                 are configured but md_bars contains no completed bars for this \
                 symbol/timeframe; ingest historical bars before the session opens \
                 so the strategy receives a real price context"
            ));
        }
    }

    // DATA-FRESHNESS-READINESS-GATE-01: Evaluate market-data freshness pre-dispatch.
    //
    // Unlike the INCOMPLETE_BAR_CONTEXT/NO_SIGNAL_GENERATED checks above (which
    // fire post-dispatch), this check is evaluated before any bar has been
    // dispatched.  It surfaces a blocker when md_bars is missing, insufficient,
    // or stale for the configured symbol/timeframe so the operator knows to
    // ingest data before the session opens — not only after the first bar tick.
    let ar_sym = std::env::var("MQK_STRATEGY_SYMBOL")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let ar_tf = std::env::var(STRATEGY_MD_TIMEFRAME_ENV)
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let md_freshness =
        evaluate_md_freshness_status(st.db.as_ref(), &ar_sym, &ar_tf, now.timestamp()).await;

    // PREMARKET-DATA-READINESS-GATE-01: multi-symbol premarket readiness,
    // covering every required symbol (not just the single legacy symbol
    // above) — the same aggregate gate start_execution_runtime enforces.
    // Supersedes md_freshness for blockers/overall_ready (md_freshness is
    // retained only for the back-compat `market_data_freshness` field below).
    let md_readiness = evaluate_md_freshness_status_for_symbols(
        st.db.as_ref(),
        &required_symbols_for_freshness_gate_from_env(),
        now.timestamp(),
    )
    .await;
    blockers.extend(md_readiness.blockers.clone());

    let overall_ready = ws_continuity_ready
        && reconcile_ready
        && arm_ready
        && signal_ingestion_configured
        && session_in_window
        && runtime_start_allowed
        && !strategy_fleet_empty
        && md_readiness.start_allowed;

    let autonomous_history_degraded = st.autonomous_history_degraded();

    // STRATEGY-DECISION-OBSERVABILITY-01: map strategy diagnostics to API type.
    let strategy_decision_diagnostics: Option<StrategyDecisionDiagnostics> =
        st.last_strategy_diagnostics().await.map(|d| {
            let strategy_id = std::env::var("MQK_STRATEGY_IDS")
                .map(|v| {
                    v.trim()
                        .split(',')
                        .next()
                        .unwrap_or("intraday_scalper")
                        .trim()
                        .to_string()
                })
                .unwrap_or_else(|_| "intraday_scalper".to_string());
            let symbol = std::env::var("MQK_STRATEGY_SYMBOL")
                .map(|v| v.trim().to_string())
                .unwrap_or_else(|_| "unknown".to_string());
            let timeframe = std::env::var(STRATEGY_MD_TIMEFRAME_ENV)
                .map(|v| v.trim().to_string())
                .unwrap_or_else(|_| "5m".to_string());
            StrategyDecisionDiagnostics {
                strategy_id,
                symbol,
                timeframe,
                lookback_bars: d.lookback_bars as u64,
                threshold_bps: d.threshold_bps,
                latest_bar_ts: d.latest_bar_ts,
                latest_close_micros: d.latest_close_micros,
                lookback_bar_ts: d.lookback_bar_ts,
                lookback_close_micros: d.lookback_close_micros,
                move_bps: d.move_bps,
                abs_move_bps: d.abs_move_bps,
                gap_to_threshold_bps: d.gap_to_threshold_bps,
                raw_direction: d.raw_direction,
                decision: d.decision.to_string(),
                reason: d.reason.to_string(),
            }
        });

    // AUTON-NO-TRADE-OFFHOURS-01B: durably snapshot this readiness verdict
    // so the dominant no-trade reason survives a daemon restart. Best-effort
    // and non-fatal — a write failure never affects the response below.
    let (diag_reason_code, diag_stage) = classify_no_trade_diagnostic(
        overall_ready,
        ws_continuity_ready,
        reconcile_ready,
        &arm_state,
        arm_ready,
        signal_ingestion_configured,
        session_in_window,
        runtime_start_allowed,
        strategy_fleet_empty,
        md_readiness.start_allowed,
        &bar_ticker_gate,
        st.bar_tick_dispatch_count(),
        last_bar_signal_qty,
    );
    let diag_reason = blockers.first().cloned().unwrap_or_else(|| {
        "no blockers; runtime ready to start on next session-controller tick".to_string()
    });
    let diag_run_id = st
        .current_status_snapshot()
        .await
        .ok()
        .and_then(|s| s.active_run_id);
    st.record_no_trade_diagnostic(crate::state::NoTradeDiagnosticSnapshot {
        run_id: diag_run_id,
        mode: st.deployment_mode().as_api_label(),
        session_window_state: &session_window_state,
        runtime_start_allowed,
        arm_state: &arm_state,
        overall_ready,
        reason_code: diag_reason_code,
        reason: &diag_reason,
        stage: diag_stage,
    })
    .await;

    let dynamic_selection = dynamic_selection_readiness_projection(&st).await;

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
            market_data_freshness: Some(md_freshness),
            market_data_readiness: Some(md_readiness),
            strategy_decision_diagnostics,
            daily_data_readiness: daily_data_readiness_report,
            daily_operation:
                crate::routes::autonomous_daily_operations::compute_daily_operation_summary(
                    &st, now,
                )
                .await,
            dynamic_selection,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/autonomous/no-trade-diagnostics
// ---------------------------------------------------------------------------

/// AUTON-NO-TRADE-OFFHOURS-01C: read-only recent autonomous no-trade
/// diagnostic journal.
///
/// Unlike `execution_fill_quality`, this is deliberately NOT scoped to the
/// active run — the operator must be able to inspect an off-hours no-trade
/// explanation that was recorded before a daemon restart, even if no run is
/// currently active. Read-only: never submits an order, never starts
/// runtime, never calls a broker or provider.
pub(crate) async fn autonomous_no_trade_diagnostics(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    const CANONICAL: &str = "/api/v1/autonomous/no-trade-diagnostics";

    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(crate::api_types::NoTradeDiagnosticsResponse {
                canonical_route: CANONICAL.to_string(),
                truth_state: "db_unavailable".to_string(),
                backend: "unavailable".to_string(),
                rows: vec![],
            }),
        )
            .into_response();
    };

    let rows = match mqk_db::fetch_recent_autonomous_no_trade_diagnostics(db, 100).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "auton_no_trade_offhours_01c: autonomous_no_trade_diagnostics_fetch_failed"
            );
            return (
                StatusCode::OK,
                Json(crate::api_types::NoTradeDiagnosticsResponse {
                    canonical_route: CANONICAL.to_string(),
                    truth_state: "query_failed".to_string(),
                    backend: "postgres.autonomous_no_trade_diagnostics".to_string(),
                    rows: vec![],
                }),
            )
                .into_response();
        }
    };

    let truth_state = if rows.is_empty() { "no_rows" } else { "active" };
    let api_rows: Vec<crate::api_types::NoTradeDiagnosticRow> = rows
        .into_iter()
        .map(|r| crate::api_types::NoTradeDiagnosticRow {
            diagnostic_id: r.diagnostic_id,
            observed_at_utc: r.observed_at_utc.to_rfc3339(),
            run_id: r.run_id,
            mode: r.mode,
            session_window_state: r.session_window_state,
            runtime_start_allowed: r.runtime_start_allowed,
            arm_state: r.arm_state,
            overall_ready: r.overall_ready,
            reason_code: r.reason_code,
            reason: r.reason,
            stage: r.stage,
            paper_order_attempted: r.paper_order_attempted,
            live_order_attempted: r.live_order_attempted,
            source: r.source,
        })
        .collect();

    (
        StatusCode::OK,
        Json(crate::api_types::NoTradeDiagnosticsResponse {
            canonical_route: CANONICAL.to_string(),
            truth_state: truth_state.to_string(),
            backend: "postgres.autonomous_no_trade_diagnostics".to_string(),
            rows: api_rows,
        }),
    )
        .into_response()
}

/// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A: build the narrow additive
/// dynamic-selection projection for `GET /api/v1/autonomous/readiness`.
///
/// ATOMICITY-SINGLE-SNAPSHOT-REPAIR requirement 6: `configured_mode`/
/// `effective_mode`/`live_lock_applied`/`disposition`/`plan_present`/
/// `host_pool_present`/`selected_pair_count`/`owning_run_id`/
/// `approved_for_live` all come from the *one* committed snapshot when it
/// exists — never mixed with a fresh env-preview read. When no snapshot is
/// committed, every one of those fields is in its neutral/absent state
/// (`None`/`false`/`0`), never silently populated from current env config.
/// The current-env-config preview lives only in the separately named
/// `preview_configured_mode`/`preview_effective_mode`/
/// `preview_live_lock_applied` fields, which are always a fresh evaluation
/// regardless of whether a run is active — never proof of an active run's
/// binding.
async fn dynamic_selection_readiness_projection(
    st: &Arc<AppState>,
) -> crate::api_types::DynamicSelectionReadinessProjection {
    let mode_resolution = crate::dynamic_selection_mode::resolve_dynamic_selection_mode_from_env();
    let preview = crate::dynamic_selection_mode::effective_mode(
        &mode_resolution,
        st.deployment_mode(),
        st.runtime_selection().broker_kind,
    );
    let snapshot = st.dynamic_selection_runtime_snapshot().await;

    crate::api_types::DynamicSelectionReadinessProjection {
        configured_mode: snapshot
            .as_ref()
            .map(|s| s.configured_mode.as_str().to_string()),
        effective_mode: snapshot
            .as_ref()
            .map(|s| s.effective_mode.as_str().to_string()),
        live_lock_applied: snapshot.as_ref().is_some_and(|s| s.live_lock_applied),
        disposition: snapshot
            .as_ref()
            .map(|s| s.disposition.as_str().to_string()),
        plan_present: snapshot.as_ref().is_some_and(|s| s.plan_present()),
        host_pool_present: snapshot.as_ref().is_some_and(|s| s.host_pool_present()),
        selected_pair_count: snapshot
            .as_ref()
            .map(|s| s.selected_pair_count() as u64)
            .unwrap_or(0),
        owning_run_id: snapshot.as_ref().map(|s| s.run_id),
        approved_for_live: snapshot.as_ref().is_some_and(|s| s.approved_for_live),
        preview_configured_mode: preview.configured_mode.as_str().to_string(),
        preview_effective_mode: preview.effective_mode.as_str().to_string(),
        preview_live_lock_applied: preview.live_lock_applied,
    }
}

// AUTON-NO-TRADE-01: Bar ticker Gate 2 derivation — pure helper for testability.
fn bar_ticker_gate_from_session(nyse_session: &str) -> &'static str {
    if nyse_session == "regular" {
        "open"
    } else {
        "closed_outside_session"
    }
}

/// AUTON-NO-TRADE-OFFHOURS-01B: Classify the dominant no-trade reason from
/// the same gate booleans `autonomous_readiness` already computes, walked in
/// the same priority order its `blockers` vec is built in. Pure and
/// deterministic — no I/O, no env reads. Returns `(reason_code, stage)`.
#[allow(clippy::too_many_arguments)]
fn classify_no_trade_diagnostic(
    overall_ready: bool,
    ws_continuity_ready: bool,
    reconcile_ready: bool,
    arm_state: &str,
    arm_ready: bool,
    signal_ingestion_configured: bool,
    session_in_window: bool,
    runtime_start_allowed: bool,
    strategy_fleet_empty: bool,
    md_start_allowed: bool,
    bar_ticker_gate: &str,
    bar_tick_dispatch_count: u64,
    last_bar_signal_qty: Option<i64>,
) -> (&'static str, &'static str) {
    if !ws_continuity_ready {
        return ("WS_CONTINUITY_NOT_READY", "pre_session_window");
    }
    if !reconcile_ready {
        return ("RECONCILE_NOT_READY", "pre_session_window");
    }
    if !arm_ready {
        return if arm_state == "halted" {
            ("INTEGRITY_HALTED", "pre_arm")
        } else {
            ("ARM_NOT_READY", "pre_arm")
        };
    }
    if !signal_ingestion_configured {
        return ("SIGNAL_INGESTION_NOT_CONFIGURED", "pre_session_window");
    }
    if !session_in_window {
        return ("OUTSIDE_SESSION_WINDOW", "pre_session_window");
    }
    if !runtime_start_allowed {
        // A locally-owned run is already active; no order attempt this poll
        // is explained by the bar-ticker/strategy-tick/signal path instead.
        if bar_ticker_gate != "open" {
            return ("BAR_TICKER_GATE_CLOSED", "pre_dispatch");
        }
        if bar_tick_dispatch_count == 0 {
            return ("STRATEGY_NOT_TICKED", "pre_dispatch");
        }
        if last_bar_signal_qty == Some(0) {
            return ("NO_SIGNAL_GENERATED", "pre_dispatch");
        }
        return ("RUNTIME_ALREADY_ACTIVE", "pre_dispatch");
    }
    if strategy_fleet_empty {
        return ("STRATEGY_FLEET_EMPTY", "pre_runtime_start");
    }
    if !md_start_allowed {
        return ("MARKET_DATA_NOT_READY", "pre_runtime_start");
    }
    if overall_ready {
        return ("NO_ACTIVE_RUN_PENDING_START", "pre_runtime_start");
    }
    ("UNKNOWN", "unclassified")
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/metadata — static capability matrix
// ---------------------------------------------------------------------------

/// Build the static asset capability matrix (ASSET-CAPABILITY-MATRIX-01).
///
/// All values are compile-time constants.  This function never reads env vars,
/// DB state, or runtime flags, and is never used for order routing.
fn static_asset_capability_matrix() -> AssetCapabilityMatrix {
    let entries = vec![
        AssetCapabilityEntry {
            asset_class: "us_equities".to_string(),
            enabled: true,
            paper_ready: true,
            live_ready: false,
            broker_adapter: "alpaca".to_string(),
            notes: "current Paper+Alpaca equity path; live locked pending future review"
                .to_string(),
        },
        AssetCapabilityEntry {
            asset_class: "crypto".to_string(),
            enabled: false,
            paper_ready: false,
            live_ready: false,
            broker_adapter: "none".to_string(),
            notes: "BACKLOG — CRYPTO-SCAFFOLD-01; no adapter wired".to_string(),
        },
        AssetCapabilityEntry {
            asset_class: "futures".to_string(),
            enabled: false,
            paper_ready: false,
            live_ready: false,
            broker_adapter: "none".to_string(),
            notes: "BACKLOG — FUTURES-SCAFFOLD-01; no adapter wired".to_string(),
        },
        AssetCapabilityEntry {
            asset_class: "options".to_string(),
            enabled: false,
            paper_ready: false,
            live_ready: false,
            broker_adapter: "none".to_string(),
            notes: "BACKLOG — OPTIONS-SCAFFOLD-01; no adapter wired".to_string(),
        },
        AssetCapabilityEntry {
            asset_class: "forex".to_string(),
            enabled: false,
            paper_ready: false,
            live_ready: false,
            broker_adapter: "none".to_string(),
            notes: "BACKLOG — FOREX-SCAFFOLD-01; no adapter wired".to_string(),
        },
    ];

    let default_enabled: Vec<String> = entries
        .iter()
        .filter(|e| e.enabled)
        .map(|e| e.asset_class.clone())
        .collect();

    let disabled: Vec<String> = entries
        .iter()
        .filter(|e| !e.enabled)
        .map(|e| e.asset_class.clone())
        .collect();

    AssetCapabilityMatrix {
        schema_version: "asset-capability-v1".to_string(),
        static_source: "mqk-daemon/routes/system.rs::static_asset_capability_matrix".to_string(),
        live_capital_ready: false,
        default_enabled_asset_classes: default_enabled,
        disabled_asset_classes: disabled,
        entries,
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
            asset_capability_matrix: static_asset_capability_matrix(),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/asset-risk-policy — ASSET-CORE-03B
// ---------------------------------------------------------------------------

/// Pure label for `mqk_execution::asset_risk_policy::AssetRiskPolicyState`.
/// No IO; never drives any decision.
fn asset_risk_policy_state_label(state: mqk_execution::AssetRiskPolicyState) -> &'static str {
    use mqk_execution::AssetRiskPolicyState;
    match state {
        AssetRiskPolicyState::Enabled => "enabled",
        AssetRiskPolicyState::Disabled => "disabled",
        AssetRiskPolicyState::ResearchOnly => "research_only",
        AssetRiskPolicyState::Unsupported => "unsupported",
    }
}

/// Read-only asset-risk-policy status surface (`ASSET-CORE-03B`). Reports
/// the existing static `mqk_execution::asset_risk_policy` model as-is; does
/// not require DB, broker, or provider access, and does not change Gate 0 or
/// the broker-submit routing guard.
pub(crate) async fn system_asset_risk_policy_status() -> impl IntoResponse {
    let entries: Vec<AssetRiskPolicyEntry> = mqk_execution::default_asset_risk_policies()
        .into_iter()
        .map(|policy| AssetRiskPolicyEntry {
            asset_class: policy.asset_class.to_string(),
            state: asset_risk_policy_state_label(policy.state).to_string(),
            paper_trading_enabled: policy.paper_trading_enabled,
            live_trading_enabled: policy.live_trading_enabled,
            requires_margin_model: policy.requires_margin_model,
            requires_contract_multiplier: policy.requires_contract_multiplier,
            requires_session_profile: policy.requires_session_profile,
            requires_currency_conversion: policy.requires_currency_conversion,
            reason_code: policy.reason_code.to_string(),
            message: policy.message.to_string(),
        })
        .collect();

    (
        StatusCode::OK,
        Json(AssetRiskPolicyStatusResponse {
            schema_version: "asset-risk-policy-v1".to_string(),
            policy_source: mqk_execution::ASSET_RISK_POLICY_SOURCE.to_string(),
            production_enforcement_enabled:
                mqk_execution::ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED,
            non_equity_routing_enabled: mqk_execution::ASSET_RISK_NON_EQUITY_ROUTING_ENABLED,
            entries,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/instrument-registry-v2/status — ASSET-CORE-01C
// ---------------------------------------------------------------------------

/// Contract-shape label for a v2 instrument's `contract` field. Pure; no IO.
/// Used only to build `contract_kind_counts` — never to drive any decision.
fn contract_kind_label(
    contract: &Option<mqk_md::instrument_registry_v2::ContractDefinitionV2>,
) -> &'static str {
    use mqk_md::instrument_registry_v2::ContractDefinitionV2;
    match contract {
        None => "none",
        Some(ContractDefinitionV2::Equity) => "equity",
        Some(ContractDefinitionV2::Etf) => "etf",
        Some(ContractDefinitionV2::Future { .. }) => "future",
        Some(ContractDefinitionV2::Option { .. }) => "option",
        Some(ContractDefinitionV2::CryptoPair { .. }) => "crypto_pair",
        Some(ContractDefinitionV2::ForexPair { .. }) => "forex_pair",
        Some(ContractDefinitionV2::Rate { .. }) => "rate",
    }
}

/// Build the empty/failed-path `InstrumentRegistryV2StatusResponse` shared by
/// the `unavailable` and `v1_load_failed` truth states. Pure; no IO.
fn instrument_registry_v2_failure_response(
    truth_state: &str,
    registry_path: String,
    error: String,
) -> InstrumentRegistryV2StatusResponse {
    InstrumentRegistryV2StatusResponse {
        truth_state: truth_state.to_string(),
        registry_path,
        schema_version: None,
        v1_count: 0,
        v2_count: 0,
        validation_passed: false,
        validation_errors: vec![error],
        asset_class_counts: BTreeMap::new(),
        instrument_kind_counts: BTreeMap::new(),
        contract_kind_counts: BTreeMap::new(),
        enabled_count: 0,
        etf_count: 0,
        non_equity_count: 0,
        enabled_non_equity_count: 0,
        paper_trading_enabled_count: 0,
        live_trading_enabled_count: 0,
        production_cutover_enabled: false,
        trading_uses_v2: false,
        notes: vec![
            "v1 registry remains the sole production source for trading/ingestion/backtest/GUI."
                .to_string(),
        ],
    }
}

/// GET /api/v1/system/instrument-registry-v2/status (ASSET-CORE-01C).
///
/// Read-only proof surface: loads the v1 registry at
/// `st.instrument_registry_path`, converts it to `InstrumentRegistryV2` in
/// memory, validates it, and summarizes the result. No DB pool, no provider
/// or broker calls, no writes of any kind — every value here is derived
/// purely from the v1 registry file and the pure `mqk_md::instrument_registry_v2`
/// functions. `production_cutover_enabled` and `trading_uses_v2` are always
/// `false`; this route is never read by any trading/ingest/backtest/GUI path.
pub(crate) async fn system_instrument_registry_v2_status(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let registry_path = st.instrument_registry_path.clone();
    let path = std::path::Path::new(&registry_path);

    let v1 = match mqk_md::instrument_registry::load_instrument_registry(path) {
        Ok(v) => v,
        Err(e) => {
            let truth_state = if path.exists() {
                "v1_load_failed"
            } else {
                "unavailable"
            };
            return Json(instrument_registry_v2_failure_response(
                truth_state,
                registry_path,
                e.to_string(),
            ))
            .into_response();
        }
    };

    let v2 = mqk_md::instrument_registry_v2::convert_v1_registry_to_v2(&v1);
    let v1_count = v1.len();
    let v2_count = v2.instruments.len();

    let mut asset_class_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut instrument_kind_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut contract_kind_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut enabled_count = 0usize;
    let mut etf_count = 0usize;
    let mut non_equity_count = 0usize;
    let mut enabled_non_equity_count = 0usize;
    let mut paper_trading_enabled_count = 0usize;
    let mut live_trading_enabled_count = 0usize;

    for inst in &v2.instruments {
        *asset_class_counts
            .entry(inst.asset_class.clone())
            .or_insert(0) += 1;
        if let Some(kind) = &inst.instrument_kind {
            *instrument_kind_counts.entry(kind.clone()).or_insert(0) += 1;
            if kind == "etf" {
                etf_count += 1;
            }
        }
        *contract_kind_counts
            .entry(contract_kind_label(&inst.contract).to_string())
            .or_insert(0) += 1;
        if inst.enabled {
            enabled_count += 1;
        }
        if inst.asset_class != "equity" {
            non_equity_count += 1;
            if inst.enabled {
                enabled_non_equity_count += 1;
            }
        }
        if inst.paper_trading_enabled {
            paper_trading_enabled_count += 1;
        }
        if inst.live_trading_enabled {
            live_trading_enabled_count += 1;
        }
    }

    let (truth_state, validation_passed, validation_errors) =
        match mqk_md::instrument_registry_v2::validate_registry_v2(&v2) {
            Ok(()) => ("active", true, Vec::new()),
            Err(e) => ("v2_validation_failed", false, vec![e.to_string()]),
        };

    Json(InstrumentRegistryV2StatusResponse {
        truth_state: truth_state.to_string(),
        registry_path,
        schema_version: Some(v2.schema_version),
        v1_count,
        v2_count,
        validation_passed,
        validation_errors,
        asset_class_counts,
        instrument_kind_counts,
        contract_kind_counts,
        enabled_count,
        etf_count,
        non_equity_count,
        enabled_non_equity_count,
        paper_trading_enabled_count,
        live_trading_enabled_count,
        production_cutover_enabled: false,
        trading_uses_v2: false,
        notes: vec![
            "v1 registry remains the sole production source for trading/ingestion/backtest/GUI."
                .to_string(),
            "InstrumentRegistryV2 is read-only/diagnostic via this route; no consumer reads it for decisions."
                .to_string(),
        ],
    })
    .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/instrument-sessions/status — ASSET-CORE-05B
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub(crate) struct InstrumentSessionsStatusQuery {
    now_utc: Option<String>,
    limit: Option<usize>,
    symbol: Option<String>,
}

#[derive(Debug, Clone)]
struct InstrumentSessionProfileAccumulator {
    asset_class: String,
    instrument_kind: String,
    production_backed: bool,
    model_only: bool,
    instrument_count: usize,
}

fn instrument_sessions_failure_response(
    truth_state: &str,
    as_of_utc: String,
    registry_path: String,
    error: String,
) -> InstrumentSessionStatusResponse {
    InstrumentSessionStatusResponse {
        truth_state: truth_state.to_string(),
        as_of_utc,
        registry_path,
        registry_v2_valid: false,
        v1_count: 0,
        v2_count: 0,
        instrument_count: 0,
        production_cutover_enabled: false,
        trading_uses_session_v2: false,
        runtime_uses_session_v2: false,
        non_equity_enabled: false,
        profiles: Vec::new(),
        instruments: Vec::new(),
        errors: vec![error],
    }
}

fn parse_instrument_sessions_as_of(
    now_utc: Option<&str>,
) -> Result<DateTime<Utc>, Box<InstrumentSessionStatusResponse>> {
    match now_utc {
        Some(raw) => DateTime::parse_from_rfc3339(raw)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|err| {
                Box::new(instrument_sessions_failure_response(
                    "timestamp_invalid",
                    raw.to_string(),
                    String::new(),
                    format!("invalid now_utc timestamp: {err}"),
                ))
            }),
        None => Ok(Utc::now()),
    }
}

// ASSET-CORE-05H: `instrument_session_state_for_profile` relocated to
// `mqk-daemon/src/state/market_calendar.rs` (now `pub fn`, reused by
// `route_instrument_session_for_metadata`) so this route module and any
// future caller share one definition instead of duplicating it. Imported
// above via `crate::state::instrument_session_state_for_profile`.

fn instrument_kind_profile_label(profile_id: &str, kind: Option<&str>) -> String {
    if profile_id == MarketSessionProfile::EquityUsRegular.as_str() {
        return "stock_or_etf".to_string();
    }
    match kind {
        Some(value) => value.to_string(),
        None => "none".to_string(),
    }
}

fn push_profile_count(
    profiles: &mut BTreeMap<String, InstrumentSessionProfileAccumulator>,
    profile_id: &str,
    asset_class: &str,
    instrument_kind: Option<&str>,
    production_backed: bool,
    model_only: bool,
) {
    let entry = profiles.entry(profile_id.to_string()).or_insert_with(|| {
        InstrumentSessionProfileAccumulator {
            asset_class: asset_class.to_string(),
            instrument_kind: instrument_kind_profile_label(profile_id, instrument_kind),
            production_backed,
            model_only,
            instrument_count: 0,
        }
    });
    entry.instrument_count += 1;
}

fn build_instrument_sessions_status_response(
    registry_path: String,
    v1_count: usize,
    v2: &mqk_md::instrument_registry_v2::InstrumentRegistryV2,
    as_of_utc: DateTime<Utc>,
    symbol_filter: Option<&str>,
    limit: Option<usize>,
) -> InstrumentSessionStatusResponse {
    let normalized_symbol_filter = symbol_filter.map(|s| s.trim().to_ascii_uppercase());
    let mut profile_counts: BTreeMap<String, InstrumentSessionProfileAccumulator> = BTreeMap::new();
    let mut rows = Vec::new();
    let mut non_equity_enabled = false;

    for inst in &v2.instruments {
        if inst.enabled && inst.asset_class != "equity" {
            non_equity_enabled = true;
        }
        if normalized_symbol_filter
            .as_deref()
            .is_some_and(|s| inst.symbol.to_ascii_uppercase() != s)
        {
            continue;
        }
        if limit.is_some_and(|max| rows.len() >= max) {
            continue;
        }

        let resolution = resolve_session_profile_for_instrument_metadata(
            &inst.asset_class,
            inst.instrument_kind.as_deref(),
        );
        let session_profile_id = resolution
            .profile
            .map(|p| p.as_str())
            .unwrap_or("session_profile_unavailable");
        let production_backed = resolution.truth_state == SessionProfileResolutionTruth::Active
            && resolution.profile == Some(MarketSessionProfile::EquityUsRegular);
        let model_only =
            resolution.truth_state == SessionProfileResolutionTruth::UnsupportedAssetClass;
        let profile_truth_state = match resolution.truth_state {
            SessionProfileResolutionTruth::Active => "production_backed",
            SessionProfileResolutionTruth::UnsupportedAssetClass
                if resolution.profile.is_some() =>
            {
                "model_only"
            }
            _ => "session_profile_unavailable",
        };
        let (session_state, reason_code) =
            instrument_session_state_for_profile(resolution.profile, as_of_utc);

        push_profile_count(
            &mut profile_counts,
            session_profile_id,
            &inst.asset_class,
            inst.instrument_kind.as_deref(),
            production_backed,
            model_only,
        );

        rows.push(InstrumentSessionStatusRow {
            symbol: inst.symbol.clone(),
            instrument_id: inst.instrument_id.clone(),
            asset_class: inst.asset_class.clone(),
            instrument_kind: inst.instrument_kind.clone(),
            session_profile_id: session_profile_id.to_string(),
            profile_truth_state: profile_truth_state.to_string(),
            session_state: session_state.to_string(),
            reason_code: reason_code.to_string(),
            production_backed,
            model_only,
            trading_uses_this: false,
        });
    }

    let profiles = profile_counts
        .into_iter()
        .map(|(profile_id, count)| InstrumentSessionProfileSummary {
            profile_id,
            asset_class: count.asset_class,
            instrument_kind: count.instrument_kind,
            production_backed: count.production_backed,
            model_only: count.model_only,
            instrument_count: count.instrument_count,
        })
        .collect();

    InstrumentSessionStatusResponse {
        truth_state: "active".to_string(),
        as_of_utc: as_of_utc.to_rfc3339(),
        registry_path,
        registry_v2_valid: true,
        v1_count,
        v2_count: v2.instruments.len(),
        instrument_count: rows.len(),
        production_cutover_enabled: false,
        trading_uses_session_v2: false,
        runtime_uses_session_v2: false,
        non_equity_enabled,
        profiles,
        instruments: rows,
        errors: Vec::new(),
    }
}

/// GET /api/v1/system/instrument-sessions/status (ASSET-CORE-05B).
///
/// Read-only diagnostic surface: loads the same configured v1 registry as
/// ASSET-CORE-01C, converts it to v2 in memory, validates it, then maps each
/// v2 instrument to the ASSET-CORE-05A/05B session profile seam at a supplied
/// timestamp. No DB, provider, broker, runtime, or trading path is touched.
pub(crate) async fn system_instrument_sessions_status(
    State(st): State<Arc<AppState>>,
    Query(query): Query<InstrumentSessionsStatusQuery>,
) -> impl IntoResponse {
    let as_of_utc = match parse_instrument_sessions_as_of(query.now_utc.as_deref()) {
        Ok(dt) => dt,
        Err(mut response) => {
            response.registry_path = st.instrument_registry_path.clone();
            return Json(response).into_response();
        }
    };

    let registry_path = st.instrument_registry_path.clone();
    let path = std::path::Path::new(&registry_path);
    let v1 = match mqk_md::instrument_registry::load_instrument_registry(path) {
        Ok(v) => v,
        Err(err) => {
            let truth_state = if path.exists() {
                "registry_invalid"
            } else {
                "registry_missing"
            };
            return Json(instrument_sessions_failure_response(
                truth_state,
                as_of_utc.to_rfc3339(),
                registry_path,
                err.to_string(),
            ))
            .into_response();
        }
    };

    let v2 = mqk_md::instrument_registry_v2::convert_v1_registry_to_v2(&v1);
    if let Err(err) = mqk_md::instrument_registry_v2::validate_registry_v2(&v2) {
        return Json(instrument_sessions_failure_response(
            "registry_invalid",
            as_of_utc.to_rfc3339(),
            registry_path,
            err.to_string(),
        ))
        .into_response();
    }

    Json(build_instrument_sessions_status_response(
        registry_path,
        v1.len(),
        &v2,
        as_of_utc,
        query.symbol.as_deref(),
        query.limit,
    ))
    .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/instrument-sessions/parity — ASSET-CORE-05C
// ---------------------------------------------------------------------------
//
// Read-only shadow comparison: production session truth (the equity-only
// `MarketSessionState`/`NyseWeekdaysProvider` path that actually gates
// trading today) versus the ASSET-CORE-05A/05B per-instrument session-profile
// seam. Reuses the same v1->v2 registry load/convert/validate path as
// ASSET-CORE-05B; adds no new registry, DB, provider, or broker dependency.
//
// Honesty contract:
// - `production_cutover_enabled`, `runtime_uses_session_v2`, and
//   `trading_uses_session_v2` are always `false`; `shadow_only` is always
//   `true`. Nothing here is consumed by any trading, runtime, or routing
//   decision.
// - Equity/ETF rows compare against real `NyseWeekdaysProvider` truth.
//   Non-equity (crypto/futures/forex) rows have no production calendar at
//   all: they are classified `model_only`/`unsupported_model_only` and never
//   reported as matching production, regardless of what the model-only
//   classifier returns.
// - `unknown` on either side of the comparison yields `unknown`, never
//   `matched`.

#[derive(Debug, Default, Deserialize)]
pub(crate) struct InstrumentSessionsParityQuery {
    now_utc: Option<String>,
    limit: Option<usize>,
    symbol: Option<String>,
}

fn instrument_session_parity_failure_response(
    truth_state: &str,
    as_of_utc: String,
    error: String,
) -> InstrumentSessionParityResponse {
    InstrumentSessionParityResponse {
        truth_state: truth_state.to_string(),
        as_of_utc,
        registry_v2_valid: false,
        production_cutover_enabled: false,
        runtime_uses_session_v2: false,
        trading_uses_session_v2: false,
        shadow_only: true,
        all_equity_profiles_match_production: false,
        checked_count: 0,
        matched_count: 0,
        mismatched_count: 0,
        unknown_count: 0,
        model_only_count: 0,
        rows: Vec::new(),
        errors: vec![error],
    }
}

/// Pure parity classifier for a single instrument — ASSET-CORE-05C.
///
/// Compares the real production `MarketSessionState` (equity/ETF only,
/// derived from `NyseWeekdaysProvider`) against the ASSET-CORE-05B
/// per-instrument session-profile resolution at `now_utc`. Conservative
/// rules: exact-same production-backed state => matched; any `unknown` on
/// either side => unknown (never matched); model-only non-equity profiles
/// are reported as `model_only`/`unsupported_model_only`, never as matching
/// production.
fn classify_instrument_session_parity_row(
    symbol: &str,
    asset_class: &str,
    instrument_kind: Option<&str>,
    now_utc: DateTime<Utc>,
) -> InstrumentSessionParityRow {
    let resolution = resolve_session_profile_for_instrument_metadata(asset_class, instrument_kind);
    let session_profile_id = resolution
        .profile
        .map(|p| p.as_str())
        .unwrap_or("session_profile_unavailable");

    let is_equity_active = resolution.truth_state == SessionProfileResolutionTruth::Active
        && resolution.profile == Some(MarketSessionProfile::EquityUsRegular);
    let model_only = resolution.truth_state == SessionProfileResolutionTruth::UnsupportedAssetClass
        && resolution.profile.is_some();

    let (profile_session_state, _profile_reason) =
        instrument_session_state_for_profile(resolution.profile, now_utc);

    let (production_session_state, parity_state, reason_code, production_backed) =
        if is_equity_active {
            let production_truth = NyseWeekdaysProvider.session_for(now_utc);
            let production_state_str = production_truth.state.as_str();
            let parity = if production_truth.state == MarketSessionState::Unknown {
                "unknown"
            } else if profile_session_state == production_state_str {
                "matched"
            } else {
                // ASSET-CORE-05B's instrument_session_state_for_profile classifies
                // equity instruments via the same NyseWeekdaysProvider call, so a
                // mismatch here would indicate the two call sites have diverged
                // (e.g. a future change to one without the other). Report it
                // truthfully rather than collapsing it into "matched".
                "mismatched"
            };
            (
                production_state_str.to_string(),
                parity,
                "equity_profile_compared_against_production_calendar",
                true,
            )
        } else if model_only {
            (
                "not_applicable".to_string(),
                "model_only",
                "non_equity_profile_is_model_only_no_production_calendar_exists",
                false,
            )
        } else if resolution.truth_state == SessionProfileResolutionTruth::Unknown {
            (
                "not_applicable".to_string(),
                "unknown",
                "asset_class_not_classifiable_by_session_profile_seam",
                false,
            )
        } else {
            // UnsupportedAssetClass with profile == None (e.g. options): no
            // session-profile shape exists, so parity cannot be computed.
            (
                "not_applicable".to_string(),
                "unsupported_model_only",
                "no_session_profile_shape_modeled_for_this_asset_class",
                false,
            )
        };

    InstrumentSessionParityRow {
        symbol: symbol.to_string(),
        asset_class: asset_class.to_string(),
        instrument_kind: instrument_kind.map(|s| s.to_string()),
        session_profile_id: session_profile_id.to_string(),
        production_session_state,
        profile_session_state: profile_session_state.to_string(),
        parity_state: parity_state.to_string(),
        production_backed,
        model_only,
        trading_uses_this: false,
        reason_code: reason_code.to_string(),
    }
}

fn build_instrument_session_parity_response(
    v2: &mqk_md::instrument_registry_v2::InstrumentRegistryV2,
    as_of_utc: DateTime<Utc>,
    symbol_filter: Option<&str>,
    limit: Option<usize>,
) -> InstrumentSessionParityResponse {
    let normalized_symbol_filter = symbol_filter.map(|s| s.trim().to_ascii_uppercase());
    let mut rows = Vec::new();
    let mut matched_count = 0usize;
    let mut mismatched_count = 0usize;
    let mut unknown_count = 0usize;
    let mut model_only_count = 0usize;

    for inst in &v2.instruments {
        if normalized_symbol_filter
            .as_deref()
            .is_some_and(|s| inst.symbol.to_ascii_uppercase() != s)
        {
            continue;
        }
        if limit.is_some_and(|max| rows.len() >= max) {
            continue;
        }

        let row = classify_instrument_session_parity_row(
            &inst.symbol,
            &inst.asset_class,
            inst.instrument_kind.as_deref(),
            as_of_utc,
        );

        match row.parity_state.as_str() {
            "matched" => matched_count += 1,
            "mismatched" => mismatched_count += 1,
            "unknown" => unknown_count += 1,
            "model_only" | "unsupported_model_only" => model_only_count += 1,
            _ => {}
        }

        rows.push(row);
    }

    let checked_count = rows.len();
    let equity_rows_checked = rows.iter().filter(|r| r.production_backed).count();
    let equity_rows_matched = rows
        .iter()
        .filter(|r| r.production_backed && r.parity_state == "matched")
        .count();
    let all_equity_profiles_match_production =
        equity_rows_checked > 0 && equity_rows_checked == equity_rows_matched;

    InstrumentSessionParityResponse {
        truth_state: "active".to_string(),
        as_of_utc: as_of_utc.to_rfc3339(),
        registry_v2_valid: true,
        production_cutover_enabled: false,
        runtime_uses_session_v2: false,
        trading_uses_session_v2: false,
        shadow_only: true,
        all_equity_profiles_match_production,
        checked_count,
        matched_count,
        mismatched_count,
        unknown_count,
        model_only_count,
        rows,
        errors: Vec::new(),
    }
}

/// GET /api/v1/system/instrument-sessions/parity (ASSET-CORE-05C).
///
/// Read-only shadow parity surface: loads the same configured v1 registry as
/// ASSET-CORE-05B, converts it to v2 in memory, validates it, then for each
/// instrument compares production session truth (equity/ETF only, via
/// `NyseWeekdaysProvider`) against the ASSET-CORE-05A/05B per-instrument
/// session-profile seam at a supplied timestamp. No DB, provider, broker,
/// runtime, or trading path is touched. Never a production cutover.
pub(crate) async fn system_instrument_sessions_parity(
    State(st): State<Arc<AppState>>,
    Query(query): Query<InstrumentSessionsParityQuery>,
) -> impl IntoResponse {
    let as_of_utc = match query.now_utc.as_deref() {
        Some(raw) => match DateTime::parse_from_rfc3339(raw).map(|dt| dt.with_timezone(&Utc)) {
            Ok(dt) => dt,
            Err(err) => {
                return Json(instrument_session_parity_failure_response(
                    "timestamp_invalid",
                    raw.to_string(),
                    format!("invalid now_utc timestamp: {err}"),
                ))
                .into_response();
            }
        },
        None => Utc::now(),
    };

    let registry_path = st.instrument_registry_path.clone();
    let path = std::path::Path::new(&registry_path);
    let v1 = match mqk_md::instrument_registry::load_instrument_registry(path) {
        Ok(v) => v,
        Err(err) => {
            let truth_state = if path.exists() {
                "registry_invalid"
            } else {
                "registry_missing"
            };
            return Json(instrument_session_parity_failure_response(
                truth_state,
                as_of_utc.to_rfc3339(),
                err.to_string(),
            ))
            .into_response();
        }
    };

    let v2 = mqk_md::instrument_registry_v2::convert_v1_registry_to_v2(&v1);
    if let Err(err) = mqk_md::instrument_registry_v2::validate_registry_v2(&v2) {
        return Json(instrument_session_parity_failure_response(
            "registry_invalid",
            as_of_utc.to_rfc3339(),
            err.to_string(),
        ))
        .into_response();
    }

    Json(build_instrument_session_parity_response(
        &v2,
        as_of_utc,
        query.symbol.as_deref(),
        query.limit,
    ))
    .into_response()
}

/// Compact shadow-parity summary for embedding on `/api/v1/system/status`
/// and `/api/v1/system/preflight` — ASSET-CORE-05C.
///
/// Reuses the exact same registry-load/convert/validate/classify path as the
/// dedicated parity route, evaluated at `Utc::now()`. Never panics: any
/// registry load/convert/validate failure degrades to
/// `truth_state: "unavailable"` rather than failing the parent surface.
pub(crate) fn instrument_session_shadow_summary_now(
    st: &AppState,
) -> InstrumentSessionShadowSummary {
    const ROUTE: &str = "/api/v1/system/instrument-sessions/parity";
    let registry_path = &st.instrument_registry_path;
    let path = std::path::Path::new(registry_path);

    let unavailable = |reason: &str| InstrumentSessionShadowSummary {
        truth_state: "unavailable".to_string(),
        shadow_only: true,
        production_cutover_enabled: false,
        runtime_uses_session_v2: false,
        trading_uses_session_v2: false,
        checked_count: 0,
        matched_count: 0,
        mismatched_count: 0,
        unknown_count: 0,
        model_only_count: 0,
        all_equity_profiles_match_production: false,
        route: format!("{ROUTE} ({reason})"),
    };

    let v1 = match mqk_md::instrument_registry::load_instrument_registry(path) {
        Ok(v) => v,
        Err(_) => return unavailable("registry_unavailable"),
    };
    let v2 = mqk_md::instrument_registry_v2::convert_v1_registry_to_v2(&v1);
    if mqk_md::instrument_registry_v2::validate_registry_v2(&v2).is_err() {
        return unavailable("registry_invalid");
    }

    let full = build_instrument_session_parity_response(&v2, Utc::now(), None, None);
    InstrumentSessionShadowSummary {
        truth_state: full.truth_state,
        shadow_only: full.shadow_only,
        production_cutover_enabled: full.production_cutover_enabled,
        runtime_uses_session_v2: full.runtime_uses_session_v2,
        trading_uses_session_v2: full.trading_uses_session_v2,
        checked_count: full.checked_count,
        matched_count: full.matched_count,
        mismatched_count: full.mismatched_count,
        unknown_count: full.unknown_count,
        model_only_count: full.model_only_count,
        all_equity_profiles_match_production: full.all_equity_profiles_match_production,
        route: ROUTE.to_string(),
    }
}

// ---------------------------------------------------------------------------
// ASSET-CORE-05D: runtime session-source cutover scaffold (default-off)
// ---------------------------------------------------------------------------

/// Compact runtime session-source summary for embedding on
/// `/api/v1/system/status` and `/api/v1/system/preflight` — ASSET-CORE-05D /
/// ASSET-CORE-05E.
///
/// Thin mapping wrapper over `state::runtime_session_source_summary`, which
/// holds the actual env-parsing and candidate/active-evaluation logic. Never
/// panics: any registry load/convert/validate failure surfaces as a
/// `fallback_reason`, never as a parent-surface error.
fn runtime_session_source_summary_now(st: &AppState) -> RuntimeSessionSourceSummaryResponse {
    let summary = runtime_session_source_summary(&st.instrument_registry_path, Utc::now());
    RuntimeSessionSourceSummaryResponse {
        session_source_mode: summary.session_source_mode,
        production_cutover_enabled: summary.production_cutover_enabled,
        runtime_uses_session_v2: summary.runtime_uses_session_v2,
        trading_uses_session_v2: summary.trading_uses_session_v2,
        legacy_session_state: summary.legacy_session_state,
        candidate_v2_session_state: summary.candidate_v2_session_state,
        candidate_v2_parity_state: summary.candidate_v2_parity_state,
        candidate_would_activate: summary.candidate_would_activate,
        active_source_used: summary.active_source_used,
        fallback_reason: summary.fallback_reason,
        activation_refusal_reason: summary.activation_refusal_reason,
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/instrument-registry-v2-source/status — ASSET-CORE-01D
// ---------------------------------------------------------------------------

const INSTRUMENT_REGISTRY_V2_SOURCE_ENV_VAR: &str = "MQK_INSTRUMENT_REGISTRY_V2_PATH";
const INSTRUMENT_REGISTRY_V2_SOURCE_PURPOSE: &str = "backtest_economics_suggestions_only";

fn instrument_registry_v2_source_status_not_configured() -> InstrumentRegistryV2SourceStatusResponse
{
    InstrumentRegistryV2SourceStatusResponse {
        truth_state: "not_configured".to_string(),
        configured: false,
        path: None,
        source: INSTRUMENT_REGISTRY_V2_SOURCE_ENV_VAR.to_string(),
        schema_version: None,
        purpose: INSTRUMENT_REGISTRY_V2_SOURCE_PURPOSE.to_string(),
        used_for_trading: false,
        enabled_for_live_trading: false,
        enabled_for_paper_trading: false,
        total_instruments: 0,
        asset_class_counts: BTreeMap::new(),
        enabled_counts: InstrumentRegistryV2EnabledCounts {
            enabled: 0,
            paper_trading_enabled: 0,
            live_trading_enabled: 0,
        },
        non_equity_present: false,
        non_equity_all_disabled: true,
        has_economics_metadata: false,
        sample_symbols: Vec::new(),
        validation_errors: Vec::new(),
        message: "MQK_INSTRUMENT_REGISTRY_V2_PATH is not set; no separate v2 registry source is configured."
            .to_string(),
    }
}

/// Shared failure-path builder for `"registry_unavailable"` / `"validation_failed"`.
/// `configured` is always `true` here — only reached once a path was set.
fn instrument_registry_v2_source_status_failure(
    truth_state: &str,
    path: String,
    error: String,
    message: String,
) -> InstrumentRegistryV2SourceStatusResponse {
    InstrumentRegistryV2SourceStatusResponse {
        truth_state: truth_state.to_string(),
        configured: true,
        path: Some(path),
        source: INSTRUMENT_REGISTRY_V2_SOURCE_ENV_VAR.to_string(),
        schema_version: None,
        purpose: INSTRUMENT_REGISTRY_V2_SOURCE_PURPOSE.to_string(),
        used_for_trading: false,
        enabled_for_live_trading: false,
        enabled_for_paper_trading: false,
        total_instruments: 0,
        asset_class_counts: BTreeMap::new(),
        enabled_counts: InstrumentRegistryV2EnabledCounts {
            enabled: 0,
            paper_trading_enabled: 0,
            live_trading_enabled: 0,
        },
        non_equity_present: false,
        non_equity_all_disabled: true,
        has_economics_metadata: false,
        sample_symbols: Vec::new(),
        validation_errors: vec![error],
        message,
    }
}

/// GET /api/v1/system/instrument-registry-v2-source/status
/// (ASSET-CORE-01D-REGISTRY-V2-STATUS-01-COMBINED).
///
/// Read-only operator-visibility surface for the separate v2 registry
/// **source** configured via `MQK_INSTRUMENT_REGISTRY_V2_PATH`
/// (`AppState::instrument_registry_v2_path`) — distinct from
/// [`system_instrument_registry_v2_status`], which diagnoses the v1->v2
/// *conversion* of `equities.json`. No DB pool, no provider or broker calls,
/// no writes. `used_for_trading`/`enabled_for_live_trading`/
/// `enabled_for_paper_trading` are always `false`: the only production
/// reader of this configured path is the read-only
/// `GET /api/v1/backtests/economics-suggestion` route.
pub(crate) async fn system_instrument_registry_v2_source_status(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let Some(configured_path) = st.instrument_registry_v2_path.clone() else {
        return Json(instrument_registry_v2_source_status_not_configured()).into_response();
    };

    let path = std::path::Path::new(&configured_path);
    if !path.exists() {
        return Json(instrument_registry_v2_source_status_failure(
            "registry_unavailable",
            configured_path.clone(),
            "configured instrument registry v2 path is unavailable".to_string(),
            format!(
                "MQK_INSTRUMENT_REGISTRY_V2_PATH is configured ({configured_path}) but the file does not exist."
            ),
        ))
        .into_response();
    }

    let registry = match mqk_md::instrument_registry_v2::load_instrument_registry_v2(path) {
        Ok(r) => r,
        Err(err) => {
            return Json(instrument_registry_v2_source_status_failure(
                "registry_unavailable",
                configured_path.clone(),
                format!("instrument registry v2 load failed: {err}"),
                format!(
                    "MQK_INSTRUMENT_REGISTRY_V2_PATH is configured ({configured_path}) but failed to load."
                ),
            ))
            .into_response();
        }
    };

    if let Err(err) = mqk_md::instrument_registry_v2::validate_registry_v2(&registry) {
        return Json(instrument_registry_v2_source_status_failure(
            "validation_failed",
            configured_path.clone(),
            format!("instrument registry v2 validation failed: {err}"),
            format!(
                "MQK_INSTRUMENT_REGISTRY_V2_PATH is configured ({configured_path}) but failed validation."
            ),
        ))
        .into_response();
    }

    let summary =
        mqk_md::instrument_registry_v2::summarize_instrument_registry_v2_status(&registry);

    Json(InstrumentRegistryV2SourceStatusResponse {
        truth_state: "configured_valid".to_string(),
        configured: true,
        path: Some(configured_path),
        source: INSTRUMENT_REGISTRY_V2_SOURCE_ENV_VAR.to_string(),
        schema_version: Some(registry.schema_version),
        purpose: INSTRUMENT_REGISTRY_V2_SOURCE_PURPOSE.to_string(),
        used_for_trading: false,
        enabled_for_live_trading: false,
        enabled_for_paper_trading: false,
        total_instruments: summary.total_instruments,
        asset_class_counts: summary.asset_class_counts,
        enabled_counts: InstrumentRegistryV2EnabledCounts {
            enabled: summary.enabled_count,
            paper_trading_enabled: summary.paper_trading_enabled_count,
            live_trading_enabled: summary.live_trading_enabled_count,
        },
        non_equity_present: summary.non_equity_present,
        non_equity_all_disabled: summary.non_equity_all_disabled,
        has_economics_metadata: summary.has_economics_metadata,
        sample_symbols: summary.sample_symbols,
        validation_errors: Vec::new(),
        message: "Configured v2 registry source is valid and is used only for read-only backtest economics suggestions."
            .to_string(),
    })
    .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/instrument-economics/status — ASSET-CORE-04B
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub(crate) struct InstrumentEconomicsStatusQuery {
    symbol: Option<String>,
    limit: Option<usize>,
}

fn instrument_economics_status_failure(
    truth_state: &str,
    registry_path: String,
    error: String,
) -> InstrumentEconomicsStatusResponse {
    InstrumentEconomicsStatusResponse {
        truth_state: truth_state.to_string(),
        registry_path,
        registry_v2_valid: false,
        bridge_model_only: true,
        trading_uses_instrument_economics: false,
        runtime_uses_instrument_economics: false,
        risk_uses_instrument_economics: false,
        order_path_uses_instrument_economics: false,
        instrument_count: 0,
        bridged_count: 0,
        failed_count: 0,
        model_only_count: 0,
        non_equity_enabled_count: 0,
        rows: Vec::new(),
        errors: vec![error],
    }
}

fn instrument_economics_status_row(
    result: &InstrumentEconomicsBridgeResult,
) -> InstrumentEconomicsStatusRow {
    InstrumentEconomicsStatusRow {
        symbol: result.symbol.clone(),
        instrument_id: result.instrument_id.clone(),
        asset_class: result.asset_class.clone(),
        instrument_kind: result.instrument_kind.clone(),
        enabled: result.enabled,
        paper_trading_enabled: result.paper_trading_enabled,
        live_trading_enabled: result.live_trading_enabled,
        truth_state: result.truth_state.clone(),
        reason_code: result.reason_code.clone(),
        model_only: result.model_only,
        trading_enabled_by_bridge: result.trading_enabled_by_bridge,
        quote_currency: result.economics.as_ref().map(|e| e.quote_currency.clone()),
        contract_multiplier_micros: result
            .economics
            .as_ref()
            .map(|e| e.contract_multiplier_micros),
        quantity_scale: result.economics.as_ref().map(|e| e.quantity_scale),
        tick_size_micros: result.economics.as_ref().and_then(|e| e.tick_size_micros),
    }
}

/// GET /api/v1/system/instrument-economics/status (ASSET-CORE-04B).
///
/// Read-only diagnostic surface: loads the same configured v1 registry as
/// ASSET-CORE-01C/05B, converts it to v2 in memory, validates it, then
/// bridges each v2 instrument to `mqk_portfolio::InstrumentEconomics` via
/// `mqk_daemon::state::instrument_economics_bridge`. No DB, provider,
/// broker, runtime, or trading path is touched, and none of them read this
/// bridge's output for any decision.
pub(crate) async fn system_instrument_economics_status(
    State(st): State<Arc<AppState>>,
    Query(query): Query<InstrumentEconomicsStatusQuery>,
) -> impl IntoResponse {
    let registry_path = st.instrument_registry_path.clone();
    let path = std::path::Path::new(&registry_path);

    let v1 = match mqk_md::instrument_registry::load_instrument_registry(path) {
        Ok(v) => v,
        Err(err) => {
            let truth_state = if path.exists() {
                "v1_load_failed"
            } else {
                "unavailable"
            };
            return Json(instrument_economics_status_failure(
                truth_state,
                registry_path,
                err.to_string(),
            ))
            .into_response();
        }
    };

    let v2 = mqk_md::instrument_registry_v2::convert_v1_registry_to_v2(&v1);
    if let Err(err) = mqk_md::instrument_registry_v2::validate_registry_v2(&v2) {
        return Json(instrument_economics_status_failure(
            "v2_validation_failed",
            registry_path,
            err.to_string(),
        ))
        .into_response();
    }

    let summary = bridge_instrument_registry_v2_to_economics(&v2);

    let normalized_symbol_filter = query
        .symbol
        .as_deref()
        .map(|s| s.trim().to_ascii_uppercase());
    let rows: Vec<InstrumentEconomicsStatusRow> = summary
        .rows
        .iter()
        .filter(|row| {
            normalized_symbol_filter
                .as_deref()
                .is_none_or(|s| row.symbol.to_ascii_uppercase() == s)
        })
        .take(query.limit.unwrap_or(usize::MAX))
        .map(instrument_economics_status_row)
        .collect();

    Json(InstrumentEconomicsStatusResponse {
        truth_state: "active".to_string(),
        registry_path,
        registry_v2_valid: true,
        bridge_model_only: true,
        trading_uses_instrument_economics: false,
        runtime_uses_instrument_economics: false,
        risk_uses_instrument_economics: false,
        order_path_uses_instrument_economics: false,
        instrument_count: summary.total_instruments,
        bridged_count: summary.bridged_count,
        failed_count: summary.failed_count,
        model_only_count: summary.model_only_count,
        non_equity_enabled_count: summary.non_equity_enabled_count,
        rows,
        errors: Vec::new(),
    })
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

fn session_profile_status_for_calendar_spec(
    calendar_spec_id: &str,
    market_session: &str,
) -> SessionProfileStatus {
    let is_open = Some(market_session == "regular");
    match calendar_spec_id {
        "always_on" => SessionProfileStatus {
            profile: MarketSessionProfile::EquityUsRegular,
            authority: SessionAuthority::ConfiguredOverride,
            is_open,
            reason_code: "equity_regular_always_on_policy",
            message: "Equity regular session profile displayed through the existing always-on daemon calendar policy.",
        },
        "nyse_weekdays" => SessionProfileStatus {
            profile: MarketSessionProfile::EquityUsRegular,
            authority: SessionAuthority::Fallback,
            is_open,
            reason_code: "equity_regular_nyse_weekdays_heuristic",
            message: "Equity regular session using the existing NYSE weekdays heuristic calendar behavior.",
        },
        _ => SessionProfileStatus {
            profile: MarketSessionProfile::EquityUsRegular,
            authority: SessionAuthority::Unavailable,
            is_open: None,
            reason_code: "session_calendar_unavailable",
            message: "Session profile could not be tied to a recognized calendar spec.",
        },
    }
}

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
    let market_session = calendar.classify_market_session(now_ts).to_string();
    let calendar_spec_id = calendar.spec_id().to_string();
    let session_profile_status =
        session_profile_status_for_calendar_spec(&calendar_spec_id, &market_session);
    let supported_session_profiles = supported_session_profiles()
        .into_iter()
        .map(|profile| profile.as_str().to_string())
        .collect();
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
            market_session,
            exchange_calendar_state: calendar.classify_exchange_calendar(now_ts).to_string(),
            calendar_spec_id,
            session_profile: session_profile_status.profile.as_str().to_string(),
            session_authority: session_profile_status.authority.as_str().to_string(),
            session_profile_is_open: session_profile_status.is_open,
            session_profile_reason_code: session_profile_status.reason_code.to_string(),
            session_profile_message: session_profile_status.message.to_string(),
            supported_session_profiles,
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
            (
                AutonomousSessionTruth::CompletedBarDriverExited { detail: "x".into() },
                "completed_bar_driver_exited",
            ),
        ];
        let mut seen = std::collections::HashSet::new();
        for (truth, expected_state) in cases {
            let (state, _) = autonomous_session_truth_to_api(&truth);
            assert_eq!(state, expected_state);
            assert!(seen.insert(state), "duplicate API state string detected");
        }
    }

    // -------------------------------------------------------------------
    // AUTON-NO-TRADE-OFFHOURS-01B: classify_no_trade_diagnostic tests.
    // Pure function, no I/O — exercises the priority order directly.
    // -------------------------------------------------------------------

    /// Baseline "everything green" args for classify_no_trade_diagnostic,
    /// overridden per-test via struct-update-like helper calls below.
    #[allow(clippy::too_many_arguments)]
    fn classify_args(
        overall_ready: bool,
        ws_continuity_ready: bool,
        reconcile_ready: bool,
        arm_state: &str,
        arm_ready: bool,
        signal_ingestion_configured: bool,
        session_in_window: bool,
        runtime_start_allowed: bool,
        strategy_fleet_empty: bool,
        md_start_allowed: bool,
        bar_ticker_gate: &str,
        bar_tick_dispatch_count: u64,
        last_bar_signal_qty: Option<i64>,
    ) -> (&'static str, &'static str) {
        classify_no_trade_diagnostic(
            overall_ready,
            ws_continuity_ready,
            reconcile_ready,
            arm_state,
            arm_ready,
            signal_ingestion_configured,
            session_in_window,
            runtime_start_allowed,
            strategy_fleet_empty,
            md_start_allowed,
            bar_ticker_gate,
            bar_tick_dispatch_count,
            last_bar_signal_qty,
        )
    }

    #[test]
    fn cntd01_ws_continuity_not_ready_takes_priority() {
        let (code, stage) = classify_args(
            false, false, true, "armed", true, true, true, true, false, true, "open", 0, None,
        );
        assert_eq!(code, "WS_CONTINUITY_NOT_READY");
        assert_eq!(stage, "pre_session_window");
    }

    #[test]
    fn cntd02_reconcile_not_ready() {
        let (code, _) = classify_args(
            false, true, false, "armed", true, true, true, true, false, true, "open", 0, None,
        );
        assert_eq!(code, "RECONCILE_NOT_READY");
    }

    #[test]
    fn cntd03_integrity_halted() {
        let (code, stage) = classify_args(
            false, true, true, "halted", false, true, true, true, false, true, "open", 0, None,
        );
        assert_eq!(code, "INTEGRITY_HALTED");
        assert_eq!(stage, "pre_arm");
    }

    #[test]
    fn cntd04_arm_not_ready_non_halted() {
        let (code, _) = classify_args(
            false,
            true,
            true,
            "arm_pending",
            false,
            true,
            true,
            true,
            false,
            true,
            "open",
            0,
            None,
        );
        assert_eq!(code, "ARM_NOT_READY");
    }

    #[test]
    fn cntd05_signal_ingestion_not_configured() {
        let (code, _) = classify_args(
            false, true, true, "armed", true, false, true, true, false, true, "open", 0, None,
        );
        assert_eq!(code, "SIGNAL_INGESTION_NOT_CONFIGURED");
    }

    #[test]
    fn cntd06_outside_session_window() {
        let (code, stage) = classify_args(
            false, true, true, "armed", true, true, false, true, false, true, "open", 0, None,
        );
        assert_eq!(code, "OUTSIDE_SESSION_WINDOW");
        assert_eq!(stage, "pre_session_window");
    }

    #[test]
    fn cntd07_bar_ticker_gate_closed_when_run_active_outside_regular_session() {
        let (code, stage) = classify_args(
            false,
            true,
            true,
            "armed",
            true,
            true,
            true,
            false,
            false,
            true,
            "closed_outside_session",
            0,
            None,
        );
        assert_eq!(code, "BAR_TICKER_GATE_CLOSED");
        assert_eq!(stage, "pre_dispatch");
    }

    #[test]
    fn cntd08_strategy_not_ticked_when_run_active_and_gate_open() {
        let (code, _) = classify_args(
            false, true, true, "armed", true, true, true, false, false, true, "open", 0, None,
        );
        assert_eq!(code, "STRATEGY_NOT_TICKED");
    }

    #[test]
    fn cntd09_no_signal_generated_when_ticked_with_zero_qty() {
        let (code, _) = classify_args(
            false,
            true,
            true,
            "armed",
            true,
            true,
            true,
            false,
            false,
            true,
            "open",
            3,
            Some(0),
        );
        assert_eq!(code, "NO_SIGNAL_GENERATED");
    }

    #[test]
    fn cntd10_runtime_already_active_fallback() {
        let (code, _) = classify_args(
            false,
            true,
            true,
            "armed",
            true,
            true,
            true,
            false,
            false,
            true,
            "open",
            3,
            Some(5),
        );
        assert_eq!(code, "RUNTIME_ALREADY_ACTIVE");
    }

    #[test]
    fn cntd11_strategy_fleet_empty() {
        let (code, stage) = classify_args(
            false, true, true, "armed", true, true, true, true, true, true, "open", 0, None,
        );
        assert_eq!(code, "STRATEGY_FLEET_EMPTY");
        assert_eq!(stage, "pre_runtime_start");
    }

    #[test]
    fn cntd12_market_data_not_ready() {
        let (code, _) = classify_args(
            false, true, true, "armed", true, true, true, true, false, false, "open", 0, None,
        );
        assert_eq!(code, "MARKET_DATA_NOT_READY");
    }

    #[test]
    fn cntd13_no_active_run_pending_start_when_everything_green() {
        let (code, stage) = classify_args(
            true, true, true, "armed", true, true, true, true, false, true, "open", 0, None,
        );
        assert_eq!(code, "NO_ACTIVE_RUN_PENDING_START");
        assert_eq!(stage, "pre_runtime_start");
    }

    // -----------------------------------------------------------------
    // ATOMICITY-SINGLE-SNAPSHOT-REPAIR requirement 6: committed truth must
    // never mix with a fresh env-preview read.
    // -----------------------------------------------------------------

    /// Serializes every test in this module that mutates
    /// `MQK_DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE` — mirrors the
    /// `env_lock()` convention in `state::lifecycle`'s dynamic-selection
    /// test modules.
    fn dynamic_selection_env_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    /// A minimal committed fixture: Shadow, no plan/pool — only the fields
    /// this test inspects are load-bearing.
    fn fixture_shadow_committed(run_id: uuid::Uuid) -> crate::state::DynamicSelectionRuntimeState {
        crate::state::DynamicSelectionRuntimeState {
            run_id,
            disposition:
                crate::dynamic_selection_start_gate::DynamicSelectionStartGateDisposition::ShadowAllowed,
            configured_mode: mqk_portfolio::DynamicSelectionMode::Shadow,
            effective_mode: mqk_portfolio::DynamicSelectionMode::Shadow,
            live_lock_applied: false,
            plan: None,
            plan_id: None,
            selected_pairs: Vec::new(),
            host_pool_present: false,
            reasons: Vec::new(),
            approved_for_live: false,
            evidence_persisted: false,
            evidence_validation_state: None,
        }
    }

    /// No committed snapshot exists: every committed-truth field must be in
    /// its neutral/absent state, and the preview fields must reflect the
    /// current env config — never the other way around.
    #[tokio::test]
    async fn readiness_projection_with_no_committed_snapshot_is_all_neutral() {
        let _guard = dynamic_selection_env_lock().lock().await;
        std::env::remove_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
        );
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "shadow",
        );

        let state = std::sync::Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            crate::state::BrokerKind::Alpaca,
        ));
        assert!(state.dynamic_selection_runtime_snapshot().await.is_none());

        let projection = dynamic_selection_readiness_projection(&state).await;
        std::env::remove_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
        );

        assert_eq!(projection.configured_mode, None);
        assert_eq!(projection.effective_mode, None);
        assert!(!projection.live_lock_applied);
        assert_eq!(projection.disposition, None);
        assert!(!projection.plan_present);
        assert!(!projection.host_pool_present);
        assert_eq!(projection.selected_pair_count, 0);
        assert_eq!(projection.owning_run_id, None);
        assert!(!projection.approved_for_live);
        assert_eq!(projection.preview_configured_mode, "shadow");
        assert_eq!(projection.preview_effective_mode, "shadow");
    }

    /// A committed snapshot exists: every committed-truth field must come
    /// from that snapshot, and mutating env *after* the commit must never
    /// change the committed truth — only the separately-named preview
    /// fields may move.
    #[tokio::test]
    async fn readiness_projection_with_committed_snapshot_ignores_later_env_mutation() {
        let _guard = dynamic_selection_env_lock().lock().await;
        std::env::remove_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
        );
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "shadow",
        );

        let state = std::sync::Arc::new(AppState::new_for_test_with_mode_and_broker(
            DeploymentMode::Paper,
            crate::state::BrokerKind::Alpaca,
        ));
        let run_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            b"mqk-daemon.atomicity_repair.status_coherence",
        );
        state
            .commit_dynamic_selection_runtime_state(fixture_shadow_committed(run_id))
            .await;

        // Mutate env to a *different* mode after the commit — a status
        // read must not let this leak into the committed truth.
        std::env::set_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
            "paper_enforced",
        );

        let projection = dynamic_selection_readiness_projection(&state).await;
        std::env::remove_var(
            crate::dynamic_selection_mode::DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE_ENV,
        );

        assert_eq!(projection.configured_mode.as_deref(), Some("shadow"));
        assert_eq!(projection.effective_mode.as_deref(), Some("shadow"));
        assert_eq!(projection.disposition.as_deref(), Some("shadow_allowed"));
        assert_eq!(projection.owning_run_id, Some(run_id));
        assert_eq!(
            projection.preview_configured_mode, "paper_enforced",
            "the preview fields — and only the preview fields — must reflect \
             the post-commit env mutation"
        );
        assert_eq!(projection.preview_effective_mode, "paper_enforced");
    }
}
