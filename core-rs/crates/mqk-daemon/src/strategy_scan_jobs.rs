//! STRATEGY-SCANNER-JOBS-GUI-01B: In-memory strategy scanner job registry.
//!
//! Modeled directly on `crate::backtest_jobs` (BACKTEST-DAEMON-JOBS-01):
//! jobs are daemon-process-lifetime only, no DB persistence. Isolated from
//! live/paper execution: no broker adapters, no OMS tables, no arm_state
//! dependency. A scan job runs the same local-data-only scanner core as
//! `mqk backtest scan-strategies` (via `mqk_backtest::execute_strategy_scan`
//! / `write_scan_artifacts`) — no provider, broker, or network call.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::api_types::StrategyScanJobSubmitRequest;

// ---------------------------------------------------------------------------
// Job status enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyScanJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

impl StrategyScanJobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            StrategyScanJobStatus::Queued => "queued",
            StrategyScanJobStatus::Running => "running",
            StrategyScanJobStatus::Completed => "completed",
            StrategyScanJobStatus::Failed => "failed",
        }
    }
}

// ---------------------------------------------------------------------------
// Job record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct StrategyScanJobRecord {
    pub job_id: Uuid,
    /// The effective request (operator-supplied fields plus resolved
    /// defaults) — echoed back on every status read so the operator can see
    /// exactly what ran, not just what was typed.
    pub request: StrategyScanJobSubmitRequest,
    pub status: StrategyScanJobStatus,
    pub created_at_utc: DateTime<Utc>,
    pub started_at_utc: Option<DateTime<Utc>>,
    pub completed_at_utc: Option<DateTime<Utc>>,
    pub artifact_dir: Option<String>,
    pub ranked_count: Option<usize>,
    pub skipped_count: Option<usize>,
    /// Full ranked/skipped summary, present only once the job completes
    /// successfully. Reuses `mqk_backtest::ScanSummary` verbatim — no
    /// daemon-local reduction or re-derivation of scanner output.
    pub summary: Option<mqk_backtest::ScanSummary>,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Store type alias
// ---------------------------------------------------------------------------

pub type StrategyScanJobStore = Arc<Mutex<HashMap<Uuid, StrategyScanJobRecord>>>;

pub fn new_strategy_scan_job_store() -> StrategyScanJobStore {
    Arc::new(Mutex::new(HashMap::new()))
}
