import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import type { OperatorTimelineEvent, SystemModel } from "../system/types";
import { formatDateTime, formatLabel } from "../../lib/format";

const TIMELINE_COLUMNS = [
  { key: "at", title: "At", render: (row: OperatorTimelineEvent) => formatDateTime(row.at) },
  { key: "category", title: "Category", render: (row: OperatorTimelineEvent) => formatLabel(row.category) },
  { key: "severity", title: "Severity", render: (row: OperatorTimelineEvent) => row.severity },
  { key: "title", title: "Title", render: (row: OperatorTimelineEvent) => row.title },
  { key: "links", title: "Linked Context", render: (row: OperatorTimelineEvent) => [row.linked_incident_id, row.linked_order_id, row.linked_strategy_id].filter(Boolean).join(" · ") || "—" },
  { key: "actor", title: "Actor", render: (row: OperatorTimelineEvent) => row.actor },
];

export function OperatorTimelineScreen({ model }: { model: SystemModel }) {
  return (
    <div className="screen-grid">
      <div className="summary-grid summary-grid-four">
        <div className="panel summary-card"><div className="eyebrow">Timeline events</div><div className="summary-value">{model.operatorTimeline.length}</div><div className="summary-detail">Alerts, actions, restarts, config changes, incidents.</div></div>
        <div className="panel summary-card"><div className="eyebrow">Latest generation</div><div className="summary-value">{model.runtimeLeadership.generation_id}</div><div className="summary-detail">Current runtime generation in force.</div></div>
        <div className="panel summary-card"><div className="eyebrow">Open incidents</div><div className="summary-value">{model.incidents.filter((incident) => incident.status !== "resolved").length}</div><div className="summary-detail">Incidents still under containment or investigation.</div></div>
        <div className="panel summary-card"><div className="eyebrow">Recent config diffs</div><div className="summary-value">{model.configDiffs.length}</div><div className="summary-detail">Version changes visible to operators.</div></div>
      </div>

      <Panel title="Operator timeline" subtitle="One place to reconstruct what the system and operators did during an incident window.">
        <div className="operator-timeline-stack">
          {model.operatorTimeline.map((event) => (
            <div key={event.timeline_event_id} className={`operator-timeline-card severity-${event.severity}`}>
              <div className="operator-timeline-head">
                <div>
                  <div className="eyebrow">{formatLabel(event.category)}</div>
                  <strong>{event.title}</strong>
                </div>
                <div className="operator-timeline-meta">
                  <span>{formatDateTime(event.at)}</span>
                  <span>{event.actor}</span>
                </div>
              </div>
              <p className="summary-detail">{event.summary}</p>
              <div className="operator-timeline-links">
                <span>Incident: {event.linked_incident_id ?? "—"}</span>
                <span>Order: {event.linked_order_id ?? "—"}</span>
                <span>Strategy: {event.linked_strategy_id ?? "—"}</span>
                <span>Generation: {event.linked_runtime_generation_id ?? "—"}</span>
              </div>
            </div>
          ))}
        </div>
      </Panel>

      <Panel title="Timeline register" subtitle="Tabular view for export-style review and fast scanning.">
        <DataTable rows={model.operatorTimeline} columns={TIMELINE_COLUMNS} rowKey={(row) => row.timeline_event_id} />
      </Panel>
    </div>
  );
}
