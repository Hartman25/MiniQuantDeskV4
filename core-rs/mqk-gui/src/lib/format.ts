import type { HealthState, MetricSeries, RuntimeStatus, Severity } from "../features/system/types";

export function formatLabel(value: string): string {
  return value
    .replace(/_/g, " ")
    .replace(/\b\w/g, (m) => m.toUpperCase());
}

export function formatDateTime(value: string | null): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
}

export function formatLatency(value: number | null): string {
  if (value === null || Number.isNaN(value)) return "—";
  return `${value.toFixed(0)} ms`;
}

export function formatDurationMs(value: number | null): string {
  if (value == null || Number.isNaN(value)) return "—";
  if (value < 1000) return `${value} ms`;
  const seconds = value / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)} s`;
  const minutes = seconds / 60;
  return `${minutes.toFixed(1)} min`;
}

export function formatNumber(value: number | null, digits = 0): string {
  if (value == null || Number.isNaN(value)) return "—";
  return value.toLocaleString(undefined, { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

export function formatMoney(value: number | null): string {
  if (value == null || Number.isNaN(value)) return "—";
  return value.toLocaleString(undefined, { style: "currency", currency: "USD", maximumFractionDigits: 2 });
}

export function formatPercent(value: number | null): string {
  if (value == null || Number.isNaN(value)) return "—";
  return `${value.toFixed(2)}%`;
}

export function formatMetricValue(series: MetricSeries): string {
  switch (series.unit) {
    case "ms":
      return formatLatency(series.current_value);
    case "pct":
      return `${series.current_value.toFixed(1)}%`;
    case "usd":
      return formatMoney(series.current_value);
    case "rate":
      return `${series.current_value.toFixed(1)}/m`;
    case "count":
    default:
      return formatNumber(series.current_value);
  }
}

export function runtimeTone(status: RuntimeStatus): Severity {
  switch (status) {
    case "running":
      return "info";
    case "starting":
    case "paused":
    case "degraded":
      return "warning";
    case "halted":
      return "critical";
    case "idle":
    default:
      return "info";
  }
}

export function healthTone(state: HealthState): Severity {
  switch (state) {
    case "ok":
      return "info";
    case "warning":
    case "unknown":
      return "warning";
    case "critical":
    case "disconnected":
      return "critical";
    default:
      return "warning";
  }
}
