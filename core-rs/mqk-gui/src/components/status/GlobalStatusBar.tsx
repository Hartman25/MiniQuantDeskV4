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

// WAVE-02-FINAL-REPAIR-01 R1: fixed Tier-0 safety truth for kill-switch,
// live-routing, critical/warning incidents, and daemon reachability.
// status.kill_switch_active / live_routing_enabled / has_critical /
// has_warning are plain booleans that fail open to a "safe-looking" default
// (see DEFAULT_STATUS) while the daemon is unreachable — so every derivation
// below is gated on status.daemon_reachable first and must render "unknown",
// never the boolean's face value, once reachability is lost.

export type KillSwitchDisplayState = "active" | "inactive" | "unknown";

export function killSwitchDisplayState(status: SystemStatus): KillSwitchDisplayState {
  if (!status.daemon_reachable) return "unknown";
  return status.kill_switch_active ? "active" : "inactive";
}

// An active kill switch means trading is halted — alarming but safe; render
// loud/critical so it cannot be mistaken for a quiet "all clear".
function killSwitchTone(state: KillSwitchDisplayState): "info" | "warning" | "critical" {
  switch (state) {
    case "active":
      return "critical";
    case "unknown":
      return "warning";
    case "inactive":
    default:
      return "info";
  }
}

export type LiveRoutingDisplayState = "enabled" | "disabled" | "unknown";

export function liveRoutingDisplayState(status: SystemStatus): LiveRoutingDisplayState {
  if (!status.daemon_reachable) return "unknown";
  return status.live_routing_enabled ? "enabled" : "disabled";
}

// Live routing enabled is the single most safety-critical truth in this
// system (real broker order routing) — always render it loud when enabled.
function liveRoutingTone(state: LiveRoutingDisplayState): "info" | "warning" | "critical" {
  switch (state) {
    case "enabled":
      return "critical";
    case "unknown":
      return "warning";
    case "disabled":
    default:
      return "info";
  }
}

export type IncidentDisplayState = "critical" | "warning" | "clear" | "unknown";

export function incidentDisplayState(status: SystemStatus): IncidentDisplayState {
  if (!status.daemon_reachable) return "unknown";
  if (status.has_critical) return "critical";
  if (status.has_warning) return "warning";
  return "clear";
}

function incidentTone(state: IncidentDisplayState): "info" | "warning" | "critical" {
  switch (state) {
    case "critical":
      return "critical";
    case "warning":
    case "unknown":
      return "warning";
    case "clear":
    default:
      return "info";
  }
}

export type ConnectivityDisplayState = "online" | "offline";

export function connectivityDisplayState(status: SystemStatus): ConnectivityDisplayState {
  return status.daemon_reachable ? "online" : "offline";
}

function connectivityTone(state: ConnectivityDisplayState): "info" | "warning" | "critical" {
  return state === "online" ? "info" : "critical";
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
  const noSnapshot = dataSource.noSnapshotEndpoints?.length ?? 0;
  const noActiveRun = dataSource.noActiveRunEndpoints?.length ?? 0;
  // "unavailable" = failed probes that are not in the pre-run state buckets above.
  const genuinelyUnavailable = dataSource.missingEndpoints.length - noSnapshot - noActiveRun;
  const parts: string[] = [`${dataSource.realEndpoints.length} real`];
  if (genuinelyUnavailable > 0) parts.push(`${genuinelyUnavailable} unavailable`);
  if (noSnapshot > 0) parts.push(`${noSnapshot} no snapshot`);
  if (noActiveRun > 0) parts.push(`${noActiveRun} no active run`);
  return parts.join(" / ");
}

export function GlobalStatusBar({ status, dataSource }: GlobalStatusBarProps) {
  const killSwitchState = killSwitchDisplayState(status);
  const liveRoutingState = liveRoutingDisplayState(status);
  const incidentState = incidentDisplayState(status);
  const connectivityState = connectivityDisplayState(status);

  return (
    <header className="global-status-bar">
      <div className="global-status-primary">
        <StatusPill
          label="Connection"
          value={connectivityState}
          tone={connectivityTone(connectivityState)}
          emphasis={connectivityState === "offline" ? "loud" : "normal"}
        />
        <StatusPill
          label="Kill Switch"
          value={killSwitchState}
          tone={killSwitchTone(killSwitchState)}
          emphasis={killSwitchState === "active" ? "loud" : "normal"}
        />
        <StatusPill
          label="Live Routing"
          value={liveRoutingState}
          tone={liveRoutingTone(liveRoutingState)}
          emphasis={liveRoutingState === "enabled" ? "loud" : "normal"}
        />
        <StatusPill
          label="Incidents"
          value={incidentState}
          tone={incidentTone(incidentState)}
          emphasis={incidentState === "critical" ? "loud" : "normal"}
        />
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
          <span className="metric-label">Unavailable</span>
          <span className="metric-value">
            {(() => {
              if (!dataSource) return "—";
              const excluded = new Set([
                ...(dataSource.noSnapshotEndpoints ?? []),
                ...(dataSource.noActiveRunEndpoints ?? []),
              ]);
              const genuinely = dataSource.missingEndpoints.filter((ep) => !excluded.has(ep));
              return genuinely.length > 0 ? genuinely.slice(0, 2).join(", ") : "—";
            })()}
          </span>
        </div>
      </div>
    </header>
  );
}
