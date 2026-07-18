//! STRATEGY-SCANNER-JOBS-GUI-01B/01C: Daemon-managed strategy scanner job API
//! and read-only artifact readback.
//!
//! Routes:
//!   POST /api/v1/strategy-scans/jobs          — submit a bounded local-data scan (operator)
//!   GET  /api/v1/strategy-scans/jobs          — list all in-memory jobs (public)
//!   GET  /api/v1/strategy-scans/jobs/:job_id  — job status + summary (public)
//!   GET  /api/v1/strategy-scans/artifact      — read-only artifact readback (public)
//!
//! Safety invariants:
//! - No broker adapter is called. No Alpaca. No provider/network call.
//! - No live/paper order submitted. No OMS/outbox/inbox table written.
//! - Does not require arm_state. Does not start/stop the trading runtime.
//! - Jobs are in-memory only (process-lifetime); no DB persistence.
//! - Runs the identical local-data-only scanner core as `mqk backtest
//!   scan-strategies` (`mqk_backtest::execute_strategy_scan` /
//!   `write_scan_artifacts`) — the daemon does not shell out to the CLI.
//! - Scanner ranking is research evidence only. Every completed-job and
//!   artifact response carries the fixed research-only warning set; no
//!   route here ever claims trading approval, and none writes to
//!   admission/promotion/order/risk state.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    api_types::{
        StrategyScanArtifactResponse, StrategyScanJobAcceptedResponse,
        StrategyScanJobStatusResponse, StrategyScanJobSubmitRequest, StrategyScanJobSummary,
        StrategyScanJobsListResponse, StrategyScanReviewArtifactResponse,
    },
    state::AppState,
    strategy_scan_jobs::{StrategyScanJobRecord, StrategyScanJobStatus},
};

const MIN_TOP: usize = 1;
const MAX_TOP: usize = 100;
const MAX_LIMIT_SYMBOLS: usize = 200;
const MAX_ARTIFACT_CANDIDATE_ROWS: usize = 200;
const MAX_REVIEW_ARTIFACT_DECISION_ROWS: usize = 200;

const RESEARCH_ONLY_WARNING: &str = "Scanner ranking is research evidence only.";
const NOT_TRADING_APPROVAL_WARNING: &str =
    "Scanner output is not autonomous trading approval.";
const NEGATIVE_RETURN_WARNING: &str =
    "Candidates can rank well while still having negative absolute returns.";

fn fixed_research_warnings() -> Vec<String> {
    vec![
        RESEARCH_ONLY_WARNING.to_string(),
        NOT_TRADING_APPROVAL_WARNING.to_string(),
        NEGATIVE_RETURN_WARNING.to_string(),
    ]
}

// ---------------------------------------------------------------------------
// POST /api/v1/strategy-scans/jobs
// ---------------------------------------------------------------------------

pub(crate) async fn strategy_scan_job_submit(
    State(st): State<Arc<AppState>>,
    Json(req): Json<StrategyScanJobSubmitRequest>,
) -> Response {
    let timeframe = req.timeframe.trim().to_string();
    let strategy = req.strategy.trim().to_string();

    if timeframe.is_empty() {
        return refused("timeframe is required and must not be empty");
    }
    if strategy.is_empty() {
        return refused("strategy is required and must not be empty");
    }
    if req.top < MIN_TOP || req.top > MAX_TOP {
        return refused(&format!("top must be between {MIN_TOP} and {MAX_TOP}"));
    }
    if let Some(limit) = req.limit_symbols {
        if !(1..=MAX_LIMIT_SYMBOLS).contains(&limit) {
            return refused(&format!(
                "limit_symbols must be between 1 and {MAX_LIMIT_SYMBOLS}"
            ));
        }
    }

    let registry_path = req
        .registry_path
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "config/instruments/equities.json".to_string());
    let bars_root = req
        .bars_root
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "exports/md_backup".to_string());
    let out_dir = req
        .out_dir
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "exports/strategy_scans".to_string());

    let effective_request = StrategyScanJobSubmitRequest {
        registry_path: Some(registry_path.clone()),
        bars_root: Some(bars_root.clone()),
        timeframe: timeframe.clone(),
        strategy: strategy.clone(),
        top: req.top,
        limit_symbols: req.limit_symbols,
        out_dir: Some(out_dir.clone()),
    };

    let job_id = Uuid::new_v4(); // allow: process-local transient job identifier
    let created_at = Utc::now(); // allow: operational job creation timestamp

    let record = StrategyScanJobRecord {
        job_id,
        request: effective_request,
        status: StrategyScanJobStatus::Queued,
        created_at_utc: created_at,
        started_at_utc: None,
        completed_at_utc: None,
        artifact_dir: None,
        ranked_count: None,
        skipped_count: None,
        summary: None,
        warnings: Vec::new(),
        blockers: Vec::new(),
        error: None,
    };

    {
        let mut store = st
            .strategy_scan_jobs
            .lock()
            .expect("strategy_scan_jobs lock poisoned");
        store.insert(job_id, record);
    }

    let jobs = Arc::clone(&st.strategy_scan_jobs);
    let top = req.top;
    let limit_symbols = req.limit_symbols;

    // --- Spawn background task: local-data-only, no provider/broker/DB IO ---
    tokio::spawn(async move {
        {
            let mut store = jobs.lock().expect("strategy_scan_jobs lock poisoned");
            if let Some(r) = store.get_mut(&job_id) {
                r.status = StrategyScanJobStatus::Running;
                r.started_at_utc = Some(Utc::now()); // allow: operational
            }
        }

        let scan_req = mqk_backtest::ScanRunRequest {
            registry_path: registry_path.clone(),
            bars_root: bars_root.clone(),
            timeframe: timeframe.clone(),
            strategies: vec![strategy.clone()],
            top,
            limit_symbols,
            git_hash: scan_git_hash(),
            created_at_utc: Utc::now().to_rfc3339(), // allow: operational manifest timestamp
        };
        let out_dir_path = PathBuf::from(&out_dir);

        let result: Result<(PathBuf, mqk_backtest::ScanRunOutput), String> =
            tokio::task::spawn_blocking(move || {
                let output = mqk_backtest::execute_strategy_scan(&scan_req)?;
                let run_dir = mqk_backtest::write_scan_artifacts(&out_dir_path, &output)?;
                Ok((run_dir, output))
            })
            .await
            .unwrap_or_else(|join_err| Err(format!("strategy scan task panicked: {join_err}")));

        let mut store = jobs.lock().expect("strategy_scan_jobs lock poisoned");
        if let Some(r) = store.get_mut(&job_id) {
            r.completed_at_utc = Some(Utc::now()); // allow: operational
            match result {
                Ok((run_dir, output)) => {
                    r.status = StrategyScanJobStatus::Completed;
                    r.artifact_dir = Some(run_dir.display().to_string());
                    r.ranked_count = Some(output.summary.ranked_count);
                    r.skipped_count = Some(output.summary.skipped_count);
                    let mut warnings = fixed_research_warnings();
                    warnings.extend(output.manifest.warnings.clone());
                    r.warnings = warnings;
                    r.summary = Some(output.summary);
                }
                Err(e) => {
                    r.status = StrategyScanJobStatus::Failed;
                    r.error = Some(e);
                }
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(StrategyScanJobAcceptedResponse {
            accepted: true,
            job_id,
            status: "queued".to_string(),
            artifact_dir: None,
            error: None,
        }),
    )
        .into_response()
}

fn refused(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(StrategyScanJobAcceptedResponse {
            accepted: false,
            job_id: Uuid::nil(),
            status: "refused".to_string(),
            artifact_dir: None,
            error: Some(message.to_string()),
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET /api/v1/strategy-scans/jobs
// ---------------------------------------------------------------------------

pub(crate) async fn strategy_scan_jobs_list(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let store = st
        .strategy_scan_jobs
        .lock()
        .expect("strategy_scan_jobs lock poisoned");

    let mut jobs: Vec<StrategyScanJobSummary> = store
        .values()
        .map(|r| StrategyScanJobSummary {
            job_id: r.job_id,
            status: r.status.as_str().to_string(),
            timeframe: r.request.timeframe.clone(),
            strategy: r.request.strategy.clone(),
            created_at_utc: r.created_at_utc.to_rfc3339(),
            started_at_utc: r.started_at_utc.map(|t| t.to_rfc3339()),
            completed_at_utc: r.completed_at_utc.map(|t| t.to_rfc3339()),
            artifact_dir: r.artifact_dir.clone(),
            ranked_count: r.ranked_count,
            skipped_count: r.skipped_count,
            error: r.error.clone(),
        })
        .collect();

    // Deterministic ordering: newest first by created_at_utc.
    jobs.sort_by(|a, b| b.created_at_utc.cmp(&a.created_at_utc));

    Json(StrategyScanJobsListResponse {
        truth_state: "active".to_string(),
        jobs,
    })
}

// ---------------------------------------------------------------------------
// GET /api/v1/strategy-scans/jobs/:job_id
// ---------------------------------------------------------------------------

pub(crate) async fn strategy_scan_job_status(
    State(st): State<Arc<AppState>>,
    AxumPath(job_id): AxumPath<Uuid>,
) -> Response {
    let store = st
        .strategy_scan_jobs
        .lock()
        .expect("strategy_scan_jobs lock poisoned");

    match store.get(&job_id) {
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("job_id {} not found", job_id),
                "truth_state": "not_found"
            })),
        )
            .into_response(),
        Some(r) => (
            StatusCode::OK,
            Json(StrategyScanJobStatusResponse {
                truth_state: "active".to_string(),
                job_id: r.job_id,
                status: r.status.as_str().to_string(),
                submitted_at_utc: r.created_at_utc.to_rfc3339(),
                completed_at_utc: r.completed_at_utc.map(|t| t.to_rfc3339()),
                request: r.request.clone(),
                artifact_dir: r.artifact_dir.clone(),
                summary: r.summary.clone(),
                blockers: r.blockers.clone(),
                warnings: r.warnings.clone(),
                error: r.error.clone(),
            }),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/strategy-scans/artifact?artifact_dir=<path>
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct StrategyScanArtifactQuery {
    artifact_dir: Option<String>,
}

pub(crate) async fn strategy_scan_artifact(
    State(st): State<Arc<AppState>>,
    Query(query): Query<StrategyScanArtifactQuery>,
) -> Response {
    let requested = query.artifact_dir.unwrap_or_default();
    let requested = requested.trim().to_string();
    let warnings = fixed_research_warnings();

    if requested.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(artifact_error_response(
                "path_rejected",
                &requested,
                warnings,
                "artifact_dir query parameter is required".to_string(),
            )),
        )
            .into_response();
    }

    let candidate_path = Path::new(&requested);
    let root = Path::new(&st.strategy_scan_artifact_root);

    // Fail-closed: only serve directories that resolve inside the configured
    // scan artifact root. Canonicalize both sides so `../` escapes and
    // symlink tricks cannot bypass the check; a directory that does not
    // exist at all is reported as `missing_artifact`, not `path_rejected`.
    let candidate_canon = match std::fs::canonicalize(candidate_path) {
        Ok(p) => p,
        Err(_) => {
            return Json(artifact_error_response(
                "missing_artifact",
                &requested,
                warnings,
                format!("artifact_dir not found: {requested}"),
            ))
            .into_response();
        }
    };
    let root_canon = match std::fs::canonicalize(root) {
        Ok(p) => p,
        Err(_) => {
            return Json(artifact_error_response(
                "path_rejected",
                &requested,
                warnings,
                format!(
                    "configured scan artifact root is unavailable: {}",
                    root.display()
                ),
            ))
            .into_response();
        }
    };
    if !candidate_canon.starts_with(&root_canon) {
        return Json(artifact_error_response(
            "path_rejected",
            &requested,
            warnings,
            "artifact_dir does not resolve inside the configured scan artifact root".to_string(),
        ))
        .into_response();
    }

    let manifest_path = candidate_canon.join("manifest.json");
    let summary_path = candidate_canon.join("summary.json");
    let candidates_path = candidate_canon.join("candidates.json");

    for p in [&manifest_path, &summary_path, &candidates_path] {
        if !p.is_file() {
            return Json(artifact_error_response(
                "missing_artifact",
                &requested,
                warnings,
                format!("expected artifact file not found: {}", p.display()),
            ))
            .into_response();
        }
    }

    let manifest: mqk_backtest::ScanManifest = match read_artifact_json(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            return Json(artifact_error_response("invalid_artifact", &requested, warnings, e))
                .into_response();
        }
    };
    let summary: mqk_backtest::ScanSummary = match read_artifact_json(&summary_path) {
        Ok(s) => s,
        Err(e) => {
            return Json(artifact_error_response("invalid_artifact", &requested, warnings, e))
                .into_response();
        }
    };
    let candidates: Vec<mqk_backtest::StrategyScanCandidate> =
        match read_artifact_json(&candidates_path) {
            Ok(c) => c,
            Err(e) => {
                return Json(artifact_error_response(
                    "invalid_artifact",
                    &requested,
                    warnings,
                    e,
                ))
                .into_response();
            }
        };

    let capped_candidates: Vec<_> = candidates
        .into_iter()
        .take(MAX_ARTIFACT_CANDIDATE_ROWS)
        .collect();
    let top_candidates = summary.top_ranked.clone();
    let skip_reasons = summary.top_skip_reasons.clone();

    (
        StatusCode::OK,
        Json(StrategyScanArtifactResponse {
            truth_state: "active".to_string(),
            artifact_dir: candidate_canon.display().to_string(),
            manifest: Some(manifest),
            summary: Some(summary),
            candidates: Some(capped_candidates),
            top_candidates,
            skip_reasons,
            warnings,
            blockers: Vec::new(),
            error: None,
        }),
    )
        .into_response()
}

fn artifact_error_response(
    truth_state: &str,
    artifact_dir: &str,
    warnings: Vec<String>,
    error: String,
) -> StrategyScanArtifactResponse {
    StrategyScanArtifactResponse {
        truth_state: truth_state.to_string(),
        artifact_dir: artifact_dir.to_string(),
        manifest: None,
        summary: None,
        candidates: None,
        top_candidates: Vec::new(),
        skip_reasons: Vec::new(),
        warnings,
        blockers: Vec::new(),
        error: Some(error),
    }
}

fn read_artifact_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("read failed: {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse failed: {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// GET /api/v1/strategy-scans/review-artifact?review_dir=<path>
//
// STRATEGY-SCANNER-PROMOTION-01D: read-only readback of a review artifact
// directory written by `mqk backtest review-scan`
// (`mqk_backtest::write_review_artifacts`). Same canonicalize+root-prefix
// path validation pattern as the sibling scanner artifact route above. No
// provider/broker/network call. Does not touch orders/outbox/inbox/
// admission/OMS state.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct StrategyScanReviewArtifactQuery {
    review_dir: Option<String>,
}

pub(crate) async fn strategy_scan_review_artifact(
    State(st): State<Arc<AppState>>,
    Query(query): Query<StrategyScanReviewArtifactQuery>,
) -> Response {
    let requested = query.review_dir.unwrap_or_default();
    let requested = requested.trim().to_string();
    let warnings = fixed_research_warnings();

    if requested.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(review_artifact_error_response(
                "path_rejected",
                &requested,
                warnings,
                "review_dir query parameter is required".to_string(),
            )),
        )
            .into_response();
    }

    let candidate_path = Path::new(&requested);
    let root = Path::new(&st.strategy_review_artifact_root);

    // Fail-closed: only serve directories that resolve inside the configured
    // review artifact root. A directory that does not exist at all is
    // reported as `missing_artifact`, not `path_rejected`.
    let candidate_canon = match std::fs::canonicalize(candidate_path) {
        Ok(p) => p,
        Err(_) => {
            return Json(review_artifact_error_response(
                "missing_artifact",
                &requested,
                warnings,
                format!("review_dir not found: {requested}"),
            ))
            .into_response();
        }
    };
    let root_canon = match std::fs::canonicalize(root) {
        Ok(p) => p,
        Err(_) => {
            return Json(review_artifact_error_response(
                "path_rejected",
                &requested,
                warnings,
                format!(
                    "configured review artifact root is unavailable: {}",
                    root.display()
                ),
            ))
            .into_response();
        }
    };
    if !candidate_canon.starts_with(&root_canon) {
        return Json(review_artifact_error_response(
            "path_rejected",
            &requested,
            warnings,
            "review_dir does not resolve inside the configured review artifact root".to_string(),
        ))
        .into_response();
    }

    let manifest_path = candidate_canon.join("manifest.json");
    let summary_path = candidate_canon.join("summary.json");
    let decisions_path = candidate_canon.join("review_decisions.json");

    for p in [&manifest_path, &summary_path, &decisions_path] {
        if !p.is_file() {
            return Json(review_artifact_error_response(
                "missing_artifact",
                &requested,
                warnings,
                format!("expected artifact file not found: {}", p.display()),
            ))
            .into_response();
        }
    }

    let manifest: mqk_backtest::ReviewManifest = match read_artifact_json(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            return Json(review_artifact_error_response(
                "invalid_artifact",
                &requested,
                warnings,
                e,
            ))
            .into_response();
        }
    };
    let summary: mqk_backtest::ReviewSummary = match read_artifact_json(&summary_path) {
        Ok(s) => s,
        Err(e) => {
            return Json(review_artifact_error_response(
                "invalid_artifact",
                &requested,
                warnings,
                e,
            ))
            .into_response();
        }
    };
    let decisions: Vec<mqk_backtest::StrategyScanReviewDecision> =
        match read_artifact_json(&decisions_path) {
            Ok(d) => d,
            Err(e) => {
                return Json(review_artifact_error_response(
                    "invalid_artifact",
                    &requested,
                    warnings,
                    e,
                ))
                .into_response();
            }
        };

    let capped_decisions: Vec<_> = decisions
        .into_iter()
        .take(MAX_REVIEW_ARTIFACT_DECISION_ROWS)
        .collect();
    let top_paper_candidates = summary.top_paper_candidates.clone();
    let top_watchlist_candidates = summary.top_watchlist_candidates.clone();

    (
        StatusCode::OK,
        Json(StrategyScanReviewArtifactResponse {
            truth_state: "active".to_string(),
            review_dir: candidate_canon.display().to_string(),
            manifest: Some(manifest),
            summary: Some(summary),
            decisions: Some(capped_decisions),
            top_paper_candidates,
            top_watchlist_candidates,
            warnings,
            blockers: Vec::new(),
            error: None,
        }),
    )
        .into_response()
}

fn review_artifact_error_response(
    truth_state: &str,
    review_dir: &str,
    warnings: Vec<String>,
    error: String,
) -> StrategyScanReviewArtifactResponse {
    StrategyScanReviewArtifactResponse {
        truth_state: truth_state.to_string(),
        review_dir: review_dir.to_string(),
        manifest: None,
        summary: None,
        decisions: None,
        top_paper_candidates: Vec::new(),
        top_watchlist_candidates: Vec::new(),
        warnings,
        blockers: Vec::new(),
        error: Some(error),
    }
}

// ---------------------------------------------------------------------------
// Operational helper (mirrors `bkt_git_hash` already duplicated in
// `routes/backtests.rs` and `mqk-cli/src/commands/bkt.rs` — small,
// process-invoking helper kept local to each caller rather than shared).
// ---------------------------------------------------------------------------

fn scan_git_hash() -> String {
    use std::process::Command;
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "UNKNOWN".to_string())
}
