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

// ---------------------------------------------------------------------------
// CRYPTO-DATA-01Q-R-LATEST-MARK-GUI-SURFACE-BUNDLE-01-COMBINED:
// Latest-mark evidence status (read-only, ticker-only, not OHLCV/md_bars)
// ---------------------------------------------------------------------------

/**
 * One latest-mark row from GET /api/v1/market-data/latest-marks/status.
 * Mirrors LatestMarkStatusRow in api_types.rs. Intentionally has no
 * open/high/low/is_complete/end_ts field: this is a ticker-style latest
 * mark, never a completed OHLCV bar.
 */
export interface LatestMarkStatusMark {
  canonical_symbol: string;
  provider_id: string | null;
  provider_symbol: string | null;
  provider_coin_id: string | null;
  price_usd: string | null;
  volume24_usd: string | null;
  as_of_client_request_ts: number | null;
  provider_ts: number | null;
  truth_state: string | null;
  kind: string | null;
}

/**
 * Response for GET /api/v1/market-data/latest-marks/status.
 * Mirrors LatestMarkStatusResponse in api_types.rs.
 *
 * truth_state: "active" | "stale" | "no_evidence" | "parse_error" |
 * "unsafe_evidence" | "backend_unavailable". "unsafe_evidence" is a
 * fail-closed safety state, not a freshness state — never treat it as
 * usable data regardless of produced_at_utc.
 */
export interface LatestMarkStatusResponse {
  canonical_route: string;
  truth_state: string;
  provider: string | null;
  produced_at_utc: string | null;
  evidence_path: string | null;
  stale_or_missing_evidence: boolean;
  max_evidence_age_secs: number;
  network_call_made: boolean | null;
  db_write: boolean | null;
  md_bars_write: boolean | null;
  completed_bar_claim: boolean | null;
  provider_enabled: boolean | null;
  symbols_requested: string[];
  marks: LatestMarkStatusMark[];
  all_passed: boolean | null;
  reason_code: string | null;
  fail_reasons: string[];
  error: string | null;
}

// ---------------------------------------------------------------------------
// CRYPTO-DATA-01AE-KRAKEN-SYNC-GUI-STATUS-SURFACE-01:
// Kraken OHLC ingest/sync evidence status (read-only)
// ---------------------------------------------------------------------------

/**
 * Response for GET /api/v1/market-data/kraken-ohlc/status.
 * Mirrors KrakenOhlcStatusResponse in api_types.rs.
 *
 * truth_state: "active" | "stale" | "no_evidence" | "parse_error" |
 * "unsafe_evidence" | "backend_unavailable". "unsafe_evidence" is a
 * fail-closed safety state, not a freshness state — never treat it as
 * usable data regardless of produced_at_utc.
 *
 * Fields absent from the selected evidence file's own schema (e.g.
 * sync_policy when latest_mode="ingest") are null, never fabricated.
 */
export interface KrakenOhlcStatusResponse {
  canonical_route: string;
  truth_state: string;
  provider: string | null;
  latest_mode: string | null;
  latest_schema_version: string | null;
  produced_at_utc: string | null;
  evidence_path: string | null;
  stale_or_missing_evidence: boolean;
  max_evidence_age_secs: number;
  network_call_made: boolean | null;
  db_write: boolean | null;
  md_bars_write: boolean | null;
  provider_id: string | null;
  provider_source: string | null;
  provider_symbol: string | null;
  ingest_mode: string | null;
  sync_policy: string | null;
  no_update_existing: boolean | null;
  symbols_requested: string[];
  bars_completed: number | null;
  bars_excluded_forming: number | null;
  bars_considered_for_sync: number | null;
  bars_missing_new: number | null;
  bars_existing_candidate: number | null;
  rows_changed: number | null;
  rows_skipped_unchanged: number | null;
  rows_changed_skipped_due_to_no_update_existing: number | null;
  rows_inserted: number | null;
  rows_updated: number | null;
  rows_skipped_if_known: number | null;
  latest_existing_end_ts_before: number | null;
  latest_completed_start_ts: number | null;
  latest_completed_end_ts: number | null;
  volume_semantics: string | null;
  volume_scale: number | null;
  all_passed: boolean | null;
  reason_code: string | null;
  fail_reasons: string[];
  error: string | null;
}

// ---------------------------------------------------------------------------
// CRYPTO-REGISTRY-04-KRAKEN-DATA-ONLY-REGISTRY-STATUS-SURFACE-01:
// Crypto registry readiness (read-only)
// ---------------------------------------------------------------------------

/**
 * Per-symbol readiness check. Mirrors CryptoRegistrySymbolReadiness in
 * api_types.rs. `enabled`/`paper_trading_enabled`/`live_trading_enabled` are
 * `null` only when the symbol was not found in the registry at all
 * (distinct from `false`).
 */
export interface CryptoRegistrySymbolReadiness {
  symbol: string;
  found: boolean;
  asset_class_ok: boolean;
  kraken_pair: string | null;
  kraken_result_key: string | null;
  alias_ok: boolean;
  enabled: boolean | null;
  paper_trading_enabled: boolean | null;
  live_trading_enabled: boolean | null;
  trading_flags_safe: boolean;
  passed: boolean;
}

/**
 * Fixed safety flags, mirroring CryptoRegistryReadinessSafety in
 * api_types.rs. Present as explicit fields rather than inferred so the GUI
 * never has to assume what this route did not do.
 */
export interface CryptoRegistryReadinessSafety {
  no_scheduler: boolean;
  no_db_connection: boolean;
  no_network_call: boolean;
  no_trading_enabled: boolean;
  no_config_file_mutated: boolean;
}

/**
 * Response for GET /api/v1/market-data/crypto-registry/readiness.
 * Mirrors CryptoRegistryReadinessResponse in api_types.rs.
 *
 * truth_state: "active" | "missing_provider" | "missing_symbol" |
 * "missing_alias" | "unsafe_trading_enabled" | "unsafe_provider_enabled" |
 * "parse_error".
 *
 * data_readiness_state is "data_ready_manual_only" (not a failure) when all
 * checks pass and every symbol's `enabled` is false; "blocked" when any
 * check fails. trading_readiness_state is always "disabled" and
 * scheduler_readiness_state is always "absent" — this route has no branch
 * that could ever report otherwise.
 */
export interface CryptoRegistryReadinessResponse {
  canonical_route: string;
  truth_state: string;
  data_readiness_state: string;
  trading_readiness_state: string;
  scheduler_readiness_state: string;
  provider: string;
  provider_enabled: boolean;
  api_key_required: boolean;
  provider_asset_classes: string[];
  provider_implementation_status: string;
  registry_path: string;
  providers_path: string;
  symbols: CryptoRegistrySymbolReadiness[];
  all_passed: boolean;
  reason_code: string;
  fail_reasons: string[];
  safety: CryptoRegistryReadinessSafety;
}

// ---------------------------------------------------------------------------
// CRYPTO-DATA-02C-KRAKEN-SCHEDULER-READINESS-STATUS-SURFACE-01:
// Kraken scheduler readiness (read-only)
// ---------------------------------------------------------------------------

/**
 * Per-symbol readiness check. Mirrors KrakenSchedulerSymbolReadiness in
 * api_types.rs. `enabled`/`paper_trading_enabled`/`live_trading_enabled` are
 * `null` only when the symbol was not found in the registry at all
 * (distinct from `false`).
 */
export interface KrakenSchedulerSymbolReadiness {
  symbol: string;
  found: boolean;
  asset_class_ok: boolean;
  alias_ok: boolean;
  enabled: boolean | null;
  paper_trading_enabled: boolean | null;
  live_trading_enabled: boolean | null;
  trading_flags_safe: boolean;
}

/**
 * Fixed safety flags, mirroring KrakenSchedulerReadinessSafety in
 * api_types.rs. Present as explicit fields rather than inferred so the GUI
 * never has to assume what this route did not do.
 */
export interface KrakenSchedulerReadinessSafety {
  no_scheduled_task_registered: boolean;
  no_daemon_job_added: boolean;
  no_network_call_made: boolean;
  no_db_connection: boolean;
  no_trading_enabled: boolean;
  no_config_file_mutated: boolean;
}

/**
 * Response for GET /api/v1/market-data/kraken-scheduler/readiness. Mirrors
 * KrakenSchedulerReadinessResponse in api_types.rs.
 *
 * truth_state: "active" | "policy_missing" | "policy_invalid" |
 * "registry_unsafe" | "provider_unsafe" | "trading_flags_unsafe" |
 * "scheduler_already_registered" | "evidence_unsafe" | "parse_error" |
 * "backend_unavailable".
 *
 * "active" (scheduler_readiness_state =
 * "scheduler_ready_manual_registration_blocked") never means a scheduler is
 * registered — it means every prerequisite this route can check is
 * satisfied for a future, separately authorized scheduler-registration
 * patch to be considered.
 */
export interface KrakenSchedulerReadinessResponse {
  canonical_route: string;
  truth_state: string;
  scheduler_readiness_state: string;
  rate_limit_policy_state: string;
  registry_readiness_state: string;
  provider_readiness_state: string;
  evidence_readiness_state: string;
  provider: string;
  provider_enabled: boolean;
  symbols: KrakenSchedulerSymbolReadiness[];
  recommended_default_cadence: string;
  min_seconds_between_pair_calls: number;
  min_seconds_between_scheduled_runs: number;
  max_ohlc_calls_per_run: number;
  max_total_network_calls_per_run: number;
  concurrency: string;
  scheduler_registration_status: string;
  daemon_job_status: string;
  network_call_made: boolean;
  db_write: boolean;
  trading_enabled: boolean;
  policy_path: string;
  registry_path: string;
  providers_path: string;
  all_passed: boolean;
  reason_code: string;
  fail_reasons: string[];
  warnings: string[];
  safety: KrakenSchedulerReadinessSafety;
}

// ---------------------------------------------------------------------------
// CRYPTO-DATA-03C-KRAKEN-SCHEDULER-TASK-STATUS-SURFACE-01:
// Kraken scheduled task status (read-only)
// ---------------------------------------------------------------------------

/**
 * The `safety` block embedded inside `kraken_ohlc_task_registration.json`
 * itself (written by Register-KrakenOhlcSyncTask.ps1), mirrored field-for-
 * field. These are the evidence producer's own claims, passed through
 * verbatim -- not a route-authored assertion.
 */
export interface KrakenSchedulerTaskStatusEvidenceSafety {
  calls_runner_script_only: boolean | null;
  no_daemon_runtime_broker_provider_order_references: boolean | null;
  no_env_vars_embedded_in_task_action: boolean | null;
  no_env_local_read: boolean | null;
  task_never_started_by_this_script: boolean | null;
}

/**
 * Response for GET /api/v1/market-data/kraken-scheduler/task-status. Mirrors
 * KrakenSchedulerTaskStatusResponse in api_types.rs.
 *
 * truth_state: "active" | "no_evidence" | "parse_error" | "unsafe_evidence" |
 * "backend_unavailable".
 *
 * "active" never means a task is registered -- see `registered` /
 * `task_exists_after` for that distinct, truthfully-surfaced fact. Every
 * field except canonical_route/truth_state/symbols/env_vars_embedded/
 * env_vars_required/fail_reasons/warnings is `null` when the evidence file
 * did not carry that field, or when no evidence could be safely read at
 * all -- never fabricated or defaulted to a "safe-looking" value.
 */
export interface KrakenSchedulerTaskStatusResponse {
  canonical_route: string;
  truth_state: string;
  schema_version: string | null;
  produced_at_utc: string | null;
  mode: string | null;
  task_name: string | null;
  task_exists_before: boolean | null;
  task_exists_after: boolean | null;
  registered: boolean | null;
  unregistered: boolean | null;
  check_only: boolean | null;
  task_action: string | null;
  runner_path: string | null;
  policy_path: string | null;
  registry_path: string | null;
  providers_path: string | null;
  symbols: string[];
  timeframe: string | null;
  at: string | null;
  scheduled_task_mutation: boolean | null;
  network_call_made: boolean | null;
  db_write: boolean | null;
  md_bars_write: boolean | null;
  env_vars_embedded: string[];
  env_vars_required: string[];
  all_passed: boolean | null;
  reason_code: string | null;
  fail_reasons: string[];
  warnings: string[];
  safety: KrakenSchedulerTaskStatusEvidenceSafety | null;
  evidence_path: string | null;
  error: string | null;
}
