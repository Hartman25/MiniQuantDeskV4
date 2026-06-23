export interface BacktestManifest {
  schema_version: number;
  run_id: string;
  strategy_name: string;
  engine_id: string;
  mode: string;
  /**
   * Optional friendly timeframe string (e.g. "5m", "1D"). Mirrors the Rust
   * RunManifest.timeframe field. Backtest jobs leave this unset and only emit
   * timeframe_secs; other producers may set it directly. Absence is not an
   * error — derive a label from timeframe_secs when this is missing.
   */
  timeframe?: string | null;
  /**
   * Optional bar interval in seconds. Mirrors the Rust RunManifest.timeframe_secs
   * field. The daemon backtest job writes this (the operator-submitted
   * timeframe_secs); the friendly label is derived via timeframeLabelFromSecs.
   */
  timeframe_secs?: number | null;
  git_hash?: string | null;
  config_hash?: string | null;
  host_fingerprint?: string | null;
  created_at_utc: string;
  artifacts?: Record<string, string>;
  /**
   * BACKTEST-DB-BARS-SOURCE-01: bars source for this run. Only present for
   * md_bars-sourced backtest jobs (additively merged into manifest.json after
   * the standard write) — absent on CSV-sourced runs and on manifests written
   * by other engines/modes. Absence means "not reported by this run", not csv;
   * callers should not assume one.
   */
  source?: string | null;
  /**
   * BACKTEST-DB-BARS-SOURCE-01: symbol queried from md_bars. Only present for
   * md_bars-sourced backtest jobs.
   */
  symbol?: string | null;
  /**
   * BACKTEST-DB-BARS-SOURCE-01: inclusive md_bars query range start (RFC3339).
   * Only present for md_bars-sourced backtest jobs.
   */
  start?: string | null;
  /**
   * BACKTEST-DB-BARS-SOURCE-01: inclusive md_bars query range end (RFC3339).
   * Only present for md_bars-sourced backtest jobs.
   */
  end?: string | null;
  /**
   * BACKTEST-DB-BARS-SOURCE-01: number of md_bars rows loaded for this run.
   * Only present for md_bars-sourced backtest jobs.
   */
  bar_count?: number | null;
}

/**
 * Buy-and-hold benchmark comparison. Mirrors `BenchmarkSection` in
 * core-rs/crates/mqk-artifacts/src/lib.rs. Only present in metrics.json when
 * the run processed >= 2 valid bars and the first bar's open price was > 0
 * (see compute_benchmark). Absence is not an error — it means the benchmark
 * could not be computed for this run.
 */
export interface BacktestBenchmark {
  first_bar_open_micros: number;
  last_bar_close_micros: number;
  buy_and_hold_return_pct: number;
  strategy_total_return_pct: number;
  alpha_pct: number;
  assumption: string;
}

export interface BacktestMetrics {
  schema_version: number;
  run_id: string;
  strategy_name: string;
  halted: boolean;
  halt_reason: string | null;
  execution_blocked: boolean;
  benchmark?: BacktestBenchmark | null;
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
  economics?: BacktestEconomicsMetadata | null;
}

export interface BacktestEconomicsMetadata {
  contract_multiplier?: number | null;
  initial_margin_micros?: number | null;
  maintenance_margin_micros?: number | null;
  realized_pnl_micros?: number | null;
  margin_enforced?: boolean | null;
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
  watchlistPromotion: FileResult<WatchlistPromotionParseResult>;
  premarketRevalidation: FileResult<PremarketRevalidationParseResult>;
  evidenceReview: FileResult<EvidenceReviewParseResult>;
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
// WATCHLIST-PROMOTION-DETAIL-GUI-SURFACE-BUNDLE-02: watchlist-v1 promotion artifact types
// ---------------------------------------------------------------------------

export interface RankedCandidate {
  rank: number | null;
  symbol: string | null;
  strategy_id: string | null;
  timeframe: string | null;
  total_score: number | null;
  liquidity_score: number | null;
  regime_label: string | null;
  regime_score: number | null;
  risk_score: number | null;
  net_expectancy_after_cost_bps: number | null;
  paper_qty_limit: number | null;
  notional_limit_usd: number | null;
  selection_reason: string | null;
  source_candidate_artifact: string | null;
}

export interface WatchlistPromotionDecision {
  passed: boolean;
  failure_reasons: string[];
  approved_symbols: string[];
  strategy_assignments: Record<string, string>;
  notes: string;
}

export interface WatchlistPromotionArtifact {
  schema_version: string;
  mode: string | null;
  generated_at_utc: string | null;
  trade_date: string | null;
  approved_for_autonomous_paper: boolean;
  approved_for_live: boolean;
  max_symbols_to_trade: number | null;
  max_concurrent_positions: number | null;
  symbols: string[];
  strategy_assignments: Record<string, string>;
  paper_qty_limits: Record<string, number>;
  notional_limits: Record<string, number>;
  ranked_candidates: RankedCandidate[];
  selection_reason: string | null;
  promotion_decision: WatchlistPromotionDecision | null;
  /**
   * Not a literal field in promoted_watchlist.json. Derived from the rank-1
   * entry of ranked_candidates, falling back to symbols[0] if no rank-1
   * candidate is present. Null if neither is available.
   */
  top_symbol: string | null;
  /**
   * approved_for_live is forced to false by the producer's
   * apply_watchlist_promotion(). If a loaded artifact reports true, it is
   * passed through honestly and its anomaly id is recorded here rather than
   * silently corrected or hidden.
   */
  hardInvariantAnomalies: string[];
}

export type WatchlistPromotionParseResult =
  | { kind: "ok"; data: WatchlistPromotionArtifact }
  | { kind: "unsupported_schema"; schemaVersion: string | null }
  | { kind: "malformed"; message: string }
  | { kind: "missing_fields"; message: string };

// ---------------------------------------------------------------------------
// WATCHLIST-PROMOTION-DETAIL-GUI-SURFACE-BUNDLE-02: premarket-revalidation-v1 artifact types
// ---------------------------------------------------------------------------

export interface SymbolPremarketResult {
  passed: boolean;
  failure_reasons: string[];
}

export interface PremarketRevalidationArtifact {
  schema_version: string;
  passed: boolean;
  failure_reasons: string[];
  top_symbol: string | null;
  symbol_results: Record<string, SymbolPremarketResult>;
  notes: string;
}

export type PremarketRevalidationParseResult =
  | { kind: "ok"; data: PremarketRevalidationArtifact }
  | { kind: "unsupported_schema"; schemaVersion: string | null }
  | { kind: "malformed"; message: string }
  | { kind: "missing_fields"; message: string };

// ---------------------------------------------------------------------------
// GUI-EVIDENCE-DISCORD-LINKS-SURFACE-BUNDLE-01: review-v2 evidence review artifact types
// ---------------------------------------------------------------------------

export interface EvidenceReviewArtifact {
  schema_version: string;
  classification: string;
  classification_reasons: string[];
  folder_name: string | null;
  reviewed_at: string | null;
  capture_ts: string | null;
  runtime_status: string | null;
  arm_state: string | null;
  kill_switch_active: boolean | null;
  integrity_halt_active: boolean | null;
  risk_halt_active: boolean | null;
  live_routing_enabled: boolean | null;
  deadman_status: string | null;
  reconcile_status: string | null;
  reconcile_total_mismatches: number | null;
  fill_count: number | null;
  open_order_count: number | null;
  position_count: number | null;
  autonomous_flatten_available: boolean | null;
  autonomous_flatten_blockers: string | null;
  autonomous_next_operator_action: string | null;
}

export type EvidenceReviewParseResult =
  | { kind: "ok"; data: EvidenceReviewArtifact }
  | { kind: "unsupported_schema"; schemaVersion: string | null }
  | { kind: "malformed"; message: string }
  | { kind: "missing_fields"; message: string };

// ---------------------------------------------------------------------------
// BACKTEST-GUI-RUNNER-01: Daemon backtest job API types
// ---------------------------------------------------------------------------

/** BACKTEST-DB-BARS-SOURCE-01: backtest bars source. */
export type BacktestSourceKind = "csv" | "md_bars";

export interface BacktestJobRequest {
  /**
   * Bars source. Optional — omitting it (pre-existing requests) defaults to
   * "csv" on the daemon. Mirrors the Rust BacktestJobRequest.source default.
   */
  source?: BacktestSourceKind;
  /** Required when source="csv". Omit (or leave empty) when source="md_bars". */
  bars_path?: string;
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
  /** Required when source="md_bars": md_bars timeframe string (e.g. "1D", "5m"). */
  timeframe?: string | null;
  /** Required when source="md_bars": inclusive range start, RFC3339. */
  start?: string | null;
  /** Required when source="md_bars": inclusive range end, RFC3339. Must be >= start. */
  end?: string | null;
  /** Optional backtest-only instrument economics override. Omit to preserve default equity behavior. */
  economics?: BacktestEconomicsRequest;
}

export interface BacktestEconomicsRequest {
  contract_multiplier?: number;
  initial_margin_micros?: number;
  maintenance_margin_micros?: number;
}

export interface BacktestEconomicsSuggestionResponse {
  truth_state: string;
  symbol: string;
  source: string;
  contract_multiplier: number | null;
  initial_margin_micros: number | null;
  maintenance_margin_micros: number | null;
  reason: string | null;
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
