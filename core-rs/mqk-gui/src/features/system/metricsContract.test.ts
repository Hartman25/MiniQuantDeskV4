import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { fetchOperatorModel } from "./api";
import { parseMetricsDashboard } from "./metricsContract";
import { MetricsScreen } from "../metrics/MetricsScreen";
import { DEFAULT_STATUS } from "./types";

const snapshot = {
  canonical_route: "/api/v1/metrics/dashboards", portfolio_snapshot_state: "no_snapshot", risk_snapshot_state: "no_snapshot", execution_snapshot_state: "no_snapshot",
  account_equity: null, cash: null, buying_power: null, long_market_value: null, short_market_value: null, daily_pnl: null,
  gross_exposure: null, net_exposure: null, concentration_pct: null, drawdown_pct: null, loss_limit_utilization_pct: null,
  kill_switch_active: true, active_breaches: 1, active_order_count: 0, pending_order_count: 0, dispatching_order_count: 0, reject_count_today: 0,
  reconcile_status: "unknown", reconcile_last_run_at: null, reconcile_total_mismatches: 0, corp_actions_screening: "not_wired",
};
test("snapshot metrics never promote absent snapshots to zero or healthy, and omit fabricated reject telemetry", () => {
  const metrics = parseMetricsDashboard(snapshot)!;
  assert.equal(metrics.execution.series[0].current_value, null);
  assert.equal(metrics.reconciliation.series[0].current_value, null);
  assert.equal(metrics.riskSafety.operational_state, "HALTED");
  assert.equal(metrics.portfolio!.series[3].current_value, null);
  assert.equal(metrics.execution.series.some((row) => row.key === "reject_count_today"), false);
  assert.equal(metrics.execution.series[0].points.length, 0);
  const active = parseMetricsDashboard({ ...snapshot, execution_snapshot_state: "active", active_order_count: 17, reconcile_status: "dirty", reconcile_total_mismatches: 3 })!;
  assert.equal(active.execution.series[0].current_value, 17);
  assert.equal(active.reconciliation.series[0].current_value, 3);
  for (const body of [null, {}, { ...snapshot, cash: undefined }, { ...snapshot, active_order_count: -1 }, { ...snapshot, cash: Infinity }, { ...snapshot, risk_snapshot_state: "future" }]) assert.equal(parseMetricsDashboard(body), null);
});
test("real flat daemon metrics traverse production fetch/model/React without nested-contract crash or false green", async () => {
  const original = globalThis.fetch;
  globalThis.fetch = (async (input, init) => {
    assert.equal(init?.method, "GET");
    const path = new URL(String(input)).pathname;
    if (path === "/api/v1/system/status") return Response.json(DEFAULT_STATUS);
    if (path === "/api/v1/metrics/dashboards") return Response.json(snapshot);
    return Response.json({}, { status: 404 });
  }) as typeof fetch;
  try {
    const model = await fetchOperatorModel();
    assert.equal(model.metrics.execution.series[0].current_value, null);
    const html = renderToStaticMarkup(React.createElement(MetricsScreen, { model }));
    assert.match(html, /HALTED/);
    assert.match(html, /Unavailable/);
    assert.match(html, /history unavailable/);
    assert.doesNotMatch(html, /All telemetry domains within bounds|>OK<|\$0/);
  } finally { globalThis.fetch = original; }
});
