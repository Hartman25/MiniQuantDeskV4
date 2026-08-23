// BACKTEST-GUI-RUNNER-01: Backtest job HTTP helpers.
//
// Safety invariants:
// - Only calls /api/v1/backtests/* routes. No live/paper execution routes.
// - POST submit uses privileged:true (operator auth token). GET status is public.
// - Pure helpers (normalizeJobStatus, isTerminalJobStatus) are exported for test isolation.

import { fetchJsonCandidate, postJson } from "../system/http";
import type {
  ActiveBacktestJob,
  BacktestEconomicsRequest,
  BacktestEconomicsSuggestionResponse,
  BacktestJobAcceptedResponse,
  BacktestJobRequest,
  BacktestJobStatusKind,
  BacktestJobStatusResponse,
  BacktestJobsListResponse,
  BacktestJobSummary,
  SessionJobRow,
} from "./types";
export { getInstrumentRegistryV2SourceStatus } from "../system/api";

// ---------------------------------------------------------------------------
// Pure helpers — testable without HTTP
// ---------------------------------------------------------------------------

export function normalizeJobStatus(raw: string): BacktestJobStatusKind {
  switch (raw) {
    case "queued":
    case "running":
    case "completed":
    case "failed":
      return raw;
    default:
      return "unknown";
  }
}

export function isTerminalJobStatus(status: BacktestJobStatusKind): boolean {
  return status === "completed" || status === "failed";
}

export function extractArtifactDir(response: BacktestJobStatusResponse): string | null {
  if (response.status === "completed" && response.artifact_dir) {
    return response.artifact_dir;
  }
  return null;
}

/**
 * Maps one daemon job-list row into the GUI's normalized SessionJobRow.
 * Pure — status goes through normalizeJobStatus so an unrecognized daemon
 * status string surfaces as "unknown", never silently coerced to "completed".
 */
export function buildSessionJobRow(summary: BacktestJobSummary): SessionJobRow {
  return {
    jobId: summary.job_id,
    status: normalizeJobStatus(summary.status),
    strategy: summary.strategy,
    symbol: summary.symbol,
    createdAt: summary.created_at_utc,
    startedAt: summary.started_at_utc ?? null,
    completedAt: summary.completed_at_utc ?? null,
    artifactDir: summary.artifact_dir ?? null,
    error: summary.error ?? null,
  };
}

export function buildActiveJob(response: BacktestJobStatusResponse): ActiveBacktestJob {
  return {
    jobId: response.job_id,
    status: normalizeJobStatus(response.status),
    strategy: response.strategy,
    symbol: response.symbol,
    createdAt: response.created_at_utc,
    startedAt: response.started_at_utc ?? null,
    completedAt: response.completed_at_utc ?? null,
    artifactDir: response.artifact_dir ?? null,
    error: response.error ?? null,
  };
}

export interface BacktestEconomicsFormFields {
  contractMultiplier: string;
  initialMarginMicros: string;
  maintenanceMarginMicros: string;
}

export type BuildBacktestEconomicsResult =
  | { ok: true; economics?: BacktestEconomicsRequest }
  | { ok: false; error: string };

function parseOptionalInteger(raw: string, fieldName: string): { ok: true; value: number | null } | { ok: false; error: string } {
  const trimmed = raw.trim();
  if (trimmed === "") return { ok: true, value: null };
  if (!/^-?\d+$/.test(trimmed)) {
    return { ok: false, error: `${fieldName} must be an integer.` };
  }
  const value = Number(trimmed);
  if (!Number.isSafeInteger(value)) {
    return { ok: false, error: `${fieldName} must be a safe integer.` };
  }
  return { ok: true, value };
}

export function buildBacktestEconomicsRequest(
  fields: BacktestEconomicsFormFields,
): BuildBacktestEconomicsResult {
  const multiplier = parseOptionalInteger(fields.contractMultiplier, "contract_multiplier");
  if (!multiplier.ok) return multiplier;
  if (multiplier.value !== null && multiplier.value <= 0) {
    return { ok: false, error: "contract_multiplier must be a positive integer." };
  }

  const initialMargin = parseOptionalInteger(fields.initialMarginMicros, "initial_margin_micros");
  if (!initialMargin.ok) return initialMargin;

  const maintenanceMargin = parseOptionalInteger(fields.maintenanceMarginMicros, "maintenance_margin_micros");
  if (!maintenanceMargin.ok) return maintenanceMargin;

  const economics: BacktestEconomicsRequest = {};
  if (multiplier.value !== null) economics.contract_multiplier = multiplier.value;
  if (initialMargin.value !== null) economics.initial_margin_micros = initialMargin.value;
  if (maintenanceMargin.value !== null) economics.maintenance_margin_micros = maintenanceMargin.value;

  return Object.keys(economics).length === 0
    ? { ok: true }
    : { ok: true, economics };
}

export type ValidateMdBarsDateRangeResult = { ok: true } | { ok: false; error: string };

/**
 * Client-side pre-check mirroring the daemon's md_bars date-range validation
 * (backtest_job_submit in routes/backtests.rs: start/end required, RFC3339,
 * end >= start). The daemon remains the authoritative gate — this only stops
 * an obviously invalid range from round-tripping to the server first.
 */
export function validateMdBarsDateRange(startRaw: string, endRaw: string): ValidateMdBarsDateRangeResult {
  const start = startRaw.trim();
  const end = endRaw.trim();
  if (!start) return { ok: false, error: "Start is required for the database source." };
  if (!end) return { ok: false, error: "End is required for the database source." };
  const startMs = Date.parse(start);
  if (Number.isNaN(startMs)) return { ok: false, error: "Start must be a valid RFC3339 timestamp." };
  const endMs = Date.parse(end);
  if (Number.isNaN(endMs)) return { ok: false, error: "End must be a valid RFC3339 timestamp." };
  if (endMs < startMs) return { ok: false, error: "End must be >= start for the database source." };
  return { ok: true };
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

export interface SubmitBacktestResult {
  ok: boolean;
  status?: number;
  data?: BacktestJobAcceptedResponse;
  error?: string;
}

export interface GetBacktestJobResult {
  ok: boolean;
  status?: number;
  data?: BacktestJobStatusResponse;
  error?: string;
  notFound?: boolean;
}

export interface GetBacktestEconomicsSuggestionResult {
  ok: boolean;
  status?: number;
  data?: BacktestEconomicsSuggestionResponse;
  error?: string;
}

export async function submitBacktestJob(req: BacktestJobRequest): Promise<SubmitBacktestResult> {
  const result = await postJson<BacktestJobAcceptedResponse>(
    ["/api/v1/backtests/jobs"],
    req as unknown as Record<string, unknown>,
    { privileged: true },
  );

  if (result.error === "desktop operator token missing") {
    return {
      ok: false,
      error: "Operator token missing — desktop auth required to submit backtest jobs.",
    };
  }

  if (!result.ok) {
    if (result.status === 404) {
      return { ok: false, status: 404, error: "Backtest job API unavailable (route not found)." };
    }
    return { ok: false, status: result.status, error: result.error ?? "Submission failed." };
  }

  return { ok: true, status: result.status, data: result.data };
}

export interface GetBacktestJobsResult {
  ok: boolean;
  status?: number;
  data?: BacktestJobsListResponse;
  error?: string;
}

/**
 * Fetch the current daemon-session job list (GET /api/v1/backtests/jobs).
 * This is process-lifetime/in-memory history — never durable. A malformed
 * payload (missing/non-array `jobs`) fails visibly rather than rendering a
 * fake empty list, so a broken response can never look like "no jobs yet".
 */
export async function getBacktestJobs(): Promise<GetBacktestJobsResult> {
  const result = await fetchJsonCandidate<BacktestJobsListResponse>("/api/v1/backtests/jobs");

  if (!result.ok) {
    const isNotFound = result.error === "HTTP 404";
    return {
      ok: false,
      error: isNotFound
        ? "Backtest job list API unavailable (route not found)."
        : (result.error ?? "Job list fetch failed."),
    };
  }

  if (!result.data || !Array.isArray(result.data.jobs)) {
    return { ok: false, error: "Malformed backtest job list response: 'jobs' is not an array." };
  }

  return { ok: true, data: result.data };
}

export async function getBacktestJob(jobId: string): Promise<GetBacktestJobResult> {
  const result = await fetchJsonCandidate<BacktestJobStatusResponse>(
    `/api/v1/backtests/jobs/${jobId}`,
  );

  if (!result.ok) {
    const isNotFound = result.error === "HTTP 404";
    return {
      ok: false,
      error: isNotFound ? "Backtest job API unavailable or job not found." : (result.error ?? "Status fetch failed."),
      notFound: isNotFound,
    };
  }

  return { ok: true, data: result.data };
}

export async function getBacktestEconomicsSuggestion(
  symbol: string,
): Promise<GetBacktestEconomicsSuggestionResult> {
  const trimmed = symbol.trim();
  if (!trimmed) {
    return { ok: false, error: "symbol is required for registry economics suggestion." };
  }

  const result = await fetchJsonCandidate<BacktestEconomicsSuggestionResponse>(
    `/api/v1/backtests/economics-suggestion?symbol=${encodeURIComponent(trimmed)}`,
  );

  if (!result.ok) {
    return {
      ok: false,
      error: result.error === "HTTP 404"
        ? "Backtest economics suggestion API unavailable (route not found)."
        : (result.error ?? "Economics suggestion fetch failed."),
    };
  }

  return { ok: true, data: result.data };
}
