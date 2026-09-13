// OT-MQD-01: Interactive Evidence Chart — pure adapter from ArtifactBundle to
// a provenance-aware chart model. No I/O, no daemon calls, no fabrication.
//
// EVIDENCE-SOURCE MAP (ArtifactBundle, as loaded by BacktestResultsScreen):
//   AUTHORITATIVE NOW — equity_curve.csv (equity), orders.csv (order intents),
//     fills.csv (fills + fee-derived cost events), manifest.json/metrics.json
//     (run/strategy/symbol/timeframe identity).
//   PARTIAL — drawdown is a client-derived display series computed from
//     equity_curve.csv (mirrors the existing DrawdownSection/
//     computeDrawdownSeries pattern) — never a substitute for metrics.json's
//     authoritative max_drawdown_pct.
//   NOT WIRED — OHLC price bars (no bars artifact is exposed to this screen),
//     strategy signal events, entry/exit classification (orders/fills carry
//     `side`, not a distinct entry/exit tag), walk-forward fold boundaries,
//     out-of-sample boundary timestamps (strategy_fit.json reports OOS
//     PASS/FAIL only, never boundary timestamps), risk events. These lanes
//     are backend/artifact prerequisites deferred out of this GUI-only wave.
//
// Every lane function that has no authoritative source in this wave (price,
// signals, entry/exit, fold/OOS regions) takes NO bundle argument at all —
// it is structurally impossible for it to derive a marker from input data,
// which is the strongest available proof it cannot manufacture evidence.

import type {
  ArtifactBundle,
  BacktestManifest,
  BacktestMetrics,
  EquityCurveRow,
  FileResult,
  FillRow,
  OrderRow,
  ParsedCsvResult,
} from "../../features/backtests/types.ts";
import { computeDrawdownSeries, manifestTimeframeLabel } from "../../features/backtests/parsers.ts";

// ---------------------------------------------------------------------------
// Status vocabulary
// ---------------------------------------------------------------------------

/**
 * "authoritative_empty" is distinct from "unavailable": an artifact that
 * loaded successfully and reported zero rows is a true authoritative empty
 * result, not a truth gap. Collapsing the two would let a read failure
 * silently render as "no events occurred".
 */
export type LaneStatus =
  | "authoritative"
  | "authoritative_empty"
  | "not_wired"
  | "unavailable"
  | "idle"
  | "loading";

export interface LaneProvenance {
  artifact: string;
  runId: string | null;
  symbol: string | null;
  timeframe: string | null;
}

function laneStatusFromFileResult<T>(
  result: FileResult<T>,
  isEmpty: (data: T) => boolean,
): LaneStatus {
  switch (result.kind) {
    case "idle":
      return "idle";
    case "loading":
      return "loading";
    case "ok":
      return isEmpty(result.data) ? "authoritative_empty" : "authoritative";
    case "missing":
    case "parse_error":
    case "read_error":
      return "unavailable";
  }
}

function unavailableReason<T>(result: FileResult<T>, artifactLabel: string): string | null {
  switch (result.kind) {
    case "missing":
      return `${artifactLabel} not found in artifact folder.`;
    case "parse_error":
      return `${artifactLabel} parse error: ${result.message}`;
    case "read_error":
      return `${artifactLabel} read error: ${result.message}`;
    default:
      return null;
  }
}

/**
 * Parses an RFC3339-ish timestamp to epoch ms. Returns null (never a
 * guessed/default value) for empty, missing, or unparsable input — callers
 * must treat null as "unknown timestamp", never as "place at bar 0".
 */
function parseTsMs(ts: string | null | undefined): number | null {
  if (ts == null) return null;
  const trimmed = ts.trim();
  if (trimmed === "") return null;
  const ms = Date.parse(trimmed);
  return Number.isFinite(ms) ? ms : null;
}

// ---------------------------------------------------------------------------
// Identity / time range
// ---------------------------------------------------------------------------

export interface EvidenceChartIdentity {
  runId: string | null;
  strategyId: string | null;
  symbols: string[];
  timeframeLabel: string | null;
  engineId: string | null;
  mode: string | null;
}

function buildIdentity(
  manifest: BacktestManifest | null,
  metrics: BacktestMetrics | null,
): EvidenceChartIdentity {
  return {
    runId: manifest?.run_id ?? metrics?.run_id ?? null,
    strategyId: manifest?.strategy_name ?? metrics?.strategy_name ?? null,
    symbols: metrics?.symbols ?? [],
    timeframeLabel: manifestTimeframeLabel(manifest?.timeframe, manifest?.timeframe_secs),
    engineId: manifest?.engine_id ?? null,
    mode: manifest?.mode ?? null,
  };
}

export interface EvidenceChartTimeRange {
  status: LaneStatus;
  startTsUtc: string | null;
  endTsUtc: string | null;
  startMs: number | null;
  endMs: number | null;
}

function buildTimeRange(equityResult: FileResult<ParsedCsvResult<EquityCurveRow>>): EvidenceChartTimeRange {
  const status = laneStatusFromFileResult(equityResult, (d) => d.rows.length === 0);
  if (equityResult.kind !== "ok" || equityResult.data.rows.length === 0) {
    return { status, startTsUtc: null, endTsUtc: null, startMs: null, endMs: null };
  }
  const rows = equityResult.data.rows;
  const startTsUtc = rows[0].ts_utc || null;
  const endTsUtc = rows[rows.length - 1].ts_utc || null;
  return {
    status,
    startTsUtc,
    endTsUtc,
    startMs: parseTsMs(startTsUtc),
    endMs: parseTsMs(endTsUtc),
  };
}

/**
 * Maps a timestamp into [0, 1] within the chart's authoritative time range.
 * Returns null (never a guessed position) when the range or the timestamp
 * itself cannot be resolved to a finite instant.
 */
export function timeFraction(range: EvidenceChartTimeRange, tsMs: number | null): number | null {
  if (tsMs == null || range.startMs == null || range.endMs == null) return null;
  if (range.endMs === range.startMs) return 0.5;
  const f = (tsMs - range.startMs) / (range.endMs - range.startMs);
  if (!Number.isFinite(f)) return null;
  return Math.min(1, Math.max(0, f));
}

// ---------------------------------------------------------------------------
// Series lanes (equity, drawdown, price)
// ---------------------------------------------------------------------------

export interface EvidencePoint {
  tsUtc: string;
  tsMs: number | null;
  value: number;
}

export interface EvidenceSeriesLane {
  status: LaneStatus;
  points: EvidencePoint[];
  malformedRowCount: number;
  provenance: LaneProvenance;
  reason: string | null;
}

function buildEquityLane(
  equityResult: FileResult<ParsedCsvResult<EquityCurveRow>>,
  identity: EvidenceChartIdentity,
): EvidenceSeriesLane {
  const status = laneStatusFromFileResult(equityResult, (d) => d.rows.length === 0);
  const provenance: LaneProvenance = {
    artifact: "equity_curve.csv",
    runId: identity.runId,
    symbol: identity.symbols.length > 0 ? identity.symbols.join(",") : null,
    timeframe: identity.timeframeLabel,
  };
  if (equityResult.kind !== "ok") {
    return { status, points: [], malformedRowCount: 0, provenance, reason: unavailableReason(equityResult, "equity_curve.csv") };
  }
  const points = equityResult.data.rows.map((r) => ({
    tsUtc: r.ts_utc,
    tsMs: parseTsMs(r.ts_utc),
    value: r.equity,
  }));
  return { status, points, malformedRowCount: equityResult.data.malformed, provenance, reason: null };
}

/**
 * Drawdown is derived from equity_curve.csv, mirroring the existing
 * computeDrawdownSeries used by BacktestResultsScreen's DrawdownSection. It
 * is a display-only recomputation, never a second source of truth — its
 * status is structurally tied to the equity lane's own status so it can
 * never claim authority the source lane doesn't have.
 */
function buildDrawdownLane(
  equityResult: FileResult<ParsedCsvResult<EquityCurveRow>>,
  identity: EvidenceChartIdentity,
): EvidenceSeriesLane {
  const status = laneStatusFromFileResult(equityResult, (d) => d.rows.length === 0);
  const provenance: LaneProvenance = {
    artifact: "equity_curve.csv (derived: drawdown)",
    runId: identity.runId,
    symbol: identity.symbols.length > 0 ? identity.symbols.join(",") : null,
    timeframe: identity.timeframeLabel,
  };
  if (equityResult.kind !== "ok") {
    return { status, points: [], malformedRowCount: 0, provenance, reason: unavailableReason(equityResult, "equity_curve.csv") };
  }
  const series = computeDrawdownSeries(equityResult.data.rows);
  const points = series.map((p) => ({
    tsUtc: p.ts_utc,
    tsMs: parseTsMs(p.ts_utc),
    value: p.drawdown_pct,
  }));
  return {
    status,
    points,
    malformedRowCount: equityResult.data.malformed,
    provenance,
    reason: status === "authoritative" || status === "authoritative_empty"
      ? "Derived from equity_curve.csv (display-only) — not a substitute for metrics.json's authoritative max_drawdown_pct."
      : null,
  };
}

export interface PriceLane {
  status: "not_wired";
  bars: never[];
  reason: string;
}

/**
 * Negative control A: takes no arguments. There is no way for this function
 * to read a bar out of the artifact bundle — it cannot manufacture price
 * data regardless of what BacktestResultsScreen loads.
 */
function buildPriceLane(): PriceLane {
  return {
    status: "not_wired",
    bars: [],
    reason:
      "No authoritative OHLC price-bar artifact/API seam is exposed to BacktestResultsScreen in this wave. " +
      "Deferred prerequisite: a backend/artifact route exposing bars for a completed backtest run.",
  };
}

// ---------------------------------------------------------------------------
// Marker lanes (orders, fills, costs) + always-not-wired lanes
// ---------------------------------------------------------------------------

export type MarkerTimeStatus = "known" | "unknown";

export interface EvidenceMarker {
  id: string;
  kind: "order_intent" | "fill" | "cost";
  tsUtc: string;
  tsMs: number | null;
  timeStatus: MarkerTimeStatus;
  label: string;
  symbol: string | null;
  side: string | null;
  qty: string | null;
  price: number | null;
  feeUsd: number | null;
  orderStatus: string | null;
  provenance: LaneProvenance;
}

export interface EvidenceMarkerLane {
  status: LaneStatus;
  markers: EvidenceMarker[];
  malformedRowCount: number;
  reason: string | null;
}

function parseFiniteNumber(raw: string | undefined): number | null {
  if (raw == null) return null;
  const trimmed = raw.trim();
  if (trimmed === "") return null;
  const n = Number(trimmed);
  return Number.isFinite(n) ? n : null;
}

function buildOrderIntentLane(
  ordersResult: FileResult<ParsedCsvResult<OrderRow>>,
  identity: EvidenceChartIdentity,
): EvidenceMarkerLane {
  const status = laneStatusFromFileResult(ordersResult, (d) => d.rows.length === 0);
  if (ordersResult.kind !== "ok") {
    return { status, markers: [], malformedRowCount: 0, reason: unavailableReason(ordersResult, "orders.csv") };
  }
  const markers: EvidenceMarker[] = ordersResult.data.rows.map((r) => {
    const tsMs = parseTsMs(r.ts_utc);
    // orders.csv limit_price/stop_price are micros (mirrors OrderIntentV2's
    // limit_price_micros/stop_price_micros in core-rs/crates/mqk-execution),
    // same unit as fills.csv price/fee below — convert to dollars for display.
    const limitPriceMicros = parseFiniteNumber(r.limit_price);
    return {
      id: `order:${r.order_id}`,
      kind: "order_intent",
      tsUtc: r.ts_utc,
      tsMs,
      timeStatus: tsMs == null ? "unknown" : "known",
      label: `${r.side || "order"} ${r.qty || ""}`.trim(),
      symbol: r.symbol || null,
      side: r.side || null,
      qty: r.qty || null,
      price: limitPriceMicros != null ? limitPriceMicros / 1_000_000 : null,
      feeUsd: null,
      orderStatus: r.status || null,
      provenance: {
        artifact: "orders.csv",
        runId: identity.runId,
        symbol: r.symbol || null,
        timeframe: identity.timeframeLabel,
      },
    };
  });
  return { status, markers, malformedRowCount: ordersResult.data.malformed, reason: null };
}

/**
 * Fill and cost markers are both derived from the single literal fills.csv
 * result so neither lane can exist without a corresponding literal row —
 * this is what makes negative controls B/D structurally true: a fill marker
 * (or a cost event) can only ever originate from an actual fills.csv row,
 * never from an order intent or a research recommendation artifact.
 */
function buildFillAndCostLanes(
  fillsResult: FileResult<ParsedCsvResult<FillRow>>,
  identity: EvidenceChartIdentity,
): { fillLane: EvidenceMarkerLane; costLane: EvidenceMarkerLane } {
  const status = laneStatusFromFileResult(fillsResult, (d) => d.rows.length === 0);
  if (fillsResult.kind !== "ok") {
    const reason = unavailableReason(fillsResult, "fills.csv");
    return {
      fillLane: { status, markers: [], malformedRowCount: 0, reason },
      costLane: { status, markers: [], malformedRowCount: 0, reason },
    };
  }

  const fillMarkers: EvidenceMarker[] = [];
  const costMarkers: EvidenceMarker[] = [];
  for (const r of fillsResult.data.rows) {
    const tsMs = parseTsMs(r.ts_utc);
    const timeStatus: MarkerTimeStatus = tsMs == null ? "unknown" : "known";
    const provenance: LaneProvenance = {
      artifact: "fills.csv",
      runId: identity.runId,
      symbol: r.symbol || null,
      timeframe: identity.timeframeLabel,
    };
    const price = parseFiniteNumber(r.price);
    const feeUsd = parseFiniteNumber(r.fee);

    fillMarkers.push({
      id: `fill:${r.fill_id}`,
      kind: "fill",
      tsUtc: r.ts_utc,
      tsMs,
      timeStatus,
      label: `${r.side || "fill"} ${r.qty || ""}`.trim(),
      symbol: r.symbol || null,
      side: r.side || null,
      qty: r.qty || null,
      price: price != null ? price / 1_000_000 : null,
      feeUsd: feeUsd != null ? feeUsd / 1_000_000 : null,
      orderStatus: null,
      provenance,
    });

    // Only a literal, parsable fee value becomes a cost event — a blank or
    // unparsable fee is a malformed field, not a fabricated zero-cost fact.
    if (feeUsd != null) {
      costMarkers.push({
        id: `cost:${r.fill_id}`,
        kind: "cost",
        tsUtc: r.ts_utc,
        tsMs,
        timeStatus,
        label: `fee ${(feeUsd / 1_000_000).toFixed(2)}`,
        symbol: r.symbol || null,
        side: r.side || null,
        qty: r.qty || null,
        price: null,
        feeUsd: feeUsd / 1_000_000,
        orderStatus: null,
        provenance,
      });
    }
  }

  return {
    fillLane: { status, markers: fillMarkers, malformedRowCount: fillsResult.data.malformed, reason: null },
    costLane: { status, markers: costMarkers, malformedRowCount: fillsResult.data.malformed, reason: null },
  };
}

/** Negative control C (structural half): no signal artifact exists in ArtifactBundle — takes no arguments. */
function buildSignalMarkersLane(): EvidenceMarkerLane {
  return {
    status: "not_wired",
    markers: [],
    malformedRowCount: 0,
    reason:
      "No strategy-signal-evaluation artifact is included in ArtifactBundle for backtest runs " +
      "(distinct from the live/autonomous strategy_signal_evaluations journal).",
  };
}

function buildEntryExitMarkersLane(): EvidenceMarkerLane {
  return {
    status: "not_wired",
    markers: [],
    malformedRowCount: 0,
    reason:
      "No artifact classifies orders/fills as entry vs exit distinct from side (buy/sell) — " +
      "inferring one here would manufacture identity the source data doesn't carry.",
  };
}

export interface RegionLane {
  status: "not_wired";
  regions: never[];
  reason: string;
}

function buildFoldRegionsLane(): RegionLane {
  return {
    status: "not_wired",
    regions: [],
    reason:
      "No walk-forward fold-boundary artifact is included in ArtifactBundle for backtest runs " +
      "— walk-forward split artifacts are a separate CLI output not wired to this screen.",
  };
}

function buildOosRegionsLane(): RegionLane {
  return {
    status: "not_wired",
    regions: [],
    reason:
      "No out-of-sample boundary timestamp is exposed to BacktestResultsScreen — strategy_fit.json " +
      "reports out_of_sample_failed (pass/fail) only, never boundary timestamps.",
  };
}

// ---------------------------------------------------------------------------
// Composite model
// ---------------------------------------------------------------------------

export interface EvidenceChartModel {
  identity: EvidenceChartIdentity;
  timeRange: EvidenceChartTimeRange;
  priceSeries: PriceLane;
  equitySeries: EvidenceSeriesLane;
  drawdownSeries: EvidenceSeriesLane;
  signalMarkers: EvidenceMarkerLane;
  orderIntentMarkers: EvidenceMarkerLane;
  fillMarkers: EvidenceMarkerLane;
  entryExitMarkers: EvidenceMarkerLane;
  foldRegions: RegionLane;
  oosRegions: RegionLane;
  costEvents: EvidenceMarkerLane;
}

export function buildEvidenceChartModel(bundle: ArtifactBundle): EvidenceChartModel {
  const manifest = bundle.manifest.kind === "ok" ? bundle.manifest.data : null;
  const metrics = bundle.metrics.kind === "ok" ? bundle.metrics.data : null;
  const identity = buildIdentity(manifest, metrics);
  const { fillLane, costLane } = buildFillAndCostLanes(bundle.fills, identity);

  return {
    identity,
    timeRange: buildTimeRange(bundle.equityCurve),
    priceSeries: buildPriceLane(),
    equitySeries: buildEquityLane(bundle.equityCurve, identity),
    drawdownSeries: buildDrawdownLane(bundle.equityCurve, identity),
    signalMarkers: buildSignalMarkersLane(),
    orderIntentMarkers: buildOrderIntentLane(bundle.orders, identity),
    fillMarkers: fillLane,
    entryExitMarkers: buildEntryExitMarkersLane(),
    foldRegions: buildFoldRegionsLane(),
    oosRegions: buildOosRegionsLane(),
    costEvents: costLane,
  };
}
