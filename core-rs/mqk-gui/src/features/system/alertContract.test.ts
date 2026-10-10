import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { fetchOperatorModel } from "./api";
import { parseActiveAlerts, parseAlertTriage } from "./alertContract";
import { AlertsScreen } from "../alerts/AlertsScreen";
import { DEFAULT_STATUS } from "./types";

const alert = { alert_id: "risk.halted", class: "risk.halted", severity: "critical", summary: "Risk halted", detail: "Position limit violated", source: "daemon.runtime_state" };
const triageRow = { alert_id: alert.alert_id, severity: alert.severity, title: alert.summary, domain: "risk", status: "acked", linked_incident_id: "incident-8", linked_incident_status: "resolved", linked_order_id: null, linked_strategy_id: null, assigned_to: null, created_at: "2026-10-10T10:00:00Z" };

test("alert contracts fail closed on unknown states, malformed rows, duplicates and unavailable acknowledgement authority", () => {
  assert.equal(parseActiveAlerts({ truth_state: "active", alert_count: 1, rows: [] }), null);
  assert.equal(parseActiveAlerts({ truth_state: "active", alert_count: 2, rows: [alert, alert] }), null);
  assert.equal(parseActiveAlerts({ truth_state: "future", alert_count: 0, rows: [] }), null);
  assert.equal(parseAlertTriage({ truth_state: "no_db", triage_note: "Unavailable", rows: [triageRow] }), null);
  assert.equal(parseAlertTriage({ truth_state: "active", triage_note: "Available", rows: [{ ...triageRow, created_at: null }] }), null);
  assert.equal(parseAlertTriage({ truth_state: "active", triage_note: "Available", rows: [{ ...triageRow, created_at: "2026-10-10T10:00:00" }] }), null);
});

for (const truth of ["active", "no_db", "invalid"] as const) test(`active fault remains visible with ${truth} acknowledgement source`, async () => {
  const original = globalThis.fetch;
  globalThis.fetch = (async (input, init) => {
    assert.equal(init?.method, "GET");
    const path = new URL(String(input)).pathname;
    if (path === "/api/v1/system/status") return Response.json(DEFAULT_STATUS);
    if (path === "/api/v1/alerts/active") return Response.json({ truth_state: "active", alert_count: 1, rows: [alert] });
    if (path === "/api/v1/alerts/triage") return Response.json({ truth_state: truth, triage_note: "Ack is advisory", rows: [truth === "active" ? triageRow : { ...triageRow, status: "unacked", created_at: null }] });
    return Response.json({}, { status: 404 });
  }) as typeof fetch;
  try {
    const model = await fetchOperatorModel();
    const html = renderToStaticMarkup(React.createElement(AlertsScreen, { model }));
    assert.match(html, /Position limit violated/);
    assert.match(html, /daemon.runtime_state/);
    assert.match(html, /risk.halted/);
    assert.doesNotMatch(html, /All alerts are acknowledged or silenced|No alerts require immediate action/);
    if (truth === "active") {
      assert.match(html, /incident-8 · resolved/);
      assert.match(html, />acked</);
    } else {
      assert.match(html, /Unavailable for this alert/);
      assert.doesNotMatch(html, />acked</);
    }
  } finally { globalThis.fetch = original; }
});
