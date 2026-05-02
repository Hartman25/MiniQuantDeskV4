//! BACKTEST-DAEMON-JOBS-01: In-memory backtest job registry.
//!
//! Defines the daemon-local job record type and store.
//! No DB persistence — jobs are process-lifetime only.
//! Isolated from live/paper execution: no broker adapters, no OMS tables.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Job status enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BacktestJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

impl BacktestJobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            BacktestJobStatus::Queued => "queued",
            BacktestJobStatus::Running => "running",
            BacktestJobStatus::Completed => "completed",
            BacktestJobStatus::Failed => "failed",
        }
    }
}

// ---------------------------------------------------------------------------
// Job record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BacktestJobRecord {
    pub job_id: Uuid,
    pub strategy: String,
    pub symbol: String,
    pub bars_path: String,
    pub timeframe_secs: i64,
    pub initial_cash_micros: i64,
    pub status: BacktestJobStatus,
    pub created_at_utc: DateTime<Utc>,
    pub started_at_utc: Option<DateTime<Utc>>,
    pub completed_at_utc: Option<DateTime<Utc>>,
    pub artifact_dir: Option<String>,
    pub manifest_path: Option<String>,
    pub metrics_path: Option<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Store type alias
// ---------------------------------------------------------------------------

pub type BacktestJobStore = Arc<Mutex<HashMap<Uuid, BacktestJobRecord>>>;

pub fn new_job_store() -> BacktestJobStore {
    Arc::new(Mutex::new(HashMap::new()))
}
