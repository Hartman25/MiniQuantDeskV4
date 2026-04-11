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
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use sqlx::Row;

use crate::api_types::{
    ActiveAlertRow, ActiveAlertsResponse, EventFeedRow, EventsFeedResponse, RuntimeErrorResponse,
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
// GET /api/v1/incidents (A3)
// ---------------------------------------------------------------------------

/// Incident surface — mounted but not wired.
///
/// No durable incident manager exists.  Returns an explicit `"not_wired"`
/// wrapper rather than 404 so the GUI can surface honest unavailable truth
/// instead of treating the missing route as a backend error.
pub(crate) async fn incidents(_: State<Arc<AppState>>) -> impl IntoResponse {
    use crate::api_types::IncidentsResponse;

    (
        StatusCode::OK,
        Json(IncidentsResponse {
            canonical_route: "/api/v1/incidents".to_string(),
            truth_state: "not_wired".to_string(),
            backend: "none".to_string(),
            note: "No incident manager is implemented. \
                   Empty rows must not be interpreted as absence of historical incidents."
                .to_string(),
            rows: vec![],
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/alerts/triage (A4)
// ---------------------------------------------------------------------------

/// Alert triage surface — active alerts with honest `"alerts_no_triage"` state.
///
/// Rows are sourced from the same in-memory fault-signal computation as
/// `/api/v1/alerts/active`.  `status` is always `"unacked"` because no
/// triage workflow (ack/escalate/assign) is implemented or backed.
pub(crate) async fn alerts_triage(State(st): State<Arc<AppState>>) -> Response {
    use crate::api_types::{AlertTriageAlertRow, AlertTriageResponse};

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
    let now_utc = chrono::Utc::now().to_rfc3339();

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

    let mut rows: Vec<AlertTriageAlertRow> = fault_signals
        .into_iter()
        .map(|s| {
            let domain = domain_from_class(&s.class);
            AlertTriageAlertRow {
                alert_id: s.class.clone(),
                severity: s.severity,
                status: "unacked".to_string(),
                title: s.summary,
                domain: domain.to_string(),
                linked_incident_id: None,
                linked_order_id: None,
                linked_strategy_id: None,
                created_at: now_utc.clone(),
                assigned_to: None,
            }
        })
        .collect();

    for (alert_id, severity, domain, title) in extra_signals {
        rows.push(AlertTriageAlertRow {
            alert_id: alert_id.to_string(),
            severity: severity.to_string(),
            status: "unacked".to_string(),
            title,
            domain: domain.to_string(),
            linked_incident_id: None,
            linked_order_id: None,
            linked_strategy_id: None,
            created_at: now_utc.clone(),
            assigned_to: None,
        });
    }

    (
        StatusCode::OK,
        Json(AlertTriageResponse {
            canonical_route: "/api/v1/alerts/triage".to_string(),
            truth_state: "alerts_no_triage".to_string(),
            backend: "daemon.runtime_state".to_string(),
            triage_note: "Alert source is real (same as /api/v1/alerts/active). \
                          Triage lifecycle (ack/escalate/assign) is not implemented; \
                          all rows carry status=unacked."
                .to_string(),
            rows,
        }),
    )
        .into_response()
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
