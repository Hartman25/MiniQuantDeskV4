//! Alert and event-feed route handlers (CC-06, OPS-09).
//!
//! Contains: alerts_active, events_feed.
//!
//! # Source model
//!
//! ## `/api/v1/alerts/active`
//!
//! Source: `build_fault_signals()` — current in-memory computation from
//! `StatusSnapshot` + `ReconcileStatusSnapshot` + DB-backed risk-block state
//! (falls back to `false` when no DB, consistent with `system/status`).
//!
//! OPS-09 adds Alpaca WS continuity supervision signals:
//! - `"paper.ws_continuity.cold_start_unproven"` (warning) when
//!   `AlpacaWsContinuityState::ColdStartUnproven` — signal ingestion blocked.
//! - `"paper.ws_continuity.gap_detected"` (critical) when
//!   `AlpacaWsContinuityState::GapDetected` — fill delivery unreliable.
//!
//! `truth_state` is always `"active"`: the computation uses in-memory state
//! that is always present.  Empty `rows` = genuinely no current fault
//! conditions (healthy state, not absence of source).
//!
//! ## `/api/v1/events/feed`
//!
//! Source: `postgres.runs` (runtime lifecycle transitions) +
//! `postgres.audit_events` (operator actions, topic=`'operator'`) +
//! `postgres.audit_events` (signal admissions, topic=`'signal_ingestion'`) +
//! `postgres.sys_autonomous_session_events` (autonomous supervisor history,
//! AUTON-PAPER-02).
//!
//! JOUR-01/OPS-09 adds `kind="signal_admission"` rows sourced from
//! `audit_events` with `topic='signal_ingestion'`.  These are written by the
//! strategy-signal route at Gate 7 `Ok(true)`.
//!
//! AUTON-PAPER-02 adds `kind="autonomous_session"` rows sourced from
//! `sys_autonomous_session_events`.  Written by `set_autonomous_session_truth`.
//!
//! `truth_state` = `"active"` when DB pool present;
//! `"backend_unavailable"` when no DB pool.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use sqlx::Row;

use crate::api_types::{
    ActiveAlertRow, ActiveAlertsResponse, AlertAckRequest, AlertAckResponse, AlertTriageAlertRow,
    AlertTriageResponse, CreateIncidentRequest, CreateIncidentResponse, EventFeedRow,
    EventsFeedResponse, IncidentRow, IncidentsResponse, ResolveIncidentResponse,
    RuntimeErrorResponse,
};
use crate::state::{
    AlpacaWsContinuityState, AppState, AutonomousSessionTruth, StrategyMarketDataSource,
};

use super::helpers::{build_fault_signals, runtime_error_response};

// ---------------------------------------------------------------------------
// GET /api/v1/alerts/active
// ---------------------------------------------------------------------------

pub(crate) async fn alerts_active(State(st): State<Arc<AppState>>) -> Response {
    let status = match st.current_status_snapshot().await {
        Ok(snap) => snap,
        Err(err) => return runtime_error_response(err),
    };
    let reconcile = st.current_reconcile_snapshot().await;

    // Risk-blocked state requires a DB query.  Falls back to false when no DB,
    // matching the behaviour of `GET /api/v1/system/status`.
    let risk_blocked = if let Some(db) = st.db.as_ref() {
        mqk_db::load_risk_block_state(db)
            .await
            .ok()
            .flatten()
            .is_some_and(|risk| risk.blocked)
    } else {
        false
    };

    let fault_signals = build_fault_signals(&status, &reconcile, risk_blocked);

    let mut rows: Vec<ActiveAlertRow> = fault_signals
        .into_iter()
        .map(|s| ActiveAlertRow {
            alert_id: s.class.clone(),
            severity: s.severity,
            class: s.class,
            summary: s.summary,
            detail: s.detail,
            source: "daemon.runtime_state".to_string(),
        })
        .collect();

    // OPS-09: Add Alpaca WS continuity supervision signals.
    //
    // ColdStartUnproven and GapDetected are both fail-closed states:
    // signal ingestion is blocked and fill delivery is unreliable.
    // Surface them as explicit active alerts so operators can react
    // without having to cross-reference /api/v1/system/status.
    let ws = st.alpaca_ws_continuity().await;
    match &ws {
        AlpacaWsContinuityState::ColdStartUnproven => {
            rows.push(ActiveAlertRow {
                alert_id: "paper.ws_continuity.cold_start_unproven".to_string(),
                severity: "warning".to_string(),
                class: "paper.ws_continuity.cold_start_unproven".to_string(),
                summary: "Alpaca WS continuity unproven (cold start); signal ingestion \
                          is blocked until WS transport establishes Live."
                    .to_string(),
                detail: None,
                source: "daemon.runtime_state".to_string(),
            });
        }
        AlpacaWsContinuityState::GapDetected { detail, .. } => {
            rows.push(ActiveAlertRow {
                alert_id: "paper.ws_continuity.gap_detected".to_string(),
                severity: "critical".to_string(),
                class: "paper.ws_continuity.gap_detected".to_string(),
                summary: "Alpaca WS gap detected; fill delivery is unreliable and \
                          signal ingestion is blocked until WS transport re-establishes Live."
                    .to_string(),
                detail: Some(detail.clone()),
                source: "daemon.runtime_state".to_string(),
            });
        }
        // NotApplicable (non-Alpaca) and Live (healthy) produce no additional signal.
        AlpacaWsContinuityState::NotApplicable | AlpacaWsContinuityState::Live { .. } => {}
    }

    match st.autonomous_session_truth().await {
        AutonomousSessionTruth::Clear => {}
        AutonomousSessionTruth::StartRefused { detail } => rows.push(ActiveAlertRow {
            alert_id: "autonomous.session.start_refused".to_string(),
            severity: "warning".to_string(),
            class: "autonomous.session.start_refused".to_string(),
            summary: "Autonomous paper session start is currently refused by backend gates; controller will retry when conditions change.".to_string(),
            detail: Some(detail),
            source: "daemon.autonomous_session".to_string(),
        }),
        AutonomousSessionTruth::RecoveryRetrying { resume_source, detail } => rows.push(ActiveAlertRow {
            alert_id: "autonomous.session.recovery_retrying".to_string(),
            severity: "warning".to_string(),
            class: "autonomous.session.recovery_retrying".to_string(),
            summary: format!(
                "Autonomous paper recovery is retrying via {} truth; start remains fail-closed until WS continuity is restored.",
                resume_source.as_str()
            ),
            detail: Some(detail),
            source: "daemon.autonomous_session".to_string(),
        }),
        AutonomousSessionTruth::RecoverySucceeded { resume_source, detail } => rows.push(ActiveAlertRow {
            alert_id: "autonomous.session.recovery_succeeded".to_string(),
            severity: "info".to_string(),
            class: "autonomous.session.recovery_succeeded".to_string(),
            summary: format!(
                "Autonomous paper recovery restored continuity using {} truth.",
                resume_source.as_str()
            ),
            detail: Some(detail),
            source: "daemon.autonomous_session".to_string(),
        }),
        AutonomousSessionTruth::RecoveryFailed { resume_source, detail } => rows.push(ActiveAlertRow {
            alert_id: "autonomous.session.recovery_failed".to_string(),
            severity: "critical".to_string(),
            class: "autonomous.session.recovery_failed".to_string(),
            summary: format!(
                "Autonomous paper recovery failed while resuming from {} truth; start remains blocked until continuity is proven again.",
                resume_source.as_str()
            ),
            detail: Some(detail),
            source: "daemon.autonomous_session".to_string(),
        }),
        AutonomousSessionTruth::RunEndedUnexpectedly { detail } => rows.push(ActiveAlertRow {
            alert_id: "autonomous.session.run_ended_unexpectedly".to_string(),
            severity: "warning".to_string(),
            class: "autonomous.session.run_ended_unexpectedly".to_string(),
            summary: "Autonomous paper run ended unexpectedly during the session window; controller retry logic is active.".to_string(),
            detail: Some(detail),
            source: "daemon.autonomous_session".to_string(),
        }),
        AutonomousSessionTruth::StopFailed { detail } => rows.push(ActiveAlertRow {
            alert_id: "autonomous.session.stop_failed".to_string(),
            severity: "warning".to_string(),
            class: "autonomous.session.stop_failed".to_string(),
            summary: "Autonomous paper stop at the session boundary failed; controller will retry while remaining fail-closed.".to_string(),
            detail: Some(detail),
            source: "daemon.autonomous_session".to_string(),
        }),
        AutonomousSessionTruth::StoppedAtBoundary { detail } => rows.push(ActiveAlertRow {
            alert_id: "autonomous.session.stopped_at_boundary".to_string(),
            severity: "info".to_string(),
            class: "autonomous.session.stopped_at_boundary".to_string(),
            summary: "Autonomous paper run stopped at the configured session boundary.".to_string(),
            detail: Some(detail),
            source: "daemon.autonomous_session".to_string(),
        }),
    }

    // AUTON-PAPER-01: Day signal limit alert.
    //
    // Surface an explicit active alert when the per-run autonomous signal
    // intake limit has been reached (PT-AUTO-02).  Only on the Paper+Alpaca
    // ExternalSignalIngestion path.  The alert fires whenever the counter is
    // saturated: if a run is active, further signals are immediately refused
    // by Gate 1d; if no run is active, the counter resets at the next run
    // start.  Both states are worth surfacing to the operator.
    if st.strategy_market_data_source() == StrategyMarketDataSource::ExternalSignalIngestion
        && st.day_signal_limit_exceeded()
    {
        rows.push(ActiveAlertRow {
            alert_id: "autonomous.signal_limit.day_limit_reached".to_string(),
            severity: "warning".to_string(),
            class: "autonomous.signal_limit.day_limit_reached".to_string(),
            summary: "Autonomous signal intake day limit reached; further signals are blocked \
                      until the next run start resets the counter (PT-AUTO-02)."
                .to_string(),
            detail: None,
            source: "daemon.runtime_state".to_string(),
        });
    }

    let alert_count = rows.len();

    (
        StatusCode::OK,
        Json(ActiveAlertsResponse {
            canonical_route: "/api/v1/alerts/active".to_string(),
            truth_state: "active".to_string(),
            backend: "daemon.runtime_state".to_string(),
            alert_count,
            rows,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/events/feed
// ---------------------------------------------------------------------------

pub(crate) async fn events_feed(State(st): State<Arc<AppState>>) -> Response {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(EventsFeedResponse {
                canonical_route: "/api/v1/events/feed".to_string(),
                truth_state: "backend_unavailable".to_string(),
                backend: "unavailable".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    // --- Runs: emit one row per durable lifecycle transition timestamp ---
    let runs = match sqlx::query(
        r#"
        select run_id, started_at_utc, armed_at_utc, running_at_utc, stopped_at_utc, halted_at_utc
        from runs
        order by started_at_utc desc, run_id desc
        limit 20
        "#,
    )
    .fetch_all(db)
    .await
    {
        Ok(r) => r,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("events/feed runs query failed: {err}"),
                    fault_class: "events.feed.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response()
        }
    };

    let mut rows: Vec<EventFeedRow> = Vec::new();

    for row in &runs {
        let run_id: uuid::Uuid = row.get("run_id");
        let started_at_utc: chrono::DateTime<chrono::Utc> = row.get("started_at_utc");
        let armed_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("armed_at_utc");
        let running_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("running_at_utc");
        let stopped_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("stopped_at_utc");
        let halted_at_utc: Option<chrono::DateTime<chrono::Utc>> = row.get("halted_at_utc");

        let run_id_str = run_id.to_string();

        rows.push(EventFeedRow {
            event_id: format!("runs:{}:started_at_utc", run_id),
            ts_utc: started_at_utc.to_rfc3339(),
            kind: "runtime_transition".to_string(),
            detail: "CREATED".to_string(),
            run_id: Some(run_id_str.clone()),
            provenance_ref: format!("runs:{}:started_at_utc", run_id),
        });
        if let Some(ts) = armed_at_utc {
            rows.push(EventFeedRow {
                event_id: format!("runs:{}:armed_at_utc", run_id),
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                detail: "ARMED".to_string(),
                run_id: Some(run_id_str.clone()),
                provenance_ref: format!("runs:{}:armed_at_utc", run_id),
            });
        }
        if let Some(ts) = running_at_utc {
            rows.push(EventFeedRow {
                event_id: format!("runs:{}:running_at_utc", run_id),
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                detail: "RUNNING".to_string(),
                run_id: Some(run_id_str.clone()),
                provenance_ref: format!("runs:{}:running_at_utc", run_id),
            });
        }
        if let Some(ts) = stopped_at_utc {
            rows.push(EventFeedRow {
                event_id: format!("runs:{}:stopped_at_utc", run_id),
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                detail: "STOPPED".to_string(),
                run_id: Some(run_id_str.clone()),
                provenance_ref: format!("runs:{}:stopped_at_utc", run_id),
            });
        }
        if let Some(ts) = halted_at_utc {
            rows.push(EventFeedRow {
                event_id: format!("runs:{}:halted_at_utc", run_id),
                ts_utc: ts.to_rfc3339(),
                kind: "runtime_transition".to_string(),
                detail: "HALTED".to_string(),
                run_id: Some(run_id_str.clone()),
                provenance_ref: format!("runs:{}:halted_at_utc", run_id),
            });
        }
    }

    // --- Operator audit events ---
    let operator_events = match sqlx::query(
        r#"
        select event_id, run_id, ts_utc, event_type
        from audit_events
        where topic = 'operator'
        order by ts_utc desc
        limit 50
        "#,
    )
    .fetch_all(db)
    .await
    {
        Ok(r) => r,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("events/feed audit query failed: {err}"),
                    fault_class: "events.feed.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response()
        }
    };

    for row in operator_events {
        let event_id: uuid::Uuid = row.get("event_id");
        let run_id: Option<uuid::Uuid> = row.get("run_id");
        let ts_utc: chrono::DateTime<chrono::Utc> = row.get("ts_utc");
        let event_type: String = row.get("event_type");

        rows.push(EventFeedRow {
            event_id: format!("audit_events:{}", event_id),
            ts_utc: ts_utc.to_rfc3339(),
            kind: "operator_action".to_string(),
            detail: event_type,
            run_id: run_id.map(|id| id.to_string()),
            provenance_ref: format!("audit_events:{}", event_id),
        });
    }

    // --- JOUR-01/OPS-09: Signal-admission events ---
    //
    // Written by strategy_signal at Gate 7 Ok(true).  Surface them in the
    // feed so operators can see signal intake alongside run transitions and
    // operator actions in one newest-first timeline.
    //
    // detail encodes "signal.admitted:{strategy_id}:{symbol}:{side}" for
    // quick scanning; the full payload lives in audit_events.
    let signal_events = match sqlx::query(
        r#"
        select event_id, run_id, ts_utc, payload
        from audit_events
        where topic = 'signal_ingestion'
          and event_type = 'signal.admitted'
        order by ts_utc desc
        limit 50
        "#,
    )
    .fetch_all(db)
    .await
    {
        Ok(r) => r,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("events/feed signal-admission query failed: {err}"),
                    fault_class: "events.feed.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response()
        }
    };

    for row in signal_events {
        let event_id: uuid::Uuid = row.get("event_id");
        let run_id: Option<uuid::Uuid> = row.get("run_id");
        let ts_utc: chrono::DateTime<chrono::Utc> = row.get("ts_utc");
        let payload: serde_json::Value = row.get("payload");

        // Build a scannable detail string.  Fall back gracefully if fields absent.
        let strategy_id = payload
            .get("strategy_id")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let symbol = payload
            .get("symbol")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let side = payload.get("side").and_then(|v| v.as_str()).unwrap_or("?");
        let detail = format!("signal.admitted:{strategy_id}:{symbol}:{side}");

        rows.push(EventFeedRow {
            event_id: format!("audit_events:{}", event_id),
            ts_utc: ts_utc.to_rfc3339(),
            kind: "signal_admission".to_string(),
            detail,
            run_id: run_id.map(|id| id.to_string()),
            provenance_ref: format!("audit_events:{}", event_id),
        });
    }

    // --- AUTON-PAPER-02: durable autonomous-session supervisor history ---
    let autonomous_events = match mqk_db::load_recent_autonomous_session_events(db, 50).await {
        Ok(r) => r,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("events/feed autonomous-session query failed: {err}"),
                    fault_class: "events.feed.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response()
        }
    };

    for row in autonomous_events {
        let detail = match row.resume_source.as_deref() {
            Some(src) => format!("{}:{}", row.event_type, src),
            None => row.event_type.clone(),
        };
        rows.push(EventFeedRow {
            event_id: format!("sys_autonomous_session_events:{}", row.id),
            ts_utc: row.ts_utc.to_rfc3339(),
            kind: "autonomous_session".to_string(),
            detail,
            run_id: row.run_id.map(|id| id.to_string()),
            provenance_ref: format!("sys_autonomous_session_events:{}", row.id),
        });
    }

    // Sort newest-first and cap at 50 rows.
    rows.sort_by(|a, b| b.ts_utc.cmp(&a.ts_utc));
    rows.truncate(50);

    (
        StatusCode::OK,
        Json(EventsFeedResponse {
            canonical_route: "/api/v1/events/feed".to_string(),
            truth_state: "active".to_string(),
            backend: "postgres.runs+postgres.audit_events+postgres.sys_autonomous_session_events"
                .to_string(),
            rows,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/incidents (OPS-01)
// ---------------------------------------------------------------------------

/// List durable incidents from `sys_incidents` (OPS-01).
///
/// Returns `truth_state = "active"` with DB-backed rows when a pool is
/// configured.  Returns `truth_state = "no_db"` with empty rows when no pool
/// is present — empty rows must not be interpreted as absence of incidents.
pub(crate) async fn incidents(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(IncidentsResponse {
                canonical_route: "/api/v1/incidents".to_string(),
                truth_state: "no_db".to_string(),
                backend: "unavailable".to_string(),
                rows: vec![],
            }),
        )
            .into_response();
    };

    match mqk_db::list_incidents(db).await {
        Ok(db_rows) => {
            let rows = db_rows
                .into_iter()
                .map(|r| IncidentRow {
                    incident_id: r.incident_id,
                    opened_at_utc: r.opened_at_utc.to_rfc3339(),
                    title: r.title,
                    severity: r.severity,
                    status: r.status,
                    linked_alert_id: r.linked_alert_id,
                    opened_by: r.opened_by,
                })
                .collect();
            (
                StatusCode::OK,
                Json(IncidentsResponse {
                    canonical_route: "/api/v1/incidents".to_string(),
                    truth_state: "active".to_string(),
                    backend: "postgres.sys_incidents".to_string(),
                    rows,
                }),
            )
                .into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(RuntimeErrorResponse {
                error: format!("incidents query failed: {err}"),
                fault_class: "incidents.query_failed".to_string(),
                gate: None,
            }),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// POST /api/v1/incidents (OPS-01)
// ---------------------------------------------------------------------------

/// Declare a new incident (OPS-01).
///
/// Inserts a row into `sys_incidents`.  Requires a DB pool.  `title` must be
/// non-empty.  `severity` must be one of `"info"`, `"warning"`, `"critical"`.
///
/// `incident_id` is derived as a UUIDv5 over the incident namespace +
/// `title:opened_at_utc` so that a re-submit at identical wall time is
/// idempotent (returns the same logical incident).
pub(crate) async fn create_incident(
    State(st): State<Arc<AppState>>,
    Json(body): Json<CreateIncidentRequest>,
) -> Response {
    if body.title.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(RuntimeErrorResponse {
                error: "title must be a non-empty string".to_string(),
                fault_class: "incidents.create.invalid_title".to_string(),
                gate: Some("title_present".to_string()),
            }),
        )
            .into_response();
    }

    let valid_severities = ["info", "warning", "critical"];
    if !valid_severities.contains(&body.severity.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(RuntimeErrorResponse {
                error: format!("severity must be one of: {}", valid_severities.join(", ")),
                fault_class: "incidents.create.invalid_severity".to_string(),
                gate: Some("severity_valid".to_string()),
            }),
        )
            .into_response();
    }

    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(RuntimeErrorResponse {
                error: "incident creation requires a DB pool; daemon has no DB configured"
                    .to_string(),
                fault_class: "incidents.create.no_db".to_string(),
                gate: Some("db_pool".to_string()),
            }),
        )
            .into_response();
    };

    let opened_at = chrono::Utc::now(); // operator-action timestamp — ops metadata only
    let opened_by = body.opened_by.as_deref().unwrap_or("operator");

    // UUIDv5: deterministic ID from title + opened_at so identical wall-time
    // re-submits resolve to the same incident_id.
    const INCIDENT_NS: uuid::Uuid = uuid::uuid!("b7e2a1c4-0d5f-4e8b-9c3a-1f6d2e4a7b0c");
    let id_name = format!("{}:{}", body.title, opened_at.to_rfc3339());
    let incident_id = uuid::Uuid::new_v5(&INCIDENT_NS, id_name.as_bytes()).to_string();

    let args = mqk_db::InsertIncidentArgs {
        incident_id: &incident_id,
        opened_at_utc: opened_at,
        title: &body.title,
        severity: &body.severity,
        linked_alert_id: body.linked_alert_id.as_deref(),
        opened_by,
    };

    match mqk_db::insert_incident(db, args).await {
        Ok(Some(row)) => (
            StatusCode::OK,
            Json(CreateIncidentResponse {
                canonical_route: "/api/v1/incidents".to_string(),
                incident_id: row.incident_id,
                opened_at_utc: row.opened_at_utc.to_rfc3339(),
                title: row.title,
                severity: row.severity,
                status: row.status,
                linked_alert_id: row.linked_alert_id,
                opened_by: row.opened_by,
            }),
        )
            .into_response(),
        // Idempotent no-op: incident_id already existed (same title+ts window).
        Ok(None) => (
            StatusCode::OK,
            Json(RuntimeErrorResponse {
                error: "incident with this id already exists (idempotent no-op)".to_string(),
                fault_class: "incidents.create.already_exists".to_string(),
                gate: None,
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(RuntimeErrorResponse {
                error: format!("incident insert failed: {err}"),
                fault_class: "incidents.create.db_write_failed".to_string(),
                gate: None,
            }),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/alerts/triage (A4)
// ---------------------------------------------------------------------------

/// Alert triage surface — active alerts with DB-backed ack state (OPS-02).
///
/// Alert rows are sourced from the same in-memory fault-signal computation as
/// `/api/v1/alerts/active`.  `status` reflects `sys_alert_acks` when DB is
/// present (`truth_state = "active"`); falls back to `"unacked"` for all rows
/// when no DB pool is available (`truth_state = "no_db"`).
pub(crate) async fn alerts_triage(State(st): State<Arc<AppState>>) -> Response {
    let status_snap = match st.current_status_snapshot().await {
        Ok(s) => s,
        Err(err) => return runtime_error_response(err),
    };
    let reconcile = st.current_reconcile_snapshot().await;

    let risk_blocked = if let Some(db) = st.db.as_ref() {
        mqk_db::load_risk_block_state(db)
            .await
            .ok()
            .flatten()
            .is_some_and(|r| r.blocked)
    } else {
        false
    };

    let fault_signals = build_fault_signals(&status_snap, &reconcile, risk_blocked);

    // WS continuity signals — same as alerts/active
    let ws = st.alpaca_ws_continuity().await;
    let mut extra_signals: Vec<(&str, &str, &str, String)> = Vec::new();
    let ws_cold_summary;
    let ws_gap_summary;
    match &ws {
        AlpacaWsContinuityState::ColdStartUnproven => {
            ws_cold_summary =
                "Alpaca WS continuity unproven (cold start); signal ingestion blocked.".to_string();
            extra_signals.push((
                "paper.ws_continuity.cold_start_unproven",
                "warning",
                "execution",
                ws_cold_summary.clone(),
            ));
        }
        AlpacaWsContinuityState::GapDetected { detail, .. } => {
            ws_gap_summary = format!("Alpaca WS gap detected: {detail}");
            extra_signals.push((
                "paper.ws_continuity.gap_detected",
                "critical",
                "execution",
                ws_gap_summary.clone(),
            ));
        }
        _ => {}
    }

    // Load DB-backed ack records and incident linkages when DB is available.
    //
    // incident_map: alert_id → incident_id (first/most-recent incident that
    // references each alert class slug via `linked_alert_id`).  Built from
    // `sys_incidents` so that triage rows carry `linked_incident_id` when an
    // operator has escalated an alert to a tracked incident (OPS-01).
    let (truth_state, backend, ack_map, incident_map) = if let Some(db) = st.db.as_ref() {
        let acks = mqk_db::load_alert_acks(db).await.unwrap_or_default();
        let ack_map: std::collections::HashMap<String, String> = acks
            .into_iter()
            .map(|r| (r.alert_id, r.acked_at_utc.to_rfc3339()))
            .collect();

        // OPS-01 / ALERTS-OPS-01B: build alert_id → (incident_id, status) map.
        // list_incidents returns rows newest-first; first match wins so the
        // most-recent incident linked to a given alert is surfaced.
        // status ("open" | "resolved") is preserved so the triage surface can
        // reflect whether the linked incident has been resolved.
        let incidents = mqk_db::list_incidents(db).await.unwrap_or_default();
        let mut inc_map: std::collections::HashMap<String, (String, String)> =
            std::collections::HashMap::new();
        for inc in incidents {
            if let Some(alert_id) = inc.linked_alert_id {
                inc_map
                    .entry(alert_id)
                    .or_insert((inc.incident_id, inc.status));
            }
        }

        (
            "active",
            "postgres.sys_alert_acks+postgres.sys_incidents",
            ack_map,
            inc_map,
        )
    } else {
        (
            "no_db",
            "daemon.runtime_state",
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
        )
    };

    let make_row = |alert_id: String, severity: String, domain: String, title: String| {
        let acked_at = ack_map.get(&alert_id).cloned();
        let status = if acked_at.is_some() {
            "acked"
        } else {
            "unacked"
        }
        .to_string();
        let linked = incident_map.get(&alert_id);
        let linked_incident_id = linked.map(|(id, _)| id.clone());
        let linked_incident_status = linked.map(|(_, s)| s.clone());
        AlertTriageAlertRow {
            alert_id,
            severity,
            status,
            title,
            domain,
            linked_incident_id,
            linked_incident_status,
            linked_order_id: None,
            linked_strategy_id: None,
            created_at: acked_at, // None for unacked (no durable creation time)
            assigned_to: None,
        }
    };

    let mut rows: Vec<AlertTriageAlertRow> = fault_signals
        .into_iter()
        .map(|s| {
            let domain = domain_from_class(&s.class).to_string();
            make_row(s.class, s.severity, domain, s.summary)
        })
        .collect();

    for (alert_id, severity, domain, title) in extra_signals {
        rows.push(make_row(
            alert_id.to_string(),
            severity.to_string(),
            domain.to_string(),
            title,
        ));
    }

    let triage_note = if truth_state == "active" {
        "Alert source is real (same as /api/v1/alerts/active). \
         Ack state is DB-backed (sys_alert_acks). \
         Incident linkage is DB-backed (sys_incidents via linked_alert_id). \
         Assign/escalate lifecycle is not implemented."
    } else {
        "Alert source is real (same as /api/v1/alerts/active). \
         Ack state and incident linkage unavailable (no DB pool); all rows carry status=unacked."
    };

    (
        StatusCode::OK,
        Json(AlertTriageResponse {
            canonical_route: "/api/v1/alerts/triage".to_string(),
            truth_state: truth_state.to_string(),
            backend: backend.to_string(),
            triage_note: triage_note.to_string(),
            rows,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /api/v1/alerts/triage/ack
// ---------------------------------------------------------------------------

/// Acknowledge an active alert by class slug.
///
/// Upserts a row into `sys_alert_acks`.  Idempotent: re-acking the same
/// `alert_id` updates the timestamp and `acked_by`.  Returns 503 when no DB
/// pool is configured (ack requires durable storage).
pub(crate) async fn alert_triage_ack(
    State(st): State<Arc<AppState>>,
    Json(body): Json<AlertAckRequest>,
) -> Response {
    if body.alert_id.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(RuntimeErrorResponse {
                error: "alert_id must be a non-empty string".to_string(),
                fault_class: "alerts.triage.ack.invalid_alert_id".to_string(),
                gate: Some("alert_id_present".to_string()),
            }),
        )
            .into_response();
    }

    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(RuntimeErrorResponse {
                error: "alert ack requires a DB pool; daemon has no DB configured".to_string(),
                fault_class: "alerts.triage.ack.no_db".to_string(),
                gate: Some("db_pool".to_string()),
            }),
        )
            .into_response();
    };

    let acked_by = body.acked_by.as_deref().unwrap_or("operator");
    let acked_at = chrono::Utc::now(); // operator action timestamp — ops metadata only

    match mqk_db::upsert_alert_ack(db, &body.alert_id, acked_at, acked_by).await {
        Ok(row) => (
            StatusCode::OK,
            Json(AlertAckResponse {
                canonical_route: "/api/v1/alerts/triage/ack".to_string(),
                alert_id: row.alert_id,
                acked_at_utc: row.acked_at_utc.to_rfc3339(),
                acked_by: row.acked_by,
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(RuntimeErrorResponse {
                error: format!("alert ack write failed: {err}"),
                fault_class: "alerts.triage.ack.db_write_failed".to_string(),
                gate: None,
            }),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// POST /api/v1/incidents/:id/resolve (ALERTS-OPS-01A)
// ---------------------------------------------------------------------------

/// Resolve an open incident (ALERTS-OPS-01A).
///
/// Sets `status = 'resolved'` on the named incident and returns the updated
/// row.  Semantics:
///
/// - `503` — no DB pool; resolve is impossible without persistent storage.
/// - `404` — no row with the given `incident_id` exists.
/// - `200` — row updated (or was already `"resolved"` — idempotent).
pub(crate) async fn resolve_incident(
    State(st): State<Arc<AppState>>,
    Path(incident_id): Path<String>,
) -> Response {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(RuntimeErrorResponse {
                error: "incident resolve requires a DB pool; daemon has no DB configured"
                    .to_string(),
                fault_class: "incidents.resolve.no_db".to_string(),
                gate: Some("db_pool".to_string()),
            }),
        )
            .into_response();
    };

    match mqk_db::resolve_incident(db, &incident_id).await {
        Ok(Some(row)) => (
            StatusCode::OK,
            Json(ResolveIncidentResponse {
                canonical_route: format!("/api/v1/incidents/{}/resolve", incident_id),
                incident_id: row.incident_id,
                opened_at_utc: row.opened_at_utc.to_rfc3339(),
                title: row.title,
                severity: row.severity,
                status: row.status,
                linked_alert_id: row.linked_alert_id,
                opened_by: row.opened_by,
            }),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(RuntimeErrorResponse {
                error: format!("incident not found: {incident_id}"),
                fault_class: "incidents.resolve.not_found".to_string(),
                gate: None,
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(RuntimeErrorResponse {
                error: format!("incident resolve failed: {err}"),
                fault_class: "incidents.resolve.db_write_failed".to_string(),
                gate: None,
            }),
        )
            .into_response(),
    }
}

/// Map a fault-signal class string to a coarse domain label for triage rows.
fn domain_from_class(class: &str) -> &'static str {
    if class.starts_with("reconcile") {
        "reconcile"
    } else if class.starts_with("risk") {
        "risk"
    } else if class.starts_with("paper.ws") || class.starts_with("broker") {
        "execution"
    } else {
        "system"
    }
}
