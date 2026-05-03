//! DATA-INGEST-DAEMON-JOBS-01: In-memory market-data ingest job registry.
//!
//! Mirrors the backtest_jobs.rs pattern.  No DB persistence — jobs are
//! process-lifetime only.  Isolated from live/paper execution: no broker
//! adapters, no OMS tables, no arm_state dependency.

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
    Failed,
}

impl IngestJobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            IngestJobStatus::Queued => "queued",
            IngestJobStatus::Running => "running",
            IngestJobStatus::Completed => "completed",
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
    /// "csv" for this patch. Reserved for future provider values.
    pub source: String,
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
}

// ---------------------------------------------------------------------------
// Store type alias
// ---------------------------------------------------------------------------

pub type IngestJobStore = Arc<Mutex<HashMap<Uuid, IngestJobRecord>>>;

pub fn new_ingest_job_store() -> IngestJobStore {
    Arc::new(Mutex::new(HashMap::new()))
}
