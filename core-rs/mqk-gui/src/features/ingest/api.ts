// DATA-INGEST-GUI-RUNNER-01: Ingest job HTTP helpers.
//
// Safety invariants:
// - Only calls /api/v1/ingest/* routes. No live/paper execution routes.
// - POST submit uses privileged:true (operator auth token). GET is public.
// - Pure helpers (normalizeIngestJobStatus, isTerminalIngestStatus,
//   buildActiveIngestJob) are exported for test isolation.

import { fetchJsonCandidate, postJson } from "../system/http";
import type {
  ActiveIngestJob,
  ActiveProviderJob,
  IngestJobAcceptedResponse,
  IngestJobRequest,
  IngestJobStatusKind,
  IngestJobStatusResponse,
  IngestJobsListResponse,
  IntradayRefreshStatusResponse,
  MdBarsCoverageResponse,
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
    status === "failed"
  );
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
