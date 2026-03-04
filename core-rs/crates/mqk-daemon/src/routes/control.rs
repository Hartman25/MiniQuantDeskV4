use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::{get, post}, Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone)]
pub struct ControlState {
    pub db: sqlx::PgPool,
    /// Identity of caller, set by auth middleware later (scaffold).
    pub node_id: String,
}

#[derive(Debug, Serialize)]
pub struct ControlStatus {
    pub desired_armed: bool,
    pub leader_holder_id: Option<String>,
    pub leader_epoch: Option<i64>,
    pub lease_expires_at_utc: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RestartRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RestartResponse {
    pub restart_id: String,
}

pub fn router(state: ControlState) -> Router {
    Router::new()
        .route("/control/status", get(status))
        .route("/control/disarm", post(disarm))
        .route("/control/arm", post(arm))
        .route("/control/restart", post(restart))
        .with_state(Arc::new(state))
}

async fn status(State(state): State<Arc<ControlState>>) -> impl IntoResponse {
    // Scaffold: return placeholders until wired to DB queries.
    let s = ControlStatus {
        desired_armed: false,
        leader_holder_id: None,
        leader_epoch: None,
        lease_expires_at_utc: None,
    };
    (StatusCode::OK, Json(s))
}

async fn disarm(State(_state): State<Arc<ControlState>>) -> impl IntoResponse {
    // Scaffold: should write runtime_control_state.desired_armed=false
    StatusCode::NO_CONTENT
}

async fn arm(State(_state): State<Arc<ControlState>>) -> impl IntoResponse {
    // Scaffold: should write runtime_control_state.desired_armed=true
    StatusCode::NO_CONTENT
}

async fn restart(State(state): State<Arc<ControlState>>, Json(req): Json<RestartRequest>) -> impl IntoResponse {
    // Scaffold: write runtime_restart_requests row. Supervisor performs actual restart.
    let restart_id = format!("restart-{}-{}", state.node_id, chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
    let _ = req;

    let resp = RestartResponse { restart_id };
    (StatusCode::ACCEPTED, Json(resp))
}
