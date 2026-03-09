import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import type { SystemModel } from "../system/types";

export function AlertsScreen({ model }: { model: SystemModel }) {
  const critical = model.alerts.filter((a) => a.severity === "critical").length;
  const warning = model.alerts.filter((a) => a.severity === "warning").length;
  const info = model.alerts.filter((a) => a.severity === "info").length;

  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Active Alerts" value={String(model.alerts.length)} detail="All alert severities" tone={model.alerts.length > 0 ? "warn" : "good"} />
        <StatCard title="Critical" value={String(critical)} detail="Immediate attention" tone={critical > 0 ? "bad" : "good"} />
        <StatCard title="Warning" value={String(warning)} detail="Needs operator review" tone={warning > 0 ? "warn" : "good"} />
        <StatCard title="Info" value={String(info)} detail="Context-only alerts" tone="neutral" />
      </div>

      <Panel title="Active alert surface" subtitle="Primary triage surface for current alerts.">
        <DataTable
          rows={model.alerts}
          rowKey={(row) => row.id}
          columns={[
            { key: "severity", title: "Severity", render: (row) => row.severity },
            { key: "domain", title: "Domain", render: (row) => row.domain },
            { key: "title", title: "Title", render: (row) => row.title },
            { key: "message", title: "Message", render: (row) => row.message },
          ]}
        />
      </Panel>

      <Panel title="Alert triage board" subtitle="Ack/escalation linkage to incidents and orders.">
        <DataTable
          rows={model.alertTriage}
          rowKey={(row) => row.alert_id}
          columns={[
            { key: "severity", title: "Severity", render: (row) => row.severity },
            { key: "status", title: "Status", render: (row) => row.status },
            { key: "title", title: "Title", render: (row) => row.title },
            { key: "domain", title: "Domain", render: (row) => row.domain },
            { key: "incident", title: "Incident", render: (row) => row.linked_incident_id ?? "—" },
            { key: "order", title: "Order", render: (row) => row.linked_order_id ?? "—" },
            { key: "strategy", title: "Strategy", render: (row) => row.linked_strategy_id ?? "—" },
            { key: "assigned", title: "Assigned", render: (row) => row.assigned_to ?? "—" },
          ]}
        />
      </Panel>
    </div>
  );
}
