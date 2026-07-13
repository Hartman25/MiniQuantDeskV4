// STRATEGY-SCANNER-JOBS-GUI-01D: Strategy scanner job/artifact HTTP helpers.
//
// Safety invariants:
// - Only calls /api/v1/strategy-scans/* routes. No live/paper execution routes.
// - POST submit uses privileged:true (operator auth token). GET routes are public.
// - Pure helpers (normalizeStrategyScanJobStatus, isTerminalStrategyScanJobStatus,
//   buildActiveStrategyScanJob) are exported for test isolation.
// - This module has no submit-order / promote / approve action of any kind.

import { fetchJsonCandidate, postJson } from "../system/http";
import type {
  ActiveStrategyScanJob,
  StrategyScanArtifactResponse,
  StrategyScanJobAcceptedResponse,
  StrategyScanJobStatusKind,
  StrategyScanJobStatusResponse,
  StrategyScanJobSubmitRequest,
  StrategyScanJobsListResponse,
  StrategyScanReviewArtifactResponse,
} from "./types";

// ---------------------------------------------------------------------------
// Pure helpers — testable without HTTP
// ---------------------------------------------------------------------------

export function normalizeStrategyScanJobStatus(raw: string): StrategyScanJobStatusKind {
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

export function isTerminalStrategyScanJobStatus(status: StrategyScanJobStatusKind): boolean {
  return status === "completed" || status === "failed";
}

export function buildActiveStrategyScanJob(response: StrategyScanJobStatusResponse): ActiveStrategyScanJob {
  return {
    jobId: response.job_id,
    status: normalizeStrategyScanJobStatus(response.status),
    timeframe: response.request.timeframe,
    strategy: response.request.strategy,
    createdAt: response.submitted_at_utc,
    startedAt: null,
    completedAt: response.completed_at_utc ?? null,
    artifactDir: response.artifact_dir ?? null,
    summary: response.summary ?? null,
    warnings: response.warnings ?? [],
    error: response.error ?? null,
  };
}

export function isArtifactActive(truthState: string): boolean {
  return truthState === "active";
}

export function artifactTruthLabel(truthState: string): string {
  switch (truthState) {
    case "active":
      return "Active";
    case "missing_artifact":
      return "Missing artifact";
    case "invalid_artifact":
      return "Invalid artifact";
    case "path_rejected":
      return "Path rejected";
    case "read_failed":
      return "Read failed";
    default:
      return `Unknown (${truthState})`;
  }
}

export interface StrategyScanSubmitFormFields {
  registryPath: string;
  barsRoot: string;
  timeframe: string;
  strategy: string;
  top: string;
  limitSymbols: string;
  outDir: string;
}

export type BuildStrategyScanJobRequestResult =
  | { ok: true; request: StrategyScanJobSubmitRequest }
  | { ok: false; error: string };

/** Builds a validated request body from raw form-field strings. Pure — no HTTP. */
export function buildStrategyScanJobRequest(
  fields: StrategyScanSubmitFormFields,
): BuildStrategyScanJobRequestResult {
  const timeframe = fields.timeframe.trim();
  const strategy = fields.strategy.trim();
  if (!timeframe) return { ok: false, error: "timeframe is required." };
  if (!strategy) return { ok: false, error: "strategy is required." };

  const topTrimmed = fields.top.trim();
  const top = topTrimmed === "" ? 20 : Number(topTrimmed);
  if (!Number.isInteger(top) || top < 1 || top > 100) {
    return { ok: false, error: "top must be an integer between 1 and 100." };
  }

  let limitSymbols: number | null = null;
  const limitTrimmed = fields.limitSymbols.trim();
  if (limitTrimmed !== "") {
    const parsed = Number(limitTrimmed);
    if (!Number.isInteger(parsed) || parsed < 1 || parsed > 200) {
      return { ok: false, error: "limit_symbols must be an integer between 1 and 200." };
    }
    limitSymbols = parsed;
  }

  const request: StrategyScanJobSubmitRequest = {
    registry_path: fields.registryPath.trim() || null,
    bars_root: fields.barsRoot.trim() || null,
    timeframe,
    strategy,
    top,
    limit_symbols: limitSymbols,
    out_dir: fields.outDir.trim() || null,
  };
  return { ok: true, request };
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

export interface SubmitStrategyScanJobResult {
  ok: boolean;
  status?: number;
  data?: StrategyScanJobAcceptedResponse;
  error?: string;
}

export interface GetStrategyScanJobResult {
  ok: boolean;
  status?: number;
  data?: StrategyScanJobStatusResponse;
  error?: string;
  notFound?: boolean;
}

export interface GetStrategyScanJobsListResult {
  ok: boolean;
  status?: number;
  data?: StrategyScanJobsListResponse;
  error?: string;
}

export interface GetStrategyScanArtifactResult {
  ok: boolean;
  status?: number;
  data?: StrategyScanArtifactResponse;
  error?: string;
}

export async function submitStrategyScanJob(
  req: StrategyScanJobSubmitRequest,
): Promise<SubmitStrategyScanJobResult> {
  const result = await postJson<StrategyScanJobAcceptedResponse>(
    ["/api/v1/strategy-scans/jobs"],
    req as unknown as Record<string, unknown>,
    { privileged: true },
  );

  if (result.error === "desktop operator token missing") {
    return {
      ok: false,
      error: "Operator token missing — desktop auth required to submit a strategy scan job.",
    };
  }

  if (!result.ok) {
    if (result.status === 404) {
      return { ok: false, status: 404, error: "Strategy scan job API unavailable (route not found)." };
    }
    return { ok: false, status: result.status, error: result.error ?? "Submission failed." };
  }

  return { ok: true, status: result.status, data: result.data };
}

export async function getStrategyScanJob(jobId: string): Promise<GetStrategyScanJobResult> {
  const result = await fetchJsonCandidate<StrategyScanJobStatusResponse>(
    `/api/v1/strategy-scans/jobs/${jobId}`,
  );

  if (!result.ok) {
    const isNotFound = result.error === "HTTP 404";
    return {
      ok: false,
      error: isNotFound
        ? "Strategy scan job API unavailable or job not found."
        : (result.error ?? "Status fetch failed."),
      notFound: isNotFound,
    };
  }

  return { ok: true, data: result.data };
}

export async function getStrategyScanJobsList(): Promise<GetStrategyScanJobsListResult> {
  const result = await fetchJsonCandidate<StrategyScanJobsListResponse>("/api/v1/strategy-scans/jobs");
  if (!result.ok) {
    return { ok: false, error: result.error ?? "Job list fetch failed." };
  }
  return { ok: true, data: result.data };
}

export async function getStrategyScanArtifact(artifactDir: string): Promise<GetStrategyScanArtifactResult> {
  const trimmed = artifactDir.trim();
  if (!trimmed) {
    return { ok: false, error: "artifact_dir is required." };
  }
  const result = await fetchJsonCandidate<StrategyScanArtifactResponse>(
    `/api/v1/strategy-scans/artifact?artifact_dir=${encodeURIComponent(trimmed)}`,
  );
  if (!result.ok) {
    return {
      ok: false,
      error: result.error === "HTTP 404"
        ? "Strategy scan artifact API unavailable (route not found)."
        : (result.error ?? "Artifact fetch failed."),
    };
  }
  return { ok: true, data: result.data };
}

// ---------------------------------------------------------------------------
// STRATEGY-SCANNER-PROMOTION-01D: review-artifact readback (GET, public).
// Same truth_state vocabulary as the scanner artifact route above --
// isArtifactActive / artifactTruthLabel apply unchanged.
// ---------------------------------------------------------------------------

export interface GetStrategyScanReviewArtifactResult {
  ok: boolean;
  status?: number;
  data?: StrategyScanReviewArtifactResponse;
  error?: string;
}

export async function getStrategyScanReviewArtifact(
  reviewDir: string,
): Promise<GetStrategyScanReviewArtifactResult> {
  const trimmed = reviewDir.trim();
  if (!trimmed) {
    return { ok: false, error: "review_dir is required." };
  }
  const result = await fetchJsonCandidate<StrategyScanReviewArtifactResponse>(
    `/api/v1/strategy-scans/review-artifact?review_dir=${encodeURIComponent(trimmed)}`,
  );
  if (!result.ok) {
    return {
      ok: false,
      error: result.error === "HTTP 404"
        ? "Strategy scan review artifact API unavailable (route not found)."
        : (result.error ?? "Review artifact fetch failed."),
    };
  }
  return { ok: true, data: result.data };
}
