//! Config-surface route handlers (MT-07D extraction from system.rs).
//!
//! Contains: system_config_fingerprint, system_config_diffs,
//! authoritative_config_diff_rows.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use sqlx::Row;

use crate::api_types::{
    ConfigDiffRow, ConfigDiffsResponse, ConfigFingerprintResponse, RuntimeErrorResponse,
};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// GET /api/v1/system/config-fingerprint
// ---------------------------------------------------------------------------

pub(crate) async fn system_config_fingerprint(
    State(st): State<Arc<AppState>>,
) -> impl IntoResponse {
    let (latest_run, truth_state) = if let Some(db) = st.db.as_ref() {
        let run = mqk_db::fetch_latest_run_for_engine(
            db,
            super::DAEMON_ENGINE_ID,
            st.deployment_mode().as_db_mode(),
        )
        .await
        .ok()
        .flatten();
        let ts = if run.is_some() { "active" } else { "no_run" }.to_string();
        (run, ts)
    } else {
        (None, "no_db".to_string())
    };

    (
        StatusCode::OK,
        Json(ConfigFingerprintResponse {
            truth_state,
            config_hash: latest_run
                .as_ref()
                .map(|run| run.config_hash.clone())
                .unwrap_or_else(|| st.run_config_hash().to_string()),
            adapter_id: st.adapter_id().to_string(),
            risk_policy_version: None,
            strategy_bundle_version: None,
            build_version: st.build.version.to_string(),
            environment_profile: st.deployment_mode().as_api_label().to_string(),
            runtime_generation_id: latest_run.as_ref().map(|run| run.run_id.to_string()),
            last_restart_at: latest_run
                .as_ref()
                .map(|run| run.started_at_utc.to_rfc3339()),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/system/config-diffs
// ---------------------------------------------------------------------------

pub(crate) async fn system_config_diffs(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(db) = st.db.as_ref() else {
        return (
            StatusCode::OK,
            Json(ConfigDiffsResponse {
                canonical_route: "/api/v1/system/config-diffs".to_string(),
                truth_state: "not_wired".to_string(),
                backend: "not_wired".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    let latest_run = match sqlx::query(
        r#"
        select
          run_id,
          engine_id,
          mode,
          started_at_utc,
          git_hash,
          config_hash,
          config_json,
          host_fingerprint,
          status,
          armed_at_utc,
          running_at_utc,
          stopped_at_utc,
          halted_at_utc,
          last_heartbeat_utc
        from runs
        where engine_id = $1
        order by started_at_utc desc, run_id desc
        limit 1
        "#,
    )
    .bind(super::DAEMON_ENGINE_ID)
    .fetch_optional(db)
    .await
    {
        Ok(Some(row)) => {
            let status = match mqk_db::RunStatus::parse(&row.get::<String, _>("status")) {
                Ok(status) => status,
                Err(err) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(RuntimeErrorResponse {
                            error: format!("system/config-diffs status parse failed: {err}"),
                            fault_class: "system.config_diffs.status_parse_failed".to_string(),
                            gate: None,
                        }),
                    )
                        .into_response();
                }
            };

            Some(mqk_db::RunRow {
                run_id: row.get("run_id"),
                engine_id: row.get("engine_id"),
                mode: row.get("mode"),
                started_at_utc: row.get("started_at_utc"),
                git_hash: row.get("git_hash"),
                config_hash: row.get("config_hash"),
                config_json: row.get("config_json"),
                host_fingerprint: row.get("host_fingerprint"),
                status,
                armed_at_utc: row.get("armed_at_utc"),
                running_at_utc: row.get("running_at_utc"),
                stopped_at_utc: row.get("stopped_at_utc"),
                halted_at_utc: row.get("halted_at_utc"),
                last_heartbeat_utc: row.get("last_heartbeat_utc"),
            })
        }
        Ok(None) => None,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(RuntimeErrorResponse {
                    error: format!("system/config-diffs query failed: {err}"),
                    fault_class: "system.config_diffs.query_failed".to_string(),
                    gate: None,
                }),
            )
                .into_response();
        }
    };

    let Some(latest_run) = latest_run else {
        return (
            StatusCode::OK,
            Json(ConfigDiffsResponse {
                canonical_route: "/api/v1/system/config-diffs".to_string(),
                truth_state: "not_wired".to_string(),
                backend: "not_wired".to_string(),
                rows: Vec::new(),
            }),
        )
            .into_response();
    };

    let rows = authoritative_config_diff_rows(&st, &latest_run);

    (
        StatusCode::OK,
        Json(ConfigDiffsResponse {
            canonical_route: "/api/v1/system/config-diffs".to_string(),
            truth_state: "active".to_string(),
            backend: "postgres.runs+daemon.runtime_selection".to_string(),
            rows,
        }),
    )
        .into_response()
}

fn authoritative_config_diff_rows(
    st: &AppState,
    latest_run: &mqk_db::RunRow,
) -> Vec<ConfigDiffRow> {
    let mut rows = Vec::new();
    let changed_at = latest_run.started_at_utc.to_rfc3339();

    if latest_run.config_hash != st.run_config_hash() {
        rows.push(ConfigDiffRow {
            diff_id: format!("{}:config_hash", latest_run.run_id),
            changed_at: changed_at.clone(),
            changed_domain: "config".to_string(),
            before_version: latest_run.config_hash.clone(),
            after_version: st.run_config_hash().to_string(),
            summary: format!(
                "current daemon config_hash differs from latest durable run {}",
                latest_run.run_id
            ),
        });
    }

    if latest_run.mode != st.deployment_mode().as_db_mode() {
        rows.push(ConfigDiffRow {
            diff_id: format!("{}:deployment_mode", latest_run.run_id),
            changed_at: changed_at.clone(),
            changed_domain: "runtime".to_string(),
            before_version: latest_run.mode.clone(),
            after_version: st.deployment_mode().as_db_mode().to_string(),
            summary: format!(
                "current daemon deployment mode differs from latest durable run {}",
                latest_run.run_id
            ),
        });
    }

    if let Some(prior_adapter) = latest_run
        .config_json
        .get("adapter")
        .and_then(|value| value.as_str())
    {
        if prior_adapter != st.adapter_id() {
            rows.push(ConfigDiffRow {
                diff_id: format!("{}:adapter", latest_run.run_id),
                changed_at,
                changed_domain: "runtime".to_string(),
                before_version: prior_adapter.to_string(),
                after_version: st.adapter_id().to_string(),
                summary: format!(
                    "current daemon adapter differs from latest durable run {}",
                    latest_run.run_id
                ),
            });
        }
    }

    rows
}
