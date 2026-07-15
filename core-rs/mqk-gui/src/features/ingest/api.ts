// DATA-INGEST-GUI-RUNNER-01: Ingest job HTTP helpers.
//
// Safety invariants:
// - Only calls /api/v1/ingest/* routes. No live/paper execution routes.
// - POST submit uses privileged:true (operator auth token). GET is public.
// - Pure helpers (normalizeIngestJobStatus, isTerminalIngestStatus,
//   buildActiveIngestJob) are exported for test isolation.

import { fetchJsonCandidate, postJson } from "../system/http";
import { getDaemonUrl } from "../../config";
import { getDesktopOperatorToken, isDesktopShell } from "../../desktop/bootstrap";
import type {
  ActiveIngestJob,
  ActiveProviderJob,
  CancelIngestJobResponse,
  CryptoRegistryReadinessResponse,
  DailyDataReadinessAssignmentResponse,
  DailyDataReadinessResponse,
  IngestJobAcceptedResponse,
  IngestJobRequest,
  IngestJobStatusKind,
  IngestJobStatusResponse,
  IngestJobsListResponse,
  IntradayRefreshStatusResponse,
  KrakenOhlcStatusResponse,
  KrakenSchedulerReadinessResponse,
  KrakenSchedulerTaskStatusResponse,
  LatestMarkStatusResponse,
  MarketDataFeedPollOnceRequest,
  MarketDataFeedPollOnceResponse,
  MarketDataFeedPollSymbolResult,
  MarketDataFeedSchedulerStartRequest,
  MarketDataFeedSchedulerStatusResponse,
  MarketDataFeedStatusResponse,
  MdBarsCoverageResponse,
  MdBarsCoverageRow,
  TrackedEquitiesResponse,
} from "./types";

// ---------------------------------------------------------------------------
// Pure helpers — testable without HTTP
// ---------------------------------------------------------------------------

export function normalizeIngestJobStatus(raw: string): IngestJobStatusKind {
  switch (raw) {
    case "queued":
    case "running":
    case "completed":
    case "dry_run_completed":
    case "partial":
    case "refused":
    case "cancelled":
    case "failed":
      return raw;
    default:
      return "unknown";
  }
}

export function isTerminalIngestStatus(status: IngestJobStatusKind): boolean {
  return (
    status === "completed" ||
    status === "dry_run_completed" ||
    status === "partial" ||
    status === "refused" ||
    status === "cancelled" ||
    status === "failed"
  );
}

export function isCancellableIngestStatus(status: IngestJobStatusKind): boolean {
  return status === "queued" || status === "running";
}

export function extractIngestRowCounts(
  response: IngestJobStatusResponse,
): { rowsRead: number | null; rowsInserted: number | null; rowsRejected: number | null } | null {
  if (response.status !== "completed") return null;
  return {
    rowsRead: response.rows_read ?? null,
    rowsInserted: response.rows_inserted ?? null,
    rowsRejected: response.rows_rejected ?? null,
  };
}

export function buildActiveIngestJob(response: IngestJobStatusResponse): ActiveIngestJob {
  return {
    jobId: response.job_id,
    source: response.source,
    timeframe: response.timeframe,
    csvPath: response.csv_path ?? null,
    createdAt: response.created_at_utc,
    startedAt: response.started_at_utc ?? null,
    completedAt: response.completed_at_utc ?? null,
    status: normalizeIngestJobStatus(response.status),
    rowsRead: response.rows_read ?? null,
    rowsInserted: response.rows_inserted ?? null,
    rowsRejected: response.rows_rejected ?? null,
    qualityReportPath: response.quality_report_path ?? null,
    error: response.error ?? null,
  };
}

// ---------------------------------------------------------------------------
// DATA-INGEST-GUI-PROVIDER-RUNNER-01: Provider sync pure helpers
// ---------------------------------------------------------------------------

/**
 * Guard: is the operator allowed to submit with these settings?
 *
 * - Dry-run (allow_provider_api_calls=false): always allowed, no confirmation needed.
 * - Real sync (allow_provider_api_calls=true): requires the operator to type "SYNC".
 */
export function isProviderSyncAllowed(
  allowProviderApiCalls: boolean,
  syncConfirmation: string,
): boolean {
  if (!allowProviderApiCalls) return true;
  return syncConfirmation.trim() === "SYNC";
}

/**
 * Build a canonical provider job request with safe defaults.
 * Caller controls dryRun and allowProviderApiCalls; all other fields use
 * the project-standard values (source=twelvedata, mode=sync_provider, registry).
 */
export function buildProviderJobRequest(opts: {
  dryRun: boolean;
  allowProviderApiCalls: boolean;
  start?: string | null;
  end?: string | null;
  apiCreditsPerMinute?: number | null;
  apiCreditsPerDay?: number | null;
}): IngestJobRequest {
  return {
    source: "twelvedata",
    mode: "sync_provider",
    timeframe: "1D",
    symbols_source: "registry",
    registry_path: "config/instruments/equities.json",
    asset_class: "equity",
    dry_run: opts.dryRun,
    allow_provider_api_calls: opts.allowProviderApiCalls,
    start: opts.start ?? null,
    end: opts.end ?? null,
    api_credits_per_minute: opts.apiCreditsPerMinute ?? null,
    api_credits_per_day: opts.apiCreditsPerDay ?? null,
  };
}

/**
 * Map a polled IngestJobStatusResponse to an ActiveProviderJob.
 * Used on every poll tick to update the in-flight job state.
 */
export function buildActiveProviderJob(
  response: IngestJobStatusResponse,
): ActiveProviderJob {
  return {
    jobId: response.job_id,
    status: normalizeIngestJobStatus(response.status),
    dryRun: response.dry_run,
    allowProviderApiCalls: response.provider_api_calls_allowed,
    createdAt: response.created_at_utc,
    startedAt: response.started_at_utc ?? null,
    completedAt: response.completed_at_utc ?? null,
    error: response.error ?? null,
    apiCallsMade: response.api_calls_made,
    symbolsCount: response.symbols_count ?? null,
    symbolsCompleted: response.symbols_completed ?? null,
    symbolsFailed: response.symbols_failed ?? null,
    rowsInserted: response.rows_inserted ?? null,
    rowsRejected: response.rows_rejected ?? null,
    plannedFirstSymbol: response.planned_first_symbol ?? null,
    plannedLastSymbol: response.planned_last_symbol ?? null,
  };
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

export interface SubmitIngestJobResult {
  ok: boolean;
  status?: number;
  data?: IngestJobAcceptedResponse;
  error?: string;
}

export interface GetIngestJobResult {
  ok: boolean;
  status?: number;
  data?: IngestJobStatusResponse;
  error?: string;
  notFound?: boolean;
}

export interface CancelIngestJobResult {
  ok: boolean;
  status?: number;
  data?: CancelIngestJobResponse;
  error?: string;
  notFound?: boolean;
}

export interface ListIngestJobsResult {
  ok: boolean;
  data?: IngestJobsListResponse;
  error?: string;
}

export async function submitIngestJob(req: IngestJobRequest): Promise<SubmitIngestJobResult> {
  const result = await postJson<IngestJobAcceptedResponse>(
    ["/api/v1/ingest/jobs"],
    req as unknown as Record<string, unknown>,
    { privileged: true },
  );

  if (result.error === "desktop operator token missing") {
    return {
      ok: false,
      error:
        "Operator token missing — configure MQK_OPERATOR_TOKEN and launch via Launch-VeritasLedger.ps1 to submit ingest jobs.",
    };
  }

  if (!result.ok) {
    if (result.status === 401 || result.status === 403) {
      return {
        ok: false,
        status: result.status,
        error: "Operator auth required. Set MQK_OPERATOR_TOKEN and launch the daemon with that token configured.",
      };
    }
    if (result.status === 404) {
      return {
        ok: false,
        status: 404,
        error: "Ingest job API unavailable (route not found). Is the daemon running with ingest routes?",
      };
    }
    return { ok: false, status: result.status, error: result.error ?? "Submission failed." };
  }

  return { ok: true, status: result.status, data: result.data };
}

export async function getIngestJob(jobId: string): Promise<GetIngestJobResult> {
  const result = await fetchJsonCandidate<IngestJobStatusResponse>(
    `/api/v1/ingest/jobs/${jobId}`,
  );

  if (!result.ok) {
    const isNotFound = result.error === "HTTP 404";
    return {
      ok: false,
      error: isNotFound
        ? "Ingest job not found."
        : (result.error ?? "Status fetch failed."),
      notFound: isNotFound,
    };
  }

  return { ok: true, data: result.data };
}

export async function cancelIngestJob(jobId: string): Promise<CancelIngestJobResult> {
  const result = await postJson<CancelIngestJobResponse>(
    [`/api/v1/ingest/jobs/${jobId}/cancel`],
    {},
    { privileged: true },
  );

  if (result.error === "desktop operator token missing") {
    return {
      ok: false,
      error:
        "Operator token missing — configure MQK_OPERATOR_TOKEN and launch via Launch-VeritasLedger.ps1 to cancel ingest jobs.",
    };
  }

  if (!result.ok) {
    if (result.status === 401 || result.status === 403) {
      return {
        ok: false,
        status: result.status,
        error: "Operator auth required. Set MQK_OPERATOR_TOKEN and launch the daemon with that token configured.",
      };
    }
    if (result.status === 404) {
      return {
        ok: false,
        status: 404,
        error: "Ingest job not found.",
        notFound: true,
      };
    }
    if (result.status === 503) {
      return {
        ok: false,
        status: 503,
        error: "Ingest job cancel backend unavailable.",
      };
    }
    return { ok: false, status: result.status, error: result.error ?? "Cancel failed." };
  }

  return { ok: true, status: result.status, data: result.data };
}

export async function listIngestJobs(): Promise<ListIngestJobsResult> {
  const result = await fetchJsonCandidate<IngestJobsListResponse>("/api/v1/ingest/jobs");

  if (!result.ok) {
    return { ok: false, error: result.error ?? "List fetch failed." };
  }

  return { ok: true, data: result.data };
}

// ---------------------------------------------------------------------------
// DATA-INGEST-GUI-RESULTS-01: md_bars coverage
// ---------------------------------------------------------------------------

export interface FetchCoverageResult {
  ok: boolean;
  data?: MdBarsCoverageResponse;
  error?: string;
}

/**
 * Fetch coverage summary from GET /api/v1/market-data/coverage.
 * Read-only. No operator auth required. No provider calls. No DB writes.
 *
 * @param timeframe - Optional filter ("1D", "1m", "5m"). Omit for all timeframes.
 */
export async function fetchMdBarsCoverage(
  timeframe?: string,
): Promise<FetchCoverageResult> {
  const url = timeframe
    ? `/api/v1/market-data/coverage?timeframe=${encodeURIComponent(timeframe)}`
    : `/api/v1/market-data/coverage`;

  const result = await fetchJsonCandidate<MdBarsCoverageResponse>(url);

  if (!result.ok) {
    return { ok: false, error: result.error ?? "Coverage fetch failed." };
  }

  return { ok: true, data: result.data };
}

/**
 * Format a Unix-second timestamp as a short date string (YYYY-MM-DD).
 * Returns "—" for zero or falsy values.
 */
export function formatEndTs(endTs: number | null | undefined): string {
  if (!endTs) return "—";
  const d = new Date(endTs * 1000);
  if (isNaN(d.getTime())) return "—";
  return d.toISOString().slice(0, 10);
}

/**
 * Return a human-readable truth-state label for the coverage response.
 */
export function coverageTruthLabel(truthState: string): string {
  switch (truthState) {
    case "active":
      return "active";
    case "empty":
      return "no data";
    case "db_unavailable":
      return "db unavailable";
    case "unavailable":
      return "unavailable";
    default:
      return truthState;
  }
}

/**
 * Return true if the coverage truth_state means "data is present and valid".
 */
export function isCoverageActive(truthState: string): boolean {
  return truthState === "active";
}

// ---------------------------------------------------------------------------
// DATA-INGEST-GUI-SYNC-ALL-01: Tracked-equities registry preview
// ---------------------------------------------------------------------------

export interface FetchTrackedEquitiesResult {
  ok: boolean;
  data?: TrackedEquitiesResponse;
  error?: string;
}

/**
 * Return true if the tracked-equities truth_state means "registry loaded successfully".
 *
 * Safety: registry_unavailable and registry_invalid are explicitly not active.
 */
export function isTrackedEquitiesActive(truthState: string): boolean {
  return truthState === "active";
}

/**
 * Return a human-readable label for a tracked-equities truth_state.
 */
export function trackedEquitiesTruthLabel(truthState: string): string {
  switch (truthState) {
    case "active":
      return "active";
    case "registry_unavailable":
      return "registry unavailable";
    case "registry_invalid":
      return "registry invalid";
    default:
      return truthState;
  }
}

/**
 * Fetch GET /api/v1/ingest/tracked-equities.
 *
 * Safety invariants:
 * - No provider API calls. No DB writes. No execution state touched.
 * - Read-only access to the instrument registry file on the daemon host.
 * - No API credits consumed.
 */
export async function fetchTrackedEquities(): Promise<FetchTrackedEquitiesResult> {
  const result = await fetchJsonCandidate<TrackedEquitiesResponse>(
    "/api/v1/ingest/tracked-equities",
  );

  if (!result.ok) {
    return { ok: false, error: result.error ?? "Tracked-equities fetch failed." };
  }

  return { ok: true, data: result.data };
}

// ---------------------------------------------------------------------------
// INTRADAY-MD-REFRESHER-GUI-01: Intraday refresh status
// ---------------------------------------------------------------------------

export interface FetchIntradayRefreshResult {
  ok: boolean;
  data?: IntradayRefreshStatusResponse;
  error?: string;
}

export function isIntradayRefreshActive(truthState: string): boolean {
  return truthState === "active";
}

export function intradayRefreshTruthLabel(truthState: string): string {
  switch (truthState) {
    case "active":
      return "active";
    case "no_evidence":
      return "no evidence";
    case "parse_error":
      return "parse error";
    case "backend_unavailable":
      return "unavailable";
    default:
      return truthState;
  }
}

/**
 * Fetch GET /api/v1/market-data/intraday-refresh/status.
 *
 * Safety: Read-only. No DB writes. No provider calls. No API credits consumed.
 */
export async function fetchIntradayRefreshStatus(): Promise<FetchIntradayRefreshResult> {
  const result = await fetchJsonCandidate<IntradayRefreshStatusResponse>(
    "/api/v1/market-data/intraday-refresh/status",
  );

  if (!result.ok) {
    return { ok: false, error: result.error ?? "Intraday refresh status fetch failed." };
  }

  return { ok: true, data: result.data };
}

// ---------------------------------------------------------------------------
// DATA-PROVIDER-GUI-FEED-SCHEDULER-01: latest closed-bar feed scheduler
// ---------------------------------------------------------------------------

export interface MarketDataFeedActionResult<T> {
  ok: boolean;
  status?: number;
  data?: T;
  error?: string;
}

export interface MarketDataFeedRequestOptions {
  providerId: string;
  symbols: string[];
  timeframe: string;
  dryRun: boolean;
  allowProviderApiCalls: boolean;
  pollImmediately?: boolean;
}

function normalizeNullableString(value: unknown): string | null {
  return typeof value === "string" && value.trim() !== "" ? value : null;
}

function normalizeString(value: unknown, fallback: string): string {
  return typeof value === "string" && value.trim() !== "" ? value : fallback;
}

function normalizeNullableNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function normalizeNullableBoolean(value: unknown): boolean | null {
  return typeof value === "boolean" ? value : null;
}

function normalizeStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((entry): entry is string => typeof entry === "string");
}

function normalizePollSymbolResult(raw: unknown): MarketDataFeedPollSymbolResult {
  const record = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  return {
    symbol: normalizeString(record.symbol, "unknown"),
    status: normalizeString(record.status, "unknown"),
    expected_latest_closed_bar_ts: normalizeNullableNumber(record.expected_latest_closed_bar_ts),
    returned_bar_ts: normalizeNullableNumber(record.returned_bar_ts),
    rows_inserted: normalizeNullableNumber(record.rows_inserted),
    rows_updated: normalizeNullableNumber(record.rows_updated),
    rows_skipped: normalizeNullableNumber(record.rows_skipped),
    error: normalizeNullableString(record.error),
  };
}

export function normalizeMarketDataFeedPollOnceResponse(
  raw: unknown,
): MarketDataFeedPollOnceResponse {
  const record = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const symbolsRaw = Array.isArray(record.symbols) ? record.symbols : [];
  return {
    canonical_route: normalizeString(record.canonical_route, "/api/v1/market-data/feed/poll-once"),
    truth_state: normalizeString(record.truth_state, "unknown"),
    provider_id: normalizeNullableString(record.provider_id),
    timeframe: normalizeNullableString(record.timeframe),
    dry_run: normalizeNullableBoolean(record.dry_run),
    provider_api_calls_allowed: normalizeNullableBoolean(record.provider_api_calls_allowed),
    symbols_count: normalizeNullableNumber(record.symbols_count),
    poll_time_utc: normalizeNullableString(record.poll_time_utc),
    latest_expected_closed_bar_ts: normalizeNullableNumber(record.latest_expected_closed_bar_ts),
    next_poll_ts: normalizeNullableNumber(record.next_poll_ts),
    inserted_count: normalizeNullableNumber(record.inserted_count),
    updated_count: normalizeNullableNumber(record.updated_count),
    skipped_count: normalizeNullableNumber(record.skipped_count),
    error_count: normalizeNullableNumber(record.error_count),
    api_calls_made: normalizeNullableNumber(record.api_calls_made),
    symbols: symbolsRaw.map(normalizePollSymbolResult),
    error: normalizeNullableString(record.error),
  };
}

export function normalizeMarketDataFeedStatusResponse(raw: unknown): MarketDataFeedStatusResponse {
  const record = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  return {
    canonical_route: normalizeString(record.canonical_route, "/api/v1/market-data/feed/status"),
    truth_state: normalizeString(record.truth_state, "unknown"),
    limitation: normalizeNullableString(record.limitation),
    last_poll: record.last_poll === null || record.last_poll === undefined
      ? null
      : normalizeMarketDataFeedPollOnceResponse(record.last_poll),
  };
}

export function normalizeMarketDataFeedSchedulerStatusResponse(
  raw: unknown,
): MarketDataFeedSchedulerStatusResponse {
  const record = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  return {
    canonical_route: normalizeString(
      record.canonical_route,
      "/api/v1/market-data/feed/scheduler/status",
    ),
    truth_state: normalizeString(record.truth_state, "unknown"),
    limitation: normalizeNullableString(record.limitation),
    running: normalizeNullableBoolean(record.running),
    provider_id: normalizeNullableString(record.provider_id),
    timeframe: normalizeNullableString(record.timeframe),
    symbols: normalizeStringArray(record.symbols),
    last_poll_utc: normalizeNullableString(record.last_poll_utc),
    next_poll_utc: normalizeNullableString(record.next_poll_utc),
    latest_expected_closed_bar_utc: normalizeNullableString(record.latest_expected_closed_bar_utc),
    last_result: record.last_result === null || record.last_result === undefined
      ? null
      : normalizeMarketDataFeedPollOnceResponse(record.last_result),
    last_error: normalizeNullableString(record.last_error),
    started_at_utc: normalizeNullableString(record.started_at_utc),
    stopped_at_utc: normalizeNullableString(record.stopped_at_utc),
    poll_count: normalizeNullableNumber(record.poll_count),
    inserted_count: normalizeNullableNumber(record.inserted_count),
    unchanged_or_skipped_count: normalizeNullableNumber(record.unchanged_or_skipped_count),
    error_count: normalizeNullableNumber(record.error_count),
  };
}

export function parseMarketDataFeedSymbols(value: string): string[] {
  const unique = new Set<string>();
  for (const symbol of value.split(/[,\s]+/)) {
    const normalized = symbol.trim().toUpperCase();
    if (normalized) unique.add(normalized);
  }
  return [...unique];
}

export function isMarketDataFeedRealActionAllowed(
  allowProviderApiCalls: boolean,
  confirmation: string,
  expected: "POLL" | "START",
): boolean {
  if (!allowProviderApiCalls) return true;
  return confirmation.trim() === expected;
}

export function buildMarketDataFeedPollOnceRequest(
  opts: MarketDataFeedRequestOptions,
): MarketDataFeedPollOnceRequest {
  const req: MarketDataFeedPollOnceRequest = {
    provider_id: opts.providerId.trim(),
    symbols: opts.symbols,
    timeframe: opts.timeframe.trim(),
    dry_run: opts.dryRun,
  };
  if (!opts.dryRun && opts.allowProviderApiCalls) {
    req.allow_provider_api_calls = true;
  }
  return req;
}

export function buildMarketDataFeedSchedulerStartRequest(
  opts: MarketDataFeedRequestOptions,
): MarketDataFeedSchedulerStartRequest {
  const req: MarketDataFeedSchedulerStartRequest = {
    provider_id: opts.providerId.trim(),
    symbols: opts.symbols,
    timeframe: opts.timeframe.trim(),
    dry_run: opts.dryRun,
    poll_immediately: opts.pollImmediately === true,
  };
  if (!opts.dryRun && opts.allowProviderApiCalls) {
    req.allow_provider_api_calls = true;
  }
  return req;
}

function marketDataFeedPostError(status: number | undefined, fallback: string): string {
  if (fallback && !/^HTTP \d+$/.test(fallback)) return fallback;
  if (status === 400) return "Request refused by daemon. Check provider, symbols, timeframe, and provider-call allowance.";
  if (status === 401 || status === 403) return "Operator auth required. Set MQK_OPERATOR_TOKEN and launch the daemon with that token configured.";
  if (status === 404) return "Market-data feed route unavailable. Is the daemon running with feed scheduler routes?";
  if (status === 409) return "Latest-bar scheduler is already running.";
  if (status === 503) return "Market-data feed backend unavailable.";
  return fallback;
}

function marketDataFeedBodyError(raw: unknown): string | null {
  if (!raw || typeof raw !== "object") return null;
  const record = raw as Record<string, unknown>;
  return normalizeNullableString(record.error) ?? normalizeNullableString(record.last_error);
}

async function postMarketDataFeedJson<T>(
  path: string,
  body: Record<string, unknown>,
): Promise<MarketDataFeedActionResult<T>> {
  const desktopOperatorToken = getDesktopOperatorToken();
  if (isDesktopShell() && !desktopOperatorToken) {
    return { ok: false, error: "desktop operator token missing" };
  }

  try {
    const headers: Record<string, string> = {
      Accept: "application/json",
      "Content-Type": "application/json",
    };
    if (desktopOperatorToken) {
      headers.Authorization = `Bearer ${desktopOperatorToken}`;
    }

    const response = await fetch(new URL(path, getDaemonUrl()).toString(), {
      method: "POST",
      headers,
      body: JSON.stringify(body),
    });
    const contentType = response.headers.get("content-type") ?? "";
    const data = contentType.includes("application/json")
      ? ((await response.json()) as T)
      : undefined;

    if (!response.ok) {
      return {
        ok: false,
        status: response.status,
        data,
        error: marketDataFeedBodyError(data) ?? `HTTP ${response.status}`,
      };
    }

    return { ok: true, status: response.status, data };
  } catch (error) {
    return {
      ok: false,
      error: error instanceof Error ? error.message : "unknown error",
    };
  }
}

export async function getMarketDataFeedStatus(): Promise<
  MarketDataFeedActionResult<MarketDataFeedStatusResponse>
> {
  const result = await fetchJsonCandidate<unknown>("/api/v1/market-data/feed/status");
  if (!result.ok) {
    return { ok: false, error: result.error ?? "Feed status fetch failed." };
  }
  return { ok: true, data: normalizeMarketDataFeedStatusResponse(result.data) };
}

export async function pollMarketDataFeedOnce(
  req: MarketDataFeedPollOnceRequest,
): Promise<MarketDataFeedActionResult<MarketDataFeedPollOnceResponse>> {
  const result = await postMarketDataFeedJson<unknown>(
    "/api/v1/market-data/feed/poll-once",
    req as unknown as Record<string, unknown>,
  );

  if (result.error === "desktop operator token missing") {
    return {
      ok: false,
      error:
        "Operator token missing — configure MQK_OPERATOR_TOKEN and launch via Launch-VeritasLedger.ps1 to poll latest bars.",
    };
  }
  if (!result.ok) {
    const data = result.data === undefined
      ? undefined
      : normalizeMarketDataFeedPollOnceResponse(result.data);
    return {
      ok: false,
      status: result.status,
      data,
      error: marketDataFeedPostError(result.status, result.error ?? "Feed poll failed."),
    };
  }
  return { ok: true, status: result.status, data: normalizeMarketDataFeedPollOnceResponse(result.data) };
}

export async function getMarketDataFeedSchedulerStatus(): Promise<
  MarketDataFeedActionResult<MarketDataFeedSchedulerStatusResponse>
> {
  const result = await fetchJsonCandidate<unknown>("/api/v1/market-data/feed/scheduler/status");
  if (!result.ok) {
    return { ok: false, error: result.error ?? "Feed scheduler status fetch failed." };
  }
  return { ok: true, data: normalizeMarketDataFeedSchedulerStatusResponse(result.data) };
}

export async function startMarketDataFeedScheduler(
  req: MarketDataFeedSchedulerStartRequest,
): Promise<MarketDataFeedActionResult<MarketDataFeedSchedulerStatusResponse>> {
  const result = await postMarketDataFeedJson<unknown>(
    "/api/v1/market-data/feed/scheduler/start",
    req as unknown as Record<string, unknown>,
  );

  if (result.error === "desktop operator token missing") {
    return {
      ok: false,
      error:
        "Operator token missing — configure MQK_OPERATOR_TOKEN and launch via Launch-VeritasLedger.ps1 to start the latest-bar scheduler.",
    };
  }
  if (!result.ok) {
    const data = result.data === undefined
      ? undefined
      : normalizeMarketDataFeedSchedulerStatusResponse(result.data);
    return {
      ok: false,
      status: result.status,
      data,
      error: marketDataFeedPostError(result.status, result.error ?? "Feed scheduler start failed."),
    };
  }
  return { ok: true, status: result.status, data: normalizeMarketDataFeedSchedulerStatusResponse(result.data) };
}

export async function stopMarketDataFeedScheduler(): Promise<
  MarketDataFeedActionResult<MarketDataFeedSchedulerStatusResponse>
> {
  const result = await postMarketDataFeedJson<unknown>(
    "/api/v1/market-data/feed/scheduler/stop",
    {},
  );

  if (result.error === "desktop operator token missing") {
    return {
      ok: false,
      error:
        "Operator token missing — configure MQK_OPERATOR_TOKEN and launch via Launch-VeritasLedger.ps1 to stop the latest-bar scheduler.",
    };
  }
  if (!result.ok) {
    const data = result.data === undefined
      ? undefined
      : normalizeMarketDataFeedSchedulerStatusResponse(result.data);
    return {
      ok: false,
      status: result.status,
      data,
      error: marketDataFeedPostError(result.status, result.error ?? "Feed scheduler stop failed."),
    };
  }
  return { ok: true, status: result.status, data: normalizeMarketDataFeedSchedulerStatusResponse(result.data) };
}

// ---------------------------------------------------------------------------
// DATA-INGEST-GUI-COVERAGE-POLISH-01: Coverage freshness, sort, filter, summary
// ---------------------------------------------------------------------------

export const COVERAGE_FRESHNESS_THRESHOLD_1D_SECS = 345600; // 4 days (handles weekends + holidays)
export const COVERAGE_FRESHNESS_THRESHOLD_INTRADAY_SECS = 900; // 15 min

export type CoverageFreshness = "fresh" | "stale" | "unknown";

/**
 * Return the freshness threshold in seconds for a timeframe, or null for unknown timeframes.
 */
export function coverageFreshnessThresholdSecs(timeframe: string): number | null {
  if (timeframe === "1D") return COVERAGE_FRESHNESS_THRESHOLD_1D_SECS;
  if (timeframe === "1m" || timeframe === "5m") return COVERAGE_FRESHNESS_THRESHOLD_INTRADAY_SECS;
  return null;
}

/**
 * Classify a coverage row as fresh/stale/unknown.
 * - unknown: timeframe unrecognized, or maxEndTs is zero/null.
 * - fresh: (nowSecs - maxEndTs) <= threshold.
 * - stale: (nowSecs - maxEndTs) > threshold.
 */
export function classifyCoverageFreshness(
  maxEndTs: number | null | undefined,
  nowSecs: number,
  timeframe: string,
): CoverageFreshness {
  if (!maxEndTs) return "unknown";
  const threshold = coverageFreshnessThresholdSecs(timeframe);
  if (threshold === null) return "unknown";
  const ageSecs = nowSecs - maxEndTs;
  return ageSecs <= threshold ? "fresh" : "stale";
}

export type CoverageSortMode =
  | "symbol_asc"
  | "symbol_desc"
  | "bars_desc"
  | "bars_asc"
  | "latest_desc"
  | "latest_asc";

/**
 * Sort coverage rows by the given mode. Ties broken by symbol ascending for determinism.
 */
export function sortCoverageRows(
  rows: MdBarsCoverageRow[],
  mode: CoverageSortMode,
): MdBarsCoverageRow[] {
  return [...rows].sort((a, b) => {
    switch (mode) {
      case "symbol_asc":
        return a.symbol.localeCompare(b.symbol);
      case "symbol_desc":
        return b.symbol.localeCompare(a.symbol);
      case "bars_desc":
        if (b.bars !== a.bars) return b.bars - a.bars;
        return a.symbol.localeCompare(b.symbol);
      case "bars_asc":
        if (a.bars !== b.bars) return a.bars - b.bars;
        return a.symbol.localeCompare(b.symbol);
      case "latest_desc":
        if (b.max_end_ts !== a.max_end_ts) return b.max_end_ts - a.max_end_ts;
        return a.symbol.localeCompare(b.symbol);
      case "latest_asc":
        if (a.max_end_ts !== b.max_end_ts) return a.max_end_ts - b.max_end_ts;
        return a.symbol.localeCompare(b.symbol);
    }
  });
}

/**
 * Filter coverage rows by symbol substring (case-insensitive). Empty query returns all rows.
 */
export function filterCoverageRows(
  rows: MdBarsCoverageRow[],
  symbolQuery: string,
): MdBarsCoverageRow[] {
  const q = symbolQuery.trim().toLowerCase();
  if (!q) return rows;
  return rows.filter((r) => r.symbol.toLowerCase().includes(q));
}

export interface CoverageSummary {
  totalDaemonRows: number;
  visibleRows: number;
  visibleBars: number;
}

/**
 * Compute totals for the coverage panel header.
 * totalDaemonRows: all rows from daemon; visibleRows/visibleBars: after filter+sort.
 */
export function computeCoverageSummary(
  allRows: MdBarsCoverageRow[],
  filtered: MdBarsCoverageRow[],
): CoverageSummary {
  return {
    totalDaemonRows: allRows.length,
    visibleRows: filtered.length,
    visibleBars: filtered.reduce((sum, r) => sum + r.bars, 0),
  };
}

/**
 * Return the sorted list of tracked symbols that have no coverage row for the given timeframe.
 *
 * Returns null when the tracked-equities registry is unavailable — explicitly NOT an empty array.
 * Returns [] when the registry is loaded and all tracked symbols have coverage.
 *
 * @param trackedSymbols - flat list of (symbol, timeframes[]) from TrackedEquitiesResponse.symbols,
 *   or null when registry is unavailable.
 * @param coverageRows - all coverage rows from the daemon.
 * @param timeframeFilter - only check tracked symbols that include this timeframe; null = no filter.
 */
export function computeMissingTrackedSymbols(
  trackedSymbols: Array<{ symbol: string; timeframes: string[] }> | null,
  coverageRows: MdBarsCoverageRow[],
  timeframeFilter: string | null,
): string[] | null {
  if (trackedSymbols === null) return null;

  const coveredSet = new Set<string>();
  for (const row of coverageRows) {
    if (timeframeFilter === null || row.timeframe === timeframeFilter) {
      coveredSet.add(row.symbol);
    }
  }

  const missing: string[] = [];
  for (const entry of trackedSymbols) {
    const relevant =
      timeframeFilter === null
        ? true
        : entry.timeframes.includes(timeframeFilter);
    if (relevant && !coveredSet.has(entry.symbol)) {
      missing.push(entry.symbol);
    }
  }

  return missing.sort();
}

// ---------------------------------------------------------------------------
// CRYPTO-DATA-01Q-R-LATEST-MARK-GUI-SURFACE-BUNDLE-01-COMBINED:
// Latest-mark evidence status
// ---------------------------------------------------------------------------

export interface FetchLatestMarkStatusResult {
  ok: boolean;
  data?: LatestMarkStatusResponse;
  error?: string;
}

/**
 * Return true only for the truth_state that means "usable display data".
 * stale/no_evidence/parse_error/unsafe_evidence/backend_unavailable are all
 * explicitly not active.
 */
export function isLatestMarkStatusActive(truthState: string): boolean {
  return truthState === "active";
}

/**
 * Human-readable label for a latest-mark truth_state. unsafe_evidence is
 * worded as a severe fail-closed condition, not a plain status label.
 */
export function latestMarkStatusTruthLabel(truthState: string): string {
  switch (truthState) {
    case "active":
      return "active";
    case "stale":
      return "stale";
    case "no_evidence":
      return "no evidence";
    case "parse_error":
      return "parse error";
    case "unsafe_evidence":
      return "UNSAFE EVIDENCE — fail-closed, not displayed as active";
    case "backend_unavailable":
      return "backend unavailable";
    default:
      return truthState;
  }
}

/**
 * Defense-in-depth check independent of the backend's own truth_state
 * classification: if the evidence body claims any DB/md_bars/completed-bar
 * write, treat it as unsafe even if truth_state were somehow not
 * "unsafe_evidence". The backend should already classify these as
 * unsafe_evidence -- this is a second, GUI-side guard so a misclassified
 * response is never rendered as trustworthy.
 */
export function isLatestMarkEvidenceUnsafe(response: LatestMarkStatusResponse): boolean {
  return (
    response.truth_state === "unsafe_evidence" ||
    response.db_write === true ||
    response.md_bars_write === true ||
    response.completed_bar_claim === true
  );
}

/**
 * Format a Unix-second timestamp as "YYYY-MM-DD HH:MM:SS". Returns "—" for
 * null/undefined/zero/invalid values.
 */
export function formatUnixSecondsDateTime(ts: number | null | undefined): string {
  if (!ts) return "—";
  const d = new Date(ts * 1000);
  if (isNaN(d.getTime())) return "—";
  return d.toISOString().slice(0, 19).replace("T", " ");
}

/**
 * Fetch GET /api/v1/market-data/latest-marks/status.
 *
 * Safety: Read-only. No DB connection. No provider/network call. No CLI
 * execution. No trading state mutation. Ticker-only latest marks -- not
 * OHLCV, not md_bars, not portfolio valuation, not trading enablement.
 */
export async function fetchLatestMarkStatus(): Promise<FetchLatestMarkStatusResult> {
  const result = await fetchJsonCandidate<LatestMarkStatusResponse>(
    "/api/v1/market-data/latest-marks/status",
  );

  if (!result.ok) {
    return { ok: false, error: result.error ?? "Latest-mark status fetch failed." };
  }

  return { ok: true, data: result.data };
}

// ---------------------------------------------------------------------------
// CRYPTO-DATA-01AE-KRAKEN-SYNC-GUI-STATUS-SURFACE-01: Kraken OHLC
// ingest/sync evidence status
// ---------------------------------------------------------------------------

export interface FetchKrakenOhlcStatusResult {
  ok: boolean;
  data?: KrakenOhlcStatusResponse;
  error?: string;
}

/**
 * Fixed warning text the GUI panel must display verbatim, so the operator
 * cannot mistake this read-only evidence surface for scheduling, strategy
 * input, or trading enablement.
 */
export const KRAKEN_OHLC_STATUS_WARNING_TEXT =
  "Kraken OHLCV evidence is data-ingestion visibility only. It is not scheduling, not strategy input by itself, not broker execution, and not crypto trading enablement.";

/**
 * Return true only for the truth_state that means "usable display data".
 * stale/no_evidence/parse_error/unsafe_evidence/backend_unavailable are all
 * explicitly not active.
 */
export function isKrakenOhlcStatusActive(truthState: string): boolean {
  return truthState === "active";
}

/**
 * Human-readable label for a Kraken OHLC status truth_state.
 * unsafe_evidence is worded as a severe fail-closed condition, not a plain
 * status label.
 */
export function krakenOhlcStatusTruthLabel(truthState: string): string {
  switch (truthState) {
    case "active":
      return "active";
    case "stale":
      return "stale";
    case "no_evidence":
      return "no evidence";
    case "parse_error":
      return "parse error";
    case "unsafe_evidence":
      return "UNSAFE EVIDENCE — fail-closed, not displayed as active";
    case "backend_unavailable":
      return "backend unavailable";
    default:
      return truthState;
  }
}

/**
 * Defense-in-depth check independent of the backend's own truth_state
 * classification: if the evidence body's own fields look inconsistent or
 * unsafe, treat it as unsafe even if truth_state were somehow not
 * "unsafe_evidence". The backend already classifies these as
 * unsafe_evidence -- this is a second, GUI-side guard so a misclassified
 * response is never rendered as trustworthy.
 */
export function isKrakenOhlcEvidenceUnsafe(response: KrakenOhlcStatusResponse): boolean {
  return (
    response.truth_state === "unsafe_evidence" ||
    (response.provider !== null && response.provider !== "kraken") ||
    (response.db_write === false &&
      ((response.rows_inserted ?? 0) > 0 || (response.rows_updated ?? 0) > 0))
  );
}

/**
 * Fetch GET /api/v1/market-data/kraken-ohlc/status.
 *
 * Safety: Read-only. No DB connection. No provider/network call. No CLI
 * execution. No sync/ingest triggered. No trading state mutation. Data-
 * ingestion visibility only -- not scheduling, not strategy input by
 * itself, not broker execution, and not crypto trading enablement.
 */
export async function fetchKrakenOhlcStatus(): Promise<FetchKrakenOhlcStatusResult> {
  const result = await fetchJsonCandidate<KrakenOhlcStatusResponse>(
    "/api/v1/market-data/kraken-ohlc/status",
  );

  if (!result.ok) {
    return { ok: false, error: result.error ?? "Kraken OHLC status fetch failed." };
  }

  return { ok: true, data: result.data };
}

// ---------------------------------------------------------------------------
// CRYPTO-REGISTRY-04-KRAKEN-DATA-ONLY-REGISTRY-STATUS-SURFACE-01: crypto
// registry readiness
// ---------------------------------------------------------------------------

export interface FetchCryptoRegistryReadinessResult {
  ok: boolean;
  data?: CryptoRegistryReadinessResponse;
  error?: string;
}

/**
 * Fixed warning text the GUI panel must display verbatim, so the operator
 * cannot mistake this read-only registry-visibility surface for crypto
 * trading, broker routing, strategy execution, or scheduling.
 */
export const CRYPTO_REGISTRY_READINESS_WARNING_TEXT =
  "Registry readiness is data-pipeline visibility only. It does not enable crypto trading, broker routing, strategy execution, or scheduling.";

/**
 * Return true only for the truth_state that means "usable display data".
 * missing_provider/missing_symbol/missing_alias/unsafe_trading_enabled/
 * unsafe_provider_enabled/parse_error are all explicitly not active.
 */
export function isCryptoRegistryReadinessActive(truthState: string): boolean {
  return truthState === "active";
}

/**
 * Human-readable label for a crypto registry readiness truth_state. The two
 * "unsafe_*" states are worded as severe fail-closed conditions, not plain
 * status labels.
 */
export function cryptoRegistryReadinessTruthLabel(truthState: string): string {
  switch (truthState) {
    case "active":
      return "active";
    case "missing_provider":
      return "provider not found";
    case "missing_symbol":
      return "symbol not found or wrong asset class";
    case "missing_alias":
      return "Kraken alias incomplete";
    case "unsafe_trading_enabled":
      return "UNSAFE — trading flag unexpectedly enabled";
    case "unsafe_provider_enabled":
      return "UNSAFE — provider unexpectedly enabled";
    case "parse_error":
      return "parse error";
    default:
      return truthState;
  }
}

/**
 * Defense-in-depth check independent of the backend's own truth_state
 * classification: if the response's own fields look inconsistent or unsafe,
 * treat it as unsafe even if truth_state were somehow not one of the
 * unsafe_* states. The backend already classifies these -- this is a
 * second, GUI-side guard so a misclassified response is never rendered as
 * trustworthy.
 */
export function isCryptoRegistryReadinessUnsafe(
  response: CryptoRegistryReadinessResponse,
): boolean {
  return (
    response.truth_state === "unsafe_trading_enabled" ||
    response.truth_state === "unsafe_provider_enabled" ||
    response.provider_enabled === true ||
    response.symbols.some((s) => s.paper_trading_enabled === true || s.live_trading_enabled === true)
  );
}

/**
 * Fetch GET /api/v1/market-data/crypto-registry/readiness.
 *
 * Safety: Read-only. No DB connection. No provider/network call. No CLI
 * execution. No config file mutated. No scheduler. No trading state.
 * Data-pipeline visibility only -- not crypto trading, broker routing,
 * strategy execution, or scheduling.
 */
export async function fetchCryptoRegistryReadiness(): Promise<FetchCryptoRegistryReadinessResult> {
  const result = await fetchJsonCandidate<CryptoRegistryReadinessResponse>(
    "/api/v1/market-data/crypto-registry/readiness",
  );

  if (!result.ok) {
    return { ok: false, error: result.error ?? "Crypto registry readiness fetch failed." };
  }

  return { ok: true, data: result.data };
}

// ---------------------------------------------------------------------------
// CRYPTO-DATA-02C-KRAKEN-SCHEDULER-READINESS-STATUS-SURFACE-01: Kraken
// scheduler readiness
// ---------------------------------------------------------------------------

export interface FetchKrakenSchedulerReadinessResult {
  ok: boolean;
  data?: KrakenSchedulerReadinessResponse;
  error?: string;
}

/**
 * Fixed warning text the GUI panel must display verbatim, so the operator
 * cannot mistake this read-only scheduler-readiness visibility surface for
 * an actual registered task, a daemon job, a Kraken call, or crypto trading
 * enablement.
 */
export const KRAKEN_SCHEDULER_READINESS_WARNING_TEXT =
  "Scheduler readiness is data-pipeline visibility only. It does not register a scheduled task, start a daemon job, call Kraken, enable crypto trading, or route broker orders.";

/**
 * Return true only for the truth_state that means "usable display data".
 * `active` never means a scheduler is registered -- see
 * `scheduler_readiness_state` for that distinction.
 */
export function isKrakenSchedulerReadinessActive(truthState: string): boolean {
  return truthState === "active";
}

/**
 * Human-readable label for a Kraken scheduler readiness truth_state. The
 * fail-closed states are worded as severe conditions, not plain status
 * labels.
 */
export function krakenSchedulerReadinessTruthLabel(truthState: string): string {
  switch (truthState) {
    case "active":
      return "active";
    case "policy_missing":
      return "UNSAFE — scheduler policy file missing";
    case "policy_invalid":
      return "UNSAFE — scheduler policy violates its own contract";
    case "registry_unsafe":
      return "UNSAFE — registry alias or asset-class check failed";
    case "provider_unsafe":
      return "UNSAFE — provider not found or unexpectedly enabled";
    case "trading_flags_unsafe":
      return "UNSAFE — trading flag unexpectedly enabled";
    case "scheduler_already_registered":
      return "UNSAFE — policy declares a scheduler already registered";
    case "evidence_unsafe":
      return "UNSAFE — latest Kraken evidence unsafe or stale";
    case "parse_error":
      return "parse error";
    case "backend_unavailable":
      return "registry/providers file unreadable";
    default:
      return truthState;
  }
}

/**
 * Defense-in-depth check independent of the backend's own truth_state
 * classification: if the response's own fields look inconsistent or unsafe,
 * treat it as unsafe even if truth_state were somehow not one of the
 * fail-closed states. The backend already classifies these -- this is a
 * second, GUI-side guard so a misclassified response is never rendered as
 * trustworthy, and so "active" is never confused with "a scheduler is
 * registered".
 */
export function isKrakenSchedulerReadinessUnsafe(
  response: KrakenSchedulerReadinessResponse,
): boolean {
  return (
    response.truth_state !== "active" ||
    response.provider_enabled === true ||
    response.scheduler_registration_status !== "not_registered" ||
    response.network_call_made === true ||
    response.db_write === true ||
    response.trading_enabled === true ||
    response.symbols.some(
      (s) => s.paper_trading_enabled === true || s.live_trading_enabled === true,
    )
  );
}

/**
 * Fetch GET /api/v1/market-data/kraken-scheduler/readiness.
 *
 * Safety: Read-only. No DB connection. No provider/network call. No CLI
 * execution. No config or policy file mutated. No scheduler registered. No
 * daemon job added. No trading state. Data-pipeline visibility only -- not
 * a registered scheduler, not crypto trading, not broker routing.
 */
export async function fetchKrakenSchedulerReadiness(): Promise<FetchKrakenSchedulerReadinessResult> {
  const result = await fetchJsonCandidate<KrakenSchedulerReadinessResponse>(
    "/api/v1/market-data/kraken-scheduler/readiness",
  );

  if (!result.ok) {
    return { ok: false, error: result.error ?? "Kraken scheduler readiness fetch failed." };
  }

  return { ok: true, data: result.data };
}

// ---------------------------------------------------------------------------
// CRYPTO-DATA-03C-KRAKEN-SCHEDULER-TASK-STATUS-SURFACE-01: Kraken scheduled
// task status (task-registration evidence visibility only)
// ---------------------------------------------------------------------------

export interface FetchKrakenSchedulerTaskStatusResult {
  ok: boolean;
  data?: KrakenSchedulerTaskStatusResponse;
  error?: string;
}

/**
 * Fixed warning text the GUI panel must display verbatim, so the operator
 * cannot mistake this read-only task-registration evidence surface for an
 * actual task registration/start, a Kraken call, a market-data write, or
 * crypto trading enablement.
 */
export const KRAKEN_SCHEDULER_TASK_STATUS_WARNING_TEXT =
  "Scheduled task status is evidence visibility only. This panel cannot register, unregister, start a task, call Kraken, write market data, or enable crypto trading.";

/**
 * Return true only for the truth_state that means "usable display data".
 * `active` never means a task is registered -- see `registered` /
 * `task_exists_after` for that distinct, truthfully-surfaced fact.
 */
export function isKrakenSchedulerTaskStatusActive(truthState: string): boolean {
  return truthState === "active";
}

/**
 * Human-readable label for a Kraken scheduled task status truth_state.
 * `unsafe_evidence` is worded as a severe fail-closed condition, not a plain
 * status label.
 */
export function krakenSchedulerTaskStatusTruthLabel(truthState: string): string {
  switch (truthState) {
    case "active":
      return "active";
    case "no_evidence":
      return "no evidence";
    case "parse_error":
      return "parse error";
    case "unsafe_evidence":
      return "UNSAFE EVIDENCE — fail-closed, not displayed as active";
    case "backend_unavailable":
      return "backend unavailable";
    default:
      return truthState;
  }
}

/**
 * Defense-in-depth check independent of the backend's own truth_state
 * classification: if the response's own fields look inconsistent or unsafe,
 * treat it as unsafe even if truth_state were somehow not "unsafe_evidence".
 * The backend already classifies these -- this is a second, GUI-side guard
 * so a misclassified response is never rendered as trustworthy.
 */
export function isKrakenSchedulerTaskEvidenceUnsafe(
  response: KrakenSchedulerTaskStatusResponse,
): boolean {
  return (
    response.truth_state === "unsafe_evidence" ||
    response.network_call_made === true ||
    response.db_write === true ||
    response.md_bars_write === true ||
    response.env_vars_embedded.length > 0 ||
    (response.mode === "check_only" &&
      (response.scheduled_task_mutation === true || response.registered === true))
  );
}

/**
 * Fetch GET /api/v1/market-data/kraken-scheduler/task-status.
 *
 * Safety: Read-only. No DB connection. No provider/network call. No CLI or
 * PowerShell execution. No Windows Task Scheduler API call. No scheduler
 * mutation. No trading state. Evidence visibility only -- not a task
 * registration/start, not a Kraken call, not a market-data write, and not
 * crypto trading enablement.
 */
export async function fetchKrakenSchedulerTaskStatus(): Promise<FetchKrakenSchedulerTaskStatusResult> {
  const result = await fetchJsonCandidate<KrakenSchedulerTaskStatusResponse>(
    "/api/v1/market-data/kraken-scheduler/task-status",
  );

  if (!result.ok) {
    return { ok: false, error: result.error ?? "Kraken scheduled task status fetch failed." };
  }

  return { ok: true, data: result.data };
}

// ---------------------------------------------------------------------------
// DAILY-DATA-READINESS-01D-GUI-01: Daily data readiness (read-only,
// configuration-preview projection of the strict readiness evaluator)
// ---------------------------------------------------------------------------

export const DAILY_DATA_READINESS_ROUTE = "/api/v1/market-data/readiness";

export interface FetchDailyDataReadinessResult {
  ok: boolean;
  data?: DailyDataReadinessResponse;
  error?: string;
}

/**
 * Normalize one assignment entry from an unknown/malformed raw value.
 *
 * - readiness/continuity/provenance states missing or non-string become
 *   "unknown", never a value that could be mistaken for "ready"/"ok".
 * - Numeric evidence (bar counts, timestamps) missing or non-finite becomes
 *   `null`, never `0`.
 * - `actual_provider_ids` / `actual_provider_symbols` / `blockers` /
 *   `remediation` default to `[]` on malformed input; unknown string
 *   entries are preserved verbatim.
 */
function normalizeDailyDataReadinessAssignment(raw: unknown): DailyDataReadinessAssignmentResponse {
  const record = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  return {
    assignment_symbol: normalizeString(record.assignment_symbol, "unknown"),
    assignment_timeframe: normalizeString(record.assignment_timeframe, "unknown"),
    configured_strategy_id: normalizeString(record.configured_strategy_id, "unknown"),
    effective_runtime_strategy_id: normalizeNullableString(record.effective_runtime_strategy_id),
    effective_runtime_target_symbol: normalizeNullableString(record.effective_runtime_target_symbol),
    effective_runtime_timeframe_secs: normalizeNullableNumber(record.effective_runtime_timeframe_secs),
    required_history_bars: normalizeNullableNumber(record.required_history_bars),
    asset_class: normalizeNullableString(record.asset_class),
    expected_provider_id: normalizeNullableString(record.expected_provider_id),
    expected_provider_symbol: normalizeNullableString(record.expected_provider_symbol),
    actual_provider_ids: normalizeStringArray(record.actual_provider_ids),
    actual_provider_symbols: normalizeStringArray(record.actual_provider_symbols),
    loaded_completed_bars: normalizeNullableNumber(record.loaded_completed_bars),
    expected_latest_bar_ts: normalizeNullableNumber(record.expected_latest_bar_ts),
    actual_latest_bar_ts: normalizeNullableNumber(record.actual_latest_bar_ts),
    continuity_state: normalizeString(record.continuity_state, "unknown"),
    provenance_state: normalizeString(record.provenance_state, "unknown"),
    readiness_state: normalizeString(record.readiness_state, "unknown"),
    blockers: normalizeStringArray(record.blockers),
    remediation: normalizeStringArray(record.remediation),
    configured_grace_seconds: normalizeNullableNumber(record.configured_grace_seconds) ?? 0,
    effective_grace_seconds: normalizeNullableNumber(record.effective_grace_seconds) ?? 0,
    configured_future_skew_seconds: normalizeNullableNumber(record.configured_future_skew_seconds) ?? 0,
    effective_future_skew_seconds: normalizeNullableNumber(record.effective_future_skew_seconds) ?? 0,
  };
}

/**
 * Normalize a raw `GET /api/v1/market-data/readiness` body into the typed
 * response shape. Pure — no HTTP, no throw.
 *
 * Fail-closed rules (D.2):
 * - `start_allowed` missing/non-boolean becomes `false`, never `true`.
 * - `applicability` missing/non-string becomes `"unknown"`, never
 *   `"applicable"`.
 * - Malformed `assignments` entries are normalized individually and never
 *   throw or drop the rest of the array.
 */
export function normalizeDailyDataReadinessResponse(raw: unknown): DailyDataReadinessResponse {
  const record = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const assignmentsRaw = Array.isArray(record.assignments) ? record.assignments : [];
  return {
    canonical_route: normalizeString(record.canonical_route, DAILY_DATA_READINESS_ROUTE),
    schema_version: normalizeString(record.schema_version, "unknown"),
    evaluated_at_utc: normalizeString(record.evaluated_at_utc, "unknown"),
    binding_scope: normalizeString(record.binding_scope, "unknown"),
    assignment_source: normalizeString(record.assignment_source, "unknown"),
    applicability: normalizeString(record.applicability, "unknown"),
    start_allowed: record.start_allowed === true,
    top_level_blocker: normalizeNullableString(record.top_level_blocker),
    configured_grace_seconds: normalizeNullableNumber(record.configured_grace_seconds) ?? 0,
    configured_future_skew_seconds: normalizeNullableNumber(record.configured_future_skew_seconds) ?? 0,
    calendar_source: normalizeNullableString(record.calendar_source),
    calendar_coverage_state: normalizeString(record.calendar_coverage_state, "unknown"),
    market_date: normalizeNullableString(record.market_date),
    session_open_utc: normalizeNullableString(record.session_open_utc),
    session_close_utc: normalizeNullableString(record.session_close_utc),
    assignments: assignmentsRaw.map(normalizeDailyDataReadinessAssignment),
  };
}

/**
 * Fetch GET /api/v1/market-data/readiness.
 *
 * Safety: Read-only GET only. No ingest job submitted. No provider polled.
 * No scheduler started. No runtime started. No readiness truth mutated.
 */
export async function fetchDailyDataReadiness(): Promise<FetchDailyDataReadinessResult> {
  const result = await fetchJsonCandidate<unknown>(DAILY_DATA_READINESS_ROUTE);

  if (!result.ok) {
    return { ok: false, error: result.error ?? "Daily data readiness fetch failed." };
  }

  return { ok: true, data: normalizeDailyDataReadinessResponse(result.data) };
}

export type DailyDataReadinessDisplayState = "ready" | "blocked" | "unknown" | "not_applicable";

/**
 * Classify the overall display state for the readiness panel.
 *
 * "ready" requires proof at every level (applicable, start_allowed=true,
 * at least one assignment, every assignment readiness_state=="ready" with
 * no blockers) — `start_allowed=true` alone is never sufficient. A
 * `start_allowed=true` response containing a blocked or unknown assignment
 * is "unknown", never "ready" and never "blocked" (the response is
 * internally contradictory, not proven either way).
 */
export function classifyDailyDataReadinessDisplay(
  response: DailyDataReadinessResponse | null,
  fetchError?: string | null,
): DailyDataReadinessDisplayState {
  if (fetchError) return "unknown";
  if (response === null) return "unknown";

  if (response.applicability === "not_applicable") return "not_applicable";
  if (response.applicability !== "applicable") return "unknown";

  if (response.start_allowed === false) return "blocked";

  if (response.assignments.length === 0) return "unknown";

  const allReady = response.assignments.every(
    (a) => a.readiness_state === "ready" && a.blockers.length === 0,
  );

  return allReady ? "ready" : "unknown";
}

/**
 * Build a bounded, plain-text diagnostic summary for clipboard copy.
 * Includes schema/evaluation identity, calendar truth, per-assignment
 * identity/provider/continuity truth, and every blocker and remediation
 * entry. Never includes credentials, tokens, or environment dumps.
 */
export function buildDailyDataReadinessDiagnosticText(response: DailyDataReadinessResponse): string {
  const overall = classifyDailyDataReadinessDisplay(response, null);
  const lines: string[] = [];

  lines.push("Daily Data Readiness Diagnostics");
  lines.push(`schema_version: ${response.schema_version}`);
  lines.push(`evaluated_at_utc: ${response.evaluated_at_utc}`);
  lines.push(`binding_scope: ${response.binding_scope}`);
  lines.push(`applicability: ${response.applicability}`);
  lines.push(`overall_state: ${overall}`);
  lines.push(`start_allowed: ${String(response.start_allowed)}`);
  lines.push(`top_level_blocker: ${response.top_level_blocker ?? "none"}`);
  lines.push(`assignment_source: ${response.assignment_source}`);
  lines.push(`calendar_source: ${response.calendar_source ?? "unknown"}`);
  lines.push(`calendar_coverage_state: ${response.calendar_coverage_state}`);
  lines.push(`market_date: ${response.market_date ?? "unknown"}`);
  lines.push(`session_open_utc: ${response.session_open_utc ?? "unknown"}`);
  lines.push(`session_close_utc: ${response.session_close_utc ?? "unknown"}`);

  if (response.binding_scope === "configuration_preview") {
    lines.push(
      "NOTE: This is a configuration preview. Runtime start re-evaluates readiness using the exact start-attempt binding.",
    );
  }

  if (response.assignments.length === 0) {
    lines.push("assignments: none");
  } else {
    response.assignments.forEach((a, idx) => {
      lines.push(`--- assignment ${idx + 1} ---`);
      lines.push(`symbol: ${a.assignment_symbol}`);
      lines.push(`timeframe: ${a.assignment_timeframe}`);
      lines.push(`configured_strategy_id: ${a.configured_strategy_id}`);
      lines.push(`effective_runtime_strategy_id: ${a.effective_runtime_strategy_id ?? "unknown"}`);
      lines.push(`effective_runtime_target_symbol: ${a.effective_runtime_target_symbol ?? "unknown"}`);
      lines.push(`expected_provider_id: ${a.expected_provider_id ?? "unknown"}`);
      lines.push(
        `actual_provider_ids: ${a.actual_provider_ids.length > 0 ? a.actual_provider_ids.join(", ") : "none observed"}`,
      );
      lines.push(`expected_provider_symbol: ${a.expected_provider_symbol ?? "unknown"}`);
      lines.push(
        `actual_provider_symbols: ${a.actual_provider_symbols.length > 0 ? a.actual_provider_symbols.join(", ") : "none observed"}`,
      );
      lines.push(`continuity_state: ${a.continuity_state}`);
      lines.push(`provenance_state: ${a.provenance_state}`);
      lines.push(`readiness_state: ${a.readiness_state}`);
      lines.push(
        `blockers: ${a.blockers.length > 0 ? a.blockers.join(", ") : "none"}`,
      );
      lines.push(
        `remediation: ${a.remediation.length > 0 ? a.remediation.join(" | ") : "none"}`,
      );
    });
  }

  return lines.join("\n");
}
