//! GET /api/v1/strategy/dry-run/status handler.
//!
//! MULTI-STRATEGY-DRY-RUN-STATUS-01: operator-visible surface for the
//! dry-run secondary-strategy diagnostics already computed by
//! `state/dry_run_strategy.rs` and stored by `state/loop_runner.rs`.
//!
//! Pure read of in-memory daemon state. No broker call, no DB query, no
//! outbox access, no submission path — this route cannot place an order any
//! more than the evaluator it reads from can (see `state/dry_run_strategy.rs`
//! module docs for the structural proof that evaluation has no `AppState`,
//! `PgPool`, or broker handle).

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;

use crate::api_types::{DryRunStrategyDiagnosticRow, DryRunStrategyStatusResponse};
use crate::state::{dry_run_strategy_ids_from_env, AppState};

pub(crate) const STRATEGY_DRY_RUN_STATUS_ROUTE: &str = "/api/v1/strategy/dry-run/status";

// ---------------------------------------------------------------------------
// GET /api/v1/strategy/dry-run/status
// ---------------------------------------------------------------------------

pub(crate) async fn strategy_dry_run_status(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(build_dry_run_status_response(&state).await),
    )
}

pub(crate) async fn build_dry_run_status_response(
    state: &AppState,
) -> DryRunStrategyStatusResponse {
    // Preferred behavior (MULTI-STRATEGY-DRY-RUN-STATUS-01): absent/blank
    // MQK_DRY_RUN_STRATEGY_IDS means default-off — diagnostics list is always
    // empty regardless of any snapshot left over in memory (e.g. a test or a
    // prior process configuration). Re-read fresh per request, matching the
    // pattern other env-driven routes in this module already use.
    let configured_dry_run_strategy_ids = dry_run_strategy_ids_from_env();

    if configured_dry_run_strategy_ids.is_empty() {
        return DryRunStrategyStatusResponse {
            canonical_route: STRATEGY_DRY_RUN_STATUS_ROUTE.to_string(),
            backend: "daemon.runtime_state".to_string(),
            truth_state: "not_configured".to_string(),
            configured_dry_run_strategy_ids,
            dry_run_strategy_diagnostics: Vec::new(),
        };
    }

    let diags = state.dry_run_diagnostics().await;
    let evaluated_at_utc = unix_ts_to_rfc3339(state.dry_run_diagnostics_evaluated_at());

    let dry_run_strategy_diagnostics: Vec<DryRunStrategyDiagnosticRow> = diags
        .into_iter()
        .map(|d| DryRunStrategyDiagnosticRow {
            strategy_id: d.strategy_id,
            symbol: d.symbol,
            timeframe_secs: d.timeframe_secs,
            current_qty: d.current_qty,
            target_qty: d.target_qty,
            delta_qty: d.delta_qty,
            decision: d.decision.to_string(),
            reason: d.reason,
            would_classify_as: d.would_classify_as.to_string(),
            would_b5_block: d.would_b5_block,
            would_policy_block: d.would_policy_block,
            policy_reason_code: d.policy_reason_code.map(|s| s.to_string()),
            submitted: d.submitted,
            evaluated_at_utc: evaluated_at_utc.clone(),
        })
        .collect();

    DryRunStrategyStatusResponse {
        canonical_route: STRATEGY_DRY_RUN_STATUS_ROUTE.to_string(),
        backend: "daemon.runtime_state".to_string(),
        truth_state: "active".to_string(),
        configured_dry_run_strategy_ids,
        dry_run_strategy_diagnostics,
    }
}

/// `0` (the never-evaluated sentinel) maps to `""` — empty rather than a
/// fabricated epoch timestamp. Only reachable when dry-run is configured but
/// no execution-loop tick has evaluated it yet this process lifetime, in
/// which case `dry_run_strategy_diagnostics` is also empty and this value is
/// never attached to a row.
fn unix_ts_to_rfc3339(ts: i64) -> String {
    if ts <= 0 {
        return String::new();
    }
    chrono::DateTime::<Utc>::from_timestamp(ts, 0) // allow: telemetry surface
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}
