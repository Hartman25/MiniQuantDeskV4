import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MOCK_MODEL } from "./mockData";
import { RightOpsRail } from "../../components/layout/RightOpsRail";
import { BottomEventRail } from "../../components/layout/BottomEventRail";
import { AlertsScreen } from "../alerts/AlertsScreen";
import { IncidentsScreen } from "../incidents/IncidentsScreen";
import { MetricsScreen } from "../metrics/MetricsScreen";
import { OperatorTimelineScreen } from "../operatorTimeline/OperatorTimelineScreen";
import { DataTable } from "../../components/common/DataTable";
import type { SystemModel } from "./types";

test("compact rails distinguish absent sources from authoritative empty sources", () => {
  const model = { ...MOCK_MODEL, alerts: [], incidents: [], connected: true,
    dataSource: { ...MOCK_MODEL.dataSource, state: "partial", reachable: true, realEndpoints: [], missingEndpoints: [] } } as SystemModel;
  let html = renderToStaticMarkup(React.createElement(RightOpsRail, { model }));
  assert.match(html, /Active alert source unavailable/);
  assert.match(html, /Incident source unavailable/);
  assert.match(html, /Portfolio snapshot unavailable/);
  assert.doesNotMatch(html, /No active alerts|No active incidents/);
  model.dataSource.realEndpoints = ["/api/v1/alerts/active", "/api/v1/incidents"];
  html = renderToStaticMarkup(React.createElement(RightOpsRail, { model }));
  assert.match(html, /No active alerts/);
  assert.match(html, /No active incidents/);
  assert.match(renderToStaticMarkup(React.createElement(BottomEventRail, { events: [] })), /no-event status is unknown/);
  assert.match(renderToStaticMarkup(React.createElement(BottomEventRail, { events: [], available: true })), /No events returned/);
});

test("stale and degraded diagnostic observations retain evidence with explicit warnings", () => {
  for (const runtime_status of ["degraded", "running"] as const) {
    const model = { ...MOCK_MODEL, connected: true, operatorTimeline: [],
      status: { ...MOCK_MODEL.status, runtime_status, last_heartbeat: "2000-01-01T00:00:00Z" },
      runtimeLeadership: { ...MOCK_MODEL.runtimeLeadership, post_restart_recovery_state: "complete" },
      panelSources: { ...MOCK_MODEL.panelSources, alerts: "runtime_memory", incidents: "db", metrics: "runtime_memory", operatorTimeline: "db" },
      dataSource: { ...MOCK_MODEL.dataSource, state: "real", reachable: true, missingEndpoints: [] } } as SystemModel;
    for (const [component, evidence] of [[AlertsScreen, /Active alerts and diagnostics/], [IncidentsScreen, /Incident evidence/],
      [MetricsScreen, /Operational metric evidence/], [OperatorTimelineScreen, /Bounded event ledger/]] as const) {
      const html = renderToStaticMarkup(React.createElement(component, { model }));
      assert.match(html, runtime_status === "degraded" ? /Degraded/ : /Stale/);
      assert.match(html, evidence);
    }
  }
});

test("wide timeline tables declare every evidence column in the rendered grid", () => {
  const html = renderToStaticMarkup(React.createElement(DataTable, { rows: ["evidence"], rowKey: (row: unknown) => String(row),
    columns: Array.from({ length: 12 }, (_, index) => ({ key: String(index), title: String(index), render: () => String(index) })) }));
  assert.equal(html.split("grid-template-columns:repeat(12, minmax(100px, 1fr))").length - 1, 2);
});
