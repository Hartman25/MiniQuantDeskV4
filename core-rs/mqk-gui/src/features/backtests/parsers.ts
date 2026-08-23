import type {
  ArtifactBundle,
  BacktestEconomicsSuggestionResponse,
  BacktestManifest,
  BacktestMetrics,
  EquityCurveRow,
  EvidenceReviewArtifact,
  EvidenceReviewParseResult,
  FillRow,
  OrderRow,
  PaperReadinessParseResult,
  PaperReadinessReport,
  ParsedCsvResult,
  PremarketRevalidationArtifact,
  PremarketRevalidationParseResult,
  RankedCandidate,
  StrategyFitArtifact,
  StrategyFitGateFlags,
  StrategyFitParseResult,
  SymbolPremarketResult,
  WatchlistPromotionArtifact,
  WatchlistPromotionDecision,
  WatchlistPromotionParseResult,
} from "./types.ts";
export {
  describeInstrumentRegistryV2SourceNonEquity,
  describeInstrumentRegistryV2SourceTradingUse,
  instrumentRegistryV2SourceStatusLabel,
} from "../system/instrumentRegistryV2Source.ts";

export function parseManifest(json: string): BacktestManifest {
  const obj: unknown = JSON.parse(json);
  if (!obj || typeof obj !== "object" || Array.isArray(obj)) {
    throw new Error("manifest.json: expected a JSON object");
  }
  const m = obj as Record<string, unknown>;
  if (typeof m.run_id !== "string") throw new Error("manifest.json: missing run_id");
  if (typeof m.strategy_name !== "string") throw new Error("manifest.json: missing strategy_name");
  if (typeof m.created_at_utc !== "string") throw new Error("manifest.json: missing created_at_utc");
  return obj as BacktestManifest;
}

export function parseMetrics(json: string): BacktestMetrics {
  const obj: unknown = JSON.parse(json);
  if (!obj || typeof obj !== "object" || Array.isArray(obj)) {
    throw new Error("metrics.json: expected a JSON object");
  }
  const m = obj as Record<string, unknown>;
  if (typeof m.run_id !== "string") throw new Error("metrics.json: missing run_id");
  if (typeof m.bars !== "number") throw new Error("metrics.json: missing bars");
  return obj as BacktestMetrics;
}

export function parseCsvRows(csv: string): { headers: string[]; rows: Record<string, string>[] } {
  const lines = csv.split(/\r?\n/).filter((line) => line.trim() !== "");
  if (lines.length === 0) return { headers: [], rows: [] };
  const headers = lines[0].split(",").map((h) => h.trim());
  const rows: Record<string, string>[] = [];
  for (let i = 1; i < lines.length; i++) {
    const cells = lines[i].split(",");
    const row: Record<string, string> = {};
    for (let j = 0; j < headers.length; j++) {
      row[headers[j]] = cells[j]?.trim() ?? "";
    }
    rows.push(row);
  }
  return { headers, rows };
}

/**
 * GUI-BACKTEST-EQUITY-DISPLAY-ROBUSTNESS-01: a row is valid only if `equity`
 * is present, non-blank, and Number.isFinite once converted — Number("")===0
 * and Number("Infinity")===Infinity would otherwise pass a bare isNaN check
 * and later corrupt min/max/drawdown geometry with 0/Infinity instead of a
 * truthfully skipped malformed row.
 */
export function parseEquityCurve(csv: string): ParsedCsvResult<EquityCurveRow> {
  const { rows } = parseCsvRows(csv);
  let malformed = 0;
  const parsed: EquityCurveRow[] = [];
  for (const r of rows) {
    if (!Object.prototype.hasOwnProperty.call(r, "equity") || r.equity.trim() === "") {
      malformed++;
      continue;
    }
    const equity = Number(r.equity);
    if (!Number.isFinite(equity)) {
      malformed++;
      continue;
    }
    parsed.push({ ts_utc: r.ts_utc ?? "", equity });
  }
  return { rows: parsed, malformed };
}

/**
 * O(n) min/max over a finite-number array. Never spreads into Math.min/max —
 * `Math.max(...values)` overflows the JS call stack (RangeError) once
 * `values` reaches tens of thousands of entries, which a long intraday
 * backtest's equity curve realistically can.
 */
export function minMax(values: number[]): { min: number; max: number } {
  let min = Infinity;
  let max = -Infinity;
  for (const v of values) {
    if (v < min) min = v;
    if (v > max) max = v;
  }
  return { min, max };
}

export interface DrawdownPoint {
  ts_utc: string;
  equity: number;
  peak: number;
  drawdown_pct: number;
}

/**
 * GUI-BACKTEST-RESULT-ANALYSIS-01: derives a running peak-to-trough drawdown
 * series from an equity curve for display only. Never overwrites
 * metrics.json's authoritative max_drawdown_pct/max_drawdown_micros — this is
 * a client-derived visualization, not a second source of truth. Pure and
 * total: empty input yields an empty series; a non-positive running peak
 * yields 0% (no division by a non-positive baseline) rather than a
 * fabricated or negative percentage.
 */
export function computeDrawdownSeries(rows: EquityCurveRow[]): DrawdownPoint[] {
  let peak = -Infinity;
  const out: DrawdownPoint[] = [];
  for (const r of rows) {
    if (r.equity > peak) peak = r.equity;
    const drawdown_pct = peak > 0 ? ((peak - r.equity) / peak) * 100 : 0;
    out.push({ ts_utc: r.ts_utc, equity: r.equity, peak, drawdown_pct });
  }
  return out;
}

export function parseOrders(csv: string): ParsedCsvResult<OrderRow> {
  const { rows } = parseCsvRows(csv);
  let malformed = 0;
  const parsed: OrderRow[] = [];
  for (const r of rows) {
    if (!r.order_id) {
      malformed++;
      continue;
    }
    parsed.push({
      ts_utc: r.ts_utc ?? "",
      order_id: r.order_id,
      symbol: r.symbol ?? "",
      side: r.side ?? "",
      qty: r.qty ?? "",
      order_type: r.order_type ?? "",
      limit_price: r.limit_price ?? "",
      stop_price: r.stop_price ?? "",
      status: r.status ?? "",
    });
  }
  return { rows: parsed, malformed };
}

export function parseFills(csv: string): ParsedCsvResult<FillRow> {
  const { rows } = parseCsvRows(csv);
  let malformed = 0;
  const parsed: FillRow[] = [];
  for (const r of rows) {
    if (!r.fill_id) {
      malformed++;
      continue;
    }
    parsed.push({
      ts_utc: r.ts_utc ?? "",
      fill_id: r.fill_id,
      order_id: r.order_id ?? "",
      symbol: r.symbol ?? "",
      side: r.side ?? "",
      qty: r.qty ?? "",
      price: r.price ?? "",
      fee: r.fee ?? "",
    });
  }
  return { rows: parsed, malformed };
}

export function microsToUsd(micros: number): number {
  return micros / 1_000_000;
}

export function formatMicrosAsDollars(micros: number | null | undefined): string {
  if (micros == null || Number.isNaN(micros)) return "—";
  const usd = microsToUsd(micros);
  return usd.toLocaleString(undefined, {
    style: "currency",
    currency: "USD",
    maximumFractionDigits: 2,
  });
}

export function formatNullableNumber(value: number | null | undefined, digits = 4): string {
  if (value == null || Number.isNaN(value)) return "—";
  return value.toFixed(digits);
}

export function formatNullablePercent(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return "—";
  return `${value.toFixed(2)}%`;
}

// ---------------------------------------------------------------------------
// BACKTEST-GUI-EXPERIENCE-01: run timeframe display helpers
// ---------------------------------------------------------------------------

/**
 * Friendly timeframe label derived from a bar interval in seconds.
 *
 * Mirrors the canonical Rust mapping in
 * core-rs/crates/mqk-artifacts/src/lib.rs (`timeframe_from_secs`) exactly so the
 * GUI label never drifts from the engine's own vocabulary. Returns null when the
 * value is absent, not a number, or non-positive, so callers can render
 * "not reported" honestly rather than a fabricated label.
 */
export function timeframeLabelFromSecs(timeframeSecs: number | null | undefined): string | null {
  if (typeof timeframeSecs !== "number" || Number.isNaN(timeframeSecs)) return null;
  switch (timeframeSecs) {
    case 60:
      return "1m";
    case 300:
      return "5m";
    case 900:
      return "15m";
    case 3600:
      return "1h";
    case 86400:
      return "1D";
    default:
      return timeframeSecs > 0 ? `${timeframeSecs}s` : null;
  }
}

/**
 * Resolve the display timeframe for a run manifest. An explicit, non-empty
 * `timeframe` string wins; otherwise a label is derived from `timeframe_secs`.
 * Mirrors the Rust precedence (manifest.timeframe, then
 * timeframe_from_secs(manifest.timeframe_secs)). Returns null when neither is
 * available so callers render "not reported" rather than fabricating a value.
 */
export function manifestTimeframeLabel(
  timeframe: string | null | undefined,
  timeframeSecs: number | null | undefined,
): string | null {
  if (typeof timeframe === "string" && timeframe.trim() !== "") return timeframe.trim();
  return timeframeLabelFromSecs(timeframeSecs);
}

// ---------------------------------------------------------------------------
// GUI-BACKTEST-RUN-COMPARISON-01: side-by-side comparison snapshot
// ---------------------------------------------------------------------------

export interface ComparisonSnapshot {
  strategy: string;
  symbols: string;
  timeframe: string;
  dataRange: string;
  totalReturnPct: number;
  alphaPct: number | null;
  maxDrawdownPct: number;
  sharpeRatio: number | null;
  sortinoRatio: number | null;
  tradeCount: number;
  winRatePct: number | null;
  profitFactor: number | null;
  expectancyMicros: number | null;
  commissionMicros: number;
}

/**
 * Extracts already-authoritative metrics/manifest values into a flat
 * comparison snapshot. Pure display mapping only — never recomputes or
 * derives a "winner"; a null field means the source artifact did not report
 * that value, rendered as such by the caller.
 */
export function extractComparisonSnapshot(bundle: ArtifactBundle): ComparisonSnapshot | null {
  if (bundle.metrics.kind !== "ok") return null;
  const m = bundle.metrics.data;
  const manifest = bundle.manifest.kind === "ok" ? bundle.manifest.data : null;
  const equityRows = bundle.equityCurve.kind === "ok" ? bundle.equityCurve.data.rows : [];
  const dataRange =
    equityRows.length > 0
      ? `${equityRows[0].ts_utc || "—"} -> ${equityRows[equityRows.length - 1].ts_utc || "—"}`
      : "not reported";

  return {
    strategy: manifest?.strategy_name ?? m.strategy_name,
    symbols: m.symbols.length > 0 ? m.symbols.join(", ") : "not reported",
    timeframe: manifestTimeframeLabel(manifest?.timeframe, manifest?.timeframe_secs) ?? "not reported",
    dataRange,
    totalReturnPct: m.total_return_pct,
    alphaPct: m.benchmark?.alpha_pct ?? null,
    maxDrawdownPct: m.max_drawdown_pct,
    sharpeRatio: m.sharpe_ratio,
    sortinoRatio: m.sortino_ratio,
    tradeCount: m.trade_count,
    winRatePct: m.win_rate_pct,
    profitFactor: m.profit_factor,
    expectancyMicros: m.expectancy_micros,
    commissionMicros: m.total_commission_micros,
  };
}

// ---------------------------------------------------------------------------
// BACKTEST-REPORT-UX-01: operator-review summary helpers
// ---------------------------------------------------------------------------

export interface AlphaSummary {
  label: string;
  tone: "good" | "bad" | "neutral";
}

/**
 * Classify a benchmark alpha_pct value for operator display. Returns
 * "Benchmark unavailable" (neutral) when alpha could not be computed for this
 * run (metrics.json has no `benchmark` section) — never fabricates a value.
 */
export function classifyAlpha(alphaPct: number | null | undefined): AlphaSummary {
  if (alphaPct == null || Number.isNaN(alphaPct)) {
    return { label: "Benchmark unavailable", tone: "neutral" };
  }
  if (alphaPct > 0) return { label: "Outperformed buy & hold", tone: "good" };
  if (alphaPct < 0) return { label: "Underperformed buy & hold", tone: "bad" };
  return { label: "Matched buy & hold exactly", tone: "neutral" };
}

/**
 * Truthful operator-facing tradability warning for a backtest economics
 * suggestion. Returns null when `enabled` is unknown (e.g. not_found,
 * registry_unavailable, validation_failed -- no instrument was matched) or
 * `true` -- callers must never infer trading permission merely from the
 * presence of suggested economics metadata (INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED).
 */
export function describeEconomicsSuggestionTradability(
  s: BacktestEconomicsSuggestionResponse,
): string | null {
  if (s.enabled === false) {
    return "not enabled for trading (suggestion only)";
  }
  return null;
}

/**
 * Explain a zero-trade run in operator-facing language. Returns null when at
 * least one trade or fill occurred (there is nothing to explain).
 */
export function describeNoTradeActivity(m: BacktestMetrics): string | null {
  if (m.trade_count > 0 || m.fills > 0) return null;
  if (m.execution_blocked) {
    return (
      "No trades were opened. Execution was blocked by the integrity gate " +
      "before any orders could be placed — see the execution-blocked warning below."
    );
  }
  if (m.orders === 0) {
    return (
      "No trades were opened. The strategy did not generate any order intents " +
      "during this run — its entry conditions were not met on this data. This " +
      "does not by itself mean the strategy is broken."
    );
  }
  if (m.orders_rejected >= m.orders) {
    return (
      `No trades were opened. All ${m.orders} order intent(s) generated by the ` +
      "strategy were rejected by the engine — see orders.csv for rejection reasons."
    );
  }
  return (
    "No trades were opened. Orders were generated but none resulted in a fill " +
    "— see orders.csv and fills.csv for details."
  );
}

export interface ExecutionWarning {
  title: string;
  detail: string;
  tone: "warn" | "bad";
}

/**
 * Collect operator-facing warnings for a completed run (execution_blocked,
 * halted). Returns an empty array when the run completed without either
 * condition.
 */
export function describeExecutionWarnings(m: BacktestMetrics): ExecutionWarning[] {
  const warnings: ExecutionWarning[] = [];
  if (m.execution_blocked) {
    warnings.push({
      title: "Execution blocked by integrity gate",
      detail:
        "No orders were placed after the block. Common cause for daily (1D) data: " +
        "the integrity stale-feed threshold was exceeded by a bar gap. Re-run via " +
        "Workflow B with the integrity stale threshold left blank (daemon default: " +
        "345600 s for daily bars, 120 s for intraday) or set it explicitly to at " +
        "least 345600 for daily bars.",
      tone: "warn",
    });
  }
  if (m.halted) {
    warnings.push({
      title: "Backtest halted",
      detail: m.halt_reason ?? "Halted for an unspecified reason — see audit log for details.",
      tone: "bad",
    });
  }
  return warnings;
}

// ---------------------------------------------------------------------------
// GUI-STRATEGY-FIT-REJECTION-SURFACE-BUNDLE-01: strategy-fit-v1 gate result parser
// ---------------------------------------------------------------------------

export const STRATEGY_FIT_SCHEMA_VERSION = "strategy-fit-v1";

// Failure reason strings — must match research-py/src/mqk_research/scanner/backtest_gates.py
// and backtest_bridge.py exactly. Used to derive PASS/FAIL gate flags from failure_reasons.
export const FAILURE_REASON_PROFIT_FACTOR = "profit_factor_failed";
export const FAILURE_REASON_EXPECTANCY = "expectancy_failed";
export const FAILURE_REASON_COST_ADJUSTED_EDGE = "cost_adjusted_edge_failed";
export const FAILURE_REASON_OUT_OF_SAMPLE = "out_of_sample_failed";
export const FAILURE_REASON_SAMPLE_QUALITY = "sample_quality_failed";
export const FAILURE_REASON_PARAMETER_STABILITY = "parameter_stability_failed";
export const FAILURE_REASON_VALIDATION_METRICS_MISSING = "validation_metrics_missing";

export function deriveStrategyFitGateFlags(failureReasons: string[]): StrategyFitGateFlags {
  return {
    profit_factor_failed: failureReasons.includes(FAILURE_REASON_PROFIT_FACTOR),
    expectancy_failed: failureReasons.includes(FAILURE_REASON_EXPECTANCY),
    cost_adjusted_edge_failed: failureReasons.includes(FAILURE_REASON_COST_ADJUSTED_EDGE),
    out_of_sample_failed: failureReasons.includes(FAILURE_REASON_OUT_OF_SAMPLE),
    sample_quality_failed: failureReasons.includes(FAILURE_REASON_SAMPLE_QUALITY),
    parameter_stability_failed: failureReasons.includes(FAILURE_REASON_PARAMETER_STABILITY),
    validation_metrics_missing: failureReasons.includes(FAILURE_REASON_VALIDATION_METRICS_MISSING),
  };
}

/**
 * Parse a strategy_fit.json artifact (strategy-fit-v1 schema).
 *
 * Never throws. Returns one of: ok / unsupported_schema / malformed / missing_fields.
 * recommended_for_live is never invented as true: if absent, recommended_for_live=false
 * and recommended_for_live_present=false so callers can render "not reported" honestly
 * rather than a fabricated pass/fail. If explicitly present (even true), it is passed
 * through unchanged so an invariant violation in the artifact is visible, not hidden.
 */
export function parseStrategyFit(json: string): StrategyFitParseResult {
  let obj: unknown;
  try {
    obj = JSON.parse(json);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return { kind: "malformed", message: `strategy_fit.json: invalid JSON (${message})` };
  }
  if (!obj || typeof obj !== "object" || Array.isArray(obj)) {
    return { kind: "malformed", message: "strategy_fit.json: expected a JSON object" };
  }
  const a = obj as Record<string, unknown>;

  const schemaVersion = typeof a.schema_version === "string" ? a.schema_version : null;
  if (schemaVersion !== STRATEGY_FIT_SCHEMA_VERSION) {
    return { kind: "unsupported_schema", schemaVersion };
  }

  const symbol = a.symbol;
  if (typeof symbol !== "string") {
    return { kind: "missing_fields", message: "strategy_fit.json: missing symbol" };
  }
  const strategyId = a.strategy_id;
  if (typeof strategyId !== "string") {
    return { kind: "missing_fields", message: "strategy_fit.json: missing strategy_id" };
  }
  const recommendedForPaper = a.recommended_for_paper;
  if (typeof recommendedForPaper !== "boolean") {
    return { kind: "missing_fields", message: "strategy_fit.json: missing recommended_for_paper" };
  }

  const recommendedForLivePresent = typeof a.recommended_for_live === "boolean";
  const recommendedForLive = recommendedForLivePresent ? (a.recommended_for_live as boolean) : false;

  const failureReasons = Array.isArray(a.failure_reasons)
    ? a.failure_reasons.filter((r): r is string => typeof r === "string")
    : [];

  const data: StrategyFitArtifact = {
    schema_version: schemaVersion,
    artifact_id: typeof a.artifact_id === "string" ? a.artifact_id : null,
    symbol,
    strategy_id: strategyId,
    timeframe: typeof a.timeframe === "string" ? a.timeframe : null,
    trades: typeof a.trades === "number" ? a.trades : null,
    profit_factor: typeof a.profit_factor === "number" ? a.profit_factor : null,
    expectancy_bps: typeof a.expectancy_bps === "number" ? a.expectancy_bps : null,
    net_expectancy_after_cost_bps:
      typeof a.net_expectancy_after_cost_bps === "number" ? a.net_expectancy_after_cost_bps : null,
    recommended_for_paper: recommendedForPaper,
    recommended_for_live: recommendedForLive,
    recommended_for_live_present: recommendedForLivePresent,
    failure_reasons: failureReasons,
    gateFlags: deriveStrategyFitGateFlags(failureReasons),
  };

  return { kind: "ok", data };
}

// ---------------------------------------------------------------------------
// WATCHLIST-PROMOTION-GUI-SURFACE-BUNDLE-01: paper-readiness-v1 chain result parser
// ---------------------------------------------------------------------------

export const PAPER_READINESS_SCHEMA_VERSION = "paper-readiness-v1";

// Status vocabulary — must match research-py/src/mqk_research/scanner/paper_readiness_runner.py
export const PAPER_READINESS_STATUS_BLOCKED = "blocked";
export const PAPER_READINESS_STATUS_PARTIAL = "partial";
export const PAPER_READINESS_STATUS_READY_FOR_OPERATOR_REVIEW = "ready_for_operator_review";
export const PAPER_READINESS_STATUS_READY_FOR_PAPER_HANDOFF = "ready_for_paper_handoff";

// Hard-invariant anomaly ids — surfaced when a loaded artifact reports an
// unsafe value for a field the producer's to_dict() always forces safe.
export const ANOMALY_APPROVED_FOR_LIVE_TRUE = "approved_for_live_true";
export const ANOMALY_LIVE_LOCKED_FALSE = "live_locked_false";
export const ANOMALY_DAEMON_ENFORCEMENT_EXECUTED_TRUE = "daemon_enforcement_executed_true";
export const ANOMALY_PAPER_HANDOFF_EXECUTED_TRUE = "paper_handoff_executed_true";

function asNullableStringRecord(value: unknown): Record<string, string | null> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const out: Record<string, string | null> = {};
  for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
    if (typeof v === "string" || v === null) out[k] = v;
  }
  return out;
}

function asBooleanRecord(value: unknown): Record<string, boolean> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const out: Record<string, boolean> = {};
  for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
    if (typeof v === "boolean") out[k] = v;
  }
  return out;
}

/**
 * Parse a readiness_report.json artifact (paper-readiness-v1 schema).
 *
 * Never throws. Returns one of: ok / unsupported_schema / malformed / missing_fields.
 * Hard-invariant fields (approved_for_live, live_locked,
 * daemon_enforcement_executed, paper_handoff_executed) default to their safe
 * value if absent or of the wrong type. If an artifact explicitly reports an
 * unsafe value (approved_for_live=true, live_locked=false,
 * daemon_enforcement_executed=true, paper_handoff_executed=true), the unsafe
 * value is passed through unchanged and recorded in hardInvariantAnomalies so
 * an invariant violation in the artifact is visible, not hidden.
 */
export function parsePaperReadiness(json: string): PaperReadinessParseResult {
  let obj: unknown;
  try {
    obj = JSON.parse(json);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return { kind: "malformed", message: `readiness_report.json: invalid JSON (${message})` };
  }
  if (!obj || typeof obj !== "object" || Array.isArray(obj)) {
    return { kind: "malformed", message: "readiness_report.json: expected a JSON object" };
  }
  const a = obj as Record<string, unknown>;

  const schemaVersion = typeof a.schema_version === "string" ? a.schema_version : null;
  if (schemaVersion !== PAPER_READINESS_SCHEMA_VERSION) {
    return { kind: "unsupported_schema", schemaVersion };
  }

  const status = a.status;
  if (typeof status !== "string") {
    return { kind: "missing_fields", message: "readiness_report.json: missing status" };
  }

  if (!Array.isArray(a.reasons)) {
    return { kind: "missing_fields", message: "readiness_report.json: missing reasons" };
  }
  const reasons = a.reasons.filter((r): r is string => typeof r === "string");

  const hardInvariantAnomalies: string[] = [];

  const approvedForLive = a.approved_for_live === true;
  if (approvedForLive) hardInvariantAnomalies.push(ANOMALY_APPROVED_FOR_LIVE_TRUE);

  const liveLocked = a.live_locked !== false;
  if (a.live_locked === false) hardInvariantAnomalies.push(ANOMALY_LIVE_LOCKED_FALSE);

  const daemonEnforcementExecuted = a.daemon_enforcement_executed === true;
  if (daemonEnforcementExecuted) hardInvariantAnomalies.push(ANOMALY_DAEMON_ENFORCEMENT_EXECUTED_TRUE);

  const paperHandoffExecuted = a.paper_handoff_executed === true;
  if (paperHandoffExecuted) hardInvariantAnomalies.push(ANOMALY_PAPER_HANDOFF_EXECUTED_TRUE);

  const data: PaperReadinessReport = {
    schema_version: schemaVersion,
    status,
    reasons,
    top_symbol: typeof a.top_symbol === "string" ? a.top_symbol : null,
    symbol_inputs_status: typeof a.symbol_inputs_status === "string" ? a.symbol_inputs_status : null,
    risk_simulation_passed: typeof a.risk_simulation_passed === "boolean" ? a.risk_simulation_passed : null,
    premarket_revalidation_passed:
      typeof a.premarket_revalidation_passed === "boolean" ? a.premarket_revalidation_passed : null,
    promotion_passed: typeof a.promotion_passed === "boolean" ? a.promotion_passed : null,
    approved_for_autonomous_paper: a.approved_for_autonomous_paper === true,
    approved_for_live: approvedForLive,
    live_locked: liveLocked,
    daemon_enforcement_executed: daemonEnforcementExecuted,
    paper_handoff_requested: a.paper_handoff_requested === true,
    paper_handoff_executed: paperHandoffExecuted,
    upstream_pipeline_toggles: asBooleanRecord(a.upstream_pipeline_toggles),
    artifacts_read: asNullableStringRecord(a.artifacts_read),
    artifacts_written: asNullableStringRecord(a.artifacts_written),
    notes: typeof a.notes === "string" ? a.notes : "",
    hardInvariantAnomalies,
  };

  return { kind: "ok", data };
}

// ---------------------------------------------------------------------------
// WATCHLIST-PROMOTION-DETAIL-GUI-SURFACE-BUNDLE-02: watchlist-v1 promotion artifact parser
// ---------------------------------------------------------------------------

export const WATCHLIST_PROMOTION_SCHEMA_VERSION = "watchlist-v1";

// Hard-invariant anomaly id — surfaced when a loaded promoted_watchlist.json
// reports an unsafe value for a field apply_watchlist_promotion() always forces.
export const ANOMALY_WATCHLIST_APPROVED_FOR_LIVE_TRUE = "watchlist_approved_for_live_true";

function asStringRecord(value: unknown): Record<string, string> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
    if (typeof v === "string") out[k] = v;
  }
  return out;
}

function asNumberRecord(value: unknown): Record<string, number> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const out: Record<string, number> = {};
  for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
    if (typeof v === "number") out[k] = v;
  }
  return out;
}

function nullableString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function nullableNumber(value: unknown): number | null {
  return typeof value === "number" ? value : null;
}

function asRankedCandidates(value: unknown): RankedCandidate[] {
  if (!Array.isArray(value)) return [];
  const out: RankedCandidate[] = [];
  for (const item of value) {
    if (!item || typeof item !== "object" || Array.isArray(item)) continue;
    const c = item as Record<string, unknown>;
    out.push({
      rank: nullableNumber(c.rank),
      symbol: nullableString(c.symbol),
      strategy_id: nullableString(c.strategy_id),
      timeframe: nullableString(c.timeframe),
      total_score: nullableNumber(c.total_score),
      liquidity_score: nullableNumber(c.liquidity_score),
      regime_label: nullableString(c.regime_label),
      regime_score: nullableNumber(c.regime_score),
      risk_score: nullableNumber(c.risk_score),
      net_expectancy_after_cost_bps: nullableNumber(c.net_expectancy_after_cost_bps),
      paper_qty_limit: nullableNumber(c.paper_qty_limit),
      notional_limit_usd: nullableNumber(c.notional_limit_usd),
      selection_reason: nullableString(c.selection_reason),
      source_candidate_artifact: nullableString(c.source_candidate_artifact),
    });
  }
  return out;
}

function asPromotionDecision(value: unknown): WatchlistPromotionDecision | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const d = value as Record<string, unknown>;
  if (typeof d.passed !== "boolean") return null;
  const failureReasons = Array.isArray(d.failure_reasons)
    ? d.failure_reasons.filter((r): r is string => typeof r === "string")
    : [];
  const approvedSymbols = Array.isArray(d.approved_symbols)
    ? d.approved_symbols.filter((s): s is string => typeof s === "string")
    : [];
  return {
    passed: d.passed,
    failure_reasons: failureReasons,
    approved_symbols: approvedSymbols,
    strategy_assignments: asStringRecord(d.strategy_assignments),
    notes: typeof d.notes === "string" ? d.notes : "",
  };
}

function deriveTopSymbol(rankedCandidates: RankedCandidate[], symbols: string[]): string | null {
  const rankOne = rankedCandidates.find((c) => c.rank === 1 && c.symbol !== null);
  if (rankOne) return rankOne.symbol;
  return symbols.length > 0 ? symbols[0] : null;
}

/**
 * Parse a promoted_watchlist.json artifact (watchlist-v1 schema).
 *
 * Never throws. Returns one of: ok / unsupported_schema / malformed / missing_fields.
 * approved_for_live is a hard invariant always forced false by the producer
 * (apply_watchlist_promotion). If a loaded artifact reports true, the value is
 * passed through unchanged and recorded in hardInvariantAnomalies rather than
 * silently corrected or hidden.
 *
 * top_symbol is not a literal field in this artifact — it is derived from the
 * rank-1 entry of ranked_candidates, falling back to symbols[0].
 */
export function parseWatchlistPromotion(json: string): WatchlistPromotionParseResult {
  let obj: unknown;
  try {
    obj = JSON.parse(json);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return { kind: "malformed", message: `promoted_watchlist.json: invalid JSON (${message})` };
  }
  if (!obj || typeof obj !== "object" || Array.isArray(obj)) {
    return { kind: "malformed", message: "promoted_watchlist.json: expected a JSON object" };
  }
  const a = obj as Record<string, unknown>;

  const schemaVersion = typeof a.schema_version === "string" ? a.schema_version : null;
  if (schemaVersion !== WATCHLIST_PROMOTION_SCHEMA_VERSION) {
    return { kind: "unsupported_schema", schemaVersion };
  }

  if (!Array.isArray(a.symbols)) {
    return { kind: "missing_fields", message: "promoted_watchlist.json: missing symbols" };
  }
  const symbols = a.symbols.filter((s): s is string => typeof s === "string");

  if (typeof a.approved_for_autonomous_paper !== "boolean") {
    return { kind: "missing_fields", message: "promoted_watchlist.json: missing approved_for_autonomous_paper" };
  }

  const promotionDecision = asPromotionDecision(a.promotion_decision);
  if (promotionDecision === null) {
    return { kind: "missing_fields", message: "promoted_watchlist.json: missing promotion_decision" };
  }

  const hardInvariantAnomalies: string[] = [];
  const approvedForLive = a.approved_for_live === true;
  if (approvedForLive) hardInvariantAnomalies.push(ANOMALY_WATCHLIST_APPROVED_FOR_LIVE_TRUE);

  const rankedCandidates = asRankedCandidates(a.ranked_candidates);

  const data: WatchlistPromotionArtifact = {
    schema_version: schemaVersion,
    mode: nullableString(a.mode),
    generated_at_utc: nullableString(a.generated_at_utc),
    trade_date: nullableString(a.trade_date),
    approved_for_autonomous_paper: a.approved_for_autonomous_paper,
    approved_for_live: approvedForLive,
    max_symbols_to_trade: nullableNumber(a.max_symbols_to_trade),
    max_concurrent_positions: nullableNumber(a.max_concurrent_positions),
    symbols,
    strategy_assignments: asStringRecord(a.strategy_assignments),
    paper_qty_limits: asNumberRecord(a.paper_qty_limits),
    notional_limits: asNumberRecord(a.notional_limits),
    ranked_candidates: rankedCandidates,
    selection_reason: nullableString(a.selection_reason),
    promotion_decision: promotionDecision,
    top_symbol: deriveTopSymbol(rankedCandidates, symbols),
    hardInvariantAnomalies,
  };

  return { kind: "ok", data };
}

// ---------------------------------------------------------------------------
// WATCHLIST-PROMOTION-DETAIL-GUI-SURFACE-BUNDLE-02: premarket-revalidation-v1 artifact parser
// ---------------------------------------------------------------------------

export const PREMARKET_REVALIDATION_SCHEMA_VERSION = "premarket-revalidation-v1";

function asSymbolPremarketResults(value: unknown): Record<string, SymbolPremarketResult> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const out: Record<string, SymbolPremarketResult> = {};
  for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
    if (!v || typeof v !== "object" || Array.isArray(v)) continue;
    const r = v as Record<string, unknown>;
    if (typeof r.passed !== "boolean") continue;
    const failureReasons = Array.isArray(r.failure_reasons)
      ? r.failure_reasons.filter((x): x is string => typeof x === "string")
      : [];
    out[k] = { passed: r.passed, failure_reasons: failureReasons };
  }
  return out;
}

/**
 * Parse a premarket_revalidation.json artifact (premarket-revalidation-v1 schema).
 *
 * Never throws. Returns one of: ok / unsupported_schema / malformed / missing_fields.
 * Per-symbol results carry only `passed` and `failure_reasons` — the producer
 * (PremarketRevalidationResult.to_dict()) does not emit per-check booleans
 * (e.g. data freshness, spread, volume); failure reason strings encode which
 * check failed.
 */
export function parsePremarketRevalidation(json: string): PremarketRevalidationParseResult {
  let obj: unknown;
  try {
    obj = JSON.parse(json);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return { kind: "malformed", message: `premarket_revalidation.json: invalid JSON (${message})` };
  }
  if (!obj || typeof obj !== "object" || Array.isArray(obj)) {
    return { kind: "malformed", message: "premarket_revalidation.json: expected a JSON object" };
  }
  const a = obj as Record<string, unknown>;

  const schemaVersion = typeof a.schema_version === "string" ? a.schema_version : null;
  if (schemaVersion !== PREMARKET_REVALIDATION_SCHEMA_VERSION) {
    return { kind: "unsupported_schema", schemaVersion };
  }

  if (typeof a.passed !== "boolean") {
    return { kind: "missing_fields", message: "premarket_revalidation.json: missing passed" };
  }

  if (!Array.isArray(a.failure_reasons)) {
    return { kind: "missing_fields", message: "premarket_revalidation.json: missing failure_reasons" };
  }
  const failureReasons = a.failure_reasons.filter((r): r is string => typeof r === "string");

  if (!a.symbol_results || typeof a.symbol_results !== "object" || Array.isArray(a.symbol_results)) {
    return { kind: "missing_fields", message: "premarket_revalidation.json: missing symbol_results" };
  }

  const data: PremarketRevalidationArtifact = {
    schema_version: schemaVersion,
    passed: a.passed,
    failure_reasons: failureReasons,
    top_symbol: typeof a.top_symbol === "string" ? a.top_symbol : null,
    symbol_results: asSymbolPremarketResults(a.symbol_results),
    notes: typeof a.notes === "string" ? a.notes : "",
  };

  return { kind: "ok", data };
}

// ---------------------------------------------------------------------------
// GUI-EVIDENCE-DISCORD-LINKS-SURFACE-BUNDLE-01: review-v2 evidence review parser
// ---------------------------------------------------------------------------

export const EVIDENCE_REVIEW_SCHEMA_VERSION = "review-v2";

/**
 * Parse a review_summary.json artifact (review-v2 schema, produced by
 * scripts/windows/Review-PaperSmokeEvidence.ps1).
 *
 * Never throws. Returns one of: ok / unsupported_schema / malformed / missing_fields.
 *
 * `classification` is passed through exactly as reported (e.g.
 * NATURAL-TRADE-LIFECYCLE-CLOSED, READINESS-CLOSED-NO-TRADE, PARTIAL, OPEN,
 * FALSE-CLOSED) -- never fabricated, inferred, or upgraded toward a closed
 * state. Safety-relevant fields (kill_switch_active, integrity_halt_active,
 * risk_halt_active, live_routing_enabled, autonomous_flatten_available)
 * default to null ("not reported by this review") when absent or of the
 * wrong type, rather than a fabricated false/healthy default.
 */
export function parseEvidenceReview(json: string): EvidenceReviewParseResult {
  let obj: unknown;
  try {
    obj = JSON.parse(json);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return { kind: "malformed", message: `review_summary.json: invalid JSON (${message})` };
  }
  if (!obj || typeof obj !== "object" || Array.isArray(obj)) {
    return { kind: "malformed", message: "review_summary.json: expected a JSON object" };
  }
  const a = obj as Record<string, unknown>;

  const schemaVersion = typeof a.schema_version === "string" ? a.schema_version : null;
  if (schemaVersion !== EVIDENCE_REVIEW_SCHEMA_VERSION) {
    return { kind: "unsupported_schema", schemaVersion };
  }

  const classification = a.classification;
  if (typeof classification !== "string") {
    return { kind: "missing_fields", message: "review_summary.json: missing classification" };
  }

  const classificationReasons = Array.isArray(a.classification_reasons)
    ? a.classification_reasons.filter((r): r is string => typeof r === "string")
    : [];

  const nullableBoolean = (value: unknown): boolean | null => (typeof value === "boolean" ? value : null);

  const data: EvidenceReviewArtifact = {
    schema_version: schemaVersion,
    classification,
    classification_reasons: classificationReasons,
    folder_name: nullableString(a.folder_name),
    reviewed_at: nullableString(a.reviewed_at),
    capture_ts: nullableString(a.capture_ts),
    runtime_status: nullableString(a.runtime_status),
    arm_state: nullableString(a.arm_state),
    kill_switch_active: nullableBoolean(a.kill_switch_active),
    integrity_halt_active: nullableBoolean(a.integrity_halt_active),
    risk_halt_active: nullableBoolean(a.risk_halt_active),
    live_routing_enabled: nullableBoolean(a.live_routing_enabled),
    deadman_status: nullableString(a.deadman_status),
    reconcile_status: nullableString(a.reconcile_status),
    reconcile_total_mismatches: nullableNumber(a.reconcile_total_mismatches),
    fill_count: nullableNumber(a.fill_count),
    open_order_count: nullableNumber(a.open_order_count),
    position_count: nullableNumber(a.position_count),
    autonomous_flatten_available: nullableBoolean(a.autonomous_flatten_available),
    autonomous_flatten_blockers: nullableString(a.autonomous_flatten_blockers),
    autonomous_next_operator_action: nullableString(a.autonomous_next_operator_action),
  };

  return { kind: "ok", data };
}

// ---------------------------------------------------------------------------
// GUI-EVIDENCE-DISCORD-LINKS-SURFACE-BUNDLE-01: static Discord workflow guidance
//
// This is reference documentation only. It does not read .env.local, send
// Discord alerts, call any webhook, or invoke these scripts. Commands and
// safety notes mirror the verbatim behavior documented in the corresponding
// scripts/windows/*.ps1 headers and docs/runbooks/operator_control_surface.md
// section 8 ("Discord Observation Checklist").
// ---------------------------------------------------------------------------

export interface DiscordWorkflowGuidance {
  name: string;
  purpose: string;
  checkOnlyCommand: string;
  sendsInNormalMode: string;
  requiredEnv: string[];
  safetyNotes: string[];
}

export const DISCORD_WORKFLOWS: DiscordWorkflowGuidance[] = [
  {
    name: "Test-DiscordAlert.ps1",
    purpose:
      "Verify Discord alert delivery configuration without starting the daemon trading runtime, arming paper trading, or touching broker/Alpaca endpoints.",
    checkOnlyCommand: "powershell -ExecutionPolicy Bypass -File scripts\\windows\\Test-DiscordAlert.ps1 -CheckOnly",
    sendsInNormalMode:
      "Normal mode sends one [TEST] Discord alert via the daemon route POST /api/v1/ops/action {\"action_key\":\"test-discord-alert\"}. -CheckOnly never issues this request.",
    requiredEnv: ["DISCORD_WEBHOOK_URL", "MQK_OPERATOR_TOKEN"],
    safetyNotes: [
      "Does not start the daemon trading runtime or arm paper trading.",
      "Does not submit, cancel, or replace any order.",
      "Does not call any broker or market-data endpoint, and does not write to the database.",
      "Only checks whether DISCORD_WEBHOOK_URL and MQK_OPERATOR_TOKEN are configured (presence only) -- values are never printed.",
    ],
  },
  {
    name: "Send-PaperReadinessDiscordAlert.ps1",
    purpose:
      "Send a sanitized Discord summary for one offline paper-readiness-v1 or strategy-fit-v1 artifact JSON file.",
    checkOnlyCommand:
      "powershell -ExecutionPolicy Bypass -File scripts\\windows\\Send-PaperReadinessDiscordAlert.ps1 -ArtifactPath <path> -CheckOnly",
    sendsInNormalMode:
      "Normal mode sends exactly one sanitized summary directly to DISCORD_WEBHOOK_URL (the daemon is never contacted). -CheckOnly never issues this POST.",
    requiredEnv: ["DISCORD_WEBHOOK_URL"],
    safetyNotes: [
      "Operator-triggered only; never invoked automatically by any pipeline.",
      "Does not start the daemon trading runtime, arm paper trading, or submit/cancel/replace any order.",
      "Does not call any broker or market-data endpoint, and does not write to the database.",
      "Sends the artifact file name only -- never the local path -- and never sends raw artifact JSON.",
      "Refuses to send (fail-closed) if the artifact reports recommended_for_live, approved_for_live, or eligible_for_live = true, or appears to contain a webhook URL or secret/token.",
      "DISCORD_WEBHOOK_URL value is never printed.",
    ],
  },
  {
    name: "Send-PaperSmokeReviewDiscordAlert.ps1",
    purpose:
      "Send a sanitized Discord summary for one offline paper-smoke evidence review (review_summary.json, preferred, or review_summary.md).",
    checkOnlyCommand:
      "powershell -ExecutionPolicy Bypass -File scripts\\windows\\Send-PaperSmokeReviewDiscordAlert.ps1 -ReviewPath <path> -CheckOnly",
    sendsInNormalMode:
      "Normal mode sends exactly one sanitized summary directly to DISCORD_WEBHOOK_URL (the daemon is never contacted). -CheckOnly never issues this POST.",
    requiredEnv: ["DISCORD_WEBHOOK_URL"],
    safetyNotes: [
      "Operator-triggered only; never invoked automatically by any pipeline.",
      "Does not start the daemon trading runtime, arm paper trading, or submit/cancel/replace any order.",
      "Does not call any broker or market-data endpoint, write to the database, or change evidence classification logic / paper-readiness gates.",
      "Sends the evidence folder name and review file name only -- never the local path -- and never sends raw review contents.",
      "Refuses to send (fail-closed) if recommended_for_live, approved_for_live, or eligible_for_live = true is present, a webhook URL or secret/token appears in the review, or the classification is missing/unrecognized.",
      "DISCORD_WEBHOOK_URL value is never printed.",
    ],
  },
];
