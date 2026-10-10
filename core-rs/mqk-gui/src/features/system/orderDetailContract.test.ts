import test from "node:test";
import assert from "node:assert/strict";
import { fetchExecutionTimeline, fetchExecutionTrace, fetchExecutionReplay, fetchExecutionChart, fetchCausalityTrace } from "./api";
import { orderTimelineNotice } from "./legacy";

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
