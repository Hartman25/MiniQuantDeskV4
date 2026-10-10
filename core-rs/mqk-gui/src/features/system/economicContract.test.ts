import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { parsePortfolioSummary, parseRiskSummary, parseReconcileSummary } from "./economicContract";
import { fetchOperatorModel } from "./api";
import { RiskScreen } from "../risk/RiskScreen";
import { DashboardScreen } from "../dashboard/DashboardScreen";
import { DEFAULT_STATUS } from "./types";
import { MOCK_MODEL } from "./mockData";
import { ReconcileScreen } from "../reconcile/ReconcileScreen";
import { PortfolioScreen } from "../portfolio/PortfolioScreen";
import type { SystemModel } from "./types";

const portfolio = { has_snapshot: true, truth_state: "active", account_equity: 12500, cash: 4000,
  long_market_value: null, short_market_value: null, buying_power: null, daily_pnl: null,
  daily_pnl_truth_state: "baseline_unavailable", daily_pnl_unavailable_reason: "prior session baseline absent" };
const risk = { has_snapshot: true, truth_state: "active", gross_exposure: null, net_exposure: null,
  concentration_pct: null, daily_pnl: null, drawdown_pct: null, loss_limit_utilization_pct: null,
  kill_switch_active: false, active_breaches: 0 };

test("nullable economic measurements stay unavailable and malformed summaries fail closed", () => {
  assert.equal(parsePortfolioSummary(portfolio)!.daily_pnl, null);
  assert.equal(parsePortfolioSummary({ ...portfolio, daily_pnl: 0 }), null);
  assert.equal(parsePortfolioSummary({ ...portfolio, cash: "0" }), null);
  assert.equal(parsePortfolioSummary({ ...portfolio, has_snapshot: false, truth_state: "no_snapshot" }), null);
  assert.equal(parseRiskSummary({ ...risk, concentration_pct: Infinity }), null);
  const failed = parseRiskSummary({ ...risk, truth_state: "query_failed" })!;
  assert.equal(failed.kill_switch_active, true);
  assert.equal(failed.active_breaches, null);
});

test("production economic payloads render without null crashes or false-clear risk summary", async () => {
  const original = globalThis.fetch;
  const payloads: Record<string, unknown> = {
    "/api/v1/system/status": DEFAULT_STATUS,
    "/api/v1/portfolio/summary": portfolio,
    "/api/v1/risk/summary": risk,
    "/api/v1/risk/denials": { truth_state: "durable_history", denials: [] },
    "/api/v1/execution/summary": { has_snapshot: true, active_orders: 0, pending_orders: 0, dispatching_orders: 0, reject_count_today: 0, cancel_replace_count_today: null, avg_ack_latency_ms: null, stuck_orders: 0 },
    "/api/v1/reconcile/status": { truth_state: "never_run", status: "unknown", last_run_at: null, mismatched_positions: 0, mismatched_fills: 0, mismatched_orders: 0, unmatched_broker_events: 0 },
  };
  globalThis.fetch = (async (input) => {
    const path = new URL(String(input)).pathname;
    return path in payloads ? Response.json(payloads[path]) : Response.json({}, { status: 404 });
  }) as typeof fetch;
  try {
    const model = await fetchOperatorModel();
    assert.equal(model.portfolioSummary.account_equity, 12500);
    assert.equal(model.riskSummary.concentration_pct, null);
    const html = renderToStaticMarkup(React.createElement(RiskScreen, { model }));
    assert.match(html, /Concentration/);
    assert.doesNotMatch(html, /stat-good/);
    assert.match(renderToStaticMarkup(React.createElement(DashboardScreen, { model })), /Loss-limit utilization/);
    payloads["/api/v1/risk/summary"] = {};
    const invalid = await fetchOperatorModel();
    assert.equal(invalid.riskSummary.active_breaches, null);
    assert.match(renderToStaticMarkup(React.createElement(RiskScreen, { model: invalid })), /No snapshot/);
  } finally { globalThis.fetch = original; }
});

test("never-run reconcile and unavailable detail lanes cannot render clean counters or empty-as-clear", () => {
  const summary = { truth_state: "never_run", status: "unknown", last_run_at: null,
    mismatched_positions: 0, mismatched_orders: 0, mismatched_fills: 0, unmatched_broker_events: 0 };
  const parsed = parseReconcileSummary(summary)!;
  assert.equal(parsed.mismatched_orders, null);
  assert.equal(parseReconcileSummary({ ...summary, truth_state: "future" }), null);
  assert.equal(parseReconcileSummary({ ...summary, last_run_at: "2026-10-10T12:00:00" }), null);
  assert.equal(parseReconcileSummary({ ...summary, mismatched_orders: -1 }), null);
  const model = { ...MOCK_MODEL, connected: true, status: DEFAULT_STATUS, reconcileSummary: parsed,
    runtimeLeadership: { ...MOCK_MODEL.runtimeLeadership, post_restart_recovery_state: "complete" },
    panelSources: { ...MOCK_MODEL.panelSources, reconcile: "mixed", portfolio: "broker_snapshot" },
    dataSource: { ...MOCK_MODEL.dataSource, reachable: true, state: "partial", realEndpoints: ["/api/v1/reconcile/status", "/api/v1/portfolio/positions"],
      missingEndpoints: ["/api/v1/reconcile/mismatches", "/api/v1/portfolio/orders/open", "/api/v1/portfolio/fills"] } } as SystemModel;
  const html = renderToStaticMarkup(React.createElement(ReconcileScreen, { model }));
  assert.match(html, /Mismatch detail unavailable/);
  assert.match(html, /Replace\/cancel lineage is unavailable/);
  assert.match(html, /Incident source unavailable/);
  assert.doesNotMatch(html, /clean across all domains|All incidents resolved|stat-good/);
  const portfolioHtml = renderToStaticMarkup(React.createElement(PortfolioScreen, { model }));
  assert.match(portfolioHtml, /Open-order snapshot unavailable/);
  assert.match(portfolioHtml, /Recent-fill snapshot unavailable/);
});
