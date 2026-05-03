// DATA-INGEST-GUI-RUNNER-01: TypeScript types matching daemon ingest job API shapes.
// Mirrors api_types.rs IngestJobRequest / IngestJobAcceptedResponse /
// IngestJobSummary / IngestJobsListResponse / IngestJobStatusResponse.

export interface IngestJobRequest {
  source: string;
  csv_path?: string | null;
  timeframe: string;
  source_label?: string | null;
  out_dir?: string | null;
}

export interface IngestJobAcceptedResponse {
  accepted: boolean;
  job_id: string;
  status: string;
  source: string;
  error: string | null;
}

export interface IngestJobSummary {
  job_id: string;
  status: string;
  source: string;
  timeframe: string;
  created_at_utc: string;
  started_at_utc: string | null;
  completed_at_utc: string | null;
  rows_read: number | null;
  rows_inserted: number | null;
  rows_rejected: number | null;
  quality_report_path: string | null;
  error: string | null;
}

export interface IngestJobsListResponse {
  truth_state: string;
  jobs: IngestJobSummary[];
}

export interface IngestJobStatusResponse {
  truth_state: string;
  job_id: string;
  status: string;
  source: string;
  timeframe: string;
  csv_path: string | null;
  created_at_utc: string;
  started_at_utc: string | null;
  completed_at_utc: string | null;
  rows_read: number | null;
  rows_inserted: number | null;
  rows_rejected: number | null;
  quality_report_path: string | null;
  error: string | null;
}

export type IngestJobStatusKind = "queued" | "running" | "completed" | "failed" | "unknown";

// ---------------------------------------------------------------------------
// DATA-INGEST-GUI-RESULTS-01: md_bars coverage types
// ---------------------------------------------------------------------------

export interface MdBarsCoverageRow {
  symbol: string;
  timeframe: string;
  /** Total bar count for this (symbol, timeframe) group. */
  bars: number;
  /** Earliest end_ts (Unix seconds). */
  min_end_ts: number;
  /** Latest end_ts (Unix seconds). */
  max_end_ts: number;
  /** RFC3339 timestamp of most-recent ingest, or null. */
  latest_ingested_at: string | null;
}

export interface MdBarsCoverageResponse {
  canonical_route: string;
  /** "active" | "empty" | "db_unavailable" | "unavailable" */
  truth_state: string;
  /** Echoed timeframe filter, or null when all timeframes requested. */
  timeframe: string | null;
  rows: MdBarsCoverageRow[];
  error: string | null;
}

export interface ActiveIngestJob {
  jobId: string;
  source: string;
  timeframe: string;
  csvPath: string | null;
  createdAt: string;
  startedAt: string | null;
  completedAt: string | null;
  status: IngestJobStatusKind;
  rowsRead: number | null;
  rowsInserted: number | null;
  rowsRejected: number | null;
  qualityReportPath: string | null;
  error: string | null;
}
