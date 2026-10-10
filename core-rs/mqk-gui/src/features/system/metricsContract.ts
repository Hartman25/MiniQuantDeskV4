import type { MetricsSection, MetricSeries, SystemMetrics } from "./types";
import { isSourceTimestamp } from "./historyContract";

// Mirrors MetricsDashboardResponse in mqk-daemon/src/api_types.rs.
// This endpoint supplies snapshots, not timestamped metric history.
export function parseMetricsDashboard(value: unknown): SystemMetrics | null {
  if (!value || typeof value !== "object") return null;
  const row = value as Record<string, unknown>;
  if (row.canonical_route !== "/api/v1/metrics/dashboards" ||
      !["active", "no_snapshot"].includes(String(row.portfolio_snapshot_state)) ||
      !["active", "no_snapshot"].includes(String(row.risk_snapshot_state)) ||
      !["active", "no_snapshot"].includes(String(row.execution_snapshot_state)) ||
      !["ok", "unknown", "dirty", "stale", "unavailable"].includes(String(row.reconcile_status)) ||
      typeof row.kill_switch_active !== "boolean" || typeof row.corp_actions_screening !== "string") return null;
  const optionalNumbers = ["account_equity", "cash", "buying_power", "long_market_value", "short_market_value", "daily_pnl", "gross_exposure", "net_exposure", "concentration_pct", "drawdown_pct", "loss_limit_utilization_pct"];
  for (const key of optionalNumbers) {
    if (!(row[key] === null || typeof row[key] === "number" && Number.isFinite(row[key]))) return null;
  }
  for (const key of ["active_order_count", "pending_order_count", "dispatching_order_count", "active_breaches", "reconcile_total_mismatches"]) {
    if (typeof row[key] !== "number" || !Number.isSafeInteger(row[key]) || row[key] < 0) return null;
  }
  if (!(row.reconcile_last_run_at === null || isSourceTimestamp(row.reconcile_last_run_at))) return null;

  const metric = (key: string, label: string, unit: MetricSeries["unit"], available: boolean): MetricSeries => ({
    key, label, unit, window: "1d", points: [],
    current_value: available ? row[key] as number | null : null,
    threshold_warning: null, threshold_critical: null,
  });
  const section = (key: string, title: string, truth_state: MetricsSection["truth_state"], description: string, series: MetricSeries[], operational_state?: string): MetricsSection =>
    ({ key, title, truth_state, description, series, operational_state });
  const portfolioActive = row.portfolio_snapshot_state === "active";
  const riskActive = row.risk_snapshot_state === "active";
  const executionActive = row.execution_snapshot_state === "active";
  const reconcileKnown = row.reconcile_status === "ok" || row.reconcile_status === "dirty";
  return {
    runtime: section("runtime", "Runtime", "not_wired", "Runtime time-series telemetry is not recorded by this endpoint.", []),
    portfolio: section("portfolio", "Portfolio", portfolioActive ? "active" : "no_snapshot", "Broker snapshot; currency/observation time are not included by this metrics endpoint.", [
      metric("account_equity", "Account equity (broker units)", "count", portfolioActive),
      metric("cash", "Cash (broker units)", "count", portfolioActive),
      metric("buying_power", "Buying power (broker units)", "count", portfolioActive),
      metric("daily_pnl", "Daily P&L (broker units)", "count", portfolioActive),
    ]),
    execution: section("execution", "Execution", executionActive ? "active" : "no_snapshot", "Current OMS snapshot. Reject count is omitted: the route hardcodes it to zero.", [
      metric("active_order_count", "Active orders", "count", executionActive),
      metric("pending_order_count", "Pending orders", "count", executionActive),
      metric("dispatching_order_count", "Dispatching orders", "count", executionActive),
    ]),
    fillQuality: section("fill_quality", "Fill quality", "not_wired", "Fill-quality metrics are not present here; inspect recorded execution fill-quality evidence.", []),
    reconciliation: section("reconciliation", "Reconciliation", reconcileKnown ? "active" : "unknown", `Backend status: ${row.reconcile_status}. Last reconciliation: ${row.reconcile_last_run_at ?? "unavailable"}.`, [
      metric("reconcile_total_mismatches", "Total mismatches", "count", reconcileKnown),
    ], String(row.reconcile_status).toUpperCase()),
    riskSafety: section("risk_safety", "Risk and safety", "active", `Exposure snapshot: ${row.risk_snapshot_state}. Corporate-actions screening: ${row.corp_actions_screening}.`, [
      metric("gross_exposure", "Gross exposure (broker units)", "count", riskActive),
      metric("net_exposure", "Net exposure (broker units)", "count", riskActive),
      metric("concentration_pct", "Concentration", "pct", riskActive),
      metric("drawdown_pct", "Drawdown", "pct", riskActive),
      metric("loss_limit_utilization_pct", "Loss limit utilization", "pct", riskActive),
    ], row.kill_switch_active ? "HALTED" : "Kill switch clear; risk readiness not established"),
  };
}
