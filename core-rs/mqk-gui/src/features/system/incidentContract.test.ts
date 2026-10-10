import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { fetchOperatorModel } from "./api";
import { parseIncidents } from "./incidentContract";
import { IncidentsScreen } from "../incidents/IncidentsScreen";
import { DEFAULT_STATUS } from "./types";

const incident = { incident_id: "case-7", opened_at_utc: "2026-10-10T12:00:00Z", title: "Broker discrepancy <script>", severity: "critical", status: "open", linked_alert_id: "reconcile.dirty", opened_by: "operator" };
test("durable incident contract rejects unavailable, malformed and duplicate rows", () => {
  for (const body of [null, {}, { truth_state: "no_db", rows: [] }, { truth_state: "future", rows: [] },
    { truth_state: "active", rows: [{}] }, { truth_state: "active", rows: [incident, incident] },
    { truth_state: "active", rows: [{ ...incident, status: "healthy" }] },
    { truth_state: "active", rows: [{ ...incident, opened_at_utc: "2026-10-10T12:00:00" }] }]) assert.equal(parseIncidents(body), null);
  assert.deepEqual(parseIncidents({ truth_state: "active", rows: [] }), []);
});

test("production fetch/model/component preserves durable incidents without invented detail or mutation", async () => {
  const original = globalThis.fetch;
  const requests: string[] = [];
  globalThis.fetch = (async (input, init) => {
    assert.equal(init?.method, "GET");
    const path = new URL(String(input)).pathname;
    requests.push(path);
    if (path === "/api/v1/system/status") return Response.json(DEFAULT_STATUS);
    if (path === "/api/v1/incidents") return Response.json({ truth_state: "active", rows: [incident] });
    return Response.json({}, { status: 404 });
  }) as typeof fetch;
  try {
    const model = await fetchOperatorModel();
    assert.equal(model.incidents.length, 1);
    assert.equal(model.incidents[0].incident_id, "case-7");
    assert.equal(model.incidents[0].updated_at, null);
    assert.equal(model.panelSources.incidents, "db_truth");
    const html = renderToStaticMarkup(React.createElement(IncidentsScreen, { model }));
    assert.match(html, /case-7/);
    assert.match(html, /reconcile.dirty/);
    assert.match(html, /last update time are unavailable/);
    assert.match(html, /&lt;script&gt;/);
    assert.doesNotMatch(html, /No active incidents|no operator actions recorded/);
    assert.ok(requests.includes("/api/v1/incidents"));
  } finally { globalThis.fetch = original; }
});
