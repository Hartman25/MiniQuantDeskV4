import { useState } from "react";
import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { selectHaltEvents } from "../system/haltSummary";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

const PRIORITY: Record<string, number> = { critical: 0, warning: 1, info: 2 };
export function AlertsScreen({ model }: { model: SystemModel }) {
  const [query, setQuery] = useState("");
  const [severity, setSeverity] = useState("all");
  const truthState = panelTruthRenderState(model, "alerts");
  if (truthState !== null) return <TruthStateNotice state={truthState} />;
  const triageAvailable = model.alertTriageTruth?.truth_state === "active";
  const triage = new Map(model.alertTriage.map((row) => [row.alert_id, row]));
  const alerts = model.alerts.filter((row) => (severity === "all" || row.severity === severity) &&
    [row.id, row.title, row.message, row.source, row.domain].join(" ").toLowerCase().includes(query.toLowerCase()))
    .sort((a, b) => PRIORITY[a.severity] - PRIORITY[b.severity] || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
  const halts = selectHaltEvents(model.feed);
  const feedAvailable = model.dataSource.realEndpoints.includes("/api/v1/events/feed");
  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Active fault signals" value={String(model.alerts.length)} detail="Acknowledged alerts remain active until the fault clears" tone={model.alerts.some((row) => row.severity === "critical") ? "bad" : model.alerts.length ? "warn" : "neutral"} />
        <StatCard title="Critical" value={String(model.alerts.filter((row) => row.severity === "critical").length)} tone="neutral" />
        <StatCard title="Acknowledgement source" value={triageAvailable ? "Reported by backend" : "Unavailable"} detail="Advisory annotation; does not resolve or suppress faults" tone="neutral" />
        <StatCard title="Observation" value={formatDateTime(model.lastUpdatedAt)} detail="Browser observation; not first or latest occurrence" tone="neutral" />
      </div>
      <Panel title="Active alerts and diagnostics" subtitle="GET /api/v1/alerts/active · current daemon fault signals · read-only">
        <label>Search alerts <input value={query} onChange={(event) => setQuery(event.target.value)} /></label>{" "}
        <label>Severity <select value={severity} onChange={(event) => setSeverity(event.target.value)}>
          <option value="all">All</option><option value="critical">Critical</option><option value="warning">Warning</option><option value="info">Info</option>
        </select></label>
        <p>{model.alertTriageTruth?.note ?? "Acknowledgement authority unavailable."}</p>
        <p>First/latest occurrence, retention, and execution domain/account are not recorded by the active alert contract. Disappearance from this poll does not establish recovery.</p>
        {alerts.length === 0 ? <div className="empty-state">{query || severity !== "all" ? "No matching active alerts." : "No current fault signals returned by the active source."}</div> :
          <div className="operator-timeline-stack">{alerts.map((alert) => {
            const annotation = triageAvailable ? triage.get(alert.id) : undefined;
            const linked = annotation?.linked_incident_id;
            return <details key={alert.id} id={`alert-${encodeURIComponent(alert.id)}`} className={`operator-timeline-card severity-${alert.severity}`}>
              <summary>{alert.severity} · {alert.title} · {alert.id}</summary>
              <dl>
                <dt>Explanation / violated condition</dt><dd>{alert.message}</dd>
                <dt>Source / subsystem</dt><dd>{alert.source ?? "Unavailable"} · {alert.domain}</dd>
                <dt>Fault identity</dt><dd>{alert.fault_class ?? alert.id}</dd>
                <dt>State</dt><dd>Present in the current active alert snapshot; advisory acknowledgement does not prove recovery.</dd>
                <dt>Acknowledgement annotation</dt><dd>{annotation ? annotation.status : "Unavailable for this alert"}</dd>
                <dt>Acknowledged at</dt><dd>{annotation?.status === "acked" ? formatDateTime(annotation.created_at) : "Unavailable"}</dd>
                <dt>Incident evidence</dt><dd>{linked ? `${linked} · ${annotation?.linked_incident_status ?? "status unavailable"}` : "No authoritative linkage available"}</dd>
                {linked && <dt>Case detail</dt>}{linked && <dd>{model.incidents.find((row) => row.incident_id === linked)?.title ?? "Incident detail unavailable in this observation; inspect Incidents by identity."}</dd>}
                <dt>Next action</dt><dd>Investigate the named subsystem and evidence. Any runtime or case mutation requires an existing authorized operator workflow.</dd>
              </dl>
            </details>;
          })}</div>}
      </Panel>
      <Panel title="Halt history" subtitle="Bounded events/feed observation · newest first · history completeness unavailable">
        {!feedAvailable ? <div className="unavailable-notice">Events feed unavailable. Halt history is unknown.</div> : halts.length === 0 ? <div className="empty-state">No halt rows in the current bounded feed.</div> :
          <DataTable rows={halts} rowKey={(row) => row.id} columns={[
            { key: "at", title: "Time", render: (row) => formatDateTime(row.at) },
            { key: "detail", title: "Evidence", render: (row) => row.text },
            { key: "run", title: "Run", render: (row) => row.run_id ?? "Unavailable" },
            { key: "audit", title: "Audit ID", render: (row) => row.audit_event_id ?? "Unavailable" },
          ]} />}
      </Panel>
      <Panel title="System events feed — context only" subtitle="Bounded durable events; use Operator Timeline for its separate bounded lifecycle projection">
        {!feedAvailable ? <div className="unavailable-notice">Event history unavailable; empty rows are not authoritative.</div> :
          <DataTable rows={model.feed} rowKey={(row) => row.id} columns={[
            { key: "at", title: "Time", render: (row) => formatDateTime(row.at) },
            { key: "id", title: "Identity", render: (row) => row.id },
            { key: "source", title: "Source", render: (row) => row.source },
            { key: "detail", title: "Evidence", render: (row) => row.text },
          ]} />}
      </Panel>
    </div>
  );
}
