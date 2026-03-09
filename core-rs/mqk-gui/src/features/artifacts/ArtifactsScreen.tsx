import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function ArtifactsScreen({ model }: { model: SystemModel }) {
  const a = model.artifactRegistry;

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Ready" value={String(a.ready_count)} detail="Artifacts ready for review" tone="good" />
        <StatCard title="Pending" value={String(a.pending_count)} detail="Artifacts still generating" tone={a.pending_count > 0 ? "warn" : "good"} />
        <StatCard title="Failed" value={String(a.failed_count)} detail="Artifact generation failures" tone={a.failed_count > 0 ? "bad" : "good"} />
        <StatCard title="Updated" value={formatDateTime(a.last_updated_at)} detail="Registry refresh time" tone="neutral" />
      </div>

      <Panel title="Artifact registry" subtitle="Trace, replay, incident, reconcile, and operator evidence bundles.">
        <DataTable
          rows={a.artifacts}
          rowKey={(row) => row.artifact_id}
          columns={[
            { key: "type", title: "Type", render: (row) => row.artifact_type },
            { key: "created", title: "Created", render: (row) => formatDateTime(row.created_at) },
            { key: "status", title: "Status", render: (row) => row.status },
            { key: "order", title: "Order", render: (row) => row.linked_order_id ?? "—" },
            { key: "incident", title: "Incident", render: (row) => row.linked_incident_id ?? "—" },
            { key: "path", title: "Path", render: (row) => row.storage_path },
            { key: "note", title: "Note", render: (row) => row.note },
          ]}
        />
      </Panel>
    </div>
  );
}
