import { Panel } from "../common/Panel";
import { formatDateTime, formatMoney } from "../../lib/format";
import type { SystemModel } from "../../features/system/types";

export function RightOpsRail({ model }: { model: SystemModel }) {
  const topAlerts = model.alerts.slice(0, 4);
  const topIncidents = model.incidents.slice(0, 3);

  return (
    <aside className="right-rail">
      <Panel title="Operator context" compact>
        <div className="metric-list compact-list">
          <div><span>Environment</span><strong>{model.status.environment}</strong></div>
          <div><span>Runtime</span><strong>{model.status.runtime_status}</strong></div>
          <div><span>Source</span><strong>{model.dataSource?.state ?? "unknown"}</strong></div>
          <div><span>Connected</span><strong>{model.connected ? "Yes" : "No"}</strong></div>
        </div>
      </Panel>

      <Panel title="Portfolio snapshot" compact>
        <div className="metric-list compact-list">
          <div><span>Equity</span><strong>{formatMoney(model.portfolioSummary.account_equity)}</strong></div>
          <div><span>Cash</span><strong>{formatMoney(model.portfolioSummary.cash)}</strong></div>
          <div><span>Buying power</span><strong>{formatMoney(model.portfolioSummary.buying_power)}</strong></div>
          <div><span>Positions</span><strong>{model.positions.length}</strong></div>
        </div>
      </Panel>

      <Panel title="Alerts" compact>
        {topAlerts.length > 0 ? (
          <div className="list-stack compact-list">
            {topAlerts.map((alert) => (
              <div key={alert.id} className="list-row">
                <strong>{alert.title}</strong>
                <span>{alert.severity}</span>
              </div>
            ))}
          </div>
        ) : (
          <div className="empty-state">No active alerts.</div>
        )}
      </Panel>

      <Panel title="Incidents" compact>
        {topIncidents.length > 0 ? (
          <div className="list-stack compact-list">
            {topIncidents.map((incident) => (
              <div key={incident.incident_id} className="list-row">
                <strong>{incident.title}</strong>
                <span>{incident.status}</span>
              </div>
            ))}
          </div>
        ) : (
          <div className="empty-state">No active incidents.</div>
        )}
      </Panel>

      <Panel title="Runtime markers" compact>
        <div className="metric-list compact-list">
          <div><span>Generation</span><strong>{model.runtimeLeadership.generation_id}</strong></div>
          <div><span>Leader</span><strong>{model.runtimeLeadership.leader_node}</strong></div>
          <div><span>Last restart</span><strong>{formatDateTime(model.runtimeLeadership.last_restart_at)}</strong></div>
          <div><span>Recovery</span><strong>{model.runtimeLeadership.post_restart_recovery_state}</strong></div>
        </div>
      </Panel>
    </aside>
  );
}
