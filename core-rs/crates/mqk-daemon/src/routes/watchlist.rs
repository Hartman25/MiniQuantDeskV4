//! PAPER-HANDOFF-READONLY-01: Read-only watchlist artifact status endpoint.
//!
//! `GET /api/v1/watchlist/status` — reports the outcome of loading the
//! `watchlist-v1` artifact configured at `MQK_PAPER_WATCHLIST_PATH`.
//!
//! # Safety invariants
//! - Read-only.  No broker calls, no DB mutations, no orders.
//! - `approved_for_live` is always `false` in the response.
//! - No secrets are exposed.
//! - No AppState mutation.
//! - Malformed JSON in the artifact → `"invalid"` status, not a panic.

use axum::{http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;

use crate::{
    api_types::WatchlistStatusResponse,
    watchlist_intake::{
        evaluate_watchlist_intake_from_env, LoadedWatchlistArtifact, WatchlistIntakeOutcome,
        ENV_PAPER_WATCHLIST_PATH,
    },
};

// ---------------------------------------------------------------------------
// GET /api/v1/watchlist/status
// ---------------------------------------------------------------------------

pub(crate) async fn watchlist_status() -> impl IntoResponse {
    let outcome = evaluate_watchlist_intake_from_env();
    let checked_at_utc = Utc::now().to_rfc3339();

    let configured_path = match std::env::var(ENV_PAPER_WATCHLIST_PATH) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    };

    let response = build_watchlist_status_response(outcome, configured_path, checked_at_utc);
    (StatusCode::OK, Json(response))
}

/// Build the `WatchlistStatusResponse` from the intake outcome.
///
/// Pure helper — extracted for testability.
pub(crate) fn build_watchlist_status_response(
    outcome: WatchlistIntakeOutcome,
    configured_path: Option<String>,
    checked_at_utc: String,
) -> WatchlistStatusResponse {
    let status = outcome.status_label().to_string();
    let approved_for_autonomous_paper = outcome.approved_for_autonomous_paper();
    let failure_reasons = outcome.failure_reasons().to_vec();

    let (symbols, top_symbol, strategy_assignments, max_symbols, max_concurrent) =
        match outcome.artifact() {
            Some(art) => artifact_fields(art),
            None => (vec![], None, serde_json::json!({}), None, None),
        };

    WatchlistStatusResponse {
        configured_path,
        status,
        approved_for_autonomous_paper,
        approved_for_live: false, // hard invariant — never true
        symbols,
        top_symbol,
        strategy_assignments,
        max_symbols_to_trade: max_symbols,
        max_concurrent_positions: max_concurrent,
        failure_reasons,
        checked_at_utc,
    }
}

fn artifact_fields(
    art: &LoadedWatchlistArtifact,
) -> (
    Vec<String>,
    Option<String>,
    serde_json::Value,
    Option<u64>,
    Option<u64>,
) {
    let assignments: serde_json::Map<String, serde_json::Value> = art
        .strategy_assignments
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();

    (
        art.symbols.clone(),
        art.top_symbol.clone(),
        serde_json::Value::Object(assignments),
        Some(art.max_symbols_to_trade),
        Some(art.max_concurrent_positions),
    )
}
