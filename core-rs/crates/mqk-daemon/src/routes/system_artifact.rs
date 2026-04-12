//! Artifact, evidence, and topology route handlers.
//!
//! Split from `routes/system.rs` (MT-01).  Contains handlers whose concern is
//! artifact provenance, parity evidence, and service topology — all pure
//! file/state reads with no DB queries.
//!
//! Contains: system_artifact_intake, system_run_artifact,
//! system_parity_evidence, system_topology.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;

use crate::api_types::{
    ArtifactIntakeResponse, ParityEvidenceResponse, RunArtifactProvenanceResponse,
    SystemTopologyResponse, SystemTopologyServiceRow,
};
use crate::artifact_intake::{
    evaluate_artifact_intake_guarded, ArtifactIntakeOutcome, ENV_ARTIFACT_PATH,
};
use crate::parity_evidence::{evaluate_parity_evidence_guarded, ParityEvidenceOutcome};
use crate::state::{AlpacaWsContinuityState, AppState, StrategyMarketDataSource};

// ---------------------------------------------------------------------------
// TV-01B: GET /api/v1/system/artifact-intake
// ---------------------------------------------------------------------------

/// TV-01B: Runtime artifact intake truth surface.
///
/// Reads `MQK_ARTIFACT_PATH` from the environment, validates the
/// `promoted_manifest.json` it points to, and returns the honest intake
/// outcome.  No AppState is needed — this is a pure file-based read.
///
/// The `State` parameter is accepted only to keep the handler signature
/// consistent with other routes; it is not used in the intake evaluation.
pub(crate) async fn system_artifact_intake(State(_st): State<Arc<AppState>>) -> impl IntoResponse {
    let evaluated_path = std::env::var(ENV_ARTIFACT_PATH)
        .ok()
        .filter(|s| !s.trim().is_empty());

    // Use the panic-safe guarded entry point so unexpected evaluator failures
    // surface as `Unavailable` rather than crashing the request handler.
    let outcome = evaluate_artifact_intake_guarded();

    let response = match outcome {
        ArtifactIntakeOutcome::NotConfigured => ArtifactIntakeResponse {
            canonical_route: "/api/v1/system/artifact-intake".to_string(),
            truth_state: "not_configured".to_string(),
            artifact_id: None,
            artifact_type: None,
            stage: None,
            produced_by: None,
            invalid_reason: None,
            evaluated_path: None,
        },
        ArtifactIntakeOutcome::Invalid { reason } => ArtifactIntakeResponse {
            canonical_route: "/api/v1/system/artifact-intake".to_string(),
            truth_state: "invalid".to_string(),
            artifact_id: None,
            artifact_type: None,
            stage: None,
            produced_by: None,
            invalid_reason: Some(reason),
            evaluated_path: evaluated_path.clone(),
        },
        ArtifactIntakeOutcome::Accepted {
            artifact_id,
            artifact_type,
            stage,
            produced_by,
        } => ArtifactIntakeResponse {
            canonical_route: "/api/v1/system/artifact-intake".to_string(),
            truth_state: "accepted".to_string(),
            artifact_id: Some(artifact_id),
            artifact_type: Some(artifact_type),
            stage: Some(stage),
            produced_by: Some(produced_by),
            invalid_reason: None,
            evaluated_path: evaluated_path.clone(),
        },
        ArtifactIntakeOutcome::Unavailable { reason } => ArtifactIntakeResponse {
            canonical_route: "/api/v1/system/artifact-intake".to_string(),
            truth_state: "unavailable".to_string(),
            artifact_id: None,
            artifact_type: None,
            stage: None,
            produced_by: None,
            // `invalid_reason` carries the reason for both `invalid` and `unavailable`
            // outcomes.  Callers must check `truth_state` to distinguish the two.
            invalid_reason: Some(reason),
            evaluated_path: evaluated_path.clone(),
        },
    };

    (StatusCode::OK, Json(response)).into_response()
}

// ---------------------------------------------------------------------------
// TV-01C: GET /api/v1/system/run-artifact
// ---------------------------------------------------------------------------

/// TV-01C: Run-artifact provenance truth surface.
///
/// Returns the artifact accepted at the most recent `start_execution_runtime`.
/// `truth_state = "active"` with all identity fields when a run is active with
/// an accepted artifact; `truth_state = "no_run"` with null fields otherwise.
///
/// Distinct from `/api/v1/system/artifact-intake`: that route re-evaluates the
/// configured file on demand; this route surfaces what was actually accepted and
/// consumed when the run started.  Fail-closed: never synthesises positive
/// provenance when no run is active.
pub(crate) async fn system_run_artifact(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let provenance = st.accepted_artifact_provenance().await;
    let response = match provenance {
        Some(p) => RunArtifactProvenanceResponse {
            canonical_route: "/api/v1/system/run-artifact".to_string(),
            truth_state: "active".to_string(),
            artifact_id: Some(p.artifact_id),
            artifact_type: Some(p.artifact_type),
            stage: Some(p.stage),
            produced_by: Some(p.produced_by),
        },
        None => RunArtifactProvenanceResponse {
            canonical_route: "/api/v1/system/run-artifact".to_string(),
            truth_state: "no_run".to_string(),
            artifact_id: None,
            artifact_type: None,
            stage: None,
            produced_by: None,
        },
    };
    (StatusCode::OK, Json(response)).into_response()
}

// ---------------------------------------------------------------------------
// TV-03B: GET /api/v1/system/parity-evidence
// ---------------------------------------------------------------------------

/// TV-03B: Parity evidence truth surface.
///
/// Reads `parity_evidence.json` (schema `parity-v1`) from the artifact
/// directory configured via `MQK_ARTIFACT_PATH` and surfaces the honest
/// parity-evidence state for the operator.
///
/// Distinct from `/api/v1/system/artifact-intake` (structural acceptance) and
/// `/api/v1/system/artifact-deployability` (tradability gate): this route
/// surfaces whether shadow/live parity evidence has been produced and what
/// trust gaps remain.
///
/// Fail-closed: absent, invalid, and unavailable are never conflated with
/// "present".  `live_trust_complete=false` is surfaced explicitly rather than
/// hidden.
pub(crate) async fn system_parity_evidence(State(_st): State<Arc<AppState>>) -> impl IntoResponse {
    let evaluated_path = std::env::var(ENV_ARTIFACT_PATH)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|p| {
            // Surface the artifact directory path, not the manifest file path,
            // so the operator can navigate directly to the evidence directory.
            std::path::PathBuf::from(p.trim())
                .parent()
                .map(|d| d.to_string_lossy().to_string())
                .unwrap_or_else(|| p.trim().to_string())
        });

    let outcome = evaluate_parity_evidence_guarded();

    let response = match outcome {
        ParityEvidenceOutcome::NotConfigured => ParityEvidenceResponse {
            canonical_route: "/api/v1/system/parity-evidence".to_string(),
            truth_state: "not_configured".to_string(),
            artifact_id: None,
            live_trust_complete: None,
            evidence_available: None,
            evidence_note: None,
            produced_at_utc: None,
            invalid_reason: None,
            evaluated_path: None,
        },
        ParityEvidenceOutcome::Absent => ParityEvidenceResponse {
            canonical_route: "/api/v1/system/parity-evidence".to_string(),
            truth_state: "absent".to_string(),
            artifact_id: None,
            live_trust_complete: None,
            evidence_available: None,
            evidence_note: None,
            produced_at_utc: None,
            invalid_reason: None,
            evaluated_path: evaluated_path.clone(),
        },
        ParityEvidenceOutcome::Invalid { reason } => ParityEvidenceResponse {
            canonical_route: "/api/v1/system/parity-evidence".to_string(),
            truth_state: "invalid".to_string(),
            artifact_id: None,
            live_trust_complete: None,
            evidence_available: None,
            evidence_note: None,
            produced_at_utc: None,
            invalid_reason: Some(reason),
            evaluated_path: evaluated_path.clone(),
        },
        ParityEvidenceOutcome::Present {
            artifact_id,
            live_trust_complete,
            evidence_available,
            evidence_note,
            produced_at_utc,
        } => ParityEvidenceResponse {
            canonical_route: "/api/v1/system/parity-evidence".to_string(),
            truth_state: "present".to_string(),
            artifact_id: Some(artifact_id),
            live_trust_complete: Some(live_trust_complete),
            evidence_available: Some(evidence_available),
            evidence_note: Some(evidence_note),
            produced_at_utc: Some(produced_at_utc),
            invalid_reason: None,
            evaluated_path: evaluated_path.clone(),
        },
        ParityEvidenceOutcome::Unavailable { reason } => ParityEvidenceResponse {
            canonical_route: "/api/v1/system/parity-evidence".to_string(),
            truth_state: "unavailable".to_string(),
            artifact_id: None,
            live_trust_complete: None,
            evidence_available: None,
            evidence_note: None,
            produced_at_utc: None,
            invalid_reason: Some(reason),
            evaluated_path: evaluated_path.clone(),
        },
    };

    (StatusCode::OK, Json(response)).into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/topology (A3)
// ---------------------------------------------------------------------------

/// Surface honest local-daemon service topology.
///
/// Derived entirely from daemon in-memory state — no DB query, no broker call.
/// `truth_state` is always `"active"`.  Represents single-process local truth
/// only; no cluster or distributed topology is claimed.
pub(crate) async fn system_topology(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let has_db = st.db.is_some();
    let exec_snap_present = st.execution_snapshot.read().await.is_some();
    let ws = st.alpaca_ws_continuity().await;
    let source = st.strategy_market_data_source();
    let now_utc = Utc::now().to_rfc3339();

    let mut services: Vec<SystemTopologyServiceRow> = Vec::new();

    // daemon.runtime
    services.push(SystemTopologyServiceRow {
        service_key: "daemon.runtime".to_string(),
        label: "MQK Daemon".to_string(),
        layer: "runtime".to_string(),
        health: "ok".to_string(),
        role: "Execution orchestrator; owns all execution gates and state transitions".to_string(),
        dependency_keys: vec![],
        failure_impact: "Total execution halt; all gates fail closed".to_string(),
        last_heartbeat: Some(now_utc.clone()),
        latency_ms: None,
        notes: format!("build={}", st.build.version),
    });

    // postgres
    let db_health = if has_db { "ok" } else { "not_configured" };
    services.push(SystemTopologyServiceRow {
        service_key: "postgres".to_string(),
        label: "PostgreSQL".to_string(),
        layer: "data".to_string(),
        health: db_health.to_string(),
        role: "Durable state: runs, outbox, broker_order_map, audit_events".to_string(),
        dependency_keys: vec!["daemon.runtime".to_string()],
        failure_impact: "No durable run state; DB-backed gates fail closed (503)".to_string(),
        last_heartbeat: None,
        latency_ms: None,
        notes: if has_db {
            "pool configured".to_string()
        } else {
            "no pool — DB-backed routes return 503 or not_wired".to_string()
        },
    });

    // execution_loop
    let exec_health = if exec_snap_present {
        "ok"
    } else {
        "not_started"
    };
    services.push(SystemTopologyServiceRow {
        service_key: "execution_loop".to_string(),
        label: "Execution Loop".to_string(),
        layer: "execution".to_string(),
        health: exec_health.to_string(),
        role: "Tick-driven OMS; claims outbox, dispatches to broker, applies inbox fills"
            .to_string(),
        dependency_keys: vec!["daemon.runtime".to_string(), "postgres".to_string()],
        failure_impact: "No order dispatch; fills and outbox processing halted".to_string(),
        last_heartbeat: None,
        latency_ms: None,
        notes: if exec_snap_present {
            "snapshot present".to_string()
        } else {
            "no active run; start execution loop to activate".to_string()
        },
    });

    // broker.adapter
    let (broker_health, broker_notes) = match &ws {
        AlpacaWsContinuityState::Live { .. } => ("ok", "WS continuity: live".to_string()),
        AlpacaWsContinuityState::ColdStartUnproven => (
            "warning",
            "WS continuity: cold_start_unproven; signal ingestion blocked".to_string(),
        ),
        AlpacaWsContinuityState::GapDetected { detail, .. } => (
            "critical",
            format!("WS continuity: gap_detected — {detail}"),
        ),
        AlpacaWsContinuityState::NotApplicable => (
            "unknown",
            "broker adapter not wired (paper or null adapter)".to_string(),
        ),
    };
    services.push(SystemTopologyServiceRow {
        service_key: "broker.adapter".to_string(),
        label: "Broker Adapter".to_string(),
        layer: "broker".to_string(),
        health: broker_health.to_string(),
        role: "Translates OMS intents to broker API calls; normalises fill events".to_string(),
        dependency_keys: vec!["daemon.runtime".to_string()],
        failure_impact: "No order submission or fill delivery to the OMS inbox".to_string(),
        last_heartbeat: None,
        latency_ms: None,
        notes: broker_notes,
    });

    // strategy.data_source
    let (md_health, md_notes) = match source {
        StrategyMarketDataSource::NotConfigured => (
            "not_configured",
            "no market-data source wired; strategy signals cannot be admitted".to_string(),
        ),
        StrategyMarketDataSource::ExternalSignalIngestion => (
            "ok",
            "signal_ingestion_ready; POST /api/v1/strategy/signal admits signals".to_string(),
        ),
    };
    services.push(SystemTopologyServiceRow {
        service_key: "strategy.data_source".to_string(),
        label: "Strategy Market-Data Source".to_string(),
        layer: "strategy".to_string(),
        health: md_health.to_string(),
        role: "Market-data ingestion path driving strategy signal computation".to_string(),
        dependency_keys: vec!["daemon.runtime".to_string()],
        failure_impact: "No strategy signals admitted; execution loop remains idle".to_string(),
        last_heartbeat: None,
        latency_ms: None,
        notes: md_notes,
    });

    (
        StatusCode::OK,
        Json(SystemTopologyResponse {
            canonical_route: "/api/v1/system/topology".to_string(),
            truth_state: "active".to_string(),
            backend: "daemon.runtime_state".to_string(),
            updated_at: now_utc,
            services,
        }),
    )
        .into_response()
}
