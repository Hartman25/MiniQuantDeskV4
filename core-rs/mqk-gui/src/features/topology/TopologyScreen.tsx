import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime, formatDurationMs } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function TopologyScreen({ model }: { model: SystemModel }) {
  const truthState = panelTruthRenderState(model, "topology");

  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  return (
    <div className="screen-grid desk-screen-grid">
      <Panel title="Service topology" subtitle="Dependency map for daemon, runtime, broker, data, reconcile, audit, strategy, and risk.">
        <DataTable
          rows={model.topology.services}
          rowKey={(row) => row.service_key}
          columns={[
            { key: "service", title: "Service", render: (row) => row.label },
            { key: "layer", title: "Layer", render: (row) => row.layer },
            { key: "health", title: "Health", render: (row) => row.health },
            { key: "role", title: "Role", render: (row) => row.role },
            { key: "deps", title: "Dependencies", render: (row) => row.dependency_keys.join(", ") || "—" },
            { key: "latency", title: "Latency", render: (row) => formatDurationMs(row.latency_ms) },
            { key: "heartbeat", title: "Last Heartbeat", render: (row) => formatDateTime(row.last_heartbeat) },
            { key: "impact", title: "Failure Impact", render: (row) => row.failure_impact },
          ]}
        />
      </Panel>
    </div>
  );
}
