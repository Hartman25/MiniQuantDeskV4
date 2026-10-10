import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { parseMarketDataQuality, parseOmsOverview, parseTransport } from "./snapshotContracts";
import { fetchOperatorModel } from "./api";
import { OmsStateMachineVisualizer } from "../execution/components/OmsStateMachineVisualizer";
import { TransportScreen } from "../transport/TransportScreen";
import { DEFAULT_STATUS } from "./types";

const oms = { canonical_route: "/api/v1/oms/overview", execution_has_snapshot: true, execution_active_orders: 7, execution_pending_orders: 3 };
const transport = { truth_state: "active", outbox_depth: 3, inbox_depth: 2, max_claim_age_ms: 1200, dispatch_retries: 1, orphaned_claims: 0, duplicate_inbox_events: 0, queues: [] };
test("snapshot adapters reject missing truth and never promote unrecorded values", () => {
  assert.equal(parseTransport({ ...transport, truth_state: "no_snapshot" }), null);
  assert.equal(parseTransport({ ...transport, queues: [{}] }), null);
  assert.equal(parseTransport(transport)!.duplicate_inbox_events, null);
  const overview = parseOmsOverview(oms)!;
  assert.equal(overview.total_active_orders, 7);
  assert.equal(overview.stuck_orders, null);
  const html = renderToStaticMarkup(React.createElement(OmsStateMachineVisualizer, { overview }));
  assert.match(html, /Active orders: 7/);
  assert.match(html, /stuck-order counts are unavailable/);
  assert.equal(parseOmsOverview({ ...oms, execution_has_snapshot: false })!.total_active_orders, null);
  const quality = parseMarketDataQuality({ truth_state: "active", overall_health: "ok", market_data_source: "signal_ingestion_ready", ws_continuity: "live", venues: [], issues: [] })!;
  assert.equal(quality.detail_authority, "transport_only");
  assert.equal(parseMarketDataQuality({ truth_state: "future" }), null);
});
test("production model maps flat OMS and transport contracts without empty-as-healthy or duplicate zero", async () => {
  const original = globalThis.fetch;
  globalThis.fetch = (async (input, init) => {
    assert.equal(init?.method, "GET");
    const path = new URL(String(input)).pathname;
    if (path === "/api/v1/system/status") return Response.json(DEFAULT_STATUS);
    if (path === "/api/v1/oms/overview") return Response.json(oms);
    if (path === "/api/v1/execution/transport") return Response.json(transport);
    return Response.json({}, { status: 404 });
  }) as typeof fetch;
  try {
    const model = await fetchOperatorModel();
    assert.equal(model.omsOverview.total_active_orders, 7);
    assert.equal(model.transport.duplicate_inbox_events, null);
    const html = renderToStaticMarkup(React.createElement(TransportScreen, { model }));
    assert.match(html, /Duplicate events<\/span><strong>Unavailable/);
    assert.match(html, /retry proxy/);
  } finally { globalThis.fetch = original; }
});
