import type { PortfolioSummary, RiskSummary, ReconcileSummary } from "./types";
import { isSourceTimestamp } from "./historyContract";

const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object";
const measurement = (value: unknown) => value === null || (typeof value === "number" && Number.isFinite(value));

export function parsePortfolioSummary(value: unknown): PortfolioSummary | null {
  if (!object(value) || typeof value.has_snapshot !== "boolean" ||
      value.truth_state !== (value.has_snapshot ? "active" : "no_snapshot")) return null;
  const keys = ["account_equity", "cash", "long_market_value", "short_market_value", "daily_pnl", "buying_power"];
  if (keys.some((key) => !measurement(value[key]) || (!value.has_snapshot && value[key] !== null))) return null;
  if (!["active", "baseline_unavailable", "stale_baseline", "no_snapshot", "db_unavailable"].includes(String(value.daily_pnl_truth_state)) ||
      !(value.daily_pnl_unavailable_reason === null || typeof value.daily_pnl_unavailable_reason === "string") ||
      (value.daily_pnl_truth_state !== "active" && value.daily_pnl !== null)) return null;
  return value as unknown as PortfolioSummary;
}

export function parseRiskSummary(value: unknown): RiskSummary | null {
  if (!object(value) || typeof value.has_snapshot !== "boolean" ||
      !["active", "no_db", "query_failed"].includes(String(value.truth_state)) || typeof value.kill_switch_active !== "boolean" ||
      typeof value.active_breaches !== "number" || !Number.isSafeInteger(value.active_breaches) || value.active_breaches < 0) return null;
  const keys = ["gross_exposure", "net_exposure", "concentration_pct", "daily_pnl", "drawdown_pct", "loss_limit_utilization_pct"];
  if (keys.some((key) => !measurement(value[key]))) return null;
  // A failed durable risk read cannot prove a clear switch or a zero breach count.
  return { ...value, kill_switch_active: value.truth_state !== "active" || value.kill_switch_active,
    active_breaches: value.truth_state === "active" ? value.active_breaches : null } as unknown as RiskSummary;
}

export function parseReconcileSummary(value: unknown): ReconcileSummary | null {
  if (!object(value) || !["active", "never_run", "stale"].includes(String(value.truth_state)) ||
      !["ok", "dirty", "stale", "unknown", "unavailable"].includes(String(value.status)) ||
      !(value.last_run_at === null || isSourceTimestamp(value.last_run_at))) return null;
  const keys = ["mismatched_positions", "mismatched_orders", "mismatched_fills", "unmatched_broker_events"];
  if (keys.some((key) => typeof value[key] !== "number" || !Number.isSafeInteger(value[key]) || (value[key] as number) < 0)) return null;
  const assessed = value.truth_state === "active" && (value.status === "ok" || value.status === "dirty");
  return { ...value, ...Object.fromEntries(keys.map((key) => [key, assessed ? value[key] : null])) } as unknown as ReconcileSummary;
}
