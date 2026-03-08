import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime, formatLabel } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function AlertsScreen({ model }: { model: SystemModel }) {
  const unacked = model.alertTriage.filter((a) => a.status === "unacked").length;
  const escalated = model.alertTriage.filter((a) => a.status === "escalated").length;
  return (
    <div className="screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Alert Queue" value={String(model.alertTriage.length)} tone="neutral" />
        <StatCard title="Unacked" value={String(unacked)} tone={unacked ? "warn" : "good"} />
        <StatCard title="Escalated" value={String(escalated)} tone={escalated ? "bad" : "good"} />
        <StatCard title="Acked" value={String(model.alertTriage.filter((a) => a.status === "acked").length)} tone="good" />
      </div>
      <Panel title="Alert triage board" subtitle="Institutional alert workflow with acknowledgment, escalation, and incident linkage.">
        <DataTable
          rows={model.alertTriage}
          rowKey={(row) => row.alert_id}
          columns={[
            { key: "alert", title: "Alert", render: (row) => row.alert_id },
            { key: "severity", title: "Severity", render: (row) => formatLabel(row.severity) },
            { key: "status", title: "Status", render: (row) => formatLabel(row.status) },
            { key: "title", title: "Title", render: (row) => row.title },
            { key: "domain", title: "Domain", render: (row) => row.domain },
            { key: "incident", title: "Incident", render: (row) => row.linked_incident_id ?? "—" },
            { key: "order", title: "Order", render: (row) => row.linked_order_id ?? "—" },
            { key: "created", title: "Created", render: (row) => formatDateTime(row.created_at) },
          ]}
        />
      </Panel>
    </div>
  );
}
