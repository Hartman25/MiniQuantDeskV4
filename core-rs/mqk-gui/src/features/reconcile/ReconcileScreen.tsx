import { DataTable } from "../../components/common/DataTable";
import { Panel } from "../../components/common/Panel";
import { StatCard } from "../../components/common/StatCard";
import { TruthStateBanner } from "../../components/common/TruthStateBanner";
import { TruthStateNotice } from "../../components/common/TruthStateNotice";
import { formatDateTime } from "../../lib/format";
import { isTruthHardBlock, panelTruthRenderState } from "../system/truthRendering";
import type { SystemModel } from "../system/types";

export function ReconcileScreen({ model }: { model: SystemModel }) {
  const r = model.reconcileSummary;
  const truthState = panelTruthRenderState(model, "reconcile");

  // Hard-block when truth is structurally absent (unavailable, no_snapshot, unimplemented,
  // not_wired). For stale/degraded, data is cached and present — show the domain body with
  // a warning banner so the operator still sees reconcile context rather than a blank screen.
  if (truthState !== null && isTruthHardBlock(truthState)) {
    return <TruthStateNotice state={truthState} />;
  }

  return (
    <div className="screen-grid desk-screen-grid">
      {truthState !== null && <TruthStateBanner state={truthState} />}
      <div className="summary-grid summary-grid-four">
        <StatCard
          title="Reconcile Status"
          value={r.status}
          detail={`Last run ${formatDateTime(r.last_run_at)}`}
          tone={r.status === "critical" ? "bad" : r.status === "warning" ? "warn" : "good"}
        />
        <StatCard title="Mismatched Positions" value={String(r.mismatched_positions)} tone={r.mismatched_positions > 0 ? "warn" : "good"} />
        <StatCard title="Mismatched Orders" value={String(r.mismatched_orders)} tone={r.mismatched_orders > 0 ? "warn" : "good"} />
        <StatCard title="Unmatched Events" value={String(r.unmatched_broker_events)} tone={r.unmatched_broker_events > 0 ? "warn" : "good"} />
      </div>

      <div className="desk-panel-grid desk-panel-grid-primary">
        <Panel title="Mismatch grid" subtitle="Primary broker-vs-internal disagreement surface.">
          <DataTable
            rows={model.mismatches}
            rowKey={(row) => row.id}
            columns={[
              { key: "domain", title: "Domain", render: (row) => row.domain },
              { key: "symbol", title: "Symbol", render: (row) => row.symbol },
              { key: "internal", title: "Internal", render: (row) => row.internal_value },
              { key: "broker", title: "Broker", render: (row) => row.broker_value },
              { key: "note", title: "Note", render: (row) => row.note },
            ]}
          />
        </Panel>

        <Panel title="Correction / chain state" subtitle="What still needs operator attention right now.">
          <div className="metric-list">
            <div><span>Mismatched fills</span><strong>{r.mismatched_fills}</strong></div>
            <div><span>Unmatched events</span><strong>{r.unmatched_broker_events}</strong></div>
            <div><span>Replace/cancel chains</span><strong>{model.replaceCancelChains.length}</strong></div>
            <div><span>Active incidents</span><strong>{model.incidents.length}</strong></div>
            <div><span>Latest runtime generation</span><strong>{model.runtimeLeadership.generation_id}</strong></div>
            <div><span>Recovery state</span><strong>{model.runtimeLeadership.post_restart_recovery_state}</strong></div>
          </div>
        </Panel>
      </div>

      <Panel title="Replace / cancel chains" compact>
        <DataTable
          rows={model.replaceCancelChains}
          rowKey={(row) => row.chain_id}
          columns={[
            { key: "symbol", title: "Symbol", render: (row) => row.symbol },
            { key: "strategy", title: "Strategy", render: (row) => row.strategy_id },
            { key: "action", title: "Action", render: (row) => row.action_type },
            { key: "status", title: "Status", render: (row) => row.status },
            { key: "request", title: "Requested", render: (row) => formatDateTime(row.request_at) },
            { key: "notes", title: "Notes", render: (row) => row.notes },
          ]}
        />
      </Panel>

      <div className="two-column-grid">
        <Panel title="Drift by domain" subtitle="Mismatch count per domain — which class of disagreement is active.">
          {model.mismatches.length === 0 ? (
            <div className="empty-state">No active mismatches. Reconcile is clean across all domains.</div>
          ) : (
            <div className="metric-list">
              {(["position", "order", "fill", "cash", "event"] as const).map((domain) => {
                const rows = model.mismatches.filter((m) => m.domain === domain);
                if (rows.length === 0) return null;
                const hasCritical = rows.some((m) => m.status === "critical");
                const hasWarning = rows.some((m) => m.status === "warning");
                return (
                  <div key={domain}>
                    <span>{domain}</span>
                    <strong style={{ color: hasCritical ? "var(--critical)" : hasWarning ? "var(--warning)" : "var(--good)" }}>
                      {rows.length}
                    </strong>
                  </div>
                );
              })}
            </div>
          )}
        </Panel>

        <Panel title="Active incidents" subtitle="Open and investigating incidents with reconcile impact.">
          {model.incidents.filter((i) => i.status !== "resolved" && i.status !== "contained").length === 0 ? (
            <div className="empty-state">No active incidents. All incidents resolved or contained.</div>
          ) : (
            <div className="list-stack">
              {model.incidents
                .filter((i) => i.status !== "resolved" && i.status !== "contained")
                .map((incident) => (
                  <div key={incident.incident_id} className="alert-card">
                    <div className="alert-header">
                      <strong>{incident.title}</strong>
                      <span>{incident.severity} · {incident.status}</span>
                    </div>
                    {incident.impacted_subsystems.length > 0 && (
                      <div className="summary-detail">Subsystems: {incident.impacted_subsystems.join(", ")}</div>
                    )}
                    {incident.reconcile_case_ids.length > 0 && (
                      <div className="summary-detail">Reconcile cases: {incident.reconcile_case_ids.join(", ")}</div>
                    )}
                    <div className="summary-detail">Opened {formatDateTime(incident.opened_at)}</div>
                  </div>
                ))}
            </div>
          )}
        </Panel>
      </div>
    </div>
  );
}
