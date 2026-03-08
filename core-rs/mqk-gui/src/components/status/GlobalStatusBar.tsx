import { formatDateTime, formatLatency, healthTone, runtimeTone } from "../../lib/format";
import type { SystemStatus } from "../../features/system/types";
import { StatusPill } from "./StatusPill";

interface GlobalStatusBarProps {
  status: SystemStatus;
}

export function GlobalStatusBar({ status }: GlobalStatusBarProps) {
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
        <StatusPill label="Database" value={status.db_status} tone={healthTone(status.db_status)} />
        <StatusPill label="Market Data" value={status.market_data_health} tone={healthTone(status.market_data_health)} />
        <StatusPill label="Reconcile" value={status.reconcile_status} tone={healthTone(status.reconcile_status)} />
        <StatusPill label="Integrity" value={status.integrity_status} tone={healthTone(status.integrity_status)} />
        <StatusPill label="Audit" value={status.audit_writer_status} tone={healthTone(status.audit_writer_status)} />
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
      </div>
    </header>
  );
}
