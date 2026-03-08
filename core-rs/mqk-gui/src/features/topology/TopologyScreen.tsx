import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime, formatDurationMs, formatLabel } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function TopologyScreen({ model }: { model: SystemModel }) {
  const warnings = model.topology.services.filter((s) => s.health === "warning").length;
  const critical = model.topology.services.filter((s) => s.health === "critical" || s.health === "disconnected").length;
  return (
    <div className="screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Tracked Services" value={String(model.topology.services.length)} tone="good" />
        <StatCard title="Warnings" value={String(warnings)} tone={warnings ? "warn" : "good"} />
        <StatCard title="Critical / Down" value={String(critical)} tone={critical ? "bad" : "good"} />
        <StatCard title="Last topology refresh" value={formatDateTime(model.topology.updated_at)} detail="Dependency map from daemon health model" tone="neutral" />
      </div>

      <Panel title="Service dependency graph" subtitle="Operator view of failure propagation across runtime, broker, data, risk, reconcile, and audit layers.">
        <div className="service-topology-grid">
          {model.topology.services.map((service) => (
            <div key={service.service_key} className={`service-node health-${service.health}`}>
              <div className="alert-header">
                <strong>{service.label}</strong>
                <span>{formatLabel(service.health)}</span>
              </div>
              <div className="summary-detail">{formatLabel(service.layer)} · {service.role}</div>
              <div className="summary-detail">Heartbeat {formatDateTime(service.last_heartbeat)} · Latency {formatDurationMs(service.latency_ms)}</div>
              <div className="summary-detail">Depends on: {service.dependency_keys.join(", ") || "—"}</div>
              <div className="summary-detail">Impact: {service.failure_impact}</div>
              <div className="summary-detail">{service.notes}</div>
            </div>
          ))}
        </div>
      </Panel>
    </div>
  );
}
