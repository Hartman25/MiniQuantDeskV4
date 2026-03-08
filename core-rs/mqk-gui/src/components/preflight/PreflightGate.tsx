import type { PreflightStatus } from "../../features/system/types";

interface PreflightGateProps {
  preflight: PreflightStatus;
}

const CHECKS: Array<{ key: keyof PreflightStatus; label: string }> = [
  { key: "daemon_reachable", label: "Daemon reachable" },
  { key: "db_reachable", label: "Database reachable" },
  { key: "broker_config_present", label: "Broker config present" },
  { key: "market_data_config_present", label: "Market data config present" },
  { key: "audit_writer_ready", label: "Audit writer ready" },
  { key: "runtime_idle", label: "Runtime idle" },
  { key: "strategy_disarmed", label: "Strategy disarmed" },
  { key: "execution_disarmed", label: "Execution disarmed" },
  { key: "live_routing_disabled", label: "Live routing disabled" },
];

export function PreflightGate({ preflight }: PreflightGateProps) {
  return (
    <section className="panel preflight-panel">
      <div className="panel-header">
        <div>
          <div className="eyebrow">Startup Safety Model</div>
          <h2>Preflight gate</h2>
        </div>
      </div>
      <div className="checklist-grid">
        {CHECKS.map((check) => {
          const ok = Boolean(preflight[check.key]);
          return (
            <div key={check.key} className={`check-card ${ok ? "is-ok" : "is-blocked"}`}>
              <span className="check-icon">{ok ? "✓" : "!"}</span>
              <div>
                <strong>{check.label}</strong>
                <p>{ok ? "Ready" : "Review required"}</p>
              </div>
            </div>
          );
        })}
      </div>

      <div className="preflight-notes">
        <div>
          <h3>Warnings</h3>
          {preflight.warnings.length > 0 ? (
            <ul>
              {preflight.warnings.map((warning) => (
                <li key={warning}>{warning}</li>
              ))}
            </ul>
          ) : (
            <p>No warnings.</p>
          )}
        </div>
        <div>
          <h3>Blockers</h3>
          {preflight.blockers.length > 0 ? (
            <ul>
              {preflight.blockers.map((blocker) => (
                <li key={blocker}>{blocker}</li>
              ))}
            </ul>
          ) : (
            <p>No blockers.</p>
          )}
        </div>
      </div>
    </section>
  );
}
