import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

const SEVERITY_PRIORITY: Record<string, number> = { critical: 0, warning: 1, info: 2 };

function statusStyle(status: string): string {
  if (status === "open" || status === "investigating") return "state-warning";
  if (status === "contained" || status === "resolved") return "state-ok";
  return "";
}

function severityStyle(severity: string): string {
  if (severity === "critical") return "state-critical";
  if (severity === "warning") return "state-warning";
  return "";
}

export function IncidentsScreen({ model }: { model: SystemModel }) {
  const truthState = panelTruthRenderState(model, "incidents");

  if (truthState !== null) {
    return <TruthStateNotice state={truthState} />;
  }

  // Active = cases still requiring containment work (open or investigating).
  // Sort critical before warning so the most urgent case is always first.
  const active = model.incidents
    .filter((i) => i.status === "open" || i.status === "investigating")
    .sort((a, b) => (SEVERITY_PRIORITY[a.severity] ?? 9) - (SEVERITY_PRIORITY[b.severity] ?? 9));

  const closed = model.incidents.filter(
    (i) => i.status === "contained" || i.status === "resolved",
  );

  const criticalActive = active.filter((i) => i.severity === "critical");
  const totalImpactedOrders = active.reduce((n, i) => n + i.impacted_orders.length, 0);
  const totalActionsOnActive = active.reduce((n, i) => n + i.operator_actions_taken.length, 0);

  return (
    <div className="screen-grid desk-screen-grid">
      {/* Containment posture header — active-case focused, not raw case count */}
      <div className="summary-grid summary-grid-four">
        <StatCard
          title="Active Cases"
          value={String(active.length)}
          detail={`${model.incidents.filter((i) => i.status === "open").length} open · ${model.incidents.filter((i) => i.status === "investigating").length} investigating`}
          tone={criticalActive.length > 0 ? "bad" : active.length > 0 ? "warn" : "good"}
        />
        <StatCard
          title="Critical Active"
          value={String(criticalActive.length)}
          detail="Active cases at highest severity"
          tone={criticalActive.length > 0 ? "bad" : "neutral"}
        />
        <StatCard
          title="Impacted Orders"
          value={String(totalImpactedOrders)}
          detail="Orders in blast radius across active cases"
          tone={totalImpactedOrders > 0 ? "warn" : "neutral"}
        />
        <StatCard
          title="Actions on Active"
          value={String(totalActionsOnActive)}
          detail="Operator actions recorded across active cases"
          tone="neutral"
        />
      </div>

      {/* Primary: active containment posture — critical before warning, scope and action history inline */}
      <Panel
        title="Active containment — open and investigating"
        subtitle={
          active.length === 0
            ? "No active incidents. All cases are contained or resolved."
            : `${active.length} case${active.length === 1 ? "" : "s"} require operator attention. Critical before warning.`
        }
      >
        {active.length === 0 ? (
          <div className="empty-state">No active incidents. All cases are contained or resolved.</div>
        ) : (
          <div className="operator-timeline-stack">
            {active.map((inc) => (
              <div key={inc.incident_id} className={`operator-timeline-card severity-${inc.severity}`}>
                <div className="operator-timeline-head">
                  <strong>
                    <span className={severityStyle(inc.severity)}>{inc.severity}</span>
                    {" · "}
                    <span className={statusStyle(inc.status)}>{inc.status}</span>
                    {" — "}
                    {inc.title}
                  </strong>
                  <span className="operator-timeline-meta">{inc.incident_id}</span>
                </div>

                {/* Timing — opened vs last updated reveals whether the case is still expanding */}
                <div className="operator-timeline-meta">
                  <span>opened {formatDateTime(inc.opened_at)}</span>
                  <span>updated {formatDateTime(inc.updated_at)}</span>
                </div>

                {/* Impacted scope — blast radius for this case */}
                {(inc.impacted_subsystems.length > 0 ||
                  inc.impacted_orders.length > 0 ||
                  inc.impacted_strategies.length > 0) && (
                  <div className="operator-timeline-meta">
                    {inc.impacted_subsystems.length > 0 && (
                      <span>subsystems: {inc.impacted_subsystems.join(", ")}</span>
                    )}
                    {inc.impacted_orders.length > 0 && (
                      <span>
                        {inc.impacted_orders.length}{" "}
                        {inc.impacted_orders.length === 1 ? "order" : "orders"} impacted
                      </span>
                    )}
                    {inc.impacted_strategies.length > 0 && (
                      <span>
                        {inc.impacted_strategies.length}{" "}
                        {inc.impacted_strategies.length === 1 ? "strategy" : "strategies"} impacted
                      </span>
                    )}
                  </div>
                )}

                {/* Evidence linkage — alerts and reconcile cases grouped to this incident */}
                {(inc.alerts.length > 0 || inc.reconcile_case_ids.length > 0) && (
                  <div className="operator-timeline-meta">
                    {inc.alerts.length > 0 && (
                      <span>
                        {inc.alerts.length} {inc.alerts.length === 1 ? "alert" : "alerts"} linked
                      </span>
                    )}
                    {inc.reconcile_case_ids.length > 0 && (
                      <span>reconcile: {inc.reconcile_case_ids.join(", ")}</span>
                    )}
                  </div>
                )}

                {/* Action history — what the operator has already done and current disposition */}
                <div className="operator-timeline-meta">
                  {inc.operator_actions_taken.length === 0 ? (
                    <span className="state-warning">no operator actions recorded</span>
                  ) : (
                    <span>actions: {inc.operator_actions_taken.join(" · ")}</span>
                  )}
                  {inc.final_disposition !== "" && (
                    <span>disposition: {inc.final_disposition}</span>
                  )}
                </div>
              </div>
            ))}
          </div>
        )}
      </Panel>

      {/* Secondary: closed cases — compact, for post-incident review only */}
      {closed.length > 0 && (
        <Panel
          title="Contained and resolved"
          subtitle="Cases no longer active. Review final disposition and actions for post-incident analysis."
          compact
        >
          <DataTable
            rows={closed}
            rowKey={(row) => row.incident_id}
            columns={[
              {
                key: "severity",
                title: "Sev",
                render: (row) => (
                  <span className={severityStyle(row.severity)}>{row.severity}</span>
                ),
              },
              {
                key: "status",
                title: "Status",
                render: (row) => (
                  <span className={statusStyle(row.status)}>{row.status}</span>
                ),
              },
              { key: "title", title: "Title", render: (row) => row.title },
              {
                key: "disposition",
                title: "Disposition",
                render: (row) => row.final_disposition || "—",
              },
              {
                key: "updated",
                title: "Updated",
                render: (row) => formatDateTime(row.updated_at),
              },
            ]}
          />
        </Panel>
      )}
    </div>
  );
}
