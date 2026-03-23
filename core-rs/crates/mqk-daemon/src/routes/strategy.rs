//! Strategy route handlers.
//!
//! Contains: strategy_summary, strategy_suppressions.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};

use crate::api_types::{
    StrategySummaryResponse, StrategySummaryRow, StrategySuppressionRow,
    StrategySuppressionsResponse,
};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// GET /api/v1/strategy/summary
// ---------------------------------------------------------------------------

pub(crate) async fn strategy_summary(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let fleet = state.strategy_fleet_snapshot().await;
    match fleet {
        None => (
            StatusCode::OK,
            Json(StrategySummaryResponse {
                canonical_route: "/api/v1/strategy/summary".to_string(),
                backend: "daemon.strategy_fleet".to_string(),
                truth_state: "not_wired".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response(),
        Some(entries) => {
            let armed = !state.integrity.read().await.is_execution_blocked();
            let rows = entries
                .into_iter()
                .map(|e| StrategySummaryRow {
                    strategy_id: e.strategy_id,
                    enabled: true,
                    armed,
                    health_status: None,
                    universe_size: None,
                    pending_intents: None,
                    open_positions: None,
                    today_pnl: None,
                    drawdown_pct: None,
                    regime: None,
                    throttle_state: None,
                    last_decision_time: None,
                })
                .collect();
            (
                StatusCode::OK,
                Json(StrategySummaryResponse {
                    canonical_route: "/api/v1/strategy/summary".to_string(),
                    backend: "daemon.strategy_fleet".to_string(),
                    truth_state: "active".to_string(),
                    rows,
                }),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/strategy/suppressions
// ---------------------------------------------------------------------------

pub(crate) async fn strategy_suppressions(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(StrategySuppressionsResponse {
                canonical_route: "/api/v1/strategy/suppressions".to_string(),
                backend: "postgres.sys_strategy_suppressions".to_string(),
                truth_state: "no_db".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    let records = match mqk_db::fetch_strategy_suppressions(db).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!("fetch_strategy_suppressions failed: {err}");
            return (
                StatusCode::OK,
                Json(StrategySuppressionsResponse {
                    canonical_route: "/api/v1/strategy/suppressions".to_string(),
                    backend: "postgres.sys_strategy_suppressions".to_string(),
                    truth_state: "no_db".to_string(),
                    rows: Vec::new(),
                }),
            )
                .into_response();
        }
    };

    let rows = records
        .into_iter()
        .map(|r| StrategySuppressionRow {
            suppression_id: r.suppression_id.to_string(),
            strategy_id: r.strategy_id,
            state: r.state,
            trigger_domain: r.trigger_domain,
            trigger_reason: r.trigger_reason,
            started_at: r.started_at_utc.to_rfc3339(),
            cleared_at: r.cleared_at_utc.map(|t| t.to_rfc3339()),
            note: r.note,
        })
        .collect();

    (
        StatusCode::OK,
        Json(StrategySuppressionsResponse {
            canonical_route: "/api/v1/strategy/suppressions".to_string(),
            backend: "postgres.sys_strategy_suppressions".to_string(),
            truth_state: "active".to_string(),
            rows,
        }),
    )
        .into_response()
}
