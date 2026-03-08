import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { formatDateTime, formatLabel } from "../../lib/format";
import type { SystemModel } from "../system/types";

export function IncidentsScreen({ model }: { model: SystemModel }) {
  const open = model.incidents.filter((i) => i.status !== "resolved").length;
  const critical = model.incidents.filter((i) => i.severity === "critical").length;
  return (
    <div className="screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Active Incidents" value={String(open)} tone={open ? "warn" : "good"} />
        <StatCard title="Critical Incidents" value={String(critical)} tone={critical ? "bad" : "good"} />
        <StatCard title="Linked Reconcile Cases" value={String(model.incidents.flatMap((i) => i.reconcile_case_ids).length)} tone="neutral" />
        <StatCard title="Operator Actions Logged" value={String(model.incidents.flatMap((i) => i.operator_actions_taken).length)} tone="neutral" />
      </div>
      <Panel title="Incident workspace" subtitle="Case-centric grouping of alerts, orders, reconcile cases, actions, and final disposition.">
        <DataTable
          rows={model.incidents}
          rowKey={(row) => row.incident_id}
          columns={[
            { key: "incident", title: "Incident", render: (row) => row.incident_id },
            { key: "title", title: "Title", render: (row) => row.title },
            { key: "status", title: "Status", render: (row) => formatLabel(row.status) },
            { key: "orders", title: "Orders", render: (row) => row.impacted_orders.join(", ") },
            { key: "subsystems", title: "Subsystems", render: (row) => row.impacted_subsystems.join(", ") },
            { key: "actions", title: "Actions Taken", render: (row) => row.operator_actions_taken.join(", ") || "—" },
            { key: "updated", title: "Updated", render: (row) => formatDateTime(row.updated_at) },
          ]}
        />
      </Panel>
      <div className="two-column-grid">
        {model.incidents.map((incident) => (
          <Panel key={incident.incident_id} title={incident.title} subtitle={`${incident.incident_id} · ${formatLabel(incident.status)}`}>
            <div className="metric-list compact-list">
              <div><span>Opened</span><strong>{formatDateTime(incident.opened_at)}</strong></div>
              <div><span>Alerts</span><strong>{incident.alerts.join(", ") || "—"}</strong></div>
              <div><span>Strategies</span><strong>{incident.impacted_strategies.join(", ") || "—"}</strong></div>
              <div><span>Reconcile</span><strong>{incident.reconcile_case_ids.join(", ") || "—"}</strong></div>
              <div><span>Disposition</span><strong>{incident.final_disposition}</strong></div>
            </div>
          </Panel>
        ))}
      </div>
    </div>
  );
}
