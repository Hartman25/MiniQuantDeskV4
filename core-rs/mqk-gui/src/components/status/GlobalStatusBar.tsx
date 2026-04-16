import { formatDateTime, formatLatency, healthTone, runtimeTone } from "../../lib/format";
import type { DataSourceDetail, SystemStatus } from "../../features/system/types";
import { StatusPill } from "./StatusPill";

// DESKTOP-12: WS continuity tone — separate from broker REST health.
// "live" is the only proven state; all others are fail-closed warnings or critical.
function wsContinuityTone(state: SystemStatus["alpaca_ws_continuity"]): "info" | "warning" | "critical" {
  switch (state) {
    case "live":
      return "info";
    case "gap_detected":
      return "critical";
    case "cold_start_unproven":
    default:
      return "warning";
  }
}

interface GlobalStatusBarProps {
  status: SystemStatus;
  dataSource?: DataSourceDetail;
}

function dataSourceTone(state: DataSourceDetail["state"]): "info" | "warning" | "critical" {
  switch (state) {
    case "real":
      return "info";
    case "partial":
    case "mock":
      return "warning";
    case "disconnected":
    default:
      return "critical";
  }
}

function dataSourceLabel(dataSource?: DataSourceDetail): string {
  return (dataSource?.state ?? "disconnected").toUpperCase();
}

function dataSourceSummary(dataSource?: DataSourceDetail): string {
  if (!dataSource) return "daemon status unknown";
  if (dataSource.state === "disconnected") return "daemon unreachable";
  if (dataSource.state === "mock") return "mock fallback active";
  return `${dataSource.realEndpoints.length} real / ${dataSource.missingEndpoints.length} missing`;
}

export function GlobalStatusBar({ status, dataSource }: GlobalStatusBarProps) {
  return (
    <header className="global-status-bar">
      <div className="global-status-primary">
        <StatusPill
          label="Environment"
          value={status.environment}
          tone={status.environment === "live" ? "critical" : status.environment === "paper" ? "warning" : "info"}
          emphasis={status.environment === "live" ? "loud" : "normal"}
        />
        <StatusPill label="Runtime" value={status.runtime_status} tone={runtimeTone(status.runtime_status)} />
        <StatusPill label="Broker" value={status.broker_status} tone={healthTone(status.broker_status)} />
        {/* DESKTOP-12: WS continuity is a distinct truth from broker REST health.
            Shown only when Alpaca WS applies — hidden for paper/synthetic deployments.
            "cold_start_unproven" and "gap_detected" are start-blocking states that
            broker_status alone does not distinguish from a healthy REST connection. */}
        {status.alpaca_ws_continuity !== "not_applicable" && (
          <StatusPill
            label="WS Continuity"
            value={status.alpaca_ws_continuity}
            tone={wsContinuityTone(status.alpaca_ws_continuity)}
            emphasis={status.alpaca_ws_continuity === "gap_detected" ? "loud" : "normal"}
          />
        )}
        <StatusPill label="Database" value={status.db_status} tone={healthTone(status.db_status)} />
        <StatusPill label="Market Data" value={status.market_data_health} tone={healthTone(status.market_data_health)} />
        <StatusPill label="Reconcile" value={status.reconcile_status} tone={healthTone(status.reconcile_status)} />
        <StatusPill label="Integrity" value={status.integrity_status} tone={healthTone(status.integrity_status)} />
        <StatusPill label="Audit" value={status.audit_writer_status} tone={healthTone(status.audit_writer_status)} />
        <StatusPill
          label="Data Source"
          value={dataSourceLabel(dataSource)}
          tone={dataSourceTone(dataSource?.state ?? "disconnected")}
          emphasis={dataSource?.state === "disconnected" ? "loud" : "normal"}
        />
      </div>
      <div className="global-status-secondary">
        <div className="status-metric">
          <span className="metric-label">Heartbeat</span>
          <span className="metric-value">{formatDateTime(status.last_heartbeat)}</span>
        </div>
        <div className="status-metric">
          <span className="metric-label">Loop Latency</span>
          <span className="metric-value">{formatLatency(status.loop_latency_ms)}</span>
        </div>
        <div className="status-metric">
          <span className="metric-label">Account</span>
          <span className="metric-value">{status.active_account_id ?? "—"}</span>
        </div>
        <div className="status-metric">
          <span className="metric-label">Config</span>
          <span className="metric-value">{status.config_profile ?? "—"}</span>
        </div>
        <div className="status-metric">
          <span className="metric-label">Source Detail</span>
          <span className="metric-value">{dataSourceSummary(dataSource)}</span>
        </div>
        <div className="status-metric">
          <span className="metric-label">Missing</span>
          <span className="metric-value">
            {dataSource && dataSource.missingEndpoints.length > 0 ? dataSource.missingEndpoints.slice(0, 2).join(", ") : "—"}
          </span>
        </div>
      </div>
    </header>
  );
}
