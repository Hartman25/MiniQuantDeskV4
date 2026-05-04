//! DATA-INGEST-DAEMON-JOBS-01: In-memory market-data ingest job registry.
//!
//! Mirrors the backtest_jobs.rs pattern.  No DB persistence — jobs are
//! process-lifetime only.  Isolated from live/paper execution: no broker
//! adapters, no OMS tables, no arm_state dependency.
//!
//! Extended in DATA-INGEST-DAEMON-PROVIDER-JOBS-01 to carry provider-specific
//! fields for dry-run registry-backed sync jobs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Job status enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestJobStatus {
    Queued,
    Running,
    Completed,
    /// Dry-run completed: symbols resolved, no provider API calls made,
    /// no DB/CSV writes performed.
    DryRunCompleted,
    Failed,
}

impl IngestJobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            IngestJobStatus::Queued => "queued",
            IngestJobStatus::Running => "running",
            IngestJobStatus::Completed => "completed",
            IngestJobStatus::DryRunCompleted => "dry_run_completed",
            IngestJobStatus::Failed => "failed",
        }
    }
}

// ---------------------------------------------------------------------------
// Job record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct IngestJobRecord {
    pub job_id: Uuid,
    /// "csv" | "twelvedata". Reserved for future provider values.
    pub source: String,
    /// Job mode. None for source="csv"; "sync_provider" for provider jobs.
    pub mode: Option<String>,
    /// Absolute/relative path to the CSV file (source="csv" only).
    pub csv_path: Option<String>,
    /// Canonical timeframe: "1D" | "1m" | "5m".
    pub timeframe: String,
    /// Source label stored in the quality report (e.g. "csv", "manual").
    pub source_label: String,
    /// Output directory root for quality report artifacts.
    pub out_dir: String,
    pub status: IngestJobStatus,
    pub created_at_utc: DateTime<Utc>,
    pub started_at_utc: Option<DateTime<Utc>>,
    pub completed_at_utc: Option<DateTime<Utc>>,
    pub rows_read: Option<i64>,
    pub rows_inserted: Option<i64>,
    pub rows_rejected: Option<i64>,
    /// Filesystem path to the written data_quality.json artifact.
    pub quality_report_path: Option<String>,
    pub error: Option<String>,

    // -----------------------------------------------------------------------
    // Provider-job fields (DATA-INGEST-DAEMON-PROVIDER-JOBS-01)
    // -----------------------------------------------------------------------
    /// Whether this job ran as a dry run (no provider calls, no DB/CSV writes).
    pub dry_run: bool,
    /// Whether real provider API calls were permitted for this job.
    pub provider_api_calls_allowed: bool,
    /// Number of provider API calls actually made (always 0 for dry-run jobs).
    pub api_calls_made: i64,
    /// Symbol source: "registry" | "list".
    pub symbols_source: Option<String>,
    /// Registry path used when symbols_source="registry".
    pub registry_path_used: Option<String>,
    /// Number of planned symbols resolved from the source.
    pub symbols_count: Option<usize>,
    /// First planned symbol alphabetically (operator preview).
    pub planned_first_symbol: Option<String>,
    /// Last planned symbol alphabetically (operator preview).
    pub planned_last_symbol: Option<String>,
}

// ---------------------------------------------------------------------------
// Store type alias
// ---------------------------------------------------------------------------

pub type IngestJobStore = Arc<Mutex<HashMap<Uuid, IngestJobRecord>>>;

pub fn new_ingest_job_store() -> IngestJobStore {
    Arc::new(Mutex::new(HashMap::new()))
}
