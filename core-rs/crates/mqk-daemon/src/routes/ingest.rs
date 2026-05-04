//! DATA-INGEST-DAEMON-JOBS-01: Daemon-managed market-data ingestion job API.
//!
//! Routes:
//!   POST /api/v1/ingest/jobs          — submit a CSV ingest job (operator)
//!   GET  /api/v1/ingest/jobs          — list all in-memory jobs (public)
//!   GET  /api/v1/ingest/jobs/:job_id  — job status + artifact paths (public)
//!
//! Safety invariants:
//! - No broker adapter is called. No Alpaca. No live/paper execution state.
//! - No OMS tables written. No orders/fills in live DB tables.
//! - Does not require arm_state. Does not start/stop the trading runtime.
//! - Jobs are in-memory only (process-lifetime); no DB persistence of job state.
//! - Quality reports are written to exports/md_ingest/<ingest_id>/ by default.
//! - Failed jobs report errors truthfully. No hidden failures.
//! - If DB is unavailable (pool is None), job fails with "no_db" error.
//! - Provider sources ("twelvedata", etc.) return not_implemented in this patch.

use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::{
    api_types::{
        IngestJobAcceptedResponse, IngestJobRequest, IngestJobStatusResponse, IngestJobSummary,
        IngestJobsListResponse, TrackedEquitiesResponse, TrackedEquitySummary,
    },
    ingest_jobs::{IngestJobRecord, IngestJobStatus},
    state::AppState,
};

// ---------------------------------------------------------------------------
// Timeframe validation (no mqk-md dep required for daemon validation)
// ---------------------------------------------------------------------------

/// Validates the timeframe string. Returns the canonical form or an error.
fn validate_timeframe(tf: &str) -> Result<&'static str, String> {
    match tf.trim() {
        "1D" | "1d" => Ok("1D"),
        "1m" | "1min" | "1minute" => Ok("1m"),
        "5m" | "5min" | "5minute" => Ok("5m"),
        other => Err(format!(
            "unsupported timeframe '{}'. accepted: 1D | 1m | 5m",
            other
        )),
    }
}

// ---------------------------------------------------------------------------
// POST /api/v1/ingest/jobs
// ---------------------------------------------------------------------------

pub(crate) async fn ingest_job_submit(
    State(st): State<Arc<AppState>>,
    Json(req): Json<IngestJobRequest>,
) -> Response {
    let source = req.source.trim().to_ascii_lowercase();

    // Source gate: only "csv" is implemented. Provider is explicitly deferred.
    if source != "csv" {
        return (
            StatusCode::BAD_REQUEST,
            Json(IngestJobAcceptedResponse {
                accepted: false,
                job_id: Uuid::nil(),
                status: "refused".to_string(),
                source: source.clone(),
                error: Some(format!(
                    "source '{}' is not implemented in this version. \
                     only 'csv' is supported. provider ingestion is deferred.",
                    source
                )),
            }),
        )
            .into_response();
    }

    // For source="csv": csv_path is required and non-empty.
    let csv_path = match &req.csv_path {
        Some(p) if !p.trim().is_empty() => p.trim().to_string(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(IngestJobAcceptedResponse {
                    accepted: false,
                    job_id: Uuid::nil(),
                    status: "refused".to_string(),
                    source: source.clone(),
                    error: Some(
                        "csv_path is required for source='csv' and must not be empty".to_string(),
                    ),
                }),
            )
                .into_response();
        }
    };

    // Timeframe validation.
    let timeframe = match validate_timeframe(&req.timeframe) {
        Ok(tf) => tf.to_string(),
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(IngestJobAcceptedResponse {
                    accepted: false,
                    job_id: Uuid::nil(),
                    status: "refused".to_string(),
                    source: source.clone(),
                    error: Some(e),
                }),
            )
                .into_response();
        }
    };

    // CSV path must exist at submission time (fail fast, not silently).
    if !Path::new(&csv_path).exists() {
        return (
            StatusCode::BAD_REQUEST,
            Json(IngestJobAcceptedResponse {
                accepted: false,
                job_id: Uuid::nil(),
                status: "refused".to_string(),
                source: source.clone(),
                error: Some(format!("csv_path not found: {}", csv_path)),
            }),
        )
            .into_response();
    }

    let source_label = req
        .source_label
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("csv")
        .to_string();

    let out_dir = req
        .out_dir
        .clone()
        .unwrap_or_else(|| "exports/md_ingest".to_string());

    // Create job record.
    let job_id = Uuid::new_v4(); // allow: process-local transient job identifier
    let created_at = Utc::now(); // allow: operational job creation timestamp
    let record = IngestJobRecord {
        job_id,
        source: source.clone(),
        csv_path: Some(csv_path.clone()),
        timeframe: timeframe.clone(),
        source_label: source_label.clone(),
        out_dir: out_dir.clone(),
        status: IngestJobStatus::Queued,
        created_at_utc: created_at,
        started_at_utc: None,
        completed_at_utc: None,
        rows_read: None,
        rows_inserted: None,
        rows_rejected: None,
        quality_report_path: None,
        error: None,
    };

    {
        let mut store = st.ingest_jobs.lock().expect("ingest_jobs lock poisoned");
        store.insert(job_id, record);
    }

    // Clone state for background task.
    let jobs = Arc::clone(&st.ingest_jobs);
    let db_pool = st.db.clone();

    tokio::spawn(async move {
        // Mark running.
        {
            let mut store = jobs.lock().expect("ingest_jobs lock poisoned");
            if let Some(r) = store.get_mut(&job_id) {
                r.status = IngestJobStatus::Running;
                r.started_at_utc = Some(Utc::now()); // allow: operational
            }
        }

        let result =
            run_csv_ingest_async(db_pool, csv_path, timeframe, source_label, out_dir).await;

        // Mark completed / failed.
        let mut store = jobs.lock().expect("ingest_jobs lock poisoned");
        if let Some(r) = store.get_mut(&job_id) {
            r.completed_at_utc = Some(Utc::now()); // allow: operational
            match result {
                Ok(outcome) => {
                    r.status = IngestJobStatus::Completed;
                    r.rows_read = Some(outcome.rows_read);
                    r.rows_inserted = Some(outcome.rows_inserted);
                    r.rows_rejected = Some(outcome.rows_rejected);
                    r.quality_report_path = outcome.quality_report_path;
                }
                Err(e) => {
                    r.status = IngestJobStatus::Failed;
                    r.error = Some(e);
                }
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(IngestJobAcceptedResponse {
            accepted: true,
            job_id,
            status: "queued".to_string(),
            source,
            error: None,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Ingest outcome (internal)
// ---------------------------------------------------------------------------

struct IngestOutcome {
    rows_read: i64,
    rows_inserted: i64,
    rows_rejected: i64,
    quality_report_path: Option<String>,
}

// ---------------------------------------------------------------------------
// Async CSV ingest execution
// ---------------------------------------------------------------------------

/// Run CSV ingest against the DB and write a quality report artifact.
///
/// Safety:
/// - No broker adapter calls. No OMS tables. No live/paper execution state.
/// - Calls `mqk_db::md::ingest_csv_to_md_bars` using the daemon's DB pool.
/// - If pool is None, returns Err("no_db: ...").
/// - Returns Err(message) on any failure — never panics.
async fn run_csv_ingest_async(
    db_pool: Option<sqlx::PgPool>,
    csv_path: String,
    timeframe: String,
    source_label: String,
    out_dir: String,
) -> Result<IngestOutcome, String> {
    let pool = match db_pool {
        Some(p) => p,
        None => return Err("no_db: database pool is not configured".to_string()),
    };

    // Deterministic ingest_id (D1-5 rule: no Uuid::new_v4 in ingestion path).
    // Uses same key format as the CLI for cross-tool idempotency.
    let ingest_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_DNS,
        format!(
            "mqk-md-ingest.csv.v1|{}|{}|{}",
            source_label, csv_path, timeframe
        )
        .as_bytes(),
    );

    let res = mqk_db::md::ingest_csv_to_md_bars(
        &pool,
        mqk_db::md::IngestCsvArgs {
            path: std::path::PathBuf::from(&csv_path),
            timeframe: timeframe.clone(),
            source: source_label.clone(),
            ingest_id,
        },
    )
    .await
    .map_err(|e| format!("ingest_csv failed: {}", e))?;

    // Write quality report artifact.
    let report_dir = std::path::Path::new(&out_dir).join(res.ingest_id.to_string());
    std::fs::create_dir_all(&report_dir).map_err(|e| format!("create report dir failed: {}", e))?;

    let report_path = report_dir.join("data_quality.json");
    let json_str = serde_json::to_string_pretty(&res.report)
        .map_err(|e| format!("serialize report failed: {}", e))?;
    std::fs::write(&report_path, json_str).map_err(|e| format!("write report failed: {}", e))?;

    let coverage = &res.report.coverage;
    Ok(IngestOutcome {
        rows_read: coverage.rows_read as i64,
        rows_inserted: coverage.rows_inserted as i64,
        rows_rejected: coverage.rows_rejected as i64,
        quality_report_path: Some(report_path.to_string_lossy().to_string()),
    })
}

// ---------------------------------------------------------------------------
// GET /api/v1/ingest/jobs
// ---------------------------------------------------------------------------

pub(crate) async fn ingest_jobs_list(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let store = st.ingest_jobs.lock().expect("ingest_jobs lock poisoned");

    let mut jobs: Vec<IngestJobSummary> = store
        .values()
        .map(|r| IngestJobSummary {
            job_id: r.job_id,
            status: r.status.as_str().to_string(),
            source: r.source.clone(),
            timeframe: r.timeframe.clone(),
            created_at_utc: r.created_at_utc.to_rfc3339(),
            started_at_utc: r.started_at_utc.map(|t| t.to_rfc3339()),
            completed_at_utc: r.completed_at_utc.map(|t| t.to_rfc3339()),
            rows_read: r.rows_read,
            rows_inserted: r.rows_inserted,
            rows_rejected: r.rows_rejected,
            quality_report_path: r.quality_report_path.clone(),
            error: r.error.clone(),
        })
        .collect();

    // Deterministic ordering: newest first by created_at_utc.
    jobs.sort_by(|a, b| b.created_at_utc.cmp(&a.created_at_utc));

    Json(IngestJobsListResponse {
        truth_state: "active".to_string(),
        jobs,
    })
}

// ---------------------------------------------------------------------------
// GET /api/v1/ingest/tracked-equities
// ---------------------------------------------------------------------------

/// Return the enabled equity universe from the canonical instrument registry.
///
/// Safety invariants:
/// - No broker adapter. No provider API calls. No DB writes.
/// - Does not touch live/paper execution state. No arm_state required.
/// - Read-only filesystem access to the registry JSON file.
/// - Provider sync job is NOT triggered. No API credits consumed.
pub(crate) async fn tracked_equities_list(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let registry_path = st.instrument_registry_path.clone();
    let path = std::path::Path::new(&registry_path);

    let instruments = match mqk_md::instrument_registry::load_instrument_registry(path) {
        Ok(v) => v,
        Err(e) => {
            let (truth_state, error_msg) = if path.exists() {
                ("registry_invalid", format!("registry parse failed: {}", e))
            } else {
                (
                    "registry_unavailable",
                    format!("registry file not found: {}", registry_path),
                )
            };
            return Json(TrackedEquitiesResponse {
                canonical_route: "/api/v1/ingest/tracked-equities".to_string(),
                truth_state: truth_state.to_string(),
                registry_path,
                count: 0,
                symbols: vec![],
                first_symbol: None,
                last_symbol: None,
                error: Some(error_msg),
            });
        }
    };

    let equities = mqk_md::instrument_registry::enabled_equities(&instruments);
    let count = equities.len();
    let first_symbol = equities.first().map(|e| e.symbol.clone());
    let last_symbol = equities.last().map(|e| e.symbol.clone());

    let symbols: Vec<TrackedEquitySummary> = equities
        .into_iter()
        .map(|e| TrackedEquitySummary {
            symbol: e.symbol.clone(),
            instrument_id: e.instrument_id.clone(),
            provider: e.provider.clone(),
            venue: e.venue.clone(),
            timeframes: e.timeframes.clone(),
        })
        .collect();

    Json(TrackedEquitiesResponse {
        canonical_route: "/api/v1/ingest/tracked-equities".to_string(),
        truth_state: "active".to_string(),
        registry_path,
        count,
        symbols,
        first_symbol,
        last_symbol,
        error: None,
    })
}

// ---------------------------------------------------------------------------
// GET /api/v1/ingest/jobs/:job_id
// ---------------------------------------------------------------------------

pub(crate) async fn ingest_job_status(
    State(st): State<Arc<AppState>>,
    AxumPath(job_id): AxumPath<Uuid>,
) -> Response {
    let store = st.ingest_jobs.lock().expect("ingest_jobs lock poisoned");

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
            Json(IngestJobStatusResponse {
                truth_state: "active".to_string(),
                job_id: r.job_id,
                status: r.status.as_str().to_string(),
                source: r.source.clone(),
                timeframe: r.timeframe.clone(),
                csv_path: r.csv_path.clone(),
                created_at_utc: r.created_at_utc.to_rfc3339(),
                started_at_utc: r.started_at_utc.map(|t| t.to_rfc3339()),
                completed_at_utc: r.completed_at_utc.map(|t| t.to_rfc3339()),
                rows_read: r.rows_read,
                rows_inserted: r.rows_inserted,
                rows_rejected: r.rows_rejected,
                quality_report_path: r.quality_report_path.clone(),
                error: r.error.clone(),
            }),
        )
            .into_response(),
    }
}
