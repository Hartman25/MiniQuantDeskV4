//! MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01: HTTP surface for the
//! required-universe market-data freshness controller.
//!
//! Thin Axum plumbing only — all control/refresh-plan logic lives in
//! [`crate::state::required_market_data_autofresh`]. Routes:
//!   GET  /api/v1/market-data/required-universe/plan     — read-only dry-run/checkonly plan (public)
//!   GET  /api/v1/market-data/required-universe/status    — read-only current status (public)
//!   POST /api/v1/market-data/required-universe/start     — start the scheduler (operator, requires auth)
//!   POST /api/v1/market-data/required-universe/stop      — stop the scheduler (operator, requires auth)
//!
//! # Safety invariants
//! - No order/execution-runtime/arm/halt/reconcile route is called from
//!   here or from the state module this delegates to (§43).
//! - `plan` is always read-only: zero provider API calls, zero DB writes
//!   (§46). It still reads `md_bars` read-only to report current freshness.
//! - `start`/`stop` control only this market-data scheduler — never the
//!   execution runtime.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;
use serde::Deserialize;

use crate::state::market_calendar::active_calendar_provider_from_env;
use crate::state::required_market_data_autofresh::{
    run_required_universe_cycle, start_required_universe_scheduler,
    stop_required_universe_scheduler,
};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/required-universe/plan — §46 checkonly/dry-run
// ---------------------------------------------------------------------------

/// Read-only required-universe plan/dry-run: shows required requirements,
/// resolved provider groups, calendar/session horizon, and current
/// freshness — with `provider_api_calls_made_this_cycle=0` and no DB write
/// (§46). No auth required: this route performs no mutation.
pub(crate) async fn required_universe_plan(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let calendar_provider = active_calendar_provider_from_env();
    let now_utc = Utc::now();
    let result = run_required_universe_cycle(
        st.db.as_ref(),
        &st.instrument_registry_path,
        &st.provider_registry_path,
        st.latest_bar_provider_client.as_ref(),
        calendar_provider.as_ref(),
        now_utc,
        /* dry_run = */ true,
    )
    .await;
    Json(result.report)
}

// ---------------------------------------------------------------------------
// GET /api/v1/market-data/required-universe/status
// ---------------------------------------------------------------------------

/// Read-only current required-universe scheduler status (§26/§27/§28).
/// Reports the last recorded cycle plus scheduler timing truthfully —
/// `truth_state="no_cycle_yet"` when the scheduler has never run in this
/// process lifetime (never a fabricated empty-but-ready default).
pub(crate) async fn required_universe_status(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let scheduler = st.required_universe_scheduler.lock().await;
    let truth_state = if scheduler.last_report.is_some() {
        "active"
    } else {
        "no_cycle_yet"
    };
    Json(serde_json::json!({
        "canonical_route": "/api/v1/market-data/required-universe/status",
        "truth_state": truth_state,
        "limitation": "process_local_only_not_persisted",
        "running": scheduler.running,
        "dry_run": scheduler.dry_run,
        "cycle_count": scheduler.cycle_count,
        "provider_api_calls_made_total": scheduler.provider_api_calls_made,
        "last_error": scheduler.last_error,
        "started_at_utc": scheduler.started_at_ts.and_then(ts_to_rfc3339),
        "stopped_at_utc": scheduler.stopped_at_ts.and_then(ts_to_rfc3339),
        "last_cycle_utc": scheduler.last_cycle_utc.and_then(ts_to_rfc3339),
        "next_cycle_utc": scheduler.next_cycle_utc.and_then(ts_to_rfc3339),
        "report": scheduler.last_report,
    }))
}

fn ts_to_rfc3339(ts: i64) -> Option<String> {
    chrono::DateTime::<Utc>::from_timestamp(ts, 0).map(|dt| dt.to_rfc3339())
}

// ---------------------------------------------------------------------------
// POST /api/v1/market-data/required-universe/start
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct RequiredUniverseStartRequest {
    #[serde(default)]
    pub dry_run: bool,
    /// Test/operator-injected reference time. `None` in production reads
    /// the wall clock.
    #[serde(default)]
    pub now_utc: Option<String>,
}

pub(crate) async fn required_universe_scheduler_start(
    State(st): State<Arc<AppState>>,
    Json(req): Json<RequiredUniverseStartRequest>,
) -> axum::response::Response {
    let now_utc = match req.now_utc.as_deref() {
        Some(value) => match chrono::DateTime::parse_from_rfc3339(value) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(err) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "canonical_route": "/api/v1/market-data/required-universe/start",
                        "truth_state": "refused",
                        "error": format!("now_utc must be RFC3339 UTC: {err}"),
                    })),
                )
                    .into_response();
            }
        },
        None => Utc::now(),
    };

    match start_required_universe_scheduler(&st, req.dry_run, now_utc).await {
        Ok(report) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "canonical_route": "/api/v1/market-data/required-universe/start",
                "truth_state": "started",
                "report": report,
            })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "canonical_route": "/api/v1/market-data/required-universe/start",
                "truth_state": "already_running",
                "error": error,
            })),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// POST /api/v1/market-data/required-universe/stop
// ---------------------------------------------------------------------------

pub(crate) async fn required_universe_scheduler_stop(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let now_utc = Utc::now();
    let was_running = stop_required_universe_scheduler(&st, now_utc).await;
    Json(serde_json::json!({
        "canonical_route": "/api/v1/market-data/required-universe/stop",
        "truth_state": if was_running { "stopped" } else { "not_running" },
        "running": false,
    }))
}
