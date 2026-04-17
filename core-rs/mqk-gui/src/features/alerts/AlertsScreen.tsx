import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

// The events feed is fail-closed: when backend_unavailable, api.ts places
// /api/v1/events/feed in missingEndpoints and model.feed === [].
// We check missingEndpoints to distinguish "genuinely empty" from "backend down".
const FEED_ENDPOINT = "/api/v1/events/feed" as const;

const STATUS_PRIORITY: Record<string, number> = { unacked: 0, escalated: 1, acked: 2, silenced: 3 };
const SEVERITY_PRIORITY: Record<string, number> = { critical: 0, warning: 1, info: 2 };

function statusStyle(status: string): string {
  if (status === "unacked") return "state-critical";
  if (status === "escalated") return "state-warning";
  if (status === "acked") return "state-ok";
  return "";
}

function severityStyle(severity: string): string {
  if (severity === "critical") return "state-critical";
  if (severity === "warning") return "state-warning";
  return "";
}

export function AlertsScreen({ model }: { model: SystemModel }) {
  const truthState = panelTruthRenderState(model, "alerts");
  if (truthState !== null) return <TruthStateNotice state={truthState} />;

  const triage = [...model.alertTriage].sort((a, b) => {
    const sd = (STATUS_PRIORITY[a.status] ?? 9) - (STATUS_PRIORITY[b.status] ?? 9);
    if (sd !== 0) return sd;
    return (SEVERITY_PRIORITY[a.severity] ?? 9) - (SEVERITY_PRIORITY[b.severity] ?? 9);
  });

  const unacked = triage.filter((r) => r.status === "unacked");
  const escalated = triage.filter((r) => r.status === "escalated");
  const managed = triage.filter((r) => r.status === "acked" || r.status === "silenced");
  const needsAction = triage.filter((r) => r.status === "unacked" || r.status === "escalated");
  const ownerGaps = unacked.filter((r) => r.assigned_to == null);
  const hasCriticalAction = needsAction.some((r) => r.severity === "critical");

  // Raw alerts not yet covered by any triage row — incoming but unmanaged.
  const triageIds = new Set(model.alertTriage.map((r) => r.alert_id));
  const untriaged = model.alerts.filter((a) => !triageIds.has(a.id));

  return (
    <div className="screen-grid desk-screen-grid">

      {/* Triage posture header — action-gap focused, not raw severity distribution */}
      <div className="summary-grid summary-grid-four">
        <StatCard
          title="Needs Action"
          value={String(needsAction.length)}
          detail={`${unacked.length} unacked · ${escalated.length} escalated`}
          tone={hasCriticalAction ? "bad" : needsAction.length > 0 ? "warn" : "good"}
        />
        <StatCard
          title="Escalated"
          value={String(escalated.length)}
          detail="Pending escalation response"
          tone={escalated.length > 0 ? "warn" : "neutral"}
        />
        <StatCard
          title="Acknowledged"
          value={String(triage.filter((r) => r.status === "acked").length)}
          detail="Under management"
          tone="neutral"
        />
        <StatCard
          title="Ownership Gaps"
          value={String(ownerGaps.length)}
          detail="Unacked with no assigned owner"
          tone={ownerGaps.length > 0 ? "warn" : "neutral"}
        />
      </div>

      {/* Primary triage surface — unacked and escalated, sorted: critical before warning */}
      <Panel
        title="Action required — unacked and escalated"
        subtitle={
          needsAction.length === 0
            ? "No alerts require immediate action."
            : `${needsAction.length} alert${needsAction.length === 1 ? "" : "s"} need operator attention. Unacked before escalated, critical before warning.`
        }
      >
        {needsAction.length === 0 ? (
          <div className="empty-state">All alerts are acknowledged or silenced.</div>
        ) : (
          <div className="operator-timeline-stack">
            {needsAction.map((row) => {
              const linkage = [
                row.linked_incident_id && `incident ${row.linked_incident_id}`,
                row.linked_order_id && `order ${row.linked_order_id}`,
                row.linked_strategy_id && `strategy ${row.linked_strategy_id}`,
              ].filter(Boolean).join(" · ");

              return (
                <div key={row.alert_id} className={`operator-timeline-card severity-${row.severity}`}>
                  <div className="operator-timeline-head">
                    <strong>
                      <span className={statusStyle(row.status)}>{row.status}</span>
                      {" · "}
                      <span className={severityStyle(row.severity)}>{row.severity}</span>
                      {" — "}
                      {row.title}
                    </strong>
                    <span className="operator-timeline-meta">{row.domain}</span>
                  </div>
                  <div className="operator-timeline-meta">
                    <span>
                      {"owner: "}
                      {row.assigned_to != null
                        ? row.assigned_to
                        : <span className="state-warning">unassigned</span>}
                    </span>
                    {linkage && <span>{linkage}</span>}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </Panel>

      {/* Secondary — acked/silenced: already under management, no immediate action */}
      {managed.length > 0 && (
        <Panel
          title="Under management — acked and silenced"
          subtitle="No immediate action required. Monitor for escalation criteria changes."
        >
          <DataTable
            rows={managed}
            rowKey={(row) => row.alert_id}
            columns={[
              { key: "status", title: "Status", render: (row) => <span className={statusStyle(row.status)}>{row.status}</span> },
              { key: "severity", title: "Sev", render: (row) => <span className={severityStyle(row.severity)}>{row.severity}</span> },
              { key: "title", title: "Title", render: (row) => row.title },
              { key: "domain", title: "Domain", render: (row) => row.domain },
              { key: "incident", title: "Incident", render: (row) => row.linked_incident_id ?? "—" },
              { key: "order", title: "Order", render: (row) => row.linked_order_id ?? "—" },
              { key: "owner", title: "Owner", render: (row) => row.assigned_to ?? <span className="state-warning">—</span> },
            ]}
          />
        </Panel>
      )}

      {/* Untriaged raw alerts — not yet under acknowledgment or escalation management */}
      {untriaged.length > 0 && (
        <Panel
          title="Untriaged raw alerts"
          subtitle="Alerts from the active surface with no triage row yet — not under acknowledgment or escalation management."
        >
          <DataTable
            rows={untriaged}
            rowKey={(row) => row.id}
            columns={[
              { key: "severity", title: "Sev", render: (row) => <span className={severityStyle(row.severity)}>{row.severity}</span> },
              { key: "domain", title: "Domain", render: (row) => row.domain },
              { key: "title", title: "Title", render: (row) => row.title },
              { key: "message", title: "Message", render: (row) => row.message },
            ]}
          />
        </Panel>
      )}

      {/* Supporting context only — not the triage surface.
          GUI-OPS-03: Events feed panel — explicit truth notice when backend unavailable.
          feed is fail-closed: api.ts places endpoint in missingEndpoints on backend_unavailable,
          so model.feed === [] even when daemon has recorded events. Operator must see this
          distinction rather than "no events" appearing authoritative. */}
      <Panel
        title="System events feed — context only"
        subtitle={
          model.dataSource.missingEndpoints.includes(FEED_ENDPOINT)
            ? "backend_unavailable — event history not accessible"
            : `postgres.runs + postgres.audit_events (${model.feed.length} events) — chronological record; use Operator Timeline for full session history`
        }
      >
        {model.dataSource.missingEndpoints.includes(FEED_ENDPOINT) ? (
          <div className="unavailable-notice">
            Events feed backend unavailable. Empty feed is NOT authoritative — do not read as "no events".
          </div>
        ) : model.feed.length === 0 ? (
          <div className="empty-state">No system events recorded yet.</div>
        ) : (
          <DataTable
            rows={model.feed}
            rowKey={(row) => row.id}
            columns={[
              { key: "at", title: "At", render: (row) => formatDateTime(row.at) },
              { key: "source", title: "Source", render: (row) => row.source },
              { key: "severity", title: "Severity", render: (row) => row.severity },
              { key: "text", title: "Event", render: (row) => row.text },
            ]}
          />
        )}
      </Panel>
    </div>
  );
}
