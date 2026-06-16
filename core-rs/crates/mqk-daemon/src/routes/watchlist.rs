//! PAPER-HANDOFF-READONLY-01 / PAPER-HANDOFF-ENFORCE-DESIGN-ONLY-01:
//! Read-only watchlist artifact status and dry-run admission check endpoints.
//!
//! `GET /api/v1/watchlist/status` — reports the outcome of loading the
//! `watchlist-v1` artifact configured at `MQK_PAPER_WATCHLIST_PATH`.
//!
//! `GET /api/v1/watchlist/admission-check?symbol=<sym>&strategy_id=<id>`
//! — dry-run admission check for a (symbol, strategy_id) pair.
//! Returns whether the pair would be admitted under the current watchlist.
//! Always includes `note: "dry_run_only_not_enforced"`.  Not wired into
//! the live signal path (`POST /api/v1/strategy/signal`).
//!
//! # Safety invariants (both endpoints)
//! - Read-only.  No broker calls, no DB mutations, no orders.
//! - `approved_for_live` is always `false` in all responses.
//! - No secrets are exposed.
//! - No AppState mutation.
//! - Malformed JSON in the artifact → `"invalid"` status, not a panic.
//! - Watchlist admission is NOT enforced in the live signal path in this patch.
//!   PAPER-HANDOFF-ENFORCE-01 will wire it into `strategy_signal()`.

use axum::{extract::Query, http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;

use crate::{
    api_types::{WatchlistAdmissionCheckResponse, WatchlistStatusResponse},
    watchlist_intake::{
        evaluate_watchlist_intake_from_env, evaluate_watchlist_signal_admission,
        LoadedWatchlistArtifact, WatchlistIntakeOutcome, ENV_PAPER_WATCHLIST_PATH,
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

    let (schema_version, symbols, top_symbol, strategy_assignments, max_symbols, max_concurrent) =
        match outcome.artifact() {
            Some(art) => artifact_fields(art),
            None => (None, vec![], None, serde_json::json!({}), None, None),
        };

    WatchlistStatusResponse {
        configured_path,
        status,
        schema_version,
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

/// (schema_version, symbols, top_symbol, strategy_assignments, max_symbols_to_trade, max_concurrent_positions)
type ArtifactFields = (
    Option<String>,
    Vec<String>,
    Option<String>,
    serde_json::Value,
    Option<u64>,
    Option<u64>,
);

fn artifact_fields(art: &LoadedWatchlistArtifact) -> ArtifactFields {
    let assignments: serde_json::Map<String, serde_json::Value> = art
        .strategy_assignments
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();

    (
        Some(art.schema_version.clone()),
        art.symbols.clone(),
        art.top_symbol.clone(),
        serde_json::Value::Object(assignments),
        Some(art.max_symbols_to_trade),
        Some(art.max_concurrent_positions),
    )
}

// ---------------------------------------------------------------------------
// GET /api/v1/watchlist/admission-check — PAPER-HANDOFF-ENFORCE-DESIGN-ONLY-01
// ---------------------------------------------------------------------------

/// Query parameters for the dry-run admission check.
#[derive(serde::Deserialize)]
pub(crate) struct AdmissionCheckParams {
    pub symbol: Option<String>,
    pub strategy_id: Option<String>,
}

/// Dry-run watchlist signal admission check.
///
/// Pure read-only evaluation of whether a (symbol, strategy_id) pair would be
/// admitted under the current watchlist.
///
/// # Safety
/// - Read-only.  No broker calls, no DB mutations, no orders.
/// - No outbox writes, no inbox writes, no AppState mutation.
/// - `approved_for_live` is always `false` in the response.
/// - `note` is always `"dry_run_only_not_enforced"`.
/// - This endpoint does NOT enforce admission in `POST /api/v1/strategy/signal`.
///   That wiring is deferred to PAPER-HANDOFF-ENFORCE-01.
pub(crate) async fn watchlist_admission_check(
    Query(params): Query<AdmissionCheckParams>,
) -> impl IntoResponse {
    let symbol = params.symbol.as_deref().unwrap_or("").trim().to_string();
    let strategy_id = params
        .strategy_id
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let checked_at_utc = Utc::now().to_rfc3339();

    let configured_path = match std::env::var(ENV_PAPER_WATCHLIST_PATH) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    };

    let outcome = evaluate_watchlist_intake_from_env();
    let admission = evaluate_watchlist_signal_admission(&outcome, &symbol, &strategy_id);

    let response = build_watchlist_admission_check_response(
        outcome,
        admission.allowed,
        admission.reason.as_str().to_string(),
        symbol,
        strategy_id,
        configured_path,
        checked_at_utc,
    );
    (StatusCode::OK, Json(response))
}

/// Build the `WatchlistAdmissionCheckResponse` from the intake outcome and admission result.
///
/// Pure helper — extracted for testability.
pub(crate) fn build_watchlist_admission_check_response(
    outcome: WatchlistIntakeOutcome,
    allowed: bool,
    reason: String,
    symbol: String,
    strategy_id: String,
    _configured_path: Option<String>,
    checked_at_utc: String,
) -> WatchlistAdmissionCheckResponse {
    let status = outcome.status_label().to_string();
    let approved_for_autonomous_paper = outcome.approved_for_autonomous_paper();

    let (top_symbol, strategy_assignments) = match outcome.artifact() {
        Some(art) => {
            let assignments: serde_json::Map<String, serde_json::Value> = art
                .strategy_assignments
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect();
            (
                art.top_symbol.clone(),
                serde_json::Value::Object(assignments),
            )
        }
        None => (None, serde_json::json!({})),
    };

    WatchlistAdmissionCheckResponse {
        allowed,
        reason,
        status,
        approved_for_autonomous_paper,
        approved_for_live: false, // hard invariant — never true
        symbol,
        strategy_id,
        top_symbol,
        strategy_assignments,
        note: "dry_run_only_not_enforced".to_string(),
        checked_at_utc,
    }
}
