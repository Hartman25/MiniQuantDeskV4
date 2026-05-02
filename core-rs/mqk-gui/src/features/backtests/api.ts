// BACKTEST-GUI-RUNNER-01: Backtest job HTTP helpers.
//
// Safety invariants:
// - Only calls /api/v1/backtests/* routes. No live/paper execution routes.
// - POST submit uses privileged:true (operator auth token). GET status is public.
// - Pure helpers (normalizeJobStatus, isTerminalJobStatus) are exported for test isolation.

import { fetchJsonCandidate, postJson } from "../system/http";
import type {
  ActiveBacktestJob,
  BacktestJobAcceptedResponse,
  BacktestJobRequest,
  BacktestJobStatusKind,
  BacktestJobStatusResponse,
} from "./types";

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
