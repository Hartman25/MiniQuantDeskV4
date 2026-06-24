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
