//! DATA-INGEST-DAEMON-JOBS-01: Market-data ingest job registry.
//!
//! Isolated from live/paper execution: no broker adapters, no OMS tables,
//! no arm_state dependency.
//!
//! Extended in DATA-INGEST-DAEMON-PROVIDER-JOBS-01 to carry provider-specific
//! fields for dry-run registry-backed sync jobs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
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
    Refused,
    /// Partial success: at least one symbol succeeded and at least one failed,
    /// or all symbols fetched but a guardrail was hit before completing the batch.
    Partial,
}

impl IngestJobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            IngestJobStatus::Queued => "queued",
            IngestJobStatus::Running => "running",
            IngestJobStatus::Completed => "completed",
            IngestJobStatus::DryRunCompleted => "dry_run_completed",
            IngestJobStatus::Failed => "failed",
            IngestJobStatus::Refused => "refused",
            IngestJobStatus::Partial => "partial",
        }
    }

    fn from_str(value: &str) -> Result<Self, String> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "dry_run_completed" => Ok(Self::DryRunCompleted),
            "failed" => Ok(Self::Failed),
            "refused" => Ok(Self::Refused),
            "partial" => Ok(Self::Partial),
            other => Err(format!("unknown ingest job status in DB: {other}")),
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
    // Provider-job fields (DATA-INGEST-DAEMON-PROVIDER-JOBS-01 / DATA-PROVIDER-FOUNDATION-01)
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
    /// Provider registry path used for provider metadata validation.
    pub provider_registry_path_used: Option<String>,
    /// Number of planned symbols resolved from the source.
    pub symbols_count: Option<usize>,
    /// First planned symbol alphabetically (operator preview).
    pub planned_first_symbol: Option<String>,
    /// Last planned symbol alphabetically (operator preview).
    pub planned_last_symbol: Option<String>,
    /// Asset class requested for this job (e.g. "equity").
    pub asset_class: String,
    /// Whether the provider is enabled per the provider registry (None if registry unavailable).
    pub provider_enabled: Option<bool>,
    /// Provider verification status from the provider registry (None if registry unavailable).
    pub provider_verification_status: Option<String>,
    /// Number of symbols for which provider fetch succeeded (real jobs only).
    pub symbols_completed: Option<usize>,
    /// Number of symbols for which provider fetch failed (real jobs only).
    pub symbols_failed: Option<usize>,
}

// ---------------------------------------------------------------------------
// Store type alias
// ---------------------------------------------------------------------------

pub type IngestJobStore = Arc<Mutex<HashMap<Uuid, IngestJobRecord>>>;

pub fn new_ingest_job_store() -> IngestJobStore {
    Arc::new(Mutex::new(HashMap::new()))
}

fn usize_to_i64(value: Option<usize>) -> Option<i64> {
    value.and_then(|v| i64::try_from(v).ok())
}

fn i64_to_usize(value: Option<i64>, column: &str) -> Result<Option<usize>, String> {
    value
        .map(|v| {
            usize::try_from(v)
                .map_err(|_| format!("invalid negative or overflowing {column} in DB: {v}"))
        })
        .transpose()
}

fn row_to_record(row: sqlx::postgres::PgRow) -> Result<IngestJobRecord, String> {
    let status: String = row
        .try_get("status")
        .map_err(|e| format!("decode status failed: {e}"))?;

    Ok(IngestJobRecord {
        job_id: row
            .try_get("job_id")
            .map_err(|e| format!("decode job_id failed: {e}"))?,
        source: row
            .try_get("source")
            .map_err(|e| format!("decode source failed: {e}"))?,
        mode: row
            .try_get("mode")
            .map_err(|e| format!("decode mode failed: {e}"))?,
        csv_path: row
            .try_get("csv_path")
            .map_err(|e| format!("decode csv_path failed: {e}"))?,
        timeframe: row
            .try_get("timeframe")
            .map_err(|e| format!("decode timeframe failed: {e}"))?,
        source_label: row
            .try_get::<String, _>("source")
            .map_err(|e| format!("decode source_label failed: {e}"))?,
        out_dir: String::new(),
        status: IngestJobStatus::from_str(&status)?,
        created_at_utc: row
            .try_get("created_at_utc")
            .map_err(|e| format!("decode created_at_utc failed: {e}"))?,
        started_at_utc: row
            .try_get("started_at_utc")
            .map_err(|e| format!("decode started_at_utc failed: {e}"))?,
        completed_at_utc: row
            .try_get("completed_at_utc")
            .map_err(|e| format!("decode completed_at_utc failed: {e}"))?,
        rows_read: row
            .try_get("rows_read")
            .map_err(|e| format!("decode rows_read failed: {e}"))?,
        rows_inserted: row
            .try_get("rows_inserted")
            .map_err(|e| format!("decode rows_inserted failed: {e}"))?,
        rows_rejected: row
            .try_get("rows_rejected")
            .map_err(|e| format!("decode rows_rejected failed: {e}"))?,
        quality_report_path: row
            .try_get("quality_report_path")
            .map_err(|e| format!("decode quality_report_path failed: {e}"))?,
        error: row
            .try_get("error")
            .map_err(|e| format!("decode error failed: {e}"))?,
        dry_run: row
            .try_get("dry_run")
            .map_err(|e| format!("decode dry_run failed: {e}"))?,
        provider_api_calls_allowed: row
            .try_get("provider_api_calls_allowed")
            .map_err(|e| format!("decode provider_api_calls_allowed failed: {e}"))?,
        api_calls_made: row
            .try_get("api_calls_made")
            .map_err(|e| format!("decode api_calls_made failed: {e}"))?,
        symbols_source: row
            .try_get("symbols_source")
            .map_err(|e| format!("decode symbols_source failed: {e}"))?,
        registry_path_used: row
            .try_get("registry_path")
            .map_err(|e| format!("decode registry_path failed: {e}"))?,
        provider_registry_path_used: row
            .try_get("provider_registry_path")
            .map_err(|e| format!("decode provider_registry_path failed: {e}"))?,
        symbols_count: i64_to_usize(
            row.try_get("symbols_count")
                .map_err(|e| format!("decode symbols_count failed: {e}"))?,
            "symbols_count",
        )?,
        planned_first_symbol: row
            .try_get("planned_first_symbol")
            .map_err(|e| format!("decode planned_first_symbol failed: {e}"))?,
        planned_last_symbol: row
            .try_get("planned_last_symbol")
            .map_err(|e| format!("decode planned_last_symbol failed: {e}"))?,
        asset_class: row
            .try_get("asset_class")
            .map_err(|e| format!("decode asset_class failed: {e}"))?,
        provider_enabled: row
            .try_get("provider_enabled")
            .map_err(|e| format!("decode provider_enabled failed: {e}"))?,
        provider_verification_status: row
            .try_get("provider_verification_status")
            .map_err(|e| format!("decode provider_verification_status failed: {e}"))?,
        symbols_completed: i64_to_usize(
            row.try_get("symbols_completed")
                .map_err(|e| format!("decode symbols_completed failed: {e}"))?,
            "symbols_completed",
        )?,
        symbols_failed: i64_to_usize(
            row.try_get("symbols_failed")
                .map_err(|e| format!("decode symbols_failed failed: {e}"))?,
            "symbols_failed",
        )?,
    })
}

pub async fn persist_ingest_job_record(
    pool: &PgPool,
    record: &IngestJobRecord,
) -> Result<(), String> {
    let updated_at_utc = Utc::now();

    sqlx::query(
        r#"
        insert into sys_ingest_jobs (
            job_id,
            source,
            mode,
            status,
            timeframe,
            csv_path,
            registry_path,
            provider_registry_path,
            symbols_source,
            asset_class,
            dry_run,
            provider_api_calls_allowed,
            api_calls_made,
            symbols_count,
            symbols_completed,
            symbols_failed,
            planned_first_symbol,
            planned_last_symbol,
            rows_read,
            rows_inserted,
            rows_rejected,
            quality_report_path,
            error,
            provider_enabled,
            provider_verification_status,
            created_at_utc,
            started_at_utc,
            completed_at_utc,
            updated_at_utc
        )
        values (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
            $11, $12, $13, $14, $15, $16, $17, $18, $19, $20,
            $21, $22, $23, $24, $25, $26, $27, $28, $29
        )
        on conflict (job_id) do update
            set source                       = excluded.source,
                mode                         = excluded.mode,
                status                       = excluded.status,
                timeframe                    = excluded.timeframe,
                csv_path                     = excluded.csv_path,
                registry_path                = excluded.registry_path,
                provider_registry_path       = excluded.provider_registry_path,
                symbols_source               = excluded.symbols_source,
                asset_class                  = excluded.asset_class,
                dry_run                      = excluded.dry_run,
                provider_api_calls_allowed   = excluded.provider_api_calls_allowed,
                api_calls_made               = excluded.api_calls_made,
                symbols_count                = excluded.symbols_count,
                symbols_completed            = excluded.symbols_completed,
                symbols_failed               = excluded.symbols_failed,
                planned_first_symbol         = excluded.planned_first_symbol,
                planned_last_symbol          = excluded.planned_last_symbol,
                rows_read                    = excluded.rows_read,
                rows_inserted                = excluded.rows_inserted,
                rows_rejected                = excluded.rows_rejected,
                quality_report_path          = excluded.quality_report_path,
                error                        = excluded.error,
                provider_enabled             = excluded.provider_enabled,
                provider_verification_status = excluded.provider_verification_status,
                started_at_utc               = excluded.started_at_utc,
                completed_at_utc             = excluded.completed_at_utc,
                updated_at_utc               = excluded.updated_at_utc
        "#,
    )
    .bind(record.job_id)
    .bind(&record.source)
    .bind(&record.mode)
    .bind(record.status.as_str())
    .bind(&record.timeframe)
    .bind(&record.csv_path)
    .bind(&record.registry_path_used)
    .bind(&record.provider_registry_path_used)
    .bind(&record.symbols_source)
    .bind(&record.asset_class)
    .bind(record.dry_run)
    .bind(record.provider_api_calls_allowed)
    .bind(record.api_calls_made)
    .bind(usize_to_i64(record.symbols_count))
    .bind(usize_to_i64(record.symbols_completed))
    .bind(usize_to_i64(record.symbols_failed))
    .bind(&record.planned_first_symbol)
    .bind(&record.planned_last_symbol)
    .bind(record.rows_read)
    .bind(record.rows_inserted)
    .bind(record.rows_rejected)
    .bind(&record.quality_report_path)
    .bind(&record.error)
    .bind(record.provider_enabled)
    .bind(&record.provider_verification_status)
    .bind(record.created_at_utc)
    .bind(record.started_at_utc)
    .bind(record.completed_at_utc)
    .bind(updated_at_utc)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|e| format!("persist ingest job {} failed: {e}", record.job_id))
}

pub async fn list_persisted_ingest_jobs(pool: &PgPool) -> Result<Vec<IngestJobRecord>, String> {
    let rows = sqlx::query(
        r#"
        select
            job_id,
            source,
            mode,
            status,
            timeframe,
            csv_path,
            registry_path,
            provider_registry_path,
            symbols_source,
            asset_class,
            dry_run,
            provider_api_calls_allowed,
            api_calls_made,
            symbols_count,
            symbols_completed,
            symbols_failed,
            planned_first_symbol,
            planned_last_symbol,
            rows_read,
            rows_inserted,
            rows_rejected,
            quality_report_path,
            error,
            provider_enabled,
            provider_verification_status,
            created_at_utc,
            started_at_utc,
            completed_at_utc
        from sys_ingest_jobs
        order by updated_at_utc desc, created_at_utc desc, job_id asc
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("list persisted ingest jobs failed: {e}"))?;

    rows.into_iter().map(row_to_record).collect()
}

pub async fn load_persisted_ingest_job(
    pool: &PgPool,
    job_id: Uuid,
) -> Result<Option<IngestJobRecord>, String> {
    let row = sqlx::query(
        r#"
        select
            job_id,
            source,
            mode,
            status,
            timeframe,
            csv_path,
            registry_path,
            provider_registry_path,
            symbols_source,
            asset_class,
            dry_run,
            provider_api_calls_allowed,
            api_calls_made,
            symbols_count,
            symbols_completed,
            symbols_failed,
            planned_first_symbol,
            planned_last_symbol,
            rows_read,
            rows_inserted,
            rows_rejected,
            quality_report_path,
            error,
            provider_enabled,
            provider_verification_status,
            created_at_utc,
            started_at_utc,
            completed_at_utc
        from sys_ingest_jobs
        where job_id = $1
        "#,
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("load persisted ingest job {job_id} failed: {e}"))?;

    row.map(row_to_record).transpose()
}
