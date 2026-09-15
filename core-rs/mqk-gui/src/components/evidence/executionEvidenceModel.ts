// R2 — UNIFIED-MQD-EVIDENCE-CHART-01: second EvidenceChartModel adapter,
// proving the model generalizes beyond backtest artifacts to a real, wired,
// currently-authoritative daemon source.
//
// SOURCE: GET /api/v1/execution/flow (fetchExecutionFlow in
// features/system/api.ts) — a durable, time-ordered join of oms_outbox,
// oms_order_lifecycle_events, and fill_quality_telemetry. Every row is real
// evidence the daemon actually recorded; this module performs NO synthesis
// (no fabricated fills/acks, no invented timestamps, no default-zero rows).
//
// `stage`/`severity` on ExecutionFlowRow are free-form strings the daemon may
// extend at any time (see execution.ts's own doc comment) — this adapter
// deliberately does NOT pattern-match them into a narrower classification
// (order intent vs fill vs cancel/replace). Inferring "contains the substring
// 'fill'" would be exactly the fragile, unspecified-string-based truth
// CLAUDE.md's data-provenance rule warns against. Instead every row renders
// as one generic lifecycle-event marker carrying its own real `stage` text,
// so the operator reads the daemon's actual words rather than a guessed
// taxonomy. orderIntentMarkers/fillMarkers/costEvents/signalMarkers/
// entryExitMarkers/foldRegions/oosRegions/priceSeries/equitySeries/
// drawdownSeries are therefore all not_wired for this source — none of those
// specific facts exist on ExecutionFlowRow.
//
// WORKSPACE ISOLATION (R2 WORKSPACE ISOLATION): the caller supplies the
// currently-linked WorkspaceIdentity's runId. If the surface's own run_id
// disagrees, every lane fails closed rather than rendering one run's
// evidence while the workspace is linked to a different run.
//
// ROW-LEVEL RUN PROVENANCE (R3 GUI-EVIDENCE-ROW-RUN-PROVENANCE-01): the
// endpoint contract says active rows belong to the queried run, but the
// contract is not proof — a malformed/inconsistent surface must not
// silently cross-contaminate runs. For truth_state "active", surface.run_id
// must be a non-empty string and every row.run_id must exactly equal it, or
// the entire surface fails closed (never a partial render of only the
// matching rows).

import type { ExecutionFlowRow, ExecutionFlowSurface } from "../../features/system/types/execution.ts";
import {
  notWiredMarkerLane,
  notWiredRegionLane,
  notWiredSeriesLane,
  parseTsMs,
  type EvidenceChartIdentity,
  type EvidenceChartModel,
  type EvidenceChartTimeRange,
  type EvidenceMarker,
  type EvidenceMarkerLane,
  type LaneStatus,
} from "./evidenceChartModel.ts";

const NOT_WIRED_REASON =
  "No source in the execution-flow evidence stream (oms_outbox / oms_order_lifecycle_events / " +
  "fill_quality_telemetry) — see the 'Execution lifecycle' layer for the raw event stream this run actually reported.";

const EMPTY_IDENTITY: EvidenceChartIdentity = {
  runId: null,
  runIdConflict: false,
  strategyName: null,
  strategyNameConflict: false,
  symbols: [],
  timeframeLabel: null,
  engineId: null,
  mode: null,
};

const UNAVAILABLE_TIME_RANGE: EvidenceChartTimeRange = {
  status: "unavailable",
  startTsUtc: null,
  endTsUtc: null,
  startMs: null,
  endMs: null,
};

function notWiredLanes(): Pick<
  EvidenceChartModel,
  | "priceSeries"
  | "equitySeries"
  | "drawdownSeries"
  | "signalMarkers"
  | "orderIntentMarkers"
  | "fillMarkers"
  | "entryExitMarkers"
  | "foldRegions"
  | "oosRegions"
  | "costEvents"
> {
  return {
    priceSeries: { status: "not_wired", bars: [], reason: NOT_WIRED_REASON },
    equitySeries: notWiredSeriesLane("execution/flow", NOT_WIRED_REASON),
    drawdownSeries: notWiredSeriesLane("execution/flow (derived: drawdown)", NOT_WIRED_REASON),
    signalMarkers: notWiredMarkerLane(NOT_WIRED_REASON),
    orderIntentMarkers: notWiredMarkerLane(NOT_WIRED_REASON),
    fillMarkers: notWiredMarkerLane(NOT_WIRED_REASON),
    entryExitMarkers: notWiredMarkerLane(NOT_WIRED_REASON),
    foldRegions: notWiredRegionLane(NOT_WIRED_REASON),
    oosRegions: notWiredRegionLane(NOT_WIRED_REASON),
    costEvents: notWiredMarkerLane(NOT_WIRED_REASON),
  };
}

function buildTimeRange(rows: ExecutionFlowRow[], status: LaneStatus): EvidenceChartTimeRange {
  if (rows.length === 0) {
    return { status, startTsUtc: null, endTsUtc: null, startMs: null, endMs: null };
  }
  const startTsUtc = rows[0].ts_utc || null;
  const endTsUtc = rows[rows.length - 1].ts_utc || null;
  return { status, startTsUtc, endTsUtc, startMs: parseTsMs(startTsUtc), endMs: parseTsMs(endTsUtc) };
}

function buildIdentity(runId: string | null, rows: ExecutionFlowRow[]): EvidenceChartIdentity {
  const symbols: string[] = [];
  for (const r of rows) {
    if (r.symbol && !symbols.includes(r.symbol)) symbols.push(r.symbol);
  }
  return { ...EMPTY_IDENTITY, runId, symbols };
}

function buildLifecycleLane(rows: ExecutionFlowRow[], status: LaneStatus, runId: string | null): EvidenceMarkerLane {
  const markers: EvidenceMarker[] = rows.map((r) => {
    const tsMs = parseTsMs(r.ts_utc);
    return {
      id: `flow:${r.row_id}`,
      kind: "lifecycle_event",
      tsUtc: r.ts_utc,
      tsMs,
      timeStatus: tsMs == null ? "unknown" : "known",
      label: r.message ? `${r.stage}: ${r.message}` : r.stage,
      symbol: r.symbol,
      side: null,
      qty: null,
      price: null,
      feeUsd: null,
      orderStatus: r.stage,
      provenance: {
        artifact: `execution/flow (${r.source_table})`,
        runId,
        runIdConflict: false,
        symbol: r.symbol,
        timeframe: null,
      },
    };
  });
  return { status, markers, malformedRowCount: 0, reason: null };
}

function failClosedEvidence(reason: string): EvidenceChartModel {
  return {
    identity: EMPTY_IDENTITY,
    timeRange: UNAVAILABLE_TIME_RANGE,
    ...notWiredLanes(),
    executionLifecycleMarkers: { status: "unavailable", markers: [], malformedRowCount: 0, reason },
  };
}

/**
 * Builds an EvidenceChartModel from a real execution-flow surface fetched via
 * fetchExecutionFlow(). `workspaceRunId` is the caller's currently-linked
 * WorkspaceIdentity.runId (null if nothing is linked yet).
 */
export function buildExecutionEvidenceChartModel(
  surface: ExecutionFlowSurface,
  workspaceRunId: string | null,
): EvidenceChartModel {
  if (surface.canonical_route !== "/api/v1/execution/flow") {
    return failClosedEvidence(
      `Execution flow surface reported an unexpected canonical_route '${surface.canonical_route}' — evidence withheld.`,
    );
  }

  if (surface.truth_state === "active") {
    if (!surface.run_id) {
      return failClosedEvidence(
        "Execution flow reported truth_state 'active' with no run_id — evidence withheld rather than shown " +
          "without a verified run identity.",
      );
    }
    // Fail closed rather than silently render a different run's evidence
    // inside the currently-linked workspace.
    if (workspaceRunId != null && surface.run_id !== workspaceRunId) {
      return failClosedEvidence(
        `Execution flow reported run_id '${surface.run_id}', which does not match the linked workspace run ` +
          `'${workspaceRunId}' — evidence withheld rather than shown against the wrong run.`,
      );
    }
    // Every row must agree with the surface's own run_id — a single
    // disagreeing row invalidates the entire surface, not just that row.
    const mismatchedRow = surface.rows.find((r) => r.run_id !== surface.run_id);
    if (mismatchedRow != null) {
      return failClosedEvidence(
        `Execution flow row '${mismatchedRow.row_id}' carries run_id '${mismatchedRow.run_id}', which does not ` +
          `match the surface's own run_id '${surface.run_id}' — evidence withheld for the entire execution ` +
          `evidence surface rather than partially rendered.`,
      );
    }
  }

  let status: LaneStatus;
  let reason: string | null;
  switch (surface.truth_state) {
    case "no_db":
      status = "unavailable";
      reason = "No database configured — execution flow evidence is not authoritative.";
      break;
    case "no_active_run":
      status = "unavailable";
      reason = "No active execution run — execution flow evidence is not available for this workspace.";
      break;
    case "active":
      status = surface.rows.length === 0 ? "artifact_empty" : "artifact_present";
      reason = null;
      break;
  }

  const rows = surface.truth_state === "active" ? surface.rows : [];
  const identity = buildIdentity(surface.run_id, rows);

  return {
    identity,
    timeRange: buildTimeRange(rows, status),
    ...notWiredLanes(),
    executionLifecycleMarkers:
      reason != null
        ? { status, markers: [], malformedRowCount: 0, reason }
        : buildLifecycleLane(rows, status, identity.runId),
  };
}
