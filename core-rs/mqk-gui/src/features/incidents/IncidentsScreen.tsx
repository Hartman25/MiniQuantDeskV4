import { useState } from "react";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function IncidentsScreen({ model }: { model: SystemModel }) {
  const [query, setQuery] = useState("");
  const truthState = panelTruthRenderState(model, "incidents");
  if (truthState !== null) return <TruthStateNotice state={truthState} />;
  const active = model.incidents.filter((row) => row.status === "open" || row.status === "investigating");
  const critical = active.filter((row) => row.severity === "critical");
  const rows = model.incidents.filter((row) =>
    [row.incident_id, row.title, row.status, row.severity, row.linked_alert_id, row.opened_by]
      .join(" ").toLowerCase().includes(query.toLowerCase()));
  return (
    <div className="screen-grid desk-screen-grid">
      <div className="summary-grid summary-grid-four">
        <StatCard title="Open incidents" value={String(active.length)} detail="Durable case status" tone={critical.length ? "bad" : active.length ? "warn" : "neutral"} />
        <StatCard title="Critical open" value={String(critical.length)} detail="Requires investigation" tone={critical.length ? "bad" : "neutral"} />
        <StatCard title="Resolved cases" value={String(model.incidents.filter((row) => row.status === "resolved").length)} detail="Case resolution does not prove fault recovery" tone="neutral" />
        <StatCard title="Observation" value={formatDateTime(model.lastUpdatedAt)} detail="Browser observation; not case update time" tone="neutral" />
      </div>
      <Panel title="Incident evidence" subtitle="GET /api/v1/incidents · postgres.sys_incidents · read-only">
        <label>Search incidents <input value={query} onChange={(event) => setQuery(event.target.value)} /></label>
        <p>Impact details, action history, resolution reason, and last update time are unavailable from this endpoint.</p>
        {rows.length === 0 ? <div className="empty-state">{query ? "No matching incidents in this observation." : "No incidents returned by the durable source."}</div> :
          <div className="operator-timeline-stack">{rows.map((row) => (
            <details key={row.incident_id} className={`operator-timeline-card severity-${row.severity}`}>
              <summary>{row.severity} · {row.status} · {row.title} · {row.incident_id}</summary>
              <dl>
                <dt>Opened</dt><dd>{formatDateTime(row.opened_at)}</dd>
                <dt>Opened by</dt><dd>{row.opened_by ?? "Unavailable"}</dd>
                <dt>Linked alert identity</dt><dd>{row.linked_alert_id ?? (row.alerts.join(", ") || "No link recorded")}</dd>
                <dt>Current fault evidence</dt><dd>{model.dataSource.missingEndpoints.includes("/api/v1/alerts/active")
                  ? "Active alert source unavailable; recovery unknown."
                  : model.alerts.find((alert) => alert.id === row.linked_alert_id)?.message ?? "No matching active alert in this observation; recovery is not established."}</dd>
                <dt>Execution domain / account</dt><dd>Not recorded on this incident.</dd>
                <dt>Authority</dt><dd>Case status from postgres.sys_incidents. Read-only investigation does not acknowledge, resolve, or change trading state.</dd>
              </dl>
            </details>
          ))}</div>}
      </Panel>
    </div>
  );
}
