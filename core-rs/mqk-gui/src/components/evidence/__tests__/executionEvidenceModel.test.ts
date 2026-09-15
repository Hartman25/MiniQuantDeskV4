import test from "node:test";
import assert from "node:assert/strict";
import { buildExecutionEvidenceChartModel } from "../executionEvidenceModel.ts";
import { buildEvidenceChartModel, timeFraction } from "../evidenceChartModel.ts";
import type { ExecutionFlowRow, ExecutionFlowSurface } from "../../../features/system/types/execution.ts";
import type { ArtifactBundle } from "../../../features/backtests/types.ts";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

function flowRow(overrides: Partial<ExecutionFlowRow> = {}): ExecutionFlowRow {
  return {
    row_id: "row-1",
    ts_utc: "2026-01-01T00:00:00Z",
    stage: "outbox_enqueued",
    severity: "info",
    run_id: "run-exec-1",
    internal_order_id: "io-1",
    broker_order_id: null,
    symbol: "AAPL",
    message: "enqueued",
    source_table: "oms_outbox",
    ...overrides,
  };
}

function activeSurface(rows: ExecutionFlowRow[], overrides: Partial<ExecutionFlowSurface> = {}): ExecutionFlowSurface {
  return {
    canonical_route: "/api/v1/execution/flow",
    truth_state: "active",
    backend: "oms_outbox+oms_order_lifecycle_events+fill_quality_telemetry",
    run_id: "run-exec-1",
    rows,
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// R2-EV-10 / negative control: every lane this source has no evidence for
// stays explicitly not_wired regardless of input — never an authoritative
// zero.
// ---------------------------------------------------------------------------

test("R2-EV-10: unsupported layers (price/equity/drawdown/signals/order-intent/fill/entry-exit/fold/OOS/cost) are always not_wired for an execution-flow source", () => {
  const model = buildExecutionEvidenceChartModel(
    activeSurface([flowRow({ stage: "broker_final_fill", source_table: "fill_quality_telemetry" })]),
    null,
  );
  assert.equal(model.priceSeries.status, "not_wired");
  assert.equal(model.equitySeries.status, "not_wired");
  assert.equal(model.drawdownSeries.status, "not_wired");
  assert.equal(model.signalMarkers.status, "not_wired");
  assert.equal(model.orderIntentMarkers.status, "not_wired");
  assert.equal(model.fillMarkers.status, "not_wired");
  assert.equal(model.entryExitMarkers.status, "not_wired");
  assert.equal(model.foldRegions.status, "not_wired");
  assert.equal(model.oosRegions.status, "not_wired");
  assert.equal(model.costEvents.status, "not_wired");
});

// ---------------------------------------------------------------------------
// R2-EV-06: FILL AUTHORITY — a lifecycle row cannot display as a verified
// fill without genuine fill-specific evidence. This adapter never promotes a
// generic lifecycle_event into fillMarkers, even when its raw `stage` string
// literally says "broker_final_fill" — see the adapter's own header comment
// on why pattern-matching `stage` would be exactly the fragile inference the
// mission prohibits.
// ---------------------------------------------------------------------------

test("R2-EV-06: a row whose stage says 'broker_final_fill' never becomes a fillMarkers entry (no stage-text inference)", () => {
  const model = buildExecutionEvidenceChartModel(
    activeSurface([flowRow({ row_id: "r1", stage: "broker_final_fill", source_table: "fill_quality_telemetry" })]),
    null,
  );
  assert.equal(model.fillMarkers.markers.length, 0);
  assert.equal(model.fillMarkers.status, "not_wired");
  // It IS real evidence — just generic, not reclassified.
  assert.equal(model.executionLifecycleMarkers.markers.length, 1);
  assert.equal(model.executionLifecycleMarkers.markers[0].kind, "lifecycle_event");
});

// ---------------------------------------------------------------------------
// R2-EV-04: artifact_empty (source loaded, genuinely zero rows) must never
// collapse with unavailable (source not authoritative at all).
// ---------------------------------------------------------------------------

test("R2-EV-04: an active run with zero rows is artifact_empty; no_active_run/no_db are unavailable — never conflated", () => {
  const empty = buildExecutionEvidenceChartModel(activeSurface([]), null);
  assert.equal(empty.executionLifecycleMarkers.status, "artifact_empty");
  assert.equal(empty.executionLifecycleMarkers.reason, null);

  const noRun = buildExecutionEvidenceChartModel(
    { canonical_route: "/api/v1/execution/flow", truth_state: "no_active_run", backend: "unavailable", run_id: null, rows: [] },
    null,
  );
  assert.equal(noRun.executionLifecycleMarkers.status, "unavailable");
  assert.ok(noRun.executionLifecycleMarkers.reason);

  const noDb = buildExecutionEvidenceChartModel(
    { canonical_route: "/api/v1/execution/flow", truth_state: "no_db", backend: "unavailable", run_id: null, rows: [] },
    null,
  );
  assert.equal(noDb.executionLifecycleMarkers.status, "unavailable");
  assert.ok(noDb.executionLifecycleMarkers.reason);
});

// ---------------------------------------------------------------------------
// R2-EV-03 / R2-EV-08: provenance / workspace contamination — evidence for a
// different run than the linked workspace must never silently render.
// ---------------------------------------------------------------------------

test("R2-EV-03/EV-08: a run_id mismatch between the surface and the linked workspace withholds ALL evidence, never renders it", () => {
  const surface = activeSurface([flowRow()], { run_id: "run-exec-1" });
  const model = buildExecutionEvidenceChartModel(surface, "a-completely-different-run");

  assert.equal(model.executionLifecycleMarkers.status, "unavailable");
  assert.equal(model.executionLifecycleMarkers.markers.length, 0);
  assert.match(model.executionLifecycleMarkers.reason ?? "", /does not match the linked workspace run/);
  assert.equal(model.identity.runId, null);

  // Positive control: no workspace link (null) or a matching link both render normally.
  const unlinked = buildExecutionEvidenceChartModel(surface, null);
  assert.equal(unlinked.executionLifecycleMarkers.markers.length, 1);
  const matching = buildExecutionEvidenceChartModel(surface, "run-exec-1");
  assert.equal(matching.executionLifecycleMarkers.markers.length, 1);
});

// ---------------------------------------------------------------------------
// R2-EV-01 / R2-EV-05: chronology / causality — rows render in the order the
// daemon reported them (oldest-first), never reordered or reclassified.
// ---------------------------------------------------------------------------

test("R2-EV-01/EV-05: lifecycle markers preserve the source's chronological row order and resolve time fractions monotonically", () => {
  const rows = [
    flowRow({ row_id: "a", ts_utc: "2026-01-01T00:00:00Z", stage: "outbox_enqueued" }),
    flowRow({ row_id: "b", ts_utc: "2026-01-01T00:00:05Z", stage: "broker_sent" }),
    flowRow({ row_id: "c", ts_utc: "2026-01-01T00:00:10Z", stage: "broker_final_fill" }),
  ];
  const model = buildExecutionEvidenceChartModel(activeSurface(rows), null);
  const ids = model.executionLifecycleMarkers.markers.map((m) => m.id);
  assert.deepEqual(ids, ["flow:a", "flow:b", "flow:c"]);

  const fractions = model.executionLifecycleMarkers.markers.map((m) => timeFraction(model.timeRange, m.tsMs));
  assert.deepEqual(fractions, [0, 0.5, 1]);
});

// ---------------------------------------------------------------------------
// R2-EV-12: missing numeric/identity evidence must not silently become zero
// or a fabricated placeholder.
// ---------------------------------------------------------------------------

test("R2-EV-12: a row with no symbol keeps symbol null in both the marker and its provenance — never defaulted", () => {
  const model = buildExecutionEvidenceChartModel(activeSurface([flowRow({ symbol: null })]), null);
  const marker = model.executionLifecycleMarkers.markers[0];
  assert.equal(marker.symbol, null);
  assert.equal(marker.provenance.symbol, null);
  assert.deepEqual(model.identity.symbols, []); // a null symbol never becomes a placeholder entry
});

test("an unparsable/empty timestamp is 'unknown', never guessed at bar 0", () => {
  const model = buildExecutionEvidenceChartModel(activeSurface([flowRow({ ts_utc: "" })]), null);
  const marker = model.executionLifecycleMarkers.markers[0];
  assert.equal(marker.timeStatus, "unknown");
  assert.equal(marker.tsMs, null);
});

// ---------------------------------------------------------------------------
// Identity: symbols are deduped, order-preserving, content-derived.
// ---------------------------------------------------------------------------

test("identity.symbols is the deduped, order-preserving set of literal symbols actually present in rows", () => {
  const model = buildExecutionEvidenceChartModel(
    activeSurface([
      flowRow({ row_id: "1", symbol: "AAPL" }),
      flowRow({ row_id: "2", symbol: "MSFT" }),
      flowRow({ row_id: "3", symbol: "AAPL" }),
      flowRow({ row_id: "4", symbol: null }),
    ]),
    null,
  );
  assert.deepEqual(model.identity.symbols, ["AAPL", "MSFT"]);
});

// ---------------------------------------------------------------------------
// R2-EV-13: CROSS-LIFECYCLE NORMALIZATION — two distinct real source types
// (backtest artifacts, execution/flow) normalize into the SAME
// EvidenceChartModel shape without losing their own source/provenance
// identity.
// ---------------------------------------------------------------------------

function minimalBacktestBundle(): ArtifactBundle {
  return {
    manifest: { kind: "ok", data: { schema_version: 1, run_id: "run-bt-1", strategy_name: "swing", engine_id: "mqk-backtest", mode: "backtest", created_at_utc: "2026-01-01T00:00:00Z" } },
    metrics: { kind: "missing" },
    equityCurve: { kind: "ok", data: { rows: [{ ts_utc: "2026-01-01T00:00:00Z", equity: 100_000_000 }], malformed: 0 } },
    orders: { kind: "missing" },
    fills: { kind: "missing" },
    strategyFit: { kind: "missing" },
    paperReadiness: { kind: "missing" },
    watchlistPromotion: { kind: "missing" },
    premarketRevalidation: { kind: "missing" },
    evidenceReview: { kind: "missing" },
  };
}

test("R2-EV-13: a backtest-artifact model and an execution-flow model both satisfy EvidenceChartModel and keep distinct, honest provenance", () => {
  const backtestModel = buildEvidenceChartModel(minimalBacktestBundle());
  const executionModel = buildExecutionEvidenceChartModel(activeSurface([flowRow()]), null);

  // Same shape, both consumable by the one EvidenceChart component.
  assert.ok("executionLifecycleMarkers" in backtestModel);
  assert.ok("executionLifecycleMarkers" in executionModel);
  assert.ok("orderIntentMarkers" in backtestModel);
  assert.ok("orderIntentMarkers" in executionModel);

  // Distinct identity — neither adapter's output can be mistaken for the other's source.
  assert.equal(backtestModel.identity.runId, "run-bt-1");
  assert.equal(executionModel.identity.runId, "run-exec-1");
  assert.equal(backtestModel.executionLifecycleMarkers.status, "not_wired");
  assert.equal(executionModel.executionLifecycleMarkers.status, "artifact_present");

  // Provenance is never merged/blended across sources.
  assert.equal(executionModel.executionLifecycleMarkers.markers[0].provenance.artifact, "execution/flow (oms_outbox)");
  assert.notEqual(executionModel.executionLifecycleMarkers.markers[0].provenance.artifact, backtestModel.identity.engineId);
});
