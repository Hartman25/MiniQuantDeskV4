export interface BacktestManifest {
  schema_version: number;
  run_id: string;
  strategy_name: string;
  engine_id: string;
  mode: string;
  git_hash?: string | null;
  config_hash?: string | null;
  host_fingerprint?: string | null;
  created_at_utc: string;
  artifacts?: Record<string, string>;
}

export interface BacktestMetrics {
  schema_version: number;
  run_id: string;
  strategy_name: string;
  halted: boolean;
  halt_reason: string | null;
  execution_blocked: boolean;
  bars: number;
  orders: number;
  orders_filled: number;
  orders_rejected: number;
  fills: number;
  final_equity_micros: number;
  symbols: string[];
  starting_equity_micros: number;
  ending_equity_micros: number;
  total_return_micros: number;
  total_return_pct: number;
  equity_high_water_mark_micros: number;
  max_drawdown_micros: number;
  max_drawdown_pct: number;
  total_commission_micros: number;
  trade_count: number;
  winning_trade_count: number;
  losing_trade_count: number;
  flat_trade_count: number;
  win_rate_pct: number | null;
  gross_profit_micros: number;
  gross_loss_micros: number;
  profit_factor: number | null;
  average_win_micros: number | null;
  average_loss_micros: number | null;
  expectancy_micros: number | null;
  best_trade_micros: number | null;
  worst_trade_micros: number | null;
  sharpe_ratio: number | null;
  sortino_ratio: number | null;
  exposure_bars: number;
  exposure_time_pct: number;
}

export interface EquityCurveRow {
  ts_utc: string;
  equity: number;
}

export interface OrderRow {
  ts_utc: string;
  order_id: string;
  symbol: string;
  side: string;
  qty: string;
  order_type: string;
  limit_price: string;
  stop_price: string;
  status: string;
}

export interface FillRow {
  ts_utc: string;
  fill_id: string;
  order_id: string;
  symbol: string;
  side: string;
  qty: string;
  price: string;
  fee: string;
}

export type FileResult<T> =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ok"; data: T }
  | { kind: "missing" }
  | { kind: "parse_error"; message: string }
  | { kind: "read_error"; message: string };

export interface ParsedCsvResult<T> {
  rows: T[];
  malformed: number;
}

export interface ArtifactBundle {
  manifest: FileResult<BacktestManifest>;
  metrics: FileResult<BacktestMetrics>;
  equityCurve: FileResult<ParsedCsvResult<EquityCurveRow>>;
  orders: FileResult<ParsedCsvResult<OrderRow>>;
  fills: FileResult<ParsedCsvResult<FillRow>>;
  strategyFit: FileResult<StrategyFitParseResult>;
  paperReadiness: FileResult<PaperReadinessParseResult>;
}

// ---------------------------------------------------------------------------
// GUI-STRATEGY-FIT-REJECTION-SURFACE-BUNDLE-01: strategy-fit-v1 gate result types
// ---------------------------------------------------------------------------

export interface StrategyFitGateFlags {
  profit_factor_failed: boolean;
  expectancy_failed: boolean;
  cost_adjusted_edge_failed: boolean;
  out_of_sample_failed: boolean;
  sample_quality_failed: boolean;
  parameter_stability_failed: boolean;
  validation_metrics_missing: boolean;
}

export interface StrategyFitArtifact {
  schema_version: string;
  artifact_id: string | null;
  symbol: string;
  strategy_id: string;
  timeframe: string | null;
  trades: number | null;
  profit_factor: number | null;
  expectancy_bps: number | null;
  net_expectancy_after_cost_bps: number | null;
  recommended_for_paper: boolean;
  recommended_for_live: boolean;
  recommended_for_live_present: boolean;
  failure_reasons: string[];
  gateFlags: StrategyFitGateFlags;
}

export type StrategyFitParseResult =
  | { kind: "ok"; data: StrategyFitArtifact }
  | { kind: "unsupported_schema"; schemaVersion: string | null }
  | { kind: "malformed"; message: string }
  | { kind: "missing_fields"; message: string };

// ---------------------------------------------------------------------------
// WATCHLIST-PROMOTION-GUI-SURFACE-BUNDLE-01: paper-readiness-v1 chain result types
// ---------------------------------------------------------------------------

export interface PaperReadinessReport {
  schema_version: string;
  status: string;
  reasons: string[];
  top_symbol: string | null;
  symbol_inputs_status: string | null;
  risk_simulation_passed: boolean | null;
  premarket_revalidation_passed: boolean | null;
  promotion_passed: boolean | null;
  approved_for_autonomous_paper: boolean;
  approved_for_live: boolean;
  live_locked: boolean;
  daemon_enforcement_executed: boolean;
  paper_handoff_requested: boolean;
  paper_handoff_executed: boolean;
  upstream_pipeline_toggles: Record<string, boolean>;
  artifacts_read: Record<string, string | null>;
  artifacts_written: Record<string, string | null>;
  notes: string;
  /**
   * Hard-invariant fields (approved_for_live, live_locked,
   * daemon_enforcement_executed, paper_handoff_executed) are forced to safe
   * values by the producer's to_dict(). If a loaded artifact reports an
   * unsafe value for any of these, it is passed through honestly and its
   * anomaly id is recorded here rather than silently corrected or hidden.
   */
  hardInvariantAnomalies: string[];
}

export type PaperReadinessParseResult =
  | { kind: "ok"; data: PaperReadinessReport }
  | { kind: "unsupported_schema"; schemaVersion: string | null }
  | { kind: "malformed"; message: string }
  | { kind: "missing_fields"; message: string };

// ---------------------------------------------------------------------------
// BACKTEST-GUI-RUNNER-01: Daemon backtest job API types
// ---------------------------------------------------------------------------

export interface BacktestJobRequest {
  bars_path: string;
  strategy: string;
  symbol: string;
  timeframe_secs: number;
  initial_cash_micros: number;
  out_dir?: string | null;
  integrity_enabled?: boolean | null;
  /**
   * Integrity stale threshold in ticks (seconds for time-indexed bar feeds).
   * When omitted, the daemon applies a timeframe-aware default:
   *   - timeframe_secs >= 86400 (daily): 172800 (2 days)
   *   - otherwise: 120 (conservative_defaults)
   * Daily bars have 86400 s gaps; the 120 s default causes execution_blocked=true
   * immediately. Always set this explicitly for daily data or rely on the default.
   */
  integrity_stale_threshold_ticks?: number | null;
  shadow?: boolean | null;
}

export interface BacktestJobAcceptedResponse {
  accepted: boolean;
  job_id: string;
  status: string;
  artifact_dir: string | null;
  error: string | null;
}

export interface BacktestJobStatusResponse {
  truth_state: string;
  job_id: string;
  status: string;
  strategy: string;
  symbol: string;
  created_at_utc: string;
  started_at_utc: string | null;
  completed_at_utc: string | null;
  artifact_dir: string | null;
  manifest_path: string | null;
  metrics_path: string | null;
  error: string | null;
}

export type BacktestJobStatusKind = "queued" | "running" | "completed" | "failed" | "unknown";

export interface ActiveBacktestJob {
  jobId: string;
  status: BacktestJobStatusKind;
  strategy: string;
  symbol: string;
  createdAt: string;
  startedAt: string | null;
  completedAt: string | null;
  artifactDir: string | null;
  error: string | null;
}
