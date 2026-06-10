import type {
  BacktestManifest,
  BacktestMetrics,
  EquityCurveRow,
  FillRow,
  OrderRow,
  ParsedCsvResult,
  StrategyFitArtifact,
  StrategyFitGateFlags,
  StrategyFitParseResult,
} from "./types.ts";

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

export function parseEquityCurve(csv: string): ParsedCsvResult<EquityCurveRow> {
  const { rows } = parseCsvRows(csv);
  let malformed = 0;
  const parsed: EquityCurveRow[] = [];
  for (const r of rows) {
    const equity = Number(r.equity);
    if (!Object.prototype.hasOwnProperty.call(r, "equity") || Number.isNaN(equity)) {
      malformed++;
      continue;
    }
    parsed.push({ ts_utc: r.ts_utc ?? "", equity });
  }
  return { rows: parsed, malformed };
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
