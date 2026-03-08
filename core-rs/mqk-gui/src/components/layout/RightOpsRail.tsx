import type { OperatorAlert, SystemModel } from "../../features/system/types";

interface RightOpsRailProps {
  model: SystemModel;
}

function AlertCard({ alert }: { alert: OperatorAlert }) {
  return (
    <div className={`alert-card tone-${alert.severity}`}>
      <div className="alert-header">
        <span className="alert-domain">{alert.domain}</span>
        <span className="alert-severity">{alert.severity}</span>
      </div>
      <div className="alert-title">{alert.title}</div>
      <div className="alert-message">{alert.message}</div>
    </div>
  );
}

export function RightOpsRail({ model }: RightOpsRailProps) {
  const { status, alerts } = model;

  const posture = [
    { label: "Strategy", value: status.strategy_armed ? "Armed" : "Disarmed", tone: status.strategy_armed ? "warning" : "info" },
    { label: "Execution", value: status.execution_armed ? "Armed" : "Disarmed", tone: status.execution_armed ? "warning" : "info" },
    {
      label: "Live Routing",
      value: status.live_routing_enabled ? "Enabled" : "Disabled",
      tone: status.live_routing_enabled ? "critical" : "info",
    },
    { label: "Kill Switch", value: status.kill_switch_active ? "Active" : "Clear", tone: status.kill_switch_active ? "critical" : "info" },
    { label: "Risk Halt", value: status.risk_halt_active ? "Active" : "Clear", tone: status.risk_halt_active ? "critical" : "info" },
    {
      label: "Integrity Halt",
      value: status.integrity_halt_active ? "Active" : "Clear",
      tone: status.integrity_halt_active ? "critical" : "info",
    },
  ] as const;

  return (
    <aside className="right-rail">
      <section className="panel">
        <div className="panel-header">
          <div>
            <div className="eyebrow">Active Posture</div>
            <h2>Safety rails</h2>
          </div>
        </div>
        <div className="badge-grid">
          {posture.map((item) => (
            <div key={item.label} className={`mini-badge tone-${item.tone}`}>
              <span>{item.label}</span>
              <strong>{item.value}</strong>
            </div>
          ))}
        </div>
      </section>

      <section className="panel">
        <div className="panel-header">
          <div>
            <div className="eyebrow">Alerts</div>
            <h2>{alerts.length} open</h2>
          </div>
        </div>
        <div className="alert-stack">
          {alerts.length > 0 ? alerts.map((alert) => <AlertCard key={alert.id} alert={alert} />) : <div className="empty-state">No active alerts.</div>}
        </div>
      </section>
    </aside>
  );
}
