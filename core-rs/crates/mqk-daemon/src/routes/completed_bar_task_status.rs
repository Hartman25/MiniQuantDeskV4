//! M1-COMPLETED-BAR-PIPELINE-REPAIR-01 (PATCH C):
//! GET /api/v1/autonomous/completed-bar-task-status
//!
//! Read-only projection of the supervised completed-bar task's process-local
//! truth (`AppState::completed_bar_task_truth()`). Diagnosed the 2026-09-14
//! Day-01 M1 defect only after the fact, by reconstructing the task's
//! outcome from durable operation state and monitor logs -- this task truth
//! (`liveness`, `last_tick_utc`, `last_outcome_code`, `restart_count`,
//! `last_error_code`) already existed in process memory the whole time but
//! had no HTTP projection.
//!
//! ## Guarantees
//!
//! - Read-only: calls only `AppState::completed_bar_task_truth()`, which
//!   itself only takes an `RwLock` read guard. No DB call, no broker call,
//!   no provider call.
//! - Nothing on this route can start, stop, or restart the completed-bar
//!   task -- there is no mutating counterpart on this surface.

use std::sync::Arc;

use axum::{extract::State, response::IntoResponse, Json};
use chrono::Utc;

use crate::api_types::CompletedBarTaskStatusResponse;
use crate::state::autonomous_completed_bar_driver::{
    AutonomousCompletedBarDriverMode, AutonomousCompletedBarDriverTaskLiveness,
};
use crate::state::AppState;

fn liveness_label(liveness: &AutonomousCompletedBarDriverTaskLiveness) -> &'static str {
    use AutonomousCompletedBarDriverTaskLiveness as L;
    match liveness {
        L::NotStarted => "not_started",
        L::Running => "running",
        L::Waiting => "waiting",
        L::Blocked => "blocked",
        L::Stopped => "stopped",
        L::Failed => "failed",
    }
}

fn mode_label(mode: &AutonomousCompletedBarDriverMode) -> &'static str {
    use AutonomousCompletedBarDriverMode as M;
    match mode {
        M::PrepareDataOnly => "prepare_data_only",
        M::RunningDispatch => "running_dispatch",
    }
}

pub(crate) async fn completed_bar_task_status(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let truth = st.completed_bar_task_truth().await;

    Json(CompletedBarTaskStatusResponse {
        canonical_route: "/api/v1/autonomous/completed-bar-task-status".to_string(),
        truth_state: "active".to_string(),
        generation: truth.generation,
        liveness: liveness_label(&truth.liveness).to_string(),
        mode: truth.mode.as_ref().map(mode_label).map(str::to_string),
        operation_id: truth.operation_id,
        last_tick_utc: truth.last_tick_utc.map(|t| t.to_rfc3339()),
        last_outcome_code: truth.last_outcome_code.map(str::to_string),
        consecutive_error_count: truth.consecutive_error_count,
        restart_count: truth.restart_count,
        last_error_code: truth.last_error_code.map(str::to_string),
        now_utc: Utc::now().to_rfc3339(),
    })
    .into_response()
}
