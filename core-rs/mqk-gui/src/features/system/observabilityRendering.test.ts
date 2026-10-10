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
import { RuntimeScreen } from "../runtime/RuntimeScreen";
import { TopologyScreen } from "../topology/TopologyScreen";
import { TransportScreen } from "../transport/TransportScreen";
import { AuditScreen } from "../audit/AuditScreen";
import { ArtifactsScreen } from "../artifacts/ArtifactsScreen";
import { DashboardScreen } from "../dashboard/DashboardScreen";
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
      panelSources: { ...MOCK_MODEL.panelSources, alerts: "runtime_memory", incidents: "db", metrics: "runtime_memory", operatorTimeline: "db",
        runtime: "runtime_memory", topology: "runtime_memory", transport: "db", audit: "db", artifacts: "db" },
      dataSource: { ...MOCK_MODEL.dataSource, state: "real", reachable: true, missingEndpoints: [] } } as SystemModel;
    for (const [component, evidence] of [[AlertsScreen, /Active alerts and diagnostics/], [IncidentsScreen, /Incident evidence/],
      [MetricsScreen, /Operational metric evidence/], [OperatorTimelineScreen, /Bounded event ledger/],
      [RuntimeScreen, /Runtime leadership/], [TopologyScreen, /topology/i], [TransportScreen, /Outbox posture/],
      [AuditScreen, /Audit actions/], [ArtifactsScreen, /Artifact registry/]] as const) {
      const html = renderToStaticMarkup(React.createElement(component, { model }));
      assert.match(html, runtime_status === "degraded" ? /Degraded/ : /Stale/);
      assert.match(html, evidence);
    }
  }
});

test("run metadata does not imply artifact generation and audit feed absence is explicit", () => {
  const model = { ...MOCK_MODEL, connected: true,
    status: { ...MOCK_MODEL.status, runtime_status: "running", last_heartbeat: new Date().toISOString() },
    artifactRegistry: { ...MOCK_MODEL.artifactRegistry, detail_authority: "run_metadata_only", artifacts: [{
      ...MOCK_MODEL.artifactRegistry.artifacts[0], artifact_id: "run-config:run-1", linked_run_id: "run-1", source_ref: "runs:run-1",
    }] }, panelSources: { ...MOCK_MODEL.panelSources, artifacts: "db", audit: "db" },
    dataSource: { ...MOCK_MODEL.dataSource, state: "real", reachable: true, realEndpoints: [], missingEndpoints: [] } } as SystemModel;
  const artifacts = renderToStaticMarkup(React.createElement(ArtifactsScreen, { model }));
  assert.match(artifacts, /run-config:run-1/);
  assert.match(artifacts, /runs:run-1/);
  assert.match(artifacts, /generation success\/failure are unavailable/);
  assert.doesNotMatch(artifacts, /Artifacts ready for review|Artifacts still generating|Artifact generation failures/);
  assert.match(renderToStaticMarkup(React.createElement(AuditScreen, { model })), /Event feed unavailable; no-event status is unknown/);
});

test("wide timeline tables declare every evidence column in the rendered grid", () => {
  const html = renderToStaticMarkup(React.createElement(DataTable, { rows: ["evidence"], rowKey: (row: unknown) => String(row),
    columns: Array.from({ length: 12 }, (_, index) => ({ key: String(index), title: String(index), render: () => String(index) })) }));
  assert.equal(html.split("grid-template-columns:repeat(12, minmax(100px, 1fr))").length - 1, 2);
});

test("a missing required endpoint cannot be downgraded to a stale or degraded evidence banner", () => {
  for (const runtime_status of ["running", "degraded"] as const) {
    const model = { ...MOCK_MODEL, alerts: [], connected: true,
      status: { ...MOCK_MODEL.status, runtime_status, last_heartbeat: "2000-01-01T00:00:00Z" },
      panelSources: { ...MOCK_MODEL.panelSources, alerts: "mixed" },
      dataSource: { ...MOCK_MODEL.dataSource, state: "partial", reachable: true, missingEndpoints: ["/api/v1/alerts/active"] } } as SystemModel;
    const html = renderToStaticMarkup(React.createElement(AlertsScreen, { model }));
    assert.match(html, /No snapshot/);
    assert.doesNotMatch(html, /No active alerts|Active alerts and diagnostics/);
  }
});

test("dashboard cannot clear alerts or halt history from missing secondary sources", () => {
  const model = { ...MOCK_MODEL, connected: true, alerts: [], feed: [],
    status: { ...MOCK_MODEL.status, daemon_reachable: true, runtime_status: "idle", kill_switch_active: false, risk_halt_active: false, integrity_halt_active: false,
      last_heartbeat: new Date().toISOString() },
    runtimeLeadership: { ...MOCK_MODEL.runtimeLeadership, post_restart_recovery_state: "complete" },
    panelSources: { ...MOCK_MODEL.panelSources, dashboard: "mixed" },
    dataSource: { ...MOCK_MODEL.dataSource, state: "partial", reachable: true, realEndpoints: [],
      missingEndpoints: ["/api/v1/alerts/active", "/api/v1/events/feed", "/api/v1/system/session", "/api/v1/system/config-fingerprint"] } } as SystemModel;
  const html = renderToStaticMarkup(React.createElement(DashboardScreen, { model }));
  assert.match(html, /Active fault source unavailable/);
  assert.match(html, /Halt status or history source unavailable/);
  assert.match(html, /Session authority unavailable/);
  assert.match(html, /Configuration authority unavailable/);
  assert.doesNotMatch(html, /No halt recorded for this run/);
});

test("backend-active empty triage never establishes successful annotation storage reads", () => {
  const model = { ...MOCK_MODEL, connected: true, alertTriage: [],
    alertTriageTruth: { truth_state: "active", note: "Backend-reported annotations" },
    status: { ...MOCK_MODEL.status, runtime_status: "halted", last_heartbeat: new Date().toISOString() },
    panelSources: { ...MOCK_MODEL.panelSources, alerts: "mixed" },
    dataSource: { ...MOCK_MODEL.dataSource, state: "real", reachable: true, missingEndpoints: [] } } as SystemModel;
  const html = renderToStaticMarkup(React.createElement(AlertsScreen, { model }));
  assert.match(html, /Annotation storage read success is not independently reported/);
  assert.match(html, /does not prove that no stored annotation exists/);
  assert.match(html, /Unavailable for this alert/);
  assert.match(html, /Present in the current active alert snapshot/);
  assert.doesNotMatch(html, /No stored annotations|Never acknowledged/);
});
