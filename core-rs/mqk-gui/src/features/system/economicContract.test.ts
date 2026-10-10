import test from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { parsePortfolioSummary, parseRiskSummary } from "./economicContract";
import { fetchOperatorModel } from "./api";
import { RiskScreen } from "../risk/RiskScreen";
import { DashboardScreen } from "../dashboard/DashboardScreen";
import { DEFAULT_STATUS } from "./types";

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
    "/api/v1/execution/summary": { has_snapshot: true, active_orders: 0, pending_orders: 0, stuck_orders: 0 },
    "/api/v1/reconcile/status": { status: "unknown", mismatched_orders: 0, unmatched_broker_events: 0 },
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
    assert.doesNotMatch(html, /stat-card stat-good/);
    assert.match(renderToStaticMarkup(React.createElement(DashboardScreen, { model })), /Loss-limit utilization/);
    payloads["/api/v1/risk/summary"] = {};
    const invalid = await fetchOperatorModel();
    assert.equal(invalid.riskSummary.active_breaches, null);
    assert.match(renderToStaticMarkup(React.createElement(RiskScreen, { model: invalid })), /No snapshot/);
  } finally { globalThis.fetch = original; }
});
