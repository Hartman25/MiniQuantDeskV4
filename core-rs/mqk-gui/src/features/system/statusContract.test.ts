import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { parseSystemStatus } from "./statusContract";
import { fetchOperatorModel } from "./api";
import { DEFAULT_STATUS } from "./types";
import { GlobalStatusBar, liveRoutingDisplayState } from "../../components/status/GlobalStatusBar";

test("status preserves unknown domain and nullable Live routing instead of Paper/disabled defaults", () => {
  const status = parseSystemStatus({ ...DEFAULT_STATUS, daemon_reachable: true, environment: null, live_routing_enabled: null })!;
  assert.equal(status.environment, "unknown");
  assert.equal(liveRoutingDisplayState(status), "unknown");
  for (const body of [{}, { ...DEFAULT_STATUS, runtime_status: "future" }, { ...DEFAULT_STATUS, execution_armed: "true" },
    { ...DEFAULT_STATUS, live_routing_enabled: undefined }, { ...DEFAULT_STATUS, last_heartbeat: "yesterday" }]) assert.equal(parseSystemStatus(body), null);
});

for (const statusResponse of [401, 403, 500, "malformed"] as const) test(`reachable unrelated panel does not confirm status on ${statusResponse}`, async () => {
  const original = globalThis.fetch;
  globalThis.fetch = (async (input) => {
    const path = new URL(String(input)).pathname;
    if (path === "/api/v1/system/status") return statusResponse === "malformed" ? Response.json({ daemon_reachable: true, live_routing_enabled: false }) : Response.json({}, { status: statusResponse });
    if (path === "/api/v1/alerts/active") return Response.json({ truth_state: "active", alert_count: 0, rows: [] });
    return Response.json({}, { status: 404 });
  }) as typeof fetch;
  try {
    const model = await fetchOperatorModel();
    assert.equal(model.connected, true);
    assert.equal(model.status.daemon_reachable, false);
    assert.equal(model.status.environment, "unknown");
    const html = renderToStaticMarkup(React.createElement(GlobalStatusBar, { status: model.status, dataSource: model.dataSource }));
    assert.match(html, /unknown/i);
    assert.doesNotMatch(html, /value-paper|>paper</);
  } finally { globalThis.fetch = original; }
});
