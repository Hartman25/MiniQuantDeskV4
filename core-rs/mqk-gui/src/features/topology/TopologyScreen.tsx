import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime, formatDurationMs } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";
import type { ServiceDependencyNode } from "../system/types/infra";

export function TopologyScreen({ model }: { model: SystemModel }) {
  const truthState = panelTruthRenderState(model, "topology");

  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  const services = model.topology.services;

  const degradedServices = services.filter((s) => s.health !== "ok");

  function getDownstream(serviceKey: string): ServiceDependencyNode[] {
    return services.filter((s) => s.dependency_keys.includes(serviceKey));
  }

  // Per-layer: count degraded vs total; sort degraded layers first
  const allLayers = Array.from(new Set(services.map((s) => s.layer))).sort();
  const layerRisk = allLayers
    .map((layer) => {
      const inLayer = services.filter((s) => s.layer === layer);
      const degradedCount = inLayer.filter((s) => s.health !== "ok").length;
      return { layer, total: inLayer.length, degraded: degradedCount };
    })
    .sort((a, b) => b.degraded - a.degraded || b.total - a.total);

  // Ledger: degraded first, then alphabetically by layer
  const sortedServices = [...services].sort((a, b) => {
    const aDeg = a.health !== "ok" ? 0 : 1;
    const bDeg = b.health !== "ok" ? 0 : 1;
    if (aDeg !== bDeg) return aDeg - bDeg;
    return a.layer.localeCompare(b.layer);
  });

  return (
    <div className="screen-grid desk-screen-grid">

      {/* Blast-radius triage — only rendered when degraded services exist */}
      {degradedServices.length > 0 && (
        <Panel
          title="Degraded services — blast-radius triage"
          subtitle="Services with active health issues. Upstream dependencies and downstream blast radius shown per entry. Resolve upstream first."
        >
          <div className="service-topology-grid">
            {degradedServices.map((svc) => {
              const upstream = services.filter((s) =>
                svc.dependency_keys.includes(s.service_key)
              );
              const downstream = getDownstream(svc.service_key);
              return (
                <div key={svc.service_key} className={`service-node health-${svc.health}`}>
                  <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", gap: 8, marginBottom: 6 }}>
                    <strong>{svc.label}</strong>
                    <span style={{ color: "var(--muted)", fontSize: "0.72rem", textTransform: "uppercase", letterSpacing: "0.07em", flexShrink: 0 }}>
                      {svc.layer}
                    </span>
                  </div>
                  <p style={{ color: "var(--muted)", fontSize: "0.82rem", margin: "0 0 10px" }}>
                    {svc.failure_impact || "—"}
                  </p>
                  {upstream.length > 0 && (
                    <div style={{ fontSize: "0.78rem", marginBottom: 8 }}>
                      <span style={{ color: "var(--muted)", textTransform: "uppercase", letterSpacing: "0.05em", fontSize: "0.70rem" }}>
                        Upstream deps
                      </span>
                      <div style={{ marginTop: 5, display: "flex", flexWrap: "wrap", gap: 4 }}>
                        {upstream.map((u) => (
                          <span
                            key={u.service_key}
                            style={{
                              padding: "2px 8px",
                              borderRadius: 6,
                              border: "1px solid var(--border)",
                              background: "rgba(11,21,36,0.7)",
                              color: u.health !== "ok" ? "var(--warning)" : "var(--text)",
                              fontSize: "0.78rem",
                            }}
                          >
                            {u.label}
                          </span>
                        ))}
                      </div>
                    </div>
                  )}
                  {downstream.length > 0 && (
                    <div style={{ fontSize: "0.78rem" }}>
                      <span style={{ color: "var(--muted)", textTransform: "uppercase", letterSpacing: "0.05em", fontSize: "0.70rem" }}>
                        Blast radius
                      </span>
                      <div style={{ marginTop: 5, display: "flex", flexWrap: "wrap", gap: 4 }}>
                        {downstream.map((d) => (
                          <span
                            key={d.service_key}
                            style={{
                              padding: "2px 8px",
                              borderRadius: 6,
                              border: "1px solid var(--border)",
                              background: "rgba(11,21,36,0.7)",
                              color: d.health !== "ok" ? "var(--critical)" : "var(--muted)",
                              fontSize: "0.78rem",
                            }}
                          >
                            {d.label}
                          </span>
                        ))}
                      </div>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </Panel>
      )}

      {/* Layer risk posture — which layer is carrying the most topology risk */}
      <Panel
        title="Layer risk posture"
        subtitle="Degraded service concentration by infrastructure layer. Layers with degraded services are listed first."
      >
        <div className="metric-list">
          {layerRisk.map(({ layer, total, degraded }) => (
            <div key={layer}>
              <span>{layer}</span>
              <strong
                className={
                  degraded > 0
                    ? degraded === total
                      ? "state-critical"
                      : "state-warning"
                    : "state-ok"
                }
              >
                {degraded > 0 ? `${degraded} / ${total} degraded` : `${total} healthy`}
              </strong>
            </div>
          ))}
        </div>
      </Panel>

      {/* Service dependency ledger — reference for tracing propagation chains */}
      <Panel
        title="Service dependency ledger"
        subtitle="Complete dependency map for chain tracing. Degraded services sorted first. Use upstream/blast-radius columns to trace propagation paths."
      >
        <DataTable
          rows={sortedServices}
          rowKey={(row) => row.service_key}
          columns={[
            { key: "health", title: "Health", render: (row) => row.health },
            { key: "service", title: "Service", render: (row) => row.label },
            { key: "layer", title: "Layer", render: (row) => row.layer },
            { key: "role", title: "Role", render: (row) => row.role },
            { key: "deps", title: "Upstream Deps", render: (row) => row.dependency_keys.join(", ") || "—" },
            { key: "impact", title: "Failure Impact", render: (row) => row.failure_impact },
            { key: "latency", title: "Latency", render: (row) => formatDurationMs(row.latency_ms) },
            { key: "heartbeat", title: "Last Heartbeat", render: (row) => formatDateTime(row.last_heartbeat) },
          ]}
        />
      </Panel>
    </div>
  );
}
