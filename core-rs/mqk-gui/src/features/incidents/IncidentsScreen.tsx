import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function IncidentsScreen({ model }: { model: SystemModel }) {
  const critical = model.incidents.filter((i) => i.severity === "critical").length;
  const investigating = model.incidents.filter((i) => i.status === "investigating").length;
  const truthState = panelTruthRenderState(model, "incidents");

  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Open Incidents" value={String(model.incidents.length)} detail="Active operator cases" tone={model.incidents.length > 0 ? "warn" : "good"} />
        <StatCard title="Critical" value={String(critical)} detail="Highest severity cases" tone={critical > 0 ? "bad" : "good"} />
        <StatCard title="Investigating" value={String(investigating)} detail="Cases under active review" tone={investigating > 0 ? "warn" : "good"} />
        <StatCard title="Artifacts Ready" value={String(model.artifactRegistry.ready_count)} detail="Evidence bundles available" tone="good" />
      </div>

      <Panel title="Incident workspace" subtitle="Group alerts, orders, reconcile cases, and actions by active incident.">
        <DataTable
          rows={model.incidents}
          rowKey={(row) => row.incident_id}
          columns={[
            { key: "severity", title: "Severity", render: (row) => row.severity },
            { key: "title", title: "Title", render: (row) => row.title },
            { key: "status", title: "Status", render: (row) => row.status },
            { key: "opened", title: "Opened", render: (row) => formatDateTime(row.opened_at) },
            { key: "updated", title: "Updated", render: (row) => formatDateTime(row.updated_at) },
            { key: "subsystems", title: "Subsystems", render: (row) => row.impacted_subsystems.join(", ") },
            { key: "actions", title: "Actions", render: (row) => row.operator_actions_taken.join(", ") || "—" },
          ]}
        />
      </Panel>
    </div>
  );
}
