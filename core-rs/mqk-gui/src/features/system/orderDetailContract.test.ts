import test from "node:test";
import assert from "node:assert/strict";
import { fetchExecutionTimeline, fetchExecutionTrace, fetchExecutionReplay, fetchExecutionChart, fetchCausalityTrace, fetchExecutionFlow } from "./api";
import { orderTimelineNotice } from "./legacy";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { ExecutionReplayViewer } from "../execution/components/ExecutionReplayViewer";
import { ExecutionTraceViewer } from "../execution/components/ExecutionTraceViewer";

test("per-order reads encode identity and reject unknown truth, wrong order and malformed rows", async () => {
  const original = globalThis.fetch;
  const orderId = "order/a?b#c";
  let payload: Record<string, unknown> = { canonical_route: `/api/v1/execution/orders/${orderId}/timeline`,
    truth_state: "filled_without_fill_quality_telemetry", backend: "postgres.fill_quality_telemetry", order_id: orderId, rows: [] };
  const paths: string[] = [];
  globalThis.fetch = (async (input, init) => {
    assert.equal(init?.method, "GET");
    paths.push(new URL(String(input)).pathname);
    return Response.json(payload);
  }) as typeof fetch;
  try {
    const timeline = await fetchExecutionTimeline(orderId);
    assert.equal(timeline!.truth_state, "filled_without_fill_quality_telemetry");
    assert.match(orderTimelineNotice(timeline!)!, /durable fill telemetry is missing/);
    await Promise.all([fetchExecutionTrace(orderId), fetchExecutionReplay(orderId), fetchExecutionChart(orderId), fetchCausalityTrace(orderId)]);
    for (const path of paths) assert.match(path, /orders\/order%2Fa%3Fb%23c\//);
    for (const invalid of [{ truth_state: "future" }, { order_id: "other-order" }, { rows: {} }, { rows: [{}] }]) {
      const previous = payload;
      payload = { ...payload, ...invalid };
      assert.equal(await fetchExecutionTimeline(orderId), null);
      payload = previous;
    }
  } finally { globalThis.fetch = original; }
});

test("execution flow validates scope and row contracts instead of accepting connected JSON as evidence", async () => {
  const original = globalThis.fetch;
  const row = { row_id: "outbox:1", ts_utc: "2026-10-10T10:00:00Z", stage: "broker_sent", severity: "info", run_id: "run-1",
    internal_order_id: "order-1", broker_order_id: null, symbol: "SIM", message: "Sent", source_table: "oms_outbox" };
  let payload: unknown = { canonical_route: "/api/v1/execution/flow", truth_state: "active", backend: "postgres", run_id: "run-1", rows: [row] };
  globalThis.fetch = (async (input, init) => { assert.equal(init?.method, "GET"); return Response.json(payload); }) as typeof fetch;
  try {
    assert.equal((await fetchExecutionFlow({ runId: "run-1", orderId: "order-1" }))!.rows[0].row_id, row.row_id);
    assert.equal(await fetchExecutionFlow({ runId: "other-run" }), null);
    assert.equal(await fetchExecutionFlow({ orderId: "other-order" }), null);
    for (const invalid of [{}, { ...(payload as object), rows: [{}] }, { ...(payload as object), truth_state: "future" }]) {
      payload = invalid;
      assert.equal(await fetchExecutionFlow(), null);
    }
  } finally { globalThis.fetch = original; }
});

test("all order detail adapters bind identity and reject malformed evidence without unsafe replay totals", async () => {
  const original = globalThis.fetch;
  const orderId = "order-1";
  const common = { backend: "postgres.fill_quality_telemetry", order_id: orderId };
  const fixtures: Record<string, Record<string, unknown>> = {
    trace: { ...common, canonical_route: `/api/v1/execution/orders/${orderId}/trace`, truth_state: "filled_without_fill_quality_telemetry",
      broker_order_id: null, symbol: "SIM", requested_qty: 2, filled_qty: 1, current_status: "PARTIALLY_FILLED", current_stage: "partial_fill",
      outbox_status: null, outbox_lifecycle_stage: null, last_event_at: null, rows: [] },
    replay: { ...common, canonical_route: `/api/v1/execution/orders/${orderId}/replay`, truth_state: "active", replay_id: orderId,
      replay_scope: "single_order", source: "fill_quality_telemetry", title: "SIM replay", current_frame_index: 0, frames: [{
        frame_id: "fill-1", timestamp: "2026-10-10T10:00:00Z", subsystem: "execution", event_type: "partial_fill", state_delta: "fill_qty=0.5 fill_price=10",
        message_digest: "inbox:1", order_execution_state: "PARTIALLY_FILLED", oms_state: "PARTIALLY_FILLED", filled_qty: 0, open_qty: 2,
        risk_state: "unknown", reconcile_state: "unknown", queue_status: "SENT", anomaly_tags: [], boundary_tags: [],
      }] },
    chart: { ...common, canonical_route: `/api/v1/execution/orders/${orderId}/chart`, truth_state: "no_bars", symbol: "SIM", comment: "No source" },
    causality: { ...common, canonical_route: `/api/v1/execution/orders/${orderId}/causality`, truth_state: "partial", symbol: "SIM", comment: "Partial proof",
      proven_lanes: ["intent"], unproven_lanes: ["signal", "risk", "portfolio", "reconcile"], nodes: [{ node_key: "outbox:1", node_type: "outbox_enqueued",
        title: "Intent enqueued", status: "ok", subsystem: "execution", summary: "SENT", linked_id: "outbox-1", timestamp: "2026-10-10T10:00:00Z",
        elapsed_from_prev_ms: null, anomaly_tags: [] }] },
  };
  let override: Record<string, unknown> | null = null;
  const reads = { trace: fetchExecutionTrace, replay: fetchExecutionReplay, chart: fetchExecutionChart, causality: fetchCausalityTrace };
  globalThis.fetch = (async (input, init) => {
    assert.equal(init?.method, "GET");
    const kind = new URL(String(input)).pathname.split("/").pop()!;
    return Response.json({ ...fixtures[kind], ...override });
  }) as typeof fetch;
  try {
    for (const [kind, read] of Object.entries(reads)) {
      assert.ok(await read(orderId), kind);
      for (const invalid of [{ order_id: "other" }, { truth_state: "future" }, { backend: {} }, { canonical_route: "/wrong" }]) {
        override = invalid;
        assert.equal(await read(orderId), null, kind);
      }
      override = kind === "trace" ? { rows: [{}] } : kind === "replay" ? { frames: [{}] } : kind === "chart" ? { bars: [{}] } : { nodes: [{}] };
      assert.equal(await read(orderId), null, kind);
      override = null;
    }
    const replay = await fetchExecutionReplay(orderId);
    const html = renderToStaticMarkup(React.createElement(ExecutionReplayViewer, { replay, selectedFrameIndex: 0, onSelectFrame: () => {} }));
    assert.match(html, /fill_qty=0.5/);
    assert.match(html, /Cumulative\/open quantities are unavailable/);
    assert.doesNotMatch(html, /Qty 0 filled \/ 2|>Play<|>Pause</);
    const trace = await fetchExecutionTrace(orderId);
    assert.match(renderToStaticMarkup(React.createElement(ExecutionTraceViewer, { trace })), /durable fill telemetry is missing/);
  } finally { globalThis.fetch = original; }
});
