// OT-MQD-01: Interactive Evidence Chart — pure adapter from ArtifactBundle to
// a provenance-aware chart model. No I/O, no daemon calls, no fabrication.
//
// TRUTH MODEL: per core-rs/crates/mqk-artifacts/src/backtest_report_artifact.rs,
// backtest_report.json (not wired to this GUI in this wave) is the canonical,
// schema-versioned, lossless authority. equity_curve.csv/orders.csv/
// fills.csv/metrics.json are derived, lossy views — real evidence when
// loaded, but never the canonical backtest authority. Lane status vocabulary
// reflects this: "artifact_present"/"artifact_empty" mean a derived GUI
// artifact loaded (with/without rows), not "this is canonical truth".
//
// EVIDENCE-SOURCE MAP (ArtifactBundle, as loaded by BacktestResultsScreen):
//   ARTIFACT EVIDENCE — equity_curve.csv (equity), orders.csv (order intents),
//     fills.csv (fills + fee-derived cost events), manifest.json/metrics.json
//     (run/strategy/symbol/timeframe identity).
//   DERIVED DISPLAY-ONLY — drawdown is a client-derived display series
//     computed from equity_curve.csv (mirrors the existing DrawdownSection/
//     computeDrawdownSeries pattern) — never a substitute for metrics.json's
//     own reported max_drawdown_pct.
//   NOT WIRED — OHLC price bars (no bars artifact is exposed to this screen),
//     strategy signal events, entry/exit classification (orders/fills carry
//     `side`, not a distinct entry/exit tag), walk-forward fold boundaries,
//     out-of-sample boundary timestamps (strategy_fit.json reports OOS
//     PASS/FAIL only, never boundary timestamps), risk events. These lanes
//     are backend/artifact prerequisites deferred out of this GUI-only wave.
//
// Every lane function that has no artifact source in this wave (price,
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
import { computeDrawdownSeries, manifestTimeframeLabel, reconcileIdentityField } from "../../features/backtests/parsers.ts";

// ---------------------------------------------------------------------------
// Status vocabulary
// ---------------------------------------------------------------------------

/**
 * These lanes render derived, lossy GUI artifacts (metrics.json/orders.csv/
 * fills.csv/equity_curve.csv) per the mqk-artifacts backtest_report_artifact
 * contract — backtest_report.json, not these files, is the canonical,
 * schema-versioned authority. "artifact_present" therefore means "this
 * derived file loaded and contains real evidence", never "this is the
 * canonical BacktestReport". "artifact_empty" is distinct from
 * "unavailable": an artifact that loaded successfully and reported zero rows
 * is a true empty result, not a truth gap. Collapsing the two would let a
 * read failure silently render as "no events occurred".
 */
export type LaneStatus =
  | "artifact_present"
  | "artifact_empty"
  | "partial"
  | "not_wired"
  | "unavailable"
  | "idle"
  | "loading";

export interface LaneProvenance {
  artifact: string;
  runId: string | null;
  /**
   * True when manifest.run_id and metrics.run_id both exist but disagree.
   * `runId` is already null in that case (neither side is authoritative) —
   * this flag lets the UI show "CONFLICT" instead of the indistinguishable
   * "not reported" for a run that genuinely has no id anywhere.
   */
  runIdConflict: boolean;
  symbol: string | null;
  timeframe: string | null;
}

interface ParsedCsvDisposition {
  status: LaneStatus;
  malformedRowCount: number;
  reason: string | null;
}

/**
 * Fail-closed disposition for parsed CSV evidence. `ParsedCsvResult.malformed`
 * is part of source truth: malformed-only input is not an empty artifact, and
 * a mixture of usable + malformed rows is partial evidence, never complete.
 */
function parsedCsvDisposition<T>(
  result: FileResult<ParsedCsvResult<T>>,
  artifactLabel: string,
): ParsedCsvDisposition {
  switch (result.kind) {
    case "idle":
      return { status: "idle", malformedRowCount: 0, reason: null };
    case "loading":
      return { status: "loading", malformedRowCount: 0, reason: null };
    case "missing":
    case "parse_error":
    case "read_error":
      return { status: "unavailable", malformedRowCount: 0, reason: unavailableReason(result, artifactLabel) };
    case "ok": {
      const usable = result.data.rows.length;
      const malformed = result.data.malformed;
      if (usable === 0 && malformed === 0) {
        return { status: "artifact_empty", malformedRowCount: 0, reason: null };
      }
      if (usable > 0 && malformed === 0) {
        return { status: "artifact_present", malformedRowCount: 0, reason: null };
      }
      if (usable > 0) {
        return {
          status: "partial",
          malformedRowCount: malformed,
          reason: `${usable} usable ${artifactLabel} row(s); ${malformed} malformed row(s) excluded.`,
        };
      }
      return {
        status: "unavailable",
        malformedRowCount: malformed,
        reason: `${artifactLabel} contained ${malformed} malformed row(s) and no usable evidence.`,
      };
    }
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
  /** manifest.run_id and metrics.run_id both exist and disagree — runId is null, never one side picked silently. */
  runIdConflict: boolean;
  /**
   * Presentation label only (source: manifest.strategy_name / metrics.strategy_name).
   * Never a durable strategy_id — this bundle carries no strategy_id source,
   * so this field must not be consumed as one.
   */
  strategyName: string | null;
  strategyNameConflict: boolean;
  symbols: string[];
  timeframeLabel: string | null;
  engineId: string | null;
  mode: string | null;
}

function buildIdentity(
  manifest: BacktestManifest | null,
  metrics: BacktestMetrics | null,
): EvidenceChartIdentity {
  const runIdField = reconcileIdentityField(manifest?.run_id, metrics?.run_id);
  const strategyNameField = reconcileIdentityField(manifest?.strategy_name, metrics?.strategy_name);
  return {
    runId: runIdField.value,
    runIdConflict: runIdField.conflict,
    strategyName: strategyNameField.value,
    strategyNameConflict: strategyNameField.conflict,
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
  const disposition = parsedCsvDisposition(equityResult, "equity_curve.csv");
  if (equityResult.kind !== "ok" || equityResult.data.rows.length === 0) {
    return { status: disposition.status, startTsUtc: null, endTsUtc: null, startMs: null, endMs: null };
  }
  const rows = equityResult.data.rows;
  const startTsUtc = rows[0].ts_utc || null;
  const endTsUtc = rows[rows.length - 1].ts_utc || null;
  return {
    status: disposition.status,
    startTsUtc,
    endTsUtc,
    startMs: parseTsMs(startTsUtc),
    endMs: parseTsMs(endTsUtc),
  };
}

/**
 * Maps a timestamp into [0, 1] within the chart's authoritative time range.
 * Returns null — meaning "do not plot", never a guessed/relocated position —
 * whenever the event does not genuinely belong inside the plotted range:
 *   - unparsable/missing timestamp or range (null inputs)
 *   - reversed/invalid range (end before start) — fails closed entirely
 *   - degenerate range (start == end) and the timestamp isn't that exact instant
 *   - a real, known timestamp that falls outside [startMs, endMs]
 * An out-of-range timestamp is a distinct fact from a missing one, and
 * neither may be silently repositioned onto the chart's boundary — callers
 * must render both as an explicit "not placed" notice, never drop them mute.
 */
export function timeFraction(range: EvidenceChartTimeRange, tsMs: number | null): number | null {
  if (tsMs == null || range.startMs == null || range.endMs == null) return null;
  if (range.endMs < range.startMs) return null;
  if (range.endMs === range.startMs) return tsMs === range.startMs ? 0.5 : null;
  if (tsMs < range.startMs || tsMs > range.endMs) return null;
  const f = (tsMs - range.startMs) / (range.endMs - range.startMs);
  return Number.isFinite(f) ? f : null;
}

export type MarkerPlacementDisposition =
  | "plottable"
  | "unknown_timestamp"
  | "range_unavailable"
  | "range_invalid"
  | "outside_range";

/**
 * Explains WHY a marker can or cannot be placed on the chart, without
 * collapsing every non-plottable case into "outside range". timeFraction()
 * correctly returns null for all of these (that's the right plotting
 * decision), but an operator-facing notice that says "known timestamp
 * outside the plotted range" is false when the range itself was never
 * resolvable (missing/unparsable start or end) or is internally invalid
 * (reversed start/end) — those are range failures, not placement facts
 * about this specific marker.
 */
export function classifyMarkerPlacement(
  range: EvidenceChartTimeRange,
  tsMs: number | null,
): MarkerPlacementDisposition {
  if (tsMs == null) return "unknown_timestamp";
  if (range.startMs == null || range.endMs == null) return "range_unavailable";
  if (range.endMs < range.startMs) return "range_invalid";
  if (range.endMs === range.startMs) return tsMs === range.startMs ? "plottable" : "outside_range";
  if (tsMs < range.startMs || tsMs > range.endMs) return "outside_range";
  return "plottable";
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
  const disposition = parsedCsvDisposition(equityResult, "equity_curve.csv");
  const provenance: LaneProvenance = {
    artifact: "equity_curve.csv",
    runId: identity.runId,
    runIdConflict: identity.runIdConflict,
    symbol: identity.symbols.length > 0 ? identity.symbols.join(",") : null,
    timeframe: identity.timeframeLabel,
  };
  if (equityResult.kind !== "ok") {
    return {
      status: disposition.status,
      points: [],
      malformedRowCount: disposition.malformedRowCount,
      provenance,
      reason: disposition.reason,
    };
  }
  const points = equityResult.data.rows.map((r) => ({
    tsUtc: r.ts_utc,
    tsMs: parseTsMs(r.ts_utc),
    value: r.equity,
  }));
  return {
    status: disposition.status,
    points,
    malformedRowCount: disposition.malformedRowCount,
    provenance,
    reason: disposition.reason,
  };
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
  const disposition = parsedCsvDisposition(equityResult, "equity_curve.csv");
  const provenance: LaneProvenance = {
    artifact: "equity_curve.csv (derived: drawdown)",
    runId: identity.runId,
    runIdConflict: identity.runIdConflict,
    symbol: identity.symbols.length > 0 ? identity.symbols.join(",") : null,
    timeframe: identity.timeframeLabel,
  };
  if (equityResult.kind !== "ok") {
    return {
      status: disposition.status,
      points: [],
      malformedRowCount: disposition.malformedRowCount,
      provenance,
      reason: disposition.reason,
    };
  }
  const series = computeDrawdownSeries(equityResult.data.rows);
  const points = series.map((p) => ({
    tsUtc: p.ts_utc,
    tsMs: parseTsMs(p.ts_utc),
    value: p.drawdown_pct,
  }));
  const derivedReason =
    "Derived from equity_curve.csv (display-only) — not a substitute for metrics.json's own reported max_drawdown_pct.";
  return {
    status: disposition.status,
    points,
    malformedRowCount: disposition.malformedRowCount,
    provenance,
    reason: disposition.reason ? `${disposition.reason} ${derivedReason}` : derivedReason,
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
  /**
   * Cost lane only: count of fills.csv rows with a fill_id but no parseable
   * finite fee value. undefined on lanes where fee evidence isn't a concept
   * (orders/fills themselves).
   */
  invalidFeeCount?: number;
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
  const disposition = parsedCsvDisposition(ordersResult, "orders.csv");
  if (ordersResult.kind !== "ok") {
    return {
      status: disposition.status,
      markers: [],
      malformedRowCount: disposition.malformedRowCount,
      reason: disposition.reason,
    };
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
        runIdConflict: identity.runIdConflict,
        symbol: r.symbol || null,
        timeframe: identity.timeframeLabel,
      },
    };
  });
  return {
    status: disposition.status,
    markers,
    malformedRowCount: disposition.malformedRowCount,
    reason: disposition.reason,
  };
}

/**
 * Fill and cost markers are both derived from the single literal fills.csv
 * result so neither lane can exist without a corresponding literal row —
 * this is what makes negative controls B/D structurally true: a fill marker
 * (or a cost event) can only ever originate from an actual fills.csv row,
 * never from an order intent or a research recommendation artifact.
 *
 * The two lanes' STATUS is deliberately NOT shared beyond this: a fill row
 * is genuine fill evidence even when its fee field is blank/malformed, but
 * that same row is an absence of cost evidence, not a zero-cost fact. Cost
 * status is therefore derived from how many rows actually carry a parseable
 * fee, never from the raw fill row count.
 */
function buildFillAndCostLanes(
  fillsResult: FileResult<ParsedCsvResult<FillRow>>,
  identity: EvidenceChartIdentity,
): { fillLane: EvidenceMarkerLane; costLane: EvidenceMarkerLane } {
  const fillDisposition = parsedCsvDisposition(fillsResult, "fills.csv");
  if (fillsResult.kind !== "ok") {
    return {
      fillLane: {
        status: fillDisposition.status,
        markers: [],
        malformedRowCount: fillDisposition.malformedRowCount,
        reason: fillDisposition.reason,
      },
      costLane: {
        status: fillDisposition.status,
        markers: [],
        malformedRowCount: fillDisposition.malformedRowCount,
        reason: fillDisposition.reason,
      },
    };
  }

  const fillMarkers: EvidenceMarker[] = [];
  const costMarkers: EvidenceMarker[] = [];
  let rowsWithFee = 0;
  let rowsWithoutFee = 0;
  for (const r of fillsResult.data.rows) {
    const tsMs = parseTsMs(r.ts_utc);
    const timeStatus: MarkerTimeStatus = tsMs == null ? "unknown" : "known";
    const provenance: LaneProvenance = {
      artifact: "fills.csv",
      runId: identity.runId,
      runIdConflict: identity.runIdConflict,
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
    // A numeric zero (feeUsd === 0) is real fee evidence and counts here.
    if (feeUsd != null) {
      rowsWithFee += 1;
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
    } else {
      rowsWithoutFee += 1;
    }
  }

  const malformedSourceRows = fillsResult.data.malformed;
  let costStatus: LaneStatus;
  let costReason: string | null;
  if (fillsResult.data.rows.length === 0 && malformedSourceRows === 0) {
    costStatus = "artifact_empty";
    costReason = null;
  } else if (fillsResult.data.rows.length === 0) {
    costStatus = "unavailable";
    costReason = `${malformedSourceRows} malformed fills.csv row(s) were excluded and no usable fee evidence remains.`;
  } else if (rowsWithoutFee === 0 && malformedSourceRows === 0) {
    costStatus = "artifact_present";
    costReason = null;
  } else if (rowsWithFee === 0) {
    costStatus = "unavailable";
    const feePart = rowsWithoutFee > 0
      ? `${rowsWithoutFee} usable fill row(s) have no parseable fee value`
      : "no usable fill row carries fee evidence";
    const malformedPart = malformedSourceRows > 0
      ? `; ${malformedSourceRows} malformed fills.csv row(s) were excluded`
      : "";
    costReason = `${feePart}${malformedPart} — cost evidence is missing, not zero.`;
  } else {
    costStatus = "partial";
    costReason =
      `${rowsWithFee} usable fill row(s) have parseable fee evidence; ` +
      `${rowsWithoutFee} usable row(s) do not; ${malformedSourceRows} malformed fills.csv row(s) were excluded.`;
  }

  return {
    fillLane: {
      status: fillDisposition.status,
      markers: fillMarkers,
      malformedRowCount: fillDisposition.malformedRowCount,
      reason: fillDisposition.reason,
    },
    costLane: {
      status: costStatus,
      markers: costMarkers,
      malformedRowCount: malformedSourceRows,
      reason: costReason,
      invalidFeeCount: rowsWithoutFee,
    },
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
