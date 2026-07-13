// STRATEGY-SCANNER-JOBS-GUI-01D: TypeScript types matching daemon strategy
// scanner job/artifact API shapes. Mirrors api_types.rs
// StrategyScanJobSubmitRequest / StrategyScanJobAcceptedResponse /
// StrategyScanJobSummary / StrategyScanJobsListResponse /
// StrategyScanJobStatusResponse / StrategyScanArtifactResponse, and
// mqk_backtest::{ScanManifest, ScanSummary, ScanSkipReasonCount,
// StrategyScanCandidate, StrategyScanMetrics}.
//
// Research/review only. This screen has no trade/promote/approve action —
// see StrategyScannerScreen.tsx.

export interface StrategyScanJobSubmitRequest {
  registry_path?: string | null;
  bars_root?: string | null;
  timeframe: string;
  strategy: string;
  top: number;
  limit_symbols?: number | null;
  out_dir?: string | null;
}

export interface StrategyScanJobAcceptedResponse {
  accepted: boolean;
  job_id: string;
  status: string;
  artifact_dir: string | null;
  error: string | null;
}

export interface StrategyScanJobSummary {
  job_id: string;
  status: string;
  timeframe: string;
  strategy: string;
  created_at_utc: string;
  started_at_utc: string | null;
  completed_at_utc: string | null;
  artifact_dir: string | null;
  ranked_count: number | null;
  skipped_count: number | null;
  error: string | null;
}

export interface StrategyScanJobsListResponse {
  truth_state: string;
  jobs: StrategyScanJobSummary[];
}

export interface StrategyScanMetrics {
  total_return_pct: number | null;
  benchmark_return_pct: number | null;
  alpha_pct: number | null;
  max_drawdown_pct: number | null;
  trade_count: number | null;
  win_rate_pct: number | null;
  profit_factor: number | null;
  fill_count: number | null;
  bars_used: number;
  data_start_ts: number | null;
  data_end_ts: number | null;
  halted: boolean;
}

export interface StrategyScanCandidate {
  symbol: string;
  timeframe: string;
  strategy_id: string;
  bars_available: number;
  truth_state: string;
  reason_code: string;
  score: number | null;
  rank: number | null;
  metrics: StrategyScanMetrics;
  warnings: string[];
  blockers: string[];
}

export interface ScanSkipReasonCount {
  reason_code: string;
  count: number;
}

export interface ScanSummary {
  scan_id: string;
  universe_count: number;
  ranked_count: number;
  skipped_count: number;
  top_ranked: StrategyScanCandidate[];
  top_skip_reasons: ScanSkipReasonCount[];
}

export interface ScanManifest {
  schema_version: number;
  scan_id: string;
  created_at_utc: string;
  git_hash: string;
  registry_path: string;
  bars_root: string;
  timeframe: string;
  strategies: string[];
  universe_count: number;
  ranked_count: number;
  skipped_count: number;
  blockers: string[];
  warnings: string[];
}

export interface StrategyScanJobStatusResponse {
  truth_state: string;
  job_id: string;
  status: string;
  submitted_at_utc: string;
  completed_at_utc: string | null;
  request: StrategyScanJobSubmitRequest;
  artifact_dir: string | null;
  summary: ScanSummary | null;
  blockers: string[];
  warnings: string[];
  error: string | null;
}

/**
 * `truth_state`: "active" | "missing_artifact" | "invalid_artifact" |
 * "path_rejected" | "read_failed". "active" is the only state where
 * manifest/summary/candidates are populated.
 */
export interface StrategyScanArtifactResponse {
  truth_state: string;
  artifact_dir: string;
  manifest: ScanManifest | null;
  summary: ScanSummary | null;
  candidates: StrategyScanCandidate[] | null;
  top_candidates: StrategyScanCandidate[];
  skip_reasons: ScanSkipReasonCount[];
  warnings: string[];
  blockers: string[];
  error: string | null;
}

export type StrategyScanJobStatusKind = "queued" | "running" | "completed" | "failed" | "unknown";

export interface ActiveStrategyScanJob {
  jobId: string;
  status: StrategyScanJobStatusKind;
  timeframe: string;
  strategy: string;
  createdAt: string;
  startedAt: string | null;
  completedAt: string | null;
  artifactDir: string | null;
  summary: ScanSummary | null;
  warnings: string[];
  error: string | null;
}
