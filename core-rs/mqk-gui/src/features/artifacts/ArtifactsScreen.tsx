import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime, formatLabel, formatNumber } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function ArtifactsScreen({ model }: { model: SystemModel }) {
  const registry = model.artifactRegistry;

  return (
    <div className="screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Ready" value={formatNumber(registry.ready_count)} tone="good" />
        <StatCard title="Pending" value={formatNumber(registry.pending_count)} tone={registry.pending_count > 0 ? "warn" : "neutral"} />
        <StatCard title="Failed" value={formatNumber(registry.failed_count)} tone={registry.failed_count > 0 ? "bad" : "good"} />
        <StatCard title="Last Updated" value={formatDateTime(registry.last_updated_at)} tone="neutral" />
      </div>

      <Panel title="Artifact registry" subtitle="Forensics are not real unless the desk can find the bundles, traces, replays, and receipts.">
        <DataTable
          rows={registry.artifacts}
          rowKey={(row) => row.artifact_id}
          columns={[
            { key: "id", title: "Artifact", render: (row) => row.artifact_id },
            { key: "type", title: "Type", render: (row) => formatLabel(row.artifact_type) },
            { key: "created", title: "Created", render: (row) => formatDateTime(row.created_at) },
            { key: "status", title: "Status", render: (row) => formatLabel(row.status) },
            { key: "order", title: "Order", render: (row) => row.linked_order_id ?? "—" },
            { key: "incident", title: "Incident", render: (row) => row.linked_incident_id ?? "—" },
            { key: "run", title: "Run", render: (row) => row.linked_run_id ?? "—" },
            { key: "path", title: "Path", render: (row) => row.storage_path },
            { key: "note", title: "Note", render: (row) => row.note },
          ]}
        />
      </Panel>
    </div>
  );
}
