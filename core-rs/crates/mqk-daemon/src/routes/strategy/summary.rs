//! GET /api/v1/strategy/summary handler and its private helpers.
//!
//! Extracted from strategy.rs (MT-07E).

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};

use crate::api_types::{StrategySummaryResponse, StrategySummaryRow};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// B2B helpers (private to this module)
// ---------------------------------------------------------------------------

/// B2B: Derive the runtime execution mode label from the fleet snapshot.
///
/// This is informational truth about the configured fleet, not the runtime
/// execution model.  Single-strategy execution remains the only wired path;
/// `"fleet"` is surfaced honestly for multi-entry configs even though the
/// runtime does not yet multi-schedule.
fn execution_mode_label(fleet_size: Option<usize>) -> &'static str {
    match fleet_size {
        None | Some(0) => "fleet_not_configured",
        Some(1) => "single_strategy",
        _ => "fleet",
    }
}

/// B2B: Compute the admission state for a registry row given fleet membership.
///
/// Authority model:
/// - Fleet (MQK_STRATEGY_IDS) is the *requested* set.
/// - DB registry is the *final authority* on whether activation is allowed.
///   Both must agree for a strategy to be "runnable".
fn admission_state_for_registry_row(
    fleet_ids: &Option<std::collections::HashSet<String>>,
    strategy_id: &str,
    enabled: bool,
) -> String {
    match fleet_ids {
        None => "no_fleet_configured".to_string(),
        Some(ids) => {
            if !ids.contains(strategy_id) {
                "not_configured".to_string()
            } else if enabled {
                "runnable".to_string()
            } else {
                "blocked_disabled".to_string()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/strategy/summary
// ---------------------------------------------------------------------------

pub(crate) async fn strategy_summary(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // CC-01B: Active-fleet truth is sourced from postgres.sys_strategy_registry.
    // B2B: Fleet admission cross-reference is derived from MQK_STRATEGY_IDS × registry.
    // Fail closed: if the DB is unavailable we cannot distinguish "no strategies"
    // from "registry unavailable", so we return "no_db" with honest fleet metadata.

    // Fleet snapshot is available without DB (env-var derived at boot).
    let fleet_snapshot = state.strategy_fleet_snapshot().await;
    let configured_fleet_size = fleet_snapshot.as_ref().map(|f| f.len());
    let runtime_execution_mode = execution_mode_label(configured_fleet_size).to_string();

    let Some(db) = state.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(StrategySummaryResponse {
                canonical_route: "/api/v1/strategy/summary".to_string(),
                backend: "postgres.sys_strategy_registry".to_string(),
                truth_state: "no_db".to_string(),
                runtime_execution_mode,
                configured_fleet_size,
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    let registry = match mqk_db::fetch_strategy_registry(db).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!("fetch_strategy_registry failed: {err}");
            return (
                StatusCode::OK,
                Json(StrategySummaryResponse {
                    canonical_route: "/api/v1/strategy/summary".to_string(),
                    backend: "postgres.sys_strategy_registry".to_string(),
                    truth_state: "no_db".to_string(),
                    runtime_execution_mode,
                    configured_fleet_size,
                    rows: Vec::new(),
                }),
            )
                .into_response();
        }
    };

    // Arm state is global (not per-strategy); read once for all rows.
    let armed = !state.integrity.read().await.is_execution_blocked();

    // B2B: Fleet membership set for admission cross-reference.
    let fleet_id_set: Option<std::collections::HashSet<String>> = fleet_snapshot
        .as_ref()
        .map(|entries| entries.iter().map(|e| e.strategy_id.clone()).collect());

    // B3: In-memory telemetry seam.
    //
    // throttle_state and last_decision_time are wired only for the single active
    // fleet strategy.  In the single-strategy runtime, the global per-run counters
    // equal the per-strategy values — surfacing them as per-strategy is honest.
    // For multi-strategy configs (fleet > 1) or no fleet, these fields are null.
    let single_fleet_id: Option<String> = match &fleet_snapshot {
        Some(f) if f.len() == 1 => Some(f[0].strategy_id.clone()),
        _ => None,
    };
    let throttle_open = !state.day_signal_limit_exceeded();
    let last_ts = state.last_bar_input_ts();
    let last_decision_time: Option<String> = if last_ts > 0 {
        chrono::DateTime::<chrono::Utc>::from_timestamp(last_ts, 0) // allow: telemetry surface
            .map(|dt| dt.to_rfc3339())
    } else {
        None
    };

    // Registry ID set used to detect fleet entries that have no registry row.
    let registry_id_set: std::collections::HashSet<&str> =
        registry.iter().map(|r| r.strategy_id.as_str()).collect();

    let mut rows: Vec<StrategySummaryRow> = Vec::new();

    // CC-01C: Registry-sourced rows.
    // All non-null fields sourced directly from sys_strategy_registry.
    // `admission_state` cross-references the fleet (B2B).
    // `throttle_state` and `last_decision_time` wired for single-strategy target (B3).
    for r in &registry {
        let admission_state =
            admission_state_for_registry_row(&fleet_id_set, &r.strategy_id, r.enabled);
        let is_single_target = single_fleet_id.as_deref() == Some(r.strategy_id.as_str());

        let (throttle_state, row_last_decision) = if is_single_target {
            (
                Some(
                    if throttle_open {
                        "open"
                    } else {
                        "day_limit_reached"
                    }
                    .to_string(),
                ),
                last_decision_time.clone(),
            )
        } else {
            (None, None)
        };

        rows.push(StrategySummaryRow {
            strategy_id: r.strategy_id.clone(),
            display_name: r.display_name.clone(),
            enabled: r.enabled,
            kind: r.kind.clone(),
            registered_at: r.registered_at_utc.to_rfc3339(),
            note: r.note.clone(),
            armed,
            admission_state,
            health_status: None,
            universe_size: None,
            pending_intents: None,
            open_positions: None,
            today_pnl: None,
            drawdown_pct: None,
            regime: None,
            throttle_state,
            last_decision_time: row_last_decision,
        });
    }

    // B2B: Synthetic rows for fleet entries that have no registry record.
    //
    // These represent a control-truth disagreement: the operator configured a
    // strategy for this daemon (MQK_STRATEGY_IDS) but never registered it in
    // sys_strategy_registry.  Silently dropping them would hide the misconfiguration.
    // They appear with admission_state="blocked_not_registered" so operators and
    // callers can surface the disagreement explicitly.
    if let Some(ref ids) = fleet_id_set {
        let mut missing: Vec<String> = ids
            .iter()
            .filter(|id| !registry_id_set.contains(id.as_str()))
            .cloned()
            .collect();
        missing.sort(); // deterministic order for synthetic rows
        for id in missing {
            rows.push(StrategySummaryRow {
                strategy_id: id,
                display_name: String::new(),
                enabled: false,
                kind: String::new(),
                registered_at: String::new(),
                note: String::new(),
                armed,
                admission_state: "blocked_not_registered".to_string(),
                health_status: None,
                universe_size: None,
                pending_intents: None,
                open_positions: None,
                today_pnl: None,
                drawdown_pct: None,
                regime: None,
                throttle_state: None,
                last_decision_time: None,
            });
        }
    }

    // Sort all rows by strategy_id for deterministic response order.
    rows.sort_by(|a, b| a.strategy_id.cmp(&b.strategy_id));

    (
        StatusCode::OK,
        Json(StrategySummaryResponse {
            canonical_route: "/api/v1/strategy/summary".to_string(),
            backend: "postgres.sys_strategy_registry".to_string(),
            truth_state: "registry".to_string(),
            runtime_execution_mode,
            configured_fleet_size,
            rows,
        }),
    )
        .into_response()
}
