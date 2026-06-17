// DATA-INGEST-GUI-RUNNER-01: TypeScript types matching daemon ingest job API shapes.
// Mirrors api_types.rs IngestJobRequest / IngestJobAcceptedResponse /
// IngestJobSummary / IngestJobsListResponse / IngestJobStatusResponse.

export interface IngestJobRequest {
  source: string;
  csv_path?: string | null;
  timeframe: string;
  source_label?: string | null;
  out_dir?: string | null;
  // Provider job fields (DATA-INGEST-GUI-PROVIDER-RUNNER-01):
  mode?: string | null;
  symbols_source?: string | null;
  registry_path?: string | null;
  provider_registry_path?: string | null;
  asset_class?: string;
  start?: string | null;
  end?: string | null;
  dry_run?: boolean;
  allow_provider_api_calls?: boolean;
  api_credits_per_minute?: number | null;
  api_credits_per_day?: number | null;
}

export interface IngestJobAcceptedResponse {
  accepted: boolean;
  job_id: string;
  status: string;
  source: string;
  error: string | null;
  // Provider job fields (null for CSV jobs or on refusal):
  dry_run?: boolean | null;
  provider_api_calls_allowed?: boolean | null;
  symbols_count?: number | null;
  api_calls_made?: number | null;
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
  /** null for CSV, "sync_provider" for provider jobs */
  mode: string | null;
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
  // Provider job fields (DATA-INGEST-GUI-PROVIDER-RUNNER-01):
  dry_run: boolean;
  provider_api_calls_allowed: boolean;
  api_calls_made: number;
  symbols_source: string | null;
  registry_path_used: string | null;
  symbols_count: number | null;
  planned_first_symbol: string | null;
  planned_last_symbol: string | null;
  asset_class: string;
  provider_enabled: boolean | null;
  provider_verification_status: string | null;
  /** Number of symbols for which provider fetch succeeded (null for dry-run). */
  symbols_completed: number | null;
  /** Number of symbols for which provider fetch failed (null for dry-run). */
  symbols_failed: number | null;
}

export interface CancelIngestJobResponse {
  canonical_route: string;
  truth_state: string;
  accepted: boolean;
  job_id: string;
  status?: string;
  error: string | null;
}

export type IngestJobStatusKind =
  | "queued"
  | "running"
  | "completed"
  | "dry_run_completed"
  | "partial"
  | "refused"
  | "cancelled"
  | "failed"
  | "unknown";

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

// ---------------------------------------------------------------------------
// DATA-INGEST-GUI-SYNC-ALL-01: Tracked-equities registry preview
// ---------------------------------------------------------------------------

export interface TrackedEquitySummary {
  symbol: string;
  instrument_id: string;
  provider: string;
  venue: string;
  timeframes: string[];
}

export interface TrackedEquitiesResponse {
  canonical_route: string;
  /** "active" | "registry_unavailable" | "registry_invalid" */
  truth_state: string;
  registry_path: string;
  count: number;
  symbols: TrackedEquitySummary[];
  first_symbol: string | null;
  last_symbol: string | null;
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

/** In-flight or terminal provider sync job tracked by the GUI. */
export interface ActiveProviderJob {
  jobId: string;
  status: IngestJobStatusKind;
  dryRun: boolean;
  allowProviderApiCalls: boolean;
  createdAt: string;
  startedAt: string | null;
  completedAt: string | null;
  error: string | null;
  apiCallsMade: number;
  symbolsCount: number | null;
  symbolsCompleted: number | null;
  symbolsFailed: number | null;
  rowsInserted: number | null;
  rowsRejected: number | null;
  plannedFirstSymbol: string | null;
  plannedLastSymbol: string | null;
}

// INTRADAY-MD-REFRESHER-GUI-01: Intraday refresh status types
export interface IntradayRefreshSymbolStatus {
  symbol: string;
  gate: string | null;
  completed_count: number | null;
  latest_completed_bar_ts: string | null;
  staleness_min: number | null;
  provider_source: string | null;
  provider_configured: boolean | null;
  provider_attempted: boolean | null;
  provider_success: boolean | null;
  rows_inserted: number | null;
  rows_updated: number | null;
  rows_filtered_incomplete: number | null;
  rows_filtered_in_progress: number | null;
  fail_reasons: string[];
}

export interface IntradayRefreshStatusResponse {
  canonical_route: string;
  truth_state: string;
  evidence_path: string | null;
  stale_or_missing_evidence: boolean;
  schema_version: string | null;
  produced_at_utc: string | null;
  mode: string | null;
  source: string | null;
  timeframe: string | null;
  all_passed: boolean | null;
  reason: string | null;
  symbols: IntradayRefreshSymbolStatus[];
  error: string | null;
}

// DATA-PROVIDER-GUI-FEED-SCHEDULER-01: latest closed-bar feed scheduler
export interface MarketDataFeedPollOnceRequest {
  provider_id: string;
  symbols: string[];
  timeframe: string;
  dry_run: boolean;
  allow_provider_api_calls?: boolean;
  now_utc?: string | null;
  provider_registry_path?: string | null;
}

export interface MarketDataFeedSchedulerStartRequest {
  provider_id: string;
  symbols: string[];
  timeframe: string;
  dry_run: boolean;
  allow_provider_api_calls?: boolean;
  poll_immediately: boolean;
  now_utc?: string | null;
  provider_registry_path?: string | null;
}

export interface MarketDataFeedPollSymbolResult {
  symbol: string;
  status: string;
  expected_latest_closed_bar_ts: number | null;
  returned_bar_ts: number | null;
  rows_inserted: number | null;
  rows_updated: number | null;
  rows_skipped: number | null;
  error: string | null;
}

export interface MarketDataFeedPollOnceResponse {
  canonical_route: string;
  truth_state: string;
  provider_id: string | null;
  timeframe: string | null;
  dry_run: boolean | null;
  provider_api_calls_allowed: boolean | null;
  symbols_count: number | null;
  poll_time_utc: string | null;
  latest_expected_closed_bar_ts: number | null;
  next_poll_ts: number | null;
  inserted_count: number | null;
  updated_count: number | null;
  skipped_count: number | null;
  error_count: number | null;
  api_calls_made: number | null;
  symbols: MarketDataFeedPollSymbolResult[];
  error: string | null;
}

export interface MarketDataFeedStatusResponse {
  canonical_route: string;
  truth_state: string;
  limitation: string | null;
  last_poll: MarketDataFeedPollOnceResponse | null;
}

export interface MarketDataFeedSchedulerStatusResponse {
  canonical_route: string;
  truth_state: string;
  limitation: string | null;
  running: boolean | null;
  provider_id: string | null;
  timeframe: string | null;
  symbols: string[];
  last_poll_utc: string | null;
  next_poll_utc: string | null;
  latest_expected_closed_bar_utc: string | null;
  last_result: MarketDataFeedPollOnceResponse | null;
  last_error: string | null;
  started_at_utc: string | null;
  stopped_at_utc: string | null;
  poll_count: number | null;
  inserted_count: number | null;
  unchanged_or_skipped_count: number | null;
  error_count: number | null;
}
