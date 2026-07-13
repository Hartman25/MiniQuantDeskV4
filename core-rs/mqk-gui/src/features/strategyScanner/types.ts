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

// ---------------------------------------------------------------------------
// STRATEGY-SCANNER-PROMOTION-01D: review-artifact types. Mirrors
// mqk_backtest::{ReviewManifest, ReviewSummary, StrategyScanReviewDecision,
// StrategyScanReviewState} and daemon api_types.rs
// StrategyScanReviewArtifactResponse.
//
// Research/review only. `paper_candidate` is NOT trading approval -- see
// StrategyScannerScreen.tsx's required warning banner.
// ---------------------------------------------------------------------------

export type StrategyScanReviewStateKind =
  | "blocked"
  | "needs_review"
  | "watchlist_candidate"
  | "paper_candidate"
  | "rejected";

export interface StrategyScanReviewDecision {
  symbol: string;
  timeframe: string;
  strategy_id: string;
  scanner_rank: number | null;
  scanner_score: number | null;
  review_state: StrategyScanReviewStateKind;
  reason_codes: string[];
  blockers: string[];
  warnings: string[];
}

export interface ReviewManifest {
  schema_version: number;
  review_id: string;
  scanner_scan_id: string;
  source_artifact_dir: string;
  created_at_utc: string;
  git_hash: string;
  policy_min_bars_used: number;
  policy_min_trade_count: number;
  policy_min_total_return_pct: number;
  policy_min_alpha_pct: number;
  policy_max_drawdown_pct: number;
  policy_min_profit_factor: number;
  candidate_count: number;
  blocked_count: number;
  needs_review_count: number;
  watchlist_candidate_count: number;
  paper_candidate_count: number;
  rejected_count: number;
  blockers: string[];
  warnings: string[];
}

export interface ReviewSummary {
  scanner_scan_id: string;
  review_id: string;
  candidate_count: number;
  blocked_count: number;
  needs_review_count: number;
  watchlist_candidate_count: number;
  paper_candidate_count: number;
  rejected_count: number;
  top_paper_candidates: StrategyScanReviewDecision[];
  top_watchlist_candidates: StrategyScanReviewDecision[];
  blockers: string[];
  warnings: string[];
}

/**
 * `truth_state`: "active" | "missing_artifact" | "invalid_artifact" |
 * "path_rejected" | "read_failed". "active" is the only state where
 * manifest/summary/decisions are populated.
 */
export interface StrategyScanReviewArtifactResponse {
  truth_state: string;
  review_dir: string;
  manifest: ReviewManifest | null;
  summary: ReviewSummary | null;
  decisions: StrategyScanReviewDecision[] | null;
  top_paper_candidates: StrategyScanReviewDecision[];
  top_watchlist_candidates: StrategyScanReviewDecision[];
  warnings: string[];
  blockers: string[];
  error: string | null;
}

// ---------------------------------------------------------------------------
// STRATEGY-PROMOTION-REGISTRY-01E: promotion control-surface types. Mirrors
// daemon api_types.rs StrategyPromotionRow / StrategyPromotionsResponse /
// StrategyPromotionHistoryResponse / StrategyPromotionCheckResponse /
// StrategyPromotionTransitionRequest / StrategyPromotionTransitionResponse.
//
// `registered + enabled` is never promotion approval. `tradable_live` is
// always `false` on every response shape below -- a paper promotion state
// never authorizes a LIVE run or live-routing path. See
// StrategyScannerScreen.tsx's PromotionControlPanel for the operator-facing
// surface; this module carries no order/broker/run-start/arm route.
// ---------------------------------------------------------------------------

export type PromotionStateKind =
  | "shadow_approved"
  | "paper_approved"
  | "active_paper"
  | "demoted"
  | "retired"
  | "rejected";

export interface StrategyPromotionRow {
  transition_id: string;
  strategy_id: string;
  symbol: string;
  timeframe_secs: number;
  config_fingerprint: string | null;
  config_identity_status: string;
  previous_state: string | null;
  new_state: string;
  evidence_review_id: string | null;
  evidence_scanner_scan_id: string | null;
  evidence_git_hash: string | null;
  evidence_artifact_path: string | null;
  evidence_fingerprint: string | null;
  effective_at_utc: string;
  expires_at_utc: string | null;
  initiated_by: string;
  reason: string;
  created_at_utc: string;
  tradable_paper: boolean;
  /** Always `false`. No GUI action can ever make this `true`. */
  tradable_live: boolean;
  reason_code: string;
  blockers: string[];
}

export interface StrategyPromotionsResponse {
  canonical_route: string;
  backend: string;
  truth_state: string;
  rows: StrategyPromotionRow[];
}

export interface StrategyPromotionHistoryResponse {
  canonical_route: string;
  backend: string;
  truth_state: string;
  strategy_id: string;
  symbol: string;
  timeframe_secs: number;
  rows: StrategyPromotionRow[];
  blockers: string[];
}

export interface StrategyPromotionCheckResponse {
  canonical_route: string;
  backend: string;
  truth_state: string;
  strategy_id: string;
  symbol: string;
  timeframe_secs: number;
  current_state: string | null;
  config_identity_status: string | null;
  tradable_paper: boolean;
  /** Always `false`. */
  tradable_live: boolean;
  reason_code: string;
  blockers: string[];
}

export interface StrategyPromotionTransitionRequest {
  strategy_id: string;
  symbol: string;
  timeframe_secs: number;
  target_state: PromotionStateKind;
  review_dir: string | null;
  effective_at_utc: string;
  expires_at_utc: string | null;
  initiated_by: string;
  reason: string;
}

export interface StrategyPromotionTransitionResponse {
  accepted: boolean;
  disposition: string;
  strategy_id: string;
  symbol: string;
  timeframe_secs: number;
  previous_state: string | null;
  target_state: string;
  transition_id: string | null;
  blockers: string[];
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
